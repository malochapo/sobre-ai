# Phase 1 audit

The audit of sobre 0.1.0 at `b864054` (branch `chore/release-prep`). It
lists every bug, incomplete feature, invariant drift, dead code and
documentation gap found, the decisions taken, and the plan to fix all of it.

The audit combined four passes:

- an end-to-end run of the release binary in a sandbox HOME, covering
  `init`, `uninstall`, `doctor`, `config`, `call`, `-c`, `raw`, hooks, the
  MCP server over stdio and the proxy;
- a read-only review of the 16 MCP tools;
- a dead-code and leftovers scan;
- a review of the documentation and install path.

## How this file is kept

- **Branches.** Every fix branch starts from `chore/release-prep` and is
  squash-merged back into it. `chore/release-prep` goes to `develop` once,
  at the end, through PR #30.
- **Tracking.** Each finding has an ID. When it is fixed, its box is ticked
  and the PR number is added. A finding closed without a fix says why.
- **References.** `file:line` references are relative to `rust/src/`
  unless the path says otherwise. They point at `b864054` and drift as the
  fixes land.

## Decisions

Taken on 2026-10-08.

| # | Question | Decision |
|---|---|---|
| 1 | Merge `ctx_impact` into `ctx_graph`? | Yes: one graph tool, 15 tools. |
| 2 | The AUTO CONTEXT prefix on the first call of a session | Remove it. |
| 3 | `edit_quality.json`, which carries state from one run to the next and changes what `mode=auto` resolves to | Stop persisting it (invariant 2). |
| 4 | The `edit_health` hook writing `ctx_knowledge` facts the agent never asked for | Remove those writes (invariant 5). |
| 5 | `ctx_refactor`: `file` and `scope` are ignored, `rename` only previews, `symbols_overview` needs an LSP | Yes, the recommendation: `file` through `path`, drop `scope`, `rename` labelled a preview, `symbols_overview` dropped for `ctx_outline` (B23). |
| 6 | `ctx_review`, whose heuristics are broken | Repair it. |
| 7 | The non-PDF extractors (about 2,100 lines that never run) | Remove them. |
| 8 | Release binaries are built without `dense` | Publish a `dense` binary as well. |
| 9 | The tools are still called `ctx_*` | Rename them `sobre_*`. |

A side effect of decision 9: hosts prefix MCP tools with the server name, so
in Claude Code `sobre_read` appears as `mcp__sobre__sobre_read`.

## Plan

The lots run in this order:

