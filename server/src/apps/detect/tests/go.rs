//! Go fixtures: main packages, server ports, air, golangci-lint, nested modules,
//! and directories that are not commands (internal/, testdata/, examples/).

use super::{detect_checked, has_run, names, run, tree};
use crate::apps::output::{ResultParser, TestResult};
use crate::config::project::RunKind;

const API_MAIN: &str = r#"package main

import (
    "log"
    "net/http"
    "os"
)

func main() {
    port := os.Getenv("PORT")
    if port == "" {
        port = "8080"
    }
    mux := http.NewServeMux()
    log.Printf("listening on :%s", port)
    log.Fatal(http.ListenAndServe(":"+port, mux))
}
"#;

#[test]
fn go_module_with_commands_air_and_lint() {
    let d = tree(&[
        ("go.mod", "module github.com/acme/widget/v2\n\ngo 1.23.2\n\nrequire github.com/go-chi/chi/v5 v5.1.0\n\ntool github.com/air-verse/air\n"),
        ("main.go", "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(\"hi\") }\n"),
        ("cmd/api/main.go", API_MAIN),
        ("cmd/api/routes.go", "package main\n"),
        ("cmd/worker/main.go", "package main\n\nfunc main() {}\n"),
        ("cmd/tools/gen/main.go", "package main\n\nfunc main() {}\n"),
        ("internal/server/server.go", "package server\n\nfunc main() {}\n"),
        ("pkg/lib/lib.go", "package lib\n"),
        ("testdata/fixture/main.go", "package main\n\nfunc main() {}\n"),
        ("examples/hello/main.go", "package main\n\nfunc main() {}\n"),
        ("examples/main.go", "package main\n\nfunc main() {}\n"),
        ("tools/go.mod", "module github.com/acme/widget/tools\n\ngo 1.23\n"),
        ("tools/main.go", "package main\n\nfunc main() {}\n"),
        (".air.toml", "root = \".\"\n[build]\n  cmd = \"go build -o ./tmp/main ./cmd/api\"\n  bin = \"./tmp/main\"\n"),
        (".golangci.yml", "linters:\n  enable: [govet]\n"),
    ]);
    let pf = detect_checked(d.path());
    let t = run(&pf, "go test");
    assert_eq!((t.kind, t.command.as_str()), (RunKind::Test, "go test ./..."));
    assert_eq!(t.result_pattern.as_deref(), Some(super::super::GO_TEST_RESULT));
    assert_eq!(run(&pf, "go build").kind, RunKind::Build);
    assert_eq!(run(&pf, "go vet").command, "go vet ./...");
    let root = run(&pf, "widget");
    assert_eq!((root.kind, root.command.as_str()), (RunKind::Task, "go run ."), "the module's base name, not `v2`");
    let api = run(&pf, "api");
    assert_eq!((api.kind, api.command.as_str(), api.port), (RunKind::Server, "go run ./cmd/api", Some(8080)));
    assert_eq!(api.source.as_deref(), Some("detected:cmd/api/main.go"));
    assert_eq!(run(&pf, "worker").kind, RunKind::Task);
    assert_eq!(run(&pf, "gen").command, "go run ./cmd/tools/gen");
    let air = run(&pf, "air");
    assert_eq!((air.command.as_str(), air.port), ("go tool air", Some(8080)), "air builds ./cmd/api");
    assert_eq!(run(&pf, "golangci-lint").command, "golangci-lint run");
    // The nested module is its own component with its own runs.
    assert_eq!(run(&pf, "go test (tools)").cwd, "tools");
    assert_eq!(run(&pf, "tools (tools)").command, "go run .", "named after its module path");
    for absent in ["server", "lib", "fixture", "hello", "examples"] {
        assert!(!has_run(&pf, absent), "{absent}: {:?}", names(&pf));
    }
    let go: Vec<(&str, Option<&str>)> = pf.components.iter().filter(|c| c.kind == "go").map(|c| (c.path.as_str(), c.version.as_deref())).collect();
    assert_eq!(go, vec![(".", Some("1.23.2")), ("tools", Some("1.23"))]);
}

#[test]
fn go_server_ports() {
    use super::super::go::detect_port;
    assert_eq!(detect_port(r#"r := gin.Default(); r.Run(":9090")"#), Some(9090));
    assert_eq!(detect_port(r#"srv := &http.Server{Addr: "127.0.0.1:7070", Handler: h}"#), Some(7070));
    assert_eq!(detect_port(r#"app := fiber.New(); app.Listen(":3000")"#), Some(3000));
    assert_eq!(detect_port(r#"addr := flag.String("addr", ":8081", "listen address")"#), Some(8081));
    assert_eq!(detect_port(r#"port := flag.Int("port", 4000, "port")"#), Some(4000));
    assert_eq!(detect_port(r#"port := cmp.Or(os.Getenv("PORT"), "3001")"#), Some(3001));
    assert_eq!(detect_port(r#"lis, _ := net.Listen("tcp", ":50051")"#), Some(50051));
    assert_eq!(detect_port("http.ListenAndServe(addr, nil)"), None);
}

#[test]
fn go_test_output_per_package() {
    let p = ResultParser::new(super::super::GO_TEST_RESULT).unwrap();
    let mut acc = TestResult::default();
    for l in [
        "ok  \tgithub.com/acme/widget/internal/api\t0.012s",
        "--- FAIL: TestParse (0.00s)",
        "FAIL\tgithub.com/acme/widget/internal/parse\t0.004s",
        "ok  \tgithub.com/acme/widget/internal/store\t(cached)",
        "?   \tgithub.com/acme/widget/cmd/api\t[no test files]",
        "FAIL",
    ] {
        p.feed(l, &mut acc);
    }
    assert_eq!((acc.passed, acc.failed), (2, 1));
    assert_eq!(acc.items[1].name, "github.com/acme/widget/internal/parse");
}
