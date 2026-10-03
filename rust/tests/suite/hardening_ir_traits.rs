//! Integration tests for the hardening sprint:
//! - Context IR hot-path recording
//! - `ContextProvider` trait interface

// =============================================================================
// Context IR: Hot-Path Recording
// =============================================================================

mod context_ir_hotpath {
    use lean_ctx::core::context_ir::{ContextIrSourceKindV1, ContextIrV1, RecordIrInput};
    use std::time::Duration;

    #[test]
    fn record_populates_all_fields() {
        let mut ir = ContextIrV1::new();
        assert_eq!(ir.items.len(), 0);
        assert_eq!(ir.next_seq, 1);

        ir.record(RecordIrInput {
            kind: ContextIrSourceKindV1::Read,
            tool: "ctx_read",
            client_name: Some("cursor".to_string()),
            agent_id: Some("agent_001".to_string()),
            path: Some("src/main.rs"),
            command: None,
            pattern: Some("full"),
            input_tokens: 1000,
            output_tokens: 200,
            duration: Duration::from_millis(42),
            content_excerpt: "fn main() { ... }",
        });

        assert_eq!(ir.items.len(), 1);
        assert_eq!(ir.next_seq, 2);

        let item = &ir.items[0];
        assert_eq!(item.seq, 1);
        assert_eq!(item.source.tool, "ctx_read");
        assert_eq!(item.input_tokens, 1000);
        assert_eq!(item.output_tokens, 200);
        assert!(item.duration_us > 0);
        assert!(item.compression_ratio < 1.0);
        assert!(!item.content_excerpt.is_empty());
    }

    #[test]
    fn record_shell_tool_with_command() {
        let mut ir = ContextIrV1::new();

        ir.record(RecordIrInput {
            kind: ContextIrSourceKindV1::Shell,
            tool: "ctx_shell",
            client_name: None,
            agent_id: None,
            path: None,
            command: Some("cargo test --lib"),
            pattern: None,
            input_tokens: 5000,
            output_tokens: 800,
            duration: Duration::from_millis(3200),
            content_excerpt: "test result: ok. 42 passed",
        });

        let item = &ir.items[0];
        assert!(matches!(item.source.kind, ContextIrSourceKindV1::Shell));
        assert!(item.source.command.is_some());
        assert_eq!(item.duration_us, 3_200_000);
    }

    #[test]
    fn record_search_tool() {
        let mut ir = ContextIrV1::new();

        ir.record(RecordIrInput {
            kind: ContextIrSourceKindV1::Search,
            tool: "ctx_search",
            client_name: None,
            agent_id: None,
            path: Some("rust/src/"),
            command: None,
            pattern: Some("fn compress"),
            input_tokens: 2000,
            output_tokens: 300,
            duration: Duration::from_millis(15),
            content_excerpt: "3 matches in 2 files",
        });

        let item = &ir.items[0];
        assert!(matches!(item.source.kind, ContextIrSourceKindV1::Search));
        assert_eq!(item.source.pattern.as_deref(), Some("fn compress"));
    }

    #[test]
    fn totals_accumulate_correctly() {
        let mut ir = ContextIrV1::new();

        for i in 0..5 {
            ir.record(RecordIrInput {
                kind: ContextIrSourceKindV1::Read,
                tool: "ctx_read",
                client_name: None,
                agent_id: None,
                path: Some("file.rs"),
                command: None,
                pattern: None,
                input_tokens: 1000 * (i + 1),
                output_tokens: 200 * (i + 1),
                duration: Duration::from_millis(10),
                content_excerpt: "x",
            });
        }

        assert_eq!(ir.items.len(), 5);
        assert_eq!(ir.totals.items_recorded, 5);
        assert_eq!(ir.totals.input_tokens, 1000 + 2000 + 3000 + 4000 + 5000);
        assert_eq!(ir.totals.output_tokens, 200 + 400 + 600 + 800 + 1000);
        assert_eq!(
            ir.totals.tokens_saved,
            (1000 - 200) + (2000 - 400) + (3000 - 600) + (4000 - 800) + (5000 - 1000)
        );
    }

