//! Updates against a local stand-in for the GitHub API: the latest release, its assets
//! behind a redirect as GitHub serves them, and a "program" that is a shell script
//! answering `--version`. Nothing here talks to GitHub.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::State as AxState;
use axum::http::{HeaderMap, Request, Uri, header};
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use tower::ServiceExt;

use super::*;
use crate::mcp::{self, McpCtx};
use crate::platform::testutil;

const REPO: &str = "acme/workbench";
const LINUX: Option<(&str, &str)> = Some(("x86_64-unknown-linux-gnu", "tar.gz"));

fn source(api: &str) -> Source {
    Source { api: api.to_string(), repo: REPO.to_string() }
}

fn cfg(repo: Option<&str>, api: Option<&str>) -> UpdateConfig {
    UpdateConfig { check: true, repo: repo.map(str::to_string), api: api.map(str::to_string) }
}

// ---------------------------------------------------------------- pure parts

#[test]
fn versions_are_three_numbers_and_order_by_them() {
    assert_eq!(Version::parse("v0.5.3"), Some(Version(0, 5, 3)));
    assert_eq!(Version::parse(" 1.20.0 "), Some(Version(1, 20, 0)));
    for bad in ["", "v", "1.2", "1.2.3.4", "1.2.x", "1.2.3-rc1", "1..3", "vv1.2.3", "+1.2.3", "1.2.99999999999", "1.2.3."] {
        assert_eq!(Version::parse(bad), None, "{bad:?}");
    }
    assert!(Version(0, 10, 0) > Version(0, 9, 9) && Version(1, 0, 0) > Version(0, 99, 99) && Version(0, 5, 10) > Version(0, 5, 9));
    assert_eq!(Version(0, 5, 3).to_string(), "0.5.3");
    assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_build_without_a_repository_has_no_source_until_config_names_one() {
    assert_eq!(Source::resolve(&UpdateConfig::default(), None), Ok(None));
    assert_eq!(Source::resolve(&UpdateConfig::default(), Some("")), Ok(None));
    // The release workflow's repository, which config.toml can replace.
    let built = Source::resolve(&UpdateConfig::default(), Some("octo-org/workbench")).unwrap().unwrap();
    assert_eq!((built.api.as_str(), built.repo.as_str()), ("https://api.github.com", "octo-org/workbench"));
    let named = Source::resolve(&cfg(Some(" acme/workbench "), Some("https://ghe.example.com/api/v3/")), Some("octo-org/workbench")).unwrap().unwrap();
    assert_eq!((named.api.as_str(), named.repo.as_str()), ("https://ghe.example.com/api/v3", "acme/workbench"));
    assert_eq!(named.asset_prefix(), "https://ghe.example.com/api/v3/repos/acme/workbench/releases/assets/");

    for repo in ["workbench", "a/b/c", "../x", "a/..", "acme/work bench", "https://github.com/acme/workbench", "acme/"] {
        let e = Source::resolve(&cfg(Some(repo), None), None).unwrap_err();
        assert!(e.contains("update.repo") && e.contains("owner/name"), "{repo}: {e}");
    }
    // What comes from there replaces the program: https, except on this computer.
    for api in ["http://ghe.example.com/api/v3", "ftp://example.com", "https://user:pw@example.com", "https://example.com/?x=1", "example.com", "http://127.0.0.1.example.com"] {
        assert!(Source::resolve(&cfg(Some(REPO), Some(api)), None).unwrap_err().contains("update.api"), "{api}");
    }
    for api in ["http://127.0.0.1:8080", "http://localhost:8080/", "http://[::1]:8080"] {
        assert!(Source::resolve(&cfg(Some(REPO), Some(api)), None).unwrap().unwrap().insecure(), "{api}");
    }
}

fn api_release(api: &str, tag: &str, assets: &[(&str, &str)]) -> ApiRelease {
    serde_json::from_value(json!({
        "tag_name": tag,
        "body": "- **Updates:** Workbench updates itself.\n",
        "html_url": format!("https://github.com/{REPO}/releases/tag/{tag}"),
        "published_at": "2026-10-02T10:00:00Z",
        "assets": assets.iter().map(|(name, url)| json!({ "name": name, "url": format!("{api}{url}"), "size": 10, "digest": null })).collect::<Vec<_>>(),
    }))
    .unwrap()
}

#[test]
fn a_release_offers_this_platforms_archive_and_only_from_its_own_assets() {
    let api = "https://api.github.com";
    let s = source(api);
    let tgz = "workbench-1.2.3-x86_64-unknown-linux-gnu.tar.gz";
    let zip = "workbench-1.2.3-x86_64-pc-windows-msvc.zip";
    let assets = [
        (zip, "/repos/acme/workbench/releases/assets/7"),
        (tgz, "/repos/acme/workbench/releases/assets/8"),
        ("workbench-1.2.3-x86_64-unknown-linux-gnu.tar.gz.sha256", "/repos/acme/workbench/releases/assets/9"),
    ];
    let r = Release::from_api(&s, api_release(api, "v1.2.3", &assets), LINUX).unwrap();
    assert_eq!(r.version, Version(1, 2, 3));
    let a = r.archive.clone().unwrap();
    assert_eq!((a.name.as_str(), a.url.as_str()), (tgz, "https://api.github.com/repos/acme/workbench/releases/assets/8"));
    assert_eq!(a.checksum_url.as_deref(), Some("https://api.github.com/repos/acme/workbench/releases/assets/9"));
    assert_eq!(r.info.url.as_deref(), Some("https://github.com/acme/workbench/releases/tag/v1.2.3"));
    assert_eq!(r.info.published_at, Some(chrono::DateTime::parse_from_rfc3339("2026-10-02T10:00:00Z").unwrap().timestamp_millis()));
    assert!(r.info.archive && r.info.notes.starts_with("- **Updates:**"));

    // Windows gets the zip (without a checksum file here); a platform with no build, nothing.
    let win = Release::from_api(&s, api_release(api, "v1.2.3", &assets), Some(("x86_64-pc-windows-msvc", "zip"))).unwrap();
    assert_eq!(win.archive.as_ref().map(|a| (a.name.as_str(), a.checksum_url.is_some())), Some((zip, false)));
    let none = Release::from_api(&s, api_release(api, "v1.2.3", &assets), None).unwrap();
    assert!(none.archive.is_none() && !none.info.archive);
    // A release that has not been built for this platform yet.
    assert!(Release::from_api(&s, api_release(api, "v1.2.3", &assets[..1]), LINUX).unwrap().archive.is_none());

    // The archive of another version, host, repository or path is not this release's.
    for url in ["https://evil.example.com/repos/acme/workbench/releases/assets/8", "/repos/other/workbench/releases/assets/8", "/repos/acme/workbench/releases/assets/8/../../x", "/repos/acme/workbench/releases/assets/"] {
        let url = if url.starts_with("https://") { url.to_string() } else { format!("{api}{url}") };
        let mut rel = api_release(api, "v1.2.3", &assets);
        rel.assets[1].url = url.clone();
        assert!(Release::from_api(&s, rel, LINUX).unwrap_err().contains("not served by"), "{url}");
    }
    let mut rel = api_release(api, "v1.2.3", &assets);
    rel.assets[2].url = "https://evil.example.com/sum".into();
    assert!(Release::from_api(&s, rel, LINUX).is_err(), "a checksum from elsewhere");

    // Tags that are not versions, and what `latest` never returns.
    for tag in ["nightly", "v1.2", "v1.2.3-rc1"] {
        assert!(Release::from_api(&s, api_release(api, tag, &assets), LINUX).unwrap_err().contains("not a version"), "{tag}");
    }
    let mut pre = api_release(api, "v1.2.3", &assets);
    pre.prerelease = true;
    assert!(Release::from_api(&s, pre, LINUX).unwrap_err().contains("pre-release"));

    // Notes are cut, a page that is not https is dropped, GitHub's digest is kept.
    let mut odd = api_release(api, "v1.2.3", &assets);
    odd.body = Some("é".repeat(MAX_NOTES_CHARS + 50));
    odd.html_url = Some("javascript:alert(1)".into());
    odd.assets[1].digest = Some(format!("sha256:{}", "AB".repeat(32)));
    let odd = Release::from_api(&s, odd, LINUX).unwrap();
    assert_eq!(odd.info.notes.chars().count(), MAX_NOTES_CHARS);
    assert_eq!(odd.info.url, None);
    assert_eq!(odd.archive.unwrap().digest, Some("ab".repeat(32)));
}

#[test]
fn a_checksum_line_must_be_a_sha256_of_that_archive() {
    let hex = "0123456789abcdef".repeat(4);
    assert_eq!(checksum_for(&format!("{hex}  a.tar.gz\n"), "a.tar.gz"), Some(hex.clone()));
    assert_eq!(checksum_for(&format!("{} *a.tar.gz", hex.to_uppercase()), "a.tar.gz"), Some(hex.clone()));
    assert_eq!(checksum_for(&hex, "a.tar.gz"), Some(hex.clone()));
    assert_eq!(checksum_for(&format!("{hex}  b.tar.gz\n"), "a.tar.gz"), None);
    assert_eq!(checksum_for(&format!("{}  a.tar.gz", &hex[..40]), "a.tar.gz"), None);
    assert_eq!(checksum_for("<html>Not Found</html>", "a.tar.gz"), None);
    assert_eq!(checksum_for("", "a.tar.gz"), None);
}

#[test]
fn the_daily_look_waits_a_day_or_an_hour_after_a_failure() {
    let s = source("https://api.github.com");
    let now = 10 * 24 * 3600 * 1000;
    let checked = |ago_min: i64, error: Option<&str>, key: String| Checked { at: now - ago_min * 60_000, source: key, latest: None, error: error.map(str::to_string) };
    assert!(due(None, &s, now));
    assert!(!due(Some(&checked(23 * 60, None, s.key())), &s, now));
    assert!(due(Some(&checked(24 * 60, None, s.key())), &s, now));
    assert!(!due(Some(&checked(59, Some("offline"), s.key())), &s, now));
    assert!(due(Some(&checked(60, Some("offline"), s.key())), &s, now));
    // Another repository's answer says nothing about this one; a clock set back looks again.
    assert!(due(Some(&checked(1, None, "https://api.github.com other/repo".into())), &s, now));
    assert!(due(Some(&checked(-5, None, s.key())), &s, now));
}

// ---------------------------------------------------------------- archives

const STEM: &str = "workbench-99.0.0-x86_64-unknown-linux-gnu";

fn program(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho \"workbench {version}\"\n").into_bytes()
}

