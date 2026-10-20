# Local-first upstream intake — 2026-10-20

Upstream: `cd4b970e6331aa7e9c18bdd441ab0a1b841a0b1c` (nine commits).
Selective ancestry merge from the clean fork master; application changes are
applied below without restoring the upstream hosted application tree.

## Accepted local results

- `00c9593cd`: associate command completion with the block captured by the
  matching command-start event. Ignore unrelated or earlier block completion,
  including missed preexec hooks. Shell exit drops both local waiters instead
  of hanging. Preserve the fork's existing channel/error API.
- `cd4b970e6`: ignore delayed CLI-task activation for a finished terminal block.
  Keep normal activation and completion notifications for running commands.
- `1e1acb4d7`: retain ANTHROPIC_MODEL and supply the local Claude ACP adapter's
  custom-model option for nonempty, non-default model IDs. Both existing local
  launch consumers use the shared model-environment helper; no model metadata
  fetch, managed secrets or hosted transport is added.

Tests were added before implementation. They cover matching and unrelated block
events, shell exit, late and normal CLI activation, and selected/default/empty/
absent model configuration. Tests execute only in final verification, per the
repository policy; no observed pre-fix failing test run is claimed.

## Existing local equivalents

- `2511c797f`: filtered task fetch is already a local sync plus TasksUpdated in
  agent_conversations_model.rs; it makes no ServerApi management request.
- `393e97a30`: local ACP dispatch already accepts supported installed adapters
  without the upstream dogfood/cloud-transport feature gate.
- `c67ff199d`: local driver setup already prepares terminal, local repositories,
  skills and MCP. local_shell_recovery and terminal manager recovery fail
  interrupted native work and discard queued input without killing external
  user-owned CLI prompts. These remain the local lifecycle consumers.
- Local mirror checkout reports a redacted exit status and returns the actual
  Git failure. User-owned Git credential helpers remain untouched.

## Excluded remote-only surfaces and consumer evidence

- `d5667a308`: standby sandbox identity and workload-token headers are used by
  GraphQL/standby callbacks. The fork has no standby client or allocated-sandbox
  identity consumer; local preparation needs neither callback credentials nor
  a replacement identity service.
- `c67ff199d`: prepare_prewarmed_assignment requires an Oz execution configuration,
  installs server run credentials, fetches task secrets/attachments, binds team
  scope and waits for a shared session. New replace_shell and
  replace_runtime_environment calls originate in this assignment flow. Their
  result is reassigning a server-owned warm sandbox, not a missing local user
  operation. Do not add that lifecycle or reset the user's MCP/shell to emulate
  a server lease. Local setup/recovery above remains available.
- `7082729a1`: the read-only credential-store wrapper protects credentials written
  and refreshed by driver/git_credentials.rs from server API responses. That
  module/writer is absent in the fork; local Git uses the user's configured
  helpers. Installing this wrapper globally would alter user-owned credentials
  rather than fix a fork-owned consumer. No server credential writer is added.
- `945201940`: the environment_checkout diff changes tracing span attributes and
  records result_ok after early returns; it does not change Git operations or
  returned results. The hosted checkout span pipeline has no local consumer.
  Keep the local mirror implementation and redacted failure diagnostics instead
  of adding manufacturer telemetry spans.

## Final gates

Run formatting, both required all-target build variants and focused runtime
tests. Inspect changed paths for forbidden surfaces, preserve AGENTS.md and
the unconditional local-first feature flags/direct-provider routing. Actual
verification, delivery and cleanup results are recorded in automation memory.
