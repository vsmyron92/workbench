//! Workbench's executable as a helper that other programs start by path, with arguments of
//! their own: git runs `GIT_ASKPASS` (and ssh `SSH_ASKPASS`) with the prompt as the only
//! argument and reads the answer from its output (docs/windows-port.md, "Git").
//!
//! Unix: a `#!/bin/sh` script in the data dir runs `<exe> askpass "$1"`; remote git commands
//! start in a new session, where ssh has no terminal to prompt on. Windows: no script (a
//! batch file would hand the prompt to cmd.exe): the variables name the executable itself,
//! `WORKBENCH_HELPER=askpass` tells it what the argument is ([`askpass_prompt`], which
//! `main.rs` checks before it parses the command line), and `SSH_ASKPASS_REQUIRE=force`
//! makes ssh ask it, instead of a console nobody sees, for passphrases and unknown host keys.

use std::ffi::{OsStr, OsString};
use std::path::Path;

/// The variable naming the helper this executable runs as (Windows).
pub const VAR: &str = "WORKBENCH_HELPER";

/// Environment for git commands that may ask for credentials: git (and on Windows ssh) asks
/// `workbench askpass`. Unix: writes `<data_dir>/git-askpass` (0700) for `GIT_ASKPASS`.
pub fn askpass_env(data_dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
    imp::askpass_env(data_dir)
}

/// The prompt, when git or ssh started this executable as its askpass program through
/// [`askpass_env`]'s variables: `WORKBENCH_HELPER=askpass` and a single argument that is not
/// a command line of Workbench's own (`is_command`: a subcommand, an option). Everything git
/// starts inherits the variable, so a hook running `workbench url`, and the interactive
/// rebase's editor (`workbench git-editor todo <dir> <file>`), still run those commands.
pub fn askpass_prompt(is_command: impl Fn(&str) -> bool) -> Option<String> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    prompt_of(std::env::var_os(VAR).as_deref(), &args, is_command)
}

fn prompt_of(helper: Option<&OsStr>, args: &[OsString], is_command: impl Fn(&str) -> bool) -> Option<String> {
    if helper != Some(OsStr::new("askpass")) {
        return None;
    }
    let [prompt] = args else { return None };
    let prompt = prompt.to_string_lossy().into_owned();
    (!is_command(&prompt)).then_some(prompt)
}

#[cfg(unix)]
mod imp {
    use std::path::{Path, PathBuf};

    pub fn askpass_env(data_dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
        let path = write_wrapper(data_dir)?;
        Ok(vec![("GIT_ASKPASS".into(), path.to_string_lossy().to_string())])
    }

    /// Write `<data_dir>/git-askpass` (0700) that runs this binary's askpass helper.
    fn write_wrapper(data_dir: &Path) -> anyhow::Result<PathBuf> {
        let exe = super::super::proc::current_exe()?;
        let exe = exe.to_string_lossy();
        let quoted = format!("'{}'", exe.replace('\'', "'\\''"));
        let script = format!("#!/bin/sh\n# Written by Workbench: answers git credential prompts for the configured GitLab host.\nexec {quoted} askpass \"$1\"\n");
        let path = data_dir.join("git-askpass");
        crate::util::fs::write_atomic(&path, script.as_bytes(), 0o700)?;
        crate::util::fs::set_mode(&path, 0o700);
        Ok(path)
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    pub fn askpass_env(_data_dir: &Path) -> anyhow::Result<Vec<(String, String)>> {
        // Git starts GIT_ASKPASS without a shell (CreateProcess), ssh with execlp: a plain
        // path, spaces included, is one argument to both.
        let exe = super::super::proc::current_exe()?.display().to_string();
        Ok(vec![
            ("GIT_ASKPASS".into(), exe.clone()),
            ("SSH_ASKPASS".into(), exe),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            (super::VAR.into(), "askpass".into()),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_a_single_prompt_under_the_variable_is_an_askpass_call() {
        let is_command = |w: &str| w.starts_with('-') || ["askpass", "git-editor", "url", "help"].contains(&w);
        let askpass = Some(OsStr::new("askpass"));
        let prompt = "Username for 'https://gitlab.com': ";
        assert_eq!(prompt_of(askpass, &args(&[prompt]), is_command).as_deref(), Some(prompt));
        assert_eq!(prompt_of(askpass, &args(&[""]), is_command).as_deref(), Some(""));
        // No variable, another helper, or Workbench's own command lines under the variable.
        assert_eq!(prompt_of(None, &args(&[prompt]), is_command), None);
        assert_eq!(prompt_of(Some(OsStr::new("other")), &args(&[prompt]), is_command), None);
        assert_eq!(prompt_of(askpass, &args(&[]), is_command), None);
        assert_eq!(prompt_of(askpass, &args(&["url"]), is_command), None);
        assert_eq!(prompt_of(askpass, &args(&["--version"]), is_command), None);
        assert_eq!(prompt_of(askpass, &args(&["git-editor", "todo", "C:/x", "C:/y"]), is_command), None);
    }

    #[cfg(unix)]
    #[test]
    fn wrapper_script_quotes_the_binary_path() {
        let d = tempfile::tempdir().unwrap();
        let env = askpass_env(d.path()).unwrap();
        let p = d.path().join("git-askpass");
        assert_eq!(env, vec![("GIT_ASKPASS".to_string(), p.to_string_lossy().to_string())]);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(text.contains(" askpass \"$1\""));
        crate::util::os::perm::assert_mode(&p, 0o700);
    }

    #[cfg(windows)]
    #[test]
    fn git_and_ssh_start_the_executable_itself() {
        let d = tempfile::tempdir().unwrap();
        let env: std::collections::HashMap<String, String> = askpass_env(d.path()).unwrap().into_iter().collect();
        let exe = crate::util::os::proc::current_exe().unwrap().display().to_string();
        assert_eq!(env["GIT_ASKPASS"], exe);
        assert_eq!(env["SSH_ASKPASS"], exe);
        assert_eq!(env["SSH_ASKPASS_REQUIRE"], "force");
        assert_eq!(env[VAR], "askpass");
        assert!(std::fs::read_dir(d.path()).unwrap().next().is_none(), "no script is written");
    }
}