/// A release archive as the workflow makes it, plus entries that must never be written.
fn archive_with(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, kind, data) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(*kind);
        h.set_mode(0o755);
        h.set_size(if *kind == tar::EntryType::Regular { data.len() as u64 } else { 0 });
        if *kind == tar::EntryType::Symlink {
            h.set_link_name(String::from_utf8_lossy(data).as_ref()).unwrap();
        }
        // Not `append_data`: it refuses the `..` paths this is here to try.
        h.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_cksum();
        tar.append(&h, if *kind == tar::EntryType::Regular { *data } else { &[][..] }).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn archive(version: &str) -> Vec<u8> {
    let (bin, readme) = (program(version), b"readme".to_vec());
    archive_with(&[
        (&format!("{STEM}/install.sh"), tar::EntryType::Regular, b"#!/bin/sh\nexit 1\n"),
        ("../escape", tar::EntryType::Regular, b"never written"),
        (&format!("{STEM}/README.md"), tar::EntryType::Regular, &readme),
        (&format!("{STEM}/workbench"), tar::EntryType::Regular, &bin),
    ])
}

fn sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

#[test]
fn only_the_program_is_taken_out_of_the_archive() {
    let dir = tempfile::tempdir().unwrap();
    let inner = dir.path().join("bin");
    std::fs::create_dir(&inner).unwrap();
    let (file, exe, tmp) = (dir.path().join("a.tar.gz"), inner.join("workbench"), inner.join(".workbench.new"));
    std::fs::write(&exe, b"old").unwrap();
    perm::apply(&exe, 0o755).unwrap();
    std::fs::write(&file, archive("99.0.0")).unwrap();
    let member = format!("{STEM}/workbench");

    extract_binary(&file, &member, &tmp, &exe).unwrap();
    assert_eq!(std::fs::read(&tmp).unwrap(), program("99.0.0"));
    perm::assert_mode(&tmp, 0o755);
    assert_eq!(std::fs::read(&exe).unwrap(), b"old", "the running program is not touched by unpacking");
    // Nothing else left the archive: not beside the program, not above it.
    let names = |d: &Path| std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names(&inner), [".workbench.new".to_string(), "workbench".to_string()].into());
    assert_eq!(names(dir.path()), ["a.tar.gz".to_string(), "bin".to_string()].into());

    // A link or a folder under the program's name is not a program; nor is nothing.
    std::fs::remove_file(&tmp).unwrap();
    for (kind, data) in [(tar::EntryType::Symlink, &b"/etc/passwd"[..]), (tar::EntryType::Directory, &b""[..]), (tar::EntryType::Regular, &b""[..])] {
        std::fs::write(&file, archive_with(&[(&member, kind, data)])).unwrap();
        assert!(extract_binary(&file, &member, &tmp, &exe).unwrap_err().contains("not a program file"), "{kind:?}");
        assert!(!tmp.exists());
    }
    std::fs::write(&file, archive_with(&[(&format!("{STEM}/README.md"), tar::EntryType::Regular, b"x")])).unwrap();
    assert!(extract_binary(&file, &member, &tmp, &exe).unwrap_err().contains("has no"));
    std::fs::write(&file, b"not gzip at all").unwrap();
    assert!(extract_binary(&file, &member, &tmp, &exe).unwrap_err().contains("cannot be read"));
    // Cut in the middle of the program.
    let whole = archive_with(&[(&member, tar::EntryType::Regular, &vec![7u8; 200_000])]);
    std::fs::write(&file, &whole[..whole.len() / 2]).unwrap();
    assert!(extract_binary(&file, &member, &tmp, &exe).is_err());
}

