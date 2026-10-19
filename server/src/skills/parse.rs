//! A `SKILL.md`: its front matter, and what the agent CLIs make of it.
//!
//! The format is the open Agent Skills standard (agentskills.io) that Claude Code, Codex, Gemini
//! CLI and Kimi Code all read: YAML front matter (`name`, `description`, and for Claude Code a
//! dozen more), then the instructions. Everything here is read-only and tolerant: a file that
//! does not parse is reported, never fatal, and never executed.
//!
//! What a CLI does with a broken front matter is documented for Claude Code: the skill still
//! loads, with no metadata at all (so no name from the file and no description to match on).
//! [`parse`] gives the same picture: `fields` stays empty and `error` says why.

use serde_json::{Map, Value};

/// The largest `SKILL.md` Workbench reads. Skills are short by design (the standard advises
/// 500 lines); a bigger file is reported as such and its text is not parsed.
pub const MAX_BYTES: u64 = 256 * 1024;
/// Claude Code cuts `description` + `when_to_use` at this many characters in the skill listing
/// it puts in every session (`skillListingMaxDescChars`).
pub const CLAUDE_LISTING_CAP: usize = 1536;
/// The Agent Skills standard's limit on `description` (the other CLIs read the same format).
pub const SPEC_DESCRIPTION_MAX: usize = 1024;
/// The standard's limit on `name`.
pub const SPEC_NAME_MAX: usize = 64;
/// Longer than this many lines, the standard advises moving detail into files the skill links to.
pub const ADVISED_MAX_LINES: usize = 500;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// The text starts with a `---` block that is closed.
    pub has_front_matter: bool,
    /// Why the front matter is unusable (a fixed message plus the YAML parser's position).
    pub error: Option<String>,
    /// The front matter as a JSON object; empty when there is none or it does not parse.
    pub fields: Map<String, Value>,
    /// The text after the front matter (all of it when there is none).
    pub body: String,
}

/// Split `text` into its front matter (the YAML between the two `---` lines) and the rest.
pub fn split(text: &str) -> (Option<&str>, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else { return (None, text) };
    if first.trim_end() != "---" {
        return (None, text);
    }
    let start = first.len();
    let mut at = start;
    for line in lines {
        let bare = line.trim_end();
        if bare == "---" || bare == "..." {
            return (Some(&text[start..at]), &text[at + line.len()..]);
        }
        at += line.len();
    }
    // Never closed: the whole file is text.
    (None, text)
}

pub fn parse(text: &str) -> Parsed {
    let (front, body) = split(text);
    let Some(front) = front else {
        return Parsed { body: body.to_string(), ..Parsed::default() };
    };
    let mut out = Parsed { has_front_matter: true, body: body.to_string(), ..Parsed::default() };
    // An empty block (`---` directly followed by `---`) is valid and says nothing.
    if front.trim().is_empty() {
        return out;
    }
    match serde_norway::from_str::<Value>(front) {
        Ok(Value::Object(m)) => out.fields = m,
        Ok(Value::Null) => {}
        Ok(_) => out.error = Some("the front matter is not a list of `key: value` lines".into()),
        // The parser's message names a line and column, never the text itself.
        Err(e) => out.error = Some(format!("the front matter is not valid YAML ({e})")),
    }
    out
}

fn scalar(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

impl Parsed {
    pub fn text(&self, key: &str) -> Option<String> {
        self.fields.get(key).and_then(scalar)
    }

    /// Booleans the way Claude Code reads them: `true/false`, `yes/no`, `on/off`, `1/0`.
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.fields.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => n.as_i64().map(|i| i != 0),
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Some(true),
                "false" | "no" | "off" | "0" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    /// A list written as a YAML list or as one string (space, comma or newline separated, with
    /// parentheses kept whole: `Bash(git add *)` is one entry).
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.fields.get(key) {
            Some(Value::Array(a)) => a.iter().filter_map(scalar).collect(),
            Some(Value::String(s)) => tokens(s),
            _ => vec![],
        }
    }

    pub fn name(&self) -> Option<String> {
        self.text("name")
    }

    pub fn description(&self) -> Option<String> {
        self.text("description")
    }

    pub fn when_to_use(&self) -> Option<String> {
        self.text("when_to_use")
    }

}

