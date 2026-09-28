//! Deploy gating follows everything a run runs: npm lifecycle hooks and script
//! chains, Make prerequisites and `$(MAKE) x`, just dependencies, Taskfile `deps` and
//! `task:` calls, Python task sequences, Composer `@x` and deno `deno task x`. A run
//! that reaches another machine or publishes through any of them is in group
//! `deploy`: the UI asks first and agents cannot start it (`runs::needs_confirmation`).
//! Names from repository files never add a command of their own.

use super::super::{TaskGraph, command_words, npm_refs, reaches_out};
use super::{detect_checked, has_run, names, run, tree};
use crate::apps::runs::needs_confirmation;
use crate::config::ProjectFile;
use crate::config::project::RunConfig;

fn group<'a>(pf: &'a ProjectFile, name: &str) -> &'a str {
    run(pf, name).group.as_deref().unwrap_or("")
}

#[track_caller]
fn gated(pf: &ProjectFile, name: &str) {
    let r = run(pf, name);
    assert_eq!(r.group.as_deref(), Some("deploy"), "{name} ({}) should be a deploy", r.command);
    assert!(needs_confirmation(r), "{name}");
}

#[track_caller]
fn open(pf: &ProjectFile, name: &str) {
    let r = run(pf, name);
    assert_ne!(r.group.as_deref(), Some("deploy"), "{name} ({}) is not a deploy", r.command);
    assert!(!needs_confirmation(r), "{name}");
}

#[test]
fn npm_lifecycle_hooks_are_part_of_the_script() {
    // `npm run build` runs `postbuild`; `npm test` runs `pretest`, which builds.
    let d = tree(&[(
        "package.json",
        r#"{"name":"site","scripts":{"dev":"vite","build":"vite build","postbuild":"aws s3 sync dist s3://prod-site --delete","test":"vitest run","pretest":"npm run build","lint":"eslint ."}}"#,
    )]);
    let pf = detect_checked(d.path());
    gated(&pf, "build");
    gated(&pf, "test");
    open(&pf, "dev");
    open(&pf, "lint");
    assert!(!has_run(&pf, "postbuild") && !has_run(&pf, "pretest"), "hooks are not runs: {:?}", names(&pf));
    // The kind still comes from the script itself.
    assert_eq!(run(&pf, "build").kind, crate::config::project::RunKind::Build);
}

#[test]
fn npm_script_chains_are_followed() {
    let d = tree(&[(
        "web/package.json",
        r#"{"name":"web","scripts":{
            "dev":"vite","build":"tsc -b && vite build","lint":"eslint .",
            "ship":"npm run build && npm run upload","upload":"aws s3 sync dist s3://my-bucket --delete",
            "all":"run-s lint build push:*","push:assets":"rsync -av dist/ web@example.com:/srv/www/",
            "watch":"concurrently \"npm:watch-*\"","watch-css":"tailwind -w","watch-sync":"rsync -av dist/ web@example.com:/srv/www/",
            "ci":"yarn lint && yarn sync","sync":"gsutil -m rsync -r dist gs://bucket",
            "bump":"npm version patch","postversion":"git push --follow-tags",
            "fresh":"npm ci && vite build","postinstall":"node scripts/check.js",
            "loop-a":"npm run loop-b","loop-b":"npm run loop-a",
            "other":"npm --prefix ../api run upload"
        }}"#,
    )]);
    let pf = detect_checked(d.path());
    for name in ["ship (web)", "upload (web)", "all (web)", "watch (web)", "ci (web)", "bump (web)"] {
        gated(&pf, name);
    }
    for name in ["dev (web)", "build (web)", "lint (web)", "fresh (web)", "loop-a (web)", "loop-b (web)", "other (web)", "watch-css (web)"] {
        open(&pf, name);
    }
}

#[test]
fn npm_references() {
    let names = ["build", "test", "start", "upload", "watch-a", "watch-b", "postinstall", "prepare"].map(String::from).into_iter().collect();
    let refs = |body: &str| npm_refs(body, &names);
    assert_eq!(refs("npm run build && npm test"), vec!["build", "test"]);
    assert_eq!(refs("cross-env NODE_ENV=production npm run -s upload -- --dry"), vec!["upload"]);
    assert_eq!(refs("pnpm build; bun run upload"), vec!["build", "upload"]);
    assert_eq!(refs("concurrently \"npm:watch-*\" \"npm start\""), vec!["watch-a", "watch-b", "start"]);
    assert_eq!(refs("npm-run-all --parallel watch-*"), vec!["watch-a", "watch-b"]);
    assert_eq!(refs("npm --prefix api run upload && yarn workspace web build && pnpm -r build"), Vec::<String>::new());
    assert_eq!(refs("npm install"), vec!["preinstall", "install", "postinstall", "preprepare", "prepare", "postprepare", "prepublish"]);
    assert_eq!(command_words("(cd web && $(MAKE) push) | tee log"), vec![vec!["cd", "web"], vec!["$(MAKE)", "push"], vec!["tee", "log"]]);
}

