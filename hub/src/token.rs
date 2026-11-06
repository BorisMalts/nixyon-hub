use sha2::{Digest, Sha256};

use crate::hex;

const BYTES: usize = 32;

pub struct NewToken {
    pub raw: String,
    pub hash: Vec<u8>,
}

pub fn generate() -> NewToken {
    let mut bytes = [0u8; BYTES];
    rand::fill(&mut bytes);
    let raw = hex::encode(&bytes);
    let hash = digest(&raw);
    NewToken { raw, hash }
}

pub fn digest(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_unique_and_stored_only_as_hash() {
        let a = generate();
        let b = generate();
        assert_eq!(a.raw.len(), BYTES * 2);
        assert_ne!(a.raw, b.raw);
        assert_eq!(a.hash, digest(&a.raw));
        assert_ne!(a.hash, a.raw.as_bytes());
    }

    #[test]
    fn constant_time_eq_compares_content_and_length() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }
}
