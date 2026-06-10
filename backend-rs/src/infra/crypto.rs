//! At-rest encryption for secrets (channel API keys). AES-256-GCM via the
//! pure-Rust RustCrypto stack.
//!
//! Tech-debt fix: in the Python backend `cryptography` was an *undeclared*
//! optional dependency, so when missing, secrets were written in plaintext.
//! Here encryption is a first-class, always-present dependency.

use std::path::PathBuf;
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;

pub struct CryptoStore {
    cipher: Aes256Gcm,
}

impl CryptoStore {
    pub fn open(key_path: PathBuf) -> Arc<Self> {
        let key_bytes = load_or_create_key(&key_path);
        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
        Arc::new(CryptoStore {
            cipher: Aes256Gcm::new(key),
        })
    }

    /// Returns `enc:<base64(nonce||ciphertext)>`. Empty input stays empty.
    pub fn encrypt(&self, plaintext: &str) -> String {
        if plaintext.is_empty() {
            return String::new();
        }
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        match self.cipher.encrypt(nonce, plaintext.as_bytes()) {
            Ok(ct) => {
                let mut blob = Vec::with_capacity(12 + ct.len());
                blob.extend_from_slice(&nonce_bytes);
                blob.extend_from_slice(&ct);
                format!("enc:{}", B64.encode(blob))
            }
            // Encryption must not silently fail to plaintext; surface a marker.
            Err(e) => {
                tracing::warn!("crypto: encryption failed: {e}");
                String::new()
            }
        }
    }

    /// Accepts both `enc:...` blobs and legacy plaintext (returned unchanged).
    pub fn decrypt(&self, stored: &str) -> String {
        let Some(b64) = stored.strip_prefix("enc:") else {
            return stored.to_string();
        };
        let Ok(blob) = B64.decode(b64) else {
            tracing::warn!("crypto: stored secret is not valid base64");
            return String::new();
        };
        if blob.len() < 12 {
            tracing::warn!("crypto: stored secret blob too short");
            return String::new();
        }
        let (nonce_bytes, ct) = blob.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        match self.cipher.decrypt(nonce, ct) {
            Ok(pt) => String::from_utf8_lossy(&pt).to_string(),
            Err(_) => {
                tracing::warn!(
                    "crypto: decryption failed (master key changed or data corrupted)"
                );
                String::new()
            }
        }
    }
}

fn load_or_create_key(path: &PathBuf) -> [u8; 32] {
    if let Ok(raw) = std::fs::read_to_string(path) {
        if let Ok(decoded) = B64.decode(raw.trim()) {
            if decoded.len() == 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&decoded);
                return key;
            }
        }
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, B64.encode(key));
    key
}
