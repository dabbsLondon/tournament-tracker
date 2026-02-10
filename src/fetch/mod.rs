//! HTTP fetching with caching.
//!
//! Fetches raw content (HTML, PDFs) from URLs and caches them locally.
//! All fetched content is stored in the raw data directory for re-processing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};
use url::Url;

/// Errors that can occur during fetching.
#[derive(Debug, Error)]
pub enum FetchError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Rate limited by {host}, retry after {retry_after_secs}s")]
    RateLimited { host: String, retry_after_secs: u64 },

    #[error("HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },

    #[error("Content too large: {size} bytes (max {max_size})")]
    ContentTooLarge { size: usize, max_size: usize },

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result of a fetch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    /// Original URL that was fetched
    pub url: Url,

    /// Path where content is cached
    pub cache_path: PathBuf,

    /// Content type (e.g., "text/html", "application/pdf")
    pub content_type: Option<String>,

    /// Content length in bytes
    pub content_length: usize,

    /// When the content was fetched
    pub fetched_at: DateTime<Utc>,

    /// Whether this was served from cache
    pub from_cache: bool,

    /// ETag if provided by server
    pub etag: Option<String>,

    /// Last-Modified header if provided
    pub last_modified: Option<String>,
}

/// Metadata stored alongside cached content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    pub url: String,
    pub fetched_at: DateTime<Utc>,
    pub content_type: Option<String>,
    pub content_length: usize,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Configuration for the HTTP fetcher.
#[derive(Debug, Clone)]
pub struct FetcherConfig {
    /// Directory to cache raw content
    pub cache_dir: PathBuf,

    /// How long cached content is considered fresh
    pub cache_ttl: Duration,

    /// Maximum content size to fetch (default 50MB)
    pub max_content_size: usize,

    /// Request timeout
    pub timeout: Duration,

    /// User agent string
    pub user_agent: String,

    /// Delay between requests to same host (rate limiting)
    pub request_delay: Duration,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        Self {
            cache_dir: PathBuf::from("./data/raw"),
            cache_ttl: Duration::from_secs(3600), // 1 hour
            max_content_size: 50 * 1024 * 1024,   // 50MB
            timeout: Duration::from_secs(30),
            user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string(),
            request_delay: Duration::from_millis(500),
        }
    }
}

/// HTTP fetcher with local caching.
pub struct Fetcher {
    client: Client,
    config: FetcherConfig,
}

