use std::time::Duration;

use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;

use crate::dependencies::service_auth::SERVICE_SECRET_HEADER;
use crate::observability::install_tls_provider;

const CONNECT_RETRIES: usize = 3;

#[derive(Clone)]
pub struct ServiceClient {
    http: reqwest::Client,
    secret: SecretString,
}

impl std::fmt::Debug for ServiceClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceClient(..)")
    }
}

impl ServiceClient {
    pub fn new(secret: SecretString, user_agent: &str) -> Result<Self, reqwest::Error> {
        install_tls_provider();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(3))
            .user_agent(user_agent)
            .build()?;
        Ok(Self { http, secret })
    }

    pub fn with_http(http: reqwest::Client, secret: SecretString) -> Self {
        Self { http, secret }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub async fn send(
        &self,
        request: impl Fn() -> reqwest::RequestBuilder,
        timeout: Duration,
        retry: bool,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let mut attempt = 0;
        loop {
            let sent =
                request().timeout(timeout).header(SERVICE_SECRET_HEADER, self.secret.expose_secret()).send().await;
            match sent {
                Err(err) if retry && err.is_connect() && attempt < CONNECT_RETRIES => attempt += 1,
                sent => return sent,
            }
        }
    }

    pub async fn internal(
        &self,
        request: impl Fn() -> reqwest::RequestBuilder,
        timeout: Duration,
        retry: bool,
    ) -> Result<reqwest::Response, reqwest::Error> {
        self.send(request, timeout, retry).await?.error_for_status()
    }

    pub async fn internal_json<T: DeserializeOwned>(
        &self,
        request: impl Fn() -> reqwest::RequestBuilder,
        timeout: Duration,
        retry: bool,
    ) -> Result<T, reqwest::Error> {
        self.internal(request, timeout, retry).await?.json().await
    }
}

pub fn url(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

pub fn isoformat(moment: DateTime<Utc>) -> String {
    if moment.timestamp_subsec_micros() == 0 {
        moment.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        moment.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn urls_join_without_a_double_slash() {
        assert_eq!(url("http://shipment:8000/", "/api/internal/x"), "http://shipment:8000/api/internal/x");
        assert_eq!(url("http://shipment:8000", "/api/internal/x"), "http://shipment:8000/api/internal/x");
    }

    #[test]
    fn isoformat_prints_microseconds_only_when_there_are_any() {
        let whole = Utc.with_ymd_and_hms(2026, 10, 20, 15, 59, 12).unwrap();
        assert_eq!(isoformat(whole), "2026-10-20T15:59:12+00:00");
        assert_eq!(isoformat(whole + chrono::TimeDelta::microseconds(120)), "2026-10-20T15:59:12.000120+00:00");
    }
}
