//! docker compose and Dockerfile fixtures: local stacks, overlays, and compose
//! files that describe a remote host (skipped).

use super::super::compose::{published_port, services};
use super::{detect_checked, has_run, names, run, tree};
use crate::config::project::RunKind;

const LOCAL_STACK: &str = r#"services:
  web:
    build: .
    ports:
      - "3000:3000"
    depends_on: [db, cache]
  db:
    image: postgres:16
    ports: ["5432:5432"]
    environment:
      POSTGRES_PASSWORD: dev
  cache:
    image: redis:7
  mailpit:
    image: axllent/mailpit
    ports:
      - target: 8025
        published: "8025"
    profiles: [mail]
"#;

#[test]
fn local_compose_stack() {
    let d = tree(&[("docker-compose.yml", LOCAL_STACK), ("Dockerfile", "FROM node:22\nCOPY . .\n"), ("package.json", "{}")]);
    let pf = detect_checked(d.path());
    let up = run(&pf, "compose up");
    assert!(up.command.ends_with("compose up --build"), "{}", up.command);
    assert_eq!((up.kind, up.port, up.preview.as_deref()), (RunKind::Server, Some(3000), Some("http://localhost:3000/")));
    assert_eq!(up.source.as_deref(), Some("detected:docker-compose.yml"));
    assert!(run(&pf, "compose build").command.ends_with("compose build"));
    let logs = run(&pf, "compose logs web");
    assert!(logs.command.ends_with("compose logs -f --tail 200 web"), "{}", logs.command);
    assert!(has_run(&pf, "compose logs db") && has_run(&pf, "compose logs mailpit"));
    assert!(!pf.runs.iter().any(|r| r.name.ends_with(" build") && r.command.contains(" build -t")), "compose builds the Dockerfile: {:?}", names(&pf));
    assert!(pf.components.iter().any(|c| c.kind == "compose"));
    assert!(pf.project.tags.contains(&"docker".to_string()));
}

#[test]
fn compose_port_syntax() {
    assert_eq!(published_port("8080:80"), Some(8080));
    assert_eq!(published_port("127.0.0.1:8080:8080"), Some(8080));
    assert_eq!(published_port("${WEB_PORT:-8081}:80"), Some(8081));
    assert_eq!(published_port("${WEB_PORT}:80"), None);
    assert_eq!(published_port("9000-9001:9000-9001"), Some(9000));
    assert_eq!(published_port("[::1]:5000:5000/tcp"), Some(5000));
    assert_eq!(published_port("53:53/udp"), Some(53));
    assert_eq!(published_port("80"), None, "a container port alone gets a random host port");
    let s = services(LOCAL_STACK, std::path::Path::new("/p"));
    assert_eq!(s.iter().map(|x| (x.name.as_str(), x.ports.clone(), x.profiled)).collect::<Vec<_>>(), vec![
        ("web", vec![3000], false),
        ("db", vec![5432], false),
        ("cache", vec![], false),
        ("mailpit", vec![8025], true),
    ]);
    assert_eq!(s[0].build.as_deref(), Some(std::path::Path::new("/p")));
}

#[test]
fn main_port_skips_databases_and_prefers_the_front_door() {
    let d = tree(&[(
        "compose.yaml",
        "services:\n  postgres:\n    image: postgres\n    ports: ['5433:5432']\n  backend:\n    image: acme/api\n    ports: ['8001:8000']\n  nginx:\n    image: nginx\n    ports: ['8080:80']\n",
    )]);
    let pf = detect_checked(d.path());
    let up = run(&pf, "compose up");
    assert_eq!(up.port, Some(8080));
    assert!(!up.command.contains("--build"));
    assert!(!has_run(&pf, "compose build"));
}

#[test]
fn overlay_and_standalone_variants() {
    let d = tree(&[
        ("docker-compose.yml", "services:\n  api:\n    image: acme/api\n  db:\n    image: postgres\n"),
        ("docker-compose.override.yml", "services:\n  api:\n    ports: ['8000:8000']\n"),
        ("docker-compose.dev.yml", "services:\n  api:\n    build: ./api\n    ports: ['8000:8000', '5678:5678']\n"),
        ("docker-compose.test.yml", "services:\n  tests:\n    build: .\n    command: pytest\n"),
        ("docker-compose.prod.yml", "services:\n  api:\n    image: registry.example.com/api:latest\n    ports: ['80:8000']\n"),
    ]);
    let pf = detect_checked(d.path());
    assert_eq!(run(&pf, "compose up").port, Some(8000), "the override file's ports count");
    let dev = run(&pf, "compose up · dev");
    assert!(dev.command.ends_with("compose -f docker-compose.yml -f docker-compose.dev.yml up --build"), "{}", dev.command);
    let test = run(&pf, "compose up · test");
    assert!(test.command.ends_with("compose -f docker-compose.test.yml up --build"), "{}", test.command);
    assert!(!pf.runs.iter().any(|r| r.name.contains("prod")), "production variant: {:?}", names(&pf));
}

