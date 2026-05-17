//! Google OAuth 2.0 + OpenID Connect for server-side login flows.
//!
//! Three operations:
//! - [`build_auth_url`] composes the Google authorize URL (no I/O).
//! - [`exchange_code`] trades an authorization code for tokens.
//! - [`Verifier::verify_id_token`] validates a Google-issued ID token, with an
//!   in-memory JWKS cache that handles key rotation transparently.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::resolve_required_string;
use crate::error::AppError;

pub use crate::config::gcp::{GcpOAuthConfig, GcpOAuthConfigBuilder};

const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_ENDPOINT: &str = "https://www.googleapis.com/oauth2/v3/certs";
const ISSUER_GOOGLE: &str = "https://accounts.google.com";
const ISSUER_GOOGLE_BARE: &str = "accounts.google.com";
const CLOCK_SKEW_SECS: u64 = 60;
const DEFAULT_JWKS_TTL: Duration = Duration::from_secs(3600);
const MAX_JWKS_TTL_SECS: u64 = 86_400;
const JWKS_STALE_GRACE_MULTIPLIER: u32 = 2;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TCP_KEEPALIVE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdTokenClaims {
    pub sub: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    /// Google Workspace hosted domain — set only for Workspace accounts.
    #[serde(default)]
    pub hd: Option<String>,
    pub aud: String,
    pub iss: String,
    pub exp: u64,
    pub iat: u64,
}

#[derive(Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub id_token: String,
    pub expires_in: u64,
    pub token_type: String,
    #[serde(default)]
    pub scope: Option<String>,
    /// Present only when the auth URL is built with `access_type=offline`. The
    /// flow shipped here uses `online`, so this stays `None`.
    #[serde(default)]
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"<redacted>")
            .field("id_token", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("malformed id token: {0}")]
    MalformedToken(String),
    #[error("unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid claim: {0}")]
    InvalidClaim(String),
    #[error("audience not allowed: got {got}, expected one of {expected:?}")]
    AudienceMismatch { got: String, expected: Vec<String> },
    #[error("issuer not allowed: {0}")]
    IssuerMismatch(String),
    #[error("id token expired")]
    Expired,
    #[error("unknown signing key: {0}")]
    UnknownKid(String),
    #[error("jwks unavailable: {0}")]
    JwksUnavailable(String),
    #[error("oauth call failed (transient): {0}")]
    OAuthFailureTransient(String),
    #[error("oauth call failed (permanent): {0}")]
    OAuthFailurePermanent(String),
    #[error("malformed upstream response: {0}")]
    MalformedResponse(String),
}

impl From<OAuthError> for AppError {
    fn from(e: OAuthError) -> Self {
        use OAuthError as O;
        match e {
            O::MalformedToken(_)
            | O::UnsupportedAlgorithm(_)
            | O::InvalidSignature
            | O::InvalidClaim(_)
            | O::AudienceMismatch { .. }
            | O::IssuerMismatch(_)
            | O::Expired
            | O::UnknownKid(_)
            | O::OAuthFailurePermanent(_) => AppError::Unauthorized,

            O::JwksUnavailable(d) => AppError::dependency_failed("gcp-oauth", d),
            O::OAuthFailureTransient(d) => AppError::dependency_failed("gcp-oauth", d),
            O::MalformedResponse(d) => AppError::dependency_failed_permanent("gcp-oauth", d),
        }
    }
}

#[derive(Clone)]
struct ResolvedOAuthConfig {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}

impl std::fmt::Debug for ResolvedOAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedOAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct OAuthClient {
    config: ResolvedOAuthConfig,
    http: reqwest::Client,
    token_endpoint: String,
}

impl OAuthClient {
    pub fn client_id(&self) -> &str {
        &self.config.client_id
    }

    pub fn redirect_uri(&self) -> &str {
        &self.config.redirect_uri
    }
}

