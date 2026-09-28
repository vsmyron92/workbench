//! `GET /api/projects/{pid}/files/find?q=&max=` — "Go to file".
//!
//! Each project's file list (gitignore-aware walk) is cached and invalidated by the
//! watcher on every `fs.changed`; a generation counter makes sure a list built
//! while files were changing is not trusted afterwards.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::fuzzy;
use super::{HARD_IGNORE, blocking};
use crate::app::AppState;
use crate::error::ApiResult;
use crate::projects::Project;

const MAX_INDEXED: usize = 200_000;
/// Safety net when a project has no working watcher.
const MAX_AGE: Duration = Duration::from_secs(120);

pub struct Index {
    root: PathBuf,
    generation: u64,
    built: Instant,
    pub files: Vec<String>,
    pub truncated: bool,
}

#[derive(Default)]
pub struct QuickOpenCache {
    indexes: Mutex<HashMap<String, Arc<Index>>>,
    generations: Mutex<HashMap<String, u64>>,
}

impl QuickOpenCache {
    /// Called by the watcher whenever files of `pid` change.
    pub fn invalidate(&self, pid: &str) {
        *self.generations.lock().entry(pid.to_string()).or_default() += 1;
    }

    fn generation(&self, pid: &str) -> u64 {
        self.generations.lock().get(pid).copied().unwrap_or(0)
    }

    pub async fn get(&self, project: &Project) -> ApiResult<Arc<Index>> {
        let generation = self.generation(&project.id);
        if let Some(i) = self.indexes.lock().get(&project.id) {
            if i.generation == generation && i.root == project.root && i.built.elapsed() < MAX_AGE {
                return Ok(i.clone());
            }
        }
        let root = project.root.clone();
        let (files, truncated) = blocking(move || Ok(build_index(&root))).await?;
        let idx = Arc::new(Index { root: project.root.clone(), generation, built: Instant::now(), files, truncated });
        self.indexes.lock().insert(project.id.clone(), idx.clone());
        Ok(idx)
    }
}

/// Every non-ignored file under `root`, relative and sorted.
pub fn build_index(root: &Path) -> (Vec<String>, bool) {
    let mut files = vec![];
    let mut truncated = false;
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| !HARD_IGNORE.contains(&e.file_name().to_string_lossy().as_ref()))
        .build();
    for ent in walk.flatten() {
        if !ent.file_type().is_some_and(|t| t.is_file() || t.is_symlink()) {
            continue;
        }
        if files.len() >= MAX_INDEXED {
            truncated = true;
            break;
        }
        if let Ok(rel) = ent.path().strip_prefix(root) {
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    files.sort();
    (files, truncated)
}

#[derive(Deserialize)]
pub struct FindQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub max: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindHit {
    pub path: String,
    pub score: i32,
    /// Matched characters as UTF-16 indices into `path`.
    pub positions: Vec<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindResult {
    pub results: Vec<FindHit>,
    /// Files that matched at all.
    pub matched: usize,
    /// Files in the index.
    pub indexed: usize,
    pub index_truncated: bool,
}

/// Unity writes a `.meta` next to every asset; they are rarely what you want to open.
const META_PENALTY: i32 = 25;

fn adjusted(score: i32, path: &str, q: &str) -> i32 {
    if path.ends_with(".meta") && !q.ends_with(".meta") { score - META_PENALTY } else { score }
}

/// Rank `files` for `q`; the best `max` with positions.
pub fn rank(files: &[String], q: &str, max: usize) -> (Vec<FindHit>, usize) {
    let Some(query) = fuzzy::Query::new(q) else { return (vec![], 0) };
    let mut scored: Vec<(i32, &String)> =
        files.iter().filter_map(|f| query.score(f).map(|s| (adjusted(s, f, q), f))).collect();
    let matched = scored.len();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
    scored.truncate(max);
    let hits = scored
        .into_iter()
        .filter_map(|(_, f)| {
            query
                .score_with_positions(f)
                .map(|(score, positions)| FindHit { path: f.clone(), score: adjusted(score, f, q), positions })
        })
        .collect();
    (hits, matched)
}

pub async fn find(
    State(state): State<AppState>,
    UrlPath(pid): UrlPath<String>,
    Query(q): Query<FindQuery>,
) -> ApiResult<Json<FindResult>> {
    let project = state.projects.require(&pid)?;
    let idx = state.files.quick.get(&project).await?;
    let max = q.max.unwrap_or(50).clamp(1, 500);
    let query = q.q;
    let idx2 = idx.clone();
    let (results, matched) = blocking(move || Ok(rank(&idx2.files, &query, max))).await?;
    Ok(Json(FindResult { results, matched, indexed: idx.files.len(), index_truncated: idx.truncated }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_respects_ignores_and_includes_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        for d in ["src", ".git", "node_modules/x", "dist"] {
            std::fs::create_dir_all(r.join(d)).unwrap();
        }
        std::fs::write(r.join(".gitignore"), "dist/\n").unwrap();
        for f in ["src/main.rs", ".gitlab-ci.yml", ".git/HEAD", "node_modules/x/i.js", "dist/app.js"] {
            std::fs::write(r.join(f), "").unwrap();
        }
        let (files, truncated) = build_index(r);
        assert_eq!(files, vec![".gitignore", ".gitlab-ci.yml", "src/main.rs"]);
        assert!(!truncated);
    }

    #[test]
    fn ranks_and_caps() {
        let files: Vec<String> = ["src/main.rs", "web/src/main.tsx", "docs/ARCHITECTURE.md", "src/app.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (hits, matched) = rank(&files, "main", 1);
        assert_eq!(matched, 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/main.rs");
        assert_eq!(hits[0].positions, vec![4, 5, 6, 7]);
        assert!(rank(&files, "", 10).0.is_empty());
    }

    #[test]
    fn unity_meta_files_rank_below_their_assets() {
        let files: Vec<String> = ["Assets/Player.cs.meta", "Assets/Player.cs"].iter().map(|s| s.to_string()).collect();
        assert_eq!(rank(&files, "player", 2).0[0].path, "Assets/Player.cs");
        assert_eq!(rank(&files, "player.cs.meta", 2).0[0].path, "Assets/Player.cs.meta");
    }
}
