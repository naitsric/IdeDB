//! Client tokens. Only a token's SHA-256 is stored; the token itself is
//! shown to the user once.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// Every token starts with it, so a leaked one is easy to recognize.
const PREFIX: &str = "idedb_";

/// Characters of a token shown to tell tokens apart: the prefix and six more.
const DISPLAY_LEN: usize = 12;

/// A new random token (`idedb_` and 32 random bytes in base64url), its
/// SHA-256 (what the store keeps) and the prefix shown to tell it apart.
pub fn new_token() -> (String, [u8; 32], String) {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system's random number generator failed");
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let hash = hash_token(&token);
    let display = token[..DISPLAY_LEN].to_owned();
    (token, hash, display)
}

/// The SHA-256 the store looks tokens up by.
pub fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_prefixed_and_hashed() {
        let (token, hash, display) = new_token();
        assert!(token.starts_with("idedb_"), "{token}");
        // 32 bytes in unpadded base64 are 43 characters, all URL safe.
        assert_eq!(token.len(), 6 + 43);
        assert!(token[6..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{token}");
        assert_eq!(URL_SAFE_NO_PAD.decode(&token[6..]).unwrap().len(), 32);
        assert_eq!(hash, hash_token(&token));
        assert_eq!(display, &token[..12]);

        let (other, other_hash, _) = new_token();
        assert_ne!(token, other);
        assert_ne!(hash, other_hash);
    }

    #[test]
    fn hashes_with_sha256() {
        // sha256("abc"), from FIPS 180-2.
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let hex: String = hash_token("abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, expected);
    }
}
