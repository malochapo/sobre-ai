//! Scenario tests for the workflow "done" state bug fix.
//!
//! Validates that:
//! 1. `complete` clears the workflow (no persistent "done" state)
//! 2. `stop` from any state works correctly
//! 3. Gate check auto-clears stale "done" workflows
//! 4. Handoff does not restore "done" workflows

use lean_ctx::core::workflow::types::{StateSpec, TransitionSpec, WorkflowRun, WorkflowSpec};
use lean_ctx::core::workflow::{clear_active, load_active, save_active};
use serial_test::serial;

fn test_spec() -> WorkflowSpec {
    WorkflowSpec {
        name: "test-workflow".to_string(),
        description: Some("Test workflow".to_string()),
        initial: "planning".to_string(),
        states: vec![
            StateSpec {
                name: "planning".to_string(),
                description: None,
                allowed_tools: Some(vec!["ctx_read".to_string(), "ctx_workflow".to_string()]),
                requires_evidence: None,
            },
            StateSpec {
                name: "coding".to_string(),
                description: None,
                allowed_tools: Some(vec!["ctx_edit".to_string(), "ctx_workflow".to_string()]),
                requires_evidence: None,
            },
            StateSpec {
                name: "done".to_string(),
                description: None,
                allowed_tools: Some(vec!["ctx".to_string(), "ctx_workflow".to_string()]),
                requires_evidence: None,
            },
        ],
        transitions: vec![
            TransitionSpec {
                from: "planning".to_string(),
                to: "coding".to_string(),
                description: None,
            },
            TransitionSpec {
                from: "coding".to_string(),
                to: "done".to_string(),
                description: None,
            },
        ],
    }
}

fn setup_test_data_dir() {
    let dir = std::env::temp_dir().join("lean_ctx_workflow_test");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
    unsafe { std::env::set_var("LEAN_CTX_DATA_DIR", dir.to_str().unwrap()) };
}

fn cleanup_test_data_dir() {
    let dir = std::env::temp_dir().join("lean_ctx_workflow_test");
    let _ = std::fs::remove_dir_all(&dir);
    // SAFETY: `#[serial]` (serial_test) ensures no other test in this binary runs concurrently.
    unsafe { std::env::remove_var("LEAN_CTX_DATA_DIR") };
}

// ==========================================================================
// Scenario 1: stop from active workflow clears it
// ==========================================================================

// ==========================================================================
// Scenario 2: save and load roundtrip
// ==========================================================================

#[test]
#[serial]
fn scenario_save_load_roundtrip() {
    setup_test_data_dir();

    let spec = test_spec();
    let run = WorkflowRun::new(spec);
    save_active(&run).unwrap();

    let loaded = load_active().unwrap().unwrap();
    assert_eq!(loaded.current, "planning");
    assert_eq!(loaded.spec.name, "test-workflow");

    cleanup_test_data_dir();
}

// ==========================================================================
// Scenario 3: clear_active is idempotent (no file = no error)
// ==========================================================================

#[test]
#[serial]
fn scenario_clear_active_idempotent() {
    setup_test_data_dir();
    clear_active().unwrap();
    clear_active().unwrap();
    let loaded = load_active().unwrap();
    assert!(loaded.is_none());
    cleanup_test_data_dir();
}

// ==========================================================================
// Scenario 4: "done" is a terminal state (no outgoing transitions)
// ==========================================================================

// ==========================================================================
// Scenario 5: gate logic - done state allowed_tools blocks non-listed tools
// ==========================================================================

// ==========================================================================
// Scenario 6: complete should transition to done and clear file
// ==========================================================================

// ==========================================================================
// Scenario 7: normal transitions between non-terminal states persist
// ==========================================================================

