//! Content hashing. XXH3-128 is used internally (≈60 GB/s, never the bottleneck);
//! SHA-256 is used for manifests that users may verify with standard tools.

use serde::{Deserialize, Serialize};
use sha2::Digest;
use xxhash_rust::xxh3::Xxh3;

/// 128-bit content fingerprint (XXH3-128).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Fingerprint(pub u128);

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// Streaming XXH3-128 hasher.
#[derive(Clone)]
pub struct Hasher(Xxh3);

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher {
    pub fn new() -> Self {
        Hasher(Xxh3::new())
    }
    #[inline]
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    pub fn finish(&self) -> Fingerprint {
        Fingerprint(self.0.digest128())
    }
}

/// One-shot fingerprint of a byte slice.
pub fn fingerprint(data: &[u8]) -> Fingerprint {
    Fingerprint(xxhash_rust::xxh3::xxh3_128(data))
}

/// Streaming SHA-256.
#[derive(Clone, Default)]
pub struct Sha256(sha2::Sha256);

impl Sha256 {
    pub fn new() -> Self {
        Sha256(sha2::Sha256::new())
    }
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    pub fn finish_hex(self) -> String {
        hex(&self.0.finalize())
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_equals_oneshot() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i * 31) as u8).collect();
        let mut h = Hasher::new();
        for c in data.chunks(777) {
            h.update(c);
        }
        assert_eq!(h.finish(), fingerprint(&data));
    }
    #[test]
    fn sha256_known() {
        let mut s = Sha256::new();
        s.update(b"abc");
        assert_eq!(
            s.finish_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
