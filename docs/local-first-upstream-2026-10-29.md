# Upstream intake: 2026-10-29

Upstream `5a55a2aca77339ad75637bed650352369ee7fcb9` (11 commits).
Fork baseline `aa3634b5e0cffa3aba5e46dd38d25574060194b3`.
The merge uses the fork tree and applies reviewed changes explicitly.

## Accepted and localized

- `3c5115418`: classify the displayed `Tab` binding without converting it to
  `TAB`. The settings dropdown now selects completions or autosuggestions
  instead of incorrectly reporting a user-defined binding. Test both choices
  and unrelated/custom bindings through the classifier used by refresh.
- `5a55a2aca`: defer pending terminal commands until final bootstrap precmd,
  not merely the earlier Bootstrapped hook. Test the real input/event path and
  ensure a second trigger does not dispatch the command twice.
  The upstream hosted checkout helper is absent. The fork instead has local
  environment shell commands and a direct-Git skills cache. Apply the bounded
  start/exit wait to the former, with a single 1800-second deadline. Timeout
  drops the wait and discards only its matching unstarted pending command.
  Preserve replacement input and never type cancellation or kill a running
  user-owned shell. A liveness receiver also prevents an already queued
  main-thread callback from submitting the command after its waiter expires.
  ModelSpawner does not cancel queued work when its returned future is dropped.
  Controlled-future, held-main-thread and driver regressions cover these cases.
- `f7f136ed8`: update the existing nextest installer action pin to v2.87.26.
- `811b70b8b`: update the local agent-dev image's golangci-lint pin to v2.15.0.
  Preserve its local toolchain/configuration; do not import hosted server CI.

## Existing local equivalents

- `6aeacb6ad`: the fork has no `CLIAgent::WarpTui`, so Warp ACP bridge commands
  cannot activate its interactive PTY input handler. Local ACP adapters already
  use a private socket and native history. Add regression coverage for plain,
  shell-quoted and development-binary bridge commands, without restoring a
  proprietary TUI variant or executing reported tools twice.
- `f883a848f`: local shell setup already supplies `WARP_PATH_APPEND` from
  `extra_path_entries()` and applies it after rc files in Bash, Zsh and Fish.
  That preserves bundled macOS CLI paths after profile resets while retaining
  user-command precedence. The upstream change specifically restores a
  `WARP_CLOUD_AGENT_FALLBACK_BIN` emitted by a hosted container entrypoint.
  This fork has no producer or consumer of that private runtime directory.
  Linux local sessions use installed/user-selected commands rather than those
  injected hosted wrappers. Keep the existing local transport and do not add
  a dormant cloud environment-variable contract or force hosted-wrapper
  precedence over user profiles. The unchanged Bash/Zsh/Fish bootstrap path
  code remains the local equivalent; no shell code is imported in this merge.

## Remote-only surfaces not imported

- `513a94bc0`: the trunk.io uploader change only sends CI JUnit results to an
  external analytics service. This fork already has no uploader. Local CI/test
  output supplies diagnostics without that upload; no app consumer exists.
- `5d26d9bb6`: the hosted oz-agent-action bump affects upstream changelog and
  release workflows, both absent here. Local release build/install already
  supplies the artifact; no independent desktop action is lost.
- `365e3c8b7`, `e7bb41fc4`: GitHub App/Octo STS tokens and GCP federation are
  consumed by private upstream release/channel-config/docs automation. The
  fork prepare-environment action has no private SSH-key setup to replace.
  Do not reintroduce those workflows, private repository URL rewrites, token
  delegation policies or cloud credentials for a local production bundle.
- `23adf48cf`: Factory automation scope-filter authoring targets server-managed
  integrations, managed secrets and webhooks. The bundled Factory skill is
  absent here. It has no local filesystem/MCP scheduler consumer to adapt.

## Verification and delivery

AGENTS, forced-off cloud/auth/billing flags and direct OpenAI-compatible routing
remain unchanged. Review changed files for prohibited surfaces; distinguish
local model events, CI GitHub tokens and local deadline errors from telemetry
or Warp account auth. Add no source-level local_only branch.

After all code changes, run fmt, both required all-target builds and focused
Tab, pending bootstrap, local environment, terminal-driver, bridge-detection,
custom/direct-provider and hidden-cloud tests. Use a private external-volume
target because the system disk has less space than the previous test batch.
Clean each batch on success or failure. Push only after successful checks, then
build/install production with pinned private cargo-bundle. Verify UUID, byte
hashes, signatures and source/installed equality without launching the app.
Observed results belong in automation memory, not this planned verification.
