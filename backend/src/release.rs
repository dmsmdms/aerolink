//! Robot app release (ROB-008, docs/08-backend.md "App updates"): `aerolink-release.json` in the APK directory,
//! written by `make android` next to the APKs:
//!
//! ```json
//! {"version_code": 2, "version_name": "0.2.0",
//!  "apks": {"armeabi-v7a": {"file": "aerolink-armeabi-v7a.apk", "size": 9123456, "sha256": "…"}}}
//! ```
//!
//! The release is read at start and on SIGHUP (`systemctl reload aerolink`) and announced to every robot; the
//! robot decides whether it is newer and downloads its ABI from `/api/v1/robot/apk/<abi>`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILE: &str = "aerolink-release.json";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Release {
    pub version_code: u32,
    pub version_name: String,
    /// By ABI (`armeabi-v7a`, `arm64-v8a`).
    pub apks: BTreeMap<String, Apk>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Apk {
    /// File name in the APK directory; robots only see the ABI.
    #[serde(skip_serializing)]
    pub file: String,
    pub size: u64,
    /// Lower-case hex.
    pub sha256: String,
}

/// The release in `dir`, `None` when there is none. A release whose files do not match it is an error:
/// robots would download something else than announced.
pub fn load(dir: &Path) -> Result<Option<Release>, String> {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let release: Release = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    for (abi, apk) in &release.apks {
        let name_ok = |s: &str| !s.is_empty() && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if !name_ok(abi) || !name_ok(&apk.file) {
            return Err(format!("{}: bad ABI or file name: {abi} {}", path.display(), apk.file));
        }
        if apk.sha256.len() != 64 || !apk.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(format!("{}: bad sha256 for {abi}", path.display()));
        }
        let size = std::fs::metadata(dir.join(&apk.file)).map_err(|e| format!("{}: {e}", apk.file))?.len();
        if size != apk.size {
            return Err(format!("{}: {size} bytes, the release says {}", apk.file, apk.size));
        }
    }
    Ok(Some(release))
}

/// Test helper: a fresh empty directory under the temp dir.
#[cfg(test)]
pub(crate) fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aerolink-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Test helper: a release with one arm64-v8a APK `a.apk` holding `body`.
#[cfg(test)]
pub(crate) fn write_test_release(dir: &Path, version_code: u32, body: &[u8]) {
    std::fs::write(dir.join("a.apk"), body).unwrap();
    let release = serde_json::json!({
        "version_code": version_code, "version_name": format!("0.{version_code}.0"),
        "apks": { "arm64-v8a": { "file": "a.apk", "size": body.len(), "sha256": "ab".repeat(32) } },
    });
    std::fs::write(dir.join(FILE), release.to_string()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_checks_the_files() {
        let dir = temp_dir("release-load");
        assert_eq!(load(&dir).unwrap(), None);
        write_test_release(&dir, 3, b"apk");
        let r = load(&dir).unwrap().unwrap();
        assert_eq!((r.version_code, r.apks["arm64-v8a"].size), (3, 3));
        // Robots are not told file names.
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["apks"]["arm64-v8a"], serde_json::json!({ "size": 3, "sha256": "ab".repeat(32) }));

        std::fs::write(dir.join("a.apk"), b"longer").unwrap();
        assert!(load(&dir).unwrap_err().contains("6 bytes"));
        std::fs::write(dir.join(FILE), r#"{"version_code":1,"version_name":"x","apks":{"x":{"file":"../a","size":1,"sha256":""}}}"#).unwrap();
        assert!(load(&dir).unwrap_err().contains("bad ABI or file name"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
