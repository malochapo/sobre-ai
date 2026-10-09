//! `remember`/`recall` knowledge operations + archive rehydration.
//! Split out of `sobre_knowledge/mod.rs`; `use super::*` re-imports parent items.

#[allow(clippy::wildcard_imports)]
use super::*;
use chrono::Utc;

#[cfg(feature = "dense")]
use super::embeddings::{embedding_engine, embedding_engine_nonblocking};

use crate::core::knowledge::{AdmissionResult, sort_fact_for_output};

/// Where `remember` files a fact given without a category.
pub(crate) const DEFAULT_CATEGORY: &str = "general";

pub(crate) fn handle_remember(
    project_root: &str,
    category: Option<&str>,
    key: Option<&str>,
    value: Option<&str>,
    session_id: &str,
    confidence: Option<f32>,
) -> String {
    // The documented form is `value` alone: an uncategorised fact goes to
    // `general`.
    let cat = category.unwrap_or(DEFAULT_CATEGORY);
    let Some(v) = value else {
        return "Error: value is required for remember".to_string();
    };
    // `key` is optional: agents (and our own injected instructions) commonly
    // call remember with just category+content. Derive a deterministic slug
    // from the value so the call succeeds instead of erroring.
    let derived_key;
    let k = if let Some(k) = key {
        k
    } else {
        derived_key = derive_key_from_value(v);
        derived_key.as_str()
    };
    let conf = confidence.unwrap_or(0.8);
    let (v, _secret_matches) = crate::core::secret_detection::scan_and_redact(v);
    let v = v.as_str();
    let policy = crate::core::memory_policy::MemoryPolicy::default();
    // Serialize the read-modify-write under a per-project lock so parallel
    // `remember` calls cannot clobber each other. The closure
    // operates on the freshly (re)loaded state inside the lock. Admission
    // is enforced here, on the agent-facing path.
    let (knowledge, admission) = match ProjectKnowledge::mutate_locked(project_root, |kn| {
        let r = kn.remember_admitted(cat, k, v, session_id, conf, &policy);
        let _ = kn.run_memory_lifecycle(&policy);
        r
    }) {
        Ok(pair) => pair,
        Err(e) => return format!("Remembered [{cat}] {k}: {v}\n(save failed: {e})"),
    };

    // The contradiction (if any) only exists on the normal stored path.
    let contradiction = match &admission {
        AdmissionResult::Stored(c) => c.clone(),
        AdmissionResult::Merged { .. } => None,
    };
    let merged = matches!(admission, AdmissionResult::Merged { .. });

    let mut result = if let AdmissionResult::Merged {
        category,
        key,
        confirmations,
        ..
    } = &admission
    {
        format!(
            "Merged [{cat}] {k} into existing [{category}/{key}] (near-duplicate; confirmed {confirmations}x). \
             Store size unchanged — the matched fact was reinforced instead of adding a row."
        )
    } else {
        let current_fact = knowledge
            .facts
            .iter()
            .find(|f| f.category == cat && f.key == k && f.is_current());
        let rev = current_fact.map_or(1, |f| f.revision_count);
        let conf_count = current_fact.map_or(1, |f| f.confirmation_count);
        if contradiction.is_some() {
            format!(
                "Updated [{cat}] {k}: {v} → revision {rev} (previous archived, confidence: {:.0}%)",
                conf * 100.0
            )
        } else if rev > 1 {
            format!(
                "Confirmed [{cat}] {k}: {v} (revision {rev}, confirmed {conf_count}x, confidence: {:.0}%)",
                current_fact.map_or(conf, |f| f.confidence) * 100.0
            )
        } else {
            format!(
                "Remembered [{cat}] {k}: {v} (revision 1, confidence: {:.0}%)",
                conf * 100.0
            )
        }
    };

    if let Some(c) = &contradiction {
        result.push_str(&format!("\n⚠ CONTRADICTION: {}", c.resolution));
    }

    // Cross-key advisories only help a genuine insert; a merge already collapsed
    // the closest duplicate, so there is nothing left for the agent to resolve.
    let similar = if merged {
        Vec::new()
    } else {
        crate::core::knowledge::find_cross_key_similar(cat, k, v, &knowledge.facts, 3)
    };
    if !similar.is_empty() {
        result.push_str(&format!("\n\nSIMILAR FACTS ({} found):", similar.len()));
        for sf in &similar {
            result.push_str(&format!(
                "\n  {}/{} ({:.0}%) — \"{}\"",
                sf.category,
                sf.key,
                sf.similarity * 100.0,
                sf.value_preview
            ));
        }
        result.push_str(
            "\n→ duplicate? sobre_knowledge(action=\"remove\", category=\"<cat>\", key=\"<key>\"), or remember under the existing key to update it"
        );
    }

    #[cfg(feature = "dense")]
    {
        // The (category, key, value) actually persisted — drives the side-car so
        // a merge refreshes the *target* fact's vector, not a row never inserted.
        let (eff_cat, eff_key, eff_val) = match &admission {
            AdmissionResult::Merged {
                category,
                key,
                value,
                ..
            } => (category.clone(), key.clone(), value.clone()),
            AdmissionResult::Stored(_) => (cat.to_string(), k.to_string(), v.to_string()),
        };
        // Non-blocking engine access: `remember` must never stall on a model
        // download/load (fresh install, offline sandbox — the blocking variant
        // hit the 120s tool watchdog before the first fact was ever embedded).
        // When the engine is not loaded yet this kicks off the one-time
        // background activation and skips the side-car for THIS call only;
        // a later remember or recall backfills the vector.
        if let Some(engine) = embedding_engine_nonblocking() {
            // Serialize the embedding index's read-modify-write under the same
            // per-project lock as the fact write above, and compact against the
            // freshly committed on-disk knowledge instead of this call's
            // snapshot. The side-car write previously ran lock-free against a
            // stale snapshot, so parallel `remember` calls clobbered each
            // other's vectors and pruned just-stored embeddings — semantic
            // recall then returned far fewer hits than facts stored.
            let (warn, semantic) = ProjectKnowledge::with_project_lock(project_root, || {
                let mut idx = crate::core::knowledge_embedding::KnowledgeEmbeddingIndex::load(
                    &knowledge.project_hash,
                )
                .unwrap_or_else(|| {
                    crate::core::knowledge_embedding::KnowledgeEmbeddingIndex::new(
                        &knowledge.project_hash,
                    )
                });

                // Semantic near-duplicate scan against the *pre-upsert* index, so
                // the new fact never matches itself. Catches paraphrases the
                // lexical `find_cross_key_similar` pass misses. Skipped on a merge:
                // the duplicate was already collapsed by admission.
                let semantic = if merged {
                    Vec::new()
                } else {
                    crate::core::knowledge_embedding::find_semantic_duplicates(
                        &idx,
                        engine,
                        &knowledge,
                        cat,
                        k,
                        v,
                        crate::core::knowledge_embedding::SEMANTIC_DUP_THRESHOLD,
                        3,
                    )
                };

                // Embed the fact that was *actually* persisted: the target key on
                // a merge (with its possibly-extended value), else the new fact.
                let warn = match crate::core::knowledge_embedding::embed_and_store(
                    &mut idx, engine, &eff_cat, &eff_key, &eff_val,
                ) {
                    Ok(()) => {
                        let fresh = ProjectKnowledge::load(project_root);
                        let kref = fresh.as_ref().unwrap_or(&knowledge);
                        // Self-healing coverage: facts written while the engine
                        // was still warming up (non-blocking remember) or via
                        // the consolidation/ETL writers have no vector yet —
                        // embed a bounded batch of them now that the engine is
                        // warm, so semantic recall converges on its own.
                        crate::core::knowledge_embedding::backfill_missing(
                            &mut idx,
                            engine,
                            kref,
                            crate::core::knowledge_embedding::BACKFILL_PER_REMEMBER,
                        );
                        crate::core::knowledge_embedding::compact_against_knowledge(
                            &mut idx, kref, &policy,
                        );
                        idx.save()
                            .err()
                            .map(|e| format!("\n(warn: embeddings save failed: {e})"))
                    }
                    Err(e) => Some(format!("\n(warn: embeddings update failed: {e})")),
                };
                (warn, semantic)
            });
            if let Some(w) = warn {
                result.push_str(&w);
            }

            // Surface only the semantic duplicates the lexical pass did not
            // already list, so the agent sees each near-duplicate once.
            let extra: Vec<_> = semantic
                .into_iter()
                .filter(|s| {
                    !similar
                        .iter()
                        .any(|l| l.category == s.category && l.key == s.key)
                })
                .collect();
            if !extra.is_empty() {
                result.push_str(&format!(
                    "\n\nSEMANTIC NEAR-DUPLICATES ({} found):",
                    extra.len()
                ));
                for sf in &extra {
                    result.push_str(&format!(
                        "\n  {}/{} ({:.0}%) — \"{}\"",
                        sf.category,
                        sf.key,
                        sf.similarity * 100.0,
                        sf.value_preview
                    ));
                }
                result.push_str(
                    "\n→ duplicate? sobre_knowledge(action=\"remove\", category=\"<cat>\", key=\"<key>\"), or remember under the existing key to update it"
                );
            }
        }
    }

    result
}

