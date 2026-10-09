//! "Is this path gitignored?" for directory listings and new-directory watches.
//!
//! The `ignore` walker answers this implicitly while walking from the root; the
//! lazy tree lists one directory at a time, so we assemble the same layers
//! explicitly: every `.gitignore` from the directory up to the project root
//! (deepest wins), then `.git/info/exclude`, then the user's global excludes.

use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{Match, WalkBuilder};

use super::HARD_IGNORE;
use crate::util::os;

/// The ignore files the `ignore` crate reads in each folder a walk visits, and in every
/// folder above where it starts (`.git/info/exclude` is git's own).
const IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];

/// Whether an ignore file of `dir` is a link that leaves this computer (Windows:
/// `os::path::leaves_machine_below`), which reading would connect to.
fn ignore_file_leaves(dir: &Path) -> bool {
    IGNORE_FILES.iter().any(|n| os::path::leaves_machine_below(dir, &dir.join(n)))
}

/// A walk of `start` as the files slice makes them (the tree's index, search, watches,
/// Local History): gitignore-aware (ignore files above `start`, `.git/info/exclude` and
/// the global excludes included; no repository needed), hidden files included, links
/// never followed, `HARD_IGNORE` folders left out. No ignore file is read through a link
/// to another computer (Windows): a folder holding one is left out, and when `start` or a
/// folder above it holds one the walk reads no `.gitignore` or `.ignore` at all. "Above"
/// is also above where `start`'s links lead (`os::path::ancestors_leave`): the crate reads
/// the ignore files of every folder above the resolved start, and a folder link the
/// watcher walks can lead below one that the walk from the root left out. `start` itself
/// must not be reached through a link to another computer (callers check). Callers may
/// add options, but must not turn ignore files on again or replace `filter_entry`.
///
/// `nested` are the working trees of the project's repositories below `start`
/// (`Project::nested_repo_dirs`). Their content belongs to them: the walk visits each as a
/// start of its own, so a root `.gitignore` that lists the clone (`web/`) does not hide what
/// is in it, and the walk from `start` leaves them out so nothing is visited twice.
pub fn walk(start: &Path, nested: &[PathBuf]) -> WalkBuilder {
    let tops: Vec<PathBuf> = nested.iter().filter(|t| t.starts_with(start) && *t != start).cloned().collect();
    let keep_tops = tops.clone();
    let keep = move |e: &ignore::DirEntry| {
        !HARD_IGNORE.contains(&e.file_name().to_string_lossy().as_ref())
            && !(e.file_type().is_some_and(|t| t.is_dir()) && ignore_file_leaves(e.path()))
            && !keep_tops.iter().any(|t| t == e.path())
    };
    let mut b = WalkBuilder::new(start);
    b.hidden(false).git_ignore(true).git_global(true).git_exclude(true).require_git(false).follow_links(false).filter_entry(keep);
    // An explicit start is never filtered or ignored by the walk itself.
    for top in &tops {
        if !ignore_file_leaves(top) && !os::path::leaves_machine(top) {
            b.add(top);
        }
    }
    if os::path::ancestors_leave(start, ignore_file_leaves) {
        tracing::warn!("{}: an ignore file there or above it (or above where its links lead) links to another computer; walking it without ignore files", start.display());
        b.git_ignore(false).ignore(false);
    }
    b
}

/// The folder whose ignore files govern `dir` first: the nearest folder from `dir` up to
/// `root` with a `.git` of its own (a clone below the root has its own ignore rules, as
/// git applies them), else `root`.
fn repo_top<'a>(root: &'a Path, dir: &'a Path) -> &'a Path {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d == root || !d.starts_with(root) {
            break;
        }
        if !os::path::leaves_machine_below(d, &d.join(".git")) && d.join(".git").exists() {
            return d;
        }
        cur = d.parent();
    }
    root
}

pub struct IgnoreChecker {
    /// Highest precedence first.
    layers: Vec<Gitignore>,
}

impl IgnoreChecker {
    /// Matchers that apply to entries of `dir` (which must be inside `root`): the
    /// `.gitignore` files from `dir` up to the top of its repository (`root`, or a clone
    /// below it that has a `.git` of its own). A `.gitignore` that is a link to another
    /// computer (Windows) is not read.
    pub fn for_dir(root: &Path, dir: &Path) -> Self {
        let mut layers = vec![];
        let base = repo_top(root, dir);
        let mut cur = Some(dir);
        while let Some(d) = cur {
            if !d.starts_with(base) {
                break;
            }
            let f = d.join(".gitignore");
            if !os::path::leaves_machine_below(d, &f) && f.is_file() {
                let (gi, _err) = Gitignore::new(&f);
                if !gi.is_empty() {
                    layers.push(gi);
                }
            }
            if d == base {
                break;
            }
            cur = d.parent();
        }
        let exclude = base.join(".git/info/exclude");
        if exclude.is_file() {
            let mut b = GitignoreBuilder::new(base);
            b.add(&exclude);
            if let Ok(gi) = b.build() {
                if !gi.is_empty() {
                    layers.push(gi);
                }
            }
        }
        let (global, _err) = GitignoreBuilder::new(root).build_global();
        if !global.is_empty() {
            layers.push(global);
        }
        Self { layers }
    }

