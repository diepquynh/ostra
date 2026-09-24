//! Web Push without OpenSSL: RFC 8291 message encryption in the RFC 8188 `aes128gcm` content
//! coding, and RFC 8292 VAPID authentication (ES256 JWT).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hkdf::Hkdf;
use ostra_core::api::PushSubscription;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Record size advertised in the aes128gcm header. One record holds the whole message.
const RECORD_SIZE: u32 = 4096;
/// Push services accept at most 4096 bytes of body; this leaves room for the header and tag.
pub const MAX_PLAINTEXT: usize = 3993;
const JWT_TTL: Duration = Duration::from_secs(12 * 3600);
const PUSH_TTL_SECS: u32 = 24 * 3600;

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    #[error("invalid key: {0}")]
    Key(String),
    #[error("payload is {0} bytes; the limit is {MAX_PLAINTEXT}")]
    TooLarge(usize),
    #[error("encryption failed")]
    Encrypt,
    #[error("invalid endpoint: {0}")]
    Endpoint(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// The notification the service worker shows. Serialized as the push message body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Deep link to the screen that needs the user.
    pub url: String,
    /// Notifications with the same tag replace each other.
    pub tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered,
    /// 404 or 410: the subscription is dead and the caller removes it.
    Gone,
    Failed {
        status: Option<u16>,
        message: String,
    },
}

/// The application server's VAPID key pair (P-256).
#[derive(Clone)]
pub struct VapidKeys {
    secret: SecretKey,
}

impl std::fmt::Debug for VapidKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VapidKeys")
            .field("public", &self.public_key_b64url())
            .finish()
    }
}

fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode URL-safe base64 with or without padding, as browsers produce either.
pub fn b64_decode(text: &str) -> Result<Vec<u8>, NotifyError> {
    URL_SAFE_NO_PAD
        .decode(text.trim().trim_end_matches('='))
        .map_err(|e| NotifyError::Key(format!("base64: {e}")))
}

fn uncompressed(public: &PublicKey) -> Vec<u8> {
    public.to_sec1_point(false).as_bytes().to_vec()
}

fn random_secret() -> SecretKey {
    SecretKey::generate_from_rng(&mut rand::rng())
}

impl VapidKeys {
    pub fn generate() -> Self {
        VapidKeys {
            secret: random_secret(),
        }
    }

    pub fn from_private_bytes(bytes: &[u8]) -> Result<Self, NotifyError> {
        SecretKey::from_slice(bytes)
            .map(|secret| VapidKeys { secret })
            .map_err(|e| NotifyError::Key(e.to_string()))
    }

    pub fn private_bytes(&self) -> Vec<u8> {
        self.secret.to_bytes().to_vec()
    }

    /// Uncompressed public point, URL-safe base64: the browser's `applicationServerKey`.
    pub fn public_key_b64url(&self) -> String {
        b64(&uncompressed(&self.secret.public_key()))
    }

    /// An ES256 JWT for the push service's origin (RFC 8292 section 2).
    pub fn jwt(&self, audience: &str, subject: &str, expires: u64) -> String {
        let header = b64(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({ "aud": audience, "exp": expires, "sub": subject });
        let signing_input = format!("{header}.{}", b64(claims.to_string().as_bytes()));
        let key = SigningKey::from(&self.secret);
        let signature: Signature = key.sign(signing_input.as_bytes());
        format!("{signing_input}.{}", b64(&signature.to_bytes()))
    }

    /// The `Authorization` header value: `vapid t=<jwt>, k=<public key>`.
    pub fn authorization(&self, audience: &str, subject: &str, expires: u64) -> String {
        format!(
            "vapid t={}, k={}",
            self.jwt(audience, subject, expires),
            self.public_key_b64url()
        )
    }
}

fn hkdf_expand(salt: &[u8], ikm: &[u8], info: &[u8], out: &mut [u8]) -> Result<(), NotifyError> {
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, out)
        .map_err(|_| NotifyError::Encrypt)
}

