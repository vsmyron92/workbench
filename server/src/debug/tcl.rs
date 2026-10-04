//! A client for OpenOCD's Tcl RPC port: the one way to read a running target's memory without
//! stopping it (gdb's debug adapter refuses `evaluate` and `readMemory` while the program runs).
//!
//! The protocol is a command, then a `0x1a` byte; the reply is text, then `0x1a`. There is no
//! status: a failed command answers with its message (`read_memory: failed to read memory`).
//!
//! **Trust.** That port runs *any* OpenOCD command, `exec` included. This client therefore has no
//! way to send text of its own: the only command it can build is `read_memory` from numbers, and
//! the port is OpenOCD's own, which listens on loopback only (checked on 0.12).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TERMINATOR: u8 = 0x1a;
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// The largest reply read: 64 KiB is thousands of words.
const MAX_REPLY: usize = 64 * 1024;

#[derive(Debug)]
pub struct TclClient {
    stream: TcpStream,
}

/// Why a read failed: the connection (reconnect, and say so) or the server's answer (a bad address:
/// that value's problem, the connection is fine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TclError {
    Transport(String),
    Command(String),
}

impl std::fmt::Display for TclError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TclError::Transport(m) | TclError::Command(m) => f.write_str(m),
        }
    }
}

impl TclClient {
    pub async fn connect(port: u16) -> Result<Self, TclError> {
        let stream = tokio::time::timeout(CALL_TIMEOUT, TcpStream::connect(("127.0.0.1", port)))
            .await
            .map_err(|_| TclError::Transport(format!("the debug server's Tcl port {port} did not answer")))?
            .map_err(|e| TclError::Transport(format!("cannot reach the debug server's Tcl port {port}: {e}")))?;
        let _ = stream.set_nodelay(true);
        Ok(TclClient { stream })
    }

    /// One command and its reply. Private on purpose: see the module's trust note.
    async fn call(&mut self, command: &str) -> Result<String, TclError> {
        let io = async {
            self.stream.write_all(command.as_bytes()).await?;
            self.stream.write_all(&[TERMINATOR]).await?;
            let mut reply = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = self.stream.read(&mut chunk).await?;
                if n == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the connection closed"));
                }
                reply.extend_from_slice(&chunk[..n]);
                if reply.last() == Some(&TERMINATOR) {
                    reply.pop();
                    return Ok(reply);
                }
                if reply.len() > MAX_REPLY {
                    return Err(std::io::Error::other("the reply is too long"));
                }
            }
        };
        let reply = tokio::time::timeout(CALL_TIMEOUT, io)
            .await
            .map_err(|_| TclError::Transport("the debug server did not answer in time".to_string()))?
            .map_err(|e| TclError::Transport(format!("the debug server's Tcl port failed: {e}")))?;
        Ok(String::from_utf8_lossy(&reply).trim().to_string())
    }

    /// `count` values of `width` bits (8, 16, 32 or 64) read from `address`.
    pub async fn read_memory(&mut self, address: u64, width: u32, count: usize) -> Result<Vec<u64>, TclError> {
        if !matches!(width, 8 | 16 | 32 | 64) || count == 0 || count > 1024 {
            return Err(TclError::Command("a read of 8, 16, 32 or 64 bits, 1 to 1024 times".into()));
        }
        let reply = self.call(&format!("read_memory {address:#x} {width} {count}")).await?;
        parse_words(&reply, count).map_err(TclError::Command)
    }
}

/// The values of a `read_memory` reply (`0x0000000a 0x00000001`), or the server's message when
/// it is something else.
pub fn parse_words(reply: &str, count: usize) -> Result<Vec<u64>, String> {
    let words: Option<Vec<u64>> = reply.split_whitespace().map(|w| w.strip_prefix("0x").and_then(|h| u64::from_str_radix(h, 16).ok())).collect();
    match words {
        Some(w) if w.len() == count => Ok(w),
        _ => {
            let text: String = reply.chars().filter(|c| !c.is_control() || *c == ' ').take(160).collect();
            Err(if text.is_empty() { "the debug server answered nothing".into() } else { text })
        }
    }
}

/// The bytes of values read `width` bits at a time, little-endian (every Cortex-M and RISC-V part
/// OpenOCD drives is).
pub fn words_to_bytes(words: &[u64], width: u32) -> Vec<u8> {
    let per = (width / 8) as usize;
    words.iter().flat_map(|w| w.to_le_bytes().into_iter().take(per)).collect()
}

