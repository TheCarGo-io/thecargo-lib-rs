# thecargo

The Rust counterpart of the shared Python package `thecargo` (thecargo-lib): what every TheCargo Rust
service needs and none should write twice.

- `dependencies`: JWT access tokens with the platform's permission grammar, service-to-service
  secrets, company context, the `Requires<P>` extractor.
- `handlers`, `schemas::errors`, `i18n`: one error envelope on every path, with stable codes and
  messages in English, Russian and Uzbek.
- `extract`: query, path and JSON extraction that answers like FastAPI and pydantic do.
- `events::consumer`: a RabbitMQ topic consumer with the Python consumer's ack and requeue contract.
- `clickhouse`, `cache`, `metrics`, `observability`, `middleware`: clients, Prometheus, tracing,
  Sentry, rate limiting and audit.
- `dashboard`: the dashboard shapers and period rules shared with the shipment service.

It depends on nothing service-specific. Services take it as a git dependency pinned to a tag:

    thecargo = { git = "https://github.com/TheCarGo-io/thecargo-lib-rs", tag = "v0.1.0" }

The repository is private, so a service's builds need read access to it: locally through git's own
credentials (`net.git-fetch-with-cli` in the service's `.cargo/config.toml`), in CI and Docker
builds through a token (see thecargo-analytics-rs).

    cargo nextest run

A change that a service needs ships as a new tag; the service moves its pin.
