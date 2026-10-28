use std::time::Duration;

use chrono::{DateTime, TimeDelta, Timelike, Utc};
use redis::aio::ConnectionManager;
use tokio_util::sync::CancellationToken;

use crate::utils::timezone::Clock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Hourly { minute: u32 },
    Daily { hour: u32, minute: u32 },
    Every(Duration),
}

impl When {
    pub fn next_after(self, now: DateTime<Utc>) -> DateTime<Utc> {
        let hour_start =
            now.with_minute(0).and_then(|t| t.with_second(0)).and_then(|t| t.with_nanosecond(0)).unwrap_or(now);
        match self {
            Self::Hourly { minute } => {
                let slot = hour_start + TimeDelta::minutes(i64::from(minute));
                if slot > now { slot } else { slot + TimeDelta::hours(1) }
            }
            Self::Daily { hour, minute } => {
                let day_start = hour_start - TimeDelta::hours(i64::from(now.hour()));
                let slot = day_start + TimeDelta::hours(i64::from(hour)) + TimeDelta::minutes(i64::from(minute));
                if slot > now { slot } else { slot + TimeDelta::days(1) }
            }
            Self::Every(period) => {
                let seconds = i64::try_from(period.as_secs().max(1)).unwrap_or(i64::MAX);
                let aligned = now.timestamp().div_euclid(seconds) * seconds + seconds;
                DateTime::from_timestamp(aligned, 0).unwrap_or(now)
            }
        }
    }

    pub fn period(self) -> Duration {
        match self {
            Self::Hourly { .. } => Duration::from_hours(1),
            Self::Daily { .. } => Duration::from_hours(24),
            Self::Every(period) => period,
        }
    }
}

pub fn slot_key(prefix: &str, job: &str, slot: DateTime<Utc>) -> String {
    format!("{prefix}:schedule:{job}:{}", slot.format("%Y-%m-%dT%H:%M:%S"))
}

pub async fn claim(redis: &ConnectionManager, key: &str, ttl: Duration) -> bool {
    let mut conn = redis.clone();
    let claimed: Result<Option<String>, redis::RedisError> =
        redis::cmd("SET").arg(key).arg("1").arg("NX").arg("EX").arg(ttl.as_secs().max(1)).query_async(&mut conn).await;
    match claimed {
        Ok(reply) => reply.is_some(),
        Err(err) => {
            tracing::warn!(key, error = %err, "cannot claim the schedule slot; skipping it");
            false
        }
    }
}

#[derive(Debug, Clone)]
pub struct Schedule {
    pub prefix: String,
    pub job: String,
    pub when: When,
    pub clock: Clock,
}

pub async fn run_schedule<F, Fut>(schedule: Schedule, redis: ConnectionManager, cancel: CancellationToken, run: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<serde_json::Value>>,
{
    let job = schedule.job.as_str();
    loop {
        let slot = schedule.when.next_after(schedule.clock.now());
        let wait = (slot - Utc::now()).to_std().unwrap_or_default();
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
        if !claim(&redis, &slot_key(&schedule.prefix, job, slot), schedule.when.period() * 2).await {
            continue;
        }
        let started = std::time::Instant::now();
        match run().await {
            Ok(result) => tracing::info!(job, seconds = started.elapsed().as_secs(), %result, "scheduled job finished"),
            Err(err) => tracing::error!(job, error = format!("{err:#}"), "scheduled job failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(raw: &str) -> DateTime<Utc> {
        raw.parse().unwrap_or_default()
    }

    #[test]
    fn slots_follow_the_wall_clock() {
        let hourly = When::Hourly { minute: 10 };
        assert_eq!(hourly.next_after(at("2026-10-24T15:05:00Z")), at("2026-10-24T15:10:00Z"));
        assert_eq!(hourly.next_after(at("2026-10-24T15:10:00Z")), at("2026-10-24T16:10:00Z"));
        let daily = When::Daily { hour: 6, minute: 20 };
        assert_eq!(daily.next_after(at("2026-10-24T05:00:00Z")), at("2026-10-24T06:20:00Z"));
        assert_eq!(daily.next_after(at("2026-10-24T07:00:00Z")), at("2026-10-25T06:20:00Z"));
    }

    #[test]
    fn every_aligns_to_the_period_from_the_epoch() {
        let every = When::Every(Duration::from_secs(120));
        assert_eq!(every.next_after(at("2026-10-24T15:05:00Z")), at("2026-10-24T15:06:00Z"));
        assert_eq!(every.next_after(at("2026-10-24T15:06:00Z")), at("2026-10-24T15:08:00Z"));
        assert_eq!(every.period(), Duration::from_secs(120));
        assert_eq!(
            slot_key("shipment", "lead_attach", at("2026-10-24T15:06:00Z")),
            "shipment:schedule:lead_attach:2026-10-24T15:06:00"
        );
    }
}
