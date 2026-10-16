//! Decoders for the files capture tools leave: CSV and tab or space separated tables (sigrok,
//! Saleae, PPK2, Otii and scope exports), JSON lines (what a script prints), and VCD (the logic
//! analyzer and HDL simulator waveform format). Each is **incremental**: lines are fed as they
//! are complete, so a file that is still being written is followed, and one that is finished is
//! imported by the same code.
//!
//! A decoder produces named samples with an optional time; what has none is stamped by the
//! caller (arrival time, or `index / rate`).

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use super::series::{Kind, MAX_NAME, MAX_UNIT, valid_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Jsonl,
    Vcd,
}

impl Format {
    /// From a name, or else the file's extension.
    pub fn parse(name: Option<&str>, path: &str) -> Option<Format> {
        let n = name.map(|s| s.trim().to_ascii_lowercase());
        match n.as_deref() {
            Some("csv" | "tsv" | "table" | "txt" | "text" | "dat") => return Some(Format::Csv),
            Some("jsonl" | "json" | "ndjson" | "jsonlines") => return Some(Format::Jsonl),
            Some("vcd") => return Some(Format::Vcd),
            Some("auto") | None => {}
            Some(_) => return None,
        }
        let ext = path.rsplit('.').next().map(str::to_ascii_lowercase)?;
        match ext.as_str() {
            "csv" | "tsv" | "dat" | "txt" | "log" => Some(Format::Csv),
            "jsonl" | "ndjson" | "json" => Some(Format::Jsonl),
            "vcd" => Some(Format::Vcd),
            _ => None,
        }
    }
}

/// What the sender says about how to read the file.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The time column: a header name or a 1-based column number. Empty: found by its name.
    pub time: Option<String>,
    /// What the time column counts in: `s`, `ms`, `us`, `ns` (default: from the header, else seconds).
    pub time_unit: Option<String>,
    /// Samples per second when the file has no time column: row `n` is at `n / rate`.
    pub rate: Option<f64>,
    /// Put before every series name (`ppk2/`).
    pub prefix: String,
    pub group: String,
    /// Kind of every numeric column (default analog).
    pub kind: Option<Kind>,
    /// Columns (names or 1-based numbers) that are digital, whatever `kind` says.
    pub digital: Vec<String>,
    /// Unit of every column without one in its header.
    pub unit: Option<String>,
    /// Only these columns (names or numbers); empty: all.
    pub columns: Vec<String>,
    /// The field separator (default: found from the first line).
    pub delimiter: Option<char>,
    /// Whether the first line names the columns (default: it does when it holds text).
    pub header: Option<bool>,
}

/// A series the decoder talks about.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesRef {
    pub name: String,
    pub kind: Kind,
    pub unit: String,
}

/// What lines decode to.
#[derive(Debug, Default)]
pub struct Decoded {
    pub series: Vec<SeriesRef>,
    /// (series index, time, value, row): `row` counts the data rows of this batch, so values of
    /// one row without a time of their own get the same one.
    pub samples: Vec<(usize, Option<f64>, f64, usize)>,
    /// (series index, time, text, row).
    pub events: Vec<(usize, Option<f64>, String, usize)>,
    /// Data rows decoded into this batch.
    pub rows: usize,
    index: HashMap<String, usize>,
}

/// Columns, keys and signals one file may have: past these the rest are left out, so a crafted file
/// cannot make a decoder's work grow with its width.
pub const MAX_SERIES_PER_FILE: usize = 256;
const MAX_VCD_VARS: usize = 4096;
const MAX_VCD_COMMAND: usize = 4096;

impl Decoded {
    fn series_index(&mut self, name: &str, kind: Kind, unit: &str) -> usize {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        self.series.push(SeriesRef { name: name.to_string(), kind, unit: unit.to_string() });
        self.index.insert(name.to_string(), self.series.len() - 1);
        self.series.len() - 1
    }
}

pub trait Decoder: Send {
    /// Decode one complete line (without its newline) into `out`.
    fn line(&mut self, line: &str, out: &mut Decoded);
    /// Rows or value changes decoded so far.
    fn rows(&self) -> u64;
}

