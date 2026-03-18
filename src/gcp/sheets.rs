use google_sheets4::{
    Sheets, hyper_rustls, hyper_util,
    yup_oauth2::{self, ServiceAccountAuthenticator},
};

use crate::config::resolve_required_string;
use crate::error::AppError;

pub use crate::config::gcp::{GcpSheetsConfig, GcpSheetsConfigBuilder};

pub type GCPSheetsClient =
    Sheets<hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>>;

#[allow(dead_code)]
pub async fn get_sheets_client(
    config: impl Into<Option<GcpSheetsConfig>>,
) -> Result<GCPSheetsClient, AppError> {
    let config = config.into().unwrap_or_default();

    let gcp_sheets_key = resolve_required_string(
        config.service_account_key_path,
        "GCP_SHEETS_KEY",
        "service_account_key_path",
    )?;

    let auth = yup_oauth2::read_service_account_key(&gcp_sheets_key)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                service = "google_sheets",
                "Failed to read service account key"
            );
            AppError::internal_error(
                format!(
                    "Config error [service_account_key/GCP_SHEETS_KEY]: Failed to read GCP key: {}",
                    e
                ),
                None,
            )
        })?;

    let authenticator = ServiceAccountAuthenticator::builder(auth)
        .build()
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                service = "google_sheets",
                "Failed to build authenticator"
            );
            AppError::internal_error(
                format!("Failed to build GCP Sheets authenticator: {}", e),
                None,
            )
        })?;

    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| {
            tracing::error!(
                error = %e,
                service = "google_sheets",
                "Failed to build HTTPS connector"
            );
            AppError::internal_error(
                format!("Failed to build HTTPS connector for GCP Sheets: {}", e),
                None,
            )
        })?
        .https_or_http()
        .enable_http1()
        .build();

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector);

    Ok(Sheets::new(client, authenticator))
}
