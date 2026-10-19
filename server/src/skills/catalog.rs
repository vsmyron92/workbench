//! Workbench's own skills: a few that teach an agent to work well inside Workbench, embedded in the
//! program (`skills/` at the top of the repository) and installed only when the user asks.
//!
//! Installing copies a skill's files into an account's own skills folder (`<account>/skills/<name>/`)
//! and writes a marker next to them listing the files and their hashes. The marker is how Workbench
//! knows the folder is its own, whether the user has edited it since, and which files an update
//! replaces or removes. Nothing outside the files Workbench wrote is ever changed or deleted: a
//! folder of the same name that has no marker belongs to someone else and is left alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::discover::{Account, Cli};
use super::parse;
use crate::config::contract_tilde;
use crate::error::ApiError;
use crate::util::os::path as ospath;

#[derive(RustEmbed)]
#[folder = "../skills/"]
#[allow_missing = true]
struct Bundle;

/// The marker file inside an installed skill's folder.
pub const MARKER: &str = ".workbench-skill.json";
/// Files of one skill and bytes of one file: a bound on what a corrupt bundle or folder can cost.
const MAX_FILES: usize = 64;
const MAX_FILE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone)]
pub struct File {
    /// Relative to the skill's folder, `/`-separated.
    pub path: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    /// `SKILL.md` first, then the rest by path.
    pub files: Vec<File>,
    pub description: String,
}

impl Skill {
    pub fn skill_md(&self) -> &str {
        self.files.first().and_then(|f| std::str::from_utf8(&f.data).ok()).unwrap_or("")
    }

    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.data.len() as u64).sum()
    }

    fn hashes(&self) -> BTreeMap<String, String> {
        self.files.iter().map(|f| (f.path.clone(), sha(&f.data))).collect()
    }

    pub fn file(&self, path: &str) -> Option<&File> {
        self.files.iter().find(|f| f.path == path)
    }
}

fn sha(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Every skill in the bundle, by name. A folder without a `SKILL.md`, a name the standard does not
/// allow, or a file that is not text is left out (a test keeps the bundle clean).
pub fn bundled() -> Vec<Skill> {
    let mut by_name: BTreeMap<String, Vec<File>> = BTreeMap::new();
    for path in Bundle::iter() {
        let Some((name, rest)) = path.split_once('/') else { continue };
        if !parse::valid_name(name) || ospath::check_relative(rest).is_err() {
            continue;
        }
        let Some(file) = Bundle::get(&path) else { continue };
        if file.data.len() > MAX_FILE_BYTES || std::str::from_utf8(&file.data).is_err() {
            continue;
        }
        let files = by_name.entry(name.to_string()).or_default();
        if files.len() < MAX_FILES {
            files.push(File { path: rest.to_string(), data: file.data.to_vec() });
        }
    }
    by_name
        .into_iter()
        .filter_map(|(name, mut files)| {
            files.sort_by(|a, b| (a.path != "SKILL.md", &a.path).cmp(&(b.path != "SKILL.md", &b.path)));
            let md = files.first().filter(|f| f.path == "SKILL.md")?;
            let p = parse::parse(std::str::from_utf8(&md.data).ok()?);
            let description = p.description().or_else(|| parse::first_line(&p.body)).unwrap_or_default();
            Some(Skill { name, files, description })
        })
        .collect()
}

pub fn find(name: &str) -> Option<Skill> {
    bundled().into_iter().find(|s| s.name == name)
}

// ---------------------------------------------------------------- the marker

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Marker {
    skill: String,
    /// The Workbench version that installed it.
    version: String,
    /// Each file Workbench wrote and the SHA-256 it had.
    files: BTreeMap<String, String>,
}

fn read_marker(dir: &Path) -> Option<Marker> {
    let path = dir.join(MARKER);
    let m = std::fs::metadata(&path).ok()?;
    if !m.is_file() || m.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(&path).ok()?).ok()
}

/// Whether the bundle has a skill of this name.
pub fn is_bundled(name: &str) -> bool {
    parse::valid_name(name) && Bundle::get(&format!("{name}/SKILL.md")).is_some()
}

/// The Workbench skill a folder is, when it was installed from the bundle: it is named after a skill the bundle
/// has, and its marker holds up (`trusted_marker`). A marker on any other folder, or one that does not hold up,
/// says nothing: anything that can write there could have written it.
pub fn installed_as(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_str()?;
    // Nearly no skill folder has a marker: the cheap check goes first.
    if !dir.join(MARKER).is_file() || !is_bundled(name) {
        return None;
    }
    trusted_marker(dir, name).map(|_| name.to_string())
}

