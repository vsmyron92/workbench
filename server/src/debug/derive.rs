//! Launch configurations derived from the project, without running anything: Cargo
//! targets from `Cargo.toml` files, CMake executables from `CMakeLists.txt` (when a
//! build directory exists), Python scripts and modules from run configurations, Go
//! main packages. Like `apps::detect`, this is pure, bounded and read-only: running
//! `cargo metadata` would already run repository code (a `rust-toolchain.toml` can
//! name a toolchain inside the repository, `.cargo/config.toml` a rustc wrapper).
//!
//! The executable of a Cargo target is found only when the user starts it: the
//! pre-launch build (`cargo build --message-format=json-render-diagnostics`) writes
//! its JSON messages to a file that `cargo_executable` reads.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::util::os::shell::Dialect;

const MAX_MEMBERS: usize = 64;
const MAX_TARGETS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CargoKind {
    Bin,
    Example,
    Test,
    /// The library's unit tests (`cargo test --lib`).
    Lib,
}

impl CargoKind {
    pub fn label(self) -> &'static str {
        match self {
            CargoKind::Bin => "bin",
            CargoKind::Example => "example",
            CargoKind::Test => "test",
            CargoKind::Lib => "unit tests",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CargoTarget {
    pub package: String,
    /// The Cargo project (workspace or package) the build runs in, relative to the
    /// project root (`""` for the root).
    pub workspace: String,
    /// The package directory, relative to the project root (`""` for the root).
    pub dir: String,
    /// Target name (for `Lib`, the library's crate name).
    pub name: String,
    pub kind: CargoKind,
}

impl CargoTarget {
    pub fn config_name(&self) -> String {
        match self.kind {
            CargoKind::Lib => format!("Cargo: unit tests ({})", self.package),
            k => format!("Cargo: {} {}", k.label(), self.name),
        }
    }

    /// `cargo …` arguments of the build that produces the executable.
    pub fn build_args(&self) -> Vec<String> {
        let mut a: Vec<String> = match self.kind {
            CargoKind::Bin => vec!["build".into(), "-p".into(), self.package.clone(), "--bin".into(), self.name.clone()],
            CargoKind::Example => vec!["build".into(), "-p".into(), self.package.clone(), "--example".into(), self.name.clone()],
            CargoKind::Test => vec!["test".into(), "-p".into(), self.package.clone(), "--test".into(), self.name.clone(), "--no-run".into()],
            CargoKind::Lib => vec!["test".into(), "-p".into(), self.package.clone(), "--lib".into(), "--no-run".into()],
        };
        a.push("--message-format=json-render-diagnostics".into());
        a
    }
}

fn read_toml(path: &Path) -> Option<toml::Value> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return None;
    }
    toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Workspace members: plain paths and a trailing `*` component (`crates/*`).
fn members(root: &Path, manifest: &toml::Value) -> Vec<PathBuf> {
    let mut out = vec![];
    let Some(list) = manifest.get("workspace").and_then(|w| w.get("members")).and_then(|m| m.as_array()) else { return out };
    let excluded: Vec<String> = manifest
        .get("workspace")
        .and_then(|w| w.get("exclude"))
        .and_then(|e| e.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.trim_end_matches('/').to_string())).collect())
        .unwrap_or_default();
    for m in list.iter().filter_map(|v| v.as_str()) {
        if m.contains("..") || crate::util::os::path::is_absolute_str(m) {
            continue;
        }
        let m = m.trim_end_matches('/');
        // On Windows `C:x` would replace `root` (a trailing `*` is expanded below).
        if !crate::util::os::path::stays_inside(m.trim_end_matches('*')) {
            continue;
        }
        if let Some(prefix) = m.strip_suffix("/*").or(if m == "*" { Some("") } else { None }) {
            let dir = root.join(prefix);
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join("Cargo.toml").is_file()).take(MAX_MEMBERS).collect();
            v.sort();
            out.extend(v);
        } else if !m.contains('*') {
            out.push(root.join(m));
        }
        if out.len() >= MAX_MEMBERS {
            break;
        }
    }
    out.retain(|p| {
        let rel = p.strip_prefix(root).map(crate::util::os::path::to_slash).unwrap_or_default();
        !excluded.contains(&rel)
    });
    out
}

