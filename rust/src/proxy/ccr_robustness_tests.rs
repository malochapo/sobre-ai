//! CCR robustness regression suite (#983) — guards the content-addressed
//! recovery path against the classes of bug that bit comparable context layers
//! (Headroom #1023/#1209/#1236/#389/#1141/#1182/#709/#1450): a lossy rewrite
//! that drops a handle it cannot back, a retrieval after the tee file is gone
//! past its TTL, an in-band splice on a streaming request, the tee store
//! colliding with the read-stub bookkeeping, and the cold-stub index resurrecting
//! content across a restart.
//!
//! Lives in-crate (not `rust/tests/`) because the invariants are over
//! `pub(crate)` internals — `ccr::{persist*, resolve_tee, MIN_TEE_BYTES,
//! }` and the read-stub index — that an
//! external integration crate cannot reach without weakening encapsulation.

use serde_json::json;

use crate::core::data_dir::test_env_lock;
use crate::core::hasher::hash_short;
use crate::proxy::ccr::{MIN_TEE_BYTES, persist, persist_json, persist_tabular, resolve_tee};
use crate::tools::ctx_expand;

/// A verbatim original comfortably above [`MIN_TEE_BYTES`], so the persist gate
/// always mints a handle (the sub-threshold case is its own test).
fn big(seed: &str) -> String {
    let line = format!("{seed} ");
    let mut s = String::new();
    while s.len() < MIN_TEE_BYTES + 64 {
        s.push_str(&line);
        s.push('\n');
    }
    s
}

/// Gap 1 — a lossy crush must never emit a handle it cannot back: below
/// [`MIN_TEE_BYTES`] every producer prefix returns `None`, so the caller keeps
/// the data verbatim instead of dropping a column behind a dead handle.
#[test]
fn persist_below_min_tee_bytes_yields_no_handle_for_any_prefix() {
    let _lock = test_env_lock();
    let small = "too small to bother persisting";
    assert!(small.len() < MIN_TEE_BYTES);
    assert!(persist(small).is_none());
    assert!(persist_json(small).is_none());
    assert!(persist_tabular(small).is_none());
}

/// Gap 2 — handles are content-addressed (idempotent, cache-safe #448/#498) and
/// segregated per producer, so the new `tbl_` store never aliases `proxy_`/`json_`.
#[test]
fn persist_is_idempotent_content_addressed_and_prefix_segregated() {
    let _lock = test_env_lock();
    let body = big("verbatim original row");
    let proxy_a = persist(&body).unwrap();
    let proxy_b = persist(&body).unwrap();
    assert_eq!(proxy_a, proxy_b, "same content -> same handle (cache-safe)");

    let json = persist_json(&body).unwrap();
    let tbl = persist_tabular(&body).unwrap();
    assert!(proxy_a.contains("proxy_") && json.contains("json_") && tbl.contains("tbl_"));
    assert_ne!(proxy_a, json);
    assert_ne!(json, tbl);
    assert_ne!(proxy_a, tbl);
    for h in [&proxy_a, &json, &tbl] {
        assert!(resolve_tee(h).is_some(), "handle resolves: {h}");
    }
}

/// Gap 3 — once the tee file is gone (24h TTL cleanup), retrieval degrades to a
/// graceful not-found message; it never panics and never serves stale content.
#[test]
fn ctx_expand_is_graceful_when_tee_file_deleted_past_ttl() {
    let _lock = test_env_lock();
    let body = big("recoverable until the ttl lapses");
    let handle = persist(&body).unwrap();
    let path = resolve_tee(&handle).expect("resolves before deletion");
    std::fs::remove_file(&path).expect("simulate 24h TTL cleanup");

    let out = ctx_expand::handle(&json!({ "id": handle }));
    assert!(
        out.contains("not found"),
        "graceful message expected: {out}"
    );
    assert!(
        !out.contains("recoverable until"),
        "must not serve stale content"
    );
}

