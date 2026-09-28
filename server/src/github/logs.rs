//! GitHub Actions job logs.
//!
//! GitHub serves a finished job's log as a plain text file (behind a redirect to
//! a signed storage URL). Every line starts with an RFC 3339 timestamp and a
//! space (`2026-09-22T19:00:31.1234567Z ##[group]Run actions/checkout@v5`), the
//! file starts with a byte-order mark, and the runner marks foldable groups with
//! `##[group]Title` … `##[endgroup]` and messages with `##[error]`,
//! `##[warning]`, `##[notice]`, `##[command]`, `##[debug]`.
//!
//! We strip the timestamps (keeping ANSI colours and the `##[…]` markers, which
//! the browser folds and colours), and use them once to find where each step of
//! the job starts, so the viewer can show steps as sections like GitHub does.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::model::Step;

static TIMESTAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z) ?").expect("valid regex"));

/// A downloaded, prefix-stripped job log.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobLogData {
    /// Log text without timestamps (ANSI and `##[…]` markers kept).
    pub text: String,
    /// Where steps start (0-based line numbers into `text`).
    pub steps: Vec<StepMark>,
    /// The start of the log was left out (it is larger than we keep).
    pub truncated: bool,
    /// Raw size in bytes.
    pub size: u64,
}

/// A cached log shared between requests (serializes as the log itself).
#[derive(Debug, Clone)]
pub struct SharedLog(pub std::sync::Arc<JobLogData>);

impl Serialize for SharedLog {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepMark {
    pub number: u64,
    pub line: usize,
}

/// Remove the BOM and the per-line timestamps. Returns the text and each line's
/// time in epoch seconds (when it had one).
pub fn strip_timestamps(raw: &str) -> (String, Vec<Option<i64>>) {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let body = raw.strip_suffix('\n').unwrap_or(raw);
    let mut out = String::with_capacity(body.len());
    let mut times = Vec::new();
    for (i, line) in body.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let line = line.strip_prefix('\u{feff}').unwrap_or(line);
        if i > 0 {
            out.push('\n');
        }
        match TIMESTAMP.captures(line) {
            Some(c) => {
                let end = c.get(0).map(|m| m.end()).unwrap_or(0);
                times.push(chrono::DateTime::parse_from_rfc3339(&c[1]).ok().map(|t| t.timestamp()));
                out.push_str(&line[end..]);
            }
            None => {
                times.push(None);
                out.push_str(line);
            }
        }
    }
    (out, times)
}

/// Lines that open a step's output within its first second.
fn opens_step(line: &str) -> bool {
    line.starts_with("##[group]Run ") || line.starts_with("Post job cleanup.") || line.starts_with("Cleaning up orphan processes")
}

/// First line of each step that ran, from the steps' start times (whole seconds)
/// and the lines' timestamps. Within a step's first second, a line that opens a
/// step (`##[group]Run …`) is preferred, since the previous step's last lines
/// often share that second.
pub fn step_marks(lines: &[&str], times: &[Option<i64>], steps: &[Step]) -> Vec<StepMark> {
    let mut steps: Vec<&Step> = steps
        .iter()
        .filter(|s| s.conclusion.as_deref() != Some("skipped") && s.started_at.is_some())
        .collect();
    steps.sort_by_key(|s| s.number);
    let mut out: Vec<StepMark> = vec![];
    let mut from = 0usize;
    for (k, s) in steps.iter().enumerate() {
        let Some(start) = s.started_at.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp())
        else {
            continue;
        };
        let first = if k == 0 {
            // The first step ("Set up job") begins the log.
            Some(0)
        } else {
            (from..times.len()).find(|&i| times[i].is_some_and(|t| t >= start))
        };
        let Some(mut i) = first else { break };
        if k > 0 {
            let mut j = i;
            while j < times.len() && times[j].is_none_or(|t| t <= start + 1) {
                if lines.get(j).is_some_and(|l| opens_step(l)) {
                    i = j;
                    break;
                }
                j += 1;
            }
        }
        if i < from && k > 0 {
            continue;
        }
        out.push(StepMark { number: s.number, line: i });
        from = i + 1;
    }
    out
}

/// Parse a downloaded log: strip timestamps and locate the steps.
pub fn parse(raw: &str, steps: &[Step], truncated: bool, size: u64) -> JobLogData {
    let (text, times) = strip_timestamps(raw);
    let lines: Vec<&str> = text.split('\n').collect();
    let marks = if truncated { vec![] } else { step_marks(&lines, &times, steps) };
    JobLogData { steps: marks, truncated, size, text }
}

