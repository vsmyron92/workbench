//! Cargo workspaces and packages → test runs and one run per binary. A binary
//! that depends on a web framework is a server; its port and ready line are read
//! from its `main.rs`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, on_path, scoped, sh, source};
use crate::config::project::{Component, Ready, RunConfig, RunKind};

const WEB_FRAMEWORKS: &[&str] = &["axum", "actix-web", "rocket", "warp", "poem", "hyper", "salvo", "tide", "ntex"];

static PORT_ENV: LazyLock<Regex> = LazyLock::new(|| {
    // env::var("PORT") … .unwrap_or(8080)  /  .unwrap_or_else(|| "8080".into())
    Regex::new(r#""[A-Z_]*PORT"\)[\s\S]{0,240}?unwrap_or(?:_else)?\(\s*(?:\|\|\s*)?"?(\d{2,5})"#).unwrap()
});
static PORT_LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(?:0\.0\.0\.0|127\.0\.0\.1|localhost|\[::\]|\[::1\]):(\d{2,5})""#).unwrap());
static PORT_TUPLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"SocketAddr::from\(\(\s*\[\s*\d+\s*,\s*\d+\s*,\s*\d+\s*,\s*\d+\s*\]\s*,\s*(\d{2,5})\s*\)\)").unwrap());
static LISTEN_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:println|eprintln|info|tracing::info|log::info)!\(\s*"([^"{]*?listening on)"#).unwrap()
});

pub fn detect(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let Ok(v) = src.parse::<toml::Table>() else { return };
    let Some(dir) = f.parent() else { return };
    let cwd = cx.rel(dir);
    if let Some(ws) = v.get("workspace").and_then(|w| w.as_table()) {
        cx.tag("rust");
        cx.pf.components.push(Component { name: scoped("rust", &cwd), path: cwd.clone(), kind: "cargo-workspace".into(), version: None });
        add_test_runs(cx, f, &cwd, true);
        let mut members: Vec<PathBuf> = vec![];
        if v.contains_key("package") {
            members.push(dir.to_path_buf());
        }
        for m in ws.get("members").and_then(|m| m.as_array()).into_iter().flatten().filter_map(|m| m.as_str()) {
            members.extend(expand_member(cx, dir, m));
        }
        for m in members {
            let mt = m.join("Cargo.toml");
            let Some(text) = cx.read(&mt) else { continue };
            let Ok(mv) = text.parse::<toml::Table>() else { continue };
            package_runs(cx, &m, &mv, &cwd, true);
        }
    } else if v.contains_key("package") {
        if inside_workspace(cx, dir) {
            return; // handled by its workspace
        }
        cx.tag("rust");
        cx.pf.components.push(Component { name: scoped("rust", &cwd), path: cwd.clone(), kind: "cargo".into(), version: None });
        add_test_runs(cx, f, &cwd, false);
        package_runs(cx, dir, &v, &cwd, false);
    }
}

/// `crates/*` → every subdirectory with a Cargo.toml; other entries as-is.
fn expand_member(cx: &Ctx, ws_dir: &Path, m: &str) -> Vec<PathBuf> {
    if let Some(prefix) = m.strip_suffix("/*") {
        if !crate::util::os::path::stays_inside(prefix) {
            return vec![];
        }
        let Some(rd) = cx.read_dir(&ws_dir.join(prefix)) else { return vec![] };
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| cx.is_file(&p.join("Cargo.toml"))).collect();
        v.sort();
        return v;
    }
    if m.contains('*') || m.contains("..") || !crate::util::os::path::stays_inside(m) {
        return vec![];
    }
    vec![ws_dir.join(m)]
}

fn inside_workspace(cx: &mut Ctx, dir: &Path) -> bool {
    let mut a = dir.parent();
    while let Some(d) = a {
        if !d.starts_with(cx.root) {
            break;
        }
        let manifest = d.join("Cargo.toml");
        if cx.is_file(&manifest) && cx.read(&manifest).is_some_and(|t| t.contains("[workspace]")) {
            return true;
        }
        a = d.parent();
    }
    false
}

