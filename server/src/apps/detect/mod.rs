//! Zero-config project detection: files on disk → a proposed `ProjectFile`.
//!
//! The result is layer 1 of the project model (see `config/project.rs`): it is
//! never written to disk, and `.workbench.toml` / the machine overlay override it
//! entry by entry (runs and envs merge by `name`).
//!
//! Rules this module keeps:
//! * **Pure.** It only reads files: no network, no writes, no child processes.
//!   (`PATH` is consulted to see whether a tool such as `cargo-nextest` exists.)
//! * **Fast and bounded.** A shallow walk (depth 4) that skips heavy directories,
//!   an entry cap, a per-file read cap and a total read budget; typically 5–20 ms.
//! * **This project only.** The walk does not descend into nested repositories
//!   (worktrees such as `.claude/worktrees/*`, submodules, clones: any directory
//!   with a `.git` entry) nor into gitignored *directories*. Gitignored *files* are
//!   still read, because deploy topology (`deploy/Caddyfile`, `deploy/*.sh`) is
//!   often kept out of git. The skipped directories are only scanned for
//!   sensitive file names.
//! * **Samples are not the project.** Files below `examples/`, `samples/`,
//!   `third_party/`, `testdata/`, `fixtures/`… (`SAMPLE_DIRS`) propose runs only
//!   when the rest of the project proposes none (a repository of examples).
//! * **Never panics into the caller.** `detect` catches panics and falls back to an
//!   empty config, because it runs for every project on every registry reload.
//! * **Provenance.** Every detected run carries `source = "detected:<file>…"`;
//!   documentation suggestions carry `"<doc>:L<n>"` and live in group `suggested`.
//! * **Nothing runs by itself.** Detection proposes click-to-run commands only.
//!   Anything that deploys, publishes or reaches a remote host lands in group
//!   `deploy` (starting it needs confirmation; agents cannot start it).
//!
//! Signals: git remote (GitLab, GitHub), `.gitlab-ci.yml` or GitHub workflows,
//! Cargo workspaces and packages, `package.json` scripts (npm, pnpm, yarn, bun;
//! turbo and nx roots), `deno.json` tasks, Python projects (uv, poetry, pdm, hatch,
//! pipenv, pip; pytest, ruff, mypy, tox, nox, Django, FastAPI, Flask, Streamlit),
//! Jupyter notebooks, Go modules, Makefiles, justfiles, Taskfiles, Procfiles,
//! docker compose files and Dockerfiles, CMake projects and presets, Gradle and
//! Maven builds, Ruby (Rails, RSpec, Rack, Jekyll), PHP (Composer, Laravel,
//! Symfony, PHPUnit), Elixir (Mix, Phoenix), Unity projects and their
//! `[MenuItem]` batch entry points, .NET solutions, `validate.*` scripts, Caddyfile
//! sites + `deploy/*.sh` → environments, ssh hosts in markdown fences, Confluence
//! links in docs, and fenced shell blocks in CLAUDE.md / README.md / SETUP.md.

mod cargo;
mod cmake;
mod compose;
mod deploy;
mod docs;
mod dotnet;
mod elixir;
mod git;
mod go;
mod jvm;
mod node;
mod php;
mod python;
mod ruby;
mod tasks;
mod unity;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use {
    cargo::CARGO_TEST_RESULT, cmake::CTEST_RESULT, dotnet::DOTNET_TEST_RESULT, go::GO_TEST_RESULT, node::VITEST_RESULT,
    python::PYTEST_RESULT,
};

use std::collections::BTreeSet;
use std::io::Read;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use crate::config::ProjectFile;
use crate::config::project::{RunConfig, RunKind};

/// Directories never descended into by the project walk. `.claude` holds Claude
/// Code's settings and its agent worktrees (full copies of the repository); the
/// rest are dependency stores, caches and build output.
const SKIP_DIRS: &[&str] = &[
    ".git", ".hg", ".svn", ".idea", ".vscode", ".claude", ".venv", "venv", ".cache", ".next", ".nuxt", ".svelte-kit",
    ".turbo", ".gradle", ".dart_tool", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".tox", ".nox", "node_modules",
    "target", "Library", "Temp", "Logs", "obj", "bin", "dist", "build", "UserSettings", "out", "__pycache__",
    "Packages", "Assets", "coverage", "vendor", ".terraform", "_build", ".elixir_ls", ".bundle", ".yarn", ".pnpm-store",
    "bower_components", "Pods", ".eggs", "htmlcov", ".hypothesis", ".ipynb_checkpoints", ".direnv", ".devenv",
    ".mvn", ".cxx", ".externalNativeBuild", "zig-cache", ".zig-cache", "zig-out", ".stack-work", "dist-newstyle",
    "elm-stuff", ".angular", ".expo", ".output", ".vercel", ".netlify", ".serverless", ".parcel-cache", ".docusaurus",
    ".astro", "storybook-static", ".deno", ".dub",
];

/// Whether the walk skips a directory: `SKIP_DIRS`, and CLion's CMake build
/// directories (`cmake-build-debug`, `cmake-build-release-clang`…).
fn skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name) || name.starts_with("cmake-build-")
}

