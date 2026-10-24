use std::borrow::Cow;

use axum::http::StatusCode;

use crate::handlers::ApiError;

pub fn or_400<T>(value: Option<T>, message: impl Into<Cow<'static, str>>) -> Result<T, ApiError> {
    or_status(value, StatusCode::BAD_REQUEST, message)
}

pub fn or_status<T>(
    value: Option<T>,
    status: StatusCode,
    message: impl Into<Cow<'static, str>>,
) -> Result<T, ApiError> {
    value.ok_or_else(|| ApiError::http(status, message))
}
