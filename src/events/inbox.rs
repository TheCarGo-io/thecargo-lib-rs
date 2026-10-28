use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::config::parse_isoformat;

pub async fn ensure_table(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS processed_events (\
         queue text NOT NULL, event_id uuid NOT NULL, processed_at timestamptz NOT NULL DEFAULT now(), \
         PRIMARY KEY (queue, event_id))",
    )
    .execute(pool)
    .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS ix_processed_events_processed_at ON processed_events (processed_at)")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn claim(conn: &mut PgConnection, queue: &str, event_id: Uuid) -> Result<bool, sqlx::Error> {
    let inserted = sqlx::query("INSERT INTO processed_events (queue, event_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(queue)
        .bind(event_id)
        .execute(conn)
        .await?
        .rows_affected();
    Ok(inserted == 1)
}

pub async fn purge(pool: &PgPool, older_than: Duration) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM processed_events WHERE processed_at < now() - $1")
        .bind(older_than)
        .execute(pool)
        .await?
        .rows_affected())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub event_id: Option<Uuid>,
    pub occurred_at: Option<DateTime<Utc>>,
    pub body: Value,
}

impl Envelope {
    pub fn parse(body: Value) -> Self {
        let event_id = body.get("event_id").and_then(Value::as_str).and_then(|raw| Uuid::parse_str(raw).ok());
        let occurred_at = ["occurred_at", "timestamp", "created_at"]
            .iter()
            .filter_map(|key| body.get(*key).and_then(Value::as_str))
            .find_map(|raw| parse_isoformat(raw).map(|moment| moment.at));
        Self { event_id, occurred_at, body }
    }

    pub fn data<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        T::deserialize(&self.body)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_envelope_reads_the_id_and_the_moment_when_present() {
        let id = Uuid::new_v4();
        let envelope = Envelope::parse(json!({"event_id": id, "occurred_at": "2026-10-20T15:59:12+00:00", "x": 1}));
        assert_eq!(envelope.event_id, Some(id));
        assert_eq!(envelope.occurred_at.map(|m| m.to_rfc3339()), Some("2026-10-20T15:59:12+00:00".to_owned()));
        assert_eq!(envelope.body["x"], 1);
        let bare = Envelope::parse(json!({"event_id": "not-a-uuid"}));
        assert_eq!(bare.event_id, None);
        assert_eq!(bare.occurred_at, None);
    }
}
