//! Documentation signals.
//!
//! * Fenced shell blocks in CLAUDE.md / README.md / SETUP.md / AGENTS.md become
//!   **suggested** runs (group `suggested`, kind task, source `CLAUDE.md:L<n>`). They
//!   are never started automatically, and anything remote, destructive or
//!   secret-handling (ssh, deploy, push, rm -rf, tokens…) is not offered at all.
//! * Confluence page links (`https://<site>.atlassian.net/wiki/spaces/<KEY>/pages/<id>`),
//!   "Moved from Confluence (<space> space, page <id>" notes and `cloudId \`…\`` mentions
//!   → `[links.confluence]`.
//! * Root docs → `project.docs` ("read me first").

use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, text::ellipsize};
use crate::config::project::{Confluence, RunConfig, RunKind};

const SHELL_LANGS: &[&str] = &["bash", "sh", "shell", "console", "zsh", "shell-session", ""];
/// Root documents whose shell blocks are offered as runs, in "read me first" order.
const RUN_DOCS: &[&str] = &["CLAUDE.md", "AGENTS.md", "README.md", "SETUP.md", "CONTRIBUTING.md"];
const MAX_SUGGESTIONS: usize = 40;

static FIRST_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:[A-Z_][A-Z0-9_]*=\S*\s+)*(?:(?:cargo|npm|pnpm|yarn|bun|node|npx|deno|python3?|uv|pip3?|pytest|dotnet|docker|make|just|go|mvn|gradle|blender|curl|ssh|echo)\b|\./gradlew\b|~/|\./|[\w.-]+\.(?:sh|py|mjs|js)\b)",
    )
    .unwrap()
});
/// Commands never offered as one-click runs.
static UNSAFE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \b(?:ssh|scp|sftp|rsync|sudo|kubectl|helm|terraform|pulumi|ansible(?:-playbook)?|fuser|kill|pkill|killall|shutdown|reboot|mkfs|chmod|chown)\b
        | \bdeploy
        | \bgit\s+(?:push|reset|checkout|clean|rebase|stash|commit|tag\s+-d|branch\s+-D)
        | \brm\s+-\w*[rf]
        | \bdocker\s+(?:push|rm|rmi|login|system|volume|network\s+rm|compose\s+down|compose\s+rm)
        | \bnpm\s+(?:publish|login|adduser)|\bcargo\s+(?:publish|login|yank)
        | \bcurl\b.*\s(?:-X\s*(?:POST|PUT|PATCH|DELETE)|-d\b|--data|-F\b|--form|-T\b|--upload-file)
        | token|secret|passw|api[_-]?key|credential|\.pem\b|id_rsa
        | \bdd\s+if= | >\s*/(?:etc|dev|usr|var|opt|root)\b
        ",
    )
    .unwrap()
});
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[A-Za-z][\w-]*>|\bYOUR_|\bxxx+\b|\.\.\.").unwrap());
static CD_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^cd\s+(\S+)$").unwrap());
static CD_AND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^cd\s+(\S+)\s*&&\s*(.+)$").unwrap());
static CONFLUENCE_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https://([\w-]+)\.atlassian\.net/wiki/spaces/([\w~-]+)/pages/(\d+)").unwrap());
static MOVED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)moved from confluence \((\w+) space, page (?:id )?(\d+)").unwrap());
static CLOUD_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)cloud ?id[^`\n]{0,20}`([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})`").unwrap());
static SITE_MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([\w-]+\.atlassian\.net)`").unwrap());

/// `acme` or `acme.atlassian.net` → `https://acme.atlassian.net`, the form
/// `[links.confluence] site` (and the Atlassian client) expects.
pub fn site_url(name_or_host: &str) -> String {
    let host = name_or_host.trim().to_ascii_lowercase();
    if host.ends_with(".atlassian.net") { format!("https://{host}") } else { format!("https://{host}.atlassian.net") }
}
static SPACE_MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bspace\s+\**`([A-Z][A-Z0-9_]{1,20})`").unwrap());

/// One command from a shell code block.
#[derive(Debug, Clone, PartialEq)]
pub struct FenceCmd {
    /// 1-based line of the command's first line.
    pub line: usize,
    pub text: String,
    /// Directory as a shell would have it: `cd X` (relative to the directory before
    /// it) and `cd X && …` stick for the lines that follow (`None`: where the block starts).
    pub cwd: Option<String>,
    /// Directory when every line is read on its own from the project root, which
    /// is how many documents are written (`cd app/server` … `cd app/web`, or
    /// `cd sim && dotnet test` followed by `python3 tools/gen.py`): the last plain
    /// `cd X` as written, and `cd X && …` for its own line only.
    pub doc_cwd: Option<String>,
}

/// Commands in shell code blocks: continuation lines joined, comments and prompts
/// (`$ `) stripped, `cd` tracked per block. Output lines of `console` blocks are skipped.
pub fn shell_fences(src: &str) -> Vec<FenceCmd> {
    let mut out = vec![];
    let mut fence: Option<(String, bool)> = None; // (marker, is shell)
    let mut cwd: Option<String> = None;
    let mut doc_cwd: Option<String> = None;
    // The directory before the last `cd` (`cd -`).
    let mut oldpwd: Option<String> = None;
    let mut buf = String::new();
    let mut buf_line = 0usize;
    let mut console = false;
    for (idx, raw) in src.lines().enumerate() {
        let n = idx + 1;
        let trimmed = raw.trim_start();
        let marker = if trimmed.starts_with("```") { Some("```") } else if trimmed.starts_with("~~~") { Some("~~~") } else { None };
        if let Some(m) = marker {
            match &fence {
                None => {
                    let lang = trimmed.trim_start_matches(m).trim().split([' ', '{', ',']).next().unwrap_or("").to_ascii_lowercase();
                    console = matches!(lang.as_str(), "console" | "shell-session");
                    fence = Some((m.to_string(), SHELL_LANGS.contains(&lang.as_str())));
                    cwd = None;
                    doc_cwd = None;
                    oldpwd = None;
                    buf.clear();
                }
                Some((open, _)) if open == m => fence = None,
                Some(_) => {}
            }
            continue;
        }
        let Some((_, true)) = &fence else { continue };
        let mut line = raw.trim().to_string();
        if console {
            match line.strip_prefix("$ ") {
                Some(rest) => line = rest.to_string(),
                None if buf.is_empty() => continue,
                None => {}
            }
        } else if let Some(rest) = line.strip_prefix("$ ") {
            line = rest.to_string();
        }
        if buf.is_empty() {
            buf_line = n;
        }
        if let Some(head) = line.strip_suffix('\\') {
            buf.push_str(head.trim());
            buf.push(' ');
            continue;
        }
        let full = format!("{buf}{line}");
        buf.clear();
        let full = full.trim();
        if full.is_empty() || full.starts_with('#') {
            continue;
        }
        let cmd = match full.find(" #") {
            Some(i) => full[..i].trim(),
            None => full,
        };
        if let Some(c) = CD_ONLY.captures(cmd) {
            cwd = change_dir(&cwd, &c[1], &mut oldpwd);
            doc_cwd = if &c[1] == "-" { cwd.clone() } else { Some(c[1].to_string()) };
            continue;
        }
        // A `cd` in a `&&` chain sticks for the lines that follow, as in a shell:
        // after `cd server && cargo build`, `./target/release/app` runs in server/.
        // `cd ..` / `cd -` later in the chain come back; a subshell (`(cd x && make)`)
        // changes nothing.
        let (c_cwd, line_doc_cwd, cmd) = match CD_AND.captures(cmd) {
            Some(c) => {
                cwd = change_dir(&cwd, &c[1], &mut oldpwd);
                (cwd.clone(), Some(c[1].to_string()), c[2].trim().to_string())
            }
            None => (cwd.clone(), doc_cwd.clone(), cmd.to_string()),
        };
        if !cmd.starts_with('(') {
            for part in cmd.split("&&") {
                if let Some(c) = CD_ONLY.captures(part.trim()) {
                    cwd = change_dir(&cwd, &c[1], &mut oldpwd);
                }
            }
        }
        out.push(FenceCmd { line: buf_line, text: cmd, cwd: c_cwd, doc_cwd: line_doc_cwd });
    }
    out
}

/// The block's directory after `cd target`: relative targets are taken from the
/// current one (`None` is where the block starts, the project root), `-` goes back
/// to the previous one.
fn change_dir(cwd: &Option<String>, target: &str, oldpwd: &mut Option<String>) -> Option<String> {
    let target = target.trim_matches(['"', '\'']);
    let next = if target == "-" {
        oldpwd.clone()
    } else if crate::util::os::path::is_absolute_str(target) || target.starts_with('~') || target.contains('$') {
        Some(target.to_string())
    } else {
        match cwd {
            Some(c) => Some(format!("{}/{target}", c.trim_end_matches('/'))),
            None => Some(target.to_string()),
        }
    };
    *oldpwd = cwd.clone();
    next
}

/// Whether a documented command may be offered as a one-click run.
pub fn is_offerable(cmd: &str) -> bool {
    FIRST_WORD.is_match(cmd) && !is_risky(cmd) && !PLACEHOLDER.is_match(cmd) && cmd.len() <= 400
}

/// Releases and hosting CLIs that publish or deploy (beyond `UNSAFE`'s `deploy`):
/// `npm run release`, `semantic-release`, `gh-pages -d dist`, `vercel --prod`, `surge`.
/// `release`/`publish` must be a word of its own, not a flag (`cargo build --release`)
/// or part of a longer name (`release-notes`); `release:prod` counts.
static RELEASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[\s:/])(?:release|publish)(?:$|[\s:])|semantic-release|\bgh-pages\b|\bvercel\b|\bsurge\b").unwrap()
});

