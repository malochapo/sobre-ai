# Appendix — MCP Tools (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/server/registry.rs` and the tool definitions it registers.

sobre registers **15 MCP tools**, the same list in every session (invariant 3). This page shows the build without local embeddings; the build with them adds one parameter, `sobre_search`'s `mode` (`bm25|dense|hybrid`). A handler reads no argument its schema does not declare (a test enforces it).

## `sobre_checkpoint`

Local shadow git history of the agent's changes — separate from the user's .git.
WORKFLOW: snapshot before+after changes to capture exactly what was modified.
Actions: snapshot (record current state), log (list checkpoints with SHAs),
diff from=... to=... (compare checkpoints), restore ref=... (revert files).
ANTIPATTERN: Never touches the user's repository — completely isolated shadow history.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | one of `snapshot`, `log`, `diff`, `restore` |  | snapshot\|log\|diff\|restore (default: log) |
| `from` | string |  | Checkpoint SHA to diff from (default: the last checkpoint, action=diff only) |
| `limit` | integer |  | Max checkpoints in log (default: 20, max: 200) |
| `message` | string |  | Label for the snapshot (action=snapshot only) |
| `path` | string |  | File/dir to restore (omit for full restore, action=restore only) |
| `ref` | string |  | Checkpoint SHA to restore (required for action=restore). The current tree is checkpointed first, so a restore can be undone |
| `to` | string |  | Checkpoint SHA to diff to (default: the working tree, action=diff only) |

- When `action` is `restore`: requires `action` and `ref`.

## `sobre_compose`

PRIMARY TOOL — call FIRST for understanding code (before editing/debugging/'how does X work').
Omit task at session start: returns the project overview — structure, markers, last task, recent decisions, facts from previous sessions.
With a task: returns ranked files with relevant symbol source inline grouped by file.
Combines BM25 lexical+semantic+associative retrieval+submodular optimization.
ANTIPATTERN: Do NOT chain search→read→symbol — one compose replaces the whole chain.
ANTIPATTERN: Do NOT Read files whose source compose already returned — it IS the source.
WORKFLOW: Fire parallel sobre_read or sobre_compose for different areas.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string |  | Project root |
| `task` | string |  | Short English task/question or symbol names. Omit for the session-start project overview. |

## `sobre_expand`

Full output that compression elided. A compressed output's footer names its id → sobre_expand(id=ID). Optional start_line/end_line (1-indexed, inclusive) or search (substring) return a slice. Saved outputs expire after 24h.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Id from the compressed output's footer |
| `end_line` | integer |  |  |
| `search` | string |  | Return only lines containing this substring |
| `start_line` | integer |  |  |

## `sobre_glob`

Find files by glob pattern (respects .gitignore; multi-root via paths).
For file CONTENT search use sobre_search.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `pattern` | string | yes | *.ts names a file at any depth; a glob with / (src/*.ts, src/**/*.ts) matches the path from the root, * staying within one directory |
| `ignore_gitignore` | boolean |  | Also match .gitignore'd files |
| `max_results` | integer |  | default 200 |
| `path` | string |  |  |
| `paths` | array of string |  |  |

## `sobre_graph`

Code graph: file dependencies and function calls.
Files: neighbors=imports±direction, impact path=file or type → files that depend on it (depth, default 5), changes → files affected by uncommitted changes, path path→to=dependency chain, related.
Calls: callers|callees symbol='fn' → every call site with file:line (depth≤5, path scopes to a file); trace symbol→to=call chain.
symbol symbol='fn' (or path="file.rs::fn") returns the definition. diagram kind=deps|calls (Mermaid). build re-scans the project; status says whether the graph is current.
For understanding code use sobre_compose FIRST.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | one of `build`, `status`, `related`, `symbol`, `impact`, `changes`, `neighbors`, `path`, `diagram`, `callers`, `callees`, `trace` | yes |  |
| `depth` | integer |  | Hops; callers\|callees 1-5 (default 1), impact\|changes (default 5) |
| `format` | string |  | neighbors\|path\|impact\|changes: text\|json |
| `kind` | string |  | diagram: deps\|calls |
| `path` | string |  | File; file scope for symbol\|callers\|callees |
| `project_root` | string |  |  |
| `symbol` | string |  | Function (symbol\|callers\|callees\|trace) |
| `to` | string |  | Target file (path) or function (trace) |

- When `action` is `related` or `impact` or `neighbors` or `path`: requires `path`.
- When `action` is `symbol`: requires `symbol` or `path`.
- When `action` is `path`: requires `path` and `to`.
- When `action` is `callers` or `callees`: requires `symbol`.
- When `action` is `trace`: requires `symbol` and `to`.

