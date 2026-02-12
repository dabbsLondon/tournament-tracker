//! Sync orchestrator.
//!
//! Coordinates the ingestion pipeline:
//! 1. Fetch content from sources
//! 2. Run AI agents to extract data
//! 3. Validate with Fact Checker
//! 4. Store in JSONL and Parquet

pub mod convert;
pub mod discovery;
pub mod repartition;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{error, info, warn};
use url::Url;

use crate::agents::backend::AiBackend;
use crate::agents::balance_watcher::{BalanceWatcherAgent, BalanceWatcherInput};
use crate::agents::event_scout::{EventScoutAgent, EventScoutInput};
use crate::agents::list_normalizer::{ListNormalizerAgent, ListNormalizerInput};
use crate::agents::result_harvester::{ResultHarvesterAgent, ResultHarvesterInput};
use crate::agents::Agent;
use crate::fetch::Fetcher;
use crate::models::{ArmyList, EpochMapper};
use crate::storage::jsonl::EntityType;
use crate::storage::{
    read_significant_events, write_significant_events, JsonlWriter, StorageConfig,
};

/// Errors that can occur during sync.
#[derive(Debug, Error)]
pub enum SyncError {
    #[error("Fetch error: {0}")]
    Fetch(#[from] crate::fetch::FetchError),

    #[error("Agent error: {0}")]
    Agent(#[from] crate::agents::AgentError),

    #[error("Storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("No sources configured")]
    NoSources,

    #[error("Sync cancelled")]
    Cancelled,
}

/// Source to sync from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SyncSource {
    /// Goonhammer Competitive Innovations articles
    #[serde(rename = "goonhammer")]
    Goonhammer {
        /// Base URL for articles
        base_url: String,
    },

    /// Warhammer Community (for balance dataslates)
    #[serde(rename = "warhammer-community")]
    WarhammerCommunity {
        /// URL to monitor
        url: String,
    },
}

impl Default for SyncSource {
    fn default() -> Self {
        SyncSource::Goonhammer {
            base_url: "https://www.goonhammer.com/category/columns/40k-competitive-innovations/"
                .to_string(),
        }
    }
}

/// Configuration for sync operations.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// Sources to sync from
    pub sources: Vec<SyncSource>,

    /// Sync interval for periodic syncs
    pub interval: Duration,

    /// Date range to sync (None = latest only)
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,

    /// Dry run mode (fetch and parse but don't store)
    pub dry_run: bool,

    /// Storage configuration
    pub storage: StorageConfig,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            sources: vec![SyncSource::default()],
            interval: Duration::from_secs(6 * 3600), // 6 hours
            date_from: None,
            date_to: None,
            dry_run: false,
            storage: StorageConfig::default(),
        }
    }
}

/// State of a sync operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// When the last sync started
    pub last_sync_started: Option<DateTime<Utc>>,

    /// When the last sync completed
    pub last_sync_completed: Option<DateTime<Utc>>,

    /// Last sync status
    pub last_sync_status: SyncStatus,

    /// Number of events synced in last run
    pub events_synced: u32,

    /// Number of placements synced in last run
    pub placements_synced: u32,

    /// Items sent to review queue
    pub items_for_review: u32,

    /// Errors encountered
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            last_sync_started: None,
            last_sync_completed: None,
            last_sync_status: SyncStatus::Idle,
            events_synced: 0,
            placements_synced: 0,
            items_for_review: 0,
            errors: Vec::new(),
        }
    }
}

/// Result of a sync run.
#[derive(Debug, Clone)]
pub struct SyncResult {
    pub events_synced: u32,
    pub placements_synced: u32,
    pub lists_normalized: u32,
    pub items_for_review: u32,
    pub errors: Vec<String>,
    pub duration: Duration,
}

