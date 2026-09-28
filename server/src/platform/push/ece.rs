//! Web Push message encryption: RFC 8291 on top of the `aes128gcm` content coding
//! of RFC 8188, as one record.
//!
//! The application server makes a fresh P-256 key pair and a random salt for every
//! message; the user agent's public key (`p256dh`) and authentication secret
//! (`auth`) come from its subscription.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use hkdf::Hkdf;
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use sha2::Sha256;

/// Record size written into the header. The whole message is one record, so this
/// only bounds it: push services accept at most 4096 octets of body.
const RECORD_SIZE: u32 = 4096;
/// salt (16) + rs (4) + idlen (1) + keyid (65, the sender's uncompressed public key).
pub const HEADER_LEN: usize = 86;
const TAG_LEN: usize = 16;
/// Largest payload that fits one 4096-octet message: header, the 0x02 delimiter and the tag.
pub const MAX_PLAINTEXT: usize = 4096 - HEADER_LEN - 1 - TAG_LEN;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EceError {
    #[error("the subscription's p256dh key is not a P-256 public key")]
    BadPublicKey,
    #[error("the subscription's auth secret must be 16 bytes")]
    BadAuthSecret,
    #[error("payload too large for one push message")]
    TooLarge,
    #[error("malformed message")]
    Malformed,
    /// The receiving side only (tests).
    #[cfg(test)]
    #[error("decryption failed")]
    Decrypt,
}

/// Uncompressed SEC1 encoding (0x04 || X || Y), what browsers hand out as `p256dh`.
pub fn uncompressed(pk: &PublicKey) -> Vec<u8> {
    pk.as_affine().to_sec1_point(false).as_bytes().to_vec()
}

/// The subscription's key, checked to be a point on the curve.
pub fn parse_public_key(raw: &[u8]) -> Result<PublicKey, EceError> {
    if raw.len() != 65 || raw[0] != 0x04 {
        return Err(EceError::BadPublicKey);
    }
    PublicKey::from_sec1_bytes(raw).map_err(|_| EceError::BadPublicKey)
}

/// Content encryption key and nonce for one message (RFC 8291 section 3.4).
fn derive(ecdh_secret: &[u8], auth: &[u8], ua_public: &[u8], as_public: &[u8], salt: &[u8]) -> ([u8; 16], [u8; 12]) {
    let mut key_info = Vec::with_capacity(14 + 65 + 65);
    key_info.extend_from_slice(b"WebPush: info\0");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), ecdh_secret)
        .expand(&key_info, &mut ikm)
        .expect("32 bytes is a valid HKDF-SHA-256 length");
    let prk = Hkdf::<Sha256>::new(Some(salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).expect("valid length");
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).expect("valid length");
    (cek, nonce)
}

