use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use chrono::Utc;
use serde::Serialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::clients::isoformat;
use crate::context::CompanyRef;
use crate::dependencies::auth::{AuthState, bearer_token, decode_claims};
#[cfg(feature = "rabbit")]
use crate::events::publisher::Publisher;
use crate::middleware::audit::RequestId;

const LABEL_MAX: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditUser {
    pub id: Option<Uuid>,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
}

impl Default for AuditUser {
    fn default() -> Self {
        Self { id: None, email: None, first_name: None, last_name: None, kind: "system".to_owned() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditContext {
    pub user: AuditUser,
    pub organization_id: Option<Uuid>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub request_id: Option<Uuid>,
}

fn names(claims: &Map<String, Value>) -> (Option<String>, Option<String>) {
    let text =
        |key: &str| claims.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let (first, last) = (text("first_name"), text("last_name"));
    if first.is_some() || last.is_some() {
        return (first, last);
    }
    let Some(name) = text("name") else { return (None, None) };
    let mut parts = name.split_whitespace();
    let first = parts.next().map(str::to_owned);
    let rest: Vec<&str> = parts.collect();
    (first, (!rest.is_empty()).then(|| rest.join(" ")))
}

impl AuditContext {
    pub fn from_parts(parts: &Parts, auth: &AuthState) -> Self {
        let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok());
        let ip_address = header("x-forwarded-for")
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        let user_agent = header("user-agent").map(|ua| ua.chars().take(LABEL_MAX).collect());
        let request_id = parts.extensions.get::<RequestId>().map(|id| id.0);
        let mut context =
            Self { user: AuditUser::default(), organization_id: None, ip_address, user_agent, request_id };
        if let Ok(token) = bearer_token(&parts.headers)
            && let Some(claims) = decode_claims(token, auth.keys.decoding())
        {
            let uuid = |key: &str| claims.get(key).and_then(Value::as_str).and_then(|raw| Uuid::parse_str(raw).ok());
            let (first_name, last_name) = names(&claims);
            context.user = AuditUser {
                id: uuid("user_id"),
                email: claims.get("email").and_then(Value::as_str).map(str::to_owned),
                first_name,
                last_name,
                kind: "user".to_owned(),
            };
            context.organization_id = uuid("org_id");
        }
        context
    }
}

impl<S> FromRequestParts<S> for AuditContext
where
    AuthState: axum::extract::FromRef<S>,
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = AuthState::from_ref(state);
        Ok(Self::from_parts(parts, &auth))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Update,
    Delete,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEvent {
    pub action: Action,
    pub resource: &'static str,
    pub resource_id: Uuid,
    pub resource_label: Option<String>,
    pub root: Option<(&'static str, Uuid)>,
    pub organization_id: Option<Uuid>,
    pub company: CompanyRef,
    pub old_data: Option<Map<String, Value>>,
    pub new_data: Option<Map<String, Value>>,
    pub changed_fields: Option<Vec<String>>,
    pub significant: &'static [&'static str],
    pub lifecycle_field: Option<&'static str>,
    pub lifecycle_value: Option<Value>,
    pub id_unassigned: bool,
}

impl AuditEvent {
    pub fn created(resource: &'static str, resource_id: Uuid, snapshot: Map<String, Value>) -> Self {
        Self {
            action: Action::Create,
            resource,
            resource_id,
            resource_label: None,
            root: None,
            organization_id: None,
            company: CompanyRef::Any,
            old_data: None,
            new_data: Some(snapshot),
            changed_fields: None,
            significant: &[],
            lifecycle_field: None,
            lifecycle_value: None,
            id_unassigned: false,
        }
    }

    pub fn deleted(resource: &'static str, resource_id: Uuid, snapshot: Map<String, Value>) -> Self {
        Self {
            action: Action::Delete,
            old_data: Some(snapshot),
            new_data: None,
            ..Self::created(resource, resource_id, Map::new())
        }
    }

    pub fn updated(
        resource: &'static str,
        resource_id: Uuid,
        old: Map<String, Value>,
        new: Map<String, Value>,
        changed: Vec<String>,
    ) -> Self {
        Self {
            action: Action::Update,
            old_data: Some(old),
            new_data: Some(new),
            changed_fields: Some(changed),
            ..Self::created(resource, resource_id, Map::new())
        }
    }

    fn significant_fields(&self) -> Vec<String> {
        let Some(changed) = &self.changed_fields else { return Vec::new() };
        let mut out: Vec<String> =
            changed.iter().filter(|field| self.significant.contains(&field.as_str())).cloned().collect();
        out.sort();
        out.dedup();
        out
    }

    fn lifecycle_state(&self) -> Value {
        let Some(field) = self.lifecycle_field else { return Value::Null };
        if let Some(value) = &self.lifecycle_value {
            return value.clone();
        }
        self.new_data
            .as_ref()
            .or(self.old_data.as_ref())
            .and_then(|data| data.get(field))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn lifecycle_transition(&self) -> Value {
        let (Some(field), Some(changed)) = (self.lifecycle_field, &self.changed_fields) else { return Value::Null };
        if !changed.iter().any(|c| c == field) {
            return Value::Null;
        }
        let pick = |data: &Option<Map<String, Value>>| {
            data.as_ref().and_then(|d| d.get(field)).cloned().unwrap_or(Value::Null)
        };
        json!({"field": field, "from": pick(&self.old_data), "to": pick(&self.new_data)})
    }

    pub fn payload(&self, service: &str, context: &AuditContext) -> Value {
        let organization_id = self.organization_id.or(context.organization_id);
        let company_id = match self.company {
            CompanyRef::Company(id) => Some(id.to_string()),
            CompanyRef::Organization | CompanyRef::Any => None,
        };
        let own_id = (!self.id_unassigned).then(|| self.resource_id.to_string());
        let (root_resource, root_id) =
            self.root.map_or((self.resource, own_id.clone()), |(resource, id)| (resource, Some(id.to_string())));
        json!({
            "audit_id": Uuid::new_v4().to_string(),
            "service": service,
            "organization_id": organization_id.map(|id| id.to_string()),
            "company_id": company_id,
            "user": context.user,
            "resource": self.resource,
            "resource_id": own_id,
            "resource_label": self.resource_label.as_ref().map(|label| label.chars().take(LABEL_MAX).collect::<String>()),
            "root_resource": root_resource,
            "root_id": root_id,
            "action": self.action.name(),
            "changed_fields": self.changed_fields,
            "significant_fields": self.significant_fields(),
            "lifecycle_state": self.lifecycle_state(),
            "lifecycle_transition": self.lifecycle_transition(),
            "old_data": self.old_data,
            "new_data": self.new_data,
            "request_id": context.request_id.map(|id| id.to_string()),
            "ip_address": context.ip_address,
            "user_agent": context.user_agent,
            "created_at": isoformat(Utc::now()),
        })
    }
}

#[cfg(feature = "rabbit")]
pub async fn publish(publisher: &Publisher, service: &str, context: &AuditContext, events: &[AuditEvent]) {
    let keys: Vec<String> = events.iter().map(|event| format!("audit.{service}.{}", event.action.name())).collect();
    let messages: Vec<(&str, Value)> =
        keys.iter().map(String::as_str).zip(events.iter().map(|event| event.payload(service, context))).collect();
    match publisher.publish_many(&messages).await {
        Ok(outcomes) => {
            for (routing_key, outcome) in keys.iter().zip(outcomes) {
                if let Err(err) = outcome {
                    tracing::error!(routing_key, error = %err, "audit publish failed");
                }
            }
        }
        Err(err) => tracing::error!(error = %err, count = events.len(), "audit publish failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_split_like_python() {
        let mut claims = Map::new();
        claims.insert("name".into(), Value::String("Ada Lovelace King".into()));
        assert_eq!(names(&claims), (Some("Ada".into()), Some("Lovelace King".into())));
        claims.insert("first_name".into(), Value::String(" Grace ".into()));
        assert_eq!(names(&claims), (Some("Grace".into()), None));
    }

    #[test]
    fn the_payload_carries_lifecycle_and_significant_fields() {
        let mut old = Map::new();
        old.insert("stage".into(), Value::String("quote".into()));
        old.insert("tariff".into(), Value::from(900.0));
        let mut new = Map::new();
        new.insert("stage".into(), Value::String("order".into()));
        new.insert("tariff".into(), Value::from(950.0));
        let mut event = AuditEvent::updated("shipment", Uuid::nil(), old, new, vec!["tariff".into(), "stage".into()]);
        event.significant = &["stage", "status", "tariff", "carrier_id", "carrier_pay"];
        event.lifecycle_field = Some("stage");
        event.resource_label = Some("Shipment TH10100".into());
        let payload = event.payload("shipment", &AuditContext::default());
        assert_eq!(payload["action"], "update");
        assert_eq!(payload["significant_fields"], json!(["stage", "tariff"]));
        assert_eq!(payload["lifecycle_state"], "order");
        assert_eq!(payload["lifecycle_transition"], json!({"field": "stage", "from": "quote", "to": "order"}));
        assert_eq!(payload["root_resource"], "shipment");
        assert_eq!(payload["user"]["type"], "system");
    }

    #[test]
    fn an_unassigned_id_is_sent_as_null_like_a_python_pending_row() {
        let mut event = AuditEvent::created("carrier", Uuid::new_v4(), Map::new());
        event.id_unassigned = true;
        let payload = event.payload("shipment", &AuditContext::default());
        assert_eq!((&payload["resource_id"], &payload["root_id"]), (&Value::Null, &Value::Null));
    }

    #[test]
    fn the_lifecycle_value_wins_when_the_field_did_not_change() {
        let mut event = AuditEvent::updated("shipment", Uuid::nil(), Map::new(), Map::new(), vec!["status".into()]);
        event.lifecycle_field = Some("stage");
        event.lifecycle_value = Some(Value::String("order".into()));
        assert_eq!(event.payload("shipment", &AuditContext::default())["lifecycle_state"], "order");
    }
}
