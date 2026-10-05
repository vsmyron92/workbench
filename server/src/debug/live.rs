//! Live Watch: values of variables (and fixed addresses) read from a running target, over and
//! over, without stopping it. gdb's debug adapter cannot do that (it refuses `evaluate` and
//! `readMemory` while the program runs), so the values come from OpenOCD's Tcl port
//! (`tcl.rs`), which reads memory on a running core, and each is shown with its history.
//!
//! * **Servers without such a port** (J-Link, pyOCD, st-util, QEMU…) have no side channel to read
//!   memory through while the core runs. A gdb session on an embedded target can still read by
//!   stopping the program for a few milliseconds each time and resuming it (`pausing.rs`): that
//!   disturbs the program, so it is off until the user allows it for the session.
//!
//! * **Finding the address.** An expression is resolved *statically*, by a separate batch gdb
//!   that only reads the program's ELF (no target, so it works while the program runs and does
//!   not touch the session's own gdb): its address, size and kind (`uint`, `float`, `bytes`…).
//!   That is also why only things at a fixed address can be watched: a global, a member of one,
//!   an array element, `*(uint32_t*)0x50000014`. A pointer's target moves; gdb would answer from
//!   the *file's* initial value of the pointer, so such expressions are refused (`fixed_address_only`).
//! * **Reading.** The poller reads every item each interval (250 ms by default) and sends the
//!   samples as `debug.live` events. It never sends the Tcl port anything but `read_memory`.
//! * **Peripheral registers** are memory too, and reading some of them changes the chip (a status
//!   flag that clears when read): with an SVD file those are refused by name, and any other
//!   address in the peripheral regions is flagged.
//! * **Trust.** The expression is the user's own text, handed to gdb as a Python string; the batch
//!   gdb runs outside the project's directory with `-nx` and no auto-loading, because the
//!   project's files are untrusted.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::session::{Session, SessionState};
use super::tcl::{self, TclClient, TclError};
use crate::app::AppState;
use crate::error::ApiError;

/// Items a session watches at once.
pub const MAX_ITEMS: usize = 16;
/// The most bytes one item covers (an array or a struct is shown as bytes).
pub const MAX_BYTES: u32 = 64;
const DEFAULT_INTERVAL_MS: u64 = 250;
const MIN_INTERVAL_MS: u64 = 50;
const MAX_INTERVAL_MS: u64 = 5000;
/// Reading by stopping the program: not faster than this, however fast the user asks.
pub const MIN_PAUSING_INTERVAL_MS: u64 = 100;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);
/// Readings kept per item (the plot viewer's own buffer holds as many): ten minutes at the fastest rate.
pub const HISTORY_MAX: usize = 12_000;

// ---------------------------------------------------------------- items

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Int,
    Uint,
    Float,
    Bool,
    Ptr,
    Enum,
    Bytes,
}

impl Kind {
    fn parse(s: &str) -> Kind {
        match s {
            "int" => Kind::Int,
            "uint" => Kind::Uint,
            "float" => Kind::Float,
            "bool" => Kind::Bool,
            "ptr" => Kind::Ptr,
            "enum" => Kind::Enum,
            _ => Kind::Bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: u32,
    pub expression: String,
    /// `None`: it could not be resolved (`error` says why); such an item is never read.
    pub address: Option<u64>,
    pub size: u32,
    pub kind: Kind,
    pub type_name: String,
    /// A note for a peripheral register, which a read may disturb.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peripheral: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Sample {
    pub id: u32,
    /// Milliseconds since the epoch.
    pub t: i64,
    /// A number, or a string for what a JSON number cannot hold exactly (64-bit values, bytes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e: Option<String>,
}

/// One reading as kept for plots: the time, the number (NaN for a failed reading and for what is no number: bytes), and
/// for a whole number a double cannot hold exactly (64-bit values) its digits.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub t: i64,
    pub v: f64,
    pub exact: Option<Box<str>>,
}

impl Point {
    pub fn of(s: &Sample) -> Point {
        let (v, exact) = match (&s.e, &s.v) {
            (None, Some(Value::Number(n))) => (n.as_f64().unwrap_or(f64::NAN), None),
            (None, Some(Value::Bool(b))) => (if *b { 1.0 } else { 0.0 }, None),
            (None, Some(Value::String(text))) => match text.parse::<i128>() {
                Ok(n) => (n as f64, Some(text.as_str().into())),
                Err(_) => (f64::NAN, None),
            },
            _ => (f64::NAN, None),
        };
        Point { t: s.t, v, exact }
    }
}

#[derive(Debug)]
struct LiveState {
    items: Vec<Item>,
    interval_ms: u64,
    last: HashMap<u32, Sample>,
    /// The newest `HISTORY_MAX` readings of each item, oldest first.
    history: HashMap<u32, VecDeque<Point>>,
}

impl Default for LiveState {
    fn default() -> Self {
        LiveState { items: vec![], interval_ms: DEFAULT_INTERVAL_MS, last: HashMap::new(), history: HashMap::new() }
    }
}

impl LiveState {
    /// Keep a reading of a watched item (one removed meanwhile stays removed).
    fn record(&mut self, sample: &Sample) {
        if !self.items.iter().any(|i| i.id == sample.id) {
            return;
        }
        self.last.insert(sample.id, sample.clone());
        let h = self.history.entry(sample.id).or_default();
        if h.len() >= HISTORY_MAX {
            h.pop_front();
        }
        h.push_back(Point::of(sample));
    }
}

/// How a session reads memory while the program runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// It cannot (a host program, a debugger without readMemory).
    None,
    /// Through the debug server's Tcl port: the program is never stopped.
    Tcl,
    /// By stopping the program for a moment and resuming it, when the user allows it.
    Pausing,
}

