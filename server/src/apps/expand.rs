//! Placeholder and secret expansion for run commands, env values and remote commands.
//!
//! * `{name}` placeholders: every `[toolchains]` entry, `{root}`, `{branch}`, `{sha}`,
//!   `{sha8}` (deploys add their own `{sha}`/`{sha8}`/`{branch}`). A `{…}` that is not
//!   a known name is left alone, as are `${VAR}` and Go-template `{{…}}`, so shell
//!   and docker syntax survives.
//! * `${secret:NAME}` is expanded **only in env values**, at spawn time; the value
//!   goes into the child's environment and never into argv, logs or the browser.

use std::collections::HashMap;
use std::path::Path;

use crate::app::AppState;
use crate::config::expand_tilde;
use crate::error::ApiError;
use crate::projects::Project;
use crate::secrets::Secret;

pub type Vars = HashMap<String, String>;

/// Replace known `{name}` placeholders in `s`.
pub fn placeholders(s: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut prev: Option<char> = None;
    while let Some(i) = rest.find('{') {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        if let Some(c) = head.chars().last() {
            prev = Some(c);
        }
        let after = &tail[1..];
        let end = after.find('}');
        let name = end.map(|e| &after[..e]);
        let valid = name.is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')));
        let shell_or_template = matches!(prev, Some('$') | Some('{')) || after.starts_with('{');
        let closes_double = end.is_some_and(|e| after[e + 1..].starts_with('}'));
        match (valid, name) {
            (true, Some(n)) if !shell_or_template && !closes_double && vars.contains_key(n) => {
                out.push_str(&vars[n]);
                rest = &after[n.len() + 1..];
                prev = Some('}');
            }
            _ => {
                out.push('{');
                rest = after;
                prev = Some('{');
            }
        }
    }
    out.push_str(rest);
    out
}

/// `{name}` placeholders in `s` that would be expanded if `name` were known —
/// i.e. unknown toolchains. Used to report problems before a start.
pub fn unknown_placeholders(s: &str, vars: &Vars) -> Vec<String> {
    let mut v = vec![];
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && (i == 0 || !matches!(bytes[i - 1], b'$' | b'{')) && bytes.get(i + 1) != Some(&b'{') {
            if let Some(e) = s[i + 1..].find('}') {
                let name = &s[i + 1..i + 1 + e];
                let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
                let double = s[i + 2 + e..].starts_with('}');
                if valid && !double && !vars.contains_key(name) && !v.iter().any(|x| x == name) {
                    v.push(name.to_string());
                }
            }
        }
        i += 1;
    }
    v
}

/// Toolchains (tilde-expanded) and `{root}`; git values are added by `with_git`.
pub fn base_vars(project: &Project) -> Vars {
    let mut vars: Vars = project
        .config
        .toolchains
        .iter()
        .map(|(k, v)| (k.clone(), expand_tilde(v).to_string_lossy().into_owned()))
        .collect();
    vars.insert("root".into(), project.root.to_string_lossy().into_owned());
    vars
}

/// Whether a branch name can be spliced into a shell command as-is. Git allows `;`,
/// `$`, quotes, backticks, `()` and `|` in branch names, and a fork's merge-request
/// branch is named by its author, so only `[A-Za-z0-9._/-]` (no leading `-`) passes.
/// Quoting is not enough: a command may put `{branch}` inside double quotes.
pub fn shell_safe_ref(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
}

/// Refuse to expand `{branch}` into `texts` when the branch name is not shell-safe.
pub fn check_branch(branch: &str, texts: &[&str]) -> Result<(), ApiError> {
    if shell_safe_ref(branch) || !texts.iter().any(|t| t.contains("{branch}")) {
        return Ok(());
    }
    Err(ApiError::bad_request(format!(
        "the checked-out branch {:?} has characters that are unsafe in a shell command, so {{branch}} is not expanded; \
         rename the branch (letters, digits, . _ / - only) or remove {{branch}} from the command",
        crate::apps::detect::text::ellipsize(branch, 80)
    )))
}

