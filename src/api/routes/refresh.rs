use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::api::state::AppState;
use crate::api::ApiError;
use crate::models::{Event, Placement};
use crate::storage::{EntityType, JsonlReader};

/// Reject requests that come through Cloudflare Tunnel (public domain).
/// Cloudflare always adds the `CF-Connecting-IP` header to proxied requests.
fn require_local(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key("cf-connecting-ip") {
        return Err(ApiError::Forbidden(
            "Refresh is only available on localhost".to_string(),
        ));
    }
    Ok(())
}

// ── Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefreshState {
    pub status: RefreshStatus,
    pub phase: RefreshPhase,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub progress: RefreshProgress,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RefreshStatus {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RefreshPhase {
    #[default]
    Idle,
    CheckingBalance,
    SyncingResults,
    DiscoveringFuture,
    Repartitioning,
    Done,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefreshProgress {
    pub balance_passes_found: u32,
    /// New events added this refresh
    pub events_synced: u32,
    /// New placements added this refresh
    pub placements_synced: u32,
    /// New lists processed this refresh
    pub lists_normalized: u32,
    pub future_events_found: u32,
    /// How many BCP events were found in the date range (denominator)
    #[serde(default)]
    pub events_discovered: u32,
    /// Current event index being processed (1-based)
    #[serde(default)]
    pub current_event_index: u32,
    /// Cumulative totals across all epochs in the database
    #[serde(default)]
    pub total_events: u32,
    #[serde(default)]
    pub total_placements: u32,
    #[serde(default)]
    pub total_lists: u32,
    /// Live status message shown in the UI during refresh
    #[serde(default)]
    pub message: String,
    /// Per-event progress for calendar view
    #[serde(default)]
    pub discovered_events: Vec<EventProgress>,
}

/// Status of a single event during sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventSyncStatus {
    Pending,
    Syncing,
    Done,
    Skipped,
}

/// Progress for a single event during sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventProgress {
    pub name: String,
    pub date: String,
    pub player_count: u32,
    pub status: EventSyncStatus,
    pub placements_found: u32,
    pub lists_found: u32,
    pub detail: String,
}

// ── Preview ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PreviewParams {
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PreviewResponse {
    pub date_from: String,
    pub date_to: String,
    pub events_in_range: u32,
    pub events_with_results: u32,
    pub scheduled_without_data: u32,
    pub total_events: u32,
    pub refresh_status: RefreshStatus,
}

/// Count past events in range that have no placements.
fn count_events_without_results(
    events: &[Event],
    placements: &[Placement],
    today: NaiveDate,
    date_from: NaiveDate,
    date_to: NaiveDate,
) -> (u32, u32, u32) {
    let events_with_placements: std::collections::HashSet<&str> =
        placements.iter().map(|p| p.event_id.as_str()).collect();
    let in_range: Vec<&Event> = events
        .iter()
        .filter(|e| e.date >= date_from && e.date <= date_to)
        .collect();
    let events_in_range = in_range.len() as u32;
    let with_results = in_range
        .iter()
        .filter(|e| events_with_placements.contains(e.id.as_str()))
        .count() as u32;
    let cutoff = today - chrono::Days::new(3);
    let scheduled_without_data = in_range
        .iter()
        .filter(|e| {
            e.date <= today
                && e.date < cutoff
                && e.player_count.unwrap_or(0) >= 10
                && !events_with_placements.contains(e.id.as_str())
        })
        .count() as u32;
    (events_in_range, with_results, scheduled_without_data)
}

fn parse_date_or(s: Option<&str>, fallback: NaiveDate) -> NaiveDate {
    s.and_then(|v| NaiveDate::parse_from_str(v, "%Y-%m-%d").ok())
        .unwrap_or(fallback)
}