/// Directories whose content is someone else's or an illustration, not this
/// project: examples, vendored code, test fixtures, templates. Their build files
/// propose runs only when the rest of the project proposes none.
const SAMPLE_DIRS: &[&str] = &[
    "examples", "example", "samples", "sample", "demo", "demos", "playground", "playgrounds", "sandbox", "cookbook",
    "tutorial", "tutorials", "starters", "starter-kits", "boilerplate", "boilerplates", "templates", "template",
    "skeleton", "skeletons", "third_party", "third-party", "thirdparty", "external", "externals", "extern", "deps",
    "testdata", "test-data", "test_data", "fixtures", "fixture", "__fixtures__", "test-fixtures", "test_fixtures",
    "test-projects", "test_projects", "testprojects", "site-packages",
];

/// Whether a project-relative path (`/` separators) lies below a sample directory.
pub(crate) fn is_sample_rel(rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    parts.pop(); // the file itself
    parts.iter().any(|p| SAMPLE_DIRS.contains(&p.to_ascii_lowercase().as_str()))
}

/// Depth of the project walk (the root is depth 0).
const MAX_DEPTH: usize = 4;
/// Entries (files + directories) visited by one walk.
const MAX_ENTRIES: usize = 20_000;
/// Bytes read from one file.
pub(crate) const MAX_FILE: u64 = 2 * 1024 * 1024;
/// Bytes read in total by one detection.
const READ_BUDGET: u64 = 48 * 1024 * 1024;

/// Propose a project config from the files in `root`. Never writes anything and
/// never panics (a detector bug yields an empty proposal and a warning in the log).
pub fn detect(root: &Path) -> ProjectFile {
    match std::panic::catch_unwind(AssertUnwindSafe(|| detect_inner(root))) {
        Ok(pf) => pf,
        Err(_) => {
            tracing::warn!("project detection panicked for {}; using an empty proposal", root.display());
            ProjectFile::default()
        }
    }
}

fn detect_inner(root: &Path) -> ProjectFile {
    let mut cx = Ctx::new(root);
    cx.pf.schema = 1;
    git::detect(&mut cx);
    let (main, samples): (Vec<PathBuf>, Vec<PathBuf>) = cx.files.iter().cloned().partition(|f| !cx.is_sample(f));
    detect_files(&mut cx, &main);
    if cx.pf.runs.is_empty() && !samples.is_empty() {
        // Nothing to run at the top level: a repository of examples. Offer them.
        detect_files(&mut cx, &samples);
    }
    git::detect_github_workflows(&mut cx);
    deploy::detect(&mut cx);
    docs::detect(&mut cx);
    detect_devcontainers(&mut cx);
    detect_sensitive(&mut cx);
    cx.finish()
}

/// `devcontainer.json` files become components (kind `devcontainer`) and the tag
/// `devcontainer`. Recorded only: building or starting one is always the user's
/// explicit, confirmed action (the devcontainer slice).
fn detect_devcontainers(cx: &mut Ctx) {
    for rel in crate::devcontainer::config::discover(cx.root) {
        let name = match rel.strip_prefix(".devcontainer/").and_then(|r| r.strip_suffix("/devcontainer.json")) {
            Some(sub) => format!("devcontainer ({sub})"),
            None => "devcontainer".to_string(),
        };
        cx.pf.components.push(crate::config::project::Component { name, path: rel, kind: "devcontainer".into(), version: None });
        cx.tag("devcontainer");
    }
}

/// Run every file-driven detector over `files` (shallowest first).
fn detect_files(cx: &mut Ctx, files: &[PathBuf]) {
    let root = cx.root;
    for f in files {
        let Some(name) = f.file_name().and_then(|n| n.to_str()) else { continue };
        match name {
            ".gitlab-ci.yml" if f.parent() == Some(root) => git::detect_gitlab_ci(cx, f),
            "Cargo.toml" => cargo::detect(cx, f),
            "package.json" => node::detect(cx, f),
            "deno.json" | "deno.jsonc" => node::detect_deno(cx, f),
            "ProjectVersion.txt" if f.parent().is_some_and(|d| d.ends_with("ProjectSettings")) => unity::detect(cx, f),
            "go.mod" => go::detect(cx, f),
            "Makefile" | "makefile" | "GNUmakefile" => tasks::detect_make(cx, f),
            "justfile" | "Justfile" | ".justfile" => tasks::detect_just(cx, f),
            "Taskfile.yml" | "Taskfile.yaml" | "taskfile.yml" | "taskfile.yaml" | "Taskfile.dist.yml" | "Taskfile.dist.yaml" => {
                tasks::detect_taskfile(cx, f)
            }
            "Procfile" | "Procfile.dev" => tasks::detect_procfile(cx, f),
            "settings.gradle" | "settings.gradle.kts" | "build.gradle" | "build.gradle.kts" => jvm::detect_gradle(cx, f),
            "pom.xml" => jvm::detect_maven(cx, f),
            "Gemfile" => ruby::detect(cx, f),
            "composer.json" => php::detect(cx, f),
            "mix.exs" => elixir::detect(cx, f),
            ".mcp.json" if f.parent() == Some(root) => cx.tag("mcp"),
            _ => {}
        }
        if name.ends_with(".sln") {
            dotnet::detect(cx, f);
        }
        if is_validate_script(name) {
            dotnet::detect_validate_script(cx, f);
        }
        if name.ends_with(".py") {
            cx.tag("python");
        }
    }
    python::detect(cx, files);
    cmake::detect(cx, files);
    compose::detect(cx, files);
}

