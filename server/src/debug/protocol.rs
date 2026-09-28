//! Debug Adapter Protocol wire format: `Content-Length: N\r\n\r\n` followed by N bytes
//! of JSON. Adapters sometimes print a stray line on stdout before the first header
//! (a warning from a Python import, gdb's banner without `-q`); such bytes are returned
//! as noise instead of breaking the stream.

use serde_json::Value;

/// Refuse messages larger than this (a garbage header must not make us allocate
/// gigabytes). Big `variables` answers of real adapters stay far below.
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// A header block longer than this is not a DAP header.
const MAX_HEADER: usize = 8 * 1024;
const MARKER: &[u8] = b"content-length:";

pub fn encode(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(msg).unwrap_or_else(|_| b"{}".to_vec());
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

#[derive(Debug, PartialEq)]
pub enum Frame {
    Message(Value),
    /// Bytes before a header (not DAP): shown in the console as adapter output.
    Noise(String),
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
    BadHeader(String),
    TooLarge(usize),
    BadJson(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::BadHeader(h) => write!(f, "malformed DAP header: {h}"),
            DecodeError::TooLarge(n) => write!(f, "DAP message of {n} bytes is too large"),
            DecodeError::BadJson(e) => write!(f, "DAP message is not JSON: {e}"),
        }
    }
}

/// Incremental decoder: `push` what the transport read, then call `next` until it
/// returns `Ok(None)`.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `find` ignoring ASCII case (`needle` is lower case).
fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle))
}

impl Decoder {
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next(&mut self) -> Result<Option<Frame>, DecodeError> {
        if self.buf.is_empty() {
            return Ok(None);
        }
        let Some(start) = find_ci(&self.buf, MARKER) else {
            // No header yet: whole lines are noise; keep a partial line (it may be the
            // start of a header).
            if let Some(nl) = self.buf.iter().rposition(|b| *b == b'\n') {
                let noise: Vec<u8> = self.buf.drain(..=nl).collect();
                return Ok(Some(Frame::Noise(String::from_utf8_lossy(&noise).into_owned())));
            }
            if self.buf.len() > MAX_HEADER {
                let noise: Vec<u8> = std::mem::take(&mut self.buf);
                return Ok(Some(Frame::Noise(String::from_utf8_lossy(&noise).into_owned())));
            }
            return Ok(None);
        };
        if start > 0 {
            let noise: Vec<u8> = self.buf.drain(..start).collect();
            let text = String::from_utf8_lossy(&noise).into_owned();
            if !text.trim().is_empty() {
                return Ok(Some(Frame::Noise(text)));
            }
        }
        let Some(end) = find(&self.buf, b"\r\n\r\n") else {
            if self.buf.len() > MAX_HEADER {
                let head = String::from_utf8_lossy(&self.buf[..64.min(self.buf.len())]).into_owned();
                self.buf.clear();
                return Err(DecodeError::BadHeader(head));
            }
            return Ok(None);
        };
        let header = String::from_utf8_lossy(&self.buf[..end]).into_owned();
        let mut len: Option<usize> = None;
        for line in header.split("\r\n") {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().ok();
                }
            }
        }
        let Some(len) = len else {
            self.buf.drain(..end + 4);
            return Err(DecodeError::BadHeader(header));
        };
        if len > MAX_MESSAGE {
            self.buf.clear();
            return Err(DecodeError::TooLarge(len));
        }
        let body_start = end + 4;
        if self.buf.len() < body_start + len {
            return Ok(None);
        }
        let body: Vec<u8> = self.buf.drain(..body_start + len).skip(body_start).collect();
        match serde_json::from_slice::<Value>(&body) {
            Ok(v) if v.is_object() => Ok(Some(Frame::Message(v))),
            Ok(_) => Err(DecodeError::BadJson("not an object".into())),
            Err(e) => Err(DecodeError::BadJson(e.to_string())),
        }
    }
}

/// The error text of a failed response: `body.error.format` with its `{variables}`
/// filled in, else `message`, else the command.
pub fn error_text(resp: &Value) -> String {
    if let Some(err) = resp.pointer("/body/error") {
        if let Some(fmt) = err.get("format").and_then(Value::as_str) {
            let mut s = fmt.to_string();
            if let Some(vars) = err.get("variables").and_then(Value::as_object) {
                for (k, v) in vars {
                    let v = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
                    s = s.replace(&format!("{{{k}}}"), &v);
                }
            }
            return s;
        }
    }
    match resp.get("message").and_then(Value::as_str) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => format!("{} failed", resp.get("command").and_then(Value::as_str).unwrap_or("request")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msgs(d: &mut Decoder) -> Vec<Frame> {
        let mut v = vec![];
        while let Some(f) = d.next().unwrap() {
            v.push(f);
        }
        v
    }

    #[test]
    fn round_trip_and_split_reads() {
        let a = encode(&json!({"seq": 1, "type": "event", "event": "initialized"}));
        let b = encode(&json!({"seq": 2, "type": "response", "request_seq": 1, "success": true, "body": {"x": "é✓"}}));
        let mut all = a.clone();
        all.extend_from_slice(&b);
        // Every split point, including inside the header and the UTF-8 body.
        for cut in 0..all.len() {
            let mut d = Decoder::default();
            d.push(&all[..cut]);
            let mut got = msgs(&mut d);
            d.push(&all[cut..]);
            got.extend(msgs(&mut d));
            assert_eq!(got.len(), 2, "cut at {cut}: {got:?}");
            assert_eq!(got[1], Frame::Message(json!({"seq": 2, "type": "response", "request_seq": 1, "success": true, "body": {"x": "é✓"}})));
        }
    }

    #[test]
    fn noise_before_headers_is_reported_not_fatal() {
        let mut d = Decoder::default();
        d.push(b"warning: something odd\n");
        d.push(&encode(&json!({"seq": 1, "type": "event", "event": "output"})));
        let got = msgs(&mut d);
        assert_eq!(got[0], Frame::Noise("warning: something odd\n".into()));
        assert!(matches!(got[1], Frame::Message(_)));
        // Extra header fields are allowed, header names are case-insensitive.
        let mut d = Decoder::default();
        d.push(b"content-length: 2\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{}");
        assert_eq!(msgs(&mut d), vec![Frame::Message(json!({}))]);
    }

    #[test]
    fn garbage_is_bounded() {
        let mut d = Decoder::default();
        d.push(b"Content-Length: 99999999999\r\n\r\n");
        assert!(matches!(d.next(), Err(DecodeError::TooLarge(_))));
        let mut d = Decoder::default();
        d.push(b"Content-Length: 3\r\n\r\n[1]");
        assert!(matches!(d.next(), Err(DecodeError::BadJson(_))));
        let mut d = Decoder::default();
        d.push(&vec![b'x'; MAX_HEADER + 10]);
        assert!(matches!(d.next(), Ok(Some(Frame::Noise(_)))));
    }

    #[test]
    fn error_text_prefers_the_formatted_error() {
        let r = json!({"command": "evaluate", "success": false, "message": "error",
            "body": {"error": {"id": 1, "format": "No symbol \"{name}\" in {ctx}.", "variables": {"name": "x", "ctx": "current context"}}}});
        assert_eq!(error_text(&r), "No symbol \"x\" in current context.");
        assert_eq!(error_text(&json!({"command": "next", "success": false, "message": "not stopped"})), "not stopped");
        assert_eq!(error_text(&json!({"command": "next", "success": false})), "next failed");
    }
}
