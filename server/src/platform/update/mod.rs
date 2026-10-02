//! Updates: Workbench looks for a newer release of itself and, on the user's say-so,
//! downloads it, checks it, replaces its own binary and restarts into it.
//!
//! * **Where releases come from.** The GitHub repository this build was made from
//!   (`WORKBENCH_UPDATE_REPO` at build time, which the release workflow sets), or
//!   `[update] repo` in config.toml. A build with neither (a local `cargo build`) never
//!   looks and never updates. Only config.toml names the source; a repository's config
//!   cannot.
//! * **Looking** is one anonymous GET of the repository's latest release: once a day while
//!   `[update] check` is on (the default), and on request. No token is ever sent.
//! * **Installing** happens only on a click in Settings or with `workbench update`, never
//!   by itself and never for an agent. The archive for this platform is downloaded from
//!   the release, its SHA-256 compared with the release's `.sha256` file (and GitHub's
//!   own digest when the API gives one), the binary taken out of it and asked for its
//!   version, the running one kept beside it as `<name>.prev`, and the new one renamed
//!   over it. The checksum comes from the same release: it catches a damaged download,
//!   and TLS to the release host is what vouches for the publisher.
//! * **Restarting** replaces the server process with the new binary in place
//!   (`util::os::proc::reexec` after the usual shutdown), so a systemd unit or a terminal
//!   keeps one process. Terminals stop as on any restart; agent sessions come back with
//!   `agents.restore_on_start`.
//!
//! Routes: `GET /api/platform/update`, `POST /api/platform/update/check`,
//! `POST /api/platform/update/install`, `POST /api/platform/restart`. Event:
//! `platform.update` (the status). CLI: `workbench update [--check] [--restart]`.
//! Installing and restarting are left out on Windows (`support::Feature::SelfUpdate`).

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::auth::Caller;
use crate::config::{GlobalConfig, Paths};
use crate::error::{ApiError, ApiResult};
use crate::util;
use crate::util::os::support::{self, Feature};
use crate::util::os::{perm, proc};

/// `workbench update`'s help line.
pub const ABOUT: &str = "Install the latest Workbench release over this one";

/// `owner/name` of the repository this build was released from; unset in local builds.
const BUILD_REPO: Option<&str> = option_env!("WORKBENCH_UPDATE_REPO");
const DEFAULT_API: &str = "https://api.github.com";

/// How often the background looks, and how soon after a look that failed.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
const RETRY_AFTER: Duration = Duration::from_secs(3600);
/// The first look after the server started, and how often it considers looking.
const FIRST_LOOK: Duration = Duration::from_secs(30);
const LOOK_TICK: Duration = Duration::from_secs(900);

const MAX_RELEASE_JSON: usize = 2 * 1024 * 1024;
const MAX_NOTES_CHARS: usize = 16_000;
const MAX_ARCHIVE: u64 = 300 * 1024 * 1024;
const MAX_BINARY: u64 = 600 * 1024 * 1024;
const MAX_CHECKSUM_FILE: usize = 4096;
const MAX_REDIRECTS: usize = 5;

const NO_SOURCE: &str = "this build of Workbench does not know where its releases are published (it was not made by the release \
     workflow): set `repo = \"owner/name\"` under [update] in config.toml to name the GitHub repository";

// ---------------------------------------------------------------- config

/// `[update]` in config.toml.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateConfig {
    /// Look for a newer release once a day. Off: only when asked (Settings, `workbench update`).
    pub check: bool,
    /// The GitHub repository (`owner/name`) whose releases are offered, instead of the one
    /// this build was released from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// The GitHub API's base URL, for GitHub Enterprise (`https://ghe.example.com/api/v3`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self { check: true, repo: None, api: None }
    }
}

/// Where releases are looked up: an API base without a trailing slash and `owner/name`.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub api: String,
    pub repo: String,
}

fn valid_repo(repo: &str) -> bool {
    let part = |p: &str| !p.is_empty() && p.len() <= 100 && p != "." && p != ".." && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    repo.split_once('/').is_some_and(|(owner, name)| part(owner) && part(name))
}