/// Deterministic key slug from a fact's value: the first four words,
/// lowercased, non-alphanumerics collapsed to single dashes, capped at 48
/// bytes. A pure function of the content (no timestamps/counters) so
/// repeated remembers of the same value merge into the same key.
pub(crate) fn derive_key_from_value(value: &str) -> String {
    let words: Vec<&str> = value.split_whitespace().take(4).collect();
    let joined = words.join(" ").to_lowercase();
    let mut slug = String::with_capacity(joined.len());
    let mut last_dash = true; // suppress a leading dash
    for c in joined.chars() {
        if c.is_alphanumeric() {
            slug.push(c);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "note".to_string()
    } else {
        slug.to_string()
    }
}

pub(crate) fn handle_recall(
    project_root: &str,
    category: Option<&str>,
    query: Option<&str>,
    session_id: &str,
    mode: Option<&str>,
    as_of: Option<&str>,
) -> String {
    let Some(mut knowledge) = ProjectKnowledge::load(project_root) else {
        return "No knowledge stored for this project yet.".to_string();
    };
    let policy = crate::core::memory_policy::MemoryPolicy::default();

    if let Some(raw) = as_of {
        let at = match parse_as_of(raw) {
            Ok(t) => t,
            Err(e) => return e,
        };
        return recall_as_of(&knowledge, category, query, at, &policy);
    }

    if let Some(cat) = category {
        let limit = policy.knowledge.recall_facts_limit;
        let (facts, total) = knowledge.recall_by_category_for_output(cat, query, limit);
        if facts.is_empty() || total == 0 {
            // System 2: archive rehydrate (category-only)
            let rehydrated =
                rehydrate_from_archives(&mut knowledge, Some(cat), None, session_id, &policy);
            if rehydrated {
                let (facts2, total2) = knowledge.recall_by_category_for_output(cat, query, limit);
                if !facts2.is_empty() && total2 > 0 {
                    let out2 = format_facts_with_annotations(&facts2, total2, Some(cat));
                    save_knowledge_deferred(knowledge, project_root);
                    return out2;
                }
            }
            return match query {
                Some(q) => format!("No facts in category '{cat}' match '{q}'."),
                None => format!("No facts in category '{cat}'."),
            };
        }
        let out = format_facts_with_annotations(&facts, total, Some(cat));
        save_knowledge_deferred(knowledge, project_root);
        return out;
    }

    if let Some(q) = query {
        let mode = mode.unwrap_or("auto").trim().to_lowercase();
        // Whether an embedding index answered (it may still find nothing).
        #[cfg_attr(not(feature = "dense"), allow(unused_mut))]
        let mut embedded = false;
        #[cfg(feature = "dense")]
        {
            // Use non-blocking engine access for auto/hybrid: never block recall
            // waiting for model load. Only explicit "semantic" mode may block.
            let engine_opt = if mode == "semantic" {
                embedding_engine()
            } else {
                embedding_engine_nonblocking()
            };
            if let Some(engine) = engine_opt {
                // Embed a bounded batch of the facts that have no vector yet,
                // so semantic recall works without any reindex step.
                let idx = ProjectKnowledge::with_project_lock(project_root, || {
                    let hash = &knowledge.project_hash;
                    let mut idx =
                        crate::core::knowledge_embedding::KnowledgeEmbeddingIndex::load(hash)
                            .unwrap_or_else(|| {
                                crate::core::knowledge_embedding::KnowledgeEmbeddingIndex::new(hash)
                            });
                    if crate::core::knowledge_embedding::backfill_missing(
                        &mut idx,
                        engine,
                        &knowledge,
                        crate::core::knowledge_embedding::BACKFILL_PER_REMEMBER,
                    ) > 0
                    {
                        let _ = idx.save();
                    }
                    idx
                });
                embedded = true;
                let limit = policy.knowledge.recall_facts_limit;
                if mode == "semantic" {
                    let scored = crate::core::knowledge_embedding::semantic_recall_semantic_only(
                        &knowledge, &idx, engine, q, limit,
                    );
                    if scored.is_empty() {
                        return format!("No semantic facts matching '{q}'.");
                    }
                    let hits: Vec<SemanticHit> = scored
                        .iter()
                        .map(|s| SemanticHit {
                            category: s.fact.category.clone(),
                            key: s.fact.key.clone(),
                            value: s.fact.value.clone(),
                            score: s.score,
                            semantic_score: s.semantic_score,
                            confidence_score: s.confidence_score,
                        })
                        .collect();
                    apply_retrieval_signals_from_hits(&mut knowledge, &hits);
                    let out = format_semantic_facts(&format!("{q} (mode=semantic)"), &hits);
                    save_knowledge_deferred(knowledge, project_root);
                    return out;
                }

                if mode == "hybrid" || mode == "auto" {
                    let scored = crate::core::knowledge_embedding::semantic_recall(
                        &knowledge, &idx, engine, q, limit,
                    );
                    if !scored.is_empty() {
                        let hits: Vec<SemanticHit> = scored
                            .iter()
                            .map(|s| SemanticHit {
                                category: s.fact.category.clone(),
                                key: s.fact.key.clone(),
                                value: s.fact.value.clone(),
                                score: s.score,
                                semantic_score: s.semantic_score,
                                confidence_score: s.confidence_score,
                            })
                            .collect();
                        apply_retrieval_signals_from_hits(&mut knowledge, &hits);
                        let out = format_semantic_facts(&format!("{q} (mode=hybrid)"), &hits);
                        save_knowledge_deferred(knowledge, project_root);
                        return out;
                    }
                }
            }
        }

        if !embedded && (mode == "semantic" || mode == "hybrid") {
            return format!(
                "mode={mode} recall needs embeddings: the sobre-dense release build (or a \
                 build with --features dense) and the model (`sobre embeddings provision`). \
                 Use mode=auto for lexical recall."
            );
        }

        let limit = policy.knowledge.recall_facts_limit;
        let (facts, total) = knowledge.recall_for_output(q, limit);
        if facts.is_empty() || total == 0 {
            // System 2: archive rehydrate (query)
            let rehydrated =
                rehydrate_from_archives(&mut knowledge, None, Some(q), session_id, &policy);
            if rehydrated {
                let (facts2, total2) = knowledge.recall_for_output(q, limit);
                if !facts2.is_empty() && total2 > 0 {
                    let out2 = format_facts_with_annotations(&facts2, total2, None);
                    save_knowledge_deferred(knowledge, project_root);
                    return out2;
                }
            }
            return format!("No facts matching '{q}'.");
        }
        let out = format_facts_with_annotations(&facts, total, None);
        save_knowledge_deferred(knowledge, project_root);
        return out;
    }

    // Bare recall (no query, no category): show the most recent current facts
    // instead of erroring — "what do we know?" is the natural first call.
    let limit = policy.knowledge.recall_facts_limit;
    let mut current: Vec<&crate::core::knowledge::KnowledgeFact> =
        knowledge.facts.iter().filter(|f| f.is_current()).collect();
    if current.is_empty() {
        return "No knowledge stored for this project yet.".to_string();
    }
    current.sort_by(|a, b| sort_fact_for_output(a, b));
    let total = current.len();
    current.truncate(limit);
    let mut out = format!("Recent facts (showing {}/{total}):\n", current.len());
    for f in &current {
        out.push_str(&format!(
            "  [{}/{}]: {} (confidence: {:.0}%)\n",
            f.category,
            f.key,
            f.value,
            f.confidence * 100.0
        ));
    }
    out.push_str("→ narrow with query=… or category=…");
    out
}

/// Parse an `as_of` timestamp: RFC 3339 (`2026-06-01T12:00:00Z`) or a bare
/// date (`2026-06-01`, interpreted as end-of-day UTC so "as of June 1st"
/// includes everything recorded during that day).
pub(crate) fn parse_as_of(raw: &str) -> Result<chrono::DateTime<Utc>, String> {
    let raw = raw.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(t.with_timezone(&Utc));
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let eod = d.and_hms_opt(23, 59, 59).expect("valid time");
        return Ok(chrono::DateTime::from_naive_utc_and_offset(eod, Utc));
    }
    Err(format!(
        "Error: invalid as_of '{raw}'. Use RFC 3339 (2026-06-01T12:00:00Z) or YYYY-MM-DD."
    ))
}

