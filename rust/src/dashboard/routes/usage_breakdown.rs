//! Usage breakdown — `GET /api/usage-breakdown`.
//!
//! This machine's measured spend (`proxy_usage.json`, real models + billed
//! tokens) as model-level rows, plus the savings-ledger's last-30-days USD as
//! the savings total. A local snapshot has no person/project dimension and no
//! per-model savings split — those fields are empty/zero rather than invented.


pub(super) fn handle(
    path: &str,
    _query_str: &str,
    method: &str,
    _body: &str,
) -> Option<(&'static str, &'static str, String)> {
    if path != "/api/usage-breakdown" {
        return None;
    }
    if !method.eq_ignore_ascii_case("GET") {
        return Some((
            "405 Method Not Allowed",
            "application/json",
            super::helpers::json_err("use GET to read the usage breakdown"),
        ));
    }

    let payload: Result<serde_json::Value, String> = Ok(local_snapshot());
    Some(match payload {
        Ok(json) => ("200 OK", "application/json", json.to_string()),
        Err(e) => (
            "502 Bad Gateway",
            "application/json",
            super::helpers::json_err(&e),
        ),
    })
}

/// Model-level spend from this machine's measured proxy usage + ledger savings.
fn local_snapshot() -> serde_json::Value {
    let spend = crate::proxy::usage_meter::persisted_snapshot();
    let rows: Vec<serde_json::Value> = spend
        .iter()
        .map(|m| {
            serde_json::json!({
                // No identity dimension in a local snapshot — empty, not invented.
                "person": "",
                "project": "",
                "model": m.model,
                "provider": "",
                "requests": m.requests,
                "input_tokens": m.input_tokens,
                "output_tokens": m.output_tokens,
                "cost_usd": m.cost_usd,
                "saved_tokens": 0,
                "saved_usd": 0.0,
            })
        })
        .collect();

    let ledger = crate::core::savings_ledger::summary();
    let now = chrono::Utc::now();
    let cutoff = (now - chrono::Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    // Ledger savings over the last 30 days: the projection window matches the
    // admin API's default so central and local numbers are comparable.
    let saved_usd_30d: f64 = ledger
        .by_day
        .iter()
        .filter(|(day, _, _)| day.as_str() >= cutoff.as_str())
        .map(|(_, _, usd)| usd)
        .sum();

    let cost_usd: f64 = spend.iter().map(|m| m.cost_usd).sum();
    let requests: u64 = spend.iter().map(|m| m.requests).sum();

    let mut json = serde_json::json!({
        "from": (now - chrono::Duration::days(30)).to_rfc3339(),
        "to": now.to_rfc3339(),
        "rows": rows,
        "totals": {
            "requests": requests,
            "cost_usd": cost_usd,
            "saved_usd": saved_usd_30d,
            "reference_cost_usd": 0.0,
            "active_persons": 1,
        },
    });
    annotate(&mut json, "local-snapshot");
    json
}

/// Stamps the data source onto the response (cockpit header).
fn annotate(json: &mut serde_json::Value, source: &str) {
    if let Some(obj) = json.as_object_mut() {
        obj.insert("source".into(), serde_json::json!(source));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_matching_paths_pass_through() {
        assert!(handle("/api/other", "", "GET", "").is_none());
    }

    #[test]
    fn post_is_rejected() {
        let (status, _, _) = handle("/api/usage-breakdown", "", "POST", "").expect("handled");
        assert_eq!(status, "405 Method Not Allowed");
    }

    #[test]
    fn local_snapshot_shape_is_stable() {
        // Contract fields the cockpit view binds to.
        let json = local_snapshot();
        assert_eq!(json["source"], "local-snapshot");
        assert_eq!(json["totals"]["active_persons"], 1);
        assert!(json["rows"].is_array());
        assert!(json["totals"]["cost_usd"].is_number());
        assert!(json["totals"]["saved_usd"].is_number());
    }
}
