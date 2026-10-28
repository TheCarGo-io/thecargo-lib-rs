use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::DecodingKey;
use redis::aio::ConnectionManager;
use serde_json::{Value, json};

use crate::dependencies::auth::{Authenticated, bearer_token, decode_claims};
use crate::dependencies::service_auth::{SERVICE_SECRET_HEADER, ServiceSecret};

const TOKEN_BUCKET_SCRIPT: &str = r"
local key = KEYS[1]
local rate = tonumber(ARGV[1])
local burst = tonumber(ARGV[2])
local clock = redis.call('TIME')
local now = tonumber(clock[1]) + tonumber(clock[2]) / 1000000
local state = redis.call('HMGET', key, 'tokens', 'at')
local tokens = tonumber(state[1]) or burst
local at = tonumber(state[2]) or now
tokens = math.min(burst, tokens + math.max(0, now - at) * rate)
local retry_ms = 0
if tokens >= 1 then
    tokens = tokens - 1
else
    retry_ms = math.ceil((1 - tokens) / rate * 1000)
end
redis.call('HSET', key, 'tokens', tostring(tokens), 'at', tostring(now))
redis.call('PEXPIRE', key, math.ceil(burst / rate * 1000) + 1000)
return retry_ms
";

const SKIP_PREFIXES: [&str; 2] = ["/health", "/metrics"];
const REDIS_TIMEOUT: Duration = Duration::from_millis(100);
const PAUSE_AFTER_REDIS_ERROR: Duration = Duration::from_secs(5);
const REPORT_EVERY: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Shadow,
    Enforce,
    Off,
}

impl Mode {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "shadow" => Self::Shadow,
            "enforce" => Self::Enforce,
            _ => Self::Off,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub mode: Mode,
    pub per_minute: u32,
    pub burst: u32,
    pub service: String,
}

// One token bucket per organization in Redis, shared by every replica. In shadow mode an empty
// bucket is only logged. A Redis failure switches the limit off for a few seconds rather than
// slowing every request down.
pub struct RateLimiter {
    settings: Settings,
    decoding: Arc<DecodingKey>,
    service_secret: ServiceSecret,
    redis: ConnectionManager,
    script: redis::Script,
    paused_until: Mutex<Option<Instant>>,
    reported: Mutex<HashMap<String, Report>>,
}

#[derive(Debug, Default)]
struct Report {
    at: Option<Instant>,
    suppressed: u64,
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter").field("settings", &self.settings).finish_non_exhaustive()
    }
}

impl RateLimiter {
    pub fn new(
        settings: Settings,
        decoding: Arc<DecodingKey>,
        service_secret: ServiceSecret,
        redis: ConnectionManager,
    ) -> Self {
        Self {
            settings,
            decoding,
            service_secret,
            redis,
            script: redis::Script::new(TOKEN_BUCKET_SCRIPT),
            paused_until: Mutex::new(None),
            reported: Mutex::new(HashMap::new()),
        }
    }

    fn org_of(&self, request: &Request) -> Option<String> {
        let path = request.uri().path();
        if SKIP_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
            return None;
        }
        let secret = request.headers().get(SERVICE_SECRET_HEADER).and_then(|value| value.to_str().ok());
        if self.service_secret.matches(secret) {
            return None;
        }
        match request.extensions().get::<Authenticated>() {
            Some(Authenticated(Ok(user))) => return Some(user.org_id.to_string()),
            Some(Authenticated(Err(_))) => return None,
            None => {}
        }
        let token = bearer_token(request.headers()).ok()?;
        match decode_claims(token, &self.decoding)?.get("org_id")? {
            Value::Null => None,
            Value::String(org) if org.is_empty() => None,
            Value::String(org) => Some(org.clone()),
            other => Some(other.to_string()),
        }
    }

    fn paused(&self) -> bool {
        let guard = self.paused_until.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.is_some_and(|until| Instant::now() < until)
    }

    fn pause(&self) {
        let mut guard = self.paused_until.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(Instant::now() + PAUSE_AFTER_REDIS_ERROR);
    }

    async fn take(&self, org: &str) -> Option<u64> {
        if self.paused() {
            return None;
        }
        let mut conn = self.redis.clone();
        let mut invocation = self.script.key(format!("ratelimit:org:{}:{org}", self.settings.service));
        invocation.arg(f64::from(self.settings.per_minute) / 60.0).arg(self.settings.burst);
        match tokio::time::timeout(REDIS_TIMEOUT, invocation.invoke_async::<i64>(&mut conn)).await {
            Ok(Ok(retry_ms)) if retry_ms > 0 => Some(u64::try_from(retry_ms).unwrap_or(0).div_ceil(1000).max(1)),
            Ok(Ok(_)) => None,
            Ok(Err(err)) => {
                self.pause();
                tracing::warn!(error = %err, pause_s = PAUSE_AFTER_REDIS_ERROR.as_secs(), "org rate limit off, redis failed");
                None
            }
            Err(_) => {
                self.pause();
                tracing::warn!(pause_s = PAUSE_AFTER_REDIS_ERROR.as_secs(), "org rate limit off, redis timed out");
                None
            }
        }
    }

    fn report(&self, org: &str, request: &Request) {
        let now = Instant::now();
        let suppressed = {
            let mut reported = self.reported.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = reported.entry(org.to_owned()).or_default();
            if entry.at.is_some_and(|at| now.duration_since(at) < REPORT_EVERY) {
                entry.suppressed += 1;
                return;
            }
            std::mem::replace(entry, Report { at: Some(now), suppressed: 0 }).suppressed
        };
        tracing::warn!(
            mode = ?self.settings.mode,
            service = %self.settings.service,
            org,
            method = %request.method(),
            path = request.uri().path(),
            suppressed,
            "org rate limit exceeded",
        );
    }
}

pub async fn rate_limit(State(limiter): State<Arc<RateLimiter>>, request: Request, next: Next) -> Response {
    if limiter.settings.mode == Mode::Off {
        return next.run(request).await;
    }
    let Some(org) = limiter.org_of(&request) else {
        return next.run(request).await;
    };
    let Some(retry_after) = limiter.take(&org).await else {
        return next.run(request).await;
    };
    limiter.report(&org, &request);
    if limiter.settings.mode == Mode::Shadow {
        return next.run(request).await;
    }
    let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(json!({"detail": "Too Many Requests"}))).into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}
