use crate::core::config::CompressionLevel;
use crate::core::rules_canonical::{self as rc, Wrapper};
use crate::tools::CrpMode;

/// Universal instruction cap for all MCP clients (in tokens, not bytes).
const INSTRUCTION_CAP_TOKENS: usize = 800;

/// Token budget for the static instruction skeleton (no session/knowledge
/// state).  Asserted in CI so instruction creep cannot silently tax every
/// session. Measured at compression `Off` (the test pins `compression_level = "off"`)
/// so the budget is deterministic across dev machines, not just clean CI (#498).
/// Raised in reviewed steps: 520→540 / 600→640 for the sharpened ctx_* redirects
/// (#1030), then 540→590 / 640→680 (#609 loop one-liner), then 590→615 / 680→712
/// (proactive `RECOVER` line). Lowered 615→545 / 712→675 by the v5 rules diet
/// (#578): the COMPACT skeleton folded loop/paradox into INTENT (measured ~505 /
/// ~639 + headroom). Clients whose rule file already carries the canonical block
/// get a one-line anchor instead of the skeleton (`client_loads_rules_from_file`)
/// and land far below even this.
#[cfg(test)]
const STATIC_INSTRUCTION_BUDGET_TOKENS: usize = 545;
#[cfg(test)]
const STATIC_INSTRUCTION_BUDGET_TDD_TOKENS: usize = 675;
/// Windows carries a one-line SHELL hint inside the skeleton.
#[cfg(all(test, windows))]
const STATIC_INSTRUCTION_SHELL_HINT_TOKENS: usize = 25;
#[cfg(all(test, not(windows)))]
const STATIC_INSTRUCTION_SHELL_HINT_TOKENS: usize = 0;

#[must_use]
pub fn build_instructions(crp_mode: CrpMode) -> String {
    build_instructions_with_client(crp_mode, "")
}

#[must_use]
pub fn build_instructions_with_client(crp_mode: CrpMode, client_name: &str) -> String {
    let cfg = crate::core::config::Config::load();
    let shadow = true;
    // Cross-channel dedup: if the client auto-loads compression from its own rule
    // file, skip it here to avoid duplicate billing.
    let level = if client_loads_compression_from_file(client_name) {
        CompressionLevel::Off
    } else {
        CompressionLevel::effective(&cfg)
    };
    build_full_instructions(crp_mode, client_name, level, shadow)
}

/// Deterministic STATIC Claude Code instructions for the char-budget test: the
/// cold first-contact handshake surface (skeleton + shell hint + decoder +
/// CLAUDE.md pointer + guidance). It pins `level=Off` and `shadow=false` (the
/// default template), so the result is independent
/// of the developer's local lean-ctx config and the assertion stays
/// deterministic (#498) for every contributor, not just clean CI.
#[must_use]
pub fn claude_code_static_instructions_for_test() -> String {
    build_full_instructions(CrpMode::Off, "", CompressionLevel::Off, false)
}

/// Deterministic variant for tests (no session/knowledge state).
#[must_use]
pub fn build_instructions_for_test(crp_mode: CrpMode) -> String {
    let shadow = false;
    // Resolve the effective compression level from config/env (matches the live
    // build_full_instructions path) so terse/compression env vars are honoured.
    let level = CompressionLevel::effective(&crate::core::config::Config::load());
    let skeleton = rc::render(shadow, Wrapper::Bare, level);
    let shell_hint = build_shell_hint();

    let base = format!(
        "{skeleton}\n\
        {shell_hint}\n\
        {decoder_block}\n\
        {origin}",
        decoder_block =
            crate::core::protocol::instruction_decoder_block(matches!(crp_mode, CrpMode::Tdd)),
        origin = crate::core::integrity::origin_line(),
    );

    match crp_mode_suffix(crp_mode) {
        "" => format!("{base}\n\n{}", rc::INTELLIGENCE),
        crp => format!("{base}\n\n{crp}\n\n{}", rc::INTELLIGENCE),
    }
}