    #[test]
    fn prune_enforces_max_items_bound() {
        let mut ir = ContextIrV1::new();

        for i in 0..200 {
            ir.record(RecordIrInput {
                kind: ContextIrSourceKindV1::Other,
                tool: "ctx_control",
                client_name: None,
                agent_id: None,
                path: None,
                command: None,
                pattern: None,
                input_tokens: 10,
                output_tokens: 5,
                duration: Duration::from_micros(100),
                content_excerpt: &format!("item_{i}"),
            });
        }

        // MAX_ITEMS = 128
        assert!(ir.items.len() <= 128);
        assert_eq!(ir.totals.items_recorded, 200);
        // Oldest items were pruned, newest remain
        assert_eq!(ir.items.last().unwrap().seq, 200);
    }

    #[test]
    fn compression_ratio_edge_cases() {
        let mut ir = ContextIrV1::new();

        // Zero input tokens -> ratio 1.0
        ir.record(RecordIrInput {
            kind: ContextIrSourceKindV1::Other,
            tool: "ctx_status",
            client_name: None,
            agent_id: None,
            path: None,
            command: None,
            pattern: None,
            input_tokens: 0,
            output_tokens: 50,
            duration: Duration::from_millis(1),
            content_excerpt: "status ok",
        });
        assert_eq!(ir.items[0].compression_ratio, 1.0);

        // Equal tokens -> ratio 1.0
        ir.record(RecordIrInput {
            kind: ContextIrSourceKindV1::Read,
            tool: "ctx_read",
            client_name: None,
            agent_id: None,
            path: Some("x.rs"),
            command: None,
            pattern: None,
            input_tokens: 500,
            output_tokens: 500,
            duration: Duration::from_millis(1),
            content_excerpt: "no compression",
        });
        assert_eq!(ir.items[1].compression_ratio, 1.0);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let mut ir = ContextIrV1::new();
        ir.record(RecordIrInput {
            kind: ContextIrSourceKindV1::Read,
            tool: "ctx_read",
            client_name: Some("test".to_string()),
            agent_id: None,
            path: Some("roundtrip.rs"),
            command: None,
            pattern: Some("map"),
            input_tokens: 4000,
            output_tokens: 800,
            duration: Duration::from_millis(25),
            content_excerpt: "pub struct Foo {}",
        });

        // Serialize then deserialize
        let json = serde_json::to_string(&ir).unwrap();
        let loaded: ContextIrV1 = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.totals.input_tokens, 4000);
        assert_eq!(loaded.totals.output_tokens, 800);
        assert_eq!(loaded.items[0].source.tool, "ctx_read");
    }
}

// =============================================================================
// ContextProvider Trait Interface
// =============================================================================

mod context_provider_trait {
    use lean_ctx::core::providers::{ContextProvider, ProviderParams, ProviderResult};

    #[test]
    fn provider_default_cache_ttl() {
        struct MinimalProvider;
        impl ContextProvider for MinimalProvider {
            fn id(&self) -> &'static str {
                "test"
            }
            fn display_name(&self) -> &'static str {
                "Test"
            }
            fn supported_actions(&self) -> &[&str] {
                &[]
            }
            fn execute(&self, _: &str, _: &ProviderParams) -> Result<ProviderResult, String> {
                Err("not impl".to_string())
            }
            fn is_available(&self) -> bool {
                false
            }
        }
        let p = MinimalProvider;
        assert_eq!(p.cache_ttl_secs(), 120); // default from trait
        assert!(p.requires_auth()); // default from trait
    }
}

// =============================================================================
// Contracts KV Block Integrity
// =============================================================================

mod contracts_integrity {
    /// Guards the load-bearing invariant behind #348: in MCP stdio mode `stdout`
    /// is the JSON-RPC transport, so every diagnostic MUST go to `stderr`. A stray
    /// switch to stdout would silently corrupt every MCP client's protocol stream.
    /// `tracing-subscriber` does not expose the runtime writer, so we assert on the
    /// logging single-source-of-truth instead.
    #[test]
    fn mcp_logging_targets_stderr_never_stdout() {
        let logging = include_str!("../../src/core/logging.rs");
        assert!(
            logging.matches("with_writer(std::io::stderr)").count() >= 2,
            "both init_logging and init_mcp_logging must pin the tracing writer to stderr"
        );
        assert!(
            !logging.contains("std::io::stdout"),
            "logging must never route tracing to stdout — it corrupts the MCP JSON-RPC channel (#348)"
        );
    }
}
