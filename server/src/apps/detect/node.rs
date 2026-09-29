//! `package.json` scripts → runs. Dev servers (vite, next, astro…) get a port, a
//! ready line and a preview URL; a vite proxy to a loopback port becomes a
//! `port:N` dependency; test / build scripts get their kinds. A plain Node server
//! (`node server.js`, `tsx watch src/index.ts`) gets the port its entry file
//! listens on. Monorepo roots add turbo (`turbo.json`) and nx (`nx.json`) tasks;
//! `deno.json` tasks become `deno task` runs.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, TaskGraph, command_words, deploy_or, group_of, kind_from_task_name, npm_graph, scoped, sh, source, text};
use crate::config::project::{Component, Ready, RunConfig, RunKind};

/// npm lifecycle scripts: never offered as runs.
const LIFECYCLE: &[&str] = &[
    "preinstall", "install", "postinstall", "prepare", "prepublish", "prepublishOnly", "prepack", "postpack",
    "publish", "postpublish", "preversion", "version", "postversion", "dependencies", "preuninstall", "uninstall",
    "postuninstall",
];

/// The ready line of vite-like dev servers (after ANSI stripping): `➜  Local:   http://localhost:5173/`.
pub const VITE_READY: &str = r"Local:\s+(https?://\S+)";
/// Any loopback URL a dev server prints once it listens (Gatsby, Docusaurus, Storybook…).
pub const LOCAL_URL: &str = r"(https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\]):\d{2,5}\S*)";

/// The port a Node server's entry file listens on.
static NODE_PORT: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        // process.env.PORT || 3000  /  Number(process.env.PORT ?? "8080")  /  Deno.env.get("PORT") ?? 8000
        Regex::new(r#"(?:process\.env\.PORT|process\.env\[['"]PORT['"]\]|env\.get\(\s*['"]PORT['"]\s*\))\)?\s*(?:\|\||\?\?)\s*['"]?(\d{2,5})\b"#).unwrap(),
        // app.listen(3000)  /  server.listen(8080, …)
        Regex::new(r"\.listen\(\s*(\d{2,5})\b").unwrap(),
        // const PORT = 3000  /  port: 8000
        Regex::new(r#"(?i)\bport\s*[:=]\s*['"]?(\d{2,5})\b"#).unwrap(),
    ]
});
/// Vitest's summary: ` Tests  2 failed | 40 passed (42)`.
pub const VITEST_RESULT: &str = r"^\s*Tests\s+(?:(?P<failed>\d+) failed\s*\|\s*)?(?:\d+ skipped\s*\|\s*)?(?P<passed>\d+) passed";

