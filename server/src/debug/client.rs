//! A DAP client over any byte stream (an adapter's stdio, a TCP socket).
//!
//! One writer task owns the output half; one reader task decodes messages and
//! routes them: responses complete the pending request with the same `seq`
//! (`request_seq`), events and reverse requests (`runInTerminal`,
//! `startDebugging`) go to the session's event loop through a channel. Every request
//! has a timeout; when the stream ends, pending requests fail with `Closed`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use super::protocol::{self, Decoder, Frame};
use crate::error::ApiError;

/// What the reader hands the session besides responses.
#[derive(Debug)]
pub enum Incoming {
    Event(Value),
    /// A reverse request from the adapter; answer it with `DapClient::respond`.
    Request(Value),
    /// Non-DAP text on the adapter's stdout, or its stderr.
    Noise(String),
    /// The stream ended (adapter exited or closed the socket); the reason if any.
    Closed(Option<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum DapError {
    Timeout(String),
    Closed,
    /// The adapter answered `success: false`.
    Failed(String),
}

impl std::fmt::Display for DapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DapError::Timeout(c) => write!(f, "the debug adapter did not answer {c} in time"),
            DapError::Closed => write!(f, "the debug adapter has exited"),
            DapError::Failed(m) => write!(f, "{m}"),
        }
    }
}

impl From<DapError> for ApiError {
    fn from(e: DapError) -> Self {
        match e {
            DapError::Timeout(_) => ApiError::new(StatusCode::GATEWAY_TIMEOUT, "timeout", e.to_string()),
            DapError::Closed => ApiError::conflict("the debug session has ended"),
            DapError::Failed(m) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "debugger_error", m),
        }
    }
}

/// Waiting requests: the waiter and an optional tag (see `request_tagged`).
type Pending = Mutex<HashMap<i64, (oneshot::Sender<Value>, Option<String>)>>;

/// The field an event gets with the tag of the request that was executing when it
/// was read: the stream's order (this event came before that answer) survives the
/// split between responses (straight to their waiters) and events (a channel).
/// Adapters handle requests in order, so the one executing is the oldest tagged
/// request still waiting (lowest `seq`); only its tag is given, as a one-element
/// array.
pub const PENDING_TAGS: &str = "_workbenchPending";

/// The tag of the oldest tagged request still waiting.
fn executing_tag(pending: &Pending) -> Option<String> {
    pending.lock().iter().filter_map(|(seq, (_, tag))| tag.as_ref().map(|t| (*seq, t))).min_by_key(|(seq, _)| *seq).map(|(_, t)| t.clone())
}

pub struct DapClient {
    out: mpsc::UnboundedSender<Vec<u8>>,
    seq: AtomicI64,
    pending: Arc<Pending>,
    closed: Arc<AtomicBool>,
}