fn auto(manifest: &toml::Value, key: &str) -> bool {
    manifest.get("package").and_then(|p| p.get(key)).and_then(|v| v.as_bool()).unwrap_or(true)
}

/// Files `dir/*.rs` and folders `dir/*/main.rs`, as target names.
fn auto_targets(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v = vec![];
    for e in rd.flatten().take(500) {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_file() {
            if let Some(stem) = name.strip_suffix(".rs") {
                v.push(stem.to_string());
            }
        } else if p.join("main.rs").is_file() {
            v.push(name);
        }
    }
    v.retain(|n| plain_name(n));
    v.sort();
    v
}

fn explicit(manifest: &toml::Value, table: &str) -> Vec<(String, Option<String>)> {
    manifest
        .get(table)
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| Some((t.get("name")?.as_str()?.to_string(), t.get("path").and_then(|p| p.as_str()).map(str::to_string))))
                .filter(|(n, _)| plain_name(n))
                .collect()
        })
        .unwrap_or_default()
}

fn package_targets(root: &Path, workspace: &str, dir: &Path, manifest: &toml::Value, out: &mut Vec<CargoTarget>) {
    let Some(pkg) = manifest.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) else { return };
    if !plain_name(pkg) {
        return;
    }
    let rel = dir.strip_prefix(root).map(crate::util::os::path::to_slash).unwrap_or_default();
    let mut push = |name: String, kind: CargoKind| {
        if out.len() < MAX_TARGETS && !out.iter().any(|t| t.package == pkg && t.name == name && t.kind == kind) {
            out.push(CargoTarget { package: pkg.to_string(), workspace: workspace.to_string(), dir: rel.clone(), name, kind });
        }
    };
    // Binaries.
    let bins = explicit(manifest, "bin");
    for (n, _) in &bins {
        push(n.clone(), CargoKind::Bin);
    }
    if auto(manifest, "autobins") {
        if dir.join("src/main.rs").is_file() && !bins.iter().any(|(_, p)| p.as_deref() == Some("src/main.rs")) {
            push(pkg.to_string(), CargoKind::Bin);
        }
        for n in auto_targets(&dir.join("src/bin")) {
            push(n, CargoKind::Bin);
        }
    }
    // Library unit tests.
    let lib = manifest.get("lib");
    if lib.is_some() || dir.join("src/lib.rs").is_file() {
        let name = lib.and_then(|l| l.get("name")).and_then(|n| n.as_str()).map(str::to_string).unwrap_or_else(|| pkg.replace('-', "_"));
        let tests = lib.and_then(|l| l.get("test")).and_then(|t| t.as_bool()).unwrap_or(true);
        if tests && plain_name(&name) {
            push(name, CargoKind::Lib);
        }
    }
    // Integration tests and examples.
    for (n, _) in explicit(manifest, "test") {
        push(n, CargoKind::Test);
    }
    if auto(manifest, "autotests") {
        for n in auto_targets(&dir.join("tests")) {
            push(n, CargoKind::Test);
        }
    }
    for (n, _) in explicit(manifest, "example") {
        push(n, CargoKind::Example);
    }
    if auto(manifest, "autoexamples") {
        for n in auto_targets(&dir.join("examples")) {
            push(n, CargoKind::Example);
        }
    }
}

/// The debuggable targets of the Cargo project (or workspace) in `workspace`, a
/// folder of the project at `root` (`""`: the root itself).
pub fn cargo_targets(root: &Path, workspace: &str) -> Vec<CargoTarget> {
    let Ok(ws_dir) = crate::util::paths::resolve_in_root(root, workspace) else { return vec![] };
    let Some(manifest) = read_toml(&ws_dir.join("Cargo.toml")) else { return vec![] };
    let workspace = crate::util::paths::relative_to(root, &ws_dir).unwrap_or_default();
    let mut out = vec![];
    package_targets(root, &workspace, &ws_dir, &manifest, &mut out);
    for m in members(&ws_dir, &manifest) {
        if m == ws_dir {
            continue;
        }
        if let Some(mm) = read_toml(&m.join("Cargo.toml")) {
            package_targets(root, &workspace, &m, &mm, &mut out);
        }
    }
    out
}

