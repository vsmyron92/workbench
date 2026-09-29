//! CMake projects → configure / build / test runs and one run per executable.
//!
//! A project is the shallowest `CMakeLists.txt` of its tree (subdirectories added
//! with `add_subdirectory` belong to it). Without presets it builds out of source
//! in `build/` — its own directory, next to (never inside) CLion's
//! `cmake-build-*` profiles:
//! `cmake -S . -B build` → `cmake --build build` → `ctest --test-dir build`.
//! With `CMakePresets.json` / `CMakeUserPresets.json`, the visible presets for
//! this OS become `cmake --preset`, `cmake --build --preset`, `ctest --preset`
//! and `cmake --workflow --preset` runs instead.
//!
//! `add_executable(name …)` targets become `cmake --build … --target name &&
//! <binary dir>/name` runs. On Windows (`super::dialect`) presets are those for
//! `Windows`, and a multi-configuration build dir (Visual Studio, CMake's default
//! there) builds and runs Debug: `cmake --build build --config Debug --target name`,
//! then `.\build\Debug\name.exe`. The generator is the preset's, else the one the
//! build dir's `CMakeCache.txt` records, else `CMAKE_GENERATOR`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, and_then, local_program, scoped, sh, source, text};
use crate::config::project::{Component, RunConfig, RunKind};
use crate::util::os::shell::Dialect;

/// `  3/12 Test  #3: parser_tests .....................   Passed    0.01 sec`
pub const CTEST_RESULT: &str = r"^\s*\d+/\d+ Test\s+#\d+: (?P<name>\S+) \.+\s*(?:\*+)?(?P<status>Passed|Failed)\b";

/// CMake files read per project (the root and its subdirectories).
const MAX_LISTS: usize = 60;
const MAX_EXECUTABLES: usize = 15;
const MAX_PRESETS: usize = 8;

