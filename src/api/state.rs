use std::sync::Arc;

use crate::models::EpochMapper;
use crate::storage::StorageConfig;

#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<StorageConfig>,
    pub epoch_mapper: Arc<EpochMapper>,
}
