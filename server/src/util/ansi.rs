//! Strip terminal escape sequences so log lines can be matched with plain regexes.

use std::sync::LazyLock;

use regex::Regex;

static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    // CSI … final byte | OSC … (BEL | ST) | two-byte ESC sequences
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]").unwrap()
});

pub fn strip(s: &str) -> String {
    ANSI.replace_all(s, "").replace('\r', "")
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_vite_banner() {
        let s = "  \x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\r";
        assert_eq!(super::strip(s), "  ➜  Local:   http://localhost:5173/");
    }

    #[test]
    fn strips_osc_hyperlinks() {
        assert_eq!(super::strip("\x1b]8;;http://x\x07link\x1b]8;;\x07"), "link");
    }
}
