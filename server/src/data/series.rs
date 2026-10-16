//! One series: the samples of a single signal, kept in memory in time order.
//!
//! Three kinds share the type:
//! * `analog`: numbers over time (a current, a rail voltage, a probe's waveform);
//! * `digital`: levels over time, stored as transitions only (a repeat of the level is not kept,
//!   so a 1 MHz capture of a quiet line costs nothing) and drawn as steps; a value above 1 is a
//!   bus, shown by its number;
//! * `events`: text at a time (a decoded byte, a trigger, "radio on"): annotations of the timeline.
//!
//! Times are seconds as `f64`: relative to the instrument's start, or the Unix epoch when the
//! sender gave none (anything above [`WALL_CLOCK_MIN`] is read as a date). A series keeps the
//! newest `max_points` samples; older ones fall off the front, so a capture left running rolls
//! instead of growing. Samples must arrive in time order: a sender that starts a new capture
//! clears the series first.
//!
//! Queries return what a plot of a given width needs ([`Series::view`]): the raw samples while
//! they are few, else each pixel column's lowest and highest, so a spike survives however much
//! data there is. [`Series::stats`] answers what a measurement cursor shows.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ApiError;

pub const MAX_NAME: usize = 64;
pub const MAX_UNIT: usize = 16;
pub const MAX_LABEL: usize = 160;
pub const MAX_GROUP: usize = 64;
pub const DEFAULT_MAX_POINTS: usize = 1_000_000;
pub const MAX_POINTS_LIMIT: usize = 10_000_000;
pub const MIN_MAX_POINTS: usize = 100;
pub const DEFAULT_MAX_EVENTS: usize = 20_000;
pub const MAX_EVENT_TEXT: usize = 200;
/// Times above this (about 1973) are Unix seconds: a clock, not a stopwatch.
pub const WALL_CLOCK_MIN: f64 = 1.0e8;
/// Events a view returns at most (more are thinned evenly, and `n` says how many there were).
pub const MAX_VIEW_EVENTS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Analog,
    Digital,
    Events,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "analog" | "analogue" | "number" | "numeric" => Some(Kind::Analog),
            "digital" | "logic" | "bit" | "bus" => Some(Kind::Digital),
            "events" | "event" | "marker" | "markers" | "annotation" | "annotations" | "text" => Some(Kind::Events),
            _ => None,
        }
    }
}

/// What a series is, as the project keeps it (the samples are not persisted).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Meta {
    pub name: String,
    pub kind: Kind,
    /// Of the values: `A`, `mV`, `°C`. Bounded text; the viewer scales it by SI prefixes (µA, mA, A).
    pub unit: String,
    /// A longer description (what is measured, where the probe sits).
    pub label: String,
    /// A heading in the list: `power run 3`, `logic capture`.
    pub group: String,
    /// Seconds added to every time when shown or asked for: lines up instruments whose clocks
    /// started apart (a trigger delay) without rewriting what was captured.
    pub offset: f64,
    /// The newest samples kept (events: the newest events).
    pub max_points: usize,
    /// Samples per second of an evenly sampled stream. Appends of bare `values` continue at the
    /// newest time plus `1 / rate`.
    pub rate: Option<f64>,
}

impl Default for Meta {
    fn default() -> Self {
        Meta { name: String::new(), kind: Kind::Analog, unit: String::new(), label: String::new(), group: String::new(), offset: 0.0, max_points: 0, rate: None }
    }
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}

/// A series name: what plots, tools and the list call it.
pub fn valid_name(name: &str) -> Result<String, ApiError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME || has_control(n) {
        return Err(ApiError::bad_request(format!("a series name of 1 to {MAX_NAME} characters on one line")));
    }
    Ok(n.to_string())
}

/// Bounded one-line text for a unit, label or group.
pub fn clean_text(what: &str, s: &str, max: usize) -> Result<String, ApiError> {
    let t = s.trim();
    if t.chars().count() > max || has_control(t) {
        return Err(ApiError::bad_request(format!("{what} of at most {max} characters on one line")));
    }
    Ok(t.to_string())
}

impl Meta {
    /// Checked and bounded; the one place a series' settings are validated.
    pub fn validated(mut self) -> Result<Meta, ApiError> {
        self.name = valid_name(&self.name)?;
        self.unit = clean_text("a unit", &self.unit, MAX_UNIT)?;
        self.label = clean_text("a label", &self.label, MAX_LABEL)?;
        self.group = clean_text("a group", &self.group, MAX_GROUP)?;
        if !self.offset.is_finite() || self.offset.abs() > 1.0e12 {
            return Err(ApiError::bad_request("an offset in seconds"));
        }
        if let Some(r) = self.rate {
            if !(r.is_finite() && r > 0.0 && r <= 1.0e12) {
                return Err(ApiError::bad_request("a rate in samples per second, above zero"));
            }
        }
        let (default, limit) = match self.kind {
            Kind::Events => (DEFAULT_MAX_EVENTS, 200_000),
            _ => (DEFAULT_MAX_POINTS, MAX_POINTS_LIMIT),
        };
        if self.max_points == 0 {
            self.max_points = default;
        }
        if !(MIN_MAX_POINTS..=limit).contains(&self.max_points) {
            return Err(ApiError::bad_request(format!("maxPoints from {MIN_MAX_POINTS} to {limit} for a {} series", if self.kind == Kind::Events { "events" } else { "numeric" })));
        }
        Ok(self)
    }
}

