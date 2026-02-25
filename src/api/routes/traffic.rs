use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::api::state::AppState;
use crate::api::ApiError;

// ── Time-series bucket ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TimeBucket {
    pub minute: DateTime<Utc>,
    pub total: u64,
    pub page_views: u64,
    pub api_requests: u64,
}

// ── Geo cache entry ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeoInfo {
    pub country: String,
    pub city: String,
    pub country_code: String,
}

// ── TrafficStats ────────────────────────────────────────────────

/// In-memory traffic stats, reset on server restart.
#[derive(Debug, Clone)]
pub struct TrafficStats {
    /// Total requests since server start
    pub total_requests: u64,
    /// Requests per unique IP
    pub requests_by_ip: HashMap<String, u64>,
    /// Requests per path prefix (e.g. "/api/analytics", "/api/events")
    pub requests_by_path: HashMap<String, u64>,
    /// Page views (non-API, non-static-asset requests)
    pub page_views: u64,
    /// When the server started (stats began)
    pub started_at: DateTime<Utc>,
    /// Per-minute request counts (last 24 hours)
    pub time_series: VecDeque<TimeBucket>,
    /// Cached geo lookups for IPs
    pub geo_cache: HashMap<String, GeoInfo>,
}

impl Default for TrafficStats {
    fn default() -> Self {
        Self {
            total_requests: 0,
            requests_by_ip: HashMap::new(),
            requests_by_path: HashMap::new(),
            page_views: 0,
            started_at: Utc::now(),
            time_series: VecDeque::with_capacity(1440),
            geo_cache: HashMap::new(),
        }
    }
}

impl TrafficStats {
    pub fn new() -> Self {
        Self {
            started_at: Utc::now(),
            ..Default::default()
        }
    }

    pub fn record(&mut self, ip: &str, path: &str) {
        self.total_requests += 1;
        *self.requests_by_ip.entry(ip.to_string()).or_insert(0) += 1;

        // Bucket by path prefix
        let bucket = if let Some(rest) = path.strip_prefix("/api/") {
            // Group to second path segment: /api/analytics/*, /api/events/*, etc.
            match rest.find('/') {
                Some(i) => &path[..5 + i],
                None => path,
            }
        } else if path.starts_with("/css/") || path.starts_with("/js/") || path.contains('.') {
            "static"
        } else {
            "page_view"
        };

        *self.requests_by_path.entry(bucket.to_string()).or_insert(0) += 1;

        let is_page = bucket == "page_view";
        let is_api = bucket != "page_view" && bucket != "static";

        if is_page {
            self.page_views += 1;
        }

        // Time-series tracking: bucket by minute
        let now = Utc::now();
        let current_minute = now.with_second(0).unwrap().with_nanosecond(0).unwrap();

        if let Some(last) = self.time_series.back_mut() {
            if last.minute == current_minute {
                last.total += 1;
                if is_page {
                    last.page_views += 1;
                }
                if is_api {
                    last.api_requests += 1;
                }
            } else {
                self.time_series.push_back(TimeBucket {
                    minute: current_minute,
                    total: 1,
                    page_views: if is_page { 1 } else { 0 },
                    api_requests: if is_api { 1 } else { 0 },
                });
            }
        } else {
            self.time_series.push_back(TimeBucket {
                minute: current_minute,
                total: 1,
                page_views: if is_page { 1 } else { 0 },
                api_requests: if is_api { 1 } else { 0 },
            });
        }

        // Evict buckets older than 24 hours
        let cutoff = now - chrono::Duration::hours(24);
        while self.time_series.front().is_some_and(|b| b.minute < cutoff) {
            self.time_series.pop_front();
        }
    }

    pub fn unique_ips(&self) -> usize {
        self.requests_by_ip.len()
    }

    /// External visitors (all IPs except 127.0.0.1 and ::1)
    pub fn external_ips(&self) -> Vec<(String, u64)> {
        self.requests_by_ip
            .iter()
            .filter(|(ip, _)| *ip != "127.0.0.1" && *ip != "::1")
            .map(|(ip, count)| (ip.clone(), *count))
            .collect()
    }
}

pub type SharedTrafficStats = Arc<RwLock<TrafficStats>>;

