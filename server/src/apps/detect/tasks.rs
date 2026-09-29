//! Task runners → runs: Makefile targets, justfile recipes, Taskfile tasks and
//! Procfile processes. Nothing is executed to list them (`make -qp`, `just --list`
//! and `task --list` would run repository code): the files are parsed.
//!
//! * **Make:** `.PHONY` targets and targets with a conventional name (`run`,
//!   `dev`, `test`, `lint`, `build`, `fmt`, `clean`…); pattern rules (`%.o`),
//!   variables, special (`.PHONY`) and private (`_x`) targets are skipped, as are
//!   generated Makefiles (CMake, automake).
//! * **just:** public recipes (`just --list` semantics: no `_x`, no `[private]`, no
//!   recipes that need arguments, none limited to another OS), except a body-less
//!   alias of one other offered recipe (`default: up`).
//! * **Task:** tasks that are not `internal` (or limited to another platform).
//! * **Procfile:** one run per process (not Heroku's `release` phase, not one
//!   that repeats a run already detected); `web` is a server. A process that reads
//!   `$PORT` gets `PORT=5000` as under foreman, unless the line names a port.
//!
//! The kind comes from the name (`kind_from_task_name`). The group comes from
//! everything the entry runs (`TaskGraph`): its own recipe plus, transitively, Make
//! prerequisites and `$(MAKE) x` (with the Makefile's simple variables expanded),
//! just dependencies and `just x`, Taskfile `deps` and `task: x`. When any of it
//! reaches another machine or publishes (`ssh`, `rsync`, `docker push`, `deploy`…)
//! the run is in group `deploy`. Entry names go into commands shell-quoted (`sh`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, TaskGraph, command_words, deploy_or, kind_from_task_name, on_path, port_in, scoped, sh, source, task_group};
use crate::config::project::{Ready, RunConfig, RunKind};

/// Entries offered per file.
const MAX_PER_FILE: usize = 30;
/// Makefile bounds (a generated or hostile file must not make detection slow or
/// large): targets recorded, recipe text kept per target and in all, prerequisites
/// per target, and bytes that variable expansion may add to the whole file.
const MAX_TARGETS: usize = 2000;
const MAX_RECIPE: usize = 64 * 1024;
const MAX_RECIPES_TOTAL: usize = 8 * 1024 * 1024;
const MAX_PREREQS: usize = 200;
const MAX_EXPANSION: usize = 1024 * 1024;

/// Make targets offered even without `.PHONY`.
const COMMON_TARGETS: &[&str] = &[
    "run", "dev", "serve", "server", "start", "up", "down", "watch", "test", "tests", "check", "lint", "fmt", "format",
    "build", "all", "clean", "install", "docs", "doc", "bench", "coverage", "generate", "gen", "migrate", "setup",
    "bootstrap", "deps", "tidy", "vet", "e2e", "release", "deploy", "publish", "help", "typecheck", "dist",
];

static MAKE_RULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_.][^:=#\t]*?)\s*::?(?:[^=]|$)").unwrap());
/// `VAR = x`, `VAR := x`, `VAR ?= x`, `VAR += x`, `export VAR = x`.
static MAKE_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:export|override)\s+)*([A-Za-z_][A-Za-z0-9_.-]*)\s*(\+=|\?=|:::=|::=|:=|!=|=)\s*(.*)$").unwrap()
});
/// `$(VAR)` / `${VAR}` (and `$$`, an escaped dollar, left alone).
static MAKE_VAR_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\$|\$\(([A-Za-z_][A-Za-z0-9_.-]*)\)|\$\{([A-Za-z_][A-Za-z0-9_.-]*)\}").unwrap());
static JUST_RECIPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(@?)([A-Za-z_][A-Za-z0-9_-]*)((?:\s+[^:]*?)?)\s*:([^=].*)?$").unwrap());
static JUST_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_-]*$").unwrap());
static PROC_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_-]+):\s*(\S.*)$").unwrap());

/// One entry of a task runner file.
pub(super) struct Entry {
    pub name: String,
    /// Its own recipe lines / commands, for `port_in`.
    pub body: String,
    /// Whether it, or anything it runs (`TaskGraph`), reaches another machine or publishes.
    pub reaches_out: bool,
}

