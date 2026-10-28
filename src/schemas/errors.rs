use serde::Serialize;
use serde_json::{Map, Value};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ValidationErrorItem {
    pub loc: String,
    pub code: String,
    pub key: String,
    pub message: String,
    #[schema(required = false)]
    pub params: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub code: String,
    pub key: String,
    pub message: String,
    #[schema(required = false)]
    pub params: Map<String, Value>,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<ValidationErrorItem>>,
}
