# WarpOss P1.5 Durable Local Prompt Queue

## Goal

Make `/queue`, the queue panel, prompt attachments, queued commands, and
auto-fire a default local feature whose state survives restart. Queue state is
owned by a local repository and never by Warp Cloud or telemetry.

## Semantic Upstream Intake

Use the current upstream queue V2 implementation as behavior evidence, not as a
blind merge. Port only local concepts:

- prompt rows with image/file attachments;
- shell-command rows;
- per-conversation ordering, edit/reorder/delete, and default/explicit queue
  toggle state;
- locked pending long-running-command rows and command-in-flight gating;
- terminal-state-driven auto-fire.

Do not port `InitialCloudMode`, hosted shared-session dispatch, cloud handoff,
telemetry wording/events, `/compact-and`, or `/fork-and-compact` coupling in
this item. Local `/compact` is P2.1.

## Local Repository

- Add an additive SQLite migration and repository boundary for ordered queue
  rows and durable per-conversation settings.
- Persist stable row ID, conversation ID, position, kind (`prompt`/`command`),
  text, local origin, serialized attachment metadata, lock state, attempt count,
  and timestamps. Persist explicit queue-toggle state separately.
- Image attachments may persist their already-bounded base64 bytes locally.
  File attachments persist path/name/MIME metadata and must be revalidated when
  fired after restart.
- Queue mutations must be atomic and observable only after persistence succeeds;
  write failures keep the previous in-memory/UI state and surface a local error.
- Load rows deterministically at startup. Corrupt individual rows are
  quarantined/skipped with a local diagnostic; they must not prevent other
  conversations from loading.
- Deleting/clearing a conversation removes its queue transactionally. Normal
  pane closure must not delete durable queue state.

## Execution Rules

- Enable the queue feature unconditionally for this local-first fork in default
  and `local_only` builds.
- Fire only the head row and only after the current agent/tool/shell action has
  a terminal state.
- A queued shell command sets `command_in_flight`; the next row cannot fire
  until that command completes. Crash/restart resets in-flight execution to a
  recoverable pending row and never reruns a side effect automatically.
- Attempt count increments before dispatch. After an uncertain crash or error,
  retain the row for explicit user retry/edit/delete; do not silently retry.
- Missing provider/capability or stale attachment leaves the row intact and
  reports a local error. No Warp fallback.

## Tests

Start with failing repository/model tests. Cover at least:

1. Append/edit/reorder/delete/toggle and prompt/command kinds survive a fresh
   model/repository instance with stable IDs and ordering.
2. Image and file attachment metadata round-trip; missing/changed files fail at
   dispatch without losing the row.
3. Locked rows and commands cannot auto-fire early; a completed command unlocks
   exactly one next row.
4. Crash/restart with a dispatched-but-unconfirmed row does not rerun it
   automatically and preserves its attempt count.
5. A failing SQLite write does not mutate the model or emit a success event.
6. Conversation deletion clears only the owned queue; pane closure does not.
7. Corrupt rows do not block valid rows, and deterministic positions are
   repaired transactionally.
8. `/queue` and the panel are available in default and `local_only`, with no
   provider and no Warp traffic.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp queued_query -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp queued_prompts -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only queued_query -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only queued_prompts -- --nocapture
```