/// RFC 8291 encryption with explicit sender key and salt, so the RFC test vector can be checked.
/// Returns the full aes128gcm body: salt, record size, key id (sender public key), ciphertext.
pub fn encrypt_with(
    plaintext: &[u8],
    ua_public: &[u8],
    auth_secret: &[u8],
    as_secret: &SecretKey,
    salt: &[u8; 16],
) -> Result<Vec<u8>, NotifyError> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(NotifyError::TooLarge(plaintext.len()));
    }
    let ua_key = PublicKey::from_sec1_bytes(ua_public)
        .map_err(|e| NotifyError::Key(format!("p256dh: {e}")))?;
    let as_public = uncompressed(&as_secret.public_key());
    let ecdh = as_secret.diffie_hellman(&ua_key);

    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(&as_public);
    let mut ikm = [0u8; 32];
    hkdf_expand(auth_secret, ecdh.raw_secret_bytes(), &key_info, &mut ikm)?;

    let mut cek = [0u8; 16];
    hkdf_expand(salt, &ikm, b"Content-Encoding: aes128gcm\0", &mut cek)?;
    let mut nonce = [0u8; 12];
    hkdf_expand(salt, &ikm, b"Content-Encoding: nonce\0", &mut nonce)?;

    let mut record = plaintext.to_vec();
    // Padding delimiter for the last (only) record.
    record.push(0x02);
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| NotifyError::Encrypt)?;
    let nonce = Nonce::try_from(&nonce[..]).map_err(|_| NotifyError::Encrypt)?;
    let ciphertext = cipher
        .encrypt(&nonce, record.as_slice())
        .map_err(|_| NotifyError::Encrypt)?;

    let mut body = Vec::with_capacity(16 + 4 + 1 + as_public.len() + ciphertext.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// RFC 8291 encryption with a fresh sender key and salt.
pub fn encrypt(
    plaintext: &[u8],
    ua_public: &[u8],
    auth_secret: &[u8],
) -> Result<Vec<u8>, NotifyError> {
    let mut salt = [0u8; 16];
    rand::fill(&mut salt);
    encrypt_with(plaintext, ua_public, auth_secret, &random_secret(), &salt)
}

/// `scheme://host[:port]` of an endpoint: the JWT audience.
fn origin_of(endpoint: &str) -> Result<String, NotifyError> {
    let url = reqwest::Url::parse(endpoint).map_err(|e| NotifyError::Endpoint(e.to_string()))?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(NotifyError::Endpoint(format!(
            "unsupported scheme `{}`",
            url.scheme()
        )));
    }
    Ok(url.origin().ascii_serialization())
}

#[derive(Debug, Clone)]
pub struct Notifier {
    keys: VapidKeys,
    subject: String,
    client: reqwest::Client,
}

impl Notifier {
    /// `subject` is a `mailto:` or `https:` contact URI (RFC 8292 section 2.1).
    pub fn new(keys: VapidKeys, subject: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Notifier {
            keys,
            subject,
            client,
        }
    }

    pub fn keys(&self) -> &VapidKeys {
        &self.keys
    }

