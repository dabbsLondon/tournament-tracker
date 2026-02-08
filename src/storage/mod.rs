//! Filesystem data lake operations.
//!
//! Handles reading and writing to the local data lake:
//! - Raw content (HTML, PDFs)
//! - Normalized JSONL files
//! - Parquet analytics files
//! - State/cursor files

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Path not found: {0}")]
    PathNotFound(PathBuf),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Configuration for storage paths.
#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub data_dir: PathBuf,
}

impl StorageConfig {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn raw_dir(&self) -> PathBuf {
        self.data_dir.join("raw")
    }

    pub fn normalized_dir(&self) -> PathBuf {
        self.data_dir.join("normalized")
    }

    pub fn parquet_dir(&self) -> PathBuf {
        self.data_dir.join("parquet")
    }

    pub fn derived_dir(&self) -> PathBuf {
        self.data_dir.join("derived")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.data_dir.join("state")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn review_queue_dir(&self) -> PathBuf {
        self.data_dir.join("review_queue")
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self::new(PathBuf::from("./data"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_config_paths() {
        let config = StorageConfig::new(PathBuf::from("/data"));

        assert_eq!(config.raw_dir(), PathBuf::from("/data/raw"));
        assert_eq!(config.normalized_dir(), PathBuf::from("/data/normalized"));
        assert_eq!(config.parquet_dir(), PathBuf::from("/data/parquet"));
        assert_eq!(config.derived_dir(), PathBuf::from("/data/derived"));
        assert_eq!(config.state_dir(), PathBuf::from("/data/state"));
    }

    #[test]
    fn test_storage_config_default() {
        let config = StorageConfig::default();
        assert_eq!(config.data_dir, PathBuf::from("./data"));
    }
}
