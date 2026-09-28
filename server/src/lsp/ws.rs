//! `GET /api/projects/{pid}/lsp/ws`: one socket per browser tab and project.
//!
//! Browser → server (JSON text frames):
//! * `{t:"open", uri, languageId?, text}` → `{t:"opened", uri, server}` and the
//!   document's current `diagnostics` per server (empty lists when they were computed
//!   for another tab's text: opening never replaces another tab's text, see `manager`);
//! * `{t:"change", uri, text}` (full text), `{t:"close", uri}`, `{t:"save", uri}`;
//! * `{t:"req", id, method, params, server?}` → `{t:"res", id, server, result | error}`;
//!   routed by `params.textDocument.uri` (the document's server), or for the
//!   call/type hierarchy follow-ups by `params.item.uri`, else `server`;
//!   `workspace/symbol` without a server asks every ready server. A change, a save or
//!   a request makes this tab the one in use: its texts go to the servers first;
//! * `{t:"cancel", id}` (→ `$/cancelRequest`), `{t:"reply", id, result}` (answer to a
//!   server request), `{t:"ping"}`.
//!
//! Server → browser: `hello` (ready servers and their capabilities), `caps` (a server
//! became ready), `down` (a server stopped), `diagnostics {uri, server, diagnostics,
//! version?}` (to the tabs whose text they were computed for, and those without the
//! file open), `message {server, level, message}`, `request {id, server, method, params}`
//! (`workspace/applyEdit`, `window/showMessageRequest`), `refresh {server, what}`,
//! `disabled` (code intelligence was turned off; the socket closes).
//!
//! In-flight requests are cancelled when the socket closes. The socket closes when
//! its device is signed out (`auth.watch`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

use super::jsonrpc::{self, RpcError};
use super::manager::ProjectLsp;
use super::server::Server;
use crate::app::AppState;
use crate::auth::{self, Caller};
use crate::error::{ApiError, ApiResult};

const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// Largest result sent to a browser.
const MAX_RESULT: usize = 24 * 1024 * 1024;

pub async fn ws(
    State(state): State<AppState>,
    Path(pid): Path<String>,
    caller: Option<Extension<Caller>>,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    let project = state.projects.require(&pid)?;
    if matches!(caller.as_ref().map(|c| &c.0), Some(Caller::Internal { .. })) {
        return Err(ApiError::forbidden("language servers are used from the editor only"));
    }
    if !super::manager::enabled(&state, &project) {
        return Err(ApiError::not_configured("code intelligence is not enabled for this project"));
    }
    let session = state.auth.watch(caller.as_ref().map(|c| &c.0));
    let lsp = state.lsp.project(&project);
    Ok(upgrade.max_message_size(MAX_DOC_FRAME).on_upgrade(move |socket| run(state, lsp, socket, session)))
}

/// A frame carries at most one document (plus JSON quoting).
const MAX_DOC_FRAME: usize = super::manager::MAX_DOC_BYTES * 2 + 64 * 1024;

type Inflight = Arc<Mutex<HashMap<i64, (Vec<(Arc<Server>, i64)>, tokio::task::AbortHandle)>>>;