/// Memory an event costs in units of one numeric sample (a time and a line of text: about 300 bytes against 16).
pub const EVENT_WEIGHT: usize = 16;

/// Whether `t` may be appended to a series that ends at `after`: finite times that never go backwards,
/// within the batch and against what is held. The one rule every append is held to.
pub fn check_times(name: &str, t: impl IntoIterator<Item = f64>, after: Option<f64>) -> Result<(), ApiError> {
    let mut prev = after;
    for x in t {
        if !x.is_finite() {
            return Err(ApiError::bad_request(format!("{name}: a time that is not a number")));
        }
        if let Some(p) = prev {
            if x < p {
                return Err(ApiError::bad_request(format!(
                    "{name}: time goes backwards ({x} after {p}). A new capture starts from an empty series: clear it first (data_series clear, or `clear: true`), or use another series"
                )));
            }
        }
        prev = Some(x);
    }
    Ok(())
}

/// What an append did.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Appended {
    /// Samples (events) taken.
    pub taken: usize,
    /// Older samples that fell off the front to make room.
    pub dropped: usize,
}

pub struct Series {
    pub meta: Meta,
    t: VecDeque<f64>,
    v: VecDeque<f64>,
    ev: VecDeque<(f64, String)>,
    /// The newest time seen, also when a digital series did not keep the sample (a repeated level).
    end: Option<f64>,
    /// The newest value seen.
    last: Option<f64>,
    /// Samples taken since the series was last cleared (including those since dropped).
    pub appended: u64,
    pub dropped: u64,
    /// Bumped by every clear: a viewer that holds an older number refetches.
    pub generation: u64,
    pub updated_ms: i64,
}

impl Series {
    pub fn new(meta: Meta) -> Series {
        Series { meta, t: VecDeque::new(), v: VecDeque::new(), ev: VecDeque::new(), end: None, last: None, appended: 0, dropped: 0, generation: 0, updated_ms: 0 }
    }

    /// Samples stored (events: events).
    pub fn len(&self) -> usize {
        if self.meta.kind == Kind::Events { self.ev.len() } else { self.t.len() }
    }

    /// What the series costs the memory budget, in numeric samples.
    pub fn points(&self) -> usize {
        if self.meta.kind == Kind::Events { self.ev.len() * EVENT_WEIGHT } else { self.t.len() }
    }

    fn first_time(&self) -> Option<f64> {
        if self.meta.kind == Kind::Events { self.ev.front().map(|e| e.0) } else { self.t.front().copied() }
    }

    /// The newest time (raw, before the offset).
    pub fn end_time(&self) -> Option<f64> {
        self.end
    }

    pub fn clear(&mut self) {
        // New buffers, not `clear()`: a deque keeps its capacity, and a capture that grew to millions
        // would hold that memory for ever after.
        self.t = VecDeque::new();
        self.v = VecDeque::new();
        self.ev = VecDeque::new();
        self.end = None;
        self.last = None;
        self.appended = 0;
        self.dropped = 0;
        self.generation += 1;
        self.updated_ms = crate::util::now_ms();
    }

    /// Change the kind: the samples go (they mean something else now).
    pub fn set_kind(&mut self, kind: Kind) {
        if self.meta.kind != kind {
            self.meta.kind = kind;
            self.clear();
        }
    }

    /// Take numeric samples. `t` and `v` have the same length; times never go backwards (within
    /// the batch and against what is stored).
    pub fn push(&mut self, t: &[f64], v: &[f64]) -> Result<Appended, ApiError> {
        if self.meta.kind == Kind::Events {
            return Err(ApiError::bad_request(format!("{} is an events series: append `events` (time and text), not numbers", self.meta.name)));
        }
        debug_assert_eq!(t.len(), v.len());
        check_times(&self.meta.name, t.iter().copied(), self.end)?;
        let digital = self.meta.kind == Kind::Digital;
        let mut taken = 0;
        for (&x, &y) in t.iter().zip(v) {
            let y = if y.is_infinite() { f64::NAN } else { y };
            self.end = Some(x);
            self.last = Some(y);
            taken += 1;
            // A level that repeats says nothing a step does not.
            if digital {
                if let Some(&p) = self.v.back() {
                    if p == y || (p.is_nan() && y.is_nan()) {
                        continue;
                    }
                }
            }
            self.t.push_back(x);
            self.v.push_back(y);
        }
        self.appended += taken as u64;
        self.updated_ms = crate::util::now_ms();
        let mut dropped = 0;
        while self.t.len() > self.meta.max_points {
            self.t.pop_front();
            self.v.pop_front();
            dropped += 1;
        }
        self.dropped += dropped as u64;
        Ok(Appended { taken, dropped })
    }