/// Whether `url` names this computer: `localhost` or a loopback address.
fn is_loopback(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or("");
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The API base as requests use it, or what is wrong with it. https only: what comes
/// from there replaces this program. Plain http is taken for this computer alone (a
/// mirror, tests).
fn checked_api(api: &str) -> Result<String, String> {
    let api = api.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(api).map_err(|e| format!("update.api {api:?} is not a URL: {e}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("update.api must not contain credentials".into());
    }
    if url.query().is_some() || url.fragment().is_some() || url.host_str().is_none() {
        return Err(format!("update.api {api:?} should look like https://ghe.example.com/api/v3"));
    }
    match url.scheme() {
        "https" => Ok(api.to_string()),
        "http" if is_loopback(&url) => Ok(api.to_string()),
        _ => Err(format!("update.api {api:?} must start with https:// (updates replace this program)")),
    }
}

impl Source {
    /// The source `cfg` names, the build's own otherwise; `Ok(None)` when there is none,
    /// `Err` when config.toml names one that cannot be used.
    pub fn from_config(cfg: &UpdateConfig) -> Result<Option<Source>, String> {
        Self::resolve(cfg, BUILD_REPO)
    }

    fn resolve(cfg: &UpdateConfig, build_repo: Option<&str>) -> Result<Option<Source>, String> {
        let api = checked_api(cfg.api.as_deref().unwrap_or(DEFAULT_API))?;
        let named = cfg.repo.as_deref().map(str::trim).filter(|r| !r.is_empty());
        if let Some(repo) = named.filter(|r| !valid_repo(r)) {
            return Err(format!("update.repo {repo:?} must be a GitHub repository like owner/name"));
        }
        let repo = named.or(build_repo.map(str::trim).filter(|r| valid_repo(r)));
        Ok(repo.map(|repo| Source { api, repo: repo.to_string() }))
    }

    /// Tells one source's cached answer from another's.
    fn key(&self) -> String {
        format!("{} {}", self.api, self.repo)
    }

    fn insecure(&self) -> bool {
        self.api.starts_with("http://")
    }

    /// Where its release assets are served: nothing else is downloaded.
    fn asset_prefix(&self) -> String {
        format!("{}/repos/{}/releases/assets/", self.api, self.repo)
    }
}

// ---------------------------------------------------------------- versions and releases

/// A release version `X.Y.Z`. Anything else (a pre-release suffix, two numbers) is not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u64, u64, u64);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let mut parts = s.strip_prefix('v').unwrap_or(s).split('.');
        let mut number = || parts.next().filter(|p| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()))?.parse::<u64>().ok();
        let version = Version(number()?, number()?, number()?);
        parts.next().is_none().then_some(version)
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The release archive's target triple and extension for an OS and CPU, as
/// `.github/workflows/release.yml` names them.
fn target_of(os: &str, arch: &str) -> Option<(&'static str, &'static str)> {
    match (os, arch) {
        ("linux", "x86_64") => Some(("x86_64-unknown-linux-gnu", "tar.gz")),
        ("windows", "x86_64") => Some(("x86_64-pc-windows-msvc", "zip")),
        _ => None,
    }
}

fn this_target() -> Option<(&'static str, &'static str)> {
    target_of(std::env::consts::OS, std::env::consts::ARCH)
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    url: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// This platform's archive in a release.
#[derive(Debug, Clone, PartialEq)]
struct Archive {
    name: String,
    url: String,
    size: u64,
    /// GitHub's own SHA-256 of the file (hex), when the API gives one.
    digest: Option<String>,
    /// The release's `<name>.sha256` file.
    checksum_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    info: ReleaseInfo,
    archive: Option<Archive>,
}

/// What the UI shows of a release (and what `update.json` keeps).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub version: String,
    /// The release notes (Markdown, untrusted like any rendered Markdown).
    pub notes: String,
    /// The release's page.
    pub url: Option<String>,
    pub published_at: Option<i64>,
    /// The release has an archive for this OS and CPU.
    pub archive: bool,
}

fn sha256_hex_of(s: &str) -> Option<String> {
    (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

impl Release {
    fn from_api(source: &Source, api: ApiRelease, target: Option<(&str, &str)>) -> Result<Release, String> {
        let version = Version::parse(&api.tag_name)
            .ok_or_else(|| format!("the latest release of {} has the tag {:?}, which is not a version like v1.2.3", source.repo, super::truncate_chars(&api.tag_name, 40)))?;
        if api.draft || api.prerelease {
            return Err(format!("the latest release of {} ({version}) is a draft or a pre-release", source.repo));
        }
        let prefix = source.asset_prefix();
        let served = |a: &ApiAsset| a.url.strip_prefix(&prefix).is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
        let archive = match target {
            None => None,
            Some((triple, ext)) => {
                let name = format!("workbench-{version}-{triple}.{ext}");
                let checksum_name = format!("{name}.sha256");
                let checksum = api.assets.iter().find(|a| a.name == checksum_name);
                match api.assets.iter().find(|a| a.name == name) {
                    None => None,
                    Some(a) if !served(a) || checksum.is_some_and(|c| !served(c)) => {
                        return Err(format!("release {version} of {} lists files that are not served by {}", source.repo, source.api));
                    }
                    Some(a) => Some(Archive {
                        name,
                        url: a.url.clone(),
                        size: a.size,
                        digest: a.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")).and_then(sha256_hex_of),
                        checksum_url: checksum.map(|c| c.url.clone()),
                    }),
                }
            }
        };
        let info = ReleaseInfo {
            version: version.to_string(),
            notes: super::truncate_chars(api.body.as_deref().unwrap_or("").trim(), MAX_NOTES_CHARS),
            url: api.html_url.filter(|u| u.starts_with("https://") && u.len() <= 500 && !u.contains(char::is_whitespace)),
            published_at: api.published_at.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp_millis()),
            archive: archive.is_some(),
        };
        Ok(Release { version, info, archive })
    }
}

// ---------------------------------------------------------------- requests

/// A client for one source: redirects (the API sends a download to its storage) only to
/// https, or within this computer when the source itself is plain http there.
fn client(source: &Source) -> Result<reqwest::Client, String> {
    let insecure = source.insecure();
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if attempt.url().scheme() == "https" || (insecure && is_loopback(attempt.url())) {
            attempt.follow()
        } else {
            attempt.error("a redirect to a plain http address")
        }
    });
    reqwest::Client::builder()
        .user_agent(concat!("workbench/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .redirect(policy)
        .build()
        .map_err(|e| format!("cannot set up the download: {e}"))
}

fn host_of(url: &str) -> String {
    reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "the release server".into())
}

fn unreachable(url: &str, e: reqwest::Error) -> String {
    if e.is_timeout() { format!("{} did not answer in time", host_of(url)) } else { format!("cannot reach {}: {}", host_of(url), e.without_url()) }
}

/// Why a request was refused, without the answer's body (it is not ours to show).
fn refused(what: &str, resp: &reqwest::Response) -> String {
    let status = resp.status();
    let spent = resp.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()) == Some("0");
    if status == StatusCode::TOO_MANY_REQUESTS || (status == StatusCode::FORBIDDEN && spent) {
        "GitHub's hourly limit for requests from this address is used up: try again later".into()
    } else {
        format!("{what}: the server answered {status}")
    }
}

