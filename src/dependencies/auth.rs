use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use redis::aio::ConnectionManager;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use uuid::Uuid;

use crate::context::CompanyRef;
use crate::handlers::ApiError;

pub const DEFAULT_ORG_TYPE: &str = "broker";
pub const ADMIN_AUDIENCE: &str = "admin-console";
const TOKEN_STATE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    All,
    Own,
    Team,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    View,
    Create,
    Update,
    Delete,
}

impl Action {
    const ALL: [Self; 4] = [Self::View, Self::Create, Self::Update, Self::Delete];

    fn name(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Permissions(HashMap<String, HashMap<&'static str, ScopeKind>>);

impl Permissions {
    // Each resource is a string of up to four characters, one per action: a(ll), o(wn), t(eam) or _.
    fn decode(raw: Option<&Value>) -> Self {
        let Some(Value::Object(resources)) = raw else { return Self::default() };
        let decoded = resources
            .iter()
            .filter_map(|(resource, scopes)| {
                let actions = scopes
                    .as_str()?
                    .chars()
                    .zip(Action::ALL)
                    .filter_map(|(ch, action)| {
                        let scope = match ch {
                            'a' => ScopeKind::All,
                            'o' => ScopeKind::Own,
                            't' => ScopeKind::Team,
                            _ => return None,
                        };
                        Some((action.name(), scope))
                    })
                    .collect();
                Some((resource.clone(), actions))
            })
            .collect();
        Self(decoded)
    }

    pub fn scope(&self, resource: &str, action: Action) -> Option<ScopeKind> {
        self.0.get(resource)?.get(action.name()).copied()
    }
}

#[derive(Debug, Clone, Default)]
pub struct StageFilters(HashMap<String, HashMap<String, Vec<String>>>);

impl StageFilters {
    fn decode(raw: Option<&Value>) -> Self {
        let Some(Value::Object(resources)) = raw else { return Self::default() };
        let decoded = resources
            .iter()
            .map(|(resource, actions)| {
                let actions = actions
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(action, stages)| {
                        let stages: Vec<String> =
                            stages.as_array()?.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect();
                        (!stages.is_empty()).then(|| (action.clone(), stages))
                    })
                    .collect();
                (resource.clone(), actions)
            })
            .collect();
        Self(decoded)
    }

    pub fn allowed(&self, resource: &str, action: Action) -> Option<&[String]> {
        self.0.get(resource)?.get(action.name()).map(Vec::as_slice)
    }
}

#[derive(Debug, Clone)]
pub struct TokenPayload {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub role_id: Option<Uuid>,
    pub is_superuser: bool,
    pub team_id: Option<Uuid>,
    pub permissions: Permissions,
    pub stage_filters: StageFilters,
    pub role_version: i64,
    pub session_id: Option<String>,
    pub tz: Option<String>,
    pub otz: Option<String>,
    pub org_type: String,
    pub company_id: Option<Uuid>,
    pub user_type: Option<String>,
    pub issued_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub kind: ScopeKind,
    pub user_id: Uuid,
    pub team_id: Option<Uuid>,
    pub allowed_stages: Option<Vec<String>>,
}

impl TokenPayload {
    pub fn company(&self) -> CompanyRef {
        CompanyRef::from_company_id(self.company_id)
    }

    pub fn requires(&self, resource: &str, action: Action) -> Result<Scope, ApiError> {
        if self.is_superuser {
            return Ok(Scope {
                kind: ScopeKind::All,
                user_id: self.user_id,
                team_id: self.team_id,
                allowed_stages: None,
            });
        }
        let kind = self
            .permissions
            .scope(resource, action)
            .ok_or_else(|| ApiError::http(StatusCode::FORBIDDEN, "Permission denied"))?;
        Ok(Scope {
            kind,
            user_id: self.user_id,
            team_id: self.team_id,
            allowed_stages: self.stage_filters.allowed(resource, action).map(<[String]>::to_vec),
        })
    }
}

#[derive(Debug, Clone)]
pub struct AdminPayload {
    pub user_id: Uuid,
    pub email: Option<String>,
    pub name: Option<String>,
}

pub struct AuthKeys {
    decoding: DecodingKey,
    allowed_org_types: HashSet<String>,
}

impl std::fmt::Debug for AuthKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthKeys").field("allowed_org_types", &self.allowed_org_types).finish_non_exhaustive()
    }
}

impl AuthKeys {
    pub fn new(secret: &SecretString, allowed_org_types: HashSet<String>) -> Self {
        Self { decoding: DecodingKey::from_secret(secret.expose_secret().as_bytes()), allowed_org_types }
    }

    pub fn decoding(&self) -> &DecodingKey {
        &self.decoding
    }
}