impl Mode {
    pub fn name(self) -> Option<&'static str> {
        match self {
            Mode::None => None,
            Mode::Tcl => Some("tcl"),
            Mode::Pausing => Some("pausing"),
        }
    }
}

/// A session's live watch: the Tcl port of its debug server (or the means to read by pausing) and what is watched.
#[derive(Debug)]
pub struct LiveWatch {
    /// OpenOCD's Tcl port; 0 while the server has none.
    port: AtomicU16,
    /// The session is a gdb session on an embedded target, so it could read by pausing the program.
    pausing_possible: bool,
    /// The user allowed that for this session.
    pausing: AtomicBool,
    /// How long a round keeps the program stopped, milliseconds, smoothed over the last rounds (0: not measured yet).
    pause_ms: AtomicU32,
    next_id: AtomicU32,
    polling: AtomicBool,
    state: Mutex<LiveState>,
}

impl LiveWatch {
    pub fn new(pausing_possible: bool) -> Self {
        LiveWatch { port: AtomicU16::new(0), pausing_possible, pausing: AtomicBool::new(false), pause_ms: AtomicU32::new(0), next_id: AtomicU32::new(0), polling: AtomicBool::new(false), state: Mutex::new(LiveState::default()) }
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::SeqCst);
    }

    pub fn mode(&self) -> Mode {
        if self.port.load(Ordering::SeqCst) != 0 {
            Mode::Tcl
        } else if self.pausing_possible {
            Mode::Pausing
        } else {
            Mode::None
        }
    }

    /// Whether this session has a live view: it can read memory while the program runs (or, with the user's say-so, by pausing it).
    pub fn enabled(&self) -> bool {
        self.mode() != Mode::None
    }

    /// The user allowed reading by pausing the program.
    pub fn pausing_on(&self) -> bool {
        self.pausing.load(Ordering::SeqCst)
    }

    /// A round kept the program stopped for `ms`: fold it into the running average the user is shown.
    pub fn note_pause(&self, ms: u32) {
        let old = self.pause_ms.load(Ordering::SeqCst);
        let next = if old == 0 { ms.max(1) } else { ((u64::from(old) * 7 + u64::from(ms) * 3) / 10).max(1) as u32 };
        self.pause_ms.store(next, Ordering::SeqCst);
    }

    /// The average time a round keeps the program stopped, if any round has run.
    pub fn pause_ms(&self) -> Option<u32> {
        Some(self.pause_ms.load(Ordering::SeqCst)).filter(|m| *m != 0)
    }

    fn port(&self) -> Option<u16> {
        Some(self.port.load(Ordering::SeqCst)).filter(|p| *p != 0)
    }

    /// What `debug_state` tells an agent: each watched expression with its latest value.
    pub fn values(&self) -> Vec<Value> {
        let st = self.state.lock();
        st.items
            .iter()
            .map(|i| {
                let last = st.last.get(&i.id);
                json!({ "expression": i.expression, "type": i.type_name, "value": last.and_then(|s| s.v.clone()), "error": i.error.clone().or_else(|| last.and_then(|s| s.e.clone())) })
            })
            .collect()
    }

    pub fn snapshot(&self) -> Value {
        let st = self.state.lock();
        json!({ "items": st.items, "intervalMs": st.interval_ms, "last": st.last, "mode": self.mode().name(), "pausing": self.pausing_on(), "pauseMs": self.pause_ms() })
    }

    /// The watched item with this expression.
    pub fn find(&self, expression: &str) -> Option<Item> {
        self.state.lock().items.iter().find(|i| i.expression == expression).cloned()
    }

    /// The readings kept of an item, oldest first: only those after `since` (ms), and at most the newest `limit`.
    pub fn readings(&self, id: u32, since: Option<i64>, limit: usize) -> Vec<Point> {
        let st = self.state.lock();
        let Some(h) = st.history.get(&id) else { return vec![] };
        let after: Vec<&Point> = h.iter().filter(|p| since.is_none_or(|s| p.t > s)).collect();
        after[after.len().saturating_sub(limit)..].iter().map(|p| (*p).clone()).collect()
    }
}

// ---------------------------------------------------------------- history

/// At most about `max` points: the readings are cut into `max / 2` equal slices and each keeps its lowest and highest
/// number, in time order, so a spike or a dip survives. A slice with no number (every reading failed) keeps its first.
pub fn decimate(points: &[Point], max: usize) -> Vec<Point> {
    let max = max.max(4);
    if points.len() <= max {
        return points.to_vec();
    }
    let size = points.len().div_ceil(max / 2);
    let mut out = Vec::with_capacity(max + 2);
    for chunk in points.chunks(size) {
        let (mut lo, mut hi): (Option<usize>, Option<usize>) = (None, None);
        for (i, p) in chunk.iter().enumerate() {
            if !p.v.is_finite() {
                continue;
            }
            if lo.is_none_or(|l| p.v < chunk[l].v) {
                lo = Some(i);
            }
            if hi.is_none_or(|h| p.v > chunk[h].v) {
                hi = Some(i);
            }
        }
        match (lo, hi) {
            (Some(l), Some(h)) => {
                let (a, b) = if l <= h { (l, h) } else { (h, l) };
                out.push(chunk[a].clone());
                if b != a {
                    out.push(chunk[b].clone());
                }
            }
            _ => out.push(chunk[0].clone()),
        }
    }
    out
}

/// `{t: [ms…], v: [number | null…], exact?: {index: "digits"}}`: null is a reading that failed or is no number; `exact`
/// has the digits of the whole numbers a double cannot hold.
pub fn points_json(points: &[Point]) -> Value {
    let t: Vec<i64> = points.iter().map(|p| p.t).collect();
    let v: Vec<Value> = points.iter().map(|p| if p.v.is_finite() { json!(p.v) } else { Value::Null }).collect();
    let exact: serde_json::Map<String, Value> = points.iter().enumerate().filter_map(|(i, p)| p.exact.as_ref().map(|x| (i.to_string(), json!(x)))).collect();
    let mut out = json!({ "t": t, "v": v });
    if !exact.is_empty() {
        out["exact"] = Value::Object(exact);
    }
    out
}