/// Whether a run name or command deploys, releases, publishes, reaches a remote host,
/// destroys data or handles secrets (see `docs::is_risky`).
pub(crate) fn is_risky_command(text: &str) -> bool {
    docs::is_risky(text)
}

fn is_validate_script(name: &str) -> bool {
    matches!(name, "validate.mjs" | "validate.js" | "validate.cjs" | "validate.py" | "validate.sh")
}

/// Detection state shared by the detectors.
pub(crate) struct Ctx<'a> {
    pub root: &'a Path,
    /// The project's own files found by the bounded walk, shallowest first, then by path.
    pub files: Vec<PathBuf>,
    /// Files in gitignored directories and nested repositories: names only, for the
    /// sensitive-file scan (never read, never a source of runs or environments).
    pub other_files: Vec<PathBuf>,
    pub pf: ProjectFile,
    budget: u64,
    tags: BTreeSet<String>,
    /// Keys of work already done (`gradle:<dir>`…), for detectors triggered by
    /// several files of one project.
    marks: BTreeSet<String>,
}

impl<'a> Ctx<'a> {
    pub fn new(root: &'a Path) -> Self {
        let w = project_walk(root, MAX_DEPTH, MAX_ENTRIES);
        Self {
            root,
            files: w.own,
            other_files: w.other,
            pf: ProjectFile::default(),
            budget: READ_BUDGET,
            tags: BTreeSet::new(),
            marks: BTreeSet::new(),
        }
    }

    /// Whether `p` lies below a sample directory of the project (`SAMPLE_DIRS`).
    pub fn is_sample(&self, p: &Path) -> bool {
        is_sample_rel(&self.rel(p))
    }

    /// Record `key`; false when it was already recorded.
    pub fn mark(&mut self, key: String) -> bool {
        self.marks.insert(key)
    }

    /// Whether some ancestor of `dir` (below the root, or the root itself) was marked
    /// with `prefix` + its path: a nested build file of a project already handled.
    pub fn ancestor_marked(&self, prefix: &str, dir: &Path) -> bool {
        let mut a = dir.parent();
        while let Some(d) = a {
            if !d.starts_with(self.root) {
                break;
            }
            if self.marks.contains(&format!("{prefix}{}", d.display())) {
                return true;
            }
            a = d.parent();
        }
        false
    }