/// The body, at most `max` bytes of it.
async fn body_capped(mut resp: reqwest::Response, max: usize, what: &str) -> Result<Vec<u8>, String> {
    let url = resp.url().to_string();
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| unreachable(&url, e))? {
        if out.len() + chunk.len() > max {
            return Err(format!("{what} is larger than expected"));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// The repository's latest release (never a draft or a pre-release).
pub async fn latest(source: &Source, client: &reqwest::Client) -> Result<Release, String> {
    let url = format!("{}/repos/{}/releases/latest", source.api, source.repo);
    let resp = client
        .get(&url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| unreachable(&url, e))?;
    if resp.status() == StatusCode::NOT_FOUND {
        return Err(format!("{} has no published release, or is not a public repository", source.repo));
    }
    if !resp.status().is_success() {
        return Err(refused("looking for the latest release", &resp));
    }
    let body = body_capped(resp, MAX_RELEASE_JSON, "the release description").await?;
    let api: ApiRelease = serde_json::from_slice(&body).map_err(|_| format!("{} did not answer with a release", host_of(&url)))?;
    Release::from_api(source, api, this_target())
}

/// Download a release asset into `dest`; the SHA-256 (hex) of what was written.
async fn download(client: &reqwest::Client, url: &str, dest: &Path, size: u64, on_progress: &(dyn Fn(u64) + Send + Sync)) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    let mut resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/octet-stream")
        .timeout(Duration::from_secs(30 * 60))
        .send()
        .await
        .map_err(|e| unreachable(url, e))?;
    if !resp.status().is_success() {
        return Err(refused("downloading the release", &resp));
    }
    let limit = if size > 0 { size.min(MAX_ARCHIVE) } else { MAX_ARCHIVE };
    let _ = std::fs::remove_file(dest);
    let file = perm::open_new(dest, 0o600, true).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    let mut file = tokio::fs::File::from_std(file);
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| unreachable(url, e))? {
        received += chunk.len() as u64;
        if received > limit {
            return Err("the download is larger than the release says".into());
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
        on_progress(received);
    }
    file.flush().await.map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    if size > 0 && received != size {
        return Err(format!("the download stopped at {received} of {size} bytes"));
    }
    Ok(hex::encode(hasher.finalize()))
}

/// The digest a `sha256sum` line gives for `name` (`<hex>  <name>`, as the release
/// workflow writes it); a line naming another file is not about this archive.
fn checksum_for(text: &str, name: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let digest = sha256_hex_of(words.next()?)?;
    match words.next() {
        None => Some(digest),
        Some(file) => (file.trim_start_matches('*') == name).then_some(digest),
    }
}

// ---------------------------------------------------------------- installing

/// What an install is doing, for whoever shows it.
pub enum Step {
    Download { received: u64, total: u64 },
    Verify,
    Install,
}

#[derive(Debug)]
pub struct Installed {
    pub version: Version,
    /// The replaced binary, kept beside the new one.
    pub previous: Option<PathBuf>,
}

fn beside(exe: &Path, name: impl FnOnce(&str) -> String) -> PathBuf {
    let file = exe.file_name().and_then(|n| n.to_str()).unwrap_or("workbench");
    exe.with_file_name(name(file))
}

/// Why this Workbench cannot install a release over itself; `None` when it can.
pub fn install_blocker(exe: Option<&Path>) -> Option<String> {
    if let Some(why) = support::unsupported(Feature::SelfUpdate) {
        return Some(why.to_string());
    }
    match this_target() {
        Some((_, "tar.gz")) => {}
        _ => return Some(format!("releases have no build for {} on {}: update from source", std::env::consts::OS, std::env::consts::ARCH)),
    }
    let Some(exe) = exe else { return Some("cannot tell where this Workbench is installed".into()) };
    let dir = exe.parent().unwrap_or(Path::new("."));
    // Renaming the new binary into place needs a writable folder, whatever the file's own mode.
    (!perm::dir_writable(dir)).then(|| {
        format!("{} is not writable by the user Workbench runs as: install the new release there with its install.sh", crate::config::contract_tilde(dir))
    })
}

/// Take `member` (a regular file) out of the gzipped tar `archive` into `tmp`, which gets
/// the permissions of `exe`. Nothing else in the archive is written anywhere.
fn extract_binary(archive: &Path, member: &str, tmp: &Path, exe: &Path) -> Result<(), String> {
    let damaged = |e: std::io::Error| format!("the release archive cannot be read: {e}");
    let file = std::fs::File::open(archive).map_err(damaged)?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(file)));
    for entry in tar.entries().map_err(damaged)? {
        let mut entry = entry.map_err(damaged)?;
        if !entry.path().is_ok_and(|p| p == Path::new(member)) {
            continue;
        }
        let size = entry.header().size().map_err(damaged)?;
        if entry.header().entry_type() != tar::EntryType::Regular || size == 0 || size > MAX_BINARY {
            return Err(format!("{member} in the release archive is not a program file"));
        }
        let _ = std::fs::remove_file(tmp);
        let written = (|| -> std::io::Result<u64> {
            let mut out = perm::create_replacement(tmp, exe, Some(0o755))?;
            let n = std::io::copy(&mut entry.by_ref().take(size), &mut out)?;
            out.sync_all()?;
            Ok(n)
        })();
        return match written {
            Ok(n) if n == size => Ok(()),
            Ok(_) => Err("the release archive ends early".into()),
            Err(e) => Err(format!("cannot write {}: {e}", tmp.display())),
        };
    }
    Err(format!("the release archive has no {member}"))
}

