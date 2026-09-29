//! Small shared helpers. Anything here is used by more than one slice.

pub mod ansi;
pub mod fs;
pub mod git;
pub mod os;
pub mod paths;
pub mod proc;

/// Open `url` in an app-style browser window when a Chromium browser is
/// installed (no tabs or address bar), otherwise in the default browser.
pub fn open_in_browser(url: &str) {
    os::desktop::open_url(url)
}

/// Whether `cmd` resolves on `PATH` (`os::exe::which`, which code in `util::os` calls
/// itself).
pub fn which(cmd: &str) -> bool {
    which_path(cmd).is_some()
}

/// The program file `cmd` names (`os::exe::which`).
pub fn which_path(cmd: &str) -> Option<std::path::PathBuf> {
    os::exe::which(cmd)
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A random URL-safe token of `bytes` random bytes.
pub fn random_token(bytes: usize) -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Lowercase slug for ids: letters, digits and `-`.
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}