    /// Read a text file (lossy UTF-8, at most `MAX_FILE` bytes, within the budget).
    pub fn read(&mut self, p: &Path) -> Option<String> {
        if self.budget == 0 {
            return None;
        }
        let cap = MAX_FILE.min(self.budget);
        // Check before opening: opening a FIFO for reading would block detection.
        if !std::fs::metadata(p).ok()?.is_file() {
            return None;
        }
        let f = std::fs::File::open(p).ok()?;
        let mut buf = Vec::new();
        f.take(cap).read_to_end(&mut buf).ok()?;
        self.budget = self.budget.saturating_sub(buf.len() as u64);
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    /// `p` relative to the project root with `/` separators; `.` for the root itself.
    pub fn rel(&self, p: &Path) -> String {
        rel(self.root, p)
    }

    pub fn tag(&mut self, t: &str) {
        self.tags.insert(t.to_string());
    }

    /// Add a run; returns the name it got, or `None` when it was dropped. Names are
    /// unique:
    /// * the same name, directory and command again is a duplicate and is dropped
    ///   (first detector wins);
    /// * the same name and directory with another command is qualified by its tool
    ///   (`serve · uv` next to npm's `serve`);
    /// * the same name from another directory is qualified by it (`app (tools/server)`).
    ///   Files are visited shallowest first, so the run closest to the root keeps the
    ///   plain name.
    pub fn add_run(&mut self, mut run: RunConfig) -> Option<String> {
        // A name with a line break or another control character (a crafted Taskfile
        // key, a script name) is nothing a person typed: not offered.
        if run.name.chars().any(char::is_control) {
            return None;
        }
        // `npm run deploy`, `release`, a `deploy` binary…: kept apart from everyday tasks
        // (starting them always asks first, and agents cannot start them).
        if run.group.as_deref() != Some("suggested")
            && (docs::is_deployish(&run.name) || docs::is_deployish(&run.command) || reaches_out(&run.name))
        {
            run.group = Some("deploy".into());
        }
        let taken = |pf: &ProjectFile, n: &str| pf.runs.iter().any(|r| r.name == n);
        let Some(existing) = self.pf.runs.iter().find(|r| r.name == run.name) else {
            let name = run.name.clone();
            self.pf.runs.push(run);
            return Some(name);
        };
        let alt = if existing.cwd == run.cwd {
            if existing.command == run.command {
                return None;
            }
            let tool = qualifier(&run);
            if tool.is_empty() {
                return None;
            }
            format!("{} · {tool}", run.name)
        } else {
            scoped(&run.name, &run.cwd)
        };
        if alt == run.name || taken(&self.pf, &alt) {
            return None;
        }
        run.name = alt.clone();
        self.pf.runs.push(run);
        Some(alt)
    }

    pub fn has_file(&self, rel_path: &str) -> bool {
        self.root.join(rel_path).is_file()
    }

    fn finish(mut self) -> ProjectFile {
        self.pf.project.tags = self.tags.into_iter().collect();
        // Stable order the UI can rely on: dev servers first, suggestions last; within
        // a group, a project's own runs before task-runner wrappers of them (the top
        // bar selects the first server: `orders-api`, not `task run`).
        self.pf.runs.sort_by_key(|r| (group_rank(r), is_wrapper(r)));
        self.pf
    }
}

/// A run that goes through a task runner (`make dev`, `just test`, a Procfile
/// process, `npx turbo run build`) rather than the tool itself.
fn is_wrapper(r: &RunConfig) -> bool {
    let src = r.source.as_deref().unwrap_or("");
    let file = src.strip_prefix("detected:").unwrap_or("").split(['#', ' ']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    matches!(command_tool(&r.command).as_str(), "make" | "just" | "task" | "go-task")
        || r.command.starts_with("composer run-script ")
        || file.starts_with("Procfile")
        || matches!(file, "turbo.json" | "nx.json")
}

fn group_rank(r: &RunConfig) -> u8 {
    match r.group.as_deref() {
        Some("dev") => 0,
        Some("test") => 2,
        Some("build") => 3,
        Some("unity") => 4,
        Some("deploy") => 8,
        Some("suggested") => 9,
        _ => match r.kind {
            RunKind::Server | RunKind::Service => 1,
            RunKind::Editor => 4,
            _ => 5,
        },
    }
}

/// `p` relative to `root` with `/` separators; `.` for the root itself.
pub(crate) fn rel(root: &Path, p: &Path) -> String {
    let r = p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/");
    if r.is_empty() { ".".into() } else { r }
}

/// `name` qualified by its directory unless it is the project root: `dev (app/web)`.
pub(crate) fn scoped(name: &str, cwd: &str) -> String {
    if cwd == "." { name.to_string() } else { format!("{name} ({cwd})") }
}

/// The program a command line starts, past `VAR=value` assignments: `uv` for
/// `uv run serve`, `make` for `make test`, `phpunit` for `vendor/bin/phpunit`.
pub(crate) fn command_tool(cmd: &str) -> String {
    let first = cmd.split_whitespace().find(|t| !(t.contains('=') && !t.starts_with('-'))).unwrap_or("");
    first.rsplit('/').next().unwrap_or("").trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_').to_string()
}

/// What tells a run apart from another of the same name in the same directory:
/// the program it starts (`serve · uv`), or, when the command is the name itself
/// (a console script), the file that declares it (`serve · pyproject`).
fn qualifier(run: &RunConfig) -> String {
    let base = run.name.split(" (").next().unwrap_or(&run.name);
    let tool = command_tool(&run.command);
    if !tool.is_empty() && tool != base {
        return tool;
    }
    let file = run.source.as_deref().and_then(|s| s.strip_prefix("detected:")).unwrap_or("");
    let file = file.split(['#', ' ']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    file.split('.').next().unwrap_or("").to_string()
}

/// The usual run group of a kind: `dev`, `test`, `build`, `tasks`.
pub(crate) fn group_of(kind: RunKind) -> &'static str {
    match kind {
        RunKind::Server | RunKind::Service => "dev",
        RunKind::Test => "test",
        RunKind::Build => "build",
        RunKind::Task | RunKind::Editor => "tasks",
    }
}

/// What a task named by a project (a Make target, a just recipe, a Composer
/// script…) most likely does, from its name: `test`, `check`, `e2e` → test;
/// `build`, `all`, `compile` → build; `dev`, `serve`, `start`, `run`, `watch` →
/// a long-running (server-ish) process; anything else (`up`, which usually
/// detaches, `lint`, `clean`…) is a task.
pub(crate) fn kind_from_task_name(name: &str) -> RunKind {
    let lower = name.to_ascii_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let any = |set: &[&str]| words.iter().any(|w| set.contains(w));
    if any(&["test", "tests", "check", "checks", "e2e", "spec", "specs", "unittest", "unittests", "integration", "coverage", "cover", "cov", "pytest"]) {
        RunKind::Test
    } else if any(&["build", "all", "compile", "bundle", "assemble", "dist"]) {
        RunKind::Build
    } else if words.first().is_some_and(|w| ["dev", "serve", "server", "start", "run", "watch", "develop", "preview", "runserver"].contains(w)) {
        RunKind::Server
    } else {
        RunKind::Task
    }
}

/// Commands that reach another machine or publish something: a task whose body
/// does this is a deploy (confirmation first, never started by agents), whatever
/// its name says.
static REMOTE_OR_PUBLISH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?x)
        \b(?:ssh|scp|sftp|rsync|kubectl|helm|terraform|tofu|pulumi|ansible(?:-playbook)?|flyctl|heroku|netlify|vercel|surge|doctl|gcloud|gsutil|eb|kamal)\s
        | \b(?:aws|az)\s+\w
        | \bfly\s+deploy | \bfirebase\s+deploy | \bwrangler\s+(?:deploy|publish) | \brailway\s+up
        | \b(?:docker|podman)\s+(?:push|login) | \bgit\s+push
        # Compose pushing images: `docker compose -f a.yml push`, `docker-compose push`.
        | \b(?:docker|podman)[-\s]compose\b[^\n;&|]*\spush\b
        # `docker buildx build --push`, `docker buildx bake --push`.
        | \bbuildx\s+(?:build|bake)\b[^\n;&|]*\s--push\b
        # Another Docker engine: `docker --context prod …`, `docker -H ssh://…`, `docker context use prod`.
        | \b(?:docker|podman)\s+(?:--context[=\s]|-c\s)
        | \b(?:docker|podman)\s+(?:-H|--host)[=\s]+[\x22']?(?:ssh|tcp)://
        | \bdocker\s+context\s+use\b
        | \bDOCKER_HOST=[\x22']?ssh://
        | \b(?:npm|yarn|pnpm|bun)\s+publish | \bcargo\s+publish | \btwine\s+upload | \bpoetry\s+publish | \buv\s+publish
        | \bgem\s+push | \bmix\s+hex\.publish | \bgh-pages\b | \bsemantic-release\b
        | \b(?:gh|glab)\s+release\s+(?:create|upload|edit|delete) | \bgh\s+workflow\s+run | \bgh\s+pr\s+merge
        | \b(?:changeset|lerna)\s+publish | \bdeploy",
    )
    .unwrap()
});

