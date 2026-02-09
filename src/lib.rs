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
