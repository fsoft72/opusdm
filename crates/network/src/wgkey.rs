//! WireGuard keys: Curve25519 keys written in base64, as `wg genkey` and
//! `wg pubkey` make them.

use crate::error::NetError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use x25519_dalek::{PublicKey, StaticSecret};

/// The length of a key in bytes.
const KEY_LEN: usize = 32;

/// A new key pair: `(private, public)`, both in base64.
pub fn generate() -> Result<(String, String), NetError> {
    let mut bytes = [0u8; KEY_LEN];
    getrandom::getrandom(&mut bytes).map_err(|e| NetError::other(format!("No random numbers: {e}")))?;
    let private = STANDARD.encode(StaticSecret::from(bytes).to_bytes());
    let public = public_of(&private)?;
    Ok((private, public))
}

/// The public key of a private one.
pub fn public_of(private: &str) -> Result<String, NetError> {
    let bytes = decode(private).ok_or_else(|| NetError::other("The private key is not a WireGuard key"))?;
    Ok(STANDARD.encode(PublicKey::from(&StaticSecret::from(bytes)).as_bytes()))
}

/// Whether `text` is a key: 32 bytes in base64.
pub fn is_key(text: &str) -> bool {
    decode(text).is_some()
}

/// The 32 bytes of a base64 key.
fn decode(text: &str) -> Option<[u8; KEY_LEN]> {
    STANDARD.decode(text.trim()).ok()?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn the_public_key_matches_rfc_7748() {
        let private = STANDARD.encode(hex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"));
        let public = STANDARD.encode(hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"));
        assert_eq!(public_of(&private).unwrap(), public);
    }

    #[test]
    fn a_generated_pair_belongs_together() {
        let (private, public) = generate().unwrap();
        assert!(is_key(&private) && is_key(&public));
        assert_eq!(public_of(&private).unwrap(), public);
        assert_ne!(generate().unwrap().0, private);
    }

    #[test]
    fn only_32_bytes_in_base64_are_a_key() {
        assert!(!is_key(""));
        assert!(!is_key("not base64!"));
        assert!(!is_key(&STANDARD.encode([0u8; 16])));
        assert!(public_of("short").is_err());
    }
}
