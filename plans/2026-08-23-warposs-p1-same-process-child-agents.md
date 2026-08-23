# WarpOss P1.11 Same-Process Local Child Agents

## Goal

Allow a tools-capable direct OpenAI-compatible model to start one or several
local child agents, send them messages, observe status/topology, and cancel
them. The P1 contract is deliberately same-process: it must not depend on Warp
SSE, server conversation tokens/echo, cloud runners, or cross-process delivery.

## Semantic Merge Boundary

- Reuse the existing `StartAgentExecutor`, `RunAgentsExecutor`, hidden child
  panes, local Oz/Claude/Codex/OpenCode launchers, execution-profile approval,
  local conversation persistence, and topology UI.
- Do not globally restore upstream hosted `OrchestrationV2`. Its current event
  service still resolves senders through `server_conversation_token`, treats
  exchange output as server delivery echo, and retains hosted streamer/SSE
  branches.
- Introduce an explicit local-orchestration readiness boundary. Direct-provider
  schemas are advertised only when `chat + tools` are effective and the local
  controller/registry is registered for the active conversation.
- Hosted/remote execution modes are normalized to local or rejected before
  launch. Never silently create a Warp task.

## Local Ownership Registry

- Add one app-owned `LocalAgentRegistry` keyed by stable local `run_id`, with
  values for conversation ID, terminal surface/pane, parent run ID, direct
  children, harness, status, cancellation handle, and controller ownership.
- Assign a run ID synchronously when a child conversation is created and persist
  parent/run/harness topology through the existing conversation SQLite fields.
  Rebuild historical topology after restart, but mark restored processes as
  stopped unless a live same-process controller reclaims them.
- Registry mutations and child creation must be idempotent by action/request ID.
  Duplicate tool calls return the original launch result rather than spawning a
  second process.
- Limit fan-out, nesting depth, and concurrently live children with explicit
  local constants. Validate non-empty prompts/names, unique sibling names, model
  and harness availability, capability, and working directory before any child
  is created.

## Start And Run Agents

- Complete `StartAgent` when the child conversation has its local run ID and
  controller/pane registration, not when `ConversationServerTokenAssigned`
  fires. Error text must say `local run ID`, never `server identifier`.
- Keep `RunAgents` fan-out bounded and aggregate results in input order. A
  timeout or failure in one slot must not hide successful child IDs; cancellation
  stops outstanding slots and returns truthful partial outcomes.
- For local Oz children, inherit the selected concrete custom model and
  execution profile. For external harnesses, reuse local CLI validation and
  model rules. Do not write provider keys into child configuration or command
  lines.
- Persist Start/Run action request/result metadata so restored parent history
  shows what was launched, while never treating historical rows as live.

## Local Messaging And Acknowledgement

- Resolve sender and recipients through the local registry/run IDs. Remove the
  `server_conversation_token` requirement from the local path.
- Queue one immutable envelope per recipient with message ID, sender, recipient,
  subject/body, sequence, and local state. At P1 this queue is process-local;
  P2 owns the durable journal/cursor lifecycle.
- Delivery acknowledgement means that the target controller atomically accepted
  the envelope into the next local `AIAgentInput`, not that a Warp server echoed
  IDs in model output. Never parse model text as transport acknowledgement.
- If a target is busy, retain one bounded pending queue and wake it after the
  current request reaches terminal state. Do not automatically repeat a prompt
  after endpoint/tool side effects have begun. Unknown, historical, stopped, or
  queue-full targets fail explicitly.

## Cancellation, Status, Topology, Notifications

- Route management/details cancellation through registry ownership to the
  existing conversation stop path and child process/PTY control. Verify the
  target belongs to the local run before signalling it.
- Update local conversation status and aggregate parent/pill state on start,
  block, success, failure, and cancellation. Preserve deterministic child order
  and safe handling of missing/cyclic historical topology.
- Enable local parent header/pill navigation from the registry and persisted
  topology without exposing shared-session or cloud-viewer controls.
- Reuse OS notifications for blocked/completed local children. Notification
  failure is non-fatal and cannot trigger a network fallback.

## Direct Provider Adapter

- Add strict OpenAI schemas/parsers for `StartAgent`, `RunAgents`, and
  `SendMessageToAgent` only after the local registry path is active. Normalize
  all legacy remote fields to the local schema or reject them.
- Tool results contain only local run IDs and structured per-child outcomes.
  They never contain Warp conversation tokens, session links, hosted task IDs,
  auth state, billing state, or server errors.
- A provider without tools, an inactive controller, or a missing compatible
  local model/harness receives a local capability/setup error before HTTP or
  process launch.

## Tests

Start with failing registry/executor/adapter tests. Cover at least:

1. Direct adapter advertises the three orchestration tools only for a ready
   local controller with `chat + tools`, and never advertises hosted variants.
2. One child and ordered multi-child fan-out succeed using local run IDs; mixed
   validation/startup failure returns truthful partial results.
3. Duplicate action/request IDs do not create duplicate conversations or
   processes; limits for fan-out/depth/concurrency fail before launch.
4. Parent-to-child, child-to-parent, and multi-recipient messages resolve by
   local run ID, acknowledge on local intake, wake an idle controller, and never
   require a server token or model/server echo.
5. Busy, stopped, unknown, queue-full, cancelled, and endpoint-failed targets
   degrade deterministically without an automatic side-effectful retry.
6. Cancellation from the inline card and management/details UI stops only the
   owned child, updates status/topology, and cannot signal an unrelated process.
7. Restart restores historical topology/status but does not claim dead children
   are live; same-process message queues are honestly absent until P2.
8. Blocked Warp domains capture zero SSE, orchestration, auth, telemetry,
   billing, or hosted task requests in default and `local_only` builds.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_agent_registry -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_start_agent -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_run_agents -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_send_message_to_agent -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_agent_registry -- --nocapture
```