/// Run the new binary once: it must start here (its C library may be newer than this
/// computer's) and be the version the release says.
async fn reports_version(bin: &Path, version: &Version) -> Result<(), String> {
    let run = tokio::process::Command::new(bin).arg("--version").stdin(std::process::Stdio::null()).kill_on_drop(true).output();
    let out = match tokio::time::timeout(Duration::from_secs(15), run).await {
        Err(_) => return Err("the new version did not answer `--version`".into()),
        Ok(Err(e)) => return Err(format!("the new version does not start on this computer: {e}")),
        Ok(Ok(out)) => out,
    };
    let said = String::from_utf8_lossy(&out.stdout);
    let said = said.trim();
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        let why = if why.is_empty() { String::new() } else { format!(": {}", super::truncate_chars(&why, 200)) };
        return Err(format!("the new version does not start on this computer ({}){why}", proc::exit_text(&out.status)));
    }
    if said != format!("workbench {version}") {
        return Err(format!("the downloaded program says it is {:?}, not workbench {version}", super::truncate_chars(said, 60)));
    }
    Ok(())
}

/// Download `release`'s archive for this platform into `work`, check it, and put its
/// binary in the place of `exe`, keeping the old one as `<exe>.prev`. `exe` is touched
/// only after every check passed.
pub async fn install_release(
    client: &reqwest::Client,
    release: &Release,
    exe: &Path,
    work: &Path,
    on: &(dyn Fn(Step) + Send + Sync),
) -> Result<Installed, String> {
    let version = release.version;
    let archive = release.archive.as_ref().ok_or_else(|| format!("release {version} has no build for this computer"))?;
    let Some(stem) = archive.name.strip_suffix(".tar.gz") else { return Err(format!("{} is not an archive Workbench can install by itself", archive.name)) };
    if archive.digest.is_none() && archive.checksum_url.is_none() {
        return Err(format!("release {version} has no checksum for {}", archive.name));
    }
    perm::create_dir_private(work).map_err(|e| format!("cannot create {}: {e}", work.display()))?;
    let file = work.join(&archive.name);
    let tmp = beside(exe, |n| format!(".{n}.new"));
    let result = async {
        let total = archive.size;
        on(Step::Download { received: 0, total });
        let sha = download(client, &archive.url, &file, total, &|received| on(Step::Download { received, total })).await?;

        on(Step::Verify);
        if let Some(url) = &archive.checksum_url {
            let resp = client.get(url).header(reqwest::header::ACCEPT, "application/octet-stream").timeout(Duration::from_secs(30)).send().await.map_err(|e| unreachable(url, e))?;
            if !resp.status().is_success() {
                return Err(refused("downloading the release's checksum", &resp));
            }
            let text = body_capped(resp, MAX_CHECKSUM_FILE, "the release's checksum file").await?;
            let expected = checksum_for(&String::from_utf8_lossy(&text), &archive.name).ok_or_else(|| format!("the release's checksum file is not a SHA-256 of {}", archive.name))?;
            if expected != sha {
                return Err(format!("the download does not match the release's checksum (SHA-256 {sha}, expected {expected}): nothing was installed"));
            }
        }
        if archive.digest.as_ref().is_some_and(|d| *d != sha) {
            return Err(format!("the download does not match the digest GitHub lists for it (SHA-256 {sha}): nothing was installed"));
        }

        on(Step::Install);
        let (member, from, to, target) = (format!("{stem}/workbench"), file.clone(), tmp.clone(), exe.to_path_buf());
        tokio::task::spawn_blocking(move || extract_binary(&from, &member, &to, &target)).await.map_err(|e| format!("unpacking failed: {e}"))??;
        reports_version(&tmp, &version).await?;

        // The running version stays beside the new one, for going back by hand.
        let previous = beside(exe, |n| format!("{n}.prev"));
        let staged = beside(exe, |n| format!(".{n}.prev.new"));
        let kept = std::fs::copy(exe, &staged).and_then(|_| std::fs::rename(&staged, &previous)).is_ok();
        if !kept {
            let _ = std::fs::remove_file(&staged);
        }
        perm::rename_into_place(&tmp, exe).map_err(|e| format!("cannot replace {}: {e}", exe.display()))?;
        Ok(Installed { version, previous: kept.then_some(previous) })
    }
    .await;
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir(work);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Verifying,
    Installing,
    Restarting,
}

