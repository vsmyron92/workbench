//! Fuzzy path matching for "Go to file".
//!
//! The query must be a (case-insensitive) subsequence of the path. Among all
//! alignments, a small dynamic program picks the best one: every matched character
//! scores, with bonuses for the start of a path segment or word (`/`, `_`, `-`, `.`,
//! camelCase), for consecutive runs, for landing in the file name, and for exact
//! case; gaps between matched characters cost a little.

const MATCH: i32 = 16;
const BONUS_SLASH: i32 = 10;
const BONUS_SEP: i32 = 8;
const BONUS_CAMEL: i32 = 7;
const BONUS_CONSEC: i32 = 6;
const BONUS_CASE: i32 = 1;
const BONUS_BASENAME: i32 = 2;
const GAP_START: i32 = -3;
const GAP_EXT: i32 = -1;
const BONUS_EXACT_NAME: i32 = 100;
const BONUS_NAME_PREFIX: i32 = 30;
const NEG: i32 = i32::MIN / 4;

pub struct Query {
    lower: Vec<char>,
    orig: Vec<char>,
    lower_str: String,
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

impl Query {
    /// `None` for an empty (or whitespace-only) query.
    pub fn new(q: &str) -> Option<Self> {
        let orig: Vec<char> = q.chars().filter(|c| !c.is_whitespace()).collect();
        if orig.is_empty() || orig.len() > 256 {
            return None;
        }
        let lower: Vec<char> = orig.iter().map(|&c| lower(c)).collect();
        let lower_str = lower.iter().collect();
        Some(Self { lower, orig, lower_str })
    }

    /// Score `cand`, or `None` if the query is not a subsequence of it.
    pub fn score(&self, cand: &str) -> Option<i32> {
        self.run(cand, false).map(|(s, _)| s)
    }

    /// Score plus the matched positions as UTF-16 indices into `cand` (for highlighting in JS).
    pub fn score_with_positions(&self, cand: &str) -> Option<(i32, Vec<u32>)> {
        let (s, pos) = self.run(cand, true)?;
        let mut utf16 = Vec::with_capacity(cand.chars().count());
        let mut acc = 0u32;
        for ch in cand.chars() {
            utf16.push(acc);
            acc += ch.len_utf16() as u32;
        }
        Some((s, pos.into_iter().map(|i| utf16[i]).collect()))
    }

