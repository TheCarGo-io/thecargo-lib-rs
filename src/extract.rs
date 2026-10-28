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
                use axum::extract::path::ErrorKind;
                let (key, reason) = match err.kind() {
                    ErrorKind::DeserializeError { key, value, message } => (key.clone(), path_reason(value, message)),
                    ErrorKind::ParseErrorAtKey { key, expected_type, .. } => {
                        (key.clone(), parse_reason(expected_type).to_owned())
                    }
                    ErrorKind::ParseError { expected_type, .. } => {
                        (String::new(), parse_reason(expected_type).to_owned())
                    }
                    ErrorKind::InvalidUtf8InPathParam { key } => (key.clone(), err.body_text()),
                    _ => (String::new(), err.body_text()),
                };
                Err(invalid("path", &key, &reason))
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
        Ok(Json(from_slice(&bytes)?))
    }
}

pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
    from_slice_at("", bytes)
}

fn json_offset(bytes: &[u8], line: usize, column: usize) -> usize {
    let text = String::from_utf8_lossy(bytes);
    let skipped: usize = text.split('\n').take(line.saturating_sub(1)).map(|row| row.chars().count() + 1).sum();
    skipped + column.saturating_sub(1)
}

pub fn from_slice_at<T: DeserializeOwned>(prefix: &str, bytes: &[u8]) -> Result<T, ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        let mut violations = Violations::default();
        violations.missing(prefix);
        return Err(violations.finish().err().unwrap_or_else(ApiError::internal));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|err| {
        if err.inner().is_syntax() || err.inner().is_eof() {
            let offset = json_offset(bytes, err.inner().line(), err.inner().column());
            return invalid(prefix, &offset.to_string(), "JSON decode error");
        }
        let message = err.inner().to_string();
        let message = if message.starts_with("UUID parsing failed") {
            raw_string_at(bytes, err.path())
                .map_or(message, |raw| format!("Input should be a valid UUID, {}", uuid_error(&raw)))
        } else {
            message
        };
        invalid(prefix, &err.path().to_string(), &message)
    })?;
    deserializer.end().map_err(|err| invalid(prefix, "", &err.to_string()))?;
    Ok(value)
}

fn raw_string_at(bytes: &[u8], path: &serde_path_to_error::Path) -> Option<String> {
    let document: Value = serde_json::from_slice(bytes).ok()?;
    let mut cursor = &document;
    for segment in path {
        cursor = match segment {
            serde_path_to_error::Segment::Map { key } => cursor.get(key)?,
            serde_path_to_error::Segment::Seq { index } => cursor.get(*index)?,
            serde_path_to_error::Segment::Enum { .. } | serde_path_to_error::Segment::Unknown => return None,
        };
    }
    cursor.as_str().map(str::to_owned)
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

fn invalid(prefix: &str, path: &str, message: &str) -> ApiError {
    let reason = pydantic_reason(message);
    let message = reason.as_str();
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

fn pydantic_reason(message: &str) -> String {
    let message = message.rfind(" at line ").map_or(message, |idx| &message[..idx]);
    if message.starts_with("unknown variant `")
        && let Some((_, expected)) = message.split_once(", expected ")
    {
        let choices: Vec<&str> = expected.trim_start_matches("one of ").split('`').skip(1).step_by(2).collect();
        return format!("Input should be {}", quoted_choices(&choices));
    }
    if message.starts_with("invalid type: ")
        && let Some((_, expected)) = message.split_once(", expected ")
    {
        return type_reason(expected).to_owned();
    }
    message.to_owned()
}

fn type_reason(expected: &str) -> &'static str {
    if expected.contains("string") {
        "Input should be a valid string"
    } else if expected.contains("boolean") {
        "Input should be a valid boolean, unable to interpret input"
    } else if expected.contains("f32") || expected.contains("f64") || expected.contains("float") {
        "Input should be a valid number"
    } else if expected.starts_with('i') || expected.starts_with('u') || expected.contains("integer") {
        "Input should be a valid integer"
    } else if expected.contains("sequence") || expected.contains("array") {
        "Input should be a valid list"
    } else if expected.contains("map") || expected.starts_with("struct") {
        "Input should be a valid dictionary or object to extract fields from"
    } else {
        "Input should be a valid value"
    }
}

fn path_reason(value: &str, message: &str) -> String {
    if message.contains("UUID parsing failed") {
        return format!("Input should be a valid UUID, {}", uuid_error(value));
    }
    message.to_owned()
}

fn parse_reason(expected_type: &str) -> &'static str {
    match type_reason(expected_type) {
        "Input should be a valid integer" => "Input should be a valid integer, unable to parse string as an integer",
        "Input should be a valid number" => "Input should be a valid number, unable to parse string as a number",
        other => other,
    }
}

pub fn out_of_range(loc: &str, too_small: bool, bound: Value, strict: bool) -> ApiError {
    ApiError::validation(vec![range_error(loc, too_small, bound, strict)])
}

pub fn range_error(loc: &str, too_small: bool, bound: Value, strict: bool) -> FieldError {
    let comparison = match (too_small, strict) {
        (true, true) => "greater than",
        (true, false) => "greater than or equal to",
        (false, true) => "less than",
        (false, false) => "less than or equal to",
    };
    let fallback = format!("Input should be {comparison} {bound}");
    let mut params = Map::new();
    params.insert("field".to_owned(), Value::from(loc));
    params.insert("limit".to_owned(), if strict { bound } else { Value::Null });
    let (code, key) =
        if too_small { ("TOO_SMALL", "validation.too_small") } else { ("TOO_LARGE", "validation.too_large") };
    FieldError { loc: loc.to_owned(), code, key, params, fallback }
}

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