/// `DOCKER_HOST=tcp://…` naming an engine that is not on this machine.
static DOCKER_HOST_TCP: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r#"\bDOCKER_HOST=["']?tcp://([^\s:/"']+)"#).unwrap());

/// Whether a task body (recipe lines, a script) deploys, publishes or reaches a remote host.
pub(crate) fn reaches_out(body: &str) -> bool {
    REMOTE_OR_PUBLISH.is_match(body)
        || DOCKER_HOST_TCP
            .captures_iter(body)
            .any(|c| !matches!(&c[1], "localhost" | "127.0.0.1" | "0.0.0.0" | "[::1]" | "docker"))
        // An ssh URL (`ssh://root@host`, `qemu+ssh://`), but not a version-control
        // dependency that is only fetched (`git+ssh://…`).
        || body.match_indices("ssh://").any(|(i, _)| !["git+", "svn+", "hg+", "bzr+"].iter().any(|v| body[..i].ends_with(v)))
}

/// The group of a task-runner entry: its kind's group, or `deploy` when its body
/// reaches out (`reaches_out`).
pub(crate) fn task_group(kind: RunKind, body: &str) -> String {
    deploy_or(kind, reaches_out(body))
}

/// `deploy` when the entry reaches out (itself or through what it runs), else its kind's group.
pub(crate) fn deploy_or(kind: RunKind, reaches: bool) -> String {
    if reaches { "deploy".into() } else { group_of(kind).into() }
}

/// Entries of one task file (npm scripts, Make targets, just recipes, Taskfile
/// tasks, Python task tables, Composer scripts) and what each one runs besides its
/// own body: lifecycle hooks (`postbuild`), prerequisites and dependencies, and
/// references in its commands (`npm run upload`, `$(MAKE) push`, `task: upload`,
/// `@upload`). A run is classified by everything it runs, not only by its own text:
/// `npm run build` also runs `postbuild`.
#[derive(Debug, Default)]
pub(crate) struct TaskGraph {
    nodes: std::collections::BTreeMap<String, (String, Vec<String>)>,
}

impl TaskGraph {
    /// Entries followed from one entry, and the length of a chain, so a hostile
    /// file cannot make classification expensive.
    const MAX_NODES: usize = 200;
    const MAX_DEPTH: usize = 12;
    /// Body text kept per entry.
    const MAX_BODY: usize = 64 * 1024;

    /// Record `name`'s body and references (appended when it is already known:
    /// Make targets may have several rules).
    pub fn add(&mut self, name: &str, body: &str, refs: Vec<String>) {
        let node = self.nodes.entry(name.to_string()).or_default();
        if !body.is_empty() && node.0.len() < Self::MAX_BODY {
            node.0.push_str(body);
            node.0.push('\n');
        }
        for r in refs {
            if r != name && !node.1.contains(&r) && node.1.len() < Self::MAX_NODES {
                node.1.push(r);
            }
        }
    }

    /// The bodies of `name` and of every entry it runs, transitively: breadth-first,
    /// each entry once (cycles are fine), unknown names ignored, bounded in depth and number.
    pub fn bodies<'a>(&'a self, name: &'a str) -> Vec<&'a str> {
        let mut out: Vec<&str> = vec![];
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut queue: std::collections::VecDeque<(&str, usize)> = [(name, 0)].into();
        while let Some((n, depth)) = queue.pop_front() {
            let Some((key, (body, refs))) = self.nodes.get_key_value(n) else { continue };
            if seen.len() >= Self::MAX_NODES || !seen.insert(key.as_str()) {
                continue;
            }
            out.push(body);
            if depth < Self::MAX_DEPTH {
                queue.extend(refs.iter().map(|r| (r.as_str(), depth + 1)));
            }
        }
        out
    }

    /// Whether `name`, or anything it runs (`bodies`), reaches another machine or publishes.
    pub fn reaches_out(&self, name: &str) -> bool {
        self.bodies(name).into_iter().any(reaches_out)
    }
}