/// Display path for the Claude config directory (respected by CLAUDE_CONFIG_DIR).
#[must_use]
pub fn claude_config_dir_display() -> String {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(dir) if !dir.trim().is_empty() => {
            let dir = dir.trim().to_string();
            if dir.starts_with('~') {
                dir
            } else if let Some(home) = dirs::home_dir() {
                let home_str = home.to_string_lossy();
                if let Some(rest) = dir.strip_prefix(home_str.as_ref()) {
                    format!("~{rest}")
                } else {
                    dir
                }
            } else {
                dir
            }
        }
        _ => "~/.claude".to_string(),
    }
}

// ── MCP per-session instructions builder ──────────────────────

fn build_full_instructions(
    crp_mode: CrpMode,
    client_name: &str,
    level: CompressionLevel,
    shadow: bool,
) -> String {
    let shell_hint = build_shell_hint();

    // Skeleton includes tool-mapping rules + compression prompt (if level active).
    // Shadow mode omits BULLETS/NEVER/CRITICAL automatically.
    //
    // Cross-channel dedup (#578): when the client's own auto-loaded rule file
    // already carries the canonical rules block (Cursor mdc, Codex
    // instructions.md), repeating the skeleton here would bill the same
    // guidance twice on every session. A one-line anchor keeps the binding;
    // the compression payload is deduped separately via `level` above.
    //
    // Hook-covered hosts (GL #1153) get the hook-aware anchor: repeating
    // "ctx_* replaces native tools" to a Cursor whose hooks already compress
    // the native calls re-creates exactly the instruction dissonance the
    // HookCovered rule profile removes.
    let skeleton = if client_loads_rules_from_file(client_name) {
        let anchor = if client_is_hook_covered(client_name) {
            hook_covered_anchor()
        } else {
            skeleton_anchor()
        };
        let compression = rc::compression_text(level);
        if compression.is_empty() {
            anchor
        } else {
            format!("{anchor}\n{compression}")
        }
    } else {
        // #987: shadow claim must match reality. `shadow_mode=true` (the
        // config default) only means the *config* prefers shadow — it does NOT
        // prove interception hooks are actually installed for this client.
        // Emitting "native calls auto-route" when the install denies native
        // tools (replace mode) wastes a round-trip when the agent trusts it.
        // Gate on hook coverage: only clients with installed hooks get the
        // shadow instruction.
        let effective_shadow = shadow && client_is_hook_covered(client_name);
        rc::render(effective_shadow, Wrapper::Bare, level)
    };

    // Pointer to the full rule file (honours CLAUDE_CONFIG_DIR): agents load the
    // detailed instructions on demand from there instead of inlining them.
    let config_dir = claude_config_dir_display();

    let base = format!(
        "{skeleton}\n\
        {shell_hint}\n\
        {decoder_block}\n\
        Full instructions at {config_dir}/CLAUDE.md\n\
        {origin}\n",
        decoder_block =
            crate::core::protocol::instruction_decoder_block(matches!(crp_mode, CrpMode::Tdd)),
        origin = crate::core::integrity::origin_line(),
    );

    // Guidance suffix: CRP mode + general output rule.
    // This is the operational contract — protected from truncation.
    let guidance_suffix = match crp_mode_suffix(crp_mode) {
        "" => rc::INTELLIGENCE.to_string(),
        crp => format!("{crp}\n\n{}", rc::INTELLIGENCE),
    };

    assemble_within_cap(&base, &guidance_suffix, INSTRUCTION_CAP_TOKENS)
}

fn crp_mode_suffix(crp_mode: CrpMode) -> &'static str {
    match crp_mode {
        CrpMode::Off => "",
        CrpMode::Compact => {
            "CRP MODE: compact — omit filler; abbreviate fn,cfg,impl,deps,req,res; \
             diff lines (+/-) only; <=200 tok; trust tool outputs."
        }
        CrpMode::Tdd => {
            "CRP MODE: tdd — max density; Fn refs + diff lines only \
             (+F1:42 | -F1:10-15 | ~F1:42 old->new); <=150 tok; zero narration."
        }
    }
}