## `sobre_knowledge`

Persistent memory across sessions — remember decisions, patterns, and facts for recall.
WORKFLOW: save after completing significant tasks; recall at session start.
action=remember value='Y' saves a fact (key optional — derived from value; content= is an accepted alias).
action=recall query='X' retrieves it (bare recall lists recent facts). action=status shows all categories.
action=consolidate imports this session's findings and decisions, runs lifecycle, then frees 25% facts/history/patterns capacity.
action=gotcha trigger='X' resolution='Y' for known pitfalls.
mode=semantic|exact for recall. category groups related facts.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | one of `remember`, `recall`, `search`, `status`, `rooms`, `timeline`, `wakeup`, `remove`, `feedback`, `pattern`, `gotcha`, `relate`, `unrelate`, `relations`, `relations_diagram`, `consolidate`, `restore`, `health`, `lifecycle_report`, `policy`, `export`, `import`, `embeddings_status`, `embeddings_reset`, `embeddings_reindex` | yes | remember: save a fact (value). recall: facts matching query, or the recent ones. search: facts and saved sessions (query). status, rooms: what is stored, by category. timeline: a category's history. wakeup: a short briefing. remove: delete a fact (category, key). feedback: value=up\|down on a fact (category, key). pattern: record a code pattern (pattern_type, value). gotcha: record a pitfall (trigger, resolution). relate\|unrelate\|relations\|relations_diagram: links between facts (category, key; query=category/key; value=kind). consolidate: import this session's findings and decisions. restore: bring back archived items. health, lifecycle_report, policy: store limits and upkeep. export\|import: a bundle (path). embeddings_*: the semantic index (dense build) |
| `as_of` | string |  | YYYY-MM-DD date filter |
| `category` | string |  | Fact category (remember: default general) |
| `confidence` | number |  | 0.0-1.0 |
| `content` | string |  | Alias for value (remember) |
| `dry_run` | boolean |  | consolidate: preview imports/reclaim without writing |
| `examples` | array of string |  |  |
| `format` | string |  | export: json\|okf (okf = portable Markdown bundle) |
| `key` | string |  |  |
| `limit` | number |  | restore: max items to recover (default 50) |
| `merge` | string |  | import: replace\|append\|skip-existing (default skip-existing) |
| `mode` | string |  | auto\|exact\|semantic\|hybrid |
| `path` | string |  | export/import: bundle directory (OKF) or file path |
| `pattern_type` | string |  |  |
| `query` | string |  | Query for recall/search/relate/restore |
| `resolution` | string |  | gotcha resolution/fix |
| `severity` | string |  | gotcha: critical\|warning\|info |
| `store` | string |  | restore: facts\|history\|patterns (default: all) |
| `trigger` | string |  | gotcha trigger pattern |
| `value` | string |  |  |

- When `action` is `remember`: requires `value` or `content`.
- When `action` is `pattern`: requires `pattern_type` and `value`.
- When `action` is `remove` or `feedback` or `relations` or `relations_diagram`: requires `category` and `key`.
- When `action` is `relate` or `unrelate`: requires `category` and `key` and `query`.
- When `action` is `timeline`: requires `category`.
- When `action` is `import`: requires `path`.
- When `action` is `search`: requires `query`.
- When `action` is `gotcha`: requires `trigger` and `resolution`.

## `sobre_outline`

WORKFLOW: call BEFORE sobre_read to map code structure (a syntax-aware table of contents).
Accepts a FILE or a DIRECTORY (folder surface — per-file symbols). Symbols come from
tree-sitter (27 languages, real line spans); a conservative regex fallback covers the rest.
kind=fn|struct|class|trait|enum|impl|all filters by kind; match=<substr> filters by name
(case-insensitive); format=json emits deterministic JSON labelling the backend per file.
ANTIPATTERN: NOT for file content (use sobre_read) or deep understanding (use sobre_compose).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | File or directory |
| `format` | string |  | Output format: text (default) \| json (deterministic) |
| `kind` | string |  | Filter by kind: fn\|struct\|class\|trait\|enum\|impl\|all |
| `match` | string |  | Keep only symbols whose name contains this (case-insensitive) |

## `sobre_patch`

