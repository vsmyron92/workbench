//! Where each agent CLI looks for skills, and what is there.
//!
//! Every CLI reads the same `SKILL.md` format from different folders, so "which skills can my
//! agents use here" is a question about each CLI's rules, and the rules are written down here,
//! each from the CLI's documentation or source:
//!
//! | CLI | personal (per account) | shared personal | in the project |
//! |---|---|---|---|
//! | Claude Code | `<CLAUDE_CONFIG_DIR>/skills`, `…/skills/synced/<account>/` (claude.ai), plugins, `…/commands` | none | `.claude/skills` and `.claude/commands` from the project up to the repository root, and in sub-folders (`nested`) |
//! | Codex | `$CODEX_HOME/skills` (deprecated, kept), `…/skills/.system` | `~/.agents/skills` | `.agents/skills` and `.codex/skills` from the repository root down to the project |
//! | Gemini CLI | `<home>/.gemini/skills` | `~/.agents/skills` | `.gemini/skills`, `.agents/skills` |
//! | Kimi Code | `$KIMI_CODE_HOME/skills`, `extra_skill_dirs` of its config | `~/.agents/skills` | `.kimi-code/skills`, `.agents/skills` at the repository root |
//!
//! Nothing here runs or follows anything beyond reading: skill folders in a repository are
//! untrusted content, links are read (a linked skill folder is a normal way to share skills) but a
//! file that is not a plain file is never opened (a FIFO would hang a read), sizes are capped, and
//! walks are bounded. A scan costs a few directory reads; nothing is cached.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::parse::{self, Issue, Parsed};
use super::session;
use crate::config::contract_tilde;
use crate::terminals::{AgentAccount, ProviderKind};
use crate::util::os::path as ospath;

/// Skills read in all: a bound on what a hostile folder can make this do.
pub const MAX_SKILLS: usize = 1500;
/// How deep below the project the search for `.claude/skills` folders in sub-folders goes.
const NESTED_DEPTH: usize = 3;
/// Files counted and listed for one skill.
pub const MAX_FILES: usize = 300;
const FILE_DEPTH: usize = 5;
/// Folders a search for nested skills never enters: dependencies and build output.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "out", "vendor", "venv", "site-packages", "__pycache__", "Pods", "coverage"];

// ---------------------------------------------------------------- limits

/// How much one scan may read: the bounds on what a hostile folder can make it do. Reading a folder costs one unit and
/// one more for each name in it, so links that make one big folder look like many cannot multiply the work.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Skills read in all.
    pub skills: usize,
    /// Names read from one folder: a folder of millions of files costs this much, not its size.
    pub entries: usize,
    /// What one root may cost, so that a flood in one root leaves the others alone.
    pub root_work: usize,
    /// What the roots of someone else's content (a repository, plugins, claude.ai's sync) may cost together.
    pub untrusted_work: usize,
    /// What the search for `.claude/skills` in the project's sub-folders may cost: a repository decides how many
    /// folders it has, and that must not leave the user's own skills unread.
    pub nested_work: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { skills: MAX_SKILLS, entries: 5000, root_work: 20_000, untrusted_work: 100_000, nested_work: 200_000 }
    }
}

// ---------------------------------------------------------------- the agent CLIs

/// The agent CLIs that read skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Cli {
    Claude,
    Codex,
    Gemini,
    Kimi,
}

impl Cli {
    pub fn of(kind: ProviderKind) -> Option<Cli> {
        match kind {
            ProviderKind::Claude => Some(Cli::Claude),
            ProviderKind::Codex => Some(Cli::Codex),
            ProviderKind::Gemini => Some(Cli::Gemini),
            ProviderKind::Kimi => Some(Cli::Kimi),
            ProviderKind::Aider | ProviderKind::Custom => None,
        }
    }
}

/// An agent account that reads skills.
#[derive(Debug, Clone)]
pub struct Account {
    pub id: String,
    pub cli: Cli,
    pub label: String,
    pub enabled: bool,
    pub installed: bool,
    /// The folder the CLI keeps its own files in (Gemini: its `.gemini`).
    pub dir: PathBuf,
    /// Its sessions get Workbench's MCP tools.
    pub mcp: bool,
}

/// `~` and `~/x` against `home` (not the real home: tests run on scratch folders).
pub fn expand(home: &Path, s: &str) -> PathBuf {
    if s == "~" {
        return home.to_path_buf();
    }
    match ospath::home_relative(s) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(s),
    }
}

impl Account {
    /// The account's folder the way the CLI resolves it: the configured one, else its variable in
    /// Workbench's own environment, else the default under `home`.
    pub fn resolve(a: &AgentAccount, home: &Path) -> Option<Account> {
        let cli = Cli::of(a.kind)?;
        let configured = a.home.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| expand(home, s));
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let dir = match cli {
            Cli::Claude => configured.or_else(|| env("CLAUDE_CONFIG_DIR")).unwrap_or_else(|| home.join(".claude")),
            Cli::Codex => configured.or_else(|| env("CODEX_HOME")).unwrap_or_else(|| home.join(".codex")),
            Cli::Kimi => configured.or_else(|| env("KIMI_CODE_HOME")).unwrap_or_else(|| home.join(".kimi-code")),
            Cli::Gemini => configured.or_else(|| env("GEMINI_CLI_HOME")).unwrap_or_else(|| home.to_path_buf()).join(".gemini"),
        };
        Some(Account { id: a.id.clone(), cli, label: a.label.clone(), enabled: a.enabled, installed: a.installed, dir, mcp: a.mcp })
    }

    /// The folder this account's own skills live in (what Install writes to).
    pub fn skills_dir(&self) -> PathBuf {
        self.dir.join("skills")
    }
}

/// Everything a scan needs, so a test can point it at scratch folders.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub home: PathBuf,
    /// The project's folder, and the top of its repository (the project itself when there is none).
    pub root: PathBuf,
    pub git_root: PathBuf,
    pub accounts: Vec<Account>,
    /// Claude Code's managed settings folder (an organization's skills live under `.claude/`).
    pub managed_claude: PathBuf,
    /// Codex's administrator folder.
    pub admin_codex: Option<PathBuf>,
    pub limits: Limits,
}

impl Inputs {
    /// The project's repository, as far as skills are concerned: where its own skills are looked for (the folders
    /// from here down to the project) and where their links must stay. A "repository" that holds the home folder (a
    /// dotfiles checkout in `~`, a stray `.git` above it) holds everything of the user's, not the project's, so then
    /// it is the project itself: a project that merely lies inside `~` must not take `~/.agents/skills` for its own
    /// folder, and a link in it must not reach `~/.ssh`.
    fn repo_boundary(&self) -> PathBuf {
        if ospath::starts_with(&self.home, &self.git_root) { self.root.clone() } else { self.git_root.clone() }
    }
}

/// The nearest folder at or above `root` that holds a `.git`; `root` itself when none does.
pub fn git_root_of(root: &Path) -> PathBuf {
    root.ancestors().find(|d| d.join(".git").exists()).unwrap_or(root).to_path_buf()
}