| Lot | Scope | Status |
|---|---|---|
| A | The user's machine: `uninstall`, `init`, agent integration | done (#31) |
| B | The MCP tools: bugs, incomplete features, drift; decisions 1–4 and 6 | done (#32, #33, #34, #35, #36, #37) |
| C | The CLI and the proxy | done (#38, #39) |
| R | Tool names `ctx_*` → `sobre_*` (decision 9) | done (#40) |
| D | Dead code, stale files, stale comments, duplicates; decision 7 | done (#41, #42, #43, #44, #45, #46) |
| F | Release: publish a `dense` binary (decision 8) | done (#47) |
| E | Documentation and install path, written last so it describes the final behaviour | done but E5, E7, E13 (invariant PR) and E20 (#49) |

## What already works

The end-to-end run found no crash and no panic. These all behaved as
documented:

- **Basic CLI.** `--version`, help, and suggestions for unknown commands
  (`doctr` → `doctor`).
- **`init`.**
  - It detects 8 agents and writes MCP entries named `sobre` with the
    absolute binary path.
  - It writes hooks per host and the skill to 5 agents, and no approval
    settings.
  - It keeps the user's own MCP entries, hooks and permissions.
  - It is idempotent; `--check` writes nothing, and `--json` is valid.
- **`doctor`.** 20/20, `integrations`, overhead, `lint-context` (plain and
  JSON).
- **`config`.** `show`, `path`, `init`, and `set` on valid keys.
- **`index`.** `status` and `build`.
- **`sobre call`, by tool:**
  - `ctx_tree`;
  - `ctx_read` in the full, lines, anchored, raw, signatures and map modes,
    with `.env` redacted;
  - `ctx_search` with regex and symbol;
  - `ctx_outline` and `ctx_glob`;
  - `ctx_compose`;
  - `ctx_graph`: build, status, related, impact, neighbors;
  - `ctx_knowledge` with a category: remember, recall, search, status,
    gotcha;
  - `ctx_session`: task, finding, decision, status, save, load, list;
  - `ctx_shell` with `[exit:N]`;
  - `ctx_expand` with range, search and full; unknown ids and `../` are
    rejected.
- **Shell commands.** `-c` compresses and puts the `[sobre: …]` footer on
  stderr; `raw` passes output through; exit codes propagate.
- **MCP over stdio.**
  - It accepts protocol versions 2025-06-18 and 2024-11-05.
  - It lists 16 tools, answers ping, and returns -32602 and -32601 errors.
  - A re-read of an unchanged file gets the short stub.
  - Nothing stray reaches stdout, and the server exits 0 on EOF.
- **Hooks.**
  - They compress output: Claude Code `updatedToolOutput`, Copilot
    `modifiedResult`, OpenCode `output`.
  - Read deduplication works, as does the Cursor and Gemini wrapping.
  - On error they let the call through; observe hooks stay silent.
- **`gain`.** All its variants.
- **Proxy.**
  - `start --port=N --detach`, `status`, `/health` (200), the token,
    `/status` and `stop` work.
  - `GET /v1/responses` answers 426 when the request carries auth.
- **`uninstall`.**
  - `--dry-run` changes nothing.
  - A real run removes entries, hooks, skills, the plugin, the directories
    and the binary, and keeps the user's other entries (except A2).

---

## Lot A — the user's machine

Fixed in #31. How the fixes work:

- **One writer per file.** Each agent's MCP entry is written in one place
  (`editor_registry::write_config`). Hooks and skills are written by
  `hooks::install_agent_hook`. Claude Code's entry used to be written three
  times; Codex, Copilot, OpenCode, Windsurf and Zed twice.
- **In-place edits.** Every JSON(C) config is edited through
  `config_io::edit_json` and `core::jsonc::rewrite_preserving`, built on a
  JSONC CST (`jsonc-parser`). Comments, key order, trailing commas and
  formatting stay. Codex's TOML goes through `toml_edit`.
- **Backups.**
  - The first edit of an existing file keeps `<file>.sobre.bak`, the file as
    the user had it.
  - When removing sobre's entries gives back exactly that content,
    `uninstall` restores the backup byte for byte; otherwise it removes only
    sobre's entries.
  - A file that existed before sobre is never deleted.
  - An unparseable file is left alone and reported. Text injection into
    broken JSON is gone.
- **Journal.** `init` records every file it writes. That record gives the
  per-agent report, and lets `init --check` run the real code with writes
  turned off.
- **Round trip, verified.** A sandbox HOME with a JSONC Zed config, Claude
  settings holding user hooks, a Codex `[features] hooks = true`, and two
  decoy processes went through `init`, `init` again, `uninstall`:
  - the second `init` changed nothing;
  - doctor scored 20/20;
  - the decoys survived;
  - the HOME came back byte-identical.

- [x] **A1 `uninstall` kills processes that are not sobre's.**
  - It runs `pgrep -x sobre` and `pgrep -f '/sobre(\s|$)'` across the whole
    machine with no user or HOME filter, then SIGTERM and SIGKILL
    (`uninstall/binary.rs:16-38`, `ipc/process.rs`).
  - In the sandbox it killed a decoy `python3 …/e2e_decoy/sobre install`
    and a `…/sobre mcp` server from another HOME. A heuristic spares any
    command containing "code".
  - `ipc/process.rs:380-384` also filters on subcommands that no longer
    exist (`dashboard`, `daemon`, `stop`).
  - Fix: only signal processes of the current user whose executable is the
    binary being removed.
- [x] **A2 `uninstall` turns off the user's Codex hooks.**
  - A user had `[features] hooks=true` and a Stop hook. After `init codex`
    and `uninstall` they had `hooks=false`, while the output said "user
    settings preserved".
  - Without a `[features]` table before `init`, `uninstall` leaves
    `[features] hooks=false` behind.
  - Fix: restore the value found before `init`, or remove the key if `init`
    added it.
- [x] **A3 `uninstall` does more than the documentation and help say.**
  - It deletes the config, data, state and cache directories: usage,
    sessions, knowledge, checkpoints and indexes
    (`uninstall/mod.rs:155,199-246`).
  - It deletes `./.sobre/` in the current directory (`mod.rs:248-260`).
  - It deletes the binary from `~/.local/bin` or `/usr/local/bin`, or the
    running exe (`binary.rs:62-74`).
  - `--keep-config` still deletes `config.toml` (`mod.rs:155`).
  - Help says `--keep-config` keeps "MCP configs and rules"
    (`mod.rs:75,118`), but sobre writes no rules.
  - `mod.rs:178` prints "Reinstall with: cargo install sobre", which only
    works for a crates.io release.
  - Fix: make the behaviour match one clear statement, and say which data
    goes.
- [x] **A4 `uninstall` leaves debris.** Compared with HOME before `init`, it
  leaves:
  - empty files that `init` created: `~/.cursor/mcp.json`,
    `~/.codeium/windsurf/mcp_config.json`, `~/.copilot/mcp-config.json`,
    `~/.config/Code/User/mcp.json` and its tree,
    `~/.config/opencode/opencode.json`;
  - an empty `~/.config/opencode/plugins/`;
  - empty sections: Gemini `"hooks":{}` and `"mcpServers":{}`, Zed
    `"context_servers":{}`, and Codex `[features] hooks=false`;
  - `.bak` files: `~/.claude.json.bak`, `~/.claude/settings.json.bak`,
    `~/.config/zed/settings.json.bak` (only `*.sobre.bak` is cleaned);
  - an empty `~/.local/state`;
  - a `.claude.json` rewritten with a `}  },` formatting artefact.
- [x] **A5 `init` reformats the user's config files.**
  - It strips comments and trailing commas from Zed's `settings.json`; they
    survive only in the `.bak`.
  - It sorts keys alphabetically in `.claude.json`,
    `.claude/settings.json` and Gemini's settings.
  - Fix: edit in place and keep the key order and comments.
- [x] **A6 `init` writes the VS Code config when VS Code is not detected.**
  - It writes `~/.config/Code/User/mcp.json` anyway.
  - `init vscode` is accepted (not in help) and also installs the Copilot
    CLI files.
- [x] **A7 `init` output is unclear.**
  - Everything goes to stderr.
  - There is no summary of detected and undetected agents, and no line
    about the Claude Code MCP entry.
  - The first run says "already configured" for entries the editors step
    has just written; the second run prints "Installed …" again.
  - The doctor warning and the cwd=$HOME warning appear only with `--json`.
  - With no agent detected it prints nothing and exits 0.
  - `--json` reports "Zed hooks: installed" (Zed gets no hooks), and its
    "Claude Code hooks" entry points at `~/.claude.json`.
- [x] **A8 `init --check` previews nothing useful**
  (`cli/init_cmd.rs:23-39`, help at `:269`).
  - It prints "Would connect: every detected agent", the data dir and the
    binary path.
  - It detects no agents and lists no files.
- [x] **A9 `uninstall --dry-run` wording is broken.**
  - Examples: "Would update MCP config removed from Cursor", "Would Cursor
    removed", "Would remove OpenCode plugin removed".
  - The config dir is called "data directory".
  - It prints "Removed 6 empty installer director(y/ies)".
- [x] **A10 Gemini hook reply drops fields.**
  - The reply is `{"hookSpecificOutput":{"tool_input":{"command":…}}}`: it
    drops `description` and `dir_path`. The Cursor reply keeps them.
  - This breaks the call if Gemini replaces `tool_input` wholesale (not
    confirmed).

- [x] **A11 The MCP server rewrote the agents' hook configs on every
  start.** `refresh_installed_hooks` ran at server start. Removed: `init` and
  `doctor --fix` re-render hooks when the binary moves.
- [x] **A12 `init` started a background `index build` of the cwd** when the
  cwd held a project marker. Removed: indexes build on first use.
- [x] **A13 Agent writes forced mode 0600 on the user's configs**
  (`config_io::write_atomic`). An existing file now keeps its mode; a new
  file is owner-only.

## Lot B — the MCP tools

Fixed so far: #32 (paths, `ctx_read`, hidden read state), #33 (what the
agent receives: prefixes, markers, footers, prompts and resources), #34
(edits), #35 (graph: `ctx_impact` merged into `ctx_graph`, `ctx_review`
repaired), #36 (errors, knowledge, sessions, the edit-quality store and
the edit_health hook), #37 (`ctx_refactor`, decision 5).

### Bugs

- [x] **B1 `ctx_tree`, `ctx_glob` and regex `ctx_search` without `path`
  walk the server's cwd instead of the project root.**
  - `server/multi_path.rs:87` falls back to `.`, and its doc wrongly calls
    that the project root.
  - An explicit `path="."` goes through `resolve_path` unchanged
    (`tools/server_paths.rs:12`), and the server never chdirs.
  - The CLI pins `.` to the root (`cli/call_cmd.rs:185-196`); MCP does
    not.
  - Fix: fall back to `ctx.project_root`, and map `.` and the empty path
    to it.
- [x] **B2 `ctx_read` rejects files over 4 MB before any line window**
  (`tools/registered/ctx_read.rs:259-274`).
  - `lines:`, `start_line`+`limit` and `anchored:N-M` all fail, while the
    error suggests exactly those, so the agent loops.
  - The core already streams anchored windows
    (`ctx_read/mod.rs` `try_disk_anchored_window`), but the handler never
    gets there.
  - The message says "or increase the limit", which cannot be configured.
  - Fix: skip the gate for windows, and stream `lines:` too.
- [x] **B3 `ctx_read` changes modes silently.**
  - It forces `full` for "instruction files": any path containing
    `/skills/` or `/agents.md`, or named `rules.md`, `skill.md` or
    `.cursorrules` (`registered/ctx_read.rs:216`, `ctx_read/helpers.rs`,
    batch at `impl_multi_read.rs:34`).
  - For example, `src/skills/mod.rs` with `mode=anchored` returns no
    anchors, and `start_line`/`limit` are ignored.
  - An unknown mode (`bogus`) returns the full file.
  - `signatures` and `map` on small files return the full content without
    saying so.
  - Fix: honour the requested mode, reject unknown modes, and label any
    fallback.
- [x] **B4 Batch `ctx_read` (`paths=[…]`) drops files.**
  - Binary and oversized files are skipped silently
    (`batch_read.rs:58-65`), while the summary says "Read N files".
  - The file that crosses the 512 KB cap is read and marked delivered
    before being dropped (`impl_multi_read.rs:39-51`). Later reads of it
    return `[unchanged]` although the agent never saw it.
  - It ignores `raw`, `start_line`, `offset`, `limit`, `aggressiveness`,
    `protect` and `lines:` validation.
  - Fix: list skipped files, check the cap before marking delivered, and
    reject parameters batch does not support.
- [x] **B5 Under lock contention `ctx_read` reports a missing or unreadable
  file as empty.**
  - `registered/ctx_read.rs:579-583` does
    `read_file_lossy(..).unwrap_or_default()`, then
    `cache.store(path, "")`.
  - Fix: return the same error as the fast path.
- [x] **B6 `ctx_expand` output is itself terse-compressed**
  (`server/post_process.rs:32-40`).
  - `skip_terse` exempts only `ctx_read`, `ctx_patch` and raw shell
    (`core/terse/mod.rs:33`).
  - This breaks invariant 5: the original must stay reachable.
  - Fix: add `ctx_expand` to the verbatim set.
- [x] **B7 Re-running a command overwrites the earlier saved output.**
  - The tee file name comes from the command text (e.g.
    `tee/python3_gen_py_1_3f13142d.log`).
  - The first run's footer path and `ctx_expand` id then point at the
    second run's output. A code comment calls this intentional.
  - This breaks "original reachable".
  - Fix: make the name unique per run.
- [x] **B8 `format=json` on `ctx_graph` (neighbors, path) and `ctx_impact`
  returns JSON that does not parse.**
  - Neither overrides `produces_machine_readable` (`tool_trait.rs:109`);
    only `ctx_outline` does.
- [x] **B9 `ctx_patch op=replace_all` sets the file mode to 0600**
  (`registered/ctx_patch.rs:606` uses `config_io::write_atomic`, which
  forces 0600 at `config_io.rs:189-197`).
  - It also reads before taking the per-file lock (`:584` vs `:600`), and
    skips the symlink guard, `session.mark_modified` and the bounce
    tracker.
  - Fix: use `edit_io` with permissions, under the lock.
- [x] **B10 Symbol edits lose the file mode and replace symlinks.** This
  covers `ctx_refactor replace_symbol_body` and `insert_*`, and
  `ctx_patch op=replace_symbol`.
  - `lsp/edit_apply.rs:154-171` writes with `std::fs::write` to a fixed
    temp name `.{f}.sobre.v2a.tmp.{pid}`, then renames, with no per-file
    lock.
  - Fix: use `edit_io` and `path_locks`.
- [x] **B11 Remove the AUTO CONTEXT prefix (decision 2).**
  - The first tool call of every MCP session gets about 1.2 KB of
    unrequested text: `--- AUTO CONTEXT --- [TASK:…] OUTPUT-HINT …
    CRITICAL: F1=…` (`server/call_tool.rs:43-94,155`).
  - The task is guessed from an old saved session.
  - `gain --by-tool` shows ctx_tree at −566 % and ctx_search at −103 %.
  - It also makes `raw` not raw.
  - The once-per-session briefing is lost if the first eligible call is an
    image read or an error.
- [x] **B12 `ctx_shell` turning `cat <file>` into a read is broken**
  (`registered/ctx_shell.rs:109-133`).
  - The path is resolved against the server cwd, ignoring the `cwd` argument
    and the session's shell cwd.
  - A failed read returns "ERROR: file not found" with exit 0.
  - An already-delivered file returns "[unchanged … fresh=true to
    re-read]", but `ctx_shell` has no `fresh` parameter.
  - `raw=true` is ignored.
- [x] **B13 The MCP prompts are wrong** (`server/prompts.rs`).
  - `context-budget` tells the agent to call
    `ctx_session(action="budget")`, which does not exist (`:92`).
  - `context-focus` claims to set the task intent but sets nothing.
- [x] **B14 The progress-notification sender is never cleared**
  (`server/server_handler.rs:309-316`).
  - Concurrent requests overwrite it.
  - `ctx_shell.rs:244-262` sends progress on a stale token.
  - Fix: one sender per request, through `ToolContext`.
- [x] **B15 `ctx_search action=symbol` with an absolute `file=` finds
  nothing** (`registered/ctx_search.rs:384`).
  - The raw path goes into SQL `LIKE '%file%'`
    (`core/property_graph/node.rs:181`), but the graph stores relative
    paths, and `_` acts as a wildcard.
  - Fix: `index_paths::graph_relative_key(file, root)`, and escape the
    pattern.
- [x] **B16 Semantic search does not split snake_case.**
  - "retry backoff" finds nothing, while "retry_with_backoff" finds the
    function.
  - The skill's own example, "where are retries handled", finds nothing.
- [x] **B17 `ctx_graph` call-site line numbers are off.**
  - Callers report L11 and L17, and the parser reports L16, where the real
    lines are 10, 16 and 15.
- [x] **B18 `ctx_graph callees symbol=main` merges Rust's and Python's
  `main`**, with no warning about the ambiguity.
- [x] **B19 `ctx_knowledge remember` rejects the documented form.**
  - `{"action":"remember","value":"…"}` (and the same with `content` or
    `key`) returns "Error: category is required for remember", with
    exit 0.
  - The description says "action=remember value='Y' saves a fact (key
    optional)".
- [x] **B20 `ctx_read mode=diff` does not diff against git.**
  - The skill and instructions say it shows changes since HEAD.
  - The CLI answers "[no cached version for diff — use mode=full first…]".
  - Over MCP, after a full read, it answers "(no changes)" despite an
    uncommitted change.
- [ ] **B21 Outlines and signatures replace parameter types with letters**,
  e.g. `parse_config(input:s)`, `normalize(s:s)`, `attempts:n`.
- [x] **B22 `raw` and `full` are not exact.**
  - Every `ctx_read` output goes through `redaction::redact_text`
    (`ctx_read/mod.rs:886,941`, `registered/ctx_read.rs:683`,
    `redaction.rs:254-277`).
  - Secret-shaped values become `[REDACTED:…]`, and an agent editing from
    that view can write the placeholder back.
  - `raw` output also carries cross-source hints, a proactive-context
    suffix and a degrade note (`registered/ctx_read.rs:853-901`).
  - Fix: `raw` returns the bytes; any redaction is labelled where it
    happens.

### Found while fixing (paths and reads)

- [x] **B50 `ctx_read` guessed the task and an "intent".**
  - Every fifth read, it inferred the session task from plan files, the git
    diff or the files touched (`auto_infer_task`).
  - It also inferred an "intent" from the files read.
  - Both steered `entropy`/`aggressive` compression, and the guessed task
    showed in the AUTO CONTEXT prefix.
  - Fix: removed. Only the task the agent states conditions compression.
- [x] **B51 Semantic search stored its last query in the cwd's latest
  session**, and later compressed reads used it as relevance keywords.
  Removed.
- [x] **B52 Compaction consolidates knowledge from the cwd's latest session**
  (`server/compaction_sync.rs:34`), behind the agent's back.
- [x] **B53 The anti-inflation cap replaced a lossy view with the whole file
  silently** (#361). The file now comes with one line saying so.
- [x] **B54 `is_instruction_file` matched any path containing `/skills/`**,
  so `src/skills/mod.rs` counted as one. Only markdown under `skills/`
  counts now.

### Found while fixing (what the agent receives)

- [x] **B55 Knowledge was consolidated behind the agent's back.** After
  every 15th shell command (`tool_lifecycle.rs`), after every compaction,
  at shutdown and at startup, sobre merged the cwd's latest session into
  `ctx_knowledge`. All four are removed; knowledge changes only when the
  agent calls `ctx_knowledge`.
- [x] **B56 A condensed output that failed verification was sent anyway**
  with a `[VERIFY] WARN(…)` line. The agent now gets the tool's own output
  instead.
- [x] **B57 Client capabilities were guessed from the client's name** and
  written to `client-id.json`, which nothing read. Removed with the prompts
  and resources (this also covers the `client_capabilities.rs` parts of D4
  and D7).
- [x] **B58 Terse compression ran on every tool but `ctx_read`, `ctx_patch`
  and raw shell**, so code from `ctx_search`, `ctx_compose`, `ctx_expand`
  or a symbol body could lose lines. It now runs only on prose and shell
  output: `ctx_shell`, `ctx_knowledge` and `ctx_session`.

### Found while fixing (edits)

- [x] **B59 A failed edit marked the file as modified.** `ctx_patch` and the
  shared edit path called `session.mark_modified` after a CONFLICT, an
  ambiguous match or any error. Only a write marks it now.
- [x] **B60 The `replace_lines` error message carried a run of spaces** from
  a broken line continuation.

### Found while fixing (graph)

- [x] **B61 `ctx_impact action=diff` found nothing when the project is below
  the git root.** `git diff --name-only` gives paths from the git root,
  the graph stores them from the project root. `ctx_graph action=changes`
  asks git for paths relative to the project.
- [x] **B62 An absolute `file=` inside the project switched `ctx_search
  action=symbol` to the nearest git root**, which has no graph when the
  project is a subdirectory. A file inside the session's project now keeps
  that project.
- [x] **B63 `ctx_graph action=impact` printed bare file names**
  (`mod.rs` four times) and stopped at two hops. It now uses the impact
  walk `ctx_impact` had: project-relative paths, `depth` (default 5), and a
  type name resolved to its file.

### Found while fixing (the rest)

- [x] **B64 The `edit_health` hook computed a notice nobody saw.** Its only
  output was the `ctx_knowledge` write that decision 4 removes, so the
  whole hook is gone.
- [x] **B65 `ctx_session export write=true` wrote into the project**
  (`<project>/.sobre/session_bundles/`). The default is now the data
  directory; a `path=` still writes where it says.

### Incomplete features

- [x] **B23 `ctx_refactor`** (decision 5: the recommendation, #37).
  - **Origin.** All four parts predate the fork. They worked only through
    the JetBrains plugin, which was removed.
  - **`file`** was meant to scope `name_path` lookup to one file, to stop
    AMBIGUOUS_SYMBOL on common names; it was advertised and never read.
    Done: `path` scopes the lookup, and the file filter is in the SQL
    query, ahead of its row limit. The worktree mismatch check is gone with
    it: a scoped lookup cannot land in another checkout.
  - **`scope`** (`project`/`all`) was ignored; only the IDE knew it.
    Removed from the schema, the backend trait and the client.
  - **`rename`** never applied an edit. It is now described and labelled as
    a preview ("nothing was written; apply with ctx_patch").
  - **`symbols_overview`** started a language server to list symbols from
    the index. Removed in favour of `ctx_outline`, with
    `overview_from_index` and its nearest-`.git` root.
  - The router's error now lists every extension `language_for_extension`
    maps, and language servers are cached per language and project root
    instead of per language.
- [x] **B24 `ctx_patch dry_run` validates nothing except `replace_all`.**
  - The anchored ops (`ctx_patch.rs:110-115`), `replace_unique`
    (`:284-295`) and `replace_symbol` (`:510-516`) return a canned
    "DRY RUN: would …".
  - `ops[]` without a top-level `path` prints an empty path.
- [x] **B25 Repair `ctx_review` (decision 6).**
  - Its checklist counts lines containing `→` (`ctx_review.rs:98`), but
    `ctx_impact` output never contains `→`.
  - It drops the impact section whenever the output contains "No" or
    "empty" (`:24`).
  - Caller tracking looks up callers of a function named after the file
    stem (`:36`).
- [x] **B26 The `ctx_checkpoint` diff is cut at about 8,000 tokens**, with
  no way to get the rest (`ctx_checkpoint.rs:137-149`).
- [x] **B27 `ctx_knowledge` export and import paths are wrong.**
  - The JSON export ignores `path` (`ctx_knowledge/mod.rs:80`).
  - Import and the OKF export resolve the raw path against the process cwd
    (`registered/ctx_knowledge.rs:197-204`).
- [x] **B28 `ctx_session` export and import are not in the schema.**
  - The handler reads `path`, `format`, `write` and `privacy`
    (`registered/ctx_session.rs:45-48`), but the schema does not declare
    them.
  - The action list omits export and import.
  - Both arms of `match privacy` are identical
    (`ccp_session_bundle.rs:107-116`), and `privacy=full` is rejected
    (`tools/ctx_session.rs:44-50`).
- [x] **B29 Errors come back as success.**
  - There is no `isError` in `ctx_refactor`, `ctx_expand`, `ctx_graph`,
    `ctx_impact`, `ctx_review`, `ctx_knowledge`, `ctx_outline`,
    `ctx_patch` CONFLICT, or the semantic search `ERR:` results
    (`impl_semantic_search/mod.rs:64,82,88`).
  - Unknown actions in `ctx_graph`, `ctx_knowledge` and `ctx_session`, and a
    missing path in `ctx_outline`, also succeed.
- [x] **B30 Resources and prompts are served but not advertised.**
  - The server answers `resources/list` (5) and `prompts/list` (3), but
    `initialize` advertises only tools.
  - The `sobre://context/pinned` resource always says "No pinned items"
    (`resources.rs:47`).

### Drift from the invariants

- [x] **B31 Merge `ctx_impact` into `ctx_graph` (decision 1).**
  - `ctx_graph` has build, status, impact and path
    (`tools/ctx_graph.rs:22-29,36,370,442`).
  - `ctx_impact` has build, status, analyze and chain
    (`tools/ctx_impact.rs:30-34,65,470,565,607`).
  - Both answer the same questions with separate code: 2 hops vs depth 5,
    path vs chain, and both build through `graph_index::scan`.
  - `ctx_impact`'s descriptions are wrong too: "diff compares git refs"
    (it compares the working tree with HEAD) and "chain traces from→to"
    (the call is `path="a->b"`).
- [x] **B32 Stop persisting `edit_quality.json` (decision 3).**
  - `edit_quality.rs:21-37` changes how `mode=auto` resolves on later
    runs (`docs/contracts/quality-loop-v1.md:135-157`).
  - `escalations_served` is written and never read.
  - FORK.md:50 says the learning loop was removed.
- [x] **B33 Remove the `ctx_knowledge` writes from the `edit_health` hook
  (decision 4).**
  - Its module doc says notices go to `additionalContext`, while the
    function doc says they never do.
  - It mentions "Phase-4 edit-gate… ctx_edit", the dashboard and
    ContextBus (`hook_handlers/edit_health.rs:3,36-37,48-50`).
- [x] **B34 Internal markers reach the agent.** Examples:
  `‹‹‹LCTX_PROJECT_KNOWLEDGE_<hash>›››` (ctx_knowledge status,
  ctx_compose), `[VERIFY] WARN(truncated_block=1) loss=14.3%` and
  `AGENTS:mcp-29069-67c9b934(context-engine)` (ctx_compose).
- [x] **B35 The MCP instructions contradict themselves.**
  - They give both "OUTPUT STYLE: dense … ≤200 tokens" and "CRP: compact
    … <=200 tok; trust tool outputs".
  - "trust tool outputs" contradicts the RECOVER line.
  - `list_tools` and the instructions use `CompressionLevel::effective`,
    including the per-session degrade (`server_handler.rs:198`), so the
    wording depends on session state. Invariant 3 only allows
    `compression_level` to change it.
- [x] **B36 The broad-root guard refuses explicit paths**
  (`tools/walk_guard.rs:19`).
  - In `ctx_tree`, `ctx_glob` and `ctx_search`, `path="~/notes"`,
    `"/usr/include"` or `"~/.config/sobre"` returns "ERROR: refusing to
    scan".
  - Its doc says it is meant for defaulted paths. Gate only those
    (invariant 5).
- [x] **B37 `ToolContext::resolve_path_sync` passes `shell_cwd=None`**
  (`tool_trait.rs:177`).
  - It is used for `ctx_patch` `ops[]` paths (`ctx_patch.rs:257,365`) and
    for `paths[]` (`multi_path.rs:95`), so those ignore the worktree rule.
- [x] **B38 Some calls read the project from the process cwd instead of the
  session's project root.**
  - `ctx_session load` without an id goes through
    `SessionState::load_latest` → `current_dir` (`ctx_session.rs:26`).
  - `render_aggressive` and `render_entropy` read the intent the same way.
- [x] **B39 The `ctx_patch` schema is wrong.**
  - It requires `start_hash`/`end_hash` for `replace_lines`
    (`ctx_patch.rs:59`), while the parser treats them as optional
    (`ctx_patch/anchors.rs:105-109`).
  - The messages "replace_unique/replace_symbol cannot be batched in
    ops[]" contradict the description and cannot be reached.
- [x] **B40 The `ctx_read` mode descriptions are wrong**
  (`registered/ctx_read.rs:48`).
  - `reference` is described as quotes; it is a one-line summary.
  - `diff` is described as a git delta; it is a diff against sobre's
    cached copy (see B20).
- [x] **B41 `ctx_search queries[]` searches only `roots[0]`**
  (`ctx_search.rs:542`).
- [x] **B42 `ctx_search` is annotated read-only and idempotent**
  (`tool_defs/mod.rs:18`), though `reindex` writes and the semantic action
  saves the session.
- [x] **B43 A client that declares the roots capability but never answers
  `roots/list` hangs the first `tools/call`** for over 30 s. There is no
  server-side timeout.

### Small issues

- [x] **B44 Stale texts.**
  - "lower the floor (memory.admission.min_salience)" (`remember.rs:60`;
    `min_salience` is always 0).
  - "Increase the limit" (`ctx_read/mod.rs:129`).
  - "Ledger cleared (0% pressure)" (`ctx_session.rs:229`).
  - `LCTX_MAX_READ_BYTES` in the `tests_windowed.rs` doc.
  - "IDE-first" (`ctx_refactor/mod.rs`).
  - ~~"read-only-roots deny" (`ctx_patch/mod.rs:14`).~~ Removed (#34).
  - "operator env pin" (`execute.rs:404`).
- [x] **B45 `ctx_shell background_action=cancel` answers
  `[background:X running]`** (`background_shell.rs:237-246`).
- [x] **B46 The first semantic search after `action=reindex` serves the old
  index.** `bm25_cache::get_or_background` serves the stale copy while it
  rebuilds (`impl_semantic_search/mod.rs:178`).
- [x] **B47 Terse compression may drop low-scoring source lines** from
  `ctx_search anchored=true`, `ctx_compose` and symbol-body output (not
  verified).
- [x] **B48 Footers differ.** Seen: `[sobre] full source:`,
  `[full output: ctx_expand(id=…)]`, and the README's `[sobre: …]`.
- [x] **B49 Wrong match count:** "4 matches in 8 files", where 8 is the
  number of files searched.

## Lot C — the CLI and the proxy

Fixed so far: #38 (doctor, colours, overhead), #39 (arguments, config,
proxy, help, `sobre call`).

- [x] **C1 `proxy start --port <N>` (with a space) is ignored** and the
  proxy listens on 4444. Only `--port=N` works; `stop` and `status` have the
  same problem.
- [x] **C2 `doctor --json` prints the coloured human report.**
  - The flag is only read with `--fix` or a subcommand
    (`doctor/mod.rs:451-455`).
  - Help advertises `doctor [--fix] [--json]`.
- [x] **C3 `config set` problems.**
  - It clamps values silently: `compression_level 7` prints "Updated …
    = 7" and stores 1.0.
  - `proxy.port 18999` fails with "invalid type: string, expected u16".
  - Without `--yes`, `proxy.upstreams.anthropic` says "Aborted" and
    exits 0.
  - `config set bogus` does not list the valid keys.
- [x] **C4 Usage errors exit 0.** Seen with `index bogus`, `proxy bogus`,
  `sobre -c` with no command, and `gain --bogus`. `gain --model=nonexistent`
  falls back silently.
- [x] **C5 Colours ignore `NO_COLOR` and non-TTY output** in doctor, init,
  config, proxy and the MCP stderr logs.
- [x] **C6 The health score contradicts itself.**
  - `doctor --fix` and `init --json` say "3/4 passed" (a missing
    `usage.jsonl` counts as a failure), while `doctor` says 20/20.
  - Four checks cannot fail but count toward the score:
    `secret_detection_outcome` (`checks/security.rs:105`),
    `memory_outcome` (`checks/storage.rs:182`, prints constants),
    `compact_format_passthrough_outcome` (`environment.rs:11`, prints
    `PRESERVE_COMPACT_FORMATS`), and `ram_guardian_outcome` (measures
    doctor's own RSS).
  - `mcp_server_cwd_outcome` (`environment.rs:234-241`) always passes,
    because doctor never runs inside the MCP server.
  - `checks/security.rs:123` computes a SHA-256 only to test that the
    binary is readable.
- [x] **C7 Overhead figures disagree:** `gain` says 4.6K, while `doctor`
  overhead says 4,944.
- [x] **C8 `sobre call` needs `--project-root` even inside a repo, and
  rejects `--project-root .`** as "refusing broad/unsafe"
  (`pathutil.rs:241-252`, `call_cmd.rs:166-169`).
- [x] **C9 Typo suggestions and bare `sobre`.**
  - There is no suggestion for `sobre inti`.
  - A bare `sobre` on a terminal says "waiting silently", then exits.
- [x] **C10 Help text** (`cli/help.rs`).
  - The banner says "sobre 0.1.0 — sobre, local context tools…", with the
    name twice (`:9,45,75`).
  - `config` is described as "Show or edit", but there is no editor
    (`:62`).
  - `uninstall` is described as "Permanent fix (removes all hooks)",
    though it removes data and the binary too (`:142`).
  - "SHELL HOOK PATTERNS" also apply to `ctx_shell` and `-c` (`:131`).
  - `proxy enable --force` is missing from `help all` (`:121-125`) and
    from `proxy --help` (`network/proxy.rs:77-95`).
- [x] **C11 `config init` overclaims.**
  - It claims a "full annotated reference… every key… and the
    environment variable that overrides it", but writes 5 of the 11 keys.
    Missing: `hook_binary`, `embedding_model`, `shell_timeout_secs`, and
    `proxy.enabled`/`port`/`upstreams`.
  - The environment-variable claim contradicts invariant 2.
- [x] **C12 Proxy behaviour.**
  - `GET /v1/responses` without auth returns 401; the 426 comes only with
    an auth header, and it has no `Upgrade` header.
  - `proxy status` and `proxy stop` assume port 4444 and do not remember
    the detached port.
  - On Windows, `proxy enable` exits 1 with "Proxy enable failed…",
    because autostart is not implemented there
    (`proxy_autostart.rs:133-138`, `network/proxy.rs:222-233`).

## Lot R — tool names

Fixed: #40. The lots done before this one keep the `ctx_*` names they were
written with; the lots after it use the new names.

- [x] **R1 Rename the MCP tools from `ctx_*` to `sobre_*` (decision 9).**
  - This touches the registry, schemas, hook matchers, instructions,
    prompts, SKILL.md, generated references, contracts and tests.
  - Do it after B31, so the merged graph tool is renamed only once.

## Lot D — dead code and cleanup

Fixed in #41 (dead code, write-only stores, decision 7), #42 (constant
settings, test-only code, dead modules), #43 (Cargo, stale files,
contracts), #44 (comments), #45 (duplicates, hashing, the rest of the
dead code) and #46 (the reachability sweep).

### Code that compiles but never does anything

- [x] **D1 About 244 `pub` items are unused from the binary's point of
  view** (reachability oracle baseline).
  - Every module is `pub mod` from `lib.rs`, so `dead_code = "deny"` never
    sees `pub` items.
  - Sweep them with the oracle; the items below are the ones confirmed by
    hand.
  - Done: the items below were already gone, and four oracle rounds
    (default and `dense`) reached a fixpoint. Besides single functions,
    methods and re-exports, the sweep removed:
    - three write-only stores: the context ledger (`context_ledger.json`;
      its pressure and eviction readers had no caller), the agent registry
      (`agents/registry.json`, written at start-up and on every call, read
      by nothing since the multi-agent briefing went), and the `@F1`
      labels in `sobre_overview`, which no tool could resolve
      (`context_handles`, `context_field`);
    - `pop_pruning`, `adaptive` (folded into `intent_engine`),
      `ThinkingBudget`, and the model registry
      (`data/model_registry.json`, whose context windows only the ledger
      read);
    - fields written and never read: `ToolOutput.mode` and `.path` (only
      the ledger read them), the terse attribution counters,
      `QualityScore` (the density gate now returns the output),
      `FileTouched.context_item_id`, `EvidenceRecord.agent_id` and a dozen
      more;
    - `DenseBackendKind`, an enum with one variant, and the `root` and
      `changed_files` parameters the dense path ignored;
    - the debug log's `Route`: only `sobre` was ever logged, so the log
      now records what hooks changed, and the `debug_log` key says so.
  - Test helpers (`test_env_lock`, `handle_with_task_resolved`,
    `end_burst`) and the docs generator (`reference_docs`, `mcp_manifest`)
    are behind `test-support`.
  - Kept on purpose: `ResolvedMode.source`, which the quality-loop golden
    test reads to check the escalation fires once, and the
    `SessionDegrade`, `EvidenceKind` and `join_command_for` re-exports the
    integration tests use.
  - Found while fixing: `sobre_shell` read a `bypass` argument (an alias of
    `raw`) that its schema does not declare, so no agent could know it.
    Removed.
- [x] **D2 Remove the non-PDF extractors (decision 7).**
  - Production calls `extractors::extract` only behind
    `is_binary_document()`, which matches `.pdf` only
    (`core/extractors/mod.rs:110-117`). The callers are
    `bm25_index/mod.rs:429` and `bm25_index/build.rs:139,372`.
  - Dead: the json, csv, eml, html and text branches (`mod.rs:46-99`),
    `json.rs`, `eml.rs`, `csv::to_text`, `csv::chunks` and
    `paragraph_chunks`.
  - Dead: `core/web/html_to_text.rs` (907 lines), and the fields
    `Extracted.kind` and `.chunks`.
  - Keep: the PDF path and `csv::parse`, which `tabular_crush` uses.
  - Update `docs/contracts/extractors-v1.md`.
- [x] **D3 `shell::exec` (`shell/exec.rs:435-462`) always compresses.**
  - `force_compress` is always true, so the `Verbatim && !force_compress`
    branch and the `if !force_compress {…}` block are dead.
  - Replace the `RAW`/`COMPRESS` globals in `core/runtime_flags.rs` with a
    mode argument.
  - `server/call_tool.rs:99` `runtime_flags::raw_enabled()` is always
    false inside the MCP server.
- [x] **D4 Unreachable branches.**
  - `tools/impl_dedup.rs:79-236` `analyze`: `handle_action` is only called
    with `"apply"`.
  - `context_ledger.rs:14-32`: the per-agent ledger path, since
    `agent_id` is always `"default"`.
  - ~~`client_capabilities.rs:29`: the hint branch of `detect_with_hint`.~~
    Removed (B57).
  - `bm25_index/mod.rs:667` `load_or_build_fast` (zero callers), hence
    `bm25_index_looks_stale_fast` (`:749`), the fast branch (`:775-810`)
    and `SENTINEL_SAMPLE_SIZE` (`:822`).
- [x] **D5 `pub fn`s with zero references.**
  - `memory_lifecycle.rs:379-419` `apply_replay_boost`, and the
    `REPLAY_*` consts (`:23-27`).
  - `paths.rs:82` `config_dir_member`.
  - `sanitize.rs:25` `neutralize_shell_content`.
  - `tools/server_paths.rs:98` `resolve_path_or_passthrough`.
- [x] **D6 Dead code hidden by `#[allow(dead_code)]`.**
  - `property_graph/path_id.rs:29,40` (`get`, `resolve`).
  - `graph_index/file_id.rs:12-100`.

### Data written and never read

- [x] **D7 Write-only stores.**
  - `graph_index/mod.rs:231` `ProjectIndex.interner` is rebuilt on every
    scan and never read. Delete it with `file_id.rs`.
  - ~~In `client_capabilities.rs:150-189`, `GLOBAL` is never read,
    `client-id.json` is written and never read, and `tier()` with the
    elicitation/sampling fields only feeds a log line.~~ Removed (B57).
  - `auto_mode_resolver.rs:8-56`: `SOURCE_COUNTS` is flushed to
    `auto_mode_sources.json` on every exit (`tool_lifecycle.rs:70`), and
    its only reader is `#[cfg(test)]`.
  - `knowledge_relations.rs:73-74,150,212-254`: edge strength and
    `decay_rate` are written and read only by test-only functions.
  - `consolidation_engine.rs:73` `emit_event`.
  - `proxy/ccr.rs:106-113` `register_compressed` fills a
    `RelevanceTracker` nothing in the proxy queries.
  - `tools/autonomy.rs:16` `large_output_hints_shown` is never read or
    written.
  - Found while fixing: with `analyze` gone, `core/codebook.rs` had no
    caller; with `SOURCE_COUNTS` gone, `tool_lifecycle::flush_all` flushed
    nothing. Both removed.

### Settings that always have the same value

- [x] **D8 `MemoryPolicy`.** Every caller uses the default.
  - `admission.enabled` is always true, so `knowledge/core.rs:208` is
    dead.
  - `min_salience` is always 0, so the salience floor (`core.rs:215-224`),
    `AdmissionResult::RejectedLowSalience` and the handler at
    `tools/sobre_knowledge/remember.rs:56-62` are dead.
  - `archetype_aware_decay` is always false (`memory_lifecycle.rs:144-148`,
    `stability_multiplier`).
  - `GotchaPolicy.retrieval_budget_per_room`, `default_decay_rate` and
    `category_decay_overrides` (`memory_policy.rs:96-98`) are never read.
- [x] **D9 Other constant options.**
  - `setup/mod.rs:49-55` `SetupOptions`: `non_interactive` and `yes` are
    never read, `fix` is always true, `skip_proxy` is always false.
  - `ConsolidateOptions.import_session` is always true
    (`consolidate.rs:57,118,190`).
  - `cli/dispatch/lifecycle.rs:31` `proxy_enabled` is always true.
  - `cli/call_cmd.rs:48` `exit_code()` always returns 2.
  - `*_fresh(note: None)` writers in `editor_registry/writers/install/*`.
  - `write_atomic_with_backup_checked(.., None)`.
  - `register_compressed(.., 0)`.
  - Found while fixing: `SetupOptions`, the `*_fresh` writers and
    `write_atomic_with_backup_checked` were already gone with lot A.

### Test-only code shipped in the library

- [x] **D10 Gate these behind `#[cfg(test)]`, or a `test-support` feature
  where the integration tests need them.**
  - `engine/mod.rs` `ContextEngine` (only `tests/suite/engine_call.rs` and
    `power_user_worksession.rs` use it).
  - `core/contracts.rs:6-90`.
  - `server/mod.rs:132,145` (`*_for_test`).
  - `server/registry.rs:119` `is_registered`.
  - `AgentRegistry::save` and `load_or_create`.
  - `shell::compress::compress_if_beneficial_pub`.
  - Single helpers:
    - `cache::set_read_count`;
    - `delta::crosses_threshold`;
    - `Config::missing_config_path`;
    - `content_chunk::extract_file_references`;
    - `ContextLedger::with_window_size`;
    - `diagnostics_store::severity_for`;
    - `hnsw::from_vecs`;
    - `knowledge::recall_by_category`;
    - `pathutil::canonicalize_secure_or_self`;
    - `property_graph::edge::with_metadata`;
    - `RelevanceTracker::with_config`;
    - `search_index::warm_blocking`;
    - `hooks::from_bash_to_native_path`;
    - `editor_registry::writers::write_config`.
  - Found while fixing: `write_config` is not test-only, `init` calls it
    (`setup/mod.rs:111`). The rest are behind `#[cfg(test)]`, or behind the
    `test-support` feature, which a self dev-dependency enables for the
    integration tests and benches only.

### Module structure

- [x] **D11 Dead modules.**
  - `core/mod.rs` has seven re-export modules with no users:
    `compression` (23), `memory` (45), `graph` (61), `context` (80),
    `knowledge_domain` (100), `search` (131) and `session_domain` (150).
  - Two files contain only a doc comment: `doctor/report.rs` and
    `core/bm25_index/coordinator.rs`.
  - `server_handler.rs:173-215` re-implements
    `tool_visibility::advertised_tool_defs_default()`.
  - `SobreServer.registry: Option<…>` is always `Some` in production.

### Cargo

- [x] **D12 Cargo.toml.**
  - The dev-dependency `wat` is unused.
  - The examples `gen_mcp_manifest` and `gen_tdd_schema` write to a
    `website/generated/` that never existed. `core/tdd_schema.rs` and
    `mcp_manifest::default_manifest_path` exist only for them. Keep
    `gen_docs`.
  - `dense` and `embeddings` are two names for one feature; the code only
    checks `embeddings`.
  - `exclude = ["examples/", …]` names a directory that does not exist.
  - Probably unused (verify): tokio `"process"` and reqwest `"json"`.
  - Stale comments:
    - pdf-extract "for ctx_url_read";
    - `struct_excessive_bools` "Config/StatsStore";
    - the `dead_code` upstream narration;
    - `format_push_string` "276 refactors".
  - The GPU features (`ort-cuda`, `ort-rocm`, `ort-webgpu`,
    `ort-directml`, `ort-coreml`) are enabled by nothing, so the GPU code is
    never compiled. Keep them as opt-in for source builds, or drop them;
    fix the claims either way (E2).
  - Done: `embeddings` is gone, `dense` is the feature the code checks; each
    GPU feature has provider code in `ort_execution_providers.rs`, so they
    stay as source-build opt-ins and now turn on `dense` themselves. Also:
    `default` listed `jemalloc` twice, the description said 16 tools, and
    the `tarpaulin` cfg had no coverage run behind it.

### Stale repository files

- [x] **D13 Files.**
  - `rust/LOCK_ORDERING.md`:
    - rows L3, L7, L8, L53 and L63 point at files that do not exist;
    - L20 and L47 are stale;
    - section 2 lists `SobreServer` fields that do not exist and omits
      `refetch_signals`;
    - `tests/suite/lock_ordering_check.rs` only checks code → doc.
  - `rust/.cargo/audit.toml` ignores advisories for crates that are not in
    Cargo.lock.
  - `.dockerignore` names a Dockerfile that does not exist.
  - `.gitattributes` names `rust/data/grammar_registry.json`, which does
    not exist.
  - `rust/.gitignore` lists `/eval-suite/`, `/ab-report.json` and `*.kt`.
  - `rust/tests/fixtures/mcp_stdio_addon.mjs` and `mcp_stdio_echo.mjs` are
    never loaded.
  - `rust/tests/pre_release_check.sh` is unreferenced and prints ✓ without
    checking.
  - `rust/.config/nextest.toml` filters on a module that no longer exists.
  - `.pre-commit-config.yaml` is a second hook system next to `.githooks/`,
    and runs clippy `--all-features`.
  - `scripts/loc-gate.sh` carries a dead allowlist, a frozen limit and
    ratchet code, and a "#660 Wave A/B" header.
  - `rust/scripts/` is never referenced.
  - `specs/` is empty upstream scaffolding.
  - Low priority:
    - `rust/data/model_registry.json`: `max_output`, `version` and
      `updated_at` are never read;
    - `Makefile test` duplicates preflight;
    - `.vscode/settings.json` has `npm.autoDetect`;
    - about 72 vendored test files in `crates/vendor/rmcp/tests` are never
      built.
  - Done, plus: `lock_ordering_check.rs` now also checks doc → code (every
    row names a file that still declares the lock); the vendored rmcp also
    loses its dev-dependencies and a `build.rs` that set `core.hooksPath`
    in a directory that does not exist.
- [x] **D14 Stale contracts** (`docs/contracts/`).
  - `knowledge-policy-contract-v1.md:24` cites a
    `MemoryPolicy::validate()` that does not exist.
  - The gotchas contract documents the dead `GotchaPolicy` fields.
  - Several cite the upstream tracker ("GitLab #2316/2317/2318",
    "GL #494", "EPIC 12.13").
  - `graph-reproducibility` (`:11,30,83`) names `$SOBRE_DATA_DIR` where it
    means the data dir.
  - `quality-loop` uses `old_string`, but the parameter is `old_text`.
  - `contracts.rs` calls the usage ledger "Frozen: externally consumed";
    nothing consumes it.

### Comments

- [x] **D15 Upstream references.**
  - 1,409 comment lines in 311 files cite upstream issue numbers
    (`GL #1140`, `(#271)`, `GH #669`…).
  - 14 test files are named after upstream issues or reporters
    (`bazsi_reported_scenarios.rs`, `daviddatu_powershell_scenarios.rs`,
    `issue_1088_*`, `*_476/554/594/595/596/669`, `p4_docker_*`).
  - Done: about 1,400 comment lines lose their references (scripted for
    the unambiguous cases, by hand for the rest), and the 13 test files are
    named after what they test.
- [x] **D16 Comments about code that no longer exists.**
  - `cli/dispatch/server.rs:1-2,68-70,86-88,142,396-401`, and
    `help.rs:1-2`.
  - `doctor/mod.rs:214-216,338-342`.
  - `core/artifacts_exec/binhash.rs:1-10`.
  - `core/agents.rs:183-189,241-248`.
  - `ipc/mod.rs:1-7`.
  - `cli/dispatch/network/mod.rs:6-8`.
  - `core/web/mod.rs:6`.
  - `core/code_health/astutil.rs:6`.
  - `cli/prompt.rs:3-8`.
  - `core/config/risk.rs:1-9`.
  - `core/mod.rs:92-93,120-121,175` (the TODOs point at a gitignored
    file).
  - `lib.rs:36-38`.
  - `server/tool_visibility.rs:1-6,13-15,42-43,55-58`.
  - `server_handler.rs:179-183`.
  - `server/registry.rs:9-10,104,107`.
  - `tools/server.rs:10-12`.
  - `config/enums.rs:1-6`.
  - `consolidation_engine.rs:6,21,47-49,97,155`.
  - `memory_policy.rs:116,120`.
  - `memory_lifecycle.rs:86`.
  - `tool_lifecycle.rs:34,91`.
  - `hebbian_cache.rs:1-8,172`.
  - `cache.rs:323`.
  - `hasher.rs:1-5`.
  - `aggressiveness.rs:1-10`.
  - `tools/sobre_read/mod.rs:299-301`.
  - `logging.rs:15-16`.
  - `session_token.rs:1`.
  - `auto_mode_resolver.rs:6-7,26-28,58-59`.
  - `core/usage/mod.rs:8`.
  - `ort_provision.rs:129`.
  - `benches/efficiency.rs:3`.
  - "PathJail": `server/tool_trait.rs:136,185`,
    `tools/sobre_refactor/mod.rs:345`, `tools/sobre_patch/mod.rs:255`,
    `core/pathutil.rs:93,397`.
  - "ctx_url_read": `Cargo.toml:177`, `proxy/compress.rs:16`, `core/web/pdf.rs:4`.
  - Names of removed tools left after lot R (`ctx_metrics`, `ctx_quality`,
    `ctx_symbol`, `ctx_semantic_search`, `ctx_multi_read`, `ctx_compress`,
    `ctx_preload`, `ctx_overview`, `ctx_md_render`, `ctx_call`,
    `ctx_compare`): `auto_mode_resolver.rs:7`,
    `code_health/{annotate.rs:53,mod.rs:36,score.rs:8}`,
    `cooccurrence.rs:210`, `git_signals.rs:6`, `session/playbook.rs:10`,
    `session/types.rs:29`, `server_handler.rs:156`, `tool_visibility.rs:56`,
    `registered/sobre_glob.rs:62`, `registered/sobre_read.rs:68`,
    `registered/sobre_search.rs:13,364`, `sobre_read/render.rs:50`,
    `LOCK_ORDERING.md:33`, and tests (`index_orchestrator.rs:1030`,
    `terse/mod.rs:102`, `intensive_benchmarks.rs`,
    `hn_hardening_scenarios.rs`, `sobre_read_lmd_md_raw.rs:6`).

  - Done, plus a sweep of the names of removed subsystems (dashboard,
    daemon, addons, gateway router, plugin system, cognition loop) in
    comments; the managed ONNX Runtime now lives in
    `<data_dir>/onnxruntime/<version>/` instead of under `addons/bin/`.

- [x] **D19 Found while fixing D16: more dead code.**
  - `consolidation_engine`: only `ConsolidateOptions::manual()` is ever
    built. `scheduled()`, `incremental_auto()`, `consolidate_latest`,
    `ConsolidationBudgets` and `ConsolidationOutcome` have no caller, so
    the budgets, the salience floor and the incremental watermark in
    `sobre_knowledge/consolidate.rs` never run.
  - The session `Playbook` is rendered at compaction but nothing ever adds
    an entry.

### Duplicates

- [x] **D17 Hashing.**
  - Three project-key hashes: `project_hash::hash_project_root`,
    `index_namespace::namespace_hash` (MD5) and `git/shadow.rs:259` (FNV-1a).
  - MD5 in `cache.rs:746`, `embedding_index.rs:432`,
    `index_namespace.rs:16`, `knowledge/ranking.rs:194` and
    `sobre_session.rs:159`, next to the BLAKE3 `hasher`.
  - `edit_io.rs:31` has an `md5` field that holds a BLAKE3 hash.
  - Three hex encoders, and two SHA-256 file hashers plus
    `ort_provision::sha256_bytes`.
  - Done: BLAKE3 everywhere a hash is a key or a fingerprint, MD5 and its
    dependency gone; `hash_project_root` used `DefaultHasher`, whose output
    is not stable across Rust versions. One hex encoder, one SHA-256 module.
    Found while fixing: the session-bundle import compared an MD5 of the
    project identity with the BLAKE3 the export writes, so it never matched.
- [x] **D18 Other duplicates.**
  - `compact_lines` is copied 10 times in `core/patterns/`.
  - `agents.rs` `mutate_persistent` duplicates
    `AgentRegistry::mutate_locked`.
  - The same process is registered twice (`tools/server_lifecycle.rs:88`,
    `server_handler.rs:140`).
  - Two sets of lifecycle defaults: `LifecycleConfig::default()`, used only
    by tests, and `MemoryPolicy`, used in production.
  - Two gotcha caps: `GotchaPolicy.max_gotchas_per_project` and
    `gotcha_tracker::MAX_GOTCHAS`.
  - Proxy liveness: the PID is parsed twice, and there are three probes.
  - `init_logging` and `init_mcp_logging` differ only by the default
    level.
  - Five private `now_unix` helpers.
  - The embeddings download builds its own HTTP agent.
  - Three different fallbacks when the data dir cannot be resolved.
  - Done. The data-dir fallbacks wrote into `.` (the user's project) or
    `.sobre`; without a data dir sobre now reads and writes nothing there.
    `LifecycleConfig::default()` is the production policy, so tests run on
    the server's numbers. The embeddings download no longer caps the whole
    transfer at five minutes.

## Lot F — release

- [x] **F1 Publish a `dense` binary next to the default one (decision 8).**
  - `release.yml` builds 5 targets with default features. Linux is static
    musl, which may not be able to load ONNX Runtime dynamically; check.
  - Checked: a static musl binary cannot load any library at run time
    (`dlopen` fails with "Dynamic loading not supported"); the same program
    built for glibc loads it.
  - Done: four dense builds next to the five default ones. Linux targets
    glibc 2.28 through `cargo zigbuild` (`x86_64` and `aarch64`), the floor
    of Microsoft's own ONNX Runtime builds; then Apple silicon and Windows
    x64. Intel macOS has none: Microsoft ships no ONNX Runtime for it.
  - Tested: the `x86_64` glibc 2.28 build needs nothing newer than
    `GLIBC_2.28`; `sobre embeddings provision` downloads the runtime and the
    model; `sobre_search` in `dense` mode ranks `login` first for "verify
    credentials when a user signs in".
- [x] **F2 Name the two binaries, and say in the release notes which one
  has embeddings.** Today `embedding_model` and `sobre embeddings
  provision` do nothing in every shipped binary.
  - Done: `sobre-<target>` and `sobre-dense-<target>`; the release notes
    open with what each has. A build without embeddings now names the
    `sobre-dense` build in every message that asks for embeddings.
  - Found while testing: search results were headed `rrf:` in every mode,
    but the score is RRF only for hybrid results (BM25 score or cosine
    similarity otherwise). Now `score:`.

## Lot E — documentation and install

### Factual errors

- [x] **E1 The build command misses the pinned toolchain.**
  - `cargo build --manifest-path rust/Cargo.toml` from the repository root
    does not use it, because `rust-toolchain.toml` is in `rust/` and rustup
    reads it from the cwd.
  - Cargo.toml has no `rust-version`, so an older toolchain fails with
    compiler errors.
  - Affected: `README.md:56-61`, `CONTRIBUTING.md:48`,
    `.pre-commit-config.yaml`. CI uses `working-directory: rust`.
  - Fix: `cd rust && cargo build --release`, and set `rust-version`.
  - Done: `rust-version = "1.99"`; the README and CONTRIBUTING run cargo
    from `rust/`. `.pre-commit-config.yaml` was already gone (D13).
- [x] **E2 "`--features dense` adds GPU providers" is false**
  (`README.md:63-68`, `rust/README.md:16`, `CONTRIBUTING.md:26`,
  `rust/CLAUDE.md`, the Cargo.toml comment).
  - `dense = ["embeddings"]`.
  - `embeddings status` says "sobre uses the GPU when ORT_DYLIB_PATH points
    at a GPU runtime" (`ort_execution_providers.rs:163`).
  - `:47` mentions a "shipped Linux/Windows GPU build" that does not
    exist.
  - Done: the docs say `dense` adds local embeddings; `embeddings status`
    says a GPU needs a GPU feature; the comment no longer names a shipped
    GPU build.
- [x] **E3 `Makefile:13` `$(sobre --version)` runs whatever `sobre` is on
  PATH.**
  - Done: it runs the binary it installed.
- [x] **E4 The agent lists omit VS Code.**
  - The lists (`README.md:77-79`, `help.rs:16,51-52`, `bug_report.md:12`)
    leave out `vscode`, which `init` accepts. The `claude-code` alias is
    accepted too.
  - FORK.md:48 says "agents beyond eight" were removed, but there are 9
    keys.
  - Done: the README, the bug template and FORK.md name the nine agents;
    `help.rs` already did.
- [ ] **E5 INVARIANTS.md contradicts `init`.**
  - It says "no per-agent init" (`:31`) and "init takes no option beyond
    which agent" (`:52`).
  - In fact there are `init <agent>`, `--check`/`--dry-run`, `--repair`,
    `--json` and `--agent`, an interactive proxy question that writes
    config and installs autostart, and the ~120 MB model question in dense
    builds.
  - Open: an `invariant:` PR (#48), which both maintainers approve.
- [x] **E6 `README.md:81` says `uninstall` removes "everything init
  wrote".** Document what it really removes once A3 is settled.
  - Done: the README's Uninstall section says what goes, including data,
    memory, models and the binary.
- [ ] **E7 "Parameters never change" does not hold across builds.**
  - `README.md:41-44` and `INVARIANTS.md:62-68` say so, but `sobre_search`
    gains `mode` in embeddings builds
    (`registered/sobre_search.rs:114-121`).
  - The generated reference shows the default build only.
  - Document that the list is fixed per build; with a published dense
    binary there are two lists.
  - The README says so (one fixed surface per build). INVARIANTS.md is in
    the `invariant:` PR (#48).
- [x] **E8 The proxy section is incomplete.**
  - `proxy enable` installs a login service:
    `~/Library/LaunchAgents/com.sobre.proxy.plist` on macOS,
    `~/.config/systemd/user/sobre-proxy.service` on Linux, and none on
    Windows.
  - The port is 4444 + (uid − 1000) % 1000 on Unix
    (`proxy_setup/util.rs:20-45`).
  - `--force` is undocumented.
  - Done in the README. `config-keys.md` now gives the per-user port.
- [x] **E9 SECURITY.md.**
  - It says paths escaping the root are refused (`:25-26`); false, sobre
    does not confine paths (`server_paths.rs:66-68`, `INVARIANTS.md`,
    FORK.md:65).
  - "Not released / no supported version / no users" is stale for 0.1.0,
    and so are `README.md:15-17` and `rust/README.md:8`.
  - "Archived" content does not exist; what exists is the 24 h tee store.
  - Done: SECURITY.md describes the real threat model (no path
    confinement, redaction, the 24 h store, the proxy's token); the
    "not released" notices are gone.
- [x] **E10 SKILL.md and the instructions.**
  - They call `full` "verbatim" and `raw` "exact bytes" (see B22).
  - "Poll the returned job_id": the real call is
    `sobre_shell(background_action="status", job_id=…)`.
  - `replace_unique(old_text,new_text)` reads like a function; it is
    `op="replace_unique"`. The anchored op names are never given.
  - Done: the skill names the `sobre_patch` ops and their fields, and the
    background `status`/`cancel` call. `full` and `raw` were already
    described as B22 left them.
- [x] **E11 Contracts.** The upstream tracker IDs and wording are covered
  in D14.
  - Done in D14.
- [x] **E12 FORK.md:55,67** name `lean-ctx call` and `lean-ctx -c`; check
  what they mean.
  - Checked: the removal table now says `sobre call`; `lean-ctx -t` is the
    upstream name of the removed shell hook, as the table's note says.
- [ ] **E13 INVARIANTS.md:152-153 says "No generated artefacts in the
  repository"**, yet `docs/reference/generated/*.md` is committed and
  drift-checked.
  - Open: the `invariant:` PR (#48).
- [x] **E14 CONTRIBUTING.md:31-32 says "git hooks run the same script"**,
  but the hooks are opt-in through `make setup-hooks`, which it never
  mentions.
  - Done: CONTRIBUTING says the hooks are opt-in and what each runs.
- [x] **E15 `compression_pattern.md:8` uses mypy as the example**, but a
  mypy pattern already exists.
  - Done: the examples are commands without a pattern.
- [x] **E16 `rust/README.md:4` "still called sobre"** is left over from the
  rename.
  - Done, with the stale `data/` line of the layout.

### Gaps for a new user

- [x] **E17 Rewrite the README in this order:**
  1. Install
  2. Connect agents
  3. Check it works
  4. Everyday use
  5. Configure
  6. Measure
  7. Proxy
  8. Where data lives
  9. Upgrade
  10. Uninstall
  11. Platform notes

  It must cover:
  - **First screen.** One sentence, the 9 supported agents, a
    three-command quick start; "What it is not" and the rules go below
    Install.
  - **Prebuilt binaries.** Five targets with `SHA256SUMS`; static musl on
    Linux, a zip on Windows; macOS unsigned, so Gatekeeper.
  - **Build prerequisites.** rustup (a distro cargo ignores the pinned
    toolchain), a C compiler (Xcode CLT, build-essential or MSVC), and a
    slow release build.
  - **`make install`.** It installs `~/.local/bin/sobre`; add that to
    PATH. The Windows equivalent too. The Makefile cannot pass
    `--features dense`.
  - **Install before `init`.** `init` writes the absolute binary path into
    every agent config (`portable_binary.rs:37-79`), so running it from
    `target/release` and then `cargo clean` breaks every agent. After
    moving the binary, run `sobre doctor --fix`.
  - **After `init`.**
    - The proxy question and the model question.
    - Restart the agent.
    - Verify with `sobre doctor`, `sobre doctor integrations`, the
      agent's MCP list (`/mcp` in Claude Code), `sobre call … --project-root
      "$PWD"`, `sobre -c "git status"` and `sobre gain`.
  - **What `init` writes, per agent**: a table, including backups.
  - **Where data lives.**
    - `~/.config/sobre/config.toml`.
    - `~/.local/share/sobre`: usage, sessions, knowledge, graphs,
      checkpoints.
    - `~/.local/state/sobre`: logs, the 24 h tee store, proxy.log.
    - `~/.cache/sobre`: models.
    - The `XDG_*` and `SOBRE_{CONFIG,DATA,STATE,CACHE}_DIR` overrides.
  - **`compression_level`** also adds an "OUTPUT STYLE" line telling the
    agent how tersely to write (`instructions.rs:36-57`).
  - **Config and tools.** Introduce `sobre config`, `config set`,
    `config init`, and link `config-keys.md` with the counting rule
    (`proxy.upstreams` is one key). Name the tools and link
    `mcp-tools.md`.
  - **Dense steps.**
    - Install with `--features dense`, then `sobre embeddings provision`
      (or `ORT_DYLIB_PATH`), then `sobre index build`.
    - On Intel macOS: Homebrew onnxruntime.
    - GPU features.
  - **Proxy.** Platforms, the login service, the port, API keys only,
    `status`/`disable`/`cleanup`, and an upstream example with `--yes`.
  - **Upgrade and uninstall.** Reinstall to the same path and restart the
    agents; run `uninstall --dry-run` first; data and memory are deleted;
    `cargo uninstall`.
  - Done: the README follows that order, with every point above.
- [x] **E18 Templates.**
  - The bug report should list VS Code and ask for build features,
    install method and `sobre doctor` output.
  - The feature request and PR templates should link INVARIANTS.md and
    ask which tool or key a change replaces.
  - Done.

### Found while writing the README

- [x] **E19 Install fixes.**
  - `init --help` omitted `--dry-run` and said `--json` implies `--repair`;
    any failure exits non-zero, not only under `--repair`.
  - Observe hooks were added once and kept the old binary path after a
    move, even through `init` and `doctor --fix`. They now follow it.
  - The Codex proxy wiring wrote `config.toml` with no `.sobre.bak`.
  - `uninstall` cleaned `~/.copilot/hooks/hooks.json` even when
    `COPILOT_HOME` had moved it.
  - Stale comments: the model download, Windsurf's hooks.
- [ ] **E20 Check on the real tools.**
  - Windows: `opencode.json` is read from `%APPDATA%\opencode`, but the
    plugin and the skill go to `~/.config/opencode`, and Zed's settings to
    `~/.config/zed`. One side is wrong; this environment cannot reach either
    tool's documentation.
  - `COPILOT_HOME` moves the Copilot hooks but not `mcp-config.json` or the
    skill: check what Copilot CLI honours.
