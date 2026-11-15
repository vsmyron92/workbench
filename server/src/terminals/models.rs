//! What a hosted service's models take and cost: the context and output windows, image
//! input, the effort levels, and DeepSeek's token prices (peak and off-peak, cache hit and
//! miss) used to price a session from the usage its transcript records.
//!
//! Prices are per million tokens, in USD, as the service published them. They are only
//! used to estimate; the service bills the account. DeepSeek's off-peak rule here ignores
//! its Chinese public holidays, so a holiday is priced as a working day (an overestimate).

use chrono::{Datelike, TimeZone, Timelike, Utc};

use super::TokenUsage;

/// What one family of models takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelCaps {
    pub context: u64,
    pub output: u64,
    /// `false`: a pasted image is replaced with "Unsupported Image" by the service.
    pub images: bool,
    pub efforts: &'static [&'static str],
}

/// Whether Workbench prices this service itself, so the CLI's own estimate (priced for
/// another vendor) should not be shown. Legacy DeepSeek models still count: no estimate is
/// better than Anthropic's prices for them.
pub fn is_priced_service(service: &str) -> bool {
    service.trim().eq_ignore_ascii_case("deepseek")
}

/// The models Workbench knows, by service and model name (a model the service names with a
/// suffix such as `[1m]`, or a retired alias, still matches its family).
pub fn caps_for(service: &str, model: &str) -> Option<ModelCaps> {
    let family = family(service, model)?;
    Some(match family {
        Family::Flash => ModelCaps { context: 1_048_576, output: 393_216, images: true, efforts: DEEPSEEK_EFFORTS },
        Family::Pro => ModelCaps { context: 1_048_576, output: 393_216, images: false, efforts: DEEPSEEK_EFFORTS },
        Family::Legacy => ModelCaps { context: 131_072, output: 65_536, images: false, efforts: DEEPSEEK_EFFORTS },
    })
}

const DEEPSEEK_EFFORTS: &[&str] = &["low", "high", "max"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Family {
    Flash,
    Pro,
    Legacy,
}

fn family(service: &str, model: &str) -> Option<Family> {
    if service.trim().to_ascii_lowercase() != "deepseek" {
        return None;
    }
    let m = model.trim().to_ascii_lowercase();
    // `deepseek-flash[1m]`, `deepseek-v4-flash` (retired, served as V4.1 Flash at Flash
    // pricing) and the current `deepseek-flash`.
    if m.starts_with("deepseek-flash") || m.starts_with("deepseek-v4-flash") {
        return Some(Family::Flash);
    }
    if m.starts_with("deepseek-v4-pro") || m.starts_with("deepseek-pro") {
        return Some(Family::Pro);
    }
    if m == "deepseek-chat" || m == "deepseek-reasoner" || m.starts_with("deepseek-v3") {
        return Some(Family::Legacy);
    }
    // DeepSeek's Anthropic-compatible endpoint maps Claude names: Opus to Pro, Sonnet,
    // Haiku and the other Claude names to Flash. Claude Code's model picker offers them,
    // and its transcripts may name the alias it asked for.
    if m.starts_with("claude-opus") || m == "opus" {
        return Some(Family::Pro);
    }
    if m.starts_with("claude-") || m == "sonnet" || m == "haiku" || m == "fable" {
        return Some(Family::Flash);
    }
    None
}

/// A model's prices per million tokens: input that missed the cache, input that hit it,
/// and output. DeepSeek sells off-peak tokens at half these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub miss: f64,
    pub hit: f64,
    pub output: f64,
}

pub fn price_for(service: &str, model: &str) -> Option<Price> {
    Some(match family(service, model)? {
        // DeepSeek's published peak prices (USD per million tokens).
        Family::Flash => Price { miss: 0.30, hit: 0.006, output: 1.20 },
        Family::Pro => Price { miss: 1.32, hit: 0.044, output: 3.96 },
        // V3.2's prices predate the V4 table and are not published alongside it; no
        // estimate is better than a wrong one.
        Family::Legacy => return None,
    })
}

/// Whether `at` (ms since the epoch) falls in DeepSeek's peak: 01:00–04:00 and
/// 06:00–10:00 UTC, Monday to Friday. Weekends and all other hours are off-peak.
pub fn is_off_peak(at_ms: i64) -> bool {
    let Some(t) = Utc.timestamp_millis_opt(at_ms).single() else { return false };
    let weekday = t.weekday().num_days_from_monday() < 5;
    let hour = t.hour();
    !(weekday && ((1..4).contains(&hour) || (6..10).contains(&hour)))
}

/// What one record's tokens cost (`at`: when it was used; `None` prices them at peak).
pub fn estimate(price: Price, usage: TokenUsage, at_ms: Option<i64>) -> f64 {
    let factor = if at_ms.is_some_and(is_off_peak) { 0.5 } else { 1.0 };
    let miss = usage.input as f64 + usage.cache_creation as f64;
    (miss * price.miss + usage.cache_read as f64 * price.hit + usage.output as f64 * price.output) / 1_000_000.0 * factor
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, cache_read: u64, output: u64) -> TokenUsage {
        TokenUsage { input, cache_read, cache_creation: 0, output }
    }

    #[test]
    fn current_models_have_windows_and_capabilities() {
        let flash = caps_for("deepseek", "deepseek-flash[1m]").unwrap();
        assert_eq!((flash.context, flash.images, flash.efforts), (1_048_576, true, DEEPSEEK_EFFORTS));
        let pro = caps_for("deepseek", "deepseek-v4-pro").unwrap();
        assert_eq!((pro.context, pro.images), (1_048_576, false));
        let legacy = caps_for("deepseek", "deepseek-chat").unwrap();
        assert_eq!(legacy.context, 131_072);
        assert!(caps_for("openrouter", "deepseek-v4-pro").is_none());
        assert!(caps_for("deepseek", "some-other-model").is_none());
        assert!(is_priced_service("DeepSeek") && !is_priced_service("openrouter"));
        // DeepSeek maps Claude names itself: Opus is Pro (text only), Sonnet/Haiku are Flash.
        assert!(!caps_for("deepseek", "opus").unwrap().images);
        assert!(caps_for("deepseek", "claude-haiku-4-5").unwrap().images);
        assert_eq!(price_for("deepseek", "claude-opus-5-5").unwrap().miss, 1.32);
    }

    #[test]
    fn prices_are_per_million_and_off_peak_is_half() {
        let flash = price_for("deepseek", "deepseek-flash").unwrap();
        // 1M miss + 1M hit + 1M output at peak.
        let peak = estimate(flash, usage(1_000_000, 1_000_000, 1_000_000), None);
        assert!((peak - (0.30 + 0.006 + 1.20)).abs() < 1e-9);
        // Wednesday 2026-11-18 08:00 UTC is peak; 2026-11-21 (Saturday) is off-peak.
        let peak_at = Utc.with_ymd_and_hms(2026, 11, 18, 8, 0, 0).unwrap().timestamp_millis();
        let off_at = Utc.with_ymd_and_hms(2026, 11, 21, 8, 0, 0).unwrap().timestamp_millis();
        assert!(!is_off_peak(peak_at));
        assert!(is_off_peak(off_at));
        let off = estimate(flash, usage(1_000_000, 0, 0), Some(off_at));
        assert!((off - 0.15).abs() < 1e-9);
        assert!(price_for("deepseek", "deepseek-chat").is_none());
    }

}
