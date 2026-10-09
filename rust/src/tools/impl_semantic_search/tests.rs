//! Unit tests for the semantic-search stack (moved verbatim in the split).

#[cfg(test)]
mod root_resolution_tests {
    #[allow(clippy::wildcard_imports)]
    use super::super::*;

    #[test]
    fn subdir_filter_scopes_results_to_subtree() {
        let f = SearchFilter::default().with_subdir(Some("crate_a/src".to_string()));
        assert!(f.is_active());
        assert!(f.matches("crate_a/src/auth.rs"));
        assert!(f.matches("crate_a/src/nested/db.rs"));
        assert!(!f.matches("crate_b/src/auth.rs"));
        // Boundary: the prefix must be a whole path segment, not a substring.
        assert!(!f.matches("crate_a/src_extra/x.rs"));
        // Backslash input is normalized before matching.
        assert!(f.matches(r"crate_a\src\win.rs"));
    }

    #[test]
    fn empty_subdir_is_no_scope() {
        let f = SearchFilter::default().with_subdir(Some(String::new()));
        assert!(!f.is_active());
        assert!(f.matches("anything/here.rs"));
    }

    #[test]
    fn search_subdir_filter_derives_relative_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let sub = root.join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(search_subdir_filter(root, &sub).as_deref(), Some("a/b"));
        assert_eq!(search_subdir_filter(root, root), None);
        // A path that is not under root yields no scope.
        assert_eq!(search_subdir_filter(&sub, root), None);
    }

    #[test]
    fn resolve_search_root_promotes_subdir_to_project_root() {
        // The index namespace is keyed on the project root; a subdir search
        // must resolve to that same root (and keep the subdir as a scope) instead
        // of hashing to a different, empty namespace.
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let root = crate::core::pathutil::safe_canonicalize_or_self(tmp.path());
        let sub = root.join("crate_a").join("src");
        std::fs::create_dir_all(&sub).unwrap();

        // A project marker at the root is what makes it the project root.
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let from_sub = resolve_search_root(&sub.to_string_lossy());
        let from_root = resolve_search_root(&root.to_string_lossy());

        let (resolved, subdir) = from_sub.unwrap();
        assert_eq!(resolved, root, "subdir must promote to the project root");
        assert_eq!(subdir.as_deref(), Some("crate_a/src"));

        let (resolved_root, subdir_root) = from_root.unwrap();
        assert_eq!(resolved_root, root);
        assert_eq!(subdir_root, None, "the root itself carries no subdir scope");
    }

    #[test]
    fn resolve_search_root_errors_on_missing_path() {
        assert!(resolve_search_root("/definitely/not/here/xyzzy-7f3a91").is_err());
    }
}

#[cfg(all(test, feature = "dense"))]
mod cold_start_guard_tests {
    #[allow(clippy::wildcard_imports)]
    use super::super::*;

    #[test]
    fn budget_zero_disables_guard() {
        // 0 = "always embed inline" (the earlier behavior), regardless of size.
        assert!(!exceeds_inline_embed_budget(1_000_000, 0));
    }

    #[test]
    fn budget_is_inclusive_and_triggers_above_threshold() {
        assert!(!exceeds_inline_embed_budget(0, 2000), "warm index: inline");
        assert!(
            !exceeds_inline_embed_budget(2000, 2000),
            "at the budget: still inline"
        );
        assert!(
            exceeds_inline_embed_budget(2001, 2000),
            "over the budget: degrade"
        );
    }

    #[test]
    fn default_threshold_is_positive() {
        // The inline-embed budget must be a real, positive guard.
        assert!(inline_embed_max_chunks() >= 1);
    }

    #[test]
    fn dense_build_hint_always_points_at_the_cli_build() {
        let full = dense_build_hint(22_741, false);
        assert!(full.ends_with("sobre index build]"));
        assert!(full.contains("22741"));
        let compact = dense_build_hint(22_741, true);
        assert!(compact.ends_with("sobre index build]"));
        assert!(compact.contains("22741"));
    }
}

#[cfg(test)]
mod determinism_tests {
    #[allow(clippy::wildcard_imports)]
    use super::super::*;
    use crate::core::hybrid_search::HybridResult;

