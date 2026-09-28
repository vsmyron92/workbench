//! Python projects → runs.
//!
//! A project is a directory with `pyproject.toml`, `setup.py`, `setup.cfg` or a
//! `Pipfile`; `requirements*.txt` or `manage.py` alone count when no enclosing
//! directory is a Python project. Each project gets:
//! * a **runner** from its lockfile or tool section: `uv run`, `poetry run`,
//!   `pdm run`, `hatch run`, `pipenv run`, `rye run`, else the project's virtualenv
//!   (`.venv/bin/python -m …`) or `python3 -m …`;
//! * its scripts: `[project.scripts]`, Poetry, PDM, Hatch, Rye and poe tasks, Pipfile `[scripts]`;
//! * tests: pytest (configured or a dependency), else `unittest discover` for a
//!   `tests/` directory; Django's `manage.py test`; tox and nox;
//! * `ruff check` / `mypy` when configured;
//! * servers: Django `runserver` (:8000), FastAPI/Starlette/Litestar through
//!   uvicorn (:8000), Flask `flask run` (:5000), Streamlit (:8501).
//!
//! Jupyter notebooks are recorded as components (one per directory).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, TaskGraph, command_words, deploy_or, group_of, kind_from_task_name, scoped, sh, source, task_group};
use crate::config::project::{Component, Ready, RunConfig, RunKind};

/// pytest's summary line: `==== 1 failed, 12 passed, 2 skipped in 0.52s ====`.
pub const PYTEST_RESULT: &str =
    r"^=+ (?:(?P<failed>\d+) failed(?:, )?)?(?:(?P<passed>\d+) passed)?[^=]* in \d+(?:\.\d+)?s(?: \([\d:.]+\))? =+$";

/// Markers that make a directory a Python project on their own.
const STRONG: &[&str] = &["pyproject.toml", "setup.py", "setup.cfg", "Pipfile"];
/// Python projects handled per detection (a monorepo of services has a few).
const MAX_PROJECTS: usize = 12;
/// Scripts / tasks offered per project.
const MAX_SCRIPTS: usize = 25;
/// Source files read per project to find a web app.
const MAX_APP_READS: usize = 30;

