//! Small shared helpers. Anything here is used by more than one slice.

pub mod ansi;
pub mod fs;
pub mod git;
pub mod os;
pub mod paths;
pub mod proc;

/// Resolves on SIGINT or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

/// Open `url` in an app-style browser window when a Chromium browser is
/// installed (no tabs or address bar), otherwise in the default browser.
pub fn open_in_browser(url: &str) {
    for browser in ["google-chrome", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"] {
        if which(browser) {
            let ok = std::process::Command::new(browser)
                .arg(format!("--app={url}"))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok();
            if ok {
                return;
            }
        }
    }
    let _ = std::process::Command::new("xdg-open")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Whether `cmd` resolves on `PATH`.
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
