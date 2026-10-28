#![cfg(feature = "rabbit")]
use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use thecargo::events::inbox;
use thecargo::events::outbox;
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    PgPool::connect(&url).await.ok()
}

#[tokio::test]
async fn an_enqueued_event_waits_as_pending_with_its_payload() {
    let Some(db) = pool().await else { return };
    let mut tx = db.begin().await.unwrap();
    let staged = outbox::enqueue(&mut tx, "shipment.test.created", json!({"id": 7, "code": "X"})).await.unwrap();
    tx.commit().await.unwrap();

    let (status, attempts, payload): (String, i32, String) =
        sqlx::query_as("SELECT status, attempts, payload::text FROM outbox_events WHERE id = $1")
            .bind(staged.id)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(attempts, 0);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&payload).unwrap(), json!({"id": 7, "code": "X"}));
    sqlx::query("DELETE FROM outbox_events WHERE id = $1").bind(staged.id).execute(&db).await.unwrap();
}

#[tokio::test]
async fn stuck_publishing_rows_return_to_pending_after_the_reclaim_window() {
    let Some(db) = pool().await else { return };
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO outbox_events (id, routing_key, payload, status, attempts, updated_at) VALUES ($1, 'k', '{}'::json, 'publishing', 1, now() - interval '10 minutes')")
        .bind(id)
        .execute(&db)
        .await
        .unwrap();
    let fresh = Uuid::new_v4();
    sqlx::query("INSERT INTO outbox_events (id, routing_key, payload, status, attempts) VALUES ($1, 'k', '{}'::json, 'publishing', 1)")
        .bind(fresh)
        .execute(&db)
        .await
        .unwrap();

    assert_eq!(outbox::reclaim_stuck(&db).await.unwrap(), 1);
    let status: String =
        sqlx::query_scalar("SELECT status FROM outbox_events WHERE id = $1").bind(id).fetch_one(&db).await.unwrap();
    assert_eq!(status, "pending");
    let still: String =
        sqlx::query_scalar("SELECT status FROM outbox_events WHERE id = $1").bind(fresh).fetch_one(&db).await.unwrap();
    assert_eq!(still, "publishing");
    sqlx::query("DELETE FROM outbox_events WHERE id = ANY($1)").bind(vec![id, fresh]).execute(&db).await.unwrap();
}

#[tokio::test]
async fn published_rows_older_than_the_retention_are_deleted() {
    let Some(db) = pool().await else { return };
    let old = Uuid::new_v4();
    sqlx::query("INSERT INTO outbox_events (id, routing_key, payload, status, attempts, published_at) VALUES ($1, 'k', '{}'::json, 'published', 1, now() - interval '8 days')")
        .bind(old)
        .execute(&db)
        .await
        .unwrap();
    assert!(outbox::cleanup_published(&db, Duration::from_hours(7 * 24)).await.unwrap() >= 1);
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox_events WHERE id = $1").bind(old).fetch_one(&db).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn an_event_is_claimed_once_per_queue() {
    let Some(db) = pool().await else { return };
    inbox::ensure_table(&db).await.unwrap();
    let event = Uuid::new_v4();
    let mut tx = db.begin().await.unwrap();
    assert!(inbox::claim(&mut tx, "shipment-org-events", event).await.unwrap());
    assert!(!inbox::claim(&mut tx, "shipment-org-events", event).await.unwrap());
    assert!(inbox::claim(&mut tx, "shipment-lead-emails", event).await.unwrap());
    tx.rollback().await.unwrap();
}
