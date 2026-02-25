//! REST API endpoints.
//!
//! Axum-based HTTP API for querying tournament data,
//! epoch information, and derived analytics.

pub mod routes;
pub mod state;

use std::collections::HashSet;

use axum::{
    extract::ConnectInfo,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use thiserror::Error;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::api::state::AppState;
use crate::models::EpochMapper;

/// Build the full application router.
pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/events", get(routes::events::list_events))
        .route("/api/events/:id", get(routes::events::get_event))
        .route("/api/meta/factions", get(routes::meta::faction_stats))
        .route(
            "/api/meta/factions/:name",
            get(routes::meta::faction_detail),
        )
        .route("/api/meta/allegiances", get(routes::meta::allegiance_stats))
        .route("/api/epochs", get(routes::epochs::list_epochs))
        .route("/api/balance", get(routes::epochs::list_balance_passes))
        .route("/api/balance/:id", get(routes::epochs::get_balance_pass))
        .route("/api/analytics/overview", get(routes::analytics::overview))
        .route(
            "/api/analytics/trends",
            get(routes::analytics::faction_trends),
        )
        .route(
            "/api/analytics/players",
            get(routes::analytics::top_players),
        )
        .route("/api/analytics/units", get(routes::analytics::top_units))
        .route("/api/refresh/preview", get(routes::refresh::preview))
        .route("/api/refresh", post(routes::refresh::start_refresh))
        .route("/api/refresh/status", get(routes::refresh::status))
        .route(
            "/api/analytics/detachments",
            get(routes::analytics::detachment_stats),
        )
        .route(
            "/api/analytics/unit-performance",
            get(routes::analytics::unit_performance),
        )
        .route(
            "/api/analytics/points-efficiency",
            get(routes::analytics::points_efficiency),
        )
        .route("/api/analytics/matchups", get(routes::analytics::matchups))
        .route(
            "/api/analytics/archetypes",
            get(routes::analytics::archetypes),
        )
        .route(
            "/api/analytics/win-rates",
            get(routes::analytics::win_rates),
        )
        .route(
            "/api/analytics/composite-scores",
            get(routes::analytics::composite_scores),
        )
        .route("/api/traffic", get(routes::traffic::traffic_stats))
        .route("/api/traffic/geo", get(routes::traffic::geo_lookup));

    let traffic = state.traffic_stats.clone();

    Router::new()
        .merge(api)
        .fallback_service(ServeDir::new("static"))
        .layer(middleware::from_fn(
            move |req: axum::extract::Request, next: Next| {
                let stats = traffic.clone();
                async move {
                    // Try X-Forwarded-For / X-Real-IP first (reverse proxy),
                    // then ConnectInfo (direct connection)
                    let ip = req
                        .headers()
                        .get("x-forwarded-for")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
                        .or_else(|| {
                            req.headers()
                                .get("x-real-ip")
                                .and_then(|v| v.to_str().ok())
                                .map(|v| v.trim().to_string())
                        })
                        .or_else(|| {
                            req.extensions()
                                .get::<ConnectInfo<std::net::SocketAddr>>()
                                .map(|ci| ci.0.ip().to_string())
                        })
                        .unwrap_or_else(|| "unknown".to_string());
                    let path = req.uri().path().to_string();
                    {
                        let mut s = stats.write().await;
                        s.record(&ip, &path);
                    }
                    next.run(req).await
                }
            },
        ))
        .layer(CorsLayer::new().allow_origin(Any))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Deduplicate entities by their ID field.
/// Keeps the first occurrence of each ID.
pub fn dedup_by_id<T, F>(entities: Vec<T>, id_fn: F) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    let mut seen = HashSet::new();
    entities
        .into_iter()
        .filter(|e| seen.insert(id_fn(e).to_string()))
        .collect()
}

/// Resolve an epoch parameter to an epoch ID string.
///
/// - `None` or `"current"` resolves to the latest epoch from the mapper,
///   or `"current"` if the mapper is empty.
/// - A specific ID is validated against the mapper if non-empty.
pub fn resolve_epoch(param: Option<&str>, mapper: &EpochMapper) -> Result<String, ApiError> {
    match param {
        None | Some("current") | Some("") => {
            if mapper.all_epochs().is_empty() {
                Ok("current".to_string())
            } else {
                Ok(mapper
                    .current_epoch()
                    .map(|e| e.id.as_str().to_string())
                    .unwrap_or_else(|| "current".to_string()))
            }
        }
        Some(id) => {
            if !mapper.all_epochs().is_empty() {
                let eid = crate::models::EntityId::from(id);
                if mapper.get_epoch(&eid).is_none() {
                    return Err(ApiError::NotFound(format!("Unknown epoch: {}", id)));
                }
            }
            Ok(id.to_string())
        }
    }
}

/// API error types.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            ApiError::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            ApiError::Forbidden(_) => (StatusCode::FORBIDDEN, "FORBIDDEN"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };

        let body = ErrorResponse {
            error: ErrorDetail {
                code: code.to_string(),
                message: self.to_string(),
            },
        };

        (status, Json(body)).into_response()
    }
}

/// Pagination parameters.
#[derive(Debug, Clone)]
pub struct Pagination {
    pub page: u32,
    pub page_size: u32,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 50,
        }
    }
}

impl Pagination {
    pub fn new(page: Option<u32>, page_size: Option<u32>) -> Self {
        Self {
            page: page.unwrap_or(1).max(1),
            page_size: page_size.unwrap_or(50).clamp(1, 500),
        }
    }

    pub fn offset(&self) -> u32 {
        (self.page - 1) * self.page_size
    }
}

