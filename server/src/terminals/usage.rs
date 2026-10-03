//! Usage of agent accounts: which provider is at its limit, until when, and how much of
//! its windows (Claude: the 5-hour and weekly ones; Codex: its primary and secondary) is
//! used.
//!
//! Everything here is read from what the CLIs report about themselves: the `rate_limits`
//! of Claude Code's status line payload, the `rate_limits` of Codex's `token_count`
//! rollout events, and the "You've hit your session limit · resets 3:45pm" message a CLI
//! prints when a turn is refused. Nothing queries a vendor, and no credential is read.
//! Usage is keyed by provider name (`[agents.providers.<name>]`: one account each) and kept
//! in `usage.json` of the data folder, so a restart does not forget a limit.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Weekday};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A window is full from this share of use on (the CLIs round).
const FULL_PCT: f64 = 99.5;
/// How long a limit whose reset time could not be read keeps an account out of use.
pub const UNKNOWN_RESET_MS: i64 = 30 * 60 * 1000;
/// No limit lasts longer than this (a weekly one is the longest).
const MAX_LIMIT_MS: i64 = 8 * 24 * 3600 * 1000;

/// One usage window of an account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    /// `five_hour`, `seven_day` (Claude), `primary`, `secondary` (Codex).
    pub name: String,
    /// What the window is called to a person: `5-hour`, `Weekly`, `7 hours`…
    pub label: String,
    pub used_pct: f64,
    /// When the window starts over (ms), when known.
    pub resets_at: Option<i64>,
}

