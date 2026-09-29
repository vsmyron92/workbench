//! Operating-system specifics behind one interface, so Workbench runs on Linux and Windows
//! (docs/windows-port.md). Each area keeps today's Unix behaviour under `cfg(unix)` and has
//! its Windows counterpart under `cfg(windows)`. Code outside this module calls these
//! functions instead of using Unix or Windows APIs itself (tests excepted).

#[cfg(windows)]
pub mod autostart;
pub mod desktop;
pub mod dll;
pub mod exe;
pub mod fs;
pub mod helper;
pub mod net;
pub mod path;
pub mod perm;
pub mod proc;
pub mod session;
pub mod shell;
pub mod support;
pub mod watch;
#[cfg(windows)]
mod win32;
