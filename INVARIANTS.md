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

The surface currently stands at 17, leaving three slots. That margin is
deliberate: a cap you are already touching is a cap you will break.

Adding a tool requires naming the tool it replaces, in the same PR. If nothing
is being replaced, the answer is no.

> The failure mode this prevents: overlapping families where several tools
> answer the same question, and the agent spends tokens choosing between them.

## 2. One configuration file, one level, no cascade

- **One** config file. One location. No per-repository init, no per-agent init.
- **No layering.** No defaults → profile → file → env → flag resolution chain.
  If a value can come from two places, it comes from one place.
- **No environment variables that change behaviour.** Variables may exist for
  paths and secrets — while the binary is still `lean-ctx`, they are
  `LEAN_CTX_*` (`LEAN_CTX_CONFIG_DIR`, `LEAN_CTX_DATA_DIR`,
  `LEAN_CTX_PROXY_TOKEN`, …). Never for what the tool does.
- **No profiles**, no presets, no modes.
- Hard cap: **15 configuration keys.** Adding one requires removing one.

The file currently has 14 keys, leaving one slot: `allow_paths`,
`read_only_roots`, `ignore`, `passthrough_commands`, `shell_timeout_secs`,
`shell_hook`, `hook_binary`, `compression_level`, `debug_log`,
`embedding_model`, and the `[proxy]` table's `enabled`, `port`, `upstreams` and `routing`. Choices made
once at setup are `init` options, recorded by what `init` installs, not keys.
The generated list is `docs/reference/generated/config-keys.md`.

If a setting can be decided correctly by the tool, it is not a setting.

> The failure mode this prevents: a resolution cascade where the effective value
> of a setting cannot be determined by reading one file.

## 3. One tool surface, always the same one

There is exactly one answer to `tools/list`. It does not depend on an
environment variable, a config key, a pinned profile, the identity of the
calling client, a role, or a workflow state.

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

## 5. Subtraction is the default; the line budget is a hard ceiling

Every release must be **smaller than or equal to** the previous one in lines of
Rust, unless the PR states the exception explicitly and it is accepted.

- Ceiling: **380,000 lines of Rust** under `rust/src`, excluding vendored code
  **and tests** — `#[cfg(test)]` blocks and `rust/tests/`.
- A PR that adds net lines must say so in its description and justify it.
- "We might need it later" is not a justification. `git revert` exists.

### Why tests are outside the ceiling

They were inside it until the count was measured: **112,160 lines across 933
`#[cfg(test)]` blocks**, a quarter of `rust/src`, all of it pushing against a
budget the production code has to fit in. That is a standing incentive to
delete tests, and this project has already been burned twice by not running the
ones it has — a warning cleanup left `--features dense` broken at 62 errors for
days, and `CORE_TOOL_NAMES references 'shell' which is not registered` sat red
from the commit that caused it until an audit went looking. The suite is the
only reliable oracle here. A budget that taxes it is aimed at the wrong thing.

### Where we actually stand

`rust/src` is about 245,800 lines, of which about 73,000 are test code.
**Production code: about 173,000 — some 207,000 below the ceiling.** (It was
343,328 when this section was first written, and 183,800 at the end of phase 1.)

So the ceiling is met, and has been for a while without anyone knowing: the
figure quoted above (491,554, "about 111,000 lines below where we stand") was
counting tests on both sides. That never made the removals optional: `addons`,
`context_package`, `plugins`, the dashboard and the config cascade went because
invariants 1, 2 and 4 said so — not to hit a number. What it changed is that
subtraction no longer needs to be argued from a budget it was already inside.

### Where 380,000 comes from

The figure is measured, not chosen. At 491,554 lines in `rust/src`, executing
**every** removal on the table when the ceiling was set — the tool surface (47,736), `cli/` (~27,100), `proxy/` (26,683),
`context_kernel` (16,759), `ocla` (10,297), plus `doctor`, `dashboard`,
`addons` and `lsp` (29,002) — lands at roughly **334,000**. Reaching 180,000
would mean cutting a further 154,000 out of `core/`, `server/`, `shell/` and
`hooks/`: the engine itself. A ceiling nobody can reach is a ceiling everybody
ignores.

380,000 is the target for the scope we chose: the MCP context layer **plus**
the local compressing proxy.

### Scope decision behind the number

`proxy/` stays. It is the only part of the system that acts on *all* traffic
rather than on the tools an agent chooses to call, and it is what makes the
token claim true rather than aspirational. It runs entirely on the user's
machine and nobody else connects to it, so it sits inside invariant 4. It also
carries `shape-xlat`, which is what lets a Claude-shaped client talk to a local
Ollama — squarely the local-first use case.

If the proxy is ever dropped, this ceiling drops with it, to 330,000.

---

## 6. Permissive by default; the developer decides what to forbid

**`sobre` allows every shell command by default. Restrictions are an explicit
deny list the user writes, never an allow list they must maintain.**

The tool it forks from ships a curated allow list of 242 executables, plus a
second list of cloud CLIs held back because "a coding agent that can run
`terraform apply` by default is an incident waiting to happen". That design
assumes the user needs protecting from their own agent. `sobre` assumes the
opposite: it is for developers who know what they are running.

The reasoning is not bravado, it is accounting. **An agent without `sobre` runs
those commands anyway** — the shell is right there. Gating them here buys no
safety; it only moves where the friction lands, and costs a data table someone
has to review forever. And the cloud CLIs specifically are *useful* to an agent
doing diagnosis: reading a cluster's state is most of the work.

A developer working against production is responsible for blocking what must
not run there, in that project. That is their call, not a default we make for
everyone.

What follows from this:

- No default allow list. Shipping one recreates the maintenance we forked away
  from, and the "empty list" path already exists in the engine.
- Blocks are opt-in, explicit, and few. What remains unconditional is what a
  deny list cannot express — inline interpreter evaluation and the shell
  builtins that hide a command from inspection.
- **No hidden behaviour.** `sobre` does not silently refuse, silently rewrite,
  or silently substitute. If it declines something, it says so and says why.

`sobre` is therefore more dangerous than a tool that says no by default — and
exactly as dangerous as the terminal the developer already has open. That is
the intended position.

## Process rules (not invariants, but not optional either)

A release cadence is only as good as the stabilisation behind it. These rules
exist so that shipping often and shipping safely are not in tension.

- `main` is always releasable. Work lands on `develop`.
- Every release is preceded by a `-rc.N` tag and a **72-hour soak**. No
  exceptions for "trivial" fixes; trivial fixes are how regressions ship.
- Branch protection on `main` and `develop`: no direct pushes, review required.
- **Two maintainers with equal rights, from day one.** A project whose bus
  factor is 1 has already reproduced the problem it was forking away from.
- No generated artefacts, marketing material, or private notes in the
  repository. The repo contains source, tests, docs and CI. Nothing else.
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