/// Gap 4 — the surgical selectors over a tee handle return exactly the requested
/// slice (the whole point of CCR: pull back a slice, not the entire original).
#[test]
fn ctx_expand_surgical_slices_over_tee_handle() {
    let _lock = test_env_lock();
    let body = (1..=60)
        .map(|i| format!("output row {i:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.len() >= MIN_TEE_BYTES);
    let handle = persist(&body).unwrap();

    let head = ctx_expand::handle(&json!({ "id": handle, "head": 2 }));
    assert!(head.contains("output row 001") && head.contains("output row 002"));
    assert!(
        !head.contains("output row 010"),
        "head leaked beyond 2: {head}"
    );

    let tail = ctx_expand::handle(&json!({ "id": handle, "tail": 2 }));
    assert!(tail.contains("output row 059") && tail.contains("output row 060"));
    assert!(
        !tail.contains("output row 001"),
        "tail leaked the head: {tail}"
    );

    let search = ctx_expand::handle(&json!({ "id": handle, "search": "row 042" }));
    assert!(search.contains("output row 042") && !search.contains("output row 001"));

    let range = ctx_expand::handle(&json!({ "id": handle, "start_line": 5, "end_line": 6 }));
    assert!(range.contains("output row 005") && range.contains("output row 006"));
    assert!(!range.contains("output row 004") && !range.contains("output row 007"));
}

/// Gap 5 — the tee resolver rejects path traversal, malformed hex (across every
/// prefix incl. the new `tbl_`) and a reference-store id, so a crafted handle can
/// never escape the store or alias another store (store separation + #936 ladder).
#[test]
fn resolve_tee_rejects_traversal_nontee_and_bad_hex_across_prefixes() {
    let _lock = test_env_lock();
    for bad in [
        "/etc/passwd",
        "../../secret",
        "proxy_nothex0000000.log",
        "json_zzzzzzzzzzzzzzzz.log",
        "tbl_zzzzzzzzzzzzzzzz.log",
        "ref_deadbeefdeadbeef", // reference-store id, not a tee
        "deadbeefdeadbeef",     // right shape, no backing file
    ] {
        assert!(resolve_tee(bad).is_none(), "must reject: {bad}");
    }
}

/// Gap 8 — end-to-end for the lossy tabular crusher (#982): the dropped
/// high-entropy column is absent from the emitted text yet fully recoverable
/// out-of-band through the same `ctx_expand` path the footer advertises.
#[test]
fn tabular_lossy_dropped_column_is_recoverable_via_ctx_expand() {
    let _lock = test_env_lock();
    let mut csv = String::from("status,uuid\n");
    for i in 0..50 {
        csv.push_str(&format!("ok,uuid-{i:08}\n"));
    }
    assert!(csv.len() >= MIN_TEE_BYTES);

    let res = crate::core::tabular_crush::crush_text_lossy_if_beneficial(&csv, ',', 0.9)
        .expect("lossy crush drops the high-entropy column");
    assert!(!res.lossless, "dropping a column is lossy");
    assert!(
        !res.text.contains("uuid-00000042"),
        "the dropped value is gone from the text"
    );

    let handle = persist_tabular(&csv).expect("tbl handle");
    let out = ctx_expand::handle(&json!({ "id": handle, "search": "uuid-00000042" }));
    assert!(
        out.contains("uuid-00000042"),
        "dropped datum recoverable out-of-band: {out}"
    );
}

/// Gap 9 — the read-stub index persists only delivery *bookkeeping*, never the
/// file content, and that bookkeeping survives a simulated daemon restart so a
/// re-read collapses to the cheap `[unchanged]` stub (#955).
#[test]
#[serial_test::serial(stub_index)]
fn read_stub_bookkeeping_survives_restart_without_storing_content() {
    use crate::core::read_stub_index as rsi;

    rsi::clear_for_test();
    let dir = tempfile::tempdir().unwrap();
    let secret = "TOP-SECRET-FILE-BODY-MUST-NOT-PERSIST";
    rsi::record(rsi::StubRecord::new(
        "/proj/handover.md".to_string(),
        hash_short(secret), // the hash, never the content
        Some(std::time::SystemTime::now()),
        128,
        "F1".to_string(),
        Some("conv-restart".to_string()),
    ));
    rsi::persist_to_dir(dir.path());

    // Simulate a restart: wipe the in-memory store, then reload from disk.
    rsi::clear_for_test();
    assert!(
        rsi::lookup("/proj/handover.md").is_none(),
        "post-restart memory starts empty"
    );
    rsi::load_from_dir(dir.path());
    let back = rsi::lookup("/proj/handover.md").expect("bookkeeping survived the restart");
    assert_eq!(back.line_count, 128);

    let on_disk =
        std::fs::read_to_string(dir.path().join("read_cache").join("stub_index.json")).unwrap();
    assert!(
        !on_disk.contains(secret),
        "the index must hold bookkeeping only, never file content (#955)"
    );
    rsi::clear_for_test();
}
