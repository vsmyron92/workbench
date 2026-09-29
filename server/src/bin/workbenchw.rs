//! `workbenchw.exe`: Workbench's Windows launcher, what the sign-in entry and the Start Menu
//! shortcut of `workbench service install` run (docs/windows-port.md §2). A GUI program, so
//! neither shows a console window; it hands over to `workbench.exe` in its own folder, which
//! runs without one:
//!
//! * `workbenchw [--name N]` starts the service's supervisor, `workbench service run`, and
//!   exits;
//! * `workbenchw [--name N] open` runs `workbench service open` (start the service if needed,
//!   then open a signed-in window) and shows its error in a message box.
//!
//! This binary cannot use the server's modules (the package has no library), so everything
//! else happens in `workbench.exe` (`platform/service_windows.rs`). Elsewhere it is a stub.

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    std::process::exit(win::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("workbenchw is Workbench's Windows launcher; here, run `workbench` (see `workbench service --help`).");
    std::process::exit(2);
}

#[cfg(windows)]
mod win {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};

    const USAGE: &str = "Usage: workbenchw [--name NAME] [open]\n\n\
        Without `open` it starts Workbench's service; with it, it opens a signed-in Workbench window, \
        starting the service first when it does not run.";

    pub fn main() -> i32 {
        let (name, open) = match parse(std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned())) {
            Ok(a) => a,
            Err(e) => {
                message_box(&format!("{e}\n\n{USAGE}"));
                return 2;
            }
        };
        let exe = match std::env::current_exe() {
            Ok(me) => me.with_file_name("workbench.exe"),
            Err(e) => {
                message_box(&format!("Cannot find this program's folder: {e}"));
                return 1;
            }
        };
        if !exe.is_file() {
            message_box(&format!("{} was not found. workbenchw.exe runs the workbench.exe in its own folder.", exe.display()));
            return 1;
        }
        let mut cmd = Command::new(&exe);
        cmd.args(["service", if open { "open" } else { "run" }]);
        if let Some(name) = &name {
            cmd.args(["--name", name]);
        }
        // No console window; not the caller's folder, which would stay in use.
        cmd.creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::null());
        if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|p| p.is_dir()) {
            cmd.current_dir(home);
        }
        if !open {
            // The supervisor keeps running; its own log tells what happens next.
            return match cmd.stderr(Stdio::null()).spawn() {
                Ok(_) => 0,
                Err(e) => {
                    message_box(&format!("Cannot start {}: {e}", exe.display()));
                    1
                }
            };
        }
        let mut child = match cmd.stderr(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                message_box(&format!("Cannot start {}: {e}", exe.display()));
                return 1;
            }
        };
        // Read on a thread: the browser `workbench open` starts may inherit the pipe and keep
        // it open, so its end never comes; what the child wrote arrives before it exits.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        if let Some(mut err) = child.stderr.take() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = err.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
        }
        let status = match child.wait() {
            Ok(s) => s,
            Err(e) => {
                message_box(&format!("Cannot wait for {}: {e}", exe.display()));
                return 1;
            }
        };
        let mut text = vec![];
        while let Ok(chunk) = rx.recv_timeout(Duration::from_millis(300)) {
            text.extend(chunk);
        }
        if status.success() {
            return 0;
        }
        let text = String::from_utf8_lossy(&text);
        let text = text.trim();
        let code = status.code().map_or_else(|| "?".into(), |c| c.to_string());
        message_box(&if text.is_empty() { format!("Workbench could not open (exit code {code}).") } else { last_lines(text, 20) });
        1
    }

    /// `[--name NAME] [open]`, in any order.
    fn parse(mut args: impl Iterator<Item = String>) -> Result<(Option<String>, bool), String> {
        let (mut name, mut open) = (None, false);
        while let Some(a) = args.next() {
            match a.as_str() {
                "open" => open = true,
                "--name" => name = Some(args.next().ok_or("--name needs a value")?),
                _ => return Err(format!("Unknown argument {a:?}.")),
            }
        }
        Ok((name, open))
    }

    fn last_lines(text: &str, n: usize) -> String {
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(n)..].join("\n")
    }

    fn message_box(text: &str) {
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (text, title) = (wide(text), wide("Workbench"));
        // SAFETY: no owner window; both strings are NUL-terminated and outlive the call.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR | MB_SETFOREGROUND) };
    }
}