/// A task body as simple commands: split at shell separators (`&&`, `||`, `;`, `|`,
/// `&`, newlines) into words, with quotes and grouping parentheses trimmed. Quotes
/// are not honoured on purpose: `concurrently "npm run a" "npm run b"` yields the
/// words of both inner commands, which is what reference scanning wants.
pub(crate) fn command_words(body: &str) -> Vec<Vec<String>> {
    body.split(['\n', ';', '|', '&'])
        .map(|seg| {
            seg.split_whitespace()
                .map(|w| {
                    let w = w.trim_matches(|c| matches!(c, '"' | '\'' | '`'));
                    let w = w.strip_prefix('(').unwrap_or(w);
                    // `$(MAKE)` keeps its parenthesis; a grouping `(…)` loses it.
                    let w = if w.ends_with(')') && !w.contains('(') { &w[..w.len() - 1] } else { w };
                    w.trim_matches(|c| matches!(c, '"' | '\'' | '`')).to_string()
                })
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Names a glob-ish word refers to among `names`: itself, or for `build:*` /
/// `watch-*` (npm-run-all, concurrently) every name with that prefix.
fn matching_names<'a>(word: &str, names: impl Iterator<Item = &'a str>) -> Vec<String> {
    match word.find('*') {
        Some(i) => {
            let prefix = &word[..i];
            names.filter(|n| n.starts_with(prefix)).map(str::to_string).collect()
        }
        None => names.filter(|n| *n == word).map(str::to_string).collect(),
    }
}

/// npm lifecycle scripts an install runs in the package itself.
const INSTALL_HOOKS: &[&str] = &["preinstall", "install", "postinstall", "preprepare", "prepare", "postprepare", "prepublish"];

/// Scripts of the same `package.json` that a script body runs through its package
/// manager: `npm run x`, `npm test`, `pnpm [run] x`, `yarn [run] x`, `bun run x`,
/// `npm-run-all a b:*`, `run-s`/`run-p`, `concurrently "npm:x"`, and the lifecycle
/// scripts of `npm install`, `npm pack` and `npm version`. Invocations aimed at
/// another package (`--prefix`, `-w`, `--filter`, `yarn workspace`) are not
/// followed: that is another file.
pub(crate) fn npm_refs(body: &str, scripts: &BTreeSet<String>) -> Vec<String> {
    let names = || scripts.iter().map(String::as_str);
    let mut out: Vec<String> = vec![];
    let push = |v: Vec<String>, out: &mut Vec<String>| {
        for n in v {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    };
    let hooks = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    for words in command_words(body) {
        let mut i = 0;
        while i < words.len() {
            let w = words[i].as_str();
            match w {
                "npm" | "pnpm" | "yarn" | "bun" => {
                    let mut j = i + 1;
                    let mut elsewhere = false;
                    // Global flags; those naming another package take a value.
                    while let Some(f) = words.get(j).filter(|f| f.starts_with('-')) {
                        if matches!(f.as_str(), "--prefix" | "-C" | "--dir" | "--cwd" | "-w" | "--workspace" | "--filter" | "-F") {
                            elsewhere = true;
                            j += 1;
                        } else if ["--prefix=", "--dir=", "--cwd=", "--workspace=", "--filter=", "-r", "--recursive", "--workspaces", "-ws"]
                            .iter()
                            .any(|p| f.starts_with(p))
                        {
                            elsewhere = true;
                        }
                        j += 1;
                    }
                    let sub = words.get(j).map(String::as_str);
                    if !elsewhere {
                        match sub {
                            Some("run" | "run-script" | "rum" | "urn") => {
                                let mut k = j + 1;
                                while words.get(k).is_some_and(|f| f.starts_with('-')) {
                                    k += 1;
                                }
                                if let Some(s) = words.get(k) {
                                    push(matching_names(s, names()), &mut out);
                                }
                                j = k;
                            }
                            Some("test" | "t" | "tst") => push(vec!["test".into()], &mut out),
                            Some(s @ ("start" | "stop" | "restart")) => push(vec![s.to_string()], &mut out),
                            Some("install" | "i" | "ci" | "add" | "isntall") => push(hooks(INSTALL_HOOKS), &mut out),
                            Some("pack") => push(hooks(&["prepack", "prepare", "postpack"]), &mut out),
                            Some("version") => push(hooks(&["preversion", "version", "postversion"]), &mut out),
                            Some("workspace" | "workspaces" | "exec" | "dlx" | "x") => {}
                            // `yarn`, `pnpm` and `bun` run a script by its bare name.
                            Some(s) if w != "npm" => push(matching_names(s, names()), &mut out),
                            None if w == "yarn" => push(hooks(INSTALL_HOOKS), &mut out),
                            _ => {}
                        }
                    }
                    i = j + 1;
                    continue;
                }
                "npm-run-all" | "npm-run-all2" | "run-s" | "run-p" => {
                    for s in words[i + 1..].iter().filter(|s| !s.starts_with('-')) {
                        push(matching_names(s, names()), &mut out);
                    }
                    break;
                }
                _ => {
                    // concurrently's `npm:watch-*` shorthand.
                    if let Some(s) = ["npm:", "yarn:", "pnpm:", "bun:"].iter().find_map(|p| w.strip_prefix(p)) {
                        push(matching_names(s, names()), &mut out);
                    }
                }
            }
            i += 1;
        }
    }
    out
}

/// The npm scripts of one `package.json` as a `TaskGraph`: each script runs its
/// `pre<name>` / `post<name>` hooks and the scripts its body invokes.
pub(crate) fn npm_graph(scripts: &serde_json::Map<String, serde_json::Value>) -> TaskGraph {
    let names: BTreeSet<String> = scripts.keys().cloned().collect();
    let mut g = TaskGraph::default();
    for (k, v) in scripts {
        let body = v.as_str().unwrap_or_default();
        let mut refs = vec![format!("pre{k}"), format!("post{k}")];
        refs.extend(npm_refs(body, &names));
        g.add(k, body, refs);
    }
    g
}

/// `s` as one shell word: unchanged when it is plain (`build`, `db:migrate`,
/// `./cmd/api`), single-quoted otherwise. Detected commands run with `bash -lc`, and
/// names from repository files (Make targets, Taskfile keys, script names, directory
/// names) must never add a command of their own.
pub(crate) fn sh(s: &str) -> String {
    crate::apps::expand::shell_quote(s)
}

static BODY_PORT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?:--port[= ]|-p\s+|\bPORT=|\$\{PORT:-|--?(?:bind|addr|address|listen|http)[= ](?:[\w.]+)?:|(?:localhost|127\.0\.0\.1|0\.0\.0\.0):)(\d{2,5})\b",
    )
    .unwrap()
});