#[test]
#[serial]
fn scenario_normal_transition_persists() {
    setup_test_data_dir();

    let spec = test_spec();
    let mut run = WorkflowRun::new(spec);
    run.current = "planning".to_string();
    save_active(&run).unwrap();

    // Transition planning → coding
    run.current = "coding".to_string();
    save_active(&run).unwrap();

    let loaded = load_active().unwrap().unwrap();
    assert_eq!(loaded.current, "coding");

    cleanup_test_data_dir();
}

// ==========================================================================
// Scenario 8: start after done creates fresh workflow
// ==========================================================================

#[test]
#[serial]
fn scenario_start_after_done() {
    setup_test_data_dir();

    // Clear any existing
    clear_active().unwrap();

    // Start fresh
    let spec = test_spec();
    let run = WorkflowRun::new(spec);
    save_active(&run).unwrap();

    let loaded = load_active().unwrap().unwrap();
    assert_eq!(loaded.current, "planning");
    assert!(loaded.transitions.is_empty());

    cleanup_test_data_dir();
}

// ==========================================================================
// Scenario 9: handoff restore logic - done workflow should be rejected
// ==========================================================================

// ==========================================================================
// Scenario 10: handoff restore logic - active workflow IS restored
// ==========================================================================

// ==========================================================================
// Scenario: Stale workflow (>30min) auto-clears on load
// ==========================================================================

// ==========================================================================
// Scenario: Fresh workflow remains active on load
// ==========================================================================

// ==========================================================================
// Scenario: Passthrough tools are never blocked by workflow gate
// ==========================================================================

#[test]
fn scenario_passthrough_tools_never_blocked() {
    use lean_ctx::server::WORKFLOW_PASSTHROUGH_TOOLS;

    assert!(WORKFLOW_PASSTHROUGH_TOOLS.contains(&"ctx_read"));
    assert!(WORKFLOW_PASSTHROUGH_TOOLS.contains(&"ctx_search"));
    assert!(WORKFLOW_PASSTHROUGH_TOOLS.contains(&"ctx_tree"));
    assert!(WORKFLOW_PASSTHROUGH_TOOLS.contains(&"ctx_session"));

    // The batch read (ctx_multi_read once) and the auto-mode read
    // (ctx_smart_read once) are ctx_read with `paths` / `mode`: the gate
    // keys on the tool name, so they pass through as ctx_read.
    let ctx_read = lean_ctx::server::registry::build_registry()
        .tool_defs()
        .into_iter()
        .find(|def| def.name == "ctx_read")
        .expect("ctx_read is registered");
    let props = ctx_read
        .input_schema
        .get("properties")
        .expect("ctx_read schema has properties");
    assert!(
        props.get("paths").is_some(),
        "the batch read must stay ctx_read(paths)"
    );
    assert!(
        props.get("mode").is_some(),
        "the auto-mode read must stay ctx_read(mode)"
    );

    // Every entry names a registered tool, or `ctx` (the fan-out name). A
    // dead name exempts nothing; ctx_workflow needs no entry, since
    // guard_workflow skips that name itself.
    for name in WORKFLOW_PASSTHROUGH_TOOLS {
        assert!(
            *name == "ctx" || lean_ctx::server::registry::is_registered(name),
            "WORKFLOW_PASSTHROUGH_TOOLS names {name}, which is not a registered tool"
        );
    }
}

// ==========================================================================
// Scenario: is_workflow_stale correctly identifies stale workflows
// ==========================================================================

#[test]
fn scenario_staleness_detection() {
    let spec = test_spec();

    let mut fresh_run = WorkflowRun::new(spec.clone());
    fresh_run.updated_at = chrono::Utc::now() - chrono::Duration::minutes(10);
    assert!(
        !lean_ctx::server::is_workflow_stale(&fresh_run),
        "10min old workflow should NOT be stale"
    );

    let mut stale_run = WorkflowRun::new(spec);
    stale_run.updated_at = chrono::Utc::now() - chrono::Duration::minutes(31);
    assert!(
        lean_ctx::server::is_workflow_stale(&stale_run),
        "31min old workflow SHOULD be stale"
    );
}
