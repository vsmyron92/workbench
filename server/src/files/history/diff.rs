//! Line diff in unified format for local history (the MCP tool, `…/history/diff`).
//! Myers' O((N+M)·D) algorithm between the common prefix and suffix; past a bound
//! on D it degrades to one hunk that replaces the whole middle.

/// Edit distance searched before giving up on a minimal diff.
const MAX_D: usize = 1500;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

fn split(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return vec![];
    }
    let mut v: Vec<&str> = text.split_inclusive('\n').collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

fn myers(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    if max == 0 {
        return Some(vec![]);
    }
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // Per step only the diagonals it can reach (k in -d-1..=d+1): memory stays O(D²).
    let mut trace: Vec<Vec<isize>> = vec![];
    for d in 0..=max.min(MAX_D) as isize {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let down = k == -d || (k != d && v[(off + k - 1) as usize] < v[(off + k + 1) as usize]);
            let mut x = if down { v[(off + k + 1) as usize] } else { v[(off + k - 1) as usize] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(off + k) as usize] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m));
            }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Op> {
    let mut ops = vec![];
    let (mut x, mut y) = (n, m);
    for d in (0..trace.len() as isize).rev() {
        let t = &trace[d as usize];
        let get = |k: isize| t[(k + d + 1) as usize];
        let k = x - y;
        let down = k == -d || (k != d && get(k - 1) < get(k + 1));
        let prev_k = if down { k + 1 } else { k - 1 };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            ops.push(if x == prev_x { Op::Insert } else { Op::Delete });
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    ops
}

fn ops_of(a: &[&str], b: &[&str]) -> Vec<Op> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let middle = myers(am, bm).unwrap_or_else(|| {
        let mut v = vec![Op::Delete; am.len()];
        v.extend(std::iter::repeat_n(Op::Insert, bm.len()));
        v
    });
    let mut ops = vec![Op::Equal; pre];
    ops.extend(middle);
    ops.extend(std::iter::repeat_n(Op::Equal, suf));
    ops
}

/// Whether two texts differ, and a unified diff of them (`context` lines around
/// each change). Empty when they are equal.
pub fn unified(old: &str, new: &str, old_name: &str, new_name: &str, context: usize) -> String {
    let (a, b) = (split(old), split(new));
    let ops = ops_of(&a, &b);
    if ops.iter().all(|o| *o == Op::Equal) {
        return String::new();
    }
    // Positions (in a and b) at each op.
    let mut pos = Vec::with_capacity(ops.len());
    let (mut i, mut j) = (0usize, 0usize);
    for op in &ops {
        pos.push((i, j));
        match op {
            Op::Equal => {
                i += 1;
                j += 1;
            }
            Op::Delete => i += 1,
            Op::Insert => j += 1,
        }
    }
    // Hunks: runs of changes with `context` equal lines around, merged when close.
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k] != Op::Equal).collect();
    let mut hunks: Vec<(usize, usize)> = vec![];
    for &k in &changed {
        let start = k.saturating_sub(context);
        let end = (k + context + 1).min(ops.len());
        match hunks.last_mut() {
            Some(h) if start <= h.1 => h.1 = h.1.max(end),
            _ => hunks.push((start, end)),
        }
    }
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    for (s, e) in hunks {
        let (a0, b0) = pos[s];
        let a_len = ops[s..e].iter().filter(|o| **o != Op::Insert).count();
        let b_len = ops[s..e].iter().filter(|o| **o != Op::Delete).count();
        let a_start = if a_len == 0 { a0 } else { a0 + 1 };
        let b_start = if b_len == 0 { b0 } else { b0 + 1 };
        out.push_str(&format!("@@ -{a_start},{a_len} +{b_start},{b_len} @@\n"));
        for k in s..e {
            let (ai, bj) = pos[k];
            let (sign, line) = match ops[k] {
                Op::Equal => (' ', a[ai]),
                Op::Delete => ('-', a[ai]),
                Op::Insert => ('+', b[bj]),
            };
            out.push(sign);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::unified;

    #[test]
    fn unified_hunks() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\n";
        let d = unified(old, new, "a/x", "b/x", 1);
        assert_eq!(d, "--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -10,1 +10,2 @@\n j\n+k\n");
        // Close changes merge into one hunk.
        let d = unified("1\n2\n3\n4\n", "x\n2\n3\ny\n", "o", "n", 2);
        assert_eq!(d.matches("@@").count(), 2, "{d}");
        assert_eq!(unified("same\n", "same\n", "o", "n", 3), "");
    }

    #[test]
    fn edges() {
        assert_eq!(unified("", "new\n", "o", "n", 3), "--- o\n+++ n\n@@ -0,0 +1,1 @@\n+new\n");
        assert_eq!(unified("gone\n", "", "o", "n", 3), "--- o\n+++ n\n@@ -1,1 +0,0 @@\n-gone\n");
        let d = unified("a", "b", "o", "n", 3);
        assert!(d.contains("-a\n\\ No newline at end of file\n+b\n"), "{d}");
    }

    #[test]
    fn large_rewrites_still_diff() {
        let old: String = (0..9000).map(|i| format!("old {i}\n")).collect();
        let new: String = (0..9000).map(|i| format!("new {i}\n")).collect();
        let d = unified(&old, &new, "o", "n", 3);
        assert!(d.starts_with("--- o\n+++ n\n@@ -1,9000 +1,9000 @@\n"), "{}", &d[..80]);
    }
}