// ---------------------------------------------------------------- a stand-in for GitHub

#[derive(Clone)]
struct Mock {
    base: Arc<Mutex<String>>,
    /// What `releases/latest` answers (status, body).
    release: Arc<Mutex<(u16, Value)>>,
    archive: Arc<Mutex<Vec<u8>>>,
    checksum: Arc<Mutex<String>>,
    /// Where asset 1 redirects; `None`: the stand-in's own storage.
    redirect: Arc<Mutex<Option<String>>>,
    /// Every request: path and whether it carried credentials.
    seen: Arc<Mutex<Vec<(String, bool, String)>>>,
}

impl Mock {
    fn paths(&self) -> Vec<String> {
        self.seen.lock().iter().map(|(p, _, _)| p.clone()).collect()
    }
}

async fn mock_handler(AxState(m): AxState<Mock>, uri: Uri, headers: HeaderMap) -> Response {
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    m.seen.lock().push((uri.path().to_string(), headers.contains_key(header::AUTHORIZATION) || headers.contains_key(header::COOKIE), accept.clone()));
    let base = m.base.lock().clone();
    match uri.path() {
        "/repos/acme/workbench/releases/latest" => {
            let (status, body) = m.release.lock().clone();
            (StatusCode::from_u16(status).unwrap(), Json(body)).into_response()
        }
        "/repos/acme/workbench/releases/assets/1" if accept == "application/octet-stream" => {
            let to = m.redirect.lock().clone().unwrap_or_else(|| format!("{base}/storage/archive"));
            (StatusCode::FOUND, [(header::LOCATION, to)]).into_response()
        }
        "/repos/acme/workbench/releases/assets/2" if accept == "application/octet-stream" => m.checksum.lock().clone().into_response(),
        "/storage/archive" => m.archive.lock().clone().into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn release_json(base: &str, version: &str, size: usize) -> Value {
    let name = format!("workbench-{version}-x86_64-unknown-linux-gnu.tar.gz");
    json!({
        "tag_name": format!("v{version}"),
        "body": "## What is new\n\n- Workbench updates itself.",
        "html_url": format!("https://github.com/{REPO}/releases/tag/v{version}"),
        "published_at": "2026-10-02T10:00:00Z",
        "draft": false,
        "prerelease": false,
        "assets": [
            { "name": name, "url": format!("{base}/repos/acme/workbench/releases/assets/1"), "size": size },
            { "name": format!("{name}.sha256"), "url": format!("{base}/repos/acme/workbench/releases/assets/2"), "size": 100 },
        ],
    })
}

/// A stand-in offering 99.0.0, a correct archive and its checksum.
async fn mock() -> Mock {
    let data = archive("99.0.0");
    let m = Mock {
        base: Default::default(),
        release: Arc::new(Mutex::new((200, Value::Null))),
        checksum: Arc::new(Mutex::new(format!("{}  {STEM}.tar.gz\n", sha256(&data)))),
        archive: Arc::new(Mutex::new(data)),
        redirect: Default::default(),
        seen: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(mock_handler).with_state(m.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    *m.release.lock() = (200, release_json(&base, "99.0.0", m.archive.lock().len()));
    *m.base.lock() = base;
    m
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    v.sort();
    v
}

/// Installing for real: a program is a shell script here, and archives exist for this target.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod install {
    use super::*;

    /// A folder with an installed "program" (the text `old`) and a work folder.
    struct Installation {
        _dir: tempfile::TempDir,
        exe: PathBuf,
        work: PathBuf,
    }

    fn installation() -> Installation {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let exe = bin.join("workbench");
        std::fs::write(&exe, b"old").unwrap();
        perm::apply(&exe, 0o755).unwrap();
        Installation { work: dir.path().join("data/update"), exe, _dir: dir }
    }

    /// Look and install as `workbench update` does; the steps seen on the way.
    async fn run(m: &Mock, at: &Installation) -> (Result<Installed, String>, Vec<String>) {
        let s = source(&m.base.lock());
        let c = client(&s).unwrap();
        let release = match latest(&s, &c).await {
            Ok(r) => r,
            Err(e) => return (Err(e), vec![]),
        };
        let steps = Mutex::new(Vec::new());
        let on = |step: Step| {
            steps.lock().push(match step {
                Step::Download { received, total } => format!("download {received}/{total}"),
                Step::Verify => "verify".to_string(),
                Step::Install => "install".to_string(),
            })
        };
        let result = install_release(&c, &release, &at.exe, &at.work, &on).await;
        (result, steps.into_inner())
    }

    /// The status once the install that was started is over.
    async fn settled(app: &testutil::TestApp) -> Value {
        for _ in 0..400 {
            let s = status_of(app).await;
            if s["phase"] == "idle" {
                return s;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the install did not finish: {}", status_of(app).await);
    }

    #[tokio::test]
    async fn a_release_is_downloaded_checked_and_put_in_the_programs_place() {
        let (m, at) = (mock().await, installation());
        let size = m.archive.lock().len();
        let (result, steps) = run(&m, &at).await;
        let installed = result.unwrap();
        assert_eq!(installed.version, Version(99, 0, 0));
        assert_eq!(std::fs::read(&at.exe).unwrap(), program("99.0.0"));
        perm::assert_mode(&at.exe, 0o755);
        // The replaced one is kept beside it; the download and the staging files are gone.
        let previous = installed.previous.unwrap();
        assert_eq!((previous.file_name().unwrap().to_str(), std::fs::read(&previous).unwrap().as_slice()), (Some("workbench.prev"), &b"old"[..]));
        assert_eq!(files_in(at.exe.parent().unwrap()), ["workbench", "workbench.prev"]);
        assert!(!at.work.exists(), "the download folder is removed");
        assert_eq!(steps.first().map(String::as_str), Some(format!("download 0/{size}").as_str()));
        assert_eq!(&steps[steps.len() - 3..], [format!("download {size}/{size}"), "verify".to_string(), "install".to_string()]);

        // The download followed the API's redirect to its storage, and nothing carried credentials.
        assert_eq!(m.paths(), ["/repos/acme/workbench/releases/latest", "/repos/acme/workbench/releases/assets/1", "/storage/archive", "/repos/acme/workbench/releases/assets/2"]);
        assert!(m.seen.lock().iter().all(|(_, credentials, _)| !credentials));
        assert!(m.seen.lock()[0].2.contains("application/vnd.github+json"));
    }

    /// The install fails with `needle` in its message and changes nothing.
    async fn refused(m: &Mock, needle: &str) {
        let at = installation();
        let (result, _) = run(m, &at).await;
        let e = result.unwrap_err();
        assert!(e.contains(needle), "{e:?} should mention {needle:?}");
        assert_eq!(std::fs::read(&at.exe).unwrap(), b"old", "{needle}");
        assert_eq!(files_in(at.exe.parent().unwrap()), ["workbench"], "{needle}");
        assert_eq!(files_in(&at.work), Vec::<String>::new(), "{needle}");
    }

    #[tokio::test]
    async fn a_download_that_does_not_match_its_checksum_installs_nothing() {
        let m = mock().await;
        // Another archive under the same name and size class: the checksum is of the real one.
        let other = archive("99.0.1");
        m.release.lock().1["assets"][0]["size"] = json!(other.len());
        *m.archive.lock() = other;
        refused(&m, "does not match the release's checksum").await;

        // A checksum of some other file, an error page, no checksum at all.
        let m = mock().await;
        *m.checksum.lock() = format!("{}  another.tar.gz\n", sha256(&m.archive.lock()));
        refused(&m, "is not a SHA-256 of").await;
        *m.checksum.lock() = "<html>rate limited</html>".into();
        refused(&m, "is not a SHA-256 of").await;
        m.release.lock().1["assets"].as_array_mut().unwrap().pop();
        refused(&m, "has no checksum").await;

        // GitHub's own digest of the asset is checked as well.
        let m = mock().await;
        m.release.lock().1["assets"][0]["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
        refused(&m, "digest GitHub lists").await;
        m.release.lock().1["assets"][0]["digest"] = json!(format!("sha256:{}", sha256(&m.archive.lock())));
        let at = installation();
        run(&m, &at).await.0.unwrap();
    }

    #[tokio::test]
    async fn a_download_of_another_size_or_another_program_installs_nothing() {
        // Longer or shorter than the release says.
        let m = mock().await;
        let size = m.archive.lock().len();
        m.release.lock().1["assets"][0]["size"] = json!(size - 1);
        refused(&m, "larger than the release says").await;
        m.release.lock().1["assets"][0]["size"] = json!(size + 1);
        refused(&m, "stopped at").await;

        // A correct checksum of a program that is another version, fails, or is not one.
        for (bin, needle) in [
            (program("98.0.0"), "says it is"),
            (b"#!/bin/sh\necho 'GLIBC_2.99 not found' >&2\nexit 1\n".to_vec(), "does not start on this computer (exit code 1): GLIBC_2.99 not found"),
            (b"\x7fELF not really".to_vec(), "does not start on this computer"),
        ] {
            let m = mock().await;
            let data = archive_with(&[(&format!("{STEM}/workbench"), tar::EntryType::Regular, &bin)]);
            *m.checksum.lock() = format!("{}  {STEM}.tar.gz\n", sha256(&data));
            m.release.lock().1["assets"][0]["size"] = json!(data.len());
            *m.archive.lock() = data;
            refused(&m, needle).await;
        }
    }

    #[tokio::test]
    async fn redirects_leave_this_computer_only_for_https() {
        let m = mock().await;
        // TEST-NET-3: refused before anything connects to it.
        *m.redirect.lock() = Some("http://203.0.113.7/archive".into());
        refused(&m, "cannot reach").await;
        assert!(!m.paths().contains(&"/storage/archive".to_string()));
        // A loop of redirects ends.
        *m.redirect.lock() = Some(format!("{}/repos/acme/workbench/releases/assets/1", m.base.lock()));
        refused(&m, "cannot reach").await;
    }

    #[tokio::test]
    async fn the_server_installs_on_request_and_asks_for_the_restart() {
        let (m, at) = (mock().await, installation());
        let app = app_for(&m).await;
        app.state.platform.update.inner.lock().exe = Some(at.exe.clone());

        let (status, s) = call(&app, "POST", "/api/platform/update/check", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!((s["available"].as_bool(), s["canInstall"].as_bool(), s["latest"]["version"].as_str()), (Some(true), Some(true), Some("99.0.0")), "{s}");

        // Only the version that was looked at, and only a newer one.
        let (status, e) = call(&app, "POST", "/api/platform/update/install", Some(json!({ "version": env!("CARGO_PKG_VERSION") }))).await;
        assert_eq!((status, e["error"]["message"].as_str().unwrap().contains("not newer")), (StatusCode::BAD_REQUEST, true));
        let mut events = app.state.events.subscribe();
        let (status, _) = call(&app, "POST", "/api/platform/update/install", Some(json!({ "version": "98.0.0" }))).await;
        assert_eq!(status, StatusCode::OK);
        let s = settled(&app).await;
        assert!(s["failure"].as_str().unwrap().contains("the latest release is now 99.0.0, not 98.0.0"), "{s}");
        assert_eq!(std::fs::read(&at.exe).unwrap(), b"old");

        let (status, s) = call(&app, "POST", "/api/platform/update/install", Some(json!({ "version": "99.0.0", "restart": true }))).await;
        assert_eq!((status, s["phase"].as_str(), s["failure"].is_null()), (StatusCode::OK, Some("downloading"), true), "{s}");
        // A second one meanwhile is refused, and so is a restart.
        for (path, body) in [("/api/platform/update/install", Some(json!({ "version": "99.0.0" }))), ("/api/platform/update/check", None), ("/api/platform/restart", None)] {
            let (status, e) = call(&app, "POST", path, body).await;
            assert_eq!((status, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("busy")), "{path}");
        }
        tokio::time::timeout(Duration::from_secs(20), app.state.platform.update.restart_requested()).await.expect("the install asks for the restart");
        assert!(app.state.platform.update.restarting());
        assert_eq!(std::fs::read(&at.exe).unwrap(), program("99.0.0"));
        assert_eq!(std::fs::read(at.exe.with_file_name("workbench.prev")).unwrap(), b"old");
        let s = status_of(&app).await;
        assert_eq!((s["phase"].as_str(), s["installed"].as_str(), s["restartPending"].as_bool()), (Some("restarting"), Some("99.0.0"), Some(true)), "{s}");

        // Every device followed it through `platform.update`.
        let mut phases = vec![];
        while let Ok(ev) = events.try_recv() {
            if ev.kind == "platform.update" {
                let phase = ev.data["phase"].as_str().unwrap().to_string();
                if phases.last() != Some(&phase) {
                    phases.push(phase);
                }
            }
        }
        assert_eq!(phases, ["downloading", "idle", "downloading", "verifying", "installing", "restarting"]);
    }
}

// ---------------------------------------------------------------- routes

async fn call(app: &testutil::TestApp, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("host", "127.0.0.1:7999").header("authorization", format!("Bearer {}", app.state.auth.master_token()));
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
}

async fn status_of(app: &testutil::TestApp) -> Value {
    let (status, s) = call(app, "GET", "/api/platform/update", None).await;
    assert_eq!(status, StatusCode::OK);
    s
}

/// A server whose config.toml names the stand-in.
async fn app_for(m: &Mock) -> testutil::TestApp {
    let mut cfg = GlobalConfig::default();
    cfg.projects.roots.clear();
    cfg.notify.desktop = false;
    cfg.update = UpdateConfig { check: true, repo: Some(REPO.into()), api: Some(m.base.lock().clone()) };
    testutil::app_with(cfg).await
}

#[tokio::test]
async fn a_build_without_a_source_says_so_and_never_looks() {
    if BUILD_REPO.is_some() {
        return; // a build made with WORKBENCH_UPDATE_REPO has one
    }
    let app = testutil::app().await;
    let s = status_of(&app).await;
    assert_eq!(s["current"], env!("CARGO_PKG_VERSION"));
    assert_eq!((s["source"].is_null(), s["available"].as_bool(), s["phase"].as_str(), s["check"].as_bool()), (true, Some(false), Some("idle"), Some(true)), "{s}");
    assert!(s["latest"].is_null() && s["checkedAt"].is_null() && s["installed"].is_null() && s["restartPending"] == false);

    let (status, e) = call(&app, "POST", "/api/platform/update/check", None).await;
    assert_eq!((status, e["error"]["code"].as_str()), (StatusCode::PRECONDITION_FAILED, Some("not_configured")), "{e}");
    assert!(e["error"]["message"].as_str().unwrap().contains("[update]"));
    if support::unsupported(Feature::SelfUpdate).is_none() {
        let (status, e) = call(&app, "POST", "/api/platform/update/install", Some(json!({ "version": "99.0.0" }))).await;
        assert_eq!((status, e["error"]["code"].as_str()), (StatusCode::PRECONDITION_FAILED, Some("not_configured")), "{e}");
    }
}

#[tokio::test]
async fn a_look_finds_the_release_and_is_remembered() {
    let m = mock().await;
    let app = app_for(&m).await;
    let s = status_of(&app).await;
    assert_eq!((s["source"].as_str(), s["available"].as_bool(), s["checkedAt"].is_null()), (Some(REPO), Some(false), true), "{s}");
    assert!(m.paths().is_empty(), "nothing is asked before the first look");

    let (status, s) = call(&app, "POST", "/api/platform/update/check", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((s["available"].as_bool(), s["latest"]["version"].as_str(), s["error"].is_null()), (Some(true), Some("99.0.0"), true), "{s}");
    assert_eq!(s["latest"]["url"], "https://github.com/acme/workbench/releases/tag/v99.0.0");
    assert!(s["latest"]["notes"].as_str().unwrap().contains("updates itself") && s["checkedAt"].as_i64().unwrap() > 0);
    assert_eq!(m.paths(), ["/repos/acme/workbench/releases/latest"]);

    // Kept for the next start, privately; a version that is not newer is not offered.
    let file = cache_file(&app.state.paths);
    perm::assert_mode(&file, 0o600);
    let kept: Checked = util::fs::read_json(&file).unwrap().unwrap();
    assert_eq!(kept.latest.unwrap().version, "99.0.0");
    *m.release.lock() = (200, release_json(&m.base.lock(), env!("CARGO_PKG_VERSION"), 10));
    let (_, s) = call(&app, "POST", "/api/platform/update/check", None).await;
    assert_eq!((s["available"].as_bool(), s["canInstall"].as_bool(), s["latest"]["version"].as_str()), (Some(false), Some(false), Some(env!("CARGO_PKG_VERSION"))), "{s}");

    // A look that fails answers 200 with why, keeps what was known, and never shows the body.
    for (code, body, needle) in [
        (404, json!({ "message": "Not Found" }), "has no published release"),
        (500, json!({ "message": "secret-detail" }), "the server answered 500"),
        (429, json!({ "message": "secret-detail" }), "hourly limit"),
        (200, json!({ "nothing": "here" }), "did not answer with a release"),
        (200, json!({ "tag_name": "nightly" }), "not a version like v1.2.3"),
    ] {
        *m.release.lock() = (code, body);
        let (status, s) = call(&app, "POST", "/api/platform/update/check", None).await;
        assert_eq!(status, StatusCode::OK);
        let e = s["error"].as_str().unwrap();
        assert!(e.contains(needle) && !e.contains("secret-detail"), "{code}: {e}");
        assert_eq!(s["latest"]["version"], env!("CARGO_PKG_VERSION"), "{code}");
    }

    // Another repository in config.toml: the remembered answer is not about it.
    let (status, _) = call(&app, "PATCH", "/api/settings", Some(json!({ "patch": { "update": { "repo": "acme/other", "check": false } } }))).await;
    assert_eq!(status, StatusCode::OK);
    let s = status_of(&app).await;
    assert_eq!((s["source"].as_str(), s["check"].as_bool(), s["latest"].is_null(), s["checkedAt"].is_null()), (Some("acme/other"), Some(false), true, true), "{s}");
    let text = std::fs::read_to_string(app.state.paths.config_file()).unwrap();
    assert!(text.contains("[update]") && text.contains("check = false") && text.contains("repo = \"acme/other\""), "{text}");
    // A source that cannot be used is refused when saving.
    let (status, e) = call(&app, "PATCH", "/api/settings", Some(json!({ "patch": { "update": { "api": "http://ghe.example.com" } } }))).await;
    assert_eq!((status, e["error"]["message"].as_str().unwrap().contains("update.api")), (StatusCode::BAD_REQUEST, true), "{e}");
}

#[tokio::test]
async fn agents_never_look_install_or_restart() {
    let m = mock().await;
    let app = app_for(&m).await;
    let ctx = McpCtx { terminal_id: Some("t1".into()), project_id: None };
    for (path, body) in [
        ("/api/platform/update/check", None),
        ("/api/platform/update/install", Some(json!({ "version": "99.0.0", "restart": true }))),
        ("/api/platform/restart", None),
    ] {
        let e = mcp::call_api(&app.state, axum::http::Method::POST, path, body, &ctx).await.unwrap_err();
        assert_eq!((e.status, e.code), (StatusCode::FORBIDDEN, "forbidden"), "{path}");
    }
    assert!(m.paths().is_empty() && !app.state.platform.update.restarting());
    // No MCP tool does it either.
    assert!(!mcp::all_tools().iter().any(|t| t.name.starts_with("workbench_") && (t.name.contains("update") || t.name.contains("restart"))));

    // The user's restart: the server is told to stop and start again.
    if support::unsupported(Feature::SelfUpdate).is_none() {
        let (status, ok) = call(&app, "POST", "/api/platform/restart", None).await;
        assert_eq!((status, ok["ok"].as_bool()), (StatusCode::OK, Some(true)));
        tokio::time::timeout(Duration::from_secs(5), app.state.platform.update.restart_requested()).await.expect("a restart was asked for");
        assert!(app.state.platform.update.restarting());
        assert_eq!(status_of(&app).await["phase"], "restarting");
        // On its way out it starts nothing more.
        for path in ["/api/platform/restart", "/api/platform/update/check"] {
            let (status, e) = call(&app, "POST", path, None).await;
            assert_eq!((status, e["error"]["message"].as_str()), (StatusCode::CONFLICT, Some("Workbench is restarting")), "{path}");
        }
    }
}

#[test]
fn update_settings_default_to_looking_and_leave_config_toml_alone() {
    let cfg = GlobalConfig::default();
    assert!(cfg.update.check && cfg.update.repo.is_none());
    assert!(!toml::to_string_pretty(&cfg).unwrap().contains("[update]"), "defaults are not written");
    let off: GlobalConfig = toml::from_str("[update]\ncheck = false\n").unwrap();
    assert!(!off.update.check);
    let named: GlobalConfig = toml::from_str("[update]\nrepo = \"acme/workbench\"\n").unwrap();
    assert!(named.update.check && named.update.repo.as_deref() == Some("acme/workbench"));
    let (errors, _) = crate::platform::settings::check_global(&toml::from_str("[update]\nrepo = \"nope\"\n").unwrap());
    assert!(errors.iter().any(|e| e.contains("update.repo")), "{errors:?}");
}

#[test]
fn only_linux_x86_64_installs_by_itself() {
    assert_eq!(target_of("linux", "x86_64"), Some(("x86_64-unknown-linux-gnu", "tar.gz")));
    assert_eq!(target_of("windows", "x86_64"), Some(("x86_64-pc-windows-msvc", "zip")));
    assert_eq!(target_of("linux", "aarch64"), None);
    assert_eq!(target_of("macos", "aarch64"), None);
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(install_blocker(Some(&dir.path().join("workbench"))), None);
        assert_eq!(files_in(dir.path()), Vec::<String>::new(), "asking writes nothing");
        // A folder the user may not write to (a system-wide install).
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        perm::apply(&locked, 0o555).unwrap();
        if perm::user_ids().is_some_and(|(uid, _)| uid != 0) {
            assert!(install_blocker(Some(&locked.join("workbench"))).unwrap().contains("not writable"));
        }
        perm::apply(&locked, 0o755).unwrap();
        assert!(install_blocker(None).unwrap().contains("cannot tell"));
        let missing = install_blocker(Some(&dir.path().join("no/such/dir/workbench"))).unwrap();
        assert!(missing.contains("not writable") && missing.contains("install.sh"), "{missing}");
    } else {
        assert!(install_blocker(Some(Path::new("workbench"))).is_some());
    }
}
