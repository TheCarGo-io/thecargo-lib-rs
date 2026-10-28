use std::any::Any;
use std::borrow::Cow;
use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::i18n::{self, DEFAULT_LANG, Lang};
use crate::schemas::errors::{ErrorResponse, ValidationErrorItem};

#[derive(Debug, Clone)]
pub struct FieldError {
    pub loc: String,
    pub code: &'static str,
    pub key: &'static str,
    pub params: Map<String, Value>,
    pub fallback: String,
}

// Boxed so a Result<T, ApiError> stays one pointer wide on the happy path.
#[derive(Debug, Clone)]
pub struct ApiError(Box<Inner>);

#[derive(Debug, Clone)]
struct Inner {
    status: StatusCode,
    code: Cow<'static, str>,
    key: Cow<'static, str>,
    params: Map<String, Value>,
    fallback: Cow<'static, str>,
    fields: Option<Vec<FieldError>>,
}

impl ApiError {
    // HTTPException(status, "text"): the code and key come from the status.
    pub fn http(status: StatusCode, detail: impl Into<Cow<'static, str>>) -> Self {
        let number = status.as_u16();
        Self::app(status, format!("HTTP_{number}"), format!("common.http_{number}"), detail)
    }

    // HTTPException(status, {"code": ..., "message": ...}): the dict's code lands in params.
    pub fn http_coded(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        let mut error = Self::http(status, message);
        error.0.params.insert("code".to_owned(), Value::from(code));
        error
    }

    pub fn app(
        status: StatusCode,
        code: impl Into<Cow<'static, str>>,
        key: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self(Box::new(Inner {
            status,
            code: code.into(),
            key: key.into(),
            params: Map::new(),
            fallback: message.into(),
            fields: None,
        }))
    }

    #[must_use]
    pub fn with_params(mut self, params: Map<String, Value>) -> Self {
        self.0.params = params;
        self
    }

    pub fn bad_request() -> Self {
        Self::app(StatusCode::BAD_REQUEST, "BAD_REQUEST", "common.bad_request", "Bad request")
    }

    pub fn unauthorized() -> Self {
        Self::app(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "auth.unauthorized", "Missing or invalid access token")
    }

    pub fn forbidden() -> Self {
        Self::app(StatusCode::FORBIDDEN, "FORBIDDEN", "auth.forbidden", "Insufficient permissions")
    }

    // The Python NotFoundException answers 400, not 404; clients already depend on that.
    pub fn not_found() -> Self {
        Self::app(StatusCode::BAD_REQUEST, "NOT_FOUND", "common.not_found", "Not found")
    }

    pub fn conflict() -> Self {
        Self::app(StatusCode::CONFLICT, "CONFLICT", "common.conflict", "Conflict")
    }

    pub fn validation(fields: Vec<FieldError>) -> Self {
        let mut error = Self::app(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            "validation.multiple_errors",
            "Validation failed",
        );
        error.0.fields = Some(fields);
        error
    }

    pub fn internal() -> Self {
        Self::app(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", "common.internal_error", "Internal server error")
    }

    pub fn internal_from(error: &(dyn std::error::Error + 'static)) -> Self {
        tracing::error!(error = %ErrorChain(error), "request failed");
        Self::internal()
    }

    pub fn upstream_status(status: StatusCode, detail: impl Into<Cow<'static, str>>) -> Self {
        Self::http(status, detail)
    }

    pub fn upstream_unavailable(service: &str) -> Self {
        let mut params = Map::new();
        params.insert("service".to_owned(), Value::from(service));
        Self::app(
            StatusCode::BAD_GATEWAY,
            "UPSTREAM_UNAVAILABLE",
            "common.upstream_unavailable",
            "Upstream service is unavailable",
        )
        .with_params(params)
    }

    pub fn status(&self) -> StatusCode {
        self.0.status
    }

    pub fn code(&self) -> &str {
        &self.0.code
    }

    pub fn params(&self) -> &Map<String, Value> {
        &self.0.params
    }

    pub fn render(&self, lang: Lang) -> ErrorResponse {
        let inner = &self.0;
        let message = i18n::translate(&inner.key, lang, &inner.params).unwrap_or_else(|| inner.fallback.to_string());
        ErrorResponse {
            code: inner.code.to_string(),
            key: inner.key.to_string(),
            message: message.clone(),
            params: inner.params.clone(),
            detail: message,
            errors: inner.fields.as_ref().map(|fields| {
                fields
                    .iter()
                    .map(|field| ValidationErrorItem {
                        loc: field.loc.clone(),
                        code: field.code.to_owned(),
                        key: field.key.to_owned(),
                        message: i18n::translate(field.key, lang, &field.params)
                            .unwrap_or_else(|| field.fallback.clone()),
                        params: field.params.clone(),
                    })
                    .collect()
            }),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status(), Json(self.render(DEFAULT_LANG))).into_response();
        response.extensions_mut().insert(Arc::new(self));
        response
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.0.status.as_u16(), self.0.code, self.0.fallback)
    }
}

impl std::error::Error for ApiError {}

// A store failing is the request's 500, logged once here with its cause; the body stays generic.
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::internal_from(&error)
    }
}

