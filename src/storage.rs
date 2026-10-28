use std::time::Duration;

use axum::body::Bytes;
use axum::http::StatusCode;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use secrecy::{ExposeSecret, SecretString};
use url::Url;

pub const MAX_RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_secs(1);
const PRESIGN_FOR_REQUEST: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REGION: &str = "auto";
pub const OCTET_STREAM: &str = "application/octet-stream";

#[derive(Debug, Clone)]
pub struct StorageSettings {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: SecretString,
    pub bucket: String,
    pub secure: bool,
    pub public_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("object storage not initialized")]
    Unconfigured,
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("object storage request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("object storage returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("invalid storage endpoint: {0}")]
    Endpoint(String),
}

impl StorageError {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }
}

pub struct Storage {
    endpoint: Url,
    signing_endpoint: Url,
    credentials: Credentials,
    bucket: String,
    public_url: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

fn is_r2_host(host: &str) -> bool {
    host.ends_with(".r2.dev") || host.ends_with(".r2.cloudflarestorage.com")
}

impl Storage {
    pub fn new(settings: &StorageSettings, http: reqwest::Client) -> Result<Self, StorageError> {
        let scheme = if settings.secure { "https" } else { "http" };
        let endpoint_text = format!("{scheme}://{}", settings.endpoint);
        let endpoint = Url::parse(&endpoint_text).map_err(|err| StorageError::Endpoint(err.to_string()))?;
        let public_url =
            if settings.public_url.is_empty() { endpoint_text.clone() } else { settings.public_url.clone() };
        let public_url = public_url.trim_end_matches('/').to_owned();
        let signing_endpoint = Url::parse(&public_url)
            .ok()
            .filter(|url| url.host_str().is_some() && url.host_str() != endpoint.host_str())
            .unwrap_or_else(|| endpoint.clone());
        Ok(Self {
            endpoint,
            signing_endpoint,
            credentials: Credentials::new(settings.access_key.clone(), settings.secret_key.expose_secret().to_owned()),
            bucket: settings.bucket.clone(),
            public_url,
            http,
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn public_url(&self, path: &str) -> String {
        format!("{}/{}/{path}", self.public_url, self.bucket)
    }

    pub fn stored_url(&self, path: &str) -> String {
        format!("{}/{}/{path}", self.endpoint.as_str().trim_end_matches('/'), self.bucket)
    }

    fn own_host(&self) -> &str {
        self.public_url.split("://").nth(1).unwrap_or_default().split('/').next().unwrap_or_default()
    }

    fn endpoint_host(&self) -> String {
        let host = self.endpoint.host_str().unwrap_or_default();
        match self.endpoint.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        }
    }

    fn is_own_host(&self, host: &str) -> bool {
        !host.is_empty() && (host == self.own_host() || host == self.endpoint_host() || is_r2_host(host))
    }

    pub fn bucket_and_path_from_url(&self, url: &str) -> (String, String) {
        if url.is_empty() {
            return (self.bucket.clone(), String::new());
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            let Ok(parsed) = Url::parse(url) else { return (self.bucket.clone(), String::new()) };
            let host = parsed.host_str().unwrap_or_default();
            let host = match parsed.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_owned(),
            };
            if !self.is_own_host(&host) {
                return (self.bucket.clone(), String::new());
            }
            let rest = parsed.path().trim_start_matches('/');
            return match rest.split_once('/') {
                Some((head, key)) if !key.is_empty() => (head.to_owned(), key.to_owned()),
                _ => (self.bucket.clone(), rest.to_owned()),
            };
        }
        let mut path = url.trim_start_matches('/');
        if !self.bucket.is_empty()
            && let Some(rest) = path.strip_prefix(&format!("{}/", self.bucket))
        {
            path = rest;
        }
        (self.bucket.clone(), path.to_owned())
    }

    pub fn is_external_url(&self, reference: &str) -> bool {
        if !(reference.starts_with("http://") || reference.starts_with("https://")) {
            return false;
        }
        let Ok(parsed) = Url::parse(reference) else { return true };
        let host = parsed.host_str().unwrap_or_default();
        let host = match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        !self.is_own_host(&host)
    }

    fn bucket_at(&self, endpoint: &Url, name: Option<&str>) -> Result<Bucket, StorageError> {
        Bucket::new(endpoint.clone(), UrlStyle::Path, name.unwrap_or(&self.bucket).to_owned(), REGION)
            .map_err(|err| StorageError::Endpoint(err.to_string()))
    }

    pub fn presigned_get_url(
        &self,
        path: &str,
        expires: Duration,
        bucket: Option<&str>,
    ) -> Result<String, StorageError> {
        let bucket = self.bucket_at(&self.signing_endpoint, bucket)?;
        Ok(bucket.get_object(Some(&self.credentials), path).sign(expires).to_string())
    }

    async fn send(&self, build: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response, StorageError> {
        let mut attempt = 1;
        loop {
            let sent = build().timeout(REQUEST_TIMEOUT).send().await;
            match sent {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) if response.status() == StatusCode::NOT_FOUND => {
                    return Err(StorageError::NotFound(response.url().path().to_owned()));
                }
                Ok(response) if response.status().is_client_error() => {
                    let status = response.status().as_u16();
                    let body = response.text().await.unwrap_or_default();
                    return Err(StorageError::Status { status, body: body.chars().take(300).collect() });
                }
                Ok(response) if attempt >= MAX_RETRIES => {
                    let status = response.status().as_u16();
                    let body = response.text().await.unwrap_or_default();
                    return Err(StorageError::Status { status, body: body.chars().take(300).collect() });
                }
                Err(err) if attempt >= MAX_RETRIES => return Err(err.into()),
                Ok(_) | Err(_) => {
                    tokio::time::sleep(RETRY_DELAY * attempt).await;
                    attempt += 1;
                }
            }
        }
    }

    pub async fn upload_bytes(&self, path: &str, data: Bytes, content_type: &str) -> Result<String, StorageError> {
        let bucket = self.bucket_at(&self.endpoint, None)?;
        let url = bucket.put_object(Some(&self.credentials), path).sign(PRESIGN_FOR_REQUEST);
        let content_type = content_type.to_owned();
        self.send(|| {
            self.http.put(url.clone()).header(reqwest::header::CONTENT_TYPE, &content_type).body(data.clone())
        })
        .await?;
        Ok(self.public_url(path))
    }

    pub async fn download(&self, path: &str, bucket: Option<&str>) -> Result<(Bytes, String), StorageError> {
        let bucket = self.bucket_at(&self.endpoint, bucket)?;
        let url = bucket.get_object(Some(&self.credentials), path).sign(PRESIGN_FOR_REQUEST);
        let response = self.send(|| self.http.get(url.clone())).await?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.split(';').next().unwrap_or_default().trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| OCTET_STREAM.to_owned());
        Ok((response.bytes().await?, content_type))
    }

