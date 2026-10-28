# thecargo

The Rust counterpart of the shared Python package `thecargo` (thecargo-lib): what every TheCargo Rust
service needs and none should write twice.

- `dependencies`: JWT access tokens with the platform's permission grammar, the `authenticate`
  middleware that decodes a token once per request, service-to-service secrets, company context, the
  `Requires<P>` extractor.
- `handlers`, `schemas::errors`, `i18n`: one error envelope on every path, with stable codes and
  messages in English, Russian and Uzbek; `Locales::base()` holds the shared keys, a service merges its own.
- `extract`: query, path and JSON extraction that answers like FastAPI and pydantic do.
- `config`, `db`, `runtime`, `clients`: environment parsing, the Postgres pool and transaction guards,
  shutdown and Redis helpers, the service-to-service HTTP client.
- `events`: a RabbitMQ topic consumer with the Python consumer's ack and requeue contract, the publisher,
  the `outbox_events` relay and the `processed_events` inbox.
- `jobs::scheduler`: wall-clock slots claimed in Redis, so any number of replicas run a job once.
- `metrics`, `observability`, `middleware`, `health`, `openapi`: Prometheus, tracing, Sentry, rate
  limiting, request ids, health probes and the OpenAPI document in FastAPI's shape.
- `clickhouse`, `cache`, `dashboard`, `utils`: clients and the shapers shared with the services.

Features: `clickhouse`, `dashboard`, `sentry`, `rabbit` (all on by default). The crates services share with
the lib (`axum`, `sqlx`, `redis`, `chrono`, `uuid`, `serde_json`, `utoipa`, `reqwest`, `tokio`) are
re-exported, so a service pins one version.

Contract decisions kept from the Python lib: `ApiError::not_found()` answers 400, as `NotFoundException`
does; the 404 and 405 bodies keep Starlette's `{"detail": ...}` shape.

It depends on nothing service-specific. Services take it as a git dependency pinned to a tag:

    thecargo = { git = "https://github.com/TheCarGo-io/thecargo-lib-rs", tag = "v0.2.0" }

    cargo nextest run

A change that a service needs ships as a new tag; the service moves its pin. `CHANGELOG.md` lists what
each tag changed.