#[test]
fn task_graphs_are_bounded_and_cycle_safe() {
    let mut g = TaskGraph::default();
    g.add("a", "echo a", vec!["b".into(), "missing".into()]);
    g.add("b", "echo b", vec!["a".into(), "c".into()]);
    g.add("c", "rsync x host:", vec!["c".into()]);
    assert_eq!(g.bodies("a"), vec!["echo a\n", "echo b\n", "rsync x host:\n"]);
    assert!(g.reaches_out("a") && g.reaches_out("b") && g.reaches_out("c"));
    assert!(g.bodies("missing").is_empty() && !g.reaches_out("missing"));
    // A long chain stops at the depth bound instead of recursing.
    let mut long = TaskGraph::default();
    for i in 0..5000 {
        long.add(&format!("t{i}"), "true", vec![format!("t{}", i + 1)]);
    }
    long.add("t5000", "ssh prod", vec![]);
    assert!(!long.reaches_out("t0"), "past the depth bound");
    assert!(long.reaches_out("t4990"));
}

const MAKEFILE: &str = "SSH = ssh -i ~/.ssh/ops_key
COMPOSE ?= docker compose
PROD_FILES := -f docker-compose.yml -f docker-compose.prod.yml
STEPS = build push
.PHONY: dev test lint build push ship migrate-prod db-reset prod-logs restart-prod ctx-up ship-all upload images-push roll web-upload
dev:
\t$(COMPOSE) up
test:
\tcd backend && uv run pytest
lint:
\tcd backend && uv run ruff check .
build:
\t$(COMPOSE) build
push:
\t$(COMPOSE) $(PROD_FILES) push
ship: build push
\t@echo shipped
roll: $(STEPS)
migrate-prod:
\tDOCKER_HOST=ssh://ops@prod.example.com docker compose run --rm api alembic upgrade head
prod-logs:
\t$(SSH) prod.example.com 'docker logs -f api'
db-reset:
\tdocker compose down -v
restart-prod:
\tDOCKER_HOST=ssh://root@prod.example.com docker compose up -d
ctx-up:
\tdocker --context prod compose up -d
upload: _sync
ship-all:
\t$(MAKE) build
\t@$(MAKE) upload
images-push:
\tdocker-compose push
web-upload:
\t$(MAKE) -C web upload
_sync:
\trsync -av dist/ web@example.com:/srv/www/
";

#[test]
fn make_prerequisites_nested_makes_and_variables() {
    let d = tree(&[("Makefile", MAKEFILE)]);
    let pf = detect_checked(d.path());
    for t in ["push", "ship", "roll", "migrate-prod", "prod-logs", "restart-prod", "ctx-up", "upload", "ship-all", "images-push"] {
        gated(&pf, &format!("make {t}"));
    }
    for t in ["dev", "test", "lint", "build", "db-reset", "web-upload"] {
        open(&pf, &format!("make {t}"));
    }
    assert!(!has_run(&pf, "make _sync"), "private targets are followed, not offered");
}

/// Variables that multiply at every level and a rule with thousands of targets do
/// not make Makefile parsing slow or large.
#[test]
fn hostile_makefiles_stay_small_and_fast() {
    let mut src = format!("A0 = {}\n", "x".repeat(50_000));
    for i in 1..5 {
        src.push_str(&format!("A{i} = $(A{p}) $(A{p}) $(A{p}) $(A{p})\n", p = i - 1));
    }
    let names: Vec<String> = (0..5000).map(|i| format!("t{i}")).collect();
    src.push_str(&format!(".PHONY: {}\n{}:\n", names.join(" "), names.join(" ")));
    for _ in 0..50 {
        src.push_str("\techo $(A4) $(A4)\n");
    }
    let t = std::time::Instant::now();
    let entries = super::super::tasks::make_targets(&src);
    assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(entries.len(), 30, "at most MAX_PER_FILE offered");
    assert!(entries.iter().all(|e| e.body.len() <= 65 * 1024), "{}", entries.iter().map(|e| e.body.len()).max().unwrap_or(0));
}

