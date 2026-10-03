//! Runtime view of the active context policy pack (GL #673 / #489 enforcement).
//!
//! [`active`] loads and resolves the project policy pack
//! (`.lean-ctx/policy.toml`) once, folds in a trusted central org policy as a
//! floor when one is installed (GL #674), then caches the [`ResolvedPolicy`]
//! together with its precompiled redaction regexes so the MCP hot path
//! ([`crate::server::policy_guard`] and the `call_tool` redaction step) can
//! consult it cheaply.
//!
//! **Opt-in & backward-compatible:** with no project pack present, [`active`]
//! returns `None` and nothing is gated — existing behavior is preserved
//! exactly. An invalid pack is ignored (logged), never bricking the agent.
//!
//! **Local-Free Invariant:** enforcement derived from this view only ever
//! constrains the *agent* pipeline; it never gates a human's own local reads.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use regex::Regex;

use super::{ResolvedPolicy, parse_file, resolve};
use crate::core::input_filters::{FilterAction, FilterConfig};

/// Project-local pack location, relative to the working directory (matches the
/// `lean-ctx policy` CLI's `PROJECT_PACK_PATH`).
const PROJECT_PACK_PATH: &str = ".lean-ctx/policy.toml";

/// A resolved policy plus its precompiled redaction regexes — the cached,
/// hot-path-ready form.
pub struct ActivePolicy {
    pub resolved: ResolvedPolicy,
    /// `(label, compiled regex)` — labels are the pack's `[redaction]` keys.
    /// Patterns that fail to compile are skipped (validation already rejects
    /// them on load, so this is defense-in-depth, not the primary guard).
    pub redaction: Vec<(String, Regex)>,
    /// Compiled inbound content filters (GL #675), built once from the pack's
    /// `[filters]` section.
    pub filters: FilterConfig,
    /// Compiled egress/output DLP config (GL #676), built once from the pack's
    /// `[egress]` section.
    pub egress: crate::core::egress::EgressConfig,
}

impl ActivePolicy {
    pub(crate) fn from_resolved(resolved: ResolvedPolicy) -> Self {
        let redaction = resolved
            .redaction
            .iter()
            .filter_map(|(label, pat)| Regex::new(pat).ok().map(|re| (label.clone(), re)))
            .collect();
        let filters = FilterConfig::new(
            filter_action(resolved.filters.pii.as_ref()),
            filter_action(resolved.filters.classification.as_ref()),
            filter_action(resolved.filters.injection.as_ref()),
            &resolved.filters.blocked_labels,
        );
        let egress = crate::core::egress::EgressConfig::new(
            &resolved.egress.forbidden_patterns,
            resolved.egress.block_secrets.unwrap_or(false),
            resolved.egress.max_writes_per_min,
        );
        Self {
            resolved,
            redaction,
            filters,
            egress,
        }
    }

    /// Whether `tool` is permitted by this policy's allow/deny lists.
    /// `deny_tools` always wins; an `allow_tools` allowlist, when set, is
    /// exclusive (only listed tools pass).
    #[must_use]
    pub fn tool_allowed(&self, tool: &str) -> bool {
        if self.resolved.deny_tools.iter().any(|t| t == tool) {
            return false;
        }
        match &self.resolved.allow_tools {
            Some(allow) => allow.iter().any(|t| t == tool),
            None => true,
        }
    }
}

/// Map a resolved `[filters]` action string to a [`FilterAction`]. Absent or
/// (defensively) unparseable ⇒ `Off`; validation already rejects bad tokens.
fn filter_action(opt: Option<&String>) -> FilterAction {
    opt.map(String::as_str)
        .and_then(FilterAction::parse)
        .unwrap_or(FilterAction::Off)
}

struct Cache {
    loaded: bool,
    active: Option<Arc<ActivePolicy>>,
}

fn cache() -> &'static RwLock<Cache> {
    static CACHE: OnceLock<RwLock<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        RwLock::new(Cache {
            loaded: false,
            active: None,
        })
    })
}