impl Fetcher {
    /// Create a new fetcher with the given configuration.
    pub fn new(config: FetcherConfig) -> Result<Self, FetchError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&config.user_agent)
                .unwrap_or_else(|_| HeaderValue::from_static("meta-agent/0.1.0")),
        );

        let client = Client::builder()
            .timeout(config.timeout)
            .default_headers(headers)
            .build()?;

        Ok(Self { client, config })
    }

    /// Create a fetcher with default configuration.
    pub fn with_defaults() -> Result<Self, FetchError> {
        Self::new(FetcherConfig::default())
    }

    /// Fetch a URL, using cache if available and fresh.
    pub async fn fetch(&self, url: &Url) -> Result<FetchResult, FetchError> {
        let cache_path = self.cache_path_for_url(url);
        let meta_path = self.meta_path_for_url(url);

        // Check cache
        if let Some(result) = self.check_cache(url, &cache_path, &meta_path).await? {
            return Ok(result);
        }

        // Fetch from network
        self.fetch_and_cache(url, &cache_path, &meta_path).await
    }

    /// Force fetch from network, ignoring cache.
    pub async fn fetch_fresh(&self, url: &Url) -> Result<FetchResult, FetchError> {
        let cache_path = self.cache_path_for_url(url);
        let meta_path = self.meta_path_for_url(url);
        self.fetch_and_cache(url, &cache_path, &meta_path).await
    }

    /// Get content from cache without network fallback.
    pub async fn get_cached(&self, url: &Url) -> Option<FetchResult> {
        let cache_path = self.cache_path_for_url(url);
        let meta_path = self.meta_path_for_url(url);
        self.check_cache(url, &cache_path, &meta_path)
            .await
            .ok()
            .flatten()
    }

    /// Check if content is cached and fresh.
    async fn check_cache(
        &self,
        url: &Url,
        cache_path: &Path,
        meta_path: &Path,
    ) -> Result<Option<FetchResult>, FetchError> {
        if !cache_path.exists() || !meta_path.exists() {
            return Ok(None);
        }

        let meta_content = fs::read_to_string(meta_path).await?;
        let meta: CacheMetadata = match serde_json::from_str(&meta_content) {
            Ok(m) => m,
            Err(_) => return Ok(None),
        };

        // Check if cache has expired
        let age = Utc::now().signed_duration_since(meta.fetched_at);
        if age.num_seconds() > self.config.cache_ttl.as_secs() as i64 {
            debug!("Cache expired for {}", url);
            return Ok(None);
        }

        info!("Serving {} from cache", url);
        Ok(Some(FetchResult {
            url: url.clone(),
            cache_path: cache_path.to_path_buf(),
            content_type: meta.content_type,
            content_length: meta.content_length,
            fetched_at: meta.fetched_at,
            from_cache: true,
            etag: meta.etag,
            last_modified: meta.last_modified,
        }))
    }

    /// Fetch from network and cache the result.
    async fn fetch_and_cache(
        &self,
        url: &Url,
        cache_path: &Path,
        meta_path: &Path,
    ) -> Result<FetchResult, FetchError> {
        info!("Fetching {}", url);

        let response = self.client.get(url.as_str()).send().await?;

        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);

            return Err(FetchError::RateLimited {
                host: url.host_str().unwrap_or("unknown").to_string(),
                retry_after_secs: retry_after,
            });
        }

        if !status.is_success() {
            return Err(FetchError::HttpStatus {
                status: status.as_u16(),
                message: status.canonical_reason().unwrap_or("Unknown").to_string(),
            });
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let content = response.bytes().await?;

        if content.len() > self.config.max_content_size {
            return Err(FetchError::ContentTooLarge {
                size: content.len(),
                max_size: self.config.max_content_size,
            });
        }

        // Ensure cache directory exists
        if let Some(parent) = cache_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Write content
        let mut file = fs::File::create(cache_path).await?;
        file.write_all(&content).await?;
        file.flush().await?;

        let fetched_at = Utc::now();

        // Write metadata
        let meta = CacheMetadata {
            url: url.to_string(),
            fetched_at,
            content_type: content_type.clone(),
            content_length: content.len(),
            etag: etag.clone(),
            last_modified: last_modified.clone(),
            expires_at: Some(
                fetched_at + chrono::Duration::seconds(self.config.cache_ttl.as_secs() as i64),
            ),
        };

        let meta_json = serde_json::to_string_pretty(&meta)?;
        fs::write(meta_path, meta_json).await?;

        Ok(FetchResult {
            url: url.clone(),
            cache_path: cache_path.to_path_buf(),
            content_type,
            content_length: content.len(),
            fetched_at,
            from_cache: false,
            etag,
            last_modified,
        })
    }

    /// Generate a cache path for a URL.
    fn cache_path_for_url(&self, url: &Url) -> PathBuf {
        let hash = Self::url_hash(url);
        let host = url.host_str().unwrap_or("unknown");
        let extension = Self::extension_for_url(url);

        self.config
            .cache_dir
            .join(host)
            .join(format!("{}.{}", hash, extension))
    }

    /// Generate a metadata path for a URL.
    fn meta_path_for_url(&self, url: &Url) -> PathBuf {
        let hash = Self::url_hash(url);
        let host = url.host_str().unwrap_or("unknown");

        self.config
            .cache_dir
            .join(host)
            .join(format!("{}.meta.json", hash))
    }

    /// Hash a URL to a short string.
    fn url_hash(url: &Url) -> String {
        let mut hasher = Sha256::new();
        hasher.update(url.as_str().as_bytes());
        let result = hasher.finalize();
        hex::encode(&result[..8])
    }

    /// Determine file extension from URL or content type.
    fn extension_for_url(url: &Url) -> &'static str {
        let path = url.path().to_lowercase();
        if path.ends_with(".pdf") {
            "pdf"
        } else if path.ends_with(".json") {
            "json"
        } else if path.ends_with(".xml") {
            "xml"
        } else {
            "html"
        }
    }

    /// Read cached content as string (for HTML/text).
    pub async fn read_cached_text(&self, result: &FetchResult) -> Result<String, FetchError> {
        Ok(fs::read_to_string(&result.cache_path).await?)
    }

    /// Read cached content as bytes (for PDFs/binary).
    pub async fn read_cached_bytes(&self, result: &FetchResult) -> Result<Vec<u8>, FetchError> {
        Ok(fs::read(&result.cache_path).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(temp_dir: &TempDir) -> FetcherConfig {
        FetcherConfig {
            cache_dir: temp_dir.path().to_path_buf(),
            cache_ttl: Duration::from_secs(3600),
            max_content_size: 1024 * 1024,
            timeout: Duration::from_secs(10),
            user_agent: "test-agent".to_string(),
            request_delay: Duration::from_millis(0),
        }
    }

    #[test]
    fn test_url_hash() {
        let url1 = Url::parse("https://example.com/page1").unwrap();
        let url2 = Url::parse("https://example.com/page2").unwrap();

        let hash1 = Fetcher::url_hash(&url1);
        let hash2 = Fetcher::url_hash(&url2);

        assert_ne!(hash1, hash2);
        assert_eq!(hash1.len(), 16); // 8 bytes = 16 hex chars
    }

    #[test]
    fn test_extension_for_url() {
        assert_eq!(
            Fetcher::extension_for_url(&Url::parse("https://example.com/doc.pdf").unwrap()),
            "pdf"
        );
        assert_eq!(
            Fetcher::extension_for_url(&Url::parse("https://example.com/page").unwrap()),
            "html"
        );
        assert_eq!(
            Fetcher::extension_for_url(&Url::parse("https://example.com/data.json").unwrap()),
            "json"
        );
    }

    #[test]
    fn test_cache_path_generation() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let fetcher = Fetcher::new(config).unwrap();

        let url = Url::parse("https://goonhammer.com/article").unwrap();
        let cache_path = fetcher.cache_path_for_url(&url);

        assert!(cache_path.starts_with(temp_dir.path()));
        assert!(cache_path.to_string_lossy().contains("goonhammer.com"));
        assert!(cache_path.to_string_lossy().ends_with(".html"));
    }

    #[tokio::test]
    async fn test_cache_metadata_serialization() {
        let meta = CacheMetadata {
            url: "https://example.com".to_string(),
            fetched_at: Utc::now(),
            content_type: Some("text/html".to_string()),
            content_length: 1234,
            etag: Some("abc123".to_string()),
            last_modified: None,
            expires_at: None,
        };

        let json = serde_json::to_string(&meta).unwrap();
        let parsed: CacheMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.url, meta.url);
        assert_eq!(parsed.content_length, meta.content_length);
    }

    #[test]
    fn test_fetcher_config_default() {
        let config = FetcherConfig::default();

        assert_eq!(config.cache_dir, PathBuf::from("./data/raw"));
        assert_eq!(config.cache_ttl, Duration::from_secs(3600));
        assert!(config.user_agent.contains("Mozilla"));
    }
}
