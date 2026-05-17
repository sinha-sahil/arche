use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::error::AppError;

const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
const JWT_LIFETIME_SECS: u64 = 3600;
const EXPIRY_SAFETY_MARGIN: Duration = Duration::from_secs(60);
const TOKEN_FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const TOKEN_FETCH_MAX_ATTEMPTS: u32 = 2;
const TOKEN_FETCH_RETRY_DELAY: Duration = Duration::from_millis(200);

#[derive(Clone, Deserialize)]
pub struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    #[serde(default)]
    private_key_id: Option<String>,
    #[serde(default)]
    token_uri: Option<String>,
    // PEM parsing is ~hundreds of microseconds; cached so signed-URL hot paths
    // pay it once. Arc so the cache is shared across clones.
    #[serde(skip, default)]
    parsed_key: Arc<OnceLock<Arc<RsaPrivateKey>>>,
}

impl std::fmt::Debug for ServiceAccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceAccountKey")
            .field("client_email", &self.client_email)
            .field("private_key_id", &self.private_key_id)
            .field("token_uri", &self.token_uri)
            .field("private_key", &"<redacted>")
            .finish()
    }
}

impl ServiceAccountKey {
    pub fn new(client_email: impl Into<String>, private_key: impl Into<String>) -> Self {
        Self {
            client_email: client_email.into(),
            private_key: normalize_private_key(&private_key.into()),
            private_key_id: None,
            token_uri: None,
            parsed_key: Arc::new(OnceLock::new()),
        }
    }

    pub async fn from_path(path: &str) -> Result<Self, AppError> {
        let json = tokio::fs::read_to_string(path).await.map_err(|e| {
            AppError::internal_error(
                format!("Failed to read service account key file at {path}: {e}"),
                None,
            )
        })?;
        Self::from_json(&json)
    }

    fn from_json(json: &str) -> Result<Self, AppError> {
        let mut key: Self = serde_json::from_str(json).map_err(|e| {
            AppError::internal_error(format!("Failed to parse service account JSON: {e}"), None)
        })?;
        key.private_key = normalize_private_key(&key.private_key);
        Ok(key)
    }

    fn token_uri(&self) -> &str {
        self.token_uri.as_deref().unwrap_or(DEFAULT_TOKEN_URI)
    }

    pub(crate) fn client_email(&self) -> &str {
        &self.client_email
    }

    pub(crate) fn sign_rs256_sha256(&self, data: &[u8]) -> Result<Vec<u8>, AppError> {
        let private_key = self.parsed_rsa_private_key()?;
        let signing_key: SigningKey<Sha256> = SigningKey::new((*private_key).clone());
        let signature = signing_key
            .try_sign(data)
            .map_err(|e| AppError::internal_error(format!("RS256 signing failed: {e}"), None))?;
        Ok(signature.to_bytes().to_vec())
    }

    fn parsed_rsa_private_key(&self) -> Result<Arc<RsaPrivateKey>, AppError> {
        if let Some(k) = self.parsed_key.get() {
            return Ok(k.clone());
        }
        let parsed = parse_rsa_pem(&self.private_key)?;
        // If another thread won the race, set() is a no-op and get() below
        // returns the value they stored.
        let _ = self.parsed_key.set(Arc::new(parsed));
        Ok(self
            .parsed_key
            .get()
            .expect("parsed_key initialised above")
            .clone())
    }
}

// Real GCP service-account keys are PKCS#8; PKCS#1 is here for hand-rolled
// keys passed to `ServiceAccountKey::new()`.
fn parse_rsa_pem(pem: &str) -> Result<RsaPrivateKey, AppError> {
    if let Ok(k) = RsaPrivateKey::from_pkcs8_pem(pem) {
        return Ok(k);
    }
    RsaPrivateKey::from_pkcs1_pem(pem).map_err(|e| {
        AppError::internal_error(
            format!("Invalid GCP service account private key (tried PKCS#8 and PKCS#1): {e}"),
            None,
        )
    })
}

// `.env` stores newlines as `\\n`; PEM parsers need real newlines.
fn normalize_private_key(raw: &str) -> String {
    raw.replace("\\n", "\n")
}

#[derive(Serialize)]
struct JwtClaims<'a> {
    iss: &'a str,
    scope: String,
    aud: &'a str,
    exp: u64,
    iat: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Clone)]
struct CachedToken {
    value: String,
    expires_at: Instant,
}

type CacheKey = (String, Vec<String>);

pub struct TokenSource {
    http: reqwest::Client,
    key: ServiceAccountKey,
    cache: Mutex<HashMap<CacheKey, CachedToken>>,
    locks: Mutex<HashMap<CacheKey, Arc<Mutex<()>>>>,
}

