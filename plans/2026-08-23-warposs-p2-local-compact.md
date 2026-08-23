# WarpOss P2.1 Transactional Local `/compact`

## Goal

Turn `/compact` into a real local history compaction operation using the
selected direct OpenAI-compatible chat model. A failed, cancelled, stale, or
invalid summary must leave conversation state byte-for-byte unchanged and must
never fall back to Warp.

## Summary Contract

- Build a bounded compaction snapshot from a contiguous prefix of the root task
  that ends only at a complete user/assistant/tool boundary. Never split a tool
  call from its result, an in-flight exchange, orchestration envelope, or active
  long-running command.
- Preserve recent turns outside the range. Require a minimum reclaimable size;
  otherwise return a local `nothing to compact` result.
- Ask the selected concrete custom model for a strict versioned summary object:
  goals, user constraints, decisions, files/symbols, commands and outcomes,
  unresolved work, child-agent results, and a concise narrative. Include the
  selected first/last message IDs and count in the request but derive/verify
  those fields locally rather than trusting the model.
- Parse only the bounded structured payload; reject empty, oversized, malformed,
  contradictory, or instruction-bearing wrapper output. The summary is data for
  conversation context, never executable tool input.

## Two-Phase Mutation

- Phase 1 is read-only: capture conversation ID, root task/revision, ordered
  message IDs, range checksum, tool chronology, current provider/model, and
  summary prompt. Release mutable model state while HTTP is in flight.
- Phase 2 reacquires the conversation and compare-and-swaps the exact revision,
  range IDs/count/checksum, and absence of an in-flight request. If anything
  changed, discard the summary and ask the user to retry; do not merge against a
  moving history.
- Convert the accepted object into one local summary message plus the existing
  `MoveMessagesToNewTask`/`Task::splice_messages` archive structure. Extend the
  direct OpenAI history adapter so the replacement summary is included in all
  later requests while the moved raw messages stay excluded from the active
  context.
- Apply task/archive/index metadata as one in-memory transaction, then enqueue
  one SQLite conversation-state write. On persistence failure, restore the
  pre-mutation snapshot and surface a local error.
- Keep the visible historical transcript available through the existing moved
  subtask/export representation. Mark compaction metadata locally with schema,
  source range, model ID, timestamp, and checksum; never store chain-of-thought.

## Provider And Capability Rules

- Resolve routers to one concrete custom model before snapshotting. Require
  effective `chat`; no tools, embeddings, vision, auth, or Warp capability is
  implied.
- Respect the configured context window when choosing the input range and
  reserve explicit output/headroom budgets. If the endpoint reports context
  overflow, reduce only the not-yet-mutated candidate range and allow one safe
  retry because no local or remote tool side effect was requested.
- No provider/model, provider change during the request, malformed response,
  cancellation, timeout, rate limit, or connection failure leaves history
  untouched with a precise local error.

## UI And Queue Semantics

- Reuse `/compact`, `/compact-and`, `/fork-and-compact`, progress, and
  cancellation UI. Cancellation aborts the summary request and preserves the
  original history.
- Queue a follow-up prompt only after the compact transaction and durable write
  succeed. A failed compact must not auto-send the follow-up.
- Display the effective custom provider/model and reclaimed message count. Do
  not show subscription, quota, cloud, or upgrade affordances.

## Tests

Start with failing snapshot/parser/CAS tests. Cover at least:

1. Valid compaction replaces exactly the selected complete prefix, preserves
   tool-call/result chronology, archives raw messages, and includes the summary
   in the next direct request.
2. Restart restores the same active summary/archive boundary and export remains
   truthful.
3. Empty/malformed/oversized output, wrong IDs/count, endpoint error, timeout,
   cancellation, and no provider leave serialized state unchanged.
4. A concurrent message/action/status mutation fails the CAS without losing
   either history branch.
5. In-flight tools, unmatched results, active LRCs, and orchestration envelopes
   are never split or silently discarded.
6. `/compact-and` and `/fork-and-compact` submit their follow-up only after a
   successful durable commit.
7. Local and user-configured remote endpoints work; unsupported capability and
   blocked Warp domains produce no hosted/auth/telemetry request.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_compaction -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp direct_openai_compaction -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_compaction -- --nocapture
```