static PORT_FLAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:--port[= ]|-p\s+)(\d{2,5})\b").unwrap());
static CFG_PORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bport\s*:\s*[^,\n}]*?(\d{2,5})").unwrap());
static LOOPBACK_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):(\d{2,5})").unwrap());
static SERVER_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bserver\s*:\s*\{").unwrap());
static PREVIEW_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bpreview\s*:\s*\{").unwrap());

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pm {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Pm {
    /// The command that runs `script` (shell-quoted: script names come from the repository).
    pub fn run(self, script: &str) -> String {
        let script = sh(script);
        match self {
            Pm::Npm if script == "test" || script == "start" => format!("npm {script}"),
            Pm::Npm => format!("npm run {script}"),
            Pm::Pnpm => format!("pnpm run {script}"),
            Pm::Yarn => format!("yarn run {script}"),
            Pm::Bun => format!("bun run {script}"),
        }
    }

    /// Run a locally installed tool: `npx turbo run build`, `pnpm exec nx …`.
    pub fn exec(self, bin: &str, args: &str) -> String {
        match self {
            Pm::Npm => format!("npx {bin} {args}"),
            Pm::Pnpm => format!("pnpm exec {bin} {args}"),
            Pm::Yarn => format!("yarn {bin} {args}"),
            Pm::Bun => format!("bunx {bin} {args}"),
        }
    }
}

fn package_manager(cx: &Ctx, dir: &Path, pkg: &serde_json::Value) -> Pm {
    if let Some(pm) = pkg.get("packageManager").and_then(|p| p.as_str()) {
        for (prefix, v) in [("pnpm", Pm::Pnpm), ("yarn", Pm::Yarn), ("bun", Pm::Bun), ("npm", Pm::Npm)] {
            if pm.starts_with(prefix) {
                return v;
            }
        }
    }
    // Lockfile in the package dir or an ancestor (workspaces) up to the project root.
    let mut d = Some(dir);
    while let Some(x) = d {
        if cx.is_file(&x.join("pnpm-lock.yaml")) {
            return Pm::Pnpm;
        }
        if cx.is_file(&x.join("yarn.lock")) {
            return Pm::Yarn;
        }
        if cx.is_file(&x.join("bun.lockb")) || cx.is_file(&x.join("bun.lock")) {
            return Pm::Bun;
        }
        if cx.is_file(&x.join("package-lock.json")) || x == cx.root {
            break;
        }
        d = x.parent();
    }
    Pm::Npm
}

/// What a dev-server config file tells us.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ViteInfo {
    pub server_port: Option<u16>,
    pub preview_port: Option<u16>,
    /// Loopback port the dev server proxies API calls to.
    pub proxy_port: Option<u16>,
}

pub fn parse_vite_config(src: &str) -> ViteInfo {
    let block_port = |re: &Regex| -> Option<u16> {
        let m = re.find(src)?;
        let (a, b) = text::block_at(src, m.start())?;
        let body = &src[a..b];
        // Only the block's own `port:`, not one inside a nested `proxy: {…}`.
        let own = match body.find("proxy") {
            Some(i) => &body[..i],
            None => body,
        };
        CFG_PORT.captures(own).and_then(|c| c[1].parse().ok())
    };
    let server_port = block_port(&SERVER_BLOCK);
    let preview_port = block_port(&PREVIEW_BLOCK);
    let proxy_port = if src.contains("proxy") {
        LOOPBACK_URL
            .captures_iter(src)
            .filter_map(|c| c[1].parse::<u16>().ok())
            .find(|p| Some(*p) != server_port)
    } else {
        None
    };
    ViteInfo { server_port, preview_port, proxy_port }
}

/// How a script behaves, from its name and command line.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptKind {
    pub kind: RunKind,
    pub port: Option<u16>,
    pub ready: Option<String>,
    pub result_pattern: Option<String>,
    /// A dev server whose API calls go to a proxy target.
    pub dev_server: bool,
}

pub fn classify(name: &str, cmd: &str, vite: &ViteInfo) -> ScriptKind {
    let tokens: Vec<&str> = cmd
        .split_whitespace()
        .skip_while(|t| (t.contains('=') && !t.starts_with('-')) || matches!(*t, "cross-env" | "dotenv" | "--"))
        .collect();
    let tool = tokens.first().copied().unwrap_or("");
    let sub = tokens.get(1).copied().unwrap_or("");
    let flag_port = PORT_FLAG.captures(cmd).and_then(|c| c[1].parse::<u16>().ok());
    let server = |port: Option<u16>, ready: Option<&str>, dev: bool| ScriptKind {
        kind: RunKind::Server,
        port,
        ready: ready.map(str::to_string),
        result_pattern: None,
        dev_server: dev,
    };
    let lname = name.to_ascii_lowercase();
    match (tool, sub) {
        ("vitest", _) => ScriptKind {
            kind: RunKind::Test,
            port: None,
            ready: None,
            result_pattern: Some(VITEST_RESULT.into()),
            dev_server: false,
        },
        ("vite", "build") => plain(RunKind::Build),
        ("vite", "preview") => server(flag_port.or(vite.preview_port).or(Some(4173)), Some(VITE_READY), false),
        ("vite", _) if !sub.starts_with("optimize") => {
            server(flag_port.or(vite.server_port).or(Some(5173)), Some(VITE_READY), true)
        }
        ("next", "dev") => server(flag_port.or(Some(3000)), Some(r"(?:Local:\s+(https?://\S+)|Ready in|started server on)"), true),
        ("next", "start") => server(flag_port.or(Some(3000)), Some(r"(?:Local:\s+(https?://\S+)|Ready in|started server on)"), false),
        ("next", "build") => plain(RunKind::Build),
        ("astro", "dev") => server(flag_port.or(Some(4321)), Some(VITE_READY), true),
        ("nuxt" | "nuxi", "dev") => server(flag_port.or(Some(3000)), Some(VITE_READY), true),
        ("ng", "serve") => server(flag_port.or(Some(4200)), Some(r"(?:Local:\s+(https?://\S+)|localhost:\d+)"), true),
        ("react-scripts", "start") => server(flag_port.or(Some(3000)), Some(r"(?:Local:\s+(https?://\S+)|compiled)"), true),
        ("webpack", "serve") | ("webpack-dev-server", _) => server(flag_port.or(Some(8080)), None, true),
        ("astro", "preview") => server(flag_port.or(Some(4321)), Some(VITE_READY), false),
        ("nuxt" | "nuxi", "preview") => server(flag_port.or(Some(3000)), Some(VITE_READY), false),
        ("svelte-kit", "dev") => server(flag_port.or(Some(5173)), Some(VITE_READY), true),
        ("remix", "vite:dev") => server(flag_port.or(Some(5173)), Some(VITE_READY), true),
        ("remix", "dev") | ("vinxi", "dev") => server(flag_port.or(Some(3000)), Some(LOCAL_URL), true),
        ("gatsby", "develop") => server(flag_port.or(Some(8000)), Some(LOCAL_URL), true),
        ("vue-cli-service", "serve") => server(flag_port.or(Some(8080)), Some(LOCAL_URL), true),
        ("docusaurus", "start") => server(flag_port.or(Some(3000)), Some(LOCAL_URL), false),
        ("storybook", "dev") | ("start-storybook", _) => server(flag_port.or(Some(6006)), Some(LOCAL_URL), false),
        ("parcel", s) if !matches!(s, "build") => server(flag_port.or(Some(1234)), Some(r"Server running at (https?://\S+)"), true),
        ("eleventy" | "@11ty/eleventy", _) if cmd.contains("--serve") => {
            server(flag_port.or(Some(8080)), Some(r"Server at (https?://\S+)"), false)
        }
        ("wrangler", "dev") => server(flag_port.or(Some(8787)), Some(r"Ready on (https?://\S+)"), false),
        ("netlify", "dev") => server(flag_port.or(Some(8888)), Some(LOCAL_URL), false),
        ("http-server" | "live-server", _) => server(flag_port.or(Some(8080)), Some(LOCAL_URL), false),
        // Vercel's static file server (`serve -s build`), not a script named `serve`.
        ("serve", _) => server(flag_port.or(Some(3000)), Some(LOCAL_URL), false),
        ("json-server", _) => server(flag_port.or(Some(3000)), Some(LOCAL_URL), false),
        // Restart-on-change runners of a Node server (its port comes from the entry file).
        ("nodemon" | "ts-node-dev" | "node-dev", _) | ("tsx", "watch") if !lname.contains("test") => server(flag_port, None, false),
        ("node" | "bun", _) if tokens.contains(&"--watch") && !lname.contains("test") => server(flag_port, None, false),
        ("jest" | "mocha" | "ava" | "playwright" | "cypress", _) => plain(RunKind::Test),
        _ if lname.contains("test") || cmd.contains("node --test") => plain(RunKind::Test),
        _ if lname.contains("build") || matches!(tool, "tsc" | "rollup" | "esbuild") => plain(RunKind::Build),
        _ if matches!(lname.as_str(), "dev" | "start" | "serve") => server(flag_port, None, lname == "dev"),
        _ => plain(RunKind::Task),
    }
}

fn plain(kind: RunKind) -> ScriptKind {
    ScriptKind { kind, port: None, ready: None, result_pattern: None, dev_server: false }
}

pub fn detect(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&src) else { return };
    let Some(dir) = f.parent() else { return };
    let cwd = cx.rel(dir);
    let pm = package_manager(cx, dir, &v);
    let scripts = v.get("scripts").and_then(|s| s.as_object());
    if scripts.is_none_or(|s| s.is_empty()) && v.get("dependencies").is_none() && v.get("devDependencies").is_none() {
        return;
    }
    cx.tag("node");
    cx.pf.components.push(Component { name: scoped("node", &cwd), path: cwd.clone(), kind: "npm".into(), version: None });
    let Some(scripts) = scripts else { return };
    let vite_src = ["vite.config.ts", "vite.config.js", "vite.config.mts", "vite.config.mjs"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| cx.is_file(p))
        .and_then(|p| cx.read(&p))
        .unwrap_or_default();
    let vite = parse_vite_config(&vite_src);
    // What each script runs: its hooks (`npm run build` runs `postbuild`) and the
    // scripts it calls (`npm run build && npm run upload`).
    let graph = npm_graph(scripts);
    for (k, cmd) in scripts {
        let Some(cmd) = cmd.as_str() else { continue };
        if LIFECYCLE.contains(&k.as_str()) || is_hook(k, scripts) {
            continue;
        }
        let mut sk = classify(k, cmd, &vite);
        if sk.kind == RunKind::Server && sk.port.is_none() {
            sk.port = entry_port(cx, dir, cmd, &v);
        }
        let depends_on: Vec<String> = match (sk.dev_server, vite.proxy_port) {
            (true, Some(p)) if Some(p) != sk.port => vec![format!("port:{p}")],
            _ => vec![],
        };
        // A script that ships something (`rsync dist/ host:`, `vercel --prod`), itself
        // or through a hook or a script it runs, is a deploy whatever its name: it asks
        // first and agents cannot start it.
        let group = deploy_or(sk.kind, graph.reaches_out(k));
        cx.add_run(RunConfig {
            name: scoped(k, &cwd),
            kind: sk.kind,
            command: pm.run(k),
            cwd: cwd.clone(),
            port: sk.port,
            ready: sk.ready.map(|log| Ready { log: Some(log), http: None, timeout_s: 120 }),
            depends_on,
            preview: sk.port.filter(|_| sk.kind == RunKind::Server).map(|p| format!("http://localhost:{p}/")),
            result_pattern: sk.result_pattern,
            source: source(cx, f, &format!("#scripts.{k}")),
            group: Some(group.into()),
            ..Default::default()
        });
    }
    monorepo_tasks(cx, dir, &cwd, pm, scripts);
}