#[test]
fn just_dependencies_and_nested_just() {
    let d = tree(&[(
        "justfile",
        "default: up\nup:\n    docker compose up -d\ndown:\n    docker compose down\nrollout: build-images push-images\n    @echo done\nbuild-images:\n    docker compose build\npush-images:\n    docker compose push\nremote-shell:\n    ssh root@hetzner\nafter: build-images && (notify \"done\")\n[private]\nnotify msg:\n    curl -X POST https://hooks.example.com -d '{{msg}}' && rsync -a logs/ ops@example.com:\nbundle:\n    just build-images\n    {{just_executable()}} push-images\n",
    )]);
    let pf = detect_checked(d.path());
    for r in ["just rollout", "just push-images", "just remote-shell", "just after", "just bundle"] {
        gated(&pf, r);
    }
    for r in ["just up", "just down", "just build-images"] {
        open(&pf, r);
    }
    assert!(!has_run(&pf, "just default"), "`default: up` repeats `just up`: {:?}", names(&pf));
}

#[test]
fn taskfile_deps_and_task_calls() {
    let d = tree(&[(
        "Taskfile.yml",
        "version: '3'\ntasks:\n  build:\n    cmds: [npm run build]\n  upload:\n    cmds: [rsync -av dist/ web@example.com:/srv/www/]\n  ship:\n    cmds:\n      - task: build\n      - task: upload\n  ship2:\n    deps: [upload]\n  ship3:\n    cmds:\n      - task build\n      - go-task upload\n  pub:\n    cmds:\n      - task: sync-internal\n  sync-internal:\n    internal: true\n    deps: [{task: upload}]\n  test: go test ./...\n",
    )]);
    let pf = detect_checked(d.path());
    for t in ["ship", "ship2", "ship3", "pub", "upload"] {
        gated(&pf, &format!("task {t}"));
    }
    open(&pf, "task build");
    open(&pf, "task test");
    assert!(!has_run(&pf, "task sync-internal"));
}

#[test]
fn python_task_sequences_refs_and_hooks() {
    let d = tree(&[
        (
            "pkg/pyproject.toml",
            "[project]\nname = \"pkg\"\nversion = \"0.1\"\n[tool.poe.tasks]\nbuild = \"python -m build\"\nupload = \"twine upload dist/*\"\nship = [\"build\", \"upload\"]\nagain = { ref = \"upload --skip-existing\" }\nseq = { sequence = [\"build\", { cmd = \"twine upload dist/*\" }] }\nboth = { shell = \"poe build && poe upload\" }\nlint = \"ruff check .\"\n",
        ),
        (
            "tool/pyproject.toml",
            "[project]\nname = \"tool\"\nversion = \"0.1\"\n[tool.pdm.scripts]\nbuild = \"python -m build\"\npost_build = \"twine upload dist/*\"\nall = { composite = [\"lint\", \"build\"] }\nlint = \"ruff check .\"\n",
        ),
    ]);
    let pf = detect_checked(d.path());
    for n in ["ship (pkg)", "again (pkg)", "seq (pkg)", "both (pkg)", "upload (pkg)", "build (tool)", "all (tool)"] {
        gated(&pf, n);
    }
    for n in ["build (pkg)", "lint (pkg)", "lint (tool)"] {
        open(&pf, n);
    }
}