pub fn decoder(format: Format, opts: Options) -> Box<dyn Decoder> {
    match format {
        Format::Csv => Box::new(Csv::new(opts)),
        Format::Jsonl => Box::new(Jsonl::new(opts)),
        Format::Vcd => Box::new(Vcd::new(opts)),
    }
}

// ---------------------------------------------------------------- shared

fn time_multiplier(unit: &str) -> Option<f64> {
    match unit.trim().to_ascii_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => Some(1.0),
        "ms" | "msec" | "millisecond" | "milliseconds" => Some(1e-3),
        "us" | "µs" | "μs" | "usec" | "microsecond" | "microseconds" => Some(1e-6),
        "ns" | "nsec" | "nanosecond" | "nanoseconds" => Some(1e-9),
        "ps" => Some(1e-12),
        "min" | "minutes" => Some(60.0),
        _ => None,
    }
}

/// `Current [uA]` → (`Current`, `uA`); `time_ms` is the time column's own convention, handled
/// by the caller. No brackets: the whole text, no unit.
fn name_and_unit(header: &str) -> (String, String) {
    let h = header.trim();
    for (open, close) in [('[', ']'), ('(', ')')] {
        if let (Some(a), true) = (h.rfind(open), h.ends_with(close)) {
            if a > 0 {
                let unit = h[a + 1..h.len() - 1].trim();
                let name = h[..a].trim();
                if !name.is_empty() && unit.chars().count() <= MAX_UNIT {
                    return (name.to_string(), unit.to_string());
                }
            }
        }
    }
    (h.to_string(), String::new())
}

fn looks_like_time(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    let base = n.split(['[', '(', '_', ' ']).next().unwrap_or("");
    matches!(base, "time" | "t" | "timestamp" | "ts" | "elapsed" | "seconds" | "sec" | "epoch" | "datetime" | "date")
}

fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(x) = t.parse::<f64>() {
        return Some(x);
    }
    // Hex, as logic analyzers print buses.
    let h = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X"))?;
    u64::from_str_radix(h, 16).ok().map(|x| x as f64)
}

/// A date and time as Unix seconds: RFC 3339, or `YYYY-MM-DD HH:MM:SS[.fff]` taken as UTC.
fn parse_datetime(s: &str) -> Option<f64> {
    let t = s.trim();
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(t) {
        return Some(d.timestamp_millis() as f64 / 1000.0 + f64::from(d.timestamp_subsec_nanos() % 1_000_000) / 1e9);
    }
    for f in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y/%m/%d %H:%M:%S%.f"] {
        if let Ok(d) = chrono::NaiveDateTime::parse_from_str(t, f) {
            let utc = d.and_utc();
            return Some(utc.timestamp() as f64 + f64::from(utc.timestamp_subsec_nanos()) / 1e9);
        }
    }
    None
}

fn bound_name(prefix: &str, name: &str) -> Option<String> {
    let full = format!("{prefix}{}", name.trim());
    let full: String = full.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    valid_name(&full).ok()
}

// ---------------------------------------------------------------- CSV and tables

struct Column {
    unit: String,
    series: Option<String>,
    kind: Kind,
}

pub struct Csv {
    opts: Options,
    delim: Option<Option<char>>,
    names: Option<Vec<Column>>,
    time_col: Option<usize>,
    time_mult: f64,
    time_is_date: bool,
    rows: u64,
}

impl Csv {
    pub fn new(opts: Options) -> Csv {
        Csv { opts, delim: None, names: None, time_col: None, time_mult: 1.0, time_is_date: false, rows: 0 }
    }

    fn split(&self, line: &str) -> Vec<String> {
        split_fields(line, self.delim.flatten())
    }

    fn pick(&self, name: &str, index: usize) -> bool {
        self.opts.columns.is_empty() || self.opts.columns.iter().any(|c| c == name || c.parse::<usize>().ok() == Some(index + 1))
    }

    fn is_digital(&self, name: &str, index: usize) -> bool {
        self.opts.digital.iter().any(|c| c == name || c.parse::<usize>().ok() == Some(index + 1))
    }

