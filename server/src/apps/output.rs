//! Run output processing: line buffering over raw PTY bytes (ANSI stripped),
//! the ready-line matcher and the test-result parser.

use regex::Regex;
use serde::Serialize;

use crate::util::ansi;

/// Longest line kept; a longer run without a newline is emitted as a line.
const MAX_LINE: usize = 64 * 1024;
/// Result items kept per run.
pub const MAX_ITEMS: usize = 500;

/// Splits a byte stream into ANSI-stripped lines. Bounded: never holds more than
/// `MAX_LINE` bytes of an unterminated line.
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    /// Feed a chunk; returns the complete lines it finished.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut out = vec![];
        for &b in chunk {
            if b == b'\n' {
                out.push(Self::clean(&self.buf));
                self.buf.clear();
            } else {
                self.buf.push(b);
                if self.buf.len() >= MAX_LINE {
                    out.push(Self::clean(&self.buf));
                    self.buf.clear();
                }
            }
        }
        out
    }

    /// The unterminated tail (prompts, progress lines), stripped.
    pub fn partial(&self) -> Option<String> {
        (!self.buf.is_empty()).then(|| Self::clean(&self.buf))
    }

    /// Emit the unterminated tail as a final line.
    pub fn flush(&mut self) -> Option<String> {
        let p = self.partial();
        self.buf.clear();
        p
    }

    fn clean(bytes: &[u8]) -> String {
        ansi::strip(&String::from_utf8_lossy(bytes))
    }
}

/// Matches the ready line; the first capture group, when it is a URL, becomes the run's URL.
pub struct ReadyMatcher {
    re: Regex,
}

impl ReadyMatcher {
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self { re: Regex::new(pattern)? })
    }

    /// `None` = no match; `Some(url)` = matched (with the URL it printed, if any).
    pub fn check(&self, line: &str) -> Option<Option<String>> {
        let c = self.re.captures(line)?;
        let url = c
            .get(1)
            .map(|m| m.as_str().trim_end_matches(['.', ',', ')']).to_string())
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
        Some(url)
    }
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub passed: u32,
    pub failed: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ResultItem>,
    /// More items matched than `MAX_ITEMS`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResultItem {
    pub name: String,
    /// passed | failed | skipped
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Parses `result_pattern` matches:
/// * named groups `passed` / `failed` (numbers) → summary counts, summed over matches
///   (one line per test binary / project);
/// * otherwise a `status` group (+ optional `name`, `detail`) → one item per match;
/// * a pattern with neither marks each matching line as a failure (e.g. `error CS\d+`).
pub struct ResultParser {
    re: Regex,
    counts: bool,
    status: bool,
}

impl ResultParser {
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        let re = Regex::new(pattern)?;
        let names: Vec<&str> = re.capture_names().flatten().collect();
        let counts = names.contains(&"passed") || names.contains(&"failed");
        let status = names.contains(&"status");
        Ok(Self { re, counts, status })
    }

    /// Feed one line; returns whether `acc` changed.
    pub fn feed(&self, line: &str, acc: &mut TestResult) -> bool {
        let Some(c) = self.re.captures(line) else { return false };
        if self.counts {
            let n = |g: &str| c.name(g).and_then(|m| m.as_str().parse::<u32>().ok()).unwrap_or(0);
            acc.passed = acc.passed.saturating_add(n("passed"));
            acc.failed = acc.failed.saturating_add(n("failed"));
            return true;
        }
        let (name, status, detail) = if self.status {
            let status = normalize_status(c.name("status").map(|m| m.as_str()).unwrap_or(""));
            let name = c.name("name").map(|m| m.as_str().trim().to_string()).unwrap_or_else(|| line.trim().to_string());
            let detail = c.name("detail").map(|m| m.as_str().trim().to_string()).filter(|d| !d.is_empty());
            (name, status, detail)
        } else {
            (c.get(0).map(|m| m.as_str()).unwrap_or(line).trim().to_string(), "failed", Some(line.trim().to_string()))
        };
        match status {
            "passed" => acc.passed = acc.passed.saturating_add(1),
            "failed" => acc.failed = acc.failed.saturating_add(1),
            _ => {}
        }
        if acc.items.len() < MAX_ITEMS {
            let clip = |s: String| crate::apps::detect::text::ellipsize(&s, 300);
            acc.items.push(ResultItem { name: clip(name), status, detail: detail.map(clip) });
        } else {
            acc.truncated = true;
        }
        true
    }
}

