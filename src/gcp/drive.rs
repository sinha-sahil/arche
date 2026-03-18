use google_drive3::{
    DriveHub,
    hyper_rustls::{self, HttpsConnector},
    hyper_util::{self, client::legacy::connect::HttpConnector},
    yup_oauth2::{self, ServiceAccountAuthenticator},
};

use crate::config::resolve_required_string;
use crate::error::AppError;

pub use crate::config::gcp::{GcpDriveConfig, GcpDriveConfigBuilder};

pub type GCPDriveClient = DriveHub<HttpsConnector<HttpConnector>>;

#[allow(dead_code)]
pub async fn get_drive_client(
    config: impl Into<Option<GcpDriveConfig>>,
) -> Result<GCPDriveClient, AppError> {
    let config = config.into().unwrap_or_default();

    let gcp_drive_key = resolve_required_string(
        config.service_account_key_path,
        "GCP_DRIVE_KEY",
        "service_account_key_path",
    )?;

    let auth = yup_oauth2::read_service_account_key(&gcp_drive_key)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                service = "google_drive",
                "Failed to read service account key"
            );
            AppError::internal_error(
                format!(
                    "Config error [service_account_key/GCP_DRIVE_KEY]: Failed to read GCP key: {}",
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
                service = "google_drive",
                "Failed to build authenticator"
            );
            AppError::internal_error(
                format!("Failed to build GCP Drive authenticator: {}", e),
                None,
            )
        })?;

    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| {
            tracing::error!(
                error = %e,
                service = "google_drive",
                "Failed to build HTTPS connector"
            );
            AppError::internal_error(
                format!("Failed to build HTTPS connector for GCP Drive: {}", e),
                None,
            )
        })?
        .https_or_http()
        .enable_http1()
        .build();

    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build(connector);

    Ok(DriveHub::new(client, authenticator))
}
