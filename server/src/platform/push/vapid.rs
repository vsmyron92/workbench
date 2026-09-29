//! VAPID (RFC 8292): the application server's P-256 identity and the ES256 JWT
//! every push request carries.
//!
//! The key pair is generated once and kept in `data_dir/push/vapid.json` (0600).
//! Browsers bind a subscription to the public key they were given, so replacing it
//! makes every existing subscription useless (devices re-subscribe the next time
//! they open Workbench).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use p256::SecretKey;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::Generate;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::ece;
use crate::util;

/// RFC 8292: `exp` at most 24 hours ahead; push services commonly want 12 or less.
pub const JWT_LIFETIME_SECS: i64 = 12 * 3600;
/// A cached token is reused while it has at least this long left.
const REUSE_MIN_LEFT_SECS: i64 = 3600;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VapidFile {
    private_key: String,
    public_key: String,
    created_at: i64,
}

pub struct Vapid {
    secret: SecretKey,
    /// Uncompressed public key, base64url: the `applicationServerKey` and the `k=` parameter.
    public_b64: String,
    /// Signed tokens by (audience, subject), with their `exp`.
    tokens: Mutex<HashMap<(String, String), (String, i64)>>,
}

impl std::fmt::Debug for Vapid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vapid").field("public", &self.public_b64).finish_non_exhaustive()
    }
}

impl Vapid {
    fn from_secret(secret: SecretKey) -> Self {
        let public_b64 = B64.encode(ece::uncompressed(&secret.public_key()));
        Self { secret, public_b64, tokens: Mutex::new(HashMap::new()) }
    }

    pub fn generate() -> anyhow::Result<Self> {
        let secret = SecretKey::try_generate().map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
        Ok(Self::from_secret(secret))
    }

    /// Load `dir/vapid.json`, creating it on first use. A file that does not parse is
    /// kept aside (`vapid.json.bad-<ms>`) and replaced; `Ok((vapid, true))` says the
    /// key is new, so stored subscriptions no longer work.
    pub fn load_or_create(dir: &Path) -> anyhow::Result<(Self, bool)> {
        let file = dir.join("vapid.json");
        match util::fs::read_json::<VapidFile>(&file) {
            Ok(Some(f)) => match Self::from_file(&f) {
                Ok(v) => {
                    util::fs::set_mode(&file, 0o600);
                    return Ok((v, false));
                }
                Err(e) => set_aside(&file, &format!("{e:#}")),
            },
            Ok(None) => {}
            Err(e) => set_aside(&file, &format!("{e:#}")),
        }
        let v = Self::generate()?;
        create_private_dir(dir)?;
        let f = VapidFile {
            private_key: B64.encode(v.secret.to_bytes()),
            public_key: v.public_b64.clone(),
            created_at: util::now_ms(),
        };
        util::fs::write_json(&file, &f)?;
        util::fs::set_mode(&file, 0o600);
        Ok((v, true))
    }

    fn from_file(f: &VapidFile) -> anyhow::Result<Self> {
        let raw = B64.decode(f.private_key.trim()).context("privateKey is not base64url")?;
        let secret = SecretKey::from_slice(&raw).map_err(|_| anyhow::anyhow!("privateKey is not a P-256 key"))?;
        let v = Self::from_secret(secret);
        if v.public_b64 != f.public_key.trim() {
            anyhow::bail!("publicKey does not belong to privateKey");
        }
        Ok(v)
    }

    pub fn public_key(&self) -> &str {
        &self.public_b64
    }

    /// `Authorization` header value for a push to `endpoint_origin` (`https://host`).
    pub fn authorization(&self, audience: &str, subject: &str, now_secs: i64) -> String {
        let key = (audience.to_string(), subject.to_string());
        let mut tokens = self.tokens.lock();
        let token = match tokens.get(&key) {
            Some((t, exp)) if exp - now_secs >= REUSE_MIN_LEFT_SECS => t.clone(),
            _ => {
                let exp = now_secs + JWT_LIFETIME_SECS;
                let t = self.sign_jwt(audience, subject, exp);
                if tokens.len() > 32 {
                    tokens.clear();
                }
                tokens.insert(key, (t.clone(), exp));
                t
            }
        };
        format!("vapid t={token}, k={}", self.public_b64)
    }

    fn sign_jwt(&self, audience: &str, subject: &str, exp: i64) -> String {
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = B64.encode(serde_json::json!({ "aud": audience, "exp": exp, "sub": subject }).to_string());
        let signing_input = format!("{header}.{claims}");
        let key = SigningKey::from(&self.secret);
        let sig: Signature = key.sign(signing_input.as_bytes());
        // JWS ES256: the raw r || s, 64 bytes.
        format!("{signing_input}.{}", B64.encode(sig.to_bytes()))
    }
}

fn set_aside(file: &Path, why: &str) {
    let bad = file.with_extension(format!("json.bad-{}", util::now_ms()));
    tracing::warn!("push: {} is unusable ({why}); moved to {} and making a new key (devices re-subscribe)", file.display(), bad.display());
    let _ = std::fs::rename(file, &bad);
}

/// `data_dir/push`, mode 0700.
pub fn create_private_dir(dir: &Path) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    util::fs::set_mode(dir, 0o700);
    Ok(dir.to_path_buf())
}

