//! Deployment topology → environments.
//!
//! * Caddyfile site blocks give each environment's URL (`staging.` prefix → staging),
//!   upstream port and basic-auth user (only the user — never the hash) plus the
//!   paths the auth matcher leaves open (`@site not path /api/*`).
//! * `deploy/*.sh` scripts, matched to a site by the port of their `HEALTH=` URL,
//!   give the health path, the remote directory (deploy command), and the container
//!   name (logs and deployed-version commands).
//! * `ssh [-i key] [-p port] user@host` lines in markdown code fences give the host.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, docs, sh};
use crate::config::project::{
    BasicAuth, Confirm, Deploy, EnvKind, Environment, Health, NamedCommand, SshHost, VersionProbe,
};

static REVERSE_PROXY_PORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*reverse_proxy\s+(?:@\S+\s+|/\S*\s+)?(?:https?://)?[\w.\-\[\]]*:(\d{2,5})\b").unwrap()
});
static BASIC_AUTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*basic_?auth(?:\s+(@\w+|/\S*))?\s*\{\s*(\S+)\s+\S+").unwrap());
static SCRIPT_HEALTH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^\s*HEALTH=["']?(\S+?)["']?\s*$"#).unwrap());
static SCRIPT_DIR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?m)^\s*DIR=["']?(/[\w./-]+)["']?\s*$"#).unwrap());
static SCRIPT_CONTAINER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"docker (?:inspect|logs|restart)\s+(?:--?\w[\w-]*(?:[= ]'[^']*'|[= ]\S+)?\s+)*([a-z0-9][\w.-]*-\d+)\b").unwrap()
});
static URL_PORT_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://[^/:]+:(\d{2,5})(/\S*)?$").unwrap());
static SSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bssh\s+((?:-[46AaCfGgKkMNnqsTtVvXxYy]+\s+|-[oiplFJ]\s*\S+\s+)*)([A-Za-z_][\w.-]*)@([\w.-]+\.[\w.-]+|\d{1,3}(?:\.\d{1,3}){3})\b")
        .unwrap()
});

/// One Caddyfile site block.
#[derive(Debug, Clone, PartialEq)]
pub struct Site {
    pub host: String,
    pub https: bool,
    pub port: Option<u16>,
    /// Basic-auth user and the paths left unguarded.
    pub auth: Option<(String, Vec<String>)>,
}

/// Parse Caddyfile site blocks (comments stripped, braces balanced).
pub fn parse_caddyfile(src: &str) -> Vec<Site> {
    let lines: Vec<String> = src.lines().map(strip_comment).collect();
    let mut sites = vec![];
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if let Some(addr) = line.strip_suffix('{').map(str::trim).filter(|a| !a.is_empty()) {
            // Collect the block body up to the matching brace.
            let mut depth = 1i32;
            let mut body = String::new();
            let mut j = i + 1;
            while j < lines.len() && depth > 0 {
                for ch in lines[j].chars() {
                    match ch {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
                if depth > 0 {
                    body.push_str(&lines[j]);
                    body.push('\n');
                }
                j += 1;
            }
            if let Some(site) = site_from_block(addr, &body) {
                sites.push(site);
            }
            i = j;
            continue;
        }
        i += 1;
    }
    sites
}

fn strip_comment(l: &str) -> String {
    // `#` starts a comment at line start or after whitespace (not inside `{…#…}` placeholders).
    let mut prev_ws = true;
    let mut in_quote = false;
    for (i, ch) in l.char_indices() {
        if ch == '"' {
            in_quote = !in_quote;
        }
        if ch == '#' && prev_ws && !in_quote {
            return l[..i].to_string();
        }
        prev_ws = ch.is_whitespace();
    }
    l.to_string()
}

fn site_from_block(addrs: &str, body: &str) -> Option<Site> {
    // Global options `{`, snippets `(name) {` and matchers are not sites.
    if addrs.starts_with('(') || addrs.starts_with('@') {
        return None;
    }
    let (host, https) = addrs.split([',', ' ']).map(str::trim).filter(|a| !a.is_empty()).find_map(|a| {
        let (https, rest) = match a.split_once("://") {
            Some(("http", r)) => (false, r),
            Some((_, r)) => (true, r),
            None => (true, a),
        };
        let host = rest.split(['/', ':']).next().unwrap_or("");
        let is_domain = host.contains('.')
            && host.chars().any(|c| c.is_ascii_alphabetic())
            && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '*')
            && !host.starts_with('*')
            && host != "localhost";
        is_domain.then(|| (host.to_ascii_lowercase(), https))
    })?;
    let port = REVERSE_PROXY_PORT.captures(body).and_then(|c| c[1].parse::<u16>().ok());
    let serves = port.is_some() || body.contains("file_server") || body.contains("respond") || body.contains("php_fastcgi");
    if !serves && body.contains("redir") {
        return None; // a redirect-only host (www → apex)
    }
    let auth = BASIC_AUTH.captures(body).map(|c| {
        let user = c[2].to_string();
        let except = c.get(1).map(|m| auth_except(body, m.as_str())).unwrap_or_default();
        (user, except)
    });
    Some(Site { host, https, port, auth })
}

