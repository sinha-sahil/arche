use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::utils::nano_id_of;

mod access;
mod keys;
mod registry;
mod store;
mod types;

pub use access::{AccessTokenIssuer, IssuedAccessToken};
pub use keys::{SigningKey, TokenSigner, rsa_public_jwk};
pub use registry::ClientRegistry;
pub use store::CodeStore;
pub use types::{
    AuthorizeParams, ClientRegistration, DiscoveryDocument, OidcServerConfig, OidcServerError,
    PendingGrant, TokenPayload, TokenRequest, ValidatedAuthorizeRequest,
};

const CODE_LENGTH: usize = 43;
const DEFAULT_CODE_TTL: Duration = Duration::from_secs(300);
const DEFAULT_ID_TOKEN_TTL: Duration = Duration::from_secs(3600);
const RESERVED_CLAIMS: &[&str] = &[
    "iss", "sub", "aud", "exp", "iat", "nbf", "nonce", "jti", "azp", "at_hash", "c_hash",
];
const DEFAULT_ALLOWED_SCOPES: &[&str] = &["openid", "email", "profile"];
const MAX_SUBJECT_LENGTH: usize = 255;

struct Inner<C, T, A, S> {
    issuer: String,
    code_ttl: Duration,
    id_token_ttl: Duration,
    allowed_scopes: Vec<String>,
    clients: C,
    signer: T,
    access_tokens: A,
    store: S,
}

pub struct OidcServer<C: ClientRegistry, T: TokenSigner, A: AccessTokenIssuer, S: CodeStore> {
    inner: Arc<Inner<C, T, A, S>>,
}