fn hash_file(path: &Path) -> Option<String> {
    let m = std::fs::metadata(path).ok()?;
    if !m.is_file() || m.len() > MAX_FILE_BYTES as u64 {
        return None;
    }
    std::fs::read(path).ok().map(|b| sha(&b))
}

/// The marker of `dir` when it names `skill`, with each file it lists as a path inside the folder. The
/// marker is data on the user's disk, not Workbench's word (anything that can write in the folder can write
/// one): a listed path that is absolute, climbs out with `..`, or goes through a link that leaves the folder
/// makes the whole marker unusable, so nothing is ever read, replaced or deleted outside the skill's folder
/// because of what a marker says.
fn trusted_marker(dir: &Path, skill: &str) -> Option<(Marker, Vec<(String, PathBuf)>)> {
    let m = read_marker(dir).filter(|m| m.skill == skill)?;
    let mut paths = Vec::with_capacity(m.files.len());
    for rel in m.files.keys() {
        paths.push((rel.clone(), crate::util::paths::resolve_in_root(dir, rel).ok()?));
    }
    Some((m, paths))
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Not installed here.
    Available,
    /// Installed, and the same as this Workbench's.
    Installed,
    /// Installed from an older Workbench; nothing edited.
    Update,
    /// Installed, and a file was edited or deleted since.
    Modified,
    /// A folder of that name exists that Workbench did not make.
    Foreign,
}

/// The state of `skill` in an account's skills folder.
pub fn state(skill: &Skill, skills_dir: &Path) -> State {
    let dir = skills_dir.join(&skill.name);
    if std::fs::symlink_metadata(&dir).is_err() {
        return State::Available;
    }
    let Some((marker, paths)) = trusted_marker(&dir, &skill.name) else { return State::Foreign };
    if paths.iter().any(|(rel, path)| hash_file(path).as_ref() != marker.files.get(rel)) {
        return State::Modified;
    }
    if marker.files == skill.hashes() { State::Installed } else { State::Update }
}

// ---------------------------------------------------------------- install and remove

fn conflict(what: &str, dir: &Path) -> ApiError {
    ApiError::conflict(format!("{what}: {}", contract_tilde(dir)))
}

fn io_error(what: &str, e: std::io::Error) -> ApiError {
    ApiError::internal(format!("{what}: {}", e.kind()))
}

fn mode_of(rel: &str) -> u32 {
    if rel.starts_with("scripts/") { 0o755 } else { 0o644 }
}

/// Copy `skill` into `skills_dir/<name>/`: a new install, an update of one nobody edited, or (with
/// `force`) the restoring of one that was edited. Returns the state afterwards.
pub fn install(skill: &Skill, skills_dir: &Path, force: bool) -> Result<State, ApiError> {
    let dir = skills_dir.join(&skill.name);
    let st = state(skill, skills_dir);
    match st {
        State::Installed => return Ok(st),
        State::Foreign => return Err(conflict("A skill with this name is already there and Workbench did not make it, so it is left alone", &dir)),
        State::Modified if !force => return Err(conflict("You changed this skill since it was installed; installing again would replace your edits", &dir)),
        _ => {}
    }
    let old = trusted_marker(&dir, &skill.name);
    let marker = Marker { skill: skill.name.clone(), version: env!("CARGO_PKG_VERSION").into(), files: skill.hashes() };
    let write = |base: &Path| -> Result<(), ApiError> {
        std::fs::create_dir_all(base).map_err(|e| io_error("The skill's folder could not be created", e))?;
        for f in &skill.files {
            // Into an existing folder (an update), a link inside it that leaves it is refused too.
            let path = crate::util::paths::resolve_in_root(base, &f.path).map_err(|_| conflict("A file of this skill would land outside its folder (a link leads out of it)", &dir))?;
            crate::util::fs::write_atomic(&path, &f.data, mode_of(&f.path)).map_err(|e| ApiError::internal(format!("{e:#}")))?;
        }
        let json = serde_json::to_vec_pretty(&marker).map_err(|e| ApiError::internal(e.to_string()))?;
        crate::util::fs::write_atomic(&base.join(MARKER), &json, 0o644).map_err(|e| ApiError::internal(format!("{e:#}")))
    };
    if st == State::Available {
        // Into a hidden folder first and renamed into place, so a failure leaves nothing half-installed
        // (which would look like a folder of someone else's).
        std::fs::create_dir_all(skills_dir).map_err(|e| io_error("The skills folder could not be created", e))?;
        let tmp = skills_dir.join(format!(".workbench-install-{}", crate::util::random_token(6)));
        let done = write(&tmp).and_then(|_| std::fs::rename(&tmp, &dir).map_err(|e| io_error("The skill could not be put in place", e)));
        if done.is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        done?;
    } else {
        write(&dir)?;
        // Files an older version had and this one does not.
        if let Some((old, paths)) = old {
            for (_, path) in paths.iter().filter(|(rel, _)| !marker.files.contains_key(rel)) {
                let _ = std::fs::remove_file(path);
            }
            prune_empty(&dir, old.files.keys());
        }
    }
    Ok(State::Installed)
}

