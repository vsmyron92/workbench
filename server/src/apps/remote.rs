//! Where environment commands (logs, custom commands, version probes, deploys) run:
//! over ssh on the env's host, or locally.
//!
//! * `env.host` names a `[hosts.<name>]` entry → `ssh -o BatchMode=yes [-i key] [-p port] user@host '<cmd>'`.
//!   BatchMode means ssh never prompts (no password or host-key questions in a PTY
//!   nobody watches); the command is one argv element, so no local shell sees it.
//! * No `env.host` (or `host = "local"` without such a `[hosts]` entry) → the run shell
//!   (`bash -lc '<cmd>'`; PowerShell on Windows) in the project root. Deploys additionally
//!   require `deploy.local = true` to run locally, so a missing host can never turn a
//!   remote deploy into a local one.

use crate::config::expand_tilde;
use crate::config::project::{Environment, SshHost};
use crate::error::ApiError;
use crate::projects::Project;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Local,
    Ssh(SshHost),
}

impl Target {
    /// Human form for the UI: `root@203.0.113.10` or `local`.
    pub fn label(&self) -> String {
        match self {
            Target::Local => "local".into(),
            Target::Ssh(h) if h.port != 22 => format!("{}@{}:{}", h.user, h.host, h.port),
            Target::Ssh(h) => format!("{}@{}", h.user, h.host),
        }
    }
}

/// The target of an env's logs / commands / version probe.
pub fn env_target(project: &Project, env: &Environment) -> Result<Target, ApiError> {
    match env.host.as_deref() {
        None => Ok(Target::Local),
        Some(name) => match project.config.hosts.get(name) {
            Some(h) => {
                validate_host(h)?;
                Ok(Target::Ssh(h.clone()))
            }
            None if name == "local" || name == "localhost" => Ok(Target::Local),
            None => Err(ApiError::not_configured(format!(
                "environment {:?} uses host {name:?}, which is not defined; add [hosts.{name}] to ~/.config/workbench/projects/{}.toml",
                env.name, project.id
            ))),
        },
    }
}

/// The target of a deploy: local only when `deploy.local = true`.
pub fn deploy_target(project: &Project, env: &Environment, local: bool) -> Result<Target, ApiError> {
    if local {
        return Ok(Target::Local);
    }
    match env_target(project, env)? {
        Target::Local => Err(ApiError::not_configured(format!(
            "environment {:?} has no ssh host for its deploy; set `host` (a [hosts] entry) or `deploy.local = true`",
            env.name
        ))),
        t => Ok(t),
    }
}

fn validate_host(h: &SshHost) -> Result<(), ApiError> {
    let ok_user = !h.user.is_empty() && h.user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) && !h.user.starts_with('-');
    let ok_host = !h.host.is_empty() && h.host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)) && !h.host.starts_with('-');
    if ok_user && ok_host {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!("invalid ssh user/host {:?}@{:?}", h.user, h.host)))
    }
}

/// `ssh -o BatchMode=yes … user@host <cmd>`. `tty` requests a remote PTY (`-t`) so
/// that closing the terminal ends a following command (`docker logs -f`).
pub fn ssh_argv(h: &SshHost, cmd: &str, tty: bool) -> Vec<String> {
    let mut v: Vec<String> = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "-o", "ServerAliveInterval=30"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if tty {
        v.push("-t".into());
    }
    if let Some(key) = h.identity_file.as_deref().filter(|k| !k.is_empty()) {
        v.push("-i".into());
        v.push(expand_tilde(key).to_string_lossy().into_owned());
    }
    if h.port != 22 {
        v.push("-p".into());
        v.push(h.port.to_string());
    }
    v.push("--".into());
    v.push(format!("{}@{}", h.user, h.host));
    v.push(cmd.to_string());
    v
}

/// argv for running `cmd` on `target`.
pub fn argv(target: &Target, cmd: &str, tty: bool) -> Vec<String> {
    match target {
        Target::Local => crate::util::os::shell::run_argv(cmd),
        Target::Ssh(h) => ssh_argv(h, cmd, tty),
    }
}

/// The language of the shell `argv` gives a command on `target`: the local run shell's
/// (`Dialect::HOST`), or POSIX on an ssh host whatever OS Workbench runs on.
pub fn dialect(target: &Target) -> crate::util::os::shell::Dialect {
    match target {
        Target::Local => crate::util::os::shell::Dialect::HOST,
        Target::Ssh(_) => crate::util::os::shell::Dialect::Posix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> SshHost {
        SshHost { host: "203.0.113.10".into(), user: "root".into(), port: 22, identity_file: Some("~/.ssh/deploy_key".into()) }
    }

    #[test]
    fn builds_batch_mode_ssh_argv() {
        let a = ssh_argv(&host(), "docker logs -f --tail 300 shop-api-1", true);
        let home = dirs::home_dir().unwrap();
        assert_eq!(a[0], "ssh");
        assert!(a.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(a.contains(&"-t".to_string()));
        assert!(a.windows(2).any(|w| w[0] == "-i" && w[1] == home.join(".ssh/deploy_key").to_string_lossy()));
        assert!(!a.contains(&"-p".to_string()));
        assert_eq!(&a[a.len() - 3..], ["--", "root@203.0.113.10", "docker logs -f --tail 300 shop-api-1"]);

        let mut h = host();
        h.port = 2222;
        h.identity_file = None;
        let a = ssh_argv(&h, "uptime", false);
        assert!(a.windows(2).any(|w| w == ["-p", "2222"]));
        assert!(!a.contains(&"-t".to_string()) && !a.contains(&"-i".to_string()));
    }

    #[test]
    fn local_commands_run_through_bash() {
        assert_eq!(argv(&Target::Local, "echo hi", true), crate::util::os::shell::run_argv("echo hi"));
        #[cfg(unix)]
        assert_eq!(argv(&Target::Local, "echo hi", true), vec!["bash", "-lc", "echo hi"]);
        assert_eq!(Target::Ssh(host()).label(), "root@203.0.113.10");
        assert_eq!(dialect(&Target::Local), crate::util::os::shell::Dialect::HOST);
        assert_eq!(dialect(&Target::Ssh(host())), crate::util::os::shell::Dialect::Posix);
    }

    #[test]
    fn rejects_option_injection_in_hosts() {
        let mut h = host();
        h.host = "-oProxyCommand=evil".into();
        assert!(validate_host(&h).is_err());
        let mut h = host();
        h.user = "root x".into();
        assert!(validate_host(&h).is_err());
        assert!(validate_host(&host()).is_ok());
    }
}
