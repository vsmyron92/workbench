//! `workbench statusline`: the status line command installed into hosted sessions, and
//! the command hook for `SessionStart` (Claude Code does not run HTTP hooks for it).
//!
//! Claude Code runs it with JSON on stdin:
//! * a **status line** payload: forwarded to the terminal's `/status` hook; prints a
//!   compact line such as `Opus · 23% ctx · $1.20`;
//! * a **hook** payload (it carries `hook_event_name`): forwarded to the terminal's hook
//!   endpoint; prints nothing, because a SessionStart hook's output becomes context.
//!
//! Forwarding is best effort with short timeouts and never fails the command.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

pub fn cli_statusline() -> anyhow::Result<()> {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(4 * 1024 * 1024).read_to_end(&mut input);
    let v: Value = serde_json::from_slice(&input).unwrap_or(Value::Null);
    if v.get("hook_event_name").is_some() {
        post("", &input);
        return Ok(());
    }
    post("/status", &input);
    println!("{}", super::hooks::status_line_text(&v));
    Ok(())
}

/// POST the JSON to `/api/hooks/claude/<id><suffix>` on the local server. Only loopback
/// URLs get the token.
fn post(suffix: &str, body: &[u8]) {
    let (Ok(url), Ok(id), Ok(token)) =
        (std::env::var("WORKBENCH_URL"), std::env::var("WORKBENCH_TERMINAL_ID"), std::env::var("WORKBENCH_AGENT_TOKEN"))
    else {
        return;
    };
    if body.is_empty() || !super::valid_id(&id) || token.contains(['\r', '\n']) {
        return;
    }
    let Some(hostport) = url.strip_prefix("http://").map(|h| h.trim_end_matches('/')) else { return };
    let host = hostport.rsplit_once(':').map(|(h, _)| h).unwrap_or(hostport);
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return;
    }
    let Some(addr) = hostport.to_socket_addrs().ok().and_then(|mut a| a.next()) else { return };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return };
    let _ = s.set_write_timeout(Some(Duration::from_millis(800)));
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    let head = format!(
        "POST /api/hooks/claude/{id}{suffix} HTTP/1.1\r\nHost: {hostport}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).is_err() {
        return;
    }
    // Wait for the answer so the server has applied the event before Claude continues.
    let mut resp = [0u8; 256];
    let _ = s.read(&mut resp);
}
