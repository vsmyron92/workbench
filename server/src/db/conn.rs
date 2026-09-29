//! From a `[[database]]` entry to a live PostgreSQL connection: credentials from
//! secrets (or `~/.pgpass`), TLS by `sslmode`, notices collected on the side.

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use parking_lot::Mutex;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio_postgres::config::{Host, SslMode};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{AsyncMessage, CancelToken, Client, Config, NoTls, Socket};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::app::AppState;
use crate::config::project::DatabaseSource;
use crate::error::ApiError;
use crate::projects::Project;
use crate::secrets::Secret;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Notices kept per session between queries.
const MAX_NOTICES: usize = 200;

/// How TLS is used, as libpq's `sslmode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tls {
    Disable,
    /// TLS when the server offers it, certificate not verified.
    Prefer,
    /// TLS required, certificate not verified (libpq's `require`).
    Require,
    /// TLS required, certificate and host name verified against the system's roots.
    VerifyFull,
}

impl Tls {
    pub fn parse(s: &str) -> Result<Tls, ApiError> {
        match s.trim() {
            "" | "prefer" => Ok(Tls::Prefer),
            "disable" => Ok(Tls::Disable),
            "require" => Ok(Tls::Require),
            "verify-full" | "verify-ca" => Ok(Tls::VerifyFull),
            other => Err(ApiError::bad_request(format!("sslmode {other:?}: use disable, prefer, require or verify-full"))),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Tls::Disable => "disable",
            Tls::Prefer => "prefer",
            Tls::Require => "require",
            Tls::VerifyFull => "verify-full",
        }
    }
}

/// A resolved connection: the driver config, the TLS policy and the secrets in it
/// (to redact from anything shown).
pub struct Resolved {
    pub config: Config,
    pub tls: Tls,
    pub read_only: bool,
    pub secrets: Vec<Secret>,
    /// Changes when anything about the connection does (sessions reconnect then).
    pub fingerprint: String,
    /// `user@host:port/db`, never the password.
    pub display: String,
}

/// `postgres://…?sslmode=verify-full` does not parse in the driver (it knows only
/// disable/prefer/require): take the mode out and apply it ourselves.
fn split_sslmode(url: &str) -> (String, Option<String>) {
    let Some((base, query)) = url.split_once('?') else { return (url.to_string(), None) };
    let mut mode = None;
    let rest: Vec<&str> = query
        .split('&')
        .filter(|kv| match kv.split_once('=') {
            Some(("sslmode", v)) => {
                mode = Some(v.to_string());
                false
            }
            _ => true,
        })
        .collect();
    let url = if rest.is_empty() { base.to_string() } else { format!("{base}?{}", rest.join("&")) };
    (url, mode)
}

fn host_string(c: &Config) -> String {
    match c.get_hosts().first() {
        Some(Host::Tcp(h)) => h.clone(),
        Some(Host::Unix(p)) => p.display().to_string(),
        None => String::new(),
    }
}

/// The OS user, libpq's default for `user` (and `dbname`).
fn os_user() -> String {
    std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_else(|_| "postgres".into())
}