    pub async fn delete(&self, path: &str, bucket: Option<&str>) -> Result<(), StorageError> {
        let bucket = self.bucket_at(&self.endpoint, bucket)?;
        let url = bucket.delete_object(Some(&self.credentials), path).sign(PRESIGN_FOR_REQUEST);
        self.send(|| self.http.delete(url.clone())).await?;
        Ok(())
    }

    pub async fn exists(&self, path: &str, bucket: Option<&str>) -> bool {
        let Ok(bucket) = self.bucket_at(&self.endpoint, bucket) else { return false };
        let url = bucket.head_object(Some(&self.credentials), path).sign(PRESIGN_FOR_REQUEST);
        self.http.head(url).timeout(REQUEST_TIMEOUT).send().await.is_ok_and(|response| response.status().is_success())
    }

    pub async fn read_reference(&self, reference: &str) -> Result<(Bytes, String), StorageError> {
        if self.is_external_url(reference) {
            let response = self.http.get(reference).timeout(Duration::from_secs(10)).send().await?;
            if response.status() == StatusCode::NOT_FOUND {
                return Err(StorageError::NotFound(reference.to_owned()));
            }
            let response = response.error_for_status()?;
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.split(';').next().unwrap_or_default().trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "image/png".to_owned());
            return Ok((response.bytes().await?, content_type));
        }
        let (bucket, path) = self.bucket_and_path_from_url(reference);
        if path.is_empty() {
            return Err(StorageError::NotFound(reference.to_owned()));
        }
        self.download(&path, Some(&bucket)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> reqwest::Client {
        crate::observability::install_tls_provider();
        reqwest::Client::new()
    }

    fn storage() -> Storage {
        let settings = StorageSettings {
            endpoint: "acct.r2.cloudflarestorage.com".into(),
            access_key: "key".into(),
            secret_key: SecretString::from("secret"),
            bucket: "thecargo-dev-shipment".into(),
            secure: true,
            public_url: String::new(),
        };
        Storage::new(&settings, client()).expect("storage")
    }

    #[test]
    fn urls_split_into_bucket_and_key_like_python() {
        let storage = storage();
        assert_eq!(
            storage.public_url("contracts/a/b.png"),
            "https://acct.r2.cloudflarestorage.com/thecargo-dev-shipment/contracts/a/b.png"
        );
        assert_eq!(
            storage.bucket_and_path_from_url("https://acct.r2.cloudflarestorage.com/other-bucket/x/y.pdf"),
            ("other-bucket".to_owned(), "x/y.pdf".to_owned())
        );
        assert_eq!(
            storage.bucket_and_path_from_url("https://cdn.example.com/x.pdf"),
            ("thecargo-dev-shipment".to_owned(), String::new())
        );
        assert_eq!(
            storage.bucket_and_path_from_url("thecargo-dev-shipment/key.pdf"),
            ("thecargo-dev-shipment".to_owned(), "key.pdf".to_owned())
        );
        assert!(storage.is_external_url("https://cdn.example.com/x.pdf"));
        assert!(!storage.is_external_url("https://pub-123.r2.dev/b/k"));
        assert!(!storage.is_external_url("contracts/k.png"));
    }

    #[test]
    fn a_local_endpoint_with_a_port_counts_as_the_own_host() {
        let settings = StorageSettings {
            endpoint: "perf-minio:9000".into(),
            access_key: "key".into(),
            secret_key: SecretString::from("secret"),
            bucket: "thecargo-dev-shipment".into(),
            secure: false,
            public_url: String::new(),
        };
        let storage = Storage::new(&settings, client()).expect("storage");
        assert_eq!(storage.public_url("k.png"), "http://perf-minio:9000/thecargo-dev-shipment/k.png");
        assert_eq!(
            storage.bucket_and_path_from_url("http://perf-minio:9000/thecargo-dev-shipment/k.png"),
            ("thecargo-dev-shipment".to_owned(), "k.png".to_owned())
        );
        let presigned = storage.presigned_get_url("k.png", Duration::from_secs(60), None).expect("url");
        assert!(presigned.starts_with("http://perf-minio:9000/thecargo-dev-shipment/k.png?"));
        assert!(presigned.contains("X-Amz-Signature="));
    }
}
