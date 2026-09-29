//! One PTY process plus the server-side screen mirror it feeds.
//!
//! * The child runs in its own session (portable-pty calls `setsid`), so its pid is
//!   also the session id and the leader's process group. Windows: a Job Object registered
//!   under that pid (`util::os::session`), and a pseudoconsole (ConPTY) instead of a PTY.
//! * Reads and writes block, so each PTY gets dedicated OS threads (reader, writer,
//!   waiter); tokio workers never block on the PTY.
//! * The reader feeds a `vt100` mirror and a broadcast channel **under one lock**, so a
//!   client that snapshots and subscribes under that same lock sees no gap and no
//!   duplicate (`Screen::attach`).
//! * While no client is attached, the reader answers the device queries programs send
//!   at startup (DA1, DA2, DSR, XTVERSION) so a headless agent does not stall waiting.
//!   With a client attached, xterm.js answers and we stay quiet. The pseudoconsole's own
//!   cursor query (Windows, `session::ASKS_CURSOR`) is always answered here and never
//!   reaches a client, which would answer it a second time.
//!
//! * Secret values a spawner declares (`LaunchSpec::redact`) are replaced before the
//!   output reaches the mirror, so no consumer (clients, saved screens, `screen_text`,
//!   MCP) ever sees them.
//! * A screen nobody watches and no process feeds can hibernate: only its snapshot is
//!   kept, and the mirror is rebuilt from it on demand (`Screen::mirror`).
//!
//! The snapshot builder and the session kill (`util::os::session::kill`) are the verified
//! probe code (see docs/ARCHITECTURE.md history): vt100 does not serialize every mode, so `Extra`
//! tracks the ones it drops (focus reporting, other DECSET modes, cursor style, kitty
//! keyboard flags, modifyOtherKeys, title) and the snapshot replays them.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use parking_lot::{MappedMutexGuard, Mutex, MutexGuard};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use super::ExitInfo;
use super::viewers::Viewers;
use crate::util::os::session;

/// Reply to DA1 (`CSI c`): the same answer xterm.js gives, so programs see one terminal
/// whether or not a browser is attached.
const DA1_REPLY: &[u8] = b"\x1b[?1;2c";
/// Reply to DA2 (`CSI > c`), as xterm.js.
const DA2_REPLY: &[u8] = b"\x1b[>0;276;0c";
/// Reply to XTVERSION (`CSI > 0 q`).
const XTVERSION_REPLY: &[u8] = b"\x1bP>|Workbench\x1b\\";

pub const MIN_COLS: u16 = 20;
pub const MAX_COLS: u16 = 500;
pub const MIN_ROWS: u16 = 5;
pub const MAX_ROWS: u16 = 200;

pub fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.clamp(MIN_COLS, MAX_COLS), rows.clamp(MIN_ROWS, MAX_ROWS))
}

// ---------------------------------------------------------------- mirror

/// Modes vt100 0.16 does not track itself, captured through its `Callbacks` trait.
#[derive(Default, Debug, Clone)]
pub struct Extra {
    pub title: Option<String>,
    /// DECSET ?1004
    pub focus_reporting: bool,
    /// DECSET ?2026, transient: never replayed.
    pub sync_output: bool,
    /// Every other DECSET/DECRST private mode vt100 ignores (e.g. ?2031 colour-scheme reports).
    pub private_modes: BTreeMap<u16, bool>,
    /// `CSI > flags u` pushes, `CSI < n u` pops, `CSI = flags u` replaces the top.
    pub kitty_kbd_stack: Vec<u16>,
    /// DECSCUSR `CSI Ps SP q`
    pub cursor_style: Option<u16>,
    /// `CSI > 4 ; n m`
    pub modify_other_keys: Option<u16>,
}

impl Extra {
    /// Forget every input mode (a new process starts in the same mirror).
    pub fn reset_modes(&mut self) {
        let title = self.title.take();
        *self = Extra { title, ..Default::default() };
    }
}

/// DECSET modes vt100 handles itself. It passes the *whole* parameter list to
/// `unhandled_csi` once per unhandled parameter, so these must be skipped there.
const VT100_HANDLED: &[u16] = &[1, 6, 9, 25, 47, 1000, 1002, 1003, 1005, 1006, 1049, 2004];

impl vt100::Callbacks for Extra {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let t: String = String::from_utf8_lossy(title).chars().filter(|c| !c.is_control()).take(200).collect();
        self.title = Some(t);
    }

    fn unhandled_csi(&mut self, _: &mut vt100::Screen, i1: Option<u8>, i2: Option<u8>, params: &[&[u16]], c: char) {
        let p0 = |i: usize| params.get(i).and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, i2, c) {
            (Some(b'?'), None, 'h' | 'l') => {
                let on = c == 'h';
                for &p in params.iter().filter_map(|p| p.first()) {
                    match p {
                        1004 => self.focus_reporting = on,
                        2026 => self.sync_output = on,
                        p if VT100_HANDLED.contains(&p) => {}
                        p => {
                            // Bounded: a hostile program cannot grow this without limit.
                            if self.private_modes.len() < 64 || self.private_modes.contains_key(&p) {
                                self.private_modes.insert(p, on);
                            }
                        }
                    }
                }
            }
            (Some(b'>'), None, 'u') => {
                if self.kitty_kbd_stack.len() < 16 {
                    self.kitty_kbd_stack.push(p0(0));
                }
            }
            (Some(b'<'), None, 'u') => {
                for _ in 0..p0(0).max(1) {
                    self.kitty_kbd_stack.pop();
                }
            }
            (Some(b'='), None, 'u') => {
                let f = p0(0);
                match self.kitty_kbd_stack.last_mut() {
                    Some(top) => *top = f,
                    None => self.kitty_kbd_stack.push(f),
                }
            }
            // DECSCUSR: the space arrives as the first intermediate.
            (Some(b' '), None, 'q') => self.cursor_style = Some(p0(0)),
            (Some(b'>'), None, 'm') if p0(0) == 4 => self.modify_other_keys = Some(p0(1)),
            _ => {}
        }
    }
}

pub type Mirror = vt100::Parser<Extra>;

pub fn new_mirror(rows: u16, cols: u16, scrollback: usize) -> Mirror {
    vt100::Parser::new_with_callbacks(rows, cols, scrollback, Extra::default())
}

/// Bytes a freshly `reset()` terminal needs to reproduce the mirror: scrollback (normal
/// screen only), the visible screen, cursor, SGR and every input mode.
///
/// Call it while holding the mirror lock, and subscribe to live output under the same
/// lock, so nothing is lost or duplicated between the snapshot and the live stream.
pub fn build_snapshot(term: &mut Mirror) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 * 1024);
    let extra = term.callbacks().clone();
    let screen = term.screen_mut();
    let (rows, cols) = screen.size();
    if !screen.alternate_screen() {
        // The history length is only reachable by clamping the view offset.
        screen.set_scrollback(usize::MAX);
        let sb_len = screen.scrollback();
        // Page through history: with offset o, visible row i is history line (len - o) + i.
        let mut start = 0usize;
        while start < sb_len {
            screen.set_scrollback(sb_len - start);
            let take = (sb_len - start).min(rows as usize);
            for (i, line) in screen.rows_formatted(0, cols).take(take).enumerate() {
                out.extend_from_slice(&line);
                out.extend_from_slice(b"\x1b[m");
                // Soft-wrapped rows are full width, so let the terminal wrap them (it keeps
                // reflow information). Wrap state only carries within one page, so break
                // hard at page boundaries.
                if !screen.row_wrapped(i as u16) || i + 1 == take {
                    out.extend_from_slice(b"\r\n");
                }
            }
            start += take;
        }
        screen.set_scrollback(0);
        if sb_len > 0 {
            // Push the tail of the history out of the viewport before ESC[H ESC[J.
            for _ in 0..rows.saturating_sub(1) {
                out.push(b'\n');
            }
        }
    } else {
        out.extend_from_slice(b"\x1b[?1049h");
    }
    // Visible screen, hide-cursor, cursor position, current SGR.
    out.extend_from_slice(&screen.contents_formatted());
    // Application keypad and cursor, bracketed paste, mouse mode and encoding.
    out.extend_from_slice(&screen.input_mode_formatted());
    if extra.focus_reporting {
        out.extend_from_slice(b"\x1b[?1004h");
    }
    for (m, on) in &extra.private_modes {
        if *on {
            out.extend_from_slice(format!("\x1b[?{m}h").as_bytes());
        }
    }
    if let Some(s) = extra.cursor_style {
        out.extend_from_slice(format!("\x1b[{s} q").as_bytes());
    }
    if let Some(&flags) = extra.kitty_kbd_stack.last() {
        out.extend_from_slice(format!("\x1b[>{flags}u").as_bytes());
    }
    if let Some(n) = extra.modify_other_keys {
        out.extend_from_slice(format!("\x1b[>4;{n}m").as_bytes());
    }
    if let Some(t) = &extra.title {
        out.extend_from_slice(format!("\x1b]2;{t}\x07").as_bytes());
    }
    out
}

