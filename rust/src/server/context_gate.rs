/// #843: precise, pinned reads (`diff`, `lines:N-M`, `anchored`/`anchored:N-M`)
/// must pass through every mode-override path untouched — bounce-prevention,
/// pressure-downgrade, and the graph/knowledge heuristics below must never
/// silently reinterpret one of these to e.g. `full`, which would discard the
/// exact window/anchors/delta the caller asked for. Classifies off the typed
/// `ReadMode` (single source of truth, see `tools::ctx_read::mode`) rather than
/// a hand-maintained string-prefix list, so a future precise mode has to be
/// added to `ReadMode::is_precise_pinned_read` explicitly instead of silently
/// falling through an allowlist. An unparseable mode is conservatively treated
/// as *not* pinned, matching prior behaviour for anything outside this set.
fn is_precise_pinned_mode(requested_mode: &str) -> bool {
    requested_mode
        .parse::<crate::tools::ctx_read::ReadMode>()
        .is_ok_and(|m| m.is_precise_pinned_read())
}

#[derive(Debug, Clone)]
pub struct PreDispatchResult {
    pub overridden_mode: Option<String>,
    pub reason: Option<&'static str>,
}

pub fn pre_dispatch_read(
    path: &str,
    requested_mode: &str,
    task: Option<&str>,
    project_root: Option<&str>,
) -> PreDispatchResult {
    let no_change = PreDispatchResult {
        overridden_mode: None,
        reason: None,
    };

    if is_precise_pinned_mode(requested_mode) {
        return no_change;
    }

    // Explicit mode=full must not be downgraded by pressure or other heuristics.
    if requested_mode == "full" {
        return no_change;
    }

    if let Ok(bt) = crate::core::bounce_tracker::global().lock()
        && bt.should_force_full(path)
    {
        return PreDispatchResult {
            overridden_mode: Some("full".to_string()),
            reason: Some("bounce-prevention"),
        };
    }

    if let Some(task_str) = task {
        let intent = crate::core::intent_engine::StructuredIntent::from_query(task_str);
        let norm = crate::core::pathutil::normalize_tool_path(path);
        let is_target = intent
            .targets
            .iter()
            .any(|t| norm.ends_with(t) || norm.contains(t));
        if is_target {
            return PreDispatchResult {
                overridden_mode: Some("full".to_string()),
                reason: Some("intent-target"),
            };
        }
    }

    if let Some(root) = project_root
        && let Some(open) = try_load_graph(root)
    {
        let gp = &open.provider;
        let related = gp.related(path, 1);
        if let Some(task_str) = task {
            let intent = crate::core::intent_engine::StructuredIntent::from_query(task_str);
            for target in &intent.targets {
                let target_related = gp.related(target, 1);
                let norm = crate::core::pathutil::normalize_tool_path(path);
                if target_related
                    .iter()
                    .any(|r| r.contains(&norm) || norm.contains(r))
                {
                    return PreDispatchResult {
                        overridden_mode: Some("map".to_string()),
                        reason: Some("graph-direct-import"),
                    };
                }
            }
        }
        if !related.is_empty() && requested_mode == "auto" {
            let reverse_deps = gp.dependents(path);
            if reverse_deps.len() > 3 {
                return PreDispatchResult {
                    overridden_mode: Some("map".to_string()),
                    reason: Some("graph-hub-file"),
                };
            }
        }
    }

    if let Some(root) = project_root
        && let Some(knowledge) = crate::core::knowledge::ProjectKnowledge::load(root)
    {
        let norm = crate::core::pathutil::normalize_tool_path(path);
        let mentions = knowledge
            .facts
            .iter()
            .filter(|f| f.value.contains(&norm) || f.key.contains(&norm))
            .count();
        if mentions >= 3 {
            return PreDispatchResult {
                overridden_mode: Some("map".to_string()),
                reason: Some("knowledge-high-relevance"),
            };
        }
    }

    no_change
}

