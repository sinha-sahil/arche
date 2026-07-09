use serde::Deserialize;

use crate::error::AppError;

use super::{default_http_client, dependency_label};

#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub key: String,
    pub issuers: Vec<String>,
    pub auth_endpoint: String,
    pub token_endpoint: String,
    pub jwks_endpoint: String,
    pub extra_auth_params: Vec<(String, String)>,
}

impl ProviderMetadata {
    pub async fn discover(
        key: impl Into<String>,
        issuer: &str,
        http: &reqwest::Client,
    ) -> Result<Self, AppError> {
        let key = key.into();
        let doc = discover_inner(issuer, http, &dependency_label(&key)).await?;
        Ok(Self {
            key,
            issuers: vec![doc.issuer],
            auth_endpoint: doc.authorization_endpoint,
            token_endpoint: doc.token_endpoint,
            jwks_endpoint: doc.jwks_uri,
            extra_auth_params: Vec::new(),
        })
    }

    pub async fn discover_default(key: impl Into<String>, issuer: &str) -> Result<Self, AppError> {
        let http = default_http_client("OIDC discovery")?;
        Self::discover(key, issuer, &http).await
    }
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

async fn discover_inner(
    issuer: &str,
    http: &reqwest::Client,
    dependency: &str,
) -> Result<DiscoveryDocument, AppError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );

    let resp =
        http.get(&url).send().await.map_err(|e| {
            AppError::dependency_failed(dependency, format!("discovery fetch: {e}"))
        })?;

    let status = resp.status();
    if status.is_server_error() {
        return Err(AppError::dependency_failed(
            dependency,
            format!("discovery endpoint returned {status}"),
        ));
    }
    if !status.is_success() {
        return Err(AppError::dependency_failed_permanent(
            dependency,
            format!("discovery endpoint returned {status}"),
        ));
    }

    let body = resp
        .bytes()
        .await
        .map_err(|e| AppError::dependency_failed(dependency, format!("discovery body: {e}")))?;
    let doc: DiscoveryDocument = serde_json::from_slice(&body).map_err(|e| {
        AppError::dependency_failed_permanent(dependency, format!("discovery parse: {e}"))
    })?;

    if doc.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err(AppError::dependency_failed_permanent(
            dependency,
            format!(
                "issuer mismatch: requested {issuer}, document says {}",
                doc.issuer
            ),
        ));
    }

    Ok(doc)
}