/// The last look, kept in `data_dir/update.json` so a restart does not look again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Checked {
    at: i64,
    source: String,
    latest: Option<ReleaseInfo>,
    error: Option<String>,
}

#[derive(Default)]
struct Inner {
    last: Option<Checked>,
    phase: Phase,
    /// Bytes received and expected while downloading.
    progress: Option<(u64, u64)>,
    /// Why the last install failed.
    failure: Option<String>,
    /// The version this process installed over itself; a restart runs it.
    installed: Option<String>,
    #[cfg(test)]
    exe: Option<PathBuf>,
}

#[derive(Default)]
pub struct UpdateState {
    inner: parking_lot::Mutex<Inner>,
    /// One look or install at a time.
    busy: Arc<tokio::sync::Mutex<()>>,
    restart: tokio::sync::Notify,
    restarting: AtomicBool,
}

impl UpdateState {
    /// Resolves when a restart was asked for (`main.rs` then shuts down and re-executes).
    pub async fn restart_requested(&self) {
        self.restart.notified().await;
    }

    /// Whether the server is stopping in order to start again.
    pub fn restarting(&self) -> bool {
        self.restarting.load(Ordering::SeqCst)
    }
}

/// `GET /api/platform/update`, and the `platform.update` event.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub current: String,
    /// `owner/name` of the repository releases come from; null: this build has none.
    pub source: Option<String>,
    /// `[update]` in config.toml names a source that cannot be used.
    pub source_error: Option<String>,
    /// Looking once a day is on.
    pub check: bool,
    pub checked_at: Option<i64>,
    /// Why the last look failed.
    pub error: Option<String>,
    pub latest: Option<ReleaseInfo>,
    /// `latest` is newer than `current`.
    pub available: bool,
    /// This Workbench can install `latest` over itself; `installNote` says why not.
    pub can_install: bool,
    pub install_note: Option<String>,
    pub phase: Phase,
    pub progress: Option<Value>,
    pub failure: Option<String>,
    pub installed: Option<String>,
    /// The binary on disk is not the running one any more: a restart runs it.
    pub restart_pending: bool,
    /// The server can restart itself (`POST /api/platform/restart`).
    pub can_restart: bool,
}

fn cache_file(paths: &Paths) -> PathBuf {
    paths.data_dir.join("update.json")
}

fn work_dir(paths: &Paths) -> PathBuf {
    paths.data_dir.join("update")
}