static FASTAPI_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(\w+)\s*(?::\s*[\w.\[\]]+\s*)?=\s*(?:fastapi\.|starlette\.applications\.|litestar\.)?(FastAPI|Starlette|Litestar)\s*\(")
        .unwrap()
});
static FLASK_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(\w+)\s*(?::\s*[\w.]+\s*)?=\s*(?:flask\.)?Flask\s*\(").unwrap());
static FACTORY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^def (create_app|make_app)\s*\(").unwrap());
static RUN_PORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\.run\([^)]{0,200}?\bport\s*=\s*(\d{2,5})").unwrap());
static STREAMLIT_IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*import streamlit\b").unwrap());

/// How commands run in a project's environment.
#[derive(Debug, Clone, PartialEq)]
pub enum Runner {
    Uv,
    Poetry,
    Pdm,
    Hatch,
    Pipenv,
    Rye,
    /// pip: the project's virtualenv directory (absolute), if it has one.
    Plain(Option<PathBuf>),
}

impl Runner {
    fn prefix(&self) -> Option<&'static str> {
        match self {
            Runner::Uv => Some("uv run"),
            Runner::Poetry => Some("poetry run"),
            Runner::Pdm => Some("pdm run"),
            Runner::Hatch => Some("hatch run"),
            Runner::Pipenv => Some("pipenv run"),
            Runner::Rye => Some("rye run"),
            Runner::Plain(_) => None,
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Runner::Uv => "uv",
            Runner::Poetry => "poetry",
            Runner::Pdm => "pdm",
            Runner::Hatch => "hatch",
            Runner::Pipenv => "pipenv",
            Runner::Rye => "rye",
            Runner::Plain(_) => "pip",
        }
    }

    /// The interpreter for a run in `cwd`: the virtualenv's, relative to `cwd`, else `python3`.
    fn interpreter(&self, cwd: &Path) -> String {
        match self {
            Runner::Plain(Some(venv)) => format!("{}/bin/python", relative(cwd, venv)),
            _ => "python3".into(),
        }
    }

    /// A tool that installs a console script (`pytest`, `ruff`, `uvicorn`):
    /// `uv run pytest`, else `python3 -m pytest` with the project's interpreter.
    pub fn tool(&self, cwd: &Path, bin: &str, args: &str) -> String {
        match self.prefix() {
            Some(p) => join(&[p, bin, args]),
            None => join(&[&self.interpreter(cwd), "-m", bin, args]),
        }
    }

    /// A module run with `python -m` (`unittest`).
    pub fn module(&self, cwd: &Path, m: &str, args: &str) -> String {
        match self.prefix() {
            Some(p) => join(&[p, "python -m", m, args]),
            None => join(&[&self.interpreter(cwd), "-m", m, args]),
        }
    }

    /// `python <args>`: `uv run python manage.py runserver`, `.venv/bin/python manage.py runserver`.
    pub fn python(&self, cwd: &Path, args: &str) -> String {
        match self.prefix() {
            Some(p) => join(&[p, "python", args]),
            None => join(&[&self.interpreter(cwd), args]),
        }
    }

    /// A console script the project installs (`[project.scripts]`).
    pub fn script(&self, cwd: &Path, name: &str) -> String {
        let name = &sh(name);
        match self {
            Runner::Plain(Some(venv)) => format!("{}/bin/{name}", relative(cwd, venv)),
            Runner::Plain(None) => name.to_string(),
            _ => join(&[self.prefix().unwrap_or_default(), name]),
        }
    }
}

fn join(parts: &[&str]) -> String {
    parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// `to` as seen from `from` (both absolute): `.venv`, `../.venv`.
fn relative(from: &Path, to: &Path) -> String {
    let f: Vec<_> = from.components().collect();
    let t: Vec<_> = to.components().collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), f.len() - common).collect();
    parts.extend(t[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

/// `requirements.txt`, `requirements-dev.txt`, or any `.txt` in a `requirements/` directory.
fn is_requirements(f: &Path) -> bool {
    let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let in_dir = f.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "requirements");
    name.ends_with(".txt") && (name.starts_with("requirements") || in_dir)
}

pub fn detect(cx: &mut Ctx, files: &[PathBuf]) {
    notebooks(cx, files);
    let mut dirs: Vec<PathBuf> = vec![];
    for f in files {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if STRONG.contains(&name) {
            if let Some(d) = f.parent().filter(|d| !dirs.iter().any(|x| x == d)) {
                dirs.push(d.to_path_buf());
            }
        }
    }
    for f in files {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !(is_requirements(f) || name == "manage.py") {
            continue;
        }
        let Some(mut d) = f.parent() else { continue };
        // `requirements/base.txt` belongs to the directory above.
        if d.file_name().is_some_and(|n| n == "requirements") {
            d = d.parent().unwrap_or(d);
        }
        // Inside a project already (`docs/requirements.txt`, Django's `mysite/manage.py`).
        if !dirs.iter().any(|x| d.starts_with(x)) {
            dirs.push(d.to_path_buf());
        }
    }
    dirs.sort_by(|a, b| a.components().count().cmp(&b.components().count()).then_with(|| a.cmp(b)));
    for d in dirs.iter().take(MAX_PROJECTS) {
        project(cx, d, files, &dirs);
    }
}

/// Everything one Python project declares.
struct Py {
    dir: PathBuf,
    cwd: String,
    /// Parsed `pyproject.toml` (empty when absent or invalid).
    toml: toml::Table,
    /// The file named in run sources: `pyproject.toml`, else the marker that made it a project.
    marker: PathBuf,
    /// Lower-cased dependency declarations (pyproject, setup.*, requirements, Pipfile).
    deps: String,
    runner: Runner,
}

impl Py {
    fn tool(&self, key: &str) -> Option<&toml::Value> {
        self.toml.get("tool").and_then(|t| t.get(key))
    }

    /// Whether dependency `name` is declared (a whole name: `pytest` is not `pytest-django`).
    fn mentions(&self, name: &str) -> bool {
        let mut from = 0;
        while let Some(i) = self.deps[from..].find(name) {
            let at = from + i;
            let before = self.deps[..at].chars().next_back();
            let after = self.deps[at + name.len()..].chars().next();
            let edge_before = before.is_none_or(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '[' | ',' | '{'));
            let edge_after = after.is_none_or(|c| c.is_whitespace() || matches!(c, '"' | '\'' | ']' | '<' | '>' | '=' | '!' | '~' | ';' | ',' | '[' | '^' | '@'));
            if edge_before && edge_after {
                return true;
            }
            from = at + name.len();
        }
        false
    }
}

fn project(cx: &mut Ctx, dir: &Path, files: &[PathBuf], all_dirs: &[PathBuf]) {
    let cwd = cx.rel(dir);
    let pyproject = dir.join("pyproject.toml");
    let raw = if pyproject.is_file() { cx.read(&pyproject).unwrap_or_default() } else { String::new() };
    let table = raw.parse::<toml::Table>().unwrap_or_default();
    let mut deps = raw.clone();
    for n in ["setup.cfg", "setup.py", "Pipfile"] {
        let p = dir.join(n);
        if p.is_file() {
            deps.push('\n');
            deps.push_str(&cx.read(&p).unwrap_or_default());
        }
    }
    let reqs: Vec<PathBuf> = files
        .iter()
        .filter(|f| is_requirements(f) && (f.parent() == Some(dir) || f.parent() == Some(&dir.join("requirements"))))
        .take(10)
        .cloned()
        .collect();
    for r in &reqs {
        deps.push('\n');
        deps.push_str(&cx.read(r).unwrap_or_default());
    }
    let marker = [pyproject.clone(), dir.join("setup.py"), dir.join("setup.cfg"), dir.join("Pipfile")]
        .into_iter()
        .find(|p| p.is_file())
        .or_else(|| reqs.first().cloned())
        .unwrap_or_else(|| dir.join("manage.py"));
    let runner = choose_runner(cx, dir, &table);
    let py = Py { dir: dir.to_path_buf(), cwd: cwd.clone(), toml: table, marker, deps: deps.to_ascii_lowercase(), runner };

    cx.tag("python");
    cx.tag(py.runner.tag());
    let version = py
        .toml
        .get("project")
        .and_then(|p| p.get("requires-python"))
        .or_else(|| py.tool("poetry").and_then(|p| p.get("dependencies")).and_then(|d| d.get("python")))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    cx.pf.components.push(Component { name: scoped("python", &cwd), path: cwd.clone(), kind: "python".into(), version });

    // The project's own files: not those of a nested Python project.
    let own: Vec<PathBuf> = files
        .iter()
        .filter(|f| f.starts_with(dir) && !all_dirs.iter().any(|o| o != dir && o.starts_with(dir) && f.starts_with(o)))
        .cloned()
        .collect();
    scripts(cx, &py);
    let django = django(cx, &py, &own);
    web_apps(cx, &py, &own, django);
    tests(cx, &py, &own, django);
    linters(cx, &py);
}

/// The project's lockfile, else its own tool sections, else the lockfile of an
/// enclosing workspace (a uv or poetry workspace root), else pip.
fn choose_runner(cx: &Ctx, dir: &Path, t: &toml::Table) -> Runner {
    let lock_in = |d: &Path| {
        [("uv.lock", Runner::Uv), ("poetry.lock", Runner::Poetry), ("pdm.lock", Runner::Pdm), ("Pipfile.lock", Runner::Pipenv)]
            .into_iter()
            .find(|(f, _)| d.join(f).is_file())
            .map(|(_, r)| r)
    };
    if let Some(r) = lock_in(dir) {
        return r;
    }
    let tool = |k: &str| t.get("tool").and_then(|x| x.get(k)).and_then(|v| v.as_table());
    // `[tool.pdm.build]` / `[tool.hatch.build]` only configure a build backend.
    let uses = |k: &str, not: &[&str]| tool(k).is_some_and(|x| x.keys().any(|key| !not.contains(&key.as_str())));
    if tool("poetry").is_some() {
        return Runner::Poetry;
    }
    if uses("pdm", &["build", "version"]) {
        return Runner::Pdm;
    }
    if tool("rye").is_some() {
        return Runner::Rye;
    }
    if tool("hatch").is_some_and(|h| h.contains_key("envs")) {
        return Runner::Hatch;
    }
    if dir.join("Pipfile").is_file() {
        return Runner::Pipenv;
    }
    if tool("uv").is_some() {
        return Runner::Uv;
    }
    let mut up = dir.parent();
    while let Some(x) = up.filter(|x| x.starts_with(cx.root)) {
        if let Some(r) = lock_in(x) {
            return r;
        }
        up = x.parent();
    }
    Runner::Plain(find_venv(cx, dir))
}

/// A virtualenv in the project directory or the repository root.
fn find_venv(cx: &Ctx, dir: &Path) -> Option<PathBuf> {
    for base in [dir, cx.root] {
        for v in [".venv", "venv", "env", ".env"] {
            let p = base.join(v);
            if p.join("pyvenv.cfg").is_file() && p.join("bin/python").exists() {
                return Some(p);
            }
        }
    }
    None
}

/// Script tables: `[project.scripts]`, Poetry scripts (console entry points), and
/// the task runners of PDM, Hatch, Rye, poe and Pipenv.
fn scripts(cx: &mut Ctx, py: &Py) {
    // (name, command, body, source file, source suffix, whether what it runs reaches out)
    type Entry = (String, String, String, PathBuf, String, Option<bool>);
    let mut entries: Vec<Entry> = vec![];
    let cwd_abs = py.dir.clone();
    let marker = py.marker.clone();
    let console = |t: Option<&toml::Value>, section: &str, out: &mut Vec<Entry>| {
        for (k, v) in t.and_then(|s| s.as_table()).into_iter().flatten() {
            let body = v.as_str().unwrap_or_default().to_string();
            out.push((k.clone(), py.runner.script(&cwd_abs, k), body, marker.clone(), format!("#{section}.{k}"), None));
        }
    };
    console(py.toml.get("project").and_then(|p| p.get("scripts")), "project.scripts", &mut entries);
    console(py.tool("poetry").and_then(|p| p.get("scripts")), "tool.poetry.scripts", &mut entries);

    // `tool`: the runner's command word (`pdm run x`, `poe x`), for references in bodies.
    let tasks = |t: Option<&toml::Value>, file: &Path, section: &str, tool: &str, cmd: &dyn Fn(&str) -> String, out: &mut Vec<Entry>| {
        let Some(table) = t.and_then(|s| s.as_table()) else { return };
        // The whole table first: an entry is classified by everything it runs
        // (`ship = ["build", "upload"]`, PDM's `pre_x` / `post_x` hooks, `poe upload`).
        let mut graph = TaskGraph::default();
        for (k, v) in table {
            let (body, mut refs) = py_task(v);
            if tool == "pdm" {
                refs.extend([format!("pre_{k}"), format!("post_{k}")]);
            }
            refs.extend(tool_refs(&body, tool));
            graph.add(k, &body, refs);
        }
        for (k, v) in table {
            // Hooks (`pre_install`, `post_lock`) and private (`_x`) entries are not runs;
            // `[tool.pdm.scripts._]` holds shared options.
            if k.starts_with('_') || k.starts_with("pre_") || k.starts_with("post_") {
                continue;
            }
            let (body, _) = py_task(v);
            out.push((k.clone(), cmd(k), body, file.to_path_buf(), format!("#{section}.{k}"), Some(graph.reaches_out(k))));
        }
    };
    let pdm = |k: &str| format!("pdm run {}", sh(k));
    tasks(py.tool("pdm").and_then(|p| p.get("scripts")), &marker, "tool.pdm.scripts", "pdm", &pdm, &mut entries);
    let hatch_scripts = py.tool("hatch").and_then(|h| h.get("envs")).and_then(|e| e.get("default")).and_then(|d| d.get("scripts"));
    let hatch = |k: &str| format!("hatch run {}", sh(k));
    tasks(hatch_scripts, &marker, "tool.hatch.envs.default.scripts", "hatch", &hatch, &mut entries);
    let rye = |k: &str| format!("rye run {}", sh(k));
    tasks(py.tool("rye").and_then(|r| r.get("scripts")), &marker, "tool.rye.scripts", "rye", &rye, &mut entries);
    let poe = |k: &str| py.runner.tool(&cwd_abs, "poe", &sh(k));
    tasks(py.tool("poe").and_then(|p| p.get("tasks")), &marker, "tool.poe.tasks", "poe", &poe, &mut entries);
    let pipfile = py.dir.join("Pipfile");
    if pipfile.is_file() {
        if let Some(t) = cx.read(&pipfile).and_then(|s| s.parse::<toml::Table>().ok()) {
            let pipenv = |k: &str| format!("pipenv run {}", sh(k));
            tasks(t.get("scripts"), &pipfile, "scripts", "pipenv", &pipenv, &mut entries);
        }
    }

    let mut added = 0;
    for (name, command, body, file, suffix, reaches) in entries {
        if added >= MAX_SCRIPTS {
            break;
        }
        let kind = kind_from_task_name(&name);
        let port = (kind == RunKind::Server).then(|| super::port_in(&body)).flatten();
        let run = RunConfig {
            name: scoped(&name, &py.cwd),
            kind,
            command,
            cwd: py.cwd.clone(),
            port,
            preview: port.map(|p| format!("http://localhost:{p}/")),
            source: source(cx, &file, &suffix),
            group: Some(match reaches {
                Some(r) => deploy_or(kind, r),
                None => task_group(kind, &body),
            }),
            ..Default::default()
        };
        if cx.add_run(run).is_some() {
            added += 1;
        }
    }
}

/// An entry of a Python task table (PDM, Hatch, Rye, poe, Pipenv): its command text
/// and the entries it names: the items of a sequence (`ship = ["build", "upload"]`,
/// `composite`, `chain`, `sequence`), `ref`, `deps` and `uses`. Names that are not
/// entries of the table are ignored later (`TaskGraph`).
fn py_task(v: &toml::Value) -> (String, Vec<String>) {
    let first_word = |s: &str| s.split_whitespace().next().map(str::to_string);
    let mut body: Vec<String> = vec![];
    let mut refs: Vec<String> = vec![];
    let items = |a: &[toml::Value], body: &mut Vec<String>, refs: &mut Vec<String>| {
        for x in a {
            match x {
                toml::Value::String(s) => {
                    body.push(s.clone());
                    refs.extend(first_word(s));
                }
                toml::Value::Table(_) => {
                    let (b, r) = py_task(x);
                    body.push(b);
                    refs.extend(r);
                }
                _ => {}
            }
        }
    };
    match v {
        toml::Value::String(s) => body.push(s.clone()),
        toml::Value::Array(a) => items(a, &mut body, &mut refs),
        toml::Value::Table(t) => {
            for key in ["cmd", "shell", "call", "script", "expr"] {
                match t.get(key) {
                    Some(toml::Value::Array(a)) => body.push(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")),
                    Some(other) => body.extend(other.as_str().map(str::to_string)),
                    None => {}
                }
            }
            for key in ["composite", "chain", "sequence", "parallel"] {
                if let Some(toml::Value::Array(a)) = t.get(key) {
                    items(a, &mut body, &mut refs);
                }
            }
            refs.extend(t.get("ref").and_then(|r| r.as_str()).and_then(first_word));
            for d in t.get("deps").and_then(|d| d.as_array()).into_iter().flatten() {
                refs.extend(d.as_str().and_then(first_word));
            }
            for u in t.get("uses").and_then(|u| u.as_table()).into_iter().flat_map(|u| u.values()) {
                refs.extend(u.as_str().and_then(first_word));
            }
        }
        _ => {}
    }
    (body.join(" && "), refs)
}

/// Entries a command runs through the table's own runner: `poe upload`,
/// `pdm run build`, `hatch run default:docs`.
fn tool_refs(body: &str, tool: &str) -> Vec<String> {
    let mut out = vec![];
    for words in command_words(body) {
        let Some(i) = words.iter().position(|w| w == tool) else { continue };
        let mut rest = words[i + 1..].iter().filter(|w| !w.starts_with('-'));
        let mut next = rest.next();
        if next.is_some_and(|w| w == "run") {
            next = rest.next();
        }
        if let Some(n) = next {
            out.push(n.rsplit(':').next().unwrap_or(n).to_string());
        }
    }
    out
}

/// `manage.py` of a Django project (in the project directory or one level down)
/// → `runserver` and `test`. Returns whether the project is a Django project.
fn django(cx: &mut Ctx, py: &Py, own: &[PathBuf]) -> bool {
    let manage = own.iter().find(|f| {
        f.file_name().is_some_and(|n| n == "manage.py")
            && (f.parent() == Some(py.dir.as_path()) || f.parent().and_then(|p| p.parent()) == Some(py.dir.as_path()))
    });
    let Some(manage) = manage.cloned() else { return false };
    let Some(src) = cx.read(&manage) else { return false };
    if !src.contains("django") && !src.contains("DJANGO_SETTINGS_MODULE") {
        return false;
    }
    let Some(dir) = manage.parent() else { return false };
    let cwd = cx.rel(dir);
    cx.tag("django");
    cx.add_run(RunConfig {
        name: scoped("runserver", &cwd),
        kind: RunKind::Server,
        command: py.runner.python(dir, "manage.py runserver"),
        cwd: cwd.clone(),
        port: Some(8000),
        ready: Some(Ready { log: Some(r"Starting development server at (https?://\S+)".into()), http: None, timeout_s: 120 }),
        preview: Some("http://localhost:8000/".into()),
        source: source(cx, &manage, ""),
        group: Some("dev".into()),
        ..Default::default()
    });
    if !py.mentions("pytest-django") {
        cx.add_run(RunConfig {
            name: scoped("django test", &cwd),
            kind: RunKind::Test,
            command: py.runner.python(dir, "manage.py test"),
            cwd,
            source: source(cx, &manage, ""),
            group: Some("test".into()),
            ..Default::default()
        });
    }
    true
}

/// A file that may define the web app: `main.py`, `app/main.py`, `src/pkg/app.py`…
fn app_candidates(py: &Py, own: &[PathBuf]) -> Vec<PathBuf> {
    const NAMES: &[&str] = &[
        "main.py", "app.py", "api.py", "server.py", "asgi.py", "wsgi.py", "application.py", "__init__.py",
        "streamlit_app.py", "Home.py", "dashboard.py",
    ];
    let mut out: Vec<PathBuf> = own
        .iter()
        .filter(|f| {
            let Ok(rel) = f.strip_prefix(&py.dir) else { return false };
            let depth = rel.components().count();
            let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let in_tests = rel.components().any(|c| matches!(c.as_os_str().to_str(), Some("tests" | "test" | "docs" | "migrations" | "scripts")));
            NAMES.contains(&name) && depth <= 3 && !in_tests && !(name == "__init__.py" && depth == 1)
        })
        .cloned()
        .collect();
    // Shallow first; within a depth, `main.py`/`app.py` before `__init__.py`.
    out.sort_by_key(|f| (f.components().count(), f.file_name().is_some_and(|n| n == "__init__.py")));
    out.truncate(MAX_APP_READS);
    out
}

/// `app/main.py` → (`app.main`, None); `src/pkg/api.py` → (`pkg.api`, Some("src")).
fn module_of(py: &Py, f: &Path) -> Option<(String, Option<&'static str>)> {
    let rel = f.strip_prefix(&py.dir).ok()?.with_extension("");
    let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let app_dir = (parts.len() > 1 && parts[0] == "src").then(|| {
        parts.remove(0);
        "src"
    });
    if parts.last().is_some_and(|p| p == "__init__") {
        parts.pop();
    }
    if parts.is_empty() || parts.iter().any(|p| !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        return None;
    }
    Some((parts.join("."), app_dir))
}

/// FastAPI / Starlette / Litestar (uvicorn), Flask and Streamlit apps.
fn web_apps(cx: &mut Ctx, py: &Py, own: &[PathBuf], django: bool) {
    let (mut asgi, mut flask, mut streamlit) = (false, false, false);
    for f in app_candidates(py, own) {
        if asgi && flask && streamlit {
            break;
        }
        let Some(src) = cx.read(&f) else { continue };
        let port = RUN_PORT.captures(&src).and_then(|c| c[1].parse::<u16>().ok()).filter(|p| *p >= 80);
        if !asgi && !django {
            let target = FASTAPI_VAR
                .captures(&src)
                .map(|c| (c[1].to_string(), c[2].to_string(), false))
                .or_else(|| {
                    let fw = ["FastAPI(", "Starlette(", "Litestar("].into_iter().find(|k| src.contains(k))?;
                    FACTORY.captures(&src).map(|c| (c[1].to_string(), fw.trim_end_matches('(').to_string(), true))
                });
            if let (Some((var, framework, factory)), Some((module, app_dir))) = (target, module_of(py, &f)) {
                asgi = true;
                let port = port.unwrap_or(8000);
                let server = if py.mentions("hypercorn") && !py.mentions("uvicorn") { "hypercorn" } else { "uvicorn" };
                let mut args = format!("{module}:{var}");
                if factory {
                    args = if server == "uvicorn" { format!("--factory {args}") } else { format!("'{module}:{var}()'") };
                }
                let mut env = BTreeMap::new();
                let args = if server == "uvicorn" {
                    let d = app_dir.map(|d| format!(" --app-dir {d}")).unwrap_or_default();
                    let p = if port == 8000 { String::new() } else { format!(" --port {port}") };
                    format!("{args} --reload{d}{p}")
                } else {
                    // hypercorn has no --app-dir: a src layout goes on PYTHONPATH.
                    if let Some(d) = app_dir {
                        env.insert("PYTHONPATH".to_string(), d.to_string());
                    }
                    format!("{args} --reload --bind 127.0.0.1:{port}")
                };
                cx.tag(&framework.to_ascii_lowercase());
                cx.add_run(RunConfig {
                    name: scoped(server, &py.cwd),
                    kind: RunKind::Server,
                    command: py.runner.tool(&py.dir, server, &args),
                    cwd: py.cwd.clone(),
                    env,
                    port: Some(port),
                    ready: Some(Ready {
                        log: Some(r"(?:Uvicorn running on|Running on) (https?://\S+)".into()),
                        http: None,
                        timeout_s: 120,
                    }),
                    preview: Some(format!("http://localhost:{port}/")),
                    source: source(cx, &f, &format!(" ({framework})")),
                    group: Some("dev".into()),
                    ..Default::default()
                });
                continue;
            }
        }
        if !flask && !django && (FLASK_VAR.is_match(&src) || (src.contains("Flask(") && FACTORY.is_match(&src))) {
            if let Some((module, app_dir)) = module_of(py, &f) {
                flask = true;
                let var = FLASK_VAR.captures(&src).map(|c| c[1].to_string()).filter(|v| v != "app" && v != "application");
                let target = match var {
                    Some(v) => format!("{module}:{v}"),
                    None => module,
                };
                let port = port.unwrap_or(5000);
                let p = if port == 5000 { String::new() } else { format!(" --port {port}") };
                let mut run = RunConfig {
                    name: scoped("flask run", &py.cwd),
                    kind: RunKind::Server,
                    command: py.runner.tool(&py.dir, "flask", &format!("--app {target} run --debug{p}")),
                    cwd: py.cwd.clone(),
                    port: Some(port),
                    ready: Some(Ready { log: Some(r"Running on (https?://\S+)".into()), http: None, timeout_s: 120 }),
                    preview: Some(format!("http://localhost:{port}/")),
                    source: source(cx, &f, " (Flask)"),
                    group: Some("dev".into()),
                    ..Default::default()
                };
                if let Some(d) = app_dir {
                    run.env = BTreeMap::from([("PYTHONPATH".to_string(), d.to_string())]);
                }
                cx.tag("flask");
                cx.add_run(run);
                continue;
            }
        }
        if !streamlit && py.mentions("streamlit") && STREAMLIT_IMPORT.is_match(&src) {
            streamlit = true;
            let rel = f.strip_prefix(&py.dir).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
            cx.tag("streamlit");
            cx.add_run(RunConfig {
                name: scoped("streamlit", &py.cwd),
                kind: RunKind::Server,
                command: py.runner.tool(&py.dir, "streamlit", &format!("run {}", sh(&rel))),
                cwd: py.cwd.clone(),
                port: Some(8501),
                ready: Some(Ready { log: Some(r"Local URL: (https?://\S+)".into()), http: None, timeout_s: 120 }),
                preview: Some("http://localhost:8501/".into()),
                source: source(cx, &f, " (Streamlit)"),
                group: Some("dev".into()),
                ..Default::default()
            });
        }
    }
}

fn tests(cx: &mut Ctx, py: &Py, own: &[PathBuf], django: bool) {
    let dir = &py.dir;
    let read_has = |cx: &mut Ctx, name: &str, needle: &str| {
        let p = dir.join(name);
        p.is_file() && cx.read(&p).is_some_and(|t| t.contains(needle))
    };
    let configured = py.tool("pytest").is_some()
        || dir.join("pytest.ini").is_file()
        || dir.join("conftest.py").is_file()
        || dir.join("tests/conftest.py").is_file()
        || read_has(cx, "setup.cfg", "[tool:pytest]")
        || read_has(cx, "tox.ini", "[pytest]");
    let tests_dir = ["tests", "test"].into_iter().find(|t| {
        let d = dir.join(t);
        own.iter().any(|f| {
            f.starts_with(&d)
                && f.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".py") && (n.starts_with("test") || n.ends_with("_test.py")))
        })
    });
    let file = py.marker.clone();
    if configured || py.mentions("pytest") || py.mentions("pytest-django") {
        cx.add_run(RunConfig {
            name: scoped("pytest", &py.cwd),
            kind: RunKind::Test,
            command: py.runner.tool(dir, "pytest", ""),
            cwd: py.cwd.clone(),
            result_pattern: Some(PYTEST_RESULT.into()),
            source: source(cx, &file, if configured { " (pytest config)" } else { " (pytest dependency)" }),
            group: Some("test".into()),
            ..Default::default()
        });
    } else if let (Some(t), false) = (tests_dir, django) {
        cx.add_run(RunConfig {
            name: scoped("unittest", &py.cwd),
            kind: RunKind::Test,
            command: py.runner.module(dir, "unittest", &format!("discover -s {t}")),
            cwd: py.cwd.clone(),
            source: source(cx, &dir.join(t), "/"),
            group: Some("test".into()),
            ..Default::default()
        });
    }
    if dir.join("tox.ini").is_file() || py.tool("tox").is_some() {
        let f = if dir.join("tox.ini").is_file() { dir.join("tox.ini") } else { file.clone() };
        cx.add_run(RunConfig {
            name: scoped("tox", &py.cwd),
            kind: RunKind::Test,
            command: "tox".into(),
            cwd: py.cwd.clone(),
            source: source(cx, &f, ""),
            group: Some("test".into()),
            ..Default::default()
        });
    }
    if dir.join("noxfile.py").is_file() {
        cx.add_run(RunConfig {
            name: scoped("nox", &py.cwd),
            kind: RunKind::Test,
            command: "nox".into(),
            cwd: py.cwd.clone(),
            source: source(cx, &dir.join("noxfile.py"), ""),
            group: Some("test".into()),
            ..Default::default()
        });
    }
}

