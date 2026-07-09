use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::de::DeserializeOwned;

use crate::error::AppError;

use super::jwks::JwksCache;
use super::{ProviderMetadata, default_http_client, dependency_label, reject_token};

const CLOCK_SKEW_SECS: u64 = 60;

#[derive(Clone)]
pub struct Verifier {
    http: reqwest::Client,
    keys: JwksCache,
    issuers: Vec<String>,
    dependency: String,
}

impl Verifier {
    pub fn new(provider: &ProviderMetadata) -> Result<Self, AppError> {
        let http = default_http_client("OIDC verifier")?;
        Ok(Self::with_http_client(provider, http))
    }

    pub fn with_http_client(provider: &ProviderMetadata, http: reqwest::Client) -> Self {
        let dependency = dependency_label(&provider.key);
        Self {
            http,
            keys: JwksCache::new(provider.jwks_endpoint.clone(), dependency.clone()),
            issuers: provider.issuers.clone(),
            dependency,
        }
    }

    pub async fn verify_id_token<C: DeserializeOwned>(
        &self,
        id_token: &str,
        audiences: &[&str],
    ) -> Result<C, AppError> {
        let claims = self.validated_claims(id_token, audiences).await?;
        serde_json::from_value(claims).map_err(|e| {
            AppError::internal_error(
                format!("verified id token does not fit requested type: {e}"),
                None,
            )
        })
    }

    async fn validated_claims(
        &self,
        id_token: &str,
        audiences: &[&str],
    ) -> Result<serde_json::Value, AppError> {
        let header =
            decode_header(id_token).map_err(|e| reject_token(format!("header decode: {e}")))?;
        if header.alg != Algorithm::RS256 {
            return Err(reject_token(format!(
                "unsupported algorithm {:?}",
                header.alg
            )));
        }
        let kid = header.kid.ok_or_else(|| reject_token("missing kid"))?;

        let jwk = self.keys.lookup(&self.http, &kid).await?;

        let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|e| {
            AppError::dependency_failed_permanent(&self.dependency, format!("jwk components: {e}"))
        })?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud"]);
        validation.set_issuer(&self.issuers);
        validation.set_audience(audiences);
        validation.leeway = CLOCK_SKEW_SECS;
        validation.validate_nbf = true;

        let token_data = decode::<serde_json::Value>(id_token, &decoding_key, &validation)
            .map_err(|e| reject_token(format!("validation failed: {e}")))?;

        Ok(token_data.claims)
    }
}
