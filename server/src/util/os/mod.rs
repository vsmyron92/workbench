//! Operating-system specifics behind one interface, so Workbench runs on Linux and Windows
//! (docs/windows-port.md). Each area keeps today's Unix behaviour under `cfg(unix)` and has
//! its Windows counterpart under `cfg(windows)`. Code outside this module calls these
//! functions instead of using Unix or Windows APIs itself (tests excepted).

pub mod desktop;
pub mod exe;
pub mod fs;
pub mod net;
pub mod path;
pub mod perm;
pub mod proc;
pub mod shell;
pub mod support;
#[cfg(windows)]
mod win32;
