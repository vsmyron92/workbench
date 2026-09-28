//! Elixir projects (`mix.exs`; an umbrella's apps belong to it) → `mix test`,
//! and `mix phx.server` for Phoenix, on the Endpoint's port from `config/dev.exs`
//! (4000 when it names none).

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::{Ctx, scoped, source};
use crate::config::project::{Component, Ready, RunConfig, RunKind};

/// The Endpoint's `http: [ip: {127, 0, 0, 1}, port: 4001]` in `config/dev.exs`.
static HTTP_OPTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bhttp:\s*\[([^\]]{0,400})\]").unwrap());
/// `port: 4001`, or `port: String.to_integer(System.get_env("PORT") || "4001")`.
static PORT_OPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bport:\s*(?:(\d{2,5})\b|[^,\]]*?"PORT"\)\s*\|\|\s*"?(\d{2,5}))"#).unwrap());

/// The development port of a Phoenix Endpoint, from `config/dev.exs`.
pub(super) fn endpoint_port(dev_exs: &str) -> Option<u16> {
    let body: String = dev_exs.lines().filter(|l| !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n");
    HTTP_OPTS.captures_iter(&body).find_map(|h| {
        let c = PORT_OPT.captures(&h[1])?;
        c.get(1).or_else(|| c.get(2))?.as_str().parse::<u16>().ok().filter(|p| *p >= 80)
    })
}

pub fn detect(cx: &mut Ctx, f: &Path) {
    let Some(dir) = f.parent() else { return };
    if cx.ancestor_marked("mix:", dir) || !cx.mark(format!("mix:{}", dir.display())) {
        return;
    }
    let Some(src) = cx.read(f) else { return };
    let cwd = cx.rel(dir);
    cx.tag("elixir");
    cx.pf.components.push(Component { name: scoped("elixir", &cwd), path: cwd.clone(), kind: "mix".into(), version: None });
    let declares_phoenix = |s: &str| s.contains("{:phoenix,");
    let mut phoenix = declares_phoenix(&src);
    // An umbrella's apps declare Phoenix; the root runs `phx.server` for all of them.
    if !phoenix && src.contains("apps_path") {
        let apps: Vec<std::path::PathBuf> = cx
            .files
            .iter()
            .filter(|p| p.starts_with(dir) && p.file_name().is_some_and(|n| n == "mix.exs") && p.parent() != Some(dir))
            .take(20)
            .cloned()
            .collect();
        phoenix = apps.iter().any(|a| cx.read(a).is_some_and(|t| declares_phoenix(&t)));
    }
    if phoenix {
        cx.tag("phoenix");
        let dev = dir.join("config/dev.exs");
        let port = if dev.is_file() { cx.read(&dev).and_then(|s| endpoint_port(&s)) } else { None }.unwrap_or(4000);
        cx.add_run(RunConfig {
            name: scoped("phx.server", &cwd),
            kind: RunKind::Server,
            command: "mix phx.server".into(),
            cwd: cwd.clone(),
            port: Some(port),
            ready: Some(Ready { log: Some(r"Running \S+ with \S+.* at \S+:\d+|Access \S+ at (https?://\S+)".into()), http: None, timeout_s: 600 }),
            preview: Some(format!("http://localhost:{port}/")),
            source: source(cx, f, " (phoenix)"),
            group: Some("dev".into()),
            ..Default::default()
        });
    }
    cx.add_run(RunConfig {
        name: scoped("mix test", &cwd),
        kind: RunKind::Test,
        command: "mix test".into(),
        cwd,
        source: source(cx, f, ""),
        group: Some("test".into()),
        ..Default::default()
    });
}