static ADD_EXECUTABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^\s*add_executable\s*\(\s*([A-Za-z0-9_.+-]+)([^)]*)").unwrap());
static TESTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^\s*(?:enable_testing\s*\(|add_test\s*\(|include\s*\(\s*CTest\b|gtest_discover_tests\s*\(|catch_discover_tests\s*\(|doctest_discover_tests\s*\()").unwrap()
});
static RUNTIME_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?im)^\s*set\s*\(\s*CMAKE_RUNTIME_OUTPUT_DIRECTORY\s+"?\$\{(?:CMAKE_BINARY_DIR|PROJECT_BINARY_DIR)\}/?([\w./-]*)"?\s*\)"#).unwrap()
});
static PROJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)^\s*(?:project|cmake_minimum_required)\s*\(").unwrap());
/// The generator a configured build dir was made with, in its `CMakeCache.txt`.
static CACHED_GENERATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^CMAKE_GENERATOR:INTERNAL=([^\r\n]+)").unwrap());

pub fn detect(cx: &mut Ctx, files: &[PathBuf]) {
    let lists: Vec<PathBuf> = files.iter().filter(|f| f.file_name().is_some_and(|n| n == "CMakeLists.txt")).cloned().collect();
    let mut roots: Vec<PathBuf> = vec![];
    for l in &lists {
        let Some(d) = l.parent() else { continue };
        if !roots.iter().any(|r| d.starts_with(r)) {
            roots.push(d.to_path_buf());
        }
    }
    for r in roots.iter().take(4) {
        project(cx, r, &lists);
    }
}

fn project(cx: &mut Ctx, dir: &Path, lists: &[PathBuf]) {
    let top = dir.join("CMakeLists.txt");
    let Some(top_src) = cx.read(&top) else { return };
    if !PROJECT.is_match(&top_src) {
        return; // a fragment, not a project
    }
    let cwd = cx.rel(dir);
    cx.tag("cmake");
    cx.tag("cpp");
    cx.pf.components.push(Component { name: scoped("cmake", &cwd), path: cwd.clone(), kind: "cmake".into(), version: None });

    // Every CMakeLists.txt of this project: executables and tests.
    let mut sources: Vec<(PathBuf, String)> = vec![(top.clone(), top_src.clone())];
    for l in lists.iter().filter(|l| l.starts_with(dir) && **l != top).take(MAX_LISTS) {
        if let Some(t) = cx.read(l) {
            sources.push((l.clone(), t));
        }
    }
    let has_tests = sources.iter().any(|(_, t)| TESTS.is_match(t));
    let runtime_dir = RUNTIME_DIR.captures(&top_src).map(|c| c[1].trim_matches('/').to_string());
    let mut exes: Vec<(String, String, PathBuf)> = vec![]; // (target, dir relative to project, CMakeLists)
    for (l, t) in &sources {
        let rel_dir = l.parent().and_then(|p| p.strip_prefix(dir).ok()).map(crate::util::os::path::to_slash).unwrap_or_default();
        for c in ADD_EXECUTABLE.captures_iter(t) {
            let name = c[1].to_string();
            let args = c[2].to_ascii_uppercase();
            if args.contains("IMPORTED") || args.contains("ALIAS") || name.contains("${") || exes.iter().any(|e| e.0 == name) {
                continue;
            }
            // Test binaries run through ctest.
            let lower = name.to_ascii_lowercase();
            if has_tests && (lower.contains("test") || lower.contains("bench")) {
                continue;
            }
            exes.push((name, rel_dir.clone(), l.clone()));
        }
    }
    exes.truncate(MAX_EXECUTABLES);

    let presets = read_presets(cx, dir);
    if presets.configure.is_empty() {
        plain(cx, dir, &cwd, has_tests, &exes, runtime_dir.as_deref());
    } else {
        with_presets(cx, dir, &cwd, &presets, has_tests, &exes, runtime_dir.as_deref());
    }
}

/// Where an executable lands in a binary dir: `<bin>/<runtime dir or its source dir>/<name>`.
fn exe_path(bin: &str, rel_dir: &str, runtime_dir: Option<&str>, name: &str) -> String {
    let sub = runtime_dir.unwrap_or(rel_dir);
    if sub.is_empty() { format!("{bin}/{name}") } else { format!("{bin}/{sub}/{name}") }
}

/// Whether a build dir made by `generator` keeps one folder per configuration
/// (`Debug/app.exe`): Visual Studio, which is CMake's default on Windows (also for a
/// preset that names no generator), Ninja Multi-Config and Xcode.
fn multi_config(generator: Option<&str>) -> bool {
    generator.is_none_or(|g| g.starts_with("Visual Studio") || g.contains("Multi-Config") || g == "Xcode")
}

/// `multi_config` for the build dir `bin` of the project in `dir`, by the generator CMake
/// uses there: the one its preset names, else the one an existing `CMakeCache.txt`
/// records, else `CMAKE_GENERATOR` (CMake's own fallback, often Ninja on Windows), else
/// CMake's default. Only the Windows forms depend on it: nothing is read on Unix.
fn build_dir_multi_config(cx: &mut Ctx, dir: &Path, bin: &str, preset: Option<&str>) -> bool {
    if super::dialect() == Dialect::Posix {
        return multi_config(preset);
    }
    let cache = dir.join(bin).join("CMakeCache.txt");
    let cached = || cx.read(&cache).and_then(|t| CACHED_GENERATOR.captures(&t).map(|c| c[1].trim().to_string()));
    let generator = preset.map(str::to_string).or_else(cached).or_else(env_generator);
    multi_config(generator.as_deref())
}

/// `CMAKE_GENERATOR` of this machine, on Windows. Not in tests: the Windows forms they
/// check must not depend on the machine that runs them.
fn env_generator() -> Option<String> {
    if cfg!(test) || !cfg!(windows) {
        return None;
    }
    std::env::var("CMAKE_GENERATOR").ok().map(|g| g.trim().to_string()).filter(|g| !g.is_empty())
}

/// Build `name` in the binary dir `bin`, then run it. On Windows a multi-configuration
/// build dir (`multi`) builds and runs its Debug configuration: `cmake --build build
/// --config Debug --target app`, then `.\build\Debug\app.exe`.
fn build_and_run(bin: &str, rel_dir: &str, runtime_dir: Option<&str>, name: &str, multi: bool) -> String {
    let windows = super::dialect() == Dialect::PowerShell;
    let (config, exe) = if windows && multi { (" --config Debug", format!("Debug/{name}")) } else { ("", name.to_string()) };
    and_then(&format!("cmake --build {}{config} --target {}", sh(bin), sh(name)), &local_program(&exe_path(bin, rel_dir, runtime_dir, &exe)))
}

fn plain(cx: &mut Ctx, dir: &Path, cwd: &str, tests: bool, exes: &[(String, String, PathBuf)], runtime_dir: Option<&str>) {
    // `build` is configured by the run below: on Windows with CMake's default generator,
    // Visual Studio, whose configurations are chosen when building (Debug by default).
    let windows = super::dialect() == Dialect::PowerShell;
    let top = &dir.join("CMakeLists.txt");
    let multi = build_dir_multi_config(cx, dir, "build", None);
    let configure = cx.add_run(RunConfig {
        name: scoped("cmake configure", cwd),
        kind: RunKind::Build,
        command: "cmake -S . -B build -DCMAKE_BUILD_TYPE=Debug".into(),
        cwd: cwd.to_string(),
        source: source(cx, top, ""),
        group: Some("build".into()),
        ..Default::default()
    });
    let deps: Vec<String> = configure.into_iter().collect();
    let build = cx.add_run(RunConfig {
        name: scoped("cmake build", cwd),
        kind: RunKind::Build,
        command: if windows { "cmake --build build --config Debug".into() } else { "cmake --build build".into() },
        cwd: cwd.to_string(),
        depends_on: deps.clone(),
        source: source(cx, top, ""),
        group: Some("build".into()),
        ..Default::default()
    });
    if tests {
        cx.add_run(RunConfig {
            name: scoped("ctest", cwd),
            kind: RunKind::Test,
            command: if windows { "ctest --test-dir build -C Debug --output-on-failure".into() } else { "ctest --test-dir build --output-on-failure".into() },
            cwd: cwd.to_string(),
            depends_on: build.into_iter().collect(),
            result_pattern: Some(CTEST_RESULT.into()),
            source: source(cx, top, " (tests)"),
            group: Some("test".into()),
            ..Default::default()
        });
    }
    for (name, rel_dir, list) in exes {
        cx.add_run(RunConfig {
            name: scoped(name, cwd),
            kind: RunKind::Task,
            command: build_and_run("build", rel_dir, runtime_dir, name, multi),
            cwd: cwd.to_string(),
            depends_on: deps.clone(),
            source: source(cx, list, &format!(" add_executable({name})")),
            group: Some("dev".into()),
            ..Default::default()
        });
    }
}

/// Visible presets for this OS, by kind.
#[derive(Debug, Default)]
pub struct Presets {
    pub file: PathBuf,
    /// (name, binary dir relative to the project when known, generator when named)
    pub configure: Vec<(String, Option<String>, Option<String>)>,
    /// (name, its configure preset)
    pub build: Vec<(String, Option<String>)>,
    pub test: Vec<(String, Option<String>)>,
    pub workflow: Vec<String>,
}

fn read_presets(cx: &mut Ctx, dir: &Path) -> Presets {
    let mut out = Presets::default();
    let mut all: BTreeMap<String, serde_json::Value> = BTreeMap::new(); // every configure preset, hidden too
    let mut raw: Vec<(String, serde_json::Value)> = vec![]; // (kind, preset)
    for name in ["CMakePresets.json", "CMakeUserPresets.json"] {
        let p = dir.join(name);
        if !cx.is_file(&p) {
            continue;
        }
        let Some(src) = cx.read(&p) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text::strip_jsonc(&src)) else { continue };
        if out.file.as_os_str().is_empty() {
            out.file = p.clone();
        }
        for kind in ["configurePresets", "buildPresets", "testPresets", "workflowPresets"] {
            for pr in v.get(kind).and_then(|x| x.as_array()).into_iter().flatten() {
                if kind == "configurePresets" {
                    if let Some(n) = pr.get("name").and_then(|n| n.as_str()) {
                        all.insert(n.to_string(), pr.clone());
                    }
                }
                raw.push((kind.to_string(), pr.clone()));
            }
        }
    }
    for (kind, pr) in raw {
        let Some(name) = pr.get("name").and_then(|n| n.as_str()).map(str::to_string) else { continue };
        let hidden = pr.get("hidden").and_then(|h| h.as_bool()).unwrap_or(false);
        if hidden || !condition_holds(&pr, &all) || name.contains(['{', '}']) {
            continue;
        }
        let conf = pr.get("configurePreset").and_then(|c| c.as_str()).map(str::to_string);
        match kind.as_str() {
            "configurePresets" if out.configure.len() < MAX_PRESETS => {
                let bin = binary_dir(&name, &pr, &all, 0);
                let generator = generator(&pr, &all, 0);
                out.configure.push((name, bin, generator));
            }
            "buildPresets" if out.build.len() < MAX_PRESETS => out.build.push((name, conf)),
            "testPresets" if out.test.len() < MAX_PRESETS => out.test.push((name, conf)),
            "workflowPresets" if out.workflow.len() < MAX_PRESETS => out.workflow.push(name),
            _ => {}
        }
    }
    // Build and test presets of a configure preset that is hidden here (another OS) go too.
    let visible: Vec<String> = out.configure.iter().map(|c| c.0.clone()).collect();
    out.build.retain(|(_, c)| c.as_ref().is_none_or(|c| visible.contains(c)));
    out.test.retain(|(_, c)| c.as_ref().is_none_or(|c| visible.contains(c)));
    out
}

