//! JSON-RPC 2.0 over stdio with LSP's `Content-Length` framing.

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest message accepted from a server (semantic tokens of a huge file, a big
/// completion list). Anything larger means a broken stream.
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// Header lines are short; a longer one means we are not reading LSP.
const MAX_HEADER_LINE: usize = 8 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("stream closed")]
    Eof,
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad header: {0}")]
    Header(String),
    #[error("message of {0} bytes is over the limit")]
    TooLarge(usize),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Read one framed message. Headers other than `Content-Length` (`Content-Type`) are
/// ignored; a stream that closes between messages is `Eof`.
pub async fn read_message<R: AsyncBufRead + Unpin>(r: &mut R) -> Result<Value, FrameError> {
    let mut length: Option<usize> = None;
    let mut line = Vec::with_capacity(64);
    let mut first = true;
    loop {
        line.clear();
        let n = read_line_bounded(r, &mut line).await?;
        if n == 0 {
            return Err(if first { FrameError::Eof } else { FrameError::Header("stream closed inside the header".into()) });
        }
        first = false;
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end_matches(['\r', '\n']);
        if text.is_empty() {
            if length.is_some() {
                break;
            }
            // Tolerate blank lines between messages.
            first = true;
            continue;
        }
        let Some((name, value)) = text.split_once(':') else {
            return Err(FrameError::Header(format!("{:?}", text.chars().take(80).collect::<String>())));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            let n: usize = value.trim().parse().map_err(|_| FrameError::Header(format!("Content-Length {:?}", value.trim())))?;
            if n > MAX_MESSAGE {
                return Err(FrameError::TooLarge(n));
            }
            length = Some(n);
        }
    }
    let n = length.unwrap_or(0);
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof { FrameError::Header("stream closed inside a message".into()) } else { e.into() }
    })?;
    Ok(serde_json::from_slice(&body)?)
}

async fn read_line_bounded<R: AsyncBufRead + Unpin>(r: &mut R, out: &mut Vec<u8>) -> Result<usize, FrameError> {
    let mut total = 0;
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            return Ok(total);
        }
        let (take, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        out.extend_from_slice(&buf[..take]);
        r.consume(take);
        total += take;
        if done {
            return Ok(total);
        }
        if total > MAX_HEADER_LINE {
            return Err(FrameError::Header("header line too long".into()));
        }
    }
}

/// Frame and write one message.
pub async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, msg: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(msg)?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    w.write_all(header.as_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// What a message is.
#[derive(Debug, PartialEq)]
pub enum Kind<'a> {
    /// A request from the server (`id` + `method`).
    Request { id: &'a Value, method: &'a str },
    Notification { method: &'a str },
    /// A response to one of our requests.
    Response { id: i64 },
    Invalid,
}

pub fn classify(msg: &Value) -> Kind<'_> {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").filter(|v| !v.is_null());
    match (method, id) {
        (Some(method), Some(id)) => Kind::Request { id, method },
        (Some(method), None) => Kind::Notification { method },
        (None, Some(id)) => match id.as_i64() {
            Some(id) => Kind::Response { id },
            // We only send integer ids.
            None => Kind::Invalid,
        },
        (None, None) => Kind::Invalid,
    }
}

pub fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

pub fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A request's failure, as the protocol reports it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INTERNAL_ERROR: i64 = -32603;
pub const SERVER_NOT_INITIALIZED: i64 = -32002;
/// Workbench's own: the request timed out, or the server went away.
pub const TIMED_OUT: i64 = -32099;
pub const SERVER_GONE: i64 = -32098;

/// The result of a response message.
pub fn result_of(msg: Value) -> Result<Value, RpcError> {
    let mut msg = msg;
    if let Some(err) = msg.get("error").filter(|e| !e.is_null()) {
        return Err(RpcError::new(
            err.get("code").and_then(Value::as_i64).unwrap_or(INTERNAL_ERROR),
            err.get("message").and_then(Value::as_str).unwrap_or("error").chars().take(2000).collect::<String>(),
        ));
    }
    Ok(msg.get_mut("result").map(Value::take).unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn frames_round_trip_with_extra_headers_and_split_reads() {
        let mut buf = Vec::new();
        write_message(&mut buf, &json!({"jsonrpc":"2.0","id":1,"result":"héllo"})).await.unwrap();
        write_message(&mut buf, &notification("x", json!([1, 2]))).await.unwrap();
        // Content-Type and lowercase names are fine.
        let body = br#"{"jsonrpc":"2.0","method":"y"}"#;
        buf.extend_from_slice(format!("content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n", body.len()).as_bytes());
        buf.extend_from_slice(body);
        // A reader that hands out 3 bytes at a time.
        let mut r = BufReader::with_capacity(3, &buf[..]);
        let a = read_message(&mut r).await.unwrap();
        assert_eq!(a["result"], "héllo");
        assert_eq!(classify(&a), Kind::Response { id: 1 });
        let b = read_message(&mut r).await.unwrap();
        assert_eq!(classify(&b), Kind::Notification { method: "x" });
        let c = read_message(&mut r).await.unwrap();
        assert_eq!(c["method"], "y");
        assert!(matches!(read_message(&mut r).await, Err(FrameError::Eof)));
    }

    #[tokio::test]
    async fn broken_streams_are_errors() {
        let mut r = BufReader::new(&b"Content-Length: 10\r\n\r\n{}"[..]);
        assert!(matches!(read_message(&mut r).await, Err(FrameError::Header(_))));
        let mut r = BufReader::new(&b"Content-Length: nope\r\n\r\n"[..]);
        assert!(matches!(read_message(&mut r).await, Err(FrameError::Header(_))));
        let big = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE + 1);
        let mut r = BufReader::new(big.as_bytes());
        assert!(matches!(read_message(&mut r).await, Err(FrameError::TooLarge(_))));
        let mut r = BufReader::new(&b"garbage without colon\r\n"[..]);
        assert!(matches!(read_message(&mut r).await, Err(FrameError::Header(_))));
        let mut r = BufReader::new(&b"Content-Length: 2\r\n\r\n{]"[..]);
        assert!(matches!(read_message(&mut r).await, Err(FrameError::Json(_))));
        let long = "X".repeat(20_000);
        let mut r = BufReader::new(long.as_bytes());
        assert!(matches!(read_message(&mut r).await, Err(FrameError::Header(_))));
    }

    #[test]
    fn messages_are_classified_and_results_extracted() {
        let req = json!({"jsonrpc":"2.0","id":"abc","method":"workspace/configuration","params":{}});
        assert!(matches!(classify(&req), Kind::Request { method: "workspace/configuration", .. }));
        assert_eq!(classify(&json!({"id": null, "method": "n"})), Kind::Notification { method: "n" });
        assert_eq!(classify(&json!({"id": "x", "result": 1})), Kind::Invalid);
        assert_eq!(classify(&json!({})), Kind::Invalid);
        assert_eq!(result_of(json!({"id":1,"result":{"a":1}})).unwrap(), json!({"a":1}));
        assert_eq!(result_of(json!({"id":1})).unwrap(), Value::Null);
        let e = result_of(json!({"id":1,"error":{"code":-32800,"message":"cancelled"}})).unwrap_err();
        assert_eq!(e, RpcError::new(-32800, "cancelled"));
    }
}