impl Window {
    fn full(&self, now: i64) -> bool {
        self.used_pct >= FULL_PCT && self.resets_at.is_none_or(|r| r > now)
    }
    fn live(&self, now: i64) -> bool {
        self.resets_at.is_none_or(|r| r > now)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountUsage {
    pub windows: Vec<Window>,
    /// A refusal was seen (or the user said so): out of use until then (ms).
    pub limited_until: Option<i64>,
    pub reason: Option<String>,
    pub updated_at: i64,
}

/// Why and until when an account cannot start a session.
#[derive(Debug, Clone, PartialEq)]
pub struct Limited {
    pub until: i64,
    pub reason: String,
}

#[derive(Default)]
pub struct Usage {
    map: Mutex<BTreeMap<String, AccountUsage>>,
    file: Mutex<Option<PathBuf>>,
}

impl Usage {
    /// Read `usage.json` of `dir` and keep writing it there.
    pub fn open(&self, dir: &std::path::Path) {
        let file = dir.join("usage.json");
        if let Some(m) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<BTreeMap<String, AccountUsage>>(&b).ok()) {
            *self.map.lock() = m;
        }
        *self.file.lock() = Some(file);
    }

    fn save(&self) {
        let Some(file) = self.file.lock().clone() else { return };
        let json = serde_json::to_vec(&*self.map.lock()).unwrap_or_default();
        if let Err(e) = crate::util::fs::write_atomic(&file, &json, 0o600) {
            tracing::debug!("usage.json not written: {e}");
        }
    }

    /// The account is out of use now: refused, or one of its windows is full.
    pub fn limited(&self, id: &str, now: i64) -> Option<Limited> {
        let map = self.map.lock();
        let u = map.get(id)?;
        let mut best: Option<Limited> = None;
        let mut offer = |until: i64, reason: String| {
            if best.as_ref().is_none_or(|b| until > b.until) {
                best = Some(Limited { until, reason });
            }
        };
        if let Some(until) = u.limited_until.filter(|t| *t > now) {
            offer(until, u.reason.clone().unwrap_or_else(|| "at its usage limit".into()));
        }
        for w in u.windows.iter().filter(|w| w.full(now)) {
            offer(w.resets_at.unwrap_or(now + UNKNOWN_RESET_MS), format!("{} limit used up", w.label));
        }
        best
    }

    /// The windows a status line or rollout reported. They replace the ones known; a
    /// report with no full window ends a refusal seen before it. Returns whether anything
    /// a person sees changed.
    pub fn record_windows(&self, id: &str, windows: Vec<Window>, now: i64) -> bool {
        let changed = {
            let mut map = self.map.lock();
            let u = map.entry(id.to_string()).or_default();
            let was = (u.windows.iter().map(|w| (w.name.clone(), w.used_pct.round() as i64, w.resets_at)).collect::<Vec<_>>(), u.limited_until);
            if !windows.iter().any(|w| w.full(now)) {
                u.limited_until = None;
                u.reason = None;
            }
            u.windows = windows;
            u.updated_at = now;
            let is = (u.windows.iter().map(|w| (w.name.clone(), w.used_pct.round() as i64, w.resets_at)).collect::<Vec<_>>(), u.limited_until);
            was != is
        };
        if changed {
            self.save();
        }
        changed
    }

    /// A refusal was seen: out of use until `until` (`None`: unknown, so a short while).
    pub fn mark_limited(&self, id: &str, until: Option<i64>, reason: &str, now: i64) -> bool {
        let until = until.unwrap_or(now + UNKNOWN_RESET_MS).clamp(now + 60_000, now + MAX_LIMIT_MS);
        let changed = {
            let mut map = self.map.lock();
            let u = map.entry(id.to_string()).or_default();
            let changed = u.limited_until != Some(until) || u.reason.as_deref() != Some(reason);
            u.limited_until = Some(until);
            u.reason = Some(reason.to_string());
            u.updated_at = now;
            changed
        };
        if changed {
            self.save();
        }
        changed
    }

    /// The account is usable again, whatever was seen (the user's word). Full windows are
    /// kept, so a wrong "usable" corrects itself at the next refusal.
    pub fn clear_limited(&self, id: &str, now: i64) -> bool {
        let changed = {
            let mut map = self.map.lock();
            let Some(u) = map.get_mut(id) else { return false };
            let had = u.limited_until.is_some() || u.windows.iter().any(|w| w.full(now));
            u.limited_until = None;
            u.reason = None;
            u.windows.retain(|w| !w.full(now));
            had
        };
        if changed {
            self.save();
        }
        changed
    }

    /// `GET /api/agents/usage`: the live part of what is known, by provider name.
    pub fn snapshot(&self, now: i64) -> Value {
        let map = self.map.lock();
        let mut out = serde_json::Map::new();
        for (id, u) in map.iter() {
            out.insert(id.clone(), describe(u, now));
        }
        Value::Object(out)
    }

    /// One account's entry of the snapshot.
    pub fn describe(&self, id: &str, now: i64) -> Value {
        self.map.lock().get(id).map(|u| describe(u, now)).unwrap_or_else(|| describe(&AccountUsage::default(), now))
    }
}

fn describe(u: &AccountUsage, now: i64) -> Value {
    let windows: Vec<&Window> = u.windows.iter().filter(|w| w.live(now)).collect();
    let limit = windows
        .iter()
        .filter(|w| w.full(now))
        .map(|w| (w.resets_at.unwrap_or(now + UNKNOWN_RESET_MS), format!("{} limit used up", w.label)))
        .chain(u.limited_until.filter(|t| *t > now).map(|t| (t, u.reason.clone().unwrap_or_else(|| "at its usage limit".into()))))
        .max_by_key(|(t, _)| *t);
    json!({
        "windows": windows,
        "limited": limit.is_some(),
        "limitedUntil": limit.as_ref().map(|(t, _)| *t),
        "reason": limit.map(|(_, r)| r),
        "updatedAt": u.updated_at,
    })
}

// ---------------------------------------------------------------- what the CLIs report

fn pct(v: &Value) -> Option<f64> {
    v.as_f64().filter(|p| p.is_finite()).map(|p| (p.clamp(0.0, 100.0) * 10.0).round() / 10.0)
}

/// The `rate_limits` of a Claude Code status line payload: `five_hour` and `seven_day`
/// with `used_percentage` and `resets_at` (epoch seconds). `None` when the payload has no
/// `rate_limits` at all (an API key or a local model has none), which says nothing.
pub fn claude_windows(v: &Value) -> Option<Vec<Window>> {
    let r = v.get("rate_limits")?.as_object()?;
    let mut out = vec![];
    for (name, label) in [("five_hour", "5-hour"), ("seven_day", "Weekly")] {
        let Some(w) = r.get(name) else { continue };
        let Some(used_pct) = w.get("used_percentage").and_then(pct) else { continue };
        let resets_at = w.get("resets_at").and_then(Value::as_f64).filter(|t| *t > 0.0).map(|t| (t * 1000.0) as i64);
        out.push(Window { name: name.into(), label: label.into(), used_pct, resets_at });
    }
    Some(out)
}

/// A usage window as Codex reports it in a rollout's `token_count` event.
#[derive(Debug, Clone, PartialEq)]
pub struct Reported {
    pub name: &'static str,
    pub used_pct: f64,
    pub window_minutes: Option<i64>,
    /// When it starts over (ms): given as an instant …
    pub resets_at: Option<i64>,
    /// … or, in rollouts of older versions, as a time from the line's own.
    pub resets_in_ms: Option<i64>,
}

/// `payload.rate_limits` of a `token_count` event: `primary` and `secondary`, each with
/// `used_percent`, `window_minutes` and `resets_at` (epoch seconds; older rollouts hold an
/// RFC 3339 string or `resets_in_seconds`). A snapshot with neither window (a rate-limit
/// update of a provider that has none) reports nothing.
pub fn codex_reported(rate_limits: &Value) -> Vec<Reported> {
    let mut out = vec![];
    for name in ["primary", "secondary"] {
        let Some(w) = rate_limits.get(name).filter(|w| w.is_object()) else { continue };
        let Some(used_pct) = w.get("used_percent").and_then(pct) else { continue };
        let resets_at = match w.get("resets_at") {
            Some(Value::Number(n)) => n.as_f64().filter(|t| *t > 0.0).map(|t| (t * 1000.0) as i64),
            Some(Value::String(t)) => chrono::DateTime::parse_from_rfc3339(t).ok().map(|d| d.timestamp_millis()),
            _ => None,
        };
        out.push(Reported {
            name: if name == "primary" { "primary" } else { "secondary" },
            used_pct,
            window_minutes: w.get("window_minutes").and_then(Value::as_i64).filter(|m| *m > 0),
            resets_at,
            resets_in_ms: w.get("resets_in_seconds").and_then(Value::as_f64).filter(|t| *t >= 0.0).map(|t| (t * 1000.0) as i64),
        });
    }
    out
}

/// "5-hour" for 300 minutes, "Weekly" for 10080, "3-day", "90-minute"…
pub fn window_label(minutes: Option<i64>, fallback: &str) -> String {
    match minutes {
        Some(10080) => "Weekly".into(),
        Some(m) if m % 1440 == 0 => format!("{}-day", m / 1440),
        Some(m) if m % 60 == 0 => format!("{}-hour", m / 60),
        Some(m) => format!("{m}-minute"),
        None => fallback.into(),
    }
}

/// Codex's reported windows as usage windows; `now` dates the ones given as a time from it.
pub fn codex_windows(reported: &[Reported], now: i64) -> Vec<Window> {
    reported
        .iter()
        .map(|r| Window {
            name: r.name.into(),
            label: window_label(r.window_minutes, if r.name == "primary" { "Primary" } else { "Secondary" }),
            used_pct: r.used_pct,
            resets_at: r.resets_at.or(r.resets_in_ms.map(|d| now + d)),
        })
        .collect()
}

/// A refusal because of the account's usage, as a CLI prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// The whole account is out of use (a session or weekly limit), not one model of it.
    pub account_wide: bool,
    /// `session`, `weekly`, `opus`…
    pub label: String,
    pub resets_at: Option<i64>,
}