pub async fn preview(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(params): Query<PreviewParams>,
) -> Result<Json<PreviewResponse>, ApiError> {
    require_local(&headers)?;
    let today = Utc::now().date_naive();
    let date_from = parse_date_or(params.date_from.as_deref(), today - chrono::Days::new(30));
    // Clamp date_to to today
    let date_to_raw = parse_date_or(params.date_to.as_deref(), today);
    let date_to = if date_to_raw > today {
        today
    } else {
        date_to_raw
    };

    let mapper = state.epoch_mapper.read().await;

    // Load events from all epochs
    let epoch_ids: Vec<String> = {
        let epochs = mapper.all_epochs();
        if epochs.is_empty() {
            vec!["current".to_string()]
        } else {
            epochs.iter().map(|e| e.id.as_str().to_string()).collect()
        }
    };

    let mut all_events: Vec<Event> = Vec::new();
    let mut all_placements: Vec<Placement> = Vec::new();

    for epoch_id in &epoch_ids {
        let reader = JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, epoch_id);
        if let Ok(events) = reader.read_all() {
            all_events.extend(events);
        }
        let reader =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id);
        if let Ok(placements) = reader.read_all() {
            all_placements.extend(placements);
        }
    }

    let total_events = all_events.len() as u32;
    let (events_in_range, events_with_results, scheduled_without_data) =
        count_events_without_results(&all_events, &all_placements, today, date_from, date_to);

    let refresh_status = state.refresh_state.read().await.status;

    Ok(Json(PreviewResponse {
        date_from: date_from.to_string(),
        date_to: date_to.to_string(),
        events_in_range,
        events_with_results,
        scheduled_without_data,
        total_events,
        refresh_status,
    }))
}

// ── Start ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct StartParams {
    pub date_from: Option<String>,
    pub date_to: Option<String>,
}

pub async fn start_refresh(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(params): Json<StartParams>,
) -> Result<impl IntoResponse, ApiError> {
    require_local(&headers)?;
    let today = Utc::now().date_naive();
    let date_from = parse_date_or(params.date_from.as_deref(), today - chrono::Days::new(30));
    let date_to_raw = parse_date_or(params.date_to.as_deref(), today);
    let date_to = if date_to_raw > today {
        today
    } else {
        date_to_raw
    };

    // Check if already running
    {
        let current = state.refresh_state.read().await;
        if current.status == RefreshStatus::Running {
            return Err(ApiError::Conflict("Refresh already running".to_string()));
        }
    }

    // Set status to Running
    let now = Utc::now();
    {
        let mut refresh = state.refresh_state.write().await;
        *refresh = RefreshState {
            status: RefreshStatus::Running,
            phase: RefreshPhase::Idle,
            started_at: Some(now),
            completed_at: None,
            progress: RefreshProgress::default(),
            errors: Vec::new(),
        };
    }

    // Spawn background task
    let refresh_state = state.refresh_state.clone();
    let storage = state.storage.clone();
    let epoch_mapper = state.epoch_mapper.clone();
    let ai_backend = state.ai_backend.clone();

    tokio::spawn(async move {
        run_refresh_pipeline(
            refresh_state,
            storage,
            epoch_mapper,
            ai_backend,
            date_from,
            date_to,
        )
        .await;
    });

    // Return 202 with current state
    let current = state.refresh_state.read().await;
    Ok((StatusCode::ACCEPTED, Json(current.clone())))
}

// ── Status ───────────────────────────────────────────────────────

pub async fn status(State(state): State<AppState>) -> Json<RefreshState> {
    let current = state.refresh_state.read().await;
    Json(current.clone())
}

// ── Background Pipeline ──────────────────────────────────────────