Safe file edit. Anchored ops use line+hash from sobre_read(mode="anchored"); CONFLICT means those lines changed: the current ones come back with fresh anchors. replace_unique(path,old_text,new_text) is a no-read, exact unique replacement. replace_symbol/create/replace_all supported. ops[] runs in order, across files: one file's anchored ops apply all-or-nothing, and the batch stops at the first failure, saying what was not applied. A syntax error an edit introduces is reported, not refused. dry_run=true shows the diff and writes nothing.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `dry_run` | boolean |  |  |
| `end_hash` | string |  |  |
| `end_line` | integer |  |  |
| `find` | string |  |  |
| `hash` | string |  |  |
| `line` | integer |  |  |
| `name` | string |  |  |
| `new_text` | string |  |  |
| `old_text` | string |  |  |
| `op` | one of `set_line`, `replace_lines`, `insert_after`, `delete`, `replace_unique`, `replace_symbol`, `create`, `replace_all` |  |  |
| `ops` | array of object |  |  |
| `path` | string |  |  |
| `replace` | string |  |  |
| `start_hash` | string |  |  |
| `start_line` | integer |  |  |

- When `op` is `set_line`: requires `op` and `line` and `hash` and `new_text`.
- When `op` is `replace_lines`: requires `op` and `start_line` and `end_line` and `new_text`.
- When `op` is `replace_unique`: requires `op` and `old_text` and `new_text`.
- When `op` is `replace_symbol`: requires `op` and `new_text`.
- When `op` is `create`: requires `op` and `new_text`.
- When `op` is `replace_all`: requires `op` and `find` and `replace`.

## `sobre_read`

Read source files. mode recommended — choose by intent (see `mode` below); defaults to auto when omitted.
To UNDERSTAND code run sobre_compose FIRST; sobre_read after it identified files.
anchored → edit by reference via sobre_patch (no exact-recall).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `aggressiveness` | number |  | 0.0–1.0 density (entropy/task) |
| `fresh` | boolean |  | Bypass cache |
| `limit` | integer |  | Max lines |
| `mode` | string |  | Defaults to auto (sobre picks). full=whole file, edit-ready; secret-like values show as [REDACTED:…] anchored=full with N:hh anchors to edit via sobre_patch raw=the bytes on disk: no framing, no redaction signatures=API map=structure aggressive\|entropy\|task=lossy summaries diff=what changed since this session last read the file reference=one line: size and type lines:N-M=window (lines:N is one line, lines:N- runs to the end, lines:5,10-20 multi-selects) anchored:N-M=anchored window density:0.NN=target share of tokens |
| `offset` | integer |  | start_line alias |
| `path` | string |  | Absolute path |
| `paths` | array of string |  | Batch read |
| `protect` | array of string |  | Symbols kept verbatim |
| `raw` | boolean |  | Same as mode=raw |
| `start_line` | integer |  | 1-based |

## `sobre_refactor`