/// Remove the files Workbench wrote (and the marker), then the folders that are left empty. What
/// the user added to the folder stays, and so does the folder then.
pub fn remove(name: &str, skills_dir: &Path, force: bool) -> Result<(), ApiError> {
    let skill = find(name).ok_or_else(|| ApiError::not_found(format!("Workbench has no skill called {name:?}")))?;
    remove_skill(&skill, skills_dir, force)
}

fn remove_skill(skill: &Skill, skills_dir: &Path, force: bool) -> Result<(), ApiError> {
    let dir = skills_dir.join(&skill.name);
    match state(skill, skills_dir) {
        State::Available => return Ok(()),
        State::Foreign => return Err(conflict("This skill was not installed by Workbench, so it is left alone", &dir)),
        State::Modified if !force => return Err(conflict("You changed this skill since it was installed; removing it would delete your edits", &dir)),
        _ => {}
    }
    let (marker, paths) = trusted_marker(&dir, &skill.name).ok_or_else(|| conflict("This skill has no Workbench marker that can be trusted", &dir))?;
    for (_, path) in &paths {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_file(dir.join(MARKER));
    prune_empty(&dir, marker.files.keys());
    Ok(())
}

/// Remove `dir` and the folders between it and the given files when they hold nothing now.
fn prune_empty<'a>(dir: &Path, rels: impl Iterator<Item = &'a String>) {
    let mut folders: Vec<PathBuf> = rels.filter_map(|r| Path::new(r).parent().map(|p| dir.join(p))).filter(|p| p != dir).collect();
    // Deepest first.
    folders.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    folders.dedup();
    for f in folders {
        let _ = std::fs::remove_dir(&f);
    }
    let _ = std::fs::remove_dir(dir);
}

// ---------------------------------------------------------------- what the UI lists

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    pub account: String,
    pub state: State,
    /// The folder it is, or would be, installed in.
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub name: String,
    pub description: String,
    pub files: usize,
    pub bytes: u64,
    pub lines: usize,
    /// The accounts it can be installed for (Claude Code and Codex: the CLIs that get Workbench's tools).
    pub installs: Vec<InstallInfo>,
}

/// Whether Workbench's skills are offered for this account: a Claude Code or Codex one that is set up (those get the MCP tools).
pub fn is_target(a: &Account) -> bool {
    a.mcp && a.enabled && a.installed && matches!(a.cli, Cli::Claude | Cli::Codex)
}

/// The accounts Workbench's skills are offered for.
pub fn targets(accounts: &[Account]) -> impl Iterator<Item = &Account> {
    accounts.iter().filter(|a| is_target(a))
}