    /// Take events (times in order, like samples).
    pub fn push_events(&mut self, events: &[(f64, String)]) -> Result<Appended, ApiError> {
        if self.meta.kind != Kind::Events {
            return Err(ApiError::bad_request(format!("{} is not an events series (kind {:?}); give it kind events first", self.meta.name, self.meta.kind)));
        }
        check_times(&self.meta.name, events.iter().map(|e| e.0), self.end)?;
        for (x, text) in events {
            let text: String = text.chars().filter(|c| !c.is_control()).take(MAX_EVENT_TEXT).collect();
            self.ev.push_back((*x, text));
            self.end = Some(*x);
        }
        self.appended += events.len() as u64;
        self.updated_ms = crate::util::now_ms();
        let mut dropped = 0;
        while self.ev.len() > self.meta.max_points {
            self.ev.pop_front();
            dropped += 1;
        }
        self.dropped += dropped as u64;
        Ok(Appended { taken: events.len(), dropped })
    }

    /// Drop the oldest tenth: the memory budget asks for room.
    pub fn shrink(&mut self) -> usize {
        let n = (self.len() / 10).max(1).min(self.len());
        for _ in 0..n {
            if self.meta.kind == Kind::Events {
                self.ev.pop_front();
            } else {
                self.t.pop_front();
                self.v.pop_front();
            }
        }
        self.dropped += n as u64;
        self.t.shrink_to_fit();
        self.v.shrink_to_fit();
        self.ev.shrink_to_fit();
        n
    }

    /// Hold to `max_points` after the limit was lowered.
    pub fn trim_to_max(&mut self) {
        let max = self.meta.max_points;
        let mut dropped = 0;
        while self.t.len() > max {
            self.t.pop_front();
            self.v.pop_front();
            dropped += 1;
        }
        while self.ev.len() > max {
            self.ev.pop_front();
            dropped += 1;
        }
        if dropped > 0 {
            self.dropped += dropped;
            self.t.shrink_to_fit();
            self.v.shrink_to_fit();
            self.ev.shrink_to_fit();
        }
    }

    /// The summary the list shows: settings plus what is held. Times include the offset.
    pub fn info(&self) -> Value {
        let off = self.meta.offset;
        json!({
            "name": self.meta.name,
            "kind": self.meta.kind,
            "unit": self.meta.unit,
            "label": self.meta.label,
            "group": self.meta.group,
            "offset": self.meta.offset,
            "maxPoints": self.meta.max_points,
            "rate": self.meta.rate,
            "count": self.len(),
            "appended": self.appended,
            "dropped": self.dropped,
            "tStart": self.first_time().map(|t| t + off),
            "tEnd": self.end.map(|t| t + off),
            // Times are Unix seconds (a clock), not time since the instrument started.
            "wall": self.end.is_some_and(|t| t + off > WALL_CLOCK_MIN),
            "last": self.last.filter(|v| v.is_finite()),
            "generation": self.generation,
            "updatedAt": self.updated_ms,
        })
    }

    /// The window `[from, to]` (display times) as stored indices: `[lo, hi)` are the samples
    /// inside it.
    fn window(&self, from: f64, to: f64) -> (usize, usize) {
        let (a, b) = (from - self.meta.offset, to - self.meta.offset);
        let lo = self.t.partition_point(|&x| x < a);
        let hi = self.t.partition_point(|&x| x <= b);
        (lo, hi.max(lo))
    }

