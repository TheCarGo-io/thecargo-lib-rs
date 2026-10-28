use std::sync::OnceLock;
use std::time::Instant;

use axum::Json;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use serde_json::json;
use sqlx::PgPool;

pub const UNMATCHED_ROUTE: &str = "unmatched";
const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const PROXY_HEADERS: [&str; 3] = ["x-forwarded-for", "x-real-ip", "forwarded"];
const DURATION_BUCKETS: [f64; 13] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.2, 0.3, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0];

static HANDLE: OnceLock<Option<PrometheusHandle>> = OnceLock::new();

// The recorder is process-wide; a second install (tests build several apps) reuses the first.
pub fn install() -> Option<PrometheusHandle> {
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .set_buckets_for_metric(Matcher::Full("http_request_duration_seconds".to_owned()), &DURATION_BUCKETS)
                .and_then(PrometheusBuilder::install_recorder)
                .inspect_err(|err| tracing::warn!(error = %err, "prometheus recorder not installed"))
                .ok()
        })
        .clone()
}

#[derive(Debug, Clone)]
pub struct Pools {
    pools: Vec<(&'static str, PgPool)>,
}

impl Pools {
    pub fn single(main: PgPool) -> Self {
        Self::named(vec![("main", main)])
    }

    pub fn named(pools: Vec<(&'static str, PgPool)>) -> Self {
        Self { pools }
    }

    fn refresh(&self) {
        for (name, pool) in &self.pools {
            let size = pool.size();
            let idle = u32::try_from(pool.num_idle()).unwrap_or(u32::MAX);
            metrics::gauge!("db_pool_size", "pool" => *name).set(f64::from(pool.options().get_max_connections()));
            metrics::gauge!("db_pool_checked_out", "pool" => *name).set(f64::from(size.saturating_sub(idle)));
            metrics::gauge!("db_pool_overflow", "pool" => *name).set(0.0);
        }
    }
}

pub async fn track(request: Request, next: Next) -> Response {
    let method = request.method().as_str();
    let method = if METHODS.contains(&method) { method.to_owned() } else { "OTHER".to_owned() };
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| UNMATCHED_ROUTE.to_owned(), |path| path.as_str().to_owned());

    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();
    metrics::counter!("http_requests_total", "method" => method.clone(), "route" => route.clone(), "status" => status.clone())
        .increment(1);
    metrics::histogram!("http_request_duration_seconds", "method" => method, "route" => route, "status" => status)
        .record(started.elapsed().as_secs_f64());
    response
}

#[expect(clippy::unused_async, reason = "an axum handler")]
pub async fn endpoint(State(pools): State<Pools>, headers: HeaderMap) -> Response {
    if PROXY_HEADERS.iter().any(|name| headers.contains_key(*name)) {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "Not Found"}))).into_response();
    }
    pools.refresh();
    let body = HANDLE.get().and_then(Option::as_ref).map(PrometheusHandle::render).unwrap_or_default();
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], body).into_response()
}
