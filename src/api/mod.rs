//! REST API endpoints.
//!
//! Axum-based HTTP API for querying tournament data,
//! epoch information, and derived analytics.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use thiserror::Error;

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
}
