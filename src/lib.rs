//! The Rust counterpart of the shared Python package `thecargo` (thecargo-lib): authentication and
//! permissions, the error envelope and its translations, request extraction, the event consumer,
//! the `ClickHouse` client, observability and the dashboard shapers the services share.
pub mod cache;
pub mod clickhouse;
pub mod context;
pub mod dashboard;
pub mod dependencies;
pub mod events;
pub mod extract;
pub mod handlers;
pub mod i18n;
pub mod json;
pub mod metrics;
pub mod middleware;
pub mod observability;
pub mod schemas;
pub mod utils;