/// A preset's `condition` for this machine (Linux; `Windows` where the run shell is
/// PowerShell): `equals`/`notEquals`/`inList` on `${hostSystemName}`; inherited from the
/// configure presets it inherits.
fn condition_holds(pr: &serde_json::Value, all: &BTreeMap<String, serde_json::Value>) -> bool {
    fn check(c: &serde_json::Value) -> bool {
        let host = match super::dialect() {
            Dialect::Posix => "Linux",
            Dialect::PowerShell => "Windows",
        };
        let s = |k: &str| c.get(k).and_then(|x| x.as_str()).unwrap_or("");
        let is_host = |x: &str| x == "${hostSystemName}";
        match s("type") {
            "equals" if is_host(s("lhs")) => s("rhs") == host,
            "equals" if is_host(s("rhs")) => s("lhs") == host,
            "notEquals" if is_host(s("lhs")) => s("rhs") != host,
            "notEquals" if is_host(s("rhs")) => s("lhs") != host,
            "inList" if is_host(s("string")) => c.get("list").and_then(|l| l.as_array()).is_some_and(|l| l.iter().any(|x| x.as_str() == Some(host))),
            "notInList" if is_host(s("string")) => !c.get("list").and_then(|l| l.as_array()).is_some_and(|l| l.iter().any(|x| x.as_str() == Some(host))),
            "anyOf" => c.get("conditions").and_then(|l| l.as_array()).is_none_or(|l| l.iter().any(check)),
            "allOf" => c.get("conditions").and_then(|l| l.as_array()).is_none_or(|l| l.iter().all(check)),
            "not" => c.get("condition").is_none_or(|x| !check(x)),
            _ => true, // conditions on other macros: assume they hold
        }
    }
    if let Some(c) = pr.get("condition") {
        if !check(c) {
            return false;
        }
    }
    // Configure presets inherit conditions.
    let parents: Vec<String> = match pr.get("inherits") {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => vec![],
    };
    parents.iter().filter_map(|p| all.get(p)).take(8).all(|p| p.get("condition").is_none_or(check))
}

