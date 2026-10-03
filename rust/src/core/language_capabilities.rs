use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageId {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    C,
    Cpp,
    Ruby,
    CSharp,
    Kotlin,
    Swift,
    Php,
    Bash,
    Dart,
    Scala,
    Elixir,
    Zig,
    Gdscript,
    Lua,
    Luau,
    Vue,
    Svelte,
    /// Godot `PackedScene` text format (`.tscn`): not source code, but carries
    /// Scene→Script dependency edges.
    Tscn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageCapabilities {
    pub deps_edges: bool,
    pub deep_queries: bool,
    pub import_resolver: bool,
}

impl LanguageId {
    pub fn id_str(&self) -> &'static str {
        match self {
            LanguageId::Rust => "rust",
            LanguageId::TypeScript => "typescript",
            LanguageId::JavaScript => "javascript",
            LanguageId::Python => "python",
            LanguageId::Go => "go",
            LanguageId::Java => "java",
            LanguageId::C => "c",
            LanguageId::Cpp => "cpp",
            LanguageId::Ruby => "ruby",
            LanguageId::CSharp => "csharp",
            LanguageId::Kotlin => "kotlin",
            LanguageId::Swift => "swift",
            LanguageId::Php => "php",
            LanguageId::Bash => "bash",
            LanguageId::Dart => "dart",
            LanguageId::Scala => "scala",
            LanguageId::Elixir => "elixir",
            LanguageId::Zig => "zig",
            LanguageId::Gdscript => "gdscript",
            LanguageId::Lua => "lua",
            LanguageId::Luau => "luau",
            LanguageId::Vue => "vue",
            LanguageId::Svelte => "svelte",
            LanguageId::Tscn => "tscn",
        }
    }
}

pub fn capabilities(lang: LanguageId) -> LanguageCapabilities {
    match lang {
        // tree-sitter backed (deep_queries + resolver can be meaningful)
        LanguageId::Rust
        | LanguageId::TypeScript
        | LanguageId::JavaScript
        | LanguageId::Python
        | LanguageId::Go
        | LanguageId::Java
        | LanguageId::C
        | LanguageId::Cpp
        | LanguageId::Ruby
        | LanguageId::CSharp
        | LanguageId::Kotlin
        | LanguageId::Swift
        | LanguageId::Php
        | LanguageId::Bash
        | LanguageId::Dart
        | LanguageId::Scala
        | LanguageId::Elixir
        | LanguageId::Zig
        | LanguageId::Gdscript
        | LanguageId::Lua
        | LanguageId::Luau => LanguageCapabilities {
            deps_edges: true,
            deep_queries: true,
            import_resolver: true,
        },
        // templating languages: we can extract deps edges, but no deep_queries/resolver.
        LanguageId::Vue | LanguageId::Svelte => LanguageCapabilities {
            deps_edges: true,
            deep_queries: false,
            import_resolver: false,
        },
        // Godot scenes: no symbols (not source code), but resolved Scene→Script
        // import edges via the GDScript `res://` resolver.
        LanguageId::Tscn => LanguageCapabilities {
            deps_edges: true,
            deep_queries: false,
            import_resolver: true,
        },
    }
}

pub fn language_for_ext(ext: &str) -> Option<LanguageId> {
    let e = ext.trim().trim_start_matches('.').to_lowercase();
    match e.as_str() {
        "rs" => Some(LanguageId::Rust),
        "ts" | "tsx" => Some(LanguageId::TypeScript),
        "js" | "jsx" => Some(LanguageId::JavaScript),
        "py" => Some(LanguageId::Python),
        "go" => Some(LanguageId::Go),
        "java" => Some(LanguageId::Java),
        "c" | "h" => Some(LanguageId::C),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some(LanguageId::Cpp),
        "rb" => Some(LanguageId::Ruby),
        "cs" => Some(LanguageId::CSharp),
        "kt" | "kts" => Some(LanguageId::Kotlin),
        "swift" => Some(LanguageId::Swift),
        "php" => Some(LanguageId::Php),
        "sh" | "bash" => Some(LanguageId::Bash),
        "dart" => Some(LanguageId::Dart),
        "scala" | "sc" => Some(LanguageId::Scala),
        "ex" | "exs" => Some(LanguageId::Elixir),
        "zig" => Some(LanguageId::Zig),
        "gd" => Some(LanguageId::Gdscript),
        "lua" => Some(LanguageId::Lua),
        "luau" => Some(LanguageId::Luau),
        "vue" => Some(LanguageId::Vue),
        "svelte" => Some(LanguageId::Svelte),
        "tscn" => Some(LanguageId::Tscn),
        _ => None,
    }
}