#[derive(Clone)]
pub struct AuthState {
    pub keys: Arc<AuthKeys>,
    pub redis: ConnectionManager,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState").field("keys", &self.keys).finish_non_exhaustive()
    }
}

// The validation PyJWT applies when it is given no audience: HS256 only, exp and nbf checked when
// present with no leeway, and no claim required.
fn pyjwt_validation() -> Validation {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    validation
}

pub fn decode_claims(token: &str, key: &DecodingKey) -> Option<serde_json::Map<String, Value>> {
    let data = jsonwebtoken::decode::<serde_json::Map<String, Value>>(token, key, &pyjwt_validation()).ok()?;
    let claims = data.claims;
    if claims.get("aud").is_some_and(truthy) || issued_in_future(&claims) {
        return None;
    }
    Some(claims)
}

pub fn decode_access_token(token: &str, keys: &AuthKeys) -> Result<TokenPayload, ApiError> {
    let invalid = || ApiError::http(StatusCode::UNAUTHORIZED, "Invalid token");
    let claims = decode_claims(token, &keys.decoding).ok_or_else(invalid)?;
    if claims.get("type").and_then(Value::as_str) != Some("access") {
        return Err(invalid());
    }
    let required = |name: &str| claims.get(name).and_then(Value::as_str).and_then(|raw| Uuid::parse_str(raw).ok());
    let optional = |name: &str| -> Result<Option<Uuid>, ApiError> {
        match claims.get(name).filter(|value| truthy(value)) {
            None => Ok(None),
            Some(value) => value.as_str().and_then(|raw| Uuid::parse_str(raw).ok()).map(Some).ok_or_else(invalid),
        }
    };
    let text = |name: &str| claims.get(name).and_then(Value::as_str).map(str::to_owned);

    Ok(TokenPayload {
        user_id: required("user_id").ok_or_else(invalid)?,
        org_id: required("org_id").ok_or_else(invalid)?,
        role_id: optional("role_id")?,
        is_superuser: claims.get("is_superuser").is_some_and(truthy),
        team_id: optional("team_id")?,
        permissions: Permissions::decode(claims.get("p")),
        stage_filters: StageFilters::decode(claims.get("ps")),
        role_version: claims.get("rv").and_then(as_whole_number).unwrap_or(0),
        session_id: text("sid"),
        tz: text("tz"),
        otz: text("otz"),
        org_type: text("ot").filter(|ot| !ot.is_empty()).unwrap_or_else(|| DEFAULT_ORG_TYPE.to_owned()),
        company_id: optional("cid")?,
        user_type: text("ut"),
        issued_at: claims.get("iat").and_then(as_whole_number),
    })
}

pub fn decode_admin_token(token: &str, key: &DecodingKey) -> Result<AdminPayload, ApiError> {
    let invalid = || ApiError::http(StatusCode::UNAUTHORIZED, "Invalid token");
    let mut validation = pyjwt_validation();
    validation.validate_aud = true;
    validation.set_audience(&[ADMIN_AUDIENCE]);
    validation.set_required_spec_claims(&["aud"]);

    let claims = jsonwebtoken::decode::<serde_json::Map<String, Value>>(token, key, &validation)
        .map_err(|_rejected| invalid())?
        .claims;
    if issued_in_future(&claims) || claims.get("type").and_then(Value::as_str) != Some("access") {
        return Err(invalid());
    }
    let user_id =
        claims.get("sub").and_then(Value::as_str).and_then(|raw| Uuid::parse_str(raw).ok()).ok_or_else(invalid)?;
    let text = |name: &str| claims.get(name).and_then(Value::as_str).map(str::to_owned);
    Ok(AdminPayload { user_id, email: text("email"), name: text("name") })
}

// FastAPI's HTTPBearer: a scheme other than bearer (any case) or an empty credential is refused.
pub fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, credentials)| scheme.eq_ignore_ascii_case("bearer") && !credentials.is_empty())
        .map(|(_, credentials)| credentials)
        .ok_or_else(|| ApiError::http(StatusCode::UNAUTHORIZED, "Not authenticated"))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

#[expect(clippy::cast_possible_truncation, reason = "claims hold Unix seconds and small counters")]
fn as_whole_number(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_f64().map(|n| n.trunc() as i64))
}

// PyJWT 2.10 and later refuse a token whose iat lies in the future.
fn issued_in_future(claims: &serde_json::Map<String, Value>) -> bool {
    claims.get("iat").and_then(as_whole_number).is_some_and(|iat| iat > chrono::Utc::now().timestamp())
}

pub fn role_version_key(role_id: Uuid) -> String {
    format!("role:{role_id}:version")
}

pub fn user_revocation_key(user_id: Uuid) -> String {
    format!("user:{user_id}:revoked_at")
}

