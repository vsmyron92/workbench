//! Plots of Live Watch values: the configurations a project keeps (which expressions are drawn
//! together, in which colours, over which span) and the data of a plot read back, for scripts
//! and agents (`GET …/plots/{id}/data`, the `debug_plots` MCP tool).
//!
//! The browser draws the plots (the web viewer, `PlotPanel.tsx`); the server keeps their
//! definitions in the project's debug file, so every browser and device of the user sees the same
//! ones, and the readings behind them (`live::History`), so a page that was reloaded and an agent
//! both have the last minutes to look at. A plot only *names* expressions: the readings come from
//! whichever session of the project watches them.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::live;
use crate::app::AppState;
use crate::error::ApiError;

/// Plots a project keeps.
pub const MAX_PLOTS: usize = 40;
/// Series in one plot: the eight colours of the chart palette (`--plot-1` … `--plot-8`).
pub const MAX_SERIES: usize = 8;
const MAX_NAME: usize = 60;
const MAX_EXPRESSION: usize = 300;
const MAX_ID: usize = 40;
/// The spans a plot offers, in milliseconds (the viewer's `PLOT_WINDOWS`).
pub const WINDOWS_MS: [u64; 7] = [5_000, 10_000, 30_000, 60_000, 300_000, 900_000, 1_800_000];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PlotSeries {
    pub expression: String,
    /// The colour, 1–8; a series keeps it for good.
    pub slot: u8,
    /// Left off the chart (it stays in the legend).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PlotConfig {
    pub id: String,
    pub name: String,
    pub series: Vec<PlotSeries>,
    /// The time span in view, one of `WINDOWS_MS`.
    pub window_ms: u64,
    /// `shared` (one axis, the values' own unit) or `normalized` (every series as a percentage of its own range in view).
    pub scale: String,
}

/// Control characters, and the ones that rearrange or break a line of text (line separators, bidi overrides).
fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}

/// A stored list of plots, read leniently: one that no longer fits (a hand edit, a limit that changed) is left out, where a strict
/// read would make the whole file unreadable and take the project's breakpoints and watches with it.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<PlotConfig>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(list) => list.into_iter().filter_map(|v| serde_json::from_value::<PlotConfig>(v).ok()?.validated().ok()).collect(),
        _ => vec![],
    })
}

impl PlotConfig {
    /// The plot as it is stored, or what is wrong with it. Everything here is text the user typed
    /// (or a browser sent): it is bounded and trimmed, and a colour is used once.
    pub fn validated(mut self) -> Result<PlotConfig, ApiError> {
        let bad = |m: String| Err(ApiError::bad_request(m));
        if self.id.is_empty() || self.id.chars().count() > MAX_ID || !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return bad(format!("a plot id of 1 to {MAX_ID} letters, digits, - or _"));
        }
        self.name = self.name.trim().to_string();
        if self.name.is_empty() || self.name.chars().count() > MAX_NAME || has_control(&self.name) {
            return bad(format!("a plot name of 1 to {MAX_NAME} characters on one line"));
        }
        if self.series.len() > MAX_SERIES {
            return bad(format!("at most {MAX_SERIES} series in a plot: they are the eight colours"));
        }
        let mut slots = HashSet::new();
        let mut seen = HashSet::new();
        for s in &mut self.series {
            s.expression = s.expression.trim().to_string();
            if s.expression.is_empty() || s.expression.chars().count() > MAX_EXPRESSION || has_control(&s.expression) {
                return bad(format!("a series is an expression of 1 to {MAX_EXPRESSION} characters on one line"));
            }
            if !seen.insert(s.expression.clone()) {
                return bad(format!("{} is in the plot twice", s.expression));
            }
            if !(1..=MAX_SERIES as u8).contains(&s.slot) || !slots.insert(s.slot) {
                return bad(format!("{}: a colour from 1 to {MAX_SERIES}, one per series", s.expression));
            }
        }
        if !WINDOWS_MS.contains(&self.window_ms) {
            return bad(format!("a time span of {}", WINDOWS_MS.iter().map(|w| format!("{} s", w / 1000)).collect::<Vec<_>>().join(", ")));
        }
        if self.scale != "shared" && self.scale != "normalized" {
            return bad("a scale of shared or normalized".into());
        }
        Ok(self)
    }

    /// What a plot list shows an agent or a script.
    pub fn summary(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "series": self.series.iter().map(|s| s.expression.clone()).collect::<Vec<_>>(), "windowMs": self.window_ms, "scale": self.scale })
    }
}

