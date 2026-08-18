//! One hash function, used everywhere.
//!
//! Context hashes, provenance hashes, workspace hashes, and doom-loop
//! fingerprints all come from here so that a hash in one part of the log is
//! directly comparable to a hash in another.

pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub fn hash_str(s: &str) -> String {
    hash_bytes(s.as_bytes())
}

/// Hash a sequence of byte slices without concatenating them first, with a
/// length prefix per chunk so that `["ab", "c"]` and `["a", "bc"]` differ.
pub fn hash_chunks<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut h = blake3::Hasher::new();
    for c in chunks {
        h.update(&(c.len() as u64).to_le_bytes());
        h.update(c);
    }
    h.finalize().to_hex().to_string()
}