static HIT_LIMIT: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)you(?:'|\u{2019})ve hit your\s+(.*?)\s*limit").unwrap());
static LIMIT_REACHED: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b(5-hour|weekly|session|usage|daily)\s+limit\s+(?:reached|exceeded)").unwrap());
static RESETS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\bresets?\s*(?:at|on)?\s+([^\n\r\u{00b7}|(]{1,40})").unwrap());
static TRY_AGAIN: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)\btry again at\s+([^\n\r|(]{1,40})").unwrap());
static CLOCK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?ix)^\s*
        (?:(?P<wd>mon|tue|wed|thu|fri|sat|sun)[a-z]*\.?,?\s+)?
        (?:(?P<mo>jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*\.?\s+(?P<md>\d{1,2})(?:st|nd|rd|th)?,?\s*(?:(?P<yr>\d{4}),?\s*)?(?:at\s+)?)?
        (?P<h>\d{1,2})(?::(?P<m>\d{2}))?\s*(?P<ap>am|pm|a\.m\.|p\.m\.)?",
    )
    .unwrap()
});

/// "3pm", "3:45 PM", "Mon 12:00am", "Oct 9 at 3pm", "15:45": the next time that is, in
/// this computer's time zone (the one the CLI prints it in).
pub fn parse_reset(text: &str, now: DateTime<Local>) -> Option<i64> {
    let c = CLOCK.captures(text)?;
    let mut hour: u32 = c["h"].parse().ok()?;
    let minute: u32 = c.name("m").map_or(Some(0), |m| m.as_str().parse().ok())?;
    let ap = c.name("ap").map(|a| a.as_str().to_lowercase());
    if c.name("ap").is_none() && c.name("m").is_none() {
        // A bare number is no time.
        return None;
    }
    if ap.is_some() && !(1..=12).contains(&hour) {
        return None;
    }
    match ap.as_deref().map(|a| a.starts_with('p')) {
        Some(true) if hour < 12 => hour += 12,
        Some(false) if hour == 12 => hour = 0,
        _ => {}
    }
    if hour > 23 || minute > 59 {
        return None;
    }
    let at = |d: NaiveDate| Local.from_local_datetime(&d.and_hms_opt(hour, minute, 0)?).earliest();
    let today = now.date_naive();
    let found = if let (Some(mo), Some(md)) = (c.name("mo"), c.name("md")) {
        let month = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| *m == mo.as_str().to_lowercase())? as u32 + 1;
        let day: u32 = md.as_str().parse().ok()?;
        if let Some(y) = c.name("yr") {
            at(NaiveDate::from_ymd_opt(y.as_str().parse().ok()?, month, day)?)?
        } else {
            let this = at(NaiveDate::from_ymd_opt(today.year(), month, day)?)?;
            if this + Duration::hours(24) < now { at(NaiveDate::from_ymd_opt(today.year() + 1, month, day)?)? } else { this }
        }
    } else if let Some(wd) = c.name("wd") {
        let want = match wd.as_str().to_lowercase().as_str() {
            "mon" => Weekday::Mon,
            "tue" => Weekday::Tue,
            "wed" => Weekday::Wed,
            "thu" => Weekday::Thu,
            "fri" => Weekday::Fri,
            "sat" => Weekday::Sat,
            _ => Weekday::Sun,
        };
        let ahead = (want.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
        let mut t = at(today + Duration::days(ahead))?;
        if t <= now {
            t = at(today + Duration::days(ahead + 7))?;
        }
        t
    } else {
        let t = at(today)?;
        if t <= now { at(today + Duration::days(1))? } else { t }
    };
    Some(found.timestamp_millis())
}

