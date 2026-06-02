//! AES-256-GCM encryption/decryption module
//!
// author: logic
// date: 2026-05-26

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Encryption/decryption error
#[derive(Debug, Error)]
pub enum CryptoError {
    /// Encryption failed
    #[error("Encryption failed: {0}")]
    Encrypt(String),
    /// Decryption failed
    #[error("Decryption failed: {0}")]
    Decrypt(String),
}

/// Derives a 32-byte key based on user Token
fn derive_key(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let intermediate: [u8; 32] = hasher.finalize().into();
    let mut hasher2 = Sha256::new();
    hasher2.update(&intermediate);
    hasher2.finalize().into()
}

/// Encrypts plaintext using AES-256-GCM, returning Base64-encoded nonce+ciphertext
///
/// Derives the encryption key using the user Token
pub fn encrypt(token: &str, plaintext: &str) -> Result<String, CryptoError> {
    let key = derive_key(token);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| CryptoError::Encrypt(e.to_string()))?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| CryptoError::Encrypt(e.to_string()))?;
    let mut combined = Vec::with_capacity(12 + ciphertext.len());
    combined.extend_from_slice(&nonce);
    combined.extend_from_slice(&ciphertext);
    Ok(BASE64.encode(&combined))
}

/// Decrypts Base64-encoded nonce+ciphertext
///
/// Derives the decryption key using the user Token (must match the one used for encryption)
pub fn decrypt(token: &str, encoded: &str) -> Result<String, CryptoError> {
    let key = derive_key(token);
    let combined = BASE64
        .decode(encoded)
        .map_err(|e| CryptoError::Decrypt(e.to_string()))?;
    if combined.len() < 12 {
        return Err(CryptoError::Decrypt("Data too short".to_string()));
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| CryptoError::Decrypt(e.to_string()))?;
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| CryptoError::Decrypt(e.to_string()))?;
    String::from_utf8(plaintext).map_err(|e| CryptoError::Decrypt(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOKEN: &str = "test_token_for_crypto";

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plaintext = "my_secret_password";
        let encrypted = encrypt(TEST_TOKEN, plaintext).unwrap();
        let decrypted = decrypt(TEST_TOKEN, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_same_token_same_result() {
        let encrypted = encrypt(TEST_TOKEN, "secret").unwrap();
        let decrypted = decrypt(TEST_TOKEN, &encrypted).unwrap();
        assert_eq!(decrypted, "secret");
    }

    #[test]
    fn test_empty_string() {
        let encrypted = encrypt(TEST_TOKEN, "").unwrap();
        let decrypted = decrypt(TEST_TOKEN, &encrypted).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_decrypt_invalid_base64() {
        let result = decrypt(TEST_TOKEN, "!!!not-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_data_too_short() {
        // 8 bytes < 12 bytes (nonce size)
        let short = BASE64.encode(&[0u8; 8]);
        let result = decrypt(TEST_TOKEN, &short);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_wrong_ciphertext() {
        // 12 bytes nonce + 1 byte garbage
        let data = vec![0u8; 13];
        let encoded = BASE64.encode(&data);
        let result = decrypt(TEST_TOKEN, &encoded);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_produces_different_ciphertexts() {
        let plaintext = "same_input";
        let e1 = encrypt(TEST_TOKEN, plaintext).unwrap();
        let e2 = encrypt(TEST_TOKEN, plaintext).unwrap();
        // Different nonces should produce different ciphertexts
        assert_ne!(e1, e2);
        // But both can be decrypted correctly
        assert_eq!(decrypt(TEST_TOKEN, &e1).unwrap(), plaintext);
        assert_eq!(decrypt(TEST_TOKEN, &e2).unwrap(), plaintext);
    }

    #[test]
    fn test_encrypt_unicode() {
        let plaintext = "你好世界🌍";
        let encrypted = encrypt(TEST_TOKEN, plaintext).unwrap();
        let decrypted = decrypt(TEST_TOKEN, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_long_string() {
        let plaintext = "a".repeat(10_000);
        let encrypted = encrypt(TEST_TOKEN, &plaintext).unwrap();
        let decrypted = decrypt(TEST_TOKEN, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_different_tokens_produce_different_keys() {
        let plaintext = "secret_data";
        let encrypted = encrypt("token_alpha", plaintext).unwrap();
        let result = decrypt("token_beta", &encrypted);
        assert!(result.is_err(), "Decryption with different tokens should fail");
    }

    #[test]
    fn test_empty_token_roundtrip() {
        let plaintext = "secret_data";
        let encrypted = encrypt("", plaintext).unwrap();
        let decrypted = decrypt("", &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_derive_key_deterministic() {
        let key1 = derive_key("my_token");
        let key2 = derive_key("my_token");
        assert_eq!(key1, key2, "The same token should derive the same key");
    }

    #[test]
    fn test_derive_key_different_tokens() {
        let key1 = derive_key("token_a");
        let key2 = derive_key("token_b");
        assert_ne!(key1, key2, "Different tokens should derive different keys");
    }

    #[test]
    fn test_decrypt_exact_nonce_size() {
        // Exactly 12 bytes (nonce only, no ciphertext), AES-GCM decryption should fail
        let data = vec![0u8; 12];
        let encoded = BASE64.encode(&data);
        let result = decrypt(TEST_TOKEN, &encoded);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_tampered_ciphertext() {
        let encrypted = encrypt(TEST_TOKEN, "hello").unwrap();
        let mut combined = BASE64.decode(&encrypted).unwrap();
        // Tamper with one byte after the nonce
        combined[12] ^= 0xFF;
        let tampered = BASE64.encode(&combined);
        let result = decrypt(TEST_TOKEN, &tampered);
        assert!(result.is_err(), "Decryption of tampered ciphertext should fail");
    }

    #[test]
    fn test_crypto_error_display_encrypt() {
        let err = CryptoError::Encrypt("something went wrong".to_string());
        assert_eq!(format!("{err}"), "Encryption failed: something went wrong");
    }

    #[test]
    fn test_crypto_error_display_decrypt() {
        let err = CryptoError::Decrypt("bad data".to_string());
        assert_eq!(format!("{err}"), "Decryption failed: bad data");
    }

    #[test]
    fn test_encrypt_with_special_char_token() {
        let token = "tok\0en\nwith\tspecial";
        let plaintext = "secret";
        let encrypted = encrypt(token, plaintext).unwrap();
        let decrypted = decrypt(token, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_whitespace_token() {
        let token = "   ";
        let plaintext = "data";
        let encrypted = encrypt(token, plaintext).unwrap();
        let decrypted = decrypt(token, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_very_long_token() {
        let token = "x".repeat(10_000);
        let plaintext = "short";
        let encrypted = encrypt(&token, plaintext).unwrap();
        let decrypted = decrypt(&token, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
