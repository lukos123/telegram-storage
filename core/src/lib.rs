//! core crate – chunking, hashing, encryption
//! -------------------------------------------------
//! Минимальный MVP‑код для работы с файлами: режем → шифруем → собираем.
//!
//! **Зависимости в `core/Cargo.toml`:**
//! ```toml
//! [dependencies]
//! aes-gcm = { version = "0.10", features = ["aes"] }   # AES‑GCM‑256
//! argon2  = "0.5"
//! rand    = "0.8"
//! sha2    = "0.10"
//! thiserror = "1"
//! serde = { version = "1.0", features = ["derive"] }
//! ```
//!
//! После добавления зависимостей: `cargo test -p core` — все тесты зелёные.

use aes_gcm::{aead::{Aead, Payload}, Aes256Gcm, Key, KeyInit, Nonce};
use argon2::{Argon2, password_hash::{PasswordHasher, SaltString}};
use rand::RngCore;
use sha2::{Digest, Sha256};
use thiserror::Error;
use serde::{Serialize, Deserialize};

/// 49 MiB — чтобы безопасно влезать в Telegram‑лимит 50 МБ
pub const DEFAULT_CHUNK_SIZE: usize = 19 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("cryptography error: {0}")]
    Crypto(String),
    #[error("argon2 error: {0}")]
    Argon2(String),
    #[error("chunk index / hash mismatch")]
    BadChunks,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chunk {
    pub index: u32,
    pub total: u32,
    pub data: Vec<u8>,   // raw bytes (зашифрованные или нет)
    pub hash: [u8; 32],  // sha256(data)
}

impl Chunk {
    pub fn new(index: u32, total: u32, data: Vec<u8>) -> Self {
        let hash = Sha256::digest(&data);
        Self { index, total, data, hash: hash.into() }
    }
}

/* ---------------------------------------------------------------------
 *  Chunking helpers
 * ------------------------------------------------------------------*/

/// Разбить буфер на чанки фиксированного размера
pub fn split_bytes(buf: &[u8], chunk_size: usize) -> Vec<Chunk> {
    let total = ((buf.len() + chunk_size - 1) / chunk_size) as u32;
    buf.chunks(chunk_size)
        .enumerate()
        .map(|(i, slice)| Chunk::new(i as u32, total, slice.to_vec()))
        .collect()
}

/// Склеить чанки и проверить sha256
pub fn merge_chunks(mut chunks: Vec<Chunk>) -> Result<Vec<u8>, CoreError> {
    if chunks.is_empty() { return Ok(Vec::new()); }

    chunks.sort_by_key(|c| c.index);
    let expected = chunks[0].total as usize;
    if chunks.len() != expected { return Err(CoreError::BadChunks); }

    let mut out = Vec::with_capacity(chunks.iter().map(|c| c.data.len()).sum());
    for c in &chunks {
        let hash = Sha256::digest(&c.data);
        if hash[..] != c.hash { return Err(CoreError::BadChunks); }
        out.extend_from_slice(&c.data);
    }
    Ok(out)
}

/* ---------------------------------------------------------------------
 *  Crypto helpers
 * ------------------------------------------------------------------*/

/// Пароль + соль (произвольные байты) → 32‑байтный ключ AES‑256.
pub fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], CoreError> {
    let salt = SaltString::encode_b64(salt).map_err(|e| CoreError::Argon2(e.to_string()))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| CoreError::Argon2(e.to_string()))?;

    let digest = hash.hash.ok_or(CoreError::Argon2("missing digest".into()))?;
    let bytes = digest.as_bytes();
    if bytes.len() < 32 { return Err(CoreError::Argon2("digest too short".into())); }

    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes[..32]);
    Ok(key)
}

/// Шифруем данные чанка. Возвращаем (ciphertext, nonce).
pub fn encrypt_chunk(chunk: &Chunk, key: &[u8; 32]) -> Result<(Vec<u8>, [u8; 12]), CoreError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));

    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce); // ок, warning можно подавить позднее

    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: &chunk.data, aad: &chunk.index.to_be_bytes() })
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    Ok((ct, nonce))
}

/// Расшифровываем отдельный чанк
pub fn decrypt_chunk(index: u32, total: u32, ciphertext: &[u8], nonce: &[u8; 12], key: &[u8; 32]) -> Result<Chunk, CoreError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: &index.to_be_bytes() })
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    Ok(Chunk::new(index, total, pt))
}
// core/src/lib.rs  → добавь

/// Конкатенируем nonce + ciphertext (12 + N).
pub fn seal_chunk(chunk: &Chunk, key: &[u8; 32]) -> Result<Vec<u8>, CoreError> {
    let (ct, nonce) = encrypt_chunk(chunk, key)?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Разбираем nonce|ciphertext -> исходный Chunk
pub fn open_chunk(index: u32, total: u32, buf: &[u8], key: &[u8; 32]) -> Result<Chunk, CoreError> {
    let (nonce, ct) = buf.split_at(12);
    let mut n = [0u8; 12];
    n.copy_from_slice(nonce);
    decrypt_chunk(index, total, ct, &n, key)
}


/* ---------------------------------------------------------------------
 *  Tests
 * ------------------------------------------------------------------*/

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_merge_roundtrip() {
        let data = vec![42u8; 123_456];
        let chunks = split_bytes(&data, DEFAULT_CHUNK_SIZE);
        let merged = merge_chunks(chunks).unwrap();
        assert_eq!(merged, data);
    }

    #[test]
    fn crypto_roundtrip() {
        let chunk = Chunk::new(0, 1, b"hello world".to_vec());
        let salt = b"rand-salt-123";
        let key = derive_key("password", salt).unwrap();
        let (ct, nonce) = encrypt_chunk(&chunk, &key).unwrap();
        let out = decrypt_chunk(chunk.index, chunk.total, &ct, &nonce, &key).unwrap();
        assert_eq!(out.data, chunk.data);
    }
}