impl DapClient {
    /// Start the reader and writer tasks. Returns the client and the channel of
    /// events, reverse requests and noise (bounded: a slow session applies
    /// backpressure to the adapter rather than growing memory).
    pub fn start<R, W>(reader: R, writer: W) -> (Arc<Self>, mpsc::Receiver<Incoming>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (in_tx, in_rx) = mpsc::channel::<Incoming>(1024);
        let pending: Arc<Pending> = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(write_loop(writer, out_rx));
        tokio::spawn(read_loop(reader, in_tx, pending.clone(), closed.clone()));
        (Arc::new(Self { out: out_tx, seq: AtomicI64::new(1), pending, closed }), in_rx)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Send a request and wait for its response. `Ok(body)` (`Null` when the response
    /// has none) on success.
    pub async fn request(&self, command: &str, arguments: Value, timeout: Duration) -> Result<Value, DapError> {
        self.request_tagged(command, arguments, timeout, None).await
    }

    /// `request` with a tag: events read while it waits carry the tag in
    /// `PENDING_TAGS` (a `stopped` event during an `evaluate` names the expression).
    pub async fn request_tagged(&self, command: &str, arguments: Value, timeout: Duration, tag: Option<String>) -> Result<Value, DapError> {
        let resp = self.request_raw(command, arguments, timeout, tag).await?;
        if resp.get("success").and_then(Value::as_bool) == Some(true) {
            Ok(resp.get("body").cloned().unwrap_or(Value::Null))
        } else {
            Err(DapError::Failed(protocol::error_text(&resp)))
        }
    }

    /// Like `request`, returning the whole response message whatever its `success`.
    async fn request_raw(&self, command: &str, arguments: Value, timeout: Duration, tag: Option<String>) -> Result<Value, DapError> {
        if self.is_closed() {
            return Err(DapError::Closed);
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(seq, (tx, tag));
        let mut msg = json!({ "seq": seq, "type": "request", "command": command });
        if !arguments.is_null() {
            msg["arguments"] = arguments;
        }
        if self.out.send(protocol::encode(&msg)).is_err() {
            self.pending.lock().remove(&seq);
            return Err(DapError::Closed);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(DapError::Closed),
            Err(_) => {
                self.pending.lock().remove(&seq);
                Err(DapError::Timeout(command.to_string()))
            }
        }
    }

    /// Answer a reverse request.
    pub fn respond(&self, request: &Value, success: bool, body: Value, message: Option<&str>) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({
            "seq": seq,
            "type": "response",
            "request_seq": request.get("seq").cloned().unwrap_or(Value::Null),
            "command": request.get("command").cloned().unwrap_or(Value::Null),
            "success": success,
        });
        if !body.is_null() {
            msg["body"] = body;
        }
        if let Some(m) = message {
            msg["message"] = json!(m);
        }
        let _ = self.out.send(protocol::encode(&msg));
    }

    /// Requests waiting for an answer (for tests and diagnostics).
    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(mut w: W, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(buf) = rx.recv().await {
        if w.write_all(&buf).await.is_err() || w.flush().await.is_err() {
            break;
        }
    }
    // Every sender is gone (the session ended): closing our half tells a stdio
    // adapter to exit (EOF on its stdin).
    let _ = w.shutdown().await;
}

async fn read_loop<R: AsyncRead + Unpin>(mut r: R, tx: mpsc::Sender<Incoming>, pending: Arc<Pending>, closed: Arc<AtomicBool>) {
    let mut dec = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    let reason: Option<String> = loop {
        let n = match r.read(&mut buf).await {
            Ok(0) => break None,
            Ok(n) => n,
            Err(e) => break Some(e.to_string()),
        };
        dec.push(&buf[..n]);
        let mut fatal = None;
        loop {
            match dec.next() {
                Ok(None) => break,
                Ok(Some(Frame::Noise(text))) => {
                    let _ = tx.send(Incoming::Noise(text)).await;
                }
                Ok(Some(Frame::Message(msg))) => match msg.get("type").and_then(Value::as_str) {
                    Some("response") => {
                        let seq = msg.get("request_seq").and_then(Value::as_i64).unwrap_or(-1);
                        if let Some((waiter, _)) = pending.lock().remove(&seq) {
                            let _ = waiter.send(msg);
                        }
                    }
                    Some("event") => {
                        let mut msg = msg;
                        if let Some(tag) = executing_tag(&pending) {
                            msg[PENDING_TAGS] = json!([tag]);
                        }
                        if tx.send(Incoming::Event(msg)).await.is_err() {
                            fatal = Some(None);
                            break;
                        }
                    }
                    Some("request") => {
                        if tx.send(Incoming::Request(msg)).await.is_err() {
                            fatal = Some(None);
                            break;
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    fatal = Some(Some(e.to_string()));
                    break;
                }
            }
        }
        if let Some(reason) = fatal {
            break reason;
        }
    };
    closed.store(true, Ordering::Release);
    // Waiters see `Closed` (their sender is dropped).
    pending.lock().clear();
    let _ = tx.send(Incoming::Closed(reason)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An in-memory "adapter": answers requests by a closure, can push events.
    fn duplex_pair() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
        tokio::io::duplex(64 * 1024)
    }

    #[tokio::test]
    async fn responses_are_correlated_out_of_order_and_events_flow() {
        let (client_end, adapter_end) = duplex_pair();
        let (cr, cw) = tokio::io::split(client_end);
        let (client, mut incoming) = DapClient::start(cr, cw);
        let (mut ar, mut aw) = tokio::io::split(adapter_end);
        // Adapter: read two requests, answer the second first, then emit an event
        // and a reverse request.
        let adapter = tokio::spawn(async move {
            let mut dec = Decoder::default();
            let mut reqs = vec![];
            let mut buf = vec![0u8; 4096];
            while reqs.len() < 2 {
                let n = ar.read(&mut buf).await.unwrap();
                dec.push(&buf[..n]);
                while let Some(Frame::Message(m)) = dec.next().unwrap() {
                    reqs.push(m);
                }
            }
            for r in reqs.iter().rev() {
                let ok = r["command"] != "evaluate";
                let resp = json!({"seq": 100, "type": "response", "request_seq": r["seq"], "command": r["command"], "success": ok,
                    "message": if ok { Value::Null } else { json!("No symbol \"zz\" in current context.") },
                    "body": {"echo": r["arguments"]}});
                aw.write_all(&protocol::encode(&resp)).await.unwrap();
            }
            aw.write_all(&protocol::encode(&json!({"seq": 101, "type": "event", "event": "stopped", "body": {"reason": "step"}}))).await.unwrap();
            aw.write_all(&protocol::encode(&json!({"seq": 102, "type": "request", "command": "runInTerminal", "arguments": {"args": ["x"]}}))).await.unwrap();
            // Read our answer to the reverse request.
            loop {
                let n = ar.read(&mut buf).await.unwrap();
                dec.push(&buf[..n]);
                if let Some(Frame::Message(m)) = dec.next().unwrap() {
                    return m;
                }
            }
        });
        let c2 = client.clone();
        let a = tokio::spawn(async move { c2.request("threads", Value::Null, Duration::from_secs(5)).await });
        let b = client.request("evaluate", json!({"expression": "zz"}), Duration::from_secs(5)).await;
        assert_eq!(b, Err(DapError::Failed("No symbol \"zz\" in current context.".into())));
        let a = a.await.unwrap().unwrap();
        assert_eq!(a, json!({"echo": null}));
        match incoming.recv().await.unwrap() {
            Incoming::Event(e) => assert_eq!(e["event"], "stopped"),
            other => panic!("{other:?}"),
        }
        let req = match incoming.recv().await.unwrap() {
            Incoming::Request(r) => r,
            other => panic!("{other:?}"),
        };
        client.respond(&req, true, json!({"processId": 42}), None);
        let answer = adapter.await.unwrap();
        assert_eq!(answer["type"], "response");
        assert_eq!(answer["request_seq"], 102);
        assert_eq!(answer["command"], "runInTerminal");
        assert_eq!(answer["body"]["processId"], 42);
        assert_eq!(client.pending_count(), 0);
    }

    /// Several watches evaluated at once: a stop is blamed on the evaluation the
    /// adapter was running (the oldest one waiting), not on any of them.
    #[tokio::test]
    async fn an_event_names_the_oldest_waiting_tagged_request() {
        let (client_end, adapter_end) = duplex_pair();
        let (cr, cw) = tokio::io::split(client_end);
        let (client, mut incoming) = DapClient::start(cr, cw);
        let (mut ar, mut aw) = tokio::io::split(adapter_end);
        let n = 6;
        let mut waiters = vec![];
        for i in 0..n {
            let c = client.clone();
            // An untagged request first: it is not an evaluation.
            let (cmd, tag) = if i == 0 { ("threads", None) } else { ("evaluate", Some(format!("w{i}"))) };
            waiters.push(tokio::spawn(async move { c.request_tagged(cmd, json!({ "i": i }), Duration::from_secs(5), tag).await }));
            // Requests go out in this order.
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut dec = Decoder::default();
        let mut reqs = vec![];
        let mut buf = vec![0u8; 8192];
        while reqs.len() < n {
            let k = ar.read(&mut buf).await.unwrap();
            dec.push(&buf[..k]);
            while let Some(Frame::Message(m)) = dec.next().unwrap() {
                reqs.push(m);
            }
        }
        reqs.sort_by_key(|r| r["seq"].as_i64());
        let answer = |r: &Value| protocol::encode(&json!({"seq": 1, "type": "response", "request_seq": r["seq"], "command": r["command"], "success": true, "body": {}}));
        // `threads` and the first evaluation are answered; the second evaluation stops.
        aw.write_all(&answer(&reqs[0])).await.unwrap();
        aw.write_all(&answer(&reqs[1])).await.unwrap();
        aw.write_all(&protocol::encode(&json!({"seq": 2, "type": "event", "event": "stopped", "body": {"reason": "breakpoint"}}))).await.unwrap();
        for r in &reqs[2..] {
            aw.write_all(&answer(r)).await.unwrap();
        }
        // The oldest evaluation still waiting (the order the requests went out in).
        let executing = reqs[2..].iter().find(|r| r["command"] == "evaluate").map(|r| format!("w{}", r["arguments"]["i"])).unwrap();
        match incoming.recv().await.unwrap() {
            Incoming::Event(e) => assert_eq!(e[PENDING_TAGS], json!([executing]), "{e}"),
            other => panic!("{other:?}"),
        }
        for w in waiters {
            w.await.unwrap().unwrap();
        }
        // Nothing waits: events carry no tag.
        aw.write_all(&protocol::encode(&json!({"seq": 3, "type": "event", "event": "continued", "body": {}}))).await.unwrap();
        match incoming.recv().await.unwrap() {
            Incoming::Event(e) => assert!(e.get(PENDING_TAGS).is_none(), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn timeouts_and_close_fail_pending_requests() {
        let (client_end, adapter_end) = duplex_pair();
        let (cr, cw) = tokio::io::split(client_end);
        let (client, mut incoming) = DapClient::start(cr, cw);
        // No answer: the request times out and is forgotten.
        let r = client.request("threads", Value::Null, Duration::from_millis(50)).await;
        assert_eq!(r, Err(DapError::Timeout("threads".into())));
        assert_eq!(client.pending_count(), 0);
        // A request in flight when the adapter goes away fails with Closed.
        let c2 = client.clone();
        let inflight = tokio::spawn(async move { c2.request("next", json!({"threadId": 1}), Duration::from_secs(10)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        drop(adapter_end);
        assert_eq!(inflight.await.unwrap(), Err(DapError::Closed));
        assert!(matches!(incoming.recv().await, Some(Incoming::Closed(_))));
        assert!(client.is_closed());
        assert_eq!(client.request("threads", Value::Null, Duration::from_secs(1)).await, Err(DapError::Closed));
    }
}