    fn run(&self, cand: &str, track: bool) -> Option<(i32, Vec<usize>)> {
        let (q, qo) = (&self.lower, &self.orig);
        let c: Vec<char> = cand.chars().collect();
        let (m, n) = (q.len(), c.len());
        if m > n {
            return None;
        }
        let lc: Vec<char> = c.iter().map(|&ch| lower(ch)).collect();
        // Cheap subsequence check first; most candidates stop here.
        let mut qi = 0;
        for &ch in &lc {
            if qi < m && ch == q[qi] {
                qi += 1;
            }
        }
        if qi < m {
            return None;
        }
        let base_start = c.iter().rposition(|&ch| ch == '/').map(|i| i + 1).unwrap_or(0);
        let bonus: Vec<i32> = (0..n).map(|j| bonus_at(&c, j, base_start)).collect();
        let mut prev = vec![NEG; n];
        let mut cur = vec![NEG; n];
        let mut from: Vec<Vec<u32>> = if track { vec![vec![u32::MAX; n]; m] } else { vec![] };
        for i in 0..m {
            // r = best over k <= j-2 of prev[k] with the gap-extension penalty applied.
            let (mut r, mut r_arg) = (NEG, u32::MAX);
            for j in 0..n {
                if i > 0 && j >= 2 {
                    r = if r > NEG { r + GAP_EXT } else { NEG };
                    if prev[j - 2] > r {
                        r = prev[j - 2];
                        r_arg = (j - 2) as u32;
                    }
                }
                cur[j] = NEG;
                if lc[j] != q[i] {
                    continue;
                }
                let s = MATCH + bonus[j] + if c[j] == qo[i] { BONUS_CASE } else { 0 };
                if i == 0 {
                    cur[j] = s;
                    continue;
                }
                let (mut best, mut arg) = (NEG, u32::MAX);
                if j >= 1 && prev[j - 1] > NEG {
                    best = prev[j - 1] + BONUS_CONSEC;
                    arg = (j - 1) as u32;
                }
                if r > NEG && r + GAP_START > best {
                    best = r + GAP_START;
                    arg = r_arg;
                }
                if best > NEG {
                    cur[j] = best + s;
                    if track {
                        from[i][j] = arg;
                    }
                }
            }
            std::mem::swap(&mut prev, &mut cur);
        }
        let (mut end, mut best) = (0usize, NEG);
        for (j, &v) in prev.iter().enumerate() {
            if v > best {
                best = v;
                end = j;
            }
        }
        if best <= NEG {
            return None;
        }
        let name: String = lc[base_start..].iter().collect();
        if name == self.lower_str {
            best += BONUS_EXACT_NAME;
        } else if name.starts_with(&self.lower_str) {
            best += BONUS_NAME_PREFIX;
        }
        let mut pos = vec![];
        if track {
            let mut j = end;
            for i in (0..m).rev() {
                pos.push(j);
                if i > 0 {
                    j = from[i][j] as usize;
                }
            }
            pos.reverse();
        }
        Some((best, pos))
    }
}

fn bonus_at(c: &[char], j: usize, base_start: usize) -> i32 {
    let mut b = 0;
    if j == 0 {
        b += BONUS_SLASH;
    } else {
        let (p, ch) = (c[j - 1], c[j]);
        if p == '/' {
            b += BONUS_SLASH;
        } else if matches!(p, '_' | '-' | '.' | ' ') {
            b += BONUS_SEP;
        } else if p.is_lowercase() && ch.is_uppercase() {
            b += BONUS_CAMEL;
        } else if !p.is_ascii_digit() && ch.is_ascii_digit() {
            b += BONUS_SEP / 2;
        }
    }
    if j >= base_start {
        b += BONUS_BASENAME;
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank<'a>(q: &str, cands: &[&'a str]) -> Vec<&'a str> {
        let q = Query::new(q).unwrap();
        let mut v: Vec<(i32, &str)> = cands.iter().filter_map(|c| q.score(c).map(|s| (s, *c))).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
        v.into_iter().map(|(_, c)| c).collect()
    }

    #[test]
    fn subsequence_required() {
        let q = Query::new("abc").unwrap();
        assert!(q.score("a/b/c").is_some());
        assert!(q.score("acb").is_none());
        assert!(q.score("ab").is_none());
        assert!(Query::new("  ").is_none());
    }

    #[test]
    fn prefers_file_names_and_word_starts() {
        assert_eq!(
            rank("main", &["src/domain/remains.rs", "src/main.rs", "maintenance/notes.txt"]),
            vec!["src/main.rs", "maintenance/notes.txt", "src/domain/remains.rs"]
        );
        assert_eq!(rank("mainrs", &["web/src/main.tsx", "server/src/main.rs"])[0], "server/src/main.rs");
        assert_eq!(rank("fm", &["frame.rs", "files/mod.rs"])[0], "files/mod.rs");
        assert_eq!(rank("cargo", &["Cargo.lock", "server/Cargo.toml", "docs/cargo-notes/x.md"])[0], "Cargo.lock");
    }

    #[test]
    fn camel_case_and_consecutive() {
        assert_eq!(rank("fb", &["foobar.ts", "FooBar.ts"])[0], "FooBar.ts");
        assert_eq!(rank("edit", &["e/d/i/t.rs", "EditorPanel.tsx"])[0], "EditorPanel.tsx");
    }

    #[test]
    fn exact_file_name_wins() {
        assert_eq!(rank("mod.rs", &["src/files/mod.rs", "src/model/modes.rs", "mod.rs.bak"])[0], "src/files/mod.rs");
    }

    #[test]
    fn positions_are_utf16_indices() {
        let q = Query::new("mr").unwrap();
        let (_, pos) = q.score_with_positions("src/main.rs").unwrap();
        assert_eq!(pos, vec![4, 9]);
        let (_, pos) = q.score_with_positions("😀/main.rs").unwrap();
        assert_eq!(pos, vec![3, 8]);
    }
}
