# Appendix — MCP Tools (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/server/registry.rs` and the tool definitions it registers.

sobre registers **15 MCP tools**. Each entry below lists the tool name, what it does, and its parameters (`*` marks required).

## `sobre_checkpoint`

Local shadow git history of the agent's changes — separate from the user's .git.
WORKFLOW: snapshot before+after changes to capture exactly what was modified.
Actions: snapshot (record current state), log (list checkpoints with SHAs),
diff from=... to=... (compare checkpoints), restore ref=... (revert files).
ANTIPATTERN: Never touches the user's repository — completely isolated shadow history.

Parameters: `action`, `from`, `limit`, `message`, `path`, `ref`, `to`

## `sobre_compose`

PRIMARY TOOL — call FIRST for understanding code (before editing/debugging/'how does X work').
Omit task at session start: returns the project overview — structure, markers, last task, recent decisions, facts from previous sessions.
With a task: returns ranked files with relevant symbol source inline grouped by file.
Combines BM25 lexical+semantic+associative retrieval+submodular optimization.
ANTIPATTERN: Do NOT chain search→read→symbol — one compose replaces the whole chain.
ANTIPATTERN: Do NOT Read files whose source compose already returned — it IS the source.
WORKFLOW: Fire parallel sobre_read or sobre_compose for different areas.

Parameters: `path`, `task`

## `sobre_expand`

Full output that compression elided. A compressed output's footer names its id → sobre_expand(id=ID). Optional start_line/end_line (1-indexed, inclusive) or search (substring) return a slice. Saved outputs expire after 24h.

Parameters: `end_line`, `id`*, `search`, `start_line`

## `sobre_glob`

Find files by glob pattern (respects .gitignore; multi-root via paths).
For file CONTENT search use sobre_search.

Parameters: `ignore_gitignore`, `max_results`, `path`, `paths`, `pattern`*

## `sobre_graph`

Code graph: file dependencies and function calls.
Files: neighbors=imports±direction, impact path=file or type → files that depend on it (depth, default 5), changes → files affected by uncommitted changes, path path→to=dependency chain, related.
Calls: callers|callees symbol='fn' → every call site with file:line (depth≤5, path scopes to a file); trace symbol→to=call chain.
symbol symbol='fn' (or path="file.rs::fn") returns the definition. diagram kind=deps|calls (Mermaid).
For understanding code use sobre_compose FIRST.

Parameters: `action`*, `depth`, `format`, `kind`, `path`, `project_root`, `symbol`, `to`

## `sobre_knowledge`

Persistent memory across sessions — remember decisions, patterns, and facts for recall.
WORKFLOW: save after completing significant tasks; recall at session start.
action=remember value='Y' saves a fact (key optional — derived from value; content= is an accepted alias).
action=recall query='X' retrieves it (bare recall lists recent facts). action=status shows all categories.
action=consolidate imports this session's findings and decisions, runs lifecycle, then frees 25% facts/history/patterns capacity.
action=gotcha trigger='X' resolution='Y' for known pitfalls.
mode=semantic|exact for recall. category groups related facts.

Parameters: `action`*, `as_of`, `category`, `confidence`, `content`, `dry_run`, `examples`, `format`, `key`, `limit`, `merge`, `mode`, `path`, `pattern_type`, `query`, `resolution`, `severity`, `store`, `trigger`, `value`

## `sobre_outline`

WORKFLOW: call BEFORE sobre_read to map code structure (a syntax-aware table of contents).
Accepts a FILE or a DIRECTORY (folder surface — per-file symbols). Symbols come from
tree-sitter (27 languages, real line spans); a conservative regex fallback covers the rest.
kind=fn|struct|class|trait|enum|impl|all filters by kind; match=<substr> filters by name
(case-insensitive); format=json emits deterministic JSON labelling the backend per file.
ANTIPATTERN: NOT for file content (use sobre_read) or deep understanding (use sobre_compose).