// ── Response types ──────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct TrafficResponse {
    pub uptime_seconds: i64,
    pub total_requests: u64,
    pub page_views: u64,
    pub unique_visitors: usize,
    pub external_visitors: usize,
    pub external_ips: Vec<IpSummary>,
    pub paths: Vec<PathSummary>,
    pub started_at: String,
    pub time_series: Vec<TimeSeriesPoint>,
}

#[derive(Debug, Serialize)]
pub struct TimeSeriesPoint {
    pub time: String,
    pub total: u64,
    pub page_views: u64,
    pub api_requests: u64,
}

#[derive(Debug, Serialize)]
pub struct IpSummary {
    pub ip: String,
    pub requests: u64,
}

#[derive(Debug, Serialize)]
pub struct PathSummary {
    pub path: String,
    pub requests: u64,
}

// ── Geo lookup types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct IpApiResponse {
    status: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    city: String,
    #[serde(default, rename = "countryCode")]
    country_code: String,
}

#[derive(Debug, Serialize)]
pub struct GeoResult {
    pub ip: String,
    pub country: String,
    pub city: String,
    pub country_code: String,
}

#[derive(Debug, Deserialize)]
pub struct GeoQuery {
    pub ips: String,
}

// ── Handlers ────────────────────────────────────────────────────

pub async fn traffic_stats(State(state): State<AppState>) -> Json<TrafficResponse> {
    let stats = state.traffic_stats.read().await;
    let now = Utc::now();
    let uptime = (now - stats.started_at).num_seconds();

    let mut external = stats.external_ips();
    external.sort_by(|a, b| b.1.cmp(&a.1));

    let mut paths: Vec<PathSummary> = stats
        .requests_by_path
        .iter()
        .map(|(p, c)| PathSummary {
            path: p.clone(),
            requests: *c,
        })
        .collect();
    paths.sort_by(|a, b| b.requests.cmp(&a.requests));

    let time_series: Vec<TimeSeriesPoint> = stats
        .time_series
        .iter()
        .map(|b| TimeSeriesPoint {
            time: b.minute.to_rfc3339(),
            total: b.total,
            page_views: b.page_views,
            api_requests: b.api_requests,
        })
        .collect();

    Json(TrafficResponse {
        uptime_seconds: uptime,
        total_requests: stats.total_requests,
        page_views: stats.page_views,
        unique_visitors: stats.unique_ips(),
        external_visitors: external.len(),
        external_ips: external
            .into_iter()
            .map(|(ip, requests)| IpSummary { ip, requests })
            .collect(),
        paths,
        started_at: stats.started_at.to_rfc3339(),
        time_series,
    })
}

/// Reject requests that come through Cloudflare Tunnel (public domain).
fn require_local(headers: &HeaderMap) -> Result<(), ApiError> {
    if headers.contains_key("cf-connecting-ip") {
        return Err(ApiError::Forbidden(
            "Geo lookup is only available on localhost".to_string(),
        ));
    }
    Ok(())
}