/// The file this server runs from and an update replaces.
fn exe_of(state: &AppState) -> Option<PathBuf> {
    #[cfg(test)]
    {
        if let Some(exe) = state.platform.update.inner.lock().exe.clone() {
            return Some(exe);
        }
    }
    let _ = state;
    proc::current_exe().ok()
}

pub fn status(state: &AppState) -> Status {
    let cfg = state.config.read().update.clone();
    let (source, source_error) = match Source::from_config(&cfg) {
        Ok(s) => (s, None),
        Err(e) => (None, Some(e)),
    };
    let blocker = install_blocker(exe_of(state).as_deref());
    let current = Version::current();
    let inner = state.platform.update.inner.lock();
    let last = inner.last.as_ref().filter(|c| source.as_ref().is_some_and(|s| s.key() == c.source));
    let latest = last.and_then(|c| c.latest.clone());
    let available = latest.as_ref().and_then(|l| Version::parse(&l.version)).is_some_and(|v| v > current);
    let no_archive = latest.as_ref().is_some_and(|l| !l.archive);
    let install_note = blocker.or_else(|| (available && no_archive).then(|| "this release has no build for this computer".to_string()));
    Status {
        current: current.to_string(),
        source: source.map(|s| s.repo),
        source_error,
        check: cfg.check,
        checked_at: last.map(|c| c.at),
        error: last.and_then(|c| c.error.clone()),
        available,
        can_install: available && install_note.is_none(),
        install_note,
        latest,
        phase: inner.phase,
        progress: inner.progress.map(|(received, total)| json!({ "received": received, "total": total })),
        failure: inner.failure.clone(),
        installed: inner.installed.clone(),
        restart_pending: inner.installed.is_some() || proc::exe_replaced(),
        can_restart: support::unsupported(Feature::SelfUpdate).is_none(),
    }
}

fn emit(state: &AppState) {
    state.events.emit("platform.update", None, status(state));
}

fn set_phase(state: &AppState, phase: Phase) {
    {
        let mut inner = state.platform.update.inner.lock();
        inner.phase = phase;
        if phase != Phase::Downloading {
            inner.progress = None;
        }
    }
    emit(state);
}

fn source_of(state: &AppState) -> ApiResult<Source> {
    let cfg = state.config.read().update.clone();
    Source::from_config(&cfg).map_err(ApiError::not_configured)?.ok_or_else(|| ApiError::not_configured(NO_SOURCE))
}

fn busy() -> ApiError {
    ApiError::new(StatusCode::CONFLICT, "busy", "Workbench is already looking for or installing an update")
}

/// The one look or install that may run now; none while the server is on its way out.
fn claim(state: &AppState) -> ApiResult<tokio::sync::OwnedMutexGuard<()>> {
    if state.platform.update.restarting() {
        return Err(ApiError::new(StatusCode::CONFLICT, "busy", "Workbench is restarting"));
    }
    state.platform.update.busy.clone().try_lock_owned().map_err(|_| busy())
}

/// Look at `source` now and remember the answer, a failure included.
async fn look(state: &AppState, source: &Source) -> Result<Release, String> {
    let result = match client(source) {
        Ok(c) => latest(source, &c).await,
        Err(e) => Err(e),
    };
    let checked = Checked {
        at: util::now_ms(),
        source: source.key(),
        // A look that failed keeps what the one before it found.
        latest: match &result {
            Ok(r) => Some(r.info.clone()),
            Err(_) => state.platform.update.inner.lock().last.as_ref().filter(|c| c.source == source.key()).and_then(|c| c.latest.clone()),
        },
        error: result.as_ref().err().cloned(),
    };
    if let Err(e) = util::fs::write_json(&cache_file(&state.paths), &checked) {
        tracing::warn!("update: cannot save the last look: {e:#}");
    }
    state.platform.update.inner.lock().last = Some(checked);
    result
}

/// Whether the daily look is due for `source`.
fn due(last: Option<&Checked>, source: &Source, now: i64) -> bool {
    match last.filter(|c| c.source == source.key()) {
        None => true,
        Some(c) => {
            let wait = if c.error.is_some() { RETRY_AFTER } else { CHECK_EVERY };
            now.saturating_sub(c.at) >= wait.as_millis() as i64 || c.at > now
        }
    }
}

