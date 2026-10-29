//! Configuration: `KEY=value` lines from `/etc/aerolink/aerolink.env` (path overridable with
//! `AEROLINK_CONFIG`), overridden by real environment variables. The service and the CLI read the same
//! file, so `sudo -u aerolink aerolink robot add …` works without extra flags. The file holds secrets
//! (Telegram token, TURN secret): it is 0640 root:aerolink and never in git.

use std::collections::HashMap;
use std::path::PathBuf;

pub const DEFAULT_CONFIG: &str = "/etc/aerolink/aerolink.env";

#[derive(Clone, Debug)]
pub struct Config {
    /// `unix:/path`, `host:port`, or `systemd` (socket activation, default when LISTEN_FDS is set).
    pub listen: String,
    pub database: PathBuf,
    /// Errors are appended here (and sent to Telegram).
    pub error_log: PathBuf,
    /// Public origin of the console, e.g. `https://aerolink.example.com`. Operator WebSockets from any
    /// other Origin are rejected (CSWSH). Empty = no check (development only).
    pub origin: String,
    /// `Secure` session cookies; off only for plain-http development.
    pub secure_cookies: bool,
    pub telegram_token: String,
    pub telegram_chat_id: String,
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    /// coturn `static-auth-secret` (`use-auth-secret`), for time-limited TURN credentials.
    pub turn_secret: String,
    pub log_filter: String,
    /// Where `aerolink guard` receives nginx's security log (syslog datagrams): `systemd` or `unix:/path`.
    pub guard_listen: String,
    /// Addresses / networks the guard never bans (loopback is always allowed).
    pub guard_allow: Vec<String>,
    /// The `nft` binary (a fake one in tests).
    pub guard_nft: String,
    /// Robot app release: APKs + `aerolink-release.json` (ROB-008). Readable by nginx.
    pub apk_dir: PathBuf,
    /// nginx `internal` location serving `apk_dir` (`X-Accel-Redirect`); empty = the backend sends the file
    /// itself (bench, no nginx).
    pub apk_accel: String,
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let path = std::env::var("AEROLINK_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG.to_owned());
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && std::env::var_os("AEROLINK_CONFIG").is_none() => {
                HashMap::new()
            }
            Err(e) => return Err(format!("{path}: {e}")),
        };
        Ok(Self::from_lookup(|key| std::env::var(key).ok().or_else(|| file.get(key).cloned())))
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let str_or = |key: &str, default: &str| get(key).unwrap_or_else(|| default.to_owned());
        let list = |key: &str| {
            get(key)
                .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
                .unwrap_or_default()
        };
        Config {
            listen: get("AEROLINK_LISTEN").unwrap_or_else(|| {
                if std::env::var_os("LISTEN_FDS").is_some() { "systemd".into() } else { "127.0.0.1:8080".into() }
            }),
            database: str_or("AEROLINK_DATABASE", "/var/lib/aerolink/aerolink.db").into(),
            error_log: str_or("AEROLINK_ERROR_LOG", "/var/log/aerolink/error.log").into(),
            origin: str_or("AEROLINK_ORIGIN", ""),
            secure_cookies: str_or("AEROLINK_SECURE_COOKIES", "true") != "false",
            telegram_token: str_or("AEROLINK_TELEGRAM_TOKEN", ""),
            telegram_chat_id: str_or("AEROLINK_TELEGRAM_CHAT_ID", ""),
            stun_urls: list("AEROLINK_STUN_URLS"),
            turn_urls: list("AEROLINK_TURN_URLS"),
            turn_secret: str_or("AEROLINK_TURN_SECRET", ""),
            log_filter: str_or("AEROLINK_LOG", "info"),
            guard_listen: get("AEROLINK_GUARD_LISTEN").unwrap_or_else(|| {
                if std::env::var_os("LISTEN_FDS").is_some() { "systemd".into() } else { "unix:/run/aerolink-guard.sock".into() }
            }),
            guard_allow: list("AEROLINK_GUARD_ALLOW"),
            guard_nft: str_or("AEROLINK_GUARD_NFT", "nft"),
            apk_dir: str_or("AEROLINK_APK_DIR", "/var/lib/aerolink-apk").into(),
            apk_accel: str_or("AEROLINK_APK_ACCEL", ""),
        }
    }
}

/// `KEY=value` per line; `#` comments, blank lines and optional quotes around the value.
fn parse(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| {
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(v);
            (k.trim().to_owned(), v.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_file() {
        let map = parse("# c\n\nAEROLINK_ORIGIN = \"https://a.b\"\nAEROLINK_TURN_URLS=turn:x:3478, turns:x:5349\n");
        let cfg = Config::from_lookup(|k| map.get(k).cloned());
        assert_eq!(cfg.origin, "https://a.b");
        assert_eq!(cfg.turn_urls, ["turn:x:3478", "turns:x:5349"]);
        assert!(cfg.secure_cookies);
        assert!(cfg.stun_urls.is_empty());
    }
}
