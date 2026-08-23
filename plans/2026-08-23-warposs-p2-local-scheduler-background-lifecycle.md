# WarpOss P2.4 Local Scheduler And Durable Background Lifecycle

## Goal

Run scheduled local agents and durable child-agent events without Warp ambient
workers. SQLite owns schedules, occurrence claims, run state, event journals,
and cursors; a local supervisor owns processes and OS notifications.

## Schedule Repository

- Persist versioned schedule rows with UUID, name, enabled state, trigger
  (`once`, interval, or validated cron), IANA timezone, DST policy, missed-run
  policy, overlap policy, named-agent/config snapshot reference, prompt,
  canonical working directory, created/updated revision, next occurrence, and
  last outcome.
- Store immutable occurrence rows keyed by `(schedule_id, intended_at,
  schedule_revision)`. Claim and transition state in SQLite transactions so two
  app windows/supervisors cannot start the same occurrence.
- Default to at-most-once execution: once an occurrence is durably claimed,
  crash recovery marks an unowned `running` row `interrupted` rather than
  repeating potentially side-effectful tools. A user may explicitly rerun it.
- Missed-run policies are `skip`, `run_latest_once`, or bounded `catch_up`.
  Never produce an unbounded backlog. Overlap policies are `skip`, `queue_one`,
  or `parallel` with an explicit concurrency cap.
- Validate timezone/DST gaps/folds and compute occurrences with a single tested
  library. Persist UTC intended times plus timezone/rule revision for audit.

## Local Supervisor

- Register one process-local supervisor with a renewable SQLite lease and
  monotonic wake timer. It scans due occurrences, claims them atomically, and
  starts the existing local `AgentDriver`/named-agent path with a stable run ID.
- By default schedules run only while WarpOss is open. Offer an explicit
  `run in background at login` setting that installs a narrowly scoped local OS
  launch entry for the WarpOss scheduler command. Never install a daemon or
  login item silently.
- Resolve provider/model/profile/MCP/skills/harness immediately before launch.
  Missing config/capability produces a terminal local failure without process
  or Warp request. Store references and redacted effective metadata, never
  secret values.
- Bound global/per-schedule concurrency, runtime, output, event queue, disk
  usage, and graceful shutdown. Cancellation verifies local ownership, signals
  the exact controller/PTY/process group, waits boundedly, then marks status.

## Durable Run And Event Journal

- Persist run rows for owner, parent, schedule occurrence, harness, status,
  timestamps, cancellation generation, last heartbeat, and local conversation.
  Reconcile live ownership on startup; never present an orphaned PID as running.
- Persist orchestration envelopes with monotonically increasing per-target
  sequence, immutable payload, state (`pending`, `leased`, `accepted`,
  `failed`), attempt count, and timestamps. Use local run IDs only.
- Delivery acknowledgement occurs when the target controller transactionally
  accepts the envelope/cursor into its next request. Never wait for server/model
  echo. After a crash, reclaim only unaccepted leases; accepted messages are not
  re-sent automatically after possible tool side effects.
- Implement `WaitForEvents` as a cancellable local journal wait with cursor and
  deadline. Same-process subscribers receive immediate notifications; separate
  scheduler processes poll/notify through SQLite/OS IPC without Warp SSE.
- Apply bounded retry only before target acceptance. Exhaustion becomes a
  visible failed event; no infinite retry loop.

## UI, CLI, And Notifications

- Add local create/list/show/update/pause/resume/delete/run-now/history commands
  and equivalent schedule UI. Use optimistic revisions and preview the next
  occurrences before save.
- Show local run status, intended/start/end time, missed/overlap decision,
  provider/model/harness, and redacted error. Remove cloud environment, owner,
  billing, and hosted task/session links.
- Post OS notifications for blocked/completed/failed scheduled or background
  runs. Notification permission denial is non-fatal; clicking a notification
  opens the local conversation/run.
- Deleting a schedule stops future claims but does not silently terminate a
  running occurrence. Cancellation is a separate explicit action.

## Tests

Start with failing occurrence/journal/lease state-machine tests. Cover at least:

1. Once/interval/cron calculations across timezones, DST gaps/folds, clock
   changes, enable/pause/update/delete, and restart persistence.
2. Two supervisors contend for one due occurrence and only one process launch
   occurs; crash recovery marks claimed/running work interrupted without an
   automatic duplicate.
3. Every missed-run and overlap policy, bounded catch-up, concurrency/runtime
   limits, and manual rerun semantics.
4. Durable ordered message/lifecycle fan-out, cursor restore, duplicate event,
   pre-accept retry, post-accept crash, queue cap, `WaitForEvents` timeout, and
   same-/cross-process wakeup.
5. Exact-owned cancellation, orphan reconciliation, stale PID reuse, process
   group cleanup, and truthful terminal states.
6. Missing provider/model/capability/profile/MCP/harness fails before launch;
   endpoint/tool failure is not automatically replayed.
7. OS login integration is opt-in/installable/removable and notification
   permission denial does not affect schedule durability.
8. Blocked Warp domains and default/`local_only` paths perform zero hosted
   schedule, SSE, auth, telemetry, billing, or cloud-runner calls.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_schedule_repository -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_scheduler_supervisor -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_orchestration_journal -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_scheduler -- --nocapture
```
