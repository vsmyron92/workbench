//! Detection tests on small fixture trees (written to temp dirs), plus a read-only
//! smoke test over real projects in ~/workspace (ignored by default).
//! Ecosystem fixtures live in `tests/<ecosystem>.rs`.

use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::config::project::{Confirm, EnvKind, RunKind};

mod compose;
mod forge;
mod gating;
mod go;
mod jvm_cmake;
mod layout;
mod make;
mod python;
mod web;

pub(super) fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A fixture tree in a temp dir: `(path, content)` pairs.
pub(super) fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        write(d.path(), rel, text);
    }
    d
}

pub(super) fn run<'a>(pf: &'a ProjectFile, name: &str) -> &'a RunConfig {
    pf.runs.iter().find(|r| r.name == name).unwrap_or_else(|| {
        panic!("no run {name:?}; have {:?}", pf.runs.iter().map(|r| &r.name).collect::<Vec<_>>())
    })
}

pub(super) fn names(pf: &ProjectFile) -> Vec<&str> {
    pf.runs.iter().map(|r| r.name.as_str()).collect()
}

pub(super) fn has_run(pf: &ProjectFile, name: &str) -> bool {
    pf.runs.iter().any(|r| r.name == name)
}

/// Detect `root` and check what every proposal must satisfy: unique names, a
/// `detected:` source (documentation suggestions: `<doc>:L<n>`), nothing that runs
/// by itself (`status`), dependencies that exist, and no unknown `{placeholder}`.
pub(super) fn detect_checked(root: &Path) -> ProjectFile {
    let pf = detect(root);
    let mut seen = std::collections::BTreeSet::new();
    for r in &pf.runs {
        assert!(seen.insert(r.name.clone()), "duplicate run name {:?} in {:?}", r.name, names(&pf));
        let src = r.source.as_deref().unwrap_or("");
        if r.group.as_deref() == Some("suggested") {
            assert!(src.contains(":L"), "{}: suggestion source {src:?}", r.name);
        } else {
            assert!(src.starts_with("detected:"), "{}: source {src:?}", r.name);
        }
        assert!(r.status.is_none() && r.stop.is_none(), "{}: detection never runs anything by itself", r.name);
        for d in &r.depends_on {
            assert!(d.starts_with("port:") || pf.runs.iter().any(|x| &x.name == d), "{}: unknown dependency {d:?}", r.name);
        }
        let unknown = crate::apps::expand::unknown_placeholders(&r.command, &pf.toolchains.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
        assert!(unknown.is_empty(), "{}: unknown placeholders {unknown:?} in {:?}", r.name, r.command);
        assert!(!r.cwd.starts_with('/') && !r.cwd.contains(".."), "{}: cwd {:?}", r.name, r.cwd);
        if let Some(re) = r.ready.as_ref().and_then(|x| x.log.as_deref()) {
            regex::Regex::new(re).unwrap_or_else(|e| panic!("{}: ready.log {re:?}: {e}", r.name));
        }
        if let Some(re) = r.result_pattern.as_deref() {
            crate::apps::output::ResultParser::new(re).unwrap_or_else(|e| panic!("{}: result_pattern: {e}", r.name));
        }
    }
    pf
}

#[test]
fn cargo_workspace_with_an_axum_server() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "app/server/Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"crates/api\", \"crates/core\"]\n");
    write(r, "app/server/crates/api/Cargo.toml", "[package]\nname = \"my-api\"\n[dependencies]\naxum = \"0.8\"\n");
    write(
        r,
        "app/server/crates/api/src/main.rs",
        "fn main() {\n    let port: u16 = std::env::var(\"PORT\")\n        .ok()\n        .and_then(|p| p.parse().ok())\n        .unwrap_or(8080);\n    println!(\"my-api listening on http://{addr}\");\n}\n",
    );
    write(r, "app/server/crates/core/Cargo.toml", "[package]\nname = \"core\"\n");
    write(r, "app/server/crates/core/src/lib.rs", "");
    let pf = detect(r);
    let api = run(&pf, "my-api");
    assert_eq!(api.kind, RunKind::Server);
    assert_eq!(api.command, "cargo run -p my-api");
    assert_eq!(api.cwd, "app/server");
    assert_eq!(api.port, Some(8080));
    assert!(api.free_port);
    assert_eq!(api.ready.as_ref().unwrap().log.as_deref(), Some(r"my\-api listening on"));
    assert_eq!(api.source.as_deref(), Some("detected:app/server/crates/api/Cargo.toml"));
    let t = run(&pf, "cargo test (app/server)");
    assert_eq!((t.kind, t.command.as_str()), (RunKind::Test, "cargo test --workspace"));
    assert!(t.result_pattern.is_some());
    assert!(!pf.runs.iter().any(|r| r.name == "core"), "a library has no run");
    assert!(pf.project.tags.contains(&"rust".to_string()));
    assert_eq!(pf.components[0].kind, "cargo-workspace");
}

