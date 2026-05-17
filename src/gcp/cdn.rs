use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::{resolve_optional_string, resolve_required_string};
use crate::error::AppError;
use crate::gcp::client::GcpClient;
use crate::gcp::token::ServiceAccountKey;

pub use crate::config::gcp::{GcpCdnConfig, GcpCdnConfigBuilder};

pub const CDN_SCOPE: &str = "https://www.googleapis.com/auth/compute";
const DEFAULT_COMPUTE_BASE_URL: &str = "https://compute.googleapis.com";

const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub async fn get_cdn_client(
    sa_key: Option<ServiceAccountKey>,
    sa_path: Option<String>,
    config: impl Into<Option<GcpCdnConfig>>,
) -> Result<GcpCdnClient, AppError> {
    let resolved = resolve_cdn_config(config.into().unwrap_or_default())?;
    let gcp = GcpClient::new(sa_key, sa_path, [CDN_SCOPE]).await?;
    Ok(GcpCdnClient::new(
        gcp,
        resolved.project_id,
        resolved.default_url_map,
        resolved.base_url,
    ))
}

#[derive(Debug)]
struct ResolvedCdnConfig {
    project_id: String,
    default_url_map: Option<String>,
    base_url: Option<String>,
}

fn resolve_cdn_config(config: GcpCdnConfig) -> Result<ResolvedCdnConfig, AppError> {
    Ok(ResolvedCdnConfig {
        project_id: resolve_required_string(config.project_id, "GCP_CDN_PROJECT_ID", "project_id")?,
        default_url_map: resolve_optional_string(config.url_map, "GCP_CDN_URL_MAP"),
        base_url: resolve_optional_string(config.compute_base_url, "GCP_CDN_BASE_URL"),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct InvalidationOperation {
    pub id: String,
    /// Operation name — pass this to `invalidation_status` to poll progress.
    pub name: String,
    /// Operation status: `PENDING`, `RUNNING`, or `DONE`.
    pub status: String,
    /// Best-effort progress, 0..=100.
    pub progress: Option<i32>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    /// Joined error messages if the operation reported failures.
    pub error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationWire {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    progress: Option<i32>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
    #[serde(default)]
    error: Option<OperationErrorBlock>,
}

#[derive(Deserialize)]
struct OperationErrorBlock {
    #[serde(default)]
    errors: Vec<OperationErrorItem>,
}

#[derive(Deserialize)]
struct OperationErrorItem {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl From<OperationWire> for InvalidationOperation {
    fn from(w: OperationWire) -> Self {
        let error = w.error.and_then(|e| {
            let parts: Vec<String> = e
                .errors
                .into_iter()
                .filter_map(|item| match (item.code, item.message) {
                    (Some(c), Some(m)) => Some(format!("{c}: {m}")),
                    (Some(c), None) => Some(c),
                    (None, Some(m)) => Some(m),
                    (None, None) => None,
                })
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("; "))
            }
        });
        Self {
            id: w.id.unwrap_or_default(),
            name: w.name.unwrap_or_default(),
            status: w.status.unwrap_or_default(),
            progress: w.progress,
            start_time: w.start_time,
            end_time: w.end_time,
            error,
        }
    }
}

#[derive(Debug, Deserialize)]
struct CdnErrorEnvelope {
    error: CdnErrorBody,
}

#[derive(Debug, Deserialize)]
struct CdnErrorBody {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Cloud CDN invalidation client.
///
/// Currently scoped to **global** URL maps and global operations. Regional
/// URL maps (`/regions/{region}/urlMaps/...`) are not supported by this client;
/// invalidation against one will return a 404.
#[derive(Clone)]
pub struct GcpCdnClient {
    gcp: GcpClient,
    project_id: String,
    default_url_map: Option<String>,
    base_url: String,
}

impl GcpCdnClient {
    pub fn new(
        gcp: GcpClient,
        project_id: String,
        default_url_map: Option<String>,
        base_url: Option<String>,
    ) -> Self {
        let base_url = base_url
            .unwrap_or_else(|| DEFAULT_COMPUTE_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        Self {
            gcp,
            project_id,
            default_url_map,
            base_url,
        }
    }

    fn resolve_url_map(&self, url_map: Option<&str>, op: &str) -> Result<String, AppError> {
        url_map
            .map(|s| s.to_string())
            .or_else(|| self.default_url_map.clone())
            .ok_or_else(|| {
                AppError::bad_request(
                    None,
                    Some(format!(
                        "Cloud CDN {op}: url_map not provided and no default configured"
                    )),
                    None,
                )
            })
    }

    /// Invalidate cached content for `path` on the URL map.
    ///
    /// `path` must start with `/` and may use `*` as a suffix wildcard
    /// (e.g. `/static/*`). Pass `host` to scope the invalidation to a single
    /// hostname routed by the URL map.
    ///
    /// Returns the operation; poll its `name` with `invalidation_status` to
    /// follow progress until `status == "DONE"`.
    pub async fn invalidate(
        &self,
        url_map: Option<&str>,
        path: &str,
        host: Option<&str>,
    ) -> Result<InvalidationOperation, AppError> {
        let url_map = self.resolve_url_map(url_map, "invalidate")?;

        if path.is_empty() {
            return Err(AppError::bad_request(
                None,
                Some("Cloud CDN invalidate: path must not be empty".to_string()),
                None,
            ));
        }

        let url = format!(
            "{}/compute/v1/projects/{}/global/urlMaps/{}/invalidateCache",
            self.base_url,
            utf8_percent_encode(&self.project_id, PATH_SEGMENT),
            utf8_percent_encode(&url_map, PATH_SEGMENT),
        );

        let mut body = json!({ "path": path });
        if let Some(h) = host {
            body["host"] = json!(h);
        }

        let resp = self
            .gcp
            .post(&url)
            .await?
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                AppError::dependency_failed("gcp-cdn", format!("invalidate request failed: {e}"))
            })?;

        let wire: OperationWire = handle_response(resp, "invalidate").await?;
        Ok(wire.into())
    }

    /// Poll an invalidation operation by `name` (the value returned in
    /// `InvalidationOperation::name` from `invalidate`).
    pub async fn invalidation_status(
        &self,
        operation_name: &str,
    ) -> Result<InvalidationOperation, AppError> {
        if operation_name.is_empty() {
            return Err(AppError::bad_request(
                None,
                Some("Cloud CDN invalidation_status: operation_name must not be empty".to_string()),
                None,
            ));
        }

        let url = format!(
            "{}/compute/v1/projects/{}/global/operations/{}",
            self.base_url,
            utf8_percent_encode(&self.project_id, PATH_SEGMENT),
            utf8_percent_encode(operation_name, PATH_SEGMENT),
        );

        let resp = self.gcp.get(&url).await?.send().await.map_err(|e| {
            AppError::dependency_failed(
                "gcp-cdn",
                format!("invalidation_status request failed: {e}"),
            )
        })?;

        let wire: OperationWire = handle_response(resp, "invalidation_status").await?;
        Ok(wire.into())
    }
}

async fn handle_response<T: for<'de> Deserialize<'de>>(
    resp: reqwest::Response,
    op: &str,
) -> Result<T, AppError> {
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| {
        AppError::dependency_failed("gcp-cdn", format!("failed reading {op} response body: {e}"))
    })?;

    if !status.is_success() {
        return Err(AppError::dependency_failed(
            "gcp-cdn",
            format!("{op} returned HTTP {status}: {}", parse_cdn_error(&bytes)),
        ));
    }

    serde_json::from_slice::<T>(&bytes).map_err(|e| {
        AppError::dependency_failed("gcp-cdn", format!("failed to parse {op} response: {e}"))
    })
}

fn parse_cdn_error(body: &[u8]) -> String {
    serde_json::from_slice::<CdnErrorEnvelope>(body)
        .ok()
        .map(|env| match (env.error.status, env.error.message) {
            (Some(s), Some(m)) => format!("{s}: {m}"),
            (Some(s), None) => s,
            (None, Some(m)) => m,
            (None, None) => String::new(),
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    const ENV_VARS: &[&str] = &["GCP_CDN_PROJECT_ID", "GCP_CDN_URL_MAP", "GCP_CDN_BASE_URL"];

    fn clear_cdn_env() {
        for k in ENV_VARS {
            unsafe { std::env::remove_var(k) };
        }
    }

    #[test]
    fn resolve_cdn_config_errors_when_project_id_missing() {
        let _g = env_guard();
        clear_cdn_env();
        let err = resolve_cdn_config(GcpCdnConfig::default()).unwrap_err();
        assert!(
            format!("{err:?}").contains("project_id"),
            "expected project_id in error, got: {err:?}"
        );
    }

    #[test]
    fn resolve_cdn_config_reads_from_env_when_unset() {
        let _g = env_guard();
        clear_cdn_env();
        unsafe {
            std::env::set_var("GCP_CDN_PROJECT_ID", "from-env");
            std::env::set_var("GCP_CDN_URL_MAP", "env-map");
            std::env::set_var("GCP_CDN_BASE_URL", "https://env.test");
        }
        let resolved = resolve_cdn_config(GcpCdnConfig::default()).unwrap();
        clear_cdn_env();
        assert_eq!(resolved.project_id, "from-env");
        assert_eq!(resolved.default_url_map.as_deref(), Some("env-map"));
        assert_eq!(resolved.base_url.as_deref(), Some("https://env.test"));
    }

    #[test]
    fn resolve_cdn_config_explicit_overrides_env() {
        let _g = env_guard();
        clear_cdn_env();
        unsafe {
            std::env::set_var("GCP_CDN_PROJECT_ID", "from-env");
            std::env::set_var("GCP_CDN_URL_MAP", "env-map");
        }
        let resolved = resolve_cdn_config(
            GcpCdnConfig::builder()
                .project_id("explicit-proj")
                .url_map("explicit-map")
                .compute_base_url("https://explicit.test")
                .build(),
        )
        .unwrap();
        clear_cdn_env();
        assert_eq!(resolved.project_id, "explicit-proj");
        assert_eq!(resolved.default_url_map.as_deref(), Some("explicit-map"));
        assert_eq!(resolved.base_url.as_deref(), Some("https://explicit.test"));
    }

    #[test]
    fn parse_cdn_error_extracts_status_and_message() {
        let body = br#"{"error":{"code":403,"status":"PERMISSION_DENIED","message":"missing compute.urlMaps.invalidateCache"}}"#;
        assert_eq!(
            parse_cdn_error(body),
            "PERMISSION_DENIED: missing compute.urlMaps.invalidateCache"
        );
    }

    #[test]
    fn parse_cdn_error_falls_back_to_raw_body() {
        assert_eq!(parse_cdn_error(b"not json"), "not json");
    }

    #[test]
    fn operation_wire_to_invalidation_operation_joins_errors() {
        let wire: OperationWire = serde_json::from_str(
            r#"{
                "id": "5678",
                "name": "operation-xyz",
                "status": "DONE",
                "progress": 100,
                "startTime": "2024-01-01T00:00:00Z",
                "endTime": "2024-01-01T00:00:30Z",
                "error": {
                    "errors": [
                        {"code": "RESOURCE_NOT_FOUND", "message": "URL map missing"},
                        {"code": "QUOTA_EXCEEDED"}
                    ]
                }
            }"#,
        )
        .expect("parse");
        let op: InvalidationOperation = wire.into();
        assert_eq!(op.id, "5678");
        assert_eq!(op.name, "operation-xyz");
        assert_eq!(op.status, "DONE");
        assert_eq!(op.progress, Some(100));
        assert_eq!(
            op.error.as_deref(),
            Some("RESOURCE_NOT_FOUND: URL map missing; QUOTA_EXCEEDED")
        );
    }

    #[test]
    fn operation_wire_no_error_yields_none() {
        let wire: OperationWire =
            serde_json::from_str(r#"{"id":"1","name":"op","status":"PENDING"}"#).expect("parse");
        let op: InvalidationOperation = wire.into();
        assert!(op.error.is_none());
    }

    const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
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

    async fn test_client(default_url_map: Option<&str>) -> GcpCdnClient {
        let sa = ServiceAccountKey::new("test@example.iam.gserviceaccount.com", TEST_PRIVATE_KEY);
        let gcp = GcpClient::new(Some(sa), None, [CDN_SCOPE])
            .await
            .expect("test gcp client");
        GcpCdnClient::new(
            gcp,
            "p".to_string(),
            default_url_map.map(String::from),
            None,
        )
    }

    #[tokio::test]
    async fn resolve_url_map_uses_default_when_per_call_missing() {
        let client = test_client(Some("default-map")).await;
        let resolved = client.resolve_url_map(None, "invalidate").unwrap();
        assert_eq!(resolved, "default-map");
    }

    #[tokio::test]
    async fn resolve_url_map_prefers_per_call_value() {
        let client = test_client(Some("default-map")).await;
        let resolved = client
            .resolve_url_map(Some("override"), "invalidate")
            .unwrap();
        assert_eq!(resolved, "override");
    }

    #[tokio::test]
    async fn resolve_url_map_errors_when_no_default_or_per_call() {
        let client = test_client(None).await;
        let err = client.resolve_url_map(None, "invalidate").unwrap_err();
        assert!(format!("{err:?}").contains("url_map"));
    }

    #[tokio::test]
    async fn new_client_strips_trailing_slash_from_base_url() {
        let sa = ServiceAccountKey::new("test@example.iam.gserviceaccount.com", TEST_PRIVATE_KEY);
        let gcp = GcpClient::new(Some(sa), None, [CDN_SCOPE])
            .await
            .expect("test gcp client");
        let client = GcpCdnClient::new(
            gcp,
            "p".to_string(),
            None,
            Some("https://compute.googleapis.com/".to_string()),
        );
        assert_eq!(client.base_url, "https://compute.googleapis.com");
    }
}