/// The port a plain Node server listens on: the entry file named on the command
/// line (`node server.js`, `tsx watch src/index.ts`, `nodemon`) or `main`.
fn entry_port(cx: &mut Ctx, dir: &Path, cmd: &str, pkg: &serde_json::Value) -> Option<u16> {
    const RUNNERS: &[&str] = &["node", "nodemon", "tsx", "ts-node", "ts-node-dev", "bun", "deno", "node-dev"];
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if !tokens.first().is_some_and(|t| RUNNERS.contains(t)) {
        return None;
    }
    let is_src = |t: &&str| [".js", ".mjs", ".cjs", ".ts", ".mts", ".cts"].iter().any(|x| t.ends_with(x)) && !t.starts_with('-');
    let entry = tokens
        .iter()
        .skip(1)
        .find(|t| is_src(t))
        .map(|t| t.to_string())
        .or_else(|| pkg.get("main").and_then(|m| m.as_str()).map(str::to_string))?;
    if entry.contains("..") || crate::util::os::path::is_absolute_str(&entry) || !crate::util::os::path::stays_inside(&entry) {
        return None;
    }
    let src = cx.read(&dir.join(entry.trim_start_matches("./")))?;
    NODE_PORT.iter().find_map(|re| re.captures(&src).and_then(|c| c[1].parse::<u16>().ok())).filter(|p| *p >= 80)
}

