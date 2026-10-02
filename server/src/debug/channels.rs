//! Output channels of a remote target: a TCP port on this computer that streams the program's
//! text out of band — SEGGER RTT's telnet port, OpenOCD's `rtt server`, a UART a simulator
//! serves on a socket, or the SWO pin of a J-Link (decoded from ITM). The stream goes to the
//! debug console as category `target`.
//!
//! A channel connects once its server runs and keeps trying (RTT's port opens when the
//! program starts, and a reset closes it): the session is never held up by one, and a channel
//! that cannot be reached costs a retry every couple of seconds, not a failure.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncReadExt;

use super::itm::Itm;
use super::session::Session;
use crate::app::AppState;

/// What the bytes of a channel are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Text as it is.
    Text,
    /// The SWO stream of a Cortex-M ITM; the bytes of this stimulus port are the text.
    Itm(u8),
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub name: String,
    pub port: u16,
    pub format: Format,
}

/// The longest line held back waiting for its end.
const MAX_LINE: usize = 4096;

/// Lines of a byte stream: carriage returns dropped, invalid UTF-8 replaced, an unfinished line
/// held until it ends (or `partial` takes it).
#[derive(Debug, Default)]
pub struct Lines {
    buf: Vec<u8>,
}

impl Lines {
    /// The complete lines of `bytes` so far (each with its `\n`), and a line that has outgrown
    /// `MAX_LINE`.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = vec![];
        for &b in bytes {
            match b {
                b'\r' => {}
                b'\n' => {
                    self.buf.push(b);
                    out.push(String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned());
                }
                _ => {
                    self.buf.push(b);
                    if self.buf.len() >= MAX_LINE {
                        out.push(String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned());
                    }
                }
            }
        }
        out
    }

    /// The unfinished line, for output that stopped without a newline (a prompt, a `printf`
    /// without `\n`).
    pub fn partial(&mut self) -> Option<String> {
        (!self.buf.is_empty()).then(|| String::from_utf8_lossy(&std::mem::take(&mut self.buf)).into_owned())
    }
}

/// Start the channel's reader for the session; it ends with the session.
pub fn spawn(state: &AppState, s: &Arc<Session>, channel: Channel, many: bool) {
    let (state, s) = (state.clone(), s.clone());
    tokio::spawn(async move { run(state, s, channel, many).await });
}

async fn run(state: AppState, s: Arc<Session>, ch: Channel, many: bool) {
    let mut delay = Duration::from_millis(300);
    let mut was_connected = false;
    loop {
        if s.cancel.is_cancelled() || !s.is_live() {
            return;
        }
        let connect = tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(("127.0.0.1", ch.port)));
        let stream = tokio::select! {
            r = connect => r,
            _ = s.cancel.cancelled() => return,
        };
        match stream {
            Ok(Ok(stream)) => {
                delay = Duration::from_millis(300);
                s.log("workbench", format!("{} connected (port {})\n", ch.name, ch.port), None);
                s.flush(&state);
                was_connected = true;
                read_until_closed(&state, &s, &ch, many, stream).await;
                s.log("workbench", format!("{} closed\n", ch.name), None);
                s.flush(&state);
            }
            _ => {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = s.cancel.cancelled() => return,
                }
                // A server that closed the channel (a reset) usually opens it again soon.
                delay = (delay * 2).min(if was_connected { Duration::from_secs(1) } else { Duration::from_secs(2) });
            }
        }
    }
}

async fn read_until_closed(state: &AppState, s: &Arc<Session>, ch: &Channel, many: bool, mut stream: tokio::net::TcpStream) {
    let mut lines = Lines::default();
    let mut itm = match ch.format {
        Format::Itm(port) => Some(Itm::new(port)),
        Format::Text => None,
    };
    let prefix = if many { format!("[{}] ", ch.name) } else { String::new() };
    let mut at_start = true;
    let emit = |piece: String, at_start: &mut bool| {
        let text = if *at_start { format!("{prefix}{piece}") } else { piece };
        *at_start = text.ends_with('\n');
        s.log("target", text, None);
    };
    let mut buf = [0u8; 4096];
    loop {
        let read = tokio::select! {
            r = tokio::time::timeout(Duration::from_millis(150), stream.read(&mut buf)) => r,
            _ = s.cancel.cancelled() => return,
        };
        match read {
            Ok(Ok(n)) if n > 0 => {
                let pieces = match itm.as_mut() {
                    Some(d) => {
                        let mut text = vec![];
                        d.feed(&buf[..n], &mut text);
                        lines.push(&text)
                    }
                    None => lines.push(&buf[..n]),
                };
                for p in pieces {
                    emit(p, &mut at_start);
                }
            }
            // The unfinished line of output that went quiet.
            Err(_) => {
                if let Some(p) = lines.partial() {
                    emit(p, &mut at_start);
                }
            }
            // Closed by the server, or broken.
            _ => {
                if let Some(p) = lines.partial() {
                    emit(p, &mut at_start);
                }
                s.flush(state);
                return;
            }
        }
        s.flush(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_cut_at_newlines_and_unfinished_ones_wait() {
        let mut l = Lines::default();
        assert_eq!(l.push(b"hel"), Vec::<String>::new());
        assert_eq!(l.push(b"lo\r\nwor"), ["hello\n"]);
        assert_eq!(l.push(b"ld\n\n"), ["world\n", "\n"]);
        assert_eq!(l.partial(), None);
        l.push(b"prompt> ");
        assert_eq!(l.partial().as_deref(), Some("prompt> "));
        assert_eq!(l.partial(), None);
        // Invalid UTF-8 is replaced, never an error.
        assert_eq!(l.push(&[b'a', 0xFF, b'b', b'\n']), ["a\u{FFFD}b\n"]);
    }

    #[test]
    fn a_line_without_end_is_not_held_forever() {
        let mut l = Lines::default();
        let out = l.push(&vec![b'x'; MAX_LINE * 2 + 5]);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.len() == MAX_LINE));
        assert_eq!(l.partial().unwrap().len(), 5);
    }
}
