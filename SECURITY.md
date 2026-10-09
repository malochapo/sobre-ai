# Security

## Supported versions

Only the latest release gets fixes. There is no advisory process and no
coordinated-disclosure timeline.

If you find a vulnerability, open a GitHub issue on this repository. Do **not**
report it to lean-ctx: this is an independent fork, and its maintainers are not
responsible for defects introduced here.

If the issue also affects upstream lean-ctx, report it to
[lean-ctx](https://github.com/yvgude/lean-ctx) through their own process — that
is their call to make, not ours.

## Threat model

`sobre` runs as your user, on your machine, with your permissions. It is not a
sandbox and does not try to be one. It reads and writes wherever the agent
points it, and `sobre_shell` runs commands as you, exactly as written: what may
run, be read or be written is the agent host's decision, not `sobre`'s (see
[INVARIANTS.md](INVARIANTS.md), invariant 5).

What it does defend against:

- **Secret leakage into context.** Secret-shaped values (API keys, tokens,
  private keys) are masked as `[REDACTED:…]` in what `sobre` returns and in the
  full outputs it keeps for `sobre_expand`. Those outputs are written with
  owner-only permissions and deleted after 24 hours. `sobre_read` with
  `raw=true` returns a file's bytes unmasked, on request.
- **Other local users.** The proxy listens on `127.0.0.1` only and refuses
  requests that carry neither a provider API key nor its own token. On Unix,
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

There is no telemetry, no update check and no usage reporting. If you find code
that contradicts this, it is a bug — report it.