/// Monorepo roots: `turbo.json` tasks and `nx.json` target defaults that no root
/// script already runs → `npx turbo run <task>` / `npx nx run-many -t <target>`.
fn monorepo_tasks(cx: &mut Ctx, dir: &Path, cwd: &str, pm: Pm, scripts: &serde_json::Map<String, serde_json::Value>) {
    let already = |task: &str| {
        scripts.iter().any(|(k, v)| k == task || v.as_str().is_some_and(|c| c.contains(&format!(" {task}")) && (c.contains("turbo") || c.contains("nx "))))
    };
    for (file, tool) in [("turbo.json", "turbo"), ("nx.json", "nx")] {
        let path = dir.join(file);
        if !cx.is_file(&path) {
            continue;
        }
        let Some(src) = cx.read(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text::strip_jsonc(&src)) else { continue };
        cx.tag(tool);
        let section = if tool == "turbo" { v.get("tasks").or_else(|| v.get("pipeline")) } else { v.get("targetDefaults") };
        let Some(tasks) = section.and_then(|t| t.as_object()) else { continue };
        for (task, def) in tasks.iter().take(40) {
            // `web#build` (one package), `//#lint` (root task), `@nx/jest:jest` (an executor).
            if task.contains(['#', ':', '/', '@']) || already(task) {
                continue;
            }
            let persistent = def.get("persistent").and_then(|p| p.as_bool()).unwrap_or(false);
            let kind = if persistent { RunKind::Server } else { kind_from_task_name(task) };
            let args = if tool == "turbo" { format!("run {}", sh(task)) } else { format!("run-many -t {}", sh(task)) };
            cx.add_run(RunConfig {
                name: scoped(&format!("{tool} {task}"), cwd),
                kind,
                command: pm.exec(tool, &args),
                cwd: cwd.to_string(),
                source: source(cx, &path, &format!("#{task}")),
                group: Some(group_of(kind).into()),
                ..Default::default()
            });
        }
    }
}

