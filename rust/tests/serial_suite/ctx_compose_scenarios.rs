//! End-to-end coverage for the `ctx_compose` task composer.
//!
//! The library unit tests only cover keyword extraction; these exercise the
//! full `handle()` path (semantic ranking + exact match + symbol body) and the
//! H1 hardening: the semantic stage must never stall the call beyond its budget.

use lean_ctx::tools::CrpMode;
use lean_ctx::tools::ctx_compose;

fn write_corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("auth.rs"),
        "pub fn authenticate_user(token: &str) -> bool {\n    \
         validate_token(token) && !token.is_empty()\n}\n\n\
         fn validate_token(token: &str) -> bool {\n    token.len() > 8\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("config.rs"),
        "pub fn parse_config(path: &str) -> String {\n    \
         std::fs::read_to_string(path).unwrap_or_default()\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn compose_returns_all_sections_with_symbol_body() {
    let dir = write_corpus();

    let (out, tokens) = ctx_compose::handle(
        "how does authenticate_user validate the token",
        &dir.path().to_string_lossy(),
        CrpMode::Off,
    );

    assert!(out.contains("TASK:"), "must echo the task header");
    assert!(
        out.contains("## Ranked files (semantic)"),
        "must contain the semantic ranking section"
    );
    assert!(
        out.contains("## Exact matches"),
        "must contain the exact-match section"
    );
    assert!(
        out.contains("authenticate_user"),
        "exact matches / symbol body must surface the queried symbol:\n{out}"
    );
    assert!(tokens > 0, "token count must be reported");
}

#[test]
fn compose_surfaces_associative_neighbours() {
    let dir = write_corpus();

    // `authenticate_user` lives in auth.rs; config.rs is a same-dir sibling, so
    // the graph connects them and spreading activation from the auth.rs seed
    // must surface config.rs as an associative neighbour.
    let (out, _tokens) = ctx_compose::handle(
        "explain authenticate_user",
        &dir.path().to_string_lossy(),
        CrpMode::Off,
    );

    assert!(
        out.contains("## Related (associative"),
        "associative section should appear when the graph connects files:\n{out}"
    );
    assert!(
        out.contains("config.rs"),
        "the sibling neighbour should be surfaced via spreading activation:\n{out}"
    );
}

/// Two files define a symbol of the same name; the task keywords point at one.
/// Regression for #993: `best_symbol_snippet` used to inline `find_symbols(name)
/// .next()` — an arbitrary match — so a trivial config accessor could win over
/// the OCPP charger method the task was about. It must now pick by path/keyword
/// relevance.
fn write_ambiguous_symbol_corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // The OCPP charger method the task is about — its path carries the keyword.
    std::fs::write(
        dir.path().join("ocpp.rs"),
        "pub fn get_max_current() -> f64 {\n    \
         // offered current from the OCPP meter\n    \
         OCPP_OFFERED_CURRENT_MARKER\n}\n",
    )
    .unwrap();
    // A same-named trivial accessor in an unrelated file — the old wrong pick.
    std::fs::write(
        dir.path().join("actionconfig.rs"),
        "pub fn get_max_current() -> f64 {\n    \
         TRIVIAL_CONFIG_MARKER\n}\n",
    )
    .unwrap();
    dir
}

#[test]
fn compose_disambiguates_same_named_symbol_by_task_keywords() {
    let dir = write_ambiguous_symbol_corpus();
    // `ctx_compose` reads symbol bodies from the graph index. Build the tiny
    // fixture explicitly so this scenario verifies disambiguation rather than
    // depending on whether a prior test happened to warm the index.
    assert!(
        !lean_ctx::core::graph_index::scan(&dir.path().to_string_lossy())
            .files
            .is_empty(),
        "ambiguous-symbol corpus graph must build"
    );

    // "OCPP" in the task keywords points the symbol picker at ocpp.rs.
    let (out, _tokens) = ctx_compose::handle(
        "OCPP charger get_max_current offered current",
        &dir.path().to_string_lossy(),
        CrpMode::Off,
    );

    // The claim is about which same-named symbol gets *inlined* under
    // "Top symbols", not about suppressing the other file everywhere — the
    // semantic "Ranked files" section may still list both. So scope the check
    // to the inlined-bodies section.
    let symbols = out
        .split("## Top symbols")
        .nth(1)
        .expect("compose must produce a Top symbols section for a resolvable symbol");
    assert!(
        symbols.contains("OCPP_OFFERED_CURRENT_MARKER"),
        "must inline the OCPP method's body, disambiguated by task keywords:\n{out}"
    );
    assert!(
        !symbols.contains("TRIVIAL_CONFIG_MARKER"),
        "must not inline the unrelated same-named accessor:\n{out}"
    );
}
