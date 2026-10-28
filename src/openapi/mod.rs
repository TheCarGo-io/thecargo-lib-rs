use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::response::Html;
use axum::routing::get;
use serde_json::{Value, json};
use utoipa::Modify;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};

const SCALAR_PAGE: &str = include_str!("docs.html");

#[derive(Debug, Clone, Copy)]
pub struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "BearerAuth",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("JWT")
                    .description(Some("JWT access token — obtain via POST /auth/api/v1/auth/login"))
                    .build(),
            ),
        );
    }
}

// Keys pydantic writes beside an `anyOf`, not inside its members.
const ANNOTATIONS: [&str; 7] = ["description", "title", "default", "examples", "deprecated", "readOnly", "writeOnly"];

// The document in the shape FastAPI publishes it, so the frontend's generated client does not change:
// an optional value is `anyOf: [T, null]` with its description beside it, an optional parameter
// without a default is nullable, and every operation that takes input may answer 422 with
// HTTPValidationError.
pub fn fastapi_shape(doc: &mut Value) {
    if let Some(paths) = doc.get_mut("paths").and_then(Value::as_object_mut) {
        for operation in paths.values_mut().filter_map(Value::as_object_mut).flat_map(|item| item.values_mut()) {
            shape_operation(operation);
        }
    }
    if let Some(schemas) = doc.pointer_mut("/components/schemas").and_then(Value::as_object_mut) {
        for schema in schemas.values_mut() {
            normalize(schema);
        }
        schemas.entry("HTTPValidationError").or_insert_with(|| {
            json!({"properties": {"detail": {"items": {"$ref": "#/components/schemas/ValidationError"}, "type": "array", "title": "Detail"}},
                   "type": "object", "title": "HTTPValidationError"})
        });
        schemas.entry("ValidationError").or_insert_with(|| {
            json!({"properties": {"loc": {"items": {"anyOf": [{"type": "string"}, {"type": "integer"}]}, "type": "array", "title": "Location"},
                                  "msg": {"type": "string", "title": "Message"}, "type": {"type": "string", "title": "Error Type"},
                                  "input": {"title": "Input"}, "ctx": {"type": "object", "title": "Context"}},
                   "type": "object", "required": ["loc", "msg", "type"], "title": "ValidationError"})
        });
        let mut sorted: Vec<(String, Value)> = std::mem::take(schemas).into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        schemas.extend(sorted);
    }
}

fn shape_operation(operation: &mut Value) {
    let Some(op) = operation.as_object_mut() else { return };
    let mut takes_input = op.contains_key("requestBody");
    if let Some(params) = op.get_mut("parameters").and_then(Value::as_array_mut) {
        takes_input |= !params.is_empty();
        for param in params.iter_mut() {
            let optional = !param.get("required").and_then(Value::as_bool).unwrap_or(false);
            if let Some(schema) = param.get_mut("schema") {
                let defaulted = schema.get("default").is_some();
                let nullable = schema.get("anyOf").is_some() || schema.get("type").is_some_and(Value::is_array);
                if optional && !defaulted && !nullable {
                    let inner = std::mem::take(schema);
                    *schema = json!({"anyOf": [inner, {"type": "null"}]});
                }
                normalize(schema);
            }
        }
    }
    for key in ["requestBody", "responses"] {
        if let Some(part) = op.get_mut(key) {
            walk_content(part);
        }
    }
    if takes_input && let Some(responses) = op.get_mut("responses").and_then(Value::as_object_mut) {
        responses.entry("422").or_insert_with(|| {
            json!({"description": "Validation Error",
                   "content": {"application/json": {"schema": {"$ref": "#/components/schemas/HTTPValidationError"}}}})
        });
    }
}

fn walk_content(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(schema) = map.get_mut("schema") {
                normalize(schema);
            }
            for (key, item) in map.iter_mut() {
                if key != "schema" {
                    walk_content(item);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(walk_content),
        _ => {}
    }
}

fn is_null(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

fn normalize(schema: &mut Value) {
    let Some(map) = schema.as_object_mut() else { return };
    for key in ["properties", "patternProperties"] {
        if let Some(Value::Object(props)) = map.get_mut(key) {
            props.values_mut().for_each(normalize);
        }
    }
    for key in ["items", "additionalProperties", "not"] {
        if let Some(inner) = map.get_mut(key) {
            normalize(inner);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(members)) = map.get_mut(key) {
            members.iter_mut().for_each(normalize);
        }
    }
    // `type: [T, "null"]` becomes `anyOf: [{type: T, ...}, {type: null}]`, annotations kept outside.
    if let Some(Value::Array(types)) = map.get("type").cloned() {
        let others: Vec<Value> = types.iter().filter(|t| t.as_str() != Some("null")).cloned().collect();
        if others.len() == 1 && others.len() < types.len() {
            let mut inner = serde_json::Map::new();
            let mut outer = serde_json::Map::new();
            for (key, value) in std::mem::take(map) {
                if ANNOTATIONS.contains(&key.as_str()) {
                    outer.insert(key, value);
                } else {
                    inner.insert(key, value);
                }
            }
            inner.insert("type".to_owned(), others[0].clone());
            outer.insert("anyOf".to_owned(), json!([Value::Object(inner), {"type": "null"}]));
            *map = outer;
            return;
        }
    }
    // utoipa writes an optional reference as `oneOf: [{$ref, description}, {type: null}]`.
    if let Some(Value::Array(mut members)) = map.get("oneOf").cloned()
        && members.iter().any(is_null)
    {
        for member in &mut members {
            if let Some(obj) = member.as_object_mut() {
                for key in ANNOTATIONS {
                    if let Some(value) = obj.remove(key) {
                        map.entry(key.to_owned()).or_insert(value);
                    }
                }
            }
        }
        map.remove("oneOf");
        let rest: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
        map.insert("anyOf".to_owned(), Value::Array(members));
        map.extend(rest);
    }
}

pub fn docs_router<S>(spec: Value) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let spec = Arc::new(spec);
    Router::new()
        .route("/docs", get(|| async { Html(SCALAR_PAGE) }))
        .route("/openapi.json", get(move || async move { Json(spec) }))
}
