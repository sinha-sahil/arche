use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;

use crate::error::AppError;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

fn make_key(password: &[u8], salt: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2_hmac::<Sha1>(password, salt, 65536, &mut key);
    key
}

fn gen_iv(salt: &[u8]) -> [u8; 16] {
    let mut iv = [0u8; 16];
    let len = std::cmp::min(salt.len(), 16);
    iv[..len].copy_from_slice(&salt[..len]);
    iv
}

pub fn encrypt_cbc(secret: &str, salt: &str, plaintext: &str) -> Result<Vec<u8>, AppError> {
    let key = make_key(secret.as_bytes(), salt.as_bytes());
    let iv = gen_iv(salt.as_bytes());

    let cipher = Aes128CbcEnc::new(&key.into(), &iv.into());
    let ciphertext =
        cipher.encrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(plaintext.as_bytes());

    Ok(ciphertext)
}

pub fn decrypt_cbc(secret: &str, salt: &str, ciphertext_b64: &str) -> Result<Vec<u8>, AppError> {
    let ciphertext = BASE64
        .decode(ciphertext_b64)
        .map_err(|e| AppError::internal_error(e.to_string(), None))?;

    let key = make_key(secret.as_bytes(), salt.as_bytes());
    let iv = gen_iv(salt.as_bytes());

    let cipher = Aes128CbcDec::new(&key.into(), &iv.into());
    let plaintext = cipher
        .decrypt_padded_vec_mut::<cbc::cipher::block_padding::Pkcs7>(&ciphertext)
        .map_err(|e| AppError::internal_error(e.to_string(), None))?;

    Ok(plaintext)
}
