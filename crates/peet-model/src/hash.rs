//! Content hashes, used as keys for incremental regeneration and cache validation.
//!
//! A value's hash is the FNV-1a hash of its `postcard` encoding, so it depends only on the
//! value's content (floats by their bits) and is the same in every run and on every
//! platform. That lets keys be compared across sessions (the caches in a saved file).

use serde::Serialize;

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn bytes(data: &[u8]) -> u64 {
    let mut h = OFFSET;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// The content hash of `value`.
pub fn of<T: Serialize + ?Sized>(value: &T) -> u64 {
    // Encoding the model's own types can't fail: they have no maps with non-string keys
    // or other shapes postcard rejects. A failure would only make two keys differ.
    postcard::to_allocvec(value).map_or(0, |b| bytes(&b))
}

pub fn combine(a: u64, b: u64) -> u64 {
    let mut data = [0u8; 16];
    data[..8].copy_from_slice(&a.to_le_bytes());
    data[8..].copy_from_slice(&b.to_le_bytes());
    bytes(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_content_based() {
        // Fixed values: these keys may be stored in files.
        assert_eq!(bytes(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(bytes(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(of(&(1.5f64, "x")), of(&(1.5f64, "x")));
        assert_ne!(of(&(1.5f64, "x")), of(&(1.5000001f64, "x")));
        assert_ne!(combine(1, 2), combine(2, 1));
    }
}
