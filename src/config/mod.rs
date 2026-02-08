//! Configuration loading and validation.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Configuration errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(#[from] std::io::Error),

    #[error("Failed to parse config: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("Invalid configuration: {0}")]
    ValidationError(String),
}

/// AI backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// Backend type: "ollama" or "openai" or "anthropic"
    #[serde(default = "default_backend")]
    pub backend: String,

    /// Base URL for the AI service
    #[serde(default = "default_base_url")]
    pub base_url: String,

    /// Model to use
    #[serde(default = "default_model")]
    pub model: String,

    /// Timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Max retries
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_backend() -> String {
    "ollama".to_string()
}

fn default_base_url() -> String {
    "http://localhost:11434".to_string()
}

fn default_model() -> String {
    "llama3.2".to_string()
}

fn default_timeout() -> u64 {
    120
}

fn default_max_retries() -> u32 {
    3
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            base_url: default_base_url(),
            model: default_model(),
            timeout_seconds: default_timeout(),
            max_retries: default_max_retries(),
        }
    }
}

/// Source configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub enabled: bool,
    pub base_url: String,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_ms: u64,
}

fn default_rate_limit() -> u64 {
    2000
}

/// Server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_cors_origin")]
    pub cors_origin: String,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_cors_origin() -> String {
    "*".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            cors_origin: default_cors_origin(),
        }
    }
}

/// Main application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default)]
    pub ai: AiConfig,

    #[serde(default)]
    pub server: ServerConfig,
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            log_level: default_log_level(),
            ai: AiConfig::default(),
            server: ServerConfig::default(),
        }
    }
}

impl AppConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &PathBuf) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let config: AppConfig = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.ai.timeout_seconds == 0 {
            return Err(ConfigError::ValidationError(
                "AI timeout must be greater than 0".to_string(),
            ));
        }

        if self.server.port == 0 {
            return Err(ConfigError::ValidationError(
                "Server port must be greater than 0".to_string(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();

        assert_eq!(config.data_dir, PathBuf::from("./data"));
        assert_eq!(config.log_level, "info");
        assert_eq!(config.ai.backend, "ollama");
        assert_eq!(config.server.port, 8080);
    }

    #[test]
    fn test_ai_config_default() {
        let ai = AiConfig::default();

        assert_eq!(ai.backend, "ollama");
        assert_eq!(ai.base_url, "http://localhost:11434");
        assert_eq!(ai.model, "llama3.2");
        assert_eq!(ai.timeout_seconds, 120);
    }

    #[test]
    fn test_config_validation_ok() {
        let config = AppConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_bad_timeout() {
        let mut config = AppConfig::default();
        config.ai.timeout_seconds = 0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_bad_port() {
        let mut config = AppConfig::default();
        config.server.port = 0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_serialization() {
        let config = AppConfig::default();
        let toml_str = toml::to_string(&config).unwrap();

        // Should be parseable
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config.data_dir, parsed.data_dir);
    }
}
