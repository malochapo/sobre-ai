/// Precise, pinned reads (`diff`, `lines:N-M`, `anchored`/`anchored:N-M`)
/// pass bounce-prevention untouched: turning one into `full` would discard
/// the exact window, anchors or delta the caller asked for. Classified by the
/// typed `ReadMode` (`ReadMode::is_precise_pinned_read`); an unparseable mode
/// is not pinned.
fn is_precise_pinned_mode(requested_mode: &str) -> bool {
    requested_mode
        .parse::<crate::tools::sobre_read::ReadMode>()
        .is_ok_and(|m| m.is_precise_pinned_read())
}

#[derive(Debug, Clone)]
pub struct PreDispatchResult {
    pub overridden_mode: Option<String>,
    pub reason: Option<&'static str>,
}

/// The mode a read is served in when this conversation's own reads call for
/// another one. Only bounce-prevention does: a file read compressed and then
/// re-read full (or edited) is read full. Neither the session nor knowledge
/// changes the answer.
pub fn pre_dispatch_read(path: &str, requested_mode: &str) -> PreDispatchResult {
    let no_change = PreDispatchResult {
        overridden_mode: None,
        reason: None,
    };

    if is_precise_pinned_mode(requested_mode) {
        return no_change;
    }

    // An explicit mode=full is already what bounce-prevention would pick.
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

    no_change
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_dispatch_passthrough_for_diff() {
        let result = pre_dispatch_read("src/main.rs", "diff");
        assert!(result.overridden_mode.is_none());
    }

    #[test]
    fn pre_dispatch_passthrough_for_anchored_window() {
        // A windowed anchored:N-M read must keep its hash anchors —
        // bounce-prevention must not clobber it to "full" and silently drop
        // the window.
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
        let result = pre_dispatch_read("anchored-new.yml", "anchored:10-40");
        assert!(
            result.overridden_mode.is_none(),
            "anchored:N-M must not be overridden by bounce-prevention"
        );
        let bare = pre_dispatch_read("anchored-new.yml", "anchored");
        assert!(
            bare.overridden_mode.is_none(),
            "bare anchored mode must not be overridden by bounce-prevention"
        );
    }

    #[test]
    fn pre_dispatch_passthrough_for_lines_multi_select() {
        // `lines:A-B,C-D` is a precise pinned window exactly like
        // `lines:A-B`. Exercises the edit-forced branch: a sobre_patch edit
        // just before the read must not turn the window into a full read.
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
        let forced = pre_dispatch_read("multi-select-971.rs", "map");
        assert_eq!(
            forced.overridden_mode.as_deref(),
            Some("full"),
            "precondition: a recent edit must force a non-pinned mode to full"
        );

        let multi = pre_dispatch_read("multi-select-971.rs", "lines:620-622,1214-1218");
        assert!(
            multi.overridden_mode.is_none(),
            "lines:A-B,C-D must not be overridden by bounce-prevention"
        );

        // Control: the single-range form was already protected.
        let single = pre_dispatch_read("multi-select-971.rs", "lines:620-622");
        assert!(
            single.overridden_mode.is_none(),
            "lines:A-B must not be overridden by bounce-prevention"
        );
    }

    #[test]
    fn pre_dispatch_no_override_without_signals() {
        let result = pre_dispatch_read("src/unknown.rs", "auto");
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
        let result = pre_dispatch_read("new.yml", "auto");
        assert_eq!(result.overridden_mode, Some("full".to_string()));
        assert_eq!(result.reason, Some("bounce-prevention"));
    }

    #[test]
    fn explicit_full_is_never_overridden() {
        let result = pre_dispatch_read("c.rs", "full");
        assert!(result.overridden_mode.is_none());
    }
}
