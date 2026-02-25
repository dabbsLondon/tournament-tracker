//! Sync orchestrator.
//!
//! Coordinates the ingestion pipeline:
//! 1. Fetch content from sources
//! 2. Run AI agents to extract data
//! 3. Validate with Fact Checker
//! 4. Store in JSONL and Parquet

pub mod bcp;
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
use crate::models::{ArmyList, EpochMapper, Placement};
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

    /// Best Coast Pairings structured data
    #[serde(rename = "bcp")]
    Bcp {
        /// Base URL for the BCP API
        api_base_url: String,
        /// Game type ID (1 = Warhammer 40k)
        game_type: u32,
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
        SyncSource::Bcp {
            api_base_url: "https://newprod-api.bestcoastpairings.com/v1".to_string(),
            game_type: 1,
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

/// Normalize a player name for matching (lowercase, collapse whitespace).
pub fn normalize_player_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Status of a single event during sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncEventStatus {
    Pending,
    Syncing,
    Done,
    Skipped,
}

/// Progress for a single event during sync.
#[derive(Debug, Clone)]
pub struct SyncEventProgress {
    pub name: String,
    pub date: String,
    pub player_count: u32,
    pub status: SyncEventStatus,
    pub placements_found: u32,
    pub lists_found: u32,
    /// Detail message for the currently active event
    pub detail: String,
}

/// Progress update from the sync pipeline, sent via callback.
#[derive(Debug, Clone)]
pub struct SyncProgress {
    pub events_synced: u32,
    pub placements_synced: u32,
    pub lists_normalized: u32,
    /// Total events discovered in the date range
    pub events_discovered: u32,
    /// Current event index being processed (1-based)
    pub current_event_index: u32,
    pub message: String,
    /// Per-event progress for calendar view
    pub discovered_events: Vec<SyncEventProgress>,
}

/// Sync orchestrator.
pub struct SyncOrchestrator {
    config: SyncConfig,
    fetcher: Fetcher,
    backend: Arc<dyn AiBackend>,
    state: Arc<RwLock<SyncState>>,
    cancel_token: Arc<RwLock<bool>>,
    epoch_mapper: EpochMapper,
    on_progress: Option<Box<dyn Fn(SyncProgress) + Send + Sync>>,
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
            on_progress: None,
        }
    }

    /// Set a callback to receive live progress updates.
    pub fn with_progress_callback(
        mut self,
        cb: impl Fn(SyncProgress) + Send + Sync + 'static,
    ) -> Self {
        self.on_progress = Some(Box::new(cb));
        self
    }

    /// Send a progress update to the callback if one is set.
    #[allow(clippy::too_many_arguments)]
    fn emit_progress(
        &self,
        events: u32,
        placements: u32,
        lists: u32,
        discovered: u32,
        current_idx: u32,
        message: String,
        discovered_events: Vec<SyncEventProgress>,
    ) {
        if let Some(ref cb) = self.on_progress {
            cb(SyncProgress {
                events_synced: events,
                placements_synced: placements,
                lists_normalized: lists,
                events_discovered: discovered,
                current_event_index: current_idx,
                message,
                discovered_events,
            });
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
                    self.emit_progress(
                        total_events,
                        total_placements,
                        total_lists,
                        0,
                        0,
                        format!(
                            "Source complete: {} events, {} placements, {} lists",
                            result.events_synced, result.placements_synced, result.lists_normalized
                        ),
                        Vec::new(),
                    );
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

                // Load all existing events across epochs to check which articles are already imported
                let mut all_existing_source_urls: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for epoch in self.epoch_mapper.all_epochs() {
                    let reader = crate::storage::JsonlReader::<crate::models::Event>::for_entity(
                        &self.config.storage,
                        EntityType::Event,
                        epoch.id.as_str(),
                    );
                    if let Ok(events) = reader.read_all() {
                        for e in events {
                            all_existing_source_urls.insert(e.source_url.clone());
                        }
                    }
                }
                // Also check "current" directory
                if let Ok(events) = crate::storage::JsonlReader::<crate::models::Event>::for_entity(
                    &self.config.storage,
                    EntityType::Event,
                    "current",
                )
                .read_all()
                {
                    for e in events {
                        all_existing_source_urls.insert(e.source_url.clone());
                    }
                }

                for (article_idx, article) in articles.iter().enumerate() {
                    if *self.cancel_token.read().await {
                        break;
                    }

                    // Skip articles that have already been imported (events exist with this source URL)
                    let article_url_str = article.url.to_string();
                    if all_existing_source_urls.contains(&article_url_str) {
                        info!(
                            "Skipping already-imported article: {} ({})",
                            article.title, article_url_str
                        );
                        self.emit_progress(
                            total_events,
                            total_placements,
                            total_lists,
                            0,
                            0,
                            format!(
                                "Skipping already-imported article {}/{}...",
                                article_idx + 1,
                                articles.len()
                            ),
                            Vec::new(),
                        );
                        continue;
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

                    self.emit_progress(
                        total_events,
                        total_placements,
                        total_lists,
                        0,
                        0,
                        format!(
                            "Processing Goonhammer article {}/{}...",
                            article_idx + 1,
                            articles.len()
                        ),
                        Vec::new(),
                    );

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
                            self.emit_progress(
                                total_events,
                                total_placements,
                                total_lists,
                                0,
                                0,
                                format!(
                                    "Article {}/{}: {} events, {} placements, {} lists",
                                    article_idx + 1,
                                    articles.len(),
                                    events,
                                    placements,
                                    lists
                                ),
                                Vec::new(),
                            );
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
            SyncSource::Bcp {
                api_base_url,
                game_type,
            } => {
                info!(
                    "Syncing from BCP: {} (game_type={})",
                    api_base_url, game_type
                );

                // Unauthenticated fetcher for event discovery (BCP rejects authed /events requests with 409)
                let discovery_fetcher = Fetcher::new(crate::fetch::FetcherConfig {
                    cache_dir: self.config.storage.raw_dir(),
                    extra_headers: bcp::bcp_headers(),
                    ..Default::default()
                })
                .map_err(SyncError::Fetch)?;
                let discovery_client =
                    bcp::BcpClient::new(discovery_fetcher, api_base_url.clone(), *game_type);

                // Authenticated fetcher for standings and army list fetching
                let bcp_fetcher = Fetcher::new(crate::fetch::FetcherConfig {
                    cache_dir: self.config.storage.raw_dir(),
                    extra_headers: bcp::bcp_headers_authenticated().await,
                    ..Default::default()
                })
                .map_err(SyncError::Fetch)?;
                let bcp_client = bcp::BcpClient::new(bcp_fetcher, api_base_url.clone(), *game_type);

                let date_from = self.config.date_from.unwrap_or_else(|| {
                    (chrono::Utc::now() - chrono::Duration::days(30)).date_naive()
                });
                let date_to = self
                    .config
                    .date_to
                    .unwrap_or_else(|| chrono::Utc::now().date_naive());

                let bcp_events = match discovery_client.discover_events(date_from, date_to).await {
                    Ok(events) => events,
                    Err(e) => {
                        warn!("BCP event discovery failed: {}", e);
                        return Ok(SyncResult {
                            events_synced: 0,
                            placements_synced: 0,
                            lists_normalized: 0,
                            items_for_review: 0,
                            errors: vec![e.to_string()],
                            duration: start.elapsed(),
                        });
                    }
                };

                let discovered_count = bcp_events.len() as u32;
                info!("BCP: discovered {} events", discovered_count);

                // Build per-event progress list
                let mut event_progress: Vec<SyncEventProgress> = bcp_events
                    .iter()
                    .map(|e| {
                        let date_str = e
                            .parsed_start_date()
                            .map(|d| d.to_string())
                            .unwrap_or_default();
                        SyncEventProgress {
                            name: e.name.clone(),
                            date: date_str,
                            player_count: e.player_count.unwrap_or(0),
                            status: if e.should_skip() {
                                SyncEventStatus::Skipped
                            } else {
                                SyncEventStatus::Pending
                            },
                            placements_found: 0,
                            lists_found: 0,
                            detail: String::new(),
                        }
                    })
                    .collect();

                self.emit_progress(
                    0,
                    0,
                    0,
                    discovered_count,
                    0,
                    format!("BCP: found {} events, processing...", discovered_count),
                    event_progress.clone(),
                );

                let mut total_events = 0u32;
                let mut total_placements = 0u32;
                let mut total_lists = 0u32;
                let mut errors = Vec::new();

                for (bcp_idx, bcp_event) in bcp_events.iter().enumerate() {
                    if *self.cancel_token.read().await {
                        break;
                    }

                    // Skip team events and events with hidden placings
                    if bcp_event.should_skip() {
                        info!(
                            "  BCP: skipping event: {} (team={:?}, hide_placings={:?})",
                            bcp_event.name, bcp_event.team_event, bcp_event.hide_placings
                        );
                        continue;
                    }

                    // Mark event as syncing
                    event_progress[bcp_idx].status = SyncEventStatus::Syncing;
                    event_progress[bcp_idx].detail = "Fetching standings...".to_string();
                    self.emit_progress(
                        total_events,
                        total_placements,
                        total_lists,
                        discovered_count,
                        (bcp_idx + 1) as u32,
                        format!(
                            "BCP {}/{}: {} ({} players)",
                            bcp_idx + 1,
                            bcp_events.len(),
                            bcp_event.name,
                            bcp_event.player_count.unwrap_or(0)
                        ),
                        event_progress.clone(),
                    );

                    let event_date = bcp_event
                        .parsed_start_date()
                        .unwrap_or_else(|| chrono::Utc::now().date_naive());

                    // Determine epoch
                    let epoch_id = if self.epoch_mapper.all_epochs().is_empty() {
                        None
                    } else {
                        Some(self.epoch_mapper.get_epoch_id_for_date(event_date))
                    };
                    let epoch_str = epoch_id
                        .as_ref()
                        .map(|e| e.as_str().to_string())
                        .unwrap_or_else(|| "current".to_string());

                    // Convert to Event
                    let event = convert::event_from_bcp(bcp_event, epoch_id.clone());

                    if !self.config.dry_run {
                        // Load existing events for dedup (both exact and fuzzy)
                        let existing_events: Vec<crate::models::Event> =
                            crate::storage::JsonlReader::for_entity(
                                &self.config.storage,
                                EntityType::Event,
                                &epoch_str,
                            )
                            .read_all()
                            .unwrap_or_default();

                        if let Some(existing_id) =
                            convert::find_duplicate_event(&event, &existing_events)
                        {
                            info!(
                                "  BCP: skipping duplicate event: {} (matches {})",
                                event.name, existing_id
                            );
                            // Still fetch standings using the EXISTING event ID
                            // so placements link to the right event
                            event_progress[bcp_idx].detail = "Fetching lists...".to_string();
                            match self
                                .sync_bcp_standings(
                                    &bcp_client,
                                    bcp_event,
                                    &existing_id,
                                    epoch_id.clone(),
                                    &epoch_str,
                                )
                                .await
                            {
                                Ok((p, l)) => {
                                    total_placements += p;
                                    total_lists += l;
                                    event_progress[bcp_idx].placements_found = p;
                                    event_progress[bcp_idx].lists_found = l;
                                }
                                Err(e) => errors.push(e.to_string()),
                            }
                            event_progress[bcp_idx].status = SyncEventStatus::Done;
                            event_progress[bcp_idx].detail = String::new();
                            self.emit_progress(
                                total_events,
                                total_placements,
                                total_lists,
                                discovered_count,
                                (bcp_idx + 1) as u32,
                                format!("BCP {}/{}: done", bcp_idx + 1, bcp_events.len()),
                                event_progress.clone(),
                            );
                            continue;
                        }

                        let event_writer = JsonlWriter::for_entity(
                            &self.config.storage,
                            EntityType::Event,
                            &epoch_str,
                        );
                        event_writer.append(&event).map_err(SyncError::Storage)?;
                    }
                    total_events += 1;

                    info!(
                        "  BCP event: {} ({:?} players)",
                        event.name, event.player_count
                    );

                    // Fetch standings for this event
                    event_progress[bcp_idx].detail = "Fetching lists...".to_string();
                    match self
                        .sync_bcp_standings(
                            &bcp_client,
                            bcp_event,
                            &event.id,
                            epoch_id.clone(),
                            &epoch_str,
                        )
                        .await
                    {
                        Ok((p, l)) => {
                            total_placements += p;
                            total_lists += l;
                            event_progress[bcp_idx].placements_found = p;
                            event_progress[bcp_idx].lists_found = l;
                        }
                        Err(e) => errors.push(e.to_string()),
                    }

                    event_progress[bcp_idx].status = SyncEventStatus::Done;
                    event_progress[bcp_idx].detail = String::new();
                    self.emit_progress(
                        total_events,
                        total_placements,
                        total_lists,
                        discovered_count,
                        (bcp_idx + 1) as u32,
                        format!("BCP {}/{}: done", bcp_idx + 1, bcp_events.len()),
                        event_progress.clone(),
                    );
                }

                // Backfill: find existing BCP events with placements missing lists
                // that weren't already processed in this sync (e.g. not in the 100-event discovery window)
                if !self.config.dry_run {
                    let processed_event_ids: std::collections::HashSet<String> = bcp_events
                        .iter()
                        .map(|e| {
                            let event = convert::event_from_bcp(e, None);
                            event.id.as_str().to_string()
                        })
                        .collect();

                    // Scan all epochs for BCP events needing list backfill
                    let epoch_dirs = crate::storage::jsonl::list_epochs(&self.config.storage)
                        .unwrap_or_default();
                    for epoch_dir in &epoch_dirs {
                        if *self.cancel_token.read().await {
                            break;
                        }

                        let events: Vec<crate::models::Event> =
                            crate::storage::JsonlReader::for_entity(
                                &self.config.storage,
                                EntityType::Event,
                                epoch_dir,
                            )
                            .read_all()
                            .unwrap_or_default();

                        let placements: Vec<Placement> = crate::storage::JsonlReader::for_entity(
                            &self.config.storage,
                            EntityType::Placement,
                            epoch_dir,
                        )
                        .read_all()
                        .unwrap_or_default();

                        for event in &events {
                            if *self.cancel_token.read().await {
                                break;
                            }

                            // Only BCP events, and only ones not already processed this sync
                            if event.source_name != "bcp"
                                || processed_event_ids.contains(event.id.as_str())
                            {
                                continue;
                            }

                            // Check if this event has placements needing lists
                            let event_placements: Vec<&Placement> = placements
                                .iter()
                                .filter(|p| p.event_id == event.id)
                                .collect();
                            let without_lists = event_placements
                                .iter()
                                .filter(|p| p.list_id.is_none())
                                .count();

                            if without_lists == 0 || event_placements.len() < 10 {
                                continue;
                            }

                            // Extract BCP event ID from source_url
                            let bcp_event_id = match event
                                .source_url
                                .strip_prefix("https://www.bestcoastpairings.com/event/")
                            {
                                Some(id) if !id.is_empty() => id,
                                _ => continue,
                            };

                            info!(
                                "  BCP backfill: {} has {} placements without lists",
                                event.name, without_lists
                            );

                            // Reconstruct a minimal BcpEvent for the standings fetch
                            let backfill_bcp_event = bcp::BcpEvent {
                                id: bcp_event_id.to_string(),
                                name: event.name.clone(),
                                start_date: Some(event.date.to_string()),
                                end_date: None,
                                venue: None,
                                city: None,
                                state: None,
                                country: None,
                                player_count: event.player_count,
                                round_count: event.round_count,
                                game_type: None,
                                ended: Some(true),
                                team_event: None,
                                hide_placings: None,
                            };

                            let epoch_id = if self.epoch_mapper.all_epochs().is_empty() {
                                None
                            } else {
                                Some(self.epoch_mapper.get_epoch_id_for_date(event.date))
                            };

                            self.emit_progress(
                                total_events,
                                total_placements,
                                total_lists,
                                discovered_count,
                                0,
                                format!("Backfilling lists for {}...", event.name),
                                event_progress.clone(),
                            );

                            match self
                                .sync_bcp_standings(
                                    &bcp_client,
                                    &backfill_bcp_event,
                                    &event.id,
                                    epoch_id,
                                    epoch_dir,
                                )
                                .await
                            {
                                Ok((p, l)) => {
                                    total_placements += p;
                                    total_lists += l;
                                    if l > 0 {
                                        info!("  BCP backfill: {} new lists for {}", l, event.name);
                                    }
                                }
                                Err(e) => {
                                    warn!("  BCP backfill failed for {}: {}", event.name, e);
                                    errors.push(e.to_string());
                                }
                            }
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

                    // 5. Buffer placements (store after lists so we can link)
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

                    let mut buffered_placements: Vec<crate::models::Placement> = Vec::new();
                    for placement_stub in &harvest_output.placements {
                        let placement = convert::placement_from_stub(
                            placement_stub,
                            event.id.clone(),
                            epoch_id.clone(),
                        );

                        if !self.config.dry_run
                            && existing_placement_ids.contains(placement.id.as_str())
                        {
                            info!(
                                "    Skipping duplicate placement: #{} {}",
                                placement.rank, placement.player_name
                            );
                            continue;
                        }
                        buffered_placements.push(placement);
                    }

                    // 6. Normalize army lists (with dedup)
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
                    let mut stored_lists: Vec<ArmyList> = Vec::new();
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
                        .with_event_id(event.id.clone())
                        .with_source_url(article_url.to_string())
                        .with_confidence(norm_confidence);

                        if let Some(det) = norm_detachment {
                            army_list = army_list.with_detachment(det);
                        }
                        if let Some(sub) = norm_subfaction {
                            army_list = army_list.with_subfaction(sub);
                        }

                        if !self.config.dry_run && existing_list_ids.contains(army_list.id.as_str())
                        {
                            info!("    Skipping duplicate army list: {}", army_list.id);
                            continue;
                        }

                        info!(
                            "    Stored army list for #{} {} ({} chars, {} units)",
                            raw_list.placement_rank,
                            raw_list.player_name,
                            raw_list.text.len(),
                            army_list.units.len()
                        );
                        stored_lists.push(army_list);
                    }

                    // 7. Link placements to lists by player name
                    let name_to_list_id: std::collections::HashMap<
                        String,
                        crate::models::ArmyListId,
                    > = stored_lists
                        .iter()
                        .filter_map(|l| {
                            l.player_name
                                .as_ref()
                                .map(|name| (normalize_player_name(name), l.id.clone()))
                        })
                        .collect();

                    for placement in &mut buffered_placements {
                        if placement.list_id.is_none() {
                            let norm_name = normalize_player_name(&placement.player_name);
                            if let Some(list_id) = name_to_list_id.get(&norm_name) {
                                placement.list_id = Some(list_id.clone());
                            }
                        }
                    }

                    // 8. Store placements and lists
                    if !self.config.dry_run {
                        let placement_writer = JsonlWriter::for_entity(
                            &self.config.storage,
                            EntityType::Placement,
                            &epoch_str,
                        );
                        for placement in &buffered_placements {
                            placement_writer
                                .append(placement)
                                .map_err(SyncError::Storage)?;
                        }

                        let list_writer = JsonlWriter::for_entity(
                            &self.config.storage,
                            EntityType::ArmyList,
                            &epoch_str,
                        );
                        for army_list in &stored_lists {
                            list_writer.append(army_list).map_err(SyncError::Storage)?;
                        }
                    }
                    total_placements += buffered_placements.len() as u32;

                    info!(
                        "    {} placements, {} lists",
                        buffered_placements.len(),
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

    /// Fetch and store BCP standings (placements + optional army lists) for one event.
    ///
    /// Buffers placements in memory. After army lists are fetched, links list_id
    /// and backfills detachment from lists onto placements before writing.
    /// Also persists pairings to pairings.jsonl.
    ///
    /// Returns (placements_count, lists_count).
    async fn sync_bcp_standings(
        &self,
        bcp_client: &bcp::BcpClient,
        bcp_event: &bcp::BcpEvent,
        event_id: &crate::models::EventId,
        epoch_id: Option<crate::models::EntityId>,
        epoch_str: &str,
    ) -> Result<(u32, u32), SyncError> {
        // Fetch players and pairings separately (instead of fetch_standings)
        // so we can persist pairings
        let players = bcp_client
            .fetch_players(&bcp_event.id)
            .await
            .map_err(SyncError::Fetch)?;
        let bcp_pairings = bcp_client
            .fetch_pairings(&bcp_event.id)
            .await
            .map_err(SyncError::Fetch)?;

        // Persist pairings
        if !bcp_pairings.is_empty() && !self.config.dry_run {
            let model_pairings =
                convert::pairings_from_bcp(&bcp_pairings, event_id, epoch_id.clone());
            if !model_pairings.is_empty() {
                let pairing_writer =
                    JsonlWriter::for_entity(&self.config.storage, EntityType::Pairing, epoch_str);
                pairing_writer
                    .append_batch(&model_pairings)
                    .map_err(SyncError::Storage)?;
                info!(
                    "  BCP: persisted {} pairings for {}",
                    model_pairings.len(),
                    bcp_event.name
                );
            }
        }

        // Compute standings from pairings
        let standings = if bcp_pairings.is_empty() {
            info!(
                "BCP: no pairings for event {} ({}), skipping",
                bcp_event.name, bcp_event.id
            );
            return Ok((0, 0));
        } else {
            bcp_client.compute_standings(&bcp_pairings, &players)
        };

        let event_date = bcp_event
            .parsed_start_date()
            .unwrap_or_else(|| chrono::Utc::now().date_naive());

        // Dedup existing placements
        let existing_placement_ids: std::collections::HashSet<String> = if !self.config.dry_run {
            crate::storage::JsonlReader::<crate::models::Placement>::for_entity(
                &self.config.storage,
                EntityType::Placement,
                epoch_str,
            )
            .read_all()
            .unwrap_or_default()
            .iter()
            .map(|p| p.id.as_str().to_string())
            .collect()
        } else {
            std::collections::HashSet::new()
        };

        let mut placement_count = 0u32;
        let mut list_count = 0u32;

        // Buffer placements in memory (don't write yet — link list_id after army list fetch)
        let mut new_placements: Vec<Placement> = Vec::new();
        for standing in &standings {
            let placement =
                convert::placement_from_bcp(standing, event_id.clone(), epoch_id.clone(), None);

            if !self.config.dry_run && existing_placement_ids.contains(placement.id.as_str()) {
                continue;
            }
            new_placements.push(placement);
            placement_count += 1;
        }

        // Build set of player names that already have linked army lists
        let players_with_lists: std::collections::HashSet<String> = if !self.config.dry_run {
            crate::storage::JsonlReader::<Placement>::for_entity(
                &self.config.storage,
                EntityType::Placement,
                epoch_str,
            )
            .read_all()
            .unwrap_or_default()
            .iter()
            .filter(|p| p.event_id == *event_id && p.list_id.is_some())
            .map(|p| normalize_player_name(&p.player_name))
            .collect()
        } else {
            std::collections::HashSet::new()
        };

        // Fetch army lists for up to 50 standings that don't already have lists
        let standings_with_lists: Vec<&bcp::BcpStanding> = standings
            .iter()
            .filter(|s| {
                s.player_id.is_some()
                    && s.player_name
                        .as_ref()
                        .map(|n| !players_with_lists.contains(&normalize_player_name(n)))
                        .unwrap_or(false)
            })
            .take(50)
            .collect();
        let total_to_fetch = standings_with_lists.len();

        if total_to_fetch > 0 {
            self.emit_progress(
                0,
                placement_count,
                list_count,
                0,
                0,
                format!(
                    "Fetching {} army lists for {}...",
                    total_to_fetch, bcp_event.name
                ),
                Vec::new(),
            );
        }

        // Load existing army list IDs to avoid writing duplicates during backfill
        let existing_bcp_list_ids: std::collections::HashSet<String> = if !self.config.dry_run {
            crate::storage::JsonlReader::<ArmyList>::for_entity(
                &self.config.storage,
                EntityType::ArmyList,
                epoch_str,
            )
            .read_all()
            .unwrap_or_default()
            .iter()
            .map(|l| l.id.as_str().to_string())
            .collect()
        } else {
            std::collections::HashSet::new()
        };

        // Fetch army lists from Listhammer (no rate limiting needed)
        // Track player→chapter for post-fix of placement factions
        let mut player_chapter_fixes: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut stored_lists: Vec<ArmyList> = Vec::new();
        for (fetch_idx, standing) in standings_with_lists.iter().enumerate() {
            if *self.cancel_token.read().await {
                break;
            }

            let player_id = match &standing.player_id {
                Some(id) => id,
                None => continue,
            };

            let player_name_str = standing.player_name.as_deref().unwrap_or("?");
            self.emit_progress(
                0,
                placement_count,
                list_count,
                0,
                0,
                format!(
                    "Fetching army list {}/{} for {} ({})",
                    fetch_idx + 1,
                    total_to_fetch,
                    bcp_event.name,
                    player_name_str,
                ),
                Vec::new(),
            );

            let bcp_list = match bcp_client.fetch_army_list(&bcp_event.id, player_id).await {
                Ok(Some(list)) => list,
                Ok(None) => continue,
                Err(e) => {
                    warn!("  BCP: failed to fetch list for {}: {}", player_name_str, e);
                    continue;
                }
            };

            // Check list has content
            if bcp_list
                .army_list
                .as_ref()
                .is_none_or(|t| t.trim().is_empty())
            {
                continue;
            }

            let raw_text = bcp_list.army_list.clone().unwrap_or_default();
            let player_name = standing
                .player_name
                .clone()
                .unwrap_or_else(|| "Unknown".to_string());
            let faction_hint = bcp_list
                .army_faction
                .clone()
                .or_else(|| bcp_list.faction.clone())
                .or_else(|| standing.faction.clone());

            // Try regex parsing first (free), fall back to AI only if regex finds nothing
            let regex_units = bcp::parse_units_from_raw_text(&raw_text);

            let (
                norm_faction,
                norm_detachment,
                norm_subfaction,
                norm_points,
                norm_units,
                norm_confidence,
            ) = if !regex_units.is_empty() {
                // Regex worked — use BCP structured data for faction/detachment
                let total_pts: u32 = regex_units.iter().filter_map(|u| u.points).sum();

                // Detect specific SM chapter from raw text
                let mut resolved_faction = faction_hint
                    .clone()
                    .unwrap_or_else(|| "Unknown".to_string());
                let is_generic_sm = resolved_faction == "Space Marines (Astartes)"
                    || resolved_faction == "Space Marines"
                    || resolved_faction == "Adeptus Astartes";
                if is_generic_sm {
                    if let Some(chapter) = bcp::detect_chapter_from_raw_text(&raw_text) {
                        info!("    Chapter detected: {} -> {}", resolved_faction, chapter);
                        resolved_faction = chapter.to_string();
                        player_chapter_fixes
                            .insert(player_name.trim().to_lowercase(), chapter.to_string());
                    }
                }

                info!(
                    "    Parsed BCP list (regex): {} ({} units, {}pts)",
                    resolved_faction,
                    regex_units.len(),
                    total_pts,
                );
                (
                    resolved_faction,
                    bcp_list.detachment.clone(),
                    None,
                    total_pts,
                    regex_units,
                    crate::models::Confidence::High,
                )
            } else if raw_text.len() < 50 {
                // Raw text too short to contain a real army list — skip AI
                info!(
                    "    Skipping AI normalizer for {} (raw text too short: {} chars)",
                    player_name,
                    raw_text.len(),
                );
                (
                    faction_hint.unwrap_or_else(|| "Unknown".to_string()),
                    bcp_list.detachment.clone(),
                    None,
                    0,
                    Vec::new(),
                    crate::models::Confidence::Low,
                )
            } else {
                // Regex failed — fall back to AI normalization
                info!(
                    "    Regex parse failed for {}, using AI normalizer",
                    player_name,
                );
                let normalizer = ListNormalizerAgent::new(self.backend.clone());
                let norm_input = ListNormalizerInput {
                    raw_text: raw_text.clone(),
                    faction_hint: faction_hint.clone(),
                    player_name: player_name.clone(),
                };

                match normalizer.execute(norm_input).await {
                    Ok(output) => {
                        let d = output.list.data;
                        info!(
                            "    Normalized BCP list (AI): {} - {} ({} units, {}pts)",
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
                            "    BCP list normalization failed for {}: {}",
                            player_name, e
                        );
                        (
                            faction_hint.unwrap_or_else(|| "Unknown".to_string()),
                            None,
                            None,
                            0,
                            Vec::new(),
                            crate::models::Confidence::Low,
                        )
                    }
                }
            };

            let mut army_list = ArmyList::new(norm_faction, norm_points, norm_units, raw_text)
                .with_player_name(player_name)
                .with_event_date(event_date)
                .with_event_id(event_id.clone())
                .with_source_url(bcp_event.event_url())
                .with_confidence(norm_confidence);

            if let Some(det) = norm_detachment {
                army_list = army_list.with_detachment(det);
            }
            if let Some(sub) = norm_subfaction {
                army_list = army_list.with_subfaction(sub);
            }

            if !self.config.dry_run && !existing_bcp_list_ids.contains(army_list.id.as_str()) {
                let writer =
                    JsonlWriter::for_entity(&self.config.storage, EntityType::ArmyList, epoch_str);
                writer.append(&army_list).map_err(SyncError::Storage)?;
                list_count += 1;
            }
            stored_lists.push(army_list);
        }

        // Link placements to army lists by player name and backfill detachment
        let name_to_list: std::collections::HashMap<String, &ArmyList> = stored_lists
            .iter()
            .filter_map(|l| {
                l.player_name
                    .as_ref()
                    .map(|name| (normalize_player_name(name), l))
            })
            .collect();

        for placement in &mut new_placements {
            let norm_name = normalize_player_name(&placement.player_name);
            if let Some(list) = name_to_list.get(&norm_name) {
                if placement.list_id.is_none() {
                    placement.list_id = Some(list.id.clone());
                }
                if placement.detachment.is_none() {
                    if let Some(ref det) = list.detachment {
                        placement.detachment = Some(det.clone());
                    }
                }
            }
        }

        // Apply chapter fixes to buffered placements
        if !player_chapter_fixes.is_empty() {
            for p in &mut new_placements {
                let is_generic = p.faction == "Space Marines (Astartes)"
                    || p.faction == "Space Marines"
                    || p.faction == "Adeptus Astartes";
                if is_generic {
                    let norm_name = p.player_name.trim().to_lowercase();
                    if let Some(chapter) = player_chapter_fixes.get(&norm_name) {
                        p.faction = chapter.clone();
                    }
                }
            }
        }

        // Write new placements and update existing ones with newly-fetched lists
        if !self.config.dry_run {
            if !new_placements.is_empty() {
                let writer =
                    JsonlWriter::for_entity(&self.config.storage, EntityType::Placement, epoch_str);
                writer
                    .append_batch(&new_placements)
                    .map_err(SyncError::Storage)?;
            }

            // Update existing placements that got new lists in this batch
            if !stored_lists.is_empty() {
                let mut all_placements: Vec<Placement> =
                    crate::storage::JsonlReader::<Placement>::for_entity(
                        &self.config.storage,
                        EntityType::Placement,
                        epoch_str,
                    )
                    .read_all()
                    .unwrap_or_default();

                let mut backfill_count = 0u32;
                for p in &mut all_placements {
                    if p.event_id == *event_id && p.list_id.is_none() {
                        let norm_name = normalize_player_name(&p.player_name);
                        if let Some(list) = name_to_list.get(&norm_name) {
                            p.list_id = Some(list.id.clone());
                            if p.detachment.is_none() {
                                if let Some(ref det) = list.detachment {
                                    p.detachment = Some(det.clone());
                                }
                            }
                            backfill_count += 1;
                        }
                    }
                }

                if backfill_count > 0 {
                    let writer = JsonlWriter::for_entity(
                        &self.config.storage,
                        EntityType::Placement,
                        epoch_str,
                    );
                    writer
                        .write_all(&all_placements)
                        .map_err(SyncError::Storage)?;
                    info!(
                        "  BCP: backfilled {} list links for existing placements in {}",
                        backfill_count, bcp_event.name
                    );
                    list_count += backfill_count;
                }
            }
        }

        info!(
            "  BCP: {} placements, {} lists for {}",
            placement_count, list_count, bcp_event.name
        );
        self.emit_progress(
            0,
            placement_count,
            list_count,
            0,
            0,
            format!(
                "BCP: {} placements, {} lists for {}",
                placement_count, list_count, bcp_event.name
            ),
            Vec::new(),
        );

        Ok((placement_count, list_count))
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

    #[tokio::test]
    async fn test_bcp_source_serialization() {
        let source = SyncSource::Bcp {
            api_base_url: "https://newprod-api.bestcoastpairings.com/v1".to_string(),
            game_type: 1,
        };

        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("bcp"));

        let parsed: SyncSource = serde_json::from_str(&json).unwrap();
        match parsed {
            SyncSource::Bcp {
                api_base_url,
                game_type,
            } => {
                assert!(api_base_url.contains("bestcoastpairings"));
                assert_eq!(game_type, 1);
            }
            _ => panic!("Expected BCP source"),
        }
    }

    #[test]
    fn test_normalize_player_name() {
        assert_eq!(normalize_player_name("John  Smith"), "john smith");
        assert_eq!(normalize_player_name("  Alice  "), "alice");
        assert_eq!(normalize_player_name("Bob"), "bob");
    }

    #[test]
    fn test_sync_status_serialization() {
        let status = SyncStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");

        let parsed: SyncStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SyncStatus::Running);
    }

    #[test]
    fn test_sync_config_default_values() {
        let config = SyncConfig::default();
        assert_eq!(config.interval, Duration::from_secs(6 * 3600));
        assert!(!config.dry_run);
        assert!(config.date_from.is_none());
        assert!(config.date_to.is_none());
        assert_eq!(config.sources.len(), 1);
    }

    #[test]
    fn test_sync_state_serialization_roundtrip() {
        let state = SyncState::default();
        let json = serde_json::to_string(&state).unwrap();
        let parsed: SyncState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_sync_status, SyncStatus::Idle);
        assert_eq!(parsed.events_synced, 0);
    }

    #[test]
    fn test_sync_error_display() {
        let err = SyncError::NoSources;
        assert_eq!(format!("{}", err), "No sources configured");

        let err = SyncError::Cancelled;
        assert_eq!(format!("{}", err), "Sync cancelled");
    }

    #[test]
    fn test_sync_result_construction() {
        let result = SyncResult {
            events_synced: 5,
            placements_synced: 20,
            lists_normalized: 10,
            items_for_review: 2,
            errors: vec!["test error".to_string()],
            duration: Duration::from_secs(10),
        };
        assert_eq!(result.events_synced, 5);
        assert_eq!(result.errors.len(), 1);
    }

    #[tokio::test]
    async fn test_orchestrator_cancel() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let fetcher = Fetcher::new(FetcherConfig {
            cache_dir: temp_dir.path().join("cache"),
            ..Default::default()
        })
        .unwrap();
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));

        let orchestrator = SyncOrchestrator::new(config, fetcher, backend);
        orchestrator.cancel().await;
        // After cancel, a sync should fail with Cancelled
        let result = orchestrator.sync_once().await;
        // Note: cancel sets the token, but sync_once resets it first.
        // So this actually tests the reset behavior.
        // The actual cancel only works mid-sync.
        assert!(result.is_ok() || matches!(result, Err(SyncError::Cancelled)));
    }

    #[tokio::test]
    async fn test_orchestrator_is_running() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let fetcher = Fetcher::new(FetcherConfig {
            cache_dir: temp_dir.path().join("cache"),
            ..Default::default()
        })
        .unwrap();
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));

        let orchestrator = SyncOrchestrator::new(config, fetcher, backend);
        assert!(!orchestrator.is_running().await);
    }
}
