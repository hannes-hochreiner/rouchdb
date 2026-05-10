#![cfg(target_arch = "wasm32")]

use md5::{Digest, Md5};
use rouchdb_core::error::{Result, RouchError};

pub fn generate_rev_hash(
    doc_data: &serde_json::Value,
    deleted: bool,
    prev_rev: Option<&str>,
) -> Result<String> {
    let mut hasher = Md5::new();
    if let Some(prev) = prev_rev {
        hasher.update(prev.as_bytes());
    }
    hasher.update(if deleted { b"1" } else { b"0" });
    let serialized = serde_json::to_string(doc_data).map_err(serde_err)?;
    hasher.update(serialized.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn rev_string(pos: u64, hash: &str) -> String {
    format!("{}-{}", pos, hash)
}

pub fn parse_rev(rev_str: &str) -> Result<(u64, String)> {
    let (pos_str, hash) = rev_str
        .split_once('-')
        .ok_or_else(|| RouchError::InvalidRev(rev_str.to_string()))?;
    let pos: u64 = pos_str
        .parse()
        .map_err(|_| RouchError::InvalidRev(rev_str.to_string()))?;
    Ok((pos, hash.to_string()))
}

pub fn compute_attachment_digest(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    let hash = hasher.finalize();
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(hash);
    format!("md5-{}", b64)
}

pub fn idb_err(e: impl std::fmt::Debug) -> RouchError {
    RouchError::DatabaseError(format!("{:?}", e))
}

pub fn serde_err(e: impl std::fmt::Debug) -> RouchError {
    RouchError::DatabaseError(format!("serde: {:?}", e))
}
