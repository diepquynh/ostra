//! Encryption at rest for credentials in the registry. Each value is sealed with AES-256-GCM under
//! the machine's master key, with a fresh nonce and the row's key as associated data, so a sealed
//! value cannot be moved to another row.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

const PREFIX: &[u8] = b"enc:v1:";
const NONCE_LEN: usize = 12;

/// Registry keys whose values are credentials: exact keys, and prefixes ending in `:`.
pub const SECRET_KEYS: &[&str] = &[
    "vapid_private",
    "provider_credentials:",
    "git_credential:",
    "mcp_oauth:",
    "mcp_secret:",
];

pub fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.iter().any(|k| {
        if k.ends_with(':') {
            key.starts_with(k)
        } else {
            key == *k
        }
    })
}

pub fn is_sealed(stored: &[u8]) -> bool {
    stored.starts_with(PREFIX)
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Sealed under another key, or altered.
    Undecryptable,
}

pub struct Sealer {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for Sealer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealer")
    }
}

impl Sealer {
    pub fn new(key: &[u8; 32]) -> Self {
        Sealer {
            cipher: Aes256Gcm::new_from_slice(key).expect("32-byte key"),
        }
    }

    /// A key that lives only as long as the process, for in-memory registries.
    pub fn ephemeral() -> Self {
        Sealer::new(&generate_key())
    }

    pub fn seal(&self, row: &str, plain: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::fill(&mut nonce);
        let ct = self
            .cipher
            .encrypt(
                &Nonce::try_from(&nonce[..]).expect("12-byte nonce"),
                Payload {
                    msg: plain,
                    aad: row.as_bytes(),
                },
            )
            .expect("AES-GCM encryption does not fail for small inputs");
        let mut body = nonce.to_vec();
        body.extend_from_slice(&ct);
        let mut out = PREFIX.to_vec();
        out.extend_from_slice(STANDARD.encode(body).as_bytes());
        out
    }

    /// The plaintext of a sealed value. A value without the prefix is returned as is, because
    /// registries written before encryption hold plaintext until they are migrated.
    pub fn open(&self, row: &str, stored: &[u8]) -> Result<Vec<u8>, OpenError> {
        let Some(b64) = stored.strip_prefix(PREFIX) else {
            return Ok(stored.to_vec());
        };
        let body = STANDARD
            .decode(b64)
            .map_err(|_| OpenError::Undecryptable)?;
        if body.len() < NONCE_LEN {
            return Err(OpenError::Undecryptable);
        }
        let (nonce, ct) = body.split_at(NONCE_LEN);
        self.cipher
            .decrypt(
                &Nonce::try_from(nonce).map_err(|_| OpenError::Undecryptable)?,
                Payload {
                    msg: ct,
                    aad: row.as_bytes(),
                },
            )
            .map_err(|_| OpenError::Undecryptable)
    }
}

pub fn generate_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::fill(&mut key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_binds_the_row() {
        let s = Sealer::ephemeral();
        let sealed = s.seal("git_credential:a", b"ghp_secret");
        assert!(is_sealed(&sealed));
        assert!(!sealed.windows(10).any(|w| w == b"ghp_secret"));
        assert_eq!(s.open("git_credential:a", &sealed).unwrap(), b"ghp_secret");
        assert_eq!(
            s.open("git_credential:b", &sealed),
            Err(OpenError::Undecryptable),
            "a value moved to another row does not open"
        );
    }

    #[test]
    fn nonces_differ_per_seal() {
        let s = Sealer::ephemeral();
        assert_ne!(s.seal("k", b"v"), s.seal("k", b"v"));
    }

    #[test]
    fn tampering_and_wrong_keys_are_detected() {
        let s = Sealer::ephemeral();
        let mut sealed = s.seal("k", b"value");
        let last = sealed.len() - 3;
        sealed[last] = if sealed[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(s.open("k", &sealed), Err(OpenError::Undecryptable));
        let other = Sealer::ephemeral();
        assert_eq!(
            other.open("k", &s.seal("k", b"value")),
            Err(OpenError::Undecryptable)
        );
    }

    #[test]
    fn plaintext_passes_through_for_migration() {
        let s = Sealer::ephemeral();
        assert_eq!(s.open("k", b"{\"a\":1}").unwrap(), b"{\"a\":1}");
    }

    #[test]
    fn secret_keys_match_exactly_or_by_prefix() {
        assert!(is_secret_key("vapid_private"));
        assert!(is_secret_key("mcp_oauth:/w:srv"));
        assert!(!is_secret_key("vapid_private_x"));
        assert!(!is_secret_key("auth:token:abc"));
    }
}