    /// What a plot `buckets` pixel columns wide shows of `[from, to]`: the samples when they are
    /// no more than two per column, else each column's lowest and highest in time order (and a
    /// failed reading where one fell). One sample before the window comes along, so a line
    /// enters from the left edge and a digital lane starts at the right level, and one after it.
    pub fn view(&self, from: f64, to: f64, buckets: usize) -> View {
        let off = self.meta.offset;
        let len = self.t.len();
        let (lo, hi) = self.window(from, to);
        let n = hi - lo;
        let start = lo.saturating_sub(1);
        let stop = (hi + 1).min(len);
        let mut out = View { n, thinned: false, t: vec![], v: vec![] };
        if buckets == 0 || stop - start <= buckets * 2 {
            out.t.extend(self.t.range(start..stop).map(|x| x + off));
            out.v.extend(self.v.range(start..stop).copied());
            return out;
        }
        out.thinned = true;
        // An open end (no `from` or `to`) is the series' own: buckets span what is held.
        let first = self.t.front().copied().unwrap_or(0.0);
        let last = self.t.back().copied().unwrap_or(first);
        let a = if from.is_finite() { from - off } else { first };
        let b = if to.is_finite() { to - off } else { last };
        let width = (b - a).max(f64::MIN_POSITIVE);
        let mut keep: Vec<usize> = Vec::with_capacity(buckets * 2 + 2);
        if start < lo {
            keep.push(start);
        }
        // Per bucket: the lowest, the highest and the first failed reading, as stored indices. A digital
        // series keeps its first and last as well: the level that leaves a busy column is what the line
        // that follows starts from, and the lowest and highest alone may both be from before it.
        let digital = self.meta.kind == Kind::Digital;
        let mut cur = usize::MAX;
        let (mut mn, mut mx, mut bad) = (usize::MAX, usize::MAX, usize::MAX);
        let (mut first_i, mut last_i) = (usize::MAX, usize::MAX);
        let flush = |keep: &mut Vec<usize>, mn: usize, mx: usize, bad: usize, first_i: usize, last_i: usize| {
            let mut ix = [mn, mx, bad, first_i, last_i];
            ix.sort_unstable();
            let mut last = usize::MAX;
            for i in ix {
                if i != usize::MAX && i != last {
                    keep.push(i);
                    last = i;
                }
            }
        };
        for (k, (&x, &y)) in self.t.range(lo..hi).zip(self.v.range(lo..hi)).enumerate() {
            let i = lo + k;
            let c = (((x - a) / width) * buckets as f64) as usize;
            let c = c.min(buckets - 1);
            if c != cur {
                if cur != usize::MAX {
                    flush(&mut keep, mn, mx, bad, first_i, last_i);
                }
                cur = c;
                mn = usize::MAX;
                mx = usize::MAX;
                bad = usize::MAX;
                first_i = if digital { i } else { usize::MAX };
            }
            if digital {
                last_i = i;
            }
            if y.is_nan() {
                if bad == usize::MAX {
                    bad = i;
                }
                continue;
            }
            if mn == usize::MAX || y < self.v[mn] {
                mn = i;
            }
            if mx == usize::MAX || y > self.v[mx] {
                mx = i;
            }
        }
        if cur != usize::MAX {
            flush(&mut keep, mn, mx, bad, first_i, last_i);
        }
        if stop > hi {
            keep.push(hi);
        }
        out.t.extend(keep.iter().map(|&i| self.t[i] + off));
        out.v.extend(keep.iter().map(|&i| self.v[i]));
        out
    }

    /// The events inside `[from, to]` (display times), at most [`MAX_VIEW_EVENTS`]; the count of all of them.
    pub fn events_in(&self, from: f64, to: f64) -> (Vec<(f64, String)>, usize) {
        let off = self.meta.offset;
        let (a, b) = (from - off, to - off);
        let lo = self.ev.partition_point(|e| e.0 < a);
        let hi = self.ev.partition_point(|e| e.0 <= b);
        let n = hi.saturating_sub(lo);
        // More than a plot can list: every k-th, so the whole window is covered, not its start.
        let stride = n.div_ceil(MAX_VIEW_EVENTS).max(1);
        let out = self.ev.range(lo..hi.max(lo)).step_by(stride).map(|(t, s)| (t + off, s.clone())).collect();
        (out, n)
    }

    /// The raw samples of `[from, to]` for an export or an exact read: at most `limit` (the newest kept).
    pub fn samples_in(&self, from: f64, to: f64, limit: usize) -> (Vec<f64>, Vec<f64>) {
        let off = self.meta.offset;
        let (lo, hi) = self.window(from, to);
        let lo = lo.max(hi.saturating_sub(limit));
        (self.t.range(lo..hi).map(|x| x + off).collect(), self.v.range(lo..hi).copied().collect())
    }

    /// The value in force at `t` (display time): a digital level, or the nearest earlier sample.
    #[cfg(test)]
    pub fn value_at(&self, t: f64) -> Option<f64> {
        let a = t - self.meta.offset;
        let i = self.t.partition_point(|&x| x <= a);
        if i == 0 { None } else { Some(self.v[i - 1]) }
    }

    /// Statistics of `[from, to]` (display times; `None` ends are the series' own).
    pub fn stats(&self, from: Option<f64>, to: Option<f64>) -> Value {
        let off = self.meta.offset;
        match self.meta.kind {
            Kind::Events => {
                // From the stored events, not from `events_in`'s list, which stops at MAX_VIEW_EVENTS.
                let a = from.map_or(f64::NEG_INFINITY, |f| f - off);
                let b = to.map_or(f64::INFINITY, |x| x - off);
                let lo = self.ev.partition_point(|e| e.0 < a);
                let hi = self.ev.partition_point(|e| e.0 <= b).max(lo);
                let at = |i: usize| self.ev.get(i).map(|e| e.0 + off);
                json!({ "kind": "events", "count": hi - lo, "first": if hi > lo { at(lo) } else { None }, "last": if hi > lo { at(hi - 1) } else { None } })
            }
            Kind::Analog => {
                let (lo, hi) = self.window(from.unwrap_or(f64::NEG_INFINITY), to.unwrap_or(f64::INFINITY));
                analog_stats(&self.t, &self.v, lo, hi, off)
            }
            Kind::Digital => digital_stats(&self.t, &self.v, from.map(|f| f - off), to.map(|x| x - off), self.end, off),
        }
    }
}

/// A thinned (or whole) window of samples, in display times.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// Samples the window holds (before thinning).
    pub n: usize,
    pub thinned: bool,
    pub t: Vec<f64>,
    pub v: Vec<f64>,
}

