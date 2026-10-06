# Appendix — MCP Tools (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/server/registry.rs` and the tool definitions it registers.

lean-ctx registers **17 MCP tools** (granular profile). Each entry below lists the tool name, what it does, and its parameters (`*` marks required).

## `ctx_callgraph`

Callers/callees for one symbol (function call edges, not const/var refs).
action=callers|callees symbol='fn' → every call site with file:line.
action=trace from→to finds the path between two symbols (depth=N).
For end-to-end flow understanding use ctx_compose FIRST.

Parameters: `action`*, `depth`, `file`, `from`, `symbol`, `to`

## `ctx_checkpoint`

Local shadow git history of the agent's changes — separate from the user's .git.
WORKFLOW: snapshot before+after changes to capture exactly what was modified.
Actions: snapshot (record current state), log (list checkpoints with SHAs),
diff from=... to=... (compare checkpoints), restore ref=... (revert files).
ANTIPATTERN: Never touches the user's repository — completely isolated shadow history.

Parameters: `action`, `from`, `limit`, `message`, `path`, `ref`, `to`

## `ctx_compose`

PRIMARY TOOL — call FIRST for understanding code (before editing/debugging/'how does X work').
Omit task at session start: returns the project overview — structure, markers, last task, recent decisions, facts from previous sessions.
With a task: returns ranked files with relevant symbol source inline grouped by file.
Combines BM25 lexical+semantic+associative retrieval+submodular optimization.
ANTIPATTERN: Do NOT chain search→read→symbol — one compose replaces the whole chain.
ANTIPATTERN: Do NOT Read files whose source compose already returned — it IS the source.
WORKFLOW: Fire parallel ctx_read or ctx_compose for different areas.

Parameters: `path`, `task`

## `ctx_expand`

Retrieve archived tool output: see [Archived:ID] → ctx_expand id=ID (zero-loss, original preserved). head/tail/search filter lines; action=list|search_all browses/queries archives.
ANTIPATTERN: not for project files — use ctx_read or ctx_compose.

Parameters: `action`, `end_line`, `head`, `id`, `json_keys`, `json_path`, `query`, `search`, `session_id`, `start_line`, `tail`

## `ctx_glob`

Find files by glob pattern (respects .gitignore; multi-root via paths).
For file CONTENT search use ctx_search.

Parameters: `ignore_gitignore`, `max_results`, `path`, `paths`, `pattern`*

## `ctx_graph`

File-level dependency graph queries.
action=symbol path="file.rs::fnName" returns the DEFINITION (not usages — use ctx_search for references). neighbors=imports±direction, impact=reverse-dep blast radius, path from→to=dependency chain, diff since=HEAD~1=git change impact, diagram kind=deps|calls (Mermaid),
cycles=dependency cycles, entrypoints=files with no dependents.
For understanding code use ctx_compose FIRST.

Parameters: `action`*, `depth`, `format`, `kind`, `path`, `project_root`, `since`, `to`

## `ctx_impact`

Change impact analysis — assess blast radius before refactoring.
action=analyze path="file.rs" maps downstream dependents; action=diff compares git refs;
action=chain traces from→to dependency paths. depth controls traversal (default 5).

Parameters: `action`, `depth`, `format`, `path`, `root`

## `ctx_knowledge`

Persistent memory across sessions — remember decisions, patterns, and facts for recall.
WORKFLOW: save after completing significant tasks; recall at session start.
action=remember value='Y' saves a fact (key optional — derived from value; content= is an accepted alias).
action=recall query='X' retrieves it (bare recall lists recent facts). action=status shows all categories.
action=consolidate imports latest session if present, runs lifecycle, then frees 25% facts/history/procedures capacity.
action=gotcha trigger='X' resolution='Y' for known pitfalls.
mode=semantic|exact for recall. category groups related facts.

Parameters: `action`*, `as_of`, `category`, `confidence`, `content`, `dry_run`, `examples`, `format`, `key`, `limit`, `merge`, `mode`, `path`, `pattern_type`, `query`, `resolution`, `severity`, `store`, `trigger`, `value`

## `ctx_outline`

