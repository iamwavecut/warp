# Local-first upstream intake — 2026-10-17

Upstream `b80b4e3e0d0492974da93f37e40751338eef5f9e`: four commits after
`3e2b62975`. The selective merge preserves the fork tree. No textual conflicts.

## New local adaptations

- ACP steering settles the interrupted exchange with `FollowUpSubmitted` and
  keeps the conversation `InProgress`. Manual Stop uses `ManuallyCancelled` and
  marks the conversation `Cancelled`. Both use existing native cancellation
  history instead of emitting a misleading Done/Cancelled status sequence.
- Local `warp-harness.json` describes an installed ACP adapter, arguments and
  optional local CLI credential checks/error markers. Explicit invalid manifests
  fail rather than falling through. Built-in PATH adapters remain the default.
  Mounted sidecar paths are filesystem discovery only: no Factory bootstrap,
  adapter download, hosted credentials or Warp API calls. Unsupported Oz,
  OpenCode and Unknown ACP dispatch remain rejected.
- Runtime diagnostic patterns now retain owned manifest strings for the run;
  they do not require leaked/static storage.
- Hosted/shared child conversations no longer materialize hidden terminal panes.
  Four older tests explicitly expecting hosted panes were replaced by a focused
  rejection case. Existing local hidden-child creation/restoration tests remain.

## Existing local equivalents

- Child panes are already discovered from local history and restored on parent
  entry in `PaneGroup::restore_missing_child_agent_panes_for_parent`. Local
  controllers still need their hidden panes eagerly once that parent is entered.
  LocalAgentRegistry supplies local lifecycle status, and conversation exchanges
  supply local recency. No server task timestamp is needed for those consumers.
- MCP setup already uses the local configured managers, profile allowlist,
  inline installation specifications and resolved per-provider configuration in
  `AgentDriver::{start_mcp_servers,start_ephemeral_mcp_servers,resolve_mcp_specs_to_json}`.
  The upstream driver-independent extraction is an internal refactor with no new
  user behavior. Importing its differently factored hosted setup is unnecessary.

## Remote-only surfaces excluded

Upstream remote-child lazy hydration obtains task snapshots through
AmbientAgentTaskModel/ancestor discovery and restores shared-session terminal
viewers on navigation. Its server-updated timestamp and task-state fields serve
only those downloaded placeholders. The fork's local children already have
controllers, exchanges and local registry state, so using remote placeholders
would discard the live local controller rather than improve its behavior.
No task fetch, shared-session transport, snapshot hydration or hosted run-status
sync was imported. The independent local parent-entry restore behavior remains.

## Verification

Tests were authored before implementation. Per repository policy, no early RED
run was performed. Final verification covers manifest acceptance/rejection,
owned diagnostic patterns, steering/Stop history, local pane restore and hosted
pane rejection, existing ACP/direct provider and MCP behavior. Actual command
results and delivery proof are recorded in automation memory.

Both all-target build variants passed. Focused tests passed: 223 harness/ACP,
9 external-harness, 122 direct-provider, 28 custom-provider, one each for hosted
pane rejection, local hidden-child restoration, parent-entry restoration, MCP
normalization and local account UI, plus all 90 CLI unit tests. Filters overlap.
No live adapter session or repository-wide runtime privacy certification is claimed.

Repairs: selected the installed Metal toolchain through process-local
`TOOLCHAINS` after default `xcrun` failed; corrected the manifest executable
helper import to the fork's existing `crate::util::path` implementation.
Failed verification targets were removed by their finalizers.
