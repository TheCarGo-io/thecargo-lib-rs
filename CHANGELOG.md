# Changelog

## 0.2.0 (unreleased)

Breaking:
- `metrics::Pools` is built with `Pools::single(pool)` or `Pools::named(...)`; pool gauges refresh on scrape only.
- `dependencies::auth::CurrentUser` and `Requires<P>::user` hold `Arc<TokenPayload>`.
- The crates `clickhouse`, `sentry` and `lapin` sit behind the features `clickhouse`, `sentry` and `rabbit` (all on by default); `dashboard` is a feature too.

Added:
- `config`: the environment helpers every service repeats (`parse_or`, `parse_flag`, `split_list`, `sqlx_url`, `swarm_database_url`, `instant`, `required`).
- `db`: `PoolSettings` + `connect_lazy`, `check_revision`, transaction `guard`, `batch::rounds`.
- `clients::ServiceClient`: one HTTP client with the service secret, connect retries and timeouts; `url`, `isoformat`.
- `runtime`: `shutdown_signal`, `connect_redis`, `serve_until`, `join_with_grace`.
- `jobs::scheduler`: `When` (hourly, daily, every), Redis slot `claim`, `run_schedule`.
- `events::publisher`, `events::outbox` (the Python `outbox_events` contract: relay, reclaim, cleanup, deliver after commit) and `events::inbox` (`processed_events` dedup, `Envelope`).
- Consumer metrics `events_consumed_total` and `events_handle_seconds`.
- `dependencies::auth::authenticate` middleware decodes the token once per request; `CurrentUser`, `Requires` and the rate limiter read it from the request extensions.
- `i18n::Locales::base()` ships the lib's own `common`, `auth`, `upload` and `validation` keys; `Locales::merge`.
- `health::router`, `openapi::fastapi_shape`, `openapi::BearerAuth`, `openapi::docs_router`.
- `ErrorResponse.params` and `ValidationErrorItem.params` are no longer listed as required in the OpenAPI document, as pydantic publishes them.
- Re-exports of the crates services share with the lib (`axum`, `sqlx`, `redis`, `chrono`, `uuid`, `serde_json`, `utoipa`, `reqwest`, `tokio`, `tokio_util`, `tracing`, `secrecy`).

## 0.1.0

The first shared crate: auth, errors, i18n, extraction, consumer, ClickHouse DSN, cache, metrics, rate limit, request id, observability, dashboard shapers, timezone and rounding helpers.