/// Paths a negative matcher (`@site not path /api/* /health` or `@site { not path … }`)
/// leaves outside the basic auth.
fn auth_except(body: &str, matcher: &str) -> Vec<String> {
    if !matcher.starts_with('@') {
        return vec![];
    }
    let name = regex::escape(matcher);
    let inline = Regex::new(&format!(r"(?m)^\s*{name}\s+not\s+path\s+([^\n{{]+)$"));
    let block = Regex::new(&format!(r"(?s){name}\s*\{{\s*not\s+path\s+([^\n}}]+)"));
    for re in [inline, block].into_iter().flatten() {
        if let Some(c) = re.captures(body) {
            return c[1].split_whitespace().map(str::to_string).collect();
        }
    }
    vec![]
}

/// What a deploy script says about the environment it deploys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptInfo {
    pub file: String,
    /// Remote directory the script lives in (`DIR=/opt/app`).
    pub dir: Option<String>,
    pub health_port: Option<u16>,
    pub health_path: Option<String>,
    pub container: Option<String>,
}

pub fn parse_deploy_script(file: &str, src: &str) -> ScriptInfo {
    let mut info = ScriptInfo { file: file.to_string(), ..Default::default() };
    info.dir = SCRIPT_DIR.captures(src).map(|c| c[1].trim_end_matches('/').to_string());
    if let Some(h) = SCRIPT_HEALTH.captures(src).map(|c| c[1].to_string()) {
        if let Some(c) = URL_PORT_PATH.captures(&h) {
            info.health_port = c[1].parse().ok();
            info.health_path = Some(c.get(2).map(|m| m.as_str().to_string()).unwrap_or_else(|| "/".into()));
        }
    }
    info.container = SCRIPT_CONTAINER.captures(src).map(|c| c[1].to_string());
    info
}

/// `ssh` targets in shell code blocks: `(user, host, port, identity_file)`.
pub fn ssh_targets(fence_cmds: &[String]) -> Vec<SshHost> {
    let mut out: Vec<SshHost> = vec![];
    for cmd in fence_cmds {
        for c in SSH.captures_iter(cmd) {
            let user = c[2].to_string();
            let host = c[3].to_string();
            if user == "git" {
                continue; // a git remote, not a machine
            }
            let opts = c.get(1).map(|m| m.as_str()).unwrap_or("");
            let mut identity_file = None;
            let mut port = 22u16;
            let toks: Vec<&str> = opts.split_whitespace().collect();
            let mut k = 0;
            while k < toks.len() {
                let t = toks[k];
                let (flag, inline) = if t.len() > 2 && (t.starts_with("-i") || t.starts_with("-p")) { (&t[..2], Some(&t[2..])) } else { (t, None) };
                let val = inline.or_else(|| toks.get(k + 1).copied());
                match flag {
                    "-i" => identity_file = val.map(str::to_string),
                    "-p" => port = val.and_then(|v| v.parse().ok()).unwrap_or(22),
                    _ => {}
                }
                k += if inline.is_some() || !matches!(flag, "-i" | "-p" | "-o" | "-l" | "-F" | "-J") { 1 } else { 2 };
            }
            let h = SshHost { host, user, port, identity_file };
            if !out.iter().any(|x| x.host == h.host && x.user == h.user) {
                out.push(h);
            }
        }
    }
    out
}

