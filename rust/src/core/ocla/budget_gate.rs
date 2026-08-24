//! Budget admission for outbound proxy requests.
//!
//! Extracted from `wire_api`, which was an Axum REST projection of the OCLA
//! contract: fifteen HTTP handlers around a process-wide budget ledger. The
//! REST surface is gone with the rest of the HTTP server; the ledger stays,
//! because `proxy::forward` asks it before every upstream call.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::budget::{BudgetLedger, BudgetLimit, BudgetScope};

#[derive(Default)]
struct BudgetStore {
    ledger: BudgetLedger,
    limits: HashMap<BudgetScope, BudgetLimit>,
}

static BUDGET_STORE: OnceLock<Mutex<BudgetStore>> = OnceLock::new();

fn budget_store() -> &'static Mutex<BudgetStore> {
    BUDGET_STORE.get_or_init(|| Mutex::new(BudgetStore::default()))
}

/// Charge `tokens`/`usd` against `scope`, refusing the request if it would
/// exceed a configured limit. With no limit set for the scope the ledger
/// admits and records, which is the default on a personal install.
pub fn admit_budgeted_request(scope: &str, tokens: u64, usd: f64) -> Result<(), String> {
    let scope = parse_budget_scope(scope)?;
    let mut store = budget_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store
        .ledger
        .check_budget_with_cost(&scope, tokens, usd)
        .map_err(|err| err.to_string())?;
    store.ledger.record_consumption(&scope, tokens, usd);
    Ok(())
}

#[cfg(test)]
pub(crate) fn set_test_budget_limit(limit: BudgetLimit) {
    let mut store = budget_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store.ledger = BudgetLedger::new();
    store.limits.clear();
    store.ledger.set_limit(limit.clone());
    store.limits.insert(limit.scope.clone(), limit);
}

fn parse_budget_scope(raw: &str) -> Result<BudgetScope, String> {
    let (kind, name) = raw
        .split_once(':')
        .ok_or_else(|| "scope must use org:name, team:name, or user:name".to_string())?;
    if name.is_empty() || name.contains(':') {
        return Err("scope name must be non-empty and contain no ':'".to_string());
    }
    match kind {
        "org" => Ok(BudgetScope::Org(name.to_string())),
        "team" => Ok(BudgetScope::Team(name.to_string())),
        "user" => Ok(BudgetScope::User(name.to_string())),
        other => Err(format!(
            "unknown budget scope kind '{other}' (expected org, team or user)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_parsing_accepts_the_three_kinds() {
        assert!(parse_budget_scope("org:acme").is_ok());
        assert!(parse_budget_scope("team:platform").is_ok());
        assert!(parse_budget_scope("user:malo").is_ok());
    }

    #[test]
    fn scope_parsing_rejects_malformed_input() {
        assert!(parse_budget_scope("acme").is_err());
        assert!(parse_budget_scope("org:").is_err());
        assert!(parse_budget_scope("org:a:b").is_err());
        assert!(parse_budget_scope("tenant:acme").is_err());
    }

    #[test]
    fn admission_succeeds_with_no_limit_configured() {
        assert!(admit_budgeted_request("user:nobody-in-particular", 10, 0.01).is_ok());
    }
}
