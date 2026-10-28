use std::time::Duration;

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use serde::de::DeserializeOwned;

const REDIS_TIMEOUT: Duration = Duration::from_millis(250);

// Read through Redis; a Redis failure only costs the cache, never the request.
pub async fn cache_aside<T, E, F, Fut>(
    redis: &ConnectionManager,
    key: &str,
    ttl_seconds: u64,
    loader: F,
) -> Result<T, E>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    if let Some(raw) = cache_get(redis, key).await {
        match serde_json::from_str(&raw) {
            Ok(value) => return Ok(value),
            Err(err) => tracing::warn!(key, error = %err, "cache_decode_failed"),
        }
    }
    let value = loader().await?;
    match serde_json::to_string(&value) {
        Ok(raw) => cache_set_raw(redis, key, &raw, ttl_seconds).await,
        Err(err) => tracing::warn!(key, error = %err, "cache_encode_failed"),
    }
    Ok(value)
}

pub async fn cache_get(redis: &ConnectionManager, key: &str) -> Option<String> {
    let mut conn = redis.clone();
    match tokio::time::timeout(REDIS_TIMEOUT, conn.get::<_, Option<String>>(key)).await {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            tracing::warn!(key, error = %err, "cache_get_failed");
            None
        }
        Err(_) => {
            tracing::warn!(key, "cache_get_timed_out");
            None
        }
    }
}

pub async fn cache_set<T: Serialize>(redis: &ConnectionManager, key: &str, value: &T, ttl_seconds: u64) {
    match serde_json::to_string(value) {
        Ok(raw) => cache_set_raw(redis, key, &raw, ttl_seconds).await,
        Err(err) => tracing::warn!(key, error = %err, "cache_encode_failed"),
    }
}

pub async fn cache_invalidate(redis: &ConnectionManager, keys: &[String]) {
    if keys.is_empty() {
        return;
    }
    let mut conn = redis.clone();
    match tokio::time::timeout(REDIS_TIMEOUT, conn.del::<_, ()>(keys)).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::warn!(?keys, error = %err, "cache_invalidate_failed"),
        Err(_) => tracing::warn!(?keys, "cache_invalidate_timed_out"),
    }
}

pub async fn cache_set_raw(redis: &ConnectionManager, key: &str, raw: &str, ttl_seconds: u64) {
    let mut conn = redis.clone();
    match tokio::time::timeout(REDIS_TIMEOUT, conn.set_ex::<_, _, ()>(key, raw, ttl_seconds)).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::warn!(key, error = %err, "cache_set_failed"),
        Err(_) => tracing::warn!(key, "cache_set_timed_out"),
    }
}
