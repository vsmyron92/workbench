//! Running a terminal's command inside the project's dev container:
//! `docker exec -it -u <user> -w <mapped cwd> <container> /bin/sh -c <wrapper> …`
//! inside the host PTY, so snapshots, resize (the docker CLI forwards SIGWINCH) and
//! input work as for any terminal.
//!
//! Environment values never go into argv: the docker CLI's own environment carries
//! them as `WB_E_<NAME>` (passed with `-e WB_E_<NAME>`, which docker reads from its
//! environment), and a small POSIX `sh` wrapper in the container renames them before
//! `exec`ing the command. `remoteEnv` values that refer to the container's environment
//! (`${containerEnv:PATH}:/opt/bin`) travel as `WB_T_<NAME>` templates whose literal
//! parts are single-quoted, so evaluating them only expands the variables they name.
//!
//! Login shells (`bash -l`, a run's `bash -lc`) read `/etc/profile`, which on Debian
//! resets `PATH`: for them the variables are applied *after* the profile (`APPLY` at
//! the start of the `-c` script, then `exec <shell> -i` for an interactive shell), and
//! `PATH` entries the image's `ENV` had and the profile dropped are appended again.
//!
//! Killing the docker CLI does not end the process in the container, so the wrapper
//! records its pid (the exec'd process leads its own session and process group under
//! `docker exec -t`) and `kill_inside` signals that group.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use super::config::valid_env_name;

/// Applies the transported variables (and unsets the transport).
pub const APPLY: &str = r#"for n in ${WB_TPL_NAMES-}; do eval "t=\${WB_T_$n}"; eval "export $n=$t"; unset "WB_T_$n"; done; for n in ${WB_ENV_NAMES-}; do eval "v=\${WB_E_$n}"; export "$n=$v"; unset "WB_E_$n"; done; for n in ${WB_UNSET_NAMES-}; do unset "$n"; done; if [ -n "${WB_PATH0-}" ]; then o=$IFS; IFS=:; for d in $WB_PATH0; do case ":$PATH:" in *":$d:"*) ;; *) PATH="$PATH:$d";; esac; done; IFS=$o; export PATH; fi; unset WB_TPL_NAMES WB_ENV_NAMES WB_UNSET_NAMES WB_PATH0 WB_LOGIN n v t o d"#;

/// The in-container wrapper (`/bin/sh -c WRAPPER wb-exec argv…`): records the pid,
/// applies the variables (or leaves that to a login shell's script), `exec`s argv.
pub const WRAPPER: &str = r#"if [ -n "${WB_PIDFILE-}" ]; then echo $$ > "$WB_PIDFILE" 2>/dev/null; fi; unset WB_PIDFILE
if [ "${WB_LOGIN-}" = 1 ]; then export WB_PATH0="$PATH"; else
for n in ${WB_TPL_NAMES-}; do eval "t=\${WB_T_$n}"; eval "export $n=$t"; unset "WB_T_$n"; done
for n in ${WB_ENV_NAMES-}; do eval "v=\${WB_E_$n}"; export "$n=$v"; unset "WB_E_$n"; done
for n in ${WB_UNSET_NAMES-}; do unset "$n"; done
unset WB_TPL_NAMES WB_ENV_NAMES WB_UNSET_NAMES n v t
fi
exec "$@""#;

/// Shells whose `-l` / `-lc` Workbench rewrites to apply the variables after the profile.
fn posix_shell(prog: &str) -> bool {
    let name = prog.rsplit('/').next().unwrap_or(prog);
    matches!(name, "bash" | "sh" | "dash" | "ash" | "zsh" | "ksh" | "mksh")
}

/// `shell -l` → `shell -lc 'APPLY; exec shell -i'`; `shell -lc CMD` → `shell -lc
/// 'APPLY\nCMD'`. `None`: not a login shell (the wrapper applies the variables).
pub fn login_rewrite(argv: &[String]) -> Option<Vec<String>> {
    let prog = argv.first()?;
    if !posix_shell(prog) {
        return None;
    }
    match &argv[1..] {
        [l] if l == "-l" || l == "--login" => Some(vec![prog.clone(), "-lc".into(), format!("{APPLY}; exec {} -i", super::sh_quote(prog))]),
        [lc, cmd] if lc == "-lc" => Some(vec![prog.clone(), "-lc".into(), format!("{APPLY}\n{cmd}")]),
        [l, c, cmd] if (l == "-l" || l == "--login") && c == "-c" => Some(vec![prog.clone(), "-lc".into(), format!("{APPLY}\n{cmd}")]),
        _ => None,
    }
}

/// Where a terminal's process id is kept in the container.
pub fn pidfile(terminal_id: &str) -> String {
    format!("/tmp/.workbench-exec-{terminal_id}.pid")
}

