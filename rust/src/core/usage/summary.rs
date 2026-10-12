//! Totals over the usage ledger, computed at read time.
//!
//! Two figures, kept apart because they are not equally certain:
//!
//! - **Real cost**: the sum over the LLM calls the proxy forwarded. Each call
//!   is priced by the price table on the tokens the provider reported, and is
//!   free when the upstream is local. Without the proxy there are no calls and
//!   no real cost.
//! - **Removed tokens**: the sum over compressions of tokens removed, minus
//!   the tokens bounces re-sent. A bounce re-sends the whole file its
//!   compressed read had shortened, so it takes back what that read removed
//!   and charges what the read sent. A compression that sent more than it
//!   read (a search whose output frames its matches) counts against the
//!   total. It is not priced: what the agent would have read and paid
//!   without sobre is not in the ledger.

use std::collections::BTreeMap;

use super::{EVENT_BOUNCE, EVENT_COMPRESSION, EVENT_LLM_CALL, Event};
use crate::core::pricing::ModelPricing;

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
    /// Tokens full re-reads sent again after a compressed read: the whole
    /// file each time.
    pub bounce_tokens: u64,
    pub llm_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Real cost of the calls, in USD.
    pub real_cost_usd: f64,
    /// Calls to a local upstream, which cost nothing.
    pub local_calls: u64,
    /// Per local day (`YYYY-MM-DD`): net removed tokens and real cost.
    pub by_day: BTreeMap<String, (i64, f64)>,
}

impl Summary {
    /// Removed tokens minus bounces.
    #[must_use]
    pub fn net_removed_tokens(&self) -> i64 {
        self.compression.saved() - signed(self.bounce_tokens)
    }
}

/// Aggregates `events` over `period`. `pricing` prices each call that did
/// not go to a local upstream.
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
                // The full re-read sent the whole file again: the compressed
                // read saved nothing, and what it sent was wasted.
                let wasted = ev.int("sobre.tokens.wasted").unwrap_or(0);
                let resent = ev.int("sobre.tokens.original").unwrap_or(0).max(wasted);
                s.bounce_tokens += resent;
                s.by_day.entry(day).or_default().0 -= signed(resent);
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
                    .unwrap_or("");
                let cost = if ev.bool("sobre.upstream.local") {
                    s.local_calls += 1;
                    0.0
                } else {
                    let uncached = input.saturating_sub(cache_read + cache_write);
                    pricing.quote(Some(model)).cost.estimate_usd(
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
                    ("sobre.tool", Value::Str("sobre_read".into())),
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
                    ("gen_ai.usage.input_tokens", Value::Int(1_000_000)),
                    ("sobre.upstream.local", Value::Bool(true)),
                ],
            ),
        ];
        let s = summarize(&events, Period::All, &pricing);
        assert_eq!(s.compression.saved(), 10_000);
        assert_eq!(s.net_removed_tokens(), 9_500);
        assert_eq!(s.by_surface["shell"].saved(), 2_000);
        assert_eq!(s.by_tool["sobre_read"].events, 1);
        assert_eq!(s.llm_calls, 2);
        assert_eq!(s.local_calls, 1);
        let table = pricing.quote(Some("gpt-4o")).cost.input_per_m;
        assert!((s.real_cost_usd - table).abs() < 1e-9);
    }

    #[test]
    fn output_larger_than_input_counts_against_the_total() {
        let events = vec![
            ev(
                EVENT_COMPRESSION,
                &[
                    ("sobre.tool", Value::Str("sobre_read".into())),
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
        assert_eq!(s.net_removed_tokens(), 600);
        assert_eq!(s.by_day.values().map(|(t, _)| t).sum::<i64>(), 600);
    }

    /// A compressed read of `c` out of `F` tokens, then a full re-read of the
    /// same file: the agent received `c + F` where one native read is `F`, so
    /// sobre cost `c`. The compressed read's claimed saving must not survive.
    #[test]
    fn a_bounced_read_costs_what_it_sent() {
        let read = |original: i64, sent: i64| {
            ev(
                EVENT_COMPRESSION,
                &[
                    ("sobre.tool", Value::Str("sobre_read".into())),
                    ("sobre.tokens.original", Value::Int(original as u64)),
                    ("sobre.tokens.sent", Value::Int(sent as u64)),
                ],
            )
        };
        let events = vec![
            read(1_000, 200),
            read(1_000, 1_000),
            ev(
                EVENT_BOUNCE,
                &[
                    ("sobre.tokens.wasted", Value::Int(200)),
                    ("sobre.tokens.original", Value::Int(1_000)),
                ],
            ),
        ];
        let s = summarize(&events, Period::All, &ModelPricing::load());
        assert_eq!(s.net_removed_tokens(), -200);
        assert_eq!(s.by_day.values().map(|(t, _)| t).sum::<i64>(), -200);
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