async fn run(state: AppState, lsp: Arc<ProjectLsp>, socket: WebSocket, mut session: auth::SessionWatch) {
    let sid = state.lsp.next_socket();
    let (tx, mut rx) = mpsc::channel::<String>(4096);
    let kill = Arc::new(Notify::new());
    lsp.add_socket(sid, tx.clone(), kill.clone());
    let inflight: Inflight = Arc::new(Mutex::new(HashMap::new()));
    let (mut sink, mut stream) = socket.split();

    let servers: serde_json::Map<String, Value> =
        lsp.ready_servers().iter().map(|s| (s.spec.id.clone(), json!({ "capabilities": s.capabilities() }))).collect();
    let hello = json!({ "t": "hello", "project": lsp.id, "servers": servers });
    if sink.send(Message::Text(hello.to_string().into())).await.is_err() {
        lsp.remove_socket(&state, sid);
        return;
    }

    loop {
        tokio::select! {
            _ = session.ended(&state.auth) => {
                let _ = sink.send(auth::session_ended_close()).await;
                break;
            }
            _ = kill.notified() => {
                // Disabled, or too slow to keep up: the client reconnects and resyncs.
                while let Ok(m) = rx.try_recv() {
                    if m.contains("\"disabled\"") {
                        let _ = sink.send(Message::Text(m.into())).await;
                    }
                }
                let _ = sink.send(Message::Close(Some(CloseFrame { code: 1013, reason: "resync".into() }))).await;
                break;
            }
            out = rx.recv() => match out {
                Some(text) => {
                    match tokio::time::timeout(SEND_TIMEOUT, sink.send(Message::Text(text.into()))).await {
                        Ok(Ok(())) => {}
                        _ => break,
                    }
                }
                None => break,
            },
            frame = stream.next() => match frame {
                Some(Ok(Message::Text(t))) => {
                    handle(&state, &lsp, sid, &tx, &inflight, t.as_str()).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }

    // Give up on everything this socket asked for.
    let pending: Vec<_> = inflight.lock().drain().collect();
    for (_, (servers, abort)) in pending {
        abort.abort();
        for (s, rid) in servers {
            s.cancel(rid);
        }
    }
    lsp.remove_socket(&state, sid);
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum ClientMsg {
    Open {
        uri: String,
        #[serde(rename = "languageId")]
        language_id: Option<String>,
        text: String,
    },
    Change {
        uri: String,
        text: String,
    },
    Close {
        uri: String,
    },
    Save {
        uri: String,
    },
    Req {
        id: i64,
        method: String,
        #[serde(default)]
        params: Value,
        server: Option<String>,
    },
    Cancel {
        id: i64,
    },
    Reply {
        id: u64,
        #[serde(default)]
        result: Value,
    },
    Ping,
}

async fn send(tx: &mpsc::Sender<String>, msg: Value) {
    let _ = tx.send(msg.to_string()).await;
}

async fn handle(state: &AppState, lsp: &Arc<ProjectLsp>, sid: u64, tx: &mpsc::Sender<String>, inflight: &Inflight, text: &str) {
    let msg: ClientMsg = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            send(tx, json!({ "t": "error", "message": format!("bad message: {e}") })).await;
            return;
        }
    };
    match msg {
        ClientMsg::Open { uri, language_id, text } => match lsp.open(state, sid, &uri, language_id.as_deref(), text, true).await {
            Ok(o) => {
                send(tx, json!({ "t": "opened", "uri": uri, "server": o.server })).await;
                for (server, diagnostics) in o.diagnostics {
                    send(tx, json!({ "t": "diagnostics", "uri": uri, "server": server, "diagnostics": diagnostics })).await;
                }
            }
            Err(e) => send(tx, json!({ "t": "opened", "uri": uri, "server": null, "error": e })).await,
        },
        ClientMsg::Change { uri, text } => {
            if let Err(e) = lsp.change(state, sid, &uri, text) {
                send(tx, json!({ "t": "error", "uri": uri, "message": e })).await;
            }
        }
        ClientMsg::Close { uri } => lsp.close(state, sid, &uri),
        ClientMsg::Save { uri } => lsp.save(state, sid, &uri),
        ClientMsg::Req { id, method, params, server } => {
            lsp.touch_socket(sid);
            // The servers answer from this tab's texts (another tab may have edited).
            lsp.activate(state, sid);
            request(lsp, tx, inflight, id, method, params, server);
        }
        ClientMsg::Cancel { id } => {
            if let Some((servers, abort)) = inflight.lock().remove(&id) {
                abort.abort();
                for (s, rid) in servers {
                    s.cancel(rid);
                }
            }
        }
        ClientMsg::Reply { id, result } => lsp.reply(sid, id, result),
        ClientMsg::Ping => send(tx, json!({ "t": "pong" })).await,
    }
}

/// The document a request is about: `textDocument.uri`, or the `item.uri` of a call or
/// type hierarchy item (incoming/outgoing calls, supertypes, subtypes).
fn document_of(params: &Value) -> Option<String> {
    params.pointer("/textDocument/uri").or_else(|| params.pointer("/item/uri")).and_then(Value::as_str).map(str::to_string)
}

/// How long a browser request may take.
fn timeout_for(method: &str) -> Duration {
    Duration::from_secs(match method {
        "textDocument/hover" | "textDocument/documentHighlight" | "textDocument/signatureHelp" => 10,
        "textDocument/completion" | "completionItem/resolve" => 15,
        "textDocument/formatting" | "textDocument/rangeFormatting" => 20,
        "textDocument/references" | "textDocument/rename" | "textDocument/implementation" | "workspace/symbol" => 60,
        "callHierarchy/incomingCalls" | "callHierarchy/outgoingCalls" | "typeHierarchy/supertypes" | "typeHierarchy/subtypes" => 60,
        "workspace/executeCommand" => 120,
        _ => 30,
    })
}

fn request(lsp: &Arc<ProjectLsp>, tx: &mpsc::Sender<String>, inflight: &Inflight, id: i64, method: String, params: Value, hint: Option<String>) {
    let doc_uri = document_of(&params);
    // Asked of every server: items say which one answered (`_server`).
    let fanout = hint.is_none() && doc_uri.is_none();
    let targets: Vec<Arc<Server>> = match (&hint, &doc_uri) {
        (Some(h), _) => lsp.ready_server(h).into_iter().collect(),
        (None, Some(u)) => lsp.doc_server(u).and_then(|s| lsp.ready_server(&s)).into_iter().collect(),
        (None, None) if method == "workspace/symbol" => lsp
            .ready_servers()
            .into_iter()
            .filter(|s| s.capabilities().get("workspaceSymbolProvider").is_some_and(|v| !v.is_null() && v != &Value::Bool(false)))
            .collect(),
        _ => vec![],
    };
    let tx = tx.clone();
    if targets.is_empty() {
        let msg = json!({ "t": "res", "id": id, "error": RpcError::new(jsonrpc::SERVER_NOT_INITIALIZED, "no language server is ready for this") });
        let _ = tx.try_send(msg.to_string());
        return;
    }
    let mut started: Vec<(Arc<Server>, i64, tokio::sync::oneshot::Receiver<Result<Value, RpcError>>)> = vec![];
    for s in &targets {
        let mut p = params.clone();
        {
            let allow = lsp.allow.lock();
            s.params_to_server(&mut p, &allow);
        }
        s.touch();
        let (rid, rx) = s.start_request(&method, p);
        started.push((s.clone(), rid, rx));
    }
    drop(params);
    let ids: Vec<(Arc<Server>, i64)> = started.iter().map(|(s, r, _)| (s.clone(), *r)).collect();
    let lsp2 = lsp.clone();
    let infl = inflight.clone();
    let timeout = timeout_for(&method);
    // Registered before the task can finish and remove itself.
    let mut guard = inflight.lock();
    let task = tokio::spawn(async move {
        let multi = fanout;
        let mut merged: Vec<Value> = vec![];
        let mut single: Option<(String, Result<Value, RpcError>)> = None;
        for (s, rid, rx) in started {
            let r = match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(r)) => r,
                Ok(Err(_)) => Err(RpcError::new(jsonrpc::SERVER_GONE, format!("{} went away", s.spec.label))),
                Err(_) => {
                    s.cancel(rid);
                    Err(RpcError::new(jsonrpc::TIMED_OUT, format!("{} did not answer within {}s", s.spec.label, timeout.as_secs())))
                }
            };
            let r = r.map(|mut v| {
                let mut allow = lsp2.allow.lock();
                s.result_to_client(&mut v, &mut allow);
                v
            });
            if multi {
                if let Ok(Value::Array(a)) = r {
                    merged.extend(a.into_iter().map(|mut x| {
                        if let Value::Object(o) = &mut x {
                            o.insert("_server".into(), Value::String(s.spec.id.clone()));
                        }
                        x
                    }));
                }
            } else {
                single = Some((s.spec.id.clone(), r));
            }
        }
        let msg = match single {
            Some((server, Ok(result))) => json!({ "t": "res", "id": id, "server": server, "result": result }),
            Some((server, Err(e))) => json!({ "t": "res", "id": id, "server": server, "error": e }),
            None => json!({ "t": "res", "id": id, "result": merged }),
        };
        let text = msg.to_string();
        let text = if text.len() > MAX_RESULT {
            json!({ "t": "res", "id": id, "error": RpcError::new(jsonrpc::INTERNAL_ERROR, "the answer is too large") }).to_string()
        } else {
            text
        };
        infl.lock().remove(&id);
        let _ = tx.send(text).await;
    });
    guard.insert(id, (ids, task.abort_handle()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchy_follow_ups_route_by_their_item() {
        let p = json!({ "item": { "name": "area", "uri": "file:///demo/src/a.ts", "range": {} } });
        assert_eq!(document_of(&p).as_deref(), Some("file:///demo/src/a.ts"));
        let p = json!({ "textDocument": { "uri": "file:///demo/b.rs" }, "position": {} });
        assert_eq!(document_of(&p).as_deref(), Some("file:///demo/b.rs"));
        assert_eq!(document_of(&json!({ "query": "x" })), None);
    }
}