fn assemble_within_cap(base: &str, suffix: &str, cap_tokens: usize) -> String {
    use crate::core::tokens::count_tokens;
    let suffix = suffix.trim_end_matches('\n');
    if suffix.is_empty() {
        let full = base.to_string();
        return if count_tokens(&full) > cap_tokens {
            truncate_to_token_cap(&full, cap_tokens)
        } else {
            full
        };
    }

    let full = format!("{base}\n\n{suffix}");
    if count_tokens(&full) <= cap_tokens {
        return full;
    }

    let suffix_tokens = count_tokens(suffix);
    let Some(base_budget) = cap_tokens.checked_sub(suffix_tokens + 1) else {
        return truncate_to_token_cap(&full, cap_tokens);
    };
    let trimmed_base = truncate_to_token_cap(base, base_budget);
    format!("{trimmed_base}\n\n{suffix}")
}

fn truncate_to_token_cap(s: &str, cap_tokens: usize) -> String {
    use crate::core::tokens::count_tokens;
    if count_tokens(s) <= cap_tokens {
        return s.to_string();
    }
    let cuts: Vec<usize> = s.match_indices('\n').map(|(i, _)| i).collect();
    let (mut lo, mut hi) = (0usize, cuts.len());
    let mut best: Option<usize> = None;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let end = cuts[mid];
        if end > 0 && count_tokens(&s[..end]) <= cap_tokens {
            best = Some(end);
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if let Some(end) = best {
        return s[..end].to_string();
    }
    let byte_approx = cap_tokens * 4;
    let safe = s.floor_char_boundary(byte_approx.min(s.len()));
    s[..safe].to_string()
}

/// Backward-compat alias kept for external callers.
#[must_use]
pub fn claude_code_instructions() -> String {
    build_instructions(CrpMode::Off)
}

/// One-line anchor for clients whose rule file carries the canonical block (#578).
/// Only names tools that are registered.
fn skeleton_anchor() -> String {
    if crate::server::registry::is_registered("ctx_compose") {
        "lean-ctx active — your auto-loaded lean-ctx rules apply: \
         ctx_* tools replace native Read/Grep/Shell/Glob (ctx_compose first)."
            .into()
    } else {
        "lean-ctx active — your auto-loaded lean-ctx rules apply: \
         ctx_* tools replace native Read/Grep/Shell/Glob."
            .into()
    }
}

/// Anchor for hook-covered hosts (GL #1153). Only names registered tools.
fn hook_covered_anchor() -> String {
    use crate::server::registry::is_registered;
    let mut s =
        String::from("lean-ctx active — hooks compress native Shell/Read/Grep transparently");
    if is_registered("ctx_compose") {
        s.push_str("; call ctx_compose to orient");
    }
    // #509: ctx_semantic_search folded into ctx_search(action=semantic)
    if is_registered("ctx_session") || is_registered("ctx_knowledge") {
        s.push_str(", ctx_search(action=semantic) / ctx_knowledge for meaning & memory");
    }
    s.push('.');
    s
}

// Test-only backward-compat constants for assertion substrings.
#[cfg(test)]
const SKELETON_ANCHOR: &str = "lean-ctx active — your auto-loaded lean-ctx rules apply: \
    ctx_* tools replace native Read/Grep/Shell/Glob (ctx_compose first).";

fn client_loads_compression_from_file(client_name: &str) -> bool {
    crate::core::home::resolve_home_dir().is_some_and(|home| {
        crate::core::rules_channel::client_autoloads_compression(client_name, &home)
    })
}

fn client_loads_rules_from_file(client_name: &str) -> bool {
    crate::core::home::resolve_home_dir()
        .is_some_and(|home| crate::core::rules_channel::client_autoloads_rules(client_name, &home))
}

fn client_is_hook_covered(client_name: &str) -> bool {
    crate::core::home::resolve_home_dir()
        .is_some_and(|home| crate::core::rules_channel::client_hook_covered(client_name, &home))
}

fn build_shell_hint() -> String {
    if !cfg!(windows) {
        return String::new();
    }
    let name = crate::shell::shell_name();
    let is_posix = matches!(name.as_str(), "bash" | "sh" | "zsh" | "fish");
    if is_posix {
        format!("\nSHELL: {name} (POSIX) — no PowerShell cmdlets.\n")
    } else if name.contains("powershell") || name.contains("pwsh") {
        format!("\nSHELL: {name}. Use PowerShell cmdlets.\n")
    } else {
        format!("\nSHELL: {name}.\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tokens::count_tokens;

    #[test]
    fn guidance_suffix_survives_oversized_base() {
        let base = "SESSION LINE\n".repeat(4000);
        let suffix = "OUTPUT STYLE: expert-terse\nFn refs only, diff lines only.";
        let out = assemble_within_cap(&base, suffix, INSTRUCTION_CAP_TOKENS);
        assert!(out.contains("OUTPUT STYLE: expert-terse"));
        assert!(count_tokens(&out) <= INSTRUCTION_CAP_TOKENS);
        assert!(out.len() < base.len());
    }

    #[test]
    fn empty_client_never_dedups_compression() {
        assert!(!client_loads_compression_from_file(""));
        assert!(!client_loads_compression_from_file("totally-unknown-agent"));
    }

    #[test]
    fn covered_client_gets_anchor_instead_of_skeleton() {
        // #578: a client whose rule file carries the canonical block must not
        // pay for the full skeleton again in every MCP session.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".cursor/rules")).unwrap();
        let shadow = true;
        std::fs::write(
            home.join(".cursor/rules/lean-ctx.mdc"),
            rc::render(
                shadow,
                Wrapper::Dedicated,
                crate::core::config::CompressionLevel::Standard,
            ),
        )
        .unwrap();
        let old_home = std::env::var("HOME").ok();
        crate::test_env::set_var("HOME", home);

        let covered = build_instructions_with_client(CrpMode::Off, "cursor");
        let uncovered = build_instructions_with_client(CrpMode::Off, "some-other-agent");

        if let Some(h) = old_home {
            crate::test_env::set_var("HOME", h);
        } else {
            crate::test_env::remove_var("HOME");
        }

        assert!(
            covered.contains(SKELETON_ANCHOR),
            "covered client must get the anchor:\n{covered}"
        );
        if shadow {
            assert!(
                !covered.contains("auto-route"),
                "covered shadow client must not re-pay the shadow nudge:\n{covered}"
            );
        } else {
            assert!(
                !covered.contains("MANDATORY MAPPING"),
                "covered client must not re-pay the skeleton:\n{covered}"
            );
        }
        // The mdc also carries the compression block → level dedups to Off.
        assert!(
            !covered.contains("OUTPUT STYLE:"),
            "covered client must not re-pay the compression prompt:\n{covered}"
        );
        // #987: uncovered client without hooks no longer gets shadow instruction.
        // `effective_shadow` = false when no hooks installed, so it falls back
        // to the non-shadow (MANDATORY MAPPING) skeleton regardless of config.
        if shadow {
            assert!(
                !uncovered.contains("auto-route"),
                "#987: uncovered client WITHOUT hooks must NOT get shadow nudge:\n{uncovered}"
            );
            assert!(
                uncovered.contains("MANDATORY MAPPING") || uncovered.contains("ctx_"),
                "uncovered client without hooks gets explicit tool mapping:\n{uncovered}"
            );
        } else {
            assert!(
                uncovered.contains("MANDATORY MAPPING"),
                "uncovered client keeps the full skeleton:\n{uncovered}"
            );
        }
        eprintln!(
            "instructions footprint: covered={} tok, uncovered={} tok",
            count_tokens(&covered),
            count_tokens(&uncovered)
        );
        assert!(
            count_tokens(&covered) < count_tokens(&uncovered),
            "anchor path must be strictly cheaper"
        );
    }

    #[test]
    fn hook_covered_client_gets_hook_aware_anchor() {
        // GL #1153: with lean-ctx hooks covering the native tools, the anchor
        // must not repeat "ctx_* replaces native tools" — that is exactly the
        // instruction dissonance the HookCovered profile removes.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".cursor/rules")).unwrap();
        std::fs::write(
            home.join(".cursor/rules/lean-ctx.mdc"),
            rc::render(
                false,
                Wrapper::HookCovered,
                crate::core::config::CompressionLevel::Off,
            ),
        )
        .unwrap();
        std::fs::write(
            home.join(".cursor/hooks.json"),
            r#"{"version":1,"hooks":{"preToolUse":[
                {"matcher":"Shell","command":"/usr/local/bin/lean-ctx hook rewrite"},
                {"matcher":"Read|Grep","command":"/usr/local/bin/lean-ctx hook redirect"}
            ]}}"#,
        )
        .unwrap();
        let old_home = std::env::var("HOME").ok();
        crate::test_env::set_var("HOME", home);

        let covered = build_instructions_with_client(CrpMode::Off, "cursor");

        if let Some(h) = old_home {
            crate::test_env::set_var("HOME", h);
        } else {
            crate::test_env::remove_var("HOME");
        }

        assert!(
            covered.contains("hooks compress native Shell/Read/Grep"),
            "hook-covered client must get the hook-aware anchor:\n{covered}"
        );
        assert!(
            !covered.contains("ctx_* tools replace native")
                && !covered.contains("MANDATORY MAPPING"),
            "hook-covered client must not carry the replace-native wording:\n{covered}"
        );
    }

    #[test]
    fn under_cap_keeps_everything() {
        let base = "tool mapping block";
        let suffix = "OUTPUT STYLE: dense";
        let out = assemble_within_cap(base, suffix, INSTRUCTION_CAP_TOKENS);
        assert!(out.contains(base));
        assert!(out.contains(suffix));
    }

    #[test]
    fn empty_suffix_caps_base_only() {
        let base = "x\n".repeat(4000);
        let out = assemble_within_cap(&base, "", INSTRUCTION_CAP_TOKENS);
        assert!(count_tokens(&out) <= INSTRUCTION_CAP_TOKENS);
    }

    #[cfg(windows)]
    #[test]
    fn shell_hint_stays_within_its_budget() {
        let hint = build_shell_hint();
        let tokens = count_tokens(&hint);
        assert!(
            tokens <= STATIC_INSTRUCTION_SHELL_HINT_TOKENS,
            "shell hint = {tokens} tok, budget {STATIC_INSTRUCTION_SHELL_HINT_TOKENS}: {hint}"
        );
    }

    #[test]
    fn minimal_overhead_instructions_stay_within_budget() {
        const MINIMAL_INSTRUCTION_BUDGET_TOKENS: usize =
            STATIC_INSTRUCTION_BUDGET_TDD_TOKENS + STATIC_INSTRUCTION_SHELL_HINT_TOKENS;
        let _iso = crate::core::data_dir::isolated_data_dir();
        let out = build_instructions(CrpMode::Compact);
        let tokens = count_tokens(&out);
        assert!(
            tokens <= MINIMAL_INSTRUCTION_BUDGET_TOKENS,
            "minimal-overhead instructions = {tokens} tok, budget {MINIMAL_INSTRUCTION_BUDGET_TOKENS}\n---\n{out}\n---"
        );
    }

    #[test]
    fn static_skeleton_stays_within_budget() {
        let iso = crate::core::data_dir::isolated_data_dir();
        // Pin compression Off so the measured skeleton — and thus this budget —
        // is deterministic regardless of the dev's local compression_level (#498).
        std::fs::write(iso.path().join("config.toml"), "compression_level = 0.0\n").unwrap();
        for (mode, base_budget) in [
            (CrpMode::Off, STATIC_INSTRUCTION_BUDGET_TOKENS),
            (CrpMode::Compact, STATIC_INSTRUCTION_BUDGET_TOKENS),
            (CrpMode::Tdd, STATIC_INSTRUCTION_BUDGET_TDD_TOKENS),
        ] {
            let budget = base_budget + STATIC_INSTRUCTION_SHELL_HINT_TOKENS;
            let out = build_instructions_for_test(mode);
            let tokens = count_tokens(&out);
            assert!(
                tokens <= budget,
                "static instructions for {mode:?} = {tokens} tok, budget {budget}\n---\n{out}\n---"
            );
        }
    }
}