fn kind_matches(kinds: &[Value], k: CargoKind) -> bool {
    let has = |s: &str| kinds.iter().any(|v| v.as_str() == Some(s));
    match k {
        CargoKind::Bin => has("bin"),
        CargoKind::Example => has("example"),
        CargoKind::Test => has("test"),
        CargoKind::Lib => ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"].iter().any(|s| has(s)),
    }
}

/// The executable Cargo built for `t`, from the JSON messages of the pre-launch build
/// (`--message-format=json…`, one message per line; other lines are ignored).
pub fn cargo_executable(messages: &str, t: &CargoTarget) -> Option<String> {
    let mut found = None;
    for line in messages.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("reason").and_then(Value::as_str) != Some("compiler-artifact") {
            continue;
        }
        let Some(exe) = v.get("executable").and_then(Value::as_str) else { continue };
        let target = &v["target"];
        if target.get("name").and_then(Value::as_str) != Some(t.name.as_str()) {
            continue;
        }
        let kinds = target.get("kind").and_then(Value::as_array).cloned().unwrap_or_default();
        if !kind_matches(&kinds, t.kind) {
            continue;
        }
        let test_profile = v.pointer("/profile/test").and_then(Value::as_bool).unwrap_or(false);
        if matches!(t.kind, CargoKind::Lib | CargoKind::Test) && !test_profile {
            continue;
        }
        found = Some(exe.to_string());
    }
    found
}

// ---------------------------------------------------------------- CMake

#[derive(Debug, Clone, PartialEq)]
pub struct CmakeTarget {
    /// The build directory (relative to the root) holding `CMakeCache.txt`.
    pub build_dir: String,
    pub name: String,
}

impl CmakeTarget {
    pub fn config_name(&self) -> String {
        format!("CMake: {}", self.name)
    }
}

const CMAKE_SKIP: &[&str] = &["node_modules", "target", "third_party", "external", "vendor", "deps", ".git"];

fn cmake_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, build_dirs: &[PathBuf]) {
    if out.len() >= 200 {
        return;
    }
    let f = dir.join("CMakeLists.txt");
    if f.is_file() {
        out.push(f);
    }
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subs: Vec<PathBuf> = rd
        .flatten()
        .take(1000)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.starts_with('.') && !CMAKE_SKIP.contains(&n.as_str())
        })
        .map(|e| e.path())
        .filter(|p| !build_dirs.contains(p) && !p.join("CMakeCache.txt").exists())
        .collect();
    subs.sort();
    for s in subs {
        cmake_files(&s, depth - 1, out, build_dirs);
    }
}

/// Build directories at the root: `build`, `cmake-build-*` and any folder with a
/// `CMakeCache.txt` (one level deep).
pub fn cmake_build_dirs(root: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root) else { return vec![] };
    let mut v: Vec<String> = rd
        .flatten()
        .take(500)
        .filter(|e| e.path().join("CMakeCache.txt").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| plain_name(n))
        .collect();
    // `build` first, then CLion's cmake-build-debug…, then the rest.
    v.sort_by_key(|n| (n != "build", !n.starts_with("cmake-build-debug"), n.clone()));
    v
}

