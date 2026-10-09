//! Several repositories in one project: the routes work on the repository `?repo=` names,
//! with project-relative paths, against throwaway repositories in temp directories.

use std::time::Duration;

use axum::http::Method;
use serde_json::{Value, json};

use super::tests::{app_with, commit_all, git, init_repo, init_repo_at, op_done, write};
use crate::app::AppState;
use crate::error::ApiError;
use crate::mcp::{McpCtx, call_api};

/// A project folder that is a repository itself and holds two more: `services/api` and `web`.
/// Each has one commit; the root's own files are `README.md`.
fn project_with_nested_repos() -> tempfile::TempDir {
    let d = init_repo();
    let p = d.path();
    write(p, "README.md", "root\n");
    commit_all(p, "root init");
    for (rel, file) in [("services/api", "src/lib.rs"), ("web", "index.html")] {
        let dir = p.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_at(&dir);
        write(&dir, file, "one\ntwo\n");
        commit_all(&dir, &format!("{rel} init"));
    }
    d
}

struct Fixture {
    _dir: tempfile::TempDir,
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    state: AppState,
    pid: String,
}

impl Fixture {
    async fn new(dir: tempfile::TempDir) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let state = app_with(dir.path(), tmp.path()).await;
        // `call_api` goes through the assembled router.
        let _router = crate::app::build_router(state.clone());
        let pid = state.projects.list()[0].id.clone();
        let root = state.projects.require(&pid).unwrap().root.clone();
        Self { _dir: dir, _tmp: tmp, root, state, pid }
    }

    fn data(&self, rel: &str) -> std::path::PathBuf {
        self.state.paths.data_dir.join(rel)
    }

    async fn call(&self, method: Method, route: &str, repo: Option<&str>, body: Option<Value>) -> Result<Value, ApiError> {
        let mut url = format!("/api/projects/{}/git/{route}", self.pid);
        if let Some(r) = repo {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&format!("repo={}", urlencoding::encode(r)));
        }
        call_api(&self.state, method, &url, body, &McpCtx::default()).await
    }

    async fn get(&self, route: &str, repo: Option<&str>) -> Value {
        self.call(Method::GET, route, repo, None).await.unwrap_or_else(|e| panic!("GET {route}: {} {}", e.code, e.message))
    }

    async fn post(&self, route: &str, repo: Option<&str>, body: Value) -> Value {
        self.call(Method::POST, route, repo, Some(body)).await.unwrap_or_else(|e| panic!("POST {route}: {} {}", e.code, e.message))
    }
}

fn paths(status: &Value) -> Vec<(String, Option<String>)> {
    status["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["path"].as_str().unwrap().to_string(), f["repo"].as_str().map(str::to_string)))
        .collect()
}

fn names(files: &Value) -> Vec<String> {
    files.as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect()
}

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<crate::events::Event>>, kind: &str) -> std::sync::Arc<crate::events::Event> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let e = rx.recv().await.unwrap();
            if e.kind == kind {
                return e;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {kind} event"))
}

#[tokio::test]
async fn the_project_lists_its_repositories_and_where_each_stands() {
    let d = project_with_nested_repos();
    write(&d.path().join("services/api"), "src/lib.rs", "one\nCHANGED\n");
    write(d.path(), "README.md", "root changed\n");
    git(&d.path().join("web"), &["checkout", "-q", "-b", "topic"]);
    let f = Fixture::new(d).await;

    let ids: Vec<String> = f.state.projects.require(&f.pid).unwrap().repos.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids, [".", "services/api", "web"]);

    let list = f.get("repos", None).await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 3);
    let by = |id: &str| list.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("{id}: {list:?}"));
    assert_eq!((by(".")["default"].clone(), by(".")["path"].clone(), by(".")["branch"].clone()), (json!(true), json!(""), json!("main")));
    assert_eq!((by("services/api")["default"].clone(), by("services/api")["path"].clone()), (json!(false), json!("services/api")));
    assert_eq!(by("services/api")["changed"], 1);
    assert_eq!(by(".")["changed"], 1);
    assert_eq!(by("web")["branch"], "topic");
    assert_eq!(by("web")["changed"], 0);
    assert_eq!(by("web")["state"], "clean");
    assert_eq!(by("web")["name"], "web");
    assert!(by("web").get("error").is_none());
    assert_eq!(list[0]["id"], ".", "the default repository comes first");
}