/// Pagination metadata in responses.
#[derive(Debug, Serialize)]
pub struct PaginationMeta {
    pub page: u32,
    pub page_size: u32,
    pub total_items: u32,
    pub total_pages: u32,
    pub has_next: bool,
    pub has_prev: bool,
}

impl PaginationMeta {
    pub fn new(pagination: &Pagination, total_items: u32) -> Self {
        let total_pages = total_items.div_ceil(pagination.page_size);
        Self {
            page: pagination.page,
            page_size: pagination.page_size,
            total_items,
            total_pages,
            has_next: pagination.page < total_pages,
            has_prev: pagination.page > 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pagination_default() {
        let p = Pagination::default();
        assert_eq!(p.page, 1);
        assert_eq!(p.page_size, 50);
        assert_eq!(p.offset(), 0);
    }

    #[test]
    fn test_pagination_new() {
        let p = Pagination::new(Some(3), Some(25));
        assert_eq!(p.page, 3);
        assert_eq!(p.page_size, 25);
        assert_eq!(p.offset(), 50);
    }

    #[test]
    fn test_pagination_bounds() {
        // Page can't be 0
        let p = Pagination::new(Some(0), Some(50));
        assert_eq!(p.page, 1);

        // Page size max is 500
        let p = Pagination::new(Some(1), Some(200));
        assert_eq!(p.page_size, 200);
    }

    #[test]
    fn test_pagination_meta() {
        let p = Pagination::new(Some(2), Some(10));
        let meta = PaginationMeta::new(&p, 25);

        assert_eq!(meta.page, 2);
        assert_eq!(meta.total_items, 25);
        assert_eq!(meta.total_pages, 3);
        assert!(meta.has_next);
        assert!(meta.has_prev);
    }

    #[test]
    fn test_pagination_meta_first_page() {
        let p = Pagination::new(Some(1), Some(10));
        let meta = PaginationMeta::new(&p, 25);

        assert!(!meta.has_prev);
        assert!(meta.has_next);
    }

    #[test]
    fn test_pagination_meta_last_page() {
        let p = Pagination::new(Some(3), Some(10));
        let meta = PaginationMeta::new(&p, 25);

        assert!(meta.has_prev);
        assert!(!meta.has_next);
    }

    #[test]
    fn test_dedup_by_id() {
        #[derive(Debug, Clone)]
        struct Item {
            id: String,
            val: i32,
        }
        let items = vec![
            Item {
                id: "a".into(),
                val: 1,
            },
            Item {
                id: "b".into(),
                val: 2,
            },
            Item {
                id: "a".into(),
                val: 3,
            },
        ];
        let deduped = dedup_by_id(items, |i| &i.id);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].val, 1);
        assert_eq!(deduped[1].val, 2);
    }

    #[test]
    fn test_resolve_epoch_none_empty_mapper() {
        let mapper = crate::models::EpochMapper::new();
        let result = resolve_epoch(None, &mapper).unwrap();
        assert_eq!(result, "current");
    }

    #[test]
    fn test_resolve_epoch_current_string() {
        let mapper = crate::models::EpochMapper::new();
        let result = resolve_epoch(Some("current"), &mapper).unwrap();
        assert_eq!(result, "current");
    }

    #[test]
    fn test_resolve_epoch_empty_string() {
        let mapper = crate::models::EpochMapper::new();
        let result = resolve_epoch(Some(""), &mapper).unwrap();
        assert_eq!(result, "current");
    }

    #[test]
    fn test_resolve_epoch_specific_id_empty_mapper() {
        let mapper = crate::models::EpochMapper::new();
        // With an empty mapper, any ID is accepted
        let result = resolve_epoch(Some("abc123"), &mapper).unwrap();
        assert_eq!(result, "abc123");
    }

    #[test]
    fn test_resolve_epoch_with_populated_mapper() {
        use crate::models::{SignificantEvent, SignificantEventType};
        let event = SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            chrono::NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            "Test Balance".to_string(),
            "https://example.com".to_string(),
        );
        let mapper = crate::models::EpochMapper::from_significant_events(&[event]);

        // None should resolve to the current epoch
        let result = resolve_epoch(None, &mapper).unwrap();
        assert!(!result.is_empty());

        // Invalid ID should fail
        let err = resolve_epoch(Some("nonexistent"), &mapper);
        assert!(err.is_err());
    }

    #[test]
    fn test_api_error_not_found() {
        use axum::response::IntoResponse;
        let error = ApiError::NotFound("test".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn test_api_error_bad_request() {
        use axum::response::IntoResponse;
        let error = ApiError::BadRequest("bad".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_api_error_internal() {
        use axum::response::IntoResponse;
        let error = ApiError::Internal("oops".to_string());
        let response = error.into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_pagination_meta_single_page() {
        let p = Pagination::new(Some(1), Some(50));
        let meta = PaginationMeta::new(&p, 10);
        assert_eq!(meta.total_pages, 1);
        assert!(!meta.has_next);
        assert!(!meta.has_prev);
    }

    #[test]
    fn test_pagination_meta_zero_items() {
        let p = Pagination::new(Some(1), Some(10));
        let meta = PaginationMeta::new(&p, 0);
        assert_eq!(meta.total_pages, 0);
        assert!(!meta.has_next);
        assert!(!meta.has_prev);
    }

    #[test]
    fn test_dedup_by_id_empty() {
        let items: Vec<String> = vec![];
        let deduped = dedup_by_id(items, |s| s.as_str());
        assert!(deduped.is_empty());
    }

    #[test]
    fn test_api_error_conflict() {
        use axum::response::IntoResponse;
        let error = ApiError::Conflict("already exists".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    }

    #[test]
    fn test_api_error_forbidden() {
        use axum::response::IntoResponse;
        let error = ApiError::Forbidden("not allowed".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    }
}
