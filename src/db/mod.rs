pub mod batch;

use std::str::FromStr;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool};

#[derive(Debug, Clone)]
pub struct PoolSettings {
    pub url: SecretString,
    pub application_name: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub statement_cache_capacity: usize,
    pub acquire_timeout: Duration,
    pub max_lifetime: Option<Duration>,
    pub idle_timeout: Option<Duration>,
}

impl PoolSettings {
    pub fn new(url: SecretString, application_name: &str) -> Self {
        Self {
            url,
            application_name: application_name.to_owned(),
            max_connections: 10,
            min_connections: 0,
            statement_cache_capacity: 100,
            acquire_timeout: Duration::from_secs(10),
            max_lifetime: Some(Duration::from_secs(300)),
            idle_timeout: Some(Duration::from_secs(60)),
        }
    }

    #[must_use]
    pub fn max_connections(mut self, max: u32) -> Self {
        self.max_connections = max;
        self
    }

    #[must_use]
    pub fn statement_cache_capacity(mut self, capacity: usize) -> Self {
        self.statement_cache_capacity = capacity;
        self
    }

    #[must_use]
    pub fn acquire_timeout(mut self, timeout: Duration) -> Self {
        self.acquire_timeout = timeout;
        self
    }
}

pub fn connect_lazy(settings: &PoolSettings) -> Result<PgPool, sqlx::Error> {
    let options = PgConnectOptions::from_str(settings.url.expose_secret())?
        .statement_cache_capacity(settings.statement_cache_capacity)
        .application_name(&settings.application_name);
    Ok(PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .min_connections(settings.min_connections)
        .acquire_timeout(settings.acquire_timeout)
        .max_lifetime(settings.max_lifetime)
        .idle_timeout(settings.idle_timeout)
        .connect_lazy_with(options))
}

pub async fn plan_cache_mode(db: &PgPool) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SHOW plan_cache_mode").fetch_one(db).await
}

#[derive(Debug)]
pub enum Revision {
    Expected,
    Other(Vec<String>),
    Unreadable(sqlx::Error),
}

pub async fn check_revision(db: &PgPool, expected: &str) -> Revision {
    match sqlx::query_scalar::<_, String>("SELECT version_num FROM alembic_version").fetch_all(db).await {
        Ok(found) if found.iter().any(|rev| rev == expected) => Revision::Expected,
        Ok(found) => Revision::Other(found),
        Err(err) => Revision::Unreadable(err),
    }
}

pub async fn report_revision(db: &PgPool, expected: &str) {
    match check_revision(db, expected).await {
        Revision::Expected => {}
        Revision::Other(found) => {
            tracing::error!(expected, found = ?found, "the database is on another schema revision");
        }
        Revision::Unreadable(err) => tracing::error!(error = %err, "cannot read the schema revision"),
    }
}

pub async fn guard(conn: &mut PgConnection, statement: &str, idle: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT set_config('statement_timeout', $1, true), set_config('idle_in_transaction_session_timeout', $2, true)",
    )
    .bind(statement)
    .bind(idle)
    .execute(conn)
    .await?;
    Ok(())
}