/// A configure preset's `binaryDir` with `${sourceDir}`, `${presetName}`,
/// `${sourceDirName}` expanded, relative to the project (inherited if unset).
fn binary_dir(name: &str, pr: &serde_json::Value, all: &BTreeMap<String, serde_json::Value>, depth: usize) -> Option<String> {
    let dir = pr.get("binaryDir").and_then(|b| b.as_str()).map(str::to_string).or_else(|| {
        if depth > 6 {
            return None;
        }
        let parents: Vec<String> = match pr.get("inherits") {
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
            _ => vec![],
        };
        parents.iter().filter_map(|p| all.get(p)).find_map(|p| binary_dir(name, p, all, depth + 1).map(|d| format!("${{sourceDir}}/{d}")))
    })?;
    let d = dir.replace("${presetName}", name);
    let d = d.strip_prefix("${sourceDir}/").or_else(|| d.strip_prefix("${sourceDir}")).unwrap_or(&d).trim_start_matches('/').to_string();
    (!d.is_empty() && !d.contains("${") && !crate::util::os::path::is_absolute_str(&d) && !d.contains("..")).then_some(d)
}

/// A configure preset's `generator` (inherited if unset).
fn generator(pr: &serde_json::Value, all: &BTreeMap<String, serde_json::Value>, depth: usize) -> Option<String> {
    pr.get("generator").and_then(|g| g.as_str()).map(str::to_string).or_else(|| {
        let parents: Vec<&str> = match pr.get("inherits") {
            Some(serde_json::Value::String(s)) => vec![s.as_str()],
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect(),
            _ => vec![],
        };
        (depth <= 6).then(|| parents.iter().filter_map(|p| all.get(*p)).find_map(|p| generator(p, all, depth + 1))).flatten()
    })
}

