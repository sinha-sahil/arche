use std::future::Future;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rsa::RsaPrivateKey;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::traits::PublicKeyParts;

use crate::error::AppError;

pub trait TokenSigner: Send + Sync + 'static {
    fn kid(&self) -> &str;

    fn sign(&self, message: &[u8]) -> impl Future<Output = Result<Vec<u8>, AppError>> + Send;

    fn jwks(&self) -> serde_json::Value;
}

pub fn rsa_public_jwk(kid: &str, public_key_pem: &str) -> Result<serde_json::Value, AppError> {
    use rsa::RsaPublicKey;
    use rsa::pkcs1::DecodeRsaPublicKey;
    use rsa::pkcs8::DecodePublicKey;

    let key = RsaPublicKey::from_public_key_pem(public_key_pem)
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(public_key_pem))
        .map_err(|e| AppError::internal_error(format!("Invalid RSA public key PEM: {e}"), None))?;
    Ok(jwk_from_parts(
        kid,
        &key.n().to_bytes_be(),
        &key.e().to_bytes_be(),
    ))
}

fn jwk_from_parts(kid: &str, n: &[u8], e: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "kty": "RSA",
        "alg": "RS256",
        "use": "sig",
        "kid": kid,
        "n": URL_SAFE_NO_PAD.encode(n),
        "e": URL_SAFE_NO_PAD.encode(e),
    })
}

#[derive(Clone)]
pub struct SigningKey {
    pub kid: String,
    encoding: jsonwebtoken::EncodingKey,
    jwk: serde_json::Value,
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigningKey")
            .field("kid", &self.kid)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl SigningKey {
    pub fn from_pem(kid: impl Into<String>, pem: &str) -> Result<Self, AppError> {
        let kid = kid.into();
        if kid.is_empty() {
            return Err(AppError::internal_error(
                "SigningKey kid must not be empty".into(),
                None,
            ));
        }

        let private = RsaPrivateKey::from_pkcs8_pem(pem)
            .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))
            .map_err(|e| {
                AppError::internal_error(format!("Invalid RSA private key PEM: {e}"), None)
            })?;
        if private.n().bits() < 2048 {
            return Err(AppError::internal_error(
                format!(
                    "RSA signing key must be at least 2048 bits, got {}",
                    private.n().bits()
                ),
                None,
            ));
        }

        let encoding = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).map_err(|e| {
            AppError::internal_error(format!("Invalid RSA private key PEM: {e}"), None)
        })?;

        let jwk = jwk_from_parts(&kid, &private.n().to_bytes_be(), &private.e().to_bytes_be());
        Ok(Self { kid, encoding, jwk })
    }
}

impl TokenSigner for SigningKey {
    fn kid(&self) -> &str {
        &self.kid
    }

    async fn sign(&self, message: &[u8]) -> Result<Vec<u8>, AppError> {
        let b64 =
            jsonwebtoken::crypto::sign(message, &self.encoding, jsonwebtoken::Algorithm::RS256)
                .map_err(|e| AppError::internal_error(format!("RSA signing failed: {e}"), None))?;
        URL_SAFE_NO_PAD
            .decode(b64)
            .map_err(|e| AppError::internal_error(format!("RSA signature decode: {e}"), None))
    }

    fn jwks(&self) -> serde_json::Value {
        serde_json::json!({ "keys": [self.jwk] })
    }
}