async fn run_refresh_pipeline(
    refresh_state: Arc<tokio::sync::RwLock<RefreshState>>,
    storage: Arc<crate::storage::StorageConfig>,
    epoch_mapper: Arc<tokio::sync::RwLock<crate::models::EpochMapper>>,
    ai_backend: Arc<dyn crate::agents::backend::AiBackend>,
    date_from: NaiveDate,
    _date_to: NaiveDate,
) {
    let today = Utc::now().date_naive();
    let mut errors: Vec<String> = Vec::new();

    // Step 1: Check for balance passes
    {
        let mut state = refresh_state.write().await;
        state.phase = RefreshPhase::CheckingBalance;
        state.progress.message = "Checking Warhammer Community for balance updates...".to_string();
    }

    let mut new_balance_passes = 0u32;
    match run_balance_check(&storage, &ai_backend).await {
        Ok(count) => {
            new_balance_passes = count;
            let mut state = refresh_state.write().await;
            state.progress.balance_passes_found = count;
            state.progress.message = if count > 0 {
                format!("Found {} new balance pass(es)", count)
            } else {
                "No new balance changes".to_string()
            };
        }
        Err(e) => {
            let msg = format!("Balance check failed: {}", e);
            tracing::warn!("{}", msg);
            errors.push(msg);
            let mut state = refresh_state.write().await;
            state.progress.message = "Balance check failed, continuing...".to_string();
        }
    }

    // Step 2: Sync past results
    {
        let mut state = refresh_state.write().await;
        state.phase = RefreshPhase::SyncingResults;
        state.progress.message = "Discovering BCP events and fetching results...".to_string();
    }

    match run_sync(
        &storage,
        &ai_backend,
        date_from,
        today,
        refresh_state.clone(),
    )
    .await
    {
        Ok((events, placements, lists)) => {
            let mut state = refresh_state.write().await;
            state.progress.events_synced = events;
            state.progress.placements_synced = placements;
            state.progress.lists_normalized = lists;
            state.progress.message = format!(
                "Synced {} events, {} placements, {} lists",
                events, placements, lists
            );
        }
        Err(e) => {
            let msg = format!("Sync failed: {}", e);
            tracing::warn!("{}", msg);
            errors.push(msg);
            let mut state = refresh_state.write().await;
            state.progress.message = "Sync failed, continuing...".to_string();
        }
    }

    // Compute cumulative totals across all epochs in the database
    {
        let mapper = crate::storage::read_significant_events(&storage).unwrap_or_default();
        let epoch_mapper = if mapper.is_empty() {
            crate::models::EpochMapper::new()
        } else {
            crate::models::EpochMapper::from_significant_events(&mapper)
        };

        let epoch_ids: Vec<String> = {
            let epochs = epoch_mapper.all_epochs();
            if epochs.is_empty() {
                vec!["current".to_string()]
            } else {
                epochs.iter().map(|e| e.id.as_str().to_string()).collect()
            }
        };

        let mut total_events = 0u32;
        let mut total_placements = 0u32;
        let mut total_lists = 0u32;

        for epoch_id in &epoch_ids {
            let event_reader =
                JsonlReader::<Event>::for_entity(&storage, EntityType::Event, epoch_id);
            if let Ok(events) = event_reader.read_all() {
                total_events += events.len() as u32;
            }
            let placement_reader =
                JsonlReader::<Placement>::for_entity(&storage, EntityType::Placement, epoch_id);
            if let Ok(placements) = placement_reader.read_all() {
                total_placements += placements.len() as u32;
            }
            let list_reader = JsonlReader::<crate::models::ArmyList>::for_entity(
                &storage,
                EntityType::ArmyList,
                epoch_id,
            );
            if let Ok(lists) = list_reader.read_all() {
                total_lists += lists.len() as u32;
            }
        }

        let mut state = refresh_state.write().await;
        state.progress.total_events = total_events;
        state.progress.total_placements = total_placements;
        state.progress.total_lists = total_lists;
    }

    // Step 3: Discover future events
    {
        let mut state = refresh_state.write().await;
        state.phase = RefreshPhase::DiscoveringFuture;
        state.progress.message = "Discovering upcoming BCP events...".to_string();
    }

    let future_end = today + chrono::Days::new(60);
    match run_future_discovery(&storage, &ai_backend, today, future_end).await {
        Ok(count) => {
            let mut state = refresh_state.write().await;
            state.progress.future_events_found = count;
            state.progress.message = format!("Found {} upcoming events", count);
        }
        Err(e) => {
            let msg = format!("Future discovery failed: {}", e);
            tracing::warn!("{}", msg);
            errors.push(msg);
            let mut state = refresh_state.write().await;
            state.progress.message = "Future discovery failed".to_string();
        }
    }

    // Step 4: Repartition if new balance passes found
    if new_balance_passes > 0 {
        {
            let mut state = refresh_state.write().await;
            state.phase = RefreshPhase::Repartitioning;
        }

        match crate::sync::repartition::repartition(&storage, "current", false, false) {
            Ok(_) => {
                tracing::info!(
                    "Repartition completed after {} new balance passes",
                    new_balance_passes
                );
            }
            Err(e) => {
                let msg = format!("Repartition failed: {}", e);
                tracing::warn!("{}", msg);
                errors.push(msg);
            }
        }
    }

    // Step 5: Rebuild epoch mapper
    {
        let sig_events = crate::storage::read_significant_events(&storage).unwrap_or_default();
        let new_mapper = if sig_events.is_empty() {
            crate::models::EpochMapper::new()
        } else {
            crate::models::EpochMapper::from_significant_events(&sig_events)
        };
        let mut mapper = epoch_mapper.write().await;
        *mapper = new_mapper;
    }

    // Final state — only mark as Failed if the core sync step had errors.
    // Balance check and future discovery errors are non-critical warnings.
    {
        let mut state = refresh_state.write().await;
        state.phase = RefreshPhase::Done;
        state.completed_at = Some(Utc::now());
        let has_sync_error = errors.iter().any(|e| e.starts_with("Sync failed"));
        state.errors = errors.clone();
        state.status = if has_sync_error {
            RefreshStatus::Failed
        } else {
            RefreshStatus::Completed
        };
    }
}