WORKFLOW: call BEFORE ctx_read to map code structure (a syntax-aware table of contents).
Accepts a FILE or a DIRECTORY (folder surface — per-file symbols). Symbols come from
tree-sitter (27 languages, real line spans); a conservative regex fallback covers the rest.
kind=fn|struct|class|trait|enum|impl|all filters by kind; match=<substr> filters by name
(case-insensitive); format=json emits deterministic JSON labelling the backend per file.
ANTIPATTERN: NOT for file content (use ctx_read) or deep understanding (use ctx_compose).

Parameters: `format`, `kind`, `match`, `path`*

## `ctx_patch`

Safe file edit. Anchored ops use line+hash from ctx_read(mode="anchored"); CONFLICT means re-read. replace_unique(path,old_text,new_text) is a no-read, exact unique replacement. replace_symbol/create/replace_all and cross-file ops[] (incl. replace_unique) supported.

Parameters: `dry_run`, `end_hash`, `end_line`, `find`, `hash`, `line`, `name`, `new_text`, `old_text`, `op`, `ops`, `path`, `replace`, `start_hash`, `start_line`

## `ctx_read`

Read source files. mode recommended — choose by intent (see `mode` below); defaults to auto when omitted.
To UNDERSTAND code run ctx_compose FIRST; ctx_read after it identified files.
anchored → edit by reference via ctx_patch (no exact-recall).

Parameters: `aggressiveness`, `fresh`, `limit`, `mode`, `offset`, `path`, `paths`, `protect`, `raw`, `start_line`

## `ctx_refactor`

Symbol navigation and edits: rename, references, definition, implementations via the language server; symbols_overview and symbol body edits work without one.
WORKFLOW: use action=references first to find usages before refactoring.
ANTIPATTERN: not for symbol discovery — use ctx_search action=symbol or ctx_compose.
Symbol edits (replace_symbol_body, insert_before_symbol, insert_after_symbol) resolve the target by name_path.

Parameters: `action`*, `column`, `end_line`, `expected_hash`, `file`, `line`, `name_path`, `new_name`, `new_text`, `path`, `scope`, `text`

## `ctx_review`

Automated code review with impact analysis, caller tracking, and test discovery.
Actions: review (single file), diff-review (from git diff text),
checklist (structured review questions). depth=N (default 3).
WORKFLOW: run tests first, then use review for structured analysis.
ANTIPATTERN: not a substitute for actual test execution.

Parameters: `action`*, `depth`, `path`

## `ctx_search`

Search code: regex(pattern, default) | semantic(query) | symbol(name|handle) | reindex | find_related(file_path,line). anchored=true enables ctx_patch refs; queries batches regex searches. Run ctx_compose FIRST.

Parameters: `action`, `anchored`, `exclude`, `exclude_pattern`, `file`, `file_path`, `handle`, `include`, `kind`, `line`, `max_results`, `mode`, `name`, `path`, `paths`, `pattern`, `queries`, `query`, `top_k`

## `ctx_session`

Session memory. save at session end, load at start, status = snapshot;
task|finding|decision record progress (value=text).
ANTIPATTERN: permanent project knowledge → ctx_knowledge.

Parameters: `action`*, `session_id`, `value`

## `ctx_shell`

WORKFLOW: preferred — auto-compresses output (build/test/log).
raw=true for verbatim output; inline=true for moderately-sized verbatim output.
[exit:N] on errors (lossless).
POLICY: every command is allowed except constructs that execute something the command text does not name (eval/exec/source, $() at command position, interpreter -c, find -exec, LD_PRELOAD=, …).
A [BLOCKED] command is permanent — do not retry it.
Multi-line scripts: write them to a file, then run the file.

Parameters: `background_action`, `command`, `cwd`, `env`, `inline`, `job_id`, `raw`, `run_in_background`, `timeout_ms`

## `ctx_tree`

Directory tree with file counts per directory. depth=N (default 3);
show_hidden for dotfiles; paths for multi-root.
respect_gitignore filters ignored files (default true).
WORKFLOW: lightweight orientation before ctx_compose.

Parameters: `depth`, `path`, `paths`, `respect_gitignore`, `show_hidden`

