//! Example cards: what a first start puts into the Home workspace, like Mr. Mak
//! Workspace's samples (MIT). They are marked `sample`, so the seven-day archive leaves
//! them alone until the user archives or deletes them.
//!
//! They are added only while Home has no registry (a first start, or a data directory
//! from before the examples), so an example the user archived or deleted never comes
//! back. Their files are compiled in; the screenshots are the ones in `docs/assets`.

use chrono::Utc;

use super::model::{self, Doc};
use super::store::{self, Scope};
use crate::error::{ApiError, ApiResult};
use crate::util;

struct Example {
    /// Card id, and the folder's name after the day.
    id: &'static str,
    title: &'static str,
    description: &'static str,
    category: &'static str,
    /// The card's picture, a file of the card.
    icon: Option<&'static str>,
    pinned: bool,
    /// Tabs in order: name and path in the card folder.
    steps: &'static [(&'static str, &'static str)],
    files: &'static [(&'static str, &'static [u8])],
}

macro_rules! card_file {
    ($card:literal, $path:literal) => {
        ($path, include_bytes!(concat!("examples/", $card, "/", $path)) as &[u8])
    };
}

macro_rules! screen {
    ($name:literal) => {
        (concat!("screens/", $name), include_bytes!(concat!("../../../docs/assets/", $name)) as &[u8])
    };
}

/// In list order: pinned first, then each one second older than the one before.
const EXAMPLES: &[Example] = &[
    Example {
        id: "welcome-to-workbench",
        title: "Welcome to Workbench",
        description: "Start a task with an agent, find your way around, and choose what to change.",
        category: "guide",
        icon: Some("cover.svg"),
        pinned: true,
        steps: &[("Get started", "get-started.md"), ("Everyday use", "everyday-use.md"), ("Make it yours", "make-it-yours.md")],
        files: &[
            card_file!("welcome", "get-started.md"),
            card_file!("welcome", "everyday-use.md"),
            card_file!("welcome", "make-it-yours.md"),
            card_file!("welcome", "cover.svg"),
        ],
    },
    Example {
        id: "workbench-tour",
        title: "A tour of Workbench",
        description: "Agents, CI, deliverables, services and the phone app, in screenshots from scratch projects.",
        category: "report",
        icon: None,
        pinned: false,
        steps: &[("Tour", "report.html"), ("Screenshots", "screens")],
        files: &[
            card_file!("tour", "report.html"),
            screen!("workbench-overview.png"),
            screen!("ci-tests.png"),
            screen!("workspace.png"),
            screen!("services.png"),
            screen!("database.png"),
            screen!("phone.png"),
        ],
    },
    Example {
        id: "hand-work-to-an-agent",
        title: "Hand work to an agent",
        description: "What to ask for, what the agent does with its Workspace tools, and a report template to copy.",
        category: "guide",
        icon: None,
        pinned: false,
        steps: &[("Ask for a card", "ask-for-a-card.md"), ("Report template", "template.html")],
        files: &[
            card_file!("agent-cards", "ask-for-a-card.md"),
            card_file!("agent-cards", "template.html"),
            card_file!("agent-cards", "img/latency.svg"),
            card_file!("agent-cards", "img/traffic.svg"),
        ],
    },
    Example {
        id: "connect-your-services",
        title: "Connect your services",
        description: "A checklist for GitLab, GitHub, Confluence, Jira, Docker, databases and your phone, with tokens kept out of files.",
        category: "setup",
        icon: None,
        pinned: false,
        steps: &[("Checklist", "report.html"), ("Connect a service", "connect.md")],
        files: &[card_file!("services", "report.html"), card_file!("services", "connect.md")],
    },
];

/// Add the examples to Home (see the module docs). Returns how many were added: none
/// once Home has a registry, even an empty one.
pub fn seed(scope: &Scope) -> ApiResult<usize> {
    if std::fs::symlink_metadata(scope.registry()).is_ok() {
        return Ok(0);
    }
    store::ensure_scope_dir(scope)?;
    let day = model::local_day();
    let now = Utc::now();
    let mut folders = vec![];
    let result = (|| -> ApiResult<usize> {
        let mut entries = vec![];
        for (i, ex) in EXAMPLES.iter().enumerate() {
            let folder = model::unique(&format!("{day}_{}", ex.id), |f| store::taken(&scope.dir, f));
            let dir = scope.dir.join(&folder);
            std::fs::create_dir(&dir)?;
            folders.push(dir.clone());
            for (path, body) in ex.files {
                util::fs::write_atomic(&dir.join(path), body, 0o644)?;
            }
            let stamp = (now - chrono::Duration::seconds(i as i64)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
            entries.push(entry(ex, &folder, &stamp));
        }
        store::update_registry(&scope.registry(), true, |doc| {
            let list = model::entities_mut(doc).ok_or_else(|| ApiError::internal("registry without entities"))?;
            if !list.is_empty() {
                // Someone made a card meanwhile: this is not a first start after all.
                return Err(ApiError::conflict("Home already has cards"));
            }
            list.extend(entries.iter().cloned());
            Ok(entries.len())
        })
    })();
    if result.is_err() {
        for dir in &folders {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    result
}

/// A registry entry in Mr. Mak's schema.
fn entry(ex: &Example, folder: &str, stamp: &str) -> Doc {
    let mut fields = vec![("id", Doc::str(ex.id)), ("title", Doc::str(ex.title)), ("description", Doc::str(ex.description))];
    if let Some(icon) = ex.icon {
        fields.push(("icon", Doc::str(icon)));
    }
    fields.extend([
        ("type", Doc::str("group")),
        ("category", Doc::str(ex.category)),
        ("created", Doc::str(stamp)),
        ("updated", Doc::str(stamp)),
        ("folder", Doc::str(folder)),
        ("steps", Doc::Array(ex.steps.iter().map(|(name, path)| store::step_doc(name, path, None)).collect())),
        ("status", Doc::str("active")),
        ("pinned", Doc::bool(ex.pinned)),
        ("sample", Doc::bool(true)),
        ("defaultStep", Doc::int(0)),
    ]);
    Doc::object(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_and_link_is_a_file_of_its_card() {
        for ex in EXAMPLES {
            let has = |p: &str| ex.files.iter().any(|(f, _)| *f == p || f.starts_with(&format!("{p}/")));
            for (name, path) in ex.steps {
                assert!(has(path), "{}: step {name:?} has no file {path}", ex.id);
            }
            assert!(ex.icon.is_none_or(has), "{}: no icon file", ex.id);
            assert!(model::valid_folder(&format!("2026-01-01_{}", ex.id)));
            for (path, body) in ex.files {
                assert!(store::check_rel(path).is_ok(), "{}: {path}", ex.id);
                let Ok(text) = std::str::from_utf8(body) else { continue };
                // Relative references stay in the card: reports are sandboxed and
                // markdown links to other files switch tabs.
                let refs = regex::Regex::new(r#"(?:src|href)="([^"]+)"|\]\(([^)]+)\)"#).unwrap();
                for c in refs.captures_iter(text) {
                    let r = c.get(1).or(c.get(2)).unwrap().as_str();
                    if r.starts_with("https://") || r.starts_with('#') {
                        continue;
                    }
                    let shared = r.strip_prefix("../_shared/").is_some_and(|f| ["report.css", "report.js"].contains(&f));
                    assert!(shared || has(r), "{}/{path} refers to {r}, which the card does not have", ex.id);
                }
            }
        }
    }
}