pub fn active_sessions_key(user_id: Uuid) -> String {
    format!("user:{user_id}:sids")
}

pub fn session_pending_key(session_id: &str) -> String {
    format!("session:{session_id}:pending")
}

#[derive(Debug, Default)]
struct TokenState {
    revoked_at: Option<String>,
    session_count: i64,
    session_member: Option<f64>,
    pending: Option<String>,
    role_version: Option<String>,
}

// Redis being down must not lock everyone out: any failure reads as "nothing recorded".
async fn token_state(redis: &ConnectionManager, user: &TokenPayload) -> TokenState {
    let session = user.session_id.as_deref().unwrap_or_default();
    let mut pipe = redis::pipe();
    pipe.get(user_revocation_key(user.user_id))
        .zcard(active_sessions_key(user.user_id))
        .zscore(active_sessions_key(user.user_id), session)
        .get(session_pending_key(session));
    if let Some(role_id) = user.role_id {
        pipe.get(role_version_key(role_id));
    }

    let mut conn = redis.clone();
    let query = async {
        if user.role_id.is_some() {
            pipe.query_async::<(Option<String>, i64, Option<f64>, Option<String>, Option<String>)>(&mut conn).await.map(
                |(revoked_at, session_count, session_member, pending, role_version)| TokenState {
                    revoked_at,
                    session_count,
                    session_member,
                    pending,
                    role_version,
                },
            )
        } else {
            pipe.query_async::<(Option<String>, i64, Option<f64>, Option<String>)>(&mut conn).await.map(
                |(revoked_at, session_count, session_member, pending)| TokenState {
                    revoked_at,
                    session_count,
                    session_member,
                    pending,
                    role_version: None,
                },
            )
        }
    };
    match tokio::time::timeout(TOKEN_STATE_TIMEOUT, query).await {
        Ok(Ok(state)) => state,
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "token state unavailable; letting the token through");
            TokenState::default()
        }
        Err(_) => {
            tracing::warn!("token state timed out; letting the token through");
            TokenState::default()
        }
    }
}

fn parse_whole(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok().or_else(|| as_whole_number(&Value::from(raw.trim().parse::<f64>().ok()?)))
}

async fn enforce_token_state(redis: &ConnectionManager, user: &TokenPayload) -> Result<(), ApiError> {
    let state = token_state(redis, user).await;
    let has_session = user.session_id.as_deref().is_some_and(|sid| !sid.is_empty());

    if let (Some(revoked_at), Some(issued_at)) = (state.revoked_at.as_deref().and_then(parse_whole), user.issued_at)
        && issued_at < revoked_at
    {
        return Err(ApiError::http_coded(
            StatusCode::UNAUTHORIZED,
            "token_revoked",
            "Session revoked; please sign in again",
        ));
    }
    if has_session && state.pending.is_some() {
        return Err(ApiError::http_coded(
            StatusCode::FORBIDDEN,
            "device_approval_pending",
            "Waiting for one of your other devices to approve this sign-in",
        ));
    }
    if has_session && state.session_count != 0 && state.session_member.is_none() {
        return Err(ApiError::http_coded(
            StatusCode::UNAUTHORIZED,
            "session_superseded",
            "Signed in on another device; this session was closed",
        ));
    }
    let checks_role = user.role_id.is_some() && !user.is_superuser;
    if checks_role
        && let Some(live) = state.role_version.as_deref().and_then(parse_whole)
        && live != user.role_version
    {
        return Err(ApiError::http_coded(
            StatusCode::UNAUTHORIZED,
            "token_stale",
            "Role permissions changed; please refresh your session",
        ));
    }
    Ok(())
}

fn enforce_org_type(user: &TokenPayload, allowed: &HashSet<String>) -> Result<(), ApiError> {
    if allowed.is_empty() || allowed.contains(&user.org_type) {
        return Ok(());
    }
    Err(ApiError::http_coded(
        StatusCode::FORBIDDEN,
        "org_type_not_allowed",
        "This service is not available for your organization type",
    ))
}

#[derive(Debug, Clone)]
pub struct CurrentUser(pub TokenPayload);

impl<S> FromRequestParts<S> for CurrentUser
where
    AuthState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = AuthState::from_ref(state);
        let user = decode_access_token(bearer_token(&parts.headers)?, &auth.keys)?;
        enforce_org_type(&user, &auth.keys.allowed_org_types)?;
        enforce_token_state(&auth.redis, &user).await?;
        Ok(Self(user))
    }
}

// A permission a route requires, named the way Python spells Requires("dashboard", "view").
pub trait Permission {
    const RESOURCE: &'static str;
    const ACTION: Action;
}

