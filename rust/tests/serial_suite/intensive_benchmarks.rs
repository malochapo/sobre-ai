use sobre::core::compressor::{aggressive_compress, lightweight_cleanup, safeguard_ratio};
use sobre::core::config::CompressionLevel;
use sobre::core::entropy::entropy_compress_adaptive;
use sobre::core::signatures::extract_signatures;
use sobre::core::tokens::count_tokens;

/// The instructions with no output style, so budgets do not move with the
/// developer's own `compression_level`.
fn instructions_at_off() -> String {
    sobre::instructions::render(CompressionLevel::Off)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compression_ratio(original: usize, compressed: usize) -> f64 {
    if original == 0 {
        return 0.0;
    }
    (original - compressed) as f64 / original as f64 * 100.0
}

fn measure_pattern(command: &str, output: &str) -> (usize, usize, f64) {
    let original = count_tokens(output);
    let compressed = sobre::core::patterns::compress_output(command, output)
        .unwrap_or_else(|| output.to_string());
    let comp_tokens = count_tokens(&compressed);
    (
        original,
        comp_tokens,
        compression_ratio(original, comp_tokens),
    )
}

fn measure_text(original: &str, compressed: &str) -> (usize, usize, f64) {
    let orig_t = count_tokens(original);
    let comp_t = count_tokens(compressed);
    (orig_t, comp_t, compression_ratio(orig_t, comp_t))
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 1: INPUT TOKEN BENCHMARKS — Tool Descriptions + System Instructions
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn bench_system_instructions_token_count() {
    let instructions = instructions_at_off();
    let tokens = count_tokens(&instructions);
    eprintln!(
        "\n  SYSTEM INSTRUCTIONS: {tokens} tokens ({} chars)",
        instructions.len()
    );
    assert!(
        tokens < 2300,
        "Base instructions should be <2300 tokens, got {tokens}"
    );
    // Claude Code truncates server instructions at 2048 characters; the text
    // every client gets must arrive whole there.
    for level in [
        CompressionLevel::Off,
        CompressionLevel::Lite,
        CompressionLevel::Standard,
        CompressionLevel::Max,
        CompressionLevel::Raw,
    ] {
        let chars = sobre::instructions::render(level).len();
        assert!(
            chars <= 2048,
            "{level:?} instructions are {chars} chars, over Claude Code's 2048"
        );
    }
}

#[test]
fn bench_tool_descriptions_token_count() {
    let descriptions = sobre::server::tool_descriptions_for_test();

    let mut total = 0usize;
    eprintln!("\n{}", "=".repeat(70));
    eprintln!("  TOOL DESCRIPTION TOKEN COUNTS");
    eprintln!("{}", "=".repeat(70));
    eprintln!("  {:<25} {:>8} {:>8}", "Tool", "Tokens", "Chars");
    eprintln!("  {}", "-".repeat(45));

    for (name, desc) in &descriptions {
        let t = count_tokens(desc);
        total += t;
        eprintln!("  {:<25} {:>8} {:>8}", name, t, desc.len());
    }

    eprintln!("  {}", "-".repeat(45));
    eprintln!("  {:<25} {:>8}", "TOTAL", total);
    eprintln!("{}", "=".repeat(70));

    // Budget = the registry's tool surface: every tool's full description,
    // exactly what `tools/list` sends. Set with headroom over the actual;
    // raise it only for a material jump, not for a routine edit.
    assert!(
        total < 6000,
        "Total tool description tokens should be <6000, got {total}"
    );

    for (name, desc) in &descriptions {
        // Every tool stays within a tight per-tool description budget.
        let t = count_tokens(desc);
        assert!(
            t < 160,
            "Tool '{name}' description should be <160 tokens, got {t}"
        );
    }
}

#[test]
fn bench_total_input_overhead() {
    let instructions = instructions_at_off();
    let descs = sobre::server::tool_descriptions_for_test();
    let schemas = sobre::server::tool_schemas_json_for_test();

    let instr_tokens = count_tokens(&instructions);
    let desc_tokens: usize = descs.iter().map(|(_, d)| count_tokens(d)).sum();
    let schema_tokens = count_tokens(&schemas);

    let total = instr_tokens + desc_tokens + schema_tokens;

    eprintln!("\n{}", "=".repeat(70));
    eprintln!("  TOTAL INPUT OVERHEAD (per session start)");
    eprintln!("{}", "=".repeat(70));
    eprintln!("  System instructions: {instr_tokens:>6} tokens");
    eprintln!("  Tool descriptions:   {desc_tokens:>6} tokens");
    eprintln!("  Tool schemas (JSON): {schema_tokens:>6} tokens");
    eprintln!("  {}", "-".repeat(40));
    eprintln!("  TOTAL overhead:      {total:>6} tokens");
    eprintln!(
        "  Estimated cost @$3/1M input: ${:.4}",
        total as f64 * 3.0 / 1_000_000.0
    );
    eprintln!("{}", "=".repeat(70));

    // Everything a session pays before its first call: instructions, tool
    // descriptions and schemas. Set with headroom over the actual.
    assert!(
        total < 18000,
        "Total input overhead should be <18000 tokens, got {total}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 2: PATTERN COMPRESSION BENCHMARKS — Shell Output Compression
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn bench_git_all_commands() {
    let scenarios = vec![
        ("git log -p -10", generate_git_log_patch(10), 70.0),
        ("git log -p -50", generate_git_log_patch(50), 80.0),
        ("git log --stat -20", generate_git_log_stat(20), 50.0),
        ("git log -30", generate_git_log_standard(30), 60.0),
        ("git status", generate_git_status(), 30.0),
        ("git diff", generate_git_diff(15), 5.0),
        (
            "git commit -m 'feat'",
            generate_git_commit_with_hooks(30),
            0.0, // verbatim: git write-commands are never compressed
        ),
        ("git push origin main", generate_git_push(), 0.0), // verbatim
    ];

    print_compression_report("GIT COMMANDS", &scenarios);
}

#[test]
fn bench_cargo_all_commands() {
    let scenarios = vec![
        ("cargo build", generate_cargo_build_success(), 30.0),
        ("cargo build", generate_cargo_build_with_warnings(20), 40.0),
        ("cargo test", generate_cargo_test(50, 2), 40.0),
        ("cargo clippy", generate_cargo_clippy(10), 30.0),
    ];

    print_compression_report("CARGO COMMANDS", &scenarios);
}

#[test]
fn bench_docker_all_commands() {
    let scenarios = vec![
        ("docker ps", generate_docker_ps(10), 0.0), // Verbatim via OutputPolicy
        ("docker images", generate_docker_images(15), 0.0), // Verbatim via OutputPolicy
        ("docker build -t app .", generate_docker_build(20), 40.0),
    ];

    print_compression_report("DOCKER COMMANDS", &scenarios);
}

#[test]
fn bench_npm_all_commands() {
    let scenarios = vec![
        ("npm install", generate_npm_install(30), 0.0), // Verbatim via OutputPolicy (npm install -> Passthrough)
        ("npm test", generate_npm_test_jest(), 30.0),
        ("npm ls", generate_npm_ls(20), 0.0), // Verbatim via OutputPolicy (is_package_manager_info)
    ];

    print_compression_report("NPM COMMANDS", &scenarios);
}

#[test]
fn bench_pip_commands() {
    let scenarios = vec![
        (
            "pip install -r requirements.txt",
            generate_pip_install(15),
            0.0, // Verbatim via OutputPolicy (is_package_manager_info)
        ),
        ("pip list", generate_pip_list(30), 0.0), // Verbatim via OutputPolicy (is_package_manager_info)
    ];

    print_compression_report("PIP COMMANDS", &scenarios);
}

#[test]
fn bench_kubectl_commands() {
    let scenarios = vec![
        ("kubectl get pods", generate_kubectl_pods(20), 0.0),
        ("kubectl get pods -A", generate_kubectl_pods_all(30), 0.0),
    ];

    print_compression_report("KUBECTL COMMANDS", &scenarios);
}

#[test]
fn bench_pattern_coverage_comprehensive() {
    let all = vec![
        ("git log -p -10", generate_git_log_patch(10)),
        ("git log --stat -20", generate_git_log_stat(20)),
        ("git status", generate_git_status()),
        ("git diff", generate_git_diff(15)),
        ("git commit -m 'feat'", generate_git_commit_with_hooks(30)),
        ("cargo build", generate_cargo_build_with_warnings(20)),
        ("cargo test", generate_cargo_test(50, 2)),
        ("cargo clippy", generate_cargo_clippy(10)),
        ("docker ps", generate_docker_ps(10)),
        ("docker build -t app .", generate_docker_build(20)),
        ("npm install", generate_npm_install(30)),
        ("npm test", generate_npm_test_jest()),
        ("pip install -r requirements.txt", generate_pip_install(15)),
        ("kubectl get pods", generate_kubectl_pods(20)),
    ];

    let mut total_orig = 0usize;
    let mut total_comp = 0usize;

    eprintln!("\n{}", "=".repeat(80));
    eprintln!("  COMPREHENSIVE PATTERN COVERAGE — ALL COMMANDS");
    eprintln!("{}", "=".repeat(80));
    eprintln!(
        "  {:<40} {:>8} {:>8} {:>7}",
        "Command", "Original", "Compr.", "Saved%"
    );
    eprintln!("  {}", "-".repeat(65));

    for (cmd, output) in &all {
        let (orig, comp, pct) = measure_pattern(cmd, output);
        total_orig += orig;
        total_comp += comp;
        eprintln!("  {cmd:<40} {orig:>8} {comp:>8} {pct:>6.1}%");
    }

    let total_pct = compression_ratio(total_orig, total_comp);
    eprintln!("  {}", "-".repeat(65));
    eprintln!(
        "  {:<40} {:>8} {:>8} {:>6.1}%",
        "TOTAL", total_orig, total_comp, total_pct
    );
    eprintln!("{}", "=".repeat(80));

    assert!(
        total_pct > 40.0,
        "Overall pattern savings should be >40%, got {total_pct:.1}%"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3: COMPRESSION MODE BENCHMARKS — sobre_read modes on real-like files
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn bench_aggressive_mode_rust() {
    let content = generate_rust_file(200);
    let compressed = aggressive_compress(&content, Some("rs"));
    let (orig, comp, pct) = measure_text(&content, &compressed);

    eprintln!("\n  [aggressive mode, Rust 200L] {orig} → {comp} tokens ({pct:.1}% saved)");
    assert!(
        pct > 5.0,
        "Aggressive on Rust should save >5%, got {pct:.1}%"
    );
}

#[test]
fn bench_aggressive_mode_python() {
    let content = generate_python_file(200);
    let compressed = aggressive_compress(&content, Some("py"));
    let (orig, comp, pct) = measure_text(&content, &compressed);

    eprintln!("\n  [aggressive mode, Python 200L] {orig} → {comp} tokens ({pct:.1}% saved)");
    assert!(
        pct > 5.0,
        "Aggressive on Python should save >5%, got {pct:.1}%"
    );
}

#[test]
fn bench_aggressive_mode_typescript() {
    let content = generate_typescript_file(200);
    let compressed = aggressive_compress(&content, Some("ts"));
    let (orig, comp, pct) = measure_text(&content, &compressed);

    eprintln!("\n  [aggressive mode, TypeScript 200L] {orig} → {comp} tokens ({pct:.1}% saved)");
    assert!(
        pct > 15.0,
        "Aggressive on TS should save >15%, got {pct:.1}%"
    );
}

#[test]
fn bench_entropy_mode_rust() {
    let content = generate_rust_file(200);
    let result = entropy_compress_adaptive(&content, "bench.rs", &[]);
    let (orig, comp, pct) = measure_text(&content, &result.output);

    eprintln!("\n  [entropy mode, Rust 200L] {orig} → {comp} tokens ({pct:.1}% saved)");
    eprintln!("  Techniques: {:?}", result.techniques);
    assert!(pct > 3.0, "Entropy on Rust should save >3%, got {pct:.1}%");
}

#[test]
fn bench_signatures_mode_rust() {
    let content = generate_rust_file(200);
    let sigs = extract_signatures(&content, "rs");

    let sig_output = sigs.iter().fold(String::new(), |mut s, sig| {
        use std::fmt::Write;
        let _ = writeln!(s, "{}", sig.to_compact());
        s
    });
    let (orig, comp, pct) = measure_text(&content, &sig_output);

    eprintln!(
        "\n  [signatures mode, Rust 200L] {orig} → {comp} tokens ({pct:.1}% saved), {} signatures",
        sigs.len()
    );
    assert!(
        pct > 50.0,
        "Signatures on Rust should save >50%, got {pct:.1}%"
    );
    assert!(
        sigs.len() > 5,
        "Should extract >5 signatures, got {}",
        sigs.len()
    );
}

#[test]
fn bench_signatures_mode_typescript() {
    let content = generate_typescript_file(200);
    let sigs = extract_signatures(&content, "ts");

    let sig_output = sigs.iter().fold(String::new(), |mut s, sig| {
        use std::fmt::Write;
        let _ = writeln!(s, "{}", sig.to_compact());
        s
    });
    let (orig, comp, pct) = measure_text(&content, &sig_output);

    eprintln!(
        "\n  [signatures mode, TS 200L] {orig} → {comp} tokens ({pct:.1}% saved), {} signatures",
        sigs.len()
    );
    assert!(
        pct > 40.0,
        "Signatures on TS should save >40%, got {pct:.1}%"
    );
}

#[test]
fn bench_safeguard_prevents_over_compression() {
    let original = generate_rust_file(100);
    let over_compressed = "fn main() {}";
    let result = safeguard_ratio(&original, over_compressed);

    let orig_tokens = count_tokens(&original);
    let result_tokens = count_tokens(&result);
    let ratio = result_tokens as f64 / orig_tokens as f64;

    eprintln!(
        "\n  [safeguard] Original: {} tokens, Over-compressed: {} tokens, Result: {} tokens, Ratio: {:.2}",
        orig_tokens,
        count_tokens(over_compressed),
        result_tokens,
        ratio
    );
    assert!(
        ratio > 0.05,
        "Safeguard should prevent extreme compression on small outputs, ratio: {ratio:.2}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 3.5: RRF EVICTION SCORING
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn bench_rrf_eviction_handles_single_entry() {
    use std::time::Instant;

    let now = Instant::now();
    let key = "solo.rs".to_string();
    let entry = sobre::core::cache::CacheEntry::new("single", "h".to_string(), 1, 50, None);

    let refs: Vec<(&String, &sobre::core::cache::CacheEntry)> = vec![(&key, &entry)];
    let scores = sobre::core::cache::eviction_scores_rrf(&refs, now);
    assert_eq!(scores.len(), 1);
    assert!(
        scores[0].1 > 0.0,
        "single entry must have positive RRF score"
    );
}

#[test]
fn bench_rrf_eviction_empty() {
    use std::time::Instant;
    let now = Instant::now();
    let scores = sobre::core::cache::eviction_scores_rrf(&[], now);
    assert!(scores.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 5: THINKING TOKEN BENCHMARKS — CRP/TDD Instruction Effectiveness
// ═══════════════════════════════════════════════════════════════════════════

// SECTION 6: PERFORMANCE BENCHMARKS removed — machine-dependent timing
// thresholds are unsuitable for OSS CI.

// ═══════════════════════════════════════════════════════════════════════════
// SECTION 7: REGRESSION GUARDS — Ensure no performance degradation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn guard_no_inflation_aggressive() {
    for size in [50, 100, 200, 500] {
        let content = generate_rust_file(size);
        let compressed = aggressive_compress(&content, Some("rs"));
        let orig = count_tokens(&content);
        let comp = count_tokens(&compressed);
        assert!(
            comp <= orig,
            "Aggressive should never inflate! Size {size}: {orig} → {comp}"
        );
    }
    eprintln!("\n  [guard] Aggressive mode never inflates output ✓");
}

#[test]
fn guard_no_inflation_lightweight() {
    for size in [50, 100, 200, 500] {
        let content = generate_rust_file(size);
        let cleaned = lightweight_cleanup(&content);
        let orig = count_tokens(&content);
        let comp = count_tokens(&cleaned);
        assert!(
            comp <= orig,
            "Lightweight should never inflate! Size {size}: {orig} → {comp}"
        );
    }
    eprintln!("\n  [guard] Lightweight cleanup never inflates output ✓");
}

#[test]
fn guard_pattern_never_inflates() {
    let commands: Vec<(&str, String)> = vec![
        ("git log -p -5", generate_git_log_patch(5)),
        ("git status", generate_git_status()),
        ("cargo build", generate_cargo_build_success()),
        ("docker ps", generate_docker_ps(5)),
        ("npm install", generate_npm_install(10)),
    ];

    for (cmd, output) in &commands {
        if let Some(compressed) = sobre::core::patterns::compress_output(cmd, output) {
            let orig = count_tokens(output);
            let comp = count_tokens(&compressed);
            assert!(
                comp <= orig,
                "Pattern '{cmd}' should never inflate: {orig} → {comp}"
            );
        }
    }
    eprintln!("\n  [guard] Pattern compression never inflates output ✓");
}

#[test]
fn guard_tool_descriptions_not_empty() {
    let descs = sobre::server::tool_descriptions_for_test();
    for (name, desc) in &descs {
        assert!(
            !desc.is_empty(),
            "Tool '{name}' must have a non-empty description"
        );
        assert!(
            desc.len() > 10,
            "Tool '{name}' description too short ({} chars): '{desc}'",
            desc.len()
        );
    }
    eprintln!(
        "\n  [guard] All {} tools have meaningful descriptions ✓",
        descs.len()
    );
}

#[test]
fn guard_essential_instructions_present() {
    let instr = instructions_at_off();

    let required = vec![
        "native tools keep working",
        "sobre_expand",
        "sobre_patch",
        "sobre_read",
        "sobre_shell",
        "sobre_search",
        "sobre_tree",
        "sobre_compose",
        "sobre_session",
    ];

    for keyword in &required {
        assert!(
            instr.contains(keyword),
            "System instructions must contain '{keyword}'"
        );
    }
    eprintln!(
        "\n  [guard] All {} essential instruction keywords present ✓",
        required.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// DATA GENERATORS
// ═══════════════════════════════════════════════════════════════════════════

fn print_compression_report(title: &str, scenarios: &[(&str, String, f64)]) {
    let mut total_orig = 0usize;
    let mut total_comp = 0usize;

    eprintln!("\n{}", "=".repeat(70));
    eprintln!("  {title}");
    eprintln!("{}", "=".repeat(70));
    eprintln!(
        "  {:<35} {:>8} {:>8} {:>7}",
        "Command", "Original", "Compr.", "Saved%"
    );
    eprintln!("  {}", "-".repeat(60));

    for (cmd, output, min_savings) in scenarios {
        let (orig, comp, pct) = measure_pattern(cmd, output);
        total_orig += orig;
        total_comp += comp;
        let status = if pct >= *min_savings { "✓" } else { "✗" };
        eprintln!("  {cmd:<35} {orig:>8} {comp:>8} {pct:>6.1}% {status}");
        assert!(
            pct >= *min_savings,
            "'{cmd}' should save ≥{min_savings}%, got {pct:.1}%"
        );
    }

    let total_pct = compression_ratio(total_orig, total_comp);
    eprintln!("  {}", "-".repeat(60));
    eprintln!(
        "  {:<35} {:>8} {:>8} {:>6.1}%",
        "TOTAL", total_orig, total_comp, total_pct
    );
    eprintln!("{}", "=".repeat(70));
}

fn generate_rust_file(lines: usize) -> String {
    let mut s = String::new();
    s.push_str("use std::collections::HashMap;\nuse std::sync::Arc;\n\n");
    s.push_str("/// Main application struct that handles all processing\n");
    s.push_str("pub struct Application {\n    config: Config,\n    state: Arc<State>,\n}\n\n");

    let mut line = 9;
    while line < lines {
        let fn_idx = line / 15;
        s.push_str(&format!(
            "/// Process item {fn_idx} with validation and error handling\n\
             pub fn process_item_{fn_idx}(input: &str, config: &Config) -> Result<Output, Error> {{\n\
             \tlet validated = validate_input(input)?;\n\
             \tlet transformed = transform_data(&validated, config);\n\
             \t// Apply business rules\n\
             \tif transformed.is_empty() {{\n\
             \t\treturn Err(Error::EmptyInput);\n\
             \t}}\n\
             \tlet result = compute_result(&transformed)?;\n\
             \tOk(Output::new(result))\n\
             }}\n\n"
        ));
        line += 13;
    }
    s
}

fn generate_python_file(lines: usize) -> String {
    let mut s = String::new();
    s.push_str("import os\nimport sys\nfrom typing import Dict, List, Optional\n\n");
    s.push_str("# Main service class for data processing\n");
    s.push_str("class DataService:\n    \"\"\"Service for processing data pipelines.\"\"\"\n\n");

    let mut line = 8;
    while line < lines {
        let fn_idx = line / 12;
        s.push_str(&format!(
            "    def process_batch_{fn_idx}(self, items: List[Dict], config: Optional[Dict] = None) -> List[Dict]:\n\
             \t\t\"\"\"Process batch {fn_idx} with validation.\"\"\"\n\
             \t\t# Validate inputs\n\
             \t\tvalidated = [self._validate(item) for item in items]\n\
             \t\tresults = []\n\
             \t\tfor item in validated:\n\
             \t\t\tif item is not None:\n\
             \t\t\t\tresults.append(self._transform(item))\n\
             \t\treturn results\n\n"
        ));
        line += 12;
    }
    s
}

fn generate_typescript_file(lines: usize) -> String {
    let mut s = String::new();
    s.push_str(
        "import { Request, Response } from 'express';\nimport { Database } from './db';\n\n",
    );
    s.push_str("// API controller for user management\n");
    s.push_str("export interface UserPayload {\n  name: string;\n  email: string;\n  role: 'admin' | 'user';\n}\n\n");

    let mut line = 9;
    while line < lines {
        let fn_idx = line / 14;
        s.push_str(&format!(
            "/**\n * Handle user request {fn_idx}\n */\n\
             export async function handleUser{fn_idx}(req: Request, res: Response): Promise<void> {{\n\
             \tconst {{ name, email }} = req.body as UserPayload;\n\
             \t// Validate the payload\n\
             \tif (!name || !email) {{\n\
             \t\tres.status(400).json({{ error: 'Missing fields' }});\n\
             \t\treturn;\n\
             \t}}\n\
             \tconst result = await Database.upsert({{ name, email }});\n\
             \tres.json({{ success: true, data: result }});\n\
             }}\n\n"
        ));
        line += 14;
    }
    s
}

fn generate_git_status() -> String {
    let mut s = String::from(
        "On branch feature/new-auth\nYour branch is ahead of 'origin/feature/new-auth' by 3 commits.\n  (use \"git push\" to publish your local commits)\n\nChanges to be committed:\n  (use \"git restore --staged <file>...\" to unstage)\n",
    );
    for i in 0..8 {
        s.push_str(&format!("\tmodified:   src/auth/handler_{i}.rs\n"));
    }
    s.push_str("\tnew file:   src/auth/middleware.rs\n\trenamed:    src/old.rs -> src/new.rs\n");
    s.push_str("\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n");
    for i in 0..5 {
        s.push_str(&format!("\tmodified:   tests/test_{i}.rs\n"));
    }
    s.push_str("\nUntracked files:\n  (use \"git add <file>...\" to include in what will be committed)\n\ttmp/debug.log\n\tnotes.md\n");
    s
}

fn generate_git_diff(files: usize) -> String {
    let mut s = String::new();
    for i in 0..files {
        s.push_str(&format!(
            "diff --git a/src/module_{i}.rs b/src/module_{i}.rs\n\
             index abc1234..def5678 100644\n\
             --- a/src/module_{i}.rs\n\
             +++ b/src/module_{i}.rs\n\
             @@ -10,8 +10,12 @@ fn existing_function() {{\n\
              \tlet x = 1;\n\
              \tlet y = 2;\n\
             -\tlet old_value = compute_old(x, y);\n\
             -\treturn old_value;\n\
             +\tlet new_value = compute_new(x, y);\n\
             +\t// Added validation\n\
             +\tif new_value > 0 {{\n\
             +\t\treturn Ok(new_value);\n\
             +\t}}\n\
             +\tErr(Error::Invalid)\n\
              }}\n\n"
        ));
    }
    s
}

fn generate_git_commit_with_hooks(hook_count: usize) -> String {
    let mut s = String::new();
    for i in 0..hook_count {
        s.push_str(&format!("check-{i:02}...........................passed\n"));
    }
    s.push_str("[feature/auth abc1234] feat: implement JWT authentication\n");
    s.push_str(" 8 files changed, 234 insertions(+), 45 deletions(-)\n");
    s.push_str(" create mode 100644 src/auth/jwt.rs\n");
    s.push_str(" create mode 100644 src/auth/middleware.rs\n");
    s
}

fn generate_git_push() -> String {
    "Enumerating objects: 15, done.\n\
     Counting objects: 100% (15/15), done.\n\
     Delta compression using up to 10 threads\n\
     Compressing objects: 100% (8/8), done.\n\
     Writing objects: 100% (9/9), 2.34 KiB | 2.34 MiB/s, done.\n\
     Total 9 (delta 5), reused 0 (delta 0), pack-reused 0\n\
     remote: Resolving deltas: 100% (5/5), completed with 3 local objects.\n\
     To github.com:user/repo.git\n\
        abc1234..def5678  main -> main\n"
        .to_string()
}

fn generate_cargo_build_success() -> String {
    "   Compiling serde v1.0.197\n\
     Compiling serde_json v1.0.115\n\
     Compiling tokio v1.37.0\n\
     Compiling my-app v0.1.0 (/home/user/project)\n\
     Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.34s\n"
        .to_string()
}

fn generate_cargo_build_with_warnings(count: usize) -> String {
    let mut s = String::from("   Compiling my-app v0.1.0 (/home/user/project)\n");
    for i in 0..count {
        s.push_str(&format!(
            "warning: unused variable: `temp_{i}`\n\
              --> src/handlers/mod.rs:{}:9\n\
               |\n\
             {} |     let temp_{i} = calculate();\n\
               |         ^^^^^^^ help: if this is intentional, prefix it with an underscore: `_temp_{i}`\n\
               |\n",
            100 + i * 5,
            100 + i * 5
        ));
    }
    s.push_str(&format!(
        "warning: `my-app` (bin \"my-app\") generated {count} warnings\n\
         Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.45s\n"
    ));
    s
}

fn generate_cargo_test(pass: usize, fail: usize) -> String {
    let mut s = String::from("\nrunning tests\n");
    for i in 0..pass {
        s.push_str(&format!("test tests::test_case_{i} ... ok\n"));
    }
    for i in 0..fail {
        s.push_str(&format!("test tests::test_failing_{i} ... FAILED\n"));
    }
    s.push_str(&format!(
        "\ntest result: {}. {} passed; {} failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.23s\n",
        if fail > 0 { "FAILED" } else { "ok" },
        pass,
        fail
    ));
    s
}

fn generate_cargo_clippy(count: usize) -> String {
    let mut s = String::from("    Checking my-app v0.1.0\n");
    for i in 0..count {
        s.push_str(&format!(
            "warning: redundant clone\n\
              --> src/handler_{i}.rs:{}:14\n\
               |\n\
             {} |     data.clone()\n\
               |          ^^^^^^^^ help: remove this\n\
               = note: `#[warn(clippy::redundant_clone)]` on by default\n\n",
            20 + i * 3,
            20 + i * 3
        ));
    }
    s.push_str(&format!(
        "warning: `my-app` (bin \"my-app\") generated {count} warnings\n"
    ));
    s
}

fn generate_docker_ps(count: usize) -> String {
    let mut s = String::from(
        "CONTAINER ID   IMAGE                    COMMAND                  CREATED        STATUS        PORTS                    NAMES\n",
    );
    for i in 0..count {
        s.push_str(&format!(
            "abc{i:04}def   nginx:1.25-alpine        \"nginx -g 'daemon of…\"   {} hours ago   Up {} hours   0.0.0.0:80{}->80/tcp     web-{i}\n",
            2 + i,
            2 + i,
            80 + i
        ));
    }
    s
}

fn generate_docker_images(count: usize) -> String {
    let mut s = String::from("REPOSITORY          TAG       IMAGE ID       CREATED        SIZE\n");
    for i in 0..count {
        s.push_str(&format!(
            "myapp-{i}            latest    sha256:abc{i:04}   {} days ago    {}MB\n",
            1 + i,
            50 + i * 30
        ));
    }
    s
}

fn generate_docker_build(steps: usize) -> String {
    let mut s = String::new();
    for i in 1..=steps {
        s.push_str(&format!(
            "[{i}/{steps}] RUN apt-get install -y package-{i} && rm -rf /var/lib/apt/lists/*\n"
        ));
        s.push_str(&format!(
            " ---> Running in abc{i:04}def\n\
              ---> sha256:abc{i:05}def\n"
        ));
    }
    s.push_str("Successfully built sha256:final123456\nSuccessfully tagged myapp:latest\n");
    s
}

fn generate_npm_install(count: usize) -> String {
    let mut s = String::new();
    for i in 0..count {
        s.push_str(&format!(
            "npm warn deprecated package-{i}@1.0.{i}: Use package-{i}@2 instead\n"
        ));
    }
    s.push_str(&format!(
        "\nadded {} packages, and audited {} packages in 5s\n\n\
         {} packages are looking for funding\n  run `npm fund` for details\n\n\
         found 0 vulnerabilities\n",
        count + 50,
        count + 100,
        count / 3
    ));
    s
}

fn generate_npm_test_jest() -> String {
    "PASS src/components/Button.test.tsx\n\
     PASS src/components/Form.test.tsx\n\
     PASS src/utils/api.test.ts\n\
     FAIL src/hooks/useAuth.test.ts\n\
       ● useAuth › should handle token refresh\n\
     \n\
     Test Suites: 1 failed, 3 passed, 4 total\n\
     Tests:       1 failed, 15 passed, 16 total\n\
     Snapshots:   0 total\n\
     Time:        3.456 s\n\
     Ran all test suites.\n"
        .to_string()
}

fn generate_npm_ls(count: usize) -> String {
    let mut s = String::from("my-app@1.0.0 /home/user/project\n");
    for i in 0..count {
        s.push_str(&format!("├── package-{i}@{}.{}.0\n", 1 + i % 5, i % 10));
    }
    s.push_str("└── last-package@1.0.0\n");
    s
}

fn generate_pip_install(count: usize) -> String {
    let mut s = String::new();
    for i in 0..count {
        s.push_str(&format!(
            "Collecting package-{i}>=1.0\n\
             \tDownloading package_{i}-1.{i}.0-cp311-cp311-manylinux_2_17_x86_64.whl ({}kB)\n",
            500 + i * 100
        ));
    }
    s.push_str(&format!(
        "Successfully installed {}\n",
        (0..count)
            .map(|i| format!("package-{i}-1.{i}.0"))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    s
}

fn generate_pip_list(count: usize) -> String {
    let mut s = String::from("Package          Version\n---------------- -------\n");
    for i in 0..count {
        s.push_str(&format!("package-{i:<8} {}.{}.0\n", 1 + i % 5, i % 10));
    }
    s
}

fn generate_kubectl_pods(count: usize) -> String {
    let mut s =
        String::from("NAME                              READY   STATUS    RESTARTS   AGE\n");
    for i in 0..count {
        let status = match i % 5 {
            0..=2 => "Running",
            3 => "Pending",
            _ => "CrashLoopBackOff",
        };
        s.push_str(&format!(
            "app-{i}-deployment-abc{i:04}def   1/1     {:<20} {}          {}d\n",
            status,
            i % 3,
            1 + i
        ));
    }
    s
}

fn generate_kubectl_pods_all(count: usize) -> String {
    let mut s = String::from(
        "NAMESPACE     NAME                              READY   STATUS    RESTARTS   AGE\n",
    );
    let namespaces = ["default", "kube-system", "monitoring", "app-prod"];
    for i in 0..count {
        let ns = namespaces[i % namespaces.len()];
        s.push_str(&format!(
            "{:<13} pod-{i}-abc{i:04}def                1/1     Running   0          {}d\n",
            ns,
            1 + i
        ));
    }
    s
}

fn generate_git_log_patch(n: usize) -> String {
    let mut output = String::new();
    for i in 0..n {
        let msg = format!("implement feature number {i} with improvements");
        output.push_str(&format!(
            "commit {i:07}abc1234567890abcdef1234567890\n\
             Author: Developer <dev@example.com>\n\
             Date:   Mon Mar {d} 10:{h:02}:00 2026 +0100\n\
             \n\
             {ty}: {msg}\n\
             \n\
             diff --git a/src/module_{i}.rs b/src/module_{i}.rs\n\
             index abc1234..def5678 100644\n\
             --- a/src/module_{i}.rs\n\
             +++ b/src/module_{i}.rs\n\
             @@ -1,{ctx} +1,{ctx2} @@\n",
            d = 10 + (i % 20),
            h = i % 24,
            ty = ["feat", "fix", "refactor", "docs", "test"][i % 5],
            ctx = 10 + i,
            ctx2 = 12 + i,
        ));
        for j in 0..(5 + i % 10) {
            output.push_str(&format!(" fn existing_function_{j}() {{}}\n"));
        }
        for j in 0..(3 + i % 5) {
            output.push_str(&format!("+fn new_function_{i}_{j}() {{ todo!() }}\n"));
        }
        for j in 0..(2 + i % 3) {
            output.push_str(&format!(
                "-fn old_function_{i}_{j}() {{ unimplemented!() }}\n"
            ));
        }
        output.push('\n');
    }
    output
}

fn generate_git_log_standard(n: usize) -> String {
    let mut output = String::new();
    for i in 0..n {
        let msg = format!("update component {i} with better error handling");
        output.push_str(&format!(
            "commit {i:07}abc1234567890abcdef1234567890\n\
             Author: Developer <dev@example.com>\n\
             Date:   Mon Mar {d} 10:{h:02}:00 2026 +0100\n\
             \n\
             {ty}: {msg}\n\
             \n\
             This is a longer commit message that describes the changes made.\n\
             It spans multiple lines to be more realistic.\n\
             \n",
            d = 10 + (i % 20),
            h = i % 24,
            ty = ["feat", "fix", "refactor", "docs", "test"][i % 5],
        ));
    }
    output
}

fn generate_git_log_stat(n: usize) -> String {
    let mut output = String::new();
    for i in 0..n {
        let msg = format!("update module {i}");
        output.push_str(&format!(
            "commit {i:07}abc1234567890abcdef1234567890\n\
             Author: Developer <dev@example.com>\n\
             Date:   Mon Mar {d} 10:{h:02}:00 2026 +0100\n\
             \n\
             {ty}: {msg}\n\
             \n",
            d = 10 + (i % 20),
            h = i % 24,
            ty = ["feat", "fix", "refactor", "docs", "test"][i % 5],
        ));
        for j in 0..(2 + i % 4) {
            let plus = 3 + j;
            let minus = 1 + j;
            let bar: String = "+".repeat(plus) + &"-".repeat(minus);
            output.push_str(&format!(
                " src/mod_{i}_{j}.rs | {total} {bar}\n",
                total = plus + minus,
            ));
        }
        let total_files = 2 + i % 4;
        output.push_str(&format!(
            " {total_files} files changed, {} insertions(+), {} deletions(-)\n\n",
            10 + i * 2,
            5 + i,
        ));
    }
    output
}

// ---------------------------------------------------------------------------
// v3.4.1 Optimization benchmarks: Token overhead + Latency
// ---------------------------------------------------------------------------

/// Points `SOBRE_DATA_DIR` at a fresh temp dir for the guard's lifetime and
/// restores the previous value on drop. Callers hold `test_env_lock()`.
struct TempConfig {
    _dir: tempfile::TempDir,
    prev_data_dir: Option<String>,
}

impl TempConfig {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let prev_data_dir = std::env::var("SOBRE_DATA_DIR").ok();
        // SAFETY: serialized by `test_env_lock()`.
        unsafe { std::env::set_var("SOBRE_DATA_DIR", dir.path()) };
        Self {
            _dir: dir,
            prev_data_dir,
        }
    }
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        match self.prev_data_dir.take() {
            // SAFETY: serialized by `test_env_lock()`.
            Some(v) => unsafe { std::env::set_var("SOBRE_DATA_DIR", v) },
            // SAFETY: serialized by `test_env_lock()`.
            None => unsafe { std::env::remove_var("SOBRE_DATA_DIR") },
        }
    }
}

#[test]
fn bench_minimal_overhead_suppresses_all_meta_strings() {
    // The data dir is process-global; serialize against the other instruction
    // benchmarks.
    let _lock = sobre::core::data_dir::test_env_lock();
    let _config = TempConfig::new();

    let instructions = instructions_at_off();
    let instr_tokens = count_tokens(&instructions);

    eprintln!("\n{}", "=".repeat(70));
    eprintln!("  MINIMAL OVERHEAD: Instructions Token Count");
    eprintln!("{}", "=".repeat(70));
    eprintln!("  Instructions (minimal):  {instr_tokens:>5} tokens");
    eprintln!("{}", "=".repeat(70));

    assert!(
        !instructions.contains("--- ACTIVE SESSION"),
        "minimal_overhead should suppress session block"
    );
}

#[test]
fn bench_hash_fast_vs_full_correctness() {
    use sobre::core::hasher::hash_str;
    use sobre::server::helpers::hash_fast;

    let small = "a".repeat(1000);
    assert_eq!(
        hash_str(&small),
        hash_fast(&small),
        "hash_fast must match hash_str for small strings"
    );

    let exactly_16k = "x".repeat(16 * 1024);
    assert_eq!(
        hash_str(&exactly_16k),
        hash_fast(&exactly_16k),
        "hash_fast must match hash_str at the 16KB boundary"
    );

    let large = "b".repeat(100_000);
    let fast_hash = hash_fast(&large);
    assert_eq!(
        fast_hash.len(),
        64,
        "hash_fast should produce valid BLAKE3 hex"
    );
    assert_ne!(
        fast_hash,
        hash_fast(&"c".repeat(100_000)),
        "different large strings should produce different hashes"
    );

    let mut middle_changed = large.clone().into_bytes();
    middle_changed[50_000] = b'd';
    let middle_changed = String::from_utf8(middle_changed).expect("ASCII test data");
    assert_eq!(
        fast_hash,
        hash_fast(&middle_changed),
        "large hash_fast fingerprints only prefix, suffix, and length"
    );

    let mut suffix_changed = large.clone().into_bytes();
    suffix_changed[99_999] = b'd';
    let suffix_changed = String::from_utf8(suffix_changed).expect("ASCII test data");
    assert_ne!(
        fast_hash,
        hash_fast(&suffix_changed),
        "large hash_fast fingerprints the suffix"
    );
}

#[test]
fn bench_count_tokens_cache_effectiveness() {
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(200);

    let start_cold = std::time::Instant::now();
    let t1 = count_tokens(&text);
    let cold_us = start_cold.elapsed().as_micros();

    let start_cached = std::time::Instant::now();
    let t2 = count_tokens(&text);
    let cached_us = start_cached.elapsed().as_micros();

    assert_eq!(t1, t2, "cached result must match");

    let slightly_different = format!("{text}X");
    let start_miss = std::time::Instant::now();
    let t3 = count_tokens(&slightly_different);
    let miss_us = start_miss.elapsed().as_micros();

    eprintln!("\n{}", "=".repeat(70));
    eprintln!("  COUNT_TOKENS CACHE BENCHMARK (~9KB input)");
    eprintln!("{}", "=".repeat(70));
    eprintln!("  Cold (first call):  {cold_us:>6} us  -> {t1} tokens");
    eprintln!("  Cached (same text): {cached_us:>6} us  -> {t2} tokens");
    eprintln!("  Miss (diff text):   {miss_us:>6} us  -> {t3} tokens");
    if cold_us > 0 {
        let speedup = cold_us as f64 / cached_us.max(1) as f64;
        eprintln!("  Cache speedup:      {speedup:>6.0}x");
    }
    eprintln!("{}", "=".repeat(70));

    assert!(
        cached_us <= cold_us + 5,
        "cached call should be faster than cold call"
    );
}

#[test]
fn bench_session_prepare_save_is_cpu_only() {
    use sobre::core::session::SessionState;

    let mut session = SessionState::new();
    session.set_task("benchmark task for save split", None);
    for _ in 0..20 {
        session.record_tool_call(100);
        session.stats.unsaved_changes = 0;
    }

    let start = std::time::Instant::now();
    for _ in 0..100 {
        session.stats.unsaved_changes = 5;
        if let Ok(prepared) = session.prepare_save() {
            std::mem::drop(prepared);
        }
    }
    let prepare_us = start.elapsed().as_micros();

    eprintln!("\n{}", "=".repeat(70));
    eprintln!("  SESSION PREPARE_SAVE BENCHMARK (100 iters)");
    eprintln!("{}", "=".repeat(70));
    eprintln!("  100x prepare_save (serialize only): {prepare_us:>6} us");
    eprintln!(
        "  Per call:                           {:>6} us",
        prepare_us / 100
    );
    eprintln!("  Note: write_to_disk() I/O runs in background thread.");
    eprintln!("{}", "=".repeat(70));

    assert!(
        prepare_us / 100 < 5000,
        "prepare_save should take <5ms per call, got {} us",
        prepare_us / 100
    );

    assert_eq!(
        session.stats.unsaved_changes, 0,
        "prepare_save must reset unsaved_changes"
    );
}

#[test]
fn bench_session_save_semantics() {
    use sobre::core::session::SessionState;

    let mut session = SessionState::new();
    for _ in 0..6 {
        session.increment();
    }
    assert!(session.should_save(), "should_save after 6 mutations");

    let prepared = session.prepare_save().expect("prepare_save");
    assert_eq!(
        session.stats.unsaved_changes, 0,
        "prepare_save must reset counter immediately (for async path)"
    );
    assert!(
        !session.should_save(),
        "should_save must be false after prepare_save"
    );

    let write_result = prepared.write_to_disk();
    assert!(
        write_result.is_ok(),
        "write_to_disk to valid dir should succeed"
    );

    for _ in 0..6 {
        session.increment();
    }
    assert!(session.should_save());

    let result = session.save();
    assert!(result.is_ok(), "synchronous save should succeed");
    assert_eq!(
        session.stats.unsaved_changes, 0,
        "save must reset counter on success"
    );
    assert!(
        !session.should_save(),
        "should_save must be false after save"
    );
}