/// Add `{branch}`, `{sha}`, `{sha8}` when `texts` use them (git is only asked then).
/// Fails when `{branch}` is used and the branch name is not shell-safe.
pub async fn with_git(mut vars: Vars, root: &Path, texts: &[&str]) -> Result<Vars, ApiError> {
    let uses = |p: &str| texts.iter().any(|t| t.contains(p));
    if uses("{branch}") {
        let b = crate::util::git::current_branch(root).await.unwrap_or_else(|| "HEAD".into());
        check_branch(&b, texts)?;
        vars.insert("branch".into(), b);
    }
    if uses("{sha}") || uses("{sha8}") {
        if let Some(sha) = crate::util::git::head_sha(root).await {
            vars.insert("sha8".into(), sha.chars().take(8).collect());
            vars.insert("sha".into(), sha);
        }
    }
    Ok(vars)
}

/// Names of `${secret:NAME}` references in `s`.
#[cfg(test)]
pub fn secret_refs(s: &str) -> Vec<String> {
    let mut v = vec![];
    let mut rest = s;
    while let Some(i) = rest.find("${secret:") {
        let after = &rest[i + 9..];
        match after.find('}') {
            Some(e) => {
                v.push(after[..e].trim().to_string());
                rest = &after[e + 1..];
            }
            None => break,
        }
    }
    v
}

/// Expand one env value: secrets, placeholders, then a leading `~/`.
pub fn env_value(raw: &str, vars: &Vars, secret: &mut dyn FnMut(&str) -> Result<Secret, ApiError>, used: &mut Vec<Secret>) -> Result<String, ApiError> {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find("${secret:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 9..];
        let Some(e) = after.find('}') else {
            return Err(ApiError::bad_request("unterminated ${secret:…} in an env value"));
        };
        let s = secret(after[..e].trim())?;
        out.push_str(s.expose());
        used.push(s);
        rest = &after[e + 1..];
    }
    out.push_str(rest);
    let out = placeholders(&out, vars);
    Ok(if out == "~" || crate::util::os::path::home_relative(&out).is_some() { expand_tilde(&out).to_string_lossy().into_owned() } else { out })
}

/// Expand a run's env map. Returns the child env and the secrets used (for redaction).
pub fn run_env(
    state: &AppState,
    project: &Project,
    env: &std::collections::BTreeMap<String, String>,
    vars: &Vars,
) -> Result<(Vec<(String, Option<String>)>, Vec<Secret>), ApiError> {
    let mut used = vec![];
    let mut out = vec![];
    let mut lookup = |name: &str| state.secret(Some(project), name);
    for (k, v) in env {
        if k.is_empty() || k.contains('=') || k.contains('\0') {
            return Err(ApiError::bad_request(format!("invalid env variable name {k:?}")));
        }
        out.push((k.clone(), Some(env_value(v, vars, &mut lookup, &mut used)?)));
    }
    Ok((out, used))
}

