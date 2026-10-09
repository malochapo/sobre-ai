# Phase 1 audit — second pass

The second audit of sobre 0.1.0, at `4be6afd` (`develop` after PR #30, which
carried every fix of the first audit, [phase1-audit.md](phase1-audit.md)). It
looks for three things:

- what the first audit missed;
- what its fixes broke or left half done;
- whether the documentation describes every environment variable, config
  key, command and tool, with its behaviour and options, and whether the
  architecture is written down.

It also checks that no documentation is published outside the repository.

The audit combined eight passes:

- an end-to-end run of the release binary in a sandbox HOME, under
  `env -i`, with nine agents' configs as a user would have them, no
  `claude` on the PATH and no systemd user bus (`init`, `uninstall`,
  `doctor`, `config`, `call`, `-c`, hooks, the MCP server over stdio, the
  proxy);
- three read-only reviews of the 15 MCP tools: editing and shell
  (`sobre_patch`, `sobre_refactor`, `sobre_shell`, `sobre_expand`,
  `sobre_checkpoint`); reading and search (`sobre_read`, `sobre_search`,
  `sobre_glob`, `sobre_tree`, `sobre_outline`, `sobre_compose`); graph,
  review, session, knowledge and `gain`;
- a regression review of the lot A, C and E fixes (install, CLI, proxy);
- a regression review of the lot D fixes (dead code, duplicates, hashing);
- a documentation review: completeness of the reference, architecture,
  publishing;
- a cross-platform build: clippy and tests compiled for
  `x86_64-pc-windows-gnu` and `aarch64-apple-darwin`, default and dense, and
  the reachability oracle (every `pub` made `pub(crate)`) on both builds.

Most findings were reproduced; the rest say "likely" and why.

## How this file is kept

- **Branches.** This file lives on `chore/audit-2`. Every fix branch starts
  from it and is squash-merged back into it. `chore/audit-2` goes to
  `develop` once, at the end.
- **Tracking.** Each finding has an ID. The letters continue after the first
  audit's (A–F, R), so an ID names one finding across both files. When a
  finding is fixed, its box is ticked and the PR number added. A finding
  closed without a fix says why.
- **Tags.** *Regression* marks a defect a phase-1 fix introduced or left
  half done, with the first audit's ID. *Invariant N* marks drift from
  [INVARIANTS.md](../INVARIANTS.md).
- **References.** `file:line` references are relative to `rust/src/`
  unless the path says otherwise. They point at `4be6afd` and drift as the
  fixes land.

## Decisions

Taken on 2026-10-09: every recommendation was accepted, and decision 13
was added.

| # | Question | Decision |
|---|---|---|
| 1 | A stale anchor in `sobre_patch` is moved to another line with the same hash, silently (J2) | Refuse with CONFLICT and show the current lines. |
| 2 | `replace_unique`/`replace_all` fall back to a whitespace-insensitive match (G4) | Remove the fallback. On no exact match, the error shows the nearest candidate with its whitespace visible. |
| 3 | The syntax check refuses an edit that leaves a syntax error, and `validate_syntax=false` is not honoured everywhere (J13) | Never refuse: write the edit, then report the syntax errors it introduced (invariant 5). The parameter goes. |
| 4 | Each new MCP process reloads the last session, whose task then drives `mode=auto`; knowledge facts can switch a read to `map` (L7, L12) | A new process starts an empty session; `sobre_session load` restores one on request. Neither the session nor knowledge changes what `mode=auto` resolves to (invariant 2). |
| 5 | `sobre_shell`: exit 1 with output is not `isError`; sobre's own failures are not either (J8) | `isError` on any non-zero exit and on sobre's own failures, as the hosts' shell tools do. The output stays in the result. |
| 6 | Stores are keyed three ways: root + git remote or package name (knowledge, graphs), identity alone (search index), canonical path (checkpoints) (L17) | Key every store on the canonical project root. Two clones are two projects; a remote change moves nothing. |
| 7 | Handlers read arguments the schema does not declare: aliases, a lenient fallback, `languages`, `path_glob`, `backup`, … (K9, N3) | Remove all of them. A test checks that every argument a handler reads is declared. Anything worth keeping gets declared in the same PR. |
| 8 | Unknown flags are ignored by most commands; `uninstall --dryrun` uninstalls (H1); help and README miss flags (N5) | One declarative table of commands and flags drives parsing (unknown flag: exit 2), help, and a generated `docs/reference/generated/cli.md`. |
| 9 | The reference is incomplete (N2, N4, N8) | Generate `mcp-tools.md` with full schemas, actions and modes, and `env-vars.md` from a registry the code reads through. Extend `config-keys.md`. Write `docs/architecture.md` by hand. All plain `.md` in the repository. |
| 10 | `proxy enable` on Windows wires nothing and reports success (H4) | Say so: on Windows `proxy enable` refuses with a clear message and the README says the proxy is started by hand there. Autostart waits for someone who can test it. |
| 11 | `docs/phase1-audit*.md` are private notes in the repository, which the process rules forbid | Keep them until phase 1 closes, then delete them in the release PR; history keeps them. |
| 12 | `doctor --fix` rewrites MCP entries in the user's repository (H13) | Report only: `doctor` names the project file and the entry; the user edits it. sobre writes nothing into a repository. |
| 13 | Does `uninstall` need `--dry-run` (H1)? | No: it is removed. Someone who runs `uninstall` means it; it lists each change as it makes it. Any argument it does not know is still an error, since `sobre uninstall claude`, by analogy with `init claude`, would remove everything. |

## Plan