// ---------------------------------------------------------------- what a scan finds

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Project,
    Nested,
    User,
    Plugin,
    Synced,
    Managed,
    System,
    Extra,
    Builtin,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Flags {
    /// Only the user starts it (`disable-model-invocation`, Codex's `allow_implicit_invocation: false`).
    pub manual_only: bool,
    /// Only the model uses it (`user-invocable: false`).
    pub model_only: bool,
    /// Tools it pre-approves while it runs (`allowed-tools`): they skip the user's prompt.
    pub preapproves: Vec<String>,
    /// It registers hooks when invoked.
    pub hooks: bool,
    /// It runs in a sub-agent (`context: fork`), and which.
    pub forks: Option<String>,
    /// It uses Claude Code's shell injection: commands run when it is invoked.
    pub shell: bool,
    /// The folder holds a `scripts/` folder.
    pub scripts: bool,
    /// It loads only while files matching these globs are in play.
    pub paths: Vec<String>,
    pub model: Option<String>,
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Invoke {
    pub agent: Cli,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    /// Opaque; the detail route finds the skill by it.
    pub id: String,
    /// What the file calls it: its front matter's `name`, else its folder.
    pub name: String,
    /// What the agent lists it as: `plugin:name`, `anthropic-skills:name`, `sub/dir:name`.
    pub qualified: String,
    /// `skill`, `command` (a Claude Code slash command file) or `builtin` (inside Claude Code).
    pub kind: &'static str,
    pub scope: Scope,
    /// The plugin, or the folder a nested skill belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_label: Option<String>,
    pub description: String,
    /// The description is the first line of the text: the file has none.
    pub description_from_text: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    pub agents: Vec<Cli>,
    /// The accounts that read it.
    pub accounts: Vec<String>,
    /// The skill's folder (a command's file), with `~` for the home folder. Empty for built-ins.
    pub path: String,
    /// The skill's file relative to the project, when it lies inside it (the editor opens it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_file: Option<String>,
    pub invoke: Vec<Invoke>,
    /// What its listing line costs every session (tokens, an estimate: four characters to one).
    pub tokens: u64,
    pub bytes: u64,
    pub lines: usize,
    /// Files in its folder besides `SKILL.md`.
    pub files: usize,
    pub flags: Flags,
    /// `active`, `name-only` (listed without its description), `disabled`, `shadowed`.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    /// Whether the newest session's listing had it (`None`: no listing to compare with).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_session: Option<bool>,
    /// The Workbench skill this is: it was installed from Workbench's own set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workbench: Option<String>,
    pub issues: Vec<Issue>,
}

/// Where a skill is on disk, kept for the detail route.
#[derive(Debug, Clone)]
pub struct Located {
    pub id: String,
    /// The `SKILL.md`, or the command's file.
    pub file: PathBuf,
    /// The skill's folder (`None` for a command: one file).
    pub dir: Option<PathBuf>,
    pub under_project: bool,
    /// Workbench may read its text and files. A project skill that is a link leaving the project is not
    /// read (agents follow such links; a repository must not make Workbench show files outside it).
    pub readable: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionNote {
    /// Skills in the listing.
    pub count: usize,
    pub chars: usize,
    pub tokens: u64,
    pub at_ms: Option<i64>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountReport {
    pub id: String,
    pub cli: Cli,
    pub label: String,
    pub installed: bool,
    pub enabled: bool,
    pub mcp: bool,
    /// The account's folder, with `~`.
    pub folder: String,
    /// Skills this account's CLI can load here.
    pub skills: usize,
    /// What their listing lines cost every session, estimated from the files.
    pub listing_tokens: u64,
    /// What the newest session in this project received (Claude Code only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionNote>,
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub skills: Vec<SkillInfo>,
    pub located: Vec<Located>,
    pub accounts: Vec<AccountReport>,
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------- roots

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Layout {
    /// `<root>/<name>/SKILL.md`; Kimi Code also takes loose `<root>/<name>.md` files.
    Folders { flat_md: bool },
    /// Any folder with a `SKILL.md` up to this depth, hidden folders left out (Codex).
    Recursive(usize),
    /// `<root>/<claude.ai account>/<name>/SKILL.md`.
    Synced,
    /// Claude Code's slash command files: `<root>/**/*.md`, sub-folders as `folder:name`.
    Commands,
}

#[derive(Debug, Clone)]
struct Root {
    dir: PathBuf,
    scope: Scope,
    layout: Layout,
    /// Indexes into `Inputs::accounts`.
    readers: BTreeSet<usize>,
    /// The plugin, or the sub-folder.
    label: Option<String>,
    plugin_enabled: bool,
    under_project: bool,
    /// Claude Code's personal skills folder holds `synced/` (claude.ai's skills, a root of its own).
    skip_synced: bool,
    /// Content someone else wrote (a repository, a plugin, claude.ai's sync) stays inside this folder: a skill
    /// whose file resolves outside it is a link to something that is not the content's own, and is not read.
    /// Personal skills have none: people link their own checkouts into them.
    boundary: Option<PathBuf>,
}

fn is_dir(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// The folders from `git_root` down to `root`, top first (`root` alone when it is the top).
fn chain(git_root: &Path, root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = root.ancestors().take_while(|d| d.starts_with(git_root)).map(Path::to_path_buf).collect();
    if v.is_empty() {
        v.push(root.to_path_buf());
    }
    v.reverse();
    v
}

struct Roots {
    list: Vec<Root>,
}

impl Roots {
    fn add(&mut self, r: Root) {
        if !is_dir(&r.dir) {
            return;
        }
        let key = ospath::canonicalize(&r.dir).unwrap_or_else(|_| r.dir.clone());
        if let Some(have) = self.list.iter_mut().find(|h| h.layout == r.layout && h.scope == r.scope && h.label == r.label && h.plugin_enabled == r.plugin_enabled && h.under_project == r.under_project && ospath::canonicalize(&h.dir).unwrap_or_else(|_| h.dir.clone()) == key) {
            have.readers.extend(r.readers);
            return;
        }
        self.list.push(r);
    }
}

fn root(dir: PathBuf, scope: Scope, layout: Layout, readers: impl IntoIterator<Item = usize>) -> Root {
    Root { dir, scope, layout, readers: readers.into_iter().collect(), label: None, plugin_enabled: true, under_project: false, skip_synced: false, boundary: None }
}

fn read_json(path: &Path, under: Option<&Path>) -> Option<Value> {
    if under.is_some_and(|root| ospath::leaves_machine_below(root, path)) {
        return None;
    }
    let m = std::fs::metadata(path).ok()?;
    if !m.is_file() || m.len() > (1 << 20) {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// The parts of Claude Code's settings that decide which skills it loads.
#[derive(Debug, Default)]
struct ClaudeSettings {
    /// `skillOverrides`: skill name → `on`, `name-only`, `user-invocable-only` or `off`.
    overrides: BTreeMap<String, String>,
    enabled_plugins: BTreeMap<String, bool>,
}

/// Claude Code's settings, lowest priority first: the account's, the project's, the project's own
/// local file, then the managed one.
fn claude_settings(acct: &Account, inp: &Inputs) -> ClaudeSettings {
    let mut s = ClaudeSettings::default();
    let layers = [
        (acct.dir.join("settings.json"), false),
        (inp.root.join(".claude").join("settings.json"), true),
        (inp.root.join(".claude").join("settings.local.json"), true),
        (inp.managed_claude.join("managed-settings.json"), false),
    ];
    for (path, project) in layers {
        let Some(v) = read_json(&path, project.then_some(inp.root.as_path())) else { continue };
        if let Some(o) = v.get("skillOverrides").and_then(Value::as_object) {
            for (k, val) in o {
                if let Some(val) = val.as_str() {
                    s.overrides.insert(k.clone(), val.to_string());
                }
            }
        }
        if let Some(o) = v.get("enabledPlugins").and_then(Value::as_object) {
            for (k, val) in o {
                s.enabled_plugins.insert(k.clone(), val.as_bool().unwrap_or(false));
            }
        }
    }
    s
}

/// The plugins Claude Code has installed for this project, from `installed_plugins.json` (the
/// plugin cache keeps every version it ever fetched, so the folders say nothing): one root for
/// each plugin's skills and one for its commands.
fn plugin_roots(idx: usize, acct: &Account, inp: &Inputs, settings: &ClaudeSettings, roots: &mut Roots) {
    let plugins_dir = acct.dir.join("plugins");
    let Some(installed) = read_json(&plugins_dir.join("installed_plugins.json"), None) else { return };
    let Some(plugins) = installed.get("plugins").and_then(Value::as_object) else { return };
    let base = ospath::canonicalize(&plugins_dir).ok();
    for (id, entries) in plugins {
        let Some(entries) = entries.as_array() else { continue };
        // A plugin installed for another project is not this project's.
        let entry = entries.iter().find(|e| match e.get("projectPath").and_then(Value::as_str) {
            None => true,
            Some(p) => Path::new(p) == inp.root,
        });
        let Some(install) = entry.and_then(|e| e.get("installPath")).and_then(Value::as_str).map(PathBuf::from) else { continue };
        // Only what lies under Claude's own plugin folder is read.
        let inside = ospath::canonicalize(&install).ok().zip(base.clone()).is_some_and(|(p, b)| ospath::starts_with(&p, &b));
        if !inside {
            continue;
        }
        let manifest = read_json(&install.join(".claude-plugin").join("plugin.json"), None);
        let name = manifest.as_ref().and_then(|m| m.get("name")).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| id.split('@').next().unwrap_or(id).to_string());
        let enabled = settings.enabled_plugins.get(id) == Some(&true);
        let mut add = |dir: PathBuf, layout: Layout| {
            // A manifest may name folders of its own; they stay inside the plugin.
            let inside = ospath::canonicalize(&dir).ok().zip(ospath::canonicalize(&install).ok()).is_some_and(|(d, i)| ospath::starts_with(&d, &i));
            if inside {
                let mut r = root(dir, Scope::Plugin, layout, [idx]);
                r.label = Some(name.clone());
                r.plugin_enabled = enabled;
                r.boundary = Some(install.clone());
                roots.add(r);
            }
        };
        add(install.join("skills"), Layout::Folders { flat_md: false });
        add(install.join("commands"), Layout::Commands);
        for (key, layout) in [("skills", Layout::Folders { flat_md: false }), ("commands", Layout::Commands)] {
            let custom = match manifest.as_ref().and_then(|m| m.get(key)) {
                Some(Value::String(s)) => vec![s.clone()],
                Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                _ => vec![],
            };
            for rel in custom {
                if ospath::check_relative(rel.trim_start_matches("./")).is_ok() {
                    add(install.join(rel.trim_start_matches("./")), layout);
                }
            }
        }
    }
}

/// What a walk may still read, and whether it had to stop short.
struct Walk {
    left: usize,
    /// Names read from one folder, at most.
    per_dir: usize,
    cut: bool,
}

impl Walk {
    fn new(left: usize, per_dir: usize) -> Walk {
        Walk { left, per_dir, cut: false }
    }

    /// One folder is about to be looked at; false (and `cut`) when nothing is left.
    fn visit(&mut self) -> bool {
        if self.left == 0 {
            self.cut = true;
            return false;
        }
        self.left -= 1;
        true
    }

    /// The names in `dir` by name: as many as the cap on one folder and what is left of the walk allow.
    fn names(&mut self, dir: &Path) -> Vec<std::fs::DirEntry> {
        let (v, more) = read_dir_sorted(dir, self.per_dir.min(self.left));
        self.left -= v.len();
        self.cut |= more;
        v
    }
}

/// A folder's entries by name, at most `cap` of them, and whether there were more.
pub fn read_dir_sorted(dir: &Path, cap: usize) -> (Vec<std::fs::DirEntry>, bool) {
    let Ok(rd) = std::fs::read_dir(dir) else { return (vec![], false) };
    let mut v: Vec<_> = rd.flatten().take(cap.saturating_add(1)).collect();
    let more = v.len() > cap;
    v.truncate(cap);
    v.sort_by_key(|e| e.file_name());
    (v, more)
}

/// Folders below `root` that hold their own `.claude/skills`, as `(skills folder, folder relative to root)`.
fn nested_skill_dirs(root: &Path, walk: &mut Walk) -> Vec<(PathBuf, String)> {
    let mut out = vec![];
    let mut level = vec![root.to_path_buf()];
    for depth in 0..=NESTED_DEPTH {
        let mut next = vec![];
        for d in &level {
            if !walk.visit() {
                return out;
            }
            if depth > 0 {
                let skills = d.join(".claude").join("skills");
                if is_dir(&skills) {
                    let rel = d.strip_prefix(root).map(|r| ospath::to_slash(r)).unwrap_or_default();
                    out.push((skills, rel));
                }
            }
            if depth == NESTED_DEPTH {
                continue;
            }
            for e in walk.names(d) {
                let n = e.file_name().to_string_lossy().into_owned();
                // Links are not followed (a loop, or a way out of the project), nor are hidden or build folders.
                if n.starts_with('.') || SKIP_DIRS.contains(&n.as_str()) || !e.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                next.push(e.path());
            }
        }
        level = next;
    }
    out
}

fn roots_of(inp: &Inputs, nested: &mut Walk) -> Roots {
    let mut roots = Roots { list: vec![] };
    let boundary = inp.repo_boundary();
    let by_cli = |c: Cli| -> Vec<usize> { inp.accounts.iter().enumerate().filter(|(_, a)| a.cli == c).map(|(i, _)| i).collect() };
    let (claude, codex, gemini, kimi) = (by_cli(Cli::Claude), by_cli(Cli::Codex), by_cli(Cli::Gemini), by_cli(Cli::Kimi));
    let folders = Layout::Folders { flat_md: false };
    let project = |roots: &mut Roots, dir: PathBuf, layout: Layout, readers: &[usize]| {
        let mut r = root(dir, Scope::Project, layout, readers.iter().copied());
        r.under_project = true;
        r.boundary = Some(boundary.clone());
        roots.add(r);
    };

    // Claude Code.
    for &i in &claude {
        let a = &inp.accounts[i];
        let settings = claude_settings(a, inp);
        let mut personal = root(a.dir.join("skills"), Scope::User, folders, [i]);
        personal.skip_synced = true;
        roots.add(personal);
        let mut synced = root(a.dir.join("skills").join("synced"), Scope::Synced, Layout::Synced, [i]);
        synced.boundary = Some(synced.dir.clone());
        roots.add(synced);
        roots.add(root(a.dir.join("commands"), Scope::User, Layout::Commands, [i]));
        plugin_roots(i, a, inp, &settings, &mut roots);
    }
    if !claude.is_empty() {
        roots.add(root(inp.managed_claude.join(".claude").join("skills"), Scope::Managed, folders, claude.iter().copied()));
        roots.add(root(inp.managed_claude.join(".claude").join("commands"), Scope::Managed, Layout::Commands, claude.iter().copied()));
        for d in chain(&boundary, &inp.root) {
            project(&mut roots, d.join(".claude").join("skills"), folders, &claude);
            project(&mut roots, d.join(".claude").join("commands"), Layout::Commands, &claude);
        }
        for (dir, rel) in nested_skill_dirs(&inp.root, nested) {
            let mut r = root(dir, Scope::Nested, folders, claude.iter().copied());
            r.label = Some(rel);
            r.under_project = true;
            r.boundary = Some(boundary.clone());
            roots.add(r);
        }
    }

    // Codex.
    for &i in &codex {
        let a = &inp.accounts[i];
        roots.add(root(a.dir.join("skills"), Scope::User, Layout::Recursive(6), [i]));
        roots.add(root(a.dir.join("skills").join(".system"), Scope::System, Layout::Recursive(6), [i]));
    }
    if !codex.is_empty() {
        if let Some(admin) = &inp.admin_codex {
            roots.add(root(admin.join("skills"), Scope::Managed, Layout::Recursive(6), codex.iter().copied()));
        }
        for d in chain(&boundary, &inp.root) {
            project(&mut roots, d.join(".agents").join("skills"), Layout::Recursive(6), &codex);
            project(&mut roots, d.join(".codex").join("skills"), Layout::Recursive(6), &codex);
        }
    }

    // Gemini CLI.
    for &i in &gemini {
        roots.add(root(inp.accounts[i].dir.join("skills"), Scope::User, folders, [i]));
    }
    if !gemini.is_empty() {
        project(&mut roots, inp.root.join(".gemini").join("skills"), folders, &gemini);
        project(&mut roots, inp.root.join(".agents").join("skills"), folders, &gemini);
    }

    // Kimi Code.
    let kimi_folders = Layout::Folders { flat_md: true };
    for &i in &kimi {
        let a = &inp.accounts[i];
        roots.add(root(a.dir.join("skills"), Scope::User, kimi_folders, [i]));
        for extra in kimi_extra_dirs(a, inp) {
            roots.add(root(extra, Scope::Extra, kimi_folders, [i]));
        }
    }
    if !kimi.is_empty() {
        project(&mut roots, boundary.join(".kimi-code").join("skills"), kimi_folders, &kimi);
        project(&mut roots, boundary.join(".agents").join("skills"), kimi_folders, &kimi);
    }

    // `~/.agents/skills`: the one personal folder the other three CLIs share.
    let shared = inp.home.join(".agents").join("skills");
    roots.add(root(shared.clone(), Scope::User, Layout::Recursive(6), codex.iter().copied()));
    roots.add(root(shared.clone(), Scope::User, folders, gemini.iter().copied()));
    roots.add(root(shared, Scope::User, kimi_folders, kimi.iter().copied()));
    roots.list.retain(|r| !r.readers.is_empty());
    roots
}

/// `extra_skill_dirs` of Kimi Code's `config.toml`: absolute, `~/…` or relative to the repository root.
fn kimi_extra_dirs(a: &Account, inp: &Inputs) -> Vec<PathBuf> {
    let Some(text) = read_small(&a.dir.join("config.toml")) else { return vec![] };
    let Ok(v) = toml::from_str::<toml::Value>(&text) else { return vec![] };
    v.get("extra_skill_dirs")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(toml::Value::as_str)
                .map(|d| {
                    let p = expand(&inp.home, d);
                    if p.is_absolute() { p } else { inp.git_root.join(p) }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `SKILL.md` paths a Codex account's `config.toml` turns off (`[[skills.config]]`, `enabled = false`).
fn codex_disabled(a: &Account) -> HashSet<PathBuf> {
    let Some(text) = read_small(&a.dir.join("config.toml")) else { return HashSet::new() };
    let Ok(v) = toml::from_str::<toml::Value>(&text) else { return HashSet::new() };
    v.get("skills")
        .and_then(|s| s.get("config"))
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|e| e.get("enabled").and_then(toml::Value::as_bool) == Some(false))
                .filter_map(|e| e.get("path").and_then(toml::Value::as_str))
                .map(|p| {
                    let p = PathBuf::from(p);
                    ospath::canonicalize(&p).unwrap_or(p)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_small(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    (m.is_file() && m.len() <= (1 << 20)).then(|| std::fs::read_to_string(path).ok()).flatten()
}

// ---------------------------------------------------------------- scanning

/// A skill file found under a root, before its text is read.
struct Hit {
    /// The skill's folder (for a command, its file).
    entry: PathBuf,
    file: PathBuf,
    /// Its folder's name, or a command's name with `folder:` in front for sub-folders.
    folder: String,
    command: bool,
}

fn sorted_entries(dir: &Path, walk: &mut Walk) -> Vec<std::fs::DirEntry> {
    if !walk.visit() {
        return vec![];
    }
    walk.names(dir)
}

fn is_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

fn hits_in(r: &Root, walk: &mut Walk) -> Vec<Hit> {
    let mut out = vec![];
    match r.layout {
        Layout::Folders { flat_md } => folders(&r.dir, flat_md, r.skip_synced, walk, &mut out),
        Layout::Synced => {
            for e in sorted_entries(&r.dir, walk) {
                let n = e.file_name().to_string_lossy().into_owned();
                if !n.starts_with('.') && is_dir(&e.path()) {
                    folders(&e.path(), false, false, walk, &mut out);
                }
            }
        }
        Layout::Recursive(depth) => recursive(&r.dir, depth, walk, &mut out),
        Layout::Commands => commands(&r.dir, "", 4, walk, &mut out),
    }
    out
}

fn folders(dir: &Path, flat_md: bool, skip_synced: bool, walk: &mut Walk, out: &mut Vec<Hit>) {
    for e in sorted_entries(dir, walk) {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        // Hidden folders (`.trash`) are not skills; `synced` holds claude.ai's, read as a root of its own.
        if name.starts_with('.') || (skip_synced && name == "synced") {
            continue;
        }
        if is_dir(&path) {
            let file = path.join("SKILL.md");
            if is_file(&file) {
                out.push(Hit { entry: path, file, folder: name, command: false });
            }
        } else if flat_md && name.ends_with(".md") && is_file(&path) {
            // A folder of the same name wins; the loose file is the Kimi Code form.
            let stem = name.trim_end_matches(".md").to_string();
            if !dir.join(&stem).is_dir() {
                out.push(Hit { entry: path.clone(), file: path, folder: stem, command: false });
            }
        }
    }
}

fn recursive(dir: &Path, depth: usize, walk: &mut Walk, out: &mut Vec<Hit>) {
    for e in sorted_entries(dir, walk) {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if name.starts_with('.') || !is_dir(&path) {
            continue;
        }
        let file = path.join("SKILL.md");
        if is_file(&file) {
            out.push(Hit { entry: path, file, folder: name, command: false });
        } else if depth > 1 {
            recursive(&path, depth - 1, walk, out);
        }
    }
}

fn commands(dir: &Path, prefix: &str, depth: usize, walk: &mut Walk, out: &mut Vec<Hit>) {
    for e in sorted_entries(dir, walk) {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if name.starts_with('.') {
            continue;
        }
        if is_dir(&path) {
            if depth > 1 {
                commands(&path, &format!("{prefix}{name}:"), depth - 1, walk, out);
            }
        } else if let Some(stem) = name.strip_suffix(".md") {
            if is_file(&path) {
                out.push(Hit { entry: path.clone(), file: path, folder: format!("{prefix}{stem}"), command: true });
            }
        }
    }
}

/// A file's first `cap` bytes, and whether it is longer; a file that is not a plain file (a FIFO, a device) is never opened.
fn read_bytes(path: &Path, cap: u64) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let m = std::fs::metadata(path)?;
    if !m.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)?.take(cap).read_to_end(&mut buf)?;
    Ok((buf, m.len() > cap))
}

/// `buf` as text: `Ok` when it is UTF-8, else `Err` with a lossy reading. A character the cap cut in two is the
/// cap's doing, not the file's: it is dropped instead of shown as a replacement mark.
fn decode(buf: &[u8], truncated: bool) -> Result<String, String> {
    match std::str::from_utf8(buf) {
        Ok(s) => Ok(s.to_string()),
        Err(e) if truncated && e.error_len().is_none() => Ok(String::from_utf8_lossy(&buf[..e.valid_up_to()]).into_owned()),
        Err(_) => Err(String::from_utf8_lossy(buf).into_owned()),
    }
}

/// A file's text up to the cap, bytes that are not UTF-8 read as replacement marks.
pub fn read_capped(path: &Path, cap: u64) -> std::io::Result<(String, bool)> {
    let (buf, truncated) = read_bytes(path, cap)?;
    Ok((decode(&buf, truncated).unwrap_or_else(|lossy| lossy), truncated))
}

/// A file's text up to the cap; `None` when it is not text (it holds a NUL, or is not UTF-8).
pub fn read_text(path: &Path, cap: u64) -> std::io::Result<(Option<String>, bool)> {
    let (buf, truncated) = read_bytes(path, cap)?;
    Ok((decode(&buf, truncated).ok().filter(|t| !t.contains('\0')), truncated))
}

/// Files in a skill's folder (relative paths with sizes), besides the skill file and Workbench's marker.
pub fn folder_files(dir: &Path, limit: usize) -> (Vec<(String, u64, bool)>, bool) {
    let mut out = vec![];
    let mut truncated = false;
    fn walk(base: &Path, dir: &Path, depth: usize, limit: usize, out: &mut Vec<(String, u64, bool)>, truncated: &mut bool) {
        // A folder of millions of names is read as far as the list could ever use, and the list says it is cut.
        let (v, more) = read_dir_sorted(dir, limit.saturating_mul(4));
        if more {
            *truncated = true;
        }
        for e in v {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == super::catalog::MARKER || name == "__pycache__" || name == ".git" {
                continue;
            }
            let path = e.path();
            // Links are listed as what they are, not followed: a skill folder is not a way into the disk.
            let Ok(t) = e.file_type() else { continue };
            let rel = path.strip_prefix(base).map(ospath::to_slash).unwrap_or_default();
            if out.len() >= limit {
                *truncated = true;
                return;
            }
            if t.is_dir() {
                out.push((rel, 0, true));
                if depth > 1 {
                    walk(base, &path, depth - 1, limit, out, truncated);
                }
            } else if !(dir == base && name == "SKILL.md") {
                out.push((rel, e.metadata().map(|m| m.len()).unwrap_or(0), false));
            }
        }
    }
    walk(dir, dir, FILE_DEPTH, limit, &mut out, &mut truncated);
    (out, truncated)
}

// ---------------------------------------------------------------- the scan

fn id_of(kind: &str, path: &Path) -> String {
    let canon = ospath::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(b"|");
    h.update(ospath::to_slash(&canon).as_bytes());
    format!("s{}", &hex::encode(h.finalize())[..12])
}

/// The id of a built-in skill (by its name).
pub fn builtin_id(name: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"builtin|");
    h.update(name.as_bytes());
    format!("b{}", &hex::encode(h.finalize())[..12])
}

fn invoke_for(cli: Cli, qualified: &str, name: &str, f: &Flags, command: bool) -> Option<String> {
    match cli {
        Cli::Claude => (!f.model_only).then(|| format!("/{qualified}")),
        // Codex: `$name` or the `/skills` list; Kimi Code: `/skill:name`; Gemini CLI has the model activate.
        Cli::Codex => (!command).then(|| format!("${name}")),
        Cli::Kimi => (!command).then(|| format!("/skill:{name}")),
        Cli::Gemini => None,
    }
}

fn qualified_of(scope: Scope, label: Option<&str>, name: &str) -> String {
    match (scope, label) {
        (Scope::Plugin, Some(p)) => format!("{p}:{name}"),
        (Scope::Synced, _) => format!("anthropic-skills:{name}"),
        (Scope::Nested, Some(rel)) if !rel.is_empty() => format!("{rel}:{name}"),
        _ => name.to_string(),
    }
}

/// How one account that reads a skill sees it. A setting turns a skill off for the account it belongs to, and a
/// personal skill hides a project skill of its name only for the account that has it: accounts never meet, so
/// neither may decide for the others.
#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Active,
    /// Listed without its description (`skillOverrides`).
    NameOnly,
    /// Off, and why: a plugin that is not enabled, `skillOverrides`, Codex's `[[skills.config]]`.
    Off(String),
    /// Hidden by the skill of the same name its CLI uses instead (`scope_word` of the winner).
    Hidden(String),
}

#[derive(Debug, Clone)]
struct Seen {
    mode: Mode,
    /// Only the user starts it, by this account's settings (`user-invocable-only`).
    manual: bool,
}

impl Seen {
    fn usable(&self) -> bool {
        matches!(self.mode, Mode::Active | Mode::NameOnly)
    }
}

struct Entry {
    info: SkillInfo,
    located: Located,
    /// How each account that reads it (an index into the accounts) sees it.
    seen: BTreeMap<usize, Seen>,
    command: bool,
    /// The listing line's size in characters, with `description` cut where the CLI cuts it.
    line_chars: usize,
}

fn flags_of(p: &Parsed, dir: Option<&Path>) -> Flags {
    let fork = p.text("context").is_some_and(|c| c == "fork");
    Flags {
        manual_only: p.flag("disable-model-invocation").unwrap_or(false),
        model_only: p.flag("user-invocable") == Some(false),
        preapproves: p.list("allowed-tools"),
        hooks: p.fields.get("hooks").is_some_and(|h| !h.is_null()),
        forks: fork.then(|| p.text("agent").unwrap_or_else(|| "general-purpose".into())),
        shell: parse::uses_shell_injection(&p.body),
        scripts: dir.is_some_and(|d| is_dir(&d.join("scripts"))),
        paths: p.list("paths"),
        model: p.text("model").filter(|m| m != "inherit"),
        argument_hint: p.text("argument-hint"),
    }
}

/// A skill file with the root that found it first and every account that reads it.
struct Gathered<'a> {
    root: &'a Root,
    hit: Hit,
    readers: BTreeSet<usize>,
}

/// What a scan had to leave out.
#[derive(Debug, Default)]
struct Cuts {
    /// The cap on skills was reached.
    skills: bool,
    /// A root's reading was stopped by a bound.
    reading: bool,
}

fn scan(inp: &Inputs, roots: &Roots) -> (Vec<Entry>, Cuts) {
    let lim = inp.limits;
    let mut by_file: HashMap<PathBuf, usize> = HashMap::new();
    let mut gathered: Vec<Gathered> = vec![];
    let mut cuts = Cuts::default();
    let mut untrusted_left = lim.untrusted_work;
    for r in &roots.list {
        // A root may cost `root_work`, and the roots of someone else's content only `untrusted_work` together:
        // a repository can fill its own roots with names and links, never the user's.
        let untrusted = r.boundary.is_some();
        let allowed = if untrusted { lim.root_work.min(untrusted_left) } else { lim.root_work };
        let mut walk = Walk::new(allowed, lim.entries);
        let hits = hits_in(r, &mut walk);
        if untrusted {
            untrusted_left -= allowed - walk.left;
        }
        cuts.reading |= walk.cut;
        for hit in hits {
            let canon = ospath::canonicalize(&hit.file).unwrap_or_else(|_| hit.file.clone());
            if let Some(&i) = by_file.get(&canon) {
                // The same file reached through another folder (a link, or `~/.agents/skills`, which three
                // CLIs read): one skill, more readers.
                gathered[i].readers.extend(r.readers.iter().copied());
                continue;
            }
            if gathered.len() >= lim.skills {
                cuts.skills = true;
                break;
            }
            if r.under_project && ospath::leaves_machine_below(&inp.root, &hit.file) {
                continue;
            }
            by_file.insert(canon, gathered.len());
            gathered.push(Gathered { root: r, hit, readers: r.readers.clone() });
        }
    }
    (gathered.iter().map(|g| entry_of(inp, g.root, &g.hit, &g.readers)).collect(), cuts)
}

/// Whether `file` lies inside `boundary` once links are resolved.
fn contained(file: &Path, boundary: &Path) -> bool {
    ospath::canonicalize(file).ok().zip(ospath::canonicalize(boundary).ok()).is_some_and(|(f, top)| ospath::starts_with(&f, &top))
}

/// Where a root's content is meant to stay, in words for the warning.
fn place(scope: Scope) -> &'static str {
    match scope {
        Scope::Project | Scope::Nested => "the project's repository",
        Scope::Plugin => "its plugin",
        Scope::Synced => "claude.ai's synced folder",
        _ => "its folder",
    }
}

/// Whether two folders are one once links are resolved.
fn same_folder(a: &Path, b: &Path) -> bool {
    ospath::canonicalize(a).ok().zip(ospath::canonicalize(b).ok()).is_some_and(|(x, y)| ospath::starts_with(&x, &y) && ospath::starts_with(&y, &x))
}

/// The Workbench skill a found skill is. Only a skill that sits directly in the skills folder of an account Workbench's
/// skills are installed for can be (that is what Install writes and what the catalog follows: a copy anywhere else is
/// an ordinary skill, and a repository could forge a marker), and only one of the bundle whose marker holds up.
fn workbench_label(inp: &Inputs, r: &Root, hit: &Hit, readers: &BTreeSet<usize>) -> Option<String> {
    // The cheap check first: nearly no skill folder has a marker.
    let name = super::catalog::installed_as(&hit.entry)?;
    let direct = hit.entry.parent().is_some_and(|p| same_folder(p, &r.dir));
    let theirs = readers.iter().map(|&ai| &inp.accounts[ai]).any(|a| super::catalog::is_target(a) && same_folder(&a.skills_dir(), &r.dir));
    (direct && theirs).then_some(name)
}

fn entry_of(inp: &Inputs, r: &Root, hit: &Hit, readers: &BTreeSet<usize>) -> Entry {
    let mut issues: Vec<Issue> = vec![];
    let readable = r.boundary.as_deref().is_none_or(|b| contained(&hit.file, b));
    let (text, truncated) = if !readable {
        issues.push(Issue::new(
            "warn",
            "link_out",
            format!("This skill is a link to a file outside {}. Agents follow such links; Workbench does not read through them, so its description and text are not shown", place(r.scope)),
        ));
        (String::new(), false)
    } else {
        match read_capped(&hit.file, parse::MAX_BYTES) {
            Ok(t) => t,
            Err(e) => {
                issues.push(Issue::new("error", "unreadable", format!("SKILL.md could not be read ({})", e.kind())));
                (String::new(), false)
            }
        }
    };
    // Not text: its bytes are no description and nothing to parse.
    let (text, truncated) = if text.contains('\0') {
        issues.push(Issue::new("error", "not_text", "SKILL.md is not a text file (it holds NUL bytes)"));
        (String::new(), false)
    } else {
        (text, truncated)
    };
    let bytes = if readable { std::fs::metadata(&hit.file).map(|m| m.len()).unwrap_or(0) } else { 0 };
    let p = if truncated {
        issues.push(Issue::new("error", "too_big", format!("The file is larger than {} KB: Workbench reads no further", parse::MAX_BYTES / 1024)));
        Parsed::default()
    } else {
        parse::parse(&text)
    };
    let lines = text.lines().count();
    let clis: BTreeSet<Cli> = readers.iter().map(|&i| inp.accounts[i].cli).collect();
    let (claude, others) = (clis.contains(&Cli::Claude), clis.iter().any(|c| *c != Cli::Claude));
    let name = if hit.command { hit.folder.clone() } else { p.name().unwrap_or_else(|| hit.folder.clone()) };
    let described = p.description();
    let from_text = described.is_none();
    let description = described.or_else(|| parse::first_line(&p.body)).unwrap_or_default();
    let when_to_use = p.when_to_use();
    let flags = flags_of(&p, (!hit.command && readable).then_some(hit.entry.as_path()));
    // A command has no folder to match and its name is its file's. A file that could not be read says so once.
    if issues.is_empty() {
        issues.extend(parse::check(&p, if hit.command { &name } else { &hit.folder }, lines, claude, others, hit.command));
    }
    let qualified = qualified_of(r.scope, r.label.as_deref(), &name);
    let (kind, tag) = if hit.command { ("command", "c") } else { ("skill", "s") };
    let invoke = clis.iter().filter_map(|c| invoke_for(*c, &qualified, &name, &flags, hit.command).map(|text| Invoke { agent: *c, text })).collect();
    let files = if hit.command || !readable { 0 } else { folder_files(&hit.entry, MAX_FILES).0.iter().filter(|(_, _, dir)| !dir).count() };
    let cap = parse::CLAUDE_LISTING_CAP;
    let described_chars = (description.chars().count() + when_to_use.as_deref().map_or(0, |w| w.chars().count())).min(cap);
    let line_chars = qualified.chars().count() + 4 + described_chars;
    let workbench = if hit.command || r.scope != Scope::User { None } else { workbench_label(inp, r, hit, readers) };
    let id = id_of(tag, &hit.file);
    let mode = if r.scope == Scope::Plugin && !r.plugin_enabled { Mode::Off(format!("the plugin {} is not enabled", r.label.as_deref().unwrap_or("it belongs to"))) } else { Mode::Active };
    let seen = readers.iter().map(|&ai| (ai, Seen { mode: mode.clone(), manual: false })).collect();
    Entry {
        info: SkillInfo {
            id: id.clone(),
            name,
            qualified,
            kind,
            scope: r.scope,
            scope_label: r.label.clone(),
            description: description.chars().take(4000).collect(),
            description_from_text: from_text,
            when_to_use: when_to_use.map(|w| w.chars().take(2000).collect()),
            agents: clis.iter().copied().collect(),
            accounts: readers.iter().map(|&i| inp.accounts[i].id.clone()).collect(),
            path: contract_tilde(&hit.entry),
            project_file: hit.file.strip_prefix(&inp.root).ok().filter(|_| readable).map(ospath::to_slash),
            invoke,
            tokens: 0,
            bytes,
            lines,
            files,
            flags,
            status: "active",
            status_reason: None,
            in_session: None,
            workbench,
            issues,
        },
        located: Located { id, file: hit.file.clone(), dir: (!hit.command).then(|| hit.entry.clone()), under_project: r.under_project, readable },
        seen,
        command: hit.command,
        line_chars,
    }
}

/// Which skill hides which: the CLIs resolve a name used twice by scope, and a hidden skill is
/// not in their listing. Claude Code: managed over personal over project, a skill over a command
/// file; Kimi Code: project over personal over extra; Gemini CLI: project over personal. Codex does
/// not merge them (both are listed), so a repeated name is only a remark there. Plugins and synced
/// skills carry a namespace, so they never meet another.
fn rank(cli: Cli, e: &Entry) -> Option<u8> {
    let s = e.info.scope;
    let base = match (cli, s) {
        (Cli::Claude, Scope::Managed) => 0,
        (Cli::Claude, Scope::User) => 1,
        (Cli::Claude, Scope::Project) => 2,
        (Cli::Kimi, Scope::Project) | (Cli::Gemini, Scope::Project) => 0,
        (Cli::Kimi | Cli::Gemini, Scope::User) => 1,
        (Cli::Kimi, Scope::Extra) => 2,
        _ => return None,
    };
    Some(base + if e.command { 10 } else { 0 })
}

/// Each account's own view: a skill hides another of its name only for an account that reads both, and a skill
/// that is off does not hide another of its name.
fn apply_precedence(inp: &Inputs, entries: &mut [Entry]) {
    for (ai, a) in inp.accounts.iter().enumerate() {
        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, e) in entries.iter().enumerate() {
            let Some(seen) = e.seen.get(&ai) else { continue };
            if matches!(seen.mode, Mode::Off(_)) || rank(a.cli, e).is_none() {
                continue;
            }
            by_name.entry(e.info.qualified.clone()).or_default().push(i);
        }
        for (_, mut idx) in by_name {
            if idx.len() < 2 {
                continue;
            }
            idx.sort_by_key(|&i| rank(a.cli, &entries[i]).unwrap_or(u8::MAX));
            let by = scope_word(&entries[idx[0]]);
            for &i in &idx[1..] {
                if let Some(seen) = entries[i].seen.get_mut(&ai) {
                    seen.mode = Mode::Hidden(by.clone());
                }
            }
        }
    }
}

/// A skill's one status, from how each account sees it: it is as available as the best of them. Where the
/// accounts differ the status says it is available and a remark says for whom it is not.
fn settle(inp: &Inputs, entries: &mut [Entry]) {
    for e in entries.iter_mut() {
        let modes: Vec<(usize, Mode)> = e.seen.iter().map(|(&ai, s)| (ai, s.mode.clone())).collect();
        let off = modes.iter().find_map(|(_, m)| if let Mode::Off(why) = m { Some(why.clone()) } else { None });
        let hidden = modes.iter().find_map(|(_, m)| if let Mode::Hidden(by) = m { Some(by.clone()) } else { None });
        let (status, reason) = if modes.iter().any(|(_, m)| *m == Mode::Active) {
            ("active", None)
        } else if modes.iter().any(|(_, m)| *m == Mode::NameOnly) {
            ("name-only", Some("listed by name only (skillOverrides): its description is not in the listing".to_string()))
        } else if let Some(why) = off {
            ("disabled", Some(why))
        } else if let Some(by) = &hidden {
            ("shadowed", Some(format!("hidden by the {by} skill of the same name")))
        } else {
            ("active", None)
        };
        e.info.status = status;
        e.info.status_reason = reason;
        if let (true, Some(by)) = (status == "shadowed", &hidden) {
            e.info.issues.push(Issue::new("info", "shadowed", format!("Hidden by the {by} skill of the same name: agents list that one")));
        }
        if matches!(status, "active" | "name-only") {
            for (ai, m) in &modes {
                let label = &inp.accounts[*ai].label;
                let note = match m {
                    Mode::Off(why) => Some(format!("Off for {label}: {why}. The accounts that have it on still list it")),
                    Mode::Hidden(by) => Some(format!("Hidden for {label} by the {by} skill of the same name. The accounts that have no such skill list this one")),
                    Mode::NameOnly if status == "active" => Some(format!("Listed by name only for {label} (skillOverrides)")),
                    _ => None,
                };
                if let Some(note) = note {
                    e.info.issues.push(Issue::new("info", "per_account", note));
                }
            }
        }
        if !e.seen.is_empty() && e.seen.values().all(|s| s.manual) {
            e.info.flags.manual_only = true;
        }
    }
}

fn scope_word(e: &Entry) -> String {
    match e.info.scope {
        Scope::Managed => "organization's",
        Scope::User => "personal",
        Scope::Project => "project",
        Scope::Extra => "extra-folder",
        _ => "other",
    }
    .into()
}

/// Codex lists same-named skills side by side; a repeated name among the skills of one account is worth a remark.
fn note_codex_duplicates(inp: &Inputs, entries: &mut [Entry]) {
    let mut noted: BTreeSet<usize> = BTreeSet::new();
    for (ai, _) in inp.accounts.iter().enumerate().filter(|(_, a)| a.cli == Cli::Codex) {
        let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, e) in entries.iter().enumerate().filter(|(_, e)| e.seen.contains_key(&ai)) {
            by_name.entry(e.info.name.clone()).or_default().push(i);
        }
        for (name, idx) in by_name.into_iter().filter(|(_, idx)| idx.len() > 1) {
            for i in idx.into_iter().filter(|i| noted.insert(*i)) {
                entries[i].info.issues.push(Issue::new("info", "duplicate_name", format!("More than one skill is called “{name}”: Codex lists them all, and an agent may pick either")));
            }
        }
    }
}

/// `skillOverrides` (Claude Code) and `[[skills.config]]` (Codex), each onto the skills of the account whose
/// settings they are.
fn apply_overrides(inp: &Inputs, entries: &mut [Entry]) {
    for (ai, a) in inp.accounts.iter().enumerate().filter(|(_, a)| a.cli == Cli::Claude) {
        let s = claude_settings(a, inp);
        for e in entries.iter_mut().filter(|e| e.info.scope != Scope::Plugin) {
            let Some(seen) = e.seen.get_mut(&ai) else { continue };
            let state = s.overrides.get(&e.info.qualified).or_else(|| s.overrides.get(&e.info.name));
            match (state.map(String::as_str), &seen.mode) {
                (Some("off"), Mode::Active) => seen.mode = Mode::Off("turned off in Claude Code's settings (skillOverrides)".into()),
                (Some("name-only"), Mode::Active) => seen.mode = Mode::NameOnly,
                (Some("user-invocable-only"), _) => seen.manual = true,
                _ => {}
            }
        }
    }
    for (ai, a) in inp.accounts.iter().enumerate().filter(|(_, a)| a.cli == Cli::Codex) {
        let off = codex_disabled(a);
        if off.is_empty() {
            continue;
        }
        for e in entries.iter_mut() {
            let Some(seen) = e.seen.get_mut(&ai).filter(|s| s.mode == Mode::Active) else { continue };
            let file = ospath::canonicalize(&e.located.file).unwrap_or_else(|_| e.located.file.clone());
            let dir = e.located.dir.as_ref().and_then(|d| ospath::canonicalize(d).ok());
            if off.contains(&file) || dir.is_some_and(|d| off.contains(&d)) {
                seen.mode = Mode::Off("turned off in Codex's config.toml ([[skills.config]])".into());
            }
        }
    }
}

/// A skill Claude Code listed that no folder here explains: one of its own (or one that was removed since the session began: nothing on disk tells them apart).
fn builtin_skill(name: String, description: String, accounts: Vec<String>, tokens: u64) -> SkillInfo {
    SkillInfo {
        id: builtin_id(&name),
        invoke: vec![Invoke { agent: Cli::Claude, text: format!("/{name}") }],
        qualified: name.clone(),
        name,
        kind: "builtin",
        scope: Scope::Builtin,
        scope_label: None,
        description_from_text: false,
        description,
        when_to_use: None,
        agents: vec![Cli::Claude],
        accounts,
        path: String::new(),
        project_file: None,
        tokens,
        bytes: 0,
        lines: 0,
        files: 0,
        flags: Flags::default(),
        status: "active",
        status_reason: None,
        in_session: Some(true),
        workbench: None,
        issues: vec![],
    }
}

/// The whole picture for a project.
pub fn discover(inp: &Inputs) -> Discovery {
    let mut nested = Walk::new(inp.limits.nested_work, inp.limits.entries);
    let roots = roots_of(inp, &mut nested);
    let (mut entries, cuts) = scan(inp, &roots);
    // Settings first: a skill that is turned off does not hide another of its name.
    apply_overrides(inp, &mut entries);
    apply_precedence(inp, &mut entries);
    settle(inp, &mut entries);
    note_codex_duplicates(inp, &mut entries);

    let mut notes = vec![];
    if cuts.skills || cuts.reading {
        notes.push(format!(
            "The search was cut short ({} skills, {} names in one folder and a bound on how much of a repository's or a plugin's folders is read, at most): some skills may be missing",
            inp.limits.skills, inp.limits.entries
        ));
    }
    if nested.cut {
        notes.push("The search for skills in folders below the project was cut short (a bound on how many are read): some may be missing".to_string());
    }

    // What Claude Code received in each of its accounts' newest session here.
    let listings: BTreeMap<usize, session::Listing> = inp
        .accounts
        .iter()
        .enumerate()
        .filter(|(_, a)| a.cli == Cli::Claude)
        .filter_map(|(i, a)| session::latest(&a.dir, &inp.root).map(|l| (i, l)))
        .collect();
    let mut explained: HashSet<String> = HashSet::new();
    for e in entries.iter_mut().filter(|e| e.info.agents.contains(&Cli::Claude)) {
        explained.insert(e.info.qualified.clone());
        // The listing says nothing about a nested skill (listed once a file below it is read), nor about one that is
        // off or hidden for an account, or that only the user starts: none of them is meant to be in it.
        if e.info.scope == Scope::Nested {
            continue;
        }
        let manual = e.info.flags.manual_only;
        let seen_by: Vec<&session::Listing> = listings.iter().filter(|(ai, _)| e.seen.get(ai).is_some_and(|s| s.usable() && !(s.manual || manual))).map(|(_, l)| l).collect();
        if !seen_by.is_empty() {
            e.info.in_session = Some(seen_by.iter().any(|l| l.entries.iter().any(|x| x.name == e.info.qualified)));
        }
    }

    // What each skill's listing line costs, what each account's listing costs in all, and how many skills it has.
    let mut chars_of_account = vec![0usize; inp.accounts.len()];
    let mut skills_of_account = vec![0usize; inp.accounts.len()];
    for e in entries.iter_mut() {
        let name_line = e.info.qualified.chars().count() + 4;
        let manual = e.info.flags.manual_only;
        // What the skill costs in a session that lists it: the longest line among the accounts that do.
        let mut line = 0;
        for (&ai, s) in &e.seen {
            if !s.usable() {
                continue;
            }
            skills_of_account[ai] += 1;
            // One that only the user starts is not listed.
            if s.manual || manual {
                continue;
            }
            let chars = if s.mode == Mode::NameOnly { name_line } else { e.line_chars };
            line = line.max(chars);
            // A nested skill is listed once a file below it is read: not part of what every session carries.
            if e.info.scope != Scope::Nested {
                chars_of_account[ai] += chars;
            }
        }
        e.info.tokens = if line > 0 { parse::tokens_of(line) } else { 0 };
    }

    // What a session listed that no folder explains. Workbench's own skills are no such thing: one that was
    // installed when the session began and removed since is not Claude Code's.
    let mut builtins: BTreeMap<String, (String, Vec<usize>)> = BTreeMap::new();
    for (&ai, l) in &listings {
        for x in l.entries.iter().filter(|x| !explained.contains(&x.name) && !super::catalog::is_bundled(&x.name)) {
            builtins.entry(x.name.clone()).or_insert_with(|| (x.description.clone(), vec![])).1.push(ai);
        }
    }

    let (mut skills, mut located) = (Vec::with_capacity(entries.len() + builtins.len()), Vec::with_capacity(entries.len()));
    for e in entries {
        located.push(e.located);
        skills.push(e.info);
    }
    for (name, (description, readers)) in builtins {
        let chars = name.chars().count() + 4 + description.chars().count().min(parse::CLAUDE_LISTING_CAP);
        for &ai in &readers {
            chars_of_account[ai] += chars;
            skills_of_account[ai] += 1;
        }
        skills.push(builtin_skill(name, description, readers.iter().map(|&i| inp.accounts[i].id.clone()).collect(), parse::tokens_of(chars)));
    }
    skills.sort_by(|a, b| (a.scope, a.scope_label.as_deref(), a.kind, &a.qualified).cmp(&(b.scope, b.scope_label.as_deref(), b.kind, &b.qualified)));

    let accounts = inp
        .accounts
        .iter()
        .enumerate()
        .map(|(ai, a)| AccountReport {
            id: a.id.clone(),
            cli: a.cli,
            label: a.label.clone(),
            installed: a.installed,
            enabled: a.enabled,
            mcp: a.mcp,
            folder: contract_tilde(&a.dir),
            skills: skills_of_account[ai],
            listing_tokens: parse::tokens_of(chars_of_account[ai]),
            session: listings.get(&ai).map(|l| SessionNote { count: l.entries.len(), chars: l.chars, tokens: parse::tokens_of(l.chars), at_ms: l.at_ms, version: l.version.clone() }),
        })
        .collect();
    Discovery { skills, located, accounts, notes }
}
