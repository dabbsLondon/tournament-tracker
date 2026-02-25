use std::sync::Arc;

use crate::agents::backend::AiBackend;
use crate::api::routes::refresh::RefreshState;
use crate::api::routes::traffic::SharedTrafficStats;
use crate::models::EpochMapper;
use crate::storage::StorageConfig;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<StorageConfig>,
    pub epoch_mapper: Arc<tokio::sync::RwLock<EpochMapper>>,
    pub refresh_state: Arc<tokio::sync::RwLock<RefreshState>>,
    pub ai_backend: Arc<dyn AiBackend>,
    pub traffic_stats: SharedTrafficStats,
}
