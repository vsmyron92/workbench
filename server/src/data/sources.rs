//! Followed files: a capture tool writes a file (sigrok-cli, a script's `tee`, a vendor CLI's
//! export), Workbench reads what is appended and puts it into series while the capture runs.
//! A file that is already complete is imported the same way and the source ends.
//!
//! * Only files inside the project are read, by the rules of every agent file read: no hidden
//!   or key-like files, nothing the project marks sensitive, symbolic links resolved first.
//! * A file that shrinks or is replaced starts the capture over: the series the source wrote
//!   are cleared, since time begins again.
//! * Rows whose time goes backwards are skipped and counted, never fatal.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;

use super::formats::{self, Decoded, Format, Options};
use super::series::{Kind, MAX_GROUP, MAX_NAME};
use super::store::{AppendReq, Body, ClearTarget, DefineOpts};
use crate::app::AppState;
use crate::error::ApiError;

/// Sources running at once in a project, and kept in its list.
pub const MAX_RUNNING: usize = 8;
const MAX_LISTED: usize = 32;
const POLL: Duration = Duration::from_millis(150);
/// How long a file that does not exist yet is waited for.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK: usize = 8 * 1024 * 1024;
const MAX_LINE: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// `waiting` (the file is not there yet), `running`, `done` (imported to its end), `stopped`, `error`.
    pub state: &'static str,
    pub rows: u64,
    pub samples: u64,
    /// Rows left out because their time went backwards.
    pub skipped: u64,
    pub bytes: u64,
    pub error: Option<String>,
    pub series: Vec<String>,
    pub started_at: i64,
    pub updated_at: i64,
}

pub struct Source {
    pub id: String,
    pub path: String,
    /// Where the file is (canonical once it exists): two names for one file are one source.
    key: PathBuf,
    pub format: Format,
    pub follow: bool,
    /// The list heading its series get (default: the file's name).
    group: Option<String>,
    status: Mutex<Status>,
    cancel: CancellationToken,
}

impl Source {
    fn view(&self) -> Value {
        let s = self.status.lock().clone();
        json!({
            "id": self.id,
            "path": self.path,
            "format": match self.format { Format::Csv => "csv", Format::Jsonl => "jsonl", Format::Vcd => "vcd" },
            "follow": self.follow,
            "state": s.state,
            "rows": s.rows,
            "samples": s.samples,
            "skipped": s.skipped,
            "bytes": s.bytes,
            "error": s.error,
            "series": s.series,
            "startedAt": s.started_at,
            "updatedAt": s.updated_at,
        })
    }

    fn running(&self) -> bool {
        matches!(self.status.lock().state, "waiting" | "running")
    }
}

#[derive(Default)]
pub struct Sources {
    by_project: Mutex<HashMap<String, Vec<Arc<Source>>>>,
    next: AtomicU64,
}

impl Sources {
    pub fn list(&self, pid: &str) -> Vec<Value> {
        self.by_project.lock().get(pid).map(|l| l.iter().map(|s| s.view()).collect()).unwrap_or_default()
    }

    /// Stop every source (the server is going down).
    pub fn shutdown(&self) {
        for l in self.by_project.lock().values() {
            for s in l {
                s.cancel.cancel();
            }
        }
    }