fn normalize_status(s: &str) -> &'static str {
    match s.to_ascii_lowercase().as_str() {
        "pass" | "passed" | "ok" | "success" | "succeeded" | "green" => "passed",
        "fail" | "failed" | "failure" | "error" | "errored" | "ko" | "red" | "crashed" => "failed",
        _ => "skipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_across_chunks_and_strips_ansi() {
        let mut lb = LineBuffer::default();
        assert!(lb.push(b"  \x1b[32m\xe2\x9e\x9c\x1b[39m  \x1b[1mLocal\x1b[22m:   http://local").is_empty());
        assert_eq!(lb.partial().as_deref(), Some("  ➜  Local:   http://local"));
        let lines = lb.push(b"host:\x1b[1m5173\x1b[22m/\r\nnext\n");
        assert_eq!(lines, vec!["  ➜  Local:   http://localhost:5173/", "next"]);
        assert!(lb.flush().is_none());
    }

    #[test]
    fn utf8_split_across_chunks_survives() {
        let mut lb = LineBuffer::default();
        let s = "✓ done\n".as_bytes();
        let mut out = lb.push(&s[..1]);
        out.extend(lb.push(&s[1..]));
        assert_eq!(out, vec!["✓ done"]);
    }

    #[test]
    fn bounds_unterminated_lines() {
        let mut lb = LineBuffer::default();
        let big = vec![b'x'; MAX_LINE + 10];
        let lines = lb.push(&big);
        assert_eq!(lines.len(), 1);
        assert_eq!(lb.partial().map(|p| p.len()), Some(10));
    }

    #[test]
    fn ready_matcher_extracts_the_url() {
        let m = ReadyMatcher::new(r"Local:\s+(https?://\S+)").unwrap();
        assert_eq!(m.check("  ➜  Local:   http://localhost:5173/"), Some(Some("http://localhost:5173/".into())));
        assert_eq!(m.check("  ➜  Network: use --host to expose"), None);
        let plain = ReadyMatcher::new(r"shop\-api listening on").unwrap();
        assert_eq!(plain.check("shop-api listening on http://0.0.0.0:8080"), Some(None));
        let py = ReadyMatcher::new(r"Serving HTTP on \S+ port (\d+)").unwrap();
        assert_eq!(py.check("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ..."), Some(None));
    }

    #[test]
    fn counts_are_summed_over_test_binaries() {
        let p = ResultParser::new(crate::apps::detect::CARGO_TEST_RESULT).unwrap();
        let mut r = TestResult::default();
        assert!(p.feed("test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured", &mut r));
        assert!(p.feed("test result: FAILED. 3 passed; 2 failed; 0 ignored", &mut r));
        assert!(!p.feed("running 5 tests", &mut r));
        assert_eq!((r.passed, r.failed), (15, 2));
        assert!(r.items.is_empty());
    }

    #[test]
    fn dotnet_and_vitest_summaries() {
        let p = ResultParser::new(crate::apps::detect::DOTNET_TEST_RESULT).unwrap();
        let mut r = TestResult::default();
        p.feed("Passed!  - Failed:     0, Passed:    41, Skipped:     0, Total:    41, Duration: 2 s", &mut r);
        p.feed("Failed!  - Failed:     2, Passed:     7, Skipped:     0, Total:     9", &mut r);
        assert_eq!((r.passed, r.failed), (48, 2));

        let v = ResultParser::new(crate::apps::detect::VITEST_RESULT).unwrap();
        let mut r = TestResult::default();
        v.feed("      Tests  2 failed | 40 passed (42)", &mut r);
        assert_eq!((r.passed, r.failed), (40, 2));
        let mut r = TestResult::default();
        v.feed("      Tests  18 passed (18)", &mut r);
        assert_eq!((r.passed, r.failed), (18, 0));
    }

    #[test]
    fn status_items_from_a_smoke_test() {
        let p = ResultParser::new(r"^SMOKETEST_RESULT: (?P<name>.+?) = (?P<status>PASS|FAIL)\b(?:\s*::\s*(?P<detail>.*))?$").unwrap();
        let mut r = TestResult::default();
        p.feed("SMOKETEST_RESULT: Combat = PASS :: warrior hit 3 times", &mut r);
        p.feed("SMOKETEST_RESULT: Economy = FAIL :: rice stayed at 0", &mut r);
        assert_eq!((r.passed, r.failed), (1, 1));
        assert_eq!(r.items[1], ResultItem { name: "Economy".into(), status: "failed", detail: Some("rice stayed at 0".into()) });
    }

    #[test]
    fn a_pattern_without_groups_marks_failures_and_caps_items() {
        let p = ResultParser::new(r"(?i)error CS\d+").unwrap();
        let mut r = TestResult::default();
        for i in 0..(MAX_ITEMS + 5) {
            p.feed(&format!("Assets/X.cs(1,{i}): error CS0103: missing"), &mut r);
        }
        assert_eq!(r.failed as usize, MAX_ITEMS + 5);
        assert_eq!(r.items.len(), MAX_ITEMS);
        assert!(r.truncated);
        assert_eq!(r.items[0].name, "error CS0103");
    }
}