/// Plain text of the last `max_lines` logical lines (scrollback + screen). Soft-wrapped
/// rows are joined; trailing blank lines are dropped.
pub fn screen_text(term: &mut Mirror, max_lines: usize) -> String {
    let screen = term.screen_mut();
    let (rows, cols) = screen.size();
    let mut physical: Vec<(String, bool)> = vec![];
    if !screen.alternate_screen() {
        screen.set_scrollback(usize::MAX);
        let sb_len = screen.scrollback();
        // Only read as much history as could possibly be needed (wrapping can make one
        // logical line span several rows, so over-read a little).
        let want = max_lines.saturating_mul(2).saturating_add(rows as usize);
        let mut start = sb_len.saturating_sub(want);
        while start < sb_len {
            screen.set_scrollback(sb_len - start);
            let take = (sb_len - start).min(rows as usize);
            for (i, line) in screen.rows(0, cols).take(take).enumerate() {
                physical.push((line, screen.row_wrapped(i as u16)));
            }
            start += take;
        }
        screen.set_scrollback(0);
    }
    for (i, line) in screen.rows(0, cols).enumerate() {
        physical.push((line, screen.row_wrapped(i as u16)));
    }
    let mut logical: Vec<String> = vec![];
    let mut cur = String::new();
    for (line, wrapped) in physical {
        cur.push_str(&line);
        if !wrapped {
            logical.push(std::mem::take(&mut cur).trim_end().to_string());
        }
    }
    if !cur.is_empty() {
        logical.push(cur.trim_end().to_string());
    }
    while logical.last().is_some_and(|l| l.is_empty()) {
        logical.pop();
    }
    let skip = logical.len().saturating_sub(max_lines);
    logical[skip..].join("\n")
}

/// Plain text of the visible screen's last `max_rows` rows, without scrollback (what a
/// dialog at the bottom of a TUI shows now, not what scrolled past). Trailing blank
/// rows are dropped first.
pub fn visible_text(term: &mut Mirror, max_rows: usize) -> String {
    let screen = term.screen();
    let (_, cols) = screen.size();
    let mut rows: Vec<String> = screen.rows(0, cols).map(|l| l.trim_end().to_string()).collect();
    while rows.last().is_some_and(|l| l.is_empty()) {
        rows.pop();
    }
    let skip = rows.len().saturating_sub(max_rows);
    rows[skip..].join("\n")
}

// ---------------------------------------------------------------- device queries

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
    /// `CSI c` / `CSI 0 c`
    Da1,
    /// `CSI > c` / `CSI > 0 c`
    Da2,
    /// `CSI 5 n`
    Status,
    /// `CSI 6 n`
    CursorPosition,
    /// `CSI > q` / `CSI > 0 q`
    XtVersion,
}

/// Finds device queries in a byte stream, across chunk boundaries.
#[derive(Default)]
pub struct QueryScanner {
    state: ScanState,
    params: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Csi,
    /// A CSI too long to be a query: skip to its final byte.
    CsiIgnore,
}

impl QueryScanner {
    /// Scan `chunk`; push `(end, query)` for each query, where `end` is the index just
    /// past the query's final byte in this chunk.
    pub fn scan(&mut self, chunk: &[u8], found: &mut Vec<(usize, Query)>) {
        let mut i = 0;
        while i < chunk.len() {
            let b = chunk[i];
            match self.state {
                ScanState::Ground => {
                    // Fast path: jump to the next ESC.
                    match chunk[i..].iter().position(|&c| c == 0x1b) {
                        Some(off) => {
                            i += off;
                            self.state = ScanState::Esc;
                        }
                        None => return,
                    }
                }
                ScanState::Esc => {
                    self.state = if b == b'[' {
                        self.params.clear();
                        ScanState::Csi
                    } else if b == 0x1b {
                        ScanState::Esc
                    } else {
                        ScanState::Ground
                    };
                }
                ScanState::Csi | ScanState::CsiIgnore => {
                    if (0x40..=0x7e).contains(&b) {
                        if self.state == ScanState::Csi {
                            if let Some(q) = classify(&self.params, b) {
                                found.push((i + 1, q));
                            }
                        }
                        self.state = ScanState::Ground;
                    } else if (0x20..=0x3f).contains(&b) {
                        if self.state == ScanState::Csi {
                            if self.params.len() >= 12 {
                                self.state = ScanState::CsiIgnore;
                            } else {
                                self.params.push(b);
                            }
                        }
                    } else if b == 0x1b {
                        self.state = ScanState::Esc;
                    } else if b < 0x20 {
                        // C0 controls execute inside CSI; keep going.
                    } else {
                        self.state = ScanState::Ground;
                    }
                }
            }
            i += 1;
        }
    }
}

fn classify(params: &[u8], final_byte: u8) -> Option<Query> {
    match (params, final_byte) {
        (b"" | b"0", b'c') => Some(Query::Da1),
        (b">" | b">0", b'c') => Some(Query::Da2),
        (b"5", b'n') => Some(Query::Status),
        (b"6", b'n') => Some(Query::CursorPosition),
        (b">" | b">0", b'q') => Some(Query::XtVersion),
        _ => None,
    }
}

fn reply_for(q: Query, term: &Mirror) -> Vec<u8> {
    match q {
        Query::Da1 => DA1_REPLY.to_vec(),
        Query::Da2 => DA2_REPLY.to_vec(),
        Query::Status => b"\x1b[0n".to_vec(),
        Query::XtVersion => XTVERSION_REPLY.to_vec(),
        Query::CursorPosition => {
            let (r, c) = term.screen().cursor_position();
            format!("\x1b[{};{}R", r + 1, c + 1).into_bytes()
        }
    }
}

// ---------------------------------------------------------------- shared screen

/// The mirror, or the snapshot of a hibernated one.
struct Slot {
    parser: Mirror,
    stash: Option<Stash>,
}

/// A hibernated screen: its snapshot and the size it was drawn at.
struct Stash {
    bytes: Vec<u8>,
    rows: u16,
    cols: u16,
}

impl Slot {
    /// Rebuild the parser of a hibernated screen.
    fn hydrate(&mut self, scrollback: usize) {
        if let Some(st) = self.stash.take() {
            let mut p = new_mirror(st.rows, st.cols, scrollback);
            p.process(&st.bytes);
            self.parser = p;
        }
    }

    /// `(rows, cols)` without rebuilding anything.
    fn dims(&self) -> (u16, u16) {
        match &self.stash {
            Some(st) => (st.rows, st.cols),
            None => self.parser.screen().size(),
        }
    }
}

/// A stand-in parser for a hibernated screen: it holds almost no cells.
fn placeholder() -> Mirror {
    new_mirror(1, 1, 0)
}