/// Build the connection for `src` of `project`: its URL secret (if any), the fields
/// over it, its password secret, else `~/.pgpass`.
pub fn resolve(state: &AppState, project: &Project, src: &DatabaseSource) -> Result<Resolved, ApiError> {
    let kind = if src.kind.is_empty() { "postgres" } else { src.kind.as_str() };
    if !matches!(kind, "postgres" | "postgresql") {
        return Err(ApiError::bad_request(format!("database kind {kind:?} is not supported (only postgres)")));
    }
    let mut secrets = vec![];
    let mut tls = Tls::parse(&src.sslmode)?;
    let mut config = if src.url.trim().is_empty() {
        Config::new()
    } else {
        let url = state.secret(Some(project), src.url.trim())?;
        secrets.push(url.clone());
        let (url_text, mode) = split_sslmode(url.expose().trim());
        if let (Some(m), true) = (mode, src.sslmode.trim().is_empty()) {
            tls = Tls::parse(&m)?;
        }
        // The driver's message could quote the URL, password and all: say where it is instead.
        Config::from_str(&url_text).map_err(|_| ApiError::bad_request(format!("the connection URL in secret {:?} does not parse", src.url.trim())))?
    };
    if !src.host.trim().is_empty() {
        config.host(src.host.trim());
    }
    if config.get_hosts().is_empty() {
        config.host("localhost");
    }
    if let Some(p) = src.port {
        config.port(p);
    }
    if !src.database.trim().is_empty() {
        config.dbname(src.database.trim());
    }
    if !src.user.trim().is_empty() {
        config.user(src.user.trim());
    }
    if config.get_user().is_none() {
        config.user(os_user());
    }
    if config.get_dbname().is_none() {
        let u = config.get_user().unwrap_or_default().to_string();
        config.dbname(u);
    }
    let host = host_string(&config);
    let port = config.get_ports().first().copied().unwrap_or(5432);
    let user = config.get_user().unwrap_or_default().to_string();
    let db = config.get_dbname().unwrap_or_default().to_string();
    if !src.password.trim().is_empty() {
        let pw = state.secret(Some(project), src.password.trim())?;
        config.password(pw.expose().as_bytes());
        secrets.push(pw);
    } else if config.get_password().is_none() {
        if let Some(file) = crate::util::os::path::pgpass_file() {
            if let Some(pw) = pgpass(&file, &host, port, &db, &user) {
                let s = Secret::from_value(pw.clone());
                config.password(pw.as_bytes());
                secrets.push(s);
            }
        }
    }
    config.application_name("Workbench");
    config.connect_timeout(CONNECT_TIMEOUT);
    config.keepalives(true);
    config.ssl_mode(match tls {
        Tls::Disable => SslMode::Disable,
        Tls::Prefer => SslMode::Prefer,
        Tls::Require | Tls::VerifyFull => SslMode::Require,
    });
    let mut h = Sha256::new();
    for part in [host.as_str(), &port.to_string(), &db, &user, tls.name(), if src.read_only { "ro" } else { "rw" }] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    if let Some(pw) = config.get_password() {
        h.update(pw);
    }
    let display = if host.starts_with('/') { format!("{user}@{host}/{db}") } else { format!("{user}@{host}:{port}/{db}") };
    Ok(Resolved { config, tls, read_only: src.read_only, secrets, fingerprint: hex::encode(h.finalize()), display })
}

/// The password `~/.pgpass` gives (libpq's rules: `host:port:database:user:password`,
/// `*` matches anything, `\:` and `\\` escape; the file must not be readable by
/// others, or libpq ignores it and so do we).
pub fn pgpass(file: &Path, host: &str, port: u16, db: &str, user: &str) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(file).ok()?;
    if meta.permissions().mode() & 0o077 != 0 {
        return None;
    }
    let text = std::fs::read_to_string(file).ok()?;
    let host = if host.starts_with('/') { "localhost" } else { host };
    let port = port.to_string();
    for line in text.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields = split_pgpass(line);
        if fields.len() != 5 {
            continue;
        }
        let m = |f: &str, v: &str| f == "*" || f == v;
        if m(&fields[0], host) && m(&fields[1], &port) && m(&fields[2], db) && m(&fields[3], user) {
            return Some(fields[4].clone());
        }
    }
    None
}

fn split_pgpass(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.last_mut().unwrap().push(n);
                }
            }
            ':' if out.len() < 5 => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out
}

// ---------------------------------------------------------------- TLS

/// libpq's `prefer` / `require`: encrypted, whoever the server says it is.
#[derive(Debug)]
struct NoVerify(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn rustls_connector(verify: bool) -> Result<MakeRustlsConnect, ApiError> {
    use rustls_platform_verifier::BuilderVerifierExt;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| ApiError::internal(format!("TLS setup: {e}")))?;
    let config = if verify {
        builder.with_platform_verifier().map_err(|e| ApiError::internal(format!("TLS setup: {e}")))?.with_no_client_auth()
    } else {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(NoVerify(provider))).with_no_client_auth()
    };
    Ok(MakeRustlsConnect::new(config))
}

/// What a session needs to cancel its running query (the same TLS as the session).
#[derive(Clone)]
pub struct Canceller {
    token: CancelToken,
    tls: Option<MakeRustlsConnect>,
}