static DEPLOYISH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bdeploy").unwrap());

/// `release`/`publish` as a word, except as a CMake build type or preset
/// (`cmake --preset release`, `ctest -C Release`, the run `cmake build: release`).
fn releases(text: &str) -> bool {
    let first = text.split_whitespace().next().unwrap_or("");
    RELEASE.is_match(text) && !matches!(first, "cmake" | "ctest")
}

/// Whether a run name or command deploys or releases (detected ones go to group `deploy`).
pub fn is_deployish(text: &str) -> bool {
    DEPLOYISH.is_match(text) || releases(text)
}

/// Whether a command (or a run's name) deploys, releases, publishes, reaches a remote
/// host, destroys data or handles secrets. Such runs are never started without the
/// user's confirmation, and never by agents.
pub fn is_risky(text: &str) -> bool {
    UNSAFE.is_match(text) || releases(text)
}

pub fn detect(cx: &mut Ctx) {
    // project.docs: the root docs that exist.
    for d in RUN_DOCS {
        if cx.has_file(d) && !cx.pf.project.docs.iter().any(|x| x == d) {
            cx.pf.project.docs.push(d.to_string());
        }
    }
    suggestions(cx);
    confluence(cx);
}

fn suggestions(cx: &mut Ctx) {
    let mut added = 0usize;
    for doc in RUN_DOCS {
        let path = cx.root.join(doc);
        if !path.is_file() {
            continue;
        }
        let Some(src) = cx.read(&path) else { continue };
        for fc in shell_fences(&src) {
            if added >= MAX_SUGGESTIONS {
                return;
            }
            if !is_offerable(&fc.text) {
                continue;
            }
            let Some(cwd) = choose_cwd(cx, &fc) else { continue };
            if cx.pf.runs.iter().any(|r| r.command == fc.text && r.cwd == cwd) {
                continue;
            }
            let base = ellipsize(&fc.text, 56);
            let name = if cx.pf.runs.iter().any(|r| r.name == base) { format!("{base} · L{}", fc.line) } else { base };
            if cx.pf.runs.iter().any(|r| r.name == name) {
                continue;
            }
            cx.pf.runs.push(RunConfig {
                name,
                kind: RunKind::Task,
                command: fc.text.clone(),
                cwd,
                source: Some(format!("{doc}:L{}", fc.line)),
                group: Some("suggested".into()),
                ..Default::default()
            });
            added += 1;
        }
    }
}

