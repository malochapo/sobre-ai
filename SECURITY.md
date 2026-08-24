# Security

## Status

`sobre` has not been released. There is no supported version, no advisory
process and no coordinated-disclosure timeline, because there are no users to
protect yet.

If you find something anyway, open a normal GitHub issue on this repository.
Do **not** report it to lean-ctx: this is an independent fork, and its
maintainers are not responsible for defects introduced here.

If the issue also affects upstream lean-ctx, report it to
[lean-ctx](https://github.com/yvgude/lean-ctx) through their own process — that
is their call to make, not ours.

## Threat model

`sobre` runs as your user, on your machine, with your permissions. It is not a
sandbox and does not try to be one. Concretely, it can read any file you can
read, and `ctx_shell` runs commands as you.

What it does defend against:

- **Path escape.** Tool arguments resolve against the project root; a path that
  climbs out is refused.
- **SSRF on URL reads.** `ctx_url_read` validates the target and blocks private
  and link-local address ranges, re-validating after every redirect.
- **Untrusted workspace config.** A project-local config file from a repository
  you have not marked trusted cannot widen what the agent may do.
- **Secret leakage into context.** Redaction runs before content is archived or
  returned.

What it does **not** defend against: a malicious repository whose *contents*
convince your agent to do something harmful. `sobre` shapes what the agent
sees; it does not decide what the agent should believe. Treat tool output from
an untrusted repository as untrusted input.

## Network

The default build talks to no network service. Two things can change that, and
both are yours to switch on:

- the local proxy, if you configure it against a provider;
- `ctx_url_read`, when you pass it a URL.

There is no telemetry, no update check and no usage reporting. If you find code
that contradicts this, it is a bug — report it.