impl Canceller {
    pub async fn cancel(&self) -> Result<(), tokio_postgres::Error> {
        match &self.tls {
            Some(t) => self.token.cancel_query(t.clone()).await,
            None => self.token.cancel_query(NoTls).await,
        }
    }
}

pub struct Connected {
    pub client: Client,
    pub canceller: Canceller,
    pub notices: Arc<Mutex<Vec<String>>>,
}

async fn drive<T>(config: &Config, tls: T) -> Result<(Client, Arc<Mutex<Vec<String>>>), tokio_postgres::Error>
where
    T: MakeTlsConnect<Socket> + Send + 'static,
    T::Stream: Send + 'static,
{
    let (client, mut connection) = config.connect(tls).await?;
    let notices = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = notices.clone();
    tokio::spawn(async move {
        let mut messages = futures::stream::poll_fn(move |cx| connection.poll_message(cx));
        while let Some(m) = messages.next().await {
            match m {
                Ok(AsyncMessage::Notice(n)) => {
                    let mut v = sink.lock();
                    if v.len() < MAX_NOTICES {
                        v.push(format!("{}: {}", n.severity(), n.message()));
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });
    Ok((client, notices))
}

/// Connect (10 s timeout), then apply the session settings.
pub async fn connect(r: &Resolved) -> Result<Connected, ApiError> {
    let tls_conn = match r.tls {
        Tls::Disable => None,
        Tls::Prefer | Tls::Require => Some(rustls_connector(false)?),
        Tls::VerifyFull => Some(rustls_connector(true)?),
    };
    let attempt = async {
        match &tls_conn {
            Some(t) => drive(&r.config, t.clone()).await,
            None => drive(&r.config, NoTls).await,
        }
    };
    let (client, notices) = match tokio::time::timeout(CONNECT_TIMEOUT + Duration::from_secs(2), attempt).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => return Err(ApiError::upstream(crate::secrets::redact(&connect_error(&e), &r.secrets))),
        Err(_) => return Err(ApiError::upstream(format!("{} did not answer in {} s", r.display, CONNECT_TIMEOUT.as_secs()))),
    };
    if r.read_only {
        client
            .simple_query("SET default_transaction_read_only = on")
            .await
            .map_err(|e| ApiError::upstream(crate::secrets::redact(&connect_error(&e), &r.secrets)))?;
    }
    let canceller = Canceller { token: client.cancel_token(), tls: tls_conn.clone() };
    Ok(Connected { client, canceller, notices })
}

/// The server's words for a failed connection (authentication, unknown database),
/// else the driver's with its cause.
pub fn connect_error(e: &tokio_postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        return db.message().to_string();
    }
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg = format!("{msg}: {s}");
        src = s.source();
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sslmode_is_taken_out_of_urls() {
        assert_eq!(split_sslmode("postgres://u@h/db?sslmode=verify-full"), ("postgres://u@h/db".into(), Some("verify-full".into())));
        assert_eq!(split_sslmode("postgres://u@h/db?application_name=x&sslmode=require"), ("postgres://u@h/db?application_name=x".into(), Some("require".into())));
        assert_eq!(split_sslmode("postgres://u@h/db"), ("postgres://u@h/db".into(), None));
        assert!(Tls::parse("verify-full").is_ok() && Tls::parse("allow").is_err());
    }

    #[test]
    fn pgpass_follows_libpq() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("pgpass");
        std::fs::write(&f, "# comment\nother:5432:*:*:no\nlocalhost:5432:shop:app:s3cr\\:et\n*:*:*:admin:any\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(pgpass(&f, "localhost", 5432, "shop", "app").as_deref(), Some("s3cr:et"));
        assert_eq!(pgpass(&f, "/var/run/postgresql", 5432, "shop", "app").as_deref(), Some("s3cr:et"), "sockets count as localhost");
        assert_eq!(pgpass(&f, "db.example.com", 6543, "x", "admin").as_deref(), Some("any"));
        assert_eq!(pgpass(&f, "localhost", 5433, "shop", "app"), None);
        // Readable by others: ignored, like libpq.
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(pgpass(&f, "localhost", 5432, "shop", "app"), None);
    }
}
