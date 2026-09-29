//! Desktop integration: opening the UI in a browser, and the desktop notification
//! program (docs/windows-port.md §1.L).

use std::path::PathBuf;

/// Open `url` in an app-style browser window when a Chromium browser is
/// installed (no tabs or address bar), otherwise in the default browser.
pub fn open_url(url: &str) {
    sys::open_url(url)
}

/// The program that shows desktop notifications (`notify-send`), when there is one.
/// Always `None` on Windows, whose notifications need an AppUserModelID shortcut: the
/// desktop channel reports "unavailable" there, as it does without notify-send.
pub fn notify_send() -> Option<PathBuf> {
    sys::notify_send()
}

#[cfg(unix)]
mod sys {
    use std::path::PathBuf;

    use super::super::exe::which;

    pub fn open_url(url: &str) {
        for browser in ["google-chrome", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"] {
            if which(browser).is_some() {
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

    pub fn notify_send() -> Option<PathBuf> {
        which("notify-send")
    }
}

#[cfg(windows)]
mod sys {
    use std::path::PathBuf;
    use std::ptr;

    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };
    use windows_sys::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use crate::util::os::win32::wide;

    /// Chromium browsers by their App Paths names, in the order Linux tries them.
    const BROWSERS: &[&str] = &["chrome.exe", "msedge.exe", "brave.exe"];

    /// The browser gets the URL as an argument of its own, never through `cmd /c start`,
    /// which would treat `&` in the URL as a command separator.
    pub fn open_url(url: &str) {
        // ShellExecute runs programs as readily as it opens pages: only web URLs go there.
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            // The URL itself is not logged: it may carry a one-time sign-in code.
            tracing::warn!("not opening a browser for a URL that is not http(s)");
            return;
        }
        for browser in BROWSERS.iter().filter_map(|exe| app_path(exe)) {
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
        shell_open(url);
    }

    pub fn notify_send() -> Option<PathBuf> {
        None
    }

    /// The executable registered under App Paths for `exe`: per user first, then per
    /// machine, where 32-bit installers (Edge's among them) write to the 32-bit view.
    fn app_path(exe: &str) -> Option<PathBuf> {
        let key = format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
        [
            (HKEY_CURRENT_USER, RRF_SUBKEY_WOW6464KEY),
            (HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6464KEY),
            (HKEY_LOCAL_MACHINE, RRF_SUBKEY_WOW6432KEY),
        ]
        .into_iter()
        .filter_map(|(root, view)| default_value(root, &key, view))
        .map(|v| PathBuf::from(v.trim().trim_matches('"')))
        .find(|p| p.is_file())
    }

    /// A registry key's default string value (REG_EXPAND_SZ expanded), read in the
    /// registry `view` (RRF_SUBKEY_WOW64…).
    fn default_value(root: HKEY, key: &str, view: u32) -> Option<String> {
        let key = wide(key);
        let mut buf = vec![0u16; 512];
        for _ in 0..3 {
            let mut bytes = (buf.len() * 2) as u32;
            // SAFETY: key is NUL-terminated, a null value name reads the default value,
            // and buf is writable for `bytes` bytes.
            let rc = unsafe {
                RegGetValueW(root, key.as_ptr(), ptr::null(), RRF_RT_REG_SZ | view, ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes)
            };
            match rc {
                ERROR_SUCCESS => {
                    let got = &buf[..(bytes as usize / 2).min(buf.len())];
                    let end = got.iter().position(|&c| c == 0).unwrap_or(got.len());
                    return Some(String::from_utf16_lossy(&got[..end]));
                }
                ERROR_MORE_DATA => buf = vec![0u16; (bytes as usize).div_ceil(2) + 1],
                _ => return None,
            }
        }
        None
    }

    /// The default browser, through the shell. On a thread of its own, because shell
    /// handlers may need a single-threaded COM apartment; joined, and without the async
    /// hand-off, so `workbench open` does not exit before the browser has the URL.
    fn shell_open(url: &str) {
        let file = wide(url);
        let opened = std::thread::spawn(move || {
            let verb = wide("open");
            // SAFETY: COM is initialised for this thread only and uninitialised when that
            // succeeded; the strings are NUL-terminated and outlive the call.
            unsafe {
                let com = CoInitializeEx(ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
                let mut info = SHELLEXECUTEINFOW {
                    cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
                    fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
                    lpVerb: verb.as_ptr(),
                    lpFile: file.as_ptr(),
                    nShow: SW_SHOWNORMAL,
                    ..Default::default()
                };
                let ok = ShellExecuteExW(&mut info) != 0;
                if !ok {
                    tracing::warn!("cannot open a browser: {}", std::io::Error::last_os_error());
                }
                if com >= 0 {
                    CoUninitialize();
                }
            }
        });
        let _ = opened.join();
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn app_paths_name_existing_programs() {
            assert_eq!(super::app_path("no-such-program-for-workbench.exe"), None);
            // windows-latest has Edge; a machine without it just has nothing to check.
            if let Some(edge) = super::app_path("msedge.exe") {
                assert!(edge.is_file(), "{edge:?}");
            }
        }
    }
}