fn try_load_graph(project_root: &str) -> Option<crate::core::graph_provider::OpenGraphProvider> {
    crate::core::graph_provider::open_best_effort(project_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_dispatch_passthrough_for_diff() {
        let result = pre_dispatch_read("src/main.rs", "diff", None, None);
        assert!(result.overridden_mode.is_none());
    }

    #[test]
    fn pre_dispatch_passthrough_for_anchored_window() {
        // #843: a windowed anchored:N-M read must keep its hash anchors —
        // bounce-prevention, pressure-downgrade, etc. must not clobber it to
        // "full" and silently drop the window.
        {
            let mut bt = crate::core::bounce_tracker::global()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bt.set_seq(101);
            bt.record_read("anchored-bouncy.yml", "map", 30, 400);
            bt.set_seq(102);
            bt.record_read("anchored-bouncy.yml", "full", 400, 400);
            bt.set_seq(103);
            bt.record_read("a2.yml", "map", 30, 400);
            bt.set_seq(104);
            bt.record_read("a2.yml", "full", 400, 400);
            bt.set_seq(105);
            bt.record_read("a3.yml", "map", 30, 400);
            bt.set_seq(106);
            bt.record_read("a3.yml", "full", 400, 400);
        }
        let result = pre_dispatch_read("anchored-new.yml", "anchored:10-40", None, None);
        assert!(
            result.overridden_mode.is_none(),
            "anchored:N-M must not be overridden by bounce-prevention"
        );
        let bare = pre_dispatch_read("anchored-new.yml", "anchored", None, None);
        assert!(
            bare.overridden_mode.is_none(),
            "bare anchored mode must not be overridden by bounce-prevention"
        );
    }

    #[test]
    fn pre_dispatch_passthrough_for_lines_multi_select() {
        // #971: `lines:A-B,C-D` is a precise pinned window exactly like
        // `lines:A-B`, but it parsed as Malformed, so `is_precise_pinned_mode`
        // reported "not pinned" and bounce-prevention rewrote an 8-line request
        // into a full-file read. Exercises the edit-forced branch, which is what
        // the field report actually hit (a ctx_patch anchored edit immediately
        // before the read).
        {
            let mut bt = crate::core::bounce_tracker::global()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bt.set_seq(201);
            bt.record_edit("multi-select-971.rs");
            bt.set_seq(202);
        }

        // Precondition: the tracker really is armed for this path, so the
        // assertions below cannot pass vacuously.
        let forced = pre_dispatch_read("multi-select-971.rs", "map", None, None);
        assert_eq!(
            forced.overridden_mode.as_deref(),
            Some("full"),
            "precondition: a recent edit must force a non-pinned mode to full"
        );

        let multi = pre_dispatch_read("multi-select-971.rs", "lines:620-622,1214-1218", None, None);
        assert!(
            multi.overridden_mode.is_none(),
            "lines:A-B,C-D must not be overridden by bounce-prevention (#971)"
        );

        // Control: the single-range form was already protected.
        let single = pre_dispatch_read("multi-select-971.rs", "lines:620-622", None, None);
        assert!(
            single.overridden_mode.is_none(),
            "lines:A-B must not be overridden by bounce-prevention"
        );
    }

    #[test]
    fn pre_dispatch_no_override_without_signals() {
        let result = pre_dispatch_read("src/unknown.rs", "auto", None, None);
        assert!(result.overridden_mode.is_none());
    }

    #[test]
    fn pre_dispatch_bounce_prevention_forces_full() {
        {
            let mut bt = crate::core::bounce_tracker::global()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bt.set_seq(1);
            bt.record_read("src/bouncy.yml", "map", 30, 400);
            bt.set_seq(2);
            bt.record_read("src/bouncy.yml", "full", 400, 400);
            bt.set_seq(3);
            bt.record_read("a2.yml", "map", 30, 400);
            bt.set_seq(4);
            bt.record_read("a2.yml", "full", 400, 400);
            bt.set_seq(5);
            bt.record_read("a3.yml", "map", 30, 400);
            bt.set_seq(6);
            bt.record_read("a3.yml", "full", 400, 400);
        }
        let result = pre_dispatch_read("new.yml", "auto", None, None);
        assert_eq!(result.overridden_mode, Some("full".to_string()));
        assert_eq!(result.reason, Some("bounce-prevention"));
    }

    #[test]
    fn pressure_does_not_downgrade_explicit_full() {
        let result = pre_dispatch_read("c.rs", "full", None, None);
        assert!(
            result.overridden_mode.is_none(),
            "explicit mode=full must never be downgraded by pressure"
        );
    }

    #[test]
    fn pressure_does_not_downgrade_when_enforce_off() {
        // Default profile has degradation.enforce = false, so pressure
        // should NOT downgrade any mode.
        let result = pre_dispatch_read("c.rs", "map", None, None);
        assert!(
            result.overridden_mode.is_none(),
            "pressure must not downgrade when degradation.enforce is off"
        );
    }
}
