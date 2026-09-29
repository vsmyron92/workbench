//! "Is this path gitignored?" for directory listings and new-directory watches.
//!
//! The `ignore` walker answers this implicitly while walking from the root; the
//! lazy tree lists one directory at a time, so we assemble the same layers
//! explicitly: every `.gitignore` from the directory up to the project root
//! (deepest wins), then `.git/info/exclude`, then the user's global excludes.

use std::path::Path;

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
/// folder above it holds one the walk reads no `.gitignore` or `.ignore` at all. Callers
/// may add options, but must not turn ignore files on again or replace `filter_entry`.
pub fn walk(start: &Path) -> WalkBuilder {
    let keep = |e: &ignore::DirEntry| {
        !HARD_IGNORE.contains(&e.file_name().to_string_lossy().as_ref())
            && !(e.file_type().is_some_and(|t| t.is_dir()) && ignore_file_leaves(e.path()))
    };
    let mut b = WalkBuilder::new(start);
    b.hidden(false).git_ignore(true).git_global(true).git_exclude(true).require_git(false).follow_links(false).filter_entry(keep);
    if start.ancestors().any(ignore_file_leaves) {
        tracing::warn!("{}: an ignore file there or above it links to another computer; walking it without ignore files", start.display());
        b.git_ignore(false).ignore(false);
    }
    b
}

pub struct IgnoreChecker {
    /// Highest precedence first.
    layers: Vec<Gitignore>,
}

impl IgnoreChecker {
    /// Matchers that apply to entries of `dir` (which must be inside `root`). A
    /// `.gitignore` that is a link to another computer (Windows) is not read.
    pub fn for_dir(root: &Path, dir: &Path) -> Self {
        let mut layers = vec![];
        let mut cur = Some(dir);
        while let Some(d) = cur {
            if !d.starts_with(root) {
                break;
            }
            let f = d.join(".gitignore");
            if !os::path::leaves_machine_below(d, &f) && f.is_file() {
                let (gi, _err) = Gitignore::new(&f);
                if !gi.is_empty() {
                    layers.push(gi);
                }
            }
            if d == root {
                break;
            }
            cur = d.parent();
        }
        let exclude = root.join(".git/info/exclude");
        if exclude.is_file() {
            let mut b = GitignoreBuilder::new(root);
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
            let mut v: Vec<String> = walk(start).build().flatten().filter(|e| e.depth() > 0).map(|e| crate::util::os::path::to_slash(e.path().strip_prefix(start).unwrap())).collect();
            v.sort();
            v
        };
        assert_eq!(entries(&root), ["b", "b/x.log"]);
        assert_eq!(entries(&root.join("a")), [".gitignore", "x.log"]);
        assert!(!IgnoreChecker::for_dir(&root, &root.join("a")).is_ignored(&root.join("a").join("x.log"), false));
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