/// The widest read (8, 16 or 32 bits: a 64-bit access is not one every part has) that `address`
/// is aligned for and `size` is a multiple of, with how many times to read it.
pub fn plan_read(address: u64, size: u32) -> (u32, usize) {
    for width in [32u32, 16, 8] {
        let bytes = u64::from(width / 8);
        if address % bytes == 0 && u64::from(size) % bytes == 0 {
            return (width, (u64::from(size) / bytes) as usize);
        }
    }
    (8, size as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A stand-in Tcl port: answers each command from `reply`, writing it in two pieces to
    /// show that a reply split across reads is put together; records what it was sent.
    async fn serve(reply: impl Fn(&str) -> String + Send + 'static) -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (mut c, _) = l.accept().await.unwrap();
            let mut pending = Vec::new();
            let mut chunk = [0u8; 256];
            loop {
                let n = c.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                pending.extend_from_slice(&chunk[..n]);
                while let Some(i) = pending.iter().position(|b| *b == TERMINATOR) {
                    let cmd = String::from_utf8_lossy(&pending[..i]).to_string();
                    pending.drain(..=i);
                    let _ = tx.send(cmd.clone());
                    let mut out = reply(&cmd).into_bytes();
                    out.push(TERMINATOR);
                    let half = out.len() / 2;
                    c.write_all(&out[..half]).await.unwrap();
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    c.write_all(&out[half..]).await.unwrap();
                }
            }
        });
        (port, rx)
    }

    #[tokio::test]
    async fn memory_is_read_with_the_only_command_the_client_can_send() {
        let (port, mut sent) = serve(|c| match c {
            "read_memory 0x20000000 32 2" => "0x0000051b 0x00000003".into(),
            "read_memory 0x20000004 8 1" => "0x01".into(),
            "read_memory 0xdeadbee0 32 1" => "read_memory: failed to read memory".into(),
            other => format!("invalid command name \"{other}\""),
        })
        .await;
        let mut c = TclClient::connect(port).await.unwrap();
        assert_eq!(c.read_memory(0x2000_0000, 32, 2).await.unwrap(), [0x51b, 3]);
        assert_eq!(c.read_memory(0x2000_0004, 8, 1).await.unwrap(), [1]);
        assert_eq!(c.read_memory(0xdead_bee0, 32, 1).await.unwrap_err(), TclError::Command("read_memory: failed to read memory".into()));
        // Nothing but numbers reaches the wire, and a bad request never does.
        assert!(c.read_memory(0, 24, 1).await.is_err() && c.read_memory(0, 32, 0).await.is_err() && c.read_memory(0, 32, 5000).await.is_err());
        let wire: Vec<String> = std::iter::from_fn(|| sent.try_recv().ok()).collect();
        assert_eq!(wire, ["read_memory 0x20000000 32 2", "read_memory 0x20000004 8 1", "read_memory 0xdeadbee0 32 1"]);
    }

    #[tokio::test]
    async fn a_port_nobody_serves_or_a_server_that_hangs_is_an_error_not_a_hang() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(matches!(TclClient::connect(port).await.unwrap_err(), TclError::Transport(m) if m.contains("cannot reach")));
        // Connected, but it never answers: the call gives up.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (_c, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let mut c = TclClient::connect(port).await.unwrap();
        let started = std::time::Instant::now();
        assert!(matches!(c.read_memory(0x2000_0000, 32, 1).await.unwrap_err(), TclError::Transport(m) if m.contains("did not answer")));
        assert!(started.elapsed() < Duration::from_secs(6));
    }

    #[test]
    fn replies_are_values_or_the_servers_words() {
        assert_eq!(parse_words("0x0000000a 0xffffffff", 2).unwrap(), [10, 0xffff_ffff]);
        assert_eq!(parse_words("0x1", 2).unwrap_err(), "0x1", "one value where two were asked for");
        assert_eq!(parse_words("invalid command name \"x\"", 1).unwrap_err(), "invalid command name \"x\"");
        assert_eq!(parse_words("", 1).unwrap_err(), "the debug server answered nothing");
        assert!(parse_words(&format!("bad\u{1b}[31m {}", "x".repeat(500)), 1).unwrap_err().len() <= 160);
    }

    #[test]
    fn values_become_little_endian_bytes_and_reads_follow_the_alignment() {
        assert_eq!(words_to_bytes(&[0x0102_0304, 0xa], 32), [4, 3, 2, 1, 10, 0, 0, 0]);
        assert_eq!(words_to_bytes(&[0x0102, 0x03], 16), [2, 1, 3, 0]);
        assert_eq!(plan_read(0x2000_0000, 4), (32, 1));
        assert_eq!(plan_read(0x2000_0000, 8), (32, 2));
        assert_eq!(plan_read(0x2000_0002, 4), (16, 2), "misaligned for a word: halfwords");
        assert_eq!(plan_read(0x2000_0001, 4), (8, 4));
        assert_eq!(plan_read(0x2000_0000, 1), (8, 1));
        assert_eq!(plan_read(0x2000_0000, 6), (16, 3));
    }
}