impl TokenSource {
    pub fn new(http: reqwest::Client, key: ServiceAccountKey) -> Self {
        Self {
            http,
            key,
            cache: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn signer_email(&self) -> &str {
        self.key.client_email()
    }

    pub(crate) fn sign_blob(&self, data: &[u8]) -> Result<Vec<u8>, AppError> {
        self.key.sign_rs256_sha256(data)
    }

    pub async fn access_token(&self, scopes: &[&str]) -> Result<String, AppError> {
        // Sort so the same scope set in different orders shares a cache slot.
        let mut sorted_scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        sorted_scopes.sort();
        let cache_key: CacheKey = (self.key.client_email.clone(), sorted_scopes);

        if let Some(token) = self.lookup_cached(&cache_key).await {
            return Ok(token);
        }

        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        if let Some(token) = self.lookup_cached(&cache_key).await {
            return Ok(token);
        }

        let fetched = self.fetch_token(scopes).await?;
        let mut cache = self.cache.lock().await;
        cache.insert(cache_key, fetched.clone());
        Ok(fetched.value)
    }

    async fn lookup_cached(&self, key: &CacheKey) -> Option<String> {
        let cache = self.cache.lock().await;
        cache
            .get(key)
            .filter(|t| t.expires_at > Instant::now())
            .map(|t| t.value.clone())
    }

    async fn fetch_token(&self, scopes: &[&str]) -> Result<CachedToken, AppError> {
        let assertion = self.sign_assertion(scopes)?;

        let mut last_transient: Option<String> = None;
        for attempt in 1..=TOKEN_FETCH_MAX_ATTEMPTS {
            match self.try_fetch_token(&assertion).await {
                Ok(token) => return Ok(token),
                Err(TokenFetchError::Permanent(e)) => return Err(e),
                Err(TokenFetchError::Transient(detail)) => {
                    if attempt < TOKEN_FETCH_MAX_ATTEMPTS {
                        tracing::warn!(
                            attempt,
                            error = %detail,
                            "Transient GCP token fetch error, retrying"
                        );
                        tokio::time::sleep(TOKEN_FETCH_RETRY_DELAY).await;
                    }
                    last_transient = Some(detail);
                }
            }
        }

        Err(AppError::dependency_failed(
            "gcp-oauth2",
            format!(
                "failed to fetch access token after {TOKEN_FETCH_MAX_ATTEMPTS} attempts: {}",
                last_transient.unwrap_or_else(|| "unknown error".into())
            ),
        ))
    }

    fn sign_assertion(&self, scopes: &[&str]) -> Result<String, AppError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| AppError::internal_error(format!("Clock error: {e}"), None))?
            .as_secs();

        let claims = JwtClaims {
            iss: &self.key.client_email,
            scope: scopes.join(" "),
            aud: self.key.token_uri(),
            iat: now,
            exp: now + JWT_LIFETIME_SECS,
        };

        let mut header = Header::new(Algorithm::RS256);
        header.kid = self.key.private_key_id.clone();

        let encoding_key =
            EncodingKey::from_rsa_pem(self.key.private_key.as_bytes()).map_err(|e| {
                AppError::internal_error(
                    format!("Invalid GCP service account private key: {e}"),
                    None,
                )
            })?;

        jsonwebtoken::encode(&header, &claims, &encoding_key).map_err(|e| {
            AppError::internal_error(format!("Failed to sign JWT for GCP token: {e}"), None)
        })
    }

    async fn try_fetch_token(&self, assertion: &str) -> Result<CachedToken, TokenFetchError> {
        let send_fut = self
            .http
            .post(self.key.token_uri())
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion),
            ])
            .send();

        let resp = match tokio::time::timeout(TOKEN_FETCH_TIMEOUT, send_fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(TokenFetchError::Transient(format!("send error: {e}"))),
            Err(_) => {
                return Err(TokenFetchError::Transient(format!(
                    "timed out after {}s",
                    TOKEN_FETCH_TIMEOUT.as_secs()
                )));
            }
        };

        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| TokenFetchError::Transient(format!("read body error: {e}")))?;

        if status.is_server_error() {
            return Err(TokenFetchError::Transient(format!(
                "HTTP {status}: {}",
                parse_token_error(&body)
            )));
        }

        if !status.is_success() {
            return Err(TokenFetchError::Permanent(
                AppError::dependency_failed_permanent(
                    "gcp-oauth2",
                    format!(
                        "token endpoint returned {status}: {}",
                        parse_token_error(&body)
                    ),
                ),
            ));
        }

        let token: TokenResponse = serde_json::from_slice(&body).map_err(|e| {
            TokenFetchError::Permanent(AppError::dependency_failed_permanent(
                "gcp-oauth2",
                format!("malformed token response: {e}"),
            ))
        })?;

        let lifetime = Duration::from_secs(token.expires_in)
            .checked_sub(EXPIRY_SAFETY_MARGIN)
            .unwrap_or(Duration::ZERO);

        Ok(CachedToken {
            value: token.access_token,
            expires_at: Instant::now() + lifetime,
        })
    }
}

enum TokenFetchError {
    Transient(String),
    Permanent(AppError),
}

fn parse_token_error(body: &[u8]) -> String {
    serde_json::from_slice::<TokenErrorResponse>(body)
        .map(|e| match e.error_description {
            Some(desc) => format!("{}: {desc}", e.error),
            None => e.error,
        })
        .unwrap_or_else(|_| String::from_utf8_lossy(body).into_owned())
}
