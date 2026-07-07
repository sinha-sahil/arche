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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn discovery_doc(issuer: &str) -> serde_json::Value {
        json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "jwks_uri": format!("{issuer}/jwks"),
        })
    }

    async fn mount_discovery(server: &MockServer, body: serde_json::Value, status: u16) {
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn discover_happy_path() {
        let server = MockServer::start().await;
        let issuer = server.uri();
        mount_discovery(&server, discovery_doc(&issuer), 200).await;

        let p = ProviderMetadata::discover("acme", &issuer, &reqwest::Client::new())
            .await
            .unwrap();
        assert_eq!(p.key, "acme");
        assert_eq!(p.issuers, vec![issuer.clone()]);
        assert_eq!(p.auth_endpoint, format!("{issuer}/authorize"));
        assert_eq!(p.token_endpoint, format!("{issuer}/token"));
        assert_eq!(p.jwks_endpoint, format!("{issuer}/jwks"));
        assert!(p.extra_auth_params.is_empty());
    }

    #[tokio::test]
    async fn discover_trailing_slash_issuer_matches() {
        let server = MockServer::start().await;
        let issuer = server.uri();
        mount_discovery(&server, discovery_doc(&issuer), 200).await;

        let p = ProviderMetadata::discover("acme", &format!("{issuer}/"), &reqwest::Client::new())
            .await
            .unwrap();
        assert_eq!(p.issuers, vec![issuer]);
    }

    #[tokio::test]
    async fn discover_rejects_issuer_mismatch() {
        let server = MockServer::start().await;
        mount_discovery(&server, discovery_doc("https://evil.example"), 200).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
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

    #[tokio::test]
    async fn discover_404_is_permanent() {
        let server = MockServer::start().await;
        mount_discovery(&server, json!({}), 404).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
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

    #[tokio::test]
    async fn discover_503_is_retryable() {
        let server = MockServer::start().await;
        mount_discovery(&server, json!({}), 503).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
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
    async fn discover_malformed_doc_is_permanent() {
        let server = MockServer::start().await;
        mount_discovery(&server, json!({"issuer": "x"}), 200).await;

        let err = ProviderMetadata::discover("acme", &server.uri(), &reqwest::Client::new())
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
}