pub fn value_error(loc: &str, reason: &str) -> FieldError {
    let mut params = Map::new();
    params.insert("field".to_owned(), Value::from(loc));
    params.insert("reason".to_owned(), Value::from(reason));
    FieldError {
        loc: loc.to_owned(),
        code: "VALUE_ERROR",
        key: "validation.value_error",
        params,
        fallback: format!("Value error, {reason}"),
    }
}

pub fn pattern_mismatch(loc: &str, pattern: &str) -> FieldError {
    let mut params = Map::new();
    params.insert("field".to_owned(), Value::from(loc));
    FieldError {
        loc: loc.to_owned(),
        code: "PATTERN_MISMATCH",
        key: "validation.pattern_mismatch",
        params,
        fallback: format!("String should match pattern '{pattern}'"),
    }
}

#[derive(Debug, Default)]
pub struct Violations(Vec<FieldError>);

impl Violations {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &FieldError> {
        self.0.iter()
    }

    pub fn push(&mut self, field: FieldError) {
        self.0.push(field);
    }

    pub fn string(&mut self, loc: &str, value: &Value) -> Option<String> {
        if let Value::String(text) = value {
            return Some(text.clone());
        }
        self.0.push(invalid_value(loc, type_reason("string")));
        None
    }

    pub fn object<'a>(&mut self, loc: &str, value: &'a Value) -> Option<&'a Map<String, Value>> {
        if let Value::Object(map) = value {
            return Some(map);
        }
        self.0.push(invalid_value(loc, type_reason("map")));
        None
    }

    pub fn range(&mut self, loc: &str, value: i64, min: Option<i64>, max: Option<i64>) {
        if let Some(min) = min.filter(|min| value < *min) {
            self.0.push(range_error(loc, true, Value::from(min), false));
        } else if let Some(max) = max.filter(|max| value > *max) {
            self.0.push(range_error(loc, false, Value::from(max), false));
        }
    }

    pub fn range_f64(&mut self, loc: &str, value: f64, min: Option<f64>, max: Option<f64>) {
        if let Some(min) = min.filter(|min| value < *min) {
            self.0.push(range_error(loc, true, Value::from(min), false));
        } else if let Some(max) = max.filter(|max| value > *max) {
            self.0.push(range_error(loc, false, Value::from(max), false));
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
    #[test]
    fn malformed_and_empty_bodies_read_like_fastapi() {
        let error = from_slice::<serde_json::Value>(b"{not json").unwrap_err();
        let rendered = serde_json::to_value(error.render(crate::i18n::Lang::En)).unwrap();
        assert_eq!(rendered["errors"][0]["loc"], "1");
        assert_eq!(rendered["errors"][0]["message"], "JSON decode error");
        let error = from_slice::<serde_json::Value>(b"{\n  \"a\": 1,\n  oops").unwrap_err();
        let rendered = serde_json::to_value(error.render(crate::i18n::Lang::En)).unwrap();
        assert_eq!(rendered["errors"][0]["loc"], "14");
        let error = from_slice::<serde_json::Value>(b"  ").unwrap_err();
        let rendered = serde_json::to_value(error.render(crate::i18n::Lang::En)).unwrap();
        assert_eq!(rendered["errors"][0]["message"], "Field required");
        assert_eq!(rendered["errors"][0]["loc"], "");
    }

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
    fn serde_type_errors_read_like_pydantic() {
        assert_eq!(
            pydantic_reason("invalid type: null, expected a string at line 1 column 10"),
            "Input should be a valid string"
        );
        assert_eq!(pydantic_reason("invalid type: string \"x\", expected i64"), "Input should be a valid integer");
        assert_eq!(
            pydantic_reason("invalid type: string \"x\", expected a boolean"),
            "Input should be a valid boolean, unable to interpret input"
        );
        assert_eq!(pydantic_reason("invalid type: integer `1`, expected a sequence"), "Input should be a valid list");
    }

    #[test]
    fn an_unknown_enum_variant_reads_as_pydantic_reports_it() {
        assert_eq!(
            pydantic_reason("unknown variant `x`, expected one of `OPEN`, `ENCLOSED`, `DRIVE_AWAY` at line 1 column 9"),
            "Input should be 'OPEN', 'ENCLOSED' or 'DRIVE_AWAY'"
        );
        assert_eq!(
            pydantic_reason("unknown variant `x`, expected `pickup` or `delivery`"),
            "Input should be 'pickup' or 'delivery'"
        );
        assert_eq!(pydantic_reason("missing field `name` at line 1 column 12"), "missing field `name`");
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

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn a_malformed_uuid_path_param_reads_like_pydantic() {
        assert_eq!(
            path_reason("not-a-uuid", "UUID parsing failed: invalid character"),
            "Input should be a valid UUID, invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `n` at 1"
        );
        assert_eq!(parse_reason("u32"), "Input should be a valid integer, unable to parse string as an integer");
    }
}

#[cfg(test)]
mod uuid_body_tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct Body {
        #[expect(dead_code, reason = "deserialized to trigger the error")]
        shipment_id: uuid::Uuid,
    }

    #[tokio::test]
    async fn a_malformed_uuid_in_a_body_reads_like_pydantic() {
        use axum::response::IntoResponse;
        let error = from_slice::<Body>(br#"{"shipment_id": "x"}"#).expect_err("rejected");
        let bytes = axum::body::to_bytes(error.into_response().into_body(), usize::MAX).await.expect("body");
        let body: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["errors"][0]["loc"], "shipment_id");
        assert_eq!(
            body["errors"][0]["message"],
            "Input should be a valid UUID, invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `x` at 1"
        );
    }
}
