//! Random opaque tokens and their storage hashes.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// 32 random bytes, base64url without padding.
pub fn generate_token() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

/// Lower-case hex SHA-256 (64 chars) used for stored token hashes.
pub fn hash_token(raw: &str) -> String {
    let d = Sha256::digest(raw.as_bytes());
    let mut s = String::with_capacity(64);
    for byte in d {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_and_hash() {
        let t = generate_token();
        assert_eq!(t.len(), 43);
        assert_ne!(t, generate_token());
        let h = hash_token(&t);
        assert_eq!(h.len(), 64);
        assert_eq!(h, hash_token(&t));
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
