//! "Is this path gitignored?" for directory listings and new-directory watches.
//!
//! The `ignore` walker answers this implicitly while walking from the root; the
//! lazy tree lists one directory at a time, so we assemble the same layers
//! explicitly: every `.gitignore` from the directory up to the project root
//! (deepest wins), then `.git/info/exclude`, then the user's global excludes.

use std::path::Path;

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

pub struct IgnoreChecker {
    /// Highest precedence first.
    layers: Vec<Gitignore>,
}

impl IgnoreChecker {
    /// Matchers that apply to entries of `dir` (which must be inside `root`).
    pub fn for_dir(root: &Path, dir: &Path) -> Self {
        let mut layers = vec![];
        let mut cur = Some(dir);
        while let Some(d) = cur {
            if !d.starts_with(root) {
                break;
            }
            let f = d.join(".gitignore");
            if f.is_file() {
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