fn number(x: f64) -> Value {
    if x.is_finite() { json!(x) } else { Value::Null }
}

/// Values the percentiles are taken from at most: past this a window is sampled evenly (every
/// k-th value), which keeps a measurement over millions of samples quick and, at this size, is
/// far finer than any display shows. Min, max, mean, RMS and the area are always exact.
const PERCENTILE_SAMPLES: usize = 262_144;

fn analog_stats(t: &VecDeque<f64>, v: &VecDeque<f64>, lo: usize, hi: usize, off: f64) -> Value {
    let stride = (hi - lo).div_ceil(PERCENTILE_SAMPLES).max(1);
    let mut vals: Vec<f64> = Vec::with_capacity((hi - lo) / stride + 1);
    let (mut bad, mut sum, mut sumsq, mut integral) = (0usize, 0.0, 0.0, 0.0);
    let mut prev: Option<(f64, f64)> = None;
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut first, mut last): (Option<(f64, f64)>, Option<(f64, f64)>) = (None, None);
    for i in lo..hi {
        let (x, y) = (t[i], v[i]);
        if !y.is_finite() {
            bad += 1;
            prev = None;
            continue;
        }
        if (i - lo) % stride == 0 {
            vals.push(y);
        }
        sum += y;
        sumsq += y * y;
        min = min.min(y);
        max = max.max(y);
        if let Some((px, py)) = prev {
            integral += (x - px) * (y + py) / 2.0;
        }
        prev = Some((x, y));
        first.get_or_insert((x, y));
        last = Some((x, y));
    }
    let n = (hi - lo) - bad;
    if n == 0 {
        return json!({ "kind": "analog", "count": 0, "failed": bad });
    }
    let mean = sum / n as f64;
    let var = (sumsq / n as f64 - mean * mean).max(0.0);
    let duration = last.zip(first).map(|(l, f)| l.0 - f.0).unwrap_or(0.0);
    vals.sort_by(|a, b| a.total_cmp(b));
    let m = vals.len();
    let pct = |p: f64| vals[(((m - 1) as f64) * p).round() as usize];
    json!({
        "kind": "analog",
        "count": n,
        "failed": bad,
        "from": first.map(|f| f.0 + off),
        "to": last.map(|l| l.0 + off),
        "duration": duration,
        "min": min,
        "max": max,
        "peakToPeak": max - min,
        "mean": mean,
        "rms": (sumsq / n as f64).sqrt(),
        "stddev": var.sqrt(),
        "median": pct(0.5),
        "p95": pct(0.95),
        "p99": pct(0.99),
        // Taken from an even sample of the window when it holds more than PERCENTILE_SAMPLES values.
        "percentilesApproximate": stride > 1,
        // The area under the curve: trapezoids between neighbours (charge for a current in A, energy for a power in W).
        "integral": integral,
        // Time-weighted mean: the integral over the duration (the sample mean when the spacing is uneven differs).
        "timeMean": if duration > 0.0 { number(integral / duration) } else { number(mean) },
        "first": first.map(|f| f.1),
        "last": last.map(|l| l.1),
    })
}

