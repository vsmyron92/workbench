//! PHP projects (`composer.json`) → runs: Composer scripts (`composer run-script
//! <name>`, event hooks skipped), Laravel (`php artisan serve` :8000, `php artisan
//! test`), Symfony (`symfony server:start` or PHP's built-in server on :8000) and
//! PHPUnit.

use std::path::Path;

use super::{Ctx, TaskGraph, command_words, deploy_or, kind_from_task_name, on_path, port_in, scoped, sh, source};
use crate::config::project::{Component, Ready, RunConfig, RunKind};

/// Composer event names: they run on install/update, not on their own.
fn is_event(name: &str) -> bool {
    name.starts_with("pre-") || name.starts_with("post-") || matches!(name, "command-event" | "init")
}

pub fn detect(cx: &mut Ctx, f: &Path) {
    let Some(dir) = f.parent() else { return };
    let Some(src) = cx.read(f) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&src) else { return };
    let cwd = cx.rel(dir);
    cx.tag("php");
    cx.pf.components.push(Component { name: scoped("php", &cwd), path: cwd.clone(), kind: "composer".into(), version: None });
    let requires = |pkg: &str| {
        ["require", "require-dev"].iter().any(|k| v.get(k).and_then(|r| r.as_object()).is_some_and(|r| r.contains_key(pkg)))
    };
    let artisan = dir.join("artisan");
    let laravel = artisan.is_file() && (requires("laravel/framework") || cx.read(&artisan).is_some_and(|t| t.contains("Illuminate")));
    if laravel {
        cx.tag("laravel");
        cx.add_run(RunConfig {
            name: scoped("artisan serve", &cwd),
            kind: RunKind::Server,
            command: "php artisan serve".into(),
            cwd: cwd.clone(),
            port: Some(8000),
            ready: Some(Ready { log: Some(r"Server running on \[?(https?://[^\]\s]+)".into()), http: None, timeout_s: 120 }),
            preview: Some("http://localhost:8000/".into()),
            source: source(cx, &artisan, ""),
            group: Some("dev".into()),
            ..Default::default()
        });
        cx.add_run(RunConfig {
            name: scoped("artisan test", &cwd),
            kind: RunKind::Test,
            command: "php artisan test".into(),
            cwd: cwd.clone(),
            source: source(cx, &artisan, ""),
            group: Some("test".into()),
            ..Default::default()
        });
    } else if requires("symfony/framework-bundle") && dir.join("bin/console").is_file() {
        cx.tag("symfony");
        let (command, ready) = if on_path("symfony") {
            ("symfony server:start".to_string(), r"Listening on (https?://\S+)")
        } else {
            ("php -S localhost:8000 -t public".to_string(), r"Development Server \((https?://[^)]+)\) started")
        };
        cx.add_run(RunConfig {
            name: scoped("symfony server", &cwd),
            kind: RunKind::Server,
            command,
            cwd: cwd.clone(),
            port: Some(8000),
            ready: Some(Ready { log: Some(ready.into()), http: None, timeout_s: 120 }),
            preview: Some("http://localhost:8000/".into()),
            source: source(cx, &dir.join("bin/console"), ""),
            group: Some("dev".into()),
            ..Default::default()
        });
    }
    if !laravel {
        if let Some(cfg) = ["phpunit.xml", "phpunit.xml.dist", "phpunit.dist.xml"].iter().map(|n| dir.join(n)).find(|p| p.is_file()) {
            cx.add_run(RunConfig {
                name: scoped("phpunit", &cwd),
                kind: RunKind::Test,
                command: "vendor/bin/phpunit".into(),
                cwd: cwd.clone(),
                source: source(cx, &cfg, ""),
                group: Some("test".into()),
                ..Default::default()
            });
        }
    }
    let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) else { return };
    let body_of = |b: &serde_json::Value| match b {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")),
        _ => None,
    };
    // What each script runs: `@other`, `@composer run-script other`, `composer other`.
    let mut graph = TaskGraph::default();
    for (name, b) in scripts {
        let body = body_of(b).unwrap_or_default();
        graph.add(name, &body, composer_refs(&body));
    }
    for (name, body) in scripts.iter().take(30) {
        if is_event(name) {
            continue;
        }
        let Some(body) = body_of(body) else { continue };
        let kind = kind_from_task_name(name);
        let port = (kind == RunKind::Server).then(|| port_in(&body)).flatten();
        cx.add_run(RunConfig {
            name: scoped(&format!("composer {name}"), &cwd),
            kind,
            command: format!("composer run-script {}", sh(name)),
            cwd: cwd.clone(),
            port,
            preview: port.map(|p| format!("http://localhost:{p}/")),
            source: source(cx, f, &format!("#scripts.{name}")),
            group: Some(deploy_or(kind, graph.reaches_out(name))),
            ..Default::default()
        });
    }
}

/// Scripts a Composer script body runs: `@name` (a reference), and `@composer
/// [run-script|run] name` / `composer [run-script|run] name`.
fn composer_refs(body: &str) -> Vec<String> {
    let mut out = vec![];
    for words in command_words(body) {
        let Some(first) = words.first() else { continue };
        let tool_at = match first.strip_prefix('@') {
            Some("composer") => Some(0),
            Some("php" | "putenv") => None,
            Some(name) => {
                out.push(name.to_string());
                None
            }
            None => words.iter().position(|w| w == "composer"),
        };
        let Some(i) = tool_at else { continue };
        let mut rest = words[i + 1..].iter().filter(|w| !w.starts_with('-'));
        let mut next = rest.next();
        if next.is_some_and(|w| w == "run-script" || w == "run") {
            next = rest.next();
        }
        out.extend(next.cloned());
    }
    out
}
