//! Line endings: a working tree that git checks out with CRLF over an index holding LF
//! (`core.autocrlf=true`, the default of Git for Windows; `text` or `eol=crlf` attributes).
//!
//! Git reads such a file with its CRLFs turned into LFs. Its diffs, and the patches built
//! from them for `git apply --cached` (staging hunks and lines), therefore already have LF on
//! the working-tree side, and `git apply` without `--cached` (rolling hunks and lines back)
//! writes CRLF again. What Workbench reads or writes itself follows git: the working-tree side
//! of a diff and a conflicted file are shown with LF, like the hunks, and a conflict resolved
//! with edited text is written back with CRLF. Every other file (LF, CRLF the automatic
//! conversions leave alone because the index has CRs too, no conversion configured) is read
//! and written byte for byte.

use super::cmd::{literal, split_z};
use super::repo::Repo;

/// How git converts one working-tree file on its way into the index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Eol {
    /// Git reads the file with its CRLFs turned into LFs (it has some; the index has none).
    pub normalized: bool,
    /// Every line of the normalized file ends in CRLF: text going back into it gets them too.
    pub crlf: bool,
}

impl Eol {
    /// The file's text as git reads it.
    pub fn read(&self, text: String) -> String {
        if self.normalized { text.replace("\r\n", "\n") } else { text }
    }

    /// Text for the file (LF, or anything a client sends) as it is written to the working tree.
    pub fn write(&self, text: String) -> String {
        if !self.crlf {
            return text;
        }
        let mut out = String::with_capacity(text.len() + text.len() / 16);
        let mut prev = '\0';
        for c in text.chars() {
            if c == '\n' && prev != '\r' {
                out.push('\r');
            }
            out.push(c);
            prev = c;
        }
        out
    }
}

/// The conversion of `repo_path` (repository-relative, tracked or not): `git ls-files --eol`
/// tells the line endings of the index and the working tree and the attributes' say;
/// `core.autocrlf` decides for files no attribute covers. A failing lookup converts nothing.
pub async fn of(repo: &Repo, repo_path: &str) -> Eol {
    let Some(out) = output(repo, &["ls-files", "--eol", "-z", "--cached", "--others", "--", &literal(repo_path)]).await else {
        return Eol::default();
    };
    let records = split_z(&out);
    let infos: Vec<EolInfo> = records.iter().filter_map(|r| parse(r)).collect();
    let autocrlf = if infos.iter().any(|i| i.attr.is_empty()) { autocrlf(repo).await } else { None };
    decide(&infos, autocrlf.as_deref())
}

/// `core.autocrlf`, when set. A bare `autocrlf` key (true) and `autocrlf =` (false) both
/// print as nothing: git's boolean reading tells them apart.
async fn autocrlf(repo: &Repo) -> Option<String> {
    let value = String::from_utf8_lossy(&output(repo, &["config", "--get", "core.autocrlf"]).await?).trim().to_string();
    if !value.is_empty() {
        return Some(value);
    }
    let value = output(repo, &["config", "--get", "--type=bool", "core.autocrlf"]).await?;
    Some(String::from_utf8_lossy(&value).trim().to_string())
}

/// Standard output of a read-only git command that succeeded.
async fn output(repo: &Repo, args: &[&str]) -> Option<Vec<u8>> {
    let out = repo.git().args(args.iter().copied()).run().await.ok()?;
    out.ok().then_some(out.stdout)
}

/// One record of `git ls-files --eol`: `i/lf    w/crlf  attr/text=auto eol=crlf\t<path>`
/// (a conflicted file has one per stage; an untracked one has an empty `i/`).
#[derive(Debug, PartialEq)]
struct EolInfo<'a> {
    index: &'a str,
    worktree: &'a str,
    attr: &'a str,
}

fn parse(record: &str) -> Option<EolInfo<'_>> {
    let (info, _path) = record.split_once('\t')?;
    let field = |prefix: &str| info.split_whitespace().find_map(|t| t.strip_prefix(prefix));
    Some(EolInfo { index: field("i/")?, worktree: field("w/")?, attr: info.split_once("attr/")?.1.trim() })
}

