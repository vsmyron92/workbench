//! CI job logs ("traces").
//!
//! Runner 17+ writes every physical line as
//! `2026-09-26T10:05:00.164362Z 01O <text>`: a timestamp, a two-digit hex stream
//! id, `O`/`E` (stdout/stderr) and then either a space or `+`. `+` marks a
//! continuation: the line belongs to the previous one (the newline between them
//! is an artifact of timestamping). Stripping prefixes and re-joining
//! continuations restores the classic log, where section markers look like
//! `section_start:1790419369:prepare_executor\r\x1b[0K<header text>`.
//!
//! Incremental reads work on byte offsets into the raw trace. While a job runs,
//! only complete lines are consumed, so a line's prefix is always parsed whole and
//! the next chunk starts at a line boundary. The newline that ends a consumed
//! line is emitted lazily, before the next line, so a chunk that begins with a
//! continuation can still join its predecessor. See `strip_prefixes`.

use std::sync::LazyLock;

use regex::Regex;

/// Longest partial line we hold back while waiting for its newline.
const MAX_PARTIAL_LINE: usize = 256 * 1024;

static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z [0-9a-fA-F]{2}[OE](\+| )?").expect("valid regex")
});

static SECTION_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"section_(?:start|end):\d+:[A-Za-z0-9_.\-]+(?:\[[^\]\r\n]*\])?\r?(?:\x1b\[0K)?").expect("valid regex")
});

/// Strip runner prefixes from a chunk of complete lines and re-join continuation
/// lines. `at_start`: the chunk starts the caller's text (no separator before its
/// first line); otherwise every non-continuation line is preceded by `\n`.
/// A trailing `\n` of the chunk is not emitted (the next chunk's first line
/// supplies it). Lines without a prefix (older runners) pass through unchanged.
pub fn strip_prefixes(chunk: &str, at_start: bool) -> String {
    if chunk.is_empty() {
        return String::new();
    }
    let body = chunk.strip_suffix('\n').unwrap_or(chunk);
    let mut out = String::with_capacity(body.len());
    for (i, line) in body.split('\n').enumerate() {
        let (content, continuation) = match PREFIX.captures(line) {
            Some(c) => {
                let end = c.get(0).map(|m| m.end()).unwrap_or(0);
                (&line[end..], c.get(1).is_some_and(|m| m.as_str() == "+"))
            }
            None => (line, false),
        };
        if !(i == 0 && at_start) && !continuation {
            out.push('\n');
        }
        out.push_str(content);
    }
    out
}

/// How many bytes of `bytes` to consume now. A finished log is consumed whole;
/// a growing one only through its last newline (unless a single line has grown
/// absurdly long, which is then flushed as is).
pub fn consumable_len(bytes: &[u8], complete: bool) -> usize {
    if complete {
        return bytes.len();
    }
    match bytes.iter().rposition(|&b| b == b'\n') {
        Some(i) => i + 1,
        None if bytes.len() > MAX_PARTIAL_LINE => bytes.len(),
        None => 0,
    }
}

/// Bytes to skip so a buffer that lost its front starts on a line boundary.
pub fn align_to_line(bytes: &[u8]) -> usize {
    bytes.iter().position(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0)
}

/// `(start, total)` from `Content-Range: bytes 43700-43780/43781` (or `bytes */43781`).
pub fn parse_content_range(v: &str) -> (Option<u64>, Option<u64>) {
    let v = v.trim();
    let Some(rest) = v.strip_prefix("bytes").map(str::trim) else { return (None, None) };
    let (range, total) = rest.split_once('/').unwrap_or((rest, ""));
    let start = range.split_once('-').and_then(|(s, _)| s.trim().parse().ok());
    (start, total.trim().parse().ok())
}

/// Plain text for agents and downloads: section markers removed (lines that
/// held only a marker dropped), terminal `\r` rewrites resolved to their last
/// visible state, ANSI escapes stripped.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut first = true;
    for line in text.split('\n') {
        let had_marker = SECTION_MARKER.is_match(line);
        let line = if had_marker { SECTION_MARKER.replace_all(line, "") } else { line.into() };
        let visible = line
            .rsplit('\r')
            .map(crate::util::ansi::strip)
            .find(|s| !s.trim().is_empty())
            .unwrap_or_default();
        if had_marker && visible.trim().is_empty() {
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(visible.trim_end());
    }
    out
}

/// The last `n` lines of `text` and the total line count.
pub fn tail_lines(text: &str, n: usize) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    (lines[total.saturating_sub(n)..].join("\n"), total)
}

/// One section of a log (name, header text, line range), for agents that want
/// to know where the script output starts. Returned in order of appearance.
#[derive(Debug, Clone, PartialEq)]
#[cfg(test)]
pub struct Section {
    pub name: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
}