async fn run_balance_check(
    storage: &crate::storage::StorageConfig,
    backend: &Arc<dyn crate::agents::backend::AiBackend>,
) -> Result<u32, anyhow::Error> {
    let fetcher = crate::fetch::Fetcher::new(crate::fetch::FetcherConfig {
        cache_dir: storage.raw_dir(),
        ..Default::default()
    })?;

    let wh_url = "https://www.warhammer-community.com/en-gb/downloads/warhammer-40000/";
    let page_url = url::Url::parse(wh_url)?;
    let fetch_result = fetcher.fetch(&page_url).await?;
    let html = fetcher.read_cached_text(&fetch_result).await?;

    use crate::agents::balance_watcher::{BalanceWatcherAgent, BalanceWatcherInput};
    use crate::agents::Agent;
    let existing = crate::storage::read_significant_events(storage).unwrap_or_default();
    let known_ids = existing.iter().map(|e| e.id.clone()).collect();

    let watcher = BalanceWatcherAgent::new(backend.clone());
    let input = BalanceWatcherInput {
        html_content: html,
        source_url: wh_url.to_string(),
        known_event_ids: known_ids,
    };

    let output = watcher.execute(input).await?;
    let new_count = output.events.len() as u32;

    if new_count > 0 {
        let mut merged = existing;
        let existing_ids: std::collections::HashSet<String> =
            merged.iter().map(|e| e.id.as_str().to_string()).collect();
        for event_output in &output.events {
            if !existing_ids.contains(event_output.data.id.as_str()) {
                merged.push(event_output.data.clone());
            }
        }
        crate::storage::write_significant_events(storage, &mut merged)?;
    }

    Ok(new_count)
}