/// Encrypt `plaintext` for a subscription with the given sender key and salt.
/// `encrypt` picks both at random; tests pass the RFC's.
pub fn encrypt_with(
    sender: &SecretKey,
    salt: &[u8; 16],
    ua_public: &[u8],
    auth: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, EceError> {
    if auth.len() != 16 {
        return Err(EceError::BadAuthSecret);
    }
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(EceError::TooLarge);
    }
    let ua_key = parse_public_key(ua_public)?;
    let as_public = uncompressed(&sender.public_key());
    let shared = p256::ecdh::diffie_hellman(sender.to_nonzero_scalar(), ua_key.as_affine());
    let (cek, nonce) = derive(shared.raw_secret_bytes().as_slice(), auth, ua_public, &as_public, salt);

    // One record, so it is the last: the padding delimiter is 0x02, no padding follows.
    let mut record = Vec::with_capacity(plaintext.len() + 1);
    record.extend_from_slice(plaintext);
    record.push(0x02);
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| EceError::Malformed)?;
    let ciphertext = cipher.encrypt(&Nonce::from(nonce), record.as_slice()).map_err(|_| EceError::Malformed)?;

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Encrypt with a fresh sender key and salt.
pub fn encrypt(ua_public: &[u8], auth: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, EceError> {
    let sender = SecretKey::try_generate().map_err(|_| EceError::Malformed)?;
    let salt: [u8; 16] = rand::random();
    encrypt_with(&sender, &salt, ua_public, auth, plaintext)
}

/// The receiving side (a browser; here the tests' mock push service and device).
#[cfg(test)]
pub fn decrypt(ua_secret: &SecretKey, auth: &[u8], body: &[u8]) -> Result<Vec<u8>, EceError> {
    if body.len() < HEADER_LEN + TAG_LEN + 1 {
        return Err(EceError::Malformed);
    }
    let salt = &body[..16];
    let idlen = body[20] as usize;
    if idlen != 65 {
        return Err(EceError::Malformed);
    }
    let as_public = &body[21..21 + idlen];
    let as_key = parse_public_key(as_public)?;
    let ua_public = uncompressed(&ua_secret.public_key());
    let shared = p256::ecdh::diffie_hellman(ua_secret.to_nonzero_scalar(), as_key.as_affine());
    let (cek, nonce) = derive(shared.raw_secret_bytes().as_slice(), auth, &ua_public, as_public, salt);
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|_| EceError::Malformed)?;
    let mut plain = cipher.decrypt(&Nonce::from(nonce), &body[21 + idlen..]).map_err(|_| EceError::Decrypt)?;
    // Strip the padding: trailing zeros, then the delimiter (0x02 for the last record).
    while plain.last() == Some(&0) {
        plain.pop();
    }
    match plain.pop() {
        Some(0x02) => Ok(plain),
        _ => Err(EceError::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;

    fn b64(s: &str) -> Vec<u8> {
        B64.decode(s.split_whitespace().collect::<String>()).unwrap()
    }

    /// RFC 8291 section 5 and Appendix A: the exact message body.
    #[test]
    fn rfc8291_appendix_a_vector() {
        let plaintext = b64("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
        assert_eq!(plaintext, b"When I grow up, I want to be a watermelon");
        let as_private = SecretKey::from_slice(&b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let as_public = b64("BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8");
        assert_eq!(uncompressed(&as_private.public_key()), as_public);
        let ua_public = b64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let ua_private = SecretKey::from_slice(&b64("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")).unwrap();
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");

        // Intermediate values.
        let ua_key = parse_public_key(&ua_public).unwrap();
        let shared = p256::ecdh::diffie_hellman(as_private.to_nonzero_scalar(), ua_key.as_affine());
        assert_eq!(shared.raw_secret_bytes().as_slice(), b64("kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs"));
        let (cek, nonce) = derive(shared.raw_secret_bytes().as_slice(), &auth, &ua_public, &as_public, &salt);
        assert_eq!(cek.to_vec(), b64("oIhVW04MRdy2XN9CiKLxTg"));
        assert_eq!(nonce.to_vec(), b64("4h_95klXJ5E_qnoN"));

        let body = encrypt_with(&as_private, &salt, &ua_public, &auth, &plaintext).unwrap();
        let expected = b64(
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27ml
             mlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPT
             pK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
        );
        // 86-octet header + 41 + 1 + 16 (the example's "Content-Length: 145" is off by one;
        // its body decodes to 144 octets).
        assert_eq!(body.len(), 144);
        assert_eq!(B64.encode(&body), B64.encode(&expected));

        // And the receiver gets the plaintext back.
        assert_eq!(decrypt(&ua_private, &auth, &body).unwrap(), plaintext);
    }

    #[test]
    fn random_keys_round_trip_and_differ() {
        let ua = SecretKey::try_generate().unwrap();
        let ua_public = uncompressed(&ua.public_key());
        let auth = [7u8; 16];
        let a = encrypt(&ua_public, &auth, b"{\"title\":\"x\"}").unwrap();
        let b = encrypt(&ua_public, &auth, b"{\"title\":\"x\"}").unwrap();
        assert_ne!(a, b, "fresh salt and sender key per message");
        assert_eq!(decrypt(&ua, &auth, &a).unwrap(), b"{\"title\":\"x\"}");
        // A wrong auth secret does not decrypt.
        assert_eq!(decrypt(&ua, &[8u8; 16], &a), Err(EceError::Decrypt));
    }

    #[test]
    fn rejects_bad_keys_and_oversized_payloads() {
        let ua = SecretKey::try_generate().unwrap();
        let ua_public = uncompressed(&ua.public_key());
        assert_eq!(encrypt(&ua_public, &[0u8; 15], b"x"), Err(EceError::BadAuthSecret));
        let mut off_curve = ua_public.clone();
        off_curve[64] ^= 1;
        assert_eq!(encrypt(&off_curve, &[0u8; 16], b"x"), Err(EceError::BadPublicKey));
        assert_eq!(encrypt(&ua_public[..33], &[0u8; 16], b"x"), Err(EceError::BadPublicKey));
        let big = vec![b'a'; MAX_PLAINTEXT + 1];
        assert_eq!(encrypt(&ua_public, &[0u8; 16], &big), Err(EceError::TooLarge));
        let max = vec![b'a'; MAX_PLAINTEXT];
        assert_eq!(encrypt(&ua_public, &[0u8; 16], &max).unwrap().len(), 4096);
    }
}