    fn set_columns(&mut self, fields: &[String], header: bool) {
        let mut cols = vec![];
        let mut used: HashMap<String, usize> = HashMap::new();
        let mut time_col = None;
        if let Some(t) = &self.opts.time {
            time_col = fields.iter().position(|f| f.trim() == t.trim() || name_and_unit(f).0 == t.trim()).or_else(|| t.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|n| *n < fields.len()));
        } else if header {
            time_col = fields.iter().position(|f| looks_like_time(f));
        }
        for (i, f) in fields.iter().enumerate() {
            let (mut name, mut unit) = if header { name_and_unit(f) } else { (format!("col{}", i + 1), String::new()) };
            if name.is_empty() {
                name = format!("col{}", i + 1);
            }
            if Some(i) == time_col {
                // `time_ms`, `Time (us)`, `t [ns]`: the unit says what the column counts in.
                let suffix = name.rsplit(['_', ' ']).next().and_then(time_multiplier);
                self.time_mult = self.opts.time_unit.as_deref().and_then(time_multiplier).or_else(|| time_multiplier(&unit)).or(suffix).unwrap_or(1.0);
                cols.push(Column { unit, series: None, kind: Kind::Analog });
                continue;
            }
            if unit.is_empty() {
                unit = self.opts.unit.clone().unwrap_or_default();
            }
            let n = used.entry(name.clone()).or_insert(0);
            *n += 1;
            let unique = if *n > 1 { format!("{name}_{n}") } else { name.clone() };
            let kind = if self.is_digital(&name, i) { Kind::Digital } else { self.opts.kind.unwrap_or(Kind::Analog) };
            let series = if i < MAX_SERIES_PER_FILE && self.pick(&name, i) { bound_name(&self.opts.prefix, &unique) } else { None };
            cols.push(Column { unit, series, kind });
        }
        self.time_col = time_col;
        self.names = Some(cols);
    }
}

/// Fields of one line: quoted fields may hold the separator; `None` splits on runs of white space.
pub fn split_fields(line: &str, delim: Option<char>) -> Vec<String> {
    let Some(d) = delim else {
        return line.split_whitespace().map(str::to_string).collect();
    };
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == d && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn find_delimiter(line: &str) -> Option<char> {
    // The commonest separator that appears; none (white space) when none does.
    [',', ';', '\t', '|'].into_iter().map(|c| (c, line.matches(c).count())).filter(|(_, n)| *n > 0).max_by_key(|(_, n)| *n).map(|(c, _)| c)
}

impl Decoder for Csv {
    fn rows(&self) -> u64 {
        self.rows
    }

    fn line(&mut self, line: &str, out: &mut Decoded) {
        let l = line.trim_end_matches('\r');
        let trimmed = l.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            return;
        }
        if self.delim.is_none() {
            self.delim = Some(self.opts.delimiter.or_else(|| find_delimiter(l)));
        }
        let fields = self.split(l);
        if self.names.is_none() {
            let any_text = fields.iter().any(|f| !f.trim().is_empty() && parse_number(f).is_none() && parse_datetime(f).is_none());
            let header = self.opts.header.unwrap_or(any_text);
            self.set_columns(&fields, header);
            if header {
                return;
            }
        }
        let cols = self.names.as_ref().expect("columns set");
        // A header-less file's time column is the first when `time: 1` says so; otherwise none.
        let mut time = None;
        if let Some(c) = self.time_col {
            match fields.get(c).map(|f| f.trim()) {
                Some(f) if !f.is_empty() => {
                    if let Some(x) = parse_number(f) {
                        time = Some(x * self.time_mult);
                    } else if let Some(x) = parse_datetime(f) {
                        self.time_is_date = true;
                        time = Some(x);
                    } else {
                        return;
                    }
                }
                _ => return,
            }
        } else if let Some(r) = self.opts.rate {
            time = Some(self.rows as f64 / r);
        }
        self.rows += 1;
        let row = out.rows;
        out.rows += 1;
        for (i, f) in fields.iter().enumerate() {
            let Some(col) = cols.get(i) else { break };
            let (Some(name), false) = (&col.series, Some(i) == self.time_col) else { continue };
            let f = f.trim();
            if f.is_empty() {
                continue;
            }
            match parse_number(f) {
                Some(v) => {
                    let s = out.series_index(name, col.kind, &col.unit);
                    out.samples.push((s, time, v, row));
                }
                None => match f.to_ascii_lowercase().as_str() {
                    "nan" | "na" | "n/a" | "-" | "--" | "null" => {
                        let s = out.series_index(name, col.kind, &col.unit);
                        out.samples.push((s, time, f64::NAN, row));
                    }
                    "true" | "high" | "h" => {
                        let s = out.series_index(name, Kind::Digital, &col.unit);
                        out.samples.push((s, time, 1.0, row));
                    }
                    "false" | "low" | "l" => {
                        let s = out.series_index(name, Kind::Digital, &col.unit);
                        out.samples.push((s, time, 0.0, row));
                    }
                    // Other text is an annotation: a decoded byte, a state name.
                    _ => {
                        let s = out.series_index(name, Kind::Events, "");
                        out.events.push((s, time, f.to_string(), row));
                    }
                },
            }
        }
    }
}