pub fn get_oauth_client(
    config: impl Into<Option<GcpOAuthConfig>>,
) -> Result<OAuthClient, AppError> {
    let config = config.into().unwrap_or_default();
    let resolved = ResolvedOAuthConfig {
        client_id: resolve_required_string(config.client_id, "GCP_OAUTH_CLIENT_ID", "client_id")?,
        client_secret: resolve_required_string(
            config.client_secret,
            "GCP_OAUTH_CLIENT_SECRET",
            "client_secret",
        )?,
        redirect_uri: resolve_required_string(
            config.redirect_uri,
            "GCP_OAUTH_REDIRECT_URI",
            "redirect_uri",
        )?,
    };

    let http = reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TOTAL_TIMEOUT)
        .tcp_keepalive(HTTP_TCP_KEEPALIVE)
        .build()
        .map_err(|e| {
            AppError::internal_error(
                format!("Failed to build Google OAuth HTTP client: {e}"),
                None,
            )
        })?;

    Ok(OAuthClient {
        config: resolved,
        http,
        token_endpoint: GOOGLE_TOKEN_ENDPOINT.to_string(),
    })
}

/// Compose the Google authorize URL. Synchronous; no I/O.
///
/// Builds with `response_type=code`, `scope=openid email profile`,
/// `prompt=select_account`, `access_type=online`, `code_challenge_method=S256`.
/// `state` and `pkce_challenge` are supplied by the caller and URL-encoded
/// here.
pub fn build_auth_url(client: &OAuthClient, state: &str, pkce_challenge: &str) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", &client.config.client_id),
        ("redirect_uri", &client.config.redirect_uri),
        ("scope", "openid email profile"),
        ("state", state),
        ("code_challenge", pkce_challenge),
        ("code_challenge_method", "S256"),
        ("prompt", "select_account"),
        ("access_type", "online"),
    ];
    Url::parse_with_params(GOOGLE_AUTH_ENDPOINT, &params)
        .expect("GOOGLE_AUTH_ENDPOINT is a valid URL")
        .into()
}

