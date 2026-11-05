//! Encryption for Printer credentials (ADR-0005).
//!
//! The operator's secret key is stretched with HKDF-SHA256 into a 256-bit key
//! for XChaCha20-Poly1305. Each value gets a fresh random nonce. The stored
//! form is `v1:` followed by base64 of the nonce and the ciphertext, so the
//! scheme can change later without guessing.
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;

const PREFIX: &str = "v1:";
const NONCE_LEN: usize = 24;

/// The key that encrypts Printer credentials. Never logged or displayed.
#[derive(Clone)]
pub struct SecretKey {
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)")
    }
}

/// A stored value could not be decrypted: it was encrypted with another key,
/// or it has been altered.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the value could not be decrypted with this secret key")]
pub struct DecryptError;

impl SecretKey {
    /// The shortest secret accepted.
    pub const MIN_LEN: usize = 16;

    /// Derives the key from the configured secret. `None` if it is too short.
    pub fn from_config(secret: &str) -> Option<Self> {
        let secret = secret.trim();
        if secret.chars().count() < Self::MIN_LEN {
            return None;
        }
        let hk = Hkdf::<Sha256>::new(Some(b"gantry"), secret.as_bytes());
        let mut key = [0u8; 32];
        hk.expand(b"gantry printer credentials v1", &mut key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Some(Self {
            cipher: XChaCha20Poly1305::new(&key.into()),
        })
    }

    /// Encrypts `plaintext` for storage.
    pub fn encrypt(&self, plaintext: &str) -> String {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("XChaCha20-Poly1305 encryption cannot fail for in-memory input");
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        format!("{PREFIX}{}", B64.encode(blob))
    }

    /// Decrypts a value produced by [`encrypt`](Self::encrypt).
    pub fn decrypt(&self, stored: &str) -> Result<String, DecryptError> {
        let blob = stored
            .strip_prefix(PREFIX)
            .and_then(|b| B64.decode(b).ok())
            .filter(|b| b.len() > NONCE_LEN)
            .ok_or(DecryptError)?;
        let (nonce, ciphertext) = blob.split_at(NONCE_LEN);
        let plaintext = self
            .cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| DecryptError)?;
        String::from_utf8(plaintext).map_err(|_| DecryptError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(secret: &str) -> SecretKey {
        SecretKey::from_config(secret).unwrap()
    }

    #[test]
    fn round_trips() {
        let k = key("correct horse battery staple");
        let stored = k.encrypt("12345678");
        assert_eq!(k.decrypt(&stored).unwrap(), "12345678");
    }

    #[test]
    fn the_ciphertext_does_not_contain_the_plaintext() {
        let k = key("correct horse battery staple");
        let stored = k.encrypt("an-access-code");
        assert!(!stored.contains("an-access-code"));
        let raw = B64.decode(stored.strip_prefix(PREFIX).unwrap()).unwrap();
        assert!(!raw.windows(14).any(|w| w == b"an-access-code"));
    }

    #[test]
    fn another_key_cannot_read_it() {
        let stored = key("correct horse battery staple").encrypt("12345678");
        assert_eq!(
            key("a different secret key!").decrypt(&stored),
            Err(DecryptError)
        );
    }

    #[test]
    fn each_encryption_uses_a_fresh_nonce() {
        let k = key("correct horse battery staple");
        assert_ne!(k.encrypt("same"), k.encrypt("same"));
    }

    #[test]
    fn tampering_is_detected() {
        let k = key("correct horse battery staple");
        let stored = k.encrypt("12345678");
        let mut raw = B64.decode(stored.strip_prefix(PREFIX).unwrap()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert_eq!(
            k.decrypt(&format!("{PREFIX}{}", B64.encode(raw))),
            Err(DecryptError)
        );
        assert_eq!(k.decrypt("not encrypted at all"), Err(DecryptError));
    }

    #[test]
    fn short_secrets_are_refused() {
        assert!(SecretKey::from_config("too short").is_none());
    }
}
