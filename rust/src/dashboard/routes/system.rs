pub(super) fn handle(
    path: &str,
    _query_str: &str,
    _method: &str,
    _body: &str,
) -> Option<(&'static str, &'static str, String)> {
    match path {
        "/metrics" => {
            let prom = crate::core::telemetry::global_metrics().to_prometheus();
            Some(("200 OK", "text/plain; version=0.0.4; charset=utf-8", prom))
        }
        "/api/anomaly" => {
            let s = crate::core::anomaly::summary();
            let json = serde_json::to_string(&s).unwrap_or_else(|_| "[]".to_string());
            Some(("200 OK", "application/json", json))
        }
        "/api/verification" => {
            let snap = crate::core::output_verification::stats_snapshot();
            let json = serde_json::to_string(&snap).unwrap_or_else(|_| "{}".to_string());
            Some(("200 OK", "application/json", json))
        }
        // Protection area (GL #487): the OWASP agentic-risk alignment that
        // `lean-ctx audit` prints, as JSON for the dashboard guards view.
        "/api/owasp" => {
            let mappings = crate::core::owasp_alignment::alignment();
            let json = serde_json::to_string(&mappings).unwrap_or_else(|_| "[]".to_string());
            Some(("200 OK", "application/json", json))
        }
        "/api/slos" => {
            let snap = crate::core::slo::evaluate_quiet();
            let history = crate::core::slo::violation_history(100);
            let payload = serde_json::json!({
                "snapshot": snap,
                "history": history,
            });
            let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
            Some(("200 OK", "application/json", json))
        }
        "/api/feedback" => {
            let store = crate::core::feedback::FeedbackStore::load();
            let json = serde_json::to_string(&store).unwrap_or_else(|_| {
                "{\"error\":\"failed to serialize feedback store\"}".to_string()
            });
            Some(("200 OK", "application/json", json))
        }
        "/api/theme-tokens" => {
            let cfg = crate::core::config::Config::load();
            let theme = crate::core::theme::load_theme(&cfg.theme);
            let color_hex = |c: &crate::core::theme::Color| -> String {
                let crate::core::theme::Color::Hex(h) = c;
                h.clone()
            };
            let payload = serde_json::json!({
                "name": theme.name,
                "tokens": {
                    "primary": color_hex(&theme.primary),
                    "secondary": color_hex(&theme.secondary),
                    "accent": color_hex(&theme.accent),
                    "success": color_hex(&theme.success),
                    "warning": color_hex(&theme.warning),
                    "danger": color_hex(&theme.danger),
                    "muted": color_hex(&theme.muted),
                    "text": color_hex(&theme.text),
                    "surface": color_hex(&theme.surface),
                    "background": color_hex(&theme.background),
                    "barStart": color_hex(&theme.bar_start),
                    "barEnd": color_hex(&theme.bar_end),
                    "border": color_hex(&theme.border),
                },
                "css": theme.to_css_vars(),
            });
            let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
            Some(("200 OK", "application/json", json))
        }
        _ => None,
    }
}