/// Sync orchestrator.
pub struct SyncOrchestrator {
    config: SyncConfig,
    fetcher: Fetcher,
    backend: Arc<dyn AiBackend>,
    state: Arc<RwLock<SyncState>>,
    cancel_token: Arc<RwLock<bool>>,
    epoch_mapper: EpochMapper,
}

impl SyncOrchestrator {
    /// Create a new sync orchestrator.
    ///
    /// Loads the epoch mapper from significant_events on disk (if any).
    pub fn new(config: SyncConfig, fetcher: Fetcher, backend: Arc<dyn AiBackend>) -> Self {
        // Load epoch mapper from stored significant events (empty = backward-compat)
        let epoch_mapper = match read_significant_events(&config.storage) {
            Ok(events) if !events.is_empty() => {
                info!(
                    "Loaded {} significant events for epoch mapping",
                    events.len()
                );
                EpochMapper::from_significant_events(&events)
            }
            _ => EpochMapper::new(),
        };

        Self {
            config,
            fetcher,
            backend,
            state: Arc::new(RwLock::new(SyncState::default())),
            cancel_token: Arc::new(RwLock::new(false)),
            epoch_mapper,
        }
    }

    /// Get current sync state.
    pub async fn state(&self) -> SyncState {
        self.state.read().await.clone()
    }

    /// Check if sync is currently running.
    pub async fn is_running(&self) -> bool {
        self.state.read().await.last_sync_status == SyncStatus::Running
    }

    /// Request cancellation of current sync.
    pub async fn cancel(&self) {
        *self.cancel_token.write().await = true;
    }

    /// Process a single article URL directly (bypasses discovery).
    ///
    /// Tries WP REST API first (extracts post slug from URL), falls back to direct fetch.
    pub async fn process_single_article(
        &self,
        article_url: &Url,
        article_date: NaiveDate,
        _config: &SyncConfig,
    ) -> Result<(u32, u32, u32), SyncError> {
        // Try to get content via WP REST API (by slug)
        let slug = article_url
            .path()
            .trim_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("");

        if !slug.is_empty() {
            let api_url = format!(
                "https://www.goonhammer.com/wp-json/wp/v2/posts?slug={}",
                slug
            );
            let api_url = Url::parse(&api_url).map_err(|e| {
                SyncError::Fetch(crate::fetch::FetchError::InvalidUrl(e.to_string()))
            })?;

            let fetch_result = self.fetcher.fetch(&api_url).await?;
            let json_text = self.fetcher.read_cached_text(&fetch_result).await?;

            if let Ok(posts) = serde_json::from_str::<Vec<serde_json::Value>>(&json_text) {
                if let Some(post) = posts.first() {
                    if let Some(content) = post
                        .get("content")
                        .and_then(|c| c.get("rendered"))
                        .and_then(|r| r.as_str())
                    {
                        let date = post
                            .get("date")
                            .and_then(|d| d.as_str())
                            .and_then(|s| {
                                chrono::NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d").ok()
                            })
                            .unwrap_or(article_date);

                        info!("Got article content via WP API ({} chars)", content.len());
                        return self
                            .process_goonhammer_article_content(article_url, date, content)
                            .await;
                    }
                }
            }
        }

        // Fallback: direct fetch
        warn!("WP API lookup failed, falling back to direct fetch");
        self.process_goonhammer_article(article_url, article_date)
            .await
    }

