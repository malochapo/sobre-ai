# Security

## Supported versions

`sobre` is in pre-release: the 0.1.x release candidates (`v0.1.0-rc.N`).
Only the newest release gets fixes; older ones are not patched.

## Reporting a vulnerability

Report it privately, through GitHub's private vulnerability reporting on this
repository: the **Security** tab, then **Report a vulnerability**. Please do
not open a public issue for a vulnerability.

The report, the fix and the advisory stay private until a release fixes the
issue; the advisory is then published. There is no guaranteed response time.

## What sobre is

A single binary that runs on your machine, as your user:

- a local MCP server that your agent starts and talks to over stdio;
- a command-line tool;
- hooks that your agent runs around its own tools;
- an opt-in local proxy between your agent and its model provider.

There is no `sobre` service, no account and no cloud component. A report
about any of the four parts above, or about the install scripts, is in scope.

## Threat model

`sobre` runs as your user, on your machine, with your permissions. It is not a
sandbox and does not try to be one. It reads and writes wherever the agent
points it, and `sobre_shell` runs commands as you, exactly as written: what may
run, be read or be written is the agent host's decision, not `sobre`'s (see
[INVARIANTS.md](INVARIANTS.md), invariant 5).

What it does defend against:

- **Secret leakage into context.** Secret-shaped values are masked as
  `[REDACTED:…]` in what `sobre` returns and in the full outputs it keeps for
  `sobre_expand`: private keys of every type (the whole block), known token
  formats (provider API keys, GitHub, GitLab, Slack, Stripe and npm tokens,
  AWS key IDs, JWTs, database URLs, bearer tokens), and long values under
  key, token, secret or password names. Masking is pattern matching, not a
  guarantee: a short value, a secret under an unrelated name, or a read window
  that starts inside a key's body can go through. Those outputs are written with
  owner-only permissions and deleted after 24 hours. `sobre_read` with
  `raw=true` returns a file's bytes unmasked, on request.
- **Other local users.** The proxy listens on `127.0.0.1` only and refuses
  requests that carry neither a provider API key nor its own token
  (`sobre proxy token` prints it; `SOBRE_PROXY_TOKEN` replaces it). On Unix,
  `sobre`'s directories are readable by you alone.

What it does **not** defend against: a malicious repository whose *contents*
convince your agent to do something harmful. `sobre` shapes what the agent
sees; it does not decide what the agent should believe. Treat tool output from
an untrusted repository as untrusted input.

## Network

`sobre` makes no network request you did not ask for. These are the only ones,
each started by you:

- the local proxy, which forwards your agent's requests to the providers you
  configure;
- in the builds with embeddings only: `sobre embeddings provision` (or saying
  yes when `sobre init` offers it), which downloads Microsoft's ONNX Runtime
  from GitHub and the embedding model from HuggingFace. Neither
  `sobre index build` nor a search ever downloads anything; without the model
  they say so. The builds without embeddings download neither. The `minilm`
  and `nomic` aliases are pinned to a HuggingFace commit; a model named as
  `org/repo` without `@revision` follows that repo's `main` branch.

The install scripts (`install.sh`, `install.ps1`) ask the GitHub API which
release is the latest, unless given one, then download that release's archive
and `SHA256SUMS`, and nothing else.

There is no telemetry, no update check and no usage reporting. If you find code
that contradicts this, it is a bug: report it.
