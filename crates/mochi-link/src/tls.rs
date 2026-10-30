//! TLS for the phone link: one self-signed certificate per desktop, generated on first run.
//! The certificate (public) is kept next to `devices.json` as `link-cert.pem`; the private
//! key lives in the platform secret store (Windows Credential Manager) as PEM. A
//! `link-key.pem` written by 0.1.14 is moved into the store and deleted on first start. The phone has no CA to check against, so it pins the
//! certificate instead: the SHA-256 fingerprint of the DER certificate is mixed into the
//! 6-digit pairing code (so a man-in-the-middle during pairing shows different codes on the
//! two screens) and stored on the phone when pairing succeeds; every later connection must
//! present the same certificate.

use base64::Engine;
use mochi_secrets::{keys, SharedSecrets};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;

pub struct LinkTls {
    pub acceptor: tokio_rustls::TlsAcceptor,
    /// SHA-256 of the DER certificate, lowercase hex.
    pub fingerprint: String,
}

/// Load the desktop certificate from `dir` and its key from `secrets`, generating both the first time.
pub fn load_or_create(dir: &Path, secrets: &SharedSecrets) -> Result<LinkTls, String> {
    let cert_path = dir.join("link-cert.pem");
    let legacy_key_path = dir.join("link-key.pem");

    // 0.1.14 kept the key beside the certificate: move it into the store.
    if legacy_key_path.exists() && secrets.get(keys::LINK_TLS_KEY)?.is_none() {
        let pem = std::fs::read_to_string(&legacy_key_path).map_err(|e| format!("read legacy key: {e}"))?;
        secrets.set(keys::LINK_TLS_KEY, &pem)?;
    }
    if legacy_key_path.exists() && secrets.get(keys::LINK_TLS_KEY)?.is_some() {
        let _ = std::fs::remove_file(&legacy_key_path);
    }

    let key_pem = match secrets.get(keys::LINK_TLS_KEY)? {
        Some(pem) if cert_path.exists() => pem,
        _ => {
            // No usable pair: make a fresh certificate (phones will have to pair again).
            let cert = rcgen::generate_simple_self_signed(vec!["mochi.local".to_string()]).map_err(|e| format!("make certificate: {e}"))?;
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
            let pem = cert.signing_key.serialize_pem();
            secrets.set(keys::LINK_TLS_KEY, &pem)?;
            std::fs::write(&cert_path, cert.cert.pem()).map_err(|e| format!("write certificate: {e}"))?;
            pem
        }
    };
    let cert = CertificateDer::from(pem_body(&std::fs::read_to_string(&cert_path).map_err(|e| format!("read certificate: {e}"))?)?);
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pem_body(&key_pem)?));
    let fingerprint = fingerprint(&cert);
    // Name the provider explicitly: other crates in the workspace (reqwest) enable rustls's
    // aws-lc-rs backend too, and with two providers compiled in `builder()` cannot pick one.
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| format!("tls config: {e}"))?;
    Ok(LinkTls { acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)), fingerprint })
}

/// SHA-256 of the DER certificate, lowercase hex — the same string the phone computes.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    hex(&Sha256::digest(cert.as_ref()))
}

/// What both screens hash into the pairing code: the X25519 secret followed by the
/// certificate fingerprint (as text), so the code also vouches for the TLS endpoint.
pub fn pairing_code(secret: &[u8], fingerprint: &str) -> String {
    let mut input = secret.to_vec();
    input.extend_from_slice(fingerprint.as_bytes());
    mochi_core::protocol::pairing_code(&input)
}

/// DER bytes of the first PEM block in `text` (rcgen writes exactly one per file).
fn pem_body(text: &str) -> Result<Vec<u8>, String> {
    let body: String = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with("-----")).collect();
    base64::engine::general_purpose::STANDARD.decode(body).map_err(|e| format!("bad pem: {e}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mochi-tls-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()))
    }

    #[test]
    fn certificate_is_created_once_and_reloaded() {
        let dir = tmp_dir();
        let secrets: SharedSecrets = Arc::new(mochi_secrets::MemoryStore::default());
        let a = load_or_create(&dir, &secrets).unwrap();
        let b = load_or_create(&dir, &secrets).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint, "second load reuses the stored certificate");
        assert_eq!(a.fingerprint.len(), 64);
        assert!(!dir.join("link-key.pem").exists(), "private key is not written to disk");
        assert!(secrets.get(keys::LINK_TLS_KEY).unwrap().unwrap().contains("PRIVATE KEY"));
        // Losing the key (e.g. a new user profile) makes a fresh certificate rather than failing.
        let fresh: SharedSecrets = Arc::new(mochi_secrets::MemoryStore::default());
        let c = load_or_create(&dir, &fresh).unwrap();
        assert_ne!(c.fingerprint, a.fingerprint);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_key_file_is_moved_into_the_store() {
        let dir = tmp_dir();
        let first: SharedSecrets = Arc::new(mochi_secrets::MemoryStore::default());
        let a = load_or_create(&dir, &first).unwrap();
        // Recreate the 0.1.14 layout: key beside the certificate, nothing in the store.
        std::fs::write(dir.join("link-key.pem"), first.get(keys::LINK_TLS_KEY).unwrap().unwrap()).unwrap();
        let second: SharedSecrets = Arc::new(mochi_secrets::MemoryStore::default());
        let b = load_or_create(&dir, &second).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint, "same certificate, so phones stay paired");
        assert!(!dir.join("link-key.pem").exists(), "legacy key file removed");
        assert!(second.get(keys::LINK_TLS_KEY).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_changes_with_the_certificate() {
        let secret = [7u8; 32];
        assert_ne!(pairing_code(&secret, "aa"), pairing_code(&secret, "bb"));
        assert_eq!(pairing_code(&secret, "aa"), pairing_code(&secret, "aa"));
    }
}

#[cfg(test)]
mod vectors {
    /// Shared with apps/android/.../CryptoTest.kt; both must agree or pairing shows two codes.
    #[test]
    fn pairing_code_known_vector() {
        let secret: Vec<u8> = (0u8..32).collect();
        let fp = "ab".repeat(32);
        assert_eq!(super::pairing_code(&secret, &fp), "299141");
    }
}