impl<C: ClientRegistry, T: TokenSigner, A: AccessTokenIssuer, S: CodeStore> Clone
    for OidcServer<C, T, A, S>
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<C: ClientRegistry, T: TokenSigner, A: AccessTokenIssuer, S: CodeStore> OidcServer<C, T, A, S> {
    pub fn new(
        config: OidcServerConfig,
        clients: C,
        signer: T,
        access_tokens: A,
        store: S,
    ) -> Result<Self, AppError> {
        let issuer = config.issuer.trim_end_matches('/').to_string();
        let parsed = Url::parse(&issuer).map_err(|e| {
            AppError::internal_error(
                format!("Config error [issuer]: invalid URL {issuer:?}: {e}"),
                None,
            )
        })?;
        let scheme_ok = match parsed.scheme() {
            "https" => true,
            "http" => matches!(
                parsed.host_str(),
                Some("localhost") | Some("127.0.0.1") | Some("[::1]")
            ),
            _ => false,
        };
        if !scheme_ok
            || parsed.host_str().is_none()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AppError::internal_error(
                format!(
                    "Config error [issuer]: must be an https URL (http allowed for loopback only) with a host and no query/fragment: {issuer:?}"
                ),
                None,
            ));
        }

        let allowed_scopes = config.allowed_scopes.unwrap_or_else(|| {
            DEFAULT_ALLOWED_SCOPES
                .iter()
                .map(|s| s.to_string())
                .collect()
        });
        if !allowed_scopes.iter().any(|s| s == "openid") {
            return Err(AppError::internal_error(
                "Config error [allowed_scopes]: must include \"openid\"".into(),
                None,
            ));
        }

        Ok(Self {
            inner: Arc::new(Inner {
                issuer,
                code_ttl: config.code_ttl.unwrap_or(DEFAULT_CODE_TTL),
                id_token_ttl: config.id_token_ttl.unwrap_or(DEFAULT_ID_TOKEN_TTL),
                allowed_scopes,
                clients,
                signer,
                access_tokens,
                store,
            }),
        })
    }

    pub fn issuer(&self) -> &str {
        &self.inner.issuer
    }

    pub async fn validate_authorize(
        &self,
        params: &AuthorizeParams,
    ) -> Result<ValidatedAuthorizeRequest, OidcServerError> {
        let client = self
            .inner
            .clients
            .find(&params.client_id)
            .await?
            .ok_or_else(|| OidcServerError::UnknownClient(params.client_id.clone()))?;

        if !client.redirect_uris.contains(&params.redirect_uri) {
            return Err(OidcServerError::UnregisteredRedirectUri(
                params.redirect_uri.clone(),
            ));
        }

        if params.response_type != "code" {
            return Err(OidcServerError::UnsupportedResponseType(
                params.response_type.clone(),
            ));
        }

        if params.request.is_some() {
            return Err(OidcServerError::RequestNotSupported);
        }
        if params.request_uri.is_some() {
            return Err(OidcServerError::RequestUriNotSupported);
        }
        match params.response_mode.as_deref() {
            None | Some("query") => {}
            Some(other) => {
                return Err(OidcServerError::UnsupportedResponseMode(other.to_string()));
            }
        }

        let code_challenge = params
            .code_challenge
            .clone()
            .ok_or(OidcServerError::MissingPkce)?;
        match params.code_challenge_method.as_deref() {
            Some("S256") => {}
            other => {
                return Err(OidcServerError::UnsupportedChallengeMethod(
                    other.unwrap_or("<none>").to_string(),
                ));
            }
        }
        if !is_valid_code_challenge(&code_challenge) {
            return Err(OidcServerError::MalformedCodeChallenge);
        }

        let scope = self.granted_scope(params.scope.as_deref())?;

        Ok(ValidatedAuthorizeRequest {
            client_id: params.client_id.clone(),
            redirect_uri: params.redirect_uri.clone(),
            scope,
            state: params.state.clone(),
            code_challenge,
            nonce: params.nonce.clone(),
            prompt: params.prompt.clone(),
            login_hint: params.login_hint.clone(),
            max_age: params.max_age,
            id_token_hint: params.id_token_hint.clone(),
            acr_values: params.acr_values.clone(),
            display: params.display.clone(),
            ui_locales: params.ui_locales.clone(),
            claims_param: params.claims.clone(),
        })
    }

    pub async fn issue_code(
        &self,
        request: ValidatedAuthorizeRequest,
        subject: impl Into<String>,
        claims: impl serde::Serialize,
    ) -> Result<String, OidcServerError> {
        let client = self
            .inner
            .clients
            .find(&request.client_id)
            .await?
            .ok_or_else(|| OidcServerError::UnknownClient(request.client_id.clone()))?;
        if !client.redirect_uris.contains(&request.redirect_uri) {
            return Err(OidcServerError::UnregisteredRedirectUri(
                request.redirect_uri.clone(),
            ));
        }
        if !is_valid_code_challenge(&request.code_challenge) {
            return Err(OidcServerError::MalformedCodeChallenge);
        }
        if !is_wellformed_scope(&request.scope) {
            return Err(OidcServerError::MalformedScope);
        }

        let subject = subject.into();
        if subject.is_empty() || !subject.is_ascii() || subject.len() > MAX_SUBJECT_LENGTH {
            return Err(OidcServerError::Internal(AppError::internal_error(
                "issue_code: subject must be 1-255 ASCII characters".into(),
                None,
            )));
        }

        let claims = match serde_json::to_value(claims) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(serde_json::Value::Null) => serde_json::Map::new(),
            Ok(_) => {
                return Err(OidcServerError::Internal(AppError::internal_error(
                    "issue_code: claims must serialize to a JSON object".into(),
                    None,
                )));
            }
            Err(e) => {
                return Err(OidcServerError::Internal(AppError::internal_error(
                    format!("issue_code: claims must serialize to JSON: {e}"),
                    None,
                )));
            }
        };

        let code = nano_id_of(CODE_LENGTH);
        let redirect_uri = request.redirect_uri.clone();
        let state = request.state.clone();

        self.inner
            .store
            .put(
                code.clone(),
                PendingGrant {
                    request,
                    subject,
                    claims,
                },
                self.inner.code_ttl,
            )
            .await?;

        let mut params = vec![("code", code.as_str())];
        if let Some(state) = state.as_deref() {
            params.push(("state", state));
        }
        Url::parse_with_params(&redirect_uri, &params)
            .map(Into::into)
            .map_err(|e| {
                OidcServerError::Internal(AppError::internal_error(
                    format!("registered redirect_uri unparseable: {e}"),
                    None,
                ))
            })
    }

    pub async fn exchange(&self, req: TokenRequest) -> Result<TokenPayload, OidcServerError> {
        if req.grant_type != "authorization_code" {
            return Err(OidcServerError::UnsupportedGrantType(req.grant_type));
        }

        let (client_id, client_secret) = resolve_client_credentials(&req)?;
        if !self
            .inner
            .clients
            .verify_secret(&client_id, &client_secret)
            .await?
        {
            return Err(OidcServerError::InvalidClient);
        }

        let grant = self
            .inner
            .store
            .take(&req.code)
            .await?
            .ok_or_else(|| OidcServerError::InvalidGrant("unknown or expired code".into()))?;

        if grant.request.client_id != client_id {
            return Err(OidcServerError::InvalidGrant(
                "code was issued to another client".into(),
            ));
        }
        if grant.request.redirect_uri != req.redirect_uri {
            return Err(OidcServerError::InvalidGrant(
                "redirect_uri mismatch".into(),
            ));
        }
        verify_pkce(&req.code_verifier, &grant.request.code_challenge)?;

        let id_token = self.mint_id_token(&grant).await?;
        let access = self.inner.access_tokens.issue(&grant).await?;
        Ok(TokenPayload {
            access_token: access.token,
            id_token,
            token_type: "Bearer".into(),
            expires_in: access.expires_in,
            scope: grant.request.scope,
        })
    }

    async fn mint_id_token(&self, grant: &PendingGrant) -> Result<String, OidcServerError> {
        let now = unix_now()?;

        let mut claims = grant.claims.clone();
        for reserved in RESERVED_CLAIMS {
            claims.remove(*reserved);
        }
        if let Some(nonce) = &grant.request.nonce {
            claims.insert("nonce".into(), nonce.clone().into());
        }
        claims.insert("iss".into(), self.inner.issuer.clone().into());
        claims.insert("sub".into(), grant.subject.clone().into());
        claims.insert("aud".into(), grant.request.client_id.clone().into());
        claims.insert("iat".into(), now.into());
        claims.insert(
            "exp".into(),
            now.saturating_add(self.inner.id_token_ttl.as_secs()).into(),
        );

        let header = serde_json::json!({
            "typ": "JWT",
            "alg": "RS256",
            "kid": self.inner.signer.kid(),
        });
        let signing_input = format!(
            "{}.{}",
            encode_jwt_part(&header)?,
            encode_jwt_part(&serde_json::Value::Object(claims))?
        );
        let signature = self.inner.signer.sign(signing_input.as_bytes()).await?;
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        ))
    }

    fn granted_scope(&self, requested: Option<&str>) -> Result<String, OidcServerError> {
        let requested = requested.ok_or(OidcServerError::MissingOpenidScope)?;
        let tokens: Vec<&str> = requested.split(' ').collect();
        if !tokens.iter().all(|s| is_scope_token(s)) {
            return Err(OidcServerError::MalformedScope);
        }
        if !tokens.contains(&"openid") {
            return Err(OidcServerError::MissingOpenidScope);
        }
        Ok(tokens
            .into_iter()
            .filter(|s| self.inner.allowed_scopes.iter().any(|a| a == s))
            .collect::<Vec<_>>()
            .join(" "))
    }

    pub fn jwks_document(&self) -> serde_json::Value {
        self.inner.signer.jwks()
    }
}