/// The part of a terminal that outlives its processes: the mirror, the output fan-out
/// and bookkeeping the reader thread updates.
pub struct Screen {
    mirror: Mutex<Slot>,
    pub out_tx: broadcast::Sender<Bytes>,
    /// WebSocket clients currently attached (they answer device queries themselves).
    pub attached: AtomicUsize,
    /// Output arrived since the last saved snapshot.
    pub dirty: AtomicBool,
    pub last_output_at: AtomicI64,
    /// Bumped when the mirror is replaced (agent restart); clients re-snapshot.
    pub generation: watch::Sender<u64>,
    /// The process allowed to feed the mirror. A reader from an older process stops.
    proc_gen: AtomicU64,
    scrollback: usize,
    /// Views attached over WebSockets and the sizes they asked for.
    pub viewers: Mutex<Viewers>,
    /// The screen size `(cols, rows)`, announced to attached views when it changes.
    pub size_tx: watch::Sender<(u16, u16)>,
}

impl Screen {
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        let (out_tx, _) = broadcast::channel(1024);
        let (generation, _) = watch::channel(1);
        let (size_tx, _) = watch::channel((cols, rows));
        Self {
            mirror: Mutex::new(Slot { parser: new_mirror(rows, cols, scrollback), stash: None }),
            out_tx,
            attached: AtomicUsize::new(0),
            dirty: AtomicBool::new(false),
            last_output_at: AtomicI64::new(0),
            generation,
            proc_gen: AtomicU64::new(0),
            scrollback,
            viewers: Mutex::new(Viewers::default()),
            size_tx,
        }
    }

    /// The mirror, rebuilt first if the screen was hibernated.
    pub fn mirror(&self) -> MappedMutexGuard<'_, Mirror> {
        let sb = self.scrollback;
        MutexGuard::map(self.mirror.lock(), |s| {
            s.hydrate(sb);
            &mut s.parser
        })
    }

    /// Snapshot + live receiver, atomic with respect to the reader thread.
    pub fn attach(&self) -> (Vec<u8>, broadcast::Receiver<Bytes>, (u16, u16)) {
        let mut m = self.mirror();
        let snap = build_snapshot(&mut m);
        let rx = self.out_tx.subscribe();
        let (rows, cols) = m.screen().size();
        (snap, rx, (cols, rows))
    }

    pub fn snapshot(&self) -> Vec<u8> {
        let mut s = self.mirror.lock();
        match &s.stash {
            Some(st) => st.bytes.clone(),
            None => build_snapshot(&mut s.parser),
        }
    }

    /// Feed bytes that did not come from a process (restored screens, separators).
    pub fn feed(&self, data: &[u8]) {
        let mut m = self.mirror();
        m.process(data);
        let _ = self.out_tx.send(Bytes::copy_from_slice(data));
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Replace the mirror with an empty one (a new process takes over) and bump the
    /// generation so attached clients re-snapshot.
    pub fn reset(&self) {
        let mut s = self.mirror.lock();
        let (rows, cols) = s.dims();
        s.parser = new_mirror(rows, cols, self.scrollback);
        s.stash = None;
        self.generation.send_modify(|g| *g += 1);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Keep only the snapshot of a screen no client watches (its process must be gone).
    /// A parser holds its whole scrollback as cells (tens of MB for a wide terminal);
    /// the snapshot is a few KB. `mirror()` rebuilds the parser when it is needed.
    pub fn hibernate(&self) -> bool {
        let mut s = self.mirror.lock();
        // Checked under the lock: a socket counts itself attached before it snapshots.
        if s.stash.is_some() || self.attached.load(Ordering::Acquire) > 0 {
            return false;
        }
        let bytes = build_snapshot(&mut s.parser);
        let (rows, cols) = s.parser.screen().size();
        s.parser = placeholder();
        s.stash = Some(Stash { bytes, rows, cols });
        true
    }

    /// Start hibernated from a saved snapshot (terminals restored at startup).
    pub fn hibernate_with(&self, bytes: Vec<u8>) {
        let mut s = self.mirror.lock();
        let (rows, cols) = s.dims();
        s.parser = placeholder();
        s.stash = Some(Stash { bytes, rows, cols });
    }

    #[cfg(test)]
    pub fn is_hibernated(&self) -> bool {
        self.mirror.lock().stash.is_some()
    }

    /// Hand the mirror to a new process; returns the generation its reader must carry.
    pub fn next_proc_gen(&self) -> u64 {
        let _m = self.mirror.lock();
        self.proc_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn bracketed_paste(&self) -> bool {
        let s = self.mirror.lock();
        s.stash.is_none() && s.parser.screen().bracketed_paste()
    }

    /// `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.mirror.lock().dims();
        (cols, rows)
    }

    /// Resize the mirror (the PTY was resized) and tell attached views.
    pub fn set_size(&self, cols: u16, rows: u16) {
        self.mirror().screen_mut().set_size(rows, cols);
        self.size_tx.send_replace((cols, rows));
    }
}

// ---------------------------------------------------------------- redaction

/// What a secret is replaced with (as in `secrets::redact`).
const MASK: &[u8] = "••••••".as_bytes();
/// Shorter values are not redacted (they would match ordinary output).
const MIN_SECRET: usize = 8;
/// How long the start of a possible secret is held back waiting for the rest.
const HOLD_BACK: Duration = Duration::from_millis(150);

/// Replaces secret values in a byte stream, including values split across reads: a
/// chunk that ends with the beginning of a secret keeps that tail until the next chunk
/// (or `flush`) shows whether the secret follows.
///
/// Where the PTY repaints what programs write (ConPTY, `session::REPAINTS`), a secret
/// written in one piece can reach the reader with escape sequences between its characters
/// (the cursor hidden around each frame, a move to where the next frame goes on): such a
/// secret is masked too, its escape sequences kept after the mask.
pub struct Redactor {
    needles: Vec<Vec<u8>>,
    carry: Vec<u8>,
    across_escapes: bool,
}

/// An unfinished escape sequence at the end of a read is held back up to this length.
const MAX_HELD_ESCAPE: usize = 4096;

impl Redactor {
    /// `None` when there is nothing to redact.
    pub fn new(secrets: impl IntoIterator<Item = Vec<u8>>) -> Option<Self> {
        Self::with_escapes(secrets, session::REPAINTS)
    }

    /// `new`, masking secrets with escape sequences between their characters too when
    /// `across_escapes`.
    fn with_escapes(secrets: impl IntoIterator<Item = Vec<u8>>, across_escapes: bool) -> Option<Self> {
        let mut needles: Vec<Vec<u8>> = secrets.into_iter().filter(|s| s.len() >= MIN_SECRET).collect();
        // Longest first, so a secret that contains another is masked whole.
        needles.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        needles.dedup();
        (!needles.is_empty()).then_some(Redactor { needles, carry: vec![], across_escapes })
    }

    /// Redact `chunk`; returns what can be shown now.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        for n in &self.needles {
            buf = replace_all(&buf, n, MASK);
        }
        let mut hold = self.held_tail(&buf);
        if self.across_escapes && buf.contains(&0x1b) {
            // The text's positions, found again only after a secret was masked.
            let mut pos = text_positions(&buf);
            for n in &self.needles {
                if let Some(masked) = replace_across_escapes(&buf, &pos.0, n, MASK) {
                    buf = masked;
                    pos = text_positions(&buf);
                }
            }
            hold = self.held_tail(&buf).max(self.held_tail_across_escapes(&buf, &pos));
        }
        self.carry = buf.split_off(buf.len() - hold);
        buf
    }

    /// Release a held-back tail (nothing followed it in time, or the stream ended).
    pub fn flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.carry)
    }

    pub fn holding(&self) -> bool {
        !self.carry.is_empty()
    }

    /// Length of the longest tail of `buf` that is a proper prefix of a secret.
    fn held_tail(&self, buf: &[u8]) -> usize {
        let mut best = 0;
        for n in &self.needles {
            let longest = (n.len() - 1).min(buf.len());
            for k in (best + 1..=longest).rev() {
                if buf.ends_with(&n[..k]) {
                    best = k;
                    break;
                }
            }
        }
        best
    }

    /// `held_tail` with escape sequences between the characters: the longest tail whose
    /// text (escape sequences left out) is a proper prefix of a secret, or else an
    /// unfinished escape sequence (what follows decides whether it splits a secret).
    /// `pos` is `text_positions(buf)`.
    fn held_tail_across_escapes(&self, buf: &[u8], pos: &(Vec<usize>, Option<usize>)) -> usize {
        let (text, unfinished) = (&pos.0, pos.1);
        let mut from = unfinished.filter(|at| buf.len() - at <= MAX_HELD_ESCAPE).unwrap_or(buf.len());
        for n in &self.needles {
            let longest = (n.len() - 1).min(text.len());
            for k in (1..=longest).rev() {
                let first = text.len() - k;
                if text[first..].iter().zip(&n[..k]).all(|(at, c)| buf[*at] == *c) {
                    from = from.min(text[first]);
                    break;
                }
            }
        }
        buf.len() - from
    }
}

