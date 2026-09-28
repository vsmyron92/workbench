//! Programmatic input: sanitising text before it reaches a PTY, bracketed paste framing,
//! named keys, and sniffing pasted images.

/// Longest text accepted by `/input` and `send_text`.
pub const MAX_TEXT: usize = 1024 * 1024;

/// Drop C0 controls (and DEL) except tab and newline, and all carriage returns. This
/// keeps programmatic text from smuggling escape sequences or an early Enter.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\t' || c == '\n' || !(c.is_control() && (c as u32) < 0xa0))
        .collect()
}

/// Bytes that paste `text`: wrapped in bracketed-paste markers when the program enabled
/// them, so newlines do not submit; otherwise newlines become carriage returns.
pub fn paste_payload(text: &str, bracketed: bool) -> Vec<u8> {
    let clean = sanitize(text);
    if bracketed {
        let mut out = Vec::with_capacity(clean.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(clean.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        clean.replace('\n', "\r").into_bytes()
    }
}

/// Bytes that type `text` as keystrokes (no paste framing).
pub fn typed_payload(text: &str) -> Vec<u8> {
    sanitize(text).replace('\n', "\r").into_bytes()
}

/// Named keys the UI and API may send without a terminal socket.
pub fn key_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "esc" | "escape" => b"\x1b",
        "enter" => b"\r",
        "tab" => b"\t",
        "shift_tab" => b"\x1b[Z",
        "ctrl_c" => b"\x03",
        "ctrl_d" => b"\x04",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "backspace" => b"\x7f",
        _ => return None,
    })
}

/// Image types accepted as attachments, by magic bytes: `(extension, mime)`.
pub fn sniff_image(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("png", "image/png"))
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("jpg", "image/jpeg"))
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some(("gif", "image/gif"))
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some(("webp", "image/webp"))
    } else {
        None
    }
}

/// Quote a path for insertion at a shell or agent prompt.
pub fn quote_path(p: &str) -> String {
    if !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%".contains(c)) {
        p.to_string()
    } else {
        format!("'{}'", p.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_controls_but_keeps_tabs_newlines_and_unicode() {
        assert_eq!(sanitize("a\x1b[31mb\r\nc\td\x07\x7f\u{9b}é漢"), "a[31mb\nc\tdé漢");
        assert_eq!(sanitize("line1\r\nline2"), "line1\nline2");
    }

    #[test]
    fn paste_is_bracketed_only_when_enabled() {
        assert_eq!(paste_payload("hi\nthere", true), b"\x1b[200~hi\nthere\x1b[201~");
        assert_eq!(paste_payload("hi\nthere", false), b"hi\rthere");
        // A pasted end marker cannot close the bracket early.
        assert_eq!(paste_payload("x\x1b[201~y", true), b"\x1b[200~x[201~y\x1b[201~");
        assert_eq!(typed_payload("ls\n"), b"ls\r");
    }

    #[test]
    fn sniffs_supported_images_only() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n rest").map(|x| x.0), Some("png"));
        assert_eq!(sniff_image(&[0xff, 0xd8, 0xff, 0xe0, 0, 0]).map(|x| x.0), Some("jpg"));
        assert_eq!(sniff_image(b"GIF89a....").map(|x| x.0), Some("gif"));
        assert_eq!(sniff_image(b"RIFF\x10\0\0\0WEBPVP8 ").map(|x| x.0), Some("webp"));
        assert_eq!(sniff_image(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_image(b"<svg xmlns=...>"), None);
        assert_eq!(sniff_image(b""), None);
    }

    #[test]
    fn quotes_paths_for_prompts() {
        assert_eq!(quote_path("/tmp/a.png"), "/tmp/a.png");
        assert_eq!(quote_path("/tmp/my file.png"), "'/tmp/my file.png'");
        assert_eq!(quote_path("/tmp/it's"), r"'/tmp/it'\''s'");
    }

    #[test]
    fn named_keys() {
        assert_eq!(key_bytes("esc"), Some(&b"\x1b"[..]));
        assert_eq!(key_bytes("shift_tab"), Some(&b"\x1b[Z"[..]));
        assert_eq!(key_bytes("rm -rf"), None);
    }
}