    pub fn stop(&self, pid: &str, id: &str) -> bool {
        let list = self.by_project.lock();
        match list.get(pid).and_then(|l| l.iter().find(|s| s.id == id)) {
            Some(s) => {
                s.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// Stop a source and drop it from the list.
    pub fn remove(&self, pid: &str, id: &str) -> bool {
        let mut all = self.by_project.lock();
        let Some(l) = all.get_mut(pid) else { return false };
        let before = l.len();
        l.retain(|s| {
            if s.id == id {
                s.cancel.cancel();
                false
            } else {
                true
            }
        });
        l.len() != before
    }
}

/// What starts a source.
#[derive(Debug, Clone)]
pub struct StartReq {
    pub path: String,
    pub format: Option<String>,
    pub follow: bool,
    pub options: Options,
}

impl StartReq {
    pub fn from_json(a: &Value) -> Result<StartReq, ApiError> {
        let path = a.get("path").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| ApiError::bad_request("path: the file to follow, relative to the project"))?;
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
        let list = |k: &str| -> Vec<String> {
            match a.get(k) {
                Some(Value::Array(l)) => l.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect(),
                Some(Value::String(t)) => t.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
                _ => vec![],
            }
        };
        let time = match a.get("timeColumn").or_else(|| a.get("time")) {
            Some(Value::String(t)) => Some(t.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        let kind = match s("kind") {
            Some(k) => Some(Kind::parse(&k).ok_or_else(|| ApiError::bad_request("kind: analog, digital or events"))?),
            None => None,
        };
        let rate = a.get("rate").and_then(Value::as_f64);
        if rate.is_some_and(|r| !(r.is_finite() && r > 0.0)) {
            return Err(ApiError::bad_request("rate: samples per second, above zero"));
        }
        Ok(StartReq {
            path: path.to_string(),
            format: s("format"),
            follow: a.get("follow").and_then(Value::as_bool).unwrap_or(true),
            options: Options {
                time,
                time_unit: s("timeUnit"),
                rate,
                prefix: s("prefix").unwrap_or_default(),
                group: s("group").unwrap_or_default(),
                kind,
                digital: list("digital"),
                unit: s("unit"),
                columns: list("columns"),
                delimiter: s("delimiter").and_then(|d| if d == "\\t" || d == "tab" { Some('\t') } else { d.chars().next() }),
                header: a.get("header").and_then(Value::as_bool),
            },
        })
    }
}

/// Hidden files and folders, key-like names and anything the project marks sensitive are not
/// read (they may hold credentials), whatever links lead to them.
fn check_readable(rel: &str, sensitive: &crate::files::Sensitive) -> Result<(), ApiError> {
    let refuse = |why: &str| Err(ApiError::forbidden(format!("{rel} is not read: {why}")));
    if rel.split('/').any(|c| c.starts_with('.')) {
        return refuse("hidden files and folders may hold credentials; keep captures in a normal folder of the project (captures/)");
    }
    let name = rel.rsplit('/').next().unwrap_or(rel).to_ascii_lowercase();
    const KEY_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".token", "_token", "_api_key", ".api_key", "_api_sk", ".api_sk", ".keystore", ".jks"];
    if name.starts_with("id_rsa") || name.starts_with("id_ecdsa") || name.starts_with("id_ed25519") || KEY_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return refuse("it looks like a key or token file");
    }
    if sensitive.matches(rel) {
        return refuse("the project marks it sensitive");
    }
    Ok(())
}

/// The file to read, canonical, and its project-relative name.
fn resolve(root: &std::path::Path, raw: &str, sensitive: &crate::files::Sensitive) -> Result<(PathBuf, String), ApiError> {
    use crate::util::paths::{relative_to, resolve_absolute_in, resolve_in_root};
    let abs = if std::path::Path::new(raw).is_absolute() { resolve_absolute_in(std::slice::from_ref(&root.to_path_buf()), raw)? } else { resolve_in_root(root, raw)? };
    let rel = relative_to(root, &abs).unwrap_or_else(|| raw.to_string());
    check_readable(&rel, sensitive)?;
    // The file may not exist yet (the capture has not started): then only the name is checked, and
    // checked again every time it is looked at. Any other failure to resolve it is a refusal.
    let canon = match crate::util::os::path::canonicalize(&abs) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((abs, rel)),
        Err(e) => return Err(ApiError::forbidden(format!("{rel} cannot be resolved ({e}), so it is not read"))),
    };
    let canon_root = crate::util::os::path::canonicalize(root).map_err(|_| ApiError::not_found("the project folder is missing"))?;
    let real = relative_to(&canon_root, &canon).filter(|r| !r.is_empty()).ok_or_else(|| ApiError::forbidden(format!("{rel} resolves outside the project")))?;
    if real != rel {
        check_readable(&real, sensitive).map_err(|_| ApiError::forbidden(format!("{rel} links to {real}, which is not read")))?;
    }
    Ok((canon, rel))
}

pub fn start(state: &AppState, pid: &str, req: StartReq) -> Result<Value, ApiError> {
    let project = state.projects.require(pid)?;
    let sensitive = crate::files::Sensitive::new(&project.config.project.sensitive);
    let (abs, rel) = resolve(&project.root, &req.path, &sensitive)?;
    let format = Format::parse(req.format.as_deref(), &rel).ok_or_else(|| ApiError::bad_request(format!("{rel}: the format is csv, jsonl or vcd; pass `format` (the extension does not say)")))?;
    if let Ok(m) = std::fs::metadata(&abs) {
        if !m.is_file() {
            return Err(ApiError::bad_request(format!("{rel} is not a file")));
        }
    }
    let sources = &state.data.sources;
    let src = Arc::new(Source {
        id: format!("src-{}", sources.next.fetch_add(1, Ordering::Relaxed) + 1),
        path: rel,
        key: abs.clone(),
        format,
        follow: req.follow,
        group: Some(req.options.group.trim().to_string()).filter(|g| !g.is_empty()),
        status: Mutex::new(Status { state: "waiting", rows: 0, samples: 0, skipped: 0, bytes: 0, error: None, series: vec![], started_at: crate::util::now_ms(), updated_at: crate::util::now_ms() }),
        cancel: CancellationToken::new(),
    });
    {
        let mut all = sources.by_project.lock();
        let list = all.entry(pid.to_string()).or_default();
        // The same file followed twice would write every row twice.
        if let Some(s) = list.iter().find(|s| s.running() && (s.path == src.path || s.key == src.key)) {
            return Err(ApiError::conflict(format!("{} is already followed ({}); stop that source first", src.path, s.id)));
        }
        if list.iter().filter(|s| s.running()).count() >= MAX_RUNNING {
            return Err(ApiError::bad_request(format!("{MAX_RUNNING} files are followed already; stop one first")));
        }
        list.push(src.clone());
        while list.len() > MAX_LISTED {
            if let Some(i) = list.iter().position(|s| !s.running()) {
                list.remove(i);
            } else {
                break;
            }
        }
    }
    let options = req.options;
    let (st, pid2, src2) = (state.clone(), pid.to_string(), src.clone());
    let guard = Guard { root: project.root.clone(), sensitive: project.config.project.sensitive.clone(), raw: src.path.clone() };
    tokio::spawn(async move {
        run(st.clone(), pid2.clone(), src2.clone(), abs, guard, options).await;
        announce(&st, &pid2);
    });
    announce(state, pid);
    Ok(src.view())
}

fn announce(state: &AppState, pid: &str) {
    state.events.emit("data.sources", Some(pid), json!({ "projectId": pid, "sources": state.data.sources.list(pid) }));
}

fn set_state(src: &Source, state: &'static str, error: Option<String>) {
    let mut s = src.status.lock();
    s.state = state;
    s.error = error;
    s.updated_at = crate::util::now_ms();
}

/// What a source needs to check its file again before reading it.
struct Guard {
    root: PathBuf,
    sensitive: Vec<String>,
    /// The name it was started with (project-relative).
    raw: String,
}

impl Guard {
    /// The file to read now. A file that did not exist when the source started may have become a
    /// link since (to a key, to something outside the project): it is resolved and checked again
    /// each time before it is read, and the canonical path is what is opened.
    fn resolve(&self) -> Result<PathBuf, ApiError> {
        let sensitive = crate::files::Sensitive::new(&self.sensitive);
        resolve(&self.root, &self.raw, &sensitive).map(|(p, _)| p)
    }
}

/// Open the (already canonical) file without following a link at its end: one put there since it was
/// checked is refused, not read.
async fn open_plain(path: &std::path::Path) -> std::io::Result<tokio::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    tokio::fs::OpenOptions::from(o).open(path).await
}