async fn run_sync(
    storage: &crate::storage::StorageConfig,
    backend: &Arc<dyn crate::agents::backend::AiBackend>,
    date_from: NaiveDate,
    date_to: NaiveDate,
    refresh_state: Arc<tokio::sync::RwLock<RefreshState>>,
) -> Result<(u32, u32, u32), anyhow::Error> {
    let fetcher = crate::fetch::Fetcher::new(crate::fetch::FetcherConfig {
        cache_dir: storage.raw_dir(),
        ..Default::default()
    })?;

    let sync_config = crate::sync::SyncConfig {
        sources: vec![crate::sync::SyncSource::default()],
        interval: std::time::Duration::from_secs(3600),
        date_from: Some(date_from),
        date_to: Some(date_to),
        dry_run: false,
        storage: storage.clone(),
    };

    let rs = refresh_state.clone();
    let orchestrator = crate::sync::SyncOrchestrator::new(sync_config, fetcher, backend.clone())
        .with_progress_callback(move |progress| {
            // Update refresh state in a blocking fashion (callback is sync)
            if let Ok(mut state) = rs.try_write() {
                state.progress.events_synced = progress.events_synced;
                state.progress.placements_synced = progress.placements_synced;
                state.progress.lists_normalized = progress.lists_normalized;
                state.progress.events_discovered = progress.events_discovered;
                state.progress.current_event_index = progress.current_event_index;
                state.progress.message = progress.message.clone();
                // Only update discovered_events when the sync sends a non-empty list;
                // inner calls (e.g. from sync_bcp_standings) send Vec::new() and
                // should NOT wipe the existing per-event progress.
                if !progress.discovered_events.is_empty() {
                    state.progress.discovered_events = progress
                        .discovered_events
                        .iter()
                        .map(|sep| EventProgress {
                            name: sep.name.clone(),
                            date: sep.date.clone(),
                            player_count: sep.player_count,
                            status: match sep.status {
                                crate::sync::SyncEventStatus::Pending => EventSyncStatus::Pending,
                                crate::sync::SyncEventStatus::Syncing => EventSyncStatus::Syncing,
                                crate::sync::SyncEventStatus::Done => EventSyncStatus::Done,
                                crate::sync::SyncEventStatus::Skipped => EventSyncStatus::Skipped,
                            },
                            placements_found: sep.placements_found,
                            lists_found: sep.lists_found,
                            detail: sep.detail.clone(),
                        })
                        .collect();
                } else if !progress.message.is_empty() {
                    // Update the detail of the currently-syncing event with the message
                    for ev in &mut state.progress.discovered_events {
                        if matches!(ev.status, EventSyncStatus::Syncing) {
                            ev.detail = progress.message.clone();
                        }
                    }
                }
            }
        });
    let result = orchestrator.sync_once().await?;

    Ok((
        result.events_synced,
        result.placements_synced,
        result.lists_normalized,
    ))
}

