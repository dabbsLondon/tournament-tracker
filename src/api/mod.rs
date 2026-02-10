//! REST API endpoints.
//!
//! Axum-based HTTP API for querying tournament data,
//! epoch information, and derived analytics.

pub mod routes;
pub mod state;

use std::collections::HashSet;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use thiserror::Error;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::models::EpochMapper;
use crate::api::state::AppState;

/// Build the full application router.
pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/events", get(routes::events::list_events))
        .route("/api/events/:id", get(routes::events::get_event))
        .route("/api/meta/factions", get(routes::meta::faction_stats))
        .route("/api/meta/factions/:name", get(routes::meta::faction_detail))
        .route("/api/epochs", get(routes::epochs::list_epochs));

    Router::new()
        .merge(api)
        .fallback_service(ServeDir::new("static"))
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
            page_size: page_size.unwrap_or(50).clamp(1, 100),
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

        // Page size max is 100
        let p = Pagination::new(Some(1), Some(200));
        assert_eq!(p.page_size, 100);
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
            Item { id: "a".into(), val: 1 },
            Item { id: "b".into(), val: 2 },
            Item { id: "a".into(), val: 3 },
        ];
        let deduped = dedup_by_id(items, |i| &i.id);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].val, 1);
        assert_eq!(deduped[1].val, 2);
    }
}