/// Count, lowest, highest and mean of the numbers among `points`, the newest number with its time, and how many readings failed.
pub fn stats(points: &[Point]) -> Value {
    let nums: Vec<&Point> = points.iter().filter(|p| p.v.is_finite()).collect();
    let mut out = json!({ "readings": points.len(), "failed": points.len() - nums.len() });
    if let (Some(lo), Some(hi), Some(last)) = (nums.iter().map(|p| p.v).reduce(f64::min), nums.iter().map(|p| p.v).reduce(f64::max), nums.last()) {
        out["min"] = json!(lo);
        out["max"] = json!(hi);
        out["mean"] = json!(nums.iter().map(|p| p.v).sum::<f64>() / nums.len() as f64);
        out["last"] = json!({ "t": last.t, "v": last.exact.as_ref().map(|x| json!(x)).unwrap_or_else(|| json!(last.v)) });
    }
    out
}

/// What `GET …/live/history` answers: the readings kept of the watched items (all, or `ids`), each as `points_json`, keyed by item id.
pub fn history(s: &Session, ids: Option<&[u32]>, since: Option<i64>, limit: usize, max_points: Option<usize>) -> Result<Value, ApiError> {
    require_live(s)?;
    let (items, interval_ms) = {
        let st = s.live.state.lock();
        (st.items.iter().filter(|i| ids.is_none_or(|ids| ids.contains(&i.id))).cloned().collect::<Vec<_>>(), st.interval_ms)
    };
    let limit = limit.clamp(1, HISTORY_MAX);
    let mut series = serde_json::Map::new();
    for item in &items {
        let mut points = s.live.readings(item.id, since, limit);
        if let Some(max) = max_points {
            points = decimate(&points, max);
        }
        series.insert(item.id.to_string(), points_json(&points));
    }
    Ok(json!({ "intervalMs": interval_ms, "items": items, "series": series, "now": crate::util::now_ms() }))
}

// ---------------------------------------------------------------- what can be watched