/// Temporal recall: facts valid at time `at`, read-only (no retrieval-signal
/// mutation — time travel must not change present-day salience). Superseded
/// facts are shown with their validity window so the history is explicit.
fn recall_as_of(
    knowledge: &ProjectKnowledge,
    category: Option<&str>,
    query: Option<&str>,
    at: chrono::DateTime<Utc>,
    policy: &MemoryPolicy,
) -> String {
    let limit = policy.knowledge.recall_facts_limit;

    let mut facts: Vec<&crate::core::knowledge::KnowledgeFact> = match (query, category) {
        (Some(q), _) => {
            let mut hits = knowledge.recall_at_time(q, at);
            if let Some(cat) = category {
                hits.retain(|f| f.category == cat);
            }
            hits
        }
        (None, Some(cat)) => {
            let mut hits: Vec<&crate::core::knowledge::KnowledgeFact> = knowledge
                .facts
                .iter()
                .filter(|f| f.category == cat && f.was_valid_at(at))
                .collect();
            hits.sort_by(|a, b| sort_fact_for_output(a, b));
            hits
        }
        (None, None) => return "Error: provide query or category for recall".to_string(),
    };

    let total = facts.len();
    facts.truncate(limit);

    if facts.is_empty() {
        return format!("No facts valid at {}.", at.format("%Y-%m-%d %H:%M UTC"));
    }

    let mut out = format!(
        "Facts as of {} (showing {}/{total}):\n",
        at.format("%Y-%m-%d %H:%M UTC"),
        facts.len()
    );
    for f in facts {
        let window = match (f.valid_from, f.valid_until) {
            (Some(from), Some(until)) => format!(
                " [valid {} → {}]",
                from.format("%Y-%m-%d"),
                until.format("%Y-%m-%d")
            ),
            (Some(from), None) => format!(" [valid since {}]", from.format("%Y-%m-%d")),
            (None, Some(until)) => format!(" [valid until {}]", until.format("%Y-%m-%d")),
            (None, None) => String::new(),
        };
        let superseded = if f.is_current() { "" } else { " [superseded]" };
        out.push_str(&format!(
            "  [{}/{}]: {} (confidence: {:.0}%){window}{superseded}\n",
            f.category,
            f.key,
            f.value,
            f.confidence * 100.0
        ));
    }
    out
}