    /// Run a single sync operation.
    pub async fn sync_once(&self) -> Result<SyncResult, SyncError> {
        if self.config.sources.is_empty() {
            return Err(SyncError::NoSources);
        }

        // Reset cancel token
        *self.cancel_token.write().await = false;

        // Update state
        {
            let mut state = self.state.write().await;
            state.last_sync_started = Some(Utc::now());
            state.last_sync_status = SyncStatus::Running;
            state.errors.clear();
        }

        let start = std::time::Instant::now();
        info!("Starting sync operation");

        let mut total_events = 0u32;
        let mut total_placements = 0u32;
        let mut total_lists = 0u32;
        let mut total_review = 0u32;
        let mut errors = Vec::new();

        for source in &self.config.sources {
            // Check for cancellation
            if *self.cancel_token.read().await {
                warn!("Sync cancelled");
                return Err(SyncError::Cancelled);
            }

            match self.sync_source(source).await {
                Ok(result) => {
                    total_events += result.events_synced;
                    total_placements += result.placements_synced;
                    total_lists += result.lists_normalized;
                    total_review += result.items_for_review;
                }
                Err(e) => {
                    error!("Error syncing source: {}", e);
                    errors.push(e.to_string());
                }
            }
        }

        let duration = start.elapsed();

        // Update final state
        {
            let mut state = self.state.write().await;
            state.last_sync_completed = Some(Utc::now());
            state.last_sync_status = if errors.is_empty() {
                SyncStatus::Completed
            } else {
                SyncStatus::Failed
            };
            state.events_synced = total_events;
            state.placements_synced = total_placements;
            state.items_for_review = total_review;
            state.errors = errors.clone();
        }

        info!(
            "Sync completed: {} events, {} placements, {} lists in {:?}",
            total_events, total_placements, total_lists, duration
        );

        Ok(SyncResult {
            events_synced: total_events,
            placements_synced: total_placements,
            lists_normalized: total_lists,
            items_for_review: total_review,
            errors,
            duration,
        })
    }

    /// Sync from a specific source.
    async fn sync_source(&self, source: &SyncSource) -> Result<SyncResult, SyncError> {
        let start = std::time::Instant::now();

        match source {
            SyncSource::Goonhammer { base_url } => {
                info!("Syncing from Goonhammer: {}", base_url);

                // 1. Fetch RSS feed for article discovery (HTML pages are JS-rendered)
                let base_with_slash = if base_url.ends_with('/') {
                    base_url.clone()
                } else {
                    format!("{}/", base_url)
                };
                let rss_url = format!("{}feed/", base_with_slash);
                let rss_url = Url::parse(&rss_url).map_err(|e| {
                    SyncError::Fetch(crate::fetch::FetchError::InvalidUrl(e.to_string()))
                })?;

                let fetch_result = self.fetcher.fetch(&rss_url).await?;
                let rss_xml = self.fetcher.read_cached_text(&fetch_result).await?;

                // 2. Discover articles from RSS
                let articles = discovery::discover_from_rss(&rss_xml);
                info!("Discovered {} articles from RSS", articles.len());

                // 3. Filter by date range
                let articles = discovery::filter_by_date_range(
                    articles,
                    self.config.date_from,
                    self.config.date_to,
                );
                info!("{} articles after date filtering", articles.len());

                // 4. Process each article
                let mut total_events = 0u32;
                let mut total_placements = 0u32;
                let mut total_lists = 0u32;
                let mut errors = Vec::new();

                for article in &articles {
                    if *self.cancel_token.read().await {
                        break;
                    }

                    info!("Processing article: {}", article.title);

                    // Fetch content via WP REST API if we have a post ID
                    let content_result = if let Some(post_id) = article.wp_post_id {
                        self.fetch_wp_article_content(post_id).await
                    } else {
                        // Fallback: fetch the page directly
                        let fetch_result = self.fetcher.fetch(&article.url).await?;
                        let html = self.fetcher.read_cached_text(&fetch_result).await?;
                        Ok(html)
                    };

                    let article_content = match content_result {
                        Ok(content) => content,
                        Err(e) => {
                            let err = format!("Error fetching {}: {}", article.url, e);
                            warn!("{}", err);
                            errors.push(err);
                            continue;
                        }
                    };

                    let article_date = article.date.unwrap_or_else(|| Utc::now().date_naive());

                    match self
                        .process_goonhammer_article_content(
                            &article.url,
                            article_date,
                            &article_content,
                        )
                        .await
                    {
                        Ok((events, placements, lists)) => {
                            total_events += events;
                            total_placements += placements;
                            total_lists += lists;
                        }
                        Err(e) => {
                            let err = format!("Error processing {}: {}", article.url, e);
                            warn!("{}", err);
                            errors.push(err);
                        }
                    }
                }

                Ok(SyncResult {
                    events_synced: total_events,
                    placements_synced: total_placements,
                    lists_normalized: total_lists,
                    items_for_review: 0,
                    errors,
                    duration: start.elapsed(),
                })
            }
            SyncSource::WarhammerCommunity { url } => {
                info!("Syncing balance updates from: {}", url);

                let page_url = Url::parse(url).map_err(|e| {
                    SyncError::Fetch(crate::fetch::FetchError::InvalidUrl(e.to_string()))
                })?;

                // 1. Fetch page
                let fetch_result = self.fetcher.fetch(&page_url).await?;
                let html = self.fetcher.read_cached_text(&fetch_result).await?;

                // 2. Run BalanceWatcherAgent
                let watcher = BalanceWatcherAgent::new(self.backend.clone());
                let input = BalanceWatcherInput {
                    html_content: html,
                    source_url: url.clone(),
                    known_event_ids: vec![],
                };

                let output = watcher.execute(input).await?;
                let event_count = output.events.len() as u32;

                // 3. Store SignificantEvent entities to global file
                if !self.config.dry_run {
                    let mut existing =
                        read_significant_events(&self.config.storage).unwrap_or_default();
                    let existing_ids: std::collections::HashSet<String> =
                        existing.iter().map(|e| e.id.as_str().to_string()).collect();
                    for event_output in &output.events {
                        if !existing_ids.contains(event_output.data.id.as_str()) {
                            existing.push(event_output.data.clone());
                        }
                    }
                    write_significant_events(&self.config.storage, &mut existing)
                        .map_err(SyncError::Storage)?;
                }

                info!("Balance watcher found {} events", event_count);

                Ok(SyncResult {
                    events_synced: event_count,
                    placements_synced: 0,
                    lists_normalized: 0,
                    items_for_review: 0,
                    errors: vec![],
                    duration: start.elapsed(),
                })
            }
        }
    }

