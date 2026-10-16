//! Plots: the saved layouts the viewer draws. A plot is a few **lanes** stacked under one shared
//! time axis (an oscilloscope's or logic analyzer's rows); a lane holds the traces that share a
//! vertical axis. Power in µA wants its own (log) lane, a rail voltage another, a digital line a
//! thin one of its own, decoded events a row of labels.
//!
//! A plot only *names* series. It can be made before the series has a sample (an agent starts
//! the capture and the plot together), and it keeps working across a clear or a restart.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::series::{Kind, valid_name};
use crate::error::ApiError;

pub const MAX_PLOTS: usize = 40;
pub const MAX_LANES: usize = 32;
pub const MAX_TRACES_PER_LANE: usize = 8;
pub const MAX_TRACES: usize = 64;
pub const MAX_MARKERS: usize = 8;
/// Colours of the chart palette (`--plot-1` … `--plot-8`): analog traces take one each.
pub const COLORS: u8 = 8;
const MAX_ID: usize = 40;
const MAX_TITLE: usize = 60;
const MAX_LANE_TITLE: usize = 60;
/// The longest span a following plot shows, seconds (a day).
pub const MAX_SPAN: f64 = 86_400.0;
pub const MIN_SPAN: f64 = 1.0e-6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Trace {
    pub series: String,
    /// The chart colour, 1–8 (`--plot-N`); 0: none (digital lanes and events are drawn in the text colour).
    pub color: u8,
    /// Left off the chart (it stays in the legend).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// `line` (the default), `step` (hold the value until the next sample: counters, states) or
    /// `dots` (samples only). Empty: the kind decides.
    pub style: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Lane {
    /// Heading at the lane's left edge; empty: the series' names.
    pub title: String,
    pub traces: Vec<Trace>,
    /// Fixed ends of the vertical axis; empty: fitted to what is in view.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// A logarithmic axis (current spans decades between sleep and transmit).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub log: bool,
    /// Height relative to the other lanes; 0: by kind (analog 3, digital 1, events 1).
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PlotConfig {
    pub id: String,
    pub title: String,
    pub lanes: Vec<Lane>,
    /// Seconds in view while following; 0: everything captured.
    pub span: f64,
    /// Keep the newest samples in view.
    pub follow: bool,
    /// Pan and zoom together with the project's other linked plots.
    pub linked: bool,
    /// Events series drawn as vertical lines through every lane.
    pub markers: Vec<String>,
}

impl Default for PlotConfig {
    fn default() -> Self {
        PlotConfig { id: String::new(), title: String::new(), lanes: vec![], span: 10.0, follow: true, linked: true, markers: vec![] }
    }
}

/// A stored list read leniently: a plot that no longer fits (a hand edit, a limit that moved)
/// is left out, not the whole file.
pub fn lenient(list: Vec<Value>) -> Vec<PlotConfig> {
    list.into_iter().filter_map(|v| serde_json::from_value::<PlotConfig>(v).ok()?.validated().ok()).collect()
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= MAX_ID && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A plot id made from a title: lower-case words joined by `-`.
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    let out: String = out.chars().take(MAX_ID).collect();
    if out.is_empty() { "plot".into() } else { out }
}

impl PlotConfig {
    /// The plot as it is stored, or what is wrong with it.
    pub fn validated(mut self) -> Result<PlotConfig, ApiError> {
        let bad = |m: String| Err(ApiError::bad_request(m));
        if !valid_id(&self.id) {
            return bad(format!("a plot id of 1 to {MAX_ID} letters, digits, - or _"));
        }
        self.title = self.title.trim().to_string();
        if self.title.is_empty() {
            self.title = self.id.clone();
        }
        if self.title.chars().count() > MAX_TITLE || self.title.chars().any(char::is_control) {
            return bad(format!("a plot title of at most {MAX_TITLE} characters on one line"));
        }
        if self.lanes.len() > MAX_LANES {
            return bad(format!("at most {MAX_LANES} lanes in a plot"));
        }
        if !(self.span == 0.0 || (MIN_SPAN..=MAX_SPAN).contains(&self.span)) {
            return bad(format!("a span of 0 (everything) or {MIN_SPAN} to {MAX_SPAN} seconds"));
        }
        let mut seen = HashSet::new();
        let mut total = 0;
        for lane in &mut self.lanes {
            lane.title = lane.title.trim().to_string();
            if lane.title.chars().count() > MAX_LANE_TITLE || lane.title.chars().any(char::is_control) {
                return bad(format!("a lane title of at most {MAX_LANE_TITLE} characters on one line"));
            }
            if lane.traces.is_empty() {
                return bad("a lane needs at least one series".into());
            }
            if lane.traces.len() > MAX_TRACES_PER_LANE {
                return bad(format!("at most {MAX_TRACES_PER_LANE} series in a lane"));
            }
            for (v, what) in [(lane.min, "min"), (lane.max, "max")] {
                if v.is_some_and(|x| !x.is_finite()) {
                    return bad(format!("a lane {what} that is a number"));
                }
            }
            if let (Some(lo), Some(hi)) = (lane.min, lane.max) {
                if lo >= hi {
                    return bad("a lane's min must be below its max".into());
                }
            }
            if lane.log && lane.min.is_some_and(|m| m <= 0.0) {
                return bad("a logarithmic lane's min must be above zero".into());
            }
            if !(lane.weight == 0.0 || (0.2..=20.0).contains(&lane.weight)) {
                return bad("a lane weight of 0 (by kind) or 0.2 to 20".into());
            }
            for t in &mut lane.traces {
                t.series = valid_name(&t.series)?;
                if !seen.insert(t.series.clone()) {
                    return bad(format!("{} is in the plot twice", t.series));
                }
                if t.color > COLORS {
                    return bad(format!("{}: a colour from 1 to {COLORS}", t.series));
                }
                if !matches!(t.style.as_str(), "" | "line" | "step" | "dots") {
                    return bad(format!("{}: a style of line, step or dots", t.series));
                }
                total += 1;
            }
        }
        if total > MAX_TRACES {
            return bad(format!("at most {MAX_TRACES} series in a plot"));
        }
        let mut marks = HashSet::new();
        for m in &mut self.markers {
            *m = valid_name(m)?;
            if !marks.insert(m.clone()) {
                return bad(format!("marker series {m} is listed twice"));
            }
        }
        if self.markers.len() > MAX_MARKERS {
            return bad(format!("at most {MAX_MARKERS} marker series"));
        }
        Ok(self)
    }

    /// Series the plot draws, lanes first, then markers.
    pub fn series_names(&self) -> Vec<&str> {
        self.lanes.iter().flat_map(|l| l.traces.iter().map(|t| t.series.as_str())).chain(self.markers.iter().map(String::as_str)).collect()
    }

    /// Give every trace without a colour the next free one among the plot's coloured traces.
    /// Digital and events traces stay uncoloured (the viewer draws them in the text colour);
    /// a series the project does not have yet counts as analog.
    pub fn assign_colors(&mut self, kind_of: &dyn Fn(&str) -> Option<Kind>) {
        let mut used: HashSet<u8> = self.lanes.iter().flat_map(|l| &l.traces).filter(|t| t.color != 0).map(|t| t.color).collect();
        let mut next = 0usize;
        for t in self.lanes.iter_mut().flat_map(|l| l.traces.iter_mut()) {
            if t.color != 0 {
                continue;
            }
            if matches!(kind_of(&t.series), Some(Kind::Digital | Kind::Events)) {
                continue;
            }
            let free = (1..=COLORS).find(|c| !used.contains(c));
            t.color = free.unwrap_or_else(|| {
                // More than eight analog traces: the colours repeat (the legend names each trace).
                next += 1;
                ((next - 1) % COLORS as usize) as u8 + 1
            });
            used.insert(t.color);
        }
    }

    /// What a list of plots shows an agent or a script.
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "lanes": self.lanes.iter().map(|l| json!({
                "title": l.title,
                "series": l.traces.iter().map(|t| t.series.clone()).collect::<Vec<_>>(),
                "min": l.min, "max": l.max, "log": l.log,
            })).collect::<Vec<_>>(),
            "markers": self.markers,
            "span": self.span,
            "follow": self.follow,
            "linked": self.linked,
        })
    }
}