    /// Whether `path` (absolute, under the root) or one of its parents is ignored.
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        for gi in &self.layers {
            // `matched_path_or_any_parents` panics for paths outside the matcher's root.
            if !path.starts_with(gi.path()) {
                continue;
            }
            match gi.matched_path_or_any_parents(path, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_gitignores_and_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/gen")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join(".gitignore"), "/target\n*.log\n").unwrap();
        std::fs::write(root.join("src/.gitignore"), "gen/\n!keep.log\n").unwrap();

        let top = IgnoreChecker::for_dir(root, root);
        assert!(top.is_ignored(&root.join("target"), true));
        assert!(top.is_ignored(&root.join("x.log"), false));
        assert!(!top.is_ignored(&root.join("src"), true));

        let src = IgnoreChecker::for_dir(root, &root.join("src"));
        assert!(src.is_ignored(&root.join("src/gen"), true));
        assert!(src.is_ignored(&root.join("src/a.log"), false));
        assert!(!src.is_ignored(&root.join("src/keep.log"), false));
        assert!(!src.is_ignored(&root.join("src/main.rs"), false));

        // Listing inside an ignored directory: every child is ignored.
        let inner = IgnoreChecker::for_dir(root, &root.join("target/debug"));
        assert!(inner.is_ignored(&root.join("target/debug/app"), false));
    }

    /// An ignore file that links to another computer is never read (Windows): a folder
    /// holding one is left out of walks, a walk starting there reads no ignore files, and
    /// `IgnoreChecker` skips it. (Its target is this computer's own share, where it would
    /// be found and would ignore `*.log`.)
    #[cfg(windows)]
    #[test]
    fn ignore_files_linked_to_network_paths_are_not_read() {
        use crate::util::os::path::{canonicalize, loopback_share, remote_link_or_skip};
        let (dir, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (root, far) = (canonicalize(dir.path()).unwrap(), canonicalize(elsewhere.path()).unwrap());
        std::fs::write(far.join("ignore"), "*.log\n").unwrap();
        for d in ["a", "b"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("x.log"), "").unwrap();
        }
        if !remote_link_or_skip(&loopback_share(&far).join("ignore"), &root.join("a").join(".gitignore"), false) {
            return;
        }
        let entries = |start: &Path| -> Vec<String> {
            let mut v: Vec<String> = walk(start, &[]).build().flatten().filter(|e| e.depth() > 0).map(|e| crate::util::os::path::to_slash(e.path().strip_prefix(start).unwrap())).collect();
            v.sort();
            v
        };
        assert_eq!(entries(&root), ["b", "b/x.log"]);
        assert_eq!(entries(&root.join("a")), [".gitignore", "x.log"]);
        assert!(!IgnoreChecker::for_dir(&root, &root.join("a")).is_ignored(&root.join("a").join("x.log"), false));

        // A folder link into the folder left out (the watcher walks one a checkout makes):
        // the crate reads the ignore files above where it leads, `a`'s among them.
        std::fs::create_dir(root.join("a").join("deep")).unwrap();
        std::fs::write(root.join("a").join("deep").join("y.log"), "").unwrap();
        std::os::windows::fs::symlink_dir(Path::new("a").join("deep"), root.join("lnk")).unwrap();
        assert_eq!(entries(&root.join("lnk")), ["y.log"]);
    }

    /// A clone below the root has its own ignore rules: the root's `.gitignore` lists the clone
    /// (`web/`) and its patterns (`*.log`) do not reach into it, as in git. The walk visits the
    /// clone once, whether the root ignores it or not.
    #[test]
    fn a_clone_below_the_root_is_governed_by_its_own_ignore_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in ["web/.git", "web/src", "tools/.git", "src"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join(".gitignore"), "web/\n*.log\n").unwrap();
        std::fs::write(root.join("web/.gitignore"), "dist/\n").unwrap();
        for f in ["web/src/app.js", "web/run.log", "web/dist.txt", "tools/t.sh", "src/main.rs", "src/x.log"] {
            std::fs::write(root.join(f), "").unwrap();
        }
        // The folder itself is the root's to ignore; what is in it is not.
        let top = IgnoreChecker::for_dir(root, root);
        assert!(top.is_ignored(&root.join("web"), true));
        let web = IgnoreChecker::for_dir(root, &root.join("web"));
        assert!(!web.is_ignored(&root.join("web/src"), true));
        assert!(!web.is_ignored(&root.join("web/run.log"), false), "the root's `*.log` stops at the clone");
        assert!(web.is_ignored(&root.join("web/dist"), true), "its own .gitignore applies");
        assert!(!IgnoreChecker::for_dir(root, &root.join("web/src")).is_ignored(&root.join("web/src/app.js"), false));
        assert!(IgnoreChecker::for_dir(root, &root.join("src")).is_ignored(&root.join("src/x.log"), false));

        let tops = [root.join("web"), root.join("tools")];
        let entries = |nested: &[PathBuf]| -> Vec<String> {
            let mut v: Vec<String> = walk(root, nested)
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .map(|e| crate::util::os::path::to_slash(e.path().strip_prefix(root).unwrap()))
                .collect();
            v.sort();
            v
        };
        // Without the clones listed, the root's rules hide `web` (and `tools` is walked as a folder).
        assert_eq!(entries(&[]), [".gitignore", "src/main.rs", "tools/t.sh"]);
        // With them, each is walked once: `tools` is not repeated, `web` is no longer hidden.
        // (The `ignore` crate reads the ignore files above every start it walks, so the root's
        // `*.log` still reaches `web/run.log` here, unlike in git and in `IgnoreChecker`.)
        assert_eq!(entries(&tops), [".gitignore", "src/main.rs", "tools/t.sh", "web/.gitignore", "web/dist.txt", "web/src/app.js"]);
    }

    #[test]
    fn info_exclude_applies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git/info")).unwrap();
        std::fs::write(root.join(".git/info/exclude"), "scratch/\n").unwrap();
        let c = IgnoreChecker::for_dir(root, root);
        assert!(c.is_ignored(&root.join("scratch"), true));
        assert!(!c.is_ignored(&root.join("src"), true));
    }
}
