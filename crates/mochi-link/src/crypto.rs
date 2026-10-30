//! Pairing and authentication primitives. Mirrored byte-for-byte in the Android app:
//!
//! - pairing: X25519 key exchange. secret = X25519(my_private, their_public).
//!   code = `pairing_code(secret)` on both sides.
//! - auth:    HMAC-SHA256(secret, nonce) over the raw 32-byte nonce from `Hello`, base64.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("os randomness");
    b
}

/// Desktop side of pairing: given the phone's public key, derive the shared secret and
/// return it with our public key (base64) to send back.
pub fn desktop_pairing(phone_public_b64: &str) -> Option<([u8; 32], String)> {
    let their: [u8; 32] = B64.decode(phone_public_b64).ok()?.try_into().ok()?;
    let mine = StaticSecret::from(random32());
    let our_pub = PublicKey::from(&mine);
    let shared = mine.diffie_hellman(&PublicKey::from(their));
    Some((shared.to_bytes(), B64.encode(our_pub.as_bytes())))
}

pub fn auth_hmac(secret: &[u8], nonce: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("any key length");
    mac.update(nonce);
    B64.encode(mac.finalize().into_bytes())
}

pub fn verify_auth(secret: &[u8], nonce: &[u8], hmac_b64: &str) -> bool {
    let Ok(given) = B64.decode(hmac_b64) else { return false };
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("any key length");
    mac.update(nonce);
    mac.verify_slice(&given).is_ok()
}

/// The phone's half. Tests use it; the Android app implements the same three steps.
pub struct PhoneCrypto {
    secret: StaticSecret,
}

impl Default for PhoneCrypto {
    fn default() -> Self {
        Self::new()
    }
}

impl PhoneCrypto {
    pub fn new() -> Self {
        Self { secret: StaticSecret::from(random32()) }
    }
    pub fn public_key_b64(&self) -> String {
        B64.encode(PublicKey::from(&self.secret).as_bytes())
    }
    pub fn shared_secret(&self, desktop_public_b64: &str) -> Option<[u8; 32]> {
        let their: [u8; 32] = B64.decode(desktop_public_b64).ok()?.try_into().ok()?;
        Some(self.secret.diffie_hellman(&PublicKey::from(their)).to_bytes())
    }
    pub fn auth_hmac(secret: &[u8; 32], nonce_b64: &str) -> String {
        let nonce = B64.decode(nonce_b64).unwrap_or_default();
        auth_hmac(secret, &nonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_derive_the_same_secret_and_code() {
        let phone = PhoneCrypto::new();
        let (desk_secret, desk_pub) = desktop_pairing(&phone.public_key_b64()).unwrap();
        let phone_secret = phone.shared_secret(&desk_pub).unwrap();
        assert_eq!(desk_secret, phone_secret);
        assert_eq!(mochi_core::protocol::pairing_code(&desk_secret), mochi_core::protocol::pairing_code(&phone_secret));
        assert!(desktop_pairing("not base64!").is_none());
    }

    #[test]
    fn hmac_round_trip_and_known_vector() {
        // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?"
        let mac = auth_hmac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(B64.decode(mac).unwrap(), hex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"));
        let nonce = random32();
        let secret = random32();
        assert!(verify_auth(&secret, &nonce, &auth_hmac(&secret, &nonce)));
        assert!(!verify_auth(&secret, &nonce, &auth_hmac(&[1u8; 32], &nonce)));
        assert!(!verify_auth(&secret, &nonce, "???"));
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
}