#[tokio::test]
async fn a_broken_repository_is_reported_without_hiding_the_others() {
    let d = project_with_nested_repos();
    write(d.path(), "README.md", "root changed\n");
    write(&d.path().join("web"), "index.html", "one\nedited\n");
    let f = Fixture::new(d).await;
    // `web` stops being a repository of its own (its `.git` is gone) after the project was loaded.
    std::fs::remove_dir_all(f.root.join("web/.git")).unwrap();

    let list = f.get("repos", None).await;
    let web = list.as_array().unwrap().iter().find(|r| r["id"] == "web").unwrap();
    assert_eq!(web["error"]["code"], "not_a_repo", "{web}");
    assert!(web["branch"].is_null());
    let root = &list[0];
    assert_eq!((root["id"].clone(), root["branch"].clone(), root["changed"].clone()), (json!("."), json!("main"), json!(1)));
    // Its own routes say so; the whole-project status skips it.
    let err = f.call(Method::GET, "status", Some("web"), None).await.unwrap_err();
    assert_eq!(err.code, "not_a_repo");
    let all = f.get("status", Some("all")).await;
    assert!(paths(&all).iter().all(|(_, repo)| repo.as_deref() == Some(".")), "{all}");
}

#[tokio::test]
async fn each_repository_has_its_own_status_with_project_relative_paths() {
    let d = project_with_nested_repos();
    write(&d.path().join("services/api"), "src/lib.rs", "one\nCHANGED\n");
    write(&d.path().join("services/api"), "src/new.rs", "new\n");
    write(&d.path().join("web"), "index.html", "edited\n");
    write(d.path(), "README.md", "root changed\n");
    let f = Fixture::new(d).await;

    // The root repository does not list the repositories below it as untracked folders.
    let root = f.get("status", None).await;
    assert_eq!(names(&root["files"]), ["README.md"], "{root}");
    assert!(root["files"][0].get("repo").is_none(), "a single repository's status leaves `repo` out");
    let api = f.get("status", Some("services/api")).await;
    let mut got = names(&api["files"]);
    got.sort();
    assert_eq!(got, ["services/api/src/lib.rs", "services/api/src/new.rs"]);
    assert_eq!(api["branch"], "main");
    assert_eq!(names(&f.get("status", Some("web")).await["files"]), ["web/index.html"]);

    // The whole project: every repository's files, each with its repository.
    let all = f.get("status", Some("all")).await;
    let mut files = paths(&all);
    files.sort();
    assert_eq!(
        files,
        [
            ("README.md".to_string(), Some(".".to_string())),
            ("services/api/src/lib.rs".to_string(), Some("services/api".to_string())),
            ("services/api/src/new.rs".to_string(), Some("services/api".to_string())),
            ("web/index.html".to_string(), Some("web".to_string())),
        ]
    );
    assert_eq!(all["branch"], "main", "branch, upstream and state are the default repository's");
    assert_eq!(all["truncated"], false);
    // Naming the default repository explicitly is the same as naming none.
    assert_eq!(f.get("status", Some(".")).await, root);
}

