use std::time::Duration;

use crate::error::AppError;

#[allow(clippy::module_inception)]
mod client;
mod jwks;
mod provider;
mod verifier;

pub use client::{OidcClient, TokenResponse};
pub use provider::ProviderMetadata;
pub use verifier::Verifier;

const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TCP_KEEPALIVE: Duration = Duration::from_secs(60);

fn default_http_client(context: &str) -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TOTAL_TIMEOUT)
        .tcp_keepalive(HTTP_TCP_KEEPALIVE)
        .build()
        .map_err(|e| {
            AppError::internal_error(format!("Failed to build {context} HTTP client: {e}"), None)
        })
}

fn dependency_label(provider_key: &str) -> String {
    format!("oidc-{provider_key}")
}

fn reject_token(reason: impl std::fmt::Display) -> AppError {
    tracing::debug!(reason = %reason, "id token rejected");
    AppError::Unauthorized
}