/// Load the last look and start the daily one.
pub fn start(state: &AppState) {
    match util::fs::read_json::<Checked>(&cache_file(&state.paths)) {
        Ok(last) => state.platform.update.inner.lock().last = last,
        Err(e) => tracing::warn!("update: {e:#}"),
    }
    // An install that was interrupted (a crash, a kill) leaves its download behind.
    let _ = std::fs::remove_dir_all(work_dir(&state.paths));
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_LOOK).await;
        loop {
            let cfg = state.config.read().update.clone();
            if let (true, Ok(Some(source))) = (cfg.check, Source::from_config(&cfg)) {
                let is_due = due(state.platform.update.inner.lock().last.as_ref(), &source, util::now_ms());
                // Never in the way of a look or an install the user started.
                if let (true, Ok(_guard)) = (is_due, state.platform.update.busy.try_lock()) {
                    match look(&state, &source).await {
                        Ok(r) if r.version > Version::current() => tracing::info!("update: Workbench {} is available", r.version),
                        Ok(_) => {}
                        Err(e) => tracing::debug!("update: {e}"),
                    }
                    emit(&state);
                }
            }
            tokio::time::sleep(LOOK_TICK).await;
        }
    });
}

/// Download and install `want`, which must still be the latest release.
async fn install(state: &AppState, source: &Source, want: Version, exe: &Path) -> Result<Installed, String> {
    let release = look(state, source).await?;
    if release.version != want {
        return Err(format!("the latest release is now {}, not {want}: look at it first", release.version));
    }
    let client = client(source)?;
    let throttle = parking_lot::Mutex::new(Instant::now());
    let on = |step: Step| match step {
        Step::Download { received, total } => {
            state.platform.update.inner.lock().progress = Some((received, total));
            let mut last = throttle.lock();
            if received == 0 || received == total || last.elapsed() >= Duration::from_millis(300) {
                *last = Instant::now();
                emit(state);
            }
        }
        Step::Verify => set_phase(state, Phase::Verifying),
        Step::Install => set_phase(state, Phase::Installing),
    };
    install_release(&client, &release, exe, &work_dir(&state.paths), &on).await
}

/// Stop the server and start it again from the binary now on disk.
pub fn request_restart(state: &AppState) {
    state.platform.update.restarting.store(true, Ordering::SeqCst);
    set_phase(state, Phase::Restarting);
    state.platform.update.restart.notify_one();
}

// ---------------------------------------------------------------- routes

/// An update replaces the program and a restart ends every terminal: the user's call, in
/// the UI or with the master token (`workbench update`), never an agent's through MCP.
fn refuse_internal(caller: Option<&Caller>) -> ApiResult<()> {
    match caller {
        Some(Caller::Internal { .. }) => Err(ApiError::forbidden("Workbench is updated and restarted by the user only")),
        _ => Ok(()),
    }
}

/// `GET /api/platform/update`.
pub async fn get_status(State(state): State<AppState>) -> Json<Status> {
    Json(status(&state))
}

