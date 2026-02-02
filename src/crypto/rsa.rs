use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rand::rngs::OsRng;
use rsa::{
    Oaep, RsaPrivateKey, RsaPublicKey,
    pkcs1::{DecodeRsaPrivateKey, DecodeRsaPublicKey},
    pkcs1v15::{Signature, SigningKey, VerifyingKey},
    pkcs8::{DecodePrivateKey, DecodePublicKey},
    signature::{SignatureEncoding, Signer, Verifier},
};
use sha1::Sha1;
use sha2::Sha256;

use crate::error::AppError;

pub fn parse_public_key(pem: &str) -> Result<RsaPublicKey, AppError> {
    RsaPublicKey::from_public_key_pem(pem)
        .or_else(|_| RsaPublicKey::from_pkcs1_pem(pem))
        .map_err(|e| AppError::internal_error(e.to_string(), None))
}

pub fn parse_private_key(pem: &str) -> Result<RsaPrivateKey, AppError> {
    RsaPrivateKey::from_pkcs8_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))
        .map_err(|e| AppError::internal_error(e.to_string(), None))
}

pub fn encrypt(public_key: &RsaPublicKey, plaintext: &str) -> Result<String, AppError> {
    let padding = Oaep::new::<Sha1>();
    let ciphertext = public_key
        .encrypt(&mut OsRng, padding, plaintext.as_bytes())
        .map_err(|e| AppError::internal_error(e.to_string(), None))?;
    Ok(BASE64.encode(ciphertext))
}

pub fn decrypt(private_key: &RsaPrivateKey, ciphertext_b64: &str) -> Result<String, AppError> {
    let ciphertext = BASE64
        .decode(ciphertext_b64)
        .map_err(|e| AppError::internal_error(e.to_string(), None))?;
    let padding = Oaep::new::<Sha1>();
    let plaintext = private_key
        .decrypt(padding, &ciphertext)
        .map_err(|e| AppError::internal_error(e.to_string(), None))?;
    String::from_utf8(plaintext).map_err(|e| AppError::internal_error(e.to_string(), None))
}

pub fn sign(private_key: &RsaPrivateKey, payload: &str) -> Result<String, AppError> {
    let signing_key = SigningKey::<Sha256>::new(private_key.clone());
    let signature = signing_key.sign(payload.as_bytes());
    Ok(BASE64.encode(signature.to_bytes()))
}

pub fn verify(
    public_key: &RsaPublicKey,
    payload: &str,
    signature_b64: &str,
) -> Result<bool, AppError> {
    let signature_bytes = BASE64
        .decode(signature_b64)
        .map_err(|e| AppError::internal_error(e.to_string(), None))?;
    let signature = Signature::try_from(signature_bytes.as_slice())
        .map_err(|e| AppError::internal_error(e.to_string(), None))?;
    let verifying_key = VerifyingKey::<Sha256>::new(public_key.clone());
    Ok(verifying_key.verify(payload.as_bytes(), &signature).is_ok())
}