pub fn language_for_path(path: &str) -> Option<LanguageId> {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(language_for_ext)
}

pub fn is_indexable_ext(ext: &str) -> bool {
    language_for_ext(ext).is_some()
}

/// Every language the property graph / code-map can index, for capability
/// enumeration and UI hints. Keep in sync with `language_for_ext`.
pub const ALL_LANGUAGES: &[LanguageId] = &[
    LanguageId::Rust,
    LanguageId::TypeScript,
    LanguageId::JavaScript,
    LanguageId::Python,
    LanguageId::Go,
    LanguageId::Java,
    LanguageId::C,
    LanguageId::Cpp,
    LanguageId::Ruby,
    LanguageId::CSharp,
    LanguageId::Kotlin,
    LanguageId::Swift,
    LanguageId::Php,
    LanguageId::Bash,
    LanguageId::Dart,
    LanguageId::Scala,
    LanguageId::Elixir,
    LanguageId::Zig,
    LanguageId::Gdscript,
    LanguageId::Lua,
    LanguageId::Luau,
    LanguageId::Vue,
    LanguageId::Svelte,
    LanguageId::Tscn,
];

/// Whether lean-ctx extracts call sites for a language (i.e. it can populate the
/// call graph). Keep in sync with `deep_queries::calls::parse_call` — a language
/// missing there yields zero call edges, which the dashboard must communicate
/// honestly instead of suggesting an index rebuild that cannot help.
pub fn supports_call_graph(lang: LanguageId) -> bool {
    matches!(
        lang,
        LanguageId::TypeScript
            | LanguageId::JavaScript
            | LanguageId::Rust
            | LanguageId::Python
            | LanguageId::Go
            | LanguageId::Java
            | LanguageId::Kotlin
            | LanguageId::Gdscript
            | LanguageId::Lua
            | LanguageId::Luau
            | LanguageId::CSharp
    )
}

/// Friendly names of every language with call-graph extraction support.
pub fn callgraph_supported_language_names() -> Vec<&'static str> {
    ALL_LANGUAGES
        .iter()
        .filter(|l| supports_call_graph(**l))
        .map(LanguageId::id_str)
        .collect()
}

/// Per-language capability row for a project: which analyses are available for a
/// language and how many files use it. Backs the dashboard capability legend so
/// each detected language is labelled honestly (symbols / import edges / calls).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LanguageCapabilityRow {
    pub language: &'static str,
    pub files: usize,
    pub symbols: bool,
    pub imports: bool,
    pub call_graph: bool,
    /// Symbols actually extracted for this language in *this* project. `None`
    /// when realized counts weren't measured in the calling context.
    pub symbols_found: Option<usize>,
    /// Import/reexport edges whose source file is in this language.
    pub imports_found: Option<usize>,
    /// Call edges whose caller file is in this language. `None` when call data
    /// isn't available in the calling context (e.g. the dependency-graph route).
    pub calls_found: Option<usize>,
}