fn replace_all(hay: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(needle) {
            out.extend_from_slice(with);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

/// `replace_all` for `needle` written with escape sequences between its characters: each
/// occurrence becomes `with`, followed by the escape sequences it contained (they still
/// take effect: a cursor shown again, a colour). `text` is `text_positions(hay).0`; `None`
/// when `needle` does not occur.
fn replace_across_escapes(hay: &[u8], text: &[usize], needle: &[u8], with: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(hay.len());
    let mut copied = 0;
    let mut i = 0;
    while i + needle.len() <= text.len() {
        if !text[i..i + needle.len()].iter().zip(needle).all(|(at, c)| hay[*at] == *c) {
            i += 1;
            continue;
        }
        let (start, end) = (text[i], text[i + needle.len() - 1] + 1);
        out.extend_from_slice(&hay[copied..start]);
        out.extend_from_slice(with);
        // Every byte of the span that is not the needle's is an escape sequence's.
        let mut t = i;
        for at in start..end {
            if t < i + needle.len() && text[t] == at {
                t += 1;
            } else {
                out.push(hay[at]);
            }
        }
        copied = end;
        i += needle.len();
    }
    if copied == 0 {
        return None;
    }
    out.extend_from_slice(&hay[copied..]);
    Some(out)
}

/// Where the bytes of `buf` that are not part of an escape sequence are, and where an
/// unfinished escape sequence at its end starts.
fn text_positions(buf: &[u8]) -> (Vec<usize>, Option<usize>) {
    let mut text = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != 0x1b {
            text.push(i);
            i += 1;
            continue;
        }
        match escape_len(&buf[i..]) {
            Some(n) => i += n,
            None => return (text, Some(i)),
        }
    }
    (text, None)
}

/// Length of the escape sequence `s` starts with (`s[0]` is ESC); `None` when it is
/// unfinished. CSI runs to its final byte; OSC to BEL or ST; DCS, SOS, PM and APC to ST;
/// others to their final byte. An ESC inside a sequence ends it (another one starts).
fn escape_len(s: &[u8]) -> Option<usize> {
    let kind = *s.get(1)?;
    match kind {
        b'[' => {
            for (k, &b) in s.iter().enumerate().skip(2) {
                match b {
                    0x40..=0x7e => return Some(k + 1),
                    // Parameters and intermediates; C0 controls execute and the CSI goes on.
                    0x20..=0x3f => {}
                    0x1b => return Some(k),
                    0x00..=0x1f => {}
                    _ => return Some(k),
                }
            }
            None
        }
        b']' | b'P' | b'X' | b'^' | b'_' => {
            for (k, &b) in s.iter().enumerate().skip(2) {
                if b == 0x07 && kind == b']' {
                    return Some(k + 1);
                }
                if b == 0x1b {
                    return match s.get(k + 1) {
                        Some(b'\\') => Some(k + 2),
                        Some(_) => Some(k),
                        None => None,
                    };
                }
            }
            None
        }
        // Intermediates (`ESC ( B`), then a final byte.
        0x20..=0x2f => s.iter().skip(2).position(|b| !(0x20..=0x2f).contains(b)).map(|k| k + 3),
        // A lone ESC: what follows is not part of it.
        0x00..=0x1f => Some(1),
        _ => Some(2),
    }
}

/// Feeds process output to the screen: the mirror and the broadcast under one lock, and
/// answers to device queries while no client is attached.
struct Feeder {
    screen: Arc<Screen>,
    proc_gen: u64,
    scanner: QueryScanner,
    queries: Vec<(usize, Query)>,
    in_tx: mpsc::Sender<Bytes>,
    /// The PTY's own cursor query is still to come (`session::ASKS_CURSOR`).
    pty_asks_cursor: bool,
}

impl Feeder {
    fn new(screen: Arc<Screen>, proc_gen: u64, in_tx: mpsc::Sender<Bytes>) -> Self {
        Feeder { screen, proc_gen, scanner: QueryScanner::default(), queries: vec![], in_tx, pty_asks_cursor: session::ASKS_CURSOR }
    }

    /// False once a newer process owns the screen (the reader must stop).
    fn feed(&mut self, chunk: &[u8]) -> bool {
        if chunk.is_empty() {
            return true;
        }
        self.queries.clear();
        self.scanner.scan(chunk, &mut self.queries);
        // The pseudoconsole's cursor query, its first: answered here whether or not a client
        // is attached (it may have missed the query in a resync), and cut from what clients
        // get, since xterm.js's answer would reach the program as typed input. One split
        // across reads was partly sent already: only its usual answer then.
        let mut own = None;
        if self.pty_asks_cursor {
            if let Some(i) = self.queries.iter().position(|&(_, q)| q == Query::CursorPosition) {
                self.pty_asks_cursor = false;
                own = Some(i).filter(|&i| chunk[..self.queries[i].0].ends_with(b"\x1b[6n"));
            }
        }
        let mut replies: Vec<Vec<u8>> = vec![];
        {
            let mut s = self.screen.mirror.lock();
            // A process from an older generation must not write into a mirror that now
            // belongs to its successor.
            if self.screen.proc_gen.load(Ordering::Acquire) != self.proc_gen {
                return false;
            }
            s.hydrate(self.screen.scrollback);
            let m = &mut s.parser;
            let headless = self.screen.attached.load(Ordering::Relaxed) == 0;
            if (headless || own.is_some()) && !self.queries.is_empty() {
                // Process up to each query so a cursor report is exact.
                let mut at = 0;
                for (i, &(end, q)) in self.queries.iter().enumerate() {
                    m.process(&chunk[at..end]);
                    at = end;
                    if headless || own == Some(i) {
                        replies.push(reply_for(q, m));
                    }
                }
                m.process(&chunk[at..]);
            } else {
                m.process(chunk);
            }
            let out = match own {
                Some(i) => {
                    let end = self.queries[i].0;
                    Bytes::from([&chunk[..end - 4], &chunk[end..]].concat())
                }
                None => Bytes::copy_from_slice(chunk),
            };
            if !out.is_empty() {
                let _ = self.screen.out_tx.send(out);
            }
        }
        self.screen.dirty.store(true, Ordering::Relaxed);
        self.screen.last_output_at.store(crate::util::now_ms(), Ordering::Relaxed);
        for r in replies {
            let _ = self.in_tx.try_send(Bytes::from(r));
        }
        true
    }
}

// ---------------------------------------------------------------- process

pub struct LaunchSpec {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// Applied on top of the inherited environment; `None` removes the variable.
    pub env: Vec<(String, Option<String>)>,
    pub cols: u16,
    pub rows: u16,
    /// Secret values the process may print; they never reach the screen.
    pub redact: Vec<Vec<u8>>,
}

/// A running PTY child.
pub struct Pty {
    pub pid: i32,
    /// The PTY's master side. Windows: its session also holds it, to close the
    /// pseudoconsole once the session is over (`util::os::session`); `None` from then on.
    master: Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>,
    in_tx: mpsc::Sender<Bytes>,
    /// Set once the waiter thread has seen the leader exit.
    exited: Arc<AtomicBool>,
}

/// What the spawner gets back besides the handle: the leader's exit status, and a signal
/// that the reader thread has drained the PTY (EOF).
pub struct PtyEvents {
    pub exit: oneshot::Receiver<ExitInfo>,
    pub reader_done: oneshot::Receiver<()>,
}

impl Pty {
    /// Start `spec.argv` in a new PTY that feeds `screen`. Blocking (fork/exec): call it
    /// from `spawn_blocking`.
    pub fn spawn(spec: &LaunchSpec, screen: Arc<Screen>, proc_gen: u64) -> anyhow::Result<(Arc<Pty>, PtyEvents)> {
        anyhow::ensure!(!spec.argv.is_empty(), "empty command");
        // Windows: an absolute program, npm shims unwrapped, a batch file only with a path and
        // arguments cmd.exe reads as they are. Unix: unchanged.
        let launch = crate::util::os::exe::launch(spec.argv.clone(), &spec.cwd, &spec.env).map_err(|e| anyhow::anyhow!(e))?;
        let argv = launch.argv;
        let pair = native_pty_system().openpty(PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.cwd(&spec.cwd);
        for k in crate::util::proc::SESSION_ENV_VARS {
            cmd.env_remove(k);
        }
        for (k, v) in &spec.env {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            }
        }
        // After `spec.env`, which cannot undo it (a batch file's, on Windows).
        for (k, v) in launch.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd)?;
        // The parent must not keep the slave open, or the reader never sees EOF.
        drop(pair.slave);
        let pid = child.process_id().map(|p| p as i32).unwrap_or(0);
        let reader = pair.master.try_clone_reader()?;
        // Dropping the writer writes "\n" + VEOF into the PTY: keep it for the session.
        let mut writer = pair.master.take_writer()?;
        let (in_tx, mut in_rx) = mpsc::channel::<Bytes>(512);
        let (exit_tx, exit_rx) = oneshot::channel();
        let (done_tx, done_rx) = oneshot::channel();
        let exited = Arc::new(AtomicBool::new(false));

        // Secrets: the start of one at the end of a read is held back until the next read
        // (or a short wait for more output times out).
        let mut redactor = Redactor::new(spec.redact.iter().cloned());
        let mut output = session::Output::new(reader, &*pair.master, redactor.is_some(), &pid.to_string())?;
        let master = Arc::new(Mutex::new(Some(pair.master)));
        {
            let master = master.clone();
            session::register(pid, move || {
                // Taken first, so resizes do not wait while the pseudoconsole closes.
                let m = master.lock().take();
                drop(m);
            });
        }

        // Reader: blocking reads → mirror + broadcast under one lock.
        {
            let mut feeder = Feeder::new(screen, proc_gen, in_tx.clone());
            std::thread::Builder::new().name(format!("pty-read-{pid}")).spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    if let Some(r) = redactor.as_mut() {
                        if r.holding() && !output.readable_within(HOLD_BACK) {
                            let held = r.flush();
                            if !feeder.feed(&held) {
                                break;
                            }
                        }
                    }
                    let n = match output.read(&mut buf) {
                        Ok(0) => break, // EOF (portable-pty maps EIO to Ok(0))
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    let ok = match redactor.as_mut() {
                        Some(r) => feeder.feed(&r.push(&buf[..n])),
                        None => feeder.feed(&buf[..n]),
                    };
                    if !ok {
                        break;
                    }
                }
                if let Some(r) = redactor.as_mut() {
                    feeder.feed(&r.flush());
                }
                drop(output);
                let _ = done_tx.send(());
            })?;
        }
        // Writer: a full PTY buffer blocks here, never on a tokio worker.
        std::thread::Builder::new().name(format!("pty-write-{pid}")).spawn(move || {
            while let Some(b) = in_rx.blocking_recv() {
                if writer.write_all(&b).and_then(|_| writer.flush()).is_err() {
                    break;
                }
            }
        })?;
        // Waiter: the leader's exit status.
        {
            let exited = exited.clone();
            std::thread::Builder::new().name(format!("pty-wait-{pid}")).spawn(move || {
                let info = match child.wait() {
                    Ok(s) => ExitInfo {
                        code: Some(s.exit_code() as i32),
                        signal: s.signal().map(str::to_owned),
                        at: crate::util::now_ms(),
                    },
                    Err(_) => ExitInfo { code: None, signal: None, at: crate::util::now_ms() },
                };
                exited.store(true, Ordering::Release);
                let _ = exit_tx.send(info);
                // Windows: once nothing of the session runs, the pseudoconsole closes and
                // the reader sees EOF (ConPTY gives none by itself).
                session::leader_exited(pid);
            })?;
        }
        Ok((Arc::new(Pty { pid, master, in_tx, exited }), PtyEvents { exit: exit_rx, reader_done: done_rx }))
    }

    /// Queue bytes for the child. Fails when the input queue is full (the child stopped
    /// reading) or the writer is gone.
    pub fn write(&self, data: Bytes) -> Result<(), &'static str> {
        self.in_tx.try_send(data).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => "terminal input is backed up",
            mpsc::error::TrySendError::Closed(_) => "terminal is not running",
        })
    }

    /// Queue bytes, waiting for room (programmatic sends of large text).
    pub async fn write_wait(&self, data: Bytes) -> Result<(), &'static str> {
        self.in_tx.send(data).await.map_err(|_| "terminal is not running")
    }

    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        // The kernel sends SIGWINCH to the foreground process group (Windows:
        // ResizePseudoConsole).
        match self.master.lock().as_ref() {
            Some(m) => m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }),
            None => anyhow::bail!("the terminal is closed"),
        }
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// Terminate everything in the PTY's session: SIGHUP (and SIGCONT, so stopped jobs
    /// can receive it) to every process group in the session, then SIGKILL whatever is
    /// left after `grace` (Windows: the pseudoconsole closes, then the job ends).
    /// portable-pty's own kill signals the leader only, which leaves background and
    /// HUP-immune jobs behind.
    pub async fn kill(&self, grace: Duration) {
        session::kill(self.pid, grace, || self.has_exited()).await;
    }
}