/// `add_executable(<name> …)` of the project's CMake files, when a build directory
/// exists (otherwise nothing can be built yet: configure the project first).
pub fn cmake_targets(root: &Path) -> Vec<CmakeTarget> {
    let dirs = cmake_build_dirs(root);
    let Some(build) = dirs.first() else { return vec![] };
    if !root.join("CMakeLists.txt").is_file() {
        return vec![];
    }
    let re = regex::Regex::new(r"(?im)^\s*add_executable\s*\(\s*([A-Za-z0-9_.+-]+)([^)]*)").expect("regex");
    let build_paths: Vec<PathBuf> = dirs.iter().map(|d| root.join(d)).collect();
    let mut files = vec![];
    cmake_files(root, 3, &mut files, &build_paths);
    let mut out: Vec<CmakeTarget> = vec![];
    for f in files {
        let Ok(meta) = std::fs::metadata(&f) else { continue };
        if meta.len() > 512 * 1024 {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for c in re.captures_iter(&text) {
            let name = c[1].to_string();
            let rest = c.get(2).map(|m| m.as_str()).unwrap_or("");
            if rest.split_whitespace().any(|w| w == "IMPORTED" || w == "ALIAS") {
                continue;
            }
            if plain_name(&name) && !out.iter().any(|t| t.name == name) && out.len() < MAX_TARGETS {
                out.push(CmakeTarget { build_dir: build.clone(), name });
            }
        }
    }
    out
}

/// The newest executable file named `name` (`name.exe` on Windows) under `dir` (a CMake
/// build tree).
pub fn find_executable(dir: &Path, name: &str) -> Option<PathBuf> {
    let file_name = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 50_000 {
                return best.map(|(_, p)| p);
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                let n = e.file_name();
                if depth < 8 && n != "CMakeFiles" && !n.to_string_lossy().starts_with('.') {
                    stack.push((p, depth + 1));
                }
            } else if ft.is_file() && e.file_name().to_string_lossy() == file_name {
                let Ok(m) = e.metadata() else { continue };
                if !crate::util::os::exe::is_executable_file(&p, &m) {
                    continue;
                }
                let t = m.modified().unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                    best = Some((t, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

// ---------------------------------------------------------------- Python

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PythonLaunch {
    pub program: Option<String>,
    pub module: Option<String>,
    pub args: Vec<String>,
    /// The interpreter the command named (`.venv/bin/python`), when it named one.
    pub python: Option<String>,
}

/// Split a command line into words (quotes and backslashes; no expansion).
/// `None` when it uses shell syntax we do not interpret (pipes, `&&`, `$(…)`).
pub fn split_words(cmd: &str) -> Option<Vec<String>> {
    let mut words = vec![];
    let mut cur = String::new();
    let mut has = false;
    let mut chars = cmd.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                has = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    has = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, '|' | '&' | ';' | '<' | '>' | '`' | '(' | ')') => return None,
            (None, '$') if chars.peek() == Some(&'(') => return None,
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if has || !cur.is_empty() {
        words.push(cur);
    }
    Some(words)
}

/// `split_words` for a PowerShell command line (the run shell on Windows): `'…'` (a
/// doubled `''` is a quote), `"…"` without variables, the backtick escape of a character
/// that stays itself (`` `" ``, `` `$ ``, a space), and a leading
/// call operator (`& 'C:\my tools\python.exe' x.py`); `\` is a character like any other.
/// `None` when it uses syntax we do not interpret (`$x`, pipes, `;`, `&&`, `(…)`, `@x`).
pub fn split_words_ps(cmd: &str) -> Option<Vec<String>> {
    let cmd = cmd.trim_start();
    let cmd = match cmd.strip_prefix('&') {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest,
        _ => cmd,
    };
    let mut words = vec![];
    let mut cur = String::new();
    let mut has = false;
    let mut chars = cmd.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') if chars.peek() == Some(&'\'') => {
                chars.next();
                cur.push('\'');
            }
            (Some('"'), '"') if chars.peek() == Some(&'"') => {
                chars.next();
                cur.push('"');
            }
            (Some(q), c) if c == q => quote = None,
            // `` `n ``, `` `t ``, `` `0 ``… are control characters: not interpreted.
            (Some('"'), '`') | (None, '`') => match chars.next()? {
                '0' | 'a' | 'b' | 'e' | 'f' | 'n' | 'r' | 't' | 'u' | 'v' => return None,
                c => cur.push(c),
            },
            (Some('"'), '$') => return None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, '$' | '|' | '&' | ';' | '<' | '>' | '(' | ')' | '{' | '}') => return None,
            (None, '@' | '#') if cur.is_empty() && !has => return None,
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if has || !cur.is_empty() {
        words.push(cur);
    }
    // `--%` passes the rest of the line on as it is.
    (!words.iter().any(|w| w == "--%")).then_some(words)
}

fn is_python(dialect: Dialect, w: &str) -> bool {
    let name = match dialect {
        Dialect::Posix => w.rsplit('/').next().unwrap_or(w).to_string(),
        // `python.exe`, `C:\Python313\python.exe`, the `py` launcher.
        Dialect::PowerShell => {
            let n = w.rsplit(['/', '\\']).next().unwrap_or(w).to_ascii_lowercase();
            n.strip_suffix(".exe").map(str::to_string).unwrap_or(n)
        }
    };
    name == "python"
        || name == "python3"
        || (name.starts_with("python3.") && name[8..].chars().all(|c| c.is_ascii_digit()))
        || (dialect == Dialect::PowerShell && name == "py")
}

/// Console scripts that are Python modules of the same name.
const MODULE_SCRIPTS: &[&str] = &["pytest", "uvicorn", "flask", "gunicorn", "streamlit", "hypercorn", "celery"];

/// A debugpy launch for a run command: `python x.py args`, `python -m pkg args`,
/// `uv run python …`, `poetry run pytest …`, `.venv/bin/python manage.py runserver`.
/// The command is in the run shell's language (`Dialect::HOST`).
pub fn python_from_command(cmd: &str) -> Option<PythonLaunch> {
    python_from_command_in(Dialect::HOST, cmd)
}

/// `python_from_command` for a command line in `dialect`: on Windows also
/// `.venv\Scripts\python.exe app.py` and `py -3 -m app`.
pub fn python_from_command_in(dialect: Dialect, cmd: &str) -> Option<PythonLaunch> {
    let words = match dialect {
        Dialect::Posix => split_words(cmd)?,
        Dialect::PowerShell => split_words_ps(cmd)?,
    };
    let mut i = 0;
    // VAR=value prefixes.
    while i < words.len() && words[i].contains('=') && !words[i].starts_with('-') && words[i].split('=').next().is_some_and(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        i += 1;
    }
    // Runners: `uv run [flags]`, `poetry run`, …
    if i + 1 < words.len() && matches!(words[i].as_str(), "uv" | "poetry" | "pdm" | "pipenv" | "hatch" | "rye") && words[i + 1] == "run" {
        i += 2;
        while i < words.len() && words[i].starts_with('-') {
            // `--with x` / `--env-file f` take a value.
            let takes = matches!(words[i].as_str(), "--with" | "--env-file" | "--python" | "-p" | "--group" | "--extra");
            i += if takes { 2 } else { 1 };
        }
    }
    let first = words.get(i)?;
    let mut out = PythonLaunch::default();
    if is_python(dialect, first) {
        let path = match dialect {
            Dialect::Posix => first.contains('/'),
            Dialect::PowerShell => first.contains(['/', '\\']),
        };
        if path {
            out.python = Some(first.clone());
        }
        i += 1;
        // Interpreter flags before the script (`-u`, `-X dev`).
        while i < words.len() && words[i].starts_with('-') && words[i] != "-m" {
            i += if words[i] == "-X" || words[i] == "-W" { 2 } else { 1 };
        }
        let w = words.get(i)?;
        if w == "-m" {
            out.module = Some(words.get(i + 1)?.clone());
            out.args = words[i + 2..].to_vec();
        } else if w.ends_with(".py") {
            out.program = Some(w.clone());
            out.args = words[i + 1..].to_vec();
        } else {
            return None;
        }
    } else if first.ends_with(".py") && i > 0 {
        // `uv run script.py`
        out.program = Some(first.clone());
        out.args = words[i + 1..].to_vec();
    } else if MODULE_SCRIPTS.contains(&first.as_str()) {
        out.module = Some(first.clone());
        out.args = words[i + 1..].to_vec();
    } else {
        return None;
    }
    let valid = |s: &str| !s.is_empty() && !s.contains('\0');
    if out.program.as_deref().is_some_and(|p| !valid(p)) || out.module.as_deref().is_some_and(|m| !valid(m)) {
        return None;
    }
    Some(out)
}

/// The project's virtualenv interpreter, if it has one (`.venv/bin/python`; on Windows
/// `.venv\Scripts\python.exe`).
pub fn venv_python(root: &Path) -> Option<String> {
    crate::apps::detect::venv_python(root, Dialect::HOST)
}

// ---------------------------------------------------------------- Go

/// Main packages of a Go module: the root and `cmd/*` (`"."`, `"./cmd/api"`).
pub fn go_mains(root: &Path) -> Vec<String> {
    if !root.join("go.mod").is_file() {
        return vec![];
    }
    let is_main = |dir: &Path| -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else { return false };
        rd.flatten().take(200).filter(|e| e.file_name().to_string_lossy().ends_with(".go") && !e.file_name().to_string_lossy().ends_with("_test.go")).take(30).any(|e| {
            use std::io::Read;
            let mut buf = vec![0u8; 4096];
            let Ok(mut f) = std::fs::File::open(e.path()) else { return false };
            let n = f.read(&mut buf).unwrap_or(0);
            String::from_utf8_lossy(&buf[..n]).lines().any(|l| l.trim() == "package main")
        })
    };
    let mut out = vec![];
    if is_main(root) {
        out.push(".".to_string());
    }
    if let Ok(rd) = std::fs::read_dir(root.join("cmd")) {
        let mut v: Vec<String> = rd
            .flatten()
            .take(200)
            .filter(|e| e.path().is_dir() && is_main(&e.path()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| plain_name(n))
            .map(|n| format!("./cmd/{n}"))
            .collect();
        v.sort();
        out.extend(v.into_iter().take(40));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn cargo_workspace_targets_without_running_cargo() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(r, "Cargo.toml", "[workspace]\nmembers = [\"crates/*\", \"tool\"]\nexclude = [\"crates/skip\"]\n[package]\nname = \"app\"\nversion = \"0.1.0\"\n[[bin]]\nname = \"admin\"\npath = \"src/admin.rs\"\n");
        write(r, "src/main.rs", "fn main() {}");
        write(r, "src/admin.rs", "fn main() {}");
        write(r, "src/bin/worker.rs", "fn main() {}");
        write(r, "src/bin/multi/main.rs", "fn main() {}");
        write(r, "tests/api.rs", "#[test] fn t() {}");
        write(r, "examples/demo.rs", "fn main() {}");
        write(r, "crates/core-lib/Cargo.toml", "[package]\nname = \"core-lib\"\nversion = \"0.1.0\"\n");
        write(r, "crates/core-lib/src/lib.rs", "");
        write(r, "crates/skip/Cargo.toml", "[package]\nname = \"skip\"\n");
        write(r, "crates/skip/src/main.rs", "");
        write(r, "tool/Cargo.toml", "[package]\nname = \"tool\"\nautobins = false\n[lib]\ntest = false\n");
        write(r, "tool/src/main.rs", "fn main() {}");
        write(r, "tool/src/lib.rs", "");
        let t = cargo_targets(r, "");
        let names: Vec<String> = t.iter().map(|t| t.config_name()).collect();
        assert_eq!(
            names,
            vec![
                "Cargo: bin admin",
                "Cargo: bin app",
                "Cargo: bin multi",
                "Cargo: bin worker",
                "Cargo: test api",
                "Cargo: example demo",
                "Cargo: unit tests (core-lib)",
            ],
            "{t:#?}"
        );
        let lib = t.iter().find(|t| t.kind == CargoKind::Lib).unwrap();
        assert_eq!((lib.name.as_str(), lib.dir.as_str()), ("core_lib", "crates/core-lib"));
        assert_eq!(lib.build_args(), vec!["test", "-p", "core-lib", "--lib", "--no-run", "--message-format=json-render-diagnostics"]);
        assert!(cargo_targets(&r.join("nowhere"), "").is_empty());
        assert!(cargo_targets(r, "../outside").is_empty(), "manifests outside the project are not read");
        // A Cargo workspace in a folder of the project (a monorepo).
        let nested = cargo_targets(r.parent().unwrap(), r.file_name().unwrap().to_str().unwrap());
        let lib = nested.iter().find(|t| t.kind == CargoKind::Lib).unwrap();
        assert_eq!(lib.workspace, r.file_name().unwrap().to_str().unwrap());
        assert_eq!(lib.dir, format!("{}/crates/core-lib", lib.workspace));
    }

    #[test]
    fn cargo_json_messages_name_the_executable() {
        let t = |package: &str, dir: &str, name: &str, kind| CargoTarget { package: package.into(), workspace: "".into(), dir: dir.into(), name: name.into(), kind };
        let bin = t("app", "", "app", CargoKind::Bin);
        let test = t("app", "", "api", CargoKind::Test);
        let lib = t("core-lib", "crates/core-lib", "core_lib", CargoKind::Lib);
        let out = r#"
{"reason":"compiler-artifact","package_id":"path+file:///w/crates/core-lib#core-lib@0.1.0","manifest_path":"/w/crates/core-lib/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"core_lib","src_path":"/w/crates/core-lib/src/lib.rs","test":true},"profile":{"test":false},"filenames":["/w/target/debug/deps/libcore_lib.rlib"],"executable":null,"fresh":true}
{"reason":"compiler-artifact","package_id":"path+file:///w#app@0.1.0","manifest_path":"/w/Cargo.toml","target":{"kind":["bin"],"name":"app"},"profile":{"test":false},"executable":"/w/target/debug/app","fresh":false}
{"reason":"compiler-artifact","package_id":"path+file:///w#app@0.1.0","target":{"kind":["test"],"name":"api"},"profile":{"test":true},"executable":"/w/target/debug/deps/api-1f2e3d","fresh":false}
{"reason":"compiler-artifact","target":{"kind":["lib"],"name":"core_lib"},"profile":{"test":true},"executable":"/w/target/debug/deps/core_lib-99aa","fresh":false}
not json at all
{"reason":"build-finished","success":true}
"#;
        assert_eq!(cargo_executable(out, &bin).as_deref(), Some("/w/target/debug/app"));
        assert_eq!(cargo_executable(out, &test).as_deref(), Some("/w/target/debug/deps/api-1f2e3d"));
        assert_eq!(cargo_executable(out, &lib).as_deref(), Some("/w/target/debug/deps/core_lib-99aa"));
        assert_eq!(cargo_executable("", &bin), None);
    }

    #[test]
    fn cmake_executables_need_a_build_dir() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(r, "CMakeLists.txt", "project(x)\nadd_executable(app main.cpp)\nadd_executable(Other::alias ALIAS app)\nadd_subdirectory(tools)\n");
        write(r, "tools/CMakeLists.txt", "  ADD_EXECUTABLE( gen gen.c )\nadd_executable(imp IMPORTED)\nadd_executable(${NAME} x.c)\n");
        assert!(cmake_targets(r).is_empty(), "no build dir yet");
        write(r, "cmake-build-debug/CMakeCache.txt", "");
        write(r, "build/CMakeCache.txt", "");
        write(r, "build/CMakeLists.txt", "add_executable(generated x.c)");
        let t = cmake_targets(r);
        assert_eq!(t.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["app", "gen"]);
        assert_eq!(t[0].build_dir, "build");
        // The executable is found after the build.
        let gen_exe = format!("gen{}", std::env::consts::EXE_SUFFIX);
        write(r, &format!("build/tools/{gen_exe}"), "bin");
        write(r, &format!("build/CMakeFiles/{gen_exe}"), "not this one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(r.join("build/tools/gen"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(find_executable(&r.join("build"), "gen"), Some(r.join("build/tools").join(&gen_exe)));
        assert_eq!(find_executable(&r.join("build"), "app"), None);
    }

    #[test]
    fn python_launches_from_run_commands() {
        let p = |c: &str| python_from_command_in(Dialect::Posix, c);
        assert_eq!(p("python3 manage.py runserver 8000").unwrap().program.as_deref(), Some("manage.py"));
        let m = p("uv run --with rich python -u -m app.main --port 8000").unwrap();
        assert_eq!((m.module.as_deref(), m.args.clone()), (Some("app.main"), vec!["--port".to_string(), "8000".to_string()]));
        let v = p("DEBUG=1 .venv/bin/python 'my script.py' --x \"a b\"").unwrap();
        assert_eq!(v.program.as_deref(), Some("my script.py"));
        assert_eq!(v.python.as_deref(), Some(".venv/bin/python"));
        assert_eq!(v.args, vec!["--x", "a b"]);
        assert_eq!(p("poetry run pytest -x tests").unwrap().module.as_deref(), Some("pytest"));
        assert_eq!(p("uv run uvicorn app:app --reload").unwrap().module.as_deref(), Some("uvicorn"));
        assert_eq!(p("uv run tools/gen.py").unwrap().program.as_deref(), Some("tools/gen.py"));
        assert!(p("cargo run").is_none());
        assert!(p("python3 x.py | tee log").is_none(), "shell pipelines are not guessed");
        assert!(p("python3 -c 'print(1)'").is_none());
        assert_eq!(split_words("a 'b c' \"d\\\"e\" f\\ g ''"), Some(vec!["a".into(), "b c".into(), "d\"e".into(), "f g".into(), "".into()]));
        assert!(p("py -3 app.py").is_none() && p("python.exe app.py").is_none(), "Windows names are not Unix ones");
    }

    #[test]
    fn python_launches_from_powershell_commands() {
        let p = |c: &str| python_from_command_in(Dialect::PowerShell, c);
        let v = p(r".venv\Scripts\python.exe -m pytest -x").unwrap();
        assert_eq!((v.python.as_deref(), v.module.as_deref(), v.args.clone()), (Some(r".venv\Scripts\python.exe"), Some("pytest"), vec!["-x".to_string()]));
        let v = p("py -3 manage.py runserver 8000").unwrap();
        assert_eq!((v.python, v.program.as_deref()), (None, Some("manage.py")));
        assert_eq!(p("python app.py").unwrap().program.as_deref(), Some("app.py"));
        let v = p(r"& 'C:\My Tools\Python313\python.exe' 'my script.py' --x 'it''s'").unwrap();
        assert_eq!(v.python.as_deref(), Some(r"C:\My Tools\Python313\python.exe"));
        assert_eq!((v.program.as_deref(), v.args.clone()), (Some("my script.py"), vec!["--x".to_string(), "it's".to_string()]));
        assert_eq!(p("uv run uvicorn app:app --reload").unwrap().module.as_deref(), Some("uvicorn"));
        assert_eq!(p(r"python tools\gen.py").unwrap().program.as_deref(), Some(r"tools\gen.py"));
        for shell_syntax in ["$env:X=1; python a.py", "python a.py | tee log", "python \"$HOME\\a.py\"", "python (Get-Item a.py)", "python a.py && b", "python @args", "python --% a.py"] {
            assert!(p(shell_syntax).is_none(), "{shell_syntax}");
        }
        assert_eq!(split_words_ps(r#"a 'b c' "d`"e" f`` g '' "x""y""#), Some(vec!["a".into(), "b c".into(), "d\"e".into(), "f`".into(), "g".into(), "".into(), "x\"y".into()]));
        assert_eq!(split_words_ps(r"C:\x\y.exe a\b"), Some(vec![r"C:\x\y.exe".into(), r"a\b".into()]));
        // `` `n `` is a line break, `` `t `` a tab: not guessed at.
        for escape in ["python a.py `n", "python \"a`tb.py\"", "python a`0.py"] {
            assert_eq!(split_words_ps(escape), None, "{escape}");
        }
    }

    #[test]
    fn virtualenv_interpreters_follow_the_os() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(venv_python(d.path()), None);
        write(d.path(), ".venv/bin/python", "");
        write(d.path(), "venv/Scripts/python.exe", "");
        assert_eq!(crate::apps::detect::venv_python(d.path(), Dialect::Posix).as_deref(), Some(".venv/bin/python"));
        assert_eq!(crate::apps::detect::venv_python(d.path(), Dialect::PowerShell).as_deref(), Some(r"venv\Scripts\python.exe"));
        assert_eq!(venv_python(d.path()), crate::apps::detect::venv_python(d.path(), Dialect::HOST));
    }

    #[test]
    fn go_main_packages() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        assert!(go_mains(r).is_empty());
        write(r, "go.mod", "module x\n");
        write(r, "lib.go", "package x\n");
        write(r, "cmd/api/main.go", "// comment\npackage main\n");
        write(r, "cmd/util/util.go", "package util\n");
        assert_eq!(go_mains(r), vec!["./cmd/api"]);
    }
}