/// Persist recall state to disk on a background thread so recall returns
/// immediately. Retrieval signals (retrieval_count, last_retrieved) are
/// best-effort metadata; losing them on crash is acceptable.
///
/// The save is reconciled against the latest on-disk state *under the shared
/// lock* so it never clobbers facts a concurrent writer committed after
/// `knowledge` was snapshotted: a bare `knowledge.save` here was
/// a blind overwrite and could drop a just-`remember`ed fact. Recall only
/// rehydrates archived facts and bumps retrieval metadata — it never removes or
/// supersedes — so re-adding any current snapshot fact missing from the fresh
/// copy (and carrying over the higher retrieval count) is the correct, lossless
/// reconciliation.
pub(crate) fn save_knowledge_deferred(knowledge: ProjectKnowledge, project_root: &str) {
    let root = project_root.to_string();
    std::thread::Builder::new()
        .name("knowledge-save".into())
        .spawn(move || {
            let _ = ProjectKnowledge::mutate_locked(&root, |fresh| {
                for sf in knowledge.facts.iter().filter(|f| f.is_current()) {
                    if let Some(existing) = fresh
                        .facts
                        .iter_mut()
                        .find(|f| f.category == sf.category && f.key == sf.key && f.is_current())
                    {
                        existing.retrieval_count = existing.retrieval_count.max(sf.retrieval_count);
                    } else {
                        fresh.facts.push(sf.clone());
                    }
                }
            });
        })
        .ok();
}