/// How many leading bytes tell one file from another that replaced it.
const HEAD: usize = 64;

async fn read_head(path: &std::path::Path) -> Vec<u8> {
    let Ok(mut f) = open_plain(path).await else { return vec![] };
    let mut buf = vec![0u8; HEAD];
    let mut got = 0;
    while got < HEAD {
        match f.read(&mut buf[got..]).await {
            Ok(0) | Err(_) => break,
            Ok(n) => got += n,
        }
    }
    buf.truncate(got);
    buf
}

/// Decode `text`'s lines off the async workers (a chunk can be megabytes of text).
async fn decode(mut decoder: Box<dyn formats::Decoder>, text: String, last_without_newline: bool) -> Option<(Box<dyn formats::Decoder>, Decoded)> {
    tokio::task::spawn_blocking(move || {
        let mut out = Decoded::default();
        let body = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut lines = body.split('\n').peekable();
        while let Some(l) = lines.next() {
            // `split` ends with an empty piece after the last newline.
            if lines.peek().is_none() && l.is_empty() && !last_without_newline {
                break;
            }
            decoder.line(l, &mut out);
        }
        (decoder, out)
    })
    .await
    .ok()
}

async fn run(state: AppState, pid: String, src: Arc<Source>, first_path: PathBuf, guard: Guard, options: Options) {
    let mut path = first_path;
    // A finished file without times counts its rows as seconds.
    let implicit_rate = (!src.follow && options.time.is_none() && options.rate.is_none() && src.format != Format::Vcd).then_some(1.0);
    let make = || formats::decoder(src.format, Options { rate: implicit_rate.or(options.rate), ..options.clone() });
    let mut decoder: Option<Box<dyn formats::Decoder>> = Some(make());
    let mut carry: Vec<u8> = vec![];
    let mut offset: u64 = 0;
    let mut identity = file_identity(&path);
    let mut head: Vec<u8> = vec![];
    let mut last_t: HashMap<String, f64> = HashMap::new();
    let mut last_arrival = crate::util::now_ms() as f64 / 1000.0;
    let mut last_announce = std::time::Instant::now();
    let waited = std::time::Instant::now();
    let mut seen_file = false;
    let mut rows_before_restart = 0u64;

    loop {
        if src.cancel.is_cancelled() {
            set_state(&src, "stopped", None);
            return;
        }
        // Before every look: where the file is now, and that it is still one this source may read.
        match guard.resolve() {
            Ok(p) => path = p,
            Err(e) => {
                set_state(&src, "error", Some(e.message));
                return;
            }
        }
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) if m.is_file() => m,
            Ok(_) => {
                set_state(&src, "error", Some("not a file".into()));
                return;
            }
            Err(_) => {
                if !seen_file && waited.elapsed() > APPEAR_TIMEOUT {
                    set_state(&src, "error", Some(format!("{} did not appear within {} seconds", src.path, APPEAR_TIMEOUT.as_secs())));
                    return;
                }
                if seen_file && !src.follow {
                    set_state(&src, "error", Some("the file disappeared".into()));
                    return;
                }
                // Gone while following: waited for again (a log rotation, a capture that restarts).
                if seen_file {
                    set_state(&src, "waiting", None);
                }
                tokio::select! { _ = src.cancel.cancelled() => {}, _ = tokio::time::sleep(POLL) => {} }
                continue;
            }
        };
        seen_file = true;
        let len = meta.len();
        let id_now = file_identity(&path);
        let head_now = read_head(&path).await;
        // Shorter, or another file (by identity, or by how it begins): a new capture, so time starts again.
        let replaced = !head.is_empty() && !head_now.starts_with(&head);
        if len < offset || replaced || (identity.is_some() && id_now != identity) {
            offset = 0;
            carry.clear();
            decoder = Some(make());
            head.clear();
            let names = src.status.lock().series.clone();
            if !names.is_empty() {
                state.data.clear(&state, &pid, ClearTarget::Names(&names));
            }
            last_t.clear();
            rows_before_restart = 0;
            let mut s = src.status.lock();
            s.rows = 0;
            s.samples = 0;
            s.skipped = 0;
            s.bytes = 0;
        }
        identity = id_now;
        if head_now.len() > head.len() && head_now.starts_with(&head) {
            head = head_now;
        }
        {
            let mut s = src.status.lock();
            if s.state == "waiting" {
                s.state = "running";
            }
        }
        if len > offset {
            let mut file = match open_plain(&path).await {
                Ok(f) => f,
                Err(e) => {
                    set_state(&src, "error", Some(format!("cannot read the file: {e}")));
                    return;
                }
            };
            if file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
                set_state(&src, "error", Some("cannot read the file".into()));
                return;
            }
            let want = ((len - offset) as usize).min(CHUNK);
            let mut buf = vec![0u8; want];
            let n = match file.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    set_state(&src, "error", Some(format!("cannot read the file: {e}")));
                    return;
                }
            };
            buf.truncate(n);
            if offset == 0 && buf.iter().take(4096).any(|b| *b == 0) {
                set_state(&src, "error", Some("this is a binary file; csv, jsonl and vcd are text".into()));
                return;
            }
            offset += n as u64;
            carry.extend_from_slice(&buf);
            // Whole lines only; the rest waits for the writer to finish it. A finished file's last line
            // may have no newline: it is taken when the end is reached.
            let at_end = !src.follow && offset >= len;
            let cut = carry.iter().rposition(|b| *b == b'\n').map(|i| i + 1);
            let take = if at_end && !carry.is_empty() { Some((carry.len(), carry.last() != Some(&b'\n'))) } else { cut.map(|c| (c, false)) };
            if take.is_none() && carry.len() > MAX_LINE {
                set_state(&src, "error", Some("a line longer than 1 MB: this is not a text table".into()));
                return;
            }
            let mut decoded = Decoded::default();
            if let Some((upto, unterminated)) = take {
                let text = String::from_utf8_lossy(&carry[..upto]).into_owned();
                carry.drain(..upto);
                let dec = decoder.take().expect("the decoder is put back");
                match decode(dec, text, unterminated).await {
                    Some((dec, out)) => {
                        decoder = Some(dec);
                        decoded = out;
                    }
                    None => {
                        set_state(&src, "error", Some("the decoder failed".into()));
                        return;
                    }
                }
            }
            let now = crate::util::now_ms() as f64 / 1000.0;
            let stats = flush(&state, &pid, &src, decoded, &mut last_t, last_arrival, now);
            last_arrival = now;
            let rows = decoder.as_ref().map_or(0, |d| d.rows());
            let mut s = src.status.lock();
            s.rows = rows_before_restart + rows;
            s.samples += stats.0;
            s.skipped += stats.1;
            s.bytes = offset;
            s.updated_at = crate::util::now_ms();
            if let Some(e) = stats.2 {
                s.error = Some(e);
            }
            drop(s);
            if last_announce.elapsed() > Duration::from_secs(1) {
                last_announce = std::time::Instant::now();
                announce(&state, &pid);
            }
            if at_end && carry.is_empty() && len <= offset {
                set_state(&src, "done", None);
                return;
            }
            // More to read: do not sleep.
            if len > offset {
                continue;
            }
        } else if !src.follow {
            set_state(&src, "done", None);
            return;
        }
        tokio::select! { _ = src.cancel.cancelled() => {}, _ = tokio::time::sleep(POLL) => {} }
    }
}