#[cfg(feature = "clickhouse")]
impl From<clickhouse::error::Error> for ApiError {
    fn from(error: clickhouse::error::Error) -> Self {
        Self::internal_from(&error)
    }
}

struct ErrorChain<'a>(&'a (dyn std::error::Error + 'static));

impl std::fmt::Display for ErrorChain<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(cause) = source {
            write!(f, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

// Error bodies are rendered in English when they are built; this re-renders them in the
// language the request asked for, the way the Python handlers read Accept-Language.
pub async fn localize(request: Request, next: Next) -> Response {
    let lang = Lang::from_accept_language(request.headers().get(header::ACCEPT_LANGUAGE).and_then(|v| v.to_str().ok()));
    let mut response = next.run(request).await;
    let Some(error) = response.extensions_mut().remove::<Arc<ApiError>>() else {
        return response;
    };
    if lang == DEFAULT_LANG {
        return response;
    }
    match serde_json::to_vec(&error.render(lang)) {
        Ok(body) => {
            response.headers_mut().remove(header::CONTENT_LENGTH);
            *response.body_mut() = Body::from(body);
        }
        Err(err) => tracing::warn!(error = %err, "could not render a localised error body"),
    }
    response
}

// Starlette's own 404 and 405 never reach the lib's handlers, so they keep FastAPI's plain shape.
#[expect(clippy::unused_async, reason = "an axum handler")]
pub async fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"detail": "Not Found"}))).into_response()
}

#[expect(clippy::unused_async, reason = "an axum handler")]
pub async fn method_not_allowed(response: Response) -> Response {
    if response.status() != StatusCode::METHOD_NOT_ALLOWED || response.headers().contains_key(header::CONTENT_TYPE) {
        return response;
    }
    let (mut parts, _) = response.into_parts();
    parts.headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
    parts.headers.remove(header::CONTENT_LENGTH);
    if let Some(allow) = parts.headers.get(header::ALLOW).and_then(|value| value.to_str().ok()).map(fastapi_allow)
        && let Ok(value) = header::HeaderValue::from_str(&allow)
    {
        parts.headers.insert(header::ALLOW, value);
    }
    Response::from_parts(parts, Body::from(r#"{"detail":"Method Not Allowed"}"#))
}

// axum answers HEAD for every GET route and lists it; FastAPI routes do neither.
fn fastapi_allow(axum_allow: &str) -> String {
    axum_allow.split(',').map(str::trim).filter(|method| *method != "HEAD").collect::<Vec<_>>().join(", ")
}

#[expect(clippy::needless_pass_by_value, reason = "CatchPanicLayer hands the payload over by value")]
pub fn panic_response(payload: Box<dyn Any + Send + 'static>) -> Response {
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    metrics::counter!("http_exceptions_total", "exception" => "panic").increment(1);
    tracing::error!(panic = %message, "request handler panicked");
    ApiError::internal().into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coded_http_error_keeps_its_code_in_params() {
        let error = ApiError::http_coded(StatusCode::UNAUTHORIZED, "token_revoked", "Session revoked");
        let body = serde_json::to_value(error.render(Lang::En)).unwrap();
        assert_eq!(body["code"], "HTTP_401");
        assert_eq!(body["key"], "common.http_401");
        assert_eq!(body["params"], json!({"code": "token_revoked"}));
        assert!(body.get("errors").is_none());
    }

    #[test]
    fn without_a_translation_the_message_is_the_detail() {
        let body =
            serde_json::to_value(ApiError::http(StatusCode::IM_A_TEAPOT, "short and stout").render(Lang::Ru)).unwrap();
        assert_eq!(body["message"], "short and stout");
        assert_eq!(body["detail"], "short and stout");
    }

    #[test]
    fn a_validation_error_lists_its_fields() {
        let mut params = Map::new();
        params.insert("field".to_owned(), Value::from("query.period"));
        let error = ApiError::validation(vec![FieldError {
            loc: "query.period".to_owned(),
            code: "FIELD_REQUIRED",
            key: "validation.field_required",
            params,
            fallback: "Field required".to_owned(),
        }]);
        let body = serde_json::to_value(error.render(Lang::En)).unwrap();
        assert_eq!(body["code"], "VALIDATION_ERROR");
        assert_eq!(body["errors"][0]["loc"], "query.period");
        assert_eq!(body["errors"][0]["code"], "FIELD_REQUIRED");
    }
}