/// Where a documented command runs. A shell keeps the directory of a `cd` for the
/// lines that follow (`cd server && cargo build --release`, then
/// `./target/release/app`), but documents are often written line by line from the
/// project root (`cd app/server` … `cd app/web`). When the two readings differ, the
/// files decide: the shell's directory unless it does not exist, or the command
/// names a path that exists only from the document's directory.
fn choose_cwd(cx: &Ctx, fc: &FenceCmd) -> Option<String> {
    let shell = resolve_cwd(cx, fc.cwd.as_deref());
    if fc.cwd == fc.doc_cwd {
        return shell;
    }
    let Some(doc) = resolve_cwd(cx, fc.doc_cwd.as_deref()) else { return shell };
    let Some(sh) = shell else { return Some(doc) };
    let is_dir = |d: &str| cx.root.join(d).is_dir();
    if !is_dir(&sh) {
        return Some(if is_dir(&doc) { doc } else { sh });
    }
    let path = fc.text.split_whitespace().map(|w| w.trim_matches(['"', '\''])).find(|w| {
        w.contains('/') && !w.starts_with(['-', '/', '~', '$']) && !w.contains("://") && !w.contains('=')
    });
    match path {
        Some(p) if cx.root.join(&doc).join(p).exists() && !cx.root.join(&sh).join(p).exists() => Some(doc),
        _ => Some(sh),
    }
}

