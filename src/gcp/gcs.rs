use std::collections::HashMap;
use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, UtcOffset};

use crate::config::{resolve_optional, resolve_optional_string};
use crate::error::AppError;
use crate::gcp::client::GcpClient;
use crate::gcp::token::ServiceAccountKey;

pub use crate::config::gcp::{GcsConfig, GcsConfigBuilder};

pub const GCS_SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_write";
const DEFAULT_GCS_BASE_URL: &str = "https://storage.googleapis.com";
const SIGNED_URL_HOST: &str = "storage.googleapis.com";
const DEFAULT_SIGNED_EXPIRY_SECS: u64 = 900;
const MAX_SIGNED_EXPIRY_SECS: u64 = 604800;

// V4 signing treats `/` as a literal separator; JSON-API encodes it so
// the route's `{object}` segment is a single name.
const SIGNING_PATH: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

const JSON_API_OBJECT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Debug, Clone, Serialize)]
pub struct ObjectMetadata {
    pub name: String,
    pub bucket: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub etag: Option<String>,
    /// Object generation. GCS sends this as a stringified int64 over the wire;
    /// we parse it so it can be round-tripped into `download`/`head`/`delete`.
    pub generation: Option<i64>,
    pub updated: Option<String>,
    pub md5_hash: Option<String>,
    /// User-defined metadata (e.g. `modified_by`).
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListPage {
    pub items: Vec<ObjectMetadata>,
    pub next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectMetadataWire {
    name: String,
    bucket: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    generation: Option<String>,
    #[serde(default)]
    updated: Option<String>,
    #[serde(default)]
    md5_hash: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
}

impl From<ObjectMetadataWire> for ObjectMetadata {
    fn from(w: ObjectMetadataWire) -> Self {
        let size = w.size.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
        let generation = w.generation.as_deref().and_then(|s| s.parse::<i64>().ok());
        Self {
            name: w.name,
            bucket: w.bucket,
            size,
            content_type: w.content_type,
            etag: w.etag,
            generation,
            updated: w.updated,
            md5_hash: w.md5_hash,
            metadata: w.metadata,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListPageWire {
    #[serde(default)]
    items: Vec<ObjectMetadataWire>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GcsErrorEnvelope {
    error: GcsErrorBody,
}

#[derive(Debug, Deserialize)]
struct GcsErrorBody {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

pub async fn get_gcs_client(
    sa_key: Option<ServiceAccountKey>,
    sa_path: Option<String>,
    config: impl Into<Option<GcsConfig>>,
) -> Result<GcsClient, AppError> {
    let resolved = resolve_gcs_config(config.into().unwrap_or_default());
    let gcp = GcpClient::new(sa_key, sa_path, [GCS_SCOPE]).await?;
    Ok(GcsClient::new(
        gcp,
        resolved.base_url,
        Duration::from_secs(resolved.default_expiry_secs),
    ))
}

#[derive(Debug)]
struct ResolvedGcsConfig {
    base_url: Option<String>,
    default_expiry_secs: u64,
}

fn resolve_gcs_config(config: GcsConfig) -> ResolvedGcsConfig {
    let base_url = resolve_optional_string(config.gcs_base_url, "GCS_BASE_URL");
    let default_expiry_secs = resolve_optional::<u64>(
        config.signed_url_default_expiry_secs,
        "GCS_SIGNED_URL_EXPIRY_SECS",
    )
    .unwrap_or(DEFAULT_SIGNED_EXPIRY_SECS);
    ResolvedGcsConfig {
        base_url,
        default_expiry_secs,
    }
}

/// Client for the Cloud Storage JSON API.
///
/// `upload` and `download` buffer the full object into memory. For large
/// objects prefer dedicated streaming machinery (not provided here).
#[derive(Clone)]
pub struct GcsClient {
    gcp: GcpClient,
    base_url: String,
    default_expiry: Duration,
}

impl GcsClient {
    pub fn new(gcp: GcpClient, base_url: Option<String>, default_expiry: Duration) -> Self {
        let base_url = base_url
            .unwrap_or_else(|| DEFAULT_GCS_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        Self {
            gcp,
            base_url,
            default_expiry,
        }
    }

    /// Upload `bytes` to `bucket/object`. Optional `metadata` becomes user-defined
    /// metadata on the object (the GCS `metadata` field).
    ///
    /// Pass `Some(generation)` as `if_generation_match` to make the write
    /// conditional on the object's current generation (compare-and-swap);
    /// `Some(0)` means "only if the object does not exist". On mismatch GCS
    /// returns HTTP 412, surfaced as a dependency error containing `412`.
    pub async fn upload(
        &self,
        bucket: &str,
        object: &str,
        bytes: Vec<u8>,
        content_type: &str,
        metadata: impl Into<Option<HashMap<String, String>>>,
        if_generation_match: impl Into<Option<i64>>,
    ) -> Result<ObjectMetadata, AppError> {
        let metadata = metadata.into();
        let url = build_upload_url(&self.base_url, bucket, if_generation_match.into());

        let boundary = format!("arche-gcs-{}", nanoid::nanoid!());
        let body = build_multipart_body(object, &bytes, content_type, metadata.as_ref(), &boundary);
        let content_type_header = format!("multipart/related; boundary={boundary}");

        let resp = self
            .gcp
            .post(&url)
            .await?
            .header(reqwest::header::CONTENT_TYPE, content_type_header)
            .body(body)
            .send()
            .await
            .map_err(|e| {
                AppError::dependency_failed("gcp-gcs", format!("upload request failed: {e}"))
            })?;

        let wire: ObjectMetadataWire = handle_json(resp, "upload").await?;
        Ok(wire.into())
    }

    /// Merge `metadata` into the object's user-defined metadata. Keys present
    /// are added/overwritten; absent keys are untouched. Per-key delete (via
    /// `null` values) is not supported.
    pub async fn patch_metadata(
        &self,
        bucket: &str,
        object: &str,
        metadata: HashMap<String, String>,
    ) -> Result<ObjectMetadata, AppError> {
        let url = format!(
            "{}/storage/v1/b/{}/o/{}",
            self.base_url,
            utf8_percent_encode(bucket, JSON_API_OBJECT),
            utf8_percent_encode(object, JSON_API_OBJECT),
        );

        let body = json!({ "metadata": metadata });

        let resp = self
            .gcp
            .patch(&url)
            .await?
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                AppError::dependency_failed(
                    "gcp-gcs",
                    format!("patch_metadata request failed: {e}"),
                )
            })?;

        let wire: ObjectMetadataWire = handle_json(resp, "patch_metadata").await?;
        Ok(wire.into())
    }

    /// Download `bucket/object`. Pass `Some(generation)` to read a specific
    /// non-current version (requires Object Versioning on the bucket).
    pub async fn download(
        &self,
        bucket: &str,
        object: &str,
        generation: impl Into<Option<i64>>,
    ) -> Result<Vec<u8>, AppError> {
        let mut url = format!(
            "{}/storage/v1/b/{}/o/{}?alt=media",
            self.base_url,
            utf8_percent_encode(bucket, JSON_API_OBJECT),
            utf8_percent_encode(object, JSON_API_OBJECT),
        );
        if let Some(g) = generation.into() {
            url.push_str(&format!("&generation={g}"));
        }

        let resp = self.gcp.get(&url).await?.send().await.map_err(|e| {
            AppError::dependency_failed("gcp-gcs", format!("download request failed: {e}"))
        })?;

        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| {
            AppError::dependency_failed("gcp-gcs", format!("download body read failed: {e}"))
        })?;

        if !status.is_success() {
            return Err(AppError::dependency_failed(
                "gcp-gcs",
                format!(
                    "download returned HTTP {status}: {}",
                    parse_gcs_error(&bytes)
                ),
            ));
        }

        Ok(bytes.to_vec())
    }

    /// Delete `bucket/object`. Pass `Some(generation)` to delete a specific
    /// non-current version; otherwise the live version is deleted (and, on
    /// versioned buckets, becomes the newest non-current version).
    pub async fn delete(
        &self,
        bucket: &str,
        object: &str,
        generation: impl Into<Option<i64>>,
    ) -> Result<(), AppError> {
        let mut url = format!(
            "{}/storage/v1/b/{}/o/{}",
            self.base_url,
            utf8_percent_encode(bucket, JSON_API_OBJECT),
            utf8_percent_encode(object, JSON_API_OBJECT),
        );
        if let Some(g) = generation.into() {
            url.push_str(&format!("?generation={g}"));
        }

        let resp = self.gcp.delete(&url).await?.send().await.map_err(|e| {
            AppError::dependency_failed("gcp-gcs", format!("delete request failed: {e}"))
        })?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }

        let bytes = resp.bytes().await.unwrap_or_default();
        Err(AppError::dependency_failed(
            "gcp-gcs",
            format!("delete returned HTTP {status}: {}", parse_gcs_error(&bytes)),
        ))
    }

    /// Fetch object metadata. Pass `Some(generation)` to inspect a specific
    /// non-current version.
    pub async fn head(
        &self,
        bucket: &str,
        object: &str,
        generation: impl Into<Option<i64>>,
    ) -> Result<ObjectMetadata, AppError> {
        let mut url = format!(
            "{}/storage/v1/b/{}/o/{}",
            self.base_url,
            utf8_percent_encode(bucket, JSON_API_OBJECT),
            utf8_percent_encode(object, JSON_API_OBJECT),
        );
        if let Some(g) = generation.into() {
            url.push_str(&format!("?generation={g}"));
        }

        let resp = self.gcp.get(&url).await?.send().await.map_err(|e| {
            AppError::dependency_failed("gcp-gcs", format!("head request failed: {e}"))
        })?;

        let wire: ObjectMetadataWire = handle_json(resp, "head").await?;
        Ok(wire.into())
    }

    /// List objects in `bucket`. Pass `versions: true` to include non-current
    /// versions (each generation appears as a separate `ObjectMetadata` entry,
    /// distinguished by its `generation` field).
    pub async fn list(
        &self,
        bucket: &str,
        prefix: Option<&str>,
        page_token: Option<&str>,
        versions: bool,
    ) -> Result<ListPage, AppError> {
        let mut url = format!(
            "{}/storage/v1/b/{}/o",
            self.base_url,
            utf8_percent_encode(bucket, JSON_API_OBJECT),
        );
        let mut sep = '?';
        if let Some(prefix) = prefix {
            url.push(sep);
            url.push_str("prefix=");
            url.push_str(&utf8_percent_encode(prefix, QUERY_VALUE).to_string());
            sep = '&';
        }
        if let Some(token) = page_token {
            url.push(sep);
            url.push_str("pageToken=");
            url.push_str(&utf8_percent_encode(token, QUERY_VALUE).to_string());
            sep = '&';
        }
        if versions {
            url.push(sep);
            url.push_str("versions=true");
        }

        let resp = self.gcp.get(&url).await?.send().await.map_err(|e| {
            AppError::dependency_failed("gcp-gcs", format!("list request failed: {e}"))
        })?;

        let wire: ListPageWire = handle_json(resp, "list").await?;
        Ok(ListPage {
            items: wire.items.into_iter().map(Into::into).collect(),
            next_page_token: wire.next_page_token,
        })
    }

    /// Generate a V4-signed GET URL for `bucket/object`, valid for `expiry`
    /// (defaults to the client's configured default, capped at 7 days).
    ///
    /// The returned URL always points at `storage.googleapis.com` regardless of
    /// any `GCS_BASE_URL` override — V4 signing is only meaningful against the
    /// real GCS endpoint, and emulators do not validate the signature.
    pub fn signed_get_url(
        &self,
        bucket: &str,
        object: &str,
        expiry: Option<Duration>,
    ) -> Result<String, AppError> {
        self.signed_get_url_at(bucket, object, expiry, OffsetDateTime::now_utc())
    }

    fn signed_get_url_at(
        &self,
        bucket: &str,
        object: &str,
        expiry: Option<Duration>,
        now: OffsetDateTime,
    ) -> Result<String, AppError> {
        let now = now.to_offset(UtcOffset::UTC);
        let expiry = expiry.unwrap_or(self.default_expiry);
        let expiry_secs = expiry.as_secs();
        if expiry_secs == 0 || expiry_secs > MAX_SIGNED_EXPIRY_SECS {
            return Err(AppError::bad_request(
                None,
                Some(format!(
                    "GCS signed URL expiry must be 1..={MAX_SIGNED_EXPIRY_SECS} seconds, got {expiry_secs}"
                )),
                None,
            ));
        }

        let request_ts = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
        );
        let datestamp = format!(
            "{:04}{:02}{:02}",
            now.year(),
            u8::from(now.month()),
            now.day(),
        );

        let credential_scope = format!("{datestamp}/auto/storage/goog4_request");
        let credential = format!("{}/{credential_scope}", self.gcp.signer_email()?);
        let encoded_credential = utf8_percent_encode(&credential, QUERY_VALUE).to_string();

        let encoded_bucket = utf8_percent_encode(bucket, SIGNING_PATH).to_string();
        let encoded_object = utf8_percent_encode(object, SIGNING_PATH).to_string();
        let path = format!("/{encoded_bucket}/{encoded_object}");

        // Keys are alphabetically pre-sorted: Algorithm < Credential < Date < Expires < SignedHeaders.
        let canonical_query = format!(
            "X-Goog-Algorithm=GOOG4-RSA-SHA256&X-Goog-Credential={encoded_credential}&X-Goog-Date={request_ts}&X-Goog-Expires={expiry_secs}&X-Goog-SignedHeaders=host"
        );

        let canonical_request = format!(
            "GET\n{path}\n{canonical_query}\nhost:{SIGNED_URL_HOST}\n\nhost\nUNSIGNED-PAYLOAD"
        );

        let hashed = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let string_to_sign =
            format!("GOOG4-RSA-SHA256\n{request_ts}\n{credential_scope}\n{hashed}");

        let signature = self.gcp.sign_blob(string_to_sign.as_bytes())?;
        let hex_sig = hex::encode(signature);

        Ok(format!(
            "https://{SIGNED_URL_HOST}{path}?{canonical_query}&X-Goog-Signature={hex_sig}"
        ))
    }
}

async fn handle_json<T: for<'de> Deserialize<'de>>(
    resp: reqwest::Response,
    op: &str,
) -> Result<T, AppError> {
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| {
        AppError::dependency_failed("gcp-gcs", format!("failed reading {op} response body: {e}"))
    })?;

    if !status.is_success() {
        return Err(AppError::dependency_failed(
            "gcp-gcs",
            format!("{op} returned HTTP {status}: {}", parse_gcs_error(&bytes)),
        ));
    }

    serde_json::from_slice::<T>(&bytes).map_err(|e| {
        AppError::dependency_failed("gcp-gcs", format!("failed to parse {op} response: {e}"))
    })
}

fn build_upload_url(base_url: &str, bucket: &str, if_generation_match: Option<i64>) -> String {
    let mut url = format!(
        "{}/upload/storage/v1/b/{}/o?uploadType=multipart",
        base_url,
        utf8_percent_encode(bucket, JSON_API_OBJECT),
    );
    if let Some(g) = if_generation_match {
        url.push_str(&format!("&ifGenerationMatch={g}"));
    }
    url
}

fn build_multipart_body(
    object: &str,
    bytes: &[u8],
    content_type: &str,
    metadata: Option<&HashMap<String, String>>,
    boundary: &str,
) -> Vec<u8> {
    let mut json_body = json!({ "name": object });
    if let Some(m) = metadata
        && !m.is_empty()
    {
        json_body["metadata"] = serde_json::to_value(m).unwrap_or(serde_json::Value::Null);
    }
    let json_str = json_body.to_string();

    let mut body = Vec::with_capacity(bytes.len() + json_str.len() + 256);
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(json_str.as_bytes());
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\nContent-Type: ");
    body.extend_from_slice(content_type.as_bytes());
    body.extend_from_slice(b"\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");
    body
}

fn parse_gcs_error(body: &[u8]) -> String {
    serde_json::from_slice::<GcsErrorEnvelope>(body)
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
    use crate::gcp::token::ServiceAccountKey;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    const ENV_VARS: &[&str] = &["GCS_BASE_URL", "GCS_SIGNED_URL_EXPIRY_SECS"];

    fn clear_gcs_env() {
        for k in ENV_VARS {
            unsafe { std::env::remove_var(k) };
        }
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

    fn test_service_account() -> ServiceAccountKey {
        ServiceAccountKey::new("test@example.iam.gserviceaccount.com", TEST_PRIVATE_KEY)
    }

    async fn test_gcp_client() -> GcpClient {
        GcpClient::new(Some(test_service_account()), None, [GCS_SCOPE])
            .await
            .expect("test gcp client")
    }

    #[test]
    fn resolve_gcs_config_uses_defaults_when_unset() {
        let _g = env_guard();
        clear_gcs_env();
        let resolved = resolve_gcs_config(GcsConfig::default());
        clear_gcs_env();
        assert!(resolved.base_url.is_none());
        assert_eq!(resolved.default_expiry_secs, DEFAULT_SIGNED_EXPIRY_SECS);
    }

    #[test]
    fn resolve_gcs_config_reads_from_env_when_unset() {
        let _g = env_guard();
        clear_gcs_env();
        unsafe {
            std::env::set_var("GCS_BASE_URL", "https://env.test");
            std::env::set_var("GCS_SIGNED_URL_EXPIRY_SECS", "1800");
        }
        let resolved = resolve_gcs_config(GcsConfig::default());
        clear_gcs_env();
        assert_eq!(resolved.base_url.as_deref(), Some("https://env.test"));
        assert_eq!(resolved.default_expiry_secs, 1800);
    }

    #[test]
    fn resolve_gcs_config_explicit_overrides_env() {
        let _g = env_guard();
        clear_gcs_env();
        unsafe {
            std::env::set_var("GCS_BASE_URL", "https://env.test");
            std::env::set_var("GCS_SIGNED_URL_EXPIRY_SECS", "1800");
        }
        let resolved = resolve_gcs_config(
            GcsConfig::builder()
                .gcs_base_url("https://explicit.test")
                .signed_url_default_expiry_secs(60)
                .build(),
        );
        clear_gcs_env();
        assert_eq!(resolved.base_url.as_deref(), Some("https://explicit.test"));
        assert_eq!(resolved.default_expiry_secs, 60);
    }

    #[test]
    fn parse_gcs_error_extracts_status_and_message() {
        let body = br#"{"error":{"code":404,"status":"NOT_FOUND","message":"No such object"}}"#;
        assert_eq!(parse_gcs_error(body), "NOT_FOUND: No such object");
    }

    #[test]
    fn parse_gcs_error_falls_back_to_raw_body() {
        assert_eq!(parse_gcs_error(b"not json"), "not json");
    }

    #[tokio::test]
    async fn new_strips_trailing_slash_from_base_url() {
        let gcp = test_gcp_client().await;
        let client = GcsClient::new(
            gcp,
            Some("https://storage.googleapis.com/".to_string()),
            Duration::from_secs(60),
        );
        assert_eq!(client.base_url, "https://storage.googleapis.com");
    }

    #[tokio::test]
    async fn signed_get_url_rejects_zero_expiry() {
        let gcp = test_gcp_client().await;
        let client = GcsClient::new(gcp, None, Duration::from_secs(60));
        let err = client
            .signed_get_url("bkt", "obj", Some(Duration::from_secs(0)))
            .unwrap_err();
        assert!(format!("{err:?}").contains("expiry"));
    }

    #[tokio::test]
    async fn signed_get_url_rejects_expiry_over_seven_days() {
        let gcp = test_gcp_client().await;
        let client = GcsClient::new(gcp, None, Duration::from_secs(60));
        let err = client
            .signed_get_url(
                "bkt",
                "obj",
                Some(Duration::from_secs(MAX_SIGNED_EXPIRY_SECS + 1)),
            )
            .unwrap_err();
        assert!(format!("{err:?}").contains("expiry"));
    }

    #[tokio::test]
    async fn signed_get_url_shape_is_stable() {
        let gcp = test_gcp_client().await;
        let client = GcsClient::new(gcp, None, Duration::from_secs(900));
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let url = client
            .signed_get_url_at(
                "my-bucket",
                "path/to/file.txt",
                Some(Duration::from_secs(900)),
                now,
            )
            .expect("signed URL");

        assert!(url.starts_with("https://storage.googleapis.com/my-bucket/path/to/file.txt?"));
        assert!(url.contains("X-Goog-Algorithm=GOOG4-RSA-SHA256"));
        assert!(url.contains("X-Goog-SignedHeaders=host"));
        assert!(url.contains("X-Goog-Expires=900"));
        assert!(url.contains("X-Goog-Date=20231114T221320Z"));
        assert!(url.contains("X-Goog-Credential=test%40example.iam.gserviceaccount.com%2F20231114%2Fauto%2Fstorage%2Fgoog4_request"));
        assert!(url.contains("&X-Goog-Signature="));
    }

    #[tokio::test]
    async fn signed_get_url_signature_is_deterministic() {
        let gcp = test_gcp_client().await;
        let client = GcsClient::new(gcp, None, Duration::from_secs(900));
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();

        let url_a = client
            .signed_get_url_at("bkt", "obj", Some(Duration::from_secs(900)), now)
            .unwrap();
        let url_b = client
            .signed_get_url_at("bkt", "obj", Some(Duration::from_secs(900)), now)
            .unwrap();
        assert_eq!(url_a, url_b, "RSA-PKCS1v15 should be deterministic");
    }

    #[test]
    fn build_upload_url_without_precondition_has_no_generation_param() {
        let url = build_upload_url("https://storage.googleapis.com", "my-bucket", None);
        assert_eq!(
            url,
            "https://storage.googleapis.com/upload/storage/v1/b/my-bucket/o?uploadType=multipart"
        );
    }

    #[test]
    fn build_upload_url_with_generation_appends_precondition() {
        let url = build_upload_url("https://storage.googleapis.com", "my-bucket", Some(1234));
        assert_eq!(
            url,
            "https://storage.googleapis.com/upload/storage/v1/b/my-bucket/o?uploadType=multipart&ifGenerationMatch=1234"
        );
    }

    #[test]
    fn build_upload_url_with_zero_generation_means_create_only() {
        let url = build_upload_url("https://storage.googleapis.com", "my-bucket", Some(0));
        assert!(url.ends_with("&ifGenerationMatch=0"));
    }

    #[test]
    fn build_multipart_body_without_metadata_includes_only_name() {
        let body = build_multipart_body("path/to/file.txt", b"hello", "text/plain", None, "BNDRY");
        let text = String::from_utf8(body).expect("ascii body");
        assert!(
            text.starts_with("--BNDRY\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n")
        );
        assert!(text.contains(r#""name":"path/to/file.txt""#));
        assert!(!text.contains("\"metadata\""));
        assert!(
            text.contains("\r\n--BNDRY\r\nContent-Type: text/plain\r\n\r\nhello\r\n--BNDRY--\r\n")
        );
    }

    #[test]
    fn build_multipart_body_with_metadata_serializes_user_keys() {
        let mut meta = HashMap::new();
        meta.insert("modified_by".to_string(), "alice".to_string());
        let body = build_multipart_body(
            "f.bin",
            b"\x00\x01\x02",
            "application/octet-stream",
            Some(&meta),
            "X",
        );
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains(r#""metadata":{"modified_by":"alice"}"#));
        assert!(text.contains(r#""name":"f.bin""#));
    }

    #[test]
    fn object_metadata_wire_deserializes_custom_metadata() {
        let json = r#"{
            "name": "foo.txt",
            "bucket": "b",
            "size": "42",
            "generation": "1234",
            "metadata": {"modified_by": "alice", "uploaded_by": "bob"}
        }"#;
        let wire: ObjectMetadataWire = serde_json::from_str(json).expect("parse");
        let meta: ObjectMetadata = wire.into();
        assert_eq!(meta.size, 42);
        assert_eq!(meta.generation, Some(1234));
        assert_eq!(
            meta.metadata.get("modified_by").map(String::as_str),
            Some("alice")
        );
        assert_eq!(
            meta.metadata.get("uploaded_by").map(String::as_str),
            Some("bob")
        );
    }

    #[test]
    fn object_metadata_wire_defaults_metadata_to_empty_when_absent() {
        let json = r#"{"name":"f","bucket":"b"}"#;
        let wire: ObjectMetadataWire = serde_json::from_str(json).expect("parse");
        let meta: ObjectMetadata = wire.into();
        assert!(meta.metadata.is_empty());
    }

    #[tokio::test]
    async fn signed_get_url_encodes_path_but_preserves_slashes() {
        let gcp = test_gcp_client().await;
        let client = GcsClient::new(gcp, None, Duration::from_secs(60));
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let url = client
            .signed_get_url_at("bkt", "a b/c.txt", Some(Duration::from_secs(60)), now)
            .unwrap();
        assert!(url.contains("/bkt/a%20b/c.txt?"));
    }
}
