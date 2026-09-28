//! Whole-repository layouts: a monorepo with several ecosystems, sample
//! directories that are not the project, a repository of examples, and the time
//! detection takes on a large tree.

use std::time::{Duration, Instant};

use super::{detect_checked, has_run, names, run, tree, write};
use crate::config::project::RunKind;

/// A product monorepo: a Next.js front end, a FastAPI service, a Go worker, a
/// local compose stack, a Makefile and CI — plus examples, vendored code and test
/// fixtures that must not add runs of their own.
#[test]
fn monorepo_with_several_ecosystems() {
    let d = tree(&[
        (".git/config", "[remote \"origin\"]\n\turl = https://github.com/acme/platform.git\n"),
        (".github/workflows/ci.yml", "jobs:\n  web: {runs-on: x}\n  api: {runs-on: x}\n"),
        ("package.json", r#"{"private":true,"workspaces":["apps/*"],"scripts":{"dev":"turbo dev","lint":"turbo lint"}}"#),
        ("pnpm-lock.yaml", ""),
        ("turbo.json", r#"{"tasks":{"build":{},"dev":{"persistent":true},"lint":{}}}"#),
        ("apps/web/package.json", r#"{"scripts":{"dev":"next dev","build":"next build","test":"vitest run"}}"#),
        ("services/api/pyproject.toml", "[project]\nname = \"api\"\ndependencies = [\"fastapi\", \"uvicorn\"]\n[dependency-groups]\ndev = [\"pytest\"]\n"),
        ("services/api/uv.lock", ""),
        ("services/api/app/main.py", "from fastapi import FastAPI\napp = FastAPI()\n"),
        ("services/worker/go.mod", "module github.com/acme/platform/worker\n\ngo 1.23\n"),
        ("services/worker/main.go", "package main\n\nfunc main() {}\n"),
        ("docker-compose.yml", "services:\n  db:\n    image: postgres:16\n    ports: ['5432:5432']\n  api:\n    build: ./services/api\n    ports: ['8000:8000']\n"),
        ("Makefile", ".PHONY: up test\nup:\n\tdocker compose up -d\ntest:\n\tpnpm test && cd services/api && uv run pytest\n"),
        // Not the product:
        ("examples/flask-demo/requirements.txt", "flask\n"),
        ("examples/flask-demo/app.py", "from flask import Flask\napp = Flask(__name__)\n"),
        ("examples/next-demo/package.json", r#"{"scripts":{"dev":"next dev"}}"#),
        ("third_party/lib/CMakeLists.txt", "cmake_minimum_required(VERSION 3.10)\nproject(lib)\nadd_executable(libtool x.c)\n"),
        ("services/worker/testdata/golden/go.mod", "module golden\n"),
        ("apps/web/test/fixtures/proj/package.json", r#"{"scripts":{"dev":"vite"}}"#),
        ("vendor/github.com/x/y/go.mod", "module github.com/x/y\n"),
        ("docs/examples/Caddyfile", "demo.example.com {\n  reverse_proxy :9999\n}\n"),
    ]);
    let pf = detect_checked(d.path());
    // Every ecosystem at its place.
    assert_eq!(run(&pf, "dev").command, "pnpm run dev");
    assert_eq!(run(&pf, "turbo build").command, "pnpm exec turbo run build");
    let web = run(&pf, "dev (apps/web)");
    assert_eq!((web.kind, web.port), (RunKind::Server, Some(3000)));
    let api = run(&pf, "uvicorn (services/api)");
    assert_eq!((api.command.as_str(), api.port), ("uv run uvicorn app.main:app --reload", Some(8000)));
    assert_eq!(run(&pf, "pytest (services/api)").command, "uv run pytest");
    assert_eq!(run(&pf, "go test (services/worker)").cwd, "services/worker");
    assert_eq!(run(&pf, "worker (services/worker)").command, "go run .");
    assert_eq!(run(&pf, "compose up").port, Some(8000));
    assert_eq!(run(&pf, "make test").kind, RunKind::Test);
    assert_eq!(run(&pf, "make up").kind, RunKind::Task, "`up` usually detaches (`-d`)");
    let repo = pf.repo.as_ref().unwrap();
    assert_eq!(repo.github.as_ref().unwrap().path, "acme/platform");
    assert_eq!(repo.ci.as_ref().unwrap().jobs, vec!["web", "api"]);
    // …and nothing from examples, third_party, testdata, fixtures or vendor.
    for r in &pf.runs {
        for bad in ["examples", "third_party", "testdata", "fixtures", "vendor"] {
            assert!(!r.cwd.contains(bad) && !r.name.contains(bad), "{} ({}) comes from {bad}: {:?}", r.name, r.cwd, names(&pf));
        }
    }
    assert!(!has_run(&pf, "flask run") && !has_run(&pf, "libtool"));
    assert!(pf.envs.is_empty(), "an example Caddyfile is not an environment: {:?}", pf.envs);
    for t in ["node", "python", "go", "docker", "make", "turbo", "uv", "fastapi", "github-actions"] {
        assert!(pf.project.tags.contains(&t.to_string()), "tag {t}: {:?}", pf.project.tags);
    }
    // Dev servers first (the UI's default run), suggestions last.
    assert_eq!(pf.runs[0].kind, RunKind::Server);
}

/// The top bar selects the first server: the project's own run, not the Makefile
/// or Taskfile wrapper of it (both sort before `package.json` / `go.mod` by name).
#[test]
fn native_runs_come_before_task_runner_wrappers() {
    let d = tree(&[
        ("Makefile", ".PHONY: dev\ndev:\n\tnpm run dev\n"),
        ("Taskfile.yml", "version: '3'\ntasks:\n  serve: {cmds: [go run ./cmd/api]}\n"),
        ("package.json", r#"{"scripts":{"dev":"vite"}}"#),
    ]);
    let pf = detect_checked(d.path());
    let servers: Vec<&str> = pf.runs.iter().filter(|r| r.kind == RunKind::Server).map(|r| r.name.as_str()).collect();
    assert_eq!(servers, vec!["dev", "make dev", "task serve"]);
}

/// A repository that *is* a collection of examples proposes them.
#[test]
fn a_repository_of_examples_offers_its_examples() {
    let d = tree(&[
        ("README.md", "# Examples\n"),
        ("examples/hello-go/go.mod", "module hello\n\ngo 1.22\n"),
        ("examples/hello-go/main.go", "package main\n\nfunc main() {}\n"),
        ("examples/todo-flask/requirements.txt", "flask\n"),
        ("examples/todo-flask/app.py", "from flask import Flask\napp = Flask(__name__)\n"),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "hello (examples/hello-go)").command, "go run .");
    assert_eq!(run(&pf, "flask run (examples/todo-flask)").command, "python3 -m flask --app app run --debug");
}

/// A library whose only runnable things are its examples keeps them out when it
/// has anything of its own (here: its tests).
#[test]
fn a_library_with_examples_keeps_its_own_runs() {
    let d = tree(&[
        ("Cargo.toml", "[package]\nname = \"widgets\"\n"),
        ("src/lib.rs", ""),
        ("examples/demo/Cargo.toml", "[package]\nname = \"demo\"\n[dependencies]\naxum = \"0.8\"\n"),
        ("examples/demo/src/main.rs", "fn main() { let a = \"127.0.0.1:3000\"; }\n"),
    ]);
    let pf = detect_checked(d.path());
    assert!(has_run(&pf, "cargo test"));
    assert!(!has_run(&pf, "demo") && !has_run(&pf, "demo (examples/demo)"), "{:?}", names(&pf));
}

/// A large tree with every kind of build file: detection stays bounded.
#[test]
fn detection_is_fast_on_a_large_polyglot_tree() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    // ~5k project files (packages, services, native modules) and ~3.5k in
    // directories the walk must skip.
    for i in 0..40 {
        let pkg = format!("packages/p{i}");
        write(r, &format!("{pkg}/package.json"), &format!(r#"{{"scripts":{{"dev":"vite --port {}","build":"vite build","test":"vitest run"}}}}"#, 5000 + i));
        for j in 0..60 {
            write(r, &format!("{pkg}/src/c{j}/index.ts"), "export const x = 1;\n");
        }
    }
    for i in 0..12 {
        let svc = format!("services/s{i}");
        write(r, &format!("{svc}/pyproject.toml"), "[project]\nname = \"s\"\ndependencies = [\"fastapi\", \"pytest\"]\n[tool.ruff]\n");
        write(r, &format!("{svc}/app/main.py"), "from fastapi import FastAPI\napp = FastAPI()\n");
        write(r, &format!("{svc}/go.mod"), &format!("module example.com/s{i}\n"));
        write(r, &format!("{svc}/cmd/s{i}/main.go"), "package main\nimport \"net/http\"\nfunc main() { http.ListenAndServe(\":8080\", nil) }\n");
        write(r, &format!("{svc}/Makefile"), ".PHONY: run test\nrun:\n\tgo run ./cmd/x\ntest:\n\tgo test ./...\n");
        for j in 0..200 {
            write(r, &format!("{svc}/app/m{j}.py"), "x = 1\n");
        }
    }
    write(r, "CMakeLists.txt", "cmake_minimum_required(VERSION 3.20)\nproject(x)\nenable_testing()\n");
    for i in 0..50 {
        write(r, &format!("native/m{i}/CMakeLists.txt"), &format!("add_executable(tool{i} main.cpp)\n"));
    }
    write(r, "docker-compose.yml", "services:\n  web:\n    build: .\n    ports: ['8080:80']\n");
    for i in 0..3000 {
        write(r, &format!("node_modules/dep{i}/package.json"), r#"{"scripts":{"dev":"vite"}}"#);
    }
    for i in 0..500 {
        write(r, &format!(".venv/lib/python3.12/site-packages/m{i}/pyproject.toml"), "[project]\nname = \"m\"\n");
    }
    let _ = detect_checked(r); // warm the page cache
    let t = Instant::now();
    let pf = detect_checked(r);
    let took = t.elapsed();
    eprintln!("large polyglot tree: {} runs in {took:?}", pf.runs.len());
    // Debug builds are several times slower than release (where this is well under 50 ms).
    assert!(took < Duration::from_millis(1500), "took {took:?}");
    assert!(pf.runs.len() > 100 && pf.runs.len() < 1000, "{}", pf.runs.len());
    assert_eq!(pf.runs.iter().filter(|x| x.name == "dev").count(), 0, "no root package.json: {:?}", &names(&pf)[..5]);
    assert!(!pf.runs.iter().any(|x| x.cwd.contains("node_modules") || x.cwd.contains(".venv")));
}
