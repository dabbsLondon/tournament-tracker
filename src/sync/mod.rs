//! Sync orchestrator.
//!
//! Coordinates the ingestion pipeline:
//! 1. Fetch content from sources
//! 2. Run AI agents to extract data
//! 3. Validate with Fact Checker
//! 4. Store in JSONL and Parquet

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{error, info, warn};

use crate::agents::backend::AiBackend;
use crate::fetch::Fetcher;
use crate::storage::StorageConfig;

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
            base_url: "https://www.goonhammer.com/category/competitive-innovations/".to_string(),
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
}

impl SyncOrchestrator {
    /// Create a new sync orchestrator.
    pub fn new(config: SyncConfig, fetcher: Fetcher, backend: Arc<dyn AiBackend>) -> Self {
        Self {
            config,
            fetcher,
            backend,
            state: Arc::new(RwLock::new(SyncState::default())),
            cancel_token: Arc::new(RwLock::new(false)),
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
        // Access fields to prevent dead_code warning (full implementation pending)
        let _ = &self.fetcher;
        let _ = &self.backend;

        match source {
            SyncSource::Goonhammer { base_url } => {
                info!("Syncing from Goonhammer: {}", base_url);
                // TODO: Implement Goonhammer sync pipeline
                // 1. Fetch article index
                // 2. For each article in date range:
                //    a. Fetch article HTML
                //    b. Run Event Scout agent
                //    c. Run Result Harvester agent
                //    d. Run List Normalizer agent
                //    e. Run Fact Checker agent
                //    f. Store results
                Ok(SyncResult {
                    events_synced: 0,
                    placements_synced: 0,
                    lists_normalized: 0,
                    items_for_review: 0,
                    errors: vec![],
                    duration: Duration::ZERO,
                })
            }
            SyncSource::WarhammerCommunity { url } => {
                info!("Syncing balance updates from: {}", url);
                // TODO: Implement Balance Watcher pipeline
                Ok(SyncResult {
                    events_synced: 0,
                    placements_synced: 0,
                    lists_normalized: 0,
                    items_for_review: 0,
                    errors: vec![],
                    duration: Duration::ZERO,
                })
            }
        }
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
