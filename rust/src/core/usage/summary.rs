//! Totals over the usage ledger, computed at read time.
//!
//! Two figures, kept apart because they are not equally certain:
//!
//! - **Real cost**: the sum over the LLM calls the proxy forwarded. Each call
//!   is priced by what the provider charged when the response says, otherwise
//!   by the price table on the tokens the provider reported. Without the proxy
//!   there are no calls and no real cost.
//! - **Saved tokens**: the sum over compressions of tokens removed, minus the
//!   tokens bounces re-sent. A compression that sent more than it read (a
//!   search whose output frames its matches) counts against the total. Priced once at the model's uncached input rate,
//!   which is a lower bound: a removed tool result would also have been re-sent
//!   (mostly as cache reads) on every later turn.
//!
//! The counterfactual cost is the real cost plus the saved tokens' price.

use std::collections::BTreeMap;

use super::{EVENT_BOUNCE, EVENT_COMPRESSION, EVENT_LLM_CALL, Event};
use crate::core::pricing::{ModelPricing, ModelQuote};

/// Which events a summary covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    All,
    /// Since local midnight.
    Today,
    /// The last `n` days, today included.
    Days(u32),
}

impl Period {
    /// `all`, `today`, or `<n>d` (`7d`, `30d`).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "all" => Some(Self::All),
            "today" => Some(Self::Today),
            _ => s
                .strip_suffix('d')
                .and_then(|n| n.parse().ok())
                .filter(|n| *n > 0)
                .map(Self::Days),
        }
    }

    fn since_unix(self) -> u64 {
        use chrono::{Duration, Local, TimeZone};
        let midnight = |days_back: i64| {
            let date = Local::now().date_naive() - Duration::days(days_back);
            date.and_hms_opt(0, 0, 0)
                .and_then(|dt| Local.from_local_datetime(&dt).earliest())
                .map_or(0, |dt| u64::try_from(dt.timestamp()).unwrap_or(0))
        };
        match self {
            Self::All => 0,
            Self::Today => midnight(0),
            Self::Days(n) => midnight(i64::from(n) - 1),
        }
    }
}

/// Compression totals for one tool (or one surface).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolTotals {
    pub events: u64,
    pub original: u64,
    pub sent: u64,
}

impl ToolTotals {
    /// Tokens removed; negative when the output was larger than the input.
    #[must_use]
    pub fn saved(&self) -> i64 {
        signed(self.original) - signed(self.sent)
    }
}

fn signed(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// Compression totals over every surface.
    pub compression: ToolTotals,
    /// Keyed by surface (`mcp`, `shell`, `hook`, `proxy`).
    pub by_surface: BTreeMap<String, ToolTotals>,
    /// Keyed by tool name.
    pub by_tool: BTreeMap<String, ToolTotals>,
    pub bounce_tokens: u64,
    pub llm_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Real cost of the calls, in USD.
    pub real_cost_usd: f64,
    /// Calls whose cost the provider reported.
    pub provider_priced_calls: u64,
    /// Calls to a local upstream, which cost nothing.
    pub local_calls: u64,
    /// Calls per answering model, to pick the model saved tokens are priced at.
    pub calls_by_model: BTreeMap<String, u64>,
    /// Per local day (`YYYY-MM-DD`): net saved tokens and real cost.
    pub by_day: BTreeMap<String, (i64, f64)>,
}

impl Summary {
    /// Removed tokens minus bounces.
    #[must_use]
    pub fn net_saved_tokens(&self) -> i64 {
        self.compression.saved() - signed(self.bounce_tokens)
    }

    /// The model that answered the most calls, if the proxy saw any.
    #[must_use]
    pub fn dominant_model(&self) -> Option<&str> {
        self.calls_by_model
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(m, _)| m.as_str())
    }

    /// Price of the net saved tokens at `quote`'s uncached input rate.
    #[must_use]
    pub fn saved_usd(&self, quote: &ModelQuote) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let tokens = self.net_saved_tokens() as f64;
        tokens / 1_000_000.0 * quote.cost.input_per_m
    }
}

/// Aggregates `events` over `period`. `pricing` prices calls whose cost the
/// provider did not report.
#[must_use]
pub fn summarize(events: &[Event], period: Period, pricing: &ModelPricing) -> Summary {
    let since = period.since_unix();
    let mut s = Summary::default();
    for ev in events.iter().filter(|e| e.unix_secs() >= since) {
        let day = day_of(ev.unix_secs());
        match ev.name.as_str() {
            EVENT_COMPRESSION => {
                let original = ev.int("sobre.tokens.original").unwrap_or(0);
                let sent = ev.int("sobre.tokens.sent").unwrap_or(0);
                for totals in [
                    &mut s.compression,
                    s.by_surface
                        .entry(ev.str("sobre.surface").unwrap_or("?").to_string())
                        .or_default(),
                    s.by_tool
                        .entry(ev.str("sobre.tool").unwrap_or("?").to_string())
                        .or_default(),
                ] {
                    totals.events += 1;
                    totals.original += original;
                    totals.sent += sent;
                }
                s.by_day.entry(day).or_default().0 += signed(original) - signed(sent);
            }
            EVENT_BOUNCE => {
                let wasted = ev.int("sobre.tokens.wasted").unwrap_or(0);
                s.bounce_tokens += wasted;
                s.by_day.entry(day).or_default().0 -= signed(wasted);
            }
            EVENT_LLM_CALL => {
                let input = ev.int("gen_ai.usage.input_tokens").unwrap_or(0);
                let output = ev.int("gen_ai.usage.output_tokens").unwrap_or(0);
                let cache_read = ev.int("gen_ai.usage.cache_read.input_tokens").unwrap_or(0);
                let cache_write = ev
                    .int("gen_ai.usage.cache_creation.input_tokens")
                    .unwrap_or(0);
                let model = ev
                    .str("gen_ai.response.model")
                    .filter(|m| !m.is_empty())
                    .or_else(|| ev.str("gen_ai.request.model"))
                    .unwrap_or("")
                    .to_string();
                let cost = if let Some(usd) = ev.double("sobre.cost.usd") {
                    s.provider_priced_calls += 1;
                    usd
                } else if ev.bool("sobre.upstream.local") {
                    s.local_calls += 1;
                    0.0
                } else {
                    let uncached = input.saturating_sub(cache_read + cache_write);
                    pricing.quote(Some(&model)).cost.estimate_usd(
                        uncached,
                        output,
                        cache_write,
                        cache_read,
                    )
                };
                s.llm_calls += 1;
                s.input_tokens += input;
                s.output_tokens += output;
                s.cache_read_tokens += cache_read;
                s.cache_write_tokens += cache_write;
                s.real_cost_usd += cost;
                if !model.is_empty() {
                    *s.calls_by_model.entry(model).or_default() += 1;
                }
                s.by_day.entry(day).or_default().1 += cost;
            }
            _ => {}
        }
    }
    s
}

