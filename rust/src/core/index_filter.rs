//! Index-time file filters: one shared corpus-membership decision for
//! every index builder.
//!
//! Source-control intent (`.gitignore`) and retrieval policy (what belongs in
//! the index corpus) are different axes: a file can be versioned and important
//! while still being bad context for code search (CSV seed data, fixtures,
//! lockfiles, vendored blobs). This module carries the declared retrieval
//! policy — the `ignore` globs from config.toml — so the BM25 and graph
//! builders can never disagree about corpus membership. The semantic
//! (embedding) index chunks the BM25 corpus, so it inherits the same universe
//! by construction.
//!
//! Excluded files never enter the pipeline: no chunks, no graph nodes, no
//! embeddings. Globs are evaluated against the root-relative path with forward
//! slashes.

/// The resolved corpus-membership filter for one indexing run.
#[derive(Debug)]
pub struct IndexFileFilter {
    exclude: Vec<glob::Pattern>,
}

impl IndexFileFilter {
    /// Resolve the effective filter: the config `ignore` globs.
    pub fn effective() -> Self {
        let cfg = crate::core::config::Config::load();
        Self::resolve(&cfg)
    }

    /// [`Self::effective`] with an already-loaded config (builders that hold
    /// one avoid a second load).
    pub fn resolve(cfg: &crate::core::config::Config) -> Self {
        Self::from_excludes(&cfg.ignore)
    }

    /// Build a filter from raw globs. Invalid globs are dropped with a warning
    /// rather than aborting the build.
    pub fn from_excludes(exclude: &[String]) -> Self {
        Self {
            exclude: exclude
                .iter()
                .filter_map(|p| match glob::Pattern::new(p) {
                    Ok(pat) => Some(pat),
                    Err(e) => {
                        tracing::warn!("[index_filter] ignoring invalid ignore glob {p:?}: {e}");
                        None
                    }
                })
                .collect(),
        }
    }

    /// Corpus-membership decision for one file, by root-relative path with
    /// forward slashes. The empty filter admits everything.
    pub fn is_excluded(&self, rel_path: &str) -> bool {
        self.exclude.iter().any(|p| p.matches(rel_path))
    }

    /// One-line human/JSON summary of the active filter for `index status`.
    /// `None` when `ignore` is empty.
    pub fn summary(&self) -> Option<String> {
        if self.exclude.is_empty() {
            return None;
        }
        let globs: Vec<&str> = self.exclude.iter().map(glob::Pattern::as_str).collect();
        Some(format!("exclude=[{}]", globs.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(exclude: &[&str]) -> IndexFileFilter {
        let exc: Vec<String> = exclude.iter().map(ToString::to_string).collect();
        IndexFileFilter::from_excludes(&exc)
    }

    #[test]
    fn empty_filter_admits_everything() {
        let f = filter(&[]);
        assert!(!f.is_excluded("src/main.rs"));
        assert!(!f.is_excluded("data/seed.csv"));
        assert!(f.summary().is_none());
    }

    #[test]
    fn exclude_glob_drops_matching_files() {
        // The motivating case: versioned Liquibase CSV seed data.
        let f = filter(&["src/main/resources/liquibase/data/*.csv"]);
        assert!(f.is_excluded("src/main/resources/liquibase/data/users.csv"));
        assert!(!f.is_excluded("src/main/java/App.java"));
    }

    #[test]
    fn recursive_exclude_matches_at_any_depth() {
        let f = filter(&["**/*.csv"]);
        assert!(f.is_excluded("data/seed.csv"));
        assert!(f.is_excluded("a/b/c/d.csv"));
        assert!(f.is_excluded("top.csv"));
        assert!(!f.is_excluded("src/lib.rs"));
    }

    #[test]
    fn invalid_glob_is_dropped_not_fatal() {
        let f = filter(&["[unclosed", "**/*.csv"]);
        assert!(f.is_excluded("data/seed.csv"));
        assert!(!f.is_excluded("src/lib.rs"));
    }

    #[test]
    fn summary_lists_the_globs() {
        let f = filter(&["**/*.csv", "**/*.jsonl"]);
        assert_eq!(
            f.summary().as_deref(),
            Some("exclude=[**/*.csv, **/*.jsonl]")
        );
    }
}
