use std::time::Duration;

use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::events::publisher::Publisher;

pub const RELAY_GRACE: Duration = Duration::from_secs(30);
pub const RELAY_BATCH: i64 = 200;
pub const RELAY_MAX_ATTEMPTS: i32 = 10;
pub const RECLAIM_AFTER: Duration = Duration::from_secs(300);
pub const RELAY_INTERVAL: Duration = Duration::from_secs(5);
pub const CLEANUP_RETENTION: Duration = Duration::from_hours(7 * 24);
pub const CLEANUP_EVERY_TICKS: u64 = 720;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub id: Uuid,
    pub routing_key: String,
    pub payload: Value,
}

pub fn stamped(payload: &Value, event_id: Uuid) -> Value {
    match payload {
        Value::Object(map) if map.get("event_id").is_some_and(|v| !v.is_null() && v.as_str() != Some("")) => {
            payload.clone()
        }
        Value::Object(map) => {
            let mut stamped = map.clone();
            stamped.insert("event_id".to_owned(), Value::from(event_id.to_string()));
            Value::Object(stamped)
        }
        other => other.clone(),
    }
}

pub async fn enqueue(conn: &mut PgConnection, routing_key: &str, payload: Value) -> Result<Staged, sqlx::Error> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO outbox_events (id, routing_key, payload, status, attempts) VALUES ($1, $2, $3::json, 'pending', 0)")
        .bind(id)
        .bind(routing_key)
        .bind(payload.to_string())
        .execute(conn)
        .await?;
    Ok(Staged { id, routing_key: routing_key.to_owned(), payload })
}

async fn mark_published(pool: &PgPool, ids: &[Uuid], count_attempt: bool) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    let sql = if count_attempt {
        "UPDATE outbox_events SET status = 'published', published_at = now(), updated_at = now(), attempts = attempts + 1 WHERE id = ANY($1)"
    } else {
        "UPDATE outbox_events SET status = 'published', published_at = now(), updated_at = now() WHERE id = ANY($1)"
    };
    sqlx::query(sql).bind(ids).execute(pool).await?;
    Ok(())
}

pub async fn deliver(pool: &PgPool, publisher: &Publisher, staged: &[Staged]) {
    let messages: Vec<(&str, Value)> =
        staged.iter().map(|row| (row.routing_key.as_str(), stamped(&row.payload, row.id))).collect();
    let outcomes = match publisher.publish_many(&messages).await {
        Ok(outcomes) => outcomes,
        Err(err) => {
            tracing::warn!(error = %err, count = staged.len(), "outbox immediate publish failed; the relay will retry");
            return;
        }
    };
    let mut delivered = Vec::with_capacity(staged.len());
    for (row, outcome) in staged.iter().zip(outcomes) {
        match outcome {
            Ok(()) => delivered.push(row.id),
            Err(err) => {
                tracing::warn!(routing_key = %row.routing_key, error = %err, "outbox immediate publish failed; the relay will retry");
            }
        }
    }
    if let Err(err) = mark_published(pool, &delivered, true).await {
        tracing::error!(error = %err, "outbox mark-published failed; the relay will republish");
    }
}

#[derive(sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    routing_key: String,
    payload: String,
}

pub async fn relay_once(pool: &PgPool, publisher: &Publisher) -> Result<usize, sqlx::Error> {
    reclaim_stuck(pool).await?;
    let mut tx = pool.begin().await?;
    let rows: Vec<Claimed> = sqlx::query_as(
        "SELECT id, routing_key, payload::text AS payload FROM outbox_events \
         WHERE status = 'pending' AND created_at < now() - $1 AND attempts < $2 \
         ORDER BY created_at LIMIT $3 FOR UPDATE SKIP LOCKED",
    )
    .bind(RELAY_GRACE)
    .bind(RELAY_MAX_ATTEMPTS)
    .bind(RELAY_BATCH)
    .fetch_all(&mut *tx)
    .await?;
    if rows.is_empty() {
        tx.rollback().await?;
        return Ok(0);
    }
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    sqlx::query("UPDATE outbox_events SET status = 'publishing', attempts = attempts + 1, updated_at = now() WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let mut delivered = Vec::new();
    let mut failed: Vec<(Uuid, String)> = Vec::new();
    for row in rows {
        let payload: Value = match serde_json::from_str(&row.payload) {
            Ok(value) => value,
            Err(err) => {
                failed.push((row.id, format!("payload is not JSON: {err}")));
                continue;
            }
        };
        match publisher.publish(&row.routing_key, &stamped(&payload, row.id)).await {
            Ok(()) => delivered.push(row.id),
            Err(err) => failed.push((row.id, err.to_string().chars().take(500).collect())),
        }
    }
    mark_published(pool, &delivered, false).await?;
    for (id, error) in &failed {
        sqlx::query("UPDATE outbox_events SET status = 'pending', last_error = $2, updated_at = now() WHERE id = $1")
            .bind(id)
            .bind(error)
            .execute(pool)
            .await?;
    }
    Ok(delivered.len())
}

pub async fn reclaim_stuck(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let reclaimed = sqlx::query("UPDATE outbox_events SET status = 'pending', updated_at = now() WHERE status = 'publishing' AND updated_at < now() - $1")
        .bind(RECLAIM_AFTER)
        .execute(pool)
        .await?
        .rows_affected();
    if reclaimed > 0 {
        tracing::warn!(reclaimed, "outbox reclaimed rows stuck in publishing");
    }
    Ok(reclaimed)
}

pub async fn cleanup_published(pool: &PgPool, older_than: Duration) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM outbox_events WHERE status = 'published' AND published_at < now() - $1")
        .bind(older_than)
        .execute(pool)
        .await?
        .rows_affected())
}

pub async fn run_relay(pool: PgPool, publisher: Publisher, cancel: CancellationToken, interval: Duration) {
    tracing::info!(interval_s = interval.as_secs(), "outbox relay loop started");
    let mut tick: u64 = 0;
    while !cancel.is_cancelled() {
        match relay_once(&pool, &publisher).await {
            Ok(sent) if sent > 0 => tracing::debug!(sent, "outbox relay published"),
            Ok(_) => {}
            Err(err) => tracing::error!(error = %err, "outbox relay tick failed"),
        }
        if tick.is_multiple_of(CLEANUP_EVERY_TICKS) {
            match cleanup_published(&pool, CLEANUP_RETENTION).await {
                Ok(deleted) if deleted > 0 => tracing::info!(deleted, "outbox cleanup deleted published rows"),
                Ok(_) => {}
                Err(err) => tracing::error!(error = %err, "outbox cleanup failed"),
            }
        }
        tick += 1;
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(interval) => {}
        }
    }
    tracing::info!("outbox relay loop stopped");
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_row_id_stamps_a_payload_without_an_event_id() {
        let id = Uuid::new_v4();
        assert_eq!(stamped(&json!({"a": 1}), id)["event_id"], id.to_string());
        assert_eq!(stamped(&json!({"a": 1, "event_id": ""}), id)["event_id"], id.to_string());
        assert_eq!(stamped(&json!({"event_id": "keep"}), id)["event_id"], "keep");
        assert_eq!(stamped(&json!([1]), id), json!([1]));
    }
}
