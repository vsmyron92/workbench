//! Direct HTTPS serving when `[server.tls]` names a certificate and key.
//!
//! One port serves both: a connection whose first byte is a TLS handshake
//! record gets TLS; plain HTTP is still answered for loopback peers, because
//! local helpers (hooks, the MCP endpoint in hosted sessions, the CLI) use
//! `http://127.0.0.1:<port>`. Plain HTTP from other hosts gets a short
//! "use https" response and is closed.
//!
//! Handshakes run on their own tasks so a slow client cannot stall the accept
//! loop. Connections from other hosts that are still being classified or are
//! mid-handshake are bounded overall and per peer, and must send their first
//! byte quickly; loopback peers are never subject to those limits, so idle
//! sockets from the network cannot lock out local hooks and MCP calls.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use parking_lot::Mutex;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use crate::config::expand_tilde;
use crate::config::global::TlsConfig;

/// Time for the TLS handshake itself (and, for loopback peers, for the first byte).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A peer from the network must send its first byte within this time.
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(3);
/// Unclassified or mid-handshake connections from the network, overall…
const MAX_PENDING_HANDSHAKES: usize = 256;
/// …and per peer (one IPv4 address, or one IPv6 /64).
const MAX_PENDING_PER_PEER: usize = 16;

/// Whether `ip` is this machine. A dual-stack `[::]` listener reports IPv4
/// clients as IPv4-mapped IPv6 (`::ffff:127.0.0.1`), which
/// `Ipv6Addr::is_loopback` does not recognise.
pub fn is_loopback_peer(ip: IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

/// Load the PEM certificate chain and private key into a TLS acceptor.
pub fn acceptor(cfg: &TlsConfig) -> anyhow::Result<TlsAcceptor> {
    let cert_path = expand_tilde(&cfg.cert);
    let key_path = expand_tilde(&cfg.key);
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cert_path)
        .with_context(|| format!("read TLS certificate {}", cert_path.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("parse TLS certificate {}", cert_path.display()))?;
    anyhow::ensure!(!certs.is_empty(), "no certificate found in {}", cert_path.display());
    let key = PrivateKeyDer::from_pem_file(&key_path).with_context(|| format!("read TLS key {}", key_path.display()))?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS protocol versions")?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("TLS certificate and key do not match")?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// A connection: TLS, or plain TCP from a loopback peer.
pub enum MaybeTls {
    Tls(Box<TlsStream<TcpStream>>),
    Plain(TcpStream),
}

impl AsyncRead for MaybeTls {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            MaybeTls::Plain(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTls {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            MaybeTls::Plain(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
            MaybeTls::Plain(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            MaybeTls::Plain(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTls::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
            MaybeTls::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            MaybeTls::Tls(s) => s.is_write_vectored(),
            MaybeTls::Plain(s) => s.is_write_vectored(),
        }
    }
}

/// An `axum::serve` listener that terminates TLS (see the module docs).
pub struct TlsListener {
    rx: mpsc::Receiver<(MaybeTls, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(listener: TcpListener, acceptor: TlsAcceptor) -> io::Result<Self> {
        let local = listener.local_addr()?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(accept_loop(listener, acceptor, tx));
        Ok(Self { rx, local })
    }
}

/// Budget for pending connections from the network (see the module docs).
#[derive(Default)]
struct Pending {
    total: usize,
    per_peer: HashMap<IpAddr, usize>,
}

/// One held pending slot; released on drop.
struct PendingSlot {
    pending: Arc<Mutex<Pending>>,
    peer: IpAddr,
}

/// The unit a peer is limited by: its IPv4 address, or its IPv6 /64 (one host
/// usually owns a whole /64 and can pick any address in it).
fn peer_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !((1u128 << 64) - 1))),
        v4 => v4,
    }
}

impl Pending {
    fn try_acquire(this: &Arc<Mutex<Pending>>, ip: IpAddr) -> Option<PendingSlot> {
        let peer = peer_key(ip);
        let mut p = this.lock();
        let mine = p.per_peer.get(&peer).copied().unwrap_or(0);
        if p.total >= MAX_PENDING_HANDSHAKES || mine >= MAX_PENDING_PER_PEER {
            return None;
        }
        p.total += 1;
        p.per_peer.insert(peer, mine + 1);
        Some(PendingSlot { pending: this.clone(), peer })
    }
}

impl Drop for PendingSlot {
    fn drop(&mut self) {
        let mut p = self.pending.lock();
        p.total = p.total.saturating_sub(1);
        if let Some(n) = p.per_peer.get_mut(&self.peer) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                p.per_peer.remove(&self.peer);
            }
        }
    }
}

