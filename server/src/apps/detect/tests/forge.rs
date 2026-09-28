//! Git remotes (GitHub, GitLab, other forges) and CI (GitHub Actions, GitLab CI).

use super::super::git::{canonical_host, config_remotes, workflow_jobs};
use super::{detect_checked, tree};

const CI_YML: &str = r#"name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
  lint:
    runs-on: ubuntu-latest
    steps: [{ run: cargo clippy }]
"#;

const RELEASE_YML: &str = r#"name: Release
on:
  push:
    tags: ["v*"]
jobs:
  build-image:
    runs-on: ubuntu-latest
    steps: [{ run: docker build . }]
  test:
    uses: ./.github/workflows/ci.yml
"#;

#[test]
fn github_remote_and_actions() {
    let d = tree(&[
        (".git/config", "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com-work:Acme/Widget.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n"),
        (".git/refs/remotes/origin/HEAD", "ref: refs/remotes/origin/trunk\n"),
        (".github/workflows/ci.yml", CI_YML),
        (".github/workflows/release.yaml", RELEASE_YML),
        (".github/dependabot.yml", "version: 2\n"),
    ]);
    let pf = detect_checked(d.path());
    let repo = pf.repo.as_ref().unwrap();
    assert_eq!((repo.remote.as_str(), repo.default_branch.as_deref()), ("origin", Some("trunk")));
    let gh = repo.github.as_ref().expect("github section");
    assert_eq!((gh.host.as_str(), gh.path.as_str()), ("github.com", "Acme/Widget"), "the ssh alias maps back to github.com");
    assert!(gh.token.is_empty(), "detection never names a secret: the global [github] token applies");
    assert!(repo.gitlab.is_none());
    let ci = repo.ci.as_ref().unwrap();
    assert_eq!((ci.provider.as_str(), ci.config.as_str()), ("github", ".github/workflows"));
    assert_eq!(ci.jobs, vec!["test", "lint", "build-image"], "job ids across workflows, each once");
    assert!(pf.project.tags.contains(&"github-actions".to_string()));
}

#[test]
fn gitlab_ci_wins_over_github_workflows() {
    let d = tree(&[
        (".git/config", "[remote \"origin\"]\n\turl = https://gitlab.example.com/group/sub/proj.git\n"),
        (".gitlab-ci.yml", "test:\n  script: [make test]\n"),
        (".github/workflows/mirror.yml", "jobs:\n  mirror:\n    runs-on: ubuntu-latest\n"),
    ]);
    let pf = detect_checked(d.path());
    let repo = pf.repo.as_ref().unwrap();
    assert_eq!(repo.gitlab.as_ref().unwrap().path, "group/sub/proj");
    assert!(repo.github.is_none());
    let ci = repo.ci.as_ref().unwrap();
    assert_eq!((ci.provider.as_str(), ci.jobs.clone()), ("gitlab", vec!["test".to_string()]));
}

#[test]
fn other_forges_have_no_forge_section() {
    for (url, tag) in [
        ("git@bitbucket.org:team/app.git", "bitbucket"),
        ("https://codeberg.org/someone/tool.git", "codeberg"),
        ("https://gitea.example.org/me/thing.git", "gitea"),
    ] {
        let d = tree(&[(".git/config", &format!("[remote \"origin\"]\n\turl = {url}\n"))]);
        let pf = detect_checked(d.path());
        let repo = pf.repo.as_ref().unwrap();
        assert!(repo.github.is_none() && repo.gitlab.is_none(), "{url}");
        assert_eq!(repo.remote, "origin");
        assert!(pf.project.tags.contains(&tag.to_string()), "{url}: {:?}", pf.project.tags);
    }
}

#[test]
fn a_clone_without_origin_uses_its_first_remote() {
    let d = tree(&[(".git/config", "[remote \"upstream\"]\n\turl = https://github.com/rust-lang/rust.git\n[remote \"fork\"]\n\turl = https://github.com/me/rust.git\n")]);
    let pf = detect_checked(d.path());
    let repo = pf.repo.as_ref().unwrap();
    assert_eq!(repo.remote, "upstream");
    assert_eq!(repo.github.as_ref().unwrap().path, "rust-lang/rust");
}

#[test]
fn remote_parsing_helpers() {
    let remotes = config_remotes("[remote \"origin\"]\n\turl = \"https://x/a.git\"\n\turl = https://x/b.git\n[remote \"b\"]\n\tfetch = x\n[remote \"c\"]\n\turl=https://x/c.git\n");
    assert_eq!(remotes, vec![("origin".to_string(), "https://x/a.git".to_string()), ("c".to_string(), "https://x/c.git".to_string())]);
    assert_eq!(canonical_host("GitHub.com"), "github.com");
    assert_eq!(canonical_host("github.com-personal"), "github.com");
    assert_eq!(canonical_host("ssh.github.com"), "github.com");
    assert_eq!(canonical_host("gitlab.com_work"), "gitlab.com");
    assert_eq!(canonical_host("github.acme.corp"), "github.acme.corp");
    assert_eq!(workflow_jobs("on: push\njobs:\n  a: {runs-on: x}\n  b:\n    runs-on: y\n"), vec!["a", "b"]);
    assert!(workflow_jobs(": not yaml :").is_empty());
}

#[test]
fn credentials_in_a_github_remote_never_enter_the_config() {
    let d = tree(&[(".git/config", "[remote \"origin\"]\n\turl = https://x-access-token:ghp_SECRETSECRET@github.com/acme/private.git\n")]);
    let pf = detect_checked(d.path());
    assert_eq!(pf.repo.as_ref().unwrap().github.as_ref().unwrap().path, "acme/private");
    assert!(!toml::to_string(&pf).unwrap().contains("ghp_"));
}
