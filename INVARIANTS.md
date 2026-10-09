# The six invariants

These are not guidelines. They are the definition of the project. A change that
breaks one of them is out of scope no matter how good it is — and if we ever
decide to break one, it happens by explicitly amending this file in its own PR,
never as a side effect of a feature.

Why they are written down: scope grows through individually reasonable
decisions, and no amount of good judgement stops that on its own. The only
defence is a rule you cannot argue with case by case.

---

## 1. Twenty tools, and every new one replaces an old one

The MCP tool surface is capped at **20 tools**. There is no exception, no
"internal" tool that doesn't count, no meta-tool that fans out to a hidden
registry.

The surface currently stands at 15, leaving five slots. That margin is
deliberate: a cap you are already touching is a cap you will break.

Adding a tool requires naming the tool it replaces, in the same PR. If nothing
is being replaced, the answer is no.

> The failure mode this prevents: overlapping families where several tools
> answer the same question, and the agent spends tokens choosing between them.

## 2. One configuration file, one level, no cascade

- **One** config file. One location. No per-repository install, and no
  per-agent settings: `init <agent>` only picks which agents to connect.
- **No layering.** No defaults → profile → file → env → flag resolution chain.
  If a value can come from two places, it comes from one place.
- **No environment variables that change behaviour.** Variables may exist for
  paths and secrets — `SOBRE_CONFIG_DIR`, `SOBRE_DATA_DIR`,
  `SOBRE_PROXY_TOKEN`, …. Never for what the tool does.
- **No profiles**, no presets, no modes.
- **No settings outside the file.** No per-project files, no settings kept in
  session state, nothing a previous run persisted that later acts as a value.
- Hard cap: **15 configuration keys.** Adding one requires removing one.

How keys are counted: a top-level field is one key, a field of `[proxy]` is
one key, and a table that a key holds (`proxy.upstreams`) counts once
whatever it contains. A test (`config::schema::key_cap`) pins the
list, so adding a key is always a visible change.

The file currently has 11 keys, leaving four slots: `search_roots`, `ignore`,
`passthrough_commands`, `shell_timeout_secs`, `hook_binary`,
`compression_level`, `debug_log`, `embedding_model`, and the `[proxy]` table's
`enabled`, `port` and `upstreams`. Every key has one value even when
the file is silent: `compression_level` defaults to 0.3, never to "unset".
`init` sets nothing of its own: besides which agents to connect, its flags
only preview (`--check`), skip its questions (`--repair`) or report in JSON
(`--json`). Its two questions write `proxy.enabled` and, in a build with
embeddings, download the model.
The generated list is `docs/reference/generated/config-keys.md`.

If a setting can be decided correctly by the tool, it is not a setting.

> The failure mode this prevents: a resolution cascade where the effective value
> of a setting cannot be determined by reading one file.

## 3. One tool surface, always the same one

There is exactly one answer to `tools/list`. It does not depend on an
environment variable, a config key, a pinned profile, the identity of the
calling client, a role, or a workflow state.

Two deliberate exceptions. On wording: `compression_level` condenses the prose
of tool descriptions, as it condenses the rest of sobre's prose. On builds:
the build with local embeddings adds one parameter, `sobre_search`'s `mode`,
which a build without them could not honour. Within a build, the set of
tools, their names and their parameters never change.

Corollary: no "universal invoker" tool, because there are no hidden tools for it
to reach.

> The failure mode this prevents: an agent that cannot rely on a tool being
> there, and a maintainer who cannot tell which surface a bug report describes.

## 4. One machine, one person, nothing to sell

**`sobre` is 100% local and 100% personal: a tool a developer installs on their
own workstation.** That is the whole product. Everything below follows from it.

No accounts. No licence checks. No plans, tiers or entitlements. No hosted
service. No cloud sync. No telemetry — not opt-out, not opt-in, none.

**No shared instance either.** No team server, no org gateway, no multi-user
mode, no per-person API keys, no storage quotas, no role hierarchy. If a feature
only makes sense when several people point at the same process, it does not
belong here.

**No remote data store.** Indexes, memory and caches live in the user's own
filesystem or in-process. A remote vector database, a Postgres metering table,
an admin console — all out. Local dense retrieval with an in-process HNSW index
is fine, including GPU acceleration; the line is whether it needs a service
someone has to run.

`sobre` makes no network request the user did not ask for.

This invariant is what buys us the right to delete code without asking whether
it might be monetisable later, or whether some organisation might want it. It is
load-bearing, and it is the sharpest tool we have against re-growing into what
we forked away from.