async fn run_future_discovery(
    storage: &crate::storage::StorageConfig,
    _backend: &Arc<dyn crate::agents::backend::AiBackend>,
    date_from: NaiveDate,
    date_to: NaiveDate,
) -> Result<u32, anyhow::Error> {
    // Discover future BCP events (no auth needed — BCP rejects authed requests to /events)
    let fetcher = crate::fetch::Fetcher::new(crate::fetch::FetcherConfig {
        cache_dir: storage.raw_dir(),
        extra_headers: crate::sync::bcp::bcp_headers(),
        ..Default::default()
    })?;

    let bcp_client = crate::sync::bcp::BcpClient::new(
        fetcher,
        "https://newprod-api.bestcoastpairings.com/v1".to_string(),
        1,
    );

    let bcp_events = bcp_client.discover_events(date_from, date_to).await?;
    let new_event_count = bcp_events.len() as u32;

    // Determine epoch for future events
    let sig_events = crate::storage::read_significant_events(storage).unwrap_or_default();
    let epoch_mapper = if sig_events.is_empty() {
        crate::models::EpochMapper::new()
    } else {
        crate::models::EpochMapper::from_significant_events(&sig_events)
    };

    let mut stored = 0u32;
    for bcp_event in &bcp_events {
        let event_date = bcp_event
            .parsed_start_date()
            .unwrap_or_else(|| Utc::now().date_naive());

        let epoch_id = if epoch_mapper.all_epochs().is_empty() {
            None
        } else {
            Some(epoch_mapper.get_epoch_id_for_date(event_date))
        };
        let epoch_str = epoch_id
            .as_ref()
            .map(|e| e.as_str().to_string())
            .unwrap_or_else(|| "current".to_string());

        let event = crate::sync::convert::event_from_bcp(bcp_event, epoch_id);

        // Dedup
        let existing_events: Vec<Event> =
            JsonlReader::for_entity(storage, EntityType::Event, &epoch_str)
                .read_all()
                .unwrap_or_default();

        if crate::sync::convert::find_duplicate_event(&event, &existing_events).is_some() {
            continue;
        }

        let writer =
            crate::storage::JsonlWriter::for_entity(storage, EntityType::Event, &epoch_str);
        writer
            .append(&event)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        stored += 1;
    }

    tracing::info!(
        "Future discovery: {} BCP events found, {} new stored",
        new_event_count,
        stored
    );

    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::build_router;
    use crate::api::state::AppState;
    use crate::models::EpochMapper;
    use crate::storage::StorageConfig;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn write_jsonl<T: serde::Serialize>(path: &std::path::Path, items: &[T]) {
        let mut content = String::new();
        for item in items {
            content.push_str(&serde_json::to_string(item).unwrap());
            content.push('\n');
        }
        std::fs::write(path, content).unwrap();
    }

    fn setup_test_state(dir: &std::path::Path) -> AppState {
        let storage = StorageConfig::new(dir.to_path_buf());
        let epoch_dir = dir.join("normalized").join("current");
        std::fs::create_dir_all(&epoch_dir).unwrap();
        AppState {
            storage: Arc::new(storage),
            epoch_mapper: Arc::new(tokio::sync::RwLock::new(EpochMapper::new())),
            refresh_state: Arc::new(tokio::sync::RwLock::new(RefreshState::default())),
            ai_backend: Arc::new(crate::agents::backend::MockBackend::new("{}")),
            traffic_stats: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::api::routes::traffic::TrafficStats::new(),
            )),
        }
    }

    fn make_event(name: &str, date: &str, source_url: &str) -> Event {
        Event::new(
            name.to_string(),
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            source_url.to_string(),
            "test".to_string(),
            "current".into(),
        )
    }

    fn make_placement(event: &Event, rank: u32, player: &str, faction: &str) -> Placement {
        Placement::new(
            event.id.clone(),
            "current".into(),
            rank,
            player.to_string(),
            faction.to_string(),
        )
    }

    async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, json)
    }

    async fn post_json(app: axum::Router, uri: &str, body: &str) -> (StatusCode, Value) {
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, json)
    }

    // ── Unit Tests ───────────────────────────────────────────────

    #[test]
    fn test_refresh_state_default() {
        let state = RefreshState::default();
        assert_eq!(state.status, RefreshStatus::Idle);
        assert_eq!(state.phase, RefreshPhase::Idle);
        assert!(state.started_at.is_none());
        assert!(state.completed_at.is_none());
        assert!(state.errors.is_empty());
    }

    #[test]
    fn test_refresh_state_serialization() {
        let state = RefreshState {
            status: RefreshStatus::Running,
            phase: RefreshPhase::SyncingResults,
            started_at: Some(Utc::now()),
            completed_at: None,
            progress: RefreshProgress {
                balance_passes_found: 1,
                events_synced: 5,
                placements_synced: 20,
                lists_normalized: 3,
                future_events_found: 10,
                ..RefreshProgress::default()
            },
            errors: vec!["test error".to_string()],
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: RefreshState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, RefreshStatus::Running);
        assert_eq!(parsed.phase, RefreshPhase::SyncingResults);
        assert_eq!(parsed.progress.events_synced, 5);
        assert_eq!(parsed.errors.len(), 1);
    }

    #[test]
    fn test_refresh_status_variants() {
        let variants = [
            (RefreshStatus::Idle, "\"idle\""),
            (RefreshStatus::Running, "\"running\""),
            (RefreshStatus::Completed, "\"completed\""),
            (RefreshStatus::Failed, "\"failed\""),
        ];
        for (status, expected) in &variants {
            let json = serde_json::to_string(status).unwrap();
            assert_eq!(&json, expected);
            let parsed: RefreshStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, status);
        }
    }

    #[test]
    fn test_refresh_phase_variants() {
        let variants = [
            (RefreshPhase::Idle, "\"idle\""),
            (RefreshPhase::CheckingBalance, "\"checking_balance\""),
            (RefreshPhase::SyncingResults, "\"syncing_results\""),
            (RefreshPhase::DiscoveringFuture, "\"discovering_future\""),
            (RefreshPhase::Repartitioning, "\"repartitioning\""),
            (RefreshPhase::Done, "\"done\""),
        ];
        for (phase, expected) in &variants {
            let json = serde_json::to_string(phase).unwrap();
            assert_eq!(&json, expected);
            let parsed: RefreshPhase = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, phase);
        }
    }

    #[test]
    fn test_refresh_progress_default() {
        let progress = RefreshProgress::default();
        assert_eq!(progress.balance_passes_found, 0);
        assert_eq!(progress.events_synced, 0);
        assert_eq!(progress.placements_synced, 0);
        assert_eq!(progress.lists_normalized, 0);
        assert_eq!(progress.future_events_found, 0);
    }

    #[test]
    fn test_count_events_without_results() {
        let today = NaiveDate::from_ymd_opt(2026, 2, 12).unwrap();
        let date_from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        let mut e1 = make_event("Past With Results", "2026-01-15", "https://example.com/a");
        e1.player_count = Some(20);
        let mut e2 = make_event("Past No Results", "2026-01-20", "https://example.com/b");
        e2.player_count = Some(30);
        let mut e3 = make_event("Future Event", "2026-03-01", "https://example.com/c");
        e3.player_count = Some(20);

        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");

        let events = vec![e1, e2, e3];
        let placements = vec![p1];

        let (in_range, with_results, without_data) =
            count_events_without_results(&events, &placements, today, date_from, today);
        assert_eq!(in_range, 2); // e1 and e2 in range (e3 is future, outside range)
        assert_eq!(with_results, 1); // e1
        assert_eq!(without_data, 1); // e2
    }

    #[test]
    fn test_count_events_without_results_all_have_results() {
        let today = NaiveDate::from_ymd_opt(2026, 2, 12).unwrap();
        let date_from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let e1 = make_event("Past Event", "2026-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");

        let (_, _, without_data) =
            count_events_without_results(&[e1], &[p1], today, date_from, today);
        assert_eq!(without_data, 0);
    }

    #[test]
    fn test_count_events_without_results_empty() {
        let today = NaiveDate::from_ymd_opt(2026, 2, 12).unwrap();
        let date_from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let (in_range, _, without_data) =
            count_events_without_results(&[], &[], today, date_from, today);
        assert_eq!(in_range, 0);
        assert_eq!(without_data, 0);
    }

    // ── Endpoint Tests ───────────────────────────────────────────

    #[tokio::test]
    async fn test_preview_endpoint() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let mut e1 = make_event("Past With Results", "2026-01-15", "https://example.com/a");
        e1.player_count = Some(20);
        let mut e2 = make_event("Past No Results", "2026-01-20", "https://example.com/b");
        e2.player_count = Some(30);
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1]);

        let app = build_router(state);
        let (status, json) = get_json(
            app,
            "/api/refresh/preview?date_from=2026-01-01&date_to=2026-02-12",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_events"], 2);
        assert_eq!(json["events_in_range"], 2);
        assert_eq!(json["events_with_results"], 1);
        assert_eq!(json["scheduled_without_data"], 1);
        assert_eq!(json["refresh_status"], "idle");
        assert!(json["date_from"].is_string());
        assert!(json["date_to"].is_string());
    }

    #[tokio::test]
    async fn test_status_endpoint_idle() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");
        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/refresh/status").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "idle");
        assert_eq!(json["phase"], "idle");
        assert!(json["started_at"].is_null());
        assert_eq!(json["progress"]["events_synced"], 0);
    }

    #[tokio::test]
    async fn test_start_returns_202() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");
        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = post_json(
            app,
            "/api/refresh",
            r#"{"date_from": "2026-02-05", "date_to": "2026-02-12"}"#,
        )
        .await;

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(json["status"], "running");
    }

    #[tokio::test]
    async fn test_start_rejects_concurrent() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");
        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);

        // Manually set refresh state to Running
        {
            let mut refresh = state.refresh_state.write().await;
            refresh.status = RefreshStatus::Running;
        }

        let app = build_router(state);
        let (status, _) = post_json(
            app,
            "/api/refresh",
            r#"{"date_from": "2026-02-05", "date_to": "2026-02-12"}"#,
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_status_shows_running() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");
        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);

        // Set state to running with some progress
        {
            let mut refresh = state.refresh_state.write().await;
            refresh.status = RefreshStatus::Running;
            refresh.phase = RefreshPhase::SyncingResults;
            refresh.progress.events_synced = 3;
        }

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/refresh/status").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "running");
        assert_eq!(json["phase"], "syncing_results");
        assert_eq!(json["progress"]["events_synced"], 3);
    }

    #[tokio::test]
    async fn test_preview_with_custom_dates() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");
        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(
            app,
            "/api/refresh/preview?date_from=2026-01-01&date_to=2026-02-12",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_events"], 0);
        assert_eq!(json["events_in_range"], 0);
        assert_eq!(json["scheduled_without_data"], 0);
    }
}