// The caller and the scope a permission grants them; refused with 403 before any parameter is read.
#[derive(Debug, Clone)]
pub struct Requires<P> {
    pub user: TokenPayload,
    pub scope: Scope,
    permission: std::marker::PhantomData<P>,
}

impl<P, S> FromRequestParts<S> for Requires<P>
where
    P: Permission,
    AuthState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let CurrentUser(user) = CurrentUser::from_request_parts(parts, state).await?;
        let scope = user.requires(P::RESOURCE, P::ACTION)?;
        Ok(Self { user, scope, permission: std::marker::PhantomData })
    }
}

#[derive(Debug, Clone)]
pub struct CurrentAdmin(pub AdminPayload);

impl<S> FromRequestParts<S> for CurrentAdmin
where
    AuthState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth = AuthState::from_ref(state);
        Ok(Self(decode_admin_token(bearer_token(&parts.headers)?, auth.keys.decoding())?))
    }
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    use super::*;

    const SECRET: &str = "unit-test-secret";

    fn keys() -> AuthKeys {
        AuthKeys::new(&SecretString::from(SECRET), HashSet::new())
    }

    fn sign(claims: &Value) -> String {
        encode(&Header::default(), claims, &EncodingKey::from_secret(SECRET.as_bytes())).unwrap()
    }

    fn user(extra: &Value) -> Value {
        let mut claims = json!({
            "user_id": Uuid::new_v4(), "org_id": Uuid::new_v4(), "type": "access", "exp": 4_102_444_800u64,
        });
        claims.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        claims
    }

    #[test]
    fn permissions_read_one_character_per_action() {
        let payload =
            decode_access_token(&sign(&user(&json!({"p": {"analytics": "ao_t", "team": "a"}}))), &keys()).unwrap();
        assert_eq!(payload.permissions.scope("analytics", Action::View), Some(ScopeKind::All));
        assert_eq!(payload.permissions.scope("analytics", Action::Create), Some(ScopeKind::Own));
        assert_eq!(payload.permissions.scope("analytics", Action::Update), None);
        assert_eq!(payload.permissions.scope("analytics", Action::Delete), Some(ScopeKind::Team));
        assert_eq!(payload.permissions.scope("team", Action::Create), None);
    }

    #[test]
    fn requires_returns_the_scope_or_forbids() {
        let token = sign(&user(&json!({"p": {"dashboard": "o___"}, "ps": {"dashboard": {"view": ["lead"]}}})));
        let payload = decode_access_token(&token, &keys()).unwrap();
        let scope = payload.requires("dashboard", Action::View).unwrap();
        assert_eq!(scope.kind, ScopeKind::Own);
        assert_eq!(scope.allowed_stages, Some(vec!["lead".to_owned()]));
        assert_eq!(payload.requires("dashboard", Action::Update).unwrap_err().status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_superuser_sees_everything() {
        let payload = decode_access_token(&sign(&user(&json!({"is_superuser": true}))), &keys()).unwrap();
        assert_eq!(payload.requires("anything", Action::Delete).unwrap().kind, ScopeKind::All);
    }

    #[test]
    fn the_org_type_defaults_to_broker_and_the_company_to_the_organization() {
        let payload = decode_access_token(&sign(&user(&json!({}))), &keys()).unwrap();
        assert_eq!(payload.org_type, "broker");
        assert_eq!(payload.company(), CompanyRef::Organization);
    }

    #[test]
    fn a_token_issued_in_the_future_is_refused() {
        let future = chrono::Utc::now().timestamp() + 3600;
        assert!(decode_access_token(&sign(&user(&json!({"iat": future}))), &keys()).is_err());
    }

    #[test]
    fn an_empty_audience_is_not_an_audience() {
        assert!(decode_access_token(&sign(&user(&json!({"aud": ""}))), &keys()).is_ok());
        assert!(decode_access_token(&sign(&user(&json!({"aud": "web"}))), &keys()).is_err());
    }

    #[test]
    fn a_malformed_claim_is_an_invalid_token() {
        let error = decode_access_token(&sign(&user(&json!({"role_id": "not-a-uuid"}))), &keys()).unwrap_err();
        assert_eq!(error.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn the_bearer_scheme_is_case_insensitive() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "bEaReR abc".parse().unwrap());
        assert_eq!(bearer_token(&headers).unwrap(), "abc");
        headers.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert!(bearer_token(&headers).is_err());
        headers.insert(header::AUTHORIZATION, "Bearer ".parse().unwrap());
        assert!(bearer_token(&headers).is_err());
    }

    #[test]
    fn whole_numbers_parse_from_redis_strings() {
        assert_eq!(parse_whole("1790000100"), Some(1_790_000_100));
        assert_eq!(parse_whole("1790000100.7"), Some(1_790_000_100));
        assert_eq!(parse_whole("soon"), None);
    }
}