#[cfg(unix)]
fn file_identity(p: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| m.ino())
}

#[cfg(not(unix))]
fn file_identity(_: &std::path::Path) -> Option<u64> {
    None
}

/// `base` with `suffix`, within the name limit.
fn with_suffix(base: &str, suffix: &str) -> String {
    let room = MAX_NAME.saturating_sub(suffix.chars().count());
    format!("{}{suffix}", base.chars().take(room).collect::<String>())
}

/// The last `max` characters of `s` (a path's end says more than its start).
fn tail_chars(s: &str, max: usize) -> String {
    let n = s.chars().count();
    s.chars().skip(n.saturating_sub(max)).collect()
}

/// Hand a batch of decoded rows to the series. Returns (samples taken, rows skipped, a note).
///
/// Text in a column that also has numbers (an instrument's `OVLD`) never turns the series into
/// events: it goes to `<name> (text)`, and numbers into a column that began as text to
/// `<name> (values)`. Rows whose time is not a number or goes backwards are skipped and counted.
fn flush(state: &AppState, pid: &str, src: &Source, d: Decoded, last_t: &mut HashMap<String, f64>, since: f64, now: f64) -> (u64, u64, Option<String>) {
    if d.samples.is_empty() && d.events.is_empty() {
        return (0, 0, None);
    }
    // Rows with no time of their own are spread over the span since the last look.
    let rows = d.rows.max(1);
    let stamp = |row: usize| since + (now - since) * (row as f64 + 1.0) / rows as f64;
    let mut per: Vec<(Vec<f64>, Vec<f64>, Vec<(f64, String)>)> = d.series.iter().map(|_| (vec![], vec![], vec![])).collect();
    for (i, t, v, row) in &d.samples {
        per[*i].0.push(t.unwrap_or_else(|| stamp(*row)));
        per[*i].1.push(*v);
    }
    for (i, t, text, row) in d.events {
        per[i].2.push((t.unwrap_or_else(|| stamp(row)), text));
    }
    let group = src.options_group();
    let (mut taken, mut skipped, mut note) = (0u64, 0u64, None);
    for (sref, (t, v, ev)) in d.series.iter().zip(per) {
        let existing = |name: &str| state.data.kind_and_unit(state, pid, name);
        // Each class of cell goes where it fits.
        let numeric_target = match existing(&sref.name) {
            Some((Kind::Events, _)) => with_suffix(&sref.name, " (values)"),
            _ => sref.name.clone(),
        };
        let has_numbers = !t.is_empty() || matches!(existing(&sref.name), Some((Kind::Analog | Kind::Digital, _)));
        let text_target = if has_numbers && sref.kind != Kind::Events { with_suffix(&sref.name, " (text)") } else { sref.name.clone() };

        if !t.is_empty() {
            let (mut kt, mut kv) = (Vec::with_capacity(t.len()), Vec::with_capacity(t.len()));
            let mut floor = last_t.get(&numeric_target).copied().or_else(|| state.data.end_time(state, pid, &numeric_target));
            for (x, y) in t.into_iter().zip(v) {
                if !x.is_finite() || floor.is_some_and(|f| x < f) {
                    skipped += 1;
                    continue;
                }
                floor = Some(x);
                kt.push(x);
                kv.push(y);
            }
            if !kt.is_empty() {
                let n = kt.len() as u64;
                let newest = *kt.last().expect("not empty");
                let kind = if sref.kind == Kind::Events { Kind::Analog } else { sref.kind };
                let define = if existing(&numeric_target).is_none() {
                    DefineOpts { kind: Some(kind), unit: Some(sref.unit.clone()), group: Some(group.clone()), ..DefineOpts::default() }
                } else {
                    DefineOpts::default()
                };
                match state.data.append(state, pid, AppendReq { name: numeric_target.clone(), define, clear: false, body: Body::Samples { t: kt, v: kv } }) {
                    Ok(_) => {
                        taken += n;
                        last_t.insert(numeric_target.clone(), newest);
                        note_series(src, &numeric_target);
                    }
                    Err(e) => {
                        skipped += n;
                        note = Some(e.message);
                    }
                }
            }
        }
        if !ev.is_empty() {
            let mut floor = last_t.get(&text_target).copied().or_else(|| state.data.end_time(state, pid, &text_target));
            let mut keep: Vec<(Option<f64>, String)> = Vec::with_capacity(ev.len());
            for (x, text) in ev {
                if !x.is_finite() || floor.is_some_and(|f| x < f) {
                    skipped += 1;
                    continue;
                }
                floor = Some(x);
                keep.push((Some(x), text));
            }
            if !keep.is_empty() {
                let n = keep.len() as u64;
                let newest = keep.last().and_then(|e| e.0).unwrap_or(0.0);
                let define = if existing(&text_target).is_none() {
                    DefineOpts { kind: Some(Kind::Events), group: Some(group.clone()), ..DefineOpts::default() }
                } else {
                    DefineOpts::default()
                };
                match state.data.append(state, pid, AppendReq { name: text_target.clone(), define, clear: false, body: Body::Events(keep) }) {
                    Ok(_) => {
                        taken += n;
                        last_t.insert(text_target.clone(), newest);
                        note_series(src, &text_target);
                    }
                    Err(e) => {
                        skipped += n;
                        note = Some(e.message);
                    }
                }
            }
        }
    }
    (taken, skipped, note)
}

fn note_series(src: &Source, name: &str) {
    let mut s = src.status.lock();
    if !s.series.iter().any(|n| n == name) {
        s.series.push(name.to_string());
    }
}

impl Source {
    fn options_group(&self) -> String {
        // A followed file's series sit under the file's name in the list unless told otherwise.
        tail_chars(&self.group.clone().unwrap_or_else(|| self.path.clone()), MAX_GROUP)
    }
}