/// `ruff check` and `mypy` when the project configures them.
fn linters(cx: &mut Ctx, py: &Py) {
    let dir = &py.dir;
    let ruff_cfg = [dir.join("ruff.toml"), dir.join(".ruff.toml")].into_iter().find(|p| p.is_file());
    if py.tool("ruff").is_some() || ruff_cfg.is_some() {
        let f = ruff_cfg.unwrap_or_else(|| py.marker.clone());
        cx.add_run(RunConfig {
            name: scoped("ruff check", &py.cwd),
            kind: RunKind::Task,
            command: py.runner.tool(dir, "ruff", "check ."),
            cwd: py.cwd.clone(),
            source: source(cx, &f, if f.ends_with("pyproject.toml") { "#tool.ruff" } else { "" }),
            group: Some(group_of(RunKind::Task).into()),
            ..Default::default()
        });
    }
    let ini = [dir.join("mypy.ini"), dir.join(".mypy.ini")].into_iter().find(|p| p.is_file());
    let cfg_text = match &ini {
        Some(p) => cx.read(p),
        None => {
            let sc = dir.join("setup.cfg");
            if sc.is_file() { cx.read(&sc).filter(|t| t.contains("[mypy]")) } else { None }
        }
    };
    let in_pyproject = py.tool("mypy");
    if in_pyproject.is_some() || cfg_text.is_some() {
        let has_files = in_pyproject.and_then(|m| m.get("files")).is_some()
            || in_pyproject.and_then(|m| m.get("packages")).is_some()
            || cfg_text.is_some_and(|t| t.lines().any(|l| l.trim_start().starts_with("files") && l.contains('=')));
        let f = ini.unwrap_or_else(|| if in_pyproject.is_some() { py.marker.clone() } else { dir.join("setup.cfg") });
        cx.add_run(RunConfig {
            name: scoped("mypy", &py.cwd),
            kind: RunKind::Task,
            command: py.runner.tool(dir, "mypy", if has_files { "" } else { "." }),
            cwd: py.cwd.clone(),
            source: source(cx, &f, if f.ends_with("pyproject.toml") { "#tool.mypy" } else { "" }),
            group: Some(group_of(RunKind::Task).into()),
            ..Default::default()
        });
    }
}

/// Directories with Jupyter notebooks → components (`kind = "jupyter"`).
fn notebooks(cx: &mut Ctx, files: &[PathBuf]) {
    let mut dirs: Vec<PathBuf> = vec![];
    for f in files {
        if f.extension().is_some_and(|e| e == "ipynb") {
            if let Some(d) = f.parent().filter(|d| !dirs.iter().any(|x| x == d)) {
                dirs.push(d.to_path_buf());
            }
        }
    }
    if dirs.is_empty() {
        return;
    }
    cx.tag("jupyter");
    for d in dirs.into_iter().take(10) {
        let cwd = cx.rel(&d);
        let name = scoped("notebooks", &cwd);
        if !cx.pf.components.iter().any(|c| c.name == name) {
            cx.pf.components.push(Component { name, path: cwd, kind: "jupyter".into(), version: None });
        }
    }
}
