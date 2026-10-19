use bytes::{Bytes, BytesMut};
use chacha20::ChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Length of the symmetric key, in bytes (ChaCha20 uses a 256-bit key).
const KEY_LEN: usize = 32;
/// Length of the ChaCha20 nonce, in bytes (96 bits).
const NONCE_LEN: usize = 12;

#[derive(Debug, Error)]
pub enum CipherError {
    #[error("encryption key must be {KEY_LEN} bytes; got {0}")]
    BadKeyLength(usize),
    #[error("encryption key is not valid hex or base64")]
    BadKeyEncoding,
}

/// Block-at-rest encryption for the gateway.
///
/// The gateway encrypts every block before it reaches a worker and decrypts it
/// again on the way back to the client, so workers only ever hold ciphertext.
///
/// ## Why a stream cipher (ChaCha20), not AES-GCM
///
/// The master plans and validates uploads by plaintext size: `StartUpload`
/// hands out one assignment per 16 MiB block and `CommitUpload` rejects any
/// block whose reported size is not exactly the planned size. An AEAD like
/// AES-GCM adds a 16-byte tag (and we would need to store a nonce), so the
/// ciphertext would be larger than the plaintext and the master would reject
/// the commit. We must not touch the master/worker contracts (owners A and B),
/// so we need a cipher that preserves length. ChaCha20 (like AES-CTR) is a
/// stream cipher: the ciphertext is exactly as long as the plaintext.
///
/// ## Integrity
///
/// A stream cipher gives confidentiality but not authentication. Integrity is
/// still covered end to end by the SHA-256 that already exists: the gateway
/// hashes the *ciphertext*, the worker verifies the bytes it stores against
/// that hash, and the download path verifies the bytes it reads against the
/// same hash before decrypting. Any tampering with the stored bytes shows up
/// as a hash mismatch and is reported as corruption, exactly as today.
///
/// ## Nonce
///
/// The (key, nonce) pair must never be reused for different content under a
/// stream cipher. We derive the nonce deterministically from the block id,
/// which the master generates as a fresh UUID v4 for every assignment and
/// every reassignment, so no two distinct blocks ever share a nonce under the
/// same key. Because encryption is deterministic given the block id, the same
/// nonce is reproduced on read, which is exactly what we need to decrypt.
#[derive(Clone)]
pub struct CipherService {
    key: [u8; KEY_LEN],
}

// Never print the key: a derived Debug would leak it into logs.
impl std::fmt::Debug for CipherService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CipherService")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl CipherService {
    /// Builds the service from a key given as 64 hex characters or as base64
    /// (standard, with or without padding). Both decode to 32 raw bytes.
    pub fn new(key_material: &str) -> Result<Self, CipherError> {
        let raw = decode_key(key_material.trim())?;
        if raw.len() != KEY_LEN {
            return Err(CipherError::BadKeyLength(raw.len()));
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(&raw);
        Ok(Self { key })
    }

    /// Encrypts a plaintext block in place and returns the ciphertext, which is
    /// the same length as the input. `block_id` fixes the nonce.
    pub fn encrypt(&self, block_id: &str, plaintext: Bytes) -> Bytes {
        self.apply(block_id, plaintext)
    }

    /// Decrypts a ciphertext block. ChaCha20 is symmetric (XOR with the
    /// keystream), so decryption is the same operation as encryption with the
    /// same (key, nonce).
    pub fn decrypt(&self, block_id: &str, ciphertext: Bytes) -> Bytes {
        self.apply(block_id, ciphertext)
    }

    fn apply(&self, block_id: &str, input: Bytes) -> Bytes {
        let nonce = nonce_from_block_id(block_id);
        let mut cipher = ChaCha20::new(&self.key.into(), &nonce.into());
        let mut buffer = BytesMut::from(input);
        cipher.apply_keystream(&mut buffer);
        buffer.freeze()
    }
}

/// Derives a 12-byte nonce from the block id: SHA-256 of the id, truncated to
/// the nonce length. The block id is a UUID unique per assignment, so the
/// nonce is unique per block under a fixed key.
fn nonce_from_block_id(block_id: &str) -> [u8; NONCE_LEN] {
    let digest = Sha256::digest(block_id.as_bytes());
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&digest[..NONCE_LEN]);
    nonce
}

/// Accepts the key as hex (64 chars) or base64. Tries hex first because its
/// alphabet is a strict subset that would otherwise also parse as base64.
fn decode_key(s: &str) -> Result<Vec<u8>, CipherError> {
    if let Some(bytes) = decode_hex(s) {
        return Ok(bytes);
    }
    decode_base64(s).ok_or(CipherError::BadKeyEncoding)
}

/// Decodes an even-length hex string, or None if it is not valid hex.
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || s.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

/// Minimal standard base64 decoder (RFC 4648), tolerant of missing padding.
/// Avoids pulling in a base64 crate for a single config value.
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
    if cleaned.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(cleaned.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for c in cleaned {
        let v = val(c)?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod service_tests;
