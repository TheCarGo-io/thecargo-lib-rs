use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::json;
use sqlx::PgPool;

const DB_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

pub fn router<S>(db: PgPool) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new().route("/health/live", get(|| async { liveness() })).route(
        "/health",
        get(move || {
            let db = db.clone();
            async move { health(&db).await }
        }),
    )
}

pub fn liveness() -> Response {
    Json(json!({"status": "alive"})).into_response()
}

pub async fn health(db: &PgPool) -> Response {
    if database_answers(db).await {
        (StatusCode::OK, Json(json!({"status": "ok", "db": "ok"}))).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"status": "degraded", "db": "unavailable"}))).into_response()
    }
}

pub async fn database_answers(db: &PgPool) -> bool {
    match tokio::time::timeout(DB_CHECK_TIMEOUT, sqlx::query("SELECT 1").execute(db)).await {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "health check: database unavailable");
            false
        }
        Err(_) => {
            tracing::warn!("health check: database timed out");
            false
        }
    }
}