pub async fn geo_lookup(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(params): Query<GeoQuery>,
) -> Result<Json<Vec<GeoResult>>, ApiError> {
    require_local(&headers)?;

    let ips: Vec<String> = params
        .ips
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(20) // limit batch size
        .collect();

    if ips.is_empty() {
        return Ok(Json(vec![]));
    }

    // Check cache first
    let mut results = Vec::new();
    let mut uncached = Vec::new();

    {
        let stats = state.traffic_stats.read().await;
        for ip in &ips {
            if let Some(geo) = stats.geo_cache.get(ip) {
                results.push(GeoResult {
                    ip: ip.clone(),
                    country: geo.country.clone(),
                    city: geo.city.clone(),
                    country_code: geo.country_code.clone(),
                });
            } else {
                uncached.push(ip.clone());
            }
        }
    }

    // Fetch uncached IPs from ip-api.com
    if !uncached.is_empty() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();

        let mut new_entries = Vec::new();

        for ip in &uncached {
            let url = format!(
                "http://ip-api.com/json/{}?fields=status,country,city,countryCode",
                ip
            );
            match client.get(&url).send().await {
                Ok(resp) => {
                    if let Ok(data) = resp.json::<IpApiResponse>().await {
                        let geo = if data.status == "success" {
                            GeoInfo {
                                country: data.country,
                                city: data.city,
                                country_code: data.country_code,
                            }
                        } else {
                            GeoInfo::default()
                        };
                        results.push(GeoResult {
                            ip: ip.clone(),
                            country: geo.country.clone(),
                            city: geo.city.clone(),
                            country_code: geo.country_code.clone(),
                        });
                        new_entries.push((ip.clone(), geo));
                    }
                }
                Err(_) => {
                    // Skip failed lookups silently
                }
            }
        }

        // Update cache
        if !new_entries.is_empty() {
            let mut stats = state.traffic_stats.write().await;
            for (ip, geo) in new_entries {
                stats.geo_cache.insert(ip, geo);
            }
        }
    }

    Ok(Json(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_traffic_stats_new() {
        let stats = TrafficStats::new();
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.page_views, 0);
        assert!(stats.requests_by_ip.is_empty());
        assert!(stats.requests_by_path.is_empty());
        assert!(stats.time_series.is_empty());
        assert!(stats.geo_cache.is_empty());
    }

    #[test]
    fn test_record_increments_total() {
        let mut stats = TrafficStats::new();
        stats.record("127.0.0.1", "/");
        assert_eq!(stats.total_requests, 1);
        stats.record("127.0.0.1", "/");
        assert_eq!(stats.total_requests, 2);
    }

    #[test]
    fn test_record_tracks_ip() {
        let mut stats = TrafficStats::new();
        stats.record("1.2.3.4", "/");
        stats.record("1.2.3.4", "/about");
        stats.record("5.6.7.8", "/");
        assert_eq!(stats.requests_by_ip.get("1.2.3.4"), Some(&2));
        assert_eq!(stats.requests_by_ip.get("5.6.7.8"), Some(&1));
    }

    #[test]
    fn test_record_page_view_bucketing() {
        let mut stats = TrafficStats::new();
        stats.record("127.0.0.1", "/");
        stats.record("127.0.0.1", "/about");
        assert_eq!(stats.page_views, 2);
        assert_eq!(stats.requests_by_path.get("page_view"), Some(&2));
    }

    #[test]
    fn test_record_api_bucketing() {
        let mut stats = TrafficStats::new();
        stats.record("127.0.0.1", "/api/events");
        stats.record("127.0.0.1", "/api/events/123");
        stats.record("127.0.0.1", "/api/analytics/overview");
        assert_eq!(stats.page_views, 0);
        assert_eq!(stats.requests_by_path.get("/api/events"), Some(&2));
        assert_eq!(stats.requests_by_path.get("/api/analytics"), Some(&1));
    }

    #[test]
    fn test_record_static_bucketing() {
        let mut stats = TrafficStats::new();
        stats.record("127.0.0.1", "/css/style.css");
        stats.record("127.0.0.1", "/js/app.js");
        stats.record("127.0.0.1", "/favicon.ico");
        assert_eq!(stats.page_views, 0);
        assert_eq!(stats.requests_by_path.get("static"), Some(&3));
    }

    #[test]
    fn test_unique_ips() {
        let mut stats = TrafficStats::new();
        assert_eq!(stats.unique_ips(), 0);
        stats.record("1.2.3.4", "/");
        stats.record("1.2.3.4", "/");
        stats.record("5.6.7.8", "/");
        assert_eq!(stats.unique_ips(), 2);
    }

    #[test]
    fn test_external_ips_excludes_localhost() {
        let mut stats = TrafficStats::new();
        stats.record("127.0.0.1", "/");
        stats.record("::1", "/");
        stats.record("1.2.3.4", "/");
        stats.record("5.6.7.8", "/");
        let external = stats.external_ips();
        assert_eq!(external.len(), 2);
        let ips: Vec<&str> = external.iter().map(|(ip, _)| ip.as_str()).collect();
        assert!(ips.contains(&"1.2.3.4"));
        assert!(ips.contains(&"5.6.7.8"));
        assert!(!ips.contains(&"127.0.0.1"));
        assert!(!ips.contains(&"::1"));
    }

    #[test]
    fn test_external_ips_empty() {
        let mut stats = TrafficStats::new();
        stats.record("127.0.0.1", "/");
        assert!(stats.external_ips().is_empty());
    }

    #[test]
    fn test_time_series_created_on_record() {
        let mut stats = TrafficStats::new();
        assert!(stats.time_series.is_empty());
        stats.record("127.0.0.1", "/");
        assert_eq!(stats.time_series.len(), 1);
        assert_eq!(stats.time_series[0].total, 1);
    }

    #[test]
    fn test_time_series_same_minute_aggregates() {
        let mut stats = TrafficStats::new();
        stats.record("127.0.0.1", "/");
        stats.record("127.0.0.1", "/about");
        stats.record("127.0.0.1", "/api/events");
        // All within the same second, so same minute bucket
        assert_eq!(stats.time_series.len(), 1);
        assert_eq!(stats.time_series[0].total, 3);
        assert_eq!(stats.time_series[0].page_views, 2);
        assert_eq!(stats.time_series[0].api_requests, 1);
    }

    #[test]
    fn test_time_series_eviction() {
        let mut stats = TrafficStats::new();
        // Manually insert an old bucket
        let old_time = Utc::now() - chrono::Duration::hours(25);
        stats.time_series.push_back(TimeBucket {
            minute: old_time,
            total: 10,
            page_views: 5,
            api_requests: 5,
        });
        assert_eq!(stats.time_series.len(), 1);
        // Recording a new request should evict the old bucket
        stats.record("127.0.0.1", "/");
        // Old bucket should be evicted, new one added
        assert_eq!(stats.time_series.len(), 1);
        assert_eq!(stats.time_series[0].total, 1); // only the new request
    }

    #[test]
    fn test_geo_info_default() {
        let geo = GeoInfo::default();
        assert_eq!(geo.country, "");
        assert_eq!(geo.city, "");
        assert_eq!(geo.country_code, "");
    }

    #[test]
    fn test_geo_info_serialization() {
        let geo = GeoInfo {
            country: "United Kingdom".to_string(),
            city: "London".to_string(),
            country_code: "GB".to_string(),
        };
        let json = serde_json::to_string(&geo).unwrap();
        let parsed: GeoInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.country, "United Kingdom");
        assert_eq!(parsed.city, "London");
        assert_eq!(parsed.country_code, "GB");
    }

    #[test]
    fn test_geo_cache_stores_entries() {
        let mut stats = TrafficStats::new();
        stats.geo_cache.insert(
            "1.2.3.4".to_string(),
            GeoInfo {
                country: "Germany".to_string(),
                city: "Berlin".to_string(),
                country_code: "DE".to_string(),
            },
        );
        assert_eq!(stats.geo_cache.len(), 1);
        let entry = stats.geo_cache.get("1.2.3.4").unwrap();
        assert_eq!(entry.country, "Germany");
    }

    #[test]
    fn test_require_local_allows_without_cf_header() {
        let headers = HeaderMap::new();
        assert!(require_local(&headers).is_ok());
    }

    #[test]
    fn test_require_local_blocks_cf_header() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", "1.2.3.4".parse().unwrap());
        let result = require_local(&headers);
        assert!(result.is_err());
    }

    #[test]
    fn test_traffic_response_serialization() {
        let resp = TrafficResponse {
            uptime_seconds: 3600,
            total_requests: 100,
            page_views: 50,
            unique_visitors: 5,
            external_visitors: 3,
            external_ips: vec![IpSummary {
                ip: "1.2.3.4".to_string(),
                requests: 10,
            }],
            paths: vec![PathSummary {
                path: "/api/events".to_string(),
                requests: 20,
            }],
            started_at: "2025-01-01T00:00:00Z".to_string(),
            time_series: vec![TimeSeriesPoint {
                time: "2025-01-01T00:00:00Z".to_string(),
                total: 5,
                page_views: 3,
                api_requests: 2,
            }],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["total_requests"], 100);
        assert_eq!(json["time_series"][0]["total"], 5);
        assert_eq!(json["external_ips"][0]["ip"], "1.2.3.4");
    }

    #[test]
    fn test_record_mixed_traffic() {
        let mut stats = TrafficStats::new();
        // Mix of page views, API calls, and static assets
        stats.record("1.2.3.4", "/");
        stats.record("1.2.3.4", "/api/events");
        stats.record("1.2.3.4", "/css/style.css");
        stats.record("5.6.7.8", "/about");
        stats.record("127.0.0.1", "/api/analytics/overview");

        assert_eq!(stats.total_requests, 5);
        assert_eq!(stats.page_views, 2); // "/" and "/about"
        assert_eq!(stats.unique_ips(), 3);
        assert_eq!(stats.external_ips().len(), 2); // excludes 127.0.0.1
    }
}