/// Trade an authorization code for `access_token` + `id_token`.
pub async fn exchange_code(
    client: &OAuthClient,
    code: &str,
    pkce_verifier: &str,
) -> Result<TokenResponse, AppError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", pkce_verifier),
        ("client_id", client.config.client_id.as_str()),
        ("client_secret", client.config.client_secret.as_str()),
        ("redirect_uri", client.config.redirect_uri.as_str()),
    ];

    let resp = client
        .http
        .post(&client.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| OAuthError::OAuthFailureTransient(format!("token request: {e}")))?;

    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| OAuthError::OAuthFailureTransient(format!("token response body: {e}")))?;

    if status.is_server_error() {
        return Err(OAuthError::OAuthFailureTransient(format!(
            "token endpoint returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        ))
        .into());
    }
    if !status.is_success() {
        return Err(OAuthError::OAuthFailurePermanent(format!(
            "token endpoint returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        ))
        .into());
    }

    serde_json::from_slice::<TokenResponse>(&bytes)
        .map_err(|e| OAuthError::MalformedResponse(format!("token response parse: {e}")).into())
}

#[derive(Debug, Clone, Deserialize)]
struct Jwk {
    kid: String,
    n: String,
    e: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Default)]
struct JwksCache {
    keys: HashMap<String, Jwk>,
    /// `None` means the cache has never been populated.
    expires_at: Option<Instant>,
    ttl: Duration,
}

impl JwksCache {
    fn is_fresh(&self) -> bool {
        self.expires_at
            .map(|exp| exp > Instant::now())
            .unwrap_or(false)
    }

    /// Within the grace window after expiry, stale keys may still be served
    /// for known kids when the upstream JWKS endpoint is unreachable.
    fn within_stale_grace(&self) -> bool {
        let Some(exp) = self.expires_at else {
            return false;
        };
        let grace = self.ttl * JWKS_STALE_GRACE_MULTIPLIER;
        Instant::now() < exp + grace
    }
}

fn parse_cache_control_max_age(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::CACHE_CONTROL)?.to_str().ok()?;
    for directive in value.split(',') {
        let directive = directive.trim();
        if let Some(rest) = directive
            .strip_prefix("max-age=")
            .or_else(|| directive.strip_prefix("max-age ="))
        {
            // Capped so downstream Duration arithmetic (stale-grace, expiry +
            // grace) can never overflow on a hostile / buggy header.
            return rest
                .parse::<u64>()
                .ok()
                .map(|s| Duration::from_secs(s.min(MAX_JWKS_TTL_SECS)));
        }
    }
    None
}

async fn fetch_jwks(
    http: &reqwest::Client,
    endpoint: &str,
) -> Result<(Vec<Jwk>, Duration), OAuthError> {
    let resp = http
        .get(endpoint)
        .send()
        .await
        .map_err(|e| OAuthError::JwksUnavailable(format!("jwks fetch: {e}")))?;
    let status = resp.status();
    let ttl = parse_cache_control_max_age(resp.headers()).unwrap_or(DEFAULT_JWKS_TTL);
    if !status.is_success() {
        return Err(OAuthError::JwksUnavailable(format!(
            "jwks endpoint returned {status}"
        )));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| OAuthError::JwksUnavailable(format!("jwks body: {e}")))?;
    let parsed: Jwks = serde_json::from_slice(&body)
        .map_err(|e| OAuthError::MalformedResponse(format!("jwks parse: {e}")))?;
    Ok((parsed.keys, ttl))
}

/// Verifies Google-issued ID tokens. Holds an in-memory JWKS cache keyed by
/// `kid`; rotation is handled transparently on the next verify.
#[derive(Clone)]
pub struct Verifier {
    http: reqwest::Client,
    cache: Arc<RwLock<JwksCache>>,
    jwks_endpoint: String,
}

impl Verifier {
    pub fn new() -> Result<Self, AppError> {
        let http = reqwest::Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(HTTP_TOTAL_TIMEOUT)
            .tcp_keepalive(HTTP_TCP_KEEPALIVE)
            .build()
            .map_err(|e| {
                AppError::internal_error(
                    format!("Failed to build Google OAuth verifier HTTP client: {e}"),
                    None,
                )
            })?;
        Ok(Self::with_http_client(http))
    }

    pub fn with_http_client(http: reqwest::Client) -> Self {
        Self {
            http,
            cache: Arc::new(RwLock::new(JwksCache::default())),
            jwks_endpoint: GOOGLE_JWKS_ENDPOINT.to_string(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_endpoint(http: reqwest::Client, jwks_endpoint: String) -> Self {
        Self {
            http,
            cache: Arc::new(RwLock::new(JwksCache::default())),
            jwks_endpoint,
        }
    }

    /// Verify a Google-issued ID token. `audiences` is the allow-list of
    /// expected `aud` values — at least one must match.
    pub async fn verify_id_token(
        &self,
        id_token: &str,
        audiences: &[&str],
    ) -> Result<IdTokenClaims, AppError> {
        let header = decode_header(id_token)
            .map_err(|e| OAuthError::MalformedToken(format!("header decode: {e}")))?;
        if header.alg != Algorithm::RS256 {
            return Err(OAuthError::UnsupportedAlgorithm(format!("{:?}", header.alg)).into());
        }
        let kid = header
            .kid
            .ok_or_else(|| OAuthError::MalformedToken("missing kid".into()))?;

        let jwk = self.lookup_key_with_refresh(&kid).await?;

        let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
            .map_err(|e| OAuthError::MalformedResponse(format!("jwk components: {e}")))?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud"]);
        validation.set_issuer(&[ISSUER_GOOGLE, ISSUER_GOOGLE_BARE]);
        validation.set_audience(audiences);
        validation.leeway = CLOCK_SKEW_SECS;
        validation.validate_nbf = true;

        let token_data = decode::<IdTokenClaims>(id_token, &decoding_key, &validation)
            .map_err(|e| jsonwebtoken_error_to_oauth(e, audiences))?;

        Ok(token_data.claims)
    }

    async fn lookup_key_with_refresh(&self, kid: &str) -> Result<Jwk, OAuthError> {
        if let Some(jwk) = self.cache_lookup_if_fresh(kid).await {
            return Ok(jwk);
        }

        let mut cache = self.cache.write().await;
        // Another task may have refreshed while we were waiting.
        if cache.is_fresh()
            && let Some(jwk) = cache.keys.get(kid).cloned()
        {
            return Ok(jwk);
        }

        match fetch_jwks(&self.http, &self.jwks_endpoint).await {
            Ok((keys, ttl)) => {
                cache.keys = keys.into_iter().map(|k| (k.kid.clone(), k)).collect();
                cache.expires_at = Some(Instant::now() + ttl);
                cache.ttl = ttl;
                cache
                    .keys
                    .get(kid)
                    .cloned()
                    .ok_or_else(|| OAuthError::UnknownKid(kid.to_string()))
            }
            Err(fetch_err) => {
                // Stale-grace: known kid + within grace window → serve it.
                if cache.within_stale_grace()
                    && let Some(jwk) = cache.keys.get(kid).cloned()
                {
                    tracing::warn!(kid = %kid, "serving stale JWKS key while upstream unreachable");
                    return Ok(jwk);
                }
                Err(fetch_err)
            }
        }
    }

    async fn cache_lookup_if_fresh(&self, kid: &str) -> Option<Jwk> {
        let cache = self.cache.read().await;
        if cache.is_fresh() {
            cache.keys.get(kid).cloned()
        } else {
            None
        }
    }
}

fn jsonwebtoken_error_to_oauth(e: jsonwebtoken::errors::Error, audiences: &[&str]) -> OAuthError {
    use jsonwebtoken::errors::ErrorKind;
    match e.kind() {
        ErrorKind::InvalidToken | ErrorKind::InvalidSignature => OAuthError::InvalidSignature,
        ErrorKind::ExpiredSignature => OAuthError::Expired,
        ErrorKind::InvalidIssuer => OAuthError::IssuerMismatch("issuer rejected".into()),
        ErrorKind::InvalidAudience => OAuthError::AudienceMismatch {
            got: "<rejected>".into(),
            expected: audiences.iter().map(|s| s.to_string()).collect(),
        },
        ErrorKind::MissingRequiredClaim(c) => OAuthError::InvalidClaim(c.clone()),
        ErrorKind::ImmatureSignature => OAuthError::InvalidClaim("nbf in future".into()),
        ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => {
            OAuthError::UnsupportedAlgorithm(format!("{:?}", e.kind()))
        }
        ErrorKind::Base64(_) | ErrorKind::Json(_) | ErrorKind::Utf8(_) => {
            OAuthError::MalformedToken(e.to_string())
        }
        _ => OAuthError::MalformedToken(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;
    use std::sync::{LazyLock, Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    const ENV_VARS: &[&str] = &[
        "GCP_OAUTH_CLIENT_ID",
        "GCP_OAUTH_CLIENT_SECRET",
        "GCP_OAUTH_REDIRECT_URI",
    ];

    fn clear_env() {
        for k in ENV_VARS {
            unsafe { std::env::remove_var(k) };
        }
    }

    // PKCS#8 RSA-2048 test key + matching JWKS components. Generated for tests.
    const TEST_PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQCYyK/iTxWeNWSb
aS/yPPBIQ+jDbGyb8pt2ycklw7wEgaIdUvZgDeC57QENCg5lgs+g6OPRRsQY9fFG
lE6wKlZ4Zgq9kzRFjIAjXZkvD/H7bqlUdX0DlJXlYwMnGj2BR3DDbNQuRSlK4gWa
ZN5SYSVaBHsr3c89nN3GJH6IgikypsRc0mOEwbUdULs09ahSai7D7bSCCvCS5Jed
jAk10NqIMYMGjhlKpOWA+WcnHXpATSHtfampJq0s77bJej4MyQbGFims5XuuDmJ7
AF5rR4BndaRfaN323DLiKgR+UQKRMrhmBpyR35Yj/0pmo1XM713N4SLny1lq/oRH
9jAr2TPvAgMBAAECggEAL0w9iulhr2MnHKeBJNQxrKV9SvZnXxXJhAou36aLL7fz
+HEE/bJ+JgDViPRahZlr7ov6bwCh13pX8boa7BWHRGmOnKaUEY3P42Ln97ZPer+E
4zUl+PRIPUWcJcBNVxbHNXCc9SALCvgStPvSCZ2yYv4tJWTa8d98lokYtOjamSel
19RwTIA7rCFQqVpz2yR9tdAyZPl/+KOma1A+GwG6H+/G7tx7AqWyaMQiziEyGYKp
hD0yzlRBfUEU+HxN+SkhvpwONBQSPRcCKHO/6/eF2l2zQWwhA5xE9oLF/g3FL27W
AqRoLBgOO0HHmhBU7hjTooNwG5d2fSAhObIlRcxshQKBgQDQVNJNErEQWwaUYtde
2MT2EH5duWyPYaGJr3AhToTHACrV30eO5yuy9mdaBuyUSo/PrI7F72qX24ZNO9xk
1uxvWMtYwG15UZiU6l7zNgta86xdz0JCeADPbf1n2i8T7QAHsm9gZ0/jEGg+6/03
XMwtuA4LiR70QwvBFXXmP7IPRQKBgQC7viGPww84zRp6KrBWaemmkxIqSL/Uo5EV
UyBmHjmyUGNoWIZOpAsZNg3/MS/7B4PVWxMtpcwpL1YEALbiEZPMrzRvJS7j6xJj
mYCM7t8XRwK5SmMuS+VK7V319bEf17kpLAlq4mPPX+2+q0kn7Xs1PeEjz48wlqG7
TpJLhpG/owKBgQC/G8BbUXU6MrZDcrRs3l839oNlSM6sbPw5iMVM2HF299FTpmJH
VgrBPcYrUMS/d/KaqInES082BPwbZ3lSy9HShtrrDIKgUtisap81bnNWOMf6ukDn
JpxfrF9UYFLlbXikluwSvFMNUaS/a846dhcbLYc8z8mkesiSlDQ2RmH6HQKBgQCm
8fJcKUMO6mvB+NXncbUAl8VObnSOvIhV4x5rUDNUGeHbtuRvZ7YqzAN0SqP04IDd
p2gNbmJ2uQ4O7yexLZo1KBNDRlhE+hLXGHfUWtFsnIuSgtBhKcISd7LW9Yx02Vpg
fzU8o2XH0PDTXPLnm2i1NnpOYtJcjYXxznOOz3IpawKBgQCN8jXNUlVpc7XIA3mY
zX3MzefeYsmqNGe18oRDMVOCMMY55p8f9t48pXNUKpcm5eFKMKGeWxHPR05r+guL
/ICg2hMWnxEem3Foq/KGLFjGYnE1I1gM/4CPOYtGkLcx1FfaE4Y7Cv3fonA5CcLL
/XuwcOxx4KaoCDZK4kU0/Lsw8A==
-----END PRIVATE KEY-----
";

    const TEST_KID: &str = "test-key-1";

    fn test_jwk_components() -> &'static (String, String) {
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use rsa::RsaPrivateKey;
        use rsa::pkcs8::DecodePrivateKey;
        use rsa::traits::PublicKeyParts;

        static COMPONENTS: LazyLock<(String, String)> = LazyLock::new(|| {
            let priv_key = RsaPrivateKey::from_pkcs8_pem(TEST_PRIVATE_KEY_PEM)
                .expect("test private key parses");
            let n = URL_SAFE_NO_PAD.encode(priv_key.n().to_bytes_be());
            let e = URL_SAFE_NO_PAD.encode(priv_key.e().to_bytes_be());
            (n, e)
        });
        &COMPONENTS
    }

    fn jwks_response() -> serde_json::Value {
        let (n, e) = test_jwk_components();
        json!({
            "keys": [{
                "kid": TEST_KID,
                "kty": "RSA",
                "alg": "RS256",
                "use": "sig",
                "n": n,
                "e": e,
            }]
        })
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn sign_test_token(claims: serde_json::Value, kid: &str, alg: Algorithm) -> String {
        let mut header = Header::new(alg);
        header.kid = Some(kid.to_string());
        let key = EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY_PEM.as_bytes()).unwrap();
        encode(&header, &claims, &key).unwrap()
    }

    fn happy_claims(aud: &str) -> serde_json::Value {
        let now = now_secs();
        json!({
            "iss": ISSUER_GOOGLE,
            "aud": aud,
            "sub": "1234567890",
            "email": "user@example.com",
            "email_verified": true,
            "name": "Test User",
            "exp": now + 600,
            "iat": now - 5,
        })
    }

    #[test]
    fn from_oauth_error_maps_to_app_error_categories() {
        // 401s
        for e in [
            OAuthError::MalformedToken("x".into()),
            OAuthError::UnsupportedAlgorithm("HS256".into()),
            OAuthError::InvalidSignature,
            OAuthError::InvalidClaim("x".into()),
            OAuthError::AudienceMismatch {
                got: "a".into(),
                expected: vec!["b".into()],
            },
            OAuthError::IssuerMismatch("x".into()),
            OAuthError::Expired,
            OAuthError::UnknownKid("kid".into()),
            OAuthError::OAuthFailurePermanent("4xx".into()),
        ] {
            assert!(matches!(AppError::from(e), AppError::Unauthorized));
        }
        // 424 retryable
        for e in [
            OAuthError::JwksUnavailable("net".into()),
            OAuthError::OAuthFailureTransient("5xx".into()),
        ] {
            let mapped = AppError::from(e);
            assert!(
                matches!(
                    mapped,
                    AppError::DependencyFailed {
                        retryable: true,
                        ..
                    }
                ),
                "expected retryable DependencyFailed, got {mapped:?}"
            );
        }
        // 424 permanent (malformed upstream response)
        let mapped = AppError::from(OAuthError::MalformedResponse("bad json".into()));
        assert!(
            matches!(
                mapped,
                AppError::DependencyFailed {
                    retryable: false,
                    ..
                }
            ),
            "expected permanent DependencyFailed, got {mapped:?}"
        );
    }

    fn headers_with(value: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(reqwest::header::CACHE_CONTROL, value.parse().unwrap());
        h
    }

    #[test]
    fn cache_control_parses_max_age() {
        assert_eq!(
            parse_cache_control_max_age(&headers_with("public, max-age=21600, must-revalidate")),
            Some(Duration::from_secs(21600))
        );
    }

    #[test]
    fn cache_control_missing_header_is_none() {
        assert!(parse_cache_control_max_age(&reqwest::header::HeaderMap::new()).is_none());
    }

    #[test]
    fn cache_control_malformed_is_none() {
        assert!(parse_cache_control_max_age(&headers_with("max-age=oops")).is_none());
    }

    fn fake_oauth_client() -> OAuthClient {
        OAuthClient {
            config: ResolvedOAuthConfig {
                client_id: "client-id".into(),
                client_secret: "secret".into(),
                redirect_uri: "https://app.example/cb?path=x".into(),
            },
            http: reqwest::Client::new(),
            token_endpoint: GOOGLE_TOKEN_ENDPOINT.to_string(),
        }
    }

    #[test]
    fn build_auth_url_contains_required_params_and_encodes_redirect() {
        let url = build_auth_url(&fake_oauth_client(), "state-abc", "challenge-xyz");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-id"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fapp.example%2Fcb%3Fpath%3Dx"));
        assert!(url.contains("scope=openid+email+profile"));
        assert!(url.contains("state=state-abc"));
        assert!(url.contains("code_challenge=challenge-xyz"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("prompt=select_account"));
        assert!(url.contains("access_type=online"));
    }

    #[test]
    fn build_auth_url_encodes_special_chars_in_state() {
        let url = build_auth_url(&fake_oauth_client(), "a b&c=d", "x");
        assert!(url.contains("state=a+b%26c%3Dd"));
    }

    #[test]
    fn get_oauth_client_reads_env() {
        let _g = env_guard();
        clear_env();
        unsafe {
            std::env::set_var("GCP_OAUTH_CLIENT_ID", "env-id");
            std::env::set_var("GCP_OAUTH_CLIENT_SECRET", "env-secret");
            std::env::set_var("GCP_OAUTH_REDIRECT_URI", "https://env.example/cb");
        }
        let client = get_oauth_client(None).unwrap();
        clear_env();
        assert_eq!(client.client_id(), "env-id");
        assert_eq!(client.redirect_uri(), "https://env.example/cb");
    }

    #[test]
    fn get_oauth_client_errors_when_missing() {
        let _g = env_guard();
        clear_env();
        let err = get_oauth_client(None).unwrap_err();
        assert!(format!("{err:?}").contains("client_id"));
    }

    #[test]
    fn get_oauth_client_explicit_overrides_env() {
        let _g = env_guard();
        clear_env();
        unsafe {
            std::env::set_var("GCP_OAUTH_CLIENT_ID", "env-id");
            std::env::set_var("GCP_OAUTH_CLIENT_SECRET", "env-secret");
            std::env::set_var("GCP_OAUTH_REDIRECT_URI", "https://env.example/cb");
        }
        let client = get_oauth_client(
            GcpOAuthConfig::builder()
                .client_id("explicit-id")
                .client_secret("explicit-secret")
                .redirect_uri("https://explicit.example/cb")
                .build(),
        )
        .unwrap();
        clear_env();
        assert_eq!(client.client_id(), "explicit-id");
        assert_eq!(client.redirect_uri(), "https://explicit.example/cb");
    }

    async fn mock_token_client(server: &MockServer) -> OAuthClient {
        let http = reqwest::Client::builder().build().unwrap();
        OAuthClient {
            config: ResolvedOAuthConfig {
                client_id: "cid".into(),
                client_secret: "secret".into(),
                redirect_uri: "https://app.example/cb".into(),
            },
            http,
            token_endpoint: format!("{}/token", server.uri()),
        }
    }

    #[tokio::test]
    async fn exchange_code_happy_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "atk",
                "id_token": "itk",
                "expires_in": 3600u64,
                "token_type": "Bearer",
                "scope": "openid email profile",
            })))
            .mount(&server)
            .await;

        let client = mock_token_client(&server).await;
        let resp = exchange_code(&client, "code-123", "verifier-456")
            .await
            .unwrap();
        assert_eq!(resp.access_token, "atk");
        assert_eq!(resp.id_token, "itk");
        assert_eq!(resp.expires_in, 3600);
        assert!(resp.refresh_token.is_none());
    }

    #[tokio::test]
    async fn exchange_code_4xx_is_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad code"))
            .mount(&server)
            .await;
        let client = mock_token_client(&server).await;
        let err = exchange_code(&client, "code", "verifier")
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn exchange_code_5xx_is_dependency_failed_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream"))
            .mount(&server)
            .await;
        let client = mock_token_client(&server).await;
        let err = exchange_code(&client, "code", "verifier")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn exchange_code_malformed_json_is_dependency_failed_permanent() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        let client = mock_token_client(&server).await;
        let err = exchange_code(&client, "code", "verifier")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: false,
                ..
            }
        ));
    }

    async fn mount_jwks(server: &MockServer, body: serde_json::Value, status: u16) {
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_json(body)
                    .insert_header("cache-control", "public, max-age=600"),
            )
            .mount(server)
            .await;
    }

    fn verifier_pointing_at(server: &MockServer) -> Verifier {
        let http = reqwest::Client::new();
        Verifier::with_endpoint(http, format!("{}/certs", server.uri()))
    }

    #[tokio::test]
    async fn verify_happy_path() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let token = sign_test_token(happy_claims("client-id"), TEST_KID, Algorithm::RS256);
        let verifier = verifier_pointing_at(&server);
        let claims = verifier
            .verify_id_token(&token, &["client-id"])
            .await
            .expect("verify");
        assert_eq!(claims.sub, "1234567890");
        assert_eq!(claims.email.as_deref(), Some("user@example.com"));
        assert_eq!(claims.email_verified, Some(true));
    }

    #[tokio::test]
    async fn verify_rejects_wrong_audience() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let token = sign_test_token(happy_claims("not-our-client"), TEST_KID, Algorithm::RS256);
        let err = verifier_pointing_at(&server)
            .verify_id_token(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn verify_rejects_expired_token() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let now = now_secs();
        let claims = json!({
            "iss": ISSUER_GOOGLE,
            "aud": "client-id",
            "sub": "x",
            "exp": now - 600,
            "iat": now - 1200,
        });
        let token = sign_test_token(claims, TEST_KID, Algorithm::RS256);
        let err = verifier_pointing_at(&server)
            .verify_id_token(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn verify_rejects_bad_issuer() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let now = now_secs();
        let claims = json!({
            "iss": "https://evil.example",
            "aud": "client-id",
            "sub": "x",
            "exp": now + 600,
            "iat": now,
        });
        let token = sign_test_token(claims, TEST_KID, Algorithm::RS256);
        let err = verifier_pointing_at(&server)
            .verify_id_token(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn verify_rejects_unknown_kid() {
        let server = MockServer::start().await;
        mount_jwks(&server, jwks_response(), 200).await;
        let token = sign_test_token(happy_claims("client-id"), "rotated-kid", Algorithm::RS256);
        let err = verifier_pointing_at(&server)
            .verify_id_token(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[tokio::test]
    async fn verify_jwks_unavailable_when_cold_cache_and_5xx() {
        let server = MockServer::start().await;
        mount_jwks(&server, json!({"keys": []}), 503).await;
        let token = sign_test_token(happy_claims("client-id"), TEST_KID, Algorithm::RS256);
        let err = verifier_pointing_at(&server)
            .verify_id_token(&token, &["client-id"])
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::DependencyFailed {
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn verify_concurrent_cold_cache_fetches_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(jwks_response())
                    .insert_header("cache-control", "max-age=600"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let verifier = verifier_pointing_at(&server);
        let token = sign_test_token(happy_claims("client-id"), TEST_KID, Algorithm::RS256);

        let mut handles = Vec::new();
        for _ in 0..32 {
            let v = verifier.clone();
            let t = token.clone();
            handles.push(tokio::spawn(async move {
                v.verify_id_token(&t, &["client-id"]).await
            }));
        }
        for h in handles {
            h.await.unwrap().expect("verify");
        }
        // Mock's .expect(1) is verified on server drop.
    }
}