/// `POST /api/platform/update/check`: look now. A look that fails still answers 200, with
/// the reason in `error`.
pub async fn post_check(State(state): State<AppState>, caller: Option<Extension<Caller>>) -> ApiResult<Json<Status>> {
    refuse_internal(caller.as_deref())?;
    let source = source_of(&state)?;
    let _guard = claim(&state)?;
    set_phase(&state, Phase::Checking);
    let _ = look(&state, &source).await;
    set_phase(&state, Phase::Idle);
    Ok(Json(status(&state)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallBody {
    /// The version the user saw and agreed to.
    version: String,
    /// Restart into it once it is installed.
    #[serde(default)]
    restart: bool,
}

/// `POST /api/platform/update/install {version, restart?}`: starts the install and
/// answers at once; `platform.update` events carry the progress and the outcome.
pub async fn post_install(State(state): State<AppState>, caller: Option<Extension<Caller>>, Json(body): Json<InstallBody>) -> ApiResult<Json<Status>> {
    refuse_internal(caller.as_deref())?;
    support::require(Feature::SelfUpdate)?;
    let source = source_of(&state)?;
    let want = Version::parse(&body.version).ok_or_else(|| ApiError::bad_request(format!("{:?} is not a version like 1.2.3", body.version)))?;
    if want <= Version::current() {
        return Err(ApiError::bad_request(format!("{want} is not newer than the running {}", Version::current())));
    }
    let exe = exe_of(&state);
    if let Some(why) = install_blocker(exe.as_deref()) {
        return Err(ApiError::bad_request(why));
    }
    let exe = exe.ok_or_else(|| ApiError::internal("no executable path"))?;
    let guard = claim(&state)?;
    state.platform.update.inner.lock().failure = None;
    set_phase(&state, Phase::Downloading);
    let task_state = state.clone();
    tokio::spawn(async move {
        let state = task_state;
        let _guard = guard;
        let result = install(&state, &source, want, &exe).await;
        let done = result.is_ok();
        match result {
            Ok(installed) => {
                tracing::info!("update: installed Workbench {} to {}", installed.version, exe.display());
                state.platform.update.inner.lock().installed = Some(installed.version.to_string());
            }
            Err(e) => {
                tracing::warn!("update: {e}");
                state.platform.update.inner.lock().failure = Some(e);
            }
        }
        if done && body.restart {
            request_restart(&state);
        } else {
            set_phase(&state, Phase::Idle);
        }
    });
    Ok(Json(status(&state)))
}

/// `POST /api/platform/restart`: shut down as on a stop signal, then run the binary on
/// disk in this process's place.
pub async fn post_restart(State(state): State<AppState>, caller: Option<Extension<Caller>>) -> ApiResult<Json<Value>> {
    refuse_internal(caller.as_deref())?;
    support::require(Feature::SelfUpdate)?;
    drop(claim(&state)?);
    tracing::info!("restart requested");
    request_restart(&state);
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------- CLI

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Only look for a newer release and say which; install nothing.
    #[arg(long)]
    pub check: bool,
    /// Also restart the running Workbench into the installed version. Its terminals stop;
    /// agent sessions come back when `agents.restore_on_start` is on.
    #[arg(long)]
    pub restart: bool,
}

/// config.toml as it is, without writing a default one (`workbench update` on a computer
/// that never ran the server).
fn read_config(paths: &Paths) -> anyhow::Result<GlobalConfig> {
    use anyhow::Context;
    let file = paths.config_file();
    match std::fs::read_to_string(&file) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parse {}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GlobalConfig::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", file.display())),
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

/// The version of the server running for this data dir, with its URL and the master token.
async fn running_server() -> Option<(String, String, String)> {
    let (url, token) = crate::app::running_server().ok()?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().ok()?;
    let health: Value = http.get(format!("{url}/api/health")).send().await.ok()?.json().await.ok()?;
    (health["service"] == "workbench").then(|| (url, token, health["version"].as_str().unwrap_or("").to_string()))
}

/// `workbench update [--check] [--restart]`.
pub fn cli(args: UpdateArgs) -> anyhow::Result<()> {
    use anyhow::anyhow;
    let paths = Paths::from_env()?;
    let cfg = read_config(&paths)?;
    let source = Source::from_config(&cfg.update).map_err(|e| anyhow!(e))?.ok_or_else(|| anyhow!(NO_SOURCE))?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async {
        let current = Version::current();
        let client = client(&source).map_err(|e| anyhow!(e))?;
        let release = latest(&source, &client).await.map_err(|e| anyhow!(e))?;
        let mut on_disk = current;
        if release.version <= current {
            println!("Workbench {current} is the latest version.");
        } else {
            println!("Workbench {} is available (this is {current}).", release.version);
            if let Some(url) = &release.info.url {
                println!("  {url}");
            }
            if args.check {
                println!("Install it with `workbench update`.");
                return Ok(());
            }
            let exe = proc::current_exe()?;
            if let Some(why) = install_blocker(Some(&exe)) {
                return Err(anyhow!(why));
            }
            let shown = parking_lot::Mutex::new(0u64);
            let on = |step: Step| match step {
                Step::Download { received, total } => {
                    // A line per 10 MB: a log, not a terminal's progress bar.
                    let mut last = shown.lock();
                    if received == total || received >= *last + 10 * 1024 * 1024 {
                        *last = received;
                        eprintln!("  downloaded {} of {}", megabytes(received), megabytes(total));
                    }
                }
                Step::Verify => eprintln!("  checking the SHA-256"),
                Step::Install => eprintln!("  installing"),
            };
            let installed = install_release(&client, &release, &exe, &work_dir(&paths), &on).await.map_err(|e| anyhow!(e))?;
            println!("Installed Workbench {} to {}", installed.version, exe.display());
            if let Some(previous) = &installed.previous {
                println!("The version it replaced is {}", previous.display());
            }
            on_disk = installed.version;
        }
        if args.check {
            return Ok(());
        }
        match running_server().await {
            Some((_, _, version)) if version == on_disk.to_string() => {
                if args.restart {
                    println!("The running Workbench already is {version}: nothing to restart.");
                }
            }
            Some((url, token, version)) if args.restart => {
                let resp = reqwest::Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()?
                    .post(format!("{url}/api/platform/restart"))
                    .bearer_auth(&token)
                    .send()
                    .await
                    .map_err(|e| anyhow!("cannot reach Workbench at {url}: {}", e.without_url()))?;
                anyhow::ensure!(resp.status().is_success(), "Workbench at {url} answered {} to the restart request: restart it yourself", resp.status());
                println!("The running Workbench ({version}) is restarting into {on_disk}.");
            }
            Some((_, _, version)) => {
                println!("The running Workbench is still {version}. Restart it to use {on_disk}: `workbench update --restart`");
                println!("(its terminals stop; agent sessions come back when agents.restore_on_start is on).");
            }
            None => {}
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests;