#[tokio::test(flavor = "multi_thread")]
async fn ignored_directories_above_a_nested_repository_do_not_make_its_files_ignored() {
    let d = project_with_nested_repos();
    // The usual arrangement: the root lists its clones in `.gitignore`.
    write(d.path(), ".gitignore", "services/\nweb/\nbuild/\n");
    write(d.path(), "build/out.o", "x");
    commit_all(d.path(), "ignore the clones");
    write(&d.path().join("web"), "index.html", "changed\n");
    let f = Fixture::new(d).await;
    let all = f.get("status?repo=all&ignored=true", None).await;
    let seen = paths(&all);
    // The root's own ignored folder stays; the ones that are or hold repositories do not
    // (they would paint `web/index.html` and everything below `services/` as ignored).
    assert!(seen.iter().any(|(p, r)| p == "build/" && r.as_deref() == Some(".")), "{seen:?}");
    assert!(!seen.iter().any(|(p, _)| p == "services/" || p == "web/"), "{seen:?}");
    assert!(seen.iter().any(|(p, r)| p == "web/index.html" && r.as_deref() == Some("web")), "{seen:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn staging_everything_leaves_the_other_repositories_alone() {
    let d = project_with_nested_repos();
    write(d.path(), "README.md", "changed\n");
    let f = Fixture::new(d).await;
    f.post("stage", None, json!({ "all": true })).await;
    let staged = std::process::Command::new("git").args(["ls-files", "--stage"]).current_dir(&f.root).output().unwrap();
    let staged = String::from_utf8_lossy(&staged.stdout).to_string();
    assert!(staged.contains("README.md"), "{staged}");
    assert!(!staged.contains("160000"), "a clone was staged as an embedded repository: {staged}");
    let status = f.get("status", None).await;
    assert_eq!(names(&status["files"]), ["README.md"]);
}

#[tokio::test]
async fn a_single_repository_project_answers_as_it_always_did() {
    let d = init_repo();
    write(d.path(), "a.txt", "1\n");
    commit_all(d.path(), "init");
    write(d.path(), "a.txt", "2\n");
    let f = Fixture::new(d).await;
    let plain = f.get("status", None).await;
    assert_eq!(names(&plain["files"]), ["a.txt"]);
    assert!(plain["files"][0].get("repo").is_none());
    let all = f.get("status", Some("all")).await;
    assert_eq!(paths(&all), [("a.txt".to_string(), Some(".".to_string()))]);
    let list = f.get("repos", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(f.get("branches", Some(".")).await["current"], "main");
}

#[tokio::test]
async fn staging_committing_and_branching_in_a_nested_repository() {
    let d = project_with_nested_repos();
    write(&d.path().join("services/api"), "src/lib.rs", "one\ntwo\nthree\n");
    let f = Fixture::new(d).await;
    let api = f.root.join("services/api");
    let api_repo = Some("services/api");

    f.post("stage", api_repo, json!({ "paths": ["services/api/src/lib.rs"] })).await;
    assert_eq!(git(&api, &["diff", "--cached", "--name-only"]).trim(), "src/lib.rs");
    assert!(git(&f.root, &["diff", "--cached", "--name-only"]).trim().is_empty(), "the root index is untouched");

    let diff = f.get("diff?path=services%2Fapi%2Fsrc%2Flib.rs&mode=staged", api_repo).await;
    assert_eq!(diff["path"], "services/api/src/lib.rs");
    assert!(diff["hunks"].as_array().is_some_and(|h| !h.is_empty()), "{diff}");

    let before_root = git(&f.root, &["rev-parse", "HEAD"]);
    let c = f.post("commit", api_repo, json!({ "message": "api: three" })).await;
    assert!(c["sha"].as_str().is_some_and(|s| s.len() >= 7), "{c}");
    assert_eq!(git(&api, &["log", "-1", "--format=%s"]).trim(), "api: three");
    assert_eq!(git(&f.root, &["rev-parse", "HEAD"]), before_root, "the root repository did not move");

    let log = f.get("log", api_repo).await;
    assert_eq!(log["commits"][0]["subject"], "api: three");
    let sha = log["commits"][0]["sha"].as_str().unwrap().to_string();
    let root_log = f.get("log", None).await;
    assert_eq!(root_log["commits"][0]["subject"], "root init");
    let details = f.get(&format!("commits/{sha}"), api_repo).await;
    assert_eq!(names(&details["files"]), ["services/api/src/lib.rs"], "paths are project-relative");

    // Branches and checkout are the repository's own.
    f.post("checkout", api_repo, json!({ "create": "feature" })).await;
    assert_eq!(git(&api, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "feature");
    assert_eq!(git(&f.root, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "main");
    let branches = f.get("branches", api_repo).await;
    assert_eq!(branches["current"], "feature");
    assert_eq!(f.get("branches", None).await["current"], "main");
    // Blame of the nested file.
    let blame = f.get("blame?path=services%2Fapi%2Fsrc%2Flib.rs", api_repo).await;
    assert_eq!(blame["lines"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn paths_of_another_repository_and_unknown_repositories_are_refused() {
    let d = project_with_nested_repos();
    write(&d.path().join("services/api"), "src/lib.rs", "one\nCHANGED\n");
    write(d.path(), "README.md", "root changed\n");
    let f = Fixture::new(d).await;
    let stage = |repo: Option<&'static str>, path: &'static str| {
        let f = &f;
        async move { f.call(Method::POST, "stage", repo, Some(json!({ "paths": [path] }))).await }
    };

    // A file of `services/api` through `web`, through the root repository, and the reverse.
    for (repo, path) in [(Some("web"), "services/api/src/lib.rs"), (None, "services/api/src/lib.rs"), (Some("services/api"), "README.md")] {
        let e = stage(repo, path).await.unwrap_err();
        assert_eq!(e.status.as_u16(), 400, "{repo:?} {path}: {}", e.message);
        assert!(e.message.contains("belongs to"), "{}", e.message);
    }
    assert!(git(&f.root.join("services/api"), &["diff", "--cached", "--name-only"]).trim().is_empty());
    assert!(git(&f.root, &["diff", "--cached", "--name-only"]).trim().is_empty());
    for route in ["status", "log", "branches", "repos"] {
        if route == "repos" {
            continue;
        }
        let e = f.call(Method::GET, route, Some("nope"), None).await.unwrap_err();
        assert_eq!((e.status.as_u16(), e.code), (404, "unknown_repo"), "{route}");
    }
    let e = stage(Some("nope"), "README.md").await.unwrap_err();
    assert_eq!((e.status.as_u16(), e.code), (404, "unknown_repo"));
    // The right pairs work.
    f.post("stage", Some("services/api"), json!({ "paths": ["services/api/src/lib.rs"] })).await;
    f.post("stage", None, json!({ "paths": ["README.md"] })).await;
    assert_eq!(git(&f.root, &["diff", "--cached", "--name-only"]).trim(), "README.md");
}

#[tokio::test]
async fn changelists_and_shelves_are_kept_per_repository() {
    let d = project_with_nested_repos();
    write(&d.path().join("services/api"), "src/lib.rs", "one\nCHANGED\n");
    write(d.path(), "README.md", "root changed\n");
    let f = Fixture::new(d).await;
    let api = Some("services/api");
    let pid = f.pid.clone();

    let root_lists = f.get("changelists", None).await;
    assert_eq!(root_lists["lists"][0]["files"], json!(["README.md"]));
    f.post("changelists", None, json!({ "name": "Docs", "paths": ["README.md"] })).await;
    let api_lists = f.get("changelists", api).await;
    assert_eq!(api_lists["lists"].as_array().unwrap().len(), 1, "{api_lists}");
    assert_eq!(api_lists["lists"][0]["files"], json!(["services/api/src/lib.rs"]));
    f.post("changelists", api, json!({ "name": "Backend", "paths": ["services/api/src/lib.rs"] })).await;
    let names_of = |v: &Value| v["lists"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert!(names_of(&f.get("changelists", None).await).contains(&"Docs".to_string()));
    assert!(!names_of(&f.get("changelists", None).await).contains(&"Backend".to_string()));
    assert!(names_of(&f.get("changelists", api).await).contains(&"Backend".to_string()));
    // The root repository keeps the project's own file; the other has a file of its own.
    assert!(f.data(&format!("git/changelists/{pid}.json")).is_file());
    assert!(f.data(&format!("git/changelists/{pid}@services~2Fapi.json")).is_file());

    // A path of the other repository is no member of this one's lists.
    let e = f.call(Method::POST, "changelists/move", None, Some(json!({ "paths": ["services/api/src/lib.rs"], "to": "x" }))).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 400);

    f.post("shelf", api, json!({ "name": "later", "paths": ["services/api/src/lib.rs"] })).await;
    assert_eq!(f.get("shelf", api).await.as_array().unwrap().len(), 1);
    assert!(f.get("shelf", None).await.as_array().unwrap().is_empty());
    assert!(f.data(&format!("git/shelf/{pid}@services~2Fapi")).is_dir());
    assert!(!f.data(&format!("git/shelf/{pid}")).exists());
    assert_eq!(git(&f.root.join("services/api"), &["status", "--porcelain"]).trim(), "");
    let shelf = f.get("shelf", api).await;
    let id = shelf[0]["id"].as_str().unwrap().to_string();
    let r = f.post(&format!("shelf/{id}/unshelve"), api, json!({})).await;
    assert!(r["applied"].as_array().is_some_and(|a| !a.is_empty()), "{r}");
    assert_eq!(git(&f.root.join("services/api"), &["status", "--porcelain"]).trim(), "M src/lib.rs");
}

#[tokio::test]
async fn events_and_operations_carry_their_repository() {
    let d = project_with_nested_repos();
    // `services/api` has a remote to fetch from.
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    let api = d.path().join("services/api");
    git(&api, &["remote", "add", "origin", remote.path().to_str().unwrap()]);
    git(&api, &["push", "-q", "origin", "main"]);
    write(&api, "src/lib.rs", "one\nCHANGED\n");
    let f = Fixture::new(d).await;
    let mut rx = f.state.events.subscribe();
    let api_repo = Some("services/api");

    f.post("stage", api_repo, json!({ "paths": ["services/api/src/lib.rs"] })).await;
    let ev = next_event(&mut rx, "git.changed").await;
    assert_eq!((ev.project_id.as_deref(), ev.data["repo"].clone()), (Some(f.pid.as_str()), json!("services/api")));
    f.post("changelists", api_repo, json!({ "name": "Backend" })).await;
    let ev = next_event(&mut rx, "git.changelists").await;
    assert_eq!((ev.project_id.as_deref(), ev.data["repo"].clone()), (Some(f.pid.as_str()), json!("services/api")));
    f.post("shelf", api_repo, json!({ "name": "s", "paths": ["services/api/src/lib.rs"] })).await;
    let ev = next_event(&mut rx, "git.shelf").await;
    assert_eq!(ev.data["repo"], "services/api");

    // A fetch is registered, listed and cancelled within its repository; its events say where.
    let started = f.post("fetch", api_repo, json!({ "opId": "fetch-api" })).await;
    assert_eq!(started["opId"], "fetch-api");
    let op = op_done(&f.state, "fetch-api", Duration::from_secs(30)).await;
    assert_eq!((op.ok, op.repo.as_str(), op.project_id.as_str()), (Some(true), "services/api", f.pid.as_str()), "{op:?}");
    let ev = next_event(&mut rx, "git.op").await;
    assert_eq!(ev.data["repo"], "services/api");
    assert_eq!(f.get("ops", api_repo).await.as_array().unwrap().len(), 1);
    assert!(f.get("ops", None).await.as_array().unwrap().is_empty(), "the root repository has no operations");
    let one = f.get("ops/fetch-api", None).await;
    assert_eq!(one["repo"], "services/api", "an operation is found by its id within the project");
    assert_eq!(f.get("ops/fetch-api", api_repo).await["opId"], "fetch-api");
}

#[tokio::test]
async fn a_folder_of_repositories_uses_its_first_repository_as_the_default() {
    let d = tempfile::tempdir().unwrap();
    for (name, file) in [("alpha", "a.txt"), ("beta", "b.txt")] {
        let dir = d.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        init_repo_at(&dir);
        write(&dir, file, "1\n");
        commit_all(&dir, "init");
        write(&dir, file, "2\n");
    }
    let f = Fixture::new(d).await;
    let ids: Vec<String> = f.state.projects.require(&f.pid).unwrap().repos.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids, ["alpha", "beta"]);

    // No repository named: the first one, with project-relative paths.
    assert_eq!(names(&f.get("status", None).await["files"]), ["alpha/a.txt"]);
    assert_eq!(names(&f.get("status", Some("beta")).await["files"]), ["beta/b.txt"]);
    let all = f.get("status", Some("all")).await;
    let mut files = paths(&all);
    files.sort();
    assert_eq!(files, [("alpha/a.txt".to_string(), Some("alpha".to_string())), ("beta/b.txt".to_string(), Some("beta".to_string()))]);
    let list = f.get("repos", None).await;
    assert_eq!((list[0]["id"].clone(), list[0]["default"].clone(), list[1]["default"].clone()), (json!("alpha"), json!(true), json!(false)));
    // A file of beta is not alpha's.
    let e = f.call(Method::POST, "stage", None, Some(json!({ "paths": ["beta/b.txt"] }))).await.unwrap_err();
    assert_eq!(e.status.as_u16(), 400);
    f.post("stage", Some("beta"), json!({ "paths": ["beta/b.txt"] })).await;
    assert_eq!(git(&f.root.join("beta"), &["diff", "--cached", "--name-only"]).trim(), "b.txt");
    f.post("commit", Some("beta"), json!({ "message": "beta two" })).await;
    assert_eq!(git(&f.root.join("beta"), &["log", "-1", "--format=%s"]).trim(), "beta two");
}

#[tokio::test]
async fn mcp_tools_name_the_repository() {
    let d = project_with_nested_repos();
    write(&d.path().join("services/api"), "src/lib.rs", "one\nCHANGED\n");
    let f = Fixture::new(d).await;
    let pid = f.pid.clone();
    let mut rx = f.state.events.subscribe();
    let tools = super::mcp_tools();
    let tool = |name: &str| tools.iter().find(|t| t.name == name).unwrap().handler.clone();
    let ctx = McpCtx { terminal_id: Some("t1".into()), project_id: Some(pid.clone()) };
    for t in &tools {
        assert!(t.input_schema["properties"]["repo"].is_object(), "{} takes `repo`", t.name);
    }

    (tool("workbench_set_commit_message"))(f.state.clone(), ctx.clone(), json!({ "message": "m", "repo": "services/api" })).await.unwrap();
    let ev = next_event(&mut rx, "git.commitMessage").await;
    assert_eq!((ev.data["repo"].clone(), ev.data["projectId"].clone()), (json!("services/api"), json!(pid)));

    // The diff panel: the project's id and the repository in the params, the scope id in the panel id.
    (tool("workbench_show_diff"))(f.state.clone(), ctx.clone(), json!({ "path": "services/api/src/lib.rs", "repo": "services/api" })).await.unwrap();
    let ev = next_event(&mut rx, "ui.open").await;
    assert_eq!(ev.data["panel"], "diff");
    assert_eq!(ev.data["id"], format!("diff:{pid}::services/api:working::services/api/src/lib.rs"));
    assert_eq!((ev.data["params"]["projectId"].clone(), ev.data["params"]["repo"].clone()), (json!(pid), json!("services/api")));
    // The default repository keeps the plain project id.
    (tool("workbench_show_diff"))(f.state.clone(), ctx.clone(), json!({ "sha": "HEAD" })).await.unwrap();
    let ev = next_event(&mut rx, "ui.open").await;
    assert!(ev.data["id"].as_str().unwrap().starts_with(&format!("commit:{pid}:")));
    assert_eq!(ev.data["params"]["repo"], ".");
    // A file of the repository asked for must be its own.
    let err = (tool("workbench_show_diff"))(f.state.clone(), ctx.clone(), json!({ "path": "services/api/src/lib.rs" })).await;
    assert!(err.is_err());
    let err = (tool("workbench_show_diff"))(f.state.clone(), ctx.clone(), json!({ "path": "README.md", "repo": "nope" })).await.unwrap_err();
    assert_eq!(err.code, "unknown_repo");

    f.post("changelists", Some("services/api"), json!({ "name": "Backend" })).await;
    let out = (tool("workbench_changelists"))(f.state.clone(), ctx.clone(), json!({ "repo": "services/api" })).await.unwrap();
    let crate::mcp::ToolOutput::Json(out) = out else { panic!("json") };
    assert!(out["changelists"].as_array().unwrap().iter().any(|l| l["name"] == "Backend"), "{out}");
    let out = (tool("workbench_changelists"))(f.state.clone(), ctx, json!({})).await.unwrap();
    let crate::mcp::ToolOutput::Json(out) = out else { panic!("json") };
    assert!(!out["changelists"].as_array().unwrap().iter().any(|l| l["name"] == "Backend"), "{out}");
}

#[tokio::test]
async fn a_folder_in_no_repository_has_none() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "notes.txt", "hello\n");
    let f = Fixture::new(d).await;
    assert_eq!(f.get("repos", None).await, json!([]));
    for repo in [None, Some("all")] {
        assert_eq!(f.call(Method::GET, "status", repo, None).await.unwrap_err().code, "not_a_repo");
    }
    assert_eq!(f.call(Method::GET, "status", Some("x"), None).await.unwrap_err().code, "unknown_repo");
}
