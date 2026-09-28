//! JSON with comments (`devcontainer.json` is JSONC): `//` and `/* */` comments and
//! trailing commas are removed before `serde_json` parses the text. Strings are copied
//! untouched, escapes included, so a `//` inside a URL survives.

use serde_json::Value;

/// Parse JSONC into a value. The error names the line and column like `serde_json`'s
/// (positions are kept: comments become spaces, newlines stay).
pub fn parse(text: &str) -> Result<Value, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let plain = strip(text);
    serde_json::from_str(&plain).map_err(|e| e.to_string())
}

/// Remove comments and trailing commas, keeping every other byte (and every newline)
/// where it was.
pub fn strip(text: &str) -> String {
    let b = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    // Index in `out` of a comma that may turn out to be trailing.
    let mut pending_comma: Option<usize> = None;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' => {
                pending_comma = None;
                out.push(c);
                i += 1;
                while i < b.len() {
                    let d = b[i];
                    out.push(d);
                    i += 1;
                    if d == b'\\' && i < b.len() {
                        out.push(b[i]);
                        i += 1;
                    } else if d == b'"' {
                        break;
                    }
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                out.extend_from_slice(b"  ");
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    out.push(if b[i] == b'\n' { b'\n' } else { b' ' });
                    i += 1;
                }
                if i < b.len() {
                    out.extend_from_slice(b"  ");
                    i += 2;
                }
            }
            b',' => {
                pending_comma = Some(out.len());
                out.push(c);
                i += 1;
            }
            b'}' | b']' => {
                if let Some(at) = pending_comma.take() {
                    out[at] = b' ';
                }
                out.push(c);
                i += 1;
            }
            c if c.is_ascii_whitespace() => {
                out.push(c);
                i += 1;
            }
            _ => {
                pending_comma = None;
                out.push(c);
                i += 1;
            }
        }
    }
    // Only ASCII bytes were replaced (by ASCII), so the text is still UTF-8.
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comments_and_trailing_commas() {
        let v = parse(
            r#"// For format details, see https://aka.ms/devcontainer.json.
{
    "name": "Rust", // the name
    /* a block
       comment */
    "image": "mcr.microsoft.com/devcontainers/rust:1-1-bookworm",
    "forwardPorts": [3000, 8080,],
    "url": "http://example.com/a//b", // not a comment inside the string
    "esc": "quote \" // still string",
    "features": {
        "ghcr.io/devcontainers/features/node:1": {},
    },
}"#,
        )
        .unwrap();
        assert_eq!(v["name"], "Rust");
        assert_eq!(v["forwardPorts"], json!([3000, 8080]));
        assert_eq!(v["url"], "http://example.com/a//b");
        assert_eq!(v["esc"], "quote \" // still string");
        assert!(v["features"]["ghcr.io/devcontainers/features/node:1"].is_object());
    }

    #[test]
    fn errors_keep_positions() {
        let e = parse("{\n  // x\n  \"a\": 1 2\n}").unwrap_err();
        assert!(e.contains("line 3"), "{e}");
        assert!(parse("\u{feff}{\"a\":1}").is_ok());
        // A comma followed by a comment and then a bracket is still trailing.
        assert_eq!(parse("[1, 2, // two\n]").unwrap(), json!([1, 2]));
    }
}
