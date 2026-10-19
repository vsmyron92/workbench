//! What Claude Code actually gave a session: the skill listing in its transcript.
//!
//! Claude Code's built-in skills (`/init`, `/loop`, `/code-review`…) live inside the program and
//! are on no disk, and which of the others it loaded (plugins that are enabled, synced claude.ai
//! skills, the budget) is its decision. A session's transcript records the listing it received as
//! an `attachment` entry of type `skill_listing` with the names and the text. That entry is not
//! documented, so it is read leniently: a transcript without one, or in another shape, gives
//! nothing, and nothing else here depends on it.
//!
//! Only the start of the newest transcripts is read (the listing is among the first lines), with
//! hard caps on lines, bytes and line length, so a transcript of hundreds of megabytes costs
//! nothing and a hostile one cannot make this allocate.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

/// Transcripts looked at, newest first.
const FILES: usize = 4;
/// Names read from a project's transcript folder to find the newest ones: a bound on a folder that holds a great many.
const MAX_TRANSCRIPTS: usize = 20_000;
/// Lines read of each: the listing comes with the session's first messages.
const MAX_LINES: usize = 3000;
/// A line longer than this is skipped unread (a tool result, not the listing, which is ~30 KB).
const MAX_LINE: usize = 2 << 20;
/// Bytes read of one transcript in all.
const MAX_BYTES: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// Every name Claude Code listed, in its order (`stripe:stripe-pay`, `init`…).
    pub entries: Vec<Listed>,
    /// The size of the listing text: what the session carried.
    pub chars: usize,
    /// When the session received it (ms).
    pub at_ms: Option<i64>,
    /// The Claude Code version that wrote it.
    pub version: Option<String>,
}