/// Add the entries of one task file as runs named `<tool> <entry>`.
fn add_entries(cx: &mut Ctx, f: &Path, tool: &str, command: &dyn Fn(&str) -> String, entries: Vec<Entry>) {
    let Some(dir) = f.parent() else { return };
    let cwd = cx.rel(dir);
    for e in entries.into_iter().take(MAX_PER_FILE) {
        let kind = kind_from_task_name(&e.name);
        let port = (kind == RunKind::Server).then(|| port_in(&e.body)).flatten();
        cx.add_run(RunConfig {
            name: scoped(&format!("{tool} {}", e.name), &cwd),
            kind,
            command: command(&e.name),
            cwd: cwd.clone(),
            port,
            ready: http_server_ready(kind, &e.body),
            preview: port.map(|p| format!("http://localhost:{p}/")),
            source: source(cx, f, &format!("#{}", e.name)),
            group: Some(deploy_or(kind, e.reaches_out)),
            ..Default::default()
        });
    }
}

/// The ready line of a server that is Python's `http.server`.
fn http_server_ready(kind: RunKind, body: &str) -> Option<Ready> {
    (kind == RunKind::Server && super::is_http_server(body))
        .then(|| Ready { log: Some(super::HTTP_SERVER_READY.into()), http: None, timeout_s: 60 })
}