/// Split on whitespace and commas outside parentheses.
pub fn tokens(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (vec![], String::new(), 0usize);
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            c if (c.is_whitespace() || c == ',') && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The first non-empty line of the body, with Markdown's heading marks and emphasis taken off:
/// what Claude Code (and Kimi Code, cut at 240 characters) use when a skill has no description.
pub fn first_line(body: &str) -> Option<String> {
    let line = body.lines().map(str::trim).find(|l| !l.is_empty())?;
    // Underscores stay: they are part of `snake_case` names far more often than emphasis.
    let line: String = line.trim_start_matches('#').chars().filter(|c| !matches!(c, '*' | '`')).collect();
    let line = line.trim();
    (!line.is_empty()).then(|| line.chars().take(240).collect())
}

/// Whether the text uses Claude Code's shell injection: an inline `` !`command` `` outside a code
/// block, or a fenced block that starts with ```` ```! ````. Claude Code runs those when the
/// skill is invoked, before the model sees the text.
pub fn uses_shell_injection(body: &str) -> bool {
    let mut fence: Option<String> = None;
    for line in body.lines() {
        let t = line.trim_start();
        let ticks = t.chars().take_while(|c| *c == '`').count();
        if ticks >= 3 {
            match &fence {
                None => {
                    if t[ticks..].trim_start().starts_with('!') {
                        return true;
                    }
                    fence = Some("`".repeat(ticks));
                }
                Some(f) if ticks >= f.len() && t[ticks..].trim().is_empty() => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if fence.is_none() && has_inline_shell(line) {
            return true;
        }
    }
    false
}

fn has_inline_shell(line: &str) -> bool {
    let b = line.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'!' && b[i + 1] == b'`' {
            // A command: something between the backticks, on this line.
            if let Some(end) = line[i + 2..].find('`') {
                if end > 0 {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// Characters to tokens, the usual estimate (four to a token).
pub fn tokens_of(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// `error`, `warn` or `info`.
    pub level: &'static str,
    pub code: &'static str,
    pub message: String,
}

impl Issue {
    pub fn new(level: &'static str, code: &'static str, message: impl Into<String>) -> Self {
        Self { level, code, message: message.into() }
    }
}

/// A name the standard allows: lowercase letters and digits, single hyphens inside, up to 64.
pub fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.chars().count() <= SPEC_NAME_MAX
        && !n.starts_with('-')
        && !n.ends_with('-')
        && !n.contains("--")
        && n.chars().all(|c| (c.is_alphanumeric() && !c.is_uppercase()) || c == '-')
}

/// What is wrong with a skill's file, for the readers that matter (`claude`: Claude Code reads it,
/// `others`: another CLI does; `command`: it is a slash command file, which needs less).
pub fn check(p: &Parsed, folder: &str, lines: usize, claude: bool, others: bool, command: bool) -> Vec<Issue> {
    let mut out = vec![];
    if let Some(e) = &p.error {
        let what = if claude { "so agents load this skill with no name and no description to match on" } else { "so agents may not load it at all" };
        out.push(Issue::new("error", "front_matter", format!("{e}, {what}")));
    }
    let description = p.description();
    if description.is_none() && p.error.is_none() {
        // A slash command is typed by the user: its description only labels it in the menu, so it is a remark.
        let (level, what) = if command {
            ("info", "No description: the menu shows the first line of the text instead")
        } else {
            ("warn", "No description: agents cannot tell when to use it (Claude Code and Kimi Code fall back to the first line of the text)")
        };
        out.push(Issue::new(level, "no_description", what));
    }
    if let Some(d) = &description {
        let n = d.chars().count() + p.when_to_use().map_or(0, |w| w.chars().count());
        if claude && n > CLAUDE_LISTING_CAP {
            out.push(Issue::new("warn", "description_long", format!("The description is {n} characters: Claude Code cuts it at {CLAUDE_LISTING_CAP} in its listing")));
        } else if others && n > SPEC_DESCRIPTION_MAX {
            out.push(Issue::new("warn", "description_long", format!("The description is {n} characters; the Agent Skills standard allows {SPEC_DESCRIPTION_MAX}")));
        }
    }
    if let Some(name) = p.name() {
        if name != folder {
            out.push(Issue::new("warn", "name_mismatch", format!("Named “{name}” in its front matter but kept in the folder “{folder}”: the standard wants them to match")));
        } else if !valid_name(&name) {
            out.push(Issue::new(
                if others { "warn" } else { "info" },
                "name_invalid",
                format!("“{name}” is not a name the standard allows (lowercase letters, digits and single hyphens, up to {SPEC_NAME_MAX} characters)"),
            ));
        }
    }
    if lines > ADVISED_MAX_LINES {
        out.push(Issue::new("info", "long_text", format!("{lines} lines: the standard advises under {ADVISED_MAX_LINES}, with detail in files the skill links to")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_is_split_at_the_two_marker_lines() {
        let (f, b) = split("---\nname: a\n---\n# Title\ntext\n");
        assert_eq!((f, b), (Some("name: a\n"), "# Title\ntext\n"));
        // CRLF, a byte order mark, trailing blanks after the markers, `...` as the closer.
        let (f, b) = split("\u{feff}---  \r\nname: a\r\n...\r\nbody");
        assert_eq!((f, b), (Some("name: a\r\n"), "body"));
        // No front matter: the first line is not the marker, or it is never closed.
        assert_eq!(split("# Title\n---\nx\n---\n"), (None, "# Title\n---\nx\n---\n"));
        assert_eq!(split("---\nname: a\nno closer\n"), (None, "---\nname: a\nno closer\n"));
        assert_eq!(split(""), (None, ""));
        // An empty block.
        assert_eq!(split("---\n---\nbody"), (Some(""), "body"));
    }

    #[test]
    fn descriptions_come_out_of_every_yaml_style_skills_use() {
        let folded = parse("---\nname: x\ndescription: >\n  Use this\n  when asked.\n---\n# X\n");
        assert_eq!(folded.description().as_deref(), Some("Use this when asked."));
        let literal = parse("---\nname: x\ndescription: |\n  Line one\n  Line two\n---\n");
        assert_eq!(literal.description().as_deref(), Some("Line one\nLine two"));
        let quoted = parse("---\nname: \"x\"\ndescription: \"Say: \\\"hi\\\" # not a comment\"\n---\n");
        assert_eq!(quoted.description().as_deref(), Some("Say: \"hi\" # not a comment"));
        let single = parse("---\ndescription: 'It''s fine'\n---\n");
        assert_eq!(single.description().as_deref(), Some("It's fine"));
        let plain = parse("---\nname: x\ndescription: Does a thing\nlicense: MIT\n---\n");
        assert_eq!((plain.description().as_deref(), plain.text("license").as_deref()), (Some("Does a thing"), Some("MIT")));
        assert!(plain.error.is_none() && plain.has_front_matter);
    }

    #[test]
    fn a_front_matter_that_does_not_parse_loads_with_no_metadata_and_says_why() {
        // A colon in a plain scalar: the commonest mistake.
        let p = parse("---\nname: x\ndescription: Use when: the user asks\n---\n# Body\n");
        assert!(p.has_front_matter);
        assert!(p.error.as_deref().is_some_and(|e| e.starts_with("the front matter is not valid YAML")), "{:?}", p.error);
        assert!(p.fields.is_empty() && p.name().is_none() && p.description().is_none());
        assert_eq!(p.body, "# Body\n");
        // Not a mapping.
        assert!(parse("---\n- a\n- b\n---\n").error.is_some());
        // An empty block is fine.
        let empty = parse("---\n\n---\nbody");
        assert!(empty.error.is_none() && empty.has_front_matter && empty.fields.is_empty());
        // No front matter at all is not an error.
        let none = parse("# Just text\n");
        assert!(none.error.is_none() && !none.has_front_matter);
        assert_eq!(first_line(&none.body).as_deref(), Some("Just text"));
    }

    #[test]
    fn a_front_matter_of_exponential_aliases_is_refused_quickly_not_expanded() {
        // Nine levels of nine references each would be 387 million leaves, from 542 bytes.
        let mut text = String::from("---\na0: &a0 [lol, lol, lol, lol, lol, lol, lol, lol, lol]\n");
        for i in 1..9 {
            text.push_str(&format!("a{i}: &a{i} [{}]\n", vec![format!("*a{}", i - 1); 9].join(", ")));
        }
        text.push_str("name: laughs\ndescription: aliases\n---\nbody\n");
        let started = std::time::Instant::now();
        let p = parse(&text);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
        assert!(p.error.is_some() && p.fields.is_empty(), "{:?}", p.error);
    }

    #[test]
    fn flags_and_lists_are_read_the_lenient_way_claude_code_reads_them() {
        let p = parse(
            "---\ndisable-model-invocation: yes\nuser-invocable: off\nx: 1\nallowed-tools: Bash(git add *), Read Grep\npaths:\n  - src/**\n  - docs/*.md\n---\n",
        );
        assert_eq!((p.flag("disable-model-invocation"), p.flag("user-invocable"), p.flag("x"), p.flag("missing")), (Some(true), Some(false), Some(true), None));
        assert_eq!(p.list("allowed-tools"), ["Bash(git add *)", "Read", "Grep"]);
        assert_eq!(p.list("paths"), ["src/**", "docs/*.md"]);
        assert!(p.list("missing").is_empty());
    }

    #[test]
    fn shell_injection_is_found_where_claude_code_would_run_it_and_not_in_examples() {
        assert!(uses_shell_injection("Branch: !`git branch --show-current`\n"));
        assert!(uses_shell_injection("Run:\n```!\nls\n```\n"));
        // Inside an ordinary fenced block it is an example, and a lone `!` is prose.
        assert!(!uses_shell_injection("Example:\n```bash\necho !`date`\n```\n"));
        assert!(!uses_shell_injection("Wow! `code` here, and !`` empty\n"));
        assert!(!uses_shell_injection("# Plain\n\ntext\n"));
        // A longer fence closes only with an equal or longer one.
        assert!(!uses_shell_injection("````md\n```\n!`x`\n```\n````\n"));
    }

    #[test]
    fn names_follow_the_standard_and_the_checks_say_what_is_off() {
        assert!(valid_name("data-capture") && valid_name("a1"));
        for bad in ["", "-a", "a-", "a--b", "Data", "a_b", "a b", &"x".repeat(65)] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        let codes = |p: &Parsed, folder: &str, lines: usize| check(p, folder, lines, true, true, false).iter().map(|i| i.code).collect::<Vec<_>>();
        let ok = parse("---\nname: a\ndescription: Does a. Use when b.\n---\n");
        assert!(codes(&ok, "a", 10).is_empty());
        assert_eq!(codes(&ok, "b", 10), ["name_mismatch"]);
        assert_eq!(codes(&ok, "a", 501), ["long_text"]);
        assert_eq!(codes(&parse("---\nname: a\n---\n"), "a", 3), ["no_description"]);
        assert_eq!(codes(&parse("---\nname: Mixed\ndescription: d\n---\n"), "Mixed", 3), ["name_invalid"]);
        let long = parse(&format!("---\nname: a\ndescription: {}\n---\n", "w ".repeat(800)));
        // Claude Code only cuts above 1,536; the standard stops at 1,024.
        let mid = parse(&format!("---\nname: a\ndescription: {}\n---\n", "w ".repeat(600)));
        assert_eq!(check(&mid, "a", 3, true, false, false).len(), 0);
        assert_eq!(check(&mid, "a", 3, false, true, false)[0].code, "description_long");
        assert!(check(&long, "a", 3, true, false, false)[0].message.contains("1536"));
        // A broken front matter is the one error, and not also "no description".
        let broken = parse("---\nname: a\ndescription: x: y\n---\n");
        assert_eq!(check(&broken, "a", 3, true, false, false).iter().map(|i| (i.level, i.code)).collect::<Vec<_>>(), [("error", "front_matter")]);
        // A command without a description is a remark, a skill without one a warning.
        let bare = parse("Just the prompt.\n");
        assert_eq!(check(&bare, "ship", 1, true, false, true).iter().map(|i| (i.level, i.code)).collect::<Vec<_>>(), [("info", "no_description")]);
        assert_eq!(check(&bare, "ship", 1, true, false, false).iter().map(|i| (i.level, i.code)).collect::<Vec<_>>(), [("warn", "no_description")]);
    }

    #[test]
    fn the_first_line_stands_in_for_a_missing_description() {
        assert_eq!(first_line("\n\n## **Deploy** the `app`\nmore\n").as_deref(), Some("Deploy the app"));
        assert_eq!(first_line("Uses my_tool and `x_y`").as_deref(), Some("Uses my_tool and x_y"));
        assert_eq!(first_line("   \n").as_deref(), None);
        assert_eq!(first_line(&"x".repeat(500)).unwrap().chars().count(), 240);
        assert_eq!(tokens_of(0), 0);
        assert_eq!(tokens_of(1), 1);
        assert_eq!(tokens_of(401), 101);
    }
}
