//! The Rust counterpart of the shared Python package `thecargo` (thecargo-lib): authentication and
//! permissions, the error envelope and its translations, request extraction, the event consumer,
//! the `ClickHouse` client, observability and the dashboard shapers the services share.
pub mod audit;
pub mod cache;
#[cfg(feature = "clickhouse")]
pub mod clickhouse;
pub mod clients;
pub mod config;
pub mod context;
#[cfg(feature = "dashboard")]
pub mod dashboard;
pub mod db;
pub mod dependencies;
pub mod events;
pub mod extract;
pub mod fernet;
pub mod handlers;
pub mod health;
pub mod i18n;
pub mod jobs;
pub mod json;
pub mod lead_fields;
pub mod metrics;
pub mod middleware;
pub mod negotiation;
pub mod observability;
pub mod openapi;
pub mod runtime;
pub mod schemas;
pub mod storage;
pub mod upload;
pub mod utils;

pub use axum;
pub use chrono;
pub use chrono_tz;
pub use redis;
pub use reqwest;
pub use secrecy;
pub use serde_json;
pub use sqlx;
pub use tokio;
pub use tokio_util;
pub use tracing;
pub use utoipa;
pub use uuid;