fn with_presets(cx: &mut Ctx, dir: &Path, cwd: &str, p: &Presets, tests: bool, exes: &[(String, String, PathBuf)], runtime_dir: Option<&str>) {
    let file = if p.file.as_os_str().is_empty() { dir.join("CMakePresets.json") } else { p.file.clone() };
    let mut configure_runs: BTreeMap<String, String> = BTreeMap::new();
    for (name, _, _) in &p.configure {
        if let Some(n) = cx.add_run(RunConfig {
            name: scoped(&format!("cmake configure: {name}"), cwd),
            kind: RunKind::Build,
            command: format!("cmake --preset {}", sh(name)),
            cwd: cwd.to_string(),
            source: source(cx, &file, &format!("#configurePresets.{name}")),
            group: Some("build".into()),
            ..Default::default()
        }) {
            configure_runs.insert(name.clone(), n);
        }
    }
    let mut build_runs: BTreeMap<String, String> = BTreeMap::new(); // configure preset → build run
    for (name, conf) in &p.build {
        let deps: Vec<String> = conf.as_ref().and_then(|c| configure_runs.get(c)).cloned().into_iter().collect();
        if let Some(n) = cx.add_run(RunConfig {
            name: scoped(&format!("cmake build: {name}"), cwd),
            kind: RunKind::Build,
            command: format!("cmake --build --preset {}", sh(name)),
            cwd: cwd.to_string(),
            depends_on: deps,
            source: source(cx, &file, &format!("#buildPresets.{name}")),
            group: Some("build".into()),
            ..Default::default()
        }) {
            if let Some(c) = conf {
                build_runs.entry(c.clone()).or_insert(n);
            }
        }
    }
    if tests {
        for (name, conf) in &p.test {
            let deps: Vec<String> = conf.as_ref().and_then(|c| build_runs.get(c)).cloned().into_iter().collect();
            cx.add_run(RunConfig {
                name: scoped(&format!("ctest: {name}"), cwd),
                kind: RunKind::Test,
                command: format!("ctest --preset {} --output-on-failure", sh(name)),
                cwd: cwd.to_string(),
                depends_on: deps,
                result_pattern: Some(CTEST_RESULT.into()),
                source: source(cx, &file, &format!("#testPresets.{name}")),
                group: Some("test".into()),
                ..Default::default()
            });
        }
    }
    for name in &p.workflow {
        cx.add_run(RunConfig {
            name: scoped(&format!("cmake workflow: {name}"), cwd),
            kind: RunKind::Build,
            command: format!("cmake --workflow --preset {}", sh(name)),
            cwd: cwd.to_string(),
            source: source(cx, &file, &format!("#workflowPresets.{name}")),
            group: Some("build".into()),
            ..Default::default()
        });
    }
    // Executables: built and run in the first configure preset whose binary dir is known.
    let Some((preset, Some(bin), generator)) = p.configure.iter().find(|c| c.1.is_some()) else { return };
    let deps: Vec<String> = configure_runs.get(preset).cloned().into_iter().collect();
    let multi = build_dir_multi_config(cx, dir, bin, generator.as_deref());
    for (name, rel_dir, list) in exes {
        cx.add_run(RunConfig {
            name: scoped(name, cwd),
            kind: RunKind::Task,
            command: build_and_run(bin, rel_dir, runtime_dir, name, multi),
            cwd: cwd.to_string(),
            depends_on: deps.clone(),
            source: source(cx, list, &format!(" add_executable({name})")),
            group: Some("dev".into()),
            ..Default::default()
        });
    }
}
