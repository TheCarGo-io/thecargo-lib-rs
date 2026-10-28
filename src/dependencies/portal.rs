use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::StatusCode;
use axum::http::request::Parts;
use jsonwebtoken::DecodingKey;
use redis::aio::ConnectionManager;
use serde_json::Value;
use uuid::Uuid;

use super::auth::{bearer_token, issued_in_future, pyjwt_validation};
use crate::cache::cache_aside;
use crate::clients::{ServiceClient, url};
use crate::handlers::ApiError;

pub const PORTAL_AUDIENCE: &str = "portal";
pub const CUSTOMER_IDS_TTL: u64 = 300;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn customer_ids_key(account_id: Uuid) -> String {
    format!("portal:customers:{account_id}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalToken {
    pub account_id: Uuid,
    pub org_id: Uuid,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerScope {
    pub account_id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    pub customer_ids: Vec<Uuid>,
}

#[derive(Clone)]
pub struct PortalState {
    pub key: Arc<DecodingKey>,
    pub redis: ConnectionManager,
    pub clients: ServiceClient,
    pub auth_url: Arc<str>,
}

impl std::fmt::Debug for PortalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortalState").field("auth_url", &self.auth_url).finish_non_exhaustive()
    }
}

pub fn decode_portal_token(token: &str, key: &DecodingKey) -> Result<PortalToken, ApiError> {
    let invalid = || ApiError::http(StatusCode::UNAUTHORIZED, "Invalid token");
    let mut validation = pyjwt_validation();
    validation.validate_aud = true;
    validation.set_audience(&[PORTAL_AUDIENCE]);
    validation.set_required_spec_claims(&["aud"]);
    let claims = jsonwebtoken::decode::<serde_json::Map<String, Value>>(token, key, &validation)
        .map_err(|_rejected| invalid())?
        .claims;
    if issued_in_future(&claims) || claims.get("type").and_then(Value::as_str) != Some("access") {
        return Err(invalid());
    }
    let id = |name: &str| claims.get(name).and_then(Value::as_str).and_then(|raw| Uuid::parse_str(raw).ok());
    let email = claims.get("email").and_then(Value::as_str).map(str::to_owned);
    match (id("account_id"), id("org_id"), email) {
        (Some(account_id), Some(org_id), Some(email)) => Ok(PortalToken { account_id, org_id, email }),
        _ => Err(invalid()),
    }
}

async fn fetch_customer_ids(state: &PortalState, account_id: Uuid) -> Result<Vec<String>, reqwest::Error> {
    let target = url(&state.auth_url, &format!("/api/internal/portal/accounts/{account_id}/customers"));
    let body: Value = state.clients.internal_json(|| state.clients.http().get(&target), RESOLVE_TIMEOUT, true).await?;
    Ok(body
        .get("customer_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect())
}

pub async fn resolve_customer_ids(state: &PortalState, account_id: Uuid) -> Result<Vec<Uuid>, ApiError> {
    let cached: Vec<String> = cache_aside(&state.redis, &customer_ids_key(account_id), CUSTOMER_IDS_TTL, || {
        fetch_customer_ids(state, account_id)
    })
    .await
    .map_err(|err| ApiError::internal_from(&err))?;
    Ok(cached.iter().filter_map(|raw| Uuid::parse_str(raw).ok()).collect())
}

impl<S> FromRequestParts<S> for CustomerScope
where
    PortalState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let portal = PortalState::from_ref(state);
        let token = decode_portal_token(bearer_token(&parts.headers)?, &portal.key)?;
        let customer_ids = resolve_customer_ids(&portal, token.account_id).await?;
        if customer_ids.is_empty() {
            return Err(ApiError::http(StatusCode::FORBIDDEN, "Portal account is not linked to a customer"));
        }
        Ok(Self { account_id: token.account_id, org_id: token.org_id, email: token.email, customer_ids })
    }
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    use super::*;

    const SECRET: &str = "portal-unit-secret";
    const ACCOUNT: &str = "0d9c6d1e-7a2b-4f3c-9e8d-1a2b3c4d5e6f";
    const ORG: &str = "6f5e4d3c-2b1a-4e9d-8c7b-6a5f4e3d2c1b";

    fn key() -> DecodingKey {
        DecodingKey::from_secret(SECRET.as_bytes())
    }

    fn token(claims: &Value) -> String {
        encode(&Header::default(), claims, &EncodingKey::from_secret(SECRET.as_bytes())).expect("encode")
    }

    fn claims() -> Value {
        json!({"account_id": ACCOUNT, "org_id": ORG, "email": "shipper@example.com", "aud": "portal", "type": "access", "exp": 4_102_444_800_i64})
    }

    #[test]
    fn a_portal_access_token_decodes_into_its_scope_claims() {
        let decoded = decode_portal_token(&token(&claims()), &key()).expect("valid");
        assert_eq!(decoded.account_id, Uuid::parse_str(ACCOUNT).expect("uuid"));
        assert_eq!(decoded.org_id, Uuid::parse_str(ORG).expect("uuid"));
        assert_eq!(decoded.email, "shipper@example.com");
    }

    #[test]
    fn a_token_minted_for_another_audience_is_refused() {
        let mut claims = claims();
        claims["aud"] = json!("admin-console");
        assert_eq!(decode_portal_token(&token(&claims), &key()).unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn a_refresh_token_is_refused() {
        let mut claims = claims();
        claims["type"] = json!("refresh");
        assert_eq!(decode_portal_token(&token(&claims), &key()).unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn the_customer_cache_key_matches_python() {
        let account = Uuid::parse_str(ACCOUNT).expect("uuid");
        assert_eq!(customer_ids_key(account), format!("portal:customers:{ACCOUNT}"));
    }
}