pub(crate) fn rehydrate_from_archives(
    knowledge: &mut ProjectKnowledge,
    category: Option<&str>,
    query: Option<&str>,
    session_id: &str,
    policy: &MemoryPolicy,
) -> bool {
    // Scan every *retained* archive: the reach now aligns with retention,
    // so a recall miss can recover from any archive still on disk instead of only
    // the newest few that the prior fixed cap left reachable.
    let archives = crate::core::memory_lifecycle::reachable_archives(
        &crate::core::memory_archive::ArchiveConfig::default(),
    );
    if archives.is_empty() {
        return false;
    }

    let terms: Vec<String> = query
        .unwrap_or("")
        .to_lowercase()
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(std::string::ToString::to_string)
        .collect();

    #[derive(Clone)]
    struct Cand {
        category: String,
        key: String,
        value: String,
        confidence: f32,
        score: f32,
    }

    let mut cands: Vec<Cand> = Vec::new();

    let rehydrate_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    for p in &archives {
        if std::time::Instant::now() >= rehydrate_deadline {
            tracing::warn!("sobre_knowledge: rehydrate time budget (10s) exceeded, stopping early");
            break;
        }
        let p_str = p.to_string_lossy().to_string();
        let Ok(facts) = crate::core::memory_lifecycle::restore_archive(&p_str) else {
            continue;
        };
        for f in facts {
            if let Some(cat) = category
                && f.category != cat
            {
                continue;
            }
            if terms.is_empty() {
                cands.push(Cand {
                    category: f.category,
                    key: f.key,
                    value: f.value,
                    confidence: f.confidence,
                    score: f.confidence,
                });
            } else {
                let searchable = format!(
                    "{} {} {} {}",
                    f.category.to_lowercase(),
                    f.key.to_lowercase(),
                    f.value.to_lowercase(),
                    f.source_session.to_lowercase()
                );
                let match_count = terms.iter().filter(|t| searchable.contains(*t)).count();
                if match_count == 0 {
                    continue;
                }
                let rel = match_count as f32 / terms.len() as f32;
                let score = rel * f.confidence;
                cands.push(Cand {
                    category: f.category,
                    key: f.key,
                    value: f.value,
                    confidence: f.confidence,
                    score,
                });
            }
        }
    }

    if cands.is_empty() {
        return false;
    }

    cands.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.key.cmp(&b.key))
            .then_with(|| a.value.cmp(&b.value))
    });
    cands.truncate(crate::core::budgets::KNOWLEDGE_REHYDRATE_LIMIT);

    let mut any = false;
    for c in &cands {
        knowledge.remember(
            &c.category,
            &c.key,
            &c.value,
            session_id,
            c.confidence.max(0.6),
            policy,
        );
        any = true;
    }
    if any {
        let _ = knowledge.run_memory_lifecycle(policy);
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_key_is_deterministic_slug() {
        let v = "fresh-demo uses greet() as the single formatting entry point";
        let k1 = derive_key_from_value(v);
        let k2 = derive_key_from_value(v);
        assert_eq!(k1, k2, "must be a pure function of the value");
        assert_eq!(k1, "fresh-demo-uses-greet-as");
        assert!(k1.len() <= 48);
    }

    #[test]
    fn derive_key_handles_degenerate_values() {
        assert_eq!(derive_key_from_value("---"), "note");
        assert_eq!(derive_key_from_value(""), "note");
        assert_eq!(
            derive_key_from_value("Fix: обновление конфига"),
            "fix-обновление-конфига"
        );
    }

    #[test]
    fn remember_without_key_derives_one() {
        let out = handle_remember(
            "/tmp/test-remember-derived-key",
            Some("decision"),
            None,
            Some("uses BTreeSet for deterministic iteration"),
            "s1",
            None,
        );
        assert!(
            out.contains("uses-btreeset-for-deterministic"),
            "derived key expected in: {out}"
        );
        assert!(!out.starts_with("Error"), "must not error: {out}");
    }

    /// The documented form, `value` alone, is accepted and filed under
    /// the default category.
    #[test]
    fn remember_with_value_only_uses_the_default_category() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        let out = handle_remember(
            "/tmp/test-remember-value-only",
            None,
            None,
            Some("the release binary is built with --features dense"),
            "s1",
            None,
        );
        assert!(!out.starts_with("Error"), "must not error: {out}");
        assert!(
            out.contains("[general]"),
            "default category expected: {out}"
        );
    }

    #[test]
    fn parse_as_of_accepts_rfc3339() {
        let t = parse_as_of("2026-06-01T12:30:00Z").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-06-01T12:30:00+00:00");
    }

    #[test]
    fn parse_as_of_accepts_bare_date_as_end_of_day() {
        let t = parse_as_of("2026-06-01").unwrap();
        assert_eq!(t.format("%H:%M:%S").to_string(), "23:59:59");
    }

    #[test]
    fn parse_as_of_rejects_garbage() {
        assert!(parse_as_of("yesterday").is_err());
        assert!(parse_as_of("").is_err());
    }

    #[test]
    fn recall_as_of_returns_superseded_value_at_past_time() {
        let policy = MemoryPolicy::default();
        let mut k = ProjectKnowledge::new("/tmp/test-as-of");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.95, &policy);
        k.facts[0].confirmation_count = 3;

        let before_change = Utc::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        k.remember("arch", "db", "MySQL", "s2", 0.9, &policy);

        let past = recall_as_of(&k, None, Some("db"), before_change, &policy);
        assert!(past.contains("PostgreSQL"), "past view: {past}");
        assert!(!past.contains("MySQL"), "past view must hide newer: {past}");
        assert!(past.contains("[superseded]"), "marks history: {past}");

        let now = recall_as_of(&k, None, Some("db"), Utc::now(), &policy);
        assert!(now.contains("MySQL"), "present view: {now}");
        assert!(!now.contains("PostgreSQL"), "present hides old: {now}");
    }

    #[test]
    fn recall_as_of_category_filter() {
        let policy = MemoryPolicy::default();
        let mut k = ProjectKnowledge::new("/tmp/test-as-of-cat");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.9, &policy);
        k.remember("deploy", "host", "AWS", "s1", 0.8, &policy);

        let out = recall_as_of(&k, Some("deploy"), None, Utc::now(), &policy);
        assert!(out.contains("AWS"));
        assert!(!out.contains("PostgreSQL"));
    }

    #[test]
    fn recall_as_of_before_any_fact_is_empty() {
        let policy = MemoryPolicy::default();
        let mut k = ProjectKnowledge::new("/tmp/test-as-of-empty");
        k.remember("arch", "db", "PostgreSQL", "s1", 0.9, &policy);

        let ancient = parse_as_of("2000-01-01").unwrap();
        let out = recall_as_of(&k, None, Some("db"), ancient, &policy);
        assert!(out.contains("No facts valid at"), "got: {out}");
    }
}