    /// Fetch article content from WordPress REST API.
    ///
    /// Returns the rendered HTML content from the post's `content.rendered` field.
    async fn fetch_wp_article_content(&self, post_id: u64) -> Result<String, SyncError> {
        let api_url = Url::parse(&format!(
            "https://www.goonhammer.com/wp-json/wp/v2/posts/{}",
            post_id
        ))
        .map_err(|e| SyncError::Fetch(crate::fetch::FetchError::InvalidUrl(e.to_string())))?;

        let fetch_result = self.fetcher.fetch(&api_url).await?;
        let json_text = self.fetcher.read_cached_text(&fetch_result).await?;

        // Parse the WP REST API JSON response
        let wp_post: serde_json::Value = serde_json::from_str(&json_text).map_err(|e| {
            SyncError::Fetch(crate::fetch::FetchError::InvalidUrl(format!(
                "Invalid WP API response: {}",
                e
            )))
        })?;

        let content = wp_post
            .get("content")
            .and_then(|c| c.get("rendered"))
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        info!("Fetched WP post {} ({} chars HTML)", post_id, content.len());

        Ok(content)
    }

    /// Process a single Goonhammer article from its URL by fetching content.
    ///
    /// Returns (events_count, placements_count, lists_count).
    async fn process_goonhammer_article(
        &self,
        article_url: &Url,
        article_date: NaiveDate,
    ) -> Result<(u32, u32, u32), SyncError> {
        // Fetch article HTML
        let fetch_result = self.fetcher.fetch(article_url).await?;
        let html = self.fetcher.read_cached_text(&fetch_result).await?;

        self.process_goonhammer_article_content(article_url, article_date, &html)
            .await
    }

