use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::http::header;
use axum::http::request::Parts;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::handlers::{ApiError, FieldError};

#[derive(Debug, Clone, Copy, Default)]
pub struct Query<T>(pub T);

impl<T, S> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let raw = parts.uri.query().unwrap_or_default();
        let deserializer = serde_html_form::Deserializer::from_bytes(raw.as_bytes());
        serde_path_to_error::deserialize(deserializer)
            .map(Query)
            .map_err(|err| invalid("query", &err.path().to_string(), &err.inner().to_string()))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Path<T>(pub T);

impl<T, S> FromRequestParts<S> for Path<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(Path(value)),
            Err(PathRejection::FailedToDeserializePathParams(err)) => {
                let key = match err.kind() {
                    axum::extract::path::ErrorKind::ParseErrorAtKey { key, .. }
                    | axum::extract::path::ErrorKind::InvalidUtf8InPathParam { key } => key.clone(),
                    _ => String::new(),
                };
                Err(invalid("path", &key, &err.body_text()))
            }
            Err(other) => {
                tracing::error!(error = %other, "path parameters are missing from a matched route");
                Err(ApiError::internal())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Json<T>(pub T);

impl<T, S> FromRequest<S> for Json<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        if !is_json(&request) {
            return Err(invalid("", "", "Expected request with `Content-Type: application/json`"));
        }
        let bytes =
            Bytes::from_request(request, state).await.map_err(|err| ApiError::http(err.status(), err.body_text()))?;
        let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
        let value = serde_path_to_error::deserialize(&mut deserializer)
            .map_err(|err| invalid("", &err.path().to_string(), &err.inner().to_string()))?;
        deserializer.end().map_err(|err| invalid("", "", &err.to_string()))?;
        Ok(Json(value))
    }
}

fn is_json(request: &Request) -> bool {
    request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| {
            let mime = mime.trim().to_ascii_lowercase();
            mime == "application/json" || (mime.starts_with("application/") && mime.ends_with("+json"))
        })
}

// FastAPI joins the location with dots and drops the "body" segment; serde reports a missing
// field on its parent, so the field's name is taken from the message.
fn invalid(prefix: &str, path: &str, message: &str) -> ApiError {
    let missing = message.strip_prefix("missing field `").and_then(|rest| rest.split('`').next());
    let segments = [prefix, path.trim_matches('.'), missing.unwrap_or_default()];
    let loc = segments.iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(".");

    let mut params = Map::new();
    params.insert("field".to_owned(), Value::from(loc.clone()));
    let field = if missing.is_some() {
        FieldError {
            loc,
            code: "FIELD_REQUIRED",
            key: "validation.field_required",
            params,
            fallback: "Field required".into(),
        }
    } else {
        params.insert("reason".to_owned(), Value::from(message));
        FieldError { loc, code: "INVALID", key: "validation.invalid_value", params, fallback: message.to_owned() }
    };
    ApiError::validation(vec![field])
}

// pydantic's ge/le (and gt/lt) failures as the lib's handler classifies them: only gt/lt put their
// bound in `limit`, so a ge/le failure reports `limit: None`, exactly as Python does.
pub fn out_of_range(loc: &str, too_small: bool, strict_bound: Option<Value>) -> ApiError {
    ApiError::validation(vec![range_error(loc, too_small, strict_bound)])
}

pub fn range_error(loc: &str, too_small: bool, strict_bound: Option<Value>) -> FieldError {
    let mut params = Map::new();
    params.insert("field".to_owned(), Value::from(loc));
    params.insert("limit".to_owned(), strict_bound.unwrap_or(Value::Null));
    let (code, key, fallback) = if too_small {
        ("TOO_SMALL", "validation.too_small", "Input should be greater than or equal to the minimum")
    } else {
        ("TOO_LARGE", "validation.too_large", "Input should be less than or equal to the maximum")
    };
    FieldError { loc: loc.to_owned(), code, key, params, fallback: fallback.to_owned() }
}

// Any other pydantic failure the lib's handler does not classify: its message becomes the reason.
pub fn invalid_value(loc: &str, reason: &str) -> FieldError {
    let mut params = Map::new();
    params.insert("field".to_owned(), Value::from(loc));
    params.insert("reason".to_owned(), Value::from(reason));
    FieldError {
        loc: loc.to_owned(),
        code: "INVALID",
        key: "validation.invalid_value",
        params,
        fallback: reason.to_owned(),
    }
}

