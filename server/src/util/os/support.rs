//! What Workbench leaves out, or offers only as experimental, on this OS
//! (docs/windows-port.md §5): `GET /api/health` reports it (`os`, `unsupported`,
//! `experimental`), so the UI hides what cannot work, and the routes and MCP tools of an
//! unsupported feature answer `ApiError::unsupported` (HTTP 501, `unsupported_platform`)
//! with the same reason. Everything is supported on Linux.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::{ApiError, ApiResult};

/// A feature whose support depends on the OS. `key` names it in the health report and
/// in an `unsupported_platform` error's `feature`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Dev containers: building and starting them, and terminals, runs and agents inside.
    Devcontainer,
    /// Desktop notifications on the computer Workbench runs on (`[notify] desktop`).
    DesktopNotifications,
    /// Attaching gdb to a running process (other adapters may still attach).
    GdbAttach,
    /// rust-gdb's pretty printers in gdb sessions of Rust programs.
    RustGdbPrettyPrinters,
    /// Project roots on a network path or inside WSL (`\\server\share`, `\\wsl$`).
    NetworkRoots,
    /// The Services tool window (this computer's Docker containers and images).
    Services,
}

impl Feature {
    pub const ALL: [Feature; 6] =
        [Feature::Devcontainer, Feature::DesktopNotifications, Feature::GdbAttach, Feature::RustGdbPrettyPrinters, Feature::NetworkRoots, Feature::Services];

    pub fn key(self) -> &'static str {
        match self {
            Feature::Devcontainer => "devcontainer",
            Feature::DesktopNotifications => "desktopNotifications",
            Feature::GdbAttach => "gdbAttach",
            Feature::RustGdbPrettyPrinters => "rustGdbPrettyPrinters",
            Feature::NetworkRoots => "networkRoots",
            Feature::Services => "services",
        }
    }
}

/// How far a feature works on an OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Supported,
    /// It works but has not been tested here; the note says so.
    Experimental(&'static str),
    /// It does not work here; the reason says why and what to do instead.
    Unsupported(&'static str),
}

/// The OS as the health report names it: `linux`, `windows`, `macos`
/// (`std::env::consts::OS`).
pub fn os() -> &'static str {
    std::env::consts::OS
}

/// How far `f` works on this OS.
pub fn support(f: Feature) -> Support {
    support_on(os(), f)
}

/// Why `f` does not work on this OS; `None` where it does (experimental included).
pub fn unsupported(f: Feature) -> Option<&'static str> {
    match support(f) {
        Support::Unsupported(why) => Some(why),
        _ => None,
    }
}

/// `Err(unsupported_platform)` when `f` does not work on this OS.
pub fn require(f: Feature) -> ApiResult<()> {
    match unsupported(f) {
        Some(why) => Err(ApiError::unsupported(f.key(), why)),
        None => Ok(()),
    }
}

/// `Err(unsupported_platform)` (feature `networkRoots`) for a project root this OS does
/// not serve: UNC and WSL paths on Windows (`os::path::unsupported_root`, whose message
/// names the one that applies). Check before touching the path: opening a network path
/// connects to its server.
pub fn require_root(root: &Path) -> ApiResult<()> {
    match super::path::unsupported_root(root) {
        Some(why) => Err(ApiError::unsupported(Feature::NetworkRoots.key(), why)),
        None => Ok(()),
    }
}

/// `{key: reason}` of every feature that does not work on this OS.
pub fn unsupported_all() -> BTreeMap<&'static str, &'static str> {
    Feature::ALL.into_iter().filter_map(|f| unsupported(f).map(|why| (f.key(), why))).collect()
}

/// `{key: note}` of every feature that is experimental on this OS.
pub fn experimental_all() -> BTreeMap<&'static str, &'static str> {
    Feature::ALL
        .into_iter()
        .filter_map(|f| match support(f) {
            Support::Experimental(note) => Some((f.key(), note)),
            _ => None,
        })
        .collect()
}

const WIN_DEVCONTAINER: &str = "dev containers are not supported on Windows yet: Workbench cannot reach agents inside Docker Desktop's VM \
     (a container network's gateway is not on this computer), and the workspace mount has no user-id mapping";
