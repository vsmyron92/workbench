//! Go modules (`go.mod`) → `go test ./...`, `go build ./...`, `go vet ./...` and
//! one `go run` per main package (the module root, `cmd/<name>/`, or a first-level
//! directory). A main package that serves HTTP is a server; its port comes from
//! `ListenAndServe(":8080")`, `Addr: ":8080"`, a `PORT` fallback or an `-addr`
//! flag default. `.air.toml` adds an `air` live-reload run; a golangci-lint config
//! adds `golangci-lint run`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, is_sample_rel, scoped, sh, source};
use crate::config::project::{Component, RunConfig, RunKind};

/// `ok  	example.com/x/pkg	0.012s` / `FAIL	example.com/x/pkg	0.3s` — one item per package.
pub const GO_TEST_RESULT: &str = r"^(?P<status>ok|FAIL)\s+(?P<name>\S+)\s+(?:\(cached\)|[\d.]+s)";

/// Main packages offered per module.
const MAX_MAINS: usize = 15;
/// Go files read per main package.
const MAX_FILES_PER_MAIN: usize = 8;

static MODULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^module\s+(\S+)").unwrap());
static GO_VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^go\s+(\d+\.\d+(?:\.\d+)?)").unwrap());
static PACKAGE_MAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^package\s+main\b").unwrap());
static FUNC_MAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^func\s+main\s*\(\s*\)").unwrap());
static SERVES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"ListenAndServe|http\.Server\s*\{|gin\.(?:Default|New)\(|echo\.New\(|fiber\.New\(|chi\.NewRouter\(|mux\.NewRouter\(|grpc\.NewServer\(|net\.Listen\(")
        .unwrap()
});
static PORTS: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    [
        // os.Getenv("PORT") with a fallback: `if port == "" { port = "8080" }` / cmp.Or(os.Getenv("PORT"), "8080")
        Regex::new(r#"Getenv\("[A-Z_]*PORT"\)[\s\S]{0,160}?"(?::)?(\d{2,5})""#).unwrap(),
        // http.ListenAndServe(":8080", …) / r.Run(":8080") / app.Listen(":3000") / e.Start(":1323")
        Regex::new(r#"(?:ListenAndServe(?:TLS)?|\.Run|\.Listen|\.Start|net\.Listen)\(\s*(?:"tcp",\s*)?"(?:[\w.\-]*|\[::\])?:(\d{2,5})""#).unwrap(),
        // http.Server{Addr: ":8080"}
        Regex::new(r#"Addr:\s*"(?:[\w.\-]*)?:(\d{2,5})""#).unwrap(),
        // flag.String("addr", ":8080", …) / flag.Int("port", 8080, …)
        Regex::new(r#"flag\.(?:String|Int)(?:Var)?\((?:[^,]*,\s*)?"(?:addr|port|listen|http|http-addr|http_addr|listen-addr)"\s*,\s*"?(?:[\w.\-]*:)?(\d{2,5})"?"#)
            .unwrap(),
    ]
});
static AIR_BUILD_TARGET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^\s*cmd\s*=\s*"[^"]*go build[^"]*?\s(\./[\w./-]+|\.)\s*""#).unwrap());

pub fn detect(cx: &mut Ctx, gomod: &Path) {
    let Some(src) = cx.read(gomod) else { return };
    let Some(module) = MODULE.captures(&src).map(|c| c[1].to_string()) else { return };
    let Some(dir) = gomod.parent() else { return };
    let cwd = cx.rel(dir);
    cx.tag("go");
    let version = GO_VERSION.captures(&src).map(|c| c[1].to_string());
    cx.pf.components.push(Component { name: scoped("go", &cwd), path: cwd.clone(), kind: "go".into(), version });

    for (name, kind, command, result, group) in [
        ("go test", RunKind::Test, "go test ./...", Some(GO_TEST_RESULT), "test"),
        ("go build", RunKind::Build, "go build ./...", None, "build"),
        ("go vet", RunKind::Task, "go vet ./...", None, "tasks"),
    ] {
        cx.add_run(RunConfig {
            name: scoped(name, &cwd),
            kind,
            command: command.into(),
            cwd: cwd.clone(),
            result_pattern: result.map(str::to_string),
            source: source(cx, gomod, ""),
            group: Some(group.into()),
            ..Default::default()
        });
    }

    // Main packages of this module (not of a nested module, not in testdata/examples).
    let nested: Vec<PathBuf> = cx
        .files
        .iter()
        .filter(|f| f.file_name().is_some_and(|n| n == "go.mod") && f.parent() != Some(dir) && f.starts_with(dir))
        .filter_map(|f| f.parent().map(Path::to_path_buf))
        .collect();
    let mut main_dirs: Vec<PathBuf> = vec![];
    for f in cx.files.iter().filter(|f| f.starts_with(dir) && f.extension().is_some_and(|e| e == "go")) {
        let Some(d) = f.parent() else { continue };
        let Ok(rel) = d.strip_prefix(dir) else { continue };
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        // The module root, `cmd/<name>`, `cmd/<group>/<name>` or a first-level directory.
        let conventional = parts.is_empty() || parts[0] == "cmd" && (2..=3).contains(&parts.len()) || parts.len() == 1;
        // `cmd/example` is a command; a top-level `examples/` is not the product.
        let sample = parts.first() != Some(&"cmd") && is_sample_rel(&format!("{}/main.go", parts.join("/")));
        let internal = parts.iter().any(|p| matches!(*p, "internal" | "pkg" | "vendor" | "testdata"));
        if conventional && !sample && !internal && !nested.iter().any(|n| d.starts_with(n)) && !main_dirs.iter().any(|x| x == d) {
            main_dirs.push(d.to_path_buf());
        }
    }
    let base_name = module.rsplit('/').find(|s| !(s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit()) && s.len() > 1)).unwrap_or(&module).to_string();
    let mut mains: Vec<(PathBuf, String, Option<u16>, bool)> = vec![]; // (dir, name, port, server)
    for d in main_dirs {
        if mains.len() >= MAX_MAINS {
            break;
        }
        let go_files: Vec<PathBuf> = cx
            .files
            .iter()
            .filter(|f| {
                f.parent() == Some(d.as_path())
                    && f.extension().is_some_and(|e| e == "go")
                    && !f.file_name().is_some_and(|n| n.to_string_lossy().ends_with("_test.go"))
            })
            .take(MAX_FILES_PER_MAIN)
            .cloned()
            .collect();
        let mut text = String::new();
        for f in &go_files {
            text.push_str(&cx.read(f).unwrap_or_default());
            text.push('\n');
        }
        if !PACKAGE_MAIN.is_match(&text) || !FUNC_MAIN.is_match(&text) {
            continue;
        }
        let server = SERVES.is_match(&text);
        let port = if server { detect_port(&text) } else { None };
        let name = if d == dir { base_name.clone() } else { d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() };
        mains.push((d, name, port, server));
    }
    for (d, name, port, server) in &mains {
        let target = if d == dir { ".".to_string() } else { format!("./{}", d.strip_prefix(dir).map(crate::util::os::path::to_slash).unwrap_or_default()) };
        let main_file = d.join("main.go");
        let src_file = if cx.is_file(&main_file) { main_file } else { gomod.to_path_buf() };
        cx.add_run(RunConfig {
            name: scoped(name, &cwd),
            kind: if *server { RunKind::Server } else { RunKind::Task },
            command: format!("go run {}", sh(&target)),
            cwd: cwd.clone(),
            port: *port,
            preview: port.map(|p| format!("http://localhost:{p}/")),
            source: super::source(cx, &src_file, ""),
            group: Some("dev".into()),
            ..Default::default()
        });
    }

    // air: live reload of the package its build command names (the root by default).
    let air = [dir.join(".air.toml"), dir.join("air.toml")].into_iter().find(|p| cx.is_file(p));
    if let Some(air) = air {
        let cfg = cx.read(&air).unwrap_or_default();
        let target = AIR_BUILD_TARGET.captures(&cfg).map(|c| c[1].trim_end_matches('/').to_string()).unwrap_or_else(|| ".".into());
        let target_dir = if target == "." { dir.to_path_buf() } else { dir.join(target.trim_start_matches("./")) };
        let port = mains.iter().find(|m| m.0 == target_dir).and_then(|m| m.2);
        let tool = src.lines().any(|l| l.contains("air-verse/air") || l.contains("cosmtrek/air"));
        let command = if tool && src.lines().any(|l| l.trim_start().starts_with("tool")) { "go tool air" } else { "air" };
        cx.add_run(RunConfig {
            name: scoped("air", &cwd),
            kind: RunKind::Server,
            command: command.into(),
            cwd: cwd.clone(),
            port,
            preview: port.map(|p| format!("http://localhost:{p}/")),
            source: source(cx, &air, ""),
            group: Some("dev".into()),
            ..Default::default()
        });
    }
    let lint_cfg = [".golangci.yml", ".golangci.yaml", ".golangci.toml", ".golangci.json"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| cx.is_file(p));
    if let Some(cfg) = lint_cfg {
        cx.add_run(RunConfig {
            name: scoped("golangci-lint", &cwd),
            kind: RunKind::Task,
            command: "golangci-lint run".into(),
            cwd,
            source: source(cx, &cfg, ""),
            group: Some("tasks".into()),
            ..Default::default()
        });
    }
}

/// The port a Go server listens on, from its source.
pub fn detect_port(src: &str) -> Option<u16> {
    PORTS.iter().find_map(|re| re.captures(src).and_then(|c| c[1].parse::<u16>().ok())).filter(|p| *p >= 80)
}