    #[test]
    fn rrf_merge_hybrid_is_deterministic_on_ties() {
        let a = HybridResult {
            file_path: "a.rs".to_string(),
            symbol_name: "foo".to_string(),
            kind: crate::core::bm25_index::ChunkKind::Function,
            start_line: 1,
            end_line: 1,
            snippet: "a".to_string(),
            rrf_score: 0.0,
            dense_score: None,
            bm25_rank: None,
            dense_rank: None,
        };
        let b = HybridResult {
            file_path: "b.rs".to_string(),
            symbol_name: "foo".to_string(),
            kind: crate::core::bm25_index::ChunkKind::Function,
            start_line: 1,
            end_line: 1,
            snippet: "b".to_string(),
            rrf_score: 0.0,
            dense_score: None,
            bm25_rank: None,
            dense_rank: None,
        };

        // Two lists with swapped ranks yield identical RRF sums for a and b.
        let fused = rrf_merge_hybrid(
            vec![
                ("root".to_string(), vec![a.clone(), b.clone()]),
                ("root".to_string(), vec![b.clone(), a.clone()]),
            ],
            10,
        );

        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].file_path, "a.rs");
        assert_eq!(fused[1].file_path, "b.rs");
    }
}

#[cfg(all(test, feature = "dense"))]
mod dense_toggle_tests {
    #[allow(clippy::wildcard_imports)]
    use super::super::*;
    use crate::core::bm25_index::{BM25Index, ChunkKind, CodeChunk, tokenize};
    use crate::core::hybrid_search::HybridConfig;

    fn small_index() -> BM25Index {
        BM25Index::from_chunks_for_test(vec![
            CodeChunk {
                file_path: "auth.rs".into(),
                symbol_name: "validate_token".into(),
                kind: ChunkKind::Function,
                start_line: 1,
                end_line: 10,
                content: "fn validate_token(token: &str) -> bool { check_jwt_expiry(token) }"
                    .into(),
                tokens: tokenize("fn validate_token token str bool check_jwt_expiry token"),
                token_count: 0,
            },
            CodeChunk {
                file_path: "db.rs".into(),
                symbol_name: "connect_database".into(),
                kind: ChunkKind::Function,
                start_line: 1,
                end_line: 5,
                content: "fn connect_database(url: &str) -> Pool { create_pool(url) }".into(),
                tokens: tokenize("fn connect_database url str Pool create_pool url"),
                token_count: 0,
            },
        ])
    }

    /// On a cold dense index hybrid ranks without the dense signal,
    /// finds the lexical match, says it degraded, and never writes embeddings.
    #[test]
    fn hybrid_on_cold_index_ranks_without_embeddings_and_says_so() {
        let _env = crate::core::data_dir::test_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let index = small_index();
        let filter = SearchFilter::default();
        let cfg = HybridConfig {
            splade_weight: 0.0,
            ..HybridConfig::default()
        };

        let Ok(ranked) = hybrid_ranked("jwt token validation", root, &index, 5, &filter, &cfg)
        else {
            // No embedding engine in this environment: nothing to rank with.
            return;
        };
        if ranked.degraded.is_some() {
            assert!(
                ranked
                    .results
                    .iter()
                    .any(|r| r.symbol_name == "validate_token"),
                "expected lexical match"
            );
            let out = render_ranked(SearchMode::Hybrid, "q", &index, 5, false, &ranked);
            assert!(out.contains("dense index not built"), "got: {out}");
        }
        assert!(!root.join("embeddings.json").exists());
    }
}

mod search_mode_tests {
    use super::super::SearchMode;

    #[test]
    fn bm25_is_the_default_and_unknown_modes_are_rejected() {
        assert_eq!(SearchMode::parse(None), Ok(SearchMode::Bm25));
        assert_eq!(SearchMode::parse(Some(" BM25 ")), Ok(SearchMode::Bm25));
        let err = SearchMode::parse(Some("vector")).unwrap_err();
        assert!(err.contains("unknown mode"), "{err}");
    }

    #[cfg(not(feature = "dense"))]
    #[test]
    fn dense_modes_are_an_error_without_embeddings() {
        for mode in ["dense", "hybrid"] {
            let err = SearchMode::parse(Some(mode)).unwrap_err();
            assert!(err.contains("--features dense"), "{err}");
        }
    }

    #[cfg(feature = "dense")]
    #[test]
    fn dense_modes_parse_with_embeddings() {
        assert_eq!(SearchMode::parse(Some("dense")), Ok(SearchMode::Dense));
        assert_eq!(SearchMode::parse(Some("hybrid")), Ok(SearchMode::Hybrid));
    }
}