/// Whether the command line runs the CLI `name`: its program, or the script an
/// interpreter runs (`node …/bin/codex.js`, `bash …/kimi`), is `name`, `name.<ext>` or
/// `name-<suffix>` (Codex's native `codex-x86_64-…` binary), or lies in the CLI's npm
/// package (`package`, e.g. `/@moonshot-ai/kimi-code/` for `node …/dist/main.mjs`).
/// Windows: names without regard to case, `\` separates, and `node.exe` is `node`.
fn runs_cli(cmdline: &[u8], name: &str, package: &str) -> bool {
    use crate::util::os::path;
    let mut args = cmdline.split(|&b| b == 0).map(|a| String::from_utf8_lossy(a).into_owned());
    let base = |a: &str| {
        let b = path::segments(a).last().unwrap_or("");
        if path::CASE_INSENSITIVE { b.to_ascii_lowercase() } else { b.to_string() }
    };
    let named = |a: &str| {
        let b = base(a);
        b == name || b.strip_prefix(name).is_some_and(|rest| rest.starts_with('.') || rest.starts_with('-')) || path::to_slash(Path::new(a)).contains(package)
    };
    let Some(program) = args.next() else { return false };
    if named(&program) {
        return true;
    }
    // The script an interpreter runs (`node …/bin/kimi`), not just any file argument.
    const INTERPRETERS: &[&str] = &["node", "nodejs", "bun", "deno", "bash", "sh", "python", "python3"];
    let interpreter = base(&program);
    let interpreter = interpreter.strip_suffix(std::env::consts::EXE_SUFFIX).unwrap_or(&interpreter);
    INTERPRETERS.contains(&interpreter) && args.next().is_some_and(|script| named(&script))
}