    /// Process a Goonhammer article given its HTML content.
    ///
    /// Strips HTML to text before sending to AI agents.
    /// Deduplicates against existing data before storing.
    /// Returns (events_count, placements_count, lists_count).
    async fn process_goonhammer_article_content(
        &self,
        article_url: &Url,
        article_date: NaiveDate,
        html_content: &str,
    ) -> Result<(u32, u32, u32), SyncError> {
        // Strip HTML to clean text for the AI (saves ~50% tokens)
        let article_text = discovery::extract_text_from_html(html_content);
        info!(
            "Extracted {} chars text from {} chars HTML",
            article_text.len(),
            html_content.len()
        );

        // Run EventScoutAgent
        let event_scout = EventScoutAgent::new(self.backend.clone());
        let scout_input = EventScoutInput {
            article_html: article_text.clone(),
            article_url: article_url.to_string(),
            article_date,
        };

        let scout_output = event_scout.execute(scout_input).await?;
        info!("Event Scout found {} events", scout_output.events.len());

        let mut total_events = 0u32;
        let mut total_placements = 0u32;
        let mut total_lists = 0u32;

        for event_stub in &scout_output.events {
            // Determine epoch from event date
            let event_date = event_stub.data.date.unwrap_or(article_date);
            let epoch_id = if self.epoch_mapper.all_epochs().is_empty() {
                None // No epochs configured — use default "current"
            } else {
                Some(self.epoch_mapper.get_epoch_id_for_date(event_date))
            };
            let epoch_str = epoch_id
                .as_ref()
                .map(|e| e.as_str().to_string())
                .unwrap_or_else(|| "current".to_string());

            // 3. Convert to Event model and store
            let event = convert::event_from_stub(
                event_stub,
                article_url.as_str(),
                article_date,
                "goonhammer",
                epoch_id.clone(),
            );

            if !self.config.dry_run {
                // Dedup: load existing event IDs and skip if already present
                let existing_events: Vec<crate::models::Event> =
                    crate::storage::JsonlReader::for_entity(
                        &self.config.storage,
                        EntityType::Event,
                        &epoch_str,
                    )
                    .read_all()
                    .unwrap_or_default();
                let existing_event_ids: std::collections::HashSet<String> = existing_events
                    .iter()
                    .map(|e| e.id.as_str().to_string())
                    .collect();

                if existing_event_ids.contains(event.id.as_str()) {
                    info!("  Skipping duplicate event: {} ({})", event.name, event.id);
                    continue;
                }

                let event_writer =
                    JsonlWriter::for_entity(&self.config.storage, EntityType::Event, &epoch_str);
                event_writer.append(&event).map_err(SyncError::Storage)?;
            }
            total_events += 1;

            info!("  Event: {} ({:?} players)", event.name, event.player_count);

            // 4. Run ResultHarvesterAgent for each event
            let harvester = ResultHarvesterAgent::new(self.backend.clone());
            let harvest_input = ResultHarvesterInput {
                article_html: article_text.clone(),
                event_stub: event_stub.data.clone(),
            };

            match harvester.execute(harvest_input).await {
                Ok(harvest_output) => {
                    let list_count = harvest_output.raw_lists.len() as u32;
                    total_lists += list_count;

                    // 5. Convert placements and store (with dedup)
                    let existing_placement_ids: std::collections::HashSet<String> =
                        if !self.config.dry_run {
                            crate::storage::JsonlReader::<crate::models::Placement>::for_entity(
                                &self.config.storage,
                                EntityType::Placement,
                                &epoch_str,
                            )
                            .read_all()
                            .unwrap_or_default()
                            .iter()
                            .map(|p| p.id.as_str().to_string())
                            .collect()
                        } else {
                            std::collections::HashSet::new()
                        };

                    for placement_stub in &harvest_output.placements {
                        let placement = convert::placement_from_stub(
                            placement_stub,
                            event.id.clone(),
                            epoch_id.clone(),
                        );

                        if !self.config.dry_run {
                            if existing_placement_ids.contains(placement.id.as_str()) {
                                info!(
                                    "    Skipping duplicate placement: #{} {}",
                                    placement.rank, placement.player_name
                                );
                                continue;
                            }

                            let placement_writer = JsonlWriter::for_entity(
                                &self.config.storage,
                                EntityType::Placement,
                                &epoch_str,
                            );
                            placement_writer
                                .append(&placement)
                                .map_err(SyncError::Storage)?;
                        }
                        total_placements += 1;
                    }

                    // 6. Normalize and store army lists (with dedup)
                    let existing_list_ids: std::collections::HashSet<String> =
                        if !self.config.dry_run {
                            crate::storage::JsonlReader::<ArmyList>::for_entity(
                                &self.config.storage,
                                EntityType::ArmyList,
                                &epoch_str,
                            )
                            .read_all()
                            .unwrap_or_default()
                            .iter()
                            .map(|l| l.id.as_str().to_string())
                            .collect()
                        } else {
                            std::collections::HashSet::new()
                        };

                    let normalizer = ListNormalizerAgent::new(self.backend.clone());
                    for (list_idx, raw_list) in harvest_output.raw_lists.iter().enumerate() {
                        // Find the matching placement to get the faction
                        let faction = harvest_output
                            .placements
                            .iter()
                            .find(|p| p.data.rank == raw_list.placement_rank)
                            .map(|p| p.data.faction.clone())
                            .unwrap_or_default();

                        // Try to normalize the list with AI
                        let norm_input = ListNormalizerInput {
                            raw_text: raw_list.text.clone(),
                            faction_hint: if faction.is_empty() {
                                None
                            } else {
                                Some(faction.clone())
                            },
                            player_name: raw_list.player_name.clone(),
                        };

                        let (
                            norm_faction,
                            norm_detachment,
                            norm_subfaction,
                            norm_points,
                            norm_units,
                            norm_confidence,
                        ) = match normalizer.execute(norm_input).await {
                            Ok(output) => {
                                let d = output.list.data;
                                info!(
                                    "    Normalized: {} - {} ({} units, {}pts)",
                                    d.faction,
                                    d.detachment.as_deref().unwrap_or("(none)"),
                                    d.units.len(),
                                    d.total_points,
                                );
                                (
                                    d.faction,
                                    d.detachment,
                                    d.subfaction,
                                    d.total_points,
                                    d.units,
                                    output.list.confidence,
                                )
                            }
                            Err(e) => {
                                warn!(
                                    "    List normalization failed for {}: {}",
                                    raw_list.player_name, e
                                );
                                (
                                    faction,
                                    None,
                                    None,
                                    0,
                                    Vec::new(),
                                    crate::models::Confidence::Low,
                                )
                            }
                        };

                        let mut army_list = ArmyList::new(
                            norm_faction,
                            norm_points,
                            norm_units,
                            raw_list.text.clone(),
                        )
                        .with_player_name(if raw_list.player_name.trim().is_empty() {
                            format!("Unknown Player {}", list_idx + 1)
                        } else {
                            raw_list.player_name.clone()
                        })
                        .with_event_date(event_date)
                        .with_source_url(article_url.to_string())
                        .with_confidence(norm_confidence);

                        if let Some(det) = norm_detachment {
                            army_list = army_list.with_detachment(det);
                        }
                        if let Some(sub) = norm_subfaction {
                            army_list = army_list.with_subfaction(sub);
                        }

                        if !self.config.dry_run {
                            if existing_list_ids.contains(army_list.id.as_str()) {
                                info!("    Skipping duplicate army list: {}", army_list.id);
                                continue;
                            }

                            let list_writer = JsonlWriter::for_entity(
                                &self.config.storage,
                                EntityType::ArmyList,
                                &epoch_str,
                            );
                            list_writer.append(&army_list).map_err(SyncError::Storage)?;
                        }

                        info!(
                            "    Stored army list for #{} {} ({} chars, {} units)",
                            raw_list.placement_rank,
                            raw_list.player_name,
                            raw_list.text.len(),
                            army_list.units.len()
                        );
                    }

                    info!(
                        "    {} placements, {} lists",
                        harvest_output.placements.len(),
                        list_count
                    );
                }
                Err(e) => {
                    warn!("Result Harvester error for {}: {}", event.name, e);
                }
            }
        }

        Ok((total_events, total_placements, total_lists))
    }