/// Lanes for series listed without any: analog series of one unit share a lane (a unit-less one
/// is alone: nothing says it is comparable), each digital series and each events series gets
/// its own. Analog lanes come first, then digital, then events; within a kind the order given.
/// `info_of` is what the project knows of a name (kind and unit); an unknown series is analog.
pub fn auto_lanes(names: &[String], info_of: &dyn Fn(&str) -> Option<(Kind, String)>) -> Vec<Lane> {
    let mut analog: Vec<(String, Lane)> = vec![];
    let mut digital = vec![];
    let mut events = vec![];
    let lane_of = |n: &str| Lane { traces: vec![Trace { series: n.to_string(), ..Trace::default() }], ..Lane::default() };
    for n in names {
        match info_of(n) {
            Some((Kind::Digital, _)) => digital.push(lane_of(n)),
            Some((Kind::Events, _)) => events.push(lane_of(n)),
            Some((Kind::Analog, unit)) if !unit.is_empty() => {
                match analog.iter_mut().find(|(u, l)| *u == unit && l.traces.len() < MAX_TRACES_PER_LANE) {
                    Some((_, lane)) => lane.traces.push(Trace { series: n.clone(), ..Trace::default() }),
                    None => analog.push((unit, lane_of(n))),
                }
            }
            _ => analog.push((String::new(), lane_of(n))),
        }
    }
    analog.into_iter().map(|(_, l)| l).chain(digital).chain(events).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(s: &str) -> Trace {
        Trace { series: s.into(), ..Trace::default() }
    }

    fn plot(lanes: Vec<Lane>) -> PlotConfig {
        PlotConfig { id: "p".into(), title: "P".into(), lanes, ..PlotConfig::default() }
    }

    fn lane(series: &[&str]) -> Lane {
        Lane { traces: series.iter().map(|s| trace(s)).collect(), ..Lane::default() }
    }

    #[test]
    fn a_valid_plot_passes_and_a_title_defaults_to_the_id() {
        let mut p = plot(vec![lane(&["a", "b"]), lane(&["c"])]);
        p.title = "  ".into();
        let p = p.validated().unwrap();
        assert_eq!(p.title, "p");
        assert_eq!(p.series_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn what_is_wrong_is_said() {
        let e = |p: PlotConfig| p.validated().unwrap_err().message;
        assert!(e(PlotConfig { id: "bad id".into(), ..plot(vec![]) }).contains("plot id"));
        assert!(e(plot(vec![lane(&["a"]), lane(&["a"])])).contains("twice"));
        assert!(e(plot(vec![Lane::default()])).contains("at least one"));
        assert!(e(plot(vec![Lane { min: Some(5.0), max: Some(1.0), ..lane(&["a"]) }])).contains("min"));
        assert!(e(plot(vec![Lane { log: true, min: Some(0.0), ..lane(&["a"]) }])).contains("logarithmic"));
        assert!(e(PlotConfig { span: -1.0, ..plot(vec![]) }).contains("span"));
        assert!(e(plot(vec![Lane { traces: vec![Trace { color: 9, ..trace("a") }], ..Lane::default() }])).contains("colour"));
        assert!(e(plot(vec![Lane { traces: vec![Trace { style: "wavy".into(), ..trace("a") }], ..Lane::default() }])).contains("style"));
        assert!(e(plot((0..33).map(|i| lane(&[&format!("s{i}")])).collect())).contains("lanes"));
        let nine: Vec<&str> = ["a", "b", "c", "d", "e", "f", "g", "h", "i"].to_vec();
        assert!(e(plot(vec![lane(&nine)])).contains("in a lane"));
        assert!(e(PlotConfig { markers: vec!["m".into(), "m".into()], ..plot(vec![]) }).contains("twice"));
    }

    #[test]
    fn ids_come_from_titles() {
        assert_eq!(slug("Power profile (run 3)"), "power-profile-run-3");
        assert_eq!(slug("  ※※  "), "plot");
        assert!(valid_id(&slug(&"x".repeat(200))));
    }

    #[test]
    fn analog_traces_take_the_free_colours_and_digital_ones_none() {
        let mut p = plot(vec![lane(&["a", "clk", "b"]), Lane { traces: vec![Trace { color: 2, ..trace("c") }], ..Lane::default() }]);
        p.assign_colors(&|n| if n == "clk" { Some(Kind::Digital) } else { None });
        let colors: Vec<u8> = p.lanes.iter().flat_map(|l| l.traces.iter().map(|t| t.color)).collect();
        // "c" holds 2; "a" and "b" take the first free ones; the digital line none.
        assert_eq!(colors, vec![1, 0, 3, 2]);
        // Assigned once: a second pass changes nothing.
        let before = p.clone();
        p.assign_colors(&|_| None);
        assert_eq!(p.lanes[0].traces[0], before.lanes[0].traces[0]);
    }

    #[test]
    fn more_than_eight_analog_traces_repeat_the_colours() {
        let names: Vec<String> = (0..10).map(|i| format!("s{i}")).collect();
        let mut p = plot(vec![Lane { traces: names[..8].iter().map(|n| trace(n)).collect(), ..Lane::default() }, Lane { traces: names[8..].iter().map(|n| trace(n)).collect(), ..Lane::default() }]);
        p.assign_colors(&|_| Some(Kind::Analog));
        let c: Vec<u8> = p.lanes.iter().flat_map(|l| l.traces.iter().map(|t| t.color)).collect();
        assert_eq!(&c[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&c[8..], &[1, 2]);
    }

    #[test]
    fn automatic_lanes_group_by_unit_and_kind() {
        let info = |n: &str| match n {
            "i1" | "i2" => Some((Kind::Analog, "A".to_string())),
            "v" => Some((Kind::Analog, "V".to_string())),
            "raw" => Some((Kind::Analog, String::new())),
            "clk" | "mosi" => Some((Kind::Digital, String::new())),
            "marks" => Some((Kind::Events, String::new())),
            _ => None,
        };
        let names: Vec<String> = ["clk", "i1", "marks", "v", "i2", "mosi", "raw", "ghost"].iter().map(|s| s.to_string()).collect();
        let lanes = auto_lanes(&names, &info);
        let shape: Vec<Vec<&str>> = lanes.iter().map(|l| l.traces.iter().map(|t| t.series.as_str()).collect()).collect();
        assert_eq!(shape, vec![vec!["i1", "i2"], vec!["v"], vec!["raw"], vec!["ghost"], vec!["clk"], vec!["mosi"], vec!["marks"]]);
        assert!(plot(lanes).validated().is_ok());
    }

    #[test]
    fn stored_plots_that_no_longer_fit_are_left_out() {
        let good = json!({ "id": "a", "title": "A", "lanes": [{ "traces": [{ "series": "x" }] }] });
        let bad = json!({ "id": "no good", "lanes": [] });
        let junk = json!(5);
        let out = lenient(vec![good, bad, junk]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "a");
        // Defaults fill what an older file lacks.
        assert!(out[0].follow && out[0].linked && out[0].span == 10.0);
    }
}
