use std::sync::Arc;
use std::time::Duration;

use reqwest::{IntoUrl, Method, RequestBuilder};

use crate::error::AppError;
use crate::gcp::token::{ServiceAccountKey, TokenSource};

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct GcpClient {
    http: reqwest::Client,
    token: Arc<TokenSource>,
    scopes: Arc<Vec<String>>,
}

impl GcpClient {
    pub async fn new(
        key: Option<ServiceAccountKey>,
        path: Option<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, AppError> {
        let http = reqwest::Client::builder()
            .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
            .timeout(DEFAULT_TOTAL_TIMEOUT)
            .build()
            .map_err(|e| {
                AppError::internal_error(format!("Failed to build GCP HTTP client: {e}"), None)
            })?;
        Self::with_http(http, key, path, scopes).await
    }

    pub async fn with_http(
        http: reqwest::Client,
        key: Option<ServiceAccountKey>,
        path: Option<String>,
        scopes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, AppError> {
        let sa_key = match (key, path) {
            (Some(k), _) => k,
            (None, Some(p)) => ServiceAccountKey::from_path(&p).await?,
            (None, None) => {
                return Err(AppError::internal_error(
                    "GcpClient requires a ServiceAccountKey or a service-account JSON file path"
                        .into(),
                    None,
                ));
            }
        };

        let token = Arc::new(TokenSource::new(http.clone(), sa_key));
        Ok(Self {
            http,
            token,
            scopes: Arc::new(scopes.into_iter().map(Into::into).collect()),
        })
    }

    /// Fork with new scopes, sharing the underlying token cache.
    pub fn with_scopes(&self, scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            http: self.http.clone(),
            token: self.token.clone(),
            scopes: Arc::new(scopes.into_iter().map(Into::into).collect()),
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn signer_email(&self) -> &str {
        self.token.signer_email()
    }

    pub(crate) fn sign_blob(&self, data: &[u8]) -> Result<Vec<u8>, AppError> {
        self.token.sign_blob(data)
    }

    pub async fn access_token(&self) -> Result<String, AppError> {
        let scopes: Vec<&str> = self.scopes.iter().map(String::as_str).collect();
        self.token.access_token(&scopes).await
    }

    pub async fn bearer(&self) -> Result<String, AppError> {
        Ok(format!("Bearer {}", self.access_token().await?))
    }

    pub async fn request<U: IntoUrl>(
        &self,
        method: Method,
        url: U,
    ) -> Result<RequestBuilder, AppError> {
        let token = self.access_token().await?;
        Ok(self.http.request(method, url).bearer_auth(token))
    }

    pub async fn get<U: IntoUrl>(&self, url: U) -> Result<RequestBuilder, AppError> {
        self.request(Method::GET, url).await
    }

    pub async fn post<U: IntoUrl>(&self, url: U) -> Result<RequestBuilder, AppError> {
        self.request(Method::POST, url).await
    }

    pub async fn put<U: IntoUrl>(&self, url: U) -> Result<RequestBuilder, AppError> {
        self.request(Method::PUT, url).await
    }

    pub async fn patch<U: IntoUrl>(&self, url: U) -> Result<RequestBuilder, AppError> {
        self.request(Method::PATCH, url).await
    }

    pub async fn delete<U: IntoUrl>(&self, url: U) -> Result<RequestBuilder, AppError> {
        self.request(Method::DELETE, url).await
    }
}