Symbol navigation via the language server: references, definition, implementations, and rename as a preview (nothing is written; apply with sobre_patch).
Symbol edits work without one: replace_symbol_body, insert_before_symbol, insert_after_symbol resolve the target by name_path (in path's file when path is given) or by path+line.
ANTIPATTERN: not for symbol discovery — use sobre_search action=symbol, sobre_outline for a file's symbols.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | one of `rename`, `references`, `definition`, `implementations`, `replace_symbol_body`, `insert_before_symbol`, `insert_after_symbol` | yes |  |
| `column` | integer |  | 0-indexed column |
| `dry_run` | boolean |  | Symbol edits: show the diff, write nothing |
| `expected_hash` | string |  | BLAKE3 hex of current range (TOCTOU guard) |
| `line` | integer |  | 1-indexed line; for symbol edits without name_path, the symbol around this line in path |
| `name_path` | string |  | Symbol path for body edits (qualified or bare) |
| `new_name` | string |  | New symbol name (rename preview) |
| `new_text` | string |  | Full replacement declaration text (replace_symbol_body) |
| `path` | string |  | File; for symbol edits, the file name_path is looked up in |
| `text` | string |  | Sibling text to insert (auto-indented) |

- When `action` is `rename`: requires `action` and `path` and `new_name`.
- When `action` is `references`: requires `action` and `path`.
- When `action` is `definition`: requires `action` and `path`.
- When `action` is `implementations`: requires `action` and `path`.
- When `action` is `replace_symbol_body`: requires `action` and `new_text`.
- When `action` is `insert_before_symbol`: requires `action` and `text`.
- When `action` is `insert_after_symbol`: requires `action` and `text`.

## `sobre_review`

What a change to a file touches: the files that depend on it, the call sites of its functions in other files, and its test files.
Actions: review (one file), diff-review (every file in git diff text, or the uncommitted changes when path is omitted), checklist (review questions). depth=N hops of dependents (default 3).
Not a substitute for running the tests.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | one of `review`, `diff-review`, `checklist` | yes |  |
| `depth` | integer |  | Analysis breadth (default 3) |
| `path` | string |  | File path (review/checklist) or git diff text (diff-review; omit for the uncommitted changes) |

- When `action` is `review`: requires `action` and `path`.
- When `action` is `checklist`: requires `action` and `path`.

## `sobre_search`

Search code: regex(pattern, default) | semantic(query) | symbol(name|handle) | reindex | find_related(file_path,line). anchored=true enables sobre_patch refs; queries batches regex searches. Run sobre_compose FIRST.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | one of `regex`, `semantic`, `symbol`, `reindex`, `find_related` |  |  |
| `anchored` | boolean |  |  |
| `exclude` | string |  | Glob of files to skip, read as include |
| `exclude_pattern` | string |  |  |
| `file` | string |  |  |
| `file_path` | string |  |  |
| `handle` | string |  |  |
| `include` | string |  | Glob of files to search: *.rs at any depth, src/*.rs from the root, *.{rs,ts} for several |
| `kind` | string |  |  |
| `line` | integer |  |  |
| `max_results` | integer |  |  |
| `name` | string |  |  |
| `path` | string |  |  |
| `paths` | array of string |  |  |
| `pattern` | string |  |  |
| `queries` | array of object |  |  |
| `query` | string |  |  |
| `top_k` | integer |  |  |
| `workspace` | boolean |  | semantic: also search the configured search_roots |

## `sobre_session`

Session memory. save at session end, load at start, status = snapshot;
task|finding|decision record progress (value=text).
export returns the session as a redacted JSON bundle (write=true or path= also saves it); import path= loads one.
ANTIPATTERN: permanent project knowledge → sobre_knowledge.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | one of `status`, `task`, `finding`, `decision`, `save`, `load`, `list`, `reset`, `cleanup`, `resume`, `snapshot`, `restore`, `diff`, `export`, `import`, `output_stats` | yes | status: this session. task\|finding\|decision: record one (value). save: write it to disk. load: a saved session (session_id, or this project's latest). list: this project's saved sessions. reset: save, then start a new one. cleanup: delete sessions older than 7 days. resume: a block to continue from. snapshot: save a compaction snapshot; restore: read one back (session_id, or this project's latest). diff: compare two sessions (value='id_a id_b [json]'). export\|import: a JSON bundle (path, format, write). output_stats: how often compressed output kept paths and identifiers intact, this process |
| `format` | string |  | export: json\|summary |
| `path` | string |  | export: file to write; import: bundle to read |
| `session_id` | string |  | load, restore: omit for this project's latest |
| `value` | string |  |  |
| `write` | boolean |  | export: also save the bundle (default location under the data directory) |

## `sobre_shell`

WORKFLOW: preferred — auto-compresses output (build/test/log).
raw=true for verbatim output.
[exit:N] on errors (lossless).
Runs every command as given; the agent host decides what may run.
Sets GIT_TERMINAL_PROMPT=0, GIT_PAGER=cat, PAGER=cat, a UTF-8 locale if none, stdin=/dev/null.
Multi-line scripts: write them to a file, then run the file.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `background_action` | one of `status`, `cancel` |  | Inspect or cancel a background sobre_shell job. |
| `command` | string |  | Shell command |
| `cwd` | string |  | Working dir (persists across calls) |
| `env` | object |  | Extra env vars |
| `job_id` | string |  | Job id returned by run_in_background. |
| `raw` | boolean |  | Skip compression (verbatim) |
| `run_in_background` | boolean |  | Detach immediately and return a job id. The command keeps timeout_ms; poll or cancel with background_action and job_id. |
| `timeout_ms` | integer |  | Stop the command once it has printed nothing for this long, in ms (max 3600000); a command runs at most 1 hour. NOT the inline wait: a command still running at the ~110s foreground cap detaches to a pollable shell_* job. |

## `sobre_tree`

Directory tree with file counts per directory. depth=N (default 3);
show_hidden for dotfiles; paths for multi-root.
respect_gitignore filters ignored files (default true).
WORKFLOW: lightweight orientation before sobre_compose.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `depth` | integer |  | Max depth |
| `path` | string |  | Dir |
| `paths` | array of string |  | Multi-root (alternative to path) |
| `respect_gitignore` | boolean |  | Filter ignored |
| `show_hidden` | boolean |  | Include dotfiles |