async fn accept_loop(listener: TcpListener, acceptor: TlsAcceptor, tx: mpsc::Sender<(MaybeTls, SocketAddr)>) {
    let pending = Arc::new(Mutex::new(Pending::default()));
    let mut last_drop_warning: Option<Instant> = None;
    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                // Out of file descriptors and similar: back off instead of spinning.
                tracing::warn!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        if tx.is_closed() {
            break;
        }
        let local = is_loopback_peer(peer.ip());
        let slot = if local {
            None
        } else {
            match Pending::try_acquire(&pending, peer.ip()) {
                Some(slot) => Some(slot),
                None => {
                    // One line per 10 s at most: a flood must not flood the log too.
                    if last_drop_warning.is_none_or(|t| t.elapsed() >= Duration::from_secs(10)) {
                        last_drop_warning = Some(Instant::now());
                        tracing::warn!("too many pending TLS handshakes; dropping connections (latest from {peer})");
                    }
                    continue;
                }
            }
        };
        let (acceptor, tx) = (acceptor.clone(), tx.clone());
        tokio::spawn(async move {
            let _slot = slot;
            let _ = tcp.set_nodelay(true);
            if let Some(conn) = classify(tcp, peer, local, &acceptor).await {
                let _ = tx.send((conn, peer)).await;
            }
        });
    }
}

/// Peek at the first byte: 0x16 starts a TLS handshake record.
async fn classify(mut tcp: TcpStream, peer: SocketAddr, local: bool, acceptor: &TlsAcceptor) -> Option<MaybeTls> {
    let mut first = [0u8; 1];
    let first_byte_timeout = if local { HANDSHAKE_TIMEOUT } else { FIRST_BYTE_TIMEOUT };
    let n = match tokio::time::timeout(first_byte_timeout, tcp.peek(&mut first)).await {
        Ok(r) => r.ok()?,
        Err(_) => {
            tracing::debug!("{peer} sent nothing; closing");
            return None;
        }
    };
    if n == 0 {
        return None;
    }
    if first[0] == 0x16 {
        return match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
            Ok(Ok(tls)) => Some(MaybeTls::Tls(Box::new(tls))),
            Ok(Err(e)) => {
                tracing::debug!("TLS handshake with {peer} failed: {e}");
                None
            }
            Err(_) => {
                tracing::debug!("TLS handshake with {peer} timed out");
                None
            }
        };
    }
    if local {
        return Some(MaybeTls::Plain(tcp));
    }
    let body = "This Workbench serves HTTPS. Use https:// in the address.\n";
    let resp = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::time::timeout(FIRST_BYTE_TIMEOUT, async {
        let _ = tcp.write_all(resp.as_bytes()).await;
        let _ = tcp.shutdown().await;
    })
    .await;
    None
}

