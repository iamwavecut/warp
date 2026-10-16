# Local-first upstream intake — 2026-10-16

Upstream: `3e2b62975ada8119f495f136f22474e1b26fa278` (13 commits after
`522481cd042ce8c48eed04dbff8e0313f174c329`). The merge retains the fork tree and
selectively adapts behavior. No textual conflicts occurred.

## New local adaptations

- ACP inbound requests run independently. Permission UI cannot block streamed
  updates or prompt responses. Cancellation answers outstanding permissions
  once, without cancelling another session's requests.
- Stop uses `session/cancel`. Mid-turn corrections queue in order, wait for the
  previous prompt response, and start a new local history segment. Late updates
  are drained before settlement. A 15-second cancellation timeout discards
  queued command text and fails the run. Only this unresponsive-adapter fallback
  interrupts its terminal. A failed turn closes the connection, not a retry loop.
- Typed adapter capabilities control local attachment embedding. Images and
  UTF-8 resources use bounded reads. Unsupported or oversized context remains
  a local file link. No cloud downloads are introduced.
- Native orchestration inputs cannot enter external ACP conversations. Their
  pending injection/echo state is discarded, without removing local child status.
- Request inputs preserve explicit cross-task order. History groups each task's
  inputs into one exchange and registers every task exchange for stream updates.
- Antigravity command aliases, local lifecycle callbacks and delayed rich-input
  submission use the existing local CLI listener. Codex notification plugin
  compatibility now requires version 0.4.3.

## Existing local equivalents retained

- CLI launch construction and setup already use local terminals, environment
  configuration, MCP and locally stored history. Upstream setup extraction and
  launch-mode refactoring do not add a separate user result. Importing their
  hosted identity, observability and setup dependencies is unnecessary.
- `ai/skills/local_checkout.rs` provides validated local mirror-backed checkouts,
  with a lock, symlink rejection and independent object stores. The benchmark
  preparation change instead binds hosted execution IDs and a server-provided
  frozen repository inventory. Local skills do not consume that inventory.
- Local follow-up attachments already accept filesystem references and text.
  This intake improves their representation for capable adapters. Initial local
  driver input is `AgentRunPrompt::Local(String)`, resolved exclusively from
  `Prompt::PlainText` in `agent_sdk/mod.rs`; it has no file-attachment field.

## Remote-only transports not imported

- Fractional billing changes feed billing-cycle rows, team billing totals and
  the TUI usage menu via GraphQL workspace billing metadata. Those surfaces are
  absent/hidden in the fork. Direct-provider token usage is a separate consumer
  and does not need fractional Warp credits. No billing UI is restored.
- Prewarmed sandbox config/status/heartbeat requires Warp agent API keys and a
  Namespace workload token. Its new API client has no desktop caller in this
  revision (the commit is part 1 of a later preparation stack). Local process
  setup has no sandbox generation to acknowledge to that control plane.
- Initial upstream attachments are `DownloadedAttachment` values prepared by
  hosted task download setup. No local launch caller produces those values.
  The cloud download transport and task attachment inventory are not imported.
  Local file context remains available through the existing ACP prompt channel.
- Deferred benchmark preparation validates hosted task/execution binding and
  frozen inventory before checkout. Its consumers are hosted execution config,
  environment bootstrap and benchmark policy files. The fork retains local
  environment preparation, but has no hosted checkout-batch/inventory binding
  or execution-policy consumer. It starts local tasks instead. No server credential,
  snapshot inventory or benchmark-only environment protocol is added.

## Verification scope

Focused tests cover connection progress and cancellation isolation, typed
capabilities, local attachment fallback, correction queue ordering, controller
Stop routing, orchestration isolation, task-exchange history and local CLI input.
Tests were authored before implementation but run only in the final phase, as
required by this repository. Live third-party adapter behavior is not claimed.
Final `cargo fmt --check`, both required all-target builds and all 90 CLI unit
tests passed. App filters passed: 216 harness/ACP, 122 direct-provider,
28 custom-provider, 3 defaults, 1 local account UI, 1 ordered-input history,
8 external-harness, 18 orchestration, 3 Antigravity and 25 Codex plugin tests.
Filters overlap and are not a count of unique tests. Warnings remain in older
modules and unused protocol metadata. No live adapter/GUI session was run.

The verification finalizer removed its 18 GiB target and unused debug docktile
plugin. Earlier failed/incomplete batches removed 4 and 12 GiB respectively.
The first failure exposed fixture/type mismatches. The second stopped after a
successful default build because its running Bash source was edited. The final
batch used an immutable script snapshot and reached `exit=0 stage=complete`.
Production delivery results are recorded in automation memory after this commit.
