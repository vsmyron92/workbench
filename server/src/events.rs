//! Server → browser event bus. Every slice emits through `EventBus::emit`; every
//! browser tab holds one WebSocket on `/api/events/ws` and receives all events.
//!
//! Wire format (JSON text frames): `{"type": "...", "projectId": "..."?, "data": {...}, "ts": 1700000000000}`.
//! Event type names are listed in docs/ARCHITECTURE.md#events. Emitting is cheap
//! and never blocks; slow sockets skip events and receive `{"type":"lagged"}`
//! so the UI can refetch.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::auth::{self, Caller, SessionWatch};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub data: Value,
    pub ts: i64,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<Event>>,
    /// Browser tabs currently connected to `/api/events/ws` (pollers idle when 0).
    ui_clients: Arc<AtomicUsize>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(2048);
        Self { tx, ui_clients: Arc::new(AtomicUsize::new(0)) }
    }

    /// Broadcast an event. `data` is any serializable payload.
    pub fn emit(&self, kind: &str, project_id: Option<&str>, data: impl Serialize) {
        let data = serde_json::to_value(data).unwrap_or(Value::Null);
        let ev = Event {
            kind: kind.to_string(),
            project_id: project_id.map(str::to_string),
            data,
            ts: crate::util::now_ms(),
        };
        let _ = self.tx.send(Arc::new(ev));
    }

    /// Ask every connected UI to open a panel (used by agents through MCP, and by
    /// server-side flows). The client ignores it if the kind is unknown.
    pub fn ui_open(&self, panel: &str, params: Value, title: Option<&str>) {
        self.emit("ui.open", None, json!({ "panel": panel, "params": params, "title": title }));
    }

    /// `ui_open` with the panel's stable id (see docs/ARCHITECTURE.md#panels), so
    /// reopening focuses the existing tab.
    #[allow(dead_code)] // contract for slices; files and git still build the event themselves
    pub fn ui_open_id(&self, panel: &str, id: &str, params: Value, title: Option<&str>) {
        self.emit("ui.open", None, json!({ "panel": panel, "id": id, "params": params, "title": title }));
    }

    /// Toast in every connected UI. `level`: info | success | warning | error.
    pub fn notify(&self, level: &str, message: &str) {
        self.emit("ui.notify", None, json!({ "level": level, "message": message }));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        self.tx.subscribe()
    }

    /// Number of UI clients (event WebSockets) connected right now. Background
    /// pollers use it to stay idle while nobody is looking.
    pub fn ui_clients(&self) -> usize {
        self.ui_clients.load(Ordering::Relaxed)
    }
}

/// Counts a connected UI client for as long as it lives.
struct UiClientGuard(Arc<AtomicUsize>);

impl UiClientGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter.clone())
    }
}

impl Drop for UiClientGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn ws_handler(State(state): State<AppState>, caller: Option<Extension<Caller>>, ws: WebSocketUpgrade) -> Response {
    let session = state.auth.watch(caller.as_ref().map(|c| &c.0));
    ws.max_message_size(64 * 1024).on_upgrade(move |socket| run_socket(state, socket, session))
}

async fn run_socket(state: AppState, socket: WebSocket, mut session: SessionWatch) {
    let (mut sink, mut stream) = socket.split();
    let mut rx = state.events.subscribe();
    let _client = UiClientGuard::new(&state.events.ui_clients);
    let hello = json!({ "type": "hello", "data": { "version": env!("CARGO_PKG_VERSION"), "startedAt": state.started_at }, "ts": crate::util::now_ms() });
    if sink.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            // A revoked or signed-out device stops receiving events at once.
            _ = session.ended(&state.auth) => {
                let _ = sink.send(auth::session_ended_close()).await;
                break;
            }
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let Ok(text) = serde_json::to_string(&*ev) else { continue };
                    if sink.send(Message::Text(text.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    let msg = json!({ "type": "lagged", "data": { "skipped": n }, "ts": crate::util::now_ms() });
                    if sink.send(Message::Text(msg.to_string().into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    if t.as_str().contains("\"ping\"") {
                        let pong = json!({ "type": "pong", "data": null, "ts": crate::util::now_ms() });
                        if sink.send(Message::Text(pong.to_string().into())).await.is_err() { break; }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