/// The listing the newest session of `project_root` received, under the account folder `claude_dir`.
pub fn latest(claude_dir: &Path, project_root: &Path) -> Option<Listing> {
    let dir = crate::terminals::claude_project_dir(claude_dir, project_root);
    let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .take(MAX_TRANSCRIPTS)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (m.modified().ok().unwrap_or(SystemTime::UNIX_EPOCH), e.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(FILES).find_map(|(_, p)| from_file(&p))
}

fn from_file(path: &Path) -> Option<Listing> {
    let mut r = BufReader::with_capacity(64 << 10, std::fs::File::open(path).ok()?);
    let (mut buf, mut total) = (Vec::new(), 0usize);
    for _ in 0..MAX_LINES {
        buf.clear();
        let n = r.by_ref().take(MAX_LINE as u64).read_until(b'\n', &mut buf).ok()?;
        if n == 0 {
            return None;
        }
        total += n;
        if total > MAX_BYTES {
            return None;
        }
        if buf.last() != Some(&b'\n') && n == MAX_LINE {
            // Cut by the limit, not by the end of the file: skip what is left of the line (and count it).
            total += r.skip_until(b'\n').ok()?;
            if total > MAX_BYTES {
                return None;
            }
            continue;
        }
        // The cheap test first: nearly every line is something else.
        if !contains(&buf, b"\"skill_listing\"") {
            continue;
        }
        if let Some(l) = std::str::from_utf8(&buf).ok().and_then(|s| serde_json::from_str::<Value>(s).ok()).and_then(|v| listing_of(&v)) {
            return Some(l);
        }
    }
    None
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// One transcript line, when it is a `skill_listing` attachment.
pub fn listing_of(line: &Value) -> Option<Listing> {
    let a = line.get("attachment")?;
    if line.get("type")?.as_str()? != "attachment" || a.get("type")?.as_str()? != "skill_listing" {
        return None;
    }
    let names: Vec<String> = a.get("names")?.as_array()?.iter().filter_map(|n| n.as_str().map(str::to_string)).collect();
    let content = a.get("content")?.as_str()?;
    if names.is_empty() {
        return None;
    }
    Some(Listing {
        entries: entries(content, &names),
        chars: content.chars().count(),
        at_ms: line.get("timestamp").and_then(Value::as_str).and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp_millis()),
        version: line.get("version").and_then(Value::as_str).map(str::to_string),
    })
}

/// The text is `- name: description` per skill, where a description may run over several lines and
/// a name may hold a colon (`plugin:skill`): so the names, in order, say where each entry starts.
fn entries(content: &str, names: &[String]) -> Vec<Listed> {
    let mut starts: Vec<Option<(usize, usize)>> = vec![];
    let mut from = 0usize;
    for n in names {
        let found = entry_start(content, n, from);
        if let Some((_, after)) = found {
            from = after;
        }
        starts.push(found);
    }
    let mut out = vec![];
    for (i, n) in names.iter().enumerate() {
        let description = match starts[i] {
            Some((_, after)) => {
                // Up to where the next entry that was found starts.
                let end = starts[i + 1..].iter().flatten().next().map_or(content.len(), |(s, _)| *s);
                content[after..end.max(after)].trim().to_string()
            }
            None => String::new(),
        };
        out.push(Listed { name: n.clone(), description });
    }
    out
}

/// `(start of the line, start of the description)` of `- name: ` or a bare `- name`, at or after `from`.
fn entry_start(content: &str, name: &str, from: usize) -> Option<(usize, usize)> {
    let marker = format!("- {name}");
    let mut at = from;
    while let Some(i) = content[at..].find(&marker) {
        let start = at + i;
        let after = start + marker.len();
        let at_line_start = start == 0 || content.as_bytes()[start - 1] == b'\n';
        if at_line_start {
            let rest = &content[after..];
            if rest.starts_with(": ") {
                return Some((start, after + 2));
            }
            if rest.is_empty() || rest.starts_with('\n') {
                return Some((start, after));
            }
        }
        at = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn line(names: &[&str], content: &str) -> Value {
        json!({"type": "attachment", "timestamp": "2026-10-16T05:00:00.000Z", "version": "2.1.301",
               "attachment": {"type": "skill_listing", "content": content, "skillCount": names.len(), "isInitial": true, "names": names}})
    }

    #[test]
    fn the_listing_splits_by_name_even_with_colons_and_multi_line_descriptions() {
        let content = "- demo: Records a demo.\n- stripe:pay: Takes payments: cards, banks.\n- claude-api: First line.\nTRIGGER: second line.\n- init: Starts a CLAUDE.md\n- bare\n- last: end";
        let names = ["demo", "stripe:pay", "claude-api", "init", "bare", "last"];
        let l = listing_of(&line(&names, content)).unwrap();
        let got: Vec<(&str, &str)> = l.entries.iter().map(|e| (e.name.as_str(), e.description.as_str())).collect();
        assert_eq!(
            got,
            [
                ("demo", "Records a demo."),
                ("stripe:pay", "Takes payments: cards, banks."),
                ("claude-api", "First line.\nTRIGGER: second line."),
                ("init", "Starts a CLAUDE.md"),
                ("bare", ""),
                ("last", "end"),
            ]
        );
        assert_eq!((l.chars, l.version.as_deref()), (content.chars().count(), Some("2.1.301")));
        assert_eq!(l.at_ms, Some(1_792_126_800_000));
    }

    #[test]
    fn a_name_that_is_a_prefix_of_another_is_not_mixed_up_with_it() {
        let content = "- a-b: the long one\n- a: the short one";
        let l = listing_of(&line(&["a-b", "a"], content)).unwrap();
        assert_eq!(l.entries[0].description, "the long one");
        assert_eq!(l.entries[1].description, "the short one");
    }

    #[test]
    fn other_lines_and_other_shapes_give_nothing() {
        assert!(listing_of(&json!({"type": "user", "message": "skill_listing"})).is_none());
        assert!(listing_of(&json!({"type": "attachment", "attachment": {"type": "date"}})).is_none());
        assert!(listing_of(&json!({"type": "attachment", "attachment": {"type": "skill_listing", "content": "x"}})).is_none());
        assert!(listing_of(&json!({"type": "attachment", "attachment": {"type": "skill_listing", "content": "x", "names": []}})).is_none());
    }

    #[test]
    fn the_newest_transcript_with_a_listing_wins_and_junk_lines_are_skipped() {
        let t = tempfile::tempdir().unwrap();
        let (claude, root) = (t.path().join("claude"), t.path().join("proj"));
        std::fs::create_dir_all(&root).unwrap();
        let dir = crate::terminals::claude_project_dir(&claude, &root);
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, lines: Vec<String>, age_secs: u64| {
            let p = dir.join(name);
            let mut f = std::fs::File::create(&p).unwrap();
            for l in lines {
                writeln!(f, "{l}").unwrap();
            }
            f.set_modified(SystemTime::now() - std::time::Duration::from_secs(age_secs)).unwrap();
        };
        let junk = "not json at all \"skill_listing\"".to_string();
        write("old.jsonl", vec![line(&["old-skill"], "- old-skill: d").to_string()], 600);
        write("new.jsonl", vec![r#"{"type":"user"}"#.into(), junk, line(&["new-skill"], "- new-skill: d").to_string()], 10);
        // Not a transcript.
        write("notes.txt", vec![line(&["x"], "- x: d").to_string()], 1);
        let l = latest(&claude, &root).unwrap();
        assert_eq!(l.entries[0].name, "new-skill");
        // The newest one has no listing: the next one is used.
        write("newest.jsonl", vec![r#"{"type":"user"}"#.into()], 1);
        assert_eq!(latest(&claude, &root).unwrap().entries[0].name, "new-skill");
        // A project that never ran a session.
        assert!(latest(&claude, &t.path().join("elsewhere")).is_none());
    }

    #[test]
    fn a_huge_line_is_skipped_unread_and_the_listing_after_it_is_still_found() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "{}", "x".repeat(MAX_LINE + 10)).unwrap();
        writeln!(f, "{}", line(&["after"], "- after: d")).unwrap();
        drop(f);
        assert_eq!(from_file(&p).unwrap().entries[0].name, "after");
    }
}