/// Build a capability matrix for the languages actually present in `file_paths`,
/// sorted by file count (desc) then name. `symbols`/`imports` come from
/// `capabilities()`; `call_graph` from `supports_call_graph()`.
pub fn language_capability_matrix<I, S>(file_paths: I) -> Vec<LanguageCapabilityRow>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut counts: std::collections::HashMap<LanguageId, usize> = std::collections::HashMap::new();
    for path in file_paths {
        if let Some(lang) = language_for_path(path.as_ref()) {
            *counts.entry(lang).or_default() += 1;
        }
    }
    let mut rows: Vec<LanguageCapabilityRow> = counts
        .into_iter()
        .map(|(lang, files)| {
            let caps = capabilities(lang);
            LanguageCapabilityRow {
                language: lang.id_str(),
                files,
                symbols: caps.deep_queries,
                imports: caps.import_resolver,
                call_graph: supports_call_graph(lang),
                symbols_found: None,
                imports_found: None,
                calls_found: None,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.files
            .cmp(&a.files)
            .then_with(|| a.language.cmp(b.language))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_mapping_basic() {
        assert_eq!(language_for_ext("rs"), Some(LanguageId::Rust));
        assert_eq!(language_for_ext(".tsx"), Some(LanguageId::TypeScript));
        assert_eq!(language_for_ext("JS"), Some(LanguageId::JavaScript));
        assert_eq!(language_for_ext("hxx"), Some(LanguageId::Cpp));
        assert_eq!(language_for_ext("exs"), Some(LanguageId::Elixir));
        assert_eq!(language_for_ext("unknown"), None);
    }

    #[test]
    fn indexable_ext_true_for_known() {
        assert!(is_indexable_ext("rs"));
        assert!(is_indexable_ext("vue"));
        assert!(!is_indexable_ext("md"));
    }

    #[test]
    fn caps_are_deterministic() {
        let c1 = capabilities(LanguageId::Rust);
        let c2 = capabilities(LanguageId::Rust);
        assert_eq!(c1, c2);
        assert!(c1.deps_edges);
    }

    #[test]
    fn callgraph_support_is_consistent() {
        // C# must be call-graph capable (issue: NINA's empty Call Graph tab).
        assert!(supports_call_graph(LanguageId::CSharp));
        let names = callgraph_supported_language_names();
        assert!(names.contains(&"csharp"));
        assert!(names.contains(&"rust"));
        assert!(names.contains(&"typescript"));
        // Every call-graph language is also graph-indexable, and the list is a
        // strict subset (some graph-indexed languages have no call extraction).
        let indexed: Vec<&str> = ALL_LANGUAGES.iter().map(LanguageId::id_str).collect();
        for name in &names {
            assert!(indexed.contains(name));
        }
        assert!(names.len() <= ALL_LANGUAGES.len());
        // A language without call extraction must report false.
        assert!(!supports_call_graph(LanguageId::Ruby));
    }

    #[test]
    fn lua_luau_are_first_class_indexed() {
        // Lua/Luau (issue #360) are graph-indexed.
        assert_eq!(language_for_ext("lua"), Some(LanguageId::Lua));
        assert_eq!(language_for_ext(".luau"), Some(LanguageId::Luau));
        assert!(is_indexable_ext("lua"));
        assert!(is_indexable_ext("luau"));
        // They participate in symbols, import edges and the call graph.
        assert!(supports_call_graph(LanguageId::Lua));
        assert!(supports_call_graph(LanguageId::Luau));
    }

    #[test]
    fn non_indexed_source_extensions_are_not_indexable() {
        assert!(!is_indexable_ext("r"));
        assert!(!is_indexable_ext("jl"));
    }

    #[test]
    fn capability_matrix_reports_per_language_support() {
        let paths = ["a.rs", "b.rs", "c.rb", "d.cs", "readme.md"];
        let matrix = language_capability_matrix(paths);

        // Non-code files are excluded; three languages are detected.
        assert_eq!(matrix.len(), 3);

        let rust = matrix.iter().find(|r| r.language == "rust").unwrap();
        assert_eq!(rust.files, 2);
        assert!(rust.symbols && rust.imports && rust.call_graph);

        // Ruby has symbols + imports but no call-graph extraction.
        let ruby = matrix.iter().find(|r| r.language == "ruby").unwrap();
        assert!(ruby.symbols && ruby.imports && !ruby.call_graph);

        // C# is fully supported (the language behind the original bug report).
        let csharp = matrix.iter().find(|r| r.language == "csharp").unwrap();
        assert!(csharp.symbols && csharp.imports && csharp.call_graph);

        // Sorted by file count desc → Rust (2 files) leads.
        assert_eq!(matrix[0].language, "rust");
    }
}
