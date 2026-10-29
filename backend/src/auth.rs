//! Credentials: operator passwords (Argon2id), session and robot tokens (256-bit random, stored as
//! SHA-256), TURN REST credentials for coturn `use-auth-secret`.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::hub::RobotId;

pub const SESSION_COOKIE: &str = "__Host-aerolink";
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
pub const TURN_TTL_SECS: i64 = 24 * 3600;

/// Argon2id with the crate defaults (19 MiB, t=2, p=1, OWASP minimum): ~20–40 ms on one core. That
/// briefly stalls the single runtime thread, which is why nginx rate-limits `/api/v1/login` hard.
pub fn hash_password(password: &str) -> String {
    let salt = SaltString::encode_b64(&random_bytes::<16>()).expect("16-byte salt");
    Argon2::default().hash_password(password.as_bytes(), &salt).expect("argon2 with valid params").to_string()
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// Burn the same time as a real check, so unknown e-mails cannot be told apart by timing.
pub fn verify_dummy(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    verify_password(password, DUMMY.get_or_init(|| hash_password("dummy")));
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).expect("OS random source");
    buf
}

pub fn new_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Robot token as given to the phone: `<robot id>.<secret>`.
pub fn robot_token(id: RobotId, secret: &str) -> String {
    format!("{id}.{secret}")
}

pub fn parse_robot_token(token: &str) -> Option<(RobotId, &str)> {
    let (id, secret) = token.split_once('.')?;
    if secret.is_empty() {
        return None;
    }
    Some((id.parse().ok()?, secret))
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Value of cookie `name` from a `Cookie` header.
pub fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').filter_map(|kv| kv.trim().split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    // `__Host-` requires Secure; plain-http development falls back to a normal name.
    let (name, secure) = if secure { (SESSION_COOKIE, "; Secure") } else { (cookie_name(false), "") };
    format!("{name}={token}; Path=/; Max-Age={SESSION_TTL_SECS}; HttpOnly; SameSite=Strict{secure}")
}

pub fn clear_cookie(secure: bool) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!("{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{secure_attr}", cookie_name(secure))
}

pub fn cookie_name(secure: bool) -> &'static str {
    if secure { SESSION_COOKIE } else { "aerolink" }
}

/// coturn REST API credentials: username `<expiry>:<user>`, password `base64(HMAC-SHA1(secret, username))`.
pub fn turn_credentials(secret: &str, user: &str, now: i64) -> (String, String) {
    let username = format!("{}:{user}", now + TURN_TTL_SECS);
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(username.as_bytes());
    (username, STANDARD.encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords() {
        let h = hash_password("correct horse");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong", &h));
        assert!(!verify_password("x", "not a hash"));
    }

    #[test]
    fn robot_tokens() {
        assert_eq!(parse_robot_token("12.abc"), Some((12, "abc")));
        assert_eq!(parse_robot_token("12."), None);
        assert_eq!(parse_robot_token("x.abc"), None);
        assert_eq!(parse_robot_token("abc"), None);
        assert_eq!(new_token().len(), 43);
    }

    #[test]
    fn cookies() {
        assert_eq!(cookie("a=1; __Host-aerolink=tok; b=2", SESSION_COOKIE), Some("tok"));
        assert_eq!(cookie("a=1", SESSION_COOKIE), None);
        assert!(session_cookie("t", true).starts_with("__Host-aerolink=t; Path=/;"));
        assert!(session_cookie("t", true).ends_with("Secure"));
        assert!(!session_cookie("t", false).contains("Secure"));
    }

    #[test]
    fn turn_matches_openssl_reference() {
        // echo -n "1000086400:op7" | openssl dgst -sha1 -hmac secret -binary | base64
        let (user, pass) = turn_credentials("secret", "op7", 1_000_000_000);
        assert_eq!(user, "1000086400:op7");
        assert_eq!(pass, "1Bjga6aTEeHB2ymn5Lax0WqFv+c=");
    }
}
