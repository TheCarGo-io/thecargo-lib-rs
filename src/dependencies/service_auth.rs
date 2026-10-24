use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::StatusCode;
use axum::http::request::Parts;
use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::context::CompanyRef;
use crate::handlers::ApiError;

pub const SERVICE_SECRET_HEADER: &str = "x-service-secret";
pub const COMPANY_HEADER: &str = "x-company-id";
const THE_ORGANIZATION: &str = "organization";

#[derive(Clone)]
pub struct ServiceSecret(Arc<SecretString>);

impl std::fmt::Debug for ServiceSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceSecret(..)")
    }
}

impl ServiceSecret {
    pub fn new(secret: SecretString) -> Self {
        Self(Arc::new(secret))
    }

    pub fn matches(&self, provided: Option<&str>) -> bool {
        let expected = self.0.expose_secret().as_bytes();
        match provided {
            Some(provided) if !expected.is_empty() && !provided.is_empty() => {
                provided.as_bytes().ct_eq(expected).into()
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ServiceAuth;

impl<S> FromRequestParts<S> for ServiceAuth
where
    ServiceSecret: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let provided = parts.headers.get(SERVICE_SECRET_HEADER).and_then(|value| value.to_str().ok());
        if ServiceSecret::from_ref(state).matches(provided) {
            Ok(Self)
        } else {
            Err(ApiError::http(StatusCode::FORBIDDEN, "Invalid service secret"))
        }
    }
}

// The company a calling service says it acts for; taken on trust, so only behind ServiceAuth.
#[derive(Debug, Clone, Copy)]
pub struct ServiceCompany(pub CompanyRef);

impl<S> FromRequestParts<S> for ServiceCompany
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(raw) =
            parts.headers.get(COMPANY_HEADER).and_then(|value| value.to_str().ok()).filter(|v| !v.is_empty())
        else {
            return Ok(Self(CompanyRef::Any));
        };
        if raw == THE_ORGANIZATION {
            return Ok(Self(CompanyRef::Organization));
        }
        Uuid::parse_str(raw)
            .map(|id| Self(CompanyRef::Company(id)))
            .map_err(|_not_a_uuid| ApiError::http(StatusCode::BAD_REQUEST, "Invalid company"))
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;

    use super::*;

    #[test]
    fn the_secret_must_match_exactly_and_never_when_unset() {
        let secret = ServiceSecret::new(SecretString::from("s3cret"));
        assert!(secret.matches(Some("s3cret")));
        assert!(!secret.matches(Some("s3cre")));
        assert!(!secret.matches(None));
        assert!(!ServiceSecret::new(SecretString::from("")).matches(Some("")));
    }

    async fn company(header: Option<&str>) -> Result<CompanyRef, ApiError> {
        let mut builder = Request::builder().uri("/");
        if let Some(value) = header {
            builder = builder.header(COMPANY_HEADER, value);
        }
        let (mut parts, _body) = builder.body(Body::empty()).unwrap().into_parts();
        ServiceCompany::from_request_parts(&mut parts, &()).await.map(|ServiceCompany(company)| company)
    }

    #[tokio::test]
    async fn the_company_header_names_any_the_organization_or_one_company() {
        let id = Uuid::new_v4();
        assert_eq!(company(None).await.unwrap(), CompanyRef::Any);
        assert_eq!(company(Some("organization")).await.unwrap(), CompanyRef::Organization);
        assert_eq!(company(Some(&id.to_string())).await.unwrap(), CompanyRef::Company(id));
        assert_eq!(company(Some("acme")).await.unwrap_err().status(), StatusCode::BAD_REQUEST);
    }
}