pub fn entries(accounts: &[Account]) -> Vec<CatalogEntry> {
    bundled()
        .into_iter()
        .map(|s| CatalogEntry {
            installs: targets(accounts).map(|a| InstallInfo { account: a.id.clone(), state: state(&s, &a.skills_dir()), path: contract_tilde(&a.skills_dir().join(&s.name)) }).collect(),
            description: s.description.clone(),
            files: s.files.len(),
            bytes: s.bytes(),
            lines: s.skill_md().lines().count(),
            name: s.name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, files: &[(&str, &str)]) -> Skill {
        Skill { name: name.into(), description: String::new(), files: files.iter().map(|(p, d)| File { path: (*p).into(), data: d.as_bytes().to_vec() }).collect() }
    }

    #[test]
    fn install_update_modify_and_remove_touch_only_what_workbench_wrote() {
        let t = tempfile::tempdir().unwrap();
        let skills = t.path().join("acct").join("skills");
        let v1 = skill("demo", &[("SKILL.md", "one\n"), ("references/a.md", "a\n"), ("references/old.md", "old\n")]);
        assert_eq!(state(&v1, &skills), State::Available);
        assert_eq!(install(&v1, &skills, false).unwrap(), State::Installed);
        assert_eq!(std::fs::read_to_string(skills.join("demo/references/a.md")).unwrap(), "a\n");
        assert!(skills.join("demo").join(MARKER).is_file());
        // No half-made folder is left behind.
        assert_eq!(std::fs::read_dir(&skills).unwrap().count(), 1);
        assert_eq!(state(&v1, &skills), State::Installed);
        // Installing again is a no-op.
        assert_eq!(install(&v1, &skills, false).unwrap(), State::Installed);

        // A newer Workbench: one file changed, one gone, one new. Files the user added stay.
        std::fs::write(skills.join("demo/mine.txt"), "mine").unwrap();
        let v2 = skill("demo", &[("SKILL.md", "two\n"), ("references/a.md", "a\n"), ("references/new.md", "new\n")]);
        assert_eq!(state(&v2, &skills), State::Update);
        assert_eq!(install(&v2, &skills, false).unwrap(), State::Installed);
        assert_eq!(std::fs::read_to_string(skills.join("demo/SKILL.md")).unwrap(), "two\n");
        assert!(!skills.join("demo/references/old.md").exists());
        assert!(skills.join("demo/references/new.md").is_file() && skills.join("demo/mine.txt").is_file());
        assert_eq!(state(&v2, &skills), State::Installed);

        // The user edits a file: it is theirs now, and only force replaces it.
        std::fs::write(skills.join("demo/SKILL.md"), "edited\n").unwrap();
        assert_eq!(state(&v2, &skills), State::Modified);
        assert!(install(&v2, &skills, false).is_err());
        assert!(remove("demo", &skills, false).is_err());
        assert_eq!(std::fs::read_to_string(skills.join("demo/SKILL.md")).unwrap(), "edited\n");
        assert_eq!(install(&v2, &skills, true).unwrap(), State::Installed);
        assert_eq!(std::fs::read_to_string(skills.join("demo/SKILL.md")).unwrap(), "two\n");

        // A deleted file counts as an edit too.
        std::fs::remove_file(skills.join("demo/references/a.md")).unwrap();
        assert_eq!(state(&v2, &skills), State::Modified);
        install(&v2, &skills, true).unwrap();
        assert_eq!(state(&v2, &skills), State::Installed);
    }

    #[test]
    fn a_folder_workbench_did_not_make_is_never_touched() {
        let t = tempfile::tempdir().unwrap();
        let skills = t.path().join("skills");
        std::fs::create_dir_all(skills.join("demo")).unwrap();
        std::fs::write(skills.join("demo/SKILL.md"), "someone else's").unwrap();
        let s = skill("demo", &[("SKILL.md", "ours\n")]);
        assert_eq!(state(&s, &skills), State::Foreign);
        assert!(install(&s, &skills, false).is_err());
        assert!(install(&s, &skills, true).is_err(), "force does not take over a foreign folder");
        assert!(remove_skill(&s, &skills, true).is_err());
        assert_eq!(std::fs::read_to_string(skills.join("demo/SKILL.md")).unwrap(), "someone else's");
        // A marker naming another skill does not count either.
        std::fs::write(skills.join("demo").join(MARKER), r#"{"skill":"other","version":"0","files":{}}"#).unwrap();
        assert_eq!(state(&s, &skills), State::Foreign);
        assert!(remove_skill(&s, &skills, true).is_err());
        // A name that is not in the bundle is refused before anything is looked at.
        assert_eq!(remove("../../etc", &skills, true).unwrap_err().status, axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn remove_deletes_the_installed_files_and_keeps_what_the_user_added() {
        let t = tempfile::tempdir().unwrap();
        let skills = t.path().join("skills");
        let s = skill("demo", &[("SKILL.md", "x\n"), ("references/a.md", "a\n")]);
        let dir = skills.join("demo");
        install(&s, &skills, false).unwrap();
        remove_skill(&s, &skills, false).unwrap();
        assert!(!dir.exists(), "an empty skill folder goes with its files");
        assert_eq!(state(&s, &skills), State::Available);
        remove_skill(&s, &skills, false).unwrap();
        // With a file of the user's in it, the folder stays.
        install(&s, &skills, false).unwrap();
        std::fs::write(dir.join("mine.txt"), "m").unwrap();
        remove_skill(&s, &skills, false).unwrap();
        assert!(dir.join("mine.txt").is_file() && !dir.join("references").exists() && !dir.join("SKILL.md").exists() && !dir.join(MARKER).exists());
        // An edited skill goes only with force.
        let t2 = tempfile::tempdir().unwrap();
        let skills = t2.path().join("skills");
        install(&s, &skills, false).unwrap();
        std::fs::write(skills.join("demo/SKILL.md"), "mine\n").unwrap();
        assert!(remove_skill(&s, &skills, false).is_err());
        remove_skill(&s, &skills, true).unwrap();
        assert!(!skills.join("demo").exists());
    }

    #[test]
    fn a_marker_that_names_files_outside_the_folder_is_not_believed() {
        let t = tempfile::tempdir().unwrap();
        let skills = t.path().join("skills");
        let s = skill("demo", &[("SKILL.md", "ours\n")]);
        let outside = t.path().join("precious.txt");
        std::fs::write(&outside, "keep me").unwrap();
        let sha = |p: &Path| sha(&std::fs::read(p).unwrap());
        let marker_with = |key: &str| {
            let mut files = BTreeMap::new();
            files.insert("SKILL.md".to_string(), sha(&skills.join("demo/SKILL.md")));
            files.insert(key.to_string(), sha(&outside));
            serde_json::to_string(&Marker { skill: "demo".into(), version: "0".into(), files }).unwrap()
        };
        let abs = outside.to_string_lossy().replace('\\', "/");
        for key in ["../../precious.txt".to_string(), abs, "./../../precious.txt".to_string()] {
            std::fs::remove_dir_all(skills.join("demo")).ok();
            install(&s, &skills, false).unwrap();
            // The hash is right, so only the path can give it away.
            std::fs::write(skills.join("demo").join(MARKER), marker_with(&key)).unwrap();
            assert_eq!(state(&s, &skills), State::Foreign, "{key}");
            assert!(install(&s, &skills, true).is_err(), "{key}");
            assert!(remove_skill(&s, &skills, true).is_err(), "{key}");
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep me", "{key}");
            assert!(skills.join("demo/SKILL.md").is_file(), "{key}: nothing of the folder was removed either");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_folder_cannot_carry_an_update_or_a_removal_out_of_it() {
        let t = tempfile::tempdir().unwrap();
        let (skills, outside) = (t.path().join("skills"), t.path().join("outside"));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("a.md"), "keep me").unwrap();
        let v1 = skill("demo", &[("SKILL.md", "one\n"), ("references/a.md", "a\n")]);
        install(&v1, &skills, false).unwrap();
        // `references` becomes a link to somewhere else, as a hostile process could make it.
        std::fs::remove_dir_all(skills.join("demo/references")).unwrap();
        std::os::unix::fs::symlink(&outside, skills.join("demo/references")).unwrap();
        // The marker is honest, but one of its files now lies behind a link that leaves the folder.
        assert_eq!(state(&v1, &skills), State::Foreign);
        let v2 = skill("demo", &[("SKILL.md", "two\n"), ("references/a.md", "changed\n")]);
        assert!(install(&v2, &skills, true).is_err());
        assert!(remove_skill(&v1, &skills, true).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("a.md")).unwrap(), "keep me");
    }

    #[test]
    fn the_bundle_is_a_set_of_valid_skills() {
        let all = bundled();
        assert!(!all.is_empty(), "skills/ holds the optional Workbench skills");
        let mut names = std::collections::BTreeSet::new();
        for s in &all {
            assert!(names.insert(s.name.clone()), "{} twice", s.name);
            assert!(parse::valid_name(&s.name), "{}", s.name);
            assert_eq!(s.files[0].path, "SKILL.md");
            let p = parse::parse(s.skill_md());
            assert!(p.error.is_none(), "{}: {:?}", s.name, p.error);
            assert_eq!(p.name().as_deref(), Some(s.name.as_str()), "the front matter's name is the folder's");
            let d = p.description().unwrap_or_default();
            assert!(!d.is_empty() && d.chars().count() <= parse::SPEC_DESCRIPTION_MAX, "{}: description of {} characters", s.name, d.chars().count());
            // Only the portable front matter: other agents reject the rest.
            for k in p.fields.keys() {
                assert!(["name", "description", "license", "compatibility", "metadata"].contains(&k.as_str()), "{}: front matter key {k:?} is not portable", s.name);
            }
            assert!(s.skill_md().lines().count() <= 300, "{} is too long for a skill", s.name);
            for f in &s.files {
                assert!(!f.data.is_empty() && f.data.ends_with(b"\n") && !f.data.contains(&b'\r'), "{}/{}: LF text with a final newline", s.name, f.path);
            }
        }
    }
}
