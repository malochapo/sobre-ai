use super::intent_lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TaskType {
    Generate,
    FixBug,
    Refactor,
    Explore,
    Test,
    Debug,
    Config,
    Deploy,
    Review,
}

#[derive(Debug)]
pub struct TaskClassification {
    pub task_type: TaskType,
    pub targets: Vec<String>,
    pub keywords: Vec<String>,
}

const PHRASE_RULES: &[(&[&str], TaskType, f64)] = &[
    (
        &[
            "add",
            "create",
            "implement",
            "build",
            "write",
            "generate",
            "make",
            "new feature",
            "new",
        ],
        TaskType::Generate,
        0.9,
    ),
    (
        &[
            "fix",
            "bug",
            "broken",
            "crash",
            "error in",
            "not working",
            "fails",
            "wrong output",
        ],
        TaskType::FixBug,
        0.95,
    ),
    (
        &[
            "refactor",
            "clean up",
            "restructure",
            "rename",
            "move",
            "extract",
            "simplify",
            "split",
        ],
        TaskType::Refactor,
        0.9,
    ),
    (
        &[
            "how",
            "what",
            "where",
            "explain",
            "understand",
            "show me",
            "describe",
            "why does",
        ],
        TaskType::Explore,
        0.85,
    ),
    (
        &[
            "test",
            "spec",
            "coverage",
            "assert",
            "unit test",
            "integration test",
            "mock",
        ],
        TaskType::Test,
        0.9,
    ),
    (
        &[
            "debug",
            "trace",
            "inspect",
            "log",
            "breakpoint",
            "step through",
            "stack trace",
        ],
        TaskType::Debug,
        0.9,
    ),
    (
        &[
            "config",
            "setup",
            "install",
            "env",
            "configure",
            "settings",
            "dotenv",
        ],
        TaskType::Config,
        0.85,
    ),
    (
        &[
            "deploy", "release", "publish", "ship", "ci/cd", "pipeline", "docker",
        ],
        TaskType::Deploy,
        0.85,
    ),
    (
        &[
            "review",
            "check",
            "audit",
            "look at",
            "evaluate",
            "assess",
            "pr review",
        ],
        TaskType::Review,
        0.8,
    ),
];

pub fn classify(query: &str) -> TaskClassification {
    let q = query.to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    let lang = intent_lang::detect_query_lang(&words);

    let mut best_type = TaskType::Explore;
    let mut best_score = 0.0_f64;

    apply_rules(
        &q,
        &words,
        PHRASE_RULES,
        |word, phrase| word == phrase,
        &mut best_type,
        &mut best_score,
    );
    // Multilingual stems (de/fr/es) match morphology-tolerant via prefix.
    apply_rules(
        &q,
        &words,
        intent_lang::STEM_RULES,
        |word, stem| word.starts_with(stem),
        &mut best_type,
        &mut best_score,
    );

    let targets = extract_targets(query, lang);
    let keywords = extract_keywords(&q, lang);

    TaskClassification {
        task_type: best_type,
        targets,
        keywords,
    }
}

/// Shared scoring loop for PHRASE_RULES (exact word match) and
/// STEM_RULES (prefix match). Multi-word entries match the whole query.
fn apply_rules(
    q: &str,
    words: &[&str],
    rules: &[(&[&str], TaskType, f64)],
    token_match: fn(&str, &str) -> bool,
    best_type: &mut TaskType,
    best_score: &mut f64,
) {
    for &(phrases, task_type, base_confidence) in rules {
        let mut match_count = 0usize;
        for phrase in phrases {
            if phrase.contains(' ') {
                if q.contains(phrase) {
                    match_count += 2;
                }
            } else if words.iter().any(|w| token_match(w, phrase)) {
                match_count += 1;
            }
        }
        if match_count > 0 {
            let score = base_confidence * (match_count as f64).min(2.0) / 2.0;
            if score > *best_score {
                *best_score = score;
                *best_type = task_type;
            }
        }
    }
}

fn extract_targets(query: &str, lang: intent_lang::QueryLang) -> Vec<String> {
    let mut targets = Vec::new();

    for word in query.split_whitespace() {
        if word.contains('.') && !word.starts_with('.') {
            let clean = word.trim_matches(|c: char| {
                !c.is_alphanumeric() && c != '.' && c != '/' && c != '_' && c != '-'
            });
            if looks_like_path(clean) {
                targets.push(clean.to_string());
            }
        }
        if word.contains('/') && !word.starts_with("//") && !word.starts_with("http") {
            let clean = word.trim_matches(|c: char| {
                !c.is_alphanumeric() && c != '.' && c != '/' && c != '_' && c != '-'
            });
            if clean.len() > 2 {
                targets.push(clean.to_string());
            }
        }
    }

    for word in query.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if w.contains('_') && w.len() > 3 && !targets.contains(&w.to_string()) {
            targets.push(w.to_string());
        }
        if w.chars().any(char::is_uppercase)
            && w.len() > 2
            && !is_stop_word(w)
            && !intent_lang::is_stop_word_for(lang, &w.to_lowercase())
            && !targets.contains(&w.to_string())
        {
            targets.push(w.to_string());
        }
    }

    targets.truncate(5);
    targets
}