fn load_from_disk() -> Option<Arc<ActivePolicy>> {
    Some(Arc::new(ActivePolicy::from_resolved(load_local_pack()?)))
}

/// The project-local pack (`.lean-ctx/policy.toml`), resolved. `None` when the
/// file is absent or invalid — a malformed local pack must never brick the
/// agent (fail-open); `lean-ctx policy validate` surfaces the same error.
fn load_local_pack() -> Option<ResolvedPolicy> {
    let path = PathBuf::from(PROJECT_PACK_PATH);
    if !path.exists() {
        return None;
    }
    match parse_file(&path).and_then(|p| resolve(&p)) {
        Ok(resolved) => Some(resolved),
        Err(e) => {
            tracing::warn!(
                "policy: ignoring invalid {} ({e}); no local policy enforced",
                path.display()
            );
            None
        }
    }
}

/// The active resolved policy, or `None` when no (valid) project pack exists.
/// Loaded once and cached; call [`reload`] after the pack changes.
#[must_use]
pub fn active() -> Option<Arc<ActivePolicy>> {
    {
        let r = cache().read().expect("policy cache poisoned");
        if r.loaded {
            return r.active.clone();
        }
    }
    let loaded = load_from_disk();
    let mut w = cache().write().expect("policy cache poisoned");
    // Another thread may have loaded between the read and the write; keep the
    // first result so concurrent callers see a stable view.
    if !w.loaded {
        w.loaded = true;
        w.active = loaded;
    }
    w.active.clone()
}

/// Cheap "is a policy active?" probe for the hot path.
#[must_use]
pub fn is_active() -> bool {
    active().is_some()
}

/// Re-read the project pack (e.g. after a `policy` edit). Idempotent.
pub fn reload() {
    let loaded = load_from_disk();
    let mut w = cache().write().expect("policy cache poisoned");
    w.loaded = true;
    w.active = loaded;
}

/// Test hook: force the active policy without touching disk.
#[cfg(test)]
pub fn set_active_for_test(resolved: Option<ResolvedPolicy>) {
    let mut w = cache().write().expect("policy cache poisoned");
    w.loaded = true;
    w.active = resolved.map(|r| Arc::new(ActivePolicy::from_resolved(r)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn rp(allow: Option<Vec<&str>>, deny: Vec<&str>, redaction: &[(&str, &str)]) -> ResolvedPolicy {
        ResolvedPolicy {
            name: "test".into(),
            version: "1.0.0".into(),
            description: "t".into(),
            chain: vec![],
            default_read_mode: None,
            allow_tools: allow.map(|a| a.into_iter().map(String::from).collect()),
            deny_tools: deny.into_iter().map(String::from).collect(),
            max_context_tokens: None,
            audit_retention_days: None,
            redaction: redaction
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<BTreeMap<_, _>>(),
            filters: crate::core::policy::FilterRules::default(),
            egress: crate::core::policy::EgressRules::default(),
        }
    }

    #[test]
    fn deny_list_blocks_listed_tool() {
        let p = ActivePolicy::from_resolved(rp(None, vec!["ctx_url_read"], &[]));
        assert!(!p.tool_allowed("ctx_url_read"));
        assert!(p.tool_allowed("ctx_read"));
    }

    #[test]
    fn allow_list_is_exclusive() {
        let p = ActivePolicy::from_resolved(rp(Some(vec!["ctx_read"]), vec![], &[]));
        assert!(p.tool_allowed("ctx_read"));
        assert!(!p.tool_allowed("ctx_shell"));
    }

    #[test]
    fn compiles_redaction_patterns() {
        let p = ActivePolicy::from_resolved(rp(None, vec![], &[("emp", r"EMP-\d{4}")]));
        assert_eq!(p.redaction.len(), 1);
        assert_eq!(p.redaction[0].0, "emp");
    }
}