const WIN_NOTIFICATIONS: &str = "desktop notifications are not supported on Windows yet (they need a Start Menu shortcut with an \
     AppUserModelID): turn on browser notifications (Settings › General) or push instead";
const WIN_GDB_ATTACH: &str = "attaching gdb to a running process is not supported on Windows yet: native programs attach with \
     lldb-dap or CodeLLDB, Python with debugpy";
const WIN_RUST_GDB: &str =
    "rust-gdb's pretty printers are not loaded on Windows yet, so gdb shows Rust values (Vec, String, Option…) unformatted";
// The same rule as `os::path::unsupported_root`, whose messages name the one that applies.
const WIN_NETWORK_ROOTS: &str = "projects on network paths (\\\\server\\share) or inside WSL (\\\\wsl$) are not supported on Windows: \
     clone the repository to a local drive, or run Workbench inside WSL for them";
const WIN_SERVICES: &str = "the Services tool window is experimental on Windows: it has not been tested with Docker Desktop yet";

/// The table, by OS name, so every OS's entries can be tested anywhere.
fn support_on(os: &str, f: Feature) -> Support {
    use Feature::*;
    match (os, f) {
        ("windows", Devcontainer) => Support::Unsupported(WIN_DEVCONTAINER),
        ("windows", DesktopNotifications) => Support::Unsupported(WIN_NOTIFICATIONS),
        ("windows", GdbAttach) => Support::Unsupported(WIN_GDB_ATTACH),
        ("windows", RustGdbPrettyPrinters) => Support::Unsupported(WIN_RUST_GDB),
        ("windows", NetworkRoots) => Support::Unsupported(WIN_NETWORK_ROOTS),
        ("windows", Services) => Support::Experimental(WIN_SERVICES),
        _ => Support::Supported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_supports_everything() {
        for f in Feature::ALL {
            assert_eq!(support_on("linux", f), Support::Supported, "{f:?}");
        }
        if cfg!(target_os = "linux") {
            assert_eq!(os(), "linux");
            assert!(unsupported_all().is_empty() && experimental_all().is_empty());
            assert!(Feature::ALL.into_iter().all(|f| require(f).is_ok()));
        }
    }

    #[test]
    fn windows_leaves_out_the_first_versions_features() {
        let unsupported: Vec<&str> = Feature::ALL
            .into_iter()
            .filter(|f| matches!(support_on("windows", *f), Support::Unsupported(_)))
            .map(Feature::key)
            .collect();
        assert_eq!(unsupported, ["devcontainer", "desktopNotifications", "gdbAttach", "rustGdbPrettyPrinters", "networkRoots"]);
        assert!(matches!(support_on("windows", Feature::Services), Support::Experimental(n) if n.contains("experimental")));
        // Each reason says what does not work on Windows, in one line.
        for f in Feature::ALL {
            if let Support::Unsupported(why) | Support::Experimental(why) = support_on("windows", f) {
                assert!(why.contains("Windows") && !why.contains('\n') && !why.contains("  ") && !why.contains(" \\ "), "{f:?}: {why}");
            }
        }
        assert!(matches!(support_on("windows", Feature::NetworkRoots), Support::Unsupported(w) if w.contains(r"(\\server\share)") && w.contains(r"(\\wsl$)")));
        // Keys are distinct (they are JSON keys).
        let mut keys: Vec<&str> = Feature::ALL.into_iter().map(Feature::key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), Feature::ALL.len());
    }

    #[test]
    fn local_roots_are_served() {
        let dir = tempfile::tempdir().unwrap();
        assert!(require_root(dir.path()).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn this_windows_build_reports_them() {
        assert_eq!(os(), "windows");
        assert_eq!(unsupported_all().len(), 5);
        assert_eq!(experimental_all().keys().copied().collect::<Vec<_>>(), ["services"]);
        let e = require(Feature::Devcontainer).unwrap_err();
        assert_eq!((e.status.as_u16(), e.code, e.feature), (501, "unsupported_platform", Some("devcontainer")));
        for (root, says) in [(r"\\wsl$\Ubuntu\home\u\proj", "WSL"), (r"\\server\share\proj", "network")] {
            let e = require_root(Path::new(root)).unwrap_err();
            assert_eq!((e.code, e.feature), ("unsupported_platform", Some("networkRoots")));
            assert!(e.message.contains(says), "{}", e.message);
        }
    }
}
