use super::*;
use std::path::Path;

fn write(home: &Path, rel: &str, content: &str) {
    let p = home.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn check(home: &Path) -> Outcome {
    claude_skill_check(home)
}

#[test]
fn recent_sessions_line_names_each_workspace_once_with_age() {
    use crate::core::session::SessionSummary;
    let now = chrono::Utc::now();
    let mk = |root: Option<&str>, mins_ago: i64| SessionSummary {
        id: format!("s{mins_ago}"),
        updated_at: now - chrono::Duration::minutes(mins_ago),
        version: 1,
        task: None,
        tool_calls: 3,
        tokens_saved: 100,
        project_root: root.map(str::to_string),
    };

    // Two windows on two projects + a stale duplicate of the first root +
    // a rootless session that must be skipped.
    let line = environment::format_recent_sessions(
        vec![
            mk(Some("/Users/me/work/frontend"), 4),
            mk(Some("/Users/me/work/backend"), 90),
            mk(Some("/Users/me/work/frontend"), 200),
            mk(None, 1),
        ],
        now,
    )
    .expect("sessions exist");

    assert_eq!(line, "recent: frontend (4m ago), backend (1h ago)");
}

#[test]
fn recent_sessions_line_is_none_without_any_rooted_session() {
    let now = chrono::Utc::now();
    assert_eq!(environment::format_recent_sessions(vec![], now), None);
}

#[test]
fn humanized_ages_cover_all_magnitudes() {
    use chrono::Duration;
    assert_eq!(environment::humanize_age(Duration::seconds(20)), "just now");
    assert_eq!(environment::humanize_age(Duration::minutes(59)), "59m ago");
    assert_eq!(environment::humanize_age(Duration::hours(47)), "47h ago");
    assert_eq!(environment::humanize_age(Duration::days(3)), "3d ago");
}

#[test]
fn capacity_hint_is_actionable_for_both_states() {
    // WARN (at/near cap): reassure it is by-design.
    let warn = capacity_hint(false, Some("/p"));
    assert!(warn.contains("healthy by design"));

    // CRIT (over cap): give an immediate compaction action for that project.
    let crit = capacity_hint(true, Some("/p"));
    assert!(crit.contains(
        r#"sobre call sobre_knowledge --project-root '/p' --json '{"action":"consolidate"}'"#
    ));

    assert_ne!(warn, crit);
}

#[test]
fn agent_config_dir_matches_both_separators() {
    for sep in ['/', '\\'] {
        for dir in [".lmstudio", ".claude", ".codex"] {
            let cwd = format!("C:{sep}Users{sep}me{sep}{dir}{sep}mcp");
            assert!(
                crate::core::pathutil::is_agent_config_dir(std::path::Path::new(&cwd)),
                "expected {cwd} to be flagged as an agent dir"
            );
        }
    }
}

#[test]
fn agent_config_dir_ignores_real_projects() {
    for cwd in [
        "/home/me/work/myproj",
        "/Users/me/code/sobre",
        "C:\\src\\app",
    ] {
        assert!(
            !crate::core::pathutil::is_agent_config_dir(std::path::Path::new(cwd)),
            "{cwd} is a real project and must not be flagged"
        );
    }
}

// `serial(claude_config_dir)`: `claude_state_dir` honours the process-global
// `CLAUDE_CONFIG_DIR`; a concurrent setter would make the check read the wrong
// `.claude` dir.
#[test]
#[serial_test::serial(claude_config_dir)]
fn installed_skill_passes() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        ".claude/skills/sobre/SKILL.md",
        crate::setup::skill::SKILL_MD,
    );
    let out = check(tmp.path());
    assert!(out.ok, "an installed skill must pass: {}", out.line);
}

#[test]
#[serial_test::serial(claude_config_dir)]
fn outdated_skill_fails() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        ".claude/skills/sobre/SKILL.md",
        "an older guide",
    );
    assert!(!check(tmp.path()).ok);
}