/// Parsed and verified claims of a VAPID token (the tests' mock push service).
#[cfg(test)]
#[derive(Debug, Deserialize)]
pub struct Claims {
    pub aud: String,
    pub exp: i64,
    pub sub: String,
}

/// Check an `Authorization: vapid t=…, k=…` header the way a push service does:
/// ES256 signature by `k`, `aud`, and `exp` within 24 hours.
#[cfg(test)]
pub fn verify_authorization(header: &str, expected_key: &str, audience: &str, now_secs: i64) -> Result<Claims, String> {
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;
    let rest = header.strip_prefix("vapid ").ok_or("not a vapid scheme")?;
    let mut t = None;
    let mut k = None;
    for part in rest.split(',') {
        let (name, value) = part.trim().split_once('=').ok_or("bad parameter")?;
        match name {
            "t" => t = Some(value),
            "k" => k = Some(value),
            _ => return Err(format!("unexpected parameter {name}")),
        }
    }
    let (t, k) = (t.ok_or("no t")?, k.ok_or("no k")?);
    if k != expected_key {
        return Err("k is not the subscription's application server key".into());
    }
    let key = VerifyingKey::from_sec1_bytes(&B64.decode(k).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let (signing_input, sig) = t.rsplit_once('.').ok_or("no signature")?;
    let sig = Signature::from_slice(&B64.decode(sig).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    key.verify(signing_input.as_bytes(), &sig).map_err(|_| "bad signature".to_string())?;
    let (header, claims) = signing_input.split_once('.').ok_or("no claims")?;
    let header: serde_json::Value = serde_json::from_slice(&B64.decode(header).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if header["alg"] != "ES256" || header["typ"] != "JWT" {
        return Err(format!("unexpected JWT header {header}"));
    }
    let claims: Claims = serde_json::from_slice(&B64.decode(claims).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if claims.aud != audience {
        return Err(format!("aud {} is not {audience}", claims.aud));
    }
    if claims.exp <= now_secs || claims.exp > now_secs + 24 * 3600 {
        return Err(format!("exp {} out of range", claims.exp));
    }
    if !(claims.sub.starts_with("mailto:") || claims.sub.starts_with("https://")) {
        return Err(format!("sub {} is neither mailto: nor https:", claims.sub));
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_verifies_with_the_public_key() {
        let v = Vapid::generate().unwrap();
        let now = 1_800_000_000;
        let h = v.authorization("https://fcm.googleapis.com", "mailto:me@example.com", now);
        let claims = verify_authorization(&h, v.public_key(), "https://fcm.googleapis.com", now).unwrap();
        assert_eq!(claims.exp, now + JWT_LIFETIME_SECS);
        assert!(claims.exp - now <= 12 * 3600);
        assert_eq!(claims.sub, "mailto:me@example.com");
        // Another audience or key does not verify.
        assert!(verify_authorization(&h, v.public_key(), "https://web.push.apple.com", now).is_err());
        let other = Vapid::generate().unwrap();
        assert!(verify_authorization(&h, other.public_key(), "https://fcm.googleapis.com", now).is_err());
        // A tampered claim breaks the signature.
        let (scheme_t, k) = h.split_once(", ").unwrap();
        let t = scheme_t.strip_prefix("vapid t=").unwrap();
        let mut parts: Vec<&str> = t.split('.').collect();
        let forged = B64.encode(br#"{"aud":"https://fcm.googleapis.com","exp":1800043200,"sub":"mailto:x@y.z"}"#);
        parts[1] = &forged;
        assert_eq!(
            verify_authorization(&format!("vapid t={}, {k}", parts.join(".")), v.public_key(), "https://fcm.googleapis.com", now).unwrap_err(),
            "bad signature"
        );
    }

    #[test]
    fn tokens_are_reused_until_an_hour_before_expiry() {
        let v = Vapid::generate().unwrap();
        let a = v.authorization("https://a.example", "mailto:x@y.z", 1000);
        assert_eq!(a, v.authorization("https://a.example", "mailto:x@y.z", 1000 + 3600));
        assert_ne!(a, v.authorization("https://a.example", "mailto:x@y.z", 1000 + JWT_LIFETIME_SECS - 1800));
        assert_ne!(a, v.authorization("https://b.example", "mailto:x@y.z", 1000));
    }

    #[test]
    fn key_file_is_created_once_private_and_recovered_when_broken() {
        use crate::util::os::perm;
        let dir = tempfile::tempdir().unwrap();
        let push = dir.path().join("push");
        let (a, fresh) = Vapid::load_or_create(&push).unwrap();
        assert!(fresh);
        perm::assert_mode(&push.join("vapid.json"), 0o600);
        perm::assert_mode(&push, 0o700);
        let (b, fresh) = Vapid::load_or_create(&push).unwrap();
        assert!(!fresh);
        assert_eq!(a.public_key(), b.public_key());
        // The public key is 65 bytes, uncompressed.
        assert_eq!(B64.decode(a.public_key()).unwrap().len(), 65);

        std::fs::write(push.join("vapid.json"), "{not json").unwrap();
        let (c, fresh) = Vapid::load_or_create(&push).unwrap();
        assert!(fresh);
        assert_ne!(c.public_key(), a.public_key());
        let kept: Vec<_> = std::fs::read_dir(&push).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().contains(".bad-")).collect();
        assert_eq!(kept.len(), 1);
    }
}