    pub async fn send(
        &self,
        sub: &PushSubscription,
        payload: &Notification,
    ) -> Result<SendOutcome, NotifyError> {
        let ua_public = b64_decode(&sub.keys.p256dh)?;
        let auth = b64_decode(&sub.keys.auth)?;
        let body = encrypt(&serde_json::to_vec(payload)?, &ua_public, &auth)?;
        let audience = origin_of(&sub.endpoint)?;
        let expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + JWT_TTL.as_secs();
        let request = self
            .client
            .post(&sub.endpoint)
            .header(
                "Authorization",
                self.keys.authorization(&audience, &self.subject, expires),
            )
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("TTL", PUSH_TTL_SECS.to_string())
            .header("Urgency", "high")
            .body(body);
        match request.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match status {
                    200..=299 => Ok(SendOutcome::Delivered),
                    404 | 410 => Ok(SendOutcome::Gone),
                    _ => {
                        let text = resp.text().await.unwrap_or_default();
                        tracing::warn!(status, "push service refused a message");
                        Ok(SendOutcome::Failed {
                            status: Some(status),
                            message: text.chars().take(500).collect(),
                        })
                    }
                }
            }
            Err(e) => Ok(SendOutcome::Failed {
                status: None,
                message: e.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;

    // RFC 8291 Appendix A.
    const PLAINTEXT: &str = "When I grow up, I want to be a watermelon";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const AS_PUBLIC: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const EXPECTED: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    fn d(s: &str) -> Vec<u8> {
        b64_decode(s).unwrap()
    }

    #[test]
    fn rfc8291_test_vector() {
        let as_secret = SecretKey::from_slice(&d(AS_PRIVATE)).unwrap();
        assert_eq!(b64(&uncompressed(&as_secret.public_key())), AS_PUBLIC);
        let salt: [u8; 16] = d(SALT).try_into().unwrap();
        let body = encrypt_with(
            PLAINTEXT.as_bytes(),
            &d(UA_PUBLIC),
            &d(AUTH),
            &as_secret,
            &salt,
        )
        .unwrap();
        assert_eq!(b64(&body), EXPECTED);
    }

    fn decrypt(body: &[u8], ua_secret: &SecretKey, auth: &[u8]) -> Vec<u8> {
        let salt = &body[..16];
        let idlen = body[20] as usize;
        let as_public = &body[21..21 + idlen];
        let ciphertext = &body[21 + idlen..];
        let ua_public = uncompressed(&ua_secret.public_key());
        let ecdh = ua_secret.diffie_hellman(&PublicKey::from_sec1_bytes(as_public).unwrap());
        let mut key_info = b"WebPush: info\0".to_vec();
        key_info.extend_from_slice(&ua_public);
        key_info.extend_from_slice(as_public);
        let mut ikm = [0u8; 32];
        hkdf_expand(auth, ecdh.raw_secret_bytes(), &key_info, &mut ikm).unwrap();
        let mut cek = [0u8; 16];
        hkdf_expand(salt, &ikm, b"Content-Encoding: aes128gcm\0", &mut cek).unwrap();
        let mut nonce = [0u8; 12];
        hkdf_expand(salt, &ikm, b"Content-Encoding: nonce\0", &mut nonce).unwrap();
        let cipher = Aes128Gcm::new_from_slice(&cek).unwrap();
        let mut plain = cipher
            .decrypt(&Nonce::try_from(&nonce[..]).unwrap(), ciphertext)
            .unwrap();
        assert_eq!(plain.pop(), Some(0x02));
        plain
    }

    #[test]
    fn round_trip_with_fresh_keys() {
        let ua_secret = SecretKey::from_slice(&d(UA_PRIVATE)).unwrap();
        let auth = d(AUTH);
        let note = Notification {
            title: "Gate waiting".into(),
            body: "Spec approval".into(),
            url: "/s/1".into(),
            tag: "g".into(),
        };
        let json = serde_json::to_vec(&note).unwrap();
        let body = encrypt(&json, &d(UA_PUBLIC), &auth).unwrap();
        assert_eq!(decrypt(&body, &ua_secret, &auth), json);
        assert_eq!(
            u32::from_be_bytes(body[16..20].try_into().unwrap()),
            RECORD_SIZE
        );
    }

    #[test]
    fn rejects_oversized_payload() {
        let big = vec![b'x'; MAX_PLAINTEXT + 1];
        assert!(matches!(
            encrypt(&big, &d(UA_PUBLIC), &d(AUTH)),
            Err(NotifyError::TooLarge(_))
        ));
    }

    #[test]
    fn vapid_jwt_verifies() {
        let keys = VapidKeys::generate();
        let restored = VapidKeys::from_private_bytes(&keys.private_bytes()).unwrap();
        assert_eq!(restored.public_key_b64url(), keys.public_key_b64url());

        let jwt = keys.jwt(
            "https://push.example.net",
            "mailto:ostra@localhost",
            1_900_000_000,
        );
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value = serde_json::from_slice(&d(parts[0])).unwrap();
        assert_eq!(header["alg"], "ES256");
        let claims: serde_json::Value = serde_json::from_slice(&d(parts[1])).unwrap();
        assert_eq!(claims["aud"], "https://push.example.net");
        assert_eq!(claims["exp"], 1_900_000_000u64);

        let public = d(&keys.public_key_b64url());
        assert_eq!(public.len(), 65);
        let verifier = VerifyingKey::from_sec1_bytes(&public).unwrap();
        let sig_bytes = d(parts[2]);
        assert_eq!(sig_bytes.len(), 64);
        let sig = Signature::from_slice(&sig_bytes).unwrap();
        let input = format!("{}.{}", parts[0], parts[1]);
        verifier.verify(input.as_bytes(), &sig).unwrap();
        assert!(verifier.verify(b"tampered", &sig).is_err());

        let auth = keys.authorization("https://push.example.net", "mailto:x", 1);
        assert!(
            auth.starts_with("vapid t=")
                && auth.ends_with(&format!("k={}", keys.public_key_b64url()))
        );
    }

    #[test]
    fn audience_is_origin() {
        assert_eq!(
            origin_of("https://fcm.googleapis.com/fcm/send/abc").unwrap(),
            "https://fcm.googleapis.com"
        );
        assert_eq!(
            origin_of("https://push.example:8443/x").unwrap(),
            "https://push.example:8443"
        );
        assert!(origin_of("file:///etc/passwd").is_err());
    }

    #[test]
    fn padded_base64_is_accepted() {
        assert_eq!(b64_decode("BTBZMqHH6r4Tts7J_aSIgg==").unwrap(), d(AUTH));
    }
}