| Lot | Scope | Status |
|---|---|---|
| G | Regressions and half-done phase-1 fixes | done (#51, #52, #53, #54, #55) |
| H | The user's machine: `init`, `uninstall`, `config`, proxy wiring | done (#51, #56, #57, #58) |
| J | Editing and shell tools | done (#59, #60) |
| K | Reading and search tools | open |
| L | Graph, session, knowledge, review, `gain` | open |
| M | Code: dense-only code in the default build, leftovers, Windows lints | open |
| P | CI and release | open |
| N | Documentation: complete reference and architecture, written last | open |

Three findings went first, in their own PR (#51): H12 (a local user
could get commands run through the hooks' shared `/tmp` directory), H1
(`uninstall --dryrun` uninstalled) and G1 (`uninstall` left debris), with
G15 (`uninstall` left a proxy running), which sat in the same code.

E20 from the first audit stays open (see [below](#e20-still-open)).

## What already works

The end-to-end run found no crash and no panic. These behaved as
documented:

- **Basic CLI.** `--version`/`-V`; `help`, `--help` and `help all`;
  suggestions for typos (`inti` → `init`, exit 2).
- **`init`.**
  - Without a terminal, per agent, `--json` (valid JSON) and `--repair`;
    an unknown agent or flag exits 2; a broken config is reported and
    exits 1.
  - `--check` writes nothing, and a second run is byte-identical.
  - The terminal question writes `proxy.enabled`.
  - Comments, key order and the user's entries are kept; `.sobre.bak`
    files are made; new files are 0600.
- **`doctor`.** 15/15 and JSON; `overhead` matches `gain` (the C7 fix
  holds); `lint-context`; `--fix` after the binary moved.
- **`config`.** `set` ranges and types, `--yes` gating, `path`.
- **The 15 tools through `sobre call`.** `.env` redacted unless
  `raw=true`; anchored and dry-run patches; checkpoint snapshot, diff and
  restore; session and knowledge in the compose overview; `sobre_expand`
  slices and refuses `../`; `mode=dense` refused in the default build.
- **Shell.** `-c` labels its output on stderr and passes through file
  redirects; `raw`; exit codes propagate.
- **`gain` and `index`.** Every `gain` variant (a bad period, model or
  flag exits 2); `index build` and `status`.
- **Hooks.**
  - Claude Code, Copilot and OpenCode compression keep their reply shapes,
    each with a `[sobre: …]` label; `cat` and interrupted runs are left
    alone.
  - The read-dedup stub resets on a new session, a changed file and
    PreCompact.
  - Observe records the session; the Cursor and Gemini rewrites keep
    `description`/`dir_path` and honour Cursor's allow and deny rules.
- **MCP over stdio.** Protocols 2025-06-18 and 2024-11-05, `roots/list`,
  15 tools, instructions under 2,048 characters, -32602 and -32601
  errors, ping, malformed lines skipped, the re-read stub, `fresh=true`,
  background jobs, nothing stray on stdout.
- **Proxy.** `start -d --port N`, `status`, `token`, `restart`, `stop`;
  `/health` 200, `/status` 401 without auth, 426 on an Upgrade header; a
  request through a mock upstream comes back labelled, with the user's key
  forwarded and none added.
- **`uninstall`.** `--dry-run` changes nothing. A real run restored every
  agent file byte for byte (except H2), kept entries the user added after
  `init`, and removed the data directories and the binary.
- **Leftovers.** No `ctx_` or `lean-ctx` text in any output or in the
  binary; no ANSI codes off a terminal.

---

## Lot G — regressions and half-done fixes

- [x] **G1 `uninstall` leaves debris again after a second `init`.** (#51)
  *Regression: A4.*
  - `config_io::write_atomic_with_backup` (`config_io.rs:18-41`) keeps a
    `.sobre.bak` of any file it rewrites, including one sobre created on the
    previous run. `uninstall` then treats the file as the user's and keeps
    it (`uninstall/agents.rs:213-221,306-314`).
  - Reproduced: empty `~/.gemini` and `~/.codex`, `init gemini codex` twice,
    `uninstall`. Left behind: `~/.gemini/settings.json` holding `{}` and an
    empty `~/.codex/config.toml`. `init`, a moved binary, `init` and
    `uninstall` leave 12 files.
  - The round-trip test runs `init` once.
  - Fix: no backup of a file sobre created; a round-trip test that runs
    `init` twice and with a moved binary.
  - Done. A file sobre creates gets an empty `.sobre.bak`, so the next
    `init` keeps it and `uninstall` knows it may delete the file. The
    backup goes before the empty `hooks/` directory is pruned, which left
    `~/.copilot/hooks` behind. `tests/suite/init_uninstall_round_trip.rs`
    runs `init` twice, once more from a moved binary, then `uninstall`,
    over nine agents, and compares the whole HOME.
- [x] **G2 The observe-hook refresh takes over other tools' hooks.** (#52)
  *Regression: E19.*
  - `refresh_command` (`hooks/agents/mod.rs:13-26`) points every hook
    command containing `hook observe` at sobre (`hooks/agents/claude.rs:118`,
    `hooks/agents/codex.rs:56`, `hooks/agents/cursor.rs:28`, `hooks/agents/windsurf.rs:41`).
  - A user's `/opt/acme/bin/acme hook observe` becomes sobre's command, and
    `uninstall` then deletes it.
  - Fix: match sobre's own command only.
  - Done. One test, `hooks::is_sobre_hook_command`, decides what a sobre
    hook is: the program before ` hook ` is a binary named `sobre` (any
    directory, quoted or not, `.exe`, PowerShell's `& "…"`), or the
    configured `hook_binary`. The refresh, every host's installer and
    `uninstall` use it; they used five different substring tests, which
    also matched `/home/sobre/bin/acme hook observe`.
- [x] **G3 `config init` writes `[proxy]` under `[proxy.upstreams]`.** (#52)
  *Regression: C11.*
  - The live `[proxy.upstreams]` header comes first, then the commented
    `# [proxy]` keys (`core/config/render.rs:68-77`).
  - Uncommenting `# port = …` alone puts `port` in `[proxy.upstreams]`: the
    file no longer parses and every key falls back to its default, with a
    warning on stderr only.
  - Uncommenting `[proxy]`, `enabled` and `upstreams.anthropic`, as the
    header says to, gives "duplicate key": the live `[proxy.upstreams]`
    table already exists.
  - Fix: render `[proxy]` before its sub-table, and no empty live table; a
    test that uncomments each key, and each documented group, and parses
    the file.
  - Done. Every section has a live header, empty when nothing in it is
    set, placed before its sub-tables. An unset upstream is listed as
    `# upstreams.openai = …` under `[proxy]`, or as `# openai = …` inside
    `[proxy.upstreams]` when that table is there. An empty
    `[proxy.upstreams]` is no longer written, by `config init` or `sobre
    config`. A test uncomments each listed key alone, with nothing set and
    with an upstream set, and checks the file parses and the key took.
- [x] **G4 The whitespace-insensitive fallback of `replace_unique` and
  `replace_all` reports edits it did not make** (#53) (`tools/impl_edit.rs:257-276`,
  `:403-425`, `:643-664`). *Regression: B9 added it to `replace_all`.*
  - On a CRLF file it answers "1 replacement" and changes nothing.
  - `replace_unique` accepts a match without checking that it is unique
    among the whitespace variants.
  - `replace_all` replaces some occurrences only.
  - Decision 2.
  - Done. The fallback is gone; only line endings are still matched
    either way (an LF `old_string` on a CRLF file), which changes no
    text. With no exact match, the error quotes the nearest line with its
    whitespace (`"  let x = 1;  "`) and writes nothing.
- [x] **G5 `sobre_patch` failures lose `isError` when the path holds a
  space.** *Regression: B29.* `is_section_label`
  (`server/tool_trait.rs:138-141`) rejects any label with a space.
  (#53) Done: a `[path]` label may hold spaces.
- [x] **G6 Instruction files ignore `start_line`/`limit`.**
  *Regression: B3.* The window applied at `tools/registered/sobre_read.rs:182`
  is replaced by `full` at `:214`. `AGENTS.md start_line=3 limit=2` returns
  40 lines. (#54) Done: an instruction file is read whole only when the
  agent asked for no mode, no window and no aggressiveness.
- [x] **G7 Some windows on files over 4 MB still fail.** *Regression: B2.*
  (#54)
  - `BufRead::lines()` stops at the first invalid UTF-8 byte; the fallback
    `read_file_lossy` refuses the file as too large, and the error tells
    the agent to read it in windows, so it loops (`tools/sobre_read/mod.rs:194,252`).
  - A 5 MB log with one Latin-1 byte fails both `lines:1-3` and
    `anchored` with `start_line`.
  - Comma multi-ranges (`lines:1-3,9-12`) are never streamed.
  - Done. The streamed window reads bytes and turns an invalid UTF-8 byte
    into U+FFFD, as the whole-file read does, and drops a leading BOM
    (which also fixes the window half of K3). A streamed `lines:` now reads
    like the in-memory one: comma parts in the order given, `N` one line,
    `N-` to the end.
- [x] **G8 Shell output is condensed with no way back.** *Regression: B58;
  invariant 5.* (#55)
  - Terse post-processing (`server/post_process.rs:19-28`,
    `tools/registered/sobre_shell.rs:279`) runs after the decision to keep the
    full output. Failed commands, small outputs, background status and
    commands in `passthrough_commands` come back labelled
    `[sobre: condensed]` with no `sobre_expand` id.
  - `passthrough_commands` promises output "passed through uncompressed".
  - Fix: keep the full output whenever the text changes, and never touch a
    passthrough command's output.
  - Done. `sobre_shell` condenses its own output before it decides whether
    to keep the full one, so any condensed answer carries its
    `sobre_expand` id; the dispatch stage no longer touches shell output.
    Output of a command the policy protects (`passthrough_commands`, the
    built-in passthrough and verbatim commands) is never condensed.
- [x] **G9 A relative path follows a `cd` into another checkout.** (#53)
  *Regression: B37 extended it to `ops[]`.*
  - After `cd vendor/lib` in `sobre_shell` (a nested repository or a
    submodule), `sobre_patch path=src/lib.rs` edits `vendor/lib/src/lib.rs`
    (`core/path_resolve.rs:33-44,94-98`, `core/session/state.rs:277`).
  - Fix: follow the shell's directory only into a worktree of the same
    repository; print the resolved path in every edit result.
  - Done. The shell's directory wins only when it is another worktree of
    the project's repository: the same `.git` directory, or a `.git` file
    whose `commondir` leads there. A nested repository or a submodule is
    another repository, and relative paths stay in the project. (Edit
    results already name the file; the path is not printed in full.)
- [x] **G10 `sobre_session restore` can return another project's snapshot.**
  *Regression: B38 missed this path.* It filters snapshots on a substring
  of the process's working directory (`core/session/compaction.rs:394-420`,
  `tools/registered/sobre_session.rs:262`); `list` shows every project's.
  (#55) Done: `restore` without an id takes the latest snapshot of a
  session of this project; `list` shows this project's sessions.
- [x] **G11 Graph outputs print bare file names.** *Regression: B63 fixed
  `impact` only.* (#55) Done: project-relative paths everywhere, the
  symbol shortlist that asks for `<file>::<name>` included. `neighbors`, `path`, `related` and the `symbol` header
  still call `shorten_path` (`tools/sobre_graph_primitives.rs`,
  `tools/registered/sobre_graph.rs:108-118,238`).
- [x] **G12 Search and glob stop at `max_results` without saying so.** (#54)
  *Regression: B49 fixed the count wording only; invariant 5.*
  - 300 matches in 30 files come back as `20 matches in 2 files (3
    searched)`; "searched" counts the files read before the cap
    (`tools/sobre_search.rs:214,312,380`).
  - `sobre_glob max_results=3` says `3 files matched (walked 4)` when 6
    exist (`tools/sobre_glob.rs:70,115`).
  - The "not searched: hidden files, symlinks…" note appears only when
    nothing matched.
  - Done. The count reads `N of M files searched`; a search or glob that
    stops at `max_results` says so and how to get the rest; every answer
    ends with what the walk never looks at. The note no longer mentions
    `ignore_gitignore` (an undeclared parameter, decision 7): it says what
    was skipped for the walk that ran.
- [x] **G13 Persisted hashes use `DefaultHasher`.** (#55) *Regression: D17 moved
  the keys to BLAKE3 but not these.* `DefaultHasher` is not stable across
  Rust releases, so a toolchain update makes every stored hash mismatch.
  - Gotcha ids (`core/gotcha_tracker/model.rs:252`, matched at
    `core/gotcha_tracker/mod.rs:127,140,146`): known gotchas come back as new.
  - The graph index content hash (`core/graph_index/mod.rs:1112`, stored
    at `:886`, compared at `:562`) and the call graph
    (`core/call_graph.rs:462`, saved at `:172`, compared at `:131`): both
    rebuild.
  - Fix: `core::hasher`.
  - Done: all three use `hasher::hash_short` (BLAKE3).
- [x] **G14 Lot D left duplicates and a fallback into the project.** (#55)
  *Regression: D18 says both are gone.*
  - `graph_dir` falls back to `<project>/.sobre/` when the data dir cannot
    be resolved (`core/property_graph/mod.rs:30-43`); `CodeGraph::open`
    then creates `graph.db` in the user's repository. The embeddings
    `model_directory` falls back to a relative `models`
    (`core/embeddings/mod.rs:341-346`).
  - 11 private `compact_output` functions in `core/patterns/` (docker,
    kubectl, golang, pip, next_build, ruff, gh, eslint, pnpm, npm,
    playwright) are copies of `patterns::compact_lines`.
  - `core/session_token.rs:19` is a second hex encoder.
  - `proxy_autostart.rs:473` imports something unused in the Windows test
    build.
  - Found while fixing H12: the proxy's log directory falls back to
    `<tmp>/sobre` when the state dir cannot be resolved
    (`proxy_autostart.rs:285`, `cli/dispatch/network/proxy.rs:325`).
  - Done. `graph_dir` and `model_directory` fail without a data or cache
    dir instead of falling back; the proxy refuses to start without a
    state dir rather than log to `/tmp`. The pattern modules use
    `compact_lines`, the session token `hasher::hex_encode`, and the
    Windows test import is gated like its user.
- [x] **G16 `sobre_shell` and `sobre_read` inject context nobody asked
  for.** *Regression: B11 removed the AUTO CONTEXT prefix (phase-1
  decision 2), not this second injector.* Found while fixing G8. (#55)
  - Every saved shell output was indexed by keyword
    (`core/relevance_tracker.rs`, 551 lines). A later `sobre_shell` or
    `sobre_read` whose words matched got up to 2,000 tokens of those old
    outputs appended, under `--- PROACTIVE CONTEXT ---`.
  - Done. Removed, with the tracker and its three constants, as the AUTO
    CONTEXT prefix was: a tool answers what it was asked.
- [x] **G15 `uninstall` leaves a proxy running on a port other than the
  default.** (#51) *Regression: C12 left this path.* `uninstall` stops the proxy
  on `default_port()` (`uninstall/binary.rs:13-24`), not on the port
  recorded in `proxy.port` that `proxy stop` reads
  (`cli/dispatch/network/proxy.rs:28-52`), then deletes that file with the
  state dir. Reproduced: after `proxy start --port 18778 -d` and
  `uninstall`, `:18778/health` still answers.
  - Done. `uninstall` stops the proxy on the configured port and on the
    port the last proxy was started on; `proxy_setup` owns that record.

## Lot H — the user's machine

- [x] **H1 `uninstall` ignores arguments it does not know.** (#51)
  - `sobre uninstall --dryrun`, `-n`, `uninstall claude` or any typo runs a
    real uninstall and exits 0 (`cli/dispatch/mod.rs:109-120`).
  - `doctor bogus` and `doctor --bogus` run the full doctor and exit 0
    (`doctor/mod.rs:432-437`). `index`, `embeddings`, `proxy` and `config`
    also ignore unknown flags.
  - Decision 8. `uninstall` gets the fix first, whatever the decision.
  - Done for `uninstall` (the other commands follow decision 8): any
    argument but `--keep-config`, `--keep-binary` and `--help` exits 2
    and removes nothing. `--dry-run` is gone (decision 13).
- [x] **H2 Any localhost URL is taken for sobre's proxy.** (#56)
  - `is_local_sobre_url` (`proxy_setup/util.rs:5`,
    `proxy_setup/codex.rs:207-210`) matches every `localhost`/`127.0.0.1`
    URL.
  - Reproduced: `~/.claude/settings.json` points `ANTHROPIC_BASE_URL` at
    Ollama (`http://localhost:11434`); `init claude`, then `uninstall`
    prints "Removed ANTHROPIC_BASE_URL" and the URL is gone. The
    `.sobre.bak` is deleted too: `uninstall/mod.rs:71` rewrites the file
    before comparing it with the backup, so they never match.
  - `proxy status` warns that Claude Code "still points to the local
    proxy" and suggests `proxy cleanup`, which removes the URL as well.
    `doctor --fix` (`cleanup_stale_proxy_env`, `proxy_setup/mod.rs:98-135`)
    and the Claude subscription guard (`proxy_setup/claude.rs:140-160`)
    do the same, even when the proxy was never enabled.
  - Fix: match the exact proxy URL on sobre's port, and only a value sobre
    wrote.
  - Done, with H3. `proxy_setup/wiring.rs` keeps, in
    `<state dir>/proxy_wiring.json`, the URL sobre wrote into each slot
    (Claude Code's `ANTHROPIC_BASE_URL`, Codex's `openai_base_url`, each
    OpenCode provider's `baseURL`) and the value it replaced. A URL is
    sobre's only when it is the recorded one or, with no record, the
    proxy's address on its port. `init`, `enable` without `--force`,
    `cleanup`, `doctor --fix`, the subscription guard and `uninstall` leave
    any other value, local or not.
- [x] **H3 `proxy disable` and `uninstall` invent Claude's base URL
  instead of restoring it** (#56) (`proxy_setup/claude.rs:65-110,152-160`,
  `proxy_setup/mod.rs:67,109`).
  - Reproduced with the README's own Ollama example: after `config set
    proxy.upstreams.anthropic http://127.0.0.1:11434`, `disable` and
    `uninstall` print "Restored ANTHROPIC_BASE_URL → http://127.0.0.1:11434"
    and "Original endpoint restored". Claude's settings never held that
    URL.
  - sobre cannot tell an endpoint it saved with `--force` from one the user
    configured. `proxy cleanup` reports "Cleaned up 1 stale proxy URL(s)"
    on every run.
  - `config set proxy.upstreams.anthropic default` stores `""`, and
    `disable` then writes `ANTHROPIC_BASE_URL=""`.
  - Fix: record what sobre wrote and what it displaced, and undo exactly
    that.
  - Done (see H2): undoing puts back the value `--force` replaced, or
    removes the key; the configured upstream is never written into
    Claude's settings.
- [x] **H4 The proxy on Windows.** (#56)
  - There is no autostart on Windows. `proxy enable` skips the agent wiring
    because the proxy is not running, and still reports success.
  - Decision 10.
  - Done. Where there is no autostart, `proxy enable` refuses while
    nothing answers on the port, says to start the proxy by hand first,
    and changes nothing; `init`'s question does the same.
- [x] **H5 `claude mcp add-json` never succeeds.** (#58) sobre pipes the JSON on
  stdin; the CLI's usage is `claude mcp add-json [options] <name> <json>`,
  and it fails with "missing required argument 'json'"
  (`core/editor_registry/writers/mod.rs:166-230`).
  - The file edit always runs instead, after spawning the CLI for up to
    3 s on each `init`.
  - When the CLI exits 0, `init` reports `~/.claude.json` updated without
    checking it.
  - The README says the entry goes "through `claude mcp add-json` when the
    claude CLI is installed".
  - Fix: remove the CLI path; the file edit is the one writer, and the
    tested one.
  - Done: `init` writes `~/.claude.json` itself and never runs the CLI;
    the smoke test checks a `claude` on PATH is left alone.
- [x] **H6 `config set`** (`cli/config_cmd.rs:21-27`,
  `core/config/setter.rs:176-190`). (#57)
  - Setting a key to its default is ignored. With `compression_level =
    0.7` in the file, `config set compression_level 0.3` prints "Updated"
    and the file still says 0.7: the default is skipped when serialising
    (`core/config/mod.rs:85-89`) and `write_toml_preserving_minimal`
    (`config_io.rs:211`) keeps the old line.
  - Invalid upstreams are accepted: `127.0.0.1:11434`, `notaurl`,
    `ftp://x.y` (`validate_upstream_url` is not called). Each load then
    warns and uses the default, so requests meant for a local model go to
    api.anthropic.com, and `doctor` still scores `✓ Proxy upstream`.
    Port 0 is accepted too.
  - `config set proxy.upstreams x` answers "did you mean
    'proxy.upstreams'"; the key list it prints has 14 names for 11 keys.
    The risk review prints before an unknown-key failure
    (`proxy.upstreams.gemini`).
  - `default` stores `""` instead of removing the key, and nothing removes
    a key from the command line.
  - `sobre config` omits `compression_level` and the proxy keys, and shows
    an empty `[proxy.upstreams]`.
  - Done. `config set` edits the TOML document in place (comments kept,
    a removed key's comment moved, not lost). A value is parsed and checked
    as the loader checks it before any review: an upstream needs
    `http(s)://` and is stored normalized, the port is 1–65535. `default`,
    an empty value or the key's default removes the key, and an emptied
    `[proxy.upstreams]` with it. An unknown key is refused before the
    review, against the 12 keys `config set` takes (`proxy.upstreams` is
    named as a table). `sobre config` lists every key with the value in
    effect, `(default)` or `not set`. `doctor` fails on an upstream the
    proxy cannot use.
- [x] **H7 `init --check --json` looks like a real run.** Its JSON says
  "connected" and "created", with no dry-run field
  (`setup/mod.rs:200-226,255`). (#58)
  Done: the report carries `"dry_run": true`, an agent is
  `would_connect`, and each file reads "would create" or "would update".
- [x] **H8 The systemd unit pins no directories** (`proxy_autostart.rs:383-405`).
  The macOS plist pins `SOBRE_*_DIR` and `HOME`; the systemd unit sets only
  `RUST_LOG`. A proxy started by systemd can read another config and token
  than the CLI that set it up. Likely; not run under systemd. (#56)
  Done: the unit sets `HOME` and the four `SOBRE_*_DIR` this CLI resolves,
  and quotes the binary in `ExecStart` (a path with a space broke it).
- [x] **H9 Zed on macOS.** sobre writes
  `~/Library/Application Support/Zed/settings.json`; Zed reads
  `~/.config/zed/settings.json` on macOS, which is what `doctor` checks
  (`core/editor_registry/paths.rs:16-30`, `doctor/common.rs:167`). Likely;
  verify with E20. (#58)
  Done, after Zed's documentation: `~/.config/zed` on macOS,
  `$XDG_CONFIG_HOME/zed` on Linux, `%APPDATA%\Zed` on Windows. `init`,
  `doctor` and `uninstall` use the one path. A run on a real Mac stays
  with E20.
- [x] **H10 Windows: an npm-installed Claude Code is not detected.**
  `where claude` prints several lines, used whole as one path
  (`core/editor_registry/detect.rs:84-88`). Likely. (#58)
  Done: the first line is the path.
- [x] **H11 Smaller ones.** (#58)
  - The default proxy port truncates the uid to 16 bits. macOS uids start
    at 501, below the 1000 that marks a system user, so every macOS user
    gets 4444: the README's "two people on one machine get different
    ports" is false there.
  - `init --agent` with no value connects every agent and exits 0.
  - `uninstall --dry-run` always says "Would remove the proxy autostart
    unit", even when there is none.
  - `sobre call` exits 0 when the tool returned an error.
  - `index`'s usage text loses its indentation (`cli/index_cmd.rs:23-28`).
  - In the build without embeddings, `embeddings status` says to run
    `sobre embeddings provision`, which that build refuses (exit 1).
  - `sobre` with stdin closed before `initialize` exits 1 with an ERROR
    line.
  - Done:
    - the port is 4444 + uid mod 1000 on Unix, so 501 gets 4945, and no
      uid is truncated;
    - `init --agent` with no name exits 2;
    - the `uninstall --dry-run` line went with `--dry-run` (decision 13);
    - `sobre call` exits 1 when the server would set `isError`;
    - `index`'s usage keeps its indentation;
    - `embeddings status` in the build without embeddings says only
      that;
    - stdin closed before `initialize` exits 0, logged at debug level.
- [x] **H12 The hooks trust a shared directory in `/tmp`.** *Security.*
  (#51)
  - The Cursor and Gemini rewrite dedup and the read dedup keep their state
    in `<tmp>/sobre-hook/` (`hook_handlers/dedup.rs:146`,
    `hook_handlers/read_dedup.rs:286-291`). `create_dir_all` succeeds on a
    directory another user created, and the failed `chmod 0700` is
    ignored.
  - A fresh `<key>.claim` with a `<key>.resp` makes the hook replay the
    response verbatim. The key is predictable (BLAKE3 of
    `rewrite\0Cursor\0git status`). Reproduced: a planted response made
    `sobre hook cursor` rewrite `git status` into `echo PLANTED`.
  - On Linux `TMPDIR` is usually unset, so the path is fixed: a local user
    who creates `/tmp/sobre-hook` first can get commands of their choice
    run by another user's Cursor or Gemini.
  - The directory is outside the documented data layout and survives
    `uninstall`.
  - Fix: keep it under sobre's state dir (owner-only, like the rest), and
    remove it on uninstall.
  - Done. The hooks share `<state dir>/hooks/dedup` and
    `<state dir>/hooks/read`, mode 0700; `uninstall` removes them with the
    state dir.
- [x] **H13 `doctor --fix` rewrites files in the user's repository.** (#58)
  - Reproduced on a repo with a committed JSONC `.cursor/mcp.json`
    (comments, a `team-db` entry and `sobre`) and `.vscode/mcp.json`:
    `doctor --fix` printed only "All 4 checks passed". `git diff` shows
    the `sobre` entries removed, the comments stripped, everything
    reformatted, the trailing newline dropped, and no backup
    (`doctor/workspace_scope.rs:147-188`).
  - `.zed/settings.json` is flagged but never fixed; a `user-sobre` key
    name is looked for that sobre no longer writes.
  - The README says "never into a repository"; A5 made every edit
    in place.
  - Decision 12.
  - Done: `doctor --fix` no longer touches workspace files. `doctor`
    still reports sobre declared both in a repository and for the user,
    and says to remove the repository entry by hand; the README says
    `--fix` never edits a repository.
- [x] **H14 `doctor` says things that are not true.** (#57, #58)
  - Codex: "hooks auto-compress when trusted (/hooks)"
    (`doctor/integrations/codex.rs:130`); sobre installs only observe hooks
    for Codex, and the README says Codex gets no shell compression.
  - Claude: "Read redirect … sobre_read + Grep/Glob still compress"
    (`doctor/integrations/editors.rs:139`); there is no redirect, and no
    hook touches Grep or Glob.
  - A `config.toml` that does not parse prints its warning twice on every
    command, with "Run `sobre doctor --fix` to repair"
    (`core/config/loader.rs:102`, `cli/config_cmd.rs:135`,
    `core/config/mod.rs:171`). `doctor --fix` never touches the file,
    says "All 4 checks passed" and exits 0, while `doctor` shows the
    failure and exits 1. `config init` refuses to overwrite it and exits 0.
    (#57) Done for the config: said once, telling the user to fix the
    file by hand; `config init` exits 1.
  - `--fix` counts 4 checks, `doctor` 15: a C6 leftover.
  - Done: Codex reads "sobre installs only observe hooks for Codex",
    with shell output compressed through `sobre_shell`; Claude's check is
    "Native Read" and says what the read-dedup hook does and does not do;
    `--fix` says "4 quick checks passed (sobre doctor runs them all)".
- [x] **H15 `proxy enable` and `disable` leave state behind and report
  what did not happen.** (#56)
  - Without a systemd user bus, `proxy enable` prints "managed service did
    not acquire port 4444" (the cause is "Failed to connect to bus") and
    exits 1, yet leaves `proxy.enabled = true`, the unit file and its
    `default.target.wants` link.
  - `init` on a terminal, answered `y`, hits the same failure and prints
    "Proxy enabled.": `cli/init_cmd.rs:96-107` ignores what `install()`
    returns.
  - `proxy disable` (the README: "stop it") leaves a proxy started by the
    MCP server answering `/health`, and prints "Original endpoint
    restored" when nothing was.
  - `uninstall` leaves an empty `~/.config/systemd/user`.
  - Done. `proxy enable` and `init` record `proxy.enabled = true` only once
    the proxy runs; a unit that could not start is removed with its wants
    link and the directories it leaves empty, and the error says what
    systemd answered. `proxy disable` also stops a proxy started by hand
    or by an MCP server, and says whether any agent was taken off it.

## Lot J — editing and shell tools

- [x] **J1 `insert_after` and an edit of the next line hit the wrong line.** (#59)
  In one `ops[]`, `insert_after` line L plus `set_line`, `replace` or
  `delete` on L+1 edits a different line depending on the order
  (`tools/sobre_patch/apply.rs:373-379`). Fix: at equal positions,
  replacements before inserts.
  Done, with a test that both orders give the same file.
- [x] **J2 A stale anchor is moved silently.** (#59) *Invariant 5.* When the
  anchored line changed, sobre searches the whole file for a unique line
  with the same 16-bit hash and edits there: no CONFLICT, no label
  (`tools/sobre_patch/apply.rs:86-97,167-174,301-312`). Decision 1.
  Done: a stale anchor is a CONFLICT for every op, with the current lines
  and their fresh anchors; the relocation code is gone.
- [x] **J3 `replace_symbol` and `sobre_refactor` edit a stale range.** (#59)
  They take the symbol's lines from the graph without checking it is
  current (`tools/sobre_refactor/mod.rs:189-252`,
  `core/graph_provider.rs:351` `open_best_effort`) and do not reindex
  after writing. After one edit, a second `replace_symbol` overwrites the
  wrong lines. Fix: resolve the symbol with tree-sitter at edit time.
  Done: the index only names the candidate files; each is parsed again,
  so the span is the file's own at edit time.
- [x] **J4 `replace_symbol` with `path`+`line` replaces only `line` to
  `end_line`** (#59) (`end_line` defaults to `line`), with no symbol lookup
  (`tools/sobre_refactor/mod.rs:268-281`). The schema says it resolves the
  symbol.
  Done: `path`+`line` edits the innermost symbol around `line`; `end_line`
  is gone from both schemas.
- [x] **J5 An anchored edit turns a mixed-EOL file into CRLF**
  (`tools/sobre_patch/apply.rs:38-47`), unlabelled. (#59)
  Done: each line keeps its own ending; a new line takes its neighbour's.
- [x] **J6 An escalated read is recorded as fully delivered.** (#59) When an edit
  escalates to a read, sobre sends 300 lines but records the whole file as
  delivered (`tools/impl_edit.rs:368-399`). The next full read returns
  the "unchanged" stub and the agent never sees the rest. `replace_all`
  reaches this since B9.
  Done: only a complete escalation is recorded; a partial one says which
  lines it shows.
- [x] **J7 Non-ASCII commands get ids `sobre_expand` refuses.** The id slug
  keeps Unicode letters (`shell/redact.rs:25-35`); the resolver accepts
  ASCII only (`proxy/ccr.rs:124-138`). (#60) Done: the slug is ASCII.
- [x] **J8 `isError` in `sobre_shell`** (`server/call_tool.rs:359-379`).
  - Exit 1 with output is not an error; exit 1 without output is.
  - sobre's own failures (a missing `cwd`, an unknown background job) are
    not errors.
  - Decision 5.
  - Done (#60): any non-zero exit sets `isError` and `exitCode`, and the
    text, output included, is unchanged; sobre's own failures exit 1, so
    they are errors too. `sobre call` follows the same rule.
- [x] **J9 Background job ids come from the command text**
  (`server/background_shell.rs:84-131,186-200`).
  - Starting a command again while the first run is going returns the
    first run's output.
  - A foreground run of the same command deletes the background result.
  - Same class as B7. Fix: one id per launch.
  - Done (#60): `shell_<pid>_<n>`, one per launch.
- [x] **J10 `sobre_checkpoint`** (`core/git/shadow.rs:136-172,264-279`).
  - `restore` runs `git checkout <ref> -- .` with no snapshot first: every
    change since the checkpoint, the user's included, is lost with no way
    back.
  - The default excludes (`*.lock`, `build/`, `dist/`) are silently not
    restored.
  - `diff` ignores `to` without `from`, and by default compares the
    worktree with HEAD; the schema says `HEAD~1..HEAD`.
  - Fix: snapshot before `restore`; say what was not restored; follow the
    schema.
  - Done (#60): `restore` checkpoints the tree first and names that
    checkpoint as the undo; it lists the excludes and says files created
    since are left in place. `diff` runs from `from` (default: the last
    checkpoint) to `to` (default: the working tree), and the schema says
    so.
- [x] **J11 `sobre -c` kills the command.** *Invariant 5: the wrapped
  command must run as the original would.*
  - It kills the command past 512 KB of stderr, 8 MB of stdout or the
    2-minute default timeout (`shell/exec.rs:85-93,225-229`).
  - The label says "truncated", not "killed", and the exit code is 1, not
    124.
  - It clears `BASH_ENV` and `ENV`.
  - Fix: past the cap, keep draining and drop the excess, labelled; no
    timeout of its own (the host has one); keep the environment.
  - Done (#60):
    - output past the cap is read and dropped, with "N more bytes of
      output not kept … the command ran to the end";
    - there is no timeout and no kill;
    - `BASH_ENV`/`ENV` are left as they are;
    - the command stays in sobre's process group, so the host's own stop
      reaches it;
    - a background process left holding the pipe no longer holds the
      result past 2 s, and the result says so.
- [x] **J12 Hook commands break when the binary path holds a space.**
  - Claude, Cursor, Gemini, Codex and Windsurf hooks are built as
    `format!("{binary} hook …")`, unquoted (`hooks/agents/claude.rs:166-168`,
    `hooks/agents/cursor.rs:62,67`, `hooks/agents/gemini.rs:33-42`,
    `hooks/agents/codex.rs:11`, `hooks/agents/windsurf.rs:7`).
    Copilot and OpenCode quote it.
  - Reproduced with a HOME holding a space: the hook command fails with
    exit 126. The Cursor and Gemini rewrite
    (`hook_handlers/rewrite.rs:114-122`) turns `git status` into a command
    that fails the same way, so the agent's own command stops working.
  - `doctor integrations` then reports a stale binary at the path's last
    word, and `init --repair` changes nothing. A Windows user named
    `Jane Doe` hits this.
  - From reading the rewrite: `\"` is not PowerShell's escape, and `$x`
    inside single quotes gets expanded.
  - Fix: quote the binary in every hook and in the rewrite; parse quoted
    paths in `doctor`.
  - Done (#60):
    - every hook command quotes a binary path that needs it: single
      quotes on Unix, double quotes on Windows;
    - the Cursor/Gemini rewrite does the same on Unix;
    - on Windows, where the host's shell may be cmd or PowerShell, a
      command or binary path that needs more than plain double quotes is
      left as written;
    - `doctor` reads hook commands from the file's JSON strings and
      unquotes the program.
- [x] **J13 The syntax check blocks writes.** (#59) *Invariant 5.*
  - An edit that leaves a syntax error is refused.
  - `validate_syntax=false` is not honoured by `replace_symbol` and
    `sobre_refactor` (`tools/sobre_refactor/mod.rs:391`,
    `tools/sobre_patch/symbol.rs`), and is undeclared for anchored ops.
  - Decision 3.
  - Done: every write path (anchored ops, `replace_unique`, `replace_all`,
    symbol edits, dry runs) writes, then reports "⚠ syntax: this edit
    leaves a syntax error near line N". `validate_syntax` and the
    brace-balance count are gone.
- [x] **J14 Smaller ones.** (#59, #60)
  - `ops[]` batches are not atomic; the schema says "batch-atomic".
  - `timeout_ms` is described as the command's lifetime; it is enforced as
    idle time. Done in #60: the schema and `shell_timeout_secs` say
    "without new output", with the 1-hour ceiling.
  - `allow_lossy_utf8` rewrites the whole file with U+FFFD, unlabelled, and
    the diff hides it. The error that would need it does not mention it.
  - `agent_runtime_env::capture` never refreshes its timestamp, so the
    2-hour expiry hits during a session. Done in #60: a capture a
    quarter of the TTL old is rewritten.
  - `sobre_refactor` checks for the language server before the action: an
    unknown action answers "pylsp not found".
  - `sobre_patch`'s not-found error names `old_string` for the parameter
    `old_text`, and its list of ops leaves out `replace_unique`,
    `replace_symbol` and `replace_all` (`tools/sobre_patch/anchors.rs:91`).
  - Done in #59 for `sobre_patch` and `sobre_refactor`:
    - `ops[]` runs in order and stops at the first failure, saying what
      was not applied; one file's anchored ops stay all-or-nothing, and
      the description says so;
    - `allow_lossy_utf8` is gone: a file that is not UTF-8 is refused,
      saying why;
    - messages name `old_text`/`new_text` (`find`/`replace` for
      `replace_all`);
    - the op list is complete;
    - an unknown `sobre_refactor` action is refused before any language
      server starts.

- [x] **J15 Compressed `git log --stat` states wrong totals.** (#60) On this
  repository's `git log -n 40 --stat`, the hook prints `[40 commits,
  +0/-119 total]`; the real totals are +19,855/−116,368.
  `compress_log_summary` (`core/patterns/git/log.rs:250-258,288-293`)
  counts lines that start with `+` or `-`, which in `--stat` output are
  bullet lines from commit messages. Fix: read the "insertions(+),
  deletions(-)" lines, or print no total.
  Done: the totals are the stat summary lines' sum; with only a diff, only
  the diff's hunk lines count.

## Lot K — reading and search tools

- [ ] **K1 A re-read of a changed file returns a diff against content the
  agent never had.**
  - `resolve_explicit_delta_mode` checks only that the full content is
    cached, and every read caches it (`tools/sobre_read/mod.rs:653-669`).
    `mode=auto` takes the same path (`core/auto_mode_resolver.rs:55`).
  - `signatures`, an edit, then `full`: the agent gets a 2-line diff. The
    same after `lines:1-3`, or in a new conversation after `/clear`.
  - Fix: a diff only when this conversation received the full content
    (`is_full_delivered` and `conversation_allows_stub`), in both places.
- [ ] **K2 An explicit `lines:N-M` can return the whole file, unlabelled.**
  Whether the file was delivered is not scoped to the conversation, and the
  delta decision runs before the compaction sync
  (`tools/registered/sobre_read.rs:229-244` before `:295-300`). Conversation B
  asking `lines:5-7` after conversation A read the file gets 302 lines.
- [ ] **K3 Files that start with a UTF-8 BOM.**
  - The cache hashes the bytes with the BOM (`core/cache.rs:716`), the
    store without it (`tools/sobre_read/mod.rs:144-147`). The entry is always
    stale: an explicit re-read returns "changed" with "(no changes)", and
    the requested lines never come.
  - A window read and `sobre_search anchored=true` hash line 1 with the BOM
    (`tools/sobre_read/mod.rs:194`, `tools/sobre_search.rs:255`); a full anchored read and
    `sobre_patch` without it. A `sobre_patch` with the window's hash gets
    CONFLICT.
  - Fix: strip the BOM in all three places.
- [ ] **K4 Under lock contention `sobre_read` runs a second copy of its
  core** (`tools/registered/sobre_read.rs:375-790`), which differs.
  - `diff` is unknown there: `[WARNING: unknown mode 'diff', falling back
    to full]` and the whole file (`tools/sobre_read/render.rs:313`). The
    `auto`→`diff` path of K1 lands there too.
  - It has no disk window, so windows on files over 4 MB fail (likely).
  - Non-full modes do not clear `full_content_delivered`.
  - Fix: one core function for both paths.
- [ ] **K5 The fast path holds the global cache write lock during the whole
  render** (`tools/registered/sobre_read.rs:343-372`). A 3.3 MB `entropy` read
  held it 27 s in a debug build; a concurrent read of a 50-line file failed
  with "cache lock contention… retry in a moment".
- [ ] **K6 `raw` is not raw on three paths.** The schema says "the bytes on
  disk: no framing, no redaction".
  - In a batch, `mode:"raw"` is redacted (`tools/registered/batch_read.rs:28-35`,
    `tools/sobre_read/mod.rs:869`); the batch refuses `raw:true` but not this.
  - `raw:true` with `start_line`/`limit` returns a numbered, redacted
    window (`tools/registered/sobre_read.rs:1008-1012`).
  - The degraded-compression note is appended to raw output too
    (`server/call_tool.rs:114-116`; from reading).
- [ ] **K7 Batch `queries[]` ignores the top-level `include`, `exclude` and
  `exclude_pattern`** (`tools/registered/sobre_search.rs:564`).
- [ ] **K8 `find_related` fails in its default form.**
  - It reads the raw `file_path`, not the resolved path, while the index
    stores relative keys: an absolute path gets "no indexed chunk found"
    (`tools/registered/sobre_search.rs:417`).
  - In the default (non-compact) output, `top_k` counts output lines, and
    the self-exclusion never matches (`tools/impl_semantic_search/mod.rs:234-235`):
    `top_k=3` returned only the source chunk's own header.
- [ ] **K9 `sobre_search` honours parameters `tools/list` does not show.**
  *Invariant 3.* `languages`, `path_glob` and `ignore_gitignore`
  (`tools/registered/sobre_search.rs:205,318-319,536`); `languages:["python"]`
  changes the results. Decision 7.
- [ ] **K10 Secrets are masked by some tools and not others.** For the same
  `api_key = "sk-ant-…"`, `sobre_read` and a regex search show
  `[REDACTED:…]`; `action=symbol` (`tools/impl_symbol.rs`), the semantic
  search (`core/bm25_index/chunking.rs:268`) and `sobre_compose`'s symbol
  bodies (`tools/sobre_compose.rs:468`) print it.
- [ ] **K11 `usage.jsonl` overstates savings.**
  - `sobre_tree` computes its "original" with a second walk that skips
    neither vendor directories nor git-ignored files
    (`tools/sobre_tree.rs:36,118-147`): `node_modules` counts as saved.
  - A windowed `sobre_read` records the whole file as the original
    (`tools/registered/sobre_read.rs:811-812,936`): `lines:1-3` logs 1,800
    tokens original, 51 sent.
- [ ] **K12 `sobre_tree` counts and labels.**
  - Directories at the depth limit show `(0)`.
  - A directory holding only `.env`, and any `depth:0` call, say "(empty
    directory)".
  - Nothing says that hidden, ignored or deeper entries were left out.
    *Invariant 5.*
- [ ] **K13 `sobre_outline` on a directory caps before it filters.** The
  600-file cap applies before `match`/`kind`
  (`tools/sobre_outline/dir.rs:24,75`): a symbol in file 651 gives "No
  symbols matching", with no note.
- [ ] **K14 Glob `*` crosses `/`.** `src/*.ts` matches
  `src/a/b/c/f0.ts` in `sobre_glob` (`tools/sobre_glob.rs:89`) and in
  `sobre_search`'s `include`/`exclude` (`tools/sobre_search.rs:574`). Fix:
  `require_literal_separator`.
- [ ] **K15 `lines:N` means two things.** On a normal file it is one line
  (`tools/sobre_read/render.rs:421`); `anchored:N`, files over 4 MB and the
  parser's own doc read it as N to the end (`tools/sobre_read/mode.rs:192`).
- [ ] **K16 B62's guard compares the raw path argument**
  (`tools/registered/sobre_search.rs:350`). With a different case or `\` on
  Windows, or `/var` against `/private/var` on macOS, it misses and the
  symbol lookup moves to the nearest git root. Likely.

- [ ] **K17 `sobre_read paths=[…]` does not name the files.** Full
  sections are joined with `---` under "Read 2 files", with no path; only
  stubs carry one (`tools/impl_multi_read.rs:69`).

## Lot L — graph, session, knowledge, review, `gain`

- [ ] **L1 The Rust call graph misses path-qualified calls.**
  - A `scoped_identifier` callee is stored whole (`util::check`,
    `Config::load`), symbols are stored bare
    (`core/deep_queries/calls.rs:297-301`).
  - `callers`, `callees` and `trace` miss every such call; `sobre_review`
    finds no callers in other files.
  - `sobre_graph symbol=Config::load` splits on `::` as if it were a file
    (`tools/registered/sobre_graph.rs:131`).
  - Reproduced on a small crate: `callers helper` finds none while
    `callees add` lists `util::helper`; `trace main→helper` finds no path.
    `sobre_review src/lib.rs` says "no file depends on it" and, in the same
    output, lists callers in `src/main.rs` and `tests/basic.rs`; it finds
    no related tests, and the checklist says "No tests found".
  - Imports through the crate's own name (`use demo::{add, Counter}`) are
    not edges either: `neighbors src/lib.rs` shows no incoming file.
- [ ] **L2 The graph is never refreshed after its first build in a
  process.** `open_or_build` (`core/graph_provider.rs:370`) and
  `open_graph_fresh` (`tools/sobre_graph_impact.rs:97-112`) rebuild
  only when the graph is empty or of another version; `index_looks_stale`
  (`core/graph_index/mod.rs:359-480`) is never what decides.
  `impact`, `changes` and `callers` answer from stale data, unlabelled.
- [ ] **L3 The first graph call races the background warm-up**
  (`server/dispatch/mod.rs:39-55`, `core/graph_index/mod.rs:615-627`).
  `build` can print "0 files… Index saved" and `callers` "0 edges"; the
  empty call graph is then saved.
- [ ] **L4 `sobre_session reset`, `load` and `import` drop the project
  root, the shell directory and the extra roots**
  (`tools/registered/sobre_session.rs:21-40,211-216`,
  `core/ccp_session_bundle.rs:190-205`; `extra_roots` is
  `serde(skip)`). Reads then resolve against `/`, and `search_roots` stops
  applying.
- [ ] **L5 One absolute read moves the session to another root.** The
  session's task and findings are relabelled to the new root and saved
  over (`server/call_tool.rs:295-312`, `tools/server_paths.rs:31-49`),
  with no note.
- [ ] **L6 Concurrent first calls race `roots/list`**
  (`server/call_tool.rs:216-221`). `roots_resolved.swap(true)` lets the
  other calls in flight run with no root.
- [ ] **L7 Two windows on one project share one session.**
  - Both processes load and save the same session id; the temporary file
    names are fixed (`.{id}.json.tmp`, `.latest.json.tmp`), so a save
    fails ("Save failed") and decisions are lost
    (`tools/server_lifecycle.rs:38-41`, `core/session/persistence.rs:35-56`).
  - Decision 4.
- [ ] **L8 `consolidate` and `load` read the session from disk, not the
  live one** (`tools/sobre_knowledge/consolidate.rs:49`, `tools/registered/sobre_session.rs:21-40`).
  Fresh decisions are not imported, `load` drops unsaved ones, and each
  `consolidate` re-imports the same facts: "confirmed ×4" and an inflated
  quality score.
- [ ] **L9 `sobre_knowledge search` returns superseded and archived facts,
  unlabelled** (`tools/sobre_knowledge/search.rs:25-37`).
- [ ] **L10 Every new server wipes the stub index.** The compaction marker
  starts at 0 (`server/compaction_sync.rs:6,15-30`), so after the first
  PreCompact ever, each new process treats it as new and clears the stubs.
- [ ] **L11 Root URIs with non-ASCII characters are rejected.**
  `percent_decode` pushes each byte as a char (`server/roots.rs:41-60`).
- [ ] **L12 The previous run steers `mode=auto`.** *Invariant 2.*
  - Each process reloads the latest session (`tools/server_lifecycle.rs:38-41`);
    its task then drives `mode=auto` (`core/auto_mode_resolver.rs:92-104`,
    `server/context_gate.rs:52-63`).
  - `context_gate` switches a read to `map` when three facts mention the
    file, archived ones included (`server/context_gate.rs:97-112`).
  - Decision 4.
- [ ] **L13 State that is written and never read.** `latest.json`
  (`core/session/persistence.rs:45-49`), `SessionState.evidence` (read only
  by `export`, `core/session/state.rs:167-197`),
  `stats.total_tokens_input` and `stats.cache_hits` (`core/session/state.rs:160-164,219-222`).
- [ ] **L14 `context_gate`'s graph branches never fire.** They look up an
  absolute path in a graph keyed by relative paths
  (`server/context_gate.rs:66-95`, `core/property_graph/queries.rs:40-62,121-140`).
- [ ] **L15 `sobre_graph` holds the cache write lock for its whole run**
  (`tools/registered/sobre_graph.rs:177-195`); concurrent reads fail meanwhile.
  Likely.
- [ ] **L16 `gain` overstates the proxy's savings.** The proxy recompresses
  the whole history on every request (`proxy/anthropic.rs:61-90`) and
  records each request's difference (`proxy/forward/prepare.rs:68-70`),
  priced as uncached input (`core/usage/summary.rs:116-121`). `gain`
  (`cli/dispatch/analytics/gain.rs:168-173`) and `docs/contracts/usage-ledger-v1.md` call the figure a lower
  bound "priced once". Likely, from reading.
- [ ] **L17 Stores disagree on what a project is.**
  - `hash_project_root` mixes the root with an identity (the git remote,
    or the package name). Knowledge, gotchas, graphs and co-occurrence move
    when a remote is added or changed, or the package is renamed;
    `normalize_git_url` only strips `git@` (`core/project_hash.rs:8-26,106`).
  - `index_namespace::namespace_hash` hashes the identity alone
    (`core/index_namespace.rs:10-17`). Two clones, or two unrelated
    projects with the same package name, share `vectors/<hash>/` and
    rebuild each other's index (`core/bm25_index/mod.rs:713-753`);
    `doctor --fix` can delete the shared directory (`doctor/fix.rs:302`).
  - Checkpoints use the canonical path (`core/git/shadow.rs:259`); sessions
    compare the root string.
  - Decision 6.
- [ ] **L18 Smaller ones.**
  - The observe hook treats any model name containing "mini" as a
    background model, so "gemini" is never detected
    (`hook_handlers/observe.rs:76-86`).
  - `sobre_review`'s diff review ignores `depth`
    (`tools/registered/sobre_review.rs:186`); `review` and `checklist` resolve the
    raw path, skipping the B37 rule.
  - `recall` with both `category` and `query` ignores the query
    (`tools/sobre_knowledge/remember.rs:299-319`).
  - Failures returned as success: graph not found, session save failed,
    unknown knowledge store, and "No session found… Starting fresh" when
    one exists.
  - `task`, `finding` or `decision` with no value stores "(no
    description)", which then drives `mode=auto`.
  - `sobre call` resolves only `path` (`cli/call_cmd.rs:183-205`), so
    `sobre_graph`'s `path`/`to` arguments fail from the CLI.
  - `server/multi_path.rs` documents a `repo` argument nothing reads.
  - With only a local upstream, `gain` says "Real cost $0.00 (… local,
    free)" and "Gain $0.56 (100.0%)", priced at a hosted model's rate.
- [ ] **L19 `sobre call` compressions are not in `usage.jsonl`.** The same
  `sobre_shell` call adds a line over MCP and none through `sobre call`;
  the README says every compression is recorded "on every path".

## Lot M — code

- [ ] **M1 The default build compiles about 2,400 lines only the dense
  build can reach.** The oracle (every `pub` made `pub(crate)`) on the
  default build flags about 62 items: `core/hnsw.rs` (516 lines),
  `core/embedding_index.rs` (778), `core/splade_retrieval.rs` (331),
  `core/search_reranking.rs` (520), `core/ann_cache.rs` (298), parts of
  `core/hybrid_search.rs`, and the BM25 `shrink_resident*` functions. They
  are `pub`, so `dead_code` misses them. Fix: gate them behind `dense`.
- [ ] **M2 Windows-only code fails `clippy -D warnings`.** 13 lints, none
  seen by CI, which runs clippy on Linux only: `core/atomic_fs.rs:114`,
  `core/pathutil.rs:298`, `ipc/process.rs:117` (`borrow_as_ptr`) and
  `:291`, `shell/platform.rs:291-293,322-324` (`collapsible_if`),
  `core/cloud_files.rs:23`, `core/home.rs:48`, `core/walk_filter.rs:34`.
  macOS is clean.
- [x] **M3 `SobreEditTool` still implements an MCP tool named
  `sobre_edit`** (#59), with a stale `tool_def`, never registered; only
  `.handle()` is used (`tools/registered/impl_edit.rs:10-38`). Comments still
  name a `sobre_edit` tool (`tools/sobre_patch/mod.rs:13,45,268`,
  `tools/sobre_patch/anchors.rs:44`, `tools/edit_recovery.rs:1`). Fix: a plain function.
  Done: `replace_unique` and `replace_all` call `impl_edit::run_io` with
  typed parameters, through the same lock/cache/session path as the
  anchored ops; the comments are fixed.
- [x] **M4 BLAKE3 hashes labelled MD5.** (#59) `sobre_patch` prints `md5=`
  (`tools/sobre_patch/mod.rs:310-312,352,372-378`); `lsp/backend.rs:28`
  says "md5-hex"; the manifest has a `schema_md5`.
  Done: the preimage/postimage lines are gone from `sobre_patch`'s
  output (nothing reads them now that `expected_hash` is gone), and the
  other labels say hash or BLAKE3.
- [ ] **M5 `SOBRE_DISABLE_SEARCH_INDEX`** is set by `benches/efficiency.rs`
  and read nowhere.
- [ ] **M6 Tests that assert nothing.** `core/task_briefing.rs:152-157,159-172`
  evaluate a bare `matches!`.
- [ ] **M7 `MaintenanceResult` is an empty struct**, returned and discarded
  (`core/storage_maintenance.rs:10-12,50-58`, `server/server_handler.rs:113`).
- [ ] **M8 Stale comments and contracts.**
  - `docs/contracts/knowledge-policy-contract-v1.md:47-48` names removed sub-policies;
    `:90` cites an upstream ticket.
  - Removed features still described: auto-consolidation
    (`server/post_dispatch.rs:14-16`), knowledge consolidation at shutdown
    (`tools/server_lifecycle.rs:132`), `core/tool_lifecycle.rs:1-3`, the
    auto-context briefing (`server/tool_trait.rs:97`), the daemon and
    auto-updater (`core/pathutil.rs:9-10,272,376-379`,
    `core/launchd.rs:57-58`), an `embeddings` feature
    (`core/code_health/fabric.rs:142`).
  - `tool_defs/mod.rs:57` names `sobre_ctx_expand`; `core/memory_policy.rs:53`
    says "Set None/off".
  - `shell/reentry.rs:7-8` misdescribes `SOBRE_WRAPPED`;
    `core/agent_runtime_env.rs:11` says the data dir for a file in the state
    dir.
  - `apply_line_window` says an explicit `map`/`signatures` is never
    replaced; it is (`signatures` with `start_line=10` becomes
    `lines:10-999999`).
  - `tools/registered/sobre_glob.rs:58` says `block_in_place`; dispatch uses
    `spawn_blocking`. The `sobre_read` warning at `:850` says 5 s; the
    timeout is 10 s.

## Lot P — CI and release

- [ ] **P1 CI never ran on phase 1.**
  - The fix PRs targeted `chore/release-prep`, and CI runs only on pull
    requests to `develop` and `main`.
  - Every run of #30, and every run since 8 October (run 50), failed in a
    few seconds without a runner (`runner_id 0`). Account or billing side;
    nothing in the repository causes it.
  - Every phase-1 merge relied on the local preflight alone, on Linux
    alone. For this audit, fix PRs should target `chore/audit-2` as before,
    and the macOS and Windows checks run locally by cross-compiling until
    CI runs again.
- [ ] **P2 A release does not wait for CI.** `release.yml` publishes an
  `-rc.N` on every push to `main`, in parallel with CI and without running
  a test.
- [ ] **P3 What CI does not cover.** No test or clippy run of the `dense`
  build except clippy through the preflight; clippy on Linux only (M2);
  pull requests test on Linux only.
- [ ] **P4 `rust/Cargo.toml` can be published.** It has no `publish =
  false` and carries crates.io metadata (keywords, categories, a binstall
  section, a "cargo install sobre" comment). `cargo publish` would put the
  README on crates.io and docs.rs. Fix: `publish = false`, metadata out.
- [ ] **P5 The release notes are written inside `release.yml`.** Fix: keep
  them in a `.md` file in the repository that the workflow reads.

## Lot N — documentation

The standard asked for: every environment variable, config key, command and
tool listed with its behaviour and options, and the architecture written
down, in `.md` files in the repository.

- [ ] **N1 There is no architecture document.** What exists: the layout in
  `rust/README.md:40-60`, the in-process lock order in
  `rust/LOCK_ORDERING.md`, and the contracts. Missing: the processes and
  their lifetimes, the request flows, the compression pipeline and how an
  original stays reachable, the data layout, cross-process concurrency,
  config loading, the two builds, the agent integration per host, the
  macOS seatbelt, the language servers, the security model. Proposed
  outline of `docs/architecture.md`:
  1. scope and the invariants it serves;
  2. component map;
  3. processes and lifetimes (MCP server, CLI, hooks, `-c`, proxy);
  4. request flows: an MCP call, a hook per host, `-c`, the proxy, `init`,
     `uninstall`;
  5. compression and recovery (tee, `sobre_expand`, labels);
  6. state and data layout;
  7. concurrency, in-process and between processes;
  8. configuration loading, and what differs between the two builds;
  9. agent integration matrix;
  10. security model;
  11. index of the reference pages;
  12. how to change it (the invariants, the generated pages).
- [ ] **N2 `mcp-tools.md` lists parameter names only.** The generator
  (`core/reference_docs.rs:108-135`) prints no type, description, enum,
  default, conditional requirement, annotation, or the parameter the dense
  build adds.
  - Undocumented actions and modes: `sobre_knowledge` has 25 actions (5
    documented), `sobre_session` 16 (7 undocumented), `sobre_graph` 12,
    `sobre_read` 16 modes (`full-compact`, `aggressive`, `entropy`,
    `reference`, `density:0.NN` among them).
  - `sobre_patch`'s parameters have no descriptions and its `ops` items no
    type.
  - `action` is a free string in `sobre_knowledge`, `sobre_session` and
    `sobre_graph`, not an enum.
  - Decision 9.
- [ ] **N3 Undeclared parameters, aliases and fallbacks.** *Invariant 3.*
  - `sobre_search`: `languages`, `path_glob`, `ignore_gitignore`, `mode`
    in the default build, the aliases `grep`/`pattern`/`search`/`related`,
    and a fallback that takes any unknown string argument as the pattern
    (`tools/registered/sobre_search.rs:153-197`).
  - `sobre_patch`: `expected_hash`, `backup`, `backup_path`, `evidence`,
    `diff_max_lines`, `validate_syntax`, `expected_size`,
    `expected_mtime_ms`, and the `old_string`/`new_string` aliases
    (`tools/registered/sobre_patch.rs:23-27,117-139,283-299`).
    Removed in #59, with `allow_lossy_utf8`, `create` on
    `replace_unique`, and `name_path`/`end_line`/`expected_hash` on
    `replace_symbol`.
  - `sobre_review`: `project_root`, `root`. `sobre_read`: `full-compact`.
  - Decision 7.
- [ ] **N4 About 30 environment variables are undocumented**; about 9 are.
  - One changes behaviour: `SOBRE_WRAPPED` makes `sobre -c` pass through
    (`shell/reentry.rs:7-19`). It is a re-entry guard sobre sets on its own
    children; it should be documented as that.
  - `SOBRE_SEATBELT`: on macOS the proxy's LaunchAgent and orphaned
    processes run under `sandbox-exec`, denied `~/Documents`, `~/Desktop`
    and `~/Downloads` (`core/tcc_guard_sandbox.rs`). Undocumented.
  - The agent environment capture: variables starting `CODEX_`, `CLAUDE_`,
    `OPENCODE_` or `GEMINI_` (credentials excluded) are read from the
    current process or the parent's (`/proc/<ppid>/environ`, `ps eww`),
    stored in `state/agent_runtime_env.json`, and passed to `sobre_shell`
    commands.
  - Read but undocumented: `SHELL` (a non-POSIX shell is replaced by
    zsh/bash/sh), `COMSPEC`, `MSYSTEM`, `MINGW_PREFIX`, `LC_*`,
    `NO_COLOR`, `RUST_LOG`, `CODEX_PROFILE`, `COPILOT_HOME`,
    `CLAUDE_PROJECT_DIR`, `WORKSPACE_FOLDER_PATHS`, `CURSOR_TASK_ID`, the
    provider keys used as signals (`ANTHROPIC_API_KEY`,
    `ANTHROPIC_AUTH_TOKEN`, `OPENAI_API_KEY`, `CODEX_API_KEY`), the ONNX
    Runtime search (`HOMEBREW_PREFIX`, `LD_LIBRARY_PATH`,
    `DYLD_LIBRARY_PATH`, a relative `ORT_DYLIB_PATH`), `SOBRE_PROXY_TOKEN`
    (absent from README and SECURITY), `SOBRE_UPDATE_FROZEN_HASHES` (tests;
    belongs in CONTRIBUTING).
  - Set by sobre on its children: `BASH_ENV=""`, `ENV=""`,
    `SOBRE_WRAPPED=1`, the forwarded agent variables; the LaunchAgent pins
    `SOBRE_*_DIR` and `HOME`; the systemd unit only `RUST_LOG=info`.
  - Decision 9.
- [ ] **N5 There is no command reference.**
  - The README covers about 60 % of the commands and flags; `sobre help
    all` misses about 10 flags (`doctor --gate`, `doctor integrations
    --fix`, `call --json-file`, `embeddings provision --force`, `proxy -p`,
    `--autostart`, `proxy token --quiet`, `help full`/`-a`, `index
    --root`, …).
  - Exit codes are documented nowhere.
  - The README does not mention `sobre mcp`, `gain`'s `--json`, `--period`,
    `--model`, `--reset`, `--limit`, `index status`, `--force`, `--root`,
    `doctor overhead` or `lint-context`.
  - The hook contract per host is not described.
  - Decision 8.
- [ ] **N6 Factual errors in the README.**
  - Logs are said to be in the state dir (`README.md:341`); `crash.log`
    is in the data dir (`core/crash_log.rs:22`).
  - "Two people on one machine get different ports" is false on macOS
    (H11).
  - Compression is lowered "for the rest of the session" (`README.md:247`);
    it comes back after 2 minutes (`core/refetch_signals.rs:12`).
  - `ignore` is said to apply to "project walks" (`config-keys.md`); it
    applies to the BM25 and graph indexes only, not to glob, tree or regex
    search.
  - A `~` in `search_roots` is not expanded on the multi-root route
    (`server/server_handler.rs:73-89`, `server/multi_path.rs:62-80`): the root is
    dropped silently.
  - "Never into a repository" (`README.md:134`): `doctor --fix` rewrites
    project files (H13).
  - "Through `claude mcp add-json` when the claude CLI is installed": it
    never is (H5).
  - "`proxy disable` stops it": it does not stop a proxy the MCP server
    started (H15).
- [ ] **N7 Config keys: what `config-keys.md` does not say.**
  - The value syntax of `config set` (booleans `true|1|yes…`, arrays
    comma-separated), and that no command removes a key.
  - A file that does not parse is ignored whole (warning on stderr); an
    unknown key is ignored.
  - `config init` keeps `config.toml.sobre.bak`.
  - Each key has two descriptions (struct doc and schema), which drift.
  - `shell_timeout_secs`: 0 times out at once; heavy commands get at least
    600 s; `timeout_ms` wins over it; a foreground command moves to the
    background after about 110 s.
  - `embedding_model`: what the aliases resolve to.
  - `passthrough_commands`: how a command is matched, and the built-in
    list.
  - `proxy.upstreams`: the defaults, live reload, validation.
  - A built-in rule: the proxy never compresses results of tools whose name
    contains `serena` (`core/config/proxy.rs:25-42,147-150`).
- [ ] **N8 Files sobre writes that no document lists.**
  `memory/archive/`, `read_cache/stub_index.json`, `detected_model.json`,
  `active_transcript.json`, `last_compaction.json`, `diagnostics.json`,
  `doctor/latest.json`, `.mcp-startup.lock`, `starts.log`, `proxy.log`,
  `proxy.port`, `cooccurrence/`, `agent_runtime_env.json`, `exports/`.
  Also unlisted: the language servers `sobre_refactor` needs
  (rust-analyzer, typescript-language-server, pylsp, gopls;
  `lsp/config.rs:20-35`) and the 27 languages `sobre_outline` parses.
- [ ] **N9 `rust/README.md`'s layout omits** `doctor/`, `uninstall/`,
  `proxy_setup/`, `proxy_autostart.rs`, `lsp/`, `ipc/`, `engine/`,
  `tool_defs/`, `mcp_stdio.rs`, `config_io.rs`, `compound_lexer.rs`,
  `rewrite_registry.rs`, `terminal_ui.rs`.

## Publishing

Checked: nothing publishes documentation outside the repository.

- GitHub Pages and the wiki are off (`has_pages: false`,
  `has_wiki: false`); there is no `gh-pages` branch.
- There is no site configuration (mkdocs, mdBook, Docusaurus, Jekyll,
  `CNAME`) and no workflow that deploys one.
- `rust/Cargo.toml` names no `homepage` or `documentation`.

Three places could still publish text; P4, P5 and decision 11 deal with
them:

- `cargo publish` would push the README to crates.io and docs.rs (P4);
- the GitHub release notes are written in `release.yml` (P5);
- the audit files are notes kept in the repository (decision 11).

The repository is private: the release download links in the README need
a signed-in GitHub account.

## E20 still open

E20 ([first audit](phase1-audit.md)) stays open: the OpenCode plugin and
skill paths against its config path on Windows, Zed's path on Windows, and
the scope of `COPILOT_HOME`. H9 (Zed on macOS and Windows) and H10
(`where claude` on Windows) are fixed from the documentation and the
code; a run on each platform joins E20, which none of them can settle
without the platform at hand.