fn unix_now() -> Result<u64, OidcServerError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| {
            OidcServerError::Internal(AppError::internal_error(
                "system clock is before the unix epoch".into(),
                None,
            ))
        })
}

fn encode_jwt_part(value: &serde_json::Value) -> Result<String, OidcServerError> {
    serde_json::to_vec(value)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|e| {
            OidcServerError::Internal(AppError::internal_error(
                format!("id token serialization: {e}"),
                None,
            ))
        })
}

/// One client-auth method per request (RFC 6749 §2.3): a Basic header and a
/// body `client_secret` together, or a body `client_id` that contradicts the
/// Basic username, are both rejected.
fn resolve_client_credentials(req: &TokenRequest) -> Result<(String, String), OidcServerError> {
    match (&req.basic_auth, &req.client_id, &req.client_secret) {
        (Some(_), _, Some(_)) => Err(OidcServerError::InvalidClient),
        (Some((id, secret)), body_id, None) => {
            if body_id.as_ref().is_some_and(|b| b != id) {
                return Err(OidcServerError::InvalidClient);
            }
            Ok((id.clone(), secret.clone()))
        }
        (None, Some(id), Some(secret)) => Ok((id.clone(), secret.clone())),
        _ => Err(OidcServerError::InvalidClient),
    }
}

fn is_valid_code_challenge(s: &str) -> bool {
    (43..=128).contains(&s.len()) && is_unreserved(s)
}

fn is_wellformed_scope(scope: &str) -> bool {
    scope.split(' ').all(is_scope_token) && scope.split(' ').any(|s| s == "openid")
}

fn is_unreserved(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// RFC 6749 scope-token: `%x21 / %x23-5B / %x5D-7E` (printable ASCII, no space/`"`/`\`).
fn is_scope_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b == 0x21 || (0x23..=0x5B).contains(&b) || (0x5D..=0x7E).contains(&b))
}

fn verify_pkce(verifier: &str, challenge: &str) -> Result<(), OidcServerError> {
    if !(43..=128).contains(&verifier.len()) || !is_unreserved(verifier) {
        return Err(OidcServerError::InvalidGrant(
            "malformed code_verifier".into(),
        ));
    }
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    if !ct_eq(computed.as_bytes(), challenge.as_bytes()) {
        return Err(OidcServerError::InvalidGrant(
            "PKCE verification failed".into(),
        ));
    }
    Ok(())
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let (da, db) = (Sha256::digest(a), Sha256::digest(b));
    da.iter()
        .zip(db.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