/// Plain text for agents and downloads: ANSI removed, groups flattened, message
/// markers turned into words.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut first = true;
    for line in text.split('\n') {
        let line = crate::util::ansi::strip(line);
        let mapped = if let Some(rest) = line.strip_prefix("##[group]") {
            rest.to_string()
        } else if line.starts_with("##[endgroup]") {
            continue;
        } else if let Some(rest) = line.strip_prefix("##[error]") {
            format!("Error: {rest}")
        } else if let Some(rest) = line.strip_prefix("##[warning]") {
            format!("Warning: {rest}")
        } else if let Some(rest) = line.strip_prefix("##[notice]") {
            format!("Notice: {rest}")
        } else if let Some(rest) = line.strip_prefix("##[debug]") {
            format!("Debug: {rest}")
        } else if let Some(rest) = line.strip_prefix("##[command]") {
            format!("$ {rest}")
        } else if let Some(rest) = line.strip_prefix("##[section]") {
            rest.to_string()
        } else {
            line
        };
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(mapped.trim_end());
    }
    out
}

/// The last `n` lines of `text` and the total line count.
pub fn tail_lines(text: &str, n: usize) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    (lines[total.saturating_sub(n)..].join("\n"), total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = "\u{feff}2026-09-22T19:00:30.9876543Z Current runner version: '2.328.0'\r\n\
2026-09-22T19:00:30.9900000Z ##[group]Operating System\n\
2026-09-22T19:00:30.9900001Z Ubuntu\n\
2026-09-22T19:00:30.9900002Z ##[endgroup]\n\
2026-09-22T19:00:31.1000000Z Complete job name: build\n\
2026-09-22T19:00:31.2000000Z ##[group]Run actions/checkout@v5\n\
2026-09-22T19:00:31.2000001Z with:\n\
2026-09-22T19:00:31.2000002Z ##[endgroup]\n\
2026-09-22T19:00:32.5000000Z Syncing repository\n\
2026-09-22T19:00:33.0000000Z ##[group]Run cargo test\n\
2026-09-22T19:00:33.0000001Z ##[command]cargo test\n\
2026-09-22T19:00:37.0000000Z \x1b[31merror[E0308]\x1b[0m: mismatched types\n\
2026-09-22T19:00:37.1000000Z ##[error]Process completed with exit code 101.\n\
2026-09-22T19:00:38.0000000Z Post job cleanup.\n\
2026-09-22T19:00:38.5000000Z Cleaning up orphan processes\n";

    fn step(number: u64, name: &str, start: &str, conclusion: &str) -> Step {
        Step {
            number,
            name: name.into(),
            status: "completed".into(),
            conclusion: Some(conclusion.into()),
            started_at: Some(start.into()),
            completed_at: Some(start.into()),
            state: String::new(),
        }
    }

    #[test]
    fn timestamps_and_bom_are_stripped() {
        let (text, times) = strip_timestamps(RAW);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines[0], "Current runner version: '2.328.0'");
        assert_eq!(lines[1], "##[group]Operating System");
        assert_eq!(lines[11], "\x1b[31merror[E0308]\x1b[0m: mismatched types", "ANSI kept");
        assert_eq!(lines.len(), 15);
        assert_eq!(times.len(), 15);
        assert!(times.iter().all(Option::is_some));
        // Lines without a timestamp pass through.
        let (t, times) = strip_timestamps("plain\nline");
        assert_eq!((t.as_str(), times), ("plain\nline", vec![None, None]));
    }

    #[test]
    fn steps_start_where_their_output_starts() {
        let steps = vec![
            step(1, "Set up job", "2026-09-22T19:00:30Z", "success"),
            // Starts in the same second the previous step's last line was written.
            step(2, "Run actions/checkout@v5", "2026-09-22T19:00:31Z", "success"),
            step(3, "Run cargo test", "2026-09-22T19:00:33Z", "failure"),
            step(4, "Run deploy", "2026-09-22T19:00:37Z", "skipped"),
            step(6, "Post Run actions/checkout@v5", "2026-09-22T19:00:38Z", "success"),
            step(7, "Complete job", "2026-09-22T19:00:38Z", "success"),
        ];
        let data = parse(RAW, &steps, false, RAW.len() as u64);
        let marks: Vec<(u64, usize)> = data.steps.iter().map(|m| (m.number, m.line)).collect();
        assert_eq!(marks, [(1, 0), (2, 5), (3, 9), (6, 13), (7, 14)]);
        // A truncated log has lost its start: no step marks.
        assert!(parse(RAW, &steps, true, 0).steps.is_empty());
    }

    #[test]
    fn plain_text_for_agents() {
        let (text, _) = strip_timestamps(RAW);
        let p = plain(&text);
        assert!(p.contains("Operating System\nUbuntu\nComplete job name"));
        assert!(p.contains("$ cargo test"));
        assert!(p.contains("error[E0308]: mismatched types"));
        assert!(p.contains("Error: Process completed with exit code 101."));
        assert!(!p.contains("##["));
        assert!(!p.contains('\x1b'));
        let (tail, total) = tail_lines(&p, 2);
        assert_eq!(total, 13);
        assert_eq!(tail, "Post job cleanup.\nCleaning up orphan processes");
    }
}