fn add_test_runs(cx: &mut Ctx, manifest: &Path, cwd: &str, workspace: bool) {
    let ws = if workspace { " --workspace" } else { "" };
    cx.add_run(RunConfig {
        name: scoped("cargo test", cwd),
        kind: RunKind::Test,
        command: format!("cargo test{ws}"),
        cwd: cwd.to_string(),
        result_pattern: Some(CARGO_TEST_RESULT.into()),
        source: source(cx, manifest, ""),
        group: Some("test".into()),
        ..Default::default()
    });
    if on_path("cargo-nextest") {
        cx.add_run(RunConfig {
            name: scoped("nextest", cwd),
            kind: RunKind::Test,
            command: format!("cargo nextest run{ws}"),
            cwd: cwd.to_string(),
            result_pattern: Some(NEXTEST_RESULT.into()),
            source: source(cx, manifest, " (cargo-nextest on PATH)"),
            group: Some("test".into()),
            ..Default::default()
        });
    }
}

/// `test result: ok. 12 passed; 0 failed; …` — one line per test binary (summed).
pub const CARGO_TEST_RESULT: &str = r"^test result: \w+\. (?P<passed>\d+) passed; (?P<failed>\d+) failed";
/// `Summary [ 1.2s] 34 tests run: 34 passed, 0 skipped` / `…: 30 passed, 4 failed, …`
pub const NEXTEST_RESULT: &str = r"^\s*Summary \[.*\] \d+ tests? run: (?P<passed>\d+) passed(?:, (?P<failed>\d+) failed)?";

/// One run per binary target of the package at `pkg_dir`.
fn package_runs(cx: &mut Ctx, pkg_dir: &Path, v: &toml::Table, cwd: &str, in_workspace: bool) {
    let Some(name) = v.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) else { return };
    let mut bins: Vec<(String, PathBuf)> = vec![];
    let autobins = v.get("package").and_then(|p| p.get("autobins")).and_then(|a| a.as_bool()).unwrap_or(true);
    if autobins && cx.is_file(&pkg_dir.join("src/main.rs")) {
        bins.push((name.to_string(), pkg_dir.join("src/main.rs")));
    }
    for b in v.get("bin").and_then(|b| b.as_array()).into_iter().flatten() {
        let Some(bn) = b.get("name").and_then(|n| n.as_str()) else { continue };
        let path = b
            .get("path")
            .and_then(|p| p.as_str())
            .filter(|p| crate::util::os::path::stays_inside(p))
            .map(|p| pkg_dir.join(p))
            .unwrap_or_else(|| pkg_dir.join(format!("src/bin/{bn}.rs")));
        if !bins.iter().any(|(n, _)| n == bn) {
            bins.push((bn.to_string(), path));
        }
    }
    if autobins {
        if let Some(rd) = cx.read_dir(&pkg_dir.join("src/bin")) {
            let mut extra: Vec<(String, PathBuf)> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "rs"))
                .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
                .collect();
            extra.sort();
            for (bn, p) in extra {
                if !bins.iter().any(|(n, _)| *n == bn) {
                    bins.push((bn, p));
                }
            }
        }
    }
    if bins.is_empty() {
        return;
    }
    let deps = v.get("dependencies").and_then(|d| d.as_table()).cloned().unwrap_or_default();
    let is_server = WEB_FRAMEWORKS.iter().any(|k| deps.contains_key(*k));
    let multi = bins.len() > 1;
    let manifest = pkg_dir.join("Cargo.toml");
    for (bin, main) in bins {
        let src = cx.read(&main).unwrap_or_default();
        let port = if is_server { detect_port(&src) } else { None };
        let ready_log = LISTEN_LINE.captures(&src).map(|c| regex::escape(c[1].trim()));
        let mut cmd = String::from("cargo run");
        if in_workspace {
            cmd.push_str(&format!(" -p {}", sh(&name)));
        }
        if multi {
            cmd.push_str(&format!(" --bin {}", sh(&bin)));
        }
        cx.add_run(RunConfig {
            name: bin.clone(),
            kind: if is_server { RunKind::Server } else { RunKind::Task },
            command: cmd,
            cwd: cwd.to_string(),
            port,
            free_port: is_server && port.is_some(),
            // Cold builds of a big workspace take minutes before the server logs anything.
            ready: is_server.then(|| Ready { log: ready_log.or_else(|| Some("(?i)listening on".into())), http: None, timeout_s: 900 }),
            source: source(cx, &manifest, if multi { "#bin" } else { "" }),
            group: Some("dev".into()),
            ..Default::default()
        });
    }
}

/// The port a Rust server listens on, from its source.
pub fn detect_port(src: &str) -> Option<u16> {
    [&*PORT_ENV, &*PORT_TUPLE, &*PORT_LITERAL]
        .iter()
        .find_map(|re| re.captures(src).and_then(|c| c[1].parse::<u16>().ok()))
        .filter(|p| *p > 0)
}