/// Whether `text` says a usage limit was hit, and when it ends. Only for text that follows
/// a refused turn (a `StopFailure`): the same words in an answer or a file mean nothing.
pub fn limit_notice(text: &str, now: DateTime<Local>) -> Option<Notice> {
    let (label, end) = if let Some(m) = HIT_LIMIT.captures(text) {
        (m[1].trim().to_lowercase(), m.get(0)?.end())
    } else if let Some(m) = LIMIT_REACHED.captures(text) {
        (m[1].to_lowercase(), m.get(0)?.end())
    } else {
        return None;
    };
    // The reset time follows on the same line.
    let rest: String = text[end..].chars().take(160).collect();
    let line = rest.split(['\n', '\r']).next().unwrap_or("");
    let resets_at = RESETS
        .captures(line)
        .or_else(|| TRY_AGAIN.captures(line))
        .and_then(|r| parse_reset(r[1].trim_end_matches(['.', ' ']), now));
    // "…usage limit for GPT-5-Codex. Switch to another model": that model's, not the account's.
    let named = rest.trim_start().to_lowercase().starts_with("for ");
    let account_wide = !named && (label.is_empty() || ["session", "week", "hour", "daily", "usage", "5"].iter().any(|w| label.contains(w)));
    Some(Notice { account_wide, label, resets_at })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).earliest().unwrap()
    }

    #[test]
    fn reads_the_status_line_windows() {
        let v = json!({"rate_limits": {"five_hour": {"used_percentage": 23.456, "resets_at": 1738425600}, "seven_day": {"used_percentage": 100, "resets_at": 1738857600}}});
        let w = claude_windows(&v).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].name.as_str(), w[0].used_pct, w[0].resets_at), ("five_hour", 23.5, Some(1738425600000)));
        assert_eq!(w[1].label, "Weekly");
        // A window may be missing on its own; no rate_limits at all says nothing.
        assert_eq!(claude_windows(&json!({"rate_limits": {"seven_day": {"used_percentage": 3}}})).unwrap().len(), 1);
        assert!(claude_windows(&json!({"model": {}})).is_none());
        assert_eq!(claude_windows(&json!({"rate_limits": {}})), Some(vec![]));
        assert!(claude_windows(&json!({"rate_limits": {"five_hour": {"used_percentage": "x"}}})).unwrap().is_empty());
    }

    #[test]
    fn reset_times_in_the_forms_a_cli_prints() {
        let now = at(2026, 10, 3, 14, 0); // a Saturday
        let ms = |d: DateTime<Local>| Some(d.timestamp_millis());
        assert_eq!(parse_reset("3:45pm", now), ms(at(2026, 10, 3, 15, 45)));
        assert_eq!(parse_reset("3pm", now), ms(at(2026, 10, 3, 15, 0)));
        // Already past today: tomorrow.
        assert_eq!(parse_reset("1:30pm", now), ms(at(2026, 10, 4, 13, 30)));
        assert_eq!(parse_reset("12:00am", now), ms(at(2026, 10, 4, 0, 0)));
        assert_eq!(parse_reset("12:30 PM", now), ms(at(2026, 10, 4, 12, 30)));
        assert_eq!(parse_reset("15:45", now), ms(at(2026, 10, 3, 15, 45)));
        assert_eq!(parse_reset("Mon 12:00am", now), ms(at(2026, 10, 5, 0, 0)));
        assert_eq!(parse_reset("Saturday 3pm", now), ms(at(2026, 10, 3, 15, 0)));
        assert_eq!(parse_reset("Sat 1pm", now), ms(at(2026, 10, 10, 13, 0)));
        assert_eq!(parse_reset("Oct 9 at 3pm", now), ms(at(2026, 10, 9, 15, 0)));
        assert_eq!(parse_reset("Jan 2, 9am", now), ms(at(2027, 1, 2, 9, 0)));
        // Not a time.
        assert_eq!(parse_reset("soon", now), None);
        assert_eq!(parse_reset("5", now), None);
        assert_eq!(parse_reset("13pm", now), None);
        assert_eq!(parse_reset("9:75am", now), None);
    }

    #[test]
    fn reads_the_limit_messages() {
        let now = at(2026, 10, 3, 14, 0);
        let n = limit_notice("You've hit your session limit · resets 3:45pm", now).unwrap();
        assert_eq!((n.account_wide, n.label.as_str(), n.resets_at), (true, "session", Some(at(2026, 10, 3, 15, 45).timestamp_millis())));
        let n = limit_notice("  ⎿ You\u{2019}ve hit your weekly limit · resets Mon 12:00am (Europe/Berlin)\n> next line 5pm", now).unwrap();
        assert_eq!((n.account_wide, n.resets_at), (true, Some(at(2026, 10, 5, 0, 0).timestamp_millis())));
        // One model's limit leaves the account usable.
        let n = limit_notice("You've hit your Opus limit · resets 3:45pm", now).unwrap();
        assert_eq!((n.account_wide, n.label.as_str()), (false, "opus"));
        // Older wording, and a message without a time.
        let n = limit_notice("5-hour limit reached ∙ resets 5pm", now).unwrap();
        assert_eq!((n.account_wide, n.resets_at), (true, Some(at(2026, 10, 3, 17, 0).timestamp_millis())));
        let n = limit_notice("Claude usage limit reached.", now).unwrap();
        assert_eq!((n.account_wide, n.resets_at), (true, None));
        let n = limit_notice("You've hit your limit", now).unwrap();
        assert!(n.account_wide);
        // Another line's time is not the reset.
        assert_eq!(limit_notice("You've hit your session limit\nresets 3pm somewhere else", now).unwrap().resets_at, None);
        assert!(limit_notice("API Error: Request rejected (429) · this may be a temporary capacity issue", now).is_none());
        assert!(limit_notice("the rate limit of the API is documented here", now).is_none());
    }

    #[test]
    fn reads_codex_windows_of_every_vintage() {
        let now = 1_700_000_000_000;
        let v = json!({"limit_id":"codex","primary":{"used_percent":100.0,"window_minutes":300,"resets_at":1893456000},"secondary":{"used_percent":41.5,"window_minutes":10080,"resets_at":"2030-01-01T00:00:00Z"}});
        let r = codex_reported(&v);
        let w = codex_windows(&r, now);
        assert_eq!((w[0].label.as_str(), w[0].used_pct, w[0].resets_at), ("5-hour", 100.0, Some(1893456000000)));
        assert_eq!((w[1].label.as_str(), w[1].resets_at), ("Weekly", Some(1893456000000)));
        // Old rollouts give the time left; windows of other lengths are named by it.
        let old = codex_windows(&codex_reported(&json!({"primary":{"used_percent":3,"window_minutes":90,"resets_in_seconds":600}})), now);
        assert_eq!((old[0].label.as_str(), old[0].resets_at), ("90-minute", Some(now + 600_000)));
        assert_eq!(window_label(Some(4320), "x"), "3-day");
        assert_eq!(window_label(Some(60), "x"), "1-hour");
        assert_eq!(window_label(None, "Primary"), "Primary");
        // A provider without limits reports every field null.
        assert!(codex_reported(&json!({"limit_id":"codex","primary":null,"secondary":null,"credits":null})).is_empty());
        assert!(codex_reported(&Value::Null).is_empty());
    }

    #[test]
    fn reads_codex_limit_messages() {
        let now = at(2026, 10, 3, 14, 0);
        let n = limit_notice("You\u{2019}ve hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/settings/usage to purchase more credits or try again at 3:45 PM.", now).unwrap();
        assert_eq!((n.account_wide, n.resets_at), (true, Some(at(2026, 10, 3, 15, 45).timestamp_millis())));
        let n = limit_notice("You\u{2019}ve hit your usage limit. Try again at Mar 3rd, 2027 4:05 PM.", now).unwrap();
        assert_eq!(n.resets_at, Some(at(2027, 3, 3, 16, 5).timestamp_millis()));
        assert_eq!(limit_notice("You\u{2019}ve hit your usage limit. Try again later.", now).unwrap().resets_at, None);
        assert!(!limit_notice("You\u{2019}ve hit your usage limit for GPT-5-Codex. Switch to another model now.", now).unwrap().account_wide);
    }

    #[test]
    fn tracks_limits_and_windows() {
        let u = Usage::default();
        let (id, now) = ("claude-a", 1_000_000_000_000);
        assert!(u.limited(id, now).is_none());
        // A refusal with a reset time.
        assert!(u.mark_limited(id, Some(now + 3_600_000), "session limit", now));
        assert!(!u.mark_limited(id, Some(now + 3_600_000), "session limit", now), "the same news changes nothing");
        assert_eq!(u.limited(id, now).unwrap(), Limited { until: now + 3_600_000, reason: "session limit".into() });
        assert!(u.limited(id, now + 3_600_001).is_none(), "it ends by itself");
        // A refusal whose time could not be read, and one that claims too much.
        u.mark_limited("b", None, "x", now);
        assert_eq!(u.limited("b", now).unwrap().until, now + UNKNOWN_RESET_MS);
        u.mark_limited("c", Some(now + 90 * 24 * 3_600_000), "x", now);
        assert_eq!(u.limited("c", now).unwrap().until, now + MAX_LIMIT_MS);
        // A status line that shows room again ends a refusal seen before it.
        let w = |p: f64| vec![Window { name: "five_hour".into(), label: "5-hour".into(), used_pct: p, resets_at: Some(now + 7_200_000) }];
        assert!(u.record_windows(id, w(40.0), now + 10));
        assert!(u.limited(id, now + 10).is_none());
        // A full window is a limit of its own, until it resets.
        assert!(u.record_windows(id, w(100.0), now + 20));
        assert_eq!(u.limited(id, now + 20).unwrap(), Limited { until: now + 7_200_000, reason: "5-hour limit used up".into() });
        assert!(u.limited(id, now + 7_200_001).is_none());
        // A refusal is not ended by a report that is still full.
        u.mark_limited(id, Some(now + 9_000_000), "weekly limit", now + 30);
        u.record_windows(id, w(100.0), now + 40);
        assert_eq!(u.limited(id, now + 40).unwrap().until, now + 9_000_000);
        // The user says it is usable.
        assert!(u.clear_limited(id, now + 50));
        assert!(u.limited(id, now + 50).is_none());
        assert!(!u.clear_limited(id, now + 50));
        let s = u.snapshot(now + 60);
        assert_eq!(s[id]["limited"], false);
        assert!(s["b"]["limited"].as_bool().unwrap());
    }

    #[test]
    fn survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let now = crate::util::now_ms();
        let u = Usage::default();
        u.open(dir.path());
        u.mark_limited("claude-a", Some(now + 3_600_000), "session limit", now);
        let again = Usage::default();
        again.open(dir.path());
        assert_eq!(again.limited("claude-a", now).unwrap().reason, "session limit");
        let mode = std::fs::metadata(dir.path().join("usage.json")).unwrap();
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o777, 0o600);
        let _ = mode;
    }
}
