//! # Meta Agent
//!
//! A local Warhammer 40k meta tracker with AI-powered extraction.
//!
//! ## Architecture
//!
//! - **models**: Core data structures (events, placements, epochs, etc.)
//! - **agents**: AI-powered extraction agents
//! - **storage**: Filesystem data lake operations (JSONL, Parquet)
//! - **api**: REST API endpoints
//! - **calculate**: Statistics and derived metrics computation
//! - **config**: Configuration loading and validation

pub mod agents;
pub mod api;
pub mod calculate;
pub mod config;
pub mod fetch;
pub mod ingest;
pub mod models;
pub mod storage;
pub mod sync;

pub use models::*;

use std::time::Duration;

/// Parse a human-friendly duration string (e.g., "6h", "30m", "90s").
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, multiplier) = if let Some(n) = s.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1)
    } else {
        // Default to seconds
        (s, 1)
    };

    let num: u64 = num_str.parse().ok()?;
    Some(Duration::from_secs(num * multiplier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("6h"), Some(Duration::from_secs(21600)));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
    }

    #[test]
    fn test_parse_duration_default_seconds() {
        assert_eq!(parse_duration("120"), Some(Duration::from_secs(120)));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration("abc"), None);
    }

    #[test]
    fn test_parse_duration_empty() {
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn test_parse_duration_zero() {
        assert_eq!(parse_duration("0s"), Some(Duration::from_secs(0)));
    }

    #[test]
    fn test_parse_duration_large() {
        assert_eq!(parse_duration("9999h"), Some(Duration::from_secs(35996400)));
    }
}