impl axum::serve::Listener for TlsListener {
    type Io = MaybeTls;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(conn) => conn,
            // The accept loop only ends when this listener is dropped.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_are_reported() {
        let Err(err) = acceptor(&TlsConfig { cert: "/nonexistent/cert.pem".into(), key: "/nonexistent/key.pem".into() }) else {
            panic!("missing files must fail");
        };
        assert!(format!("{err:#}").contains("/nonexistent/cert.pem"), "{err:#}");
    }

    #[test]
    fn ipv4_mapped_loopback_is_loopback() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(is_loopback_peer(ip("127.0.0.1")));
        assert!(is_loopback_peer(ip("127.0.0.2")));
        assert!(is_loopback_peer(ip("::1")));
        assert!(is_loopback_peer(ip("::ffff:127.0.0.1")));
        assert!(!is_loopback_peer(ip("::ffff:192.168.1.20")));
        assert!(!is_loopback_peer(ip("100.64.1.2")));
        assert!(!is_loopback_peer(ip("fd7a:115c:a1e0::1")));
    }

    #[test]
    fn pending_budget_is_per_peer_and_released_on_drop() {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let mut held: Vec<PendingSlot> = (0..MAX_PENDING_PER_PEER)
            .map(|_| Pending::try_acquire(&pending, ip("192.168.1.20")).expect("slot"))
            .collect();
        // The same peer is out of slots, also when it shows up IPv4-mapped…
        assert!(Pending::try_acquire(&pending, ip("192.168.1.20")).is_none());
        assert!(Pending::try_acquire(&pending, ip("::ffff:192.168.1.20")).is_none());
        // …but another peer is not.
        assert!(Pending::try_acquire(&pending, ip("192.168.1.21")).is_some());
        // Addresses in one IPv6 /64 share a budget.
        let v6: Vec<PendingSlot> =
            (0..MAX_PENDING_PER_PEER).map(|i| Pending::try_acquire(&pending, ip(&format!("2001:db8:1:2::{:x}", i + 1))).unwrap()).collect();
        assert!(Pending::try_acquire(&pending, ip("2001:db8:1:2::ffff")).is_none());
        assert!(Pending::try_acquire(&pending, ip("2001:db8:1:3::1")).is_some());
        // Dropping a slot frees it.
        held.pop();
        assert!(Pending::try_acquire(&pending, ip("192.168.1.20")).is_some());
        drop(held);
        drop(v6);
        assert_eq!(pending.lock().total, 0);
        assert!(pending.lock().per_peer.is_empty());
    }

    #[test]
    fn pending_budget_has_a_global_cap() {
        let pending = Arc::new(Mutex::new(Pending::default()));
        let held: Vec<PendingSlot> = (0..MAX_PENDING_HANDSHAKES)
            .map(|i| Pending::try_acquire(&pending, IpAddr::from([10, 0, (i / 250) as u8, (i % 250) as u8 + 1])).unwrap())
            .collect();
        assert!(Pending::try_acquire(&pending, IpAddr::from([10, 9, 9, 9])).is_none());
        drop(held);
        assert!(Pending::try_acquire(&pending, IpAddr::from([10, 9, 9, 9])).is_some());
    }

    /// A self-signed certificate made with the openssl CLI (`None` when it is absent).
    fn test_acceptor(dir: &std::path::Path) -> Option<TlsAcceptor> {
        let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
        let ok = std::process::Command::new("openssl")
            .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=localhost"])
            .arg("-keyout")
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("openssl not available; skipping");
            return None;
        }
        Some(acceptor(&TlsConfig { cert: cert.display().to_string(), key: key.display().to_string() }).unwrap())
    }

    /// Serve "hello" on `tcp` through a TLS listener.
    fn serve_hello(tcp: TcpListener, acc: TlsAcceptor) -> SocketAddr {
        let listener = TlsListener::new(tcp, acc).unwrap();
        let addr = axum::serve::Listener::local_addr(&listener).unwrap();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "hello" }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    #[tokio::test]
    async fn serves_tls_and_loopback_plain_http() {
        let dir = tempfile::tempdir().unwrap();
        let Some(acc) = test_acceptor(dir.path()) else { return };
        let addr = serve_hello(TcpListener::bind("127.0.0.1:0").await.unwrap(), acc);

        let client = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
        let body = client.get(format!("https://{addr}/")).send().await.unwrap().text().await.unwrap();
        assert_eq!(body, "hello");
        let body = reqwest::get(format!("http://{addr}/")).await.unwrap().text().await.unwrap();
        assert_eq!(body, "hello");
    }

    /// A dual-stack `[::]` listener sees IPv4 clients as `::ffff:127.0.0.1`: local
    /// helpers on `http://127.0.0.1:<port>` must still get plain HTTP.
    #[tokio::test]
    async fn dual_stack_listener_serves_ipv4_loopback_plain_http() {
        let dir = tempfile::tempdir().unwrap();
        let Some(acc) = test_acceptor(dir.path()) else { return };
        let Ok(tcp) = crate::util::os::net::bind("[::]:0".parse().unwrap()).await else {
            eprintln!("no IPv6; skipping");
            return;
        };
        let port = serve_hello(tcp, acc).port();
        // With net.ipv6.bindv6only=1 the listener is IPv6-only and IPv4 cannot connect at all.
        if TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            eprintln!("[::] is IPv6-only here; skipping");
            return;
        }
        let body = reqwest::get(format!("http://127.0.0.1:{port}/")).await.unwrap().text().await.unwrap();
        assert_eq!(body, "hello");
        let body = reqwest::get(format!("http://[::1]:{port}/")).await.unwrap().text().await.unwrap();
        assert_eq!(body, "hello");
    }

    /// Hundreds of idle, never-classified connections must not cut off local
    /// helpers (hooks, MCP) on loopback.
    #[tokio::test]
    async fn idle_connections_do_not_lock_out_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let Some(acc) = test_acceptor(dir.path()) else { return };
        let addr = serve_hello(TcpListener::bind("127.0.0.1:0").await.unwrap(), acc);
        let mut idle = vec![];
        for _ in 0..MAX_PENDING_HANDSHAKES + 40 {
            let sock = tokio::net::TcpSocket::new_v4().unwrap();
            // Another loopback address, like the review's reproduction.
            sock.bind("127.0.0.2:0".parse().unwrap()).unwrap();
            match sock.connect(addr).await {
                Ok(s) => idle.push(s),
                Err(e) => {
                    eprintln!("cannot open more idle connections ({e}); skipping");
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
        let body = client.get(format!("http://{addr}/")).send().await.unwrap().text().await.unwrap();
        assert_eq!(body, "hello");
        drop(idle);
    }
}