fn looks_like_path(s: &str) -> bool {
    let exts = [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".toml", ".yaml", ".yml", ".json", ".md",
    ];
    exts.iter().any(|ext| s.ends_with(ext)) || s.contains('/')
}

fn is_stop_word(w: &str) -> bool {
    matches!(
        w.to_lowercase().as_str(),
        "the"
            | "this"
            | "that"
            | "with"
            | "from"
            | "into"
            | "have"
            | "please"
            | "could"
            | "would"
            | "should"
            | "also"
            | "just"
            | "then"
            | "when"
            | "what"
            | "where"
            | "which"
            | "there"
            | "here"
            | "these"
            | "those"
            | "does"
            | "will"
            | "shall"
            | "can"
            | "may"
            | "must"
            | "need"
            | "want"
            | "like"
            | "make"
            | "take"
    )
}

fn extract_keywords(query: &str, lang: intent_lang::QueryLang) -> Vec<String> {
    query
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .filter(|w| !is_stop_word(w))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .filter(|w| !intent_lang::is_stop_word_for(lang, w))
        .take(8)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    Mechanical,
    Standard,
    Architectural,
}

pub fn classify_complexity(query: &str, classification: &TaskClassification) -> TaskComplexity {
    let q = query.to_lowercase();
    let word_count = q.split_whitespace().count();
    let target_count = classification.targets.len();

    let has_multi_file = target_count >= 3;
    let has_cross_cutting = q.contains("all files")
        || q.contains("across")
        || q.contains("everywhere")
        || q.contains("every")
        || q.contains("migration")
        || q.contains("architecture");

    let is_simple = word_count < 8
        && target_count <= 1
        && matches!(
            classification.task_type,
            TaskType::Generate | TaskType::Config
        );

    if is_simple {
        TaskComplexity::Mechanical
    } else if has_multi_file || has_cross_cutting {
        TaskComplexity::Architectural
    } else {
        TaskComplexity::Standard
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentScope {
    SingleFile,
    MultiFile,
    CrossModule,
    ProjectWide,
}

/// What `aggressive` mode needs from a stated task: its type, its keywords
/// and how wide it reaches.
#[derive(Debug, Clone)]
pub struct StructuredIntent {
    pub task_type: TaskType,
    pub keywords: Vec<String>,
    pub scope: IntentScope,
}

impl StructuredIntent {
    pub fn from_query(query: &str) -> Self {
        let classification = classify(query);
        let complexity = classify_complexity(query, &classification);
        let file_targets = classification
            .targets
            .iter()
            .filter(|t| t.contains('.') || t.contains('/'))
            .count();
        let scope = match complexity {
            TaskComplexity::Mechanical => IntentScope::SingleFile,
            TaskComplexity::Standard => {
                if file_targets > 1 {
                    IntentScope::MultiFile
                } else {
                    IntentScope::SingleFile
                }
            }
            TaskComplexity::Architectural => {
                let q = query.to_lowercase();
                if q.contains("all files") || q.contains("everywhere") || q.contains("migration") {
                    IntentScope::ProjectWide
                } else {
                    IntentScope::CrossModule
                }
            }
        };

        StructuredIntent {
            task_type: classification.task_type,
            keywords: classification.keywords,
            scope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_fix_bug() {
        let r = classify("fix the bug in entropy.rs where token_entropy returns NaN");
        assert_eq!(r.task_type, TaskType::FixBug);
        assert!(r.targets.iter().any(|t| t.contains("entropy.rs")));
    }

    #[test]
    fn classify_generate() {
        let r = classify("add a new function normalized_token_entropy to entropy.rs");
        assert_eq!(r.task_type, TaskType::Generate);
    }

    #[test]
    fn classify_refactor() {
        let r = classify("refactor the compression pipeline to split into smaller modules");
        assert_eq!(r.task_type, TaskType::Refactor);
    }

    #[test]
    fn classify_explore() {
        let r = classify("how does the session cache work?");
        assert_eq!(r.task_type, TaskType::Explore);
    }

    #[test]
    fn classify_debug() {
        let r = classify("debug why the compression ratio drops for large files");
        assert_eq!(r.task_type, TaskType::Debug);
    }

    #[test]
    fn classify_test() {
        let r = classify("write unit tests for the token_optimizer module");
        assert_eq!(r.task_type, TaskType::Test);
    }

    #[test]
    fn targets_extract_paths() {
        let r = classify("fix entropy.rs and update core/mod.rs");
        assert!(r.targets.iter().any(|t| t.contains("entropy.rs")));
        assert!(r.targets.iter().any(|t| t.contains("core/mod.rs")));
    }

    #[test]
    fn targets_extract_identifiers() {
        let r = classify("refactor SessionCache to use LRU eviction");
        assert!(r.targets.iter().any(|t| t == "SessionCache"));
    }

    #[test]
    fn fallback_to_explore() {
        let r = classify("xyz qqq bbb");
        assert_eq!(r.task_type, TaskType::Explore);
    }

    #[test]
    fn complexity_mechanical() {
        let r = classify("add a comment");
        let c = classify_complexity("add a comment", &r);
        assert_eq!(c, super::TaskComplexity::Mechanical);
    }

    #[test]
    fn complexity_architectural() {
        let r = classify("refactor auth across all files and update the migration");
        let c = classify_complexity(
            "refactor auth across all files and update the migration",
            &r,
        );
        assert_eq!(c, super::TaskComplexity::Architectural);
    }

    /// de/fr/es sentences per TaskType, ≥90% hit rate.
    #[test]
    fn classify_multilingual_table() {
        let table: &[(&str, TaskType)] = &[
            // German
            ("behebe den fehler in auth.rs", TaskType::FixBug),
            (
                "erstelle eine neue funktion für das datums-parsing",
                TaskType::Generate,
            ),
            ("räum die funktion auf", TaskType::Refactor),
            (
                "refaktorisiere das modul in kleinere teile",
                TaskType::Refactor,
            ),
            (
                "erkläre wie der session cache funktioniert",
                TaskType::Explore,
            ),
            ("schreibe tests für den parser", TaskType::Test),
            (
                "prüfe ob die validierung korrekt funktioniert",
                TaskType::Test,
            ),
            ("debugge warum der server abstürzt", TaskType::Debug),
            (
                "konfiguriere die umgebungsvariablen für den daemon",
                TaskType::Config,
            ),
            ("veröffentliche die neue version", TaskType::Deploy),
            ("überprüfe die änderungen vor dem merge", TaskType::Review),
            // French
            ("corrige le bug dans le parseur", TaskType::FixBug),
            ("ajoute une fonction de validation", TaskType::Generate),
            ("explique comment fonctionne le cache", TaskType::Explore),
            ("nettoie ce module pour le simplifier", TaskType::Refactor),
            ("vérifie que les tests passent", TaskType::Test),
            ("déploie la nouvelle version", TaskType::Deploy),
            ("pourquoi le serveur plante-t-il", TaskType::Explore),
            // Spanish
            ("corrige el error en el módulo de auth", TaskType::FixBug),
            ("crea una función para parsear fechas", TaskType::Generate),
            ("explica cómo funciona la caché", TaskType::Explore),
            ("agrega soporte para webhooks", TaskType::Generate),
            (
                "muestra dónde se define la configuración",
                TaskType::Explore,
            ),
            ("revisa este pull request", TaskType::Review),
            ("despliega la nueva versión", TaskType::Deploy),
            ("escribe pruebas para el parser", TaskType::Test),
        ];

        let misses: Vec<String> = table
            .iter()
            .filter_map(|(query, expected)| {
                let got = classify(query).task_type;
                (got != *expected).then(|| format!("'{query}': want {expected:?}, got {got:?}"))
            })
            .collect();

        let hit_rate = (table.len() - misses.len()) as f64 / table.len() as f64;
        assert!(
            hit_rate >= 0.9,
            "multilingual hit rate {:.0}% < 90%:\n{}",
            hit_rate * 100.0,
            misses.join("\n")
        );
    }

    /// German filler words must not pollute keywords.
    #[test]
    fn keywords_filter_german_fillers() {
        let r = classify("bitte kannst du diese funktion aufräumen, ich möchte sauberen code");
        for filler in ["bitte", "kannst", "möchte", "diese"] {
            assert!(
                !r.keywords.iter().any(|k| k == filler),
                "filler '{filler}' leaked into keywords: {:?}",
                r.keywords
            );
        }
        assert!(r.keywords.iter().any(|k| k == "funktion"));
        assert_eq!(r.task_type, TaskType::Refactor);
    }

    #[test]
    fn targets_filter_capitalized_german_fillers() {
        let r = classify("Bitte behebe den Fehler in der Konfiguration");
        assert!(!r.targets.iter().any(|t| t == "Bitte"), "{:?}", r.targets);
        assert_eq!(r.task_type, TaskType::FixBug);
    }

    #[test]
    fn german_fix_is_a_bug_fix() {
        let intent = StructuredIntent::from_query("dringend: behebe den absturz sofort");
        assert_eq!(intent.task_type, TaskType::FixBug);
    }

    #[test]
    fn german_tidy_up_is_a_refactor() {
        let intent = StructuredIntent::from_query("räum die funktion auf");
        assert_eq!(intent.task_type, TaskType::Refactor);
    }
}