// ---------------------------------------------------------------- JSON lines

pub struct Jsonl {
    opts: Options,
    rows: u64,
}

impl Jsonl {
    pub fn new(opts: Options) -> Jsonl {
        Jsonl { opts, rows: 0 }
    }
}

impl Decoder for Jsonl {
    fn rows(&self) -> u64 {
        self.rows
    }

    fn line(&mut self, line: &str, out: &mut Decoded) {
        let l = line.trim();
        if l.is_empty() {
            return;
        }
        let Ok(Value::Object(o)) = serde_json::from_str::<Value>(l) else { return };
        let time_key = self.opts.time.as_deref().map(str::to_string).or_else(|| o.keys().find(|k| looks_like_time(k)).cloned());
        let mult = self.opts.time_unit.as_deref().and_then(time_multiplier).unwrap_or(1.0);
        let mut time = time_key.as_ref().and_then(|k| o.get(k)).and_then(|t| match t {
            Value::Number(n) => n.as_f64().map(|x| x * mult),
            Value::String(s) => parse_datetime(s).or_else(|| parse_number(s).map(|x| x * mult)),
            _ => None,
        });
        if time.is_none() {
            if let Some(r) = self.opts.rate {
                time = Some(self.rows as f64 / r);
            }
        }
        self.rows += 1;
        let row = out.rows;
        out.rows += 1;
        // `{"series": "x", "v": 1.5}`: one named sample.
        if let (Some(Value::String(name)), Some(v)) = (o.get("series").or_else(|| o.get("name")), o.get("v").or_else(|| o.get("value"))) {
            if let (Some(n), Some(x)) = (bound_name(&self.opts.prefix, name), as_value(v)) {
                let s = out.series_index(&n, self.opts.kind.unwrap_or(Kind::Analog), self.opts.unit.as_deref().unwrap_or(""));
                out.samples.push((s, time, x, row));
            }
            return;
        }
        for (k, v) in o.iter().take(MAX_SERIES_PER_FILE) {
            if Some(k) == time_key.as_ref() {
                continue;
            }
            if !self.opts.columns.is_empty() && !self.opts.columns.contains(k) {
                continue;
            }
            let Some(name) = bound_name(&self.opts.prefix, k) else { continue };
            match v {
                Value::String(s) if parse_number(s).is_none() => {
                    let i = out.series_index(&name, Kind::Events, "");
                    out.events.push((i, time, s.clone(), row));
                }
                _ => {
                    if let Some(x) = as_value(v) {
                        let kind = if self.opts.digital.contains(k) || v.is_boolean() { Kind::Digital } else { self.opts.kind.unwrap_or(Kind::Analog) };
                        let i = out.series_index(&name, kind, self.opts.unit.as_deref().unwrap_or(""));
                        out.samples.push((i, time, x, row));
                    }
                }
            }
        }
    }
}

fn as_value(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Null => Some(f64::NAN),
        Value::String(s) => parse_number(s),
        _ => None,
    }
}

// ---------------------------------------------------------------- VCD

struct VcdVar {
    name: String,
    width: u32,
    real: bool,
}

pub struct Vcd {
    opts: Options,
    scope: Vec<String>,
    vars: HashMap<String, VcdVar>,
    scale: f64,
    header: String,
    in_header_cmd: bool,
    time: f64,
    changes: u64,
}

impl Vcd {
    pub fn new(opts: Options) -> Vcd {
        Vcd { opts, scope: vec![], vars: HashMap::new(), scale: 1e-9, header: String::new(), in_header_cmd: false, time: 0.0, changes: 0 }
    }

