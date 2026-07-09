use reqwest::Url;
use serde::Deserialize;

use crate::config::oidc::OidcConfig;
use crate::error::AppError;

use super::{ProviderMetadata, default_http_client, dependency_label};

const DEFAULT_SCOPES: &str = "openid email profile";

#[derive(Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub id_token: String,
    pub expires_in: u64,
    pub token_type: String,
    #[serde(default)]
    pub scope: Option<String>,
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

#[derive(Clone)]
pub struct OidcClient {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    scopes: String,
    provider: ProviderMetadata,
    http: reqwest::Client,
}

impl std::fmt::Debug for OidcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcClient")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .field("provider", &self.provider)
            .finish()
    }
}

impl OidcClient {
    pub fn new(provider: ProviderMetadata, config: OidcConfig) -> Result<Self, AppError> {
        validate_endpoint("auth_endpoint", &provider.auth_endpoint)?;
        validate_endpoint("token_endpoint", &provider.token_endpoint)?;
        validate_endpoint("jwks_endpoint", &provider.jwks_endpoint)?;

        Ok(Self {
            client_id: config.client_id,
            client_secret: config.client_secret,
            redirect_uri: config.redirect_uri,
            scopes: config.scopes.unwrap_or_else(|| DEFAULT_SCOPES.to_string()),
            provider,
            http: default_http_client("OIDC")?,
        })
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn provider(&self) -> &ProviderMetadata {
        &self.provider
    }

    pub fn auth_url(&self, state: &str, pkce_challenge: &str) -> String {
        let mut params: Vec<(&str, &str)> = vec![
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", &self.redirect_uri),
            ("scope", &self.scopes),
            ("state", state),
            ("code_challenge", pkce_challenge),
            ("code_challenge_method", "S256"),
        ];
        for (k, v) in &self.provider.extra_auth_params {
            params.push((k, v));
        }
        Url::parse_with_params(&self.provider.auth_endpoint, &params)
            .expect("endpoints validated in OidcClient::new")
            .into()
    }

    pub async fn exchange_code(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> Result<TokenResponse, AppError> {
        let dep = dependency_label(&self.provider.key);
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", pkce_verifier),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
        ];

        let resp = self
            .http
            .post(&self.provider.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|e| AppError::dependency_failed(&dep, format!("token request: {e}")))?;

        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::dependency_failed(&dep, format!("token response body: {e}")))?;

        if status.is_server_error() {
            return Err(AppError::dependency_failed(
                &dep,
                format!(
                    "token endpoint returned {status}: {}",
                    String::from_utf8_lossy(&bytes)
                ),
            ));
        }
        if !status.is_success() {
            tracing::debug!(
                status = %status,
                body = %String::from_utf8_lossy(&bytes),
                "token exchange rejected"
            );
            return Err(AppError::Unauthorized);
        }

        serde_json::from_slice::<TokenResponse>(&bytes).map_err(|e| {
            AppError::dependency_failed_permanent(&dep, format!("token response parse: {e}"))
        })
    }
}

fn validate_endpoint(field: &str, value: &str) -> Result<(), AppError> {
    Url::parse(value).map(|_| ()).map_err(|e| {
        AppError::internal_error(
            format!("Config error [{field}]: invalid URL {value:?}: {e}"),
            None,
        )
    })
}