/// Single-quote `s` for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        [("unity", "/opt/Unity"), ("root", "/w/p"), ("sha8", "4b8e2508"), ("dotnet", "/h/.dotnet/dotnet")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn expands_known_placeholders_only() {
        let v = vars();
        assert_eq!(placeholders("{unity} -projectPath \"$PWD\"", &v), "/opt/Unity -projectPath \"$PWD\"");
        assert_eq!(placeholders("tools/x.sh {sha8} {nope}", &v), "tools/x.sh 4b8e2508 {nope}");
        assert_eq!(placeholders("file://{root}/a.html", &v), "file:///w/p/a.html");
    }

    #[test]
    fn leaves_shell_and_template_syntax_alone() {
        let v = vars();
        assert_eq!(placeholders("echo ${root} {a,b} {}", &v), "echo ${root} {a,b} {}");
        let docker = "docker inspect --format '{{.Image}}' x && echo {{root}}";
        assert_eq!(placeholders(docker, &v), docker);
        assert_eq!(placeholders("{root}{sha8}", &v), "/w/p4b8e2508");
        assert_eq!(placeholders("unterminated {root", &v), "unterminated {root");
        assert_eq!(placeholders("ünïcödé {root} ✓", &v), "ünïcödé /w/p ✓");
    }

    #[test]
    fn reports_unknown_placeholders() {
        let v = vars();
        assert_eq!(unknown_placeholders("{unity-6000.5.6f1} -batchmode {root}", &v), vec!["unity-6000.5.6f1"]);
        assert!(unknown_placeholders("${HOME} {{.Image}} {a,b}", &v).is_empty());
    }

    #[test]
    fn expands_secrets_in_env_values_and_records_them() {
        let v = vars();
        let mut used = vec![];
        let mut lookup = |n: &str| -> Result<Secret, ApiError> {
            if n == "meshy" { Ok(test_secret("s3cr3t-value")) } else { Err(ApiError::not_configured(format!("no secret {n}"))) }
        };
        let got = env_value("Bearer ${secret:meshy}", &v, &mut lookup, &mut used).unwrap();
        assert_eq!(got, "Bearer s3cr3t-value");
        assert_eq!(used.len(), 1);
        let err = env_value("${secret:missing}", &v, &mut lookup, &mut used).unwrap_err();
        assert_eq!(err.code, "not_configured");
        assert!(env_value("${secret:meshy", &v, &mut lookup, &mut used).is_err());
        assert_eq!(secret_refs("a ${secret:x} b ${secret: y }"), vec!["x", "y"]);
    }

    #[test]
    fn expands_tilde_and_placeholders_in_env_values() {
        let v = vars();
        let mut used = vec![];
        let mut lookup = |_: &str| -> Result<Secret, ApiError> { unreachable!() };
        let home = dirs::home_dir().unwrap();
        assert_eq!(env_value("~/.cache/x", &v, &mut lookup, &mut used).unwrap(), home.join(".cache/x").to_string_lossy());
        assert_eq!(env_value("{root}/data", &v, &mut lookup, &mut used).unwrap(), "/w/p/data");
        assert_eq!(env_value("", &v, &mut lookup, &mut used).unwrap(), "");
    }

    #[test]
    fn unsafe_branch_names_are_never_spliced_into_commands() {
        for ok in ["main", "feature/x-y", "release-1.2", "HEAD", "fix_42"] {
            assert!(shell_safe_ref(ok), "{ok}");
            assert!(check_branch(ok, &["echo {branch}"]).is_ok());
        }
        for bad in ["fix;touch${IFS}INJECTED", "a$(id)", "x`id`", "a b", "it's", "a|b", "-x", "a&b", ""] {
            assert!(!shell_safe_ref(bad), "{bad:?}");
            let e = check_branch(bad, &["deploy.sh {sha8} on {branch}"]).unwrap_err();
            assert_eq!(e.code, "bad_request");
            // Not used by the command: nothing to refuse.
            assert!(check_branch(bad, &["deploy.sh {sha8}"]).is_ok());
        }
    }

    #[tokio::test]
    async fn with_git_refuses_an_unsafe_branch_only_when_used() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "c"]);
        git(&["checkout", "-q", "-b", "fix;touch${IFS}INJECTED"]);
        let e = with_git(Vars::new(), root, &["echo {branch}"]).await.unwrap_err();
        assert!(e.message.contains("unsafe"), "{}", e.message);
        let v = with_git(Vars::new(), root, &["echo {sha8}"]).await.unwrap();
        assert_eq!(v["sha8"].len(), 8);
        assert!(!root.join("INJECTED").exists());
    }

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("http://127.0.0.1:8081/api/health"), "http://127.0.0.1:8081/api/health");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    pub(crate) fn test_secret(v: &str) -> Secret {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s");
        std::fs::write(&p, v).unwrap();
        crate::util::fs::set_mode(&p, 0o600);
        let store = crate::secrets::SecretStore::default();
        store.resolve_ref("t", &crate::config::SecretRef::File(p.to_string_lossy().into_owned())).unwrap()
    }
}