/// `deno.json` / `deno.jsonc` tasks → `deno task <name>`; `deno test -A` when the
/// project has test files but no `test` task.
pub fn detect_deno(cx: &mut Ctx, f: &Path) {
    let Some(src) = cx.read(f) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text::strip_jsonc(&src)) else { return };
    let Some(dir) = f.parent() else { return };
    let cwd = cx.rel(dir);
    cx.tag("deno");
    cx.pf.components.push(Component { name: scoped("deno", &cwd), path: cwd.clone(), kind: "deno".into(), version: None });
    let fresh = v.get("imports").and_then(|i| i.as_object()).is_some_and(|i| i.keys().any(|k| k.contains("fresh")))
        || cx.is_file(&dir.join("fresh.gen.ts"));
    let tasks = v.get("tasks").and_then(|t| t.as_object()).cloned().unwrap_or_default();
    // What each task runs: `deno task x` in its command and its `dependencies`.
    let mut graph = TaskGraph::default();
    for (k, t) in &tasks {
        let cmd = t.as_str().or_else(|| t.get("command").and_then(|c| c.as_str())).unwrap_or_default();
        let mut refs: Vec<String> =
            t.get("dependencies").and_then(|d| d.as_array()).into_iter().flatten().filter_map(|d| d.as_str().map(str::to_string)).collect();
        for words in command_words(cmd) {
            if let Some(i) = words.windows(2).position(|w| w[0] == "deno" && w[1] == "task") {
                refs.extend(words.get(i + 2).cloned());
            }
        }
        graph.add(k, cmd, refs);
    }
    for (k, t) in tasks.iter().take(40) {
        // `"dev": "deno run -A --watch main.ts"` or `{"command": "…", "description": "…"}`.
        let Some(cmd) = t.as_str().or_else(|| t.get("command").and_then(|c| c.as_str())) else { continue };
        let mut sk = classify(k, cmd, &ViteInfo::default());
        if sk.kind == RunKind::Task {
            sk.kind = if cmd.contains("deno serve") { RunKind::Server } else { kind_from_task_name(k) };
        }
        if sk.kind == RunKind::Server && sk.port.is_none() {
            sk.port = super::port_in(cmd)
                .or_else(|| entry_port(cx, dir, cmd, &serde_json::Value::Null))
                .or_else(|| (cmd.contains("deno serve") || fresh).then_some(8000));
        }
        let group = deploy_or(sk.kind, graph.reaches_out(k));
        cx.add_run(RunConfig {
            name: scoped(k, &cwd),
            kind: sk.kind,
            command: format!("deno task {}", sh(k)),
            cwd: cwd.clone(),
            port: sk.port,
            ready: sk.ready.map(|log| Ready { log: Some(log), http: None, timeout_s: 120 }),
            preview: sk.port.filter(|_| sk.kind == RunKind::Server).map(|p| format!("http://localhost:{p}/")),
            result_pattern: sk.result_pattern,
            source: source(cx, f, &format!("#tasks.{k}")),
            group: Some(group.into()),
            ..Default::default()
        });
    }
    let has_tests = cx.files.iter().any(|p| {
        p.starts_with(dir)
            && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                ["_test.ts", ".test.ts", "_test.tsx", ".test.tsx", "_test.js", ".test.js"].iter().any(|s| n.ends_with(s))
            })
            && !p.components().any(|c| c.as_os_str() == "node_modules")
    });
    if has_tests && !tasks.contains_key("test") {
        cx.add_run(RunConfig {
            name: scoped("deno test", &cwd),
            kind: RunKind::Test,
            command: "deno test -A".into(),
            cwd,
            source: source(cx, f, " (test files)"),
            group: Some("test".into()),
            ..Default::default()
        });
    }
}

/// `prebuild` / `postbuild` are hooks of `build`, not runs of their own.
fn is_hook(name: &str, scripts: &serde_json::Map<String, serde_json::Value>) -> bool {
    ["pre", "post"]
        .iter()
        .any(|p| name.strip_prefix(p).is_some_and(|rest| !rest.is_empty() && scripts.contains_key(rest)))
}