fn day_of(unix_secs: u64) -> String {
    use chrono::{Local, TimeZone};
    i64::try_from(unix_secs)
        .ok()
        .and_then(|t| Local.timestamp_opt(t, 0).single())
        .map_or_else(String::new, |dt| dt.format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::super::Value;
    use super::*;

    fn ev(name: &str, attrs: &[(&str, Value)]) -> Event {
        let mut e = Event::now(name);
        for (k, v) in attrs {
            e.attr(k, v.clone());
        }
        e
    }

    #[test]
    fn totals_net_out_bounces_and_price_calls() {
        let pricing = ModelPricing::load();
        let events = vec![
            ev(
                EVENT_COMPRESSION,
                &[
                    ("sobre.surface", Value::Str("mcp".into())),
                    ("sobre.tool", Value::Str("ctx_read".into())),
                    ("sobre.tokens.original", Value::Int(10_000)),
                    ("sobre.tokens.sent", Value::Int(2_000)),
                ],
            ),
            ev(
                EVENT_COMPRESSION,
                &[
                    ("sobre.surface", Value::Str("shell".into())),
                    ("sobre.tool", Value::Str("cargo".into())),
                    ("sobre.tokens.original", Value::Int(3_000)),
                    ("sobre.tokens.sent", Value::Int(1_000)),
                ],
            ),
            ev(EVENT_BOUNCE, &[("sobre.tokens.wasted", Value::Int(500))]),
            ev(
                EVENT_LLM_CALL,
                &[
                    ("gen_ai.response.model", Value::Str("gpt-4o".into())),
                    ("gen_ai.usage.input_tokens", Value::Int(1_000_000)),
                    ("gen_ai.usage.output_tokens", Value::Int(0)),
                ],
            ),
            ev(
                EVENT_LLM_CALL,
                &[
                    ("gen_ai.response.model", Value::Str("gpt-4o".into())),
                    ("sobre.cost.usd", Value::Double(0.5)),
                ],
            ),
        ];
        let s = summarize(&events, Period::All, &pricing);
        assert_eq!(s.compression.saved(), 10_000);
        assert_eq!(s.net_saved_tokens(), 9_500);
        assert_eq!(s.by_surface["shell"].saved(), 2_000);
        assert_eq!(s.by_tool["ctx_read"].events, 1);
        assert_eq!(s.llm_calls, 2);
        assert_eq!(s.provider_priced_calls, 1);
        assert_eq!(s.local_calls, 0);
        assert_eq!(s.dominant_model(), Some("gpt-4o"));
        let table = pricing.quote(Some("gpt-4o")).cost.input_per_m;
        assert!((s.real_cost_usd - (table + 0.5)).abs() < 1e-9);
    }

    #[test]
    fn output_larger_than_input_counts_against_the_total() {
        let events = vec![
            ev(
                EVENT_COMPRESSION,
                &[
                    ("sobre.tool", Value::Str("ctx_read".into())),
                    ("sobre.tokens.original", Value::Int(1_000)),
                    ("sobre.tokens.sent", Value::Int(200)),
                ],
            ),
            ev(
                EVENT_COMPRESSION,
                &[
                    ("sobre.tool", Value::Str("grep".into())),
                    ("sobre.tokens.original", Value::Int(100)),
                    ("sobre.tokens.sent", Value::Int(300)),
                ],
            ),
        ];
        let s = summarize(&events, Period::All, &ModelPricing::load());
        assert_eq!(s.by_tool["grep"].saved(), -200);
        assert_eq!(s.net_saved_tokens(), 600);
        assert_eq!(s.by_day.values().map(|(t, _)| t).sum::<i64>(), 600);
    }

    #[test]
    fn period_parses() {
        assert_eq!(Period::parse("all"), Some(Period::All));
        assert_eq!(Period::parse("7d"), Some(Period::Days(7)));
        assert_eq!(Period::parse("0d"), None);
        assert_eq!(Period::parse("week"), None);
    }

    #[test]
    fn period_filters_old_events() {
        let mut old = ev(
            EVENT_COMPRESSION,
            &[
                ("sobre.tokens.original", Value::Int(100)),
                ("sobre.tokens.sent", Value::Int(0)),
            ],
        );
        old.time_unix_nano = 1_000_000_000;
        let s = summarize(&[old], Period::Days(7), &ModelPricing::load());
        assert_eq!(s.compression.events, 0);
    }
}