#[test]
#[serial_test::serial(claude_config_dir)]
fn nothing_installed_fails_and_suggests_setup() {
    let tmp = tempfile::tempdir().unwrap();
    let out = check(tmp.path());
    assert!(!out.ok);
    assert!(
        out.line.contains("sobre init"),
        "must suggest a command that actually fixes it: {}",
        out.line
    );
}

#[test]
fn unknown_config_keys_flags_typos_only() {
    let raw = "compresion_level = 0.5\nshell_timeout_secs = 30\n\n\
               [proxy]\nport = 4444\n\n[proxy.upstreams]\nanthropic = \"https://x\"\n";
    let unknown = unknown_config_keys(raw);
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert!(
        unknown[0].starts_with("compresion_level, did you mean compression_level"),
        "{unknown:?}"
    );
    assert!(unknown_config_keys("not = = toml").is_empty());

    let unknown =
        unknown_config_keys("[proxy]\nprot = 4444\n\n[proxy.upstreams]\nantrhopic = \"x\"\n");
    assert_eq!(unknown.len(), 2, "{unknown:?}");
    assert!(
        unknown[0].starts_with("proxy.prot, did you mean proxy.port"),
        "{unknown:?}"
    );
    assert!(
        unknown[1].starts_with("proxy.upstreams.antrhopic"),
        "{unknown:?}"
    );

    // No table under [proxy] is free-form.
    let unknown = unknown_config_keys("[proxy.routing.aliases]\n\"gpt-4.1\" = \"gpt-4.1-mini\"\n");
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert!(
        unknown[0].starts_with("proxy.routing.aliases."),
        "{unknown:?}"
    );

    // The file `config init` writes is clean.
    let schema = crate::core::config::schema::ConfigSchema::generate();
    let rendered = crate::core::config::render_annotated_config(
        &crate::core::config::Config::default(),
        &schema,
    );
    assert!(unknown_config_keys(&rendered).is_empty(), "{rendered}");
}

#[test]
fn local_model_upstream_passes_and_only_the_proxy_itself_fails() {
    let with_anthropic = |url: &str| {
        let mut proxy = crate::core::config::ProxyConfig::default();
        proxy.upstreams.insert("anthropic".into(), url.into());
        proxy
    };

    // Ollama on loopback, as the README documents it.
    let ollama = proxy::upstream_outcome(&with_anthropic("http://127.0.0.1:11434"), 4444);
    assert!(ollama.ok, "{}", ollama.line);
    assert!(
        ollama.line.contains("Anthropic=http://127.0.0.1:11434"),
        "{}",
        ollama.line
    );
    assert!(
        !ollama.line.contains("plaintext"),
        "loopback is not a network hop: {}",
        ollama.line
    );

    for own in ["http://127.0.0.1:4444", "http://localhost:4444/v1"] {
        let looped = proxy::upstream_outcome(&with_anthropic(own), 4444);
        assert!(!looped.ok, "{own}: {}", looped.line);
        assert!(looped.line.contains("proxy itself"), "{}", looped.line);
    }

    let remote = proxy::upstream_outcome(&with_anthropic("http://10.0.0.5:8080"), 4444);
    assert!(
        remote.ok && remote.line.contains("plaintext"),
        "{}",
        remote.line
    );
}

/// An upstream the proxy cannot use fails the check: the proxy would
/// send those requests to the provider's default instead.
#[test]
fn an_invalid_upstream_fails() {
    for url in ["127.0.0.1:11434", "notaurl", "ftp://x.y"] {
        let mut proxy = crate::core::config::ProxyConfig::default();
        proxy.upstreams.insert("anthropic".into(), url.into());
        let outcome = proxy::upstream_outcome(&proxy, 4444);
        assert!(!outcome.ok, "{url}: {}", outcome.line);
        assert!(
            outcome.line.contains("invalid Anthropic upstream"),
            "{}",
            outcome.line
        );
    }
}