/// `python -m http.server [port]` (port 8000 by default).
static HTTP_SERVER: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"-m\s+http\.server\b([^\n;&|]*)").unwrap());

/// A port a command line or task body names: `--port 8000`, `-p 3000`, `PORT=5000`,
/// `${PORT:-4000}`, `--bind 0.0.0.0:8000`, `-addr :4040`, `localhost:8080`, and
/// `python -m http.server 8123` (8000 when it names none).
pub(crate) fn port_in(body: &str) -> Option<u16> {
    BODY_PORT
        .captures(body)
        .and_then(|c| c[1].parse::<u16>().ok())
        .filter(|p| *p >= 80)
        .or_else(|| HTTP_SERVER.captures(body).and_then(|c| http_server_port(&c[1])))
}

/// The port of `python -m http.server <args>`: its positional argument, else 8000
/// (`None` when the argument is not a number, such as `$PORT`).
fn http_server_port(args: &str) -> Option<u16> {
    let mut words = args.split_whitespace();
    while let Some(w) = words.next() {
        if matches!(w, "-b" | "--bind" | "-d" | "--directory" | "-p" | "--protocol") {
            words.next(); // the option's value
        } else if !w.starts_with('-') {
            return w.parse::<u16>().ok().filter(|p| *p > 0);
        }
    }
    Some(8000)
}

/// The ready line of `python -m http.server`: `Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ...`.
pub(crate) const HTTP_SERVER_READY: &str = r"Serving HTTP on \S+ port \d+ \((https?://[^)]+)\)";

/// Whether a command line starts Python's `http.server`.
pub(crate) fn is_http_server(cmd: &str) -> bool {
    HTTP_SERVER.is_match(cmd)
}

/// `detected:<rel>` provenance string.
pub(crate) fn source(cx: &Ctx, p: &Path, suffix: &str) -> Option<String> {
    Some(format!("detected:{}{suffix}", cx.rel(p)))
}

/// Result of the project walk.
pub(crate) struct ProjectFiles {
    /// Files of the project itself, shallowest first, then by path.
    pub own: Vec<PathBuf>,
    /// Files below gitignored directories and nested repositories, sorted.
    pub other: Vec<PathBuf>,
}

/// Gitignore layers that apply inside one directory, lowest precedence first:
/// global excludes, `info/exclude`, then each `.gitignore` from the root down.
type IgnoreLayers = Vec<std::sync::Arc<ignore::gitignore::Gitignore>>;

fn root_ignore_layers(root: &Path) -> IgnoreLayers {
    use ignore::gitignore::GitignoreBuilder;
    let mut layers: IgnoreLayers = vec![];
    let (global, _err) = GitignoreBuilder::new(root).build_global();
    if !global.is_empty() {
        layers.push(global.into());
    }
    if let Some(gd) = git::git_dir(root) {
        let exclude = gd.join("info/exclude");
        if exclude.is_file() {
            let mut b = GitignoreBuilder::new(root);
            b.add(&exclude);
            if let Ok(gi) = b.build() {
                if !gi.is_empty() {
                    layers.push(gi.into());
                }
            }
        }
    }
    layers
}

/// `layers` plus `dir/.gitignore`, if there is one.
fn with_dir_gitignore(layers: &IgnoreLayers, dir: &Path) -> IgnoreLayers {
    let f = dir.join(".gitignore");
    let mut out = layers.clone();
    if std::fs::symlink_metadata(&f).is_ok_and(|m| m.is_file()) {
        let (gi, _err) = ignore::gitignore::Gitignore::new(&f);
        if !gi.is_empty() {
            out.push(gi.into());
        }
    }
    out
}

/// Whether the directory `path` is gitignored. The deepest layer with an opinion
/// decides; its parents were already checked on the way down.
fn dir_ignored(layers: &IgnoreLayers, path: &Path) -> bool {
    for gi in layers.iter().rev() {
        if !path.starts_with(gi.path()) {
            continue;
        }
        match gi.matched(path, true) {
            ignore::Match::Ignore(_) => return true,
            ignore::Match::Whitelist(_) => return false,
            ignore::Match::None => {}
        }
    }
    false
}

/// A directory below the root that is a repository of its own (a worktree, a
/// submodule, a clone): a separate project, not part of this one.
fn is_nested_repo(dir: &Path) -> bool {
    std::fs::symlink_metadata(dir.join(".git")).is_ok()
}