/// A `cd` target from a doc → a project-relative cwd, or `None` when it points
/// outside the project or depends on shell expansion.
fn resolve_cwd(cx: &Ctx, cd: Option<&str>) -> Option<String> {
    let Some(cd) = cd.map(|c| c.trim_matches(['"', '\''])) else { return Some(".".into()) };
    if cd.contains('$') || cd.contains('`') || cd == "-" {
        return None;
    }
    let p = crate::config::expand_tilde(cd);
    let joined: PathBuf = if p.is_absolute() { p } else { cx.root.join(p) };
    // Lexical normalisation (the directory may not exist yet).
    let mut norm = PathBuf::new();
    for comp in joined.components() {
        match comp {
            std::path::Component::ParentDir => {
                norm.pop();
            }
            std::path::Component::CurDir => {}
            c => norm.push(c),
        }
    }
    norm.starts_with(cx.root).then(|| cx.rel(&norm))
}

fn confluence(cx: &mut Ctx) {
    // Index documents only (any depth of the walk): links scattered through every
    // design note would make every page a "root".
    let md: Vec<PathBuf> = cx
        .files
        .iter()
        .filter(|f| f.file_name().is_some_and(|n| matches!(n.to_str(), Some("CLAUDE.md" | "README.md" | "AGENTS.md"))))
        .take(50)
        .cloned()
        .collect();
    let mut conf: Option<Confluence> = None;
    let mut site_hint: Option<String> = None;
    let mut space_hint: Option<String> = None;
    for f in md {
        let Some(src) = cx.read(&f) else { continue };
        for c in CONFLUENCE_LINK.captures_iter(&src) {
            let Ok(id) = c[3].parse::<u64>() else { continue };
            let entry = conf.get_or_insert_with(|| Confluence {
                site: site_url(&c[1]),
                space: c[2].to_string(),
                ..Default::default()
            });
            if !entry.root_pages.contains(&id) && entry.root_pages.len() < 20 {
                entry.root_pages.push(id);
            }
        }
        if let Some(c) = MOVED.captures(&src) {
            if let Ok(id) = c[2].parse::<u64>() {
                let entry = conf.get_or_insert_with(|| Confluence { space: c[1].to_uppercase(), ..Default::default() });
                entry.archived = true;
                if !entry.root_pages.contains(&id) {
                    entry.root_pages.push(id);
                }
            }
        }
        if let Some(c) = CLOUD_ID.captures(&src) {
            let entry = conf.get_or_insert_with(Confluence::default);
            entry.cloud_id.get_or_insert_with(|| c[1].to_string());
        }
        if site_hint.is_none() {
            site_hint = SITE_MENTION.captures(&src).map(|c| site_url(&c[1]));
        }
        if space_hint.is_none() {
            space_hint = SPACE_MENTION.captures(&src).map(|c| c[1].to_string());
        }
    }
    if let Some(mut c) = conf {
        if c.site.is_empty() {
            c.site = site_hint.unwrap_or_default();
        }
        if c.space.is_empty() {
            c.space = space_hint.unwrap_or_default();
        }
        cx.pf.links.confluence = Some(c);
    }
}