/// A common layout: the root compose file is what `deploy/deploy.sh` runs on
/// the server (`docker compose up` without -f), and `deploy/staging/` holds
/// another one. Neither is a local stack; the Dockerfile is still a local build.
#[test]
fn compose_files_that_describe_a_server_are_skipped() {
    let d = tree(&[
        ("docker-compose.yml", "services:\n  api:\n    image: registry.gitlab.com/acme/shop:latest\n    build: .\n    ports: ['127.0.0.1:8080:8080']\n    env_file: [server.env]\n"),
        ("deploy/staging/docker-compose.yml", "services:\n  api:\n    image: x\n    ports: ['127.0.0.1:8081:8080']\n"),
        ("deploy/deploy.sh", "#!/bin/bash\nDIR=/opt/shop\ncd \"$DIR\"\ndocker compose --env-file server.env up -d --force-recreate api\n"),
        ("Dockerfile", "FROM rust:1 AS build\nCOPY . .\nFROM debian\nCOPY --from=build /app /app\n"),
    ]);
    let pf = detect_checked(d.path());
    assert!(!pf.runs.iter().any(|r| r.name.starts_with("compose")), "{:?}", names(&pf));
    let b = image_build(&pf, "");
    assert!(b.command.contains(" build -t ") && b.command.ends_with(":dev ."), "{}", b.command);
    assert_eq!((b.kind, b.cwd.as_str()), (RunKind::Build, "."));

    // Named by a deploy script (scp'd to the host).
    let d = tree(&[
        ("compose.yml", "services:\n  web:\n    image: nginx\n    ports: ['80:80']\n"),
        ("scripts/deploy-prod.sh", "scp compose.yml root@203.0.113.9:/srv/app/\n"),
    ]);
    assert!(!detect_checked(d.path()).runs.iter().any(|r| r.name.starts_with("compose")));

    // A deploy script that ships its own compose file leaves the root's local one alone.
    let d = tree(&[
        ("docker-compose.yml", "services:\n  db:\n    image: postgres\n    ports: ['5432:5432']\n"),
        ("deploy/docker-compose.yml", "services:\n  app:\n    image: acme/app\n"),
        ("deploy/deploy.sh", "cd /srv/app && docker compose pull && docker compose up -d\n"),
    ]);
    let pf = detect_checked(d.path());
    let up = run(&pf, "compose up");
    assert_eq!((up.port, up.preview.as_deref()), (None, None), "a database port is not a page to preview");
}

/// `docker build` or `podman build` (whichever engine this machine has) for the
/// Dockerfile in `dir` (`""` for the root).
fn image_build<'a>(pf: &'a crate::config::ProjectFile, dir: &str) -> &'a crate::config::project::RunConfig {
    let want = |engine: &str| if dir.is_empty() { format!("{engine} build") } else { format!("{engine} build ({dir})") };
    pf.runs
        .iter()
        .find(|r| r.name == want("docker") || r.name == want("podman"))
        .unwrap_or_else(|| panic!("no image build for {dir:?}: {:?}", names(pf)))
}

/// full-stack-fastapi-template's layout: `compose.yml` + `compose.override.yml` for
/// local development, and a deploy script that stacks `compose.traefik.yml` on the
/// same base for production.
#[test]
fn shared_base_with_a_production_overlay() {
    let d = tree(&[
        ("compose.yml", "services:\n  db:\n    image: postgres\n  backend:\n    build:\n      context: ./backend\n  frontend:\n    build:\n      context: ./frontend\n      dockerfile: docker/Dockerfile.dev\n"),
        ("compose.override.yml", "services:\n  backend:\n    ports: ['8000:8000']\n  frontend:\n    ports: ['5173:80']\n"),
        ("compose.traefik.yml", "services:\n  traefik:\n    image: traefik:3\n    ports: ['80:80', '443:443']\n"),
        ("scripts/deploy.sh", "docker compose -f compose.yml -f compose.traefik.yml config > docker-stack.yml\ndocker stack deploy -c docker-stack.yml app\n"),
        ("backend/Dockerfile", "FROM python:3.12\n"),
        ("frontend/docker/Dockerfile.dev", "FROM node:22\n"),
        ("frontend/Dockerfile", "FROM node:22 AS prod\n"),
    ]);
    let pf = detect_checked(d.path());
    let up = run(&pf, "compose up");
    assert_eq!(up.port, Some(5173), "the override's ports; `frontend` is a front door");
    assert!(up.command.ends_with("compose up --build"), "{}", up.command);
    assert!(!pf.runs.iter().any(|r| r.name.contains("traefik")), "the production overlay: {:?}", names(&pf));
    // backend/Dockerfile is built by compose; frontend builds docker/Dockerfile.dev,
    // so its production Dockerfile is a build of its own.
    assert!(!pf.runs.iter().any(|r| r.name.ends_with("build (backend)")), "{:?}", names(&pf));
    assert_eq!(image_build(&pf, "frontend").cwd, "frontend");
}

#[test]
fn dockerfile_contexts() {
    let d = tree(&[
        ("services/api/Dockerfile", "FROM golang:1.23\nWORKDIR /src\nCOPY go.mod go.sum ./\nCOPY services/api ./services/api\n"),
        ("go.mod", "module x\n"),
        ("go.sum", ""),
        ("services/web/Dockerfile", "FROM node:22\nCOPY package.json ./\n"),
        ("services/web/package.json", "{}"),
        ("examples/demo/Dockerfile", "FROM alpine\n"),
    ]);
    let pf = detect_checked(d.path());
    let api = image_build(&pf, "services/api");
    assert_eq!(api.cwd, ".", "COPY paths resolve from the repository root");
    assert!(api.command.contains(" build -f services/api/Dockerfile -t api:dev ."), "{}", api.command);
    let web = image_build(&pf, "services/web");
    assert_eq!(web.cwd, "services/web");
    assert!(web.command.contains(" build -t web:dev ."), "{}", web.command);
    assert!(!pf.runs.iter().any(|r| r.name.contains("examples")), "{:?}", names(&pf));
}
