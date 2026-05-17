use crate::config::resolve_required_string;
use crate::error::AppError;

#[derive(Debug, Clone, Default)]
pub struct GcpKmsConfig {
    pub project_id: Option<String>,
    pub location: Option<String>,
    pub kms_base_url: Option<String>,
}

impl GcpKmsConfig {
    pub fn builder() -> GcpKmsConfigBuilder {
        GcpKmsConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GcpKmsConfigBuilder {
    project_id: Option<String>,
    location: Option<String>,
    kms_base_url: Option<String>,
}

impl GcpKmsConfigBuilder {
    pub fn project_id(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub fn kms_base_url(mut self, url: impl Into<String>) -> Self {
        self.kms_base_url = Some(url.into());
        self
    }

    pub fn build(self) -> GcpKmsConfig {
        GcpKmsConfig {
            project_id: self.project_id,
            location: self.location,
            kms_base_url: self.kms_base_url,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GcpKmsKey {
    pub key_ring: String,
    pub key_name: String,
}

impl GcpKmsKey {
    pub fn new(key_ring: impl Into<String>, key_name: impl Into<String>) -> Self {
        Self {
            key_ring: key_ring.into(),
            key_name: key_name.into(),
        }
    }

    pub fn from_env() -> Result<Self, AppError> {
        let key_ring = resolve_required_string(None, "GCP_KMS_KEY_RING", "key_ring")?;
        let key_name = resolve_required_string(None, "GCP_KMS_KEY_NAME", "key_name")?;
        Ok(Self { key_ring, key_name })
    }
}

#[derive(Debug, Clone, Default)]
pub struct GcsConfig {
    pub gcs_base_url: Option<String>,
    pub signed_url_default_expiry_secs: Option<u64>,
}

impl GcsConfig {
    pub fn builder() -> GcsConfigBuilder {
        GcsConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GcsConfigBuilder {
    gcs_base_url: Option<String>,
    signed_url_default_expiry_secs: Option<u64>,
}

impl GcsConfigBuilder {
    pub fn gcs_base_url(mut self, url: impl Into<String>) -> Self {
        self.gcs_base_url = Some(url.into());
        self
    }

    pub fn signed_url_default_expiry_secs(mut self, secs: u64) -> Self {
        self.signed_url_default_expiry_secs = Some(secs);
        self
    }

    pub fn build(self) -> GcsConfig {
        GcsConfig {
            gcs_base_url: self.gcs_base_url,
            signed_url_default_expiry_secs: self.signed_url_default_expiry_secs,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GcpCdnConfig {
    pub project_id: Option<String>,
    pub url_map: Option<String>,
    pub compute_base_url: Option<String>,
}

impl GcpCdnConfig {
    pub fn builder() -> GcpCdnConfigBuilder {
        GcpCdnConfigBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GcpCdnConfigBuilder {
    project_id: Option<String>,
    url_map: Option<String>,
    compute_base_url: Option<String>,
}

impl GcpCdnConfigBuilder {
    pub fn project_id(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    pub fn url_map(mut self, url_map: impl Into<String>) -> Self {
        self.url_map = Some(url_map.into());
        self
    }

    pub fn compute_base_url(mut self, url: impl Into<String>) -> Self {
        self.compute_base_url = Some(url.into());
        self
    }

    pub fn build(self) -> GcpCdnConfig {
        GcpCdnConfig {
            project_id: self.project_id,
            url_map: self.url_map,
            compute_base_url: self.compute_base_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_builder_collects_fields() {
        let config = GcpKmsConfig::builder()
            .project_id("p")
            .location("us-east1")
            .kms_base_url("https://example.test")
            .build();

        assert_eq!(config.project_id.as_deref(), Some("p"));
        assert_eq!(config.location.as_deref(), Some("us-east1"));
        assert_eq!(config.kms_base_url.as_deref(), Some("https://example.test"));
    }

    #[test]
    fn config_default_is_all_none() {
        let config = GcpKmsConfig::default();
        assert!(config.project_id.is_none());
        assert!(config.location.is_none());
        assert!(config.kms_base_url.is_none());
    }

    #[test]
    fn key_new_stores_fields() {
        let key = GcpKmsKey::new("ring", "name");
        assert_eq!(key.key_ring, "ring");
        assert_eq!(key.key_name, "name");
    }

    #[test]
    fn gcs_config_builder_collects_fields() {
        let config = GcsConfig::builder()
            .gcs_base_url("https://example.test")
            .signed_url_default_expiry_secs(1234)
            .build();

        assert_eq!(config.gcs_base_url.as_deref(), Some("https://example.test"));
        assert_eq!(config.signed_url_default_expiry_secs, Some(1234));
    }

    #[test]
    fn gcs_config_default_is_all_none() {
        let config = GcsConfig::default();
        assert!(config.gcs_base_url.is_none());
        assert!(config.signed_url_default_expiry_secs.is_none());
    }

    #[test]
    fn cdn_config_builder_collects_fields() {
        let config = GcpCdnConfig::builder()
            .project_id("p")
            .url_map("my-url-map")
            .compute_base_url("https://example.test")
            .build();

        assert_eq!(config.project_id.as_deref(), Some("p"));
        assert_eq!(config.url_map.as_deref(), Some("my-url-map"));
        assert_eq!(
            config.compute_base_url.as_deref(),
            Some("https://example.test")
        );
    }

    #[test]
    fn cdn_config_default_is_all_none() {
        let config = GcpCdnConfig::default();
        assert!(config.project_id.is_none());
        assert!(config.url_map.is_none());
        assert!(config.compute_base_url.is_none());
    }
}