#[test]
fn composer_and_deno_references() {
    let d = tree(&[
        (
            "composer.json",
            r#"{"scripts":{"build":"php bin/build.php","upload":"rsync -av public/ web@example.com:/srv/www/","ship":["@build","@upload"],"ship2":"@composer run-script upload","test":"phpunit"}}"#,
        ),
        ("app/deno.json", r#"{"tasks":{"build":"deno run -A build.ts","sync":"rsync -av dist/ web@example.com:/srv","ship":"deno task build && deno task sync","all":{"command":"echo all","dependencies":["sync"]}}}"#),
    ]);
    let pf = detect_checked(d.path());
    for n in ["composer ship", "composer ship2", "composer upload", "ship (app)", "all (app)"] {
        gated(&pf, n);
    }
    for n in ["composer build", "composer test", "build (app)"] {
        open(&pf, n);
    }
}

#[test]
fn remote_engines_compose_pushes_and_releases_reach_out() {
    for body in [
        "DOCKER_HOST=ssh://root@prod.example.com docker compose up -d",
        "DOCKER_HOST=\"ssh://root@prod\" docker ps",
        "DOCKER_HOST=tcp://10.0.0.5:2376 docker compose up -d",
        "docker --context prod compose up -d",
        "docker --context=prod ps",
        "docker -c prod compose logs",
        "docker -H ssh://ops@box ps",
        "docker context use prod",
        "docker-compose push",
        "docker compose -f docker-compose.yml -f docker-compose.prod.yml push",
        "podman compose push api",
        "docker buildx build --platform linux/amd64 --push -t ghcr.io/acme/app .",
        "gh release upload v1 dist/*",
        "gh release create v1 --generate-notes",
        "glab release create v1",
        "gh workflow run ci.yml",
        "kamal app exec 'bin/rails db:migrate'",
        "virsh -c qemu+ssh://root@host/system list",
    ] {
        assert!(reaches_out(body), "{body}");
    }
    for body in [
        "docker compose up -d",
        "docker compose build",
        "docker compose run --rm api pytest",
        "docker compose logs -f api",
        "docker run -c 512 alpine true",
        "DOCKER_HOST=unix:///run/user/1000/docker.sock docker ps",
        "DOCKER_HOST=tcp://localhost:2375 docker ps",
        "pip install git+ssh://git@github.com/acme/lib.git",
        "docker buildx build -t app:dev .",
        "gh release view v1",
        "cargo build --release",
    ] {
        assert!(!reaches_out(body), "{body}");
    }
    // A configured run (not only a detected one) that reaches out asks first.
    let r = RunConfig { name: "restart".into(), command: "docker --context prod compose up -d".into(), ..Default::default() };
    assert!(needs_confirmation(&r));
    let r = RunConfig { name: "up".into(), command: "docker compose up -d".into(), ..Default::default() };
    assert!(!needs_confirmation(&r));
}

#[test]
fn names_from_repository_files_are_one_shell_word() {
    let d = tree(&[
        ("Taskfile.yml", "version: '3'\ntasks:\n  \"lint; touch PWNED_TASK #\":\n    cmds: [echo linting]\n  \"two\\nlines\":\n    cmds: [echo x]\n"),
        ("Makefile", ".PHONY: a;touch\na;touch:\n\techo a\n"),
        ("package.json", r#"{"scripts":{"x; touch PWNED_NPM":"echo x","build:prod":"vite build"}}"#),
        ("composer.json", r#"{"scripts":{"t$(touch PWNED_PHP)":"phpunit"}}"#),
    ]);
    let pf = detect_checked(d.path());
    let task = run(&pf, "task lint; touch PWNED_TASK #");
    assert!(task.command.ends_with("task 'lint; touch PWNED_TASK #'"), "{}", task.command);
    assert_eq!(group(&pf, "task lint; touch PWNED_TASK #"), "tasks");
    assert!(!pf.runs.iter().any(|r| r.name.contains('\n')), "a name with a line break is not offered: {:?}", names(&pf));
    assert_eq!(run(&pf, "x; touch PWNED_NPM").command, "npm run 'x; touch PWNED_NPM'");
    assert_eq!(run(&pf, "build:prod").command, "npm run build:prod", "plain names stay as they are");
    assert_eq!(run(&pf, "composer t$(touch PWNED_PHP)").command, "composer run-script 't$(touch PWNED_PHP)'");
    assert_eq!(run(&pf, "make a;touch").command, "make 'a;touch'");

    // The quoted command hands the whole name to the tool and runs nothing else.
    let bin = d.path().join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    for tool in ["task", "go-task", "npm", "composer"] {
        let p = bin.join(tool);
        std::fs::write(&p, "#!/bin/sh\nprintf '%s|' \"$@\" >> args.txt\n").unwrap();
        crate::util::fs::set_mode(&p, 0o755);
    }
    for name in ["task lint; touch PWNED_TASK #", "x; touch PWNED_NPM", "composer t$(touch PWNED_PHP)"] {
        let ok = std::process::Command::new("bash")
            .arg("-c")
            .arg(&run(&pf, name).command)
            .current_dir(d.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .status()
            .unwrap()
            .success();
        assert!(ok, "{name}");
    }
    let args = std::fs::read_to_string(d.path().join("args.txt")).unwrap();
    assert_eq!(args, "lint; touch PWNED_TASK #|run|x; touch PWNED_NPM|run-script|t$(touch PWNED_PHP)|");
    for f in ["PWNED_TASK", "PWNED_NPM", "PWNED_PHP"] {
        assert!(!d.path().join(f).exists(), "{f} was created");
    }
}