#[test]
fn single_crate_with_several_binaries() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "Cargo.toml", "[package]\nname = \"tool\"\n[[bin]]\nname = \"serve\"\npath = \"src/serve.rs\"\n[dependencies]\nwarp = \"0.3\"\n");
    write(r, "src/main.rs", "fn main() {}");
    write(r, "src/serve.rs", "fn main() { let addr = \"127.0.0.1:3030\"; }");
    let pf = detect(r);
    assert_eq!(run(&pf, "tool").command, "cargo run --bin tool");
    let s = run(&pf, "serve");
    assert_eq!(s.command, "cargo run --bin serve");
    assert_eq!(s.port, Some(3030));
    assert_eq!(run(&pf, "cargo test").command, "cargo test");
}

#[test]
fn port_detection_variants() {
    assert_eq!(cargo::detect_port(r#"env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8080)"#), Some(8080));
    assert_eq!(cargo::detect_port(r#"env::var("APP_PORT").unwrap_or_else(|_| "9000".into())"#), None);
    assert_eq!(cargo::detect_port(r#"std::env::var("HTTP_PORT").unwrap_or_else(|| "9000".into())"#), Some(9000));
    assert_eq!(cargo::detect_port("SocketAddr::from(([0, 0, 0, 0], 4000))"), Some(4000));
    assert_eq!(cargo::detect_port(r#"bind("0.0.0.0:7000")"#), Some(7000));
    assert_eq!(cargo::detect_port("no port here"), None);
}

#[test]
fn package_json_scripts_and_vite() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "app/web/package.json",
        r#"{"scripts":{"dev":"vite","build":"tsc -b && vite build","lint":"oxlint","preview":"vite preview","test":"vitest run","postinstall":"patch-package","prebuild":"echo pre"}}"#,
    );
    write(r, "app/web/package-lock.json", "{}");
    write(r, "app/web/vite.config.ts", "export default defineConfig({\n  server: {\n    proxy: {\n      '/api': 'http://localhost:8080',\n    },\n  },\n})\n");
    let pf = detect(r);
    let dev = run(&pf, "dev (app/web)");
    assert_eq!((dev.kind, dev.port, dev.command.as_str()), (RunKind::Server, Some(5173), "npm run dev"));
    assert_eq!(dev.depends_on, vec!["port:8080"]);
    assert_eq!(dev.preview.as_deref(), Some("http://localhost:5173/"));
    assert_eq!(dev.ready.as_ref().unwrap().log.as_deref(), Some(VITE_READY_FOR_TESTS));
    let preview = run(&pf, "preview (app/web)");
    assert_eq!((preview.port, preview.depends_on.len()), (Some(4173), 0));
    let test = run(&pf, "test (app/web)");
    assert_eq!((test.kind, test.command.as_str()), (RunKind::Test, "npm test"));
    assert!(test.result_pattern.is_some());
    assert_eq!(run(&pf, "build (app/web)").kind, RunKind::Build);
    assert_eq!(run(&pf, "lint (app/web)").kind, RunKind::Task);
    assert!(!pf.runs.iter().any(|r| r.name.starts_with("postinstall") || r.name.starts_with("prebuild")));
    assert_eq!(dev.source.as_deref(), Some("detected:app/web/package.json#scripts.dev"));
}

const VITE_READY_FOR_TESTS: &str = r"Local:\s+(https?://\S+)";

#[test]
fn vite_config_ports() {
    let v = node::parse_vite_config("export default { server: { port: 3001, proxy: { '/api': { target: 'http://127.0.0.1:7777' } } }, preview: { port: 4000 } }");
    assert_eq!(v, node::ViteInfo { server_port: Some(3001), preview_port: Some(4000), proxy_port: Some(7777) });
    let v = node::parse_vite_config("server: { host: '127.0.0.1', port: Number(process.env.X ?? 5174), strictPort: true }");
    assert_eq!(v.server_port, Some(5174));
    assert_eq!(node::parse_vite_config("").proxy_port, None);
}

#[test]
fn package_managers_and_frameworks() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "package.json", r#"{"scripts":{"dev":"next dev -p 3100","start":"next start","e2e":"playwright test"}}"#);
    write(r, "pnpm-lock.yaml", "");
    let pf = detect(r);
    let dev = run(&pf, "dev");
    assert_eq!((dev.command.as_str(), dev.port), ("pnpm run dev", Some(3100)));
    assert_eq!(run(&pf, "start").port, Some(3000));
    assert_eq!(run(&pf, "e2e").kind, RunKind::Test);
    let k = node::classify("dev", "cross-env NODE_ENV=development vite --port 5999", &node::ViteInfo::default());
    assert_eq!((k.kind, k.port), (RunKind::Server, Some(5999)));
    assert_eq!(node::classify("unit", "vitest", &node::ViteInfo::default()).kind, RunKind::Test);
    assert_eq!(node::Pm::Bun.run("test"), "bun run test");
    assert_eq!(node::Pm::Yarn.run("dev"), "yarn run dev");
}

const BUILDER_CS: &str = r#"using UnityEditor;
namespace Game.Tools
{
    public static class Builder
    {
        // [MenuItem("Game/Commented Out")] public static void Nope() {}
        [MenuItem("Game/Build Everything %#b")]
        public static void BuildAll() => BuildWith("x");

        [MenuItem("Game/Build Everything", true)]
        public static bool BuildAllValidate() => true;

        [MenuItem("Game/Validate Scene", false, 20)]
        public static void ValidateScene()
        {
            var s = "{ not a brace }";
        }

        [MenuItem("Game/Open Window")]
        public static void Open() { EditorWindow.GetWindow<Win>(); }

        private static void BuildWith(string enemy) { }

        public class Nested
        {
            [MenuItem("Game/Nested Thing")]
            public static void Run() { }
        }
    }
}
"#;

const SMOKE_CS: &str = r#"using UnityEditor;
public static class SmokeTest
{
    [MenuItem("Game/Run PlayMode Smoke Test")]
    public static void Run() { Tick(); }
    static void Report(string name, bool pass) {
        Debug.Log($"SMOKETEST_RESULT: {name} = {(pass ? "PASS" : "FAIL")} :: detail");
        EditorApplication.Exit(pass ? 0 : 1);
    }
}
"#;

#[test]
fn unity_project_and_menu_items() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "game/ProjectSettings/ProjectVersion.txt", "m_EditorVersion: 6000.5.6f1\nm_EditorVersionWithRevision: 6000.5.6f1 (0e0577a1a2ac)\n");
    write(r, "game/Assets/Editor/Builder.cs", BUILDER_CS);
    write(r, "game/Assets/Editor/SmokeTest.cs", SMOKE_CS);
    write(r, "game/Assets/Scripts/Runtime.cs", "[MenuItem(\"Not/Editor\")] public static void X() {}");
    write(r, "game/Assets/Editor~/Ignored.cs", "[MenuItem(\"Ignored/Item\")] public static void X() {}");
    write(r, "game/game.sln", "Project(\"{FAE04EC0}\") = \"Assembly-CSharp\", \"Assembly-CSharp.csproj\", \"{1}\"\n");
    let pf = detect(r);
    let home = dirs::home_dir().unwrap();
    let editor = format!("{}", tilde(&home.join("Unity/Hub/Editor/6000.5.6f1/Editor/Unity")));
    assert_eq!(pf.toolchains.get("unity"), Some(&editor));
    let ed = run(&pf, "Unity Editor (game)");
    assert_eq!((ed.kind, ed.command.as_str(), ed.cwd.as_str()), (RunKind::Editor, "{unity} -projectPath \"$PWD\"", "game"));

    let build = run(&pf, "Unity: Game/Build Everything (game)");
    assert_eq!(build.kind, RunKind::Build);
    assert_eq!(build.command, "{unity} -batchmode -nographics -quit -projectPath \"$PWD\" -executeMethod Game.Tools.Builder.BuildAll -logFile -");
    let validate = run(&pf, "Unity: Game/Validate Scene (game)");
    assert_eq!(validate.kind, RunKind::Test);
    assert!(validate.command.contains("-executeMethod Game.Tools.Builder.ValidateScene"));
    let nested = run(&pf, "Unity: Game/Nested Thing (game)");
    assert!(nested.command.contains("-executeMethod Game.Tools.Builder+Nested.Run"), "{}", nested.command);
    let smoke = run(&pf, "Unity: Game/Run PlayMode Smoke Test (game)");
    assert_eq!(smoke.kind, RunKind::Test);
    assert!(!smoke.command.contains("-quit"), "the test exits by itself: {}", smoke.command);
    assert!(smoke.command.contains("-executeMethod SmokeTest.Run"));
    assert!(smoke.result_pattern.as_deref().unwrap().starts_with("^SMOKETEST_RESULT: "));
    for absent in ["Commented Out", "Open Window", "Not/Editor", "Ignored/Item"] {
        assert!(!pf.runs.iter().any(|r| r.name.contains(absent)), "{absent} must not be a run");
    }
    assert!(!pf.runs.iter().any(|r| r.name.starts_with("dotnet")), "Unity's generated .sln is not a .NET solution");
    assert!(pf.project.tags.contains(&"unity".to_string()));
}

#[test]
fn class_nesting_is_tracked_through_strings_and_comments() {
    let items = unity::menu_items(BUILDER_CS);
    let names: Vec<&str> = items.iter().map(|i| i.menu.as_str()).collect();
    assert_eq!(names, vec!["Game/Build Everything", "Game/Validate Scene", "Game/Nested Thing"]);
}

#[test]
fn dotnet_solution_with_tests_and_validate_script() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "slice/sim/Sim.sln",
        "Project(\"{9A19103F}\") = \"Sim\", \"Sim\\Sim.csproj\", \"{A}\"\nEndProject\nProject(\"{9A19103F}\") = \"Sim.Tests\", \"Sim.Tests\\Sim.Tests.csproj\", \"{B}\"\nEndProject\n",
    );
    write(r, "slice/sim/Sim/Sim.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>");
    write(r, "slice/sim/Sim.Tests/Sim.Tests.csproj", "<Project><ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" /></ItemGroup></Project>");
    write(r, "data/validate.mjs", "process.exit(0)");
    let pf = detect(r);
    let t = run(&pf, "dotnet test (slice/sim)");
    assert_eq!(t.kind, RunKind::Test);
    assert!(t.command.ends_with("test"));
    assert!(t.result_pattern.is_some());
    assert_eq!(run(&pf, "dotnet build (slice/sim)").kind, RunKind::Build);
    let v = run(&pf, "validate (data)");
    assert_eq!((v.kind, v.command.as_str(), v.cwd.as_str()), (RunKind::Test, "node validate.mjs", "data"));
}

const CADDYFILE: &str = r#"# front end
www.example.app {
	redir https://example.app{uri} permanent
}

example.app {
	encode gzip
	handle {
		reverse_proxy localhost:8080
	}
	handle_errors {
		respond "{err.status_code}" {err.status_code}
	}
}

staging.example.app {
	@site not path /api/* /healthz
	basic_auth @site {
		staging $2a$14$HASHHASHHASHHASHHASH
	}
	handle {
		reverse_proxy 127.0.0.1:8081
	}
}
"#;

#[test]
fn caddy_sites_deploy_scripts_and_ssh_hosts_become_environments() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, ".gitlab-ci.yml", "stages: [test]\n.hidden: {}\nvariables: {A: b}\ntests:\n  script: [cargo test]\ndocker-image:\n  script:\n    - docker push r:$CI_COMMIT_SHORT_SHA\n");
    write(r, ".git/config", "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://oauth2:glpat-SECRET@gitlab.com/acme/shop.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n");
    write(r, ".git/refs/remotes/origin/HEAD", "ref: refs/remotes/origin/main\n");
    write(r, "deploy/Caddyfile", CADDYFILE);
    write(r, "deploy/deploy.sh", "#!/bin/bash\nDIR=/opt/shop\nHEALTH=http://localhost:8080/api/health\nprevious=$(docker inspect shop-api-1 --format '{{.Image}}')\n");
    write(r, "deploy/deploy-staging.sh", "#!/bin/bash\nDIR=/opt/shop-staging\nHEALTH=http://127.0.0.1:8081/api/health\ndocker inspect --format '{{.Image}}' shop-staging-api-1\n");
    write(r, "docs/deploy.md", "# Deploy\n\n```bash\necho \"cd /opt/shop && ./deploy.sh <sha8>\" | ssh -o BatchMode=yes -i ~/.ssh/shop_deploy root@203.0.113.7 bash -s\ngit clone git@gitlab.com:acme/shop.git\n```\n");
    let pf = detect(r);

    let repo = pf.repo.as_ref().unwrap();
    assert_eq!(repo.default_branch.as_deref(), Some("main"));
    let gl = repo.gitlab.as_ref().unwrap();
    assert_eq!((gl.host.as_str(), gl.path.as_str()), ("gitlab.com", "acme/shop"));
    // Detection never names a secret: a repository-derived name only resolves from the
    // machine overlay, so naming one would stop the global [gitlab] token from working.
    assert!(gl.token.is_empty(), "detected GitLab config must fall back to the global token");
    let ci = repo.ci.as_ref().unwrap();
    assert_eq!(ci.jobs, vec!["tests", "docker-image"]);
    assert_eq!(ci.image_tag.as_deref(), Some("short_sha"));
    assert!(!toml::to_string(&pf).unwrap().contains("glpat"), "credentials from the remote URL must never enter the config");

    let host = &pf.hosts["deploy"];
    assert_eq!((host.user.as_str(), host.host.as_str(), host.port), ("root", "203.0.113.7", 22));
    assert_eq!(host.identity_file.as_deref(), Some("~/.ssh/shop_deploy"));
    assert_eq!(pf.hosts.len(), 1, "git@ remotes are not hosts");

    assert_eq!(pf.envs.len(), 2, "the www redirect is not an environment");
    let prod = pf.envs.iter().find(|e| e.name == "production").unwrap();
    assert_eq!((prod.kind, prod.url.as_str(), prod.host.as_deref()), (EnvKind::Production, "https://example.app", Some("deploy")));
    assert_eq!(prod.health.as_ref().unwrap().url, "https://example.app/api/health");
    let dep = prod.deploy.as_ref().unwrap();
    assert_eq!(dep.command, "cd /opt/shop && ./deploy.sh {sha8}");
    assert_eq!((dep.confirm, dep.require_green_pipeline), (Confirm::Typed, true));
    assert_eq!((dep.only_ref.as_deref(), dep.after.as_deref()), (Some("main"), Some("staging")));
    assert_eq!(prod.logs[0].command, "docker logs -f --tail 300 shop-api-1");
    assert!(prod.version.as_ref().unwrap().command.as_ref().unwrap().contains("shop-api-1"));
    assert!(prod.auth.is_none());

    let st = pf.envs.iter().find(|e| e.name == "staging").unwrap();
    assert_eq!(st.kind, EnvKind::Staging);
    let auth = st.auth.as_ref().unwrap();
    assert_eq!(auth.user, "staging");
    assert_eq!(auth.except, vec!["/api/*", "/healthz"]);
    assert_eq!(auth.password, "staging-basic-auth", "a secret *name*, never the hash");
    assert_eq!(st.deploy.as_ref().unwrap().confirm, Confirm::Click);
    assert!(st.deploy.as_ref().unwrap().after.is_none());
    assert!(!toml::to_string(&pf).unwrap().contains("HASHHASH"));
}

#[test]
fn caddyfile_parsing_edge_cases() {
    let sites = deploy::parse_caddyfile(
        "{\n\temail a@b.c\n}\n(common) {\n\tencode gzip\n}\nhttp://plain.example.org, alt.example.org {\n\treverse_proxy :3000 # comment\n}\n:8080 {\n\trespond ok\n}\npreview.example.org {\n\t@m {\n\t\tnot path /public/*\n\t}\n\tbasicauth @m {\n\t\tbob hash\n\t}\n\tfile_server\n}\n",
    );
    assert_eq!(sites.len(), 2);
    assert_eq!((sites[0].host.as_str(), sites[0].https, sites[0].port), ("plain.example.org", false, Some(3000)));
    assert_eq!(sites[1].auth, Some(("bob".into(), vec!["/public/*".into()])));
    assert_eq!(deploy::env_kind("preview.example.org"), EnvKind::Preview);
}

#[test]
fn doc_fences_become_suggested_runs() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "app/web/.keep", "");
    write(
        r,
        "CLAUDE.md",
        "# Commands\n\n```bash\ncd app/web\nnpm run lint && npm run test   # :5173\nnpm run build \\\n  --mode production\n```\n\n```bash\ncargo test --workspace\nssh root@203.0.113.7 uptime\ncurl -H \"PRIVATE-TOKEN: $(cat ~/.gitlab_token)\" https://gitlab.com/api/v4/projects\ngit push origin main\nrm -rf target\ntools/deploy.sh <sha8>\ncd ../outside && make\n```\n\n```json\n{\"not\": \"shell\"}\n```\n\n```console\n$ node tools/simulate.mjs\nsimulated 12 battles\n```\n",
    );
    let pf = detect(r);
    let sug: Vec<&RunConfig> = pf.runs.iter().filter(|r| r.group.as_deref() == Some("suggested")).collect();
    let cmds: Vec<(&str, &str)> = sug.iter().map(|r| (r.command.as_str(), r.cwd.as_str())).collect();
    assert_eq!(
        cmds,
        vec![
            ("npm run lint && npm run test", "app/web"),
            ("npm run build --mode production", "app/web"),
            ("cargo test --workspace", "."),
            ("node tools/simulate.mjs", "."),
        ]
    );
    assert!(sug.iter().all(|r| r.kind == RunKind::Task));
    assert_eq!(sug[0].source.as_deref(), Some("CLAUDE.md:L5"));
    assert_eq!(sug[1].source.as_deref(), Some("CLAUDE.md:L6"));
    assert!(pf.project.docs.contains(&"CLAUDE.md".to_string()));
}

/// A `cd` in a `&&` chain sticks for the following lines of the block, as it does in
/// a shell (this repository's README: `cd server && cargo build --release`, then
/// `./target/release/workbench serve`); `cd ..`, `cd -` and relative `cd`s are followed,
/// a subshell changes nothing.
#[test]
fn doc_fences_follow_the_shell_directory() {
    let cwds = |src: &str| docs::shell_fences(src).into_iter().map(|f| (f.text, f.cwd)).collect::<Vec<_>>();
    let s = |x: &str| Some(x.to_string());
    assert_eq!(
        cwds("```bash\ncd server && cargo build --release\n./target/release/workbench serve --open\n```\n"),
        vec![("cargo build --release".into(), s("server")), ("./target/release/workbench serve --open".into(), s("server"))]
    );
    let got = cwds(
        "```sh\ncd app && npm ci && cd ..\nmake test\nmkdir -p build && cd build\nmake\ncd -\nmake lint\ncd web\ncd src\nnode gen.js\n(cd .. && make docs)\nnode check.js\n```\n```sh\nmake fresh\n```\n",
    );
    assert_eq!(
        got,
        vec![
            ("npm ci && cd ..".into(), s("app")),
            ("make test".into(), s("app/..")),
            ("mkdir -p build && cd build".into(), s("app/..")),
            ("make".into(), s("app/../build")),
            ("make lint".into(), s("app/..")),
            ("node gen.js".into(), s("app/../web/src")),
            ("(cd .. && make docs)".into(), s("app/../web/src")),
            ("node check.js".into(), s("app/../web/src")),
            ("make fresh".into(), None),
        ]
    );
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "server/Cargo.toml", "[package]\nname = \"x\"\n");
    write(d.path(), "README.md", "```bash\ncd server && cargo build --release\n./target/release/workbench serve --open\n```\n");
    let pf = detect(d.path());
    let sug = pf.runs.iter().find(|r| r.command == "./target/release/workbench serve --open").unwrap();
    assert_eq!(sug.cwd, "server");

    // Documents written line by line from the root keep working: `cd app/server` …
    // `cd app/web` (no app/server/app/web), and a root-relative script after a
    // `cd x && …` line.
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "app/server/Cargo.toml", "");
    write(d.path(), "app/web/package.json", "{}");
    write(d.path(), "slice/sim/Sim.csproj", "");
    write(d.path(), "slice/pipelines/gen.py", "");
    write(
        d.path(),
        "CLAUDE.md",
        "```bash\ncd app/server\ncargo test\ncd app/web\nnpm run lint\n```\n\n```bash\ncd slice/sim && dotnet test\npython3 slice/pipelines/gen.py\ndotnet build\n```\n",
    );
    let pf = detect(d.path());
    let at = |cmd: &str| pf.runs.iter().find(|r| r.command == cmd).map(|r| r.cwd.as_str()).unwrap_or_else(|| panic!("{cmd}: {:?}", names(&pf)));
    assert_eq!(at("cargo test"), "app/server");
    assert_eq!(at("npm run lint"), "app/web");
    assert_eq!(at("dotnet test"), "slice/sim");
    assert_eq!(at("python3 slice/pipelines/gen.py"), ".");
    assert_eq!(at("dotnet build"), "slice/sim", "nothing says otherwise: the shell's directory");
}

#[test]
fn suggestions_skip_commands_detection_already_has() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "package.json", r#"{"scripts":{"dev":"vite"}}"#);
    write(r, "README.md", "```sh\nnpm run dev\nnpm run storybook\n```\n");
    let pf = detect(r);
    let sug: Vec<&str> = pf.runs.iter().filter(|r| r.group.as_deref() == Some("suggested")).map(|r| r.command.as_str()).collect();
    assert_eq!(sug, vec!["npm run storybook"]);
}

#[test]
fn unsafe_doc_commands_are_never_offered() {
    for c in [
        "ssh root@1.2.3.4 'docker ps'",
        "scp deploy/Caddyfile root@host:/etc/caddy/Caddyfile",
        "git push --force",
        "docker push registry/x:latest",
        "./deploy.sh 1234abcd",
        "curl -X POST https://api.example.com/x",
        "curl -d @body.json https://api.example.com/x",
        "export GITLAB_TOKEN=abc && npm run release",
        "rm -rf ~/.cache",
        "sudo systemctl restart caddy",
        "fuser -k 8080/tcp",
        "kubectl apply -f k8s/",
        "npm publish",
    ] {
        assert!(!docs::is_offerable(c), "{c}");
    }
    for c in ["npm run dev", "cargo nextest run --workspace", "python3 tools/gen.py", "./scripts/lint.sh", "curl -s http://localhost:8080/api/health"] {
        assert!(docs::is_offerable(c), "{c}");
    }
}

#[test]
fn confluence_links_in_docs() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "CLAUDE.md",
        "Design lives in Confluence: site `acme.atlassian.net`, space **`DESIGN`**, cloudId `127de92c-f2ea-4e41-8bc5-8f5b62025841`.\n\
         Start at <https://acme.atlassian.net/wiki/spaces/DESIGN/pages/65601/Design+Notes>.\n",
    );
    write(r, "docs/design/README.md", "> Moved from Confluence (Design space, page 3342337) on 2026-09-26.\n");
    write(r, "docs/design/note.md", "> Moved from Confluence (Design space, page 999) on 2026-09-26.\n");
    let pf = detect(r);
    let c = pf.links.confluence.as_ref().unwrap();
    assert_eq!((c.site.as_str(), c.space.as_str()), ("https://acme.atlassian.net", "DESIGN"));
    assert_eq!(c.cloud_id.as_deref(), Some("127de92c-f2ea-4e41-8bc5-8f5b62025841"));
    assert!(c.root_pages.contains(&65601) && c.root_pages.contains(&3342337));
    assert!(!c.root_pages.contains(&999), "only index docs are scanned");
    assert!(c.archived);
}

/// The Atlassian client rejects a site without a scheme, and a project's
/// `[links.confluence] site` overrides the global one: a bare host would break
/// Confluence and Jira for the project.
#[test]
fn detected_confluence_site_has_a_scheme() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(r, "CLAUDE.md", "Docs moved from Confluence (DOC space, page 42). Site: `Fixture-Site.atlassian.net`.\n");
    let c = detect(r).links.confluence.unwrap();
    assert_eq!(c.site, "https://fixture-site.atlassian.net");
    assert_eq!(docs::site_url("acme"), "https://acme.atlassian.net");
    assert_eq!(docs::site_url("acme.atlassian.net"), "https://acme.atlassian.net");
}

#[test]
fn sensitive_files_by_name() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for f in ["~.anthropic_api_sk", "app/server/.env", "app/web/.env.local", "server.env", ".env.example", "server.env.example", "certs/tls.pem"] {
        write(r, f, "x");
    }
    let pf = detect(r);
    let s = &pf.project.sensitive;
    for f in ["~.anthropic_api_sk", "app/server/.env", "app/web/.env.local", "server.env", "certs/tls.pem"] {
        assert!(s.contains(&f.to_string()), "{f} in {s:?}");
    }
    assert!(!s.iter().any(|x| x.ends_with(".example")));
}

/// Worktrees, submodules and nested clones are separate projects, and gitignored
/// directories are not part of this one: none of them adds runs, environments or
/// ssh hosts. Gitignored *files* still count (deploy topology is often ignored).
#[test]
fn nested_repos_and_ignored_dirs_are_not_part_of_the_project() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let axum = "[package]\nname = \"app\"\n[dependencies]\naxum = \"0.8\"\n";
    let main = "fn main() { let addr = \"127.0.0.1:3000\"; }";
    let vite = r#"{"scripts":{"dev":"vite"}}"#;
    std::fs::create_dir_all(r.join(".git/info")).unwrap();
    std::fs::write(r.join(".git/info/exclude"), "scratch/\n").unwrap();
    write(r, ".gitignore", "tmp/\nCaddyfile\n");
    write(r, "server/Cargo.toml", axum);
    write(r, "server/src/main.rs", main);
    write(r, "web/package.json", vite);
    write(r, "deploy/Caddyfile", "staging.example.com {\n  reverse_proxy 127.0.0.1:8081\n}\n");
    // Claude Code worktrees (a `.git` file each) — under .claude and elsewhere.
    for wt in [".claude/worktrees/agent-a", "wt/feature"] {
        write(r, &format!("{wt}/.git"), "gitdir: /nowhere/.git/worktrees/x\n");
        write(r, &format!("{wt}/server/Cargo.toml"), axum);
        write(r, &format!("{wt}/server/src/main.rs"), main);
        write(r, &format!("{wt}/web/package.json"), vite);
    }
    // A nested clone with its own deploy topology and ssh host.
    std::fs::create_dir_all(r.join("nested-clone/.git")).unwrap();
    write(r, "nested-clone/web/package.json", vite);
    write(r, "nested-clone/deploy/Caddyfile", "otherapp.example.com {\n  reverse_proxy 127.0.0.1:9000\n}\n");
    write(r, "nested-clone/README.md", "```bash\nssh -i ~/.ssh/other_key root@203.0.113.7 'docker ps'\n```\n");
    // Gitignored directories (by .gitignore and by info/exclude), one holding a secret.
    write(r, "tmp/old/web/package.json", vite);
    write(r, "tmp/secrets/prod.env", "X=1");
    write(r, "scratch/web/package.json", vite);

    let pf = detect(r);
    let names: Vec<&str> = pf.runs.iter().map(|x| x.name.as_str()).collect();
    for x in &pf.runs {
        for bad in [".claude", "wt/", "nested-clone", "tmp/", "scratch"] {
            assert!(!x.cwd.contains(bad) && !x.name.contains(bad), "run {} ({}) comes from {bad}; runs: {names:?}", x.name, x.cwd);
        }
    }
    let app = run(&pf, "app");
    assert_eq!((app.cwd.as_str(), app.port), ("server", Some(3000)));
    assert_eq!(run(&pf, "dev (web)").cwd, "web");
    let expected = if on_path("cargo-nextest") { 4 } else { 3 };
    assert_eq!(pf.runs.len(), expected, "app, cargo test, (nextest,) dev: {names:?}");
    // The gitignored Caddyfile of this project still gives its environment…
    assert_eq!(pf.envs.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), vec!["https://staging.example.com"]);
    // …but the nested clone's site and ssh host do not leak in.
    assert!(pf.hosts.is_empty(), "{:?}", pf.hosts);
    // Secrets in ignored directories are still flagged.
    assert!(pf.project.sensitive.contains(&"tmp/secrets/prod.env".to_string()), "{:?}", pf.project.sensitive);
}

/// Deploy and release scripts stay runs of their own group; starting them needs the
/// user's confirmation (`runs::needs_confirmation`) and agents cannot start them.
#[test]
fn deploy_and_release_scripts_are_set_apart() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    write(
        r,
        "web/package.json",
        r#"{"scripts":{"dev":"vite","deploy":"firebase deploy","release":"semantic-release","deploy:prod":"gh-pages -d dist","build:rs":"cargo build --release","lint":"eslint ."}}"#,
    );
    let pf = detect(r);
    for name in ["deploy (web)", "release (web)", "deploy:prod (web)"] {
        let x = run(&pf, name);
        assert_eq!(x.group.as_deref(), Some("deploy"), "{name}");
        assert!(crate::apps::runs::needs_confirmation(x), "{name}");
    }
    for name in ["dev (web)", "build:rs (web)", "lint (web)"] {
        let x = run(&pf, name);
        assert_ne!(x.group.as_deref(), Some("deploy"), "{name}");
        assert!(!crate::apps::runs::needs_confirmation(x), "{name}");
    }
    assert!(docs::is_risky("npm run release") && docs::is_risky("vercel --prod") && docs::is_risky("npx changeset publish"));
    assert!(!docs::is_risky("cargo build --release") && !docs::is_risky("npm run build -- --mode release-candidate"));
}

#[test]
fn same_run_name_in_two_directories_is_qualified() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let axum = "[package]\nname = \"app\"\n[dependencies]\naxum = \"0.8\"\n";
    // `a/…` sorts before `server/`, but the shallower package keeps the plain name.
    write(r, "a/b/server/Cargo.toml", axum);
    write(r, "a/b/server/src/main.rs", "fn main() {}");
    write(r, "server/Cargo.toml", axum);
    write(r, "server/src/main.rs", "fn main() {}");
    let pf = detect(r);
    assert_eq!(run(&pf, "app").cwd, "server");
    assert_eq!(run(&pf, "app (a/b/server)").cwd, "a/b/server");
}

#[test]
fn hostile_and_empty_trees_are_fine() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    assert_eq!(detect(r).runs.len(), 0);
    assert_eq!(detect(&r.join("does-not-exist")).runs.len(), 0);
    write(r, "package.json", "\u{0}\u{1} not json");
    write(r, "Cargo.toml", "[[[ not toml");
    write(r, ".gitlab-ci.yml", ": : :");
    write(r, "ProjectSettings/ProjectVersion.txt", "garbage");
    write(r, "Caddyfile", "{{{{ }}");
    write(r, "CLAUDE.md", "```bash\n\\\n\\\n");
    // A symlink loop must not be followed, and a FIFO must not block.
    crate::util::os::fs::symlink(r, r.join("loop")).unwrap();
    nix::unistd::mkfifo(&r.join("CLAUDE.md.fifo"), nix::sys::stat::Mode::S_IRWXU).unwrap();
    std::fs::remove_file(r.join("CLAUDE.md")).unwrap();
    std::fs::rename(r.join("CLAUDE.md.fifo"), r.join("CLAUDE.md")).unwrap();
    let pf = detect(r);
    assert!(pf.runs.is_empty());
}

#[test]
fn detection_is_fast_on_a_mixed_tree() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for i in 0..300 {
        write(r, &format!("src/mod{i}/file{i}.rs"), "fn x() {}");
    }
    for i in 0..500 {
        write(r, &format!("node_modules/p{i}/package.json"), r#"{"scripts":{"dev":"vite"}}"#);
    }
    write(r, "package.json", r#"{"scripts":{"dev":"vite"}}"#);
    let t = Instant::now();
    let pf = detect(r);
    assert!(t.elapsed() < Duration::from_millis(500), "took {:?}", t.elapsed());
    assert_eq!(pf.runs.iter().filter(|r| r.name == "dev").count(), 1, "node_modules is skipped");
}

/// Read-only smoke test: prints what detection proposes for every project under
/// `~/workspace`, and for the directories in `WORKBENCH_DETECT_DIRS`
/// (`:`-separated). Ignored by default because it depends on the machine.
#[test]
#[ignore = "reads ~/workspace (or WORKBENCH_DETECT_DIRS); run: cargo test detect_real_projects -- --ignored --nocapture"]
fn detect_real_projects() {
    let Some(home) = dirs::home_dir() else { return };
    let mut roots: Vec<std::path::PathBuf> = std::fs::read_dir(home.join("workspace"))
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    roots.sort();
    if let Some(extra) = std::env::var_os("WORKBENCH_DETECT_DIRS") {
        roots.extend(std::env::split_paths(&extra).filter(|p| p.is_dir()));
    }
    for root in roots {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = name.as_str();
        let _ = detect(&root); // warm: regexes compile once per process, the page cache fills
        let t = Instant::now();
        let pf = detect_checked(&root);
        let took = t.elapsed();
        println!("# ---- {name}: {} runs, {} envs, {} hosts in {took:?}", pf.runs.len(), pf.envs.len(), pf.hosts.len());
        let confirm: Vec<&str> = pf.runs.iter().filter(|r| crate::apps::runs::needs_confirmation(r)).map(|r| r.name.as_str()).collect();
        println!("# needs confirmation: {confirm:?}");
        println!("{}", toml::to_string_pretty(&pf).unwrap());
        assert!(took < Duration::from_millis(400), "{name} took {took:?}");
        assert!(pf.runs.iter().all(|r| r.source.is_some()), "every detected run has a source");
        assert!(!pf.runs.iter().any(|r| r.cwd.starts_with(".claude")), "agent worktrees are not part of the project");
    }
}