// Constraint failures collected in field order, the way pydantic reports every one of them.
#[derive(Debug, Default)]
pub struct Violations(Vec<FieldError>);

impl Violations {
    pub fn range(&mut self, loc: &str, value: i64, min: Option<i64>, max: Option<i64>) {
        if min.is_some_and(|min| value < min) {
            self.0.push(range_error(loc, true, None));
        } else if max.is_some_and(|max| value > max) {
            self.0.push(range_error(loc, false, None));
        }
    }

    pub fn range_f64(&mut self, loc: &str, value: f64, min: Option<f64>, max: Option<f64>) {
        if min.is_some_and(|min| value < min) {
            self.0.push(range_error(loc, true, None));
        } else if max.is_some_and(|max| value > max) {
            self.0.push(range_error(loc, false, None));
        }
    }

    pub fn one_of(&mut self, loc: &str, value: &str, allowed: &[&str]) {
        if !allowed.contains(&value) {
            self.0.push(invalid_value(loc, &format!("Input should be {}", quoted_choices(allowed))));
        }
    }

    pub fn length(&mut self, loc: &str, len: usize, min: usize, max: usize) {
        if len < min {
            let items = if min == 1 { "item" } else { "items" };
            self.0.push(invalid_value(
                loc,
                &format!("List should have at least {min} {items} after validation, not {len}"),
            ));
        } else if len > max {
            let items = if max == 1 { "item" } else { "items" };
            self.0.push(invalid_value(
                loc,
                &format!("List should have at most {max} {items} after validation, not {len}"),
            ));
        }
    }

    pub fn missing(&mut self, loc: &str) {
        let mut params = Map::new();
        params.insert("field".to_owned(), Value::from(loc));
        self.0.push(FieldError {
            loc: loc.to_owned(),
            code: "FIELD_REQUIRED",
            key: "validation.field_required",
            params,
            fallback: "Field required".into(),
        });
    }

    // A required field the request left out; its value when it is there.
    pub fn required<T: Copy>(&mut self, loc: &str, value: Option<T>) -> Option<T> {
        if value.is_none() {
            self.missing(loc);
        }
        value
    }

    pub fn min_chars(&mut self, loc: &str, text: &str, min: usize) {
        if text.chars().count() < min {
            let unit = if min == 1 { "character" } else { "characters" };
            self.0.push(invalid_value(loc, &format!("String should have at least {min} {unit}")));
        }
    }

    pub fn max_chars(&mut self, loc: &str, text: &str, max: usize) {
        if text.chars().count() > max {
            let unit = if max == 1 { "character" } else { "characters" };
            self.0.push(invalid_value(loc, &format!("String should have at most {max} {unit}")));
        }
    }

    pub fn uuid(&mut self, loc: &str, text: &str) -> Option<uuid::Uuid> {
        if let Ok(id) = uuid::Uuid::parse_str(text) {
            return Some(id);
        }
        self.0.push(invalid_value(loc, &format!("Input should be a valid UUID, {}", uuid_error(text))));
        None
    }

    pub fn finish(self) -> Result<(), ApiError> {
        if self.0.is_empty() { Ok(()) } else { Err(ApiError::validation(self.0)) }
    }
}

// Why a string is not a UUID, worded as the uuid crate's 1.21 diagnostics that pydantic-core
// shows; later releases of the crate word it differently. Called only for a string that failed.
pub fn uuid_error(text: &str) -> String {
    const BLOCK_STARTS: [usize; 5] = [0, 9, 14, 19, 24];
    let bytes = text.as_bytes();
    let (inner, offset, simple) = if let [b'{', inner @ .., b'}'] = bytes {
        (&text[1..=inner.len()], 1, false)
    } else if let Some(rest) = text.strip_prefix("urn:uuid:") {
        (rest, "urn:uuid:".len(), false)
    } else {
        (text, 0, true)
    };
    let mut hyphens = 0;
    let mut bounds = [0_usize; 4];
    for (index, character) in inner.char_indices() {
        let is_hex = character.is_ascii_hexdigit();
        if character == '-' {
            if hyphens < 4 {
                bounds[hyphens] = index;
            }
            hyphens += 1;
        } else if !is_hex {
            return format!(
                "invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `{character}` at {}",
                index + offset + 1
            );
        }
    }
    if hyphens == 0 && simple {
        return format!("invalid length: expected length 32 for simple format, found {}", text.len());
    }
    if hyphens != 4 {
        return format!("invalid group count: expected 5, found {}", hyphens + 1);
    }
    let expected = [8, 4, 4, 4, 12];
    for i in 0..4 {
        if bounds[i] != BLOCK_STARTS[i + 1] - 1 {
            return format!(
                "invalid group length in group {i}: expected {}, found {}",
                expected[i],
                bounds[i].saturating_sub(BLOCK_STARTS[i])
            );
        }
    }
    format!("invalid group length in group 4: expected 12, found {}", text.len().saturating_sub(BLOCK_STARTS[4]))
}

