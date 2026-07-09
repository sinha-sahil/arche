use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Clone)]
pub struct ClientRegistration {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uris: Vec<String>,
}

impl std::fmt::Debug for ClientRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientRegistration")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("redirect_uris", &self.redirect_uris)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct OidcServerConfig {
    pub issuer: String,
    pub code_ttl: Option<Duration>,
    pub id_token_ttl: Option<Duration>,
    pub refresh_token_ttl: Option<Duration>,
    pub allowed_scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryDocument {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub userinfo_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introspection_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_session_endpoint: Option<String>,
    pub response_types_supported: Vec<String>,
    pub response_modes_supported: Vec<String>,
    pub grant_types_supported: Vec<String>,
    pub subject_types_supported: Vec<String>,
    pub id_token_signing_alg_values_supported: Vec<String>,
    pub code_challenge_methods_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
    pub scopes_supported: Vec<String>,
    pub claims_supported: Vec<String>,
    pub request_uri_parameter_supported: bool,
}

impl DiscoveryDocument {
    pub fn standard(issuer: impl Into<String>) -> Self {
        let issuer = issuer.into().trim_end_matches('/').to_string();
        Self {
            authorization_endpoint: format!("{issuer}/authorize"),
            token_endpoint: format!("{issuer}/token"),
            jwks_uri: format!("{issuer}/jwks"),
            issuer,
            userinfo_endpoint: None,
            revocation_endpoint: None,
            introspection_endpoint: None,
            end_session_endpoint: None,
            response_types_supported: vec!["code".into()],
            response_modes_supported: vec!["query".into()],
            grant_types_supported: vec!["authorization_code".into(), "refresh_token".into()],
            subject_types_supported: vec!["public".into()],
            id_token_signing_alg_values_supported: vec!["RS256".into()],
            code_challenge_methods_supported: vec!["S256".into()],
            token_endpoint_auth_methods_supported: vec![
                "client_secret_post".into(),
                "client_secret_basic".into(),
            ],
            scopes_supported: vec!["openid".into(), "email".into(), "profile".into()],
            claims_supported: vec![
                "iss".into(),
                "sub".into(),
                "aud".into(),
                "iat".into(),
                "exp".into(),
            ],
            request_uri_parameter_supported: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthorizeParams {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub code_challenge: Option<String>,
    #[serde(default)]
    pub code_challenge_method: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub login_hint: Option<String>,
    #[serde(default)]
    pub max_age: Option<u64>,
    #[serde(default)]
    pub id_token_hint: Option<String>,
    #[serde(default)]
    pub acr_values: Option<String>,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub ui_locales: Option<String>,
    #[serde(default)]
    pub claims: Option<String>,
    #[serde(default)]
    pub response_mode: Option<String>,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub request_uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatedAuthorizeRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub nonce: Option<String>,
    pub prompt: Option<String>,
    pub login_hint: Option<String>,
    pub max_age: Option<u64>,
    pub id_token_hint: Option<String>,
    pub acr_values: Option<String>,
    pub display: Option<String>,
    pub ui_locales: Option<String>,
    pub claims_param: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingGrant {
    pub request: ValidatedAuthorizeRequest,
    pub subject: String,
    pub claims: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub basic_auth: Option<(String, String)>,
}

impl std::fmt::Debug for TokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRequest")
            .field("grant_type", &self.grant_type)
            .field("code", &"<redacted>")
            .field("code_verifier", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "basic_auth",
                &self.basic_auth.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl TokenRequest {
    pub fn parse_basic_authorization(header_value: &str) -> Option<(String, String)> {
        let (scheme, payload) = header_value.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        let decoded = BASE64_STANDARD.decode(payload.trim()).ok()?;
        let decoded = String::from_utf8(decoded).ok()?;
        let (id, secret) = decoded.split_once(':')?;
        Some((form_urldecode(id)?, form_urldecode(secret)?))
    }
}

fn form_urldecode(s: &str) -> Option<String> {
    percent_encoding::percent_decode_str(&s.replace('+', " "))
        .decode_utf8()
        .ok()
        .map(|c| c.into_owned())
}

#[derive(Clone, Serialize)]
pub struct TokenPayload {
    pub access_token: String,
    pub id_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for TokenPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenPayload")
            .field("access_token", &"<redacted>")
            .field("id_token", &"<redacted>")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OidcServerError {
    #[error("unknown client: {0}")]
    UnknownClient(String),
    #[error("redirect_uri not registered: {0}")]
    UnregisteredRedirectUri(String),
    #[error("unsupported response_type: {0}")]
    UnsupportedResponseType(String),
    #[error("code_challenge is required (PKCE)")]
    MissingPkce,
    #[error("unsupported code_challenge_method: {0}")]
    UnsupportedChallengeMethod(String),
    #[error("malformed code_challenge")]
    MalformedCodeChallenge,
    #[error("malformed scope")]
    MalformedScope,
    #[error("scope must include openid")]
    MissingOpenidScope,
    #[error("unsupported response_mode: {0}")]
    UnsupportedResponseMode(String),
    #[error("request parameter is not supported")]
    RequestNotSupported,
    #[error("request_uri parameter is not supported")]
    RequestUriNotSupported,
    #[error("the resource owner denied the request")]
    AccessDenied,
    #[error("end-user authentication is required")]
    LoginRequired,
    #[error("end-user interaction is required")]
    InteractionRequired,
    #[error("end-user consent is required")]
    ConsentRequired,
    #[error("client authentication failed")]
    InvalidClient,
    #[error("invalid grant: {0}")]
    InvalidGrant(String),
    #[error("unsupported grant_type: {0}")]
    UnsupportedGrantType(String),
    #[error("internal error: {0}")]
    Internal(#[from] AppError),
}

impl OidcServerError {
    pub fn error_code(&self) -> &'static str {
        use OidcServerError as E;
        match self {
            E::UnknownClient(_) | E::InvalidClient => "invalid_client",
            E::UnregisteredRedirectUri(_)
            | E::MissingPkce
            | E::UnsupportedChallengeMethod(_)
            | E::MalformedCodeChallenge
            | E::UnsupportedResponseMode(_) => "invalid_request",
            E::MissingOpenidScope | E::MalformedScope => "invalid_scope",
            E::RequestNotSupported => "request_not_supported",
            E::RequestUriNotSupported => "request_uri_not_supported",
            E::AccessDenied => "access_denied",
            E::LoginRequired => "login_required",
            E::InteractionRequired => "interaction_required",
            E::ConsentRequired => "consent_required",
            E::UnsupportedResponseType(_) => "unsupported_response_type",
            E::InvalidGrant(_) => "invalid_grant",
            E::UnsupportedGrantType(_) => "unsupported_grant_type",
            E::Internal(_) => "server_error",
        }
    }

    pub fn redirectable(&self) -> bool {
        use OidcServerError as E;
        matches!(
            self,
            E::UnsupportedResponseType(_)
                | E::MissingPkce
                | E::UnsupportedChallengeMethod(_)
                | E::MalformedCodeChallenge
                | E::MalformedScope
                | E::MissingOpenidScope
                | E::UnsupportedResponseMode(_)
                | E::RequestNotSupported
                | E::RequestUriNotSupported
                | E::AccessDenied
                | E::LoginRequired
                | E::InteractionRequired
                | E::ConsentRequired
        )
    }

    pub fn redirect_url(
        &self,
        redirect_uri: &str,
        state: Option<&str>,
    ) -> Result<String, AppError> {
        if !self.redirectable() {
            return Err(AppError::internal_error(
                format!("redirect_url called on non-redirectable error: {self}"),
                None,
            ));
        }
        let description: String = self
            .to_string()
            .chars()
            .filter(|c| matches!(c, ' '..='~') && *c != '"' && *c != '\\')
            .collect();
        let mut params = vec![
            ("error", self.error_code()),
            ("error_description", description.as_str()),
        ];
        if let Some(state) = state {
            params.push(("state", state));
        }
        Url::parse_with_params(redirect_uri, &params)
            .map(Into::into)
            .map_err(|e| {
                AppError::internal_error(format!("redirect_url: invalid redirect_uri: {e}"), None)
            })
    }
}