    /// Run periodic sync in the background.
    pub async fn run_periodic(self: Arc<Self>) {
        let mut ticker = interval(self.config.interval);

        info!("Starting periodic sync every {:?}", self.config.interval);

        loop {
            ticker.tick().await;

            if *self.cancel_token.read().await {
                info!("Periodic sync stopped");
                break;
            }

            match self.sync_once().await {
                Ok(result) => {
                    info!(
                        "Periodic sync completed: {} events, {} placements",
                        result.events_synced, result.placements_synced
                    );
                }
                Err(SyncError::Cancelled) => {
                    info!("Periodic sync cancelled");
                    break;
                }
                Err(e) => {
                    error!("Periodic sync failed: {}", e);
                }
            }
        }
    }

    /// Trigger a manual sync (for API endpoint).
    pub async fn trigger(&self) -> Result<SyncResult, SyncError> {
        if self.is_running().await {
            warn!("Sync already in progress");
            // Return current state instead of error
            let state = self.state().await;
            return Ok(SyncResult {
                events_synced: state.events_synced,
                placements_synced: state.placements_synced,
                lists_normalized: 0,
                items_for_review: state.items_for_review,
                errors: state.errors,
                duration: Duration::ZERO,
            });
        }

        self.sync_once().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::backend::MockBackend;
    use crate::fetch::FetcherConfig;
    use tempfile::TempDir;

    fn test_config(temp_dir: &TempDir) -> SyncConfig {
        SyncConfig {
            sources: vec![SyncSource::default()],
            interval: Duration::from_secs(60),
            date_from: None,
            date_to: None,
            dry_run: true,
            storage: StorageConfig::new(temp_dir.path().to_path_buf()),
        }
    }

    #[tokio::test]
    async fn test_sync_state_default() {
        let state = SyncState::default();
        assert_eq!(state.last_sync_status, SyncStatus::Idle);
        assert!(state.last_sync_started.is_none());
    }

    #[tokio::test]
    async fn test_sync_orchestrator_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let fetcher = Fetcher::new(FetcherConfig {
            cache_dir: temp_dir.path().join("cache"),
            ..Default::default()
        })
        .unwrap();
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));

        let orchestrator = SyncOrchestrator::new(config, fetcher, backend);

        let state = orchestrator.state().await;
        assert_eq!(state.last_sync_status, SyncStatus::Idle);
    }

    #[tokio::test]
    async fn test_sync_no_sources() {
        let temp_dir = TempDir::new().unwrap();
        let mut config = test_config(&temp_dir);
        config.sources = vec![];

        let fetcher = Fetcher::new(FetcherConfig {
            cache_dir: temp_dir.path().join("cache"),
            ..Default::default()
        })
        .unwrap();
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));

        let orchestrator = SyncOrchestrator::new(config, fetcher, backend);

        let result = orchestrator.sync_once().await;
        assert!(matches!(result, Err(SyncError::NoSources)));
    }

    #[tokio::test]
    async fn test_sync_source_serialization() {
        let source = SyncSource::Goonhammer {
            base_url: "https://test.com".to_string(),
        };

        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("goonhammer"));

        let parsed: SyncSource = serde_json::from_str(&json).unwrap();
        match parsed {
            SyncSource::Goonhammer { base_url } => {
                assert_eq!(base_url, "https://test.com");
            }
            _ => panic!("Expected Goonhammer source"),
        }
    }

    #[test]
    fn test_sync_status_serialization() {
        let status = SyncStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let parsed: SyncStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SyncStatus::Running);
    }
}