## 5. The agent host is in control

**`sobre` never decides what may run, be read or be written: no allow list, no
deny list, no block. What the agent host already governs is neither reproduced
nor bypassed by `sobre`.**

Agent hosts — Claude Code, Copilot, Cursor, Codex, Gemini CLI — ship their own
control: permission rules, approval prompts, auto modes, classifiers. They are
better placed to decide than a context tool sitting beside them, and a second
gate can only disagree with the first. The tool `sobre` forks from shipped such a
gate: a curated allow list of shell commands, a path jail, refusals of
secret-looking files, a throttle on repeated calls, and approval settings written
into the host's configuration. Each one either duplicated the host's decision or
overrode it. `sobre` keeps none of them.

What follows from this:

- **No gate of its own.** No command filter, no path confinement, no read-only
  roots, no refusal of files by name, no throttling of repeated calls. A command
  runs as the agent wrote it; a path is read or written where the agent pointed.
- **Nothing written into the host's permissions.** No allow rules, no
  `autoApprove`, no `trust`, no plan-mode settings. A hook never returns an
  approval or a denial: it either says nothing or, where the host allows it,
  replaces a result or rewrites a command.
- **No transformation that escapes the host's rules.** Output is compressed
  after the host ran the command wherever the host supports it. Where only a
  rewrite is possible, a command is wrapped only when every host rule `sobre`
  can read treats the original and the wrapped form alike; otherwise it runs as
  written.
- **Nothing hidden.** When `sobre` changes something — compresses or condenses
  output, lowers compression for a session, skips files in a search — it says so
  in the output, and the full original stays reachable.

Masking secrets in `sobre`'s own output (`[REDACTED:…]`) stays: it governs what
`sobre` shows, not what the agent may do.

`sobre` is therefore exactly as permissive, and exactly as strict, as the host
it runs in. That is the intended position.

## 6. Use what exists; write only what is sobre's own

**What a maintained library already does, `sobre` uses. It does not rewrite
it, copy it, fork it or vendor it.** `sobre`'s own code is what no library
provides: the compression, the tools' behaviour, the agent integration.

- **A dependency before a local implementation**, even a small one: the MCP
  protocol, parsers, file formats, HTTP, locking, path lookup, string
  algorithms. Prefer a crate already in the dependency tree; a new, heavy
  dependency for a small need is justified in its PR, since a dependency is a
  cost too.
- **No vendored copy and no `[patch]`.** A bug in a dependency is fixed
  upstream. Until the fix ships, the workaround is the smallest change in
  `sobre`'s own code, next to a comment that links the upstream issue, and it
  goes when the fix ships.
- **A local implementation that stays says why**, in a comment next to it: the
  feature the library lacks, the measured cost, or the rule it would break
  (determinism, invariant 4). "It is only a few lines" is not a reason.

> The failure mode this prevents: private copies of libraries that drift from
> upstream, miss its fixes and end up owned by nobody — a vendored MCP SDK
> three major versions behind for a six-line fix, hand-written parsers for
> formats whose parser is already a dependency.

The code written before this rule is inventoried, with a verdict for each
case, in issue #69. Each item there is either replaced or given its written
reason.

## Process rules (not invariants, but not optional either)

A release cadence is only as good as the stabilisation behind it. These rules
exist so that shipping often and shipping safely are not in tension.

- `main` is always releasable. Work lands on `develop`.
- Every release is preceded by a `-rc.N` tag and a **72-hour soak**. No
  exceptions for "trivial" fixes; trivial fixes are how regressions ship.
- Branch protection on `main` and `develop`: no direct pushes, review required.
- **Two maintainers with equal rights, from day one.** A project whose bus
  factor is 1 has already reproduced the problem it was forking away from.
- No marketing material or private notes in the repository, and no generated
  artefacts except the reference pages in `docs/reference/generated/`, which
  CI regenerates and compares with the code. The repo contains source, tests,
  docs and CI. Nothing else.
- **Commit messages and comments describe what `sobre` does and why.** They do
  not narrate what the upstream project got wrong. The fork is justified by
  what it is. Technical observations about inherited code belong in the
  maintainers' private notes, not in a permanent public record that sits
  directly above the commits of the people it discusses.

---

## Amending this file

Any change here is its own PR, titled `invariant:`, with a written rationale,
approved by both maintainers. If we find ourselves amending it more than once a
quarter, the project has failed and we should say so out loud rather than
quietly widen the rules.