/// Breadth-first, symlink-free walk of the project at `root` up to `max_depth`,
/// skipping `skip_dir` directories, visiting at most `max_entries` entries. Nested repositories
/// and gitignored directories are not part of the project: they are walked last,
/// with whatever entry budget is left, and their files land in `other`.
pub(crate) fn project_walk(root: &Path, max_depth: usize, max_entries: usize) -> ProjectFiles {
    let mut own = vec![];
    let mut deferred: Vec<(PathBuf, usize)> = vec![];
    let mut seen = 0usize;
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0usize, root_ignore_layers(root))]);
    'walk: while let Some((dir, depth, parent_layers)) = queue.pop_front() {
        let layers = with_dir_gitignore(&parent_layers, &dir);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            seen += 1;
            if seen > max_entries {
                break 'walk;
            }
            // DirEntry::file_type does not follow symlinks: links are neither walked nor read.
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() {
                if depth >= max_depth || skip_dir(&name) {
                    continue;
                }
                let path = e.path();
                if is_nested_repo(&path) || dir_ignored(&layers, &path) {
                    deferred.push((path, depth + 1));
                } else {
                    queue.push_back((path, depth + 1, layers.clone()));
                }
            } else if ft.is_file() {
                own.push(e.path());
            }
        }
    }
    let mut other = vec![];
    for (dir, depth) in deferred {
        let left = max_entries.saturating_sub(seen);
        if left == 0 {
            break;
        }
        let found = walk_filtered(&dir, max_depth.saturating_sub(depth), left, &skip_dir);
        seen += found.len().max(1);
        other.extend(found);
    }
    own.sort_by(|a, b| a.components().count().cmp(&b.components().count()).then_with(|| a.cmp(b)));
    other.sort();
    ProjectFiles { own, other }
}

pub(crate) fn walk_filtered(root: &Path, max_depth: usize, max_entries: usize, skip: &dyn Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut queue = std::collections::VecDeque::from([(root.to_path_buf(), 0usize)]);
    let mut seen = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            seen += 1;
            if seen > max_entries {
                out.sort();
                return out;
            }
            // DirEntry::file_type does not follow symlinks: links are neither walked nor read.
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name();
            let name = name.to_string_lossy();
            if ft.is_dir() {
                if depth < max_depth && !skip(&name) {
                    queue.push_back((e.path(), depth + 1));
                }
            } else if ft.is_file() {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

/// Whether `bin` is an executable file on `PATH`.
pub(crate) fn on_path(bin: &str) -> bool {
    crate::util::which(bin)
}

/// `~/…` form of an absolute path under `$HOME`, for display and for configs.
pub(crate) fn tilde(p: &Path) -> String {
    crate::config::contract_tilde(p)
}

/// Files that must never be previewed, served or attached: env files, keys, tokens.
/// Names only; contents are never read.
fn detect_sensitive(cx: &mut Ctx) {
    let mut found: Vec<String> = vec![];
    for f in cx.files.iter().chain(&cx.other_files) {
        let Some(name) = f.file_name().and_then(|n| n.to_str()) else { continue };
        if is_sensitive_name(name) {
            found.push(cx.rel(f));
        }
        if found.len() >= 50 {
            break;
        }
    }
    for s in found {
        if !cx.pf.project.sensitive.contains(&s) {
            cx.pf.project.sensitive.push(s);
        }
    }
}

pub(crate) fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let template = ["example", "sample", "template", "dist", "defaults"].iter().any(|t| lower.ends_with(t));
    if template {
        return false;
    }
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".env")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || lower.contains("api_sk")
        || lower.ends_with("_token")
        || lower.ends_with(".token")
        || (lower.starts_with("credentials") && lower.ends_with(".json"))
        || (lower.starts_with("service-account") && lower.ends_with(".json"))
}

/// Innermost enclosing `{…}` body helpers shared by the source-code detectors.
pub(crate) mod text {
    /// The balanced `{…}` block starting at the first `{` at or after `from`
    /// (returns the byte range including both braces).
    pub fn block_at(s: &str, from: usize) -> Option<(usize, usize)> {
        let bytes = s.as_bytes();
        let start = from + s.get(from..)?.find('{')?;
        let mut depth = 0usize;
        for (i, &b) in bytes.iter().enumerate().skip(start) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some((start, i + 1));
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// JSON with comments and trailing commas (`deno.jsonc`, `tsconfig.json`,
    /// `turbo.json`) → plain JSON. Strings are left alone.
    pub fn strip_jsonc(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        let mut in_str = false;
        while let Some(c) = chars.next() {
            if in_str {
                out.push(c);
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                            out.push(n);
                        }
                    }
                    '"' => in_str = false,
                    _ => {}
                }
                continue;
            }
            match (c, chars.peek()) {
                ('"', _) => {
                    in_str = true;
                    out.push(c);
                }
                ('/', Some('/')) => {
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                ('/', Some('*')) => {
                    chars.next();
                    let mut prev = ' ';
                    for n in chars.by_ref() {
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                    out.push(' ');
                }
                _ => out.push(c),
            }
        }
        // Trailing commas: `,` followed only by whitespace before `}` or `]`.
        let bytes = out.as_bytes();
        let mut keep = String::with_capacity(out.len());
        let mut in_str = false;
        let mut escaped = false;
        for (i, ch) in out.char_indices() {
            if in_str {
                keep.push(ch);
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_str = false;
                }
                continue;
            }
            if ch == '"' {
                in_str = true;
            } else if ch == ',' {
                let next = bytes[i + 1..].iter().find(|b| !b.is_ascii_whitespace());
                if matches!(next, Some(b'}') | Some(b']')) {
                    continue;
                }
            }
            keep.push(ch);
        }
        keep
    }

    /// Truncate to at most `max` chars on a char boundary, with an ellipsis.
    pub fn ellipsize(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            return s.to_string();
        }
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