    fn command(&mut self, text: &str) {
        let mut tokens = text.split_whitespace();
        match tokens.next() {
            Some("$timescale") => {
                let rest: Vec<&str> = tokens.filter(|t| *t != "$end").collect();
                let joined = rest.join("");
                let digits: String = joined.chars().take_while(|c| c.is_ascii_digit()).collect();
                let unit = &joined[digits.len()..];
                if let (Ok(n), Some(m)) = (digits.parse::<f64>(), time_multiplier(unit)) {
                    self.scale = n * m;
                }
            }
            Some("$scope") => {
                let _kind = tokens.next();
                if let Some(n) = tokens.next() {
                    self.scope.push(n.to_string());
                }
            }
            Some("$upscope") => {
                self.scope.pop();
            }
            Some("$var") => {
                // $var wire 1 ! CLK [3:0] $end
                let ty = tokens.next().unwrap_or("wire");
                let width: u32 = tokens.next().and_then(|w| w.parse().ok()).unwrap_or(1);
                let (Some(id), Some(name)) = (tokens.next(), tokens.next()) else { return };
                let mut full = self.scope.join(".");
                if !full.is_empty() {
                    full.push('.');
                }
                full.push_str(name);
                if self.vars.len() < MAX_VCD_VARS {
                    self.vars.insert(id.to_string(), VcdVar { name: full, width, real: ty == "real" });
                }
            }
            _ => {}
        }
    }

    fn sample(&mut self, id: &str, value: f64, out: &mut Decoded) {
        let Some(v) = self.vars.get(id) else { return };
        let Some(name) = bound_name(&self.opts.prefix, &v.name) else { return };
        if !self.opts.columns.is_empty() && !self.opts.columns.iter().any(|c| *c == v.name || *c == name) {
            return;
        }
        let kind = if v.real { Kind::Analog } else { Kind::Digital };
        let i = out.series_index(&name, kind, "");
        out.samples.push((i, Some(self.time), value, 0));
        self.changes += 1;
    }
}

impl Decoder for Vcd {
    fn rows(&self) -> u64 {
        self.changes
    }