/// Whether a process outside the process sessions `ours` runs the CLI `name` (npm package
/// path `package`) with `cwd` as its working directory (blocking; reads `/proc`, on
/// Windows this user's processes). Used to tell a session file of that CLI running
/// outside Workbench from a hosted session's own.
pub fn cli_running_in(cwd: &Path, name: &str, package: &str, ours: &std::collections::HashSet<i32>) -> bool {
    session::runs_outside(cwd, ours, |cmdline| runs_cli(cmdline, name, package))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay(snapshot: &[u8], rows: u16, cols: u16) -> Mirror {
        let mut p = new_mirror(rows, cols, 5000);
        p.process(snapshot);
        p
    }

    fn history_len(m: &mut Mirror) -> usize {
        let s = m.screen_mut();
        s.set_scrollback(usize::MAX);
        let n = s.scrollback();
        s.set_scrollback(0);
        n
    }

    #[test]
    fn snapshot_round_trips_main_screen_with_scrollback() {
        let mut m = new_mirror(10, 40, 5000);
        for i in 0..130 {
            m.process(format!("\x1b[3{}mline {i}\x1b[0m {}\r\n", i % 7, "x".repeat(i % 60)).as_bytes());
        }
        m.process("wide: 漢字テスト\r\nprompt$ ".as_bytes());
        let snap = build_snapshot(&mut m);
        let mut r = replay(&snap, 10, 40);
        assert_eq!(history_len(&mut m), history_len(&mut r));
        assert_eq!(m.screen().contents(), r.screen().contents());
        assert_eq!(m.screen().cursor_position(), r.screen().cursor_position());
        // Every history row matches in text and colour.
        let n = history_len(&mut m);
        for off in (1..=n).rev().step_by(10) {
            m.screen_mut().set_scrollback(off);
            r.screen_mut().set_scrollback(off);
            let a: Vec<String> = m.screen().rows(0, 40).collect();
            let b: Vec<String> = r.screen().rows(0, 40).collect();
            assert_eq!(a, b, "history page at offset {off}");
            assert_eq!(m.screen().cell(0, 0).map(|c| c.fgcolor()), r.screen().cell(0, 0).map(|c| c.fgcolor()));
        }
    }

    #[test]
    fn snapshot_round_trips_alt_screen_modes() {
        let mut m = new_mirror(24, 80, 1000);
        m.process(b"shell history\r\n");
        m.process(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?1h\x1b=\x1b[?1004h\x1b[5 q\x1b[>1u\x1b[>4;2m\x1b]2;my-title\x07");
        m.process(b"\x1b[?1000;2031;1004;2004h\x1b[2;5HALT-SCREEN-TEXT\x1b[?25l");
        let snap = build_snapshot(&mut m);
        let r = replay(&snap, 24, 80);
        let (a, b) = (m.screen(), r.screen());
        assert!(b.alternate_screen());
        assert_eq!(b.mouse_protocol_mode(), vt100::MouseProtocolMode::PressRelease);
        assert_eq!(b.mouse_protocol_encoding(), vt100::MouseProtocolEncoding::Sgr);
        assert!(b.bracketed_paste());
        assert!(b.application_cursor());
        assert!(b.application_keypad());
        assert!(b.hide_cursor());
        assert_eq!(a.contents(), b.contents());
        assert_eq!(a.cursor_position(), b.cursor_position());
        let (x, y) = (m.callbacks(), r.callbacks());
        assert!(y.focus_reporting);
        assert_eq!(y.private_modes.get(&2031), Some(&true));
        assert_eq!(y.cursor_style, Some(5));
        assert_eq!(y.kitty_kbd_stack, vec![1]);
        assert_eq!(y.modify_other_keys, Some(2));
        assert_eq!(x.title, y.title);
        assert_eq!(y.title.as_deref(), Some("my-title"));
    }

    #[test]
    fn kitty_stack_push_pop_replace() {
        let mut m = new_mirror(5, 20, 0);
        m.process(b"\x1b[>1u\x1b[>3u\x1b[=7u");
        assert_eq!(m.callbacks().kitty_kbd_stack, vec![1, 7]);
        m.process(b"\x1b[<u");
        assert_eq!(m.callbacks().kitty_kbd_stack, vec![1]);
        m.process(b"\x1b[<5u");
        assert!(m.callbacks().kitty_kbd_stack.is_empty());
    }

    #[test]
    fn screen_text_joins_wrapped_lines_and_limits() {
        let mut m = new_mirror(5, 20, 100);
        for i in 0..30 {
            m.process(format!("row {i}\r\n").as_bytes());
        }
        m.process(format!("{}\r\n", "y".repeat(45)).as_bytes());
        let t = screen_text(&mut m, 3);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines, vec!["row 28", "row 29", "y".repeat(45).as_str()]);
        // The view is back at the bottom afterwards.
        assert_eq!(m.screen().scrollback(), 0);
        // What is on screen now, without what scrolled past.
        assert_eq!(visible_text(&mut m, 2), format!("{}\n{}", "y".repeat(20), "y".repeat(5)));
        let all = visible_text(&mut m, 50);
        assert!(all.contains("row 29") && !all.contains("row 20"), "{all:?}");
    }

    #[test]
    fn clis_are_recognized_by_their_command_line() {
        let kimi = |c: &[u8]| runs_cli(c, "kimi", "/@moonshot-ai/kimi-code/");
        let codex = |c: &[u8]| runs_cli(c, "codex", "/@openai/codex/");
        assert!(kimi(b"node\0/home/u/.npm-global/bin/kimi\0--yolo\0"));
        assert!(kimi(b"node\0/usr/lib/node_modules/@moonshot-ai/kimi-code/dist/main.mjs\0"));
        assert!(kimi(b"bash\0/tmp/t/kimi\0"));
        assert!(codex(b"node\0/usr/lib/node_modules/@openai/codex/bin/codex.js\0resume\0"));
        assert!(codex(b"/usr/lib/node_modules/@openai/codex/vendor/x86_64-unknown-linux-musl/codex/codex\0"));
        assert!(codex(b"/opt/bin/codex-x86_64-unknown-linux-musl\0"));
        // A file named like the CLI is not the CLI.
        assert!(!kimi(b"vim\0kimi-notes.txt\0") && !kimi(b"less\0/w/kimi\0") && !codex(b"bash\0-c\0codex\0"));
        assert!(!kimi(b""));
        #[cfg(windows)]
        {
            assert!(codex(b"C:\\Program Files\\nodejs\\node.exe\0C:\\Users\\u\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\bin\\codex.js\0"));
            assert!(codex(b"C:\\Users\\u\\.local\\bin\\Codex.EXE\0resume\0"));
            assert!(kimi(b"C:\\Python312\\python.exe\0C:\\t\\kimi.py\0"));
            assert!(!kimi(b"C:\\Windows\\notepad.exe\0C:\\t\\kimi-notes.txt\0"));
        }
    }

    #[test]
    fn scanner_finds_queries_across_chunks() {
        let mut s = QueryScanner::default();
        let mut found = vec![];
        s.scan(b"hello\x1b[", &mut found);
        assert!(found.is_empty());
        s.scan(b"c text \x1b[>0q\x1b[6n\x1b[?u \x1b[31m", &mut found);
        assert_eq!(found.iter().map(|f| f.1).collect::<Vec<_>>(), vec![Query::Da1, Query::XtVersion, Query::CursorPosition]);
        assert_eq!(found[0].0, 1);
        let mut found = vec![];
        s.scan(b"\x1b[>c\x1b[5n\x1b[0c\x1b[1;2c\x1b[?6n", &mut found);
        assert_eq!(found.iter().map(|f| f.1).collect::<Vec<_>>(), vec![Query::Da2, Query::Status, Query::Da1]);
    }

    #[test]
    fn cursor_report_uses_position_at_the_query() {
        let mut m = new_mirror(10, 40, 0);
        m.process(b"\x1b[3;7H");
        assert_eq!(reply_for(Query::CursorPosition, &m), b"\x1b[3;7R");
    }

    /// `argv` in a PTY that feeds a new screen, with `secrets` masked.
    fn spawn_argv(argv: Vec<String>, secrets: &[&str]) -> (Arc<Pty>, PtyEvents, Arc<Screen>) {
        let screen = Arc::new(Screen::new(24, 80, 1000));
        let spec = LaunchSpec {
            argv,
            cwd: std::env::temp_dir(),
            env: vec![],
            cols: 80,
            rows: 24,
            redact: secrets.iter().map(|s| s.as_bytes().to_vec()).collect(),
        };
        let gen_ = screen.next_proc_gen();
        let (pty, ev) = Pty::spawn(&spec, screen.clone(), gen_).unwrap();
        (pty, ev, screen)
    }

    /// A bash script (the Unix-only tests).
    #[cfg(unix)]
    fn spawn_sh(script: &str) -> (Arc<Pty>, PtyEvents, Arc<Screen>) {
        spawn_argv(vec!["bash".into(), "-c".into(), script.into()], &[])
    }

    /// A Python program, on every OS (`util::os::exe::python`).
    fn spawn_py(code: &str, secrets: &[&str]) -> (Arc<Pty>, PtyEvents, Arc<Screen>) {
        let mut argv = crate::util::os::exe::python();
        argv.extend(["-c".to_string(), code.to_string()]);
        spawn_argv(argv, secrets)
    }

    #[test]
    fn redactor_masks_secrets_even_split_across_reads() {
        let mut r = Redactor::new([b"glpat-SECRET123".to_vec(), b"short".to_vec()]).unwrap();
        assert_eq!(r.push(b"token is glpat-SECRET123\r\n"), "token is ••••••\r\n".as_bytes());
        // Split in the middle: the start is held back, then masked with the rest.
        let a = r.push(b"again: glpat-SEC");
        assert_eq!(a, b"again: ");
        assert!(r.holding());
        let b = r.push(b"RET123 done");
        assert_eq!(b, "•••••• done".as_bytes());
        // A tail that only looks like the start of a secret comes out on flush.
        assert_eq!(r.push(b"prefix glp"), b"prefix ");
        assert_eq!(r.flush(), b"glp");
        assert!(!r.holding());
        // Values shorter than 8 bytes are never redacted.
        assert_eq!(r.push(b"short"), b"short");
        assert!(Redactor::new([b"tiny".to_vec()]).is_none());
        // Without a repainting PTY, escape sequences between the characters are a break.
        let mut plain = Redactor::with_escapes([b"glpat-SECRET123".to_vec()], false).unwrap();
        assert_eq!(plain.push(b"glpat-SEC\x1b[?25hRET123\n"), b"glpat-SEC\x1b[?25hRET123\n");
    }

    #[test]
    fn a_repainting_pty_cannot_split_a_secret_with_escape_sequences() {
        let mut r = Redactor::with_escapes([b"glpat-SECRET123".to_vec()], true).unwrap();
        // ConPTY paints a frame, hides the cursor for the next one and moves to where it goes on.
        let s = r.push(b"split glpat-SEC\x1b[?25h\x1b[?25l\x1b[5;16HRET123 done\r\n");
        assert_eq!(s, "split ••••••\x1b[?25h\x1b[?25l\x1b[5;16H done\r\n".as_bytes());
        // Across reads: the start and its escape sequences are held back.
        assert_eq!(r.push(b"again glpat-SE\x1b[?25h"), b"again ");
        assert!(r.holding());
        assert_eq!(r.push(b"\x1b[?25lCRET123!"), "••••••\x1b[?25h\x1b[?25l!".as_bytes());
        // An unfinished escape sequence waits for the rest; with none coming, it is flushed.
        assert_eq!(r.push(b"red \x1b[3"), b"red ");
        assert_eq!(r.push(b"1mtext"), b"\x1b[31mtext");
        assert_eq!(r.push(b"\x1b]0;tit"), b"");
        assert_eq!(r.flush(), b"\x1b]0;tit");
        // In a title (OSC), in one piece: masked as before.
        assert_eq!(r.push(b"\x1b]0;glpat-SECRET123\x07ok"), "\x1b]0;••••••\x07ok".as_bytes());
        // A line break is a break: that is not the secret written in one piece.
        assert_eq!(r.push(b"glpat-SEC\r\nRET123\n"), b"glpat-SEC\r\nRET123\n");
        // Escape sequences of every kind are skipped: charset (ESC ( B), keypad (ESC =), OSC with ST.
        assert_eq!(r.push(b"glpat\x1b(B-SE\x1b=CR\x1b]8;;\x1b\\ET123."), "••••••\x1b(B\x1b=\x1b]8;;\x1b\\.".as_bytes());
        assert!(!r.holding());
    }

    #[test]
    fn escape_sequences_are_measured() {
        assert_eq!(escape_len(b"\x1b[?25h rest"), Some(6));
        assert_eq!(escape_len(b"\x1b[38;5;123mx"), Some(11));
        assert_eq!(escape_len(b"\x1b[12"), None);
        assert_eq!(escape_len(b"\x1b[1\x1b[2J"), Some(3));
        assert_eq!(escape_len(b"\x1b]2;title\x07x"), Some(10));
        assert_eq!(escape_len(b"\x1b]2;title\x1b\\x"), Some(11));
        assert_eq!(escape_len(b"\x1b]2;tit"), None);
        assert_eq!(escape_len(b"\x1bP>|x\x1b\\"), Some(7));
        assert_eq!(escape_len(b"\x1b(B"), Some(3));
        assert_eq!(escape_len(b"\x1b("), None);
        assert_eq!(escape_len(b"\x1b7"), Some(2));
        assert_eq!(escape_len(b"\x1b\x1b[m"), Some(1));
        assert_eq!(escape_len(b"\x1b"), None);
        assert_eq!(text_positions(b"a\x1b[mb\x1b[1"), (vec![0, 4], Some(5)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn secrets_never_reach_the_mirror_or_the_stream() {
        let secret = "glpat-FAKEGLOBALTOKEN123456";
        // The second line writes the secret in two pieces, with a pause in between.
        let code = format!(
            "import sys, time\no = sys.stdout\no.write('token is {secret}\\n'); o.flush()\no.write('split {}'); o.flush()\ntime.sleep(0.05)\no.write('{}\\n'); o.write('tail glpat-'); o.flush()\n",
            &secret[..9],
            &secret[9..]
        );
        let (_pty, ev, screen) = spawn_py(&code, &[secret]);
        let mut rx = screen.out_tx.subscribe();
        let _ = tokio::time::timeout(Duration::from_secs(10), ev.exit).await;
        let _ = tokio::time::timeout(Duration::from_secs(5), ev.reader_done).await;
        let text = screen_text(&mut screen.mirror(), 10);
        assert!(!text.contains("FAKEGLOBAL"), "secret on screen: {text:?}");
        assert!(text.contains("token is ••••••") && text.contains("split ••••••"), "{text:?}");
        // A held-back tail that turned out not to be a secret is released at the end.
        assert!(text.contains("tail glpat-"), "{text:?}");
        let mut streamed = Vec::new();
        while let Ok(b) = rx.try_recv() {
            streamed.extend_from_slice(&b);
        }
        assert!(!String::from_utf8_lossy(&streamed).contains("FAKEGLOBAL"));
        assert!(!String::from_utf8_lossy(&screen.snapshot()).contains("FAKEGLOBAL"));
    }

    #[test]
    fn hibernated_screens_keep_their_content_and_size() {
        let screen = Screen::new(10, 40, 1000);
        for i in 0..200 {
            screen.feed(format!("line {i}\r\n").as_bytes());
        }
        screen.feed(b"\x1b]2;kept title\x07prompt$ ");
        let before = screen.snapshot();
        let text_before = screen_text(&mut screen.mirror(), 500);
        assert!(screen.hibernate());
        assert!(screen.is_hibernated());
        assert!(!screen.hibernate(), "already hibernated");
        assert_eq!(screen.size(), (40, 10));
        assert_eq!(screen.snapshot(), before, "a saved snapshot needs no rebuild");
        // Any use of the mirror rebuilds it.
        assert_eq!(screen_text(&mut screen.mirror(), 500), text_before);
        assert!(!screen.is_hibernated());
        assert_eq!(screen.mirror().callbacks().title.as_deref(), Some("kept title"));
        // Never while a client is attached.
        screen.attached.fetch_add(1, Ordering::AcqRel);
        assert!(!screen.hibernate());
        screen.attached.fetch_sub(1, Ordering::AcqRel);
        // Startup: a saved snapshot becomes the stash directly.
        let restored = Screen::new(10, 40, 1000);
        restored.hibernate_with(before);
        assert_eq!(screen_text(&mut restored.mirror(), 500), text_before);
    }

    #[test]
    fn set_size_announces_the_new_size() {
        let screen = Screen::new(24, 80, 100);
        let mut rx = screen.size_tx.subscribe();
        screen.set_size(100, 30);
        assert!(rx.has_changed().unwrap());
        assert_eq!(*rx.borrow_and_update(), (100, 30));
        assert_eq!(screen.size(), (100, 30));
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_takes_down_hup_immune_jobs_in_other_process_groups() {
        // An interactive-style shell with a job-controlled, HUP-immune background job.
        let (pty, ev, _screen) =
            spawn_sh("set -m; (trap '' HUP TERM; exec sleep 1003) & sleep 1004 & echo ready; wait");
        let sid = pty.pid;
        for _ in 0..100 {
            if session::members(sid).len() >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let groups: std::collections::HashSet<i32> = session::members(sid).into_iter().map(|(_, g)| g).collect();
        assert!(groups.len() >= 2, "expected several process groups, got {groups:?}");
        let started = std::time::Instant::now();
        pty.kill(Duration::from_millis(600)).await;
        let info = tokio::time::timeout(Duration::from_secs(5), ev.exit).await.unwrap().unwrap();
        assert!(info.signal.is_some() || info.code.is_some());
        for _ in 0..50 {
            if session::members(sid).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(session::members(sid).is_empty(), "stragglers survived the kill");
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    /// On every OS (Windows: the job and the pseudoconsole): a kill ends what the
    /// terminal's process started, and the output ends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn kill_ends_what_the_session_started() {
        let code = "import subprocess, sys\nc = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(1003)'])\nprint('child', c.pid, flush=True)\nc.wait()\n";
        let (pty, ev, screen) = spawn_py(code, &[]);
        let sid = pty.pid;
        let mut child = None;
        for _ in 0..400 {
            let text = screen_text(&mut screen.mirror(), 10);
            child = text.lines().find_map(|l| l.trim().strip_prefix("child ")).and_then(|p| p.trim().parse::<i32>().ok());
            if child.is_some_and(|c| session::members(sid).iter().any(|(p, _)| *p == c)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let child = child.expect("the child's pid on screen");
        assert!(crate::util::os::proc::pid_alive(child));
        assert!(session::members(sid).iter().any(|(p, _)| *p == child), "{child} not in {:?}", session::members(sid));
        pty.kill(Duration::from_millis(600)).await;
        tokio::time::timeout(Duration::from_secs(10), ev.exit).await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(10), ev.reader_done).await.expect("the output ends").unwrap();
        for _ in 0..200 {
            if !crate::util::os::proc::pid_alive(child) && session::members(sid).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!crate::util::os::proc::pid_alive(child), "the child survived the kill");
        assert!(session::members(sid).is_empty());
    }

    /// Windows: a batch file runs only with arguments cmd.exe reads as they are, and the
    /// output ends once the process is gone (ConPTY gives no EOF by itself).
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn batch_files_run_only_with_arguments_cmd_reads_as_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("tool.cmd");
        std::fs::write(&tool, "@echo args: %*\r\n").unwrap();
        let screen = Arc::new(Screen::new(24, 80, 100));
        let argv = vec![tool.display().to_string(), "a & calc".to_string()];
        let spec = LaunchSpec { argv, cwd: dir.path().to_path_buf(), env: vec![], cols: 80, rows: 24, redact: vec![] };
        let err = Pty::spawn(&spec, screen.clone(), screen.next_proc_gen()).err().expect("refused");
        assert!(err.to_string().contains("batch file"), "{err}");
        let (_pty, ev, screen) = spawn_argv(vec![tool.display().to_string(), "plain".into()], &[]);
        let info = tokio::time::timeout(Duration::from_secs(10), ev.exit).await.unwrap().unwrap();
        assert_eq!(info.code, Some(0));
        tokio::time::timeout(Duration::from_secs(10), ev.reader_done).await.expect("the output ends").unwrap();
        assert!(screen_text(&mut screen.mirror(), 10).contains("args: plain"));
    }

    /// A program's cursor query is answered while no client is attached.
    #[test]
    fn a_cursor_query_is_answered_while_headless() {
        let screen = Arc::new(Screen::new(24, 80, 100));
        let (in_tx, mut in_rx) = mpsc::channel(8);
        let proc_gen = screen.next_proc_gen();
        let mut f = Feeder::new(screen.clone(), proc_gen, in_tx);
        f.pty_asks_cursor = false;
        let mut out = screen.out_tx.subscribe();
        assert!(f.feed(b"\x1b[?9001h\x1b[?1004h\x1b[6n"));
        assert_eq!(&in_rx.try_recv().unwrap()[..], b"\x1b[1;1R");
        assert_eq!(&out.try_recv().unwrap()[..], b"\x1b[?9001h\x1b[?1004h\x1b[6n");
        // Split across reads, after some output.
        assert!(f.feed(b"hello\x1b["));
        assert!(in_rx.try_recv().is_err());
        assert!(f.feed(b"6n"));
        assert_eq!(&in_rx.try_recv().unwrap()[..], b"\x1b[1;6R");
        // An attached client answers itself.
        screen.attached.fetch_add(1, Ordering::AcqRel);
        assert!(f.feed(b"\x1b[6n"));
        assert!(in_rx.try_recv().is_err());
    }

    /// ConPTY, created with INHERIT_CURSOR, asks for the cursor position before anything
    /// else and waits for the answer. The reader gives it even with a client attached (that
    /// client may never see the query), and the client does not get the query, or the
    /// program would read its answer as typed input. Later queries are the client's again.
    #[test]
    fn the_pseudoconsoles_cursor_query_is_answered_here_only() {
        let screen = Arc::new(Screen::new(24, 80, 100));
        let (in_tx, mut in_rx) = mpsc::channel(8);
        let proc_gen = screen.next_proc_gen();
        let mut f = Feeder::new(screen.clone(), proc_gen, in_tx);
        assert_eq!(f.pty_asks_cursor, session::ASKS_CURSOR);
        f.pty_asks_cursor = true;
        screen.feed(b"restarted\r\n");
        let mut out = screen.out_tx.subscribe();
        screen.attached.fetch_add(1, Ordering::AcqRel);
        // What ConPTY writes first: win32-input-mode and focus reports on, then the query.
        assert!(f.feed(b"\x1b[?9001h\x1b[?1004h\x1b[6n"));
        assert_eq!(&in_rx.try_recv().unwrap()[..], b"\x1b[2;1R");
        assert_eq!(&out.try_recv().unwrap()[..], b"\x1b[?9001h\x1b[?1004h");
        assert!(f.feed(b"$ \x1b[6n"));
        assert!(in_rx.try_recv().is_err());
        assert_eq!(&out.try_recv().unwrap()[..], b"$ \x1b[6n");
        // The query alone sends clients nothing.
        f.pty_asks_cursor = true;
        assert!(f.feed(b"\x1b[6n"));
        assert_eq!(&in_rx.try_recv().unwrap()[..], b"\x1b[2;3R");
        assert!(out.try_recv().is_err());
        // Split across reads, its start already sent: the client answers it.
        f.pty_asks_cursor = true;
        assert!(f.feed(b"\x1b["));
        assert!(f.feed(b"6n"));
        assert!(in_rx.try_recv().is_err() && !f.pty_asks_cursor);
        assert_eq!(&out.try_recv().unwrap()[..], b"\x1b[");
        assert_eq!(&out.try_recv().unwrap()[..], b"6n");
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn headless_terminal_answers_device_queries() {
        // `read` gets the DA1 reply because no client is attached.
        let (_pty, ev, screen) = spawn_sh("printf '\\033[c'; IFS= read -rs -d c -t 3 reply; printf 'got:%q\\n' \"$reply\"");
        let _ = tokio::time::timeout(Duration::from_secs(5), ev.exit).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), ev.reader_done).await;
        let text = screen_text(&mut screen.mirror(), 10);
        assert!(text.contains("got:$'\\E[?1;2'"), "unexpected: {text:?}");
    }
}