/// Statistics of a digital series over `[from, to]` (stored times; `None`: the capture's own
/// ends). The level is constant from one transition to the next, the last one up to `end`.
fn digital_stats(t: &VecDeque<f64>, v: &VecDeque<f64>, from: Option<f64>, to: Option<f64>, end: Option<f64>, off: f64) -> Value {
    let Some(&newest) = t.back() else { return json!({ "kind": "digital", "count": 0 }) };
    let stop = end.unwrap_or(newest).max(newest);
    let a = from.unwrap_or(t[0]).max(t[0]);
    let b = to.unwrap_or(stop).min(stop);
    if b <= a {
        return json!({ "kind": "digital", "count": 0 });
    }
    // The transition in force at `a`, then every one up to `b`.
    let first = t.partition_point(|&x| x <= a).saturating_sub(1);
    let (mut high, mut low) = (0.0, 0.0);
    let (mut rising, mut falling) = (vec![], vec![]);
    let (mut pulses_hi, mut pulses_lo): (Vec<f64>, Vec<f64>) = (vec![], vec![]);
    let mut values = std::collections::BTreeSet::new();
    let mut last_edge: Option<f64> = None;
    let mut edges = 0usize;
    for i in first..t.len() {
        if t[i] > b {
            break;
        }
        let y = v[i];
        let end_of = if i + 1 < t.len() { t[i + 1] } else { stop }.min(b);
        let width = end_of - t[i].max(a);
        if y.is_finite() {
            values.insert(y.to_bits());
            if width > 0.0 {
                if y > 0.0 { high += width } else { low += width }
            }
        }
        // A transition inside the window is an edge; a pulse is complete when both its edges are.
        if i > 0 && t[i] > a {
            edges += 1;
            let p = v[i - 1];
            if p.is_finite() && y.is_finite() {
                if p <= 0.0 && y > 0.0 {
                    rising.push(t[i]);
                } else if p > 0.0 && y <= 0.0 {
                    falling.push(t[i]);
                }
            }
            if let Some(e) = last_edge {
                if p.is_finite() {
                    if p > 0.0 { pulses_hi.push(t[i] - e) } else { pulses_lo.push(t[i] - e) }
                }
            }
            last_edge = Some(t[i]);
        }
    }
    let total = high + low;
    let freq = if rising.len() >= 2 { Some((rising.len() - 1) as f64 / (rising[rising.len() - 1] - rising[0])) } else { None };
    let range = |l: &[f64]| -> (Value, Value) {
        if l.is_empty() {
            (Value::Null, Value::Null)
        } else {
            (number(l.iter().copied().fold(f64::INFINITY, f64::min)), number(l.iter().copied().fold(f64::NEG_INFINITY, f64::max)))
        }
    };
    let (hmin, hmax) = range(&pulses_hi);
    let (lmin, lmax) = range(&pulses_lo);
    json!({
        "kind": "digital",
        "count": edges,
        "from": a + off,
        "to": b + off,
        "duration": b - a,
        "highTime": high,
        "lowTime": low,
        "duty": if total > 0.0 { number(high / total) } else { Value::Null },
        "rising": rising.len(),
        "falling": falling.len(),
        "frequency": freq,
        "period": freq.map(|f| 1.0 / f),
        "minHigh": hmin,
        "maxHigh": hmax,
        "minLow": lmin,
        "maxLow": lmax,
        // A bus: the distinct values it took.
        "distinct": values.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analog(max: usize) -> Series {
        Series::new(Meta { name: "i".into(), max_points: max, ..Meta::default() }.validated().unwrap())
    }

    fn digital() -> Series {
        Series::new(Meta { name: "clk".into(), kind: Kind::Digital, ..Meta::default() }.validated().unwrap())
    }

    #[test]
    fn names_and_settings_are_bounded() {
        assert!(valid_name("  VDD 3V3 ").is_ok_and(|n| n == "VDD 3V3"));
        assert!(valid_name("").is_err() && valid_name("a\nb").is_err());
        assert!(valid_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert!(Meta { name: "a".into(), unit: "x".repeat(40), ..Meta::default() }.validated().is_err());
        assert!(Meta { name: "a".into(), offset: f64::NAN, ..Meta::default() }.validated().is_err());
        assert!(Meta { name: "a".into(), rate: Some(0.0), ..Meta::default() }.validated().is_err());
        assert!(Meta { name: "a".into(), max_points: 5, ..Meta::default() }.validated().is_err());
        let m = Meta { name: "a".into(), ..Meta::default() }.validated().unwrap();
        assert_eq!(m.max_points, DEFAULT_MAX_POINTS);
    }

    #[test]
    fn samples_must_arrive_in_time_order() {
        let mut s = analog(1000);
        s.push(&[0.0, 1.0, 2.0], &[1.0, 2.0, 3.0]).unwrap();
        let e = s.push(&[1.5], &[9.0]).unwrap_err();
        assert!(e.message.contains("goes backwards") && e.message.contains("clear"), "{}", e.message);
        // Nothing of a refused batch is kept.
        assert_eq!(s.len(), 3);
        // Equal times are fine; so is a batch that is itself out of order only at the seam.
        s.push(&[2.0, 2.0], &[4.0, 5.0]).unwrap();
        assert!(s.push(&[3.0, 2.5], &[0.0, 0.0]).is_err());
        assert!(s.push(&[f64::NAN], &[0.0]).is_err());
        s.clear();
        assert_eq!((s.len(), s.generation), (0, 1));
        s.push(&[0.0], &[1.0]).unwrap();
    }

    #[test]
    fn the_oldest_samples_roll_off() {
        let mut s = analog(MIN_MAX_POINTS);
        let t: Vec<f64> = (0..250).map(f64::from).collect();
        let r = s.push(&t, &t).unwrap();
        assert_eq!((r.taken, r.dropped, s.len()), (250, 150, 100));
        assert_eq!(s.info()["tStart"], 150.0);
        assert_eq!(s.info()["appended"], 250);
        assert_eq!(s.info()["dropped"], 150);
    }

    #[test]
    fn a_digital_series_keeps_transitions_only() {
        let mut s = digital();
        let t: Vec<f64> = (0..10).map(f64::from).collect();
        s.push(&t, &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s.info()["appended"], 10);
        // The capture still ends at the last sample, so the final level reaches it.
        assert_eq!(s.end_time(), Some(9.0));
        assert_eq!(s.value_at(4.5), Some(1.0));
        assert_eq!(s.value_at(7.0), Some(0.0));
        assert_eq!(s.value_at(-1.0), None);
    }

    #[test]
    fn a_view_of_few_samples_is_exact_and_brackets_the_window() {
        let mut s = analog(1000);
        let t: Vec<f64> = (0..10).map(f64::from).collect();
        s.push(&t, &t.iter().map(|x| x * 10.0).collect::<Vec<_>>()).unwrap();
        let v = s.view(3.5, 6.5, 100);
        assert!(!v.thinned);
        assert_eq!(v.n, 3);
        // One before and one after the window.
        assert_eq!(v.t, vec![3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(v.v, vec![30.0, 40.0, 50.0, 60.0, 70.0]);
        // Ask for beyond the data: what exists.
        let all = s.view(-100.0, 100.0, 100);
        assert_eq!(all.t.len(), 10);
    }

    #[test]
    fn a_thinned_view_keeps_the_spike() {
        let mut s = analog(MAX_POINTS_LIMIT);
        let n = 100_000;
        let t: Vec<f64> = (0..n).map(|i| i as f64 / 1000.0).collect();
        let mut v = vec![1.0; n];
        v[50_123] = 500.0; // a one-sample spike
        v[70_000] = -40.0;
        s.push(&t, &v).unwrap();
        let view = s.view(0.0, 100.0, 200);
        assert!(view.thinned);
        assert!(view.t.len() <= 2 * 200 + 4, "{}", view.t.len());
        assert!(view.v.iter().any(|&x| x == 500.0) && view.v.iter().any(|&x| x == -40.0));
        assert!(view.t.windows(2).all(|w| w[0] <= w[1]), "time order");
        assert_eq!(view.n, n);
    }

    #[test]
    fn a_thinned_digital_view_keeps_the_level_a_busy_column_ends_on() {
        let mut s = digital();
        // Many edges in each of a few columns, then a long idle stretch at the level the last burst left.
        let mut t = vec![];
        let mut v = vec![];
        let mut level = 0.0;
        for burst in 0..4 {
            for k in 0..7 {
                t.push(f64::from(burst) * 100.0 + f64::from(k) * 0.01);
                level = if level == 0.0 { 1.0 } else { 0.0 };
                v.push(level);
            }
        }
        // 7 edges per burst: the level after each burst is the opposite of before it; then idle.
        t.push(1000.0);
        v.push(level);
        s.push(&t, &v).unwrap();
        let view = s.view(0.0, 1000.0, 3);
        assert!(view.thinned);
        // The last sample kept before the idle stretch carries the level the bursts ended on.
        let last = view.v[view.v.len() - 1];
        assert_eq!(last, level);
        // And the burst columns still show both levels.
        assert!(view.v.iter().any(|&x| x == 0.0) && view.v.iter().any(|&x| x == 1.0));
    }

    #[test]
    fn a_view_without_a_window_spans_the_whole_series() {
        let mut s = analog(MAX_POINTS_LIMIT);
        let t: Vec<f64> = (0..10_000).map(f64::from).collect();
        s.push(&t, &t).unwrap();
        let v = s.view(f64::NEG_INFINITY, f64::INFINITY, 100);
        assert!(v.thinned);
        // About two per bucket, from one end to the other: not the three points a collapsed bucket gives.
        assert!(v.t.len() > 150 && v.t.len() <= 204, "{}", v.t.len());
        assert_eq!((v.t[0], v.t[v.t.len() - 1]), (0.0, 9999.0));
        // One open end.
        let half = s.view(5000.0, f64::INFINITY, 100);
        assert!(half.t.len() > 150, "{}", half.t.len());
        assert!(half.t[0] >= 4999.0);
    }

    #[test]
    fn a_failed_reading_stays_a_gap_in_a_thinned_view() {
        let mut s = analog(MAX_POINTS_LIMIT);
        let t: Vec<f64> = (0..10_000).map(f64::from).collect();
        let mut v = vec![2.0; 10_000];
        v[5_000] = f64::NAN;
        s.push(&t, &v).unwrap();
        let view = s.view(0.0, 10_000.0, 100);
        assert!(view.thinned && view.v.iter().any(|x| x.is_nan()));
    }

    #[test]
    fn the_offset_moves_times_in_and_out() {
        let mut s = Series::new(Meta { name: "a".into(), offset: 100.0, ..Meta::default() }.validated().unwrap());
        s.push(&[0.0, 1.0, 2.0], &[5.0, 6.0, 7.0]).unwrap();
        let v = s.view(100.5, 101.5, 50);
        assert_eq!(v.t, vec![100.0, 101.0, 102.0]);
        assert_eq!(s.info()["tStart"], 100.0);
        assert_eq!(s.stats(Some(100.5), Some(101.5))["count"], 1);
    }

    #[test]
    fn analog_statistics() {
        let mut s = analog(1000);
        // A 0..=100 ramp over 10 s: the mean is 50, the area under it 500.
        let t: Vec<f64> = (0..=100).map(|i| f64::from(i) / 10.0).collect();
        let v: Vec<f64> = (0..=100).map(f64::from).collect();
        s.push(&t, &v).unwrap();
        let st = s.stats(None, None);
        assert_eq!(st["count"], 101);
        assert_eq!((st["min"].as_f64(), st["max"].as_f64(), st["mean"].as_f64()), (Some(0.0), Some(100.0), Some(50.0)));
        assert!((st["integral"].as_f64().unwrap() - 500.0).abs() < 1e-9);
        assert!((st["timeMean"].as_f64().unwrap() - 50.0).abs() < 1e-9);
        assert_eq!(st["median"], 50.0);
        // A window.
        let w = s.stats(Some(2.0), Some(4.0));
        assert_eq!(w["count"], 21);
        assert_eq!(w["min"], 20.0);
        // Nothing in the window.
        assert_eq!(s.stats(Some(50.0), Some(60.0))["count"], 0);
    }

    #[test]
    fn statistics_over_millions_are_exact_where_they_can_be_and_sampled_where_not() {
        let mut s = analog(MAX_POINTS_LIMIT);
        let n = 1_000_000;
        let t: Vec<f64> = (0..n).map(|i| i as f64 * 1e-6).collect();
        let v: Vec<f64> = (0..n).map(|i| (i % 1000) as f64).collect();
        s.push(&t, &v).unwrap();
        let st = s.stats(None, None);
        assert_eq!(st["count"], n);
        assert_eq!(st["percentilesApproximate"], true);
        assert_eq!((st["min"].as_f64(), st["max"].as_f64()), (Some(0.0), Some(999.0)));
        assert!((st["mean"].as_f64().unwrap() - 499.5).abs() < 1e-6);
        // A sample of a sawtooth: the median is within one step of the true 499.5.
        assert!((st["median"].as_f64().unwrap() - 499.5).abs() < 2.0, "{}", st["median"]);
        // A small window is whole, and says so.
        let w = s.stats(Some(0.0), Some(0.0099));
        assert_eq!(w["percentilesApproximate"], false);
    }

    #[test]
    fn digital_statistics_give_frequency_and_duty() {
        let mut s = digital();
        // 1 kHz, 25 % duty, sampled every 50 µs for 10 ms.
        let (mut t, mut v) = (vec![], vec![]);
        for i in 0..=200 {
            let x = f64::from(i) * 50e-6;
            t.push(x);
            v.push(if (x * 1000.0).fract() < 0.25 { 1.0 } else { 0.0 });
        }
        s.push(&t, &v).unwrap();
        let st = s.stats(None, None);
        assert_eq!(st["kind"], "digital");
        let f = st["frequency"].as_f64().unwrap();
        assert!((f - 1000.0).abs() < 1.0, "{f}");
        let d = st["duty"].as_f64().unwrap();
        assert!((d - 0.25).abs() < 0.03, "{d}");
        assert!(st["rising"].as_u64().unwrap() >= 9);
        assert!(st["minHigh"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn events_are_kept_in_order_and_bounded() {
        let mut s = Series::new(Meta { name: "marks".into(), kind: Kind::Events, max_points: 100, ..Meta::default() }.validated().unwrap());
        assert!(s.push(&[0.0], &[1.0]).is_err());
        let evs: Vec<(f64, String)> = (0..150).map(|i| (f64::from(i), format!("tx {i}\u{7}"))).collect();
        let r = s.push_events(&evs).unwrap();
        assert_eq!((r.taken, r.dropped, s.len()), (150, 50, 100));
        let (list, n) = s.events_in(60.0, 70.0);
        assert_eq!((list.len(), n), (11, 11));
        assert_eq!(list[0].1, "tx 60");
        assert!(s.push_events(&[(1.0, "late".into())]).is_err());
        assert!(analog(1000).push_events(&evs).is_err());
        let st = s.stats(None, None);
        assert_eq!(st["count"], 100);
        assert_eq!((st["first"].as_f64(), st["last"].as_f64()), (Some(50.0), Some(149.0)));
        assert_eq!(s.stats(Some(1000.0), None)["first"], Value::Null);
    }

    #[test]
    fn event_statistics_are_whole_beyond_what_a_view_lists() {
        let mut s = Series::new(Meta { name: "e".into(), kind: Kind::Events, max_points: 100_000, ..Meta::default() }.validated().unwrap());
        let evs: Vec<(f64, String)> = (0..5000).map(|i| (f64::from(i), "x".to_string())).collect();
        s.push_events(&evs).unwrap();
        let st = s.stats(None, None);
        assert_eq!((st["count"].as_u64(), st["last"].as_f64()), (Some(5000), Some(4999.0)));
        // The list a view returns is capped and spread over the whole window; the count says how many there were.
        let (list, n) = s.events_in(f64::NEG_INFINITY, f64::INFINITY);
        assert_eq!(n, 5000);
        assert!(list.len() <= MAX_VIEW_EVENTS && list.len() > MAX_VIEW_EVENTS / 2, "{}", list.len());
        assert!(list.last().unwrap().0 > 4990.0, "covers the end of the window, not only its start");
    }

    #[test]
    fn kinds_parse_from_the_words_agents_use() {
        assert_eq!(Kind::parse("Logic"), Some(Kind::Digital));
        assert_eq!(Kind::parse("markers"), Some(Kind::Events));
        assert_eq!(Kind::parse("analog"), Some(Kind::Analog));
        assert_eq!(Kind::parse("waveform"), None);
    }
}