/// A `remoteEnv` value as a shell word for the wrapper: `${containerEnv:X}` becomes
/// `"${X}"` (`"${X:-default}"`), everything else is single-quoted literal text.
pub fn template(value: &str) -> String {
    let mut out = String::new();
    let mut lit = String::new();
    let flush = |lit: &mut String, out: &mut String| {
        if !lit.is_empty() {
            out.push_str(&super::sh_quote_always(lit));
            lit.clear();
        }
    };
    let mut rest = value;
    while let Some(start) = rest.find("${containerEnv:") {
        lit.push_str(&rest[..start]);
        let after = &rest[start + "${containerEnv:".len()..];
        let Some(end) = after.find('}') else {
            lit.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let spec = &after[..end];
        let (name, default) = match spec.split_once(':') {
            Some((n, d)) => (n, Some(d)),
            None => (spec, None),
        };
        if valid_env_name(name) {
            flush(&mut lit, &mut out);
            match default {
                Some(d) => out.push_str(&format!("\"${{{name}:-{}}}\"", dq_escape(d))),
                None => out.push_str(&format!("\"${{{name}}}\"")),
            }
        } else {
            lit.push_str(&rest[start..start + "${containerEnv:".len() + end + 1]);
        }
        rest = &after[end + 1..];
    }
    lit.push_str(rest);
    flush(&mut lit, &mut out);
    if out.is_empty() { "''".into() } else { out }
}

/// Escape for the inside of a double-quoted shell word.
fn dq_escape(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

/// A project's running container, as terminals use it.
#[derive(Debug, Clone)]
pub struct ExecTarget {
    pub project_id: String,
    pub container_id: String,
    pub container_name: String,
    /// `-u`: remoteUser, else containerUser, else the image's default.
    pub user: Option<String>,
    /// Host directory ↔ container directory of the workspace mount.
    pub map: Option<(PathBuf, String)>,
    /// The workspace folder in the container (the default working directory).
    pub folder: String,
    /// `remoteEnv` (execution values; `None` unsets).
    pub remote_env: Vec<(String, Option<String>)>,
    /// `WORKBENCH_URL` inside: the bridge listener, when one runs.
    pub workbench_url: Option<String>,
    pub docker: String,
    /// The user's login shell in the container, and whether bash exists.
    pub shell: String,
    pub has_bash: bool,
}

impl ExecTarget {
    /// The container path of a host path under the workspace mount.
    pub fn map_path(&self, host: &Path) -> Option<String> {
        let (src, dst) = self.map.as_ref()?;
        let rel = host.strip_prefix(src).ok()?;
        let rel = rel.to_string_lossy();
        Some(if rel.is_empty() { dst.clone() } else { format!("{}/{}", dst.trim_end_matches('/'), rel) })
    }

    /// Replace host paths under the workspace mount in a variable's value (also each
    /// element of a `:`-separated list).
    pub fn map_value(&self, v: &str) -> String {
        let Some((src, _)) = &self.map else { return v.to_string() };
        let src_s = src.display().to_string();
        if !v.contains(&src_s) {
            return v.to_string();
        }
        v.split(':')
            .map(|part| {
                let p = Path::new(part);
                if p.is_absolute() { self.map_path(p).unwrap_or_else(|| part.to_string()) } else { part.to_string() }
            })
            .collect::<Vec<_>>()
            .join(":")
    }

    /// The docker command line and the docker CLI's environment for running `argv` in
    /// `cwd` with `env` (a terminal's launch environment) inside the container.
    pub fn wrap(
        &self,
        terminal_id: &str,
        argv: &[String],
        cwd: &Path,
        env: &[(String, Option<String>)],
    ) -> (Vec<String>, Vec<(String, Option<String>)>) {
        let workdir = self.map_path(cwd).unwrap_or_else(|| self.folder.clone());
        let mut host_env: Vec<(String, Option<String>)> = vec![];
        let mut literal: Vec<(String, String)> = vec![];
        let mut unset: Vec<String> = vec![];
        let mut templates: Vec<(String, String)> = vec![];
        for (k, v) in &self.remote_env {
            match v {
                Some(v) => templates.push((k.clone(), template(v))),
                None => unset.push(k.clone()),
            }
        }
        for (k, v) in env {
            if !valid_env_name(k) {
                continue;
            }
            match v {
                // Removals describe the host terminal; they mean nothing inside.
                None => host_env.push((k.clone(), None)),
                // The host's loopback URL is not reachable inside: the bridge's is.
                Some(_) if k == "WORKBENCH_URL" => {
                    if let Some(u) = &self.workbench_url {
                        literal.push((k.clone(), u.clone()));
                    }
                }
                Some(v) => {
                    // Terminal settings also describe the docker CLI's own terminal.
                    if matches!(k.as_str(), "TERM" | "COLORTERM") {
                        host_env.push((k.clone(), Some(v.clone())));
                    }
                    literal.push((k.clone(), self.map_value(v)));
                }
            }
        }
        // Later values win (a run's env over the terminal defaults).
        let mut seen = std::collections::HashSet::new();
        let mut lit_dedup: Vec<(String, String)> = vec![];
        for (k, v) in literal.into_iter().rev() {
            if seen.insert(k.clone()) {
                lit_dedup.push((k, v));
            }
        }
        lit_dedup.reverse();
        templates.retain(|(k, _)| !seen.contains(k));
        unset.retain(|k| !seen.contains(k));

        let mut a: Vec<String> = vec![self.docker.clone(), "exec".into(), "-i".into(), "-t".into()];
        // Ctrl+P is history in shells and a key in agent CLIs: no detach sequence.
        a.push("--detach-keys=ctrl-^,ctrl-^,ctrl-^".into());
        if let Some(u) = &self.user {
            a.extend(["-u".into(), u.clone()]);
        }
        a.extend(["-w".into(), workdir]);
        let mut names_env = |list: &str, prefix: &str, items: Vec<(String, String)>, a: &mut Vec<String>| {
            if items.is_empty() {
                return;
            }
            let names: Vec<String> = items.iter().map(|(k, _)| k.clone()).collect();
            host_env.push((list.into(), Some(names.join(" "))));
            a.extend(["-e".into(), list.into()]);
            for (k, v) in items {
                let var = format!("{prefix}{k}");
                a.extend(["-e".into(), var.clone()]);
                host_env.push((var, Some(v)));
            }
        };
        names_env("WB_TPL_NAMES", "WB_T_", templates, &mut a);
        names_env("WB_ENV_NAMES", "WB_E_", lit_dedup, &mut a);
        if !unset.is_empty() {
            host_env.push(("WB_UNSET_NAMES".into(), Some(unset.join(" "))));
            a.extend(["-e".into(), "WB_UNSET_NAMES".into()]);
        }
        host_env.push(("WB_PIDFILE".into(), Some(pidfile(terminal_id))));
        a.extend(["-e".into(), "WB_PIDFILE".into()]);
        let mut inner = argv.to_vec();
        // Run configurations use `bash -lc`; images without bash have sh.
        if !self.has_bash && inner.first().is_some_and(|p| p == "bash" || p.ends_with("/bash")) {
            inner[0] = "/bin/sh".into();
        }
        if let Some(login) = login_rewrite(&inner) {
            inner = login;
            host_env.push(("WB_LOGIN".into(), Some("1".into())));
            a.extend(["-e".into(), "WB_LOGIN".into()]);
        }
        a.push(self.container_id.clone());
        a.extend(["/bin/sh".into(), "-c".into(), WRAPPER.into(), "wb-exec".into()]);
        a.extend(inner);
        (a, host_env)
    }

    /// `TerminalInfo.meta.container`.
    pub fn describe(&self) -> Value {
        json!({
            "id": &self.container_id[..self.container_id.len().min(12)],
            "name": self.container_name,
            "user": self.user,
            "folder": self.folder,
            "docker": self.docker,
        })
    }
}

/// Stop the process group a wrapped terminal started in `container`: SIGHUP (shells
/// exit on it), SIGTERM, then SIGKILL. Best effort, bounded.
pub async fn kill_inside(docker: &str, container: &str, terminal_id: &str) {
    if !crate::terminals::valid_id(terminal_id) {
        return;
    }
    let f = pidfile(terminal_id);
    let script = r#"f="$1"; [ -f "$f" ] || exit 0; p=$(cat "$f"); rm -f "$f"
case "$p" in ""|*[!0-9]*) exit 0;; esac
alive() { kill -0 -"$p" 2>/dev/null; }
kill -HUP -"$p" 2>/dev/null || kill -HUP "$p" 2>/dev/null
i=0; while alive && [ $i -lt 5 ]; do sleep 0.1; i=$((i+1)); done
alive && kill -TERM -"$p" 2>/dev/null
i=0; while alive && [ $i -lt 20 ]; do sleep 0.1; i=$((i+1)); done
alive && kill -KILL -"$p" 2>/dev/null
exit 0"#;
    let _ = super::docker::exec(docker, container, Some("root"), &["/bin/sh", "-c", script, "sh", &f], Duration::from_secs(8)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> ExecTarget {
        ExecTarget {
            project_id: "app".into(),
            container_id: "0123456789abcdef".into(),
            container_name: "wbdc-app".into(),
            user: Some("vscode".into()),
            map: Some((PathBuf::from("/home/u/app"), "/workspaces/app".into())),
            folder: "/workspaces/app".into(),
            remote_env: vec![("PATH".into(), Some("${containerEnv:PATH}:/opt/x".into())), ("GONE".into(), None)],
            workbench_url: Some("http://172.17.0.1:7981".into()),
            docker: "/usr/bin/docker".into(),
            shell: "/bin/bash".into(),
            has_bash: false,
        }
    }

    #[test]
    fn templates_expand_only_container_variables() {
        assert_eq!(template("${containerEnv:PATH}:/opt/x"), "\"${PATH}\"':/opt/x'");
        assert_eq!(template("a'b $(rm -rf /)"), "'a'\\''b $(rm -rf /)'");
        assert_eq!(template("${containerEnv:HOME:/root}/bin"), "\"${HOME:-/root}\"'/bin'");
        assert_eq!(template("${containerEnv:bad name}"), "'${containerEnv:bad name}'");
        assert_eq!(template(""), "''");
    }

    /// Values travel in the docker CLI's environment, never in its argv, on every OS (host
    /// paths in them: `wrapping_maps_host_paths`).
    #[test]
    fn wrapping_keeps_values_out_of_argv() {
        let t = target();
        let env = vec![
            ("TERM".to_string(), Some("xterm-256color".to_string())),
            ("WORKBENCH_URL".to_string(), Some("http://127.0.0.1:7981".to_string())),
            ("API_TOKEN".to_string(), Some("s3cret-value-123".to_string())),
            ("TMUX".to_string(), None),
        ];
        let (argv, host_env) = t.wrap("abc123", &["bash".into(), "-lc".into(), "cargo run".into()], Path::new("/home/u/app/server"), &env);
        let joined = argv.join(" ");
        assert!(!joined.contains("s3cret"), "{joined}");
        assert!(joined.contains("-u vscode"));
        // A login shell applies the variables after its profile (which resets PATH).
        let n = argv.len();
        assert_eq!(argv[n - 3..n - 1], ["/bin/sh".to_string(), "-lc".to_string()], "{argv:?}");
        assert!(argv[n - 1].starts_with(APPLY) && argv[n - 1].ends_with("\ncargo run"), "{argv:?}");
        let get = |k: &str| host_env.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.clone());
        assert_eq!(get("WB_E_API_TOKEN").as_deref(), Some("s3cret-value-123"));
        assert_eq!(get("WB_E_WORKBENCH_URL").as_deref(), Some("http://172.17.0.1:7981"));
        assert_eq!(get("WB_T_PATH").as_deref(), Some("\"${PATH}\"':/opt/x'"));
        assert_eq!(get("WB_UNSET_NAMES").as_deref(), Some("GONE"));
        assert_eq!(get("WB_PIDFILE").as_deref(), Some("/tmp/.workbench-exec-abc123.pid"));
        assert_eq!(get("WB_LOGIN").as_deref(), Some("1"));
        assert_eq!(login_rewrite(&["/bin/zsh".into(), "-l".into()]).unwrap()[2], format!("{APPLY}; exec /bin/zsh -i"));
        assert!(login_rewrite(&["claude".into(), "--resume".into()]).is_none());
        assert!(login_rewrite(&["fish".into(), "-l".into()]).is_none());
        assert!(get("WB_ENV_NAMES").unwrap().split(' ').any(|n| n == "API_TOKEN"));
    }

    /// Host paths under the workspace mount (the cwd, values and `:`-separated lists of
    /// them) become the container's. Unix host paths: dev containers are unsupported on
    /// Windows (`util::os::support`, `Feature::Devcontainer`), whose paths are `C:\…`.
    #[cfg(unix)]
    #[test]
    fn wrapping_maps_host_paths() {
        let t = target();
        let env = vec![
            ("CARGO_TARGET_DIR".to_string(), Some("/home/u/app/target".to_string())),
            ("OUTSIDE".to_string(), Some("/home/u/other".to_string())),
        ];
        let (argv, host_env) = t.wrap("abc123", &["bash".into(), "-lc".into(), "cargo run".into()], Path::new("/home/u/app/server"), &env);
        assert!(argv.join(" ").contains("-w /workspaces/app/server"), "{argv:?}");
        let get = |k: &str| host_env.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.clone());
        assert_eq!(get("WB_E_CARGO_TARGET_DIR").as_deref(), Some("/workspaces/app/target"));
        assert_eq!(get("WB_E_OUTSIDE").as_deref(), Some("/home/u/other"));
        // A cwd outside the mount opens the workspace folder.
        let (argv, _) = t.wrap("x", &["sh".into()], Path::new("/tmp"), &[]);
        assert!(argv.join(" ").contains("-w /workspaces/app "));
        assert_eq!(t.map_value("/home/u/app/a:/usr/bin:/home/u/app"), "/workspaces/app/a:/usr/bin:/workspaces/app");
    }
}