pub fn detect(cx: &mut Ctx) {
    // An example Caddyfile or a runbook under `examples/` is not this project's
    // topology: its sites would be polled as environments.
    let files: Vec<PathBuf> = cx.files.iter().filter(|f| !cx.is_sample(f)).cloned().collect();
    let mut sites: Vec<Site> = vec![];
    for f in files.iter().filter(|f| f.file_name().is_some_and(|n| n == "Caddyfile" || n.to_string_lossy().ends_with(".caddyfile"))) {
        if let Some(src) = cx.read(f) {
            sites.extend(parse_caddyfile(&src));
        }
    }
    let scripts: Vec<ScriptInfo> = files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "sh"))
        .filter(|f| {
            let r = cx.rel(f);
            r.starts_with("deploy/") || (r.starts_with("deploy") && !r.contains('/'))
        })
        .cloned()
        .collect::<Vec<PathBuf>>()
        .into_iter()
        .filter_map(|f| {
            let name = f.file_name()?.to_string_lossy().into_owned();
            let src = cx.read(&f)?;
            Some(parse_deploy_script(&name, &src))
        })
        .collect();

    // ssh hosts from shell code blocks in markdown docs.
    let md: Vec<PathBuf> = files.iter().filter(|f| f.extension().is_some_and(|e| e == "md")).cloned().collect();
    let mut fence_cmds: Vec<String> = vec![];
    for f in md.iter().take(200) {
        if let Some(src) = cx.read(f) {
            fence_cmds.extend(docs::shell_fences(&src).into_iter().map(|c| c.text));
        }
    }
    let hosts = ssh_targets(&fence_cmds);
    for (i, h) in hosts.iter().enumerate() {
        let key = if i == 0 { "deploy".to_string() } else { format!("deploy-{}", i + 1) };
        cx.pf.hosts.entry(key).or_insert_with(|| h.clone());
    }
    let host_key = (hosts.len() == 1).then(|| "deploy".to_string());

    if sites.is_empty() {
        return;
    }
    let by_port: BTreeMap<u16, &ScriptInfo> = scripts.iter().filter_map(|s| Some((s.health_port?, s))).collect();
    let short_sha = cx.pf.repo.as_ref().and_then(|r| r.ci.as_ref()).is_some_and(|c| c.image_tag.as_deref() == Some("short_sha"));
    let has_ci = cx.pf.repo.as_ref().is_some_and(|r| r.ci.is_some());
    let default_branch = cx.pf.repo.as_ref().and_then(|r| r.default_branch.clone());
    let has_staging = sites.iter().any(|s| s.host.starts_with("staging."));

    for site in sites {
        let kind = env_kind(&site.host);
        let base = match kind {
            EnvKind::Staging => "staging",
            EnvKind::Production => "production",
            EnvKind::Preview => "preview",
            EnvKind::Development => "development",
        };
        let name = if cx.pf.envs.iter().any(|e| e.name == base) { site.host.clone() } else { base.to_string() };
        let scheme = if site.https { "https" } else { "http" };
        let url = format!("{scheme}://{}", site.host);
        let script = site.port.and_then(|p| by_port.get(&p).copied());
        let health_path = script.and_then(|s| s.health_path.clone());
        let mut env = Environment {
            name: name.clone(),
            kind,
            url: url.clone(),
            host: host_key.clone(),
            health: Some(Health {
                url: format!("{url}{}", health_path.as_deref().unwrap_or("/")),
                expect_status: 200,
                json_pointer: None,
                equals: None,
                interval_s: if kind == EnvKind::Production { 60 } else { 120 },
                timeout_ms: 5000,
                via_host: false,
            }),
            ..Default::default()
        };
        if let Some((user, except)) = site.auth {
            let secret = format!("{name}-basic-auth");
            env.auth = Some(BasicAuth { user, password: secret, except });
        }
        if let (Some(s), Some(_)) = (script, host_key.as_ref()) {
            if let Some(dir) = &s.dir {
                let tag = if short_sha { "{sha8}" } else { "{sha}" };
                env.deploy = Some(Deploy {
                    command: format!("cd {dir} && {} {tag}", sh(&format!("./{}", s.file))),
                    local: false,
                    confirm: if kind == EnvKind::Production { Confirm::Typed } else { Confirm::Click },
                    require_green_pipeline: has_ci,
                    only_ref: default_branch.clone(),
                    after: (kind == EnvKind::Production && has_staging).then(|| "staging".to_string()),
                });
            }
            if let Some(c) = &s.container {
                env.logs.push(NamedCommand { name: c.clone(), command: format!("docker logs -f --tail 300 {c}"), confirm: false });
                env.version = Some(VersionProbe {
                    http: None,
                    json_pointer: None,
                    command: Some(format!(
                        "docker image inspect --format '{{{{join .RepoTags \" \"}}}}' $(docker inspect --format '{{{{.Image}}}}' {c})"
                    )),
                    pattern: Some(if short_sha { r":(?P<sha>[0-9a-f]{8})\b" } else { r":(?P<sha>[0-9a-f]{7,40})\b" }.into()),
                });
            }
        }
        cx.pf.envs.push(env);
    }
}

pub fn env_kind(host: &str) -> EnvKind {
    let first = host.split('.').next().unwrap_or("");
    if first == "staging" || first.starts_with("staging-") || first == "stage" {
        EnvKind::Staging
    } else if matches!(first, "preview" | "dev" | "test" | "qa" | "uat" | "beta") {
        EnvKind::Preview
    } else {
        EnvKind::Production
    }
}