Parameters: `format`, `kind`, `match`, `path`*

## `sobre_patch`

Safe file edit. Anchored ops use line+hash from sobre_read(mode="anchored"); CONFLICT means those lines changed: the current ones come back with fresh anchors. replace_unique(path,old_text,new_text) is a no-read, exact unique replacement. replace_symbol/create/replace_all supported. ops[] runs in order, across files: one file's anchored ops apply all-or-nothing, and the batch stops at the first failure, saying what was not applied. A syntax error an edit introduces is reported, not refused. dry_run=true shows the diff and writes nothing.

Parameters: `dry_run`, `end_hash`, `end_line`, `find`, `hash`, `line`, `name`, `new_text`, `old_text`, `op`, `ops`, `path`, `replace`, `start_hash`, `start_line`

## `sobre_read`

Read source files. mode recommended — choose by intent (see `mode` below); defaults to auto when omitted.
To UNDERSTAND code run sobre_compose FIRST; sobre_read after it identified files.
anchored → edit by reference via sobre_patch (no exact-recall).

Parameters: `aggressiveness`, `fresh`, `limit`, `mode`, `offset`, `path`, `paths`, `protect`, `raw`, `start_line`

## `sobre_refactor`

Symbol navigation via the language server: references, definition, implementations, and rename as a preview (nothing is written; apply with sobre_patch).
Symbol edits work without one: replace_symbol_body, insert_before_symbol, insert_after_symbol resolve the target by name_path (in path's file when path is given) or by path+line.
ANTIPATTERN: not for symbol discovery — use sobre_search action=symbol, sobre_outline for a file's symbols.

Parameters: `action`*, `column`, `dry_run`, `expected_hash`, `line`, `name_path`, `new_name`, `new_text`, `path`, `text`

## `sobre_review`

What a change to a file touches: the files that depend on it, the call sites of its functions in other files, and its test files.
Actions: review (one file), diff-review (every file in git diff text, or the uncommitted changes when path is omitted), checklist (review questions). depth=N hops of dependents (default 3).
Not a substitute for running the tests.

Parameters: `action`*, `depth`, `path`

## `sobre_search`

Search code: regex(pattern, default) | semantic(query) | symbol(name|handle) | reindex | find_related(file_path,line). anchored=true enables sobre_patch refs; queries batches regex searches. Run sobre_compose FIRST.

Parameters: `action`, `anchored`, `exclude`, `exclude_pattern`, `file`, `file_path`, `handle`, `include`, `kind`, `line`, `max_results`, `name`, `path`, `paths`, `pattern`, `queries`, `query`, `top_k`, `workspace`

## `sobre_session`

Session memory. save at session end, load at start, status = snapshot;
task|finding|decision record progress (value=text).
export returns the session as a redacted JSON bundle (write=true or path= also saves it); import path= loads one.
ANTIPATTERN: permanent project knowledge → sobre_knowledge.

Parameters: `action`*, `format`, `path`, `session_id`, `value`, `write`

## `sobre_shell`

WORKFLOW: preferred — auto-compresses output (build/test/log).
raw=true for verbatim output.
[exit:N] on errors (lossless).
Runs every command as given; the agent host decides what may run.
Sets GIT_TERMINAL_PROMPT=0, GIT_PAGER=cat, PAGER=cat, a UTF-8 locale if none, stdin=/dev/null.
Multi-line scripts: write them to a file, then run the file.

Parameters: `background_action`, `command`, `cwd`, `env`, `job_id`, `raw`, `run_in_background`, `timeout_ms`

## `sobre_tree`

Directory tree with file counts per directory. depth=N (default 3);
show_hidden for dotfiles; paths for multi-root.
respect_gitignore filters ignored files (default true).
WORKFLOW: lightweight orientation before sobre_compose.

Parameters: `depth`, `path`, `paths`, `respect_gitignore`, `show_hidden`