/// The words after a task runner's name that name entries of the same file:
/// `make a b`, `just x`, `task y`. `None` when the invocation is aimed at another
/// file or directory (`make -C sub`, `just -f other`, `task -d dir`).
fn invoked(words: &[String], other_file: &[&str]) -> Option<Vec<String>> {
    let mut out = vec![];
    for w in words {
        let elsewhere = other_file.iter().any(|f| {
            w == f || (f.starts_with("--") && w.starts_with(&format!("{f}="))) || (f.len() == 2 && w.len() > 2 && w.starts_with(f))
        });
        if elsewhere {
            return None;
        }
        if !w.starts_with('-') && !w.contains('=') {
            out.push(w.clone());
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- make

pub fn detect_make(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let entries = make_targets(&src);
    if entries.is_empty() {
        return;
    }
    cx.tag("make");
    add_entries(cx, f, "make", &|t| format!("make {}", sh(t)), entries);
}

/// Simple variables of a Makefile, for `$(SSH) host` and `ship: $(STEPS)`.
struct MakeVars {
    vars: BTreeMap<String, String>,
    /// Bytes that expansion may still add for the whole file: nested variables
    /// that double at every level must not make detection slow or large.
    budget: std::cell::Cell<usize>,
}

impl MakeVars {
    /// `$(VAR)` / `${VAR}` replaced by their values (recursively, a few levels
    /// deep); functions (`$(shell …)`), automatic and unknown variables stay.
    fn expand(&self, s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        self.expand_into(s, 0, &mut out);
        out
    }

    fn expand_into(&self, s: &str, depth: usize, out: &mut String) {
        if depth > 4 || !s.contains('$') || self.budget.get() == 0 {
            out.push_str(s);
            return;
        }
        let mut last = 0;
        for c in MAKE_VAR_REF.captures_iter(s) {
            let Some(whole) = c.get(0) else { continue };
            out.push_str(&s[last..whole.start()]);
            last = whole.end();
            match c.get(1).or_else(|| c.get(2)).and_then(|n| self.vars.get(n.as_str())) {
                Some(v) if v.len() <= self.budget.get() => {
                    self.budget.set(self.budget.get() - v.len());
                    self.expand_into(v, depth + 1, out);
                }
                _ => out.push_str(whole.as_str()),
            }
        }
        out.push_str(&s[last..]);
    }
}

/// Targets a recipe runs through a nested make of the same Makefile: `$(MAKE) push`,
/// `make build`.
fn make_recipe_refs(recipe: &str) -> Vec<String> {
    let mut out = vec![];
    for words in command_words(recipe) {
        let is_make = |w: &String| matches!(w.trim_start_matches(['@', '-', '+']), "$(MAKE)" | "${MAKE}" | "make" | "gmake");
        let Some(i) = words.iter().position(is_make) else { continue };
        if let Some(targets) = invoked(&words[i + 1..], &["-C", "-f", "--directory", "--file", "--makefile"]) {
            out.extend(targets);
        }
    }
    out
}

/// Offered targets of a Makefile, in file order (conventional names first when
/// there are more than `MAX_PER_FILE`).
pub(super) fn make_targets(src: &str) -> Vec<Entry> {
    let head: String = src.lines().take(8).collect::<Vec<_>>().join("\n").to_ascii_lowercase();
    if head.contains("generated by") || head.contains("generated automatically") || head.contains("cmake generated") {
        return vec![];
    }
    // Join `\` continuations first (`.PHONY: a \` / `  b`).
    let mut lines: Vec<String> = vec![];
    for raw in src.lines() {
        match lines.last_mut() {
            Some(prev) if prev.ends_with('\\') => {
                prev.pop();
                prev.push(' ');
                prev.push_str(raw.trim());
            }
            _ => lines.push(raw.to_string()),
        }
    }
    // Simple variables, for `$(SSH) host` and `ship: $(STEPS)`.
    let mut vars: BTreeMap<String, String> = BTreeMap::new();
    for line in lines.iter().filter(|l| !l.starts_with('\t')) {
        let Some(c) = MAKE_ASSIGN.captures(line.trim_end()) else { continue };
        let (name, value) = (c[1].to_string(), c[3].split(" #").next().unwrap_or("").trim().to_string());
        match &c[2] {
            "+=" => {
                let v = vars.entry(name).or_default();
                if !v.is_empty() {
                    v.push(' ');
                }
                v.push_str(&value);
            }
            "?=" => {
                vars.entry(name).or_insert(value);
            }
            "!=" => {}
            _ => {
                vars.insert(name, value);
            }
        }
    }
    let vars = MakeVars { vars, budget: std::cell::Cell::new(MAX_EXPANSION) };
    let mut phony: BTreeSet<String> = BTreeSet::new();
    // Every target in file order: (name, recipe, prerequisites).
    let mut targets: Vec<(String, String, Vec<String>)> = vec![];
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut current: Vec<usize> = vec![];
    // A rule with many targets repeats its recipe for each: the text kept is bounded
    // per target and for the whole file.
    let mut stored = 0usize;
    let mut push_recipe = |t: &mut (String, String, Vec<String>), text: &str| {
        let room = MAX_RECIPE.saturating_sub(t.1.len()).min(MAX_RECIPES_TOTAL.saturating_sub(stored));
        let mut end = text.len().min(room);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end > 0 {
            t.1.push_str(&text[..end]);
            t.1.push('\n');
            stored += end + 1;
        }
    };
    for line in &lines {
        if line.starts_with('\t') {
            if !current.is_empty() {
                let recipe = vars.expand(line.trim());
                for &i in &current {
                    push_recipe(&mut targets[i], &recipe);
                }
            }
            continue;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        current.clear();
        // Assignments (`X := y`, `X ::= y`, target-specific `t: X := y`) are not rules.
        if line.contains(":=") {
            continue;
        }
        let Some(c) = MAKE_RULE.captures(line) else { continue };
        let names = c[1].trim();
        if let Some(rest) = line.strip_prefix(".PHONY") {
            let list = rest.trim_start().trim_start_matches(':');
            phony.extend(vars.expand(list.split('#').next().unwrap_or("")).split_whitespace().map(str::to_string));
            continue;
        }
        // `target: prereqs ; recipe` and `target: ## help text`.
        let after = line.split_once(':').map(|(_, r)| r).unwrap_or("");
        let after = after.strip_prefix(':').unwrap_or(after);
        let (prereq_text, inline) = match after.split_once(';') {
            Some((p, r)) => (p, vars.expand(r.trim())),
            None => (after, String::new()),
        };
        let prereq_text = prereq_text.split('#').next().unwrap_or("");
        // `target: VAR = value` sets a variable for the target; it names no prerequisite.
        let prereqs: Vec<String> = if prereq_text.contains('=') {
            vec![]
        } else {
            vars.expand(prereq_text).split_whitespace().filter(|p| *p != "|").take(MAX_PREREQS).map(str::to_string).collect()
        };
        for n in names.split_whitespace() {
            if n.starts_with('.') || n.contains('%') {
                continue;
            }
            let i = match index.get(n) {
                Some(&i) => i,
                None if targets.len() < MAX_TARGETS => {
                    targets.push((n.to_string(), String::new(), vec![]));
                    index.insert(n.to_string(), targets.len() - 1);
                    targets.len() - 1
                }
                None => continue,
            };
            if !inline.is_empty() {
                push_recipe(&mut targets[i], &inline);
            }
            if targets[i].2.len() < MAX_PREREQS {
                targets[i].2.extend(prereqs.iter().cloned());
            }
            current.push(i);
        }
    }
    let mut graph = TaskGraph::default();
    for (name, recipe, prereqs) in &targets {
        let mut refs = prereqs.clone();
        refs.extend(make_recipe_refs(recipe));
        graph.add(name, recipe, refs);
    }
    let offered: Vec<(String, String)> = targets
        .into_iter()
        .filter(|(t, _, _)| plain_target(t) && (phony.contains(t) || COMMON_TARGETS.contains(&t.as_str())))
        .map(|(t, body, _)| (t, body))
        .collect();
    let keep: BTreeSet<String> = if offered.len() > MAX_PER_FILE {
        let mut k: Vec<&String> = offered.iter().map(|(t, _)| t).filter(|t| COMMON_TARGETS.contains(&t.as_str())).collect();
        k.extend(offered.iter().map(|(t, _)| t).filter(|t| !COMMON_TARGETS.contains(&t.as_str())));
        k.into_iter().take(MAX_PER_FILE).cloned().collect()
    } else {
        offered.iter().map(|(t, _)| t.clone()).collect()
    };
    offered
        .into_iter()
        .filter(|(t, _)| keep.contains(t))
        .map(|(name, body)| Entry { reaches_out: graph.reaches_out(&name), name, body })
        .collect()
}

/// A target a person would type: not special (`.PHONY`), private (`_x`), a pattern
/// (`%.o`), a variable (`$(BIN)`) or a path (`build/app`). File targets (`main.o`)
/// are neither phony nor conventional, so the caller leaves them out.
pub(super) fn plain_target(t: &str) -> bool {
    !t.is_empty() && !t.starts_with(['.', '_', '-']) && !t.contains(['%', '$', '/', '(', ')', '{', '}', '\\', '*', '?'])
}

// ---------------------------------------------------------------- just

pub fn detect_just(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let entries = just_recipes(&src);
    if entries.is_empty() {
        return;
    }
    cx.tag("just");
    // `just` resolves the justfile upwards from its working directory.
    add_entries(cx, f, "just", &|r| format!("just {}", sh(r)), entries);
}

/// Recipes a recipe header names after its colon: `rollout: build push`,
/// `test: (build "x") && post`.
fn just_dependencies(deps: &str) -> Vec<String> {
    let text = deps.split(" #").next().unwrap_or("").replace('(', " ( ").replace(')', " ) ");
    let mut out = vec![];
    let (mut in_call, mut first) = (false, false);
    for tok in text.split_whitespace() {
        match tok {
            "(" => (in_call, first) = (true, true),
            ")" => in_call = false,
            t => {
                if (!in_call || first) && JUST_NAME.is_match(t) {
                    out.push(t.to_string());
                }
                first = false;
            }
        }
    }
    out
}

/// Public recipes of a justfile that run without arguments.
pub(super) fn just_recipes(src: &str) -> Vec<Entry> {
    // Every recipe: (name, body, dependencies, offered).
    let mut all: Vec<(String, String, Vec<String>, bool)> = vec![];
    let mut attrs: Vec<String> = vec![];
    let mut current: Option<usize> = None;
    for line in src.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(i) = current {
                all[i].1.push_str(line.trim());
                all[i].1.push('\n');
            }
            continue;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        current = None;
        if t.starts_with('#') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            attrs.push(t.to_ascii_lowercase());
            continue;
        }
        let pending = std::mem::take(&mut attrs);
        if ["set ", "alias ", "export ", "import ", "mod ", "import?", "mod?"].iter().any(|k| t.starts_with(k)) || t.contains(":=") {
            continue;
        }
        let Some(c) = JUST_RECIPE.captures(line) else { continue };
        let name = c[2].to_string();
        let params = c[3].trim();
        let deps = just_dependencies(c.get(4).map(|m| m.as_str()).unwrap_or(""));
        let private = name.starts_with('_') || pending.iter().any(|a| a.contains("private"));
        let other_os = {
            let oses: Vec<&str> = ["linux", "unix", "macos", "windows", "openbsd"].into_iter().filter(|o| pending.iter().any(|a| a.contains(o))).collect();
            !oses.is_empty() && !oses.iter().any(|o| matches!(*o, "linux" | "unix"))
        };
        all.push((name, String::new(), deps, !(private || other_os || needs_args(params))));
        current = Some(all.len() - 1);
    }
    let mut graph = TaskGraph::default();
    for (name, body, deps, _) in &all {
        let mut refs = deps.clone();
        for words in command_words(body) {
            let is_just = |w: &String| w.trim_start_matches(['@', '-']) == "just" || w.contains("just_executable()");
            let Some(i) = words.iter().position(is_just) else { continue };
            if let Some(r) = invoked(&words[i + 1..], &["-f", "--justfile", "-d", "--working-directory"]) {
                refs.extend(r);
            }
        }
        graph.add(name, body, refs);
    }
    let offered: BTreeSet<&str> = all.iter().filter(|r| r.3).map(|r| r.0.as_str()).collect();
    all.iter()
        .filter(|(_, _, _, o)| *o)
        // `default: up` only repeats `just up`.
        .filter(|(_, body, deps, _)| !(body.trim().is_empty() && deps.len() == 1 && offered.contains(deps[0].as_str())))
        .map(|(name, body, _, _)| Entry { name: name.clone(), body: body.clone(), reaches_out: graph.reaches_out(name) })
        .collect()
}

/// Whether recipe parameters (`name *args flag="x" +files`) include a required one.
pub(super) fn needs_args(params: &str) -> bool {
    // Default values may be quoted strings with spaces: drop them first.
    let mut cleaned = String::new();
    let mut quote: Option<char> = None;
    for ch in params.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => {}
            None if ch == '"' || ch == '\'' || ch == '`' => quote = Some(ch),
            None => cleaned.push(ch),
        }
    }
    cleaned.split_whitespace().any(|p| {
        let p = p.trim_start_matches('$');
        p.starts_with('+') || (!p.starts_with('*') && !p.contains('=') && p.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_'))
    })
}

// ---------------------------------------------------------------- Taskfile

pub fn detect_taskfile(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let entries = taskfile_tasks(&src);
    if entries.is_empty() {
        return;
    }
    cx.tag("taskfile");
    // go-task installs as `task`, some distributions as `go-task`.
    let bin = if !on_path("task") && on_path("go-task") { "go-task" } else { "task" };
    add_entries(cx, f, "task", &|t| format!("{bin} {}", sh(t)), entries);
}

/// A Taskfile `cmds` / `deps` item: its command text and the task it calls
/// (`- task: upload`, `deps: [upload]`, `- defer: {task: cleanup}`).
fn taskfile_item(v: &serde_norway::Value, dep: bool) -> (String, Option<String>) {
    if let Some(s) = v.as_str() {
        return if dep { (String::new(), Some(s.to_string())) } else { (s.to_string(), None) };
    }
    let defer = v.get("defer");
    let cmd = v.get("cmd").or_else(|| defer.filter(|d| d.is_string())).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let task = v.get("task").or_else(|| defer.and_then(|d| d.get("task"))).and_then(|x| x.as_str()).map(str::to_string);
    (cmd, task)
}

pub(super) fn taskfile_tasks(src: &str) -> Vec<Entry> {
    let Ok(y) = serde_norway::from_str::<serde_norway::Value>(src) else { return vec![] };
    let Some(tasks) = y.get("tasks").and_then(|t| t.as_mapping()) else { return vec![] };
    let mut graph = TaskGraph::default();
    let mut out: Vec<(String, String)> = vec![];
    for (k, v) in tasks {
        let Some(name) = k.as_str() else { continue };
        let mut offered = !(name.starts_with('_') || name.contains('*'));
        let mut cmds: Vec<String> = vec![];
        let mut refs: Vec<String> = vec![];
        let mut item = |x: &serde_norway::Value, dep: bool| {
            let (c, t) = taskfile_item(x, dep);
            if !c.is_empty() {
                cmds.push(c);
            }
            refs.extend(t);
        };
        match v {
            serde_norway::Value::String(s) => item(&serde_norway::Value::String(s.clone()), false),
            serde_norway::Value::Sequence(s) => s.iter().for_each(|x| item(x, false)),
            serde_norway::Value::Mapping(_) => {
                if v.get("internal").and_then(|i| i.as_bool()) == Some(true) {
                    offered = false;
                }
                if let Some(p) = v.get("platforms").and_then(|p| p.as_sequence()) {
                    let here = p.iter().filter_map(|x| x.as_str()).any(|x| x.starts_with("linux") || x == "amd64" || x == "arm64");
                    if !here {
                        offered = false;
                    }
                }
                // Tasks that need a variable passed on the command line (`requires: {vars: [X]}`).
                if v.get("requires").and_then(|r| r.get("vars")).and_then(|x| x.as_sequence()).is_some_and(|s| !s.is_empty()) {
                    offered = false;
                }
                for x in v.get("deps").and_then(|c| c.as_sequence()).into_iter().flatten() {
                    item(x, true);
                }
                for x in v.get("cmds").and_then(|c| c.as_sequence()).into_iter().flatten() {
                    item(x, false);
                }
                if let Some(c) = v.get("cmd") {
                    item(c, false);
                }
            }
            _ => {}
        }
        let body = cmds.join("\n");
        for words in command_words(&body) {
            let Some(i) = words.iter().position(|w| matches!(w.as_str(), "task" | "go-task")) else { continue };
            if let Some(r) = invoked(&words[i + 1..], &["-d", "--dir", "-t", "--taskfile", "-g", "--global"]) {
                refs.extend(r);
            }
        }
        graph.add(name, &body, refs);
        if offered {
            out.push((name.to_string(), body));
        }
    }
    out.into_iter().map(|(name, body)| Entry { reaches_out: graph.reaches_out(&name), name, body }).collect()
}

// ---------------------------------------------------------------- Procfile

pub fn detect_procfile(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let Some(dir) = f.parent() else { return };
    let cwd = cx.rel(dir);
    let file = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut added = 0;
    for line in src.lines() {
        let Some(c) = PROC_LINE.captures(line.trim_end()) else { continue };
        let (proc, line_cmd) = (c[1].to_string(), c[2].trim().to_string());
        // Heroku's release phase runs on deploy (migrations against the live database).
        if proc == "release" || added >= MAX_PER_FILE {
            continue;
        }
        // Procfile lines are POSIX shell: where the run shell is PowerShell, only those that
        // read the same there, or have a Windows form (`$env:PORT`), are offered.
        let Some(cmd) = super::repository_command(cx, &line_cmd, &cwd) else { continue };
        let web = proc == "web";
        let named_port = port_in(&line_cmd);
        // The same server already detected in this directory: the same command
        // (`web: bin/rails server`), the same program with options (`bin/rails server
        // -p 3000` next to `rails server` on :3000), or a web process on its port.
        let repeats = cx.pf.runs.iter().any(|r| {
            let same_program = cmd == r.command
                || (cmd.starts_with(&format!("{} ", r.command)) && (named_port.is_none() || named_port == r.port));
            let same_port = web && named_port.is_some() && named_port == r.port && matches!(r.kind, RunKind::Server | RunKind::Service);
            r.cwd == cwd && (same_program || same_port)
        });
        if repeats {
            continue;
        }
        // foreman / honcho give the first process PORT=5000.
        let uses_port = line_cmd.contains("$PORT") || line_cmd.contains("${PORT");
        let port = named_port.or(uses_port.then_some(5000));
        let kind = if web { RunKind::Server } else { RunKind::Task };
        let mut env = BTreeMap::new();
        if uses_port {
            if let Some(p) = port {
                env.insert("PORT".to_string(), p.to_string());
            }
        }
        cx.add_run(RunConfig {
            name: scoped(&format!("{file}: {proc}"), &cwd),
            kind,
            command: cmd.clone(),
            cwd: cwd.clone(),
            env,
            port: port.filter(|_| web),
            ready: http_server_ready(kind, &cmd),
            preview: port.filter(|_| web).map(|p| format!("http://localhost:{p}/")),
            source: source(cx, f, &format!("#{proc}")),
            group: Some(task_group(kind, &cmd)),
            ..Default::default()
        });
        added += 1;
    }
    if added > 0 {
        cx.tag("procfile");
    }
}