/// A live watch is found in the ELF at link time, so an expression that follows a pointer or an
/// index held in memory would name whatever those held *in the file* (their initial values), not
/// what they hold now. Refuse those; a fixed address is always fine.
pub fn fixed_address_only(expression: &str) -> Result<(), String> {
    const MESSAGE: &str = "it follows a pointer or an index held in memory, which moves while the program runs: watch a variable, a member or element at a fixed place, or a fixed address (*(uint32_t*)0x50000014)";
    if expression.contains("->") {
        return Err(MESSAGE.into());
    }
    let chars: Vec<char> = expression.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                let close = chars[i..].iter().position(|c| *c == ']').map(|p| i + p).ok_or("an unclosed [")?;
                let inside: String = chars[i + 1..close].iter().collect();
                let inside = inside.trim();
                let number = inside.strip_prefix("0x").or_else(|| inside.strip_prefix("0X")).map(|h| h.chars().all(|c| c.is_ascii_hexdigit())).unwrap_or_else(|| inside.chars().all(|c| c.is_ascii_digit()));
                if inside.is_empty() || !number {
                    return Err(MESSAGE.into());
                }
                i = close;
            }
            '*' => {
                let next = chars[i + 1..].iter().find(|c| !c.is_whitespace()).copied();
                match next {
                    // `*name`: a dereference of a variable.
                    Some(c) if c.is_alphabetic() || c == '_' => return Err(MESSAGE.into()),
                    // `*(type*)<address>`: fine when the group is a cast to a pointer type and what follows is a number or `&name`.
                    Some('(') => {
                        let open = i + 1 + chars[i + 1..].iter().position(|c| *c == '(').unwrap();
                        let mut depth = 0;
                        let mut close = None;
                        for (j, c) in chars.iter().enumerate().skip(open) {
                            match c {
                                '(' => depth += 1,
                                ')' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        close = Some(j);
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        let close = close.ok_or("an unclosed (")?;
                        let group: String = chars[open + 1..close].iter().collect();
                        let operand = chars[close + 1..].iter().find(|c| !c.is_whitespace()).copied();
                        if !group.trim_end().ends_with('*') || !operand.is_some_and(|c| c.is_ascii_digit() || c == '&') {
                            return Err(MESSAGE.into());
                        }
                        i = close;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// Whether `[address, address + size)` may be read over and over, and what to say about it:
/// refused for an SVD register that a read changes; noted for any other peripheral register.
fn peripheral_note(svd: Option<&super::svd::Svd>, address: u64, size: u32) -> Result<Option<String>, String> {
    let end = address.saturating_add(u64::from(size));
    if let Some(svd) = svd {
        for a in address..end {
            if let Some((p, r)) = svd.register_at(a) {
                if r.read_action {
                    return Err(format!("reading {}.{} changes the chip (a status flag that clears when read), so it is not read over and over: read it by name in the Peripherals tab", p.name, r.name));
                }
            }
        }
        if let Some((p, r)) = svd.register_at(address) {
            return Ok(Some(format!("{}.{}", p.name, r.name)));
        }
    }
    let peripheral_region = |a: u64| (0x4000_0000..0x6000_0000).contains(&a) || a >= 0xE000_0000;
    Ok((peripheral_region(address) || peripheral_region(end.saturating_sub(1))).then(|| "a peripheral register".to_string()))
}

// ---------------------------------------------------------------- values

/// A value as JSON: a number when it is exact, a string when it is not (or is bytes).
pub fn decode(kind: Kind, size: u32, bytes: &[u8]) -> Value {
    let hex = |b: &[u8]| Value::String(b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
    if bytes.len() != size as usize || size == 0 {
        return hex(bytes);
    }
    let unsigned = || -> u64 { bytes.iter().rev().fold(0u64, |a, b| (a << 8) | u64::from(*b)) };
    let number = |v: i128| -> Value {
        if v.unsigned_abs() <= (1u128 << 53) { json!(v as i64) } else { Value::String(v.to_string()) }
    };
    match kind {
        Kind::Uint | Kind::Enum | Kind::Ptr if size <= 8 => number(i128::from(unsigned())),
        Kind::Bool if size <= 8 => json!(unsigned() != 0),
        Kind::Int if size <= 8 => {
            let bits = size * 8;
            let v = unsigned();
            let signed = if bits < 64 && v & (1 << (bits - 1)) != 0 { (v as i128) - (1i128 << bits) } else { i128::from(v as i64) };
            number(if bits == 64 { i128::from(v as i64) } else { signed })
        }
        Kind::Float if size == 4 => finite(f64::from(f32::from_le_bytes(bytes.try_into().unwrap()))),
        Kind::Float if size == 8 => finite(f64::from_le_bytes(bytes.try_into().unwrap())),
        _ => hex(bytes),
    }
}

fn finite(v: f64) -> Value {
    if v.is_finite() { json!(v) } else if v.is_nan() { json!("NaN") } else if v > 0.0 { json!("inf") } else { json!("-inf") }
}

// ---------------------------------------------------------------- finding the address

/// Run by a batch gdb with `WB_EXPR` set: the address, size and kind of an expression, or why not.
const RESOLVE_CODE: &str = r#"import json, gdb
try:
    v = gdb.parse_and_eval(WB_EXPR)
    a = v.address
    if a is None:
        raise ValueError("it has no address in memory: only variables and memory can be watched live")
    t = v.type.strip_typedefs()
    c = t.code
    if c == gdb.TYPE_CODE_FLT: kind = "float"
    elif c == gdb.TYPE_CODE_BOOL: kind = "bool"
    elif c == gdb.TYPE_CODE_PTR: kind = "ptr"
    elif c == gdb.TYPE_CODE_ENUM: kind = "enum"
    elif c in (gdb.TYPE_CODE_INT, gdb.TYPE_CODE_CHAR): kind = "int" if int(gdb.Value(-1).cast(t)) < 0 else "uint"
    else: kind = "bytes"
    out = {"addr": int(a), "size": t.sizeof, "kind": kind, "type": str(v.type)}
except Exception as e:
    out = {"error": str(e)}
print("WBLIVE " + json.dumps(out))
"#;

#[derive(Debug, PartialEq)]
pub struct Resolved {
    pub address: u64,
    pub size: u32,
    pub kind: Kind,
    pub type_name: String,
}

/// The arguments of the batch gdb: the session's own (`-q -i dap` minus the DAP part), then no
/// init files, no auto-loading, the program, and the one command.
pub fn resolve_args(adapter_args: &[String], program: &str, expression: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![];
    let mut it = adapter_args.iter().peekable();
    while let Some(a) = it.next() {
        if a == "-i" || a == "--interpreter" {
            it.next();
        } else if !a.starts_with("--interpreter=") {
            args.push(a.clone());
        }
    }
    let command = format!("python WB_EXPR = {}; exec({})", serde_json::to_string(expression).unwrap(), serde_json::to_string(RESOLVE_CODE).unwrap());
    args.extend(["-nx", "-batch", "-iex", "set auto-load off", "-iex", "set debuginfod enabled off", program, "-ex", &command].map(String::from));
    args
}

/// The result line of the batch gdb's output.
pub fn parse_resolved(stdout: &str, stderr: &str) -> Result<Resolved, String> {
    let line = stdout.lines().find_map(|l| l.strip_prefix("WBLIVE "));
    let Some(line) = line else {
        let why: String = stderr.lines().chain(stdout.lines()).rev().find(|l| !l.trim().is_empty()).unwrap_or("it printed nothing").chars().take(200).collect();
        return Err(format!("gdb could not read the program's symbols: {why}"));
    };
    let v: Value = serde_json::from_str(line).map_err(|_| "gdb's answer was not understood".to_string())?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        let hint = if e.contains("Cannot access memory") { " (a value read from the target cannot be found without it: watch something at a fixed address)" } else { "" };
        return Err(format!("{e}{hint}"));
    }
    let get = |k: &str| v.get(k).and_then(Value::as_u64);
    Ok(Resolved {
        address: get("addr").ok_or("gdb gave no address")?,
        size: u32::try_from(get("size").ok_or("gdb gave no size")?).map_err(|_| "its size is too large".to_string())?,
        kind: Kind::parse(v.get("kind").and_then(Value::as_str).unwrap_or("bytes")),
        type_name: v.get("type").and_then(Value::as_str).unwrap_or("").chars().take(120).collect(),
    })
}

async fn resolve(state: &AppState, s: &Arc<Session>, expression: &str) -> Result<Resolved, String> {
    let program = s.program_path().ok_or("this configuration has no program to read the symbols of")?;
    let args = resolve_args(&s.adapter.args, &program.display().to_string(), expression);
    let mut argv = vec![s.adapter.command.clone()];
    argv.extend(args);
    // Outside the project: Python would import `json.py` from the working directory.
    let cwd = state.paths.data_dir.join("debug").join("tmp");
    std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;
    let mut cmd = crate::util::os::shell::command(&argv);
    cmd.current_dir(&cwd).envs(s.adapter.env.iter().cloned()).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(RESOLVE_TIMEOUT, cmd.output())
        .await
        .map_err(|_| format!("gdb did not answer in {} s", RESOLVE_TIMEOUT.as_secs()))?
        .map_err(|e| format!("could not run {}: {e}", s.adapter.command))?;
    parse_resolved(&String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr))
}

// ---------------------------------------------------------------- the API

fn emit(state: &AppState, s: &Session, payload: Value) {
    let mut p = payload;
    p["sessionId"] = json!(s.id);
    state.events.emit("debug.live", Some(&s.project_id), p);
}

fn clean_expression(expression: &str) -> Result<String, ApiError> {
    let e = expression.trim();
    if e.is_empty() || e.chars().count() > 300 || e.chars().any(char::is_control) {
        return Err(ApiError::bad_request("an expression of up to 300 characters on one line"));
    }
    Ok(e.to_string())
}

fn require_live(s: &Session) -> Result<(), ApiError> {
    if s.live.enabled() {
        Ok(())
    } else {
        Err(ApiError::conflict("this session has no live view: it needs a gdb session on an embedded target, and a debug server with OpenOCD's Tcl port (the OpenOCD preset has it) to read without stopping the program"))
    }
}

/// Allow (or stop) reading by pausing the program, for this session. The user's call: it disturbs the program.
pub fn set_pausing(state: &AppState, s: &Arc<Session>, on: bool) -> Result<(), ApiError> {
    if s.live.mode() != Mode::Pausing {
        return Err(ApiError::conflict("this session reads through the debug server's Tcl port: the program is never stopped"));
    }
    s.live.pausing.store(on, Ordering::SeqCst);
    emit(state, s, json!({ "pausing": on }));
    if on {
        ensure_poller(state, s);
    }
    Ok(())
}

async fn persist(state: &AppState, s: &Session) {
    let list: Vec<String> = s.live.state.lock().items.iter().map(|i| i.expression.clone()).collect();
    let _ = state.debug.store.update(&state.paths.data_dir, &s.project_id, |p| p.set_live_watches(list)).await;
}

/// Resolve `expression` and watch it. `Err` for a mistake the user should hear about.
pub async fn add(state: &AppState, s: &Arc<Session>, expression: &str) -> Result<Item, ApiError> {
    require_live(s)?;
    let expression = clean_expression(expression)?;
    if let Some(i) = s.live.state.lock().items.iter().find(|i| i.expression == expression) {
        return Ok(i.clone());
    }
    if s.live.state.lock().items.len() >= MAX_ITEMS {
        return Err(ApiError::bad_request(format!("at most {MAX_ITEMS} expressions are watched at once")));
    }
    fixed_address_only(&expression).map_err(|e| ApiError::bad_request(format!("{expression}: {e}")))?;
    let r = resolve(state, s, &expression).await.map_err(|e| ApiError::bad_request(format!("{expression}: {e}")))?;
    if r.size == 0 || r.size > MAX_BYTES {
        return Err(ApiError::bad_request(format!("{expression} is {} bytes: at most {MAX_BYTES} can be watched (watch a member or an element)", r.size)));
    }
    let svd = s.svd().await.ok();
    let peripheral = peripheral_note(svd.as_deref(), r.address, r.size).map_err(|e| ApiError::conflict(format!("{expression}: {e}")))?;
    let item = Item { id: s.live.next_id.fetch_add(1, Ordering::SeqCst) + 1, expression, address: Some(r.address), size: r.size, kind: r.kind, type_name: r.type_name, peripheral, error: None };
    {
        let mut st = s.live.state.lock();
        // The same expression may have been added while this one was being resolved (the watches brought back at the
        // start of a session, another tab): it is one item.
        if let Some(i) = st.items.iter().find(|i| i.expression == item.expression) {
            return Ok(i.clone());
        }
        if st.items.len() >= MAX_ITEMS {
            return Err(ApiError::bad_request(format!("at most {MAX_ITEMS} expressions are watched at once")));
        }
        st.items.push(item.clone());
    }
    persist(state, s).await;
    emit(state, s, json!({ "items": s.live.state.lock().items }));
    ensure_poller(state, s);
    Ok(item)
}

pub async fn remove(state: &AppState, s: &Arc<Session>, id: u32) -> Result<(), ApiError> {
    require_live(s)?;
    {
        let mut st = s.live.state.lock();
        let before = st.items.len();
        st.items.retain(|i| i.id != id);
        st.last.remove(&id);
        st.history.remove(&id);
        if st.items.len() == before {
            return Err(ApiError::not_found(format!("no watched expression {id}")));
        }
    }
    persist(state, s).await;
    emit(state, s, json!({ "items": s.live.state.lock().items }));
    Ok(())
}

pub fn set_interval(state: &AppState, s: &Arc<Session>, ms: u64) -> Result<(), ApiError> {
    require_live(s)?;
    let min = if s.live.mode() == Mode::Pausing { MIN_PAUSING_INTERVAL_MS } else { MIN_INTERVAL_MS };
    if !(min..=MAX_INTERVAL_MS).contains(&ms) {
        return Err(ApiError::bad_request(format!("an interval of {min} to {MAX_INTERVAL_MS} ms")));
    }
    s.live.state.lock().interval_ms = ms;
    emit(state, s, json!({ "intervalMs": ms }));
    Ok(())
}

pub fn snapshot(s: &Session) -> Result<Value, ApiError> {
    require_live(s)?;
    Ok(s.live.snapshot())
}

/// The session's server is up and has a Tcl port: start reading, and bring back the expressions
/// the project watched last time (an unresolvable one stays, with the reason).
pub fn start(state: &AppState, s: &Arc<Session>) {
    ensure_poller(state, s);
    let (state, s) = (state.clone(), s.clone());
    tokio::spawn(async move {
        let list = state.debug.store.get(&state.paths.data_dir, &s.project_id).live_watches;
        for expression in list {
            if s.cancel.is_cancelled() {
                return;
            }
            if let Err(e) = add(&state, &s, &expression).await {
                let id = s.live.next_id.fetch_add(1, Ordering::SeqCst) + 1;
                let mut st = s.live.state.lock();
                if st.items.len() < MAX_ITEMS && !st.items.iter().any(|i| i.expression == expression) {
                    st.items.push(Item { id, expression, address: None, size: 0, kind: Kind::Bytes, type_name: String::new(), peripheral: None, error: Some(e.message) });
                }
                drop(st);
                emit(&state, &s, json!({ "items": s.live.state.lock().items }));
            }
        }
    });
}

// ---------------------------------------------------------------- reading

fn ensure_poller(state: &AppState, s: &Arc<Session>) {
    if !s.live.polling.swap(true, Ordering::SeqCst) {
        tokio::spawn(poll(state.clone(), s.clone()));
    }
}

async fn read_item(client: &mut TclClient, item: &Item) -> Result<Value, TclError> {
    let address = item.address.ok_or_else(|| TclError::Command("not resolved".into()))?;
    let (width, count) = tcl::plan_read(address, item.size);
    let words = client.read_memory(address, width, count).await?;
    Ok(decode(item.kind, item.size, &tcl::words_to_bytes(&words, width)))
}

/// One round of readings through the Tcl port.
async fn read_tcl(client: &mut Option<TclClient>, port: u16, items: &[Item]) -> Vec<Sample> {
    let now = crate::util::now_ms();
    let mut samples: Vec<Sample> = vec![];
    if client.is_none() {
        match TclClient::connect(port).await {
            Ok(c) => *client = Some(c),
            Err(e) => samples.extend(items.iter().map(|i| Sample { id: i.id, t: now, v: None, e: Some(e.to_string()) })),
        }
    }
    if let Some(c) = client.as_mut() {
        let mut broken = false;
        for item in items {
            if broken {
                samples.push(Sample { id: item.id, t: now, v: None, e: Some("the connection to the debug server was lost".into()) });
                continue;
            }
            match read_item(c, item).await {
                Ok(v) => samples.push(Sample { id: item.id, t: now, v: Some(v), e: None }),
                Err(TclError::Command(m)) => samples.push(Sample { id: item.id, t: now, v: None, e: Some(m) }),
                Err(TclError::Transport(m)) => {
                    broken = true;
                    samples.push(Sample { id: item.id, t: now, v: None, e: Some(m) });
                }
            }
        }
        if broken {
            *client = None;
        }
    }
    samples
}

async fn poll(state: AppState, s: Arc<Session>) {
    let mut client: Option<TclClient> = None;
    // Rounds in a row that could not read by pausing the program: each one waits longer than the last.
    let mut failed: u32 = 0;
    // How long a round keeps the program stopped (average), told along with the readings.
    let mut pause_ms: Option<u32> = None;
    loop {
        let mode = s.live.mode();
        let mut interval = s.live.state.lock().interval_ms;
        if mode == Mode::Pausing {
            interval = (interval.max(MIN_PAUSING_INTERVAL_MS) << failed.min(4)).min(MAX_INTERVAL_MS);
        }
        tokio::select! {
            _ = s.cancel.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_millis(interval)) => {}
        }
        if !s.is_live() {
            return;
        }
        let items: Vec<Item> = s.live.state.lock().items.iter().filter(|i| i.address.is_some()).cloned().collect();
        // The server is not up yet (starting, downloading): nothing to read from.
        if items.is_empty() || s.state() == SessionState::Starting {
            continue;
        }
        let samples = match mode {
            Mode::None => continue,
            Mode::Tcl => {
                let Some(port) = s.live.port() else { continue };
                read_tcl(&mut client, port, &items).await
            }
            // Only once the user allowed it: reading stops the program for a moment.
            Mode::Pausing if !s.live.pausing_on() => continue,
            Mode::Pausing => match super::pausing::read(&state, &s, &items).await {
                super::pausing::Outcome::Skip => continue,
                super::pausing::Outcome::Done(samples, stopped) => {
                    failed = 0;
                    if let Some(ms) = stopped {
                        s.live.note_pause(ms);
                        pause_ms = s.live.pause_ms();
                    }
                    samples
                }
                super::pausing::Outcome::Failed(samples) => {
                    failed = failed.saturating_add(1);
                    samples
                }
            },
        };
        {
            let mut st = s.live.state.lock();
            for sample in &samples {
                // A watch removed while this was reading stays removed.
                st.record(sample);
            }
        }
        let mut payload = json!({ "samples": samples });
        if let (Mode::Pausing, Some(ms)) = (mode, pause_ms) {
            payload["pauseMs"] = json!(ms);
        }
        emit(&state, &s, payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: u32, t: i64, v: Option<Value>, e: Option<&str>) -> Sample {
        Sample { id, t, v, e: e.map(String::from) }
    }

    #[test]
    fn the_time_a_round_keeps_the_program_stopped_is_averaged() {
        let live = LiveWatch::new(true);
        assert_eq!(live.pause_ms(), None);
        live.note_pause(40);
        assert_eq!(live.pause_ms(), Some(40), "the first round is the average");
        live.note_pause(50);
        assert_eq!(live.pause_ms(), Some(43), "70% of the old, 30% of the new");
        for _ in 0..40 {
            live.note_pause(10);
        }
        assert!(live.pause_ms().unwrap() <= 11, "it follows the recent rounds");
        live.note_pause(0);
        assert!(live.pause_ms().unwrap() >= 1, "never back to 'not measured'");
        assert_eq!(live.snapshot()["pauseMs"], json!(live.pause_ms()));
    }

    #[test]
    fn a_reading_is_kept_as_a_number_with_a_gap_for_what_is_none() {
        assert_eq!(Point::of(&sample(1, 5, Some(json!(42)), None)), Point { t: 5, v: 42.0, exact: None });
        assert_eq!(Point::of(&sample(1, 5, Some(json!(true)), None)).v, 1.0);
        assert_eq!(Point::of(&sample(1, 5, Some(json!(false)), None)).v, 0.0);
        assert_eq!(Point::of(&sample(1, 5, Some(json!(-2.5)), None)).v, -2.5);
        // A 64-bit value keeps its digits beside the nearest double.
        let big = Point::of(&sample(1, 5, Some(json!("18446744073709551615")), None));
        assert_eq!((big.v, big.exact.as_deref()), (18446744073709551615u64 as f64, Some("18446744073709551615")));
        assert_eq!(Point::of(&sample(1, 5, Some(json!("-9223372036854775808")), None)).exact.as_deref(), Some("-9223372036854775808"));
        // Bytes, text, a failure and nothing are gaps.
        for s in [sample(1, 5, Some(json!("01 ab ff")), None), sample(1, 5, Some(json!("NaN")), None), sample(1, 5, None, Some("read failed")), sample(1, 5, Some(json!(7)), Some("but failed")), sample(1, 5, None, None), sample(1, 5, Some(json!([1])), None)] {
            let p = Point::of(&s);
            assert!(p.v.is_nan() && p.exact.is_none() && p.t == 5, "{s:?}");
        }
    }

    #[test]
    fn the_history_is_capped_and_forgets_what_is_no_longer_watched() {
        let item = |id| Item { id, expression: format!("v{id}"), address: Some(0x2000_0000), size: 4, kind: Kind::Uint, type_name: "uint32_t".into(), peripheral: None, error: None };
        let mut st = LiveState::default();
        st.items = vec![item(1)];
        for i in 0..(HISTORY_MAX as i64 + 5) {
            st.record(&sample(1, i, Some(json!(i)), None));
        }
        assert_eq!(st.history[&1].len(), HISTORY_MAX);
        assert_eq!((st.history[&1].front().unwrap().t, st.history[&1].back().unwrap().t), (5, HISTORY_MAX as i64 + 4), "the oldest went");
        assert_eq!(st.last[&1].t, HISTORY_MAX as i64 + 4);
        // A reading of something not watched (removed while the poller was reading) is not kept.
        st.record(&sample(2, 1, Some(json!(1)), None));
        assert!(!st.history.contains_key(&2) && !st.last.contains_key(&2));
    }

    #[test]
    fn thinning_keeps_the_lowest_and_the_highest_of_every_slice_in_time_order() {
        let pts: Vec<Point> = (0..1000).map(|i| Point { t: i, v: if i == 500 { 1000.0 } else if i == 250 { -1000.0 } else { (i % 7) as f64 }, exact: None }).collect();
        let few = decimate(&pts, 20);
        assert!(few.len() <= 20, "{}", few.len());
        assert!(few.iter().any(|p| p.v == 1000.0) && few.iter().any(|p| p.v == -1000.0), "a spike and a dip survive");
        assert!(few.windows(2).all(|w| w[0].t < w[1].t), "times go forward");
        // Short enough: untouched. A tiny maximum still gives something sensible.
        assert_eq!(decimate(&pts[..10], 20), pts[..10].to_vec());
        assert!(decimate(&pts, 1).len() <= 4);
        // A slice with no number (every reading failed) keeps its first reading, as a gap.
        let mut gappy: Vec<Point> = (0..40).map(|i| Point { t: i, v: f64::NAN, exact: None }).collect();
        gappy[39].v = 5.0;
        let thin = decimate(&gappy, 8);
        assert!(thin.iter().any(|p| p.v == 5.0) && thin.iter().any(|p| p.v.is_nan()), "{thin:?}");
        assert!(decimate(&[], 10).is_empty());
    }

    #[test]
    fn points_and_statistics_read_back_as_json() {
        let pts = vec![
            Point { t: 10, v: 1.0, exact: None },
            Point { t: 20, v: f64::NAN, exact: None },
            Point { t: 30, v: 4.0, exact: None },
            Point { t: 40, v: 18446744073709551615u64 as f64, exact: Some("18446744073709551615".into()) },
        ];
        let j = points_json(&pts);
        assert_eq!((j["t"].clone(), j["v"][1].clone(), j["exact"].clone()), (json!([10, 20, 30, 40]), Value::Null, json!({ "3": "18446744073709551615" })));
        assert!(points_json(&pts[..3]).get("exact").is_none());
        let s = stats(&pts[..3]);
        assert_eq!((s["readings"].clone(), s["failed"].clone(), s["min"].clone(), s["max"].clone(), s["mean"].clone()), (json!(3), json!(1), json!(1.0), json!(4.0), json!(2.5)));
        assert_eq!(s["last"], json!({ "t": 30, "v": 4.0 }));
        // The newest number is told by its digits when it has them.
        assert_eq!(stats(&pts)["last"]["v"], json!("18446744073709551615"));
        let none = stats(&[Point { t: 1, v: f64::NAN, exact: None }]);
        assert_eq!((none["readings"].clone(), none["failed"].clone()), (json!(1), json!(1)));
        assert!(none.get("min").is_none() && none.get("last").is_none());
        assert_eq!(stats(&[])["readings"], json!(0));
    }

    #[test]
    fn only_things_at_a_fixed_address_can_be_watched() {
        for ok in ["ticks", "cfg.threshold", "buf[3]", "buf[0x10]", "m.rows[2].cols[1]", "*(unsigned int*)0x50000014", "*(volatile uint32_t *)0x50000014", "*(int*)&ticks", "(unsigned)ticks"] {
            assert_eq!(fixed_address_only(ok), Ok(()), "{ok}");
        }
        for bad in ["*ptr", "p->x", "(*p).x", "buf[i]", "buf[ i + 1 ]", "*(p + 1)", "*(int*)ptr", "*(int*)(base + 4)", "buf[]"] {
            assert!(fixed_address_only(bad).unwrap_err().contains("fixed"), "{bad}");
        }
        assert!(fixed_address_only("buf[3").is_err());
    }

    #[test]
    fn values_are_decoded_by_their_kind_and_size() {
        let le = |v: u64, n: usize| v.to_le_bytes()[..n].to_vec();
        assert_eq!(decode(Kind::Uint, 4, &le(0x51b, 4)), json!(0x51b));
        assert_eq!(decode(Kind::Uint, 1, &[200]), json!(200));
        assert_eq!(decode(Kind::Int, 1, &[0xff]), json!(-1));
        assert_eq!(decode(Kind::Int, 2, &le(0x8000, 2)), json!(-32768));
        assert_eq!(decode(Kind::Int, 4, &le(0xffff_fff6, 4)), json!(-10));
        assert_eq!(decode(Kind::Int, 8, &le(u64::MAX, 8)), json!(-1));
        // What a JSON number cannot hold exactly travels as text.
        assert_eq!(decode(Kind::Uint, 8, &le(u64::MAX, 8)), json!("18446744073709551615"));
        assert_eq!(decode(Kind::Int, 8, &le(i64::MIN as u64, 8)), json!("-9223372036854775808"));
        assert_eq!(decode(Kind::Ptr, 4, &le(0x2000_0000, 4)), json!(0x2000_0000));
        assert_eq!(decode(Kind::Bool, 1, &[0]), json!(false));
        assert_eq!(decode(Kind::Bool, 1, &[7]), json!(true));
        assert_eq!(decode(Kind::Float, 4, &1.5f32.to_le_bytes()), json!(1.5));
        assert_eq!(decode(Kind::Float, 8, &(-2.25f64).to_le_bytes()), json!(-2.25));
        assert_eq!(decode(Kind::Float, 4, &f32::NAN.to_le_bytes()), json!("NaN"));
        assert_eq!(decode(Kind::Float, 4, &f32::INFINITY.to_le_bytes()), json!("inf"));
        assert_eq!(decode(Kind::Float, 8, &f64::NEG_INFINITY.to_le_bytes()), json!("-inf"));
        // Arrays and structs, odd sizes and a length that is not the size: bytes.
        assert_eq!(decode(Kind::Bytes, 3, &[1, 0xab, 255]), json!("01 ab ff"));
        assert_eq!(decode(Kind::Uint, 12, &[0; 12]), json!("00 00 00 00 00 00 00 00 00 00 00 00"));
        assert_eq!(decode(Kind::Uint, 4, &[1, 2]), json!("01 02"));
    }

    #[test]
    fn a_register_a_read_changes_is_not_read_over_and_over() {
        let svd = super::super::svd::parse(include_str!("testdata/cortex_m_systick.svd")).unwrap();
        // CSR clears COUNTFLAG when read; RVR is an ordinary register.
        let e = peripheral_note(Some(&svd), 0xE000_E010, 4).unwrap_err();
        assert!(e.contains("SYST.CSR") && e.contains("changes the chip"), "{e}");
        assert!(peripheral_note(Some(&svd), 0xE000_E00E, 4).is_err(), "a read that touches its first byte");
        assert_eq!(peripheral_note(Some(&svd), 0xE000_E014, 4).unwrap().as_deref(), Some("SYST.RVR"));
        // Without the register's description, the region is still flagged; RAM and flash are not.
        assert_eq!(peripheral_note(None, 0x5000_0014, 4).unwrap().as_deref(), Some("a peripheral register"));
        assert_eq!(peripheral_note(Some(&svd), 0x4002_0000, 4).unwrap().as_deref(), Some("a peripheral register"));
        assert_eq!(peripheral_note(None, 0x2000_0000, 4).unwrap(), None);
        assert_eq!(peripheral_note(None, 0x0800_0000, 64).unwrap(), None);
    }

    #[test]
    fn the_batch_gdb_gets_the_sessions_own_arguments_without_the_dap_part_and_nothing_from_the_project() {
        let args = resolve_args(&["-q".into(), "-i".into(), "dap".into(), "--readnow".into()], "/p/fw.elf", "a\"b");
        assert_eq!(&args[..2], ["-q", "--readnow"]);
        assert!(!args.contains(&"dap".to_string()) && !args.iter().any(|a| a.starts_with("--interpreter")));
        for need in ["-nx", "-batch", "set auto-load off", "/p/fw.elf"] {
            assert!(args.iter().any(|a| a == need), "{need}: {args:?}");
        }
        // The expression is a string literal, quotes escaped, never part of the code.
        let command = args.last().unwrap();
        assert!(command.starts_with("python WB_EXPR = \"a\\\"b\"; exec("), "{command}");
        assert_eq!(resolve_args(&["--interpreter=dap".into()], "x", "e")[0], "-nx");
    }

    #[test]
    fn gdbs_answer_is_the_address_or_the_reason() {
        let ok = "Reading symbols from x...\nWBLIVE {\"addr\": 536870912, \"size\": 4, \"kind\": \"uint\", \"type\": \"volatile uint32_t\"}\n";
        assert_eq!(parse_resolved(ok, "").unwrap(), Resolved { address: 0x2000_0000, size: 4, kind: Kind::Uint, type_name: "volatile uint32_t".into() });
        let no_symbol = "WBLIVE {\"error\": \"No symbol \\\"nosuch\\\" in current context.\"}";
        assert_eq!(parse_resolved(no_symbol, "").unwrap_err(), "No symbol \"nosuch\" in current context.");
        let memory = "WBLIVE {\"error\": \"Cannot access memory at address 0x20000100\"}";
        assert!(parse_resolved(memory, "").unwrap_err().contains("watch something at a fixed address"));
        assert!(parse_resolved("", "gdb: command not found").unwrap_err().contains("command not found"));
        assert!(parse_resolved("WBLIVE not json", "").is_err());
    }
}
