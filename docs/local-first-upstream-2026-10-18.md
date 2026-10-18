# Local-first upstream intake — 2026-10-18

Upstream `1e6ef379bc9f51257c4243e8c4396a046cede8af`, three commits after
`b80b4e3e0`. Selective ancestry merge preserves the fork tree and its instructions.

## Accepted local behavior

`063a7643a` pins `$BASH` to the executable selected by Warp before bootstrap.
This prevents a Dock/launchd PATH from making Homebrew bash report `/bin/bash`.
Executable paths are quoted through both shell layers, including apostrophes.
The implementation is adapted to the fork's existing terminal module.

Tests were added before implementation: command construction and real PTY startup
with a different PATH, spaces and apostrophes in executable paths. No early RED
run is claimed because repository policy requires final-only verification.

## Existing local equivalents

Local `AgentDriver::run_task` already bootstraps its terminal, resolves configured
MCP servers, clones local configured skill/workspace repositories, prepares the
environment and loads skills before a prompt. No server lease is needed.
`AIBlockAction::CopyOutput` already copies local output through the OS clipboard.

## Remote-only surfaces excluded after consumer tracing

- `3f02b7028`: the new `standby::prepare_and_wait` obtains configuration from
  `PrewarmedSandboxClient`, maintains a Namespace lease, reports Ready/Failed and
  waits for server expiry. `prepare` accepts only Oz and obtains managed secrets,
  Git credentials, Team metadata and Warp Drive sync. `maintain_sandbox` retains
  its result only while waiting indefinitely; no local run claims it. New
  `prepare_resources` and MCP Standby mode exist solely for this entrypoint.
  Existing local run preparation supplies the independent useful behavior.
  No lease, Factory clone, telemetry, hosted MCP, new CLI command or standby pool
  is imported. No local standalone consumer/result is lost.
- `1e6ef379b`: CopyText actions are decoded exclusively from hosted
  ChatGPTSubscriptionError payloads and routed by subscription recovery UI.
  The new text is a Business administrator request supplied by the server.
  This fork has no such subscription identity, recovery payload or billing UI.
  Its direct-provider errors and ordinary output use existing local presentation
  and clipboard paths. No server-authored admin action, protocol capability or
  hosted API schema update is imported.

## Verification and delivery

Final gates: formatting, both all-target builds, local shell unit/real-PTY tests,
direct/custom-provider regressions and local account UI. Delivery SHA, command
results, release signatures/UUID/hash and cleanup evidence are recorded in
automation memory after the verified source commit. No new `local_only` source
branch, dependency or hosted fallback is added.

Final verification passed: `cargo fmt --check`, both all-target build variants,
six shell tests (including the real PTY cases), 122 direct-provider tests,
28 custom-provider tests, three defaults tests and one local account UI test.
Filters can overlap. The first batch failed because the upstream helper module
differs in this fork; the import was corrected to the existing command executor
helper. Both failed and passing private targets were removed by finalizers.