/// Tell every browser of the project that its plots changed.
pub fn emit_plots(state: &AppState, pid: &str) {
    let plots = state.debug.store.get(&state.paths.data_dir, pid).plots;
    state.events.emit("debug.plots", Some(pid), json!({ "projectId": pid, "plots": plots }));
}

/// What a plot shows, as data: for each series its statistics over the last `seconds` and the readings, thinned to about
/// `max_points` (each slice keeps its lowest and highest, so a spike survives). The readings come from the project's
/// newest session that can read values; a series that session does not watch says so.
pub fn plot_data(state: &AppState, pid: &str, plot: &PlotConfig, seconds: u64, max_points: usize) -> Value {
    let sessions = state.debug.sessions_of(pid);
    let readers = || sessions.iter().rev().filter(|s| s.live.enabled());
    let session = readers().find(|s| s.is_live()).or_else(|| readers().next());
    let found: Vec<Option<live::Item>> = plot.series.iter().map(|s| session.and_then(|sess| sess.live.find(&s.expression))).collect();
    let readings: Vec<Vec<live::Point>> = found.iter().map(|item| match (session, item) {
        (Some(sess), Some(item)) => sess.live.readings(item.id, None, live::HISTORY_MAX),
        _ => vec![],
    }).collect();
    // The span ends at the newest reading of any series, like the viewer's right edge.
    let until = readings.iter().filter_map(|r| r.last().map(|p| p.t)).max();
    let since = until.map(|u| u - (seconds as i64) * 1000);
    let series: Vec<Value> = plot
        .series
        .iter()
        .zip(found.iter().zip(readings))
        .map(|(s, (item, points))| {
            let inside: Vec<live::Point> = points.into_iter().filter(|p| since.is_none_or(|t| p.t >= t)).collect();
            let thinned = live::decimate(&inside, max_points);
            let mut out = json!({ "expression": s.expression, "slot": s.slot, "hidden": s.hidden, "watched": item.is_some() });
            if let Some(i) = item {
                out["type"] = json!(i.type_name);
                out["stats"] = live::stats(&inside);
            }
            if let Value::Object(data) = live::points_json(&thinned) {
                out.as_object_mut().unwrap().extend(data);
            }
            out
        })
        .collect();
    let mut out = json!({
        "plot": plot.summary(),
        "sessionId": session.map(|s| s.id.clone()),
        "intervalMs": session.map(|s| s.live.snapshot()["intervalMs"].clone()),
        "until": until,
        "seconds": seconds,
        "series": series,
    });
    if session.is_none() {
        out["note"] = json!("no debug session of this project can read values while the program runs, so there are no readings; the user starts one from the Debug tool window");
    } else if series.iter().any(|s| s["watched"] == false) {
        out["note"] = json!("series with watched: false are not read in that session (the user watches them in the Live tab, or from the plot)");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot() -> PlotConfig {
        PlotConfig {
            id: "motor-1".into(),
            name: "  Motor ".into(),
            series: vec![PlotSeries { expression: " rpm ".into(), slot: 1, hidden: false }, PlotSeries { expression: "temp".into(), slot: 4, hidden: true }],
            window_ms: 30_000,
            scale: "shared".into(),
        }
    }

    #[test]
    fn a_good_plot_is_stored_trimmed() {
        let p = plot().validated().unwrap();
        assert_eq!((p.name.as_str(), p.series[0].expression.as_str()), ("Motor", "rpm"));
        assert_eq!(p.summary()["series"], json!(["rpm", "temp"]));
        // Camel case in and out; a hidden series says so, a shown one is silent.
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains("\"windowMs\":30000") && text.contains("\"hidden\":true") && text.matches("hidden").count() == 1, "{text}");
        let back: PlotConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn everything_the_browser_sends_is_checked() {
        let cases: Vec<(&str, Box<dyn Fn(&mut PlotConfig)>)> = vec![
            ("id", Box::new(|p| p.id = String::new())),
            ("id", Box::new(|p| p.id = "a b".into())),
            ("id", Box::new(|p| p.id = "../x".into())),
            ("id", Box::new(|p| p.id = "x".repeat(41))),
            ("name", Box::new(|p| p.name = "   ".into())),
            ("name", Box::new(|p| p.name = "x".repeat(61))),
            ("name", Box::new(|p| p.name = "two\nlines".into())),
            ("at most 8", Box::new(|p| p.series = (1..=9).map(|i| PlotSeries { expression: format!("v{i}"), slot: (i as u8).min(8), hidden: false }).collect())),
            ("expression", Box::new(|p| p.series[0].expression = "  ".into())),
            ("expression", Box::new(|p| p.series[0].expression = "a\nb".into())),
            ("expression", Box::new(|p| p.series[0].expression = "x".repeat(301))),
            ("twice", Box::new(|p| p.series[1].expression = "rpm".into())),
            ("colour", Box::new(|p| p.series[0].slot = 0)),
            ("colour", Box::new(|p| p.series[0].slot = 9)),
            ("colour", Box::new(|p| p.series[1].slot = 1)),
            ("time span", Box::new(|p| p.window_ms = 1234)),
            ("time span", Box::new(|p| p.window_ms = 0)),
            ("scale", Box::new(|p| p.scale = "log".into())),
        ];
        for (needle, change) in cases {
            let mut p = plot();
            change(&mut p);
            let e = p.validated().expect_err(needle);
            assert_eq!(e.status, 400, "{needle}");
            assert!(e.message.contains(needle), "{needle}: {}", e.message);
        }
        // The limits themselves are fine.
        let mut p = plot();
        p.id = "x".repeat(40);
        p.name = "n".repeat(60);
        p.series = (1..=8).map(|i| PlotSeries { expression: "e".repeat(300 - i) + &i.to_string(), slot: i as u8, hidden: false }).collect();
        p.window_ms = 1_800_000;
        p.scale = "normalized".into();
        assert!(p.validated().is_ok());
    }

    #[test]
    fn text_that_rearranges_a_line_is_refused() {
        for bad in ["a\u{2028}b", "a\u{2029}b", "\u{202e}gpj.exe", "a\u{2066}b", "a\u{200f}", "tab\there"] {
            let mut p = plot();
            p.name = bad.into();
            assert!(p.clone().validated().is_err(), "{bad:?}");
            let mut q = plot();
            q.series[0].expression = bad.into();
            assert!(q.validated().is_err(), "{bad:?}");
        }
        let mut ok = plot();
        ok.name = "Motor \u{b5}C \u{2013} \u{65e5}\u{672c}\u{8a9e}".into();
        assert!(ok.validated().is_ok(), "ordinary non-ASCII text is fine");
    }

    #[test]
    fn a_stored_plot_that_does_not_fit_costs_the_project_nothing_else() {
        use crate::debug::breakpoints::ProjectDebug;
        let stored = r#"{"watches": ["x"], "liveWatches": ["ticks"], "breakpoints": [{"id": "b1", "path": "main.c", "line": 3}], "plots": [
            {"id": "bad1", "name": "slot too big", "series": [{"expression": "e", "slot": 300}], "windowMs": 5000, "scale": "shared"},
            {"id": "bad2", "name": "wrong span", "series": [], "windowMs": 1234, "scale": "shared"},
            "not even an object",
            {"id": "good", "name": "Kept", "series": [{"expression": "e", "slot": 2}], "windowMs": 5000, "scale": "shared"}]}"#;
        let p: ProjectDebug = serde_json::from_str(stored).unwrap();
        assert_eq!((p.watches.len(), p.live_watches.len(), p.breakpoints.len()), (1, 1, 1), "the rest of the file is kept");
        assert_eq!(p.plots.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["good"]);
        // Plots that are not even a list leave no plots, and still the rest.
        let p: ProjectDebug = serde_json::from_str(r#"{"watches": ["x"], "plots": "oops"}"#).unwrap();
        assert!(p.plots.is_empty() && p.watches.len() == 1);
    }

    #[test]
    fn a_plot_with_no_series_is_a_plot() {
        let mut p = plot();
        p.series.clear();
        assert!(p.validated().is_ok());
    }
}