/// Sections with their start/end unix times, parsed from a prefix-stripped log.
#[cfg(test)]
pub fn sections(text: &str) -> Vec<Section> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"section_(start|end):(\d+):([A-Za-z0-9_.\-]+)").expect("valid regex")
    });
    let mut out: Vec<Section> = vec![];
    for c in RE.captures_iter(text) {
        let ts = c[2].parse::<i64>().ok();
        let name = c[3].to_string();
        if &c[1] == "start" {
            out.push(Section { name, start: ts, end: None });
        } else if let Some(s) = out.iter_mut().rev().find(|s| s.name == name && s.end.is_none()) {
            s.end = ts;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = "2026-09-26T10:42:49.140026Z 00O \x1b[0KRunning with gitlab-runner 19.5.0\x1b[0;m\n\
2026-09-26T10:42:49.140140Z 00O section_start:1790419369:prepare_executor\r\x1b[0K\n\
2026-09-26T10:42:49.140145Z 00O+\x1b[0K\x1b[36;1mPreparing the \"docker+machine\" executor\x1b[0;m\x1b[0;m\n\
2026-09-26T10:42:49.313871Z 00O \x1b[0KUsing Docker executor with image node:22-alpine ...\x1b[0;m\n\
2026-09-26T10:42:54.803738Z 00O section_end:1790419374:prepare_executor\r\x1b[0K\n\
2026-09-26T10:42:54.803963Z 00O+section_start:1790419374:step_script\r\x1b[0K\n\
2026-09-26T10:42:54.803972Z 01E npm warn deprecated x\n\
2026-09-26T10:43:32.034628Z 00O \x1b[32;1mJob succeeded\x1b[0;m\n";

    const CLASSIC: &str = "\x1b[0KRunning with gitlab-runner 19.5.0\x1b[0;m\n\
section_start:1790419369:prepare_executor\r\x1b[0K\x1b[0K\x1b[36;1mPreparing the \"docker+machine\" executor\x1b[0;m\x1b[0;m\n\
\x1b[0KUsing Docker executor with image node:22-alpine ...\x1b[0;m\n\
section_end:1790419374:prepare_executor\r\x1b[0Ksection_start:1790419374:step_script\r\x1b[0K\n\
npm warn deprecated x\n\
\x1b[32;1mJob succeeded\x1b[0;m";

    #[test]
    fn strips_prefixes_and_joins_continuations() {
        assert_eq!(strip_prefixes(RAW, true), CLASSIC);
    }

    #[test]
    fn incremental_chunks_equal_the_whole() {
        // Split the raw log at every line boundary into two chunks; the stripped
        // halves concatenated must equal the stripped whole.
        let whole = strip_prefixes(RAW, true);
        let mut cuts: Vec<usize> = RAW.match_indices('\n').map(|(i, _)| i + 1).collect();
        cuts.pop();
        for cut in cuts {
            let (a, b) = RAW.split_at(cut);
            let joined = strip_prefixes(a, true) + &strip_prefixes(b, false);
            assert_eq!(joined, whole, "cut at {cut}");
        }
    }

    #[test]
    fn lines_without_prefix_pass_through() {
        assert_eq!(strip_prefixes("a\nb\n", true), "a\nb");
        assert_eq!(strip_prefixes("c\n", false), "\nc");
        assert_eq!(strip_prefixes("", false), "");
        // Content that merely starts with '+' after a space-flag is kept.
        assert_eq!(strip_prefixes("2026-09-26T10:42:49Z 00O +foo\n", true), "+foo");
    }

    #[test]
    fn consumes_only_complete_lines_while_running() {
        assert_eq!(consumable_len(b"a\nb\npartial", false), 4);
        assert_eq!(consumable_len(b"a\nb\npartial", true), 11);
        assert_eq!(consumable_len(b"no newline yet", false), 0);
        assert_eq!(consumable_len(b"", false), 0);
        let long = vec![b'x'; MAX_PARTIAL_LINE + 1];
        assert_eq!(consumable_len(&long, false), long.len());
        assert_eq!(align_to_line(b"tail of line\nnext\n"), 13);
        assert_eq!(align_to_line(b"no newline"), 0);
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(parse_content_range("bytes 43700-43780/43781"), (Some(43700), Some(43781)));
        assert_eq!(parse_content_range("bytes */43781"), (None, Some(43781)));
        assert_eq!(parse_content_range("bytes 0-9/*"), (Some(0), None));
        assert_eq!(parse_content_range("garbage"), (None, None));
    }

    #[test]
    fn plain_text_for_agents() {
        let p = plain(CLASSIC);
        assert_eq!(
            p,
            "Running with gitlab-runner 19.5.0\nPreparing the \"docker+machine\" executor\nUsing Docker executor with image node:22-alpine ...\nnpm warn deprecated x\nJob succeeded"
        );
        // Progress bars keep their final state.
        assert_eq!(plain("10%\r50%\r100%\ndone"), "100%\ndone");
    }

    #[test]
    fn tails_and_sections() {
        let (t, n) = tail_lines("a\nb\nc\nd", 2);
        assert_eq!((t.as_str(), n), ("c\nd", 4));
        let (t, _) = tail_lines("a", 10);
        assert_eq!(t, "a");
        let s = sections(CLASSIC);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], Section { name: "prepare_executor".into(), start: Some(1790419369), end: Some(1790419374) });
        assert_eq!(s[1].end, None);
    }
}