    fn line(&mut self, line: &str, out: &mut Decoded) {
        let l = line.trim();
        if l.is_empty() {
            return;
        }
        // Header commands run from `$name` to `$end`, possibly over several lines.
        if self.in_header_cmd || l.starts_with("$timescale") || l.starts_with("$scope") || l.starts_with("$upscope") || l.starts_with("$var") {
            if !self.in_header_cmd {
                self.header.clear();
            }
            // A command that never ends is not a header: dropped, not accumulated for the whole file.
            if self.header.len() + l.len() > MAX_VCD_COMMAND {
                self.header.clear();
                self.in_header_cmd = false;
                return;
            }
            self.header.push(' ');
            self.header.push_str(l);
            if l.contains("$end") || l.starts_with("$upscope") {
                let text = std::mem::take(&mut self.header);
                self.command(&text);
                self.in_header_cmd = false;
            } else {
                self.in_header_cmd = true;
            }
            return;
        }
        if l.starts_with('$') {
            // $date, $version, $comment, $enddefinitions, $dumpvars, $end: nothing to take.
            return;
        }
        if let Some(t) = l.strip_prefix('#') {
            if let Ok(n) = t.trim().parse::<f64>() {
                self.time = n * self.scale;
            }
            return;
        }
        let first = l.as_bytes()[0];
        match first {
            b'0' | b'1' | b'x' | b'X' | b'z' | b'Z' | b'u' | b'U' | b'-' | b'w' | b'W' | b'h' | b'H' | b'l' | b'L' => {
                let v = match first {
                    b'1' | b'h' | b'H' => 1.0,
                    b'0' | b'l' | b'L' => 0.0,
                    _ => f64::NAN,
                };
                self.sample(&l[1..], v, out);
            }
            b'b' | b'B' | b'r' | b'R' => {
                let mut parts = l[1..].split_whitespace();
                let (Some(val), Some(id)) = (parts.next(), parts.next()) else { return };
                let v = if matches!(first, b'r' | b'R') {
                    val.parse::<f64>().unwrap_or(f64::NAN)
                } else if val.bytes().all(|c| c == b'0' || c == b'1') && val.len() <= 53 {
                    u64::from_str_radix(val, 2).map(|x| x as f64).unwrap_or(f64::NAN)
                } else {
                    f64::NAN
                };
                let _ = self.vars.get(id).map(|v| v.width);
                self.sample(id, v, out);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(format: Format, opts: Options, text: &str) -> Decoded {
        let mut d = decoder(format, opts);
        let mut out = Decoded::default();
        for l in text.lines() {
            d.line(l, &mut out);
        }
        out
    }

    fn col(d: &Decoded, name: &str) -> Vec<(Option<f64>, f64)> {
        let i = d.series.iter().position(|s| s.name == name).unwrap_or_else(|| panic!("no series {name}: {:?}", d.series));
        d.samples.iter().filter(|s| s.0 == i).map(|s| (s.1, s.2)).collect()
    }

    #[test]
    fn formats_come_from_names_or_extensions() {
        assert_eq!(Format::parse(None, "cap/run.CSV"), Some(Format::Csv));
        assert_eq!(Format::parse(Some("auto"), "x.vcd"), Some(Format::Vcd));
        assert_eq!(Format::parse(Some("ndjson"), "x"), Some(Format::Jsonl));
        assert_eq!(Format::parse(None, "x.bin"), None);
        assert_eq!(Format::parse(Some("excel"), "x.csv"), None);
    }

    #[test]
    fn csv_with_units_in_the_header() {
        let d = run(Format::Csv, Options::default(), "Time [ms],Current (uA),VDD [V]\n0,12.5,3.30\n1,13.0,3.29\n2,900.0,3.10\n");
        assert_eq!(d.series.iter().map(|s| (s.name.as_str(), s.unit.as_str())).collect::<Vec<_>>(), vec![("Current", "uA"), ("VDD", "V")]);
        // The time column is in milliseconds.
        assert_eq!(col(&d, "Current"), vec![(Some(0.0), 12.5), (Some(0.001), 13.0), (Some(0.002), 900.0)]);
    }

    #[test]
    fn csv_without_a_header_or_time_uses_the_rate() {
        let d = run(Format::Csv, Options { rate: Some(1000.0), ..Options::default() }, "1.0\n2.0\n3.0\n");
        assert_eq!(col(&d, "col1"), vec![(Some(0.0), 1.0), (Some(0.001), 2.0), (Some(0.002), 3.0)]);
        // No rate: the caller stamps the arrival.
        let d = run(Format::Csv, Options::default(), "1\n2\n");
        assert!(col(&d, "col1").iter().all(|s| s.0.is_none()));
    }

    #[test]
    fn delimiters_quotes_and_odd_rows() {
        let text = "t;\"a;b\";c\n0;1;2\n1;;4\n#comment\n2;5;nan\n";
        let d = run(Format::Csv, Options::default(), text);
        assert_eq!(col(&d, "a;b"), vec![(Some(0.0), 1.0), (Some(2.0), 5.0)]);
        let c = col(&d, "c");
        assert_eq!(c.len(), 3);
        assert!(c[2].1.is_nan());
        // Tabs and white space.
        let d = run(Format::Csv, Options::default(), "time\tx\n0\t1\n1\t2\n");
        assert_eq!(col(&d, "x").len(), 2);
        let d = run(Format::Csv, Options::default(), "time x y\n0 1 2\n1 3 4\n");
        assert_eq!(col(&d, "y"), vec![(Some(0.0), 2.0), (Some(1.0), 4.0)]);
    }

    #[test]
    fn csv_text_columns_become_events_and_duplicates_are_told_apart() {
        let d = run(Format::Csv, Options::default(), "time,type,v,v\n0,start,1,2\n1,data,3,4\n");
        assert_eq!(d.events.len(), 2);
        assert_eq!(d.series[d.events[0].0].kind, Kind::Events);
        assert_eq!(d.events[0].2, "start");
        assert!(d.series.iter().any(|s| s.name == "v_2"));
    }

    #[test]
    fn csv_options_pick_columns_kinds_and_prefix() {
        let o = Options { digital: vec!["CLK".into()], columns: vec!["CLK".into(), "I".into()], prefix: "la/".into(), unit: Some("A".into()), group: "run".into(), ..Options::default() };
        let d = run(Format::Csv, o, "time,CLK,I,skipped\n0,0,1,9\n1,1,2,9\n");
        let names: Vec<_> = d.series.iter().map(|s| (s.name.clone(), s.kind, s.unit.clone())).collect();
        assert_eq!(names, vec![("la/CLK".to_string(), Kind::Digital, "A".to_string()), ("la/I".to_string(), Kind::Analog, "A".to_string())]);
    }

    #[test]
    fn dates_are_times() {
        let d = run(Format::Csv, Options::default(), "timestamp,v\n2026-10-15T12:00:00.500Z,1\n2026-10-15 12:00:01,2\n");
        let v = col(&d, "v");
        assert_eq!(v.len(), 2);
        assert!((v[1].0.unwrap() - v[0].0.unwrap() - 0.5).abs() < 1e-6);
        assert!(v[0].0.unwrap() > 1.7e9);
    }

    #[test]
    fn json_lines() {
        let text = "{\"t\": 0.5, \"i\": 1.5, \"on\": true, \"state\": \"RX\"}\n{\"t\": 1.0, \"i\": 2.5}\nnot json\n{\"series\": \"x\", \"t\": 2, \"v\": 7}\n";
        let d = run(Format::Jsonl, Options::default(), text);
        assert_eq!(col(&d, "i"), vec![(Some(0.5), 1.5), (Some(1.0), 2.5)]);
        let on = d.series.iter().find(|s| s.name == "on").unwrap();
        assert_eq!(on.kind, Kind::Digital);
        assert_eq!(d.events.len(), 1);
        assert_eq!(col(&d, "x"), vec![(Some(2.0), 7.0)]);
    }

    #[test]
    fn vcd_signals_buses_and_scope_names() {
        let vcd = "$date today $end\n$timescale 10 ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" bus [3:0] $end\n$var real 1 # vref $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\nb0000 \"\nr1.2 #\n$end\n#5\n1!\nb1010 \"\n#10\n0!\nbx1x0 \"\n";
        let d = run(Format::Vcd, Options::default(), vcd);
        let clk = col(&d, "top.clk");
        assert_eq!(clk.len(), 3);
        assert_eq!(clk[1], (Some(50e-9), 1.0));
        let bus = col(&d, "top.bus");
        assert_eq!(bus[1], (Some(50e-9), 10.0));
        assert!(bus[2].1.is_nan(), "unknown bits are a gap");
        let k = |n: &str| d.series.iter().find(|s| s.name == n).unwrap().kind;
        assert_eq!((k("top.clk"), k("top.vref")), (Kind::Digital, Kind::Analog));
    }

    #[test]
    fn a_wide_file_is_cut_at_the_column_limit() {
        let header: Vec<String> = (0..1000).map(|i| format!("c{i}")).collect();
        let row: Vec<String> = (0..1000).map(|i| i.to_string()).collect();
        let text = format!("time,{}\n0,{}\n", header.join(","), row.join(","));
        let d = run(Format::Csv, Options::default(), &text);
        assert_eq!(d.series.len(), MAX_SERIES_PER_FILE - 1, "the time column is one of the columns counted");
        let keys: Vec<String> = (0..1000).map(|i| format!("\"k{i}\": {i}")).collect();
        let d = run(Format::Jsonl, Options::default(), &format!("{{{}}}\n", keys.join(",")));
        assert_eq!(d.series.len(), MAX_SERIES_PER_FILE);
    }

    #[test]
    fn a_command_that_never_ends_does_not_swallow_the_file() {
        let mut vcd = String::from("$comment\n$var wire 1 ! a\n");
        for i in 0..2000 {
            vcd.push_str(&format!("$var wire 1 id{i} sig{i}\n"));
        }
        vcd.push_str("$end\n$var wire 1 z real $end\n#0\n1z\n");
        // Whatever it makes of the unterminated command, it is bounded and the later signal is read.
        let d = run(Format::Vcd, Options::default(), &vcd);
        assert!(d.series.len() <= 1);
    }

    #[test]
    fn a_multi_line_timescale_is_read() {
        let d = run(Format::Vcd, Options::default(), "$timescale\n 1 us\n$end\n$var wire 1 a x $end\n#3\n1a\n");
        assert_eq!(col(&d, "x"), vec![(Some(3e-6), 1.0)]);
    }
}