fn decide(infos: &[EolInfo], autocrlf: Option<&str>) -> Eol {
    let Some(first) = infos.first() else { return Eol::default() };
    // The automatic conversions (`text=auto`, `core.autocrlf`) leave a file whose index
    // version has CRs alone; `text` (and `eol=`) converts it all the same (git then shows
    // every line changed, until the file is renormalized).
    let index_lf = infos.iter().all(|i| matches!(i.index, "lf" | "none" | ""));
    let converts = match first.attr {
        "-text" => false,
        // No attribute decides: `core.autocrlf` true or input.
        "" => index_lf && autocrlf.is_some_and(autocrlf_on),
        a if a.starts_with("text=auto") => index_lf,
        // text, text eol=lf|crlf
        _ => true,
    };
    let normalized = converts && matches!(first.worktree, "crlf" | "mixed");
    Eol { normalized, crlf: normalized && first.worktree == "crlf" }
}

/// `core.autocrlf` as git reads it: `input`, or a true boolean.
fn autocrlf_on(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    matches!(v.as_str(), "input" | "true" | "yes" | "on") || v.parse::<i64>().is_ok_and(|n| n != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info<'a>(index: &'a str, worktree: &'a str, attr: &'a str) -> EolInfo<'a> {
        EolInfo { index, worktree, attr }
    }

    #[test]
    fn parses_ls_files_eol_records() {
        assert_eq!(parse("i/lf    w/crlf  attr/                 \tw.txt"), Some(info("lf", "crlf", "")));
        assert_eq!(parse("i/      w/crlf  attr/text=auto eol=crlf\tdir/new file.txt"), Some(info("", "crlf", "text=auto eol=crlf")));
        assert_eq!(parse("i/-text w/-text attr/-text             \tx.png"), Some(info("-text", "-text", "-text")));
        assert_eq!(parse("garbage"), None);
    }

    #[test]
    fn only_files_git_normalizes_are_converted() {
        // core.autocrlf decides when no attribute does.
        assert_eq!(decide(&[info("lf", "crlf", "")], Some("true")), Eol { normalized: true, crlf: true });
        assert_eq!(decide(&[info("lf", "crlf", "")], Some("input")), Eol { normalized: true, crlf: true });
        assert_eq!(decide(&[info("", "crlf", "")], Some("1")), Eol { normalized: true, crlf: true }, "untracked");
        assert_eq!(decide(&[info("lf", "crlf", "")], Some("false")), Eol::default(), "a real change of line endings");
        assert_eq!(decide(&[info("lf", "crlf", "")], None), Eol::default());
        // Attributes.
        assert_eq!(decide(&[info("lf", "crlf", "text eol=crlf")], None), Eol { normalized: true, crlf: true });
        assert_eq!(decide(&[info("none", "crlf", "text=auto")], Some("false")), Eol { normalized: true, crlf: true });
        assert_eq!(decide(&[info("lf", "crlf", "-text")], Some("true")), Eol::default());
        // Mixed endings are read as git reads them, but not written back with CRLF.
        assert_eq!(decide(&[info("lf", "mixed", "")], Some("true")), Eol { normalized: true, crlf: false });
        // CRs in the index (CRLF committed as is) stop the automatic conversions, not `text`.
        assert_eq!(decide(&[info("crlf", "crlf", "")], Some("true")), Eol::default());
        assert_eq!(decide(&[info("crlf", "crlf", "text=auto")], None), Eol::default());
        assert_eq!(decide(&[info("crlf", "crlf", "text")], None), Eol { normalized: true, crlf: true });
        assert_eq!(decide(&[info("mixed", "crlf", "text eol=crlf")], Some("true")), Eol { normalized: true, crlf: true });
        // An LF working tree, nothing known.
        assert_eq!(decide(&[info("lf", "lf", "")], Some("true")), Eol::default());
        assert_eq!(decide(&[], Some("true")), Eol::default());
        // A conflicted file: every stage must be LF.
        assert!(decide(&[info("lf", "crlf", ""), info("lf", "crlf", ""), info("none", "crlf", "")], Some("true")).crlf);
        assert!(!decide(&[info("lf", "crlf", ""), info("crlf", "crlf", "")], Some("true")).normalized);
    }

    #[test]
    fn reads_with_lf_and_writes_crlf_back() {
        let e = Eol { normalized: true, crlf: true };
        assert_eq!(e.read("a\r\nb\r\n".into()), "a\nb\n");
        assert_eq!(e.read("lone\rcr\r\n".into()), "lone\rcr\n");
        assert_eq!(e.write("a\nb\r\nc".into()), "a\r\nb\r\nc");
        let raw = Eol::default();
        assert_eq!(raw.read("a\r\nb\n".into()), "a\r\nb\n");
        assert_eq!(raw.write("a\nb\n".into()), "a\nb\n");
        assert!(!autocrlf_on("0") && !autocrlf_on("off") && !autocrlf_on("") && autocrlf_on(" True ") && autocrlf_on("yes"));
    }
}
