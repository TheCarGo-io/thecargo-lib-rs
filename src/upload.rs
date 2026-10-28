use axum::body::Bytes;
use axum::extract::Multipart;
use axum::http::StatusCode;
use regex::Regex;
use std::sync::LazyLock;

use crate::extract::Violations;
use crate::handlers::ApiError;
use crate::storage::Storage;

pub const IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/gif"];
pub const DOCUMENT_TYPES: [&str; 3] = [
    "application/pdf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
];
pub const PROOF_TYPES: [&str; 3] = ["image/jpeg", "image/png", "application/pdf"];
pub const PERMISSIVE_PREFIXES: [&str; 4] = ["image/", "application/pdf", "application/msword", "application/vnd."];
pub const PERMISSIVE_EXACT: [&str; 3] = ["text/plain", "text/csv", "application/zip"];
pub const MAX_BYTES_TINY: usize = 2 * 1024 * 1024;
pub const MAX_BYTES_SMALL: usize = 10 * 1024 * 1024;
pub const MAX_BYTES_LARGE: usize = 25 * 1024 * 1024;
const MARKUP_PREFIXES: [&[u8]; 7] = [b"<?xml", b"<!doctype", b"<html", b"<svg", b"<script", b"<!--", b"<?php"];

static FILENAME_DROP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^A-Za-z0-9._\-]+").unwrap_or_else(|_| unreachable!()));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadResult {
    pub name: String,
    pub url: String,
    pub mime_type: String,
    pub size_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Bytes,
}

#[derive(Debug, Clone, Copy)]
pub struct UploadRules<'a> {
    pub allowed_exact: &'a [&'a str],
    pub allowed_prefixes: &'a [&'a str],
    pub max_bytes: usize,
}

pub fn safe_filename(name: Option<&str>) -> String {
    let raw = name.unwrap_or("file").trim();
    let cleaned = FILENAME_DROP.replace_all(raw, "_").replace("..", "_");
    let cleaned = cleaned.trim_start_matches('.');
    let cleaned = if cleaned.is_empty() { "file" } else { cleaned };
    cleaned.chars().take(255).collect()
}

fn sniff(raw: &[u8]) -> Option<&'static str> {
    if raw.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if raw.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if raw.starts_with(b"GIF87a") || raw.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if raw.starts_with(b"RIFF") && raw.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if raw.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if raw.starts_with(b"PK\x03\x04") {
        Some("application/zip")
    } else if raw.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1") {
        Some("application/msword")
    } else {
        None
    }
}

fn looks_like_markup(raw: &[u8]) -> bool {
    let head = &raw[..raw.len().min(512)];
    let trimmed =
        head.iter().position(|byte| !byte.is_ascii_whitespace()).map_or(&head[head.len()..], |start| &head[start..]);
    let lowered: Vec<u8> = trimmed.iter().map(u8::to_ascii_lowercase).collect();
    MARKUP_PREFIXES.iter().any(|prefix| lowered.starts_with(prefix))
}

fn unsupported(content_type: &str) -> ApiError {
    ApiError::http(StatusCode::UNSUPPORTED_MEDIA_TYPE, format!("Unsupported MIME type: {content_type}"))
}

fn check_mime(content_type: &str, rules: UploadRules<'_>) -> Result<(), ApiError> {
    if rules.allowed_exact.is_empty() && rules.allowed_prefixes.is_empty() {
        return Ok(());
    }
    if rules.allowed_exact.contains(&content_type) || rules.allowed_prefixes.iter().any(|p| content_type.starts_with(p))
    {
        return Ok(());
    }
    Err(unsupported(content_type))
}

pub fn resolve_mime(raw: &[u8], declared: &str, rules: UploadRules<'_>) -> Result<String, ApiError> {
    let Some(sniffed) = sniff(raw) else {
        if looks_like_markup(raw) {
            return Err(ApiError::http(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Markup and script uploads are not accepted",
            ));
        }
        check_mime(declared, rules)?;
        return Ok(declared.to_owned());
    };
    if sniffed == "application/zip" && (declared == "application/zip" || declared.starts_with("application/vnd.")) {
        check_mime(declared, rules)?;
        return Ok(declared.to_owned());
    }
    check_mime(sniffed, rules)?;
    Ok(sniffed.to_owned())
}

pub async fn file_field(multipart: &mut Multipart, name: &str) -> Result<UploadedFile, ApiError> {
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => {
                let mut violations = Violations::default();
                violations.missing(name);
                return Err(violations.finish().err().unwrap_or_else(ApiError::internal));
            }
            Err(err) => return Err(ApiError::http(StatusCode::BAD_REQUEST, err.body_text())),
        };
        if field.name() != Some(name) {
            continue;
        }
        let filename = field.file_name().map(str::to_owned);
        let content_type = field.content_type().map(str::to_owned);
        let data = field.bytes().await.map_err(|err| ApiError::http(StatusCode::BAD_REQUEST, err.body_text()))?;
        return Ok(UploadedFile { filename, content_type, data });
    }
}

pub async fn upload_to_storage(
    storage: Option<&Storage>,
    file: UploadedFile,
    path_builder: impl Fn(&str, &str) -> String,
    rules: UploadRules<'_>,
) -> Result<UploadResult, ApiError> {
    if file.data.is_empty() {
        return Err(ApiError::http(StatusCode::BAD_REQUEST, "Empty file"));
    }
    if file.data.len() > rules.max_bytes {
        let limit_mb = rules.max_bytes / (1024 * 1024);
        return Err(ApiError::http(StatusCode::PAYLOAD_TOO_LARGE, format!("File exceeds {limit_mb} MB limit")));
    }
    let declared =
        file.content_type.clone().filter(|ct| !ct.is_empty()).unwrap_or_else(|| "application/octet-stream".to_owned());
    let content_type = resolve_mime(&file.data, &declared, rules)?;
    let safe = safe_filename(file.filename.as_deref());
    let storage_path = path_builder(&safe, &content_type);
    let unavailable = || ApiError::http(StatusCode::BAD_GATEWAY, "Object storage unavailable");
    let Some(storage) = storage else { return Err(unavailable()) };
    let size_bytes = file.data.len();
    storage.upload_bytes(&storage_path, file.data, &content_type).await.map_err(|err| {
        tracing::error!(path = %storage_path, error = %err, "object upload failed");
        unavailable()
    })?;
    Ok(UploadResult { name: safe, url: storage.stored_url(&storage_path), mime_type: content_type, size_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: UploadRules<'static> =
        UploadRules { allowed_exact: &["image/png", "image/jpeg"], allowed_prefixes: &[], max_bytes: MAX_BYTES_TINY };

    #[test]
    fn filenames_are_sanitised_like_python() {
        assert_eq!(safe_filename(Some(" my sig (1).png ")), "my_sig_1_.png");
        assert_eq!(safe_filename(Some("../../etc/passwd")), "____etc_passwd");
        assert_eq!(safe_filename(Some("...")), "_.");
        assert_eq!(safe_filename(Some("a..b")), "a_b");
        assert_eq!(safe_filename(None), "file");
    }

    #[test]
    fn the_sniffed_type_wins_and_markup_is_refused() {
        assert_eq!(
            resolve_mime(b"\x89PNG\r\n\x1a\nxxxx", "application/octet-stream", RULES).expect("png"),
            "image/png"
        );
        assert_eq!(
            resolve_mime(b"%PDF-1.4", "application/pdf", RULES).unwrap_err().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            resolve_mime(b"  <svg xmlns='x'/>", "image/png", RULES).unwrap_err().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(resolve_mime(b"plain", "image/png", RULES).expect("declared"), "image/png");
    }
}
