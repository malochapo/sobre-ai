use std::collections::BTreeSet;

use crate::core::gotcha_tracker::GotchaStore;

/// The project's gotchas that share a word with `task`, best first, as
/// `[remember]` lines. Words are compared whole; stop words and words shorter
/// than three characters never match.
pub fn reminders_for_task(project_root: &str, task: &str) -> Vec<String> {
    let task_terms = tokenize(task);
    if task_terms.is_empty() {
        return Vec::new();
    }

    let store = GotchaStore::load(project_root);
    if store.gotchas.is_empty() {
        return Vec::new();
    }

    #[derive(Clone)]
    struct Scored {
        line: String,
        score: f32,
    }

    let mut scored: Vec<Scored> = Vec::new();

    for g in &store.gotchas {
        let gotcha_terms = tokenize(&format!(
            "{} {} {}",
            g.trigger,
            g.resolution,
            g.category.short_label()
        ));
        let matches = task_terms.intersection(&gotcha_terms).count();
        if matches == 0 {
            continue;
        }
        let rel = matches as f32 / task_terms.len() as f32;
        let score = rel * g.confidence * g.severity.multiplier();

        let mut line = format!(
            "gotcha: {} → {}",
            sanitize_one_line(&g.trigger),
            sanitize_one_line(&g.resolution)
        );
        line = truncate_chars(&line, crate::core::budgets::PROSPECTIVE_REMINDER_MAX_CHARS);
        scored.push(Scored { line, score });
    }

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.line.cmp(&b.line))
    });

    scored
        .into_iter()
        .take(crate::core::budgets::PROSPECTIVE_REMINDERS_LIMIT)
        .map(|s| format!("[remember] {}", s.line))
        .collect()
}

fn tokenize(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(str::to_ascii_lowercase)
        .filter(|w| !crate::core::task_relevance::STOP_WORDS.contains(&w.as_str()))
        .collect()
}

fn sanitize_one_line(s: &str) -> String {
    let mut t = s.replace(['\n', '\r'], " ");
    t = t.replace('`', "");
    while t.contains("  ") {
        t = t.replace("  ", " ");
    }
    t.trim().to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i + 1 >= max {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::gotcha_tracker::{Gotcha, GotchaCategory, GotchaSeverity};
    use chrono::Utc;

    #[test]
    fn reminders_budgeted() {
        let _lock = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        crate::test_env::set_var("SOBRE_DATA_DIR", tmp.path().to_string_lossy().to_string());

        let project_root = tmp.path().join("proj");
        std::fs::create_dir_all(&project_root).expect("mkdir");
        let project_root_str = project_root.to_string_lossy().to_string();

        let mut store = GotchaStore::load(&project_root_str);
        for i in 0..10 {
            store.gotchas.push(Gotcha {
                id: format!("g{i}"),
                category: GotchaCategory::Build,
                severity: GotchaSeverity::Warning,
                trigger: format!("cargo build error E050{i}"),
                resolution: "split borrows".to_string(),
                occurrences: 2,
                first_seen: Utc::now(),
                last_seen: Utc::now(),
                confidence: 0.8,
                decayed_at: None,
            });
        }

        // Persist gotchas where GotchaStore::load expects them.
        store.save(&project_root_str).expect("save");

        let reminders = reminders_for_task(&project_root_str, "fix cargo build error E0502 borrow");
        assert!(reminders.len() <= crate::core::budgets::PROSPECTIVE_REMINDERS_LIMIT);

        crate::test_env::remove_var("SOBRE_DATA_DIR");
    }

    #[test]
    fn only_whole_words_that_are_not_stop_words_match() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        let root = "/tmp/sobre-prospective-whole-words";
        let mut store = GotchaStore::load(root);
        store.report_gotcha(
            "attestation fails when the other cache is stale",
            "clear it first",
            "convention",
            "warning",
        );
        store.save(root).unwrap();

        // "the" is a stop word, "test" is only inside "attestation", "her"
        // only inside "other".
        assert!(reminders_for_task(root, "test the docs for her").is_empty());
        let hits = reminders_for_task(root, "the cache is wrong");
        assert_eq!(hits.len(), 1, "{hits:?}");
    }
}