// pydantic's enum message: 'a', 'b' or 'c'.
fn quoted_choices(allowed: &[&str]) -> String {
    let quoted: Vec<String> = allowed.iter().map(|v| format!("'{v}'")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

pub fn within(loc: &str, value: i64, min: i64, max: i64) -> Result<i64, ApiError> {
    let mut violations = Violations::default();
    violations.range(loc, value, Some(min), Some(max));
    violations.finish().map(|()| value)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    #[expect(dead_code, reason = "the fields exist to be deserialized")]
    struct Window {
        period: String,
        days: Option<u32>,
    }

    async fn query(raw: &str) -> Result<Query<Window>, ApiError> {
        let request = Request::builder().uri(format!("/x?{raw}")).body(Body::empty()).unwrap();
        let (mut parts, _body) = request.into_parts();
        Query::<Window>::from_request_parts(&mut parts, &()).await
    }

    fn first_field(error: &ApiError) -> serde_json::Value {
        serde_json::to_value(error.render(crate::i18n::Lang::En)).unwrap()["errors"][0].clone()
    }

    #[test]
    fn uuid_diagnostics_match_pydantic_core() {
        let cases = [
            ("1234", "invalid length: expected length 32 for simple format, found 4"),
            ("123e4567-e89b-12d3-a456-42661417400", "invalid group length in group 4: expected 12, found 11"),
            ("123e4567-e89b-12d3-a456-4266141740000", "invalid group length in group 4: expected 12, found 13"),
            (
                "123e4567e89b12d3a45642661417400z",
                "invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `z` at 32",
            ),
            ("123e4567-e89b-12d3-a456", "invalid group count: expected 5, found 4"),
            ("123e4567-e89b-12d3-a4566-426614174000", "invalid group length in group 3: expected 4, found 5"),
            ("", "invalid length: expected length 32 for simple format, found 0"),
            ("12-3e4567-e89b-12d3-a456-42661417400", "invalid group count: expected 5, found 6"),
            (
                "é",
                "invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `é` at 1",
            ),
        ];
        for (text, want) in cases {
            assert_eq!(uuid_error(text), want, "{text:?}");
        }
    }

    #[test]
    fn a_bad_uuid_reads_as_pydantic_reports_it() {
        let mut violations = Violations::default();
        assert!(violations.uuid("organization_id", "nope").is_none());
        let field = first_field(&violations.finish().unwrap_err());
        assert_eq!(
            field["params"]["reason"],
            "Input should be a valid UUID, invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `n` at 1"
        );
    }

    #[tokio::test]
    async fn a_missing_query_field_is_field_required_at_its_location() {
        let error = query("days=3").await.unwrap_err();
        assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        let field = first_field(&error);
        assert_eq!(field["loc"], "query.period");
        assert_eq!(field["code"], "FIELD_REQUIRED");
    }

    #[tokio::test]
    async fn a_malformed_query_value_is_invalid_at_its_location() {
        let field = first_field(&query("period=month&days=many").await.unwrap_err());
        assert_eq!(field["loc"], "query.days");
        assert_eq!(field["code"], "INVALID");
    }

    #[tokio::test]
    async fn a_body_that_is_not_json_is_refused() {
        let request = Request::builder().method("POST").uri("/x").body(Body::from("period=month")).unwrap();
        let error = Json::<Window>::from_request(request, &()).await.unwrap_err();
        assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn a_missing_body_field_has_no_body_prefix() {
        let request = Request::builder()
            .method("POST")
            .uri("/x")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"days": 2}"#))
            .unwrap();
        let field = first_field(&Json::<Window>::from_request(request, &()).await.unwrap_err());
        assert_eq!(field["loc"], "period");
    }
}
