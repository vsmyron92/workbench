//! The card registry: Mr. Mak Workspace's `workspace.json` schema.
//!
//! Adapted from Mr. Mak Workspace (MIT): the entity schema, the freshness sort
//! (`updated ?? created`, newest first; status only breaks ties) and auto-archive
//! (not touched for 7 days, unless pinned or a sample; an explicit `archived`
//! status always wins).
//!
//! Registries are edited concurrently by agents, repositories and us, so the file
//! is read leniently (a malformed entity is skipped, not fatal) and written back
//! through [`Doc`], an order-preserving JSON tree: unknown keys, key order and
//! entities we could not parse all survive a write.

use std::fmt;

use chrono::{Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Value;

/// Cards not touched for this many days are archived (unless pinned or samples).
pub const ARCHIVE_DAYS: i64 = 7;

/// Step viewers an agent or the UI may set. `auto` (the default) picks by extension.
pub const VIEWERS: &[&str] = &["auto", "html", "markdown", "image", "gallery", "compare3d", "pdf", "video", "audio", "text"];

pub const STATUSES: &[&str] = &["active", "done", "archived"];

// ---------------------------------------------------------------- order-preserving JSON

/// A JSON value that keeps object keys in document order.
#[derive(Debug, Clone, PartialEq)]
pub enum Doc {
    Object(Vec<(String, Doc)>),
    Array(Vec<Doc>),
    /// null, bool, number or string.
    Scalar(Value),
}

impl Doc {
    pub fn parse(bytes: &[u8]) -> Result<Doc, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Two-space indentation plus a final newline, as `JSON.stringify(v, null, 2)` writers produce.
    pub fn to_pretty(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec_pretty(self).unwrap_or_else(|_| b"{}".to_vec());
        out.push(b'\n');
        out
    }

    pub fn str(s: impl Into<String>) -> Doc {
        Doc::Scalar(Value::String(s.into()))
    }

    pub fn bool(b: bool) -> Doc {
        Doc::Scalar(Value::Bool(b))
    }

    pub fn int(n: i64) -> Doc {
        Doc::Scalar(Value::from(n))
    }

    pub fn object(pairs: Vec<(&str, Doc)>) -> Doc {
        Doc::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Doc> {
        match self {
            Doc::Object(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Doc> {
        match self {
            Doc::Object(pairs) => pairs.iter_mut().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Replace `key` in place, or append it.
    pub fn set(&mut self, key: &str, value: Doc) {
        self.set_after(key, value, &[]);
    }

    /// Replace `key` in place; a new key goes right after the first of `after` that
    /// exists (so `updated` lands next to `created`), else at the end.
    pub fn set_after(&mut self, key: &str, value: Doc, after: &[&str]) {
        let Doc::Object(pairs) = self else { return };
        if let Some(slot) = pairs.iter_mut().rev().find(|(k, _)| k == key) {
            slot.1 = value;
            return;
        }
        let at = after.iter().find_map(|a| pairs.iter().position(|(k, _)| k == a)).map(|i| i + 1);
        match at {
            Some(i) => pairs.insert(i, (key.to_string(), value)),
            None => pairs.push((key.to_string(), value)),
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Doc> {
        let Doc::Object(pairs) = self else { return None };
        let mut removed = None;
        pairs.retain_mut(|(k, v)| {
            if k == key {
                removed = Some(std::mem::replace(v, Doc::Scalar(Value::Null)));
                false
            } else {
                true
            }
        });
        removed
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Doc::Scalar(Value::String(s)) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Doc::Scalar(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Doc::Scalar(v) => v.as_i64(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Doc>> {
        match self {
            Doc::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Doc>> {
        match self {
            Doc::Array(a) => Some(a),
            _ => None,
        }
    }

    fn string_field(&self, key: &str) -> Option<String> {
        self.get(key).and_then(Doc::as_str).map(str::to_string)
    }
}

impl Serialize for Doc {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Doc::Scalar(v) => v.serialize(s),
            Doc::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for i in items {
                    seq.serialize_element(i)?;
                }
                seq.end()
            }
            Doc::Object(pairs) => {
                let mut map = s.serialize_map(Some(pairs.len()))?;
                for (k, v) in pairs {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Doc {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Doc, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Doc;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E>(self, b: bool) -> Result<Doc, E> {
                Ok(Doc::Scalar(Value::Bool(b)))
            }
            fn visit_i64<E>(self, n: i64) -> Result<Doc, E> {
                Ok(Doc::Scalar(Value::from(n)))
            }
            fn visit_u64<E>(self, n: u64) -> Result<Doc, E> {
                Ok(Doc::Scalar(Value::from(n)))
            }
            fn visit_f64<E>(self, n: f64) -> Result<Doc, E> {
                Ok(Doc::Scalar(serde_json::Number::from_f64(n).map(Value::Number).unwrap_or(Value::Null)))
            }
            fn visit_str<E>(self, s: &str) -> Result<Doc, E> {
                Ok(Doc::str(s))
            }
            fn visit_string<E>(self, s: String) -> Result<Doc, E> {
                Ok(Doc::str(s))
            }
            fn visit_unit<E>(self) -> Result<Doc, E> {
                Ok(Doc::Scalar(Value::Null))
            }
            fn visit_none<E>(self) -> Result<Doc, E> {
                Ok(Doc::Scalar(Value::Null))
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Doc, D::Error> {
                Doc::deserialize(d)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Doc, A::Error> {
                let mut out = Vec::with_capacity(a.size_hint().unwrap_or(0).min(4096));
                while let Some(v) = a.next_element::<Doc>()? {
                    out.push(v);
                }
                Ok(Doc::Array(out))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Doc, A::Error> {
                let mut out: Vec<(String, Doc)> = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, Doc>()? {
                    // JSON.parse keeps the last duplicate; so do we (in the first one's place).
                    match out.iter_mut().find(|(key, _)| *key == k) {
                        Some(slot) => slot.1 = v,
                        None => out.push((k, v)),
                    }
                }
                Ok(Doc::Object(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// An empty registry.
pub fn empty_registry() -> Doc {
    Doc::object(vec![("entities", Doc::Array(vec![]))])
}

/// The `entities` array of a registry document, created when missing.
pub fn entities_mut(doc: &mut Doc) -> Option<&mut Vec<Doc>> {
    if !matches!(doc, Doc::Object(_)) {
        return None;
    }
    if !matches!(doc.get("entities"), Some(Doc::Array(_))) {
        doc.set("entities", Doc::Array(vec![]));
    }
    doc.get_mut("entities").and_then(Doc::as_array_mut)
}

/// Position of the entity with `id` in the registry.
pub fn find_entity(doc: &Doc, id: &str) -> Option<usize> {
    doc.get("entities")?.as_array()?.iter().position(|e| e.get("id").and_then(Doc::as_str) == Some(id))
}

// ---------------------------------------------------------------- typed view

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub name: String,
    /// Relative to the card folder. Empty when the entry had no usable path.
    pub path: String,
    pub viewer: Option<String>,
}

/// A card as read from a registry (lenient: wrong types fall back to defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub description: String,
    pub icon: Option<String>,
    pub kind: String,
    pub category: String,
    pub created: String,
    pub updated: Option<String>,
    pub folder: String,
    pub steps: Vec<Step>,
    pub default_step: Option<i64>,
    pub status: String,
    pub pinned: bool,
    pub sample: bool,
}

impl Card {
    /// `None` for entries without an id or folder (they are kept in the file, not shown).
    pub fn from_doc(d: &Doc) -> Option<Card> {
        let id = d.string_field("id").filter(|s| !s.trim().is_empty())?;
        let folder = d.string_field("folder").filter(|s| !s.trim().is_empty())?;
        let steps = d
            .get("steps")
            .and_then(Doc::as_array)
            .map(|a| {
                a.iter()
                    .enumerate()
                    .map(|(i, s)| Step {
                        name: s.string_field("name").filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Step {}", i + 1)),
                        path: s.string_field("path").unwrap_or_default(),
                        viewer: s.string_field("viewer").filter(|v| !v.is_empty()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Card {
            title: d.string_field("title").filter(|t| !t.trim().is_empty()).unwrap_or_else(|| id.clone()),
            id,
            description: d.string_field("description").unwrap_or_default(),
            icon: d.string_field("icon").filter(|s| !s.is_empty()),
            kind: d.string_field("type").unwrap_or_else(|| "standalone".into()),
            category: d.string_field("category").unwrap_or_default(),
            created: d.string_field("created").unwrap_or_default(),
            updated: d.string_field("updated").filter(|s| !s.is_empty()),
            folder,
            steps,
            default_step: d.get("defaultStep").and_then(Doc::as_i64),
            status: d.string_field("status").filter(|s| !s.is_empty()).unwrap_or_else(|| "active".into()),
            pinned: d.get("pinned").and_then(Doc::as_bool).unwrap_or(false),
            sample: d.get("sample").and_then(Doc::as_bool).unwrap_or(false),
        })
    }

    /// Cards of a registry document, plus how many entries were unusable.
    pub fn all_from(doc: &Doc) -> (Vec<Card>, usize) {
        let Some(list) = doc.get("entities").and_then(Doc::as_array) else { return (vec![], 0) };
        let mut skipped = 0;
        let cards = list
            .iter()
            .filter_map(|e| {
                let c = Card::from_doc(e);
                if c.is_none() {
                    skipped += 1;
                }
                c
            })
            .collect();
        (cards, skipped)
    }

    /// Last touched (`updated`, else `created`) in ms; 0 when neither parses.
    pub fn touched_ms(&self) -> i64 {
        self.updated.as_deref().and_then(parse_date_ms).or_else(|| parse_date_ms(&self.created)).unwrap_or(0)
    }

    /// Explicitly archived, or untouched for [`ARCHIVE_DAYS`] (pinned cards and samples stay).
    pub fn archived(&self, cutoff_ms: i64) -> bool {
        self.status == "archived" || (!self.pinned && !self.sample && self.touched_ms() < cutoff_ms)
    }

    /// The tab to open without an explicit step: `defaultStep` when valid, else the last one.
    pub fn default_index(&self) -> Option<usize> {
        if self.steps.is_empty() {
            return None;
        }
        match self.default_step {
            Some(i) if i >= 0 && (i as usize) < self.steps.len() => Some(i as usize),
            _ => Some(self.steps.len() - 1),
        }
    }
}

/// Order for lists: pinned first, then freshest; active before done/archived on ties.
pub fn compare_cards(a: &Card, b: &Card) -> std::cmp::Ordering {
    let rank = |c: &Card| if c.status == "active" { 0 } else { 1 };
    b.pinned
        .cmp(&a.pinned)
        .then(b.touched_ms().cmp(&a.touched_ms()))
        .then(rank(a).cmp(&rank(b)))
        .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
}

// ---------------------------------------------------------------- dates

/// `YYYY-MM-DD` (local midnight), RFC 3339, or a naive `YYYY-MM-DDTHH:MM[:SS]` (local).
pub fn parse_date_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return Local.from_local_datetime(&n).earliest().map(|d| d.timestamp_millis());
        }
    }
    let d = NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?;
    Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest().map(|d| d.timestamp_millis())
}

/// Local midnight [`ARCHIVE_DAYS`] ago: a card last touched before it is archived
/// (day granularity, like Mr. Mak's `lastTouched < cutoff` on date strings).
pub fn archive_cutoff_ms() -> i64 {
    let day = Local::now().date_naive() - ChronoDuration::days(ARCHIVE_DAYS);
    day.and_hms_opt(0, 0, 0)
        .and_then(|n| Local.from_local_datetime(&n).earliest())
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

/// Timestamp written into our own registries (UTC, second precision).
pub fn now_stamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Mr. Mak's own format (local day), used when writing back into a repository's registry.
pub fn local_day() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------- names

/// Card id from a title: lowercase slug, at most 60 characters.
pub fn card_slug(title: &str) -> String {
    let mut s = crate::util::slug(title);
    if s.len() > 60 {
        s.truncate(60);
        s = s.trim_end_matches('-').to_string();
    }
    if s.is_empty() { "card".into() } else { s }
}

/// `base`, or `base-2`, `base-3`… — the first that `taken` does not reject.
pub fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..10_000).map(|n| format!("{base}-{n}")).find(|c| !taken(c)).unwrap_or_else(|| format!("{base}-{}", crate::util::random_token(4)))
}

/// A card folder name we accept from a registry: one plain path component.
pub fn valid_folder(folder: &str) -> bool {
    !folder.is_empty()
        && folder.len() <= 200
        && !folder.starts_with('.')
        && !folder.contains(['/', '\\', '\0'])
        && folder != "_shared"
}

/// What a step shows, from its explicit viewer or its path.
pub fn step_kind(viewer: Option<&str>, path: &str, is_dir: bool) -> &'static str {
    if let Some(v) = viewer {
        if let Some(k) = VIEWERS.iter().find(|x| **x == v && v != "auto") {
            return k;
        }
    }
    if is_dir {
        return "gallery";
    }
    kind_for_path(path)
}

/// Viewer kind by file extension (`file` = download only).
pub fn kind_for_path(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "html",
        "md" | "markdown" => "markdown",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" | "ico" => "image",
        "pdf" => "pdf",
        "mp4" | "webm" | "mov" | "m4v" | "ogv" => "video",
        "mp3" | "wav" | "ogg" | "oga" | "m4a" | "flac" | "aac" | "opus" => "audio",
        "glb" | "gltf" => "compare3d",
        "txt" | "log" | "json" | "jsonl" | "csv" | "tsv" | "yaml" | "yml" | "toml" | "xml" | "ini" | "cfg" | "conf" | "rs" | "ts"
        | "tsx" | "js" | "mjs" | "jsx" | "py" | "sh" | "css" | "sql" | "diff" | "patch" | "cs" | "go" | "java" | "c" | "h"
        | "cpp" | "hpp" => "text",
        _ => "file",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MRMAK: &str = r#"{
  "entities": [
    {
      "id": "creative-mcp",
      "title": "Creative MCP Connections",
      "description": "A list.",
      "type": "group",
      "category": "research",
      "created": "2026-09-15",
      "updated": "2026-09-16",
      "folder": "2026-09-15_creative-mcp",
      "steps": [
        { "name": "Tool shortlist", "path": "report.html", "custom": 1 },
        { "name": "Connect a tool", "path": "connect.md" }
      ],
      "status": "active",
      "pinned": false,
      "sample": true,
      "defaultStep": 0,
      "zzUnknown": { "b": 1, "a": [1.5, null] }
    },
    { "title": "no id: kept, not shown" }
  ],
  "version": 3
}
"#;

    #[test]
    fn documents_round_trip_in_order_with_unknown_keys() {
        let doc = Doc::parse(MRMAK.as_bytes()).unwrap();
        let out = String::from_utf8(doc.to_pretty()).unwrap();
        let reparsed: Value = serde_json::from_str(&out).unwrap();
        let original: Value = serde_json::from_str(MRMAK).unwrap();
        assert_eq!(reparsed, original);
        // Key order survives (serde_json alone would sort them).
        assert!(out.find("\"title\"").unwrap() < out.find("\"description\"").unwrap());
        assert!(out.find("\"zzUnknown\"").unwrap() > out.find("\"defaultStep\"").unwrap());
        assert!(out.find("\"b\"").unwrap() < out.find("\"a\"").unwrap());
        assert!(out.ends_with("}\n"));
    }

    #[test]
    fn patching_keeps_positions_and_places_new_keys() {
        let mut doc = Doc::parse(MRMAK.as_bytes()).unwrap();
        let i = find_entity(&doc, "creative-mcp").unwrap();
        let e = &mut entities_mut(&mut doc).unwrap()[i];
        e.set("status", Doc::str("done"));
        e.remove("updated");
        e.set_after("updated", Doc::str("2026-09-20"), &["created"]);
        let out = String::from_utf8(doc.to_pretty()).unwrap();
        let created = out.find("\"created\"").unwrap();
        let updated = out.find("\"updated\"").unwrap();
        let folder = out.find("\"folder\"").unwrap();
        assert!(created < updated && updated < folder, "{out}");
        assert!(out.contains("\"status\": \"done\""));
        // The unusable second entity is still there.
        assert!(out.contains("no id: kept, not shown"));
    }

    #[test]
    fn cards_are_read_leniently() {
        let doc = Doc::parse(MRMAK.as_bytes()).unwrap();
        let (cards, skipped) = Card::all_from(&doc);
        assert_eq!(skipped, 1);
        let c = &cards[0];
        assert_eq!(c.steps.len(), 2);
        assert_eq!(c.steps[1].path, "connect.md");
        assert!(c.sample && !c.pinned);
        assert_eq!(c.default_index(), Some(0));
        let odd = Doc::parse(br#"{"id":"x","folder":"f","title":5,"steps":[{"path":7}],"pinned":"yes","defaultStep":9}"#).unwrap();
        let c = Card::from_doc(&odd).unwrap();
        assert_eq!(c.title, "x");
        assert_eq!(c.steps[0].name, "Step 1");
        assert_eq!(c.steps[0].path, "");
        assert!(!c.pinned);
        assert_eq!(c.default_index(), Some(0), "an out-of-range defaultStep falls back to the last step");
        assert_eq!(c.status, "active");
    }

    #[test]
    fn freshness_sort_and_archive_rules() {
        let card = |id: &str, updated: &str, pinned: bool, status: &str| Card {
            id: id.into(),
            title: id.into(),
            description: String::new(),
            icon: None,
            kind: "standalone".into(),
            category: String::new(),
            created: "2020-01-01".into(),
            updated: Some(updated.into()),
            folder: id.into(),
            steps: vec![],
            default_step: None,
            status: status.into(),
            pinned,
            sample: false,
        };
        let today = local_day();
        let old = card("old", "2020-02-01", false, "active");
        let fresh = card("fresh", &now_stamp(), false, "done");
        let pinned_old = card("pinned", "2019-01-01", true, "active");
        let mut list = vec![old.clone(), fresh.clone(), pinned_old.clone()];
        list.sort_by(compare_cards);
        assert_eq!(list.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["pinned", "fresh", "old"]);

        let cutoff = archive_cutoff_ms();
        assert!(old.archived(cutoff));
        assert!(!fresh.archived(cutoff));
        assert!(!pinned_old.archived(cutoff), "pinned cards never auto-archive");
        assert!(card("x", &today, true, "archived").archived(cutoff), "explicit archived wins");
        let mut sample = old.clone();
        sample.sample = true;
        assert!(!sample.archived(cutoff));
        // Day granularity: touched exactly 7 days ago is still in.
        let edge = (Local::now().date_naive() - ChronoDuration::days(ARCHIVE_DAYS)).format("%Y-%m-%d").to_string();
        assert!(!card("edge", &edge, false, "active").archived(cutoff));
    }

    #[test]
    fn dates_parse_in_every_form_we_write_or_meet() {
        assert!(parse_date_ms("2026-09-15").is_some());
        assert_eq!(parse_date_ms("2026-09-15T10:00:00Z"), Some(1_789_466_400_000));
        assert!(parse_date_ms("2026-09-15T10:00").is_some());
        assert!(parse_date_ms("soon").is_none());
        assert!(parse_date_ms(&now_stamp()).unwrap() > parse_date_ms("2026-01-01").unwrap());
    }

    #[test]
    fn names_and_kinds() {
        assert_eq!(card_slug("  Q3 Load Report!  "), "q3-load-report");
        assert_eq!(card_slug("!!!"), "card");
        assert_eq!(unique("a", |c| c == "a" || c == "a-2"), "a-3");
        assert!(valid_folder("2026-09-15_x"));
        for bad in ["", ".hidden", "a/b", "..", "_shared", "a\\b"] {
            assert!(!valid_folder(bad), "{bad}");
        }
        assert_eq!(step_kind(None, "a/Report.HTML", false), "html");
        assert_eq!(step_kind(Some("compare3d"), "tests.json", false), "compare3d");
        assert_eq!(step_kind(Some("auto"), "x.json", false), "text");
        assert_eq!(step_kind(Some("bogus"), "x.png", false), "image");
        assert_eq!(step_kind(None, "shots", true), "gallery");
        assert_eq!(kind_for_path("model.glb"), "compare3d");
        assert_eq!(kind_for_path("archive.zip"), "file");
    }
}
