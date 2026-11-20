//! PBKDF2-HMAC-SHA512 password hashing (NFR-SC-02).
//! Stored format: `pbkdf2-sha512$<iterations>$<b64 salt>$<b64 hash>`.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngCore;
use sha2::Sha512;

pub const ITERATIONS: u32 = 100_000;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 64;
const SCHEME: &str = "pbkdf2-sha512";

fn derive(password: &str, salt: &[u8], iterations: u32) -> [u8; HASH_LEN] {
    let mut out = [0u8; HASH_LEN];
    pbkdf2::pbkdf2_hmac::<Sha512>(password.as_bytes(), salt, iterations, &mut out);
    out
}

pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    let hash = derive(password, &salt, ITERATIONS);
    format!(
        "{SCHEME}${ITERATIONS}${}${}",
        STANDARD.encode(salt),
        STANDARD.encode(hash)
    )
}

/// Constant-time equality for equal-length inputs (false on length mismatch).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Verify a password against a stored hash. Malformed hashes never verify.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some(SCHEME), Some(iters), Some(salt), Some(hash), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    let Ok(iters) = iters.parse::<u32>() else {
        return false;
    };
    if iters == 0 || iters > 10_000_000 {
        return false;
    }
    let (Ok(salt), Ok(expected)) = (STANDARD.decode(salt), STANDARD.decode(hash)) else {
        return false;
    };
    ct_eq(&derive(password, &salt, iters), &expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let h = hash_password("correct horse");
        assert!(h.starts_with("pbkdf2-sha512$100000$"));
        assert_eq!(h.split('$').count(), 4);
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong", &h));
        assert_ne!(h, hash_password("correct horse"), "salt must be random");
    }

    #[test]
    fn malformed_never_verifies() {
        assert!(!verify_password("x", ""));
        assert!(!verify_password("x", "pbkdf2-sha512$100000$AAAA"));
        assert!(!verify_password("x", "md5$1$AAAA$AAAA"));
        assert!(!verify_password("x", "pbkdf2-sha512$abc$AAAA$AAAA"));
        assert!(!verify_password("x", "pbkdf2-sha512$100000$!!$!!"));
    }

    #[test]
    fn ct_eq_basics() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }
}
