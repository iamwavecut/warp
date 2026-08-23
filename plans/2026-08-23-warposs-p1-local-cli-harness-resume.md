# WarpOss P1.10 Local CLI Harness Resume

## Goal

Allow Codex and Claude Code harness runs to continue after WarpOss restarts by
binding a stable local run ID to the harness-owned session UUID and transcript.
Resume must use only files already produced by the local CLI. Warp
`/harness_support`, conversation tokens, block snapshots, transcript uploads,
and server-side rehydration remain absent.

## Current Boundary

- `CodexHarnessRunner` already captures a local session UUID from
  `CLIAgentSessionsModel`, and `codex_command` already has a `resume <uuid>`
  branch, but every new runner currently passes `None`.
- `ClaudeHarnessRunner` already assigns `--session-id <uuid>`, but does not
  retain that UUID and has no `--resume <uuid>` command branch.
- Both `save_conversation` implementations are intentional no-ops after the
  hosted transcript/snapshot code was removed. The driver still invokes
  periodic, post-turn, and final save points, providing the correct lifecycle
  boundary for a local index.
- Standalone CLI runs currently have no `task_id`; local child runs do. Resume
  therefore needs its own stable local run ID rather than a Warp conversation
  token.

## Local Repository

- Store one versioned metadata document per run under
  `warp_core::paths::data_dir()/harness-sessions/<run-id>.json`; the UUID
  filename is the stable local resume ID.
- Persist only: schema version, run ID, harness kind, harness session UUID,
  canonical working directory, validated transcript locator, created/updated
  timestamps, last successful save point, terminal/complete state, and optional
  local parent/child task ID. Do not copy transcript contents into the index.
- Create/update through a repository interface with atomic temp-file rename,
  restrictive permissions, compare-and-swap revision, and a per-run lock.
  A failed write must leave the previous record intact.
- Treat the Codex/Claude transcript directories as allowlisted roots. Store a
  root-relative locator, reject symlinks/path traversal, reopen with no-follow
  protection where available, and verify the parsed transcript session UUID
  equals the indexed UUID before launching a resume.
- Invalid, missing, corrupt, mismatched, or unsupported-version records are
  actionable local errors. Never recreate them from a network service.

## Lifecycle And Commands

- Generate a local run ID before starting a fresh third-party harness. Reuse an
  existing local `task_id` as an association, not as the harness session UUID.
- Extend the harness runner construction boundary with a typed local resume
  payload. Codex receives its known UUID in `codex ... resume`; Claude switches
  from `--session-id` to `--resume` while preserving prompt, system-prompt, MCP,
  model, environment, and working-directory handling.
- On session-start/post-turn, discover the CLI-owned JSONL using the narrowly
  reusable upstream local parsers, validate it, and atomically advance the
  index. Periodic saves may update an already-known locator but must not fail a
  healthy run merely because a CLI has not created JSONL yet. Final save must
  record the terminal state and report durable-index failure.
- Expose `warp agent run --resume <local-run-id> --prompt ...`. The stored
  harness and working directory are the defaults; an explicit conflicting
  harness is rejected. A caller may deliberately override the working directory
  only when the harness supports it and the transcript still validates.
- Print the local run ID in text/JSON/NDJSON output instead of a Warp
  conversation token. Never label it a cloud conversation ID.
- Cleanup may remove temporary bridge/wake state only. It must not delete a
  harness transcript or a completed index record. Explicit local history
  deletion may remove the index record but leaves third-party CLI data alone
  unless a separate destructive action is authorized.

## Compatibility And Failure Semantics

- Keep fresh Codex/Claude execution unchanged when resume is not requested.
- A resume request fails before process launch if the indexed harness differs,
  the transcript is missing/malformed, the session UUID does not match, or the
  CLI does not advertise compatible resume syntax.
- OpenCode/Gemini/Oz resume remains explicitly unsupported until each has a
  proven local session contract.
- Do not automatically retry after the resumed harness starts: the prompt may
  have caused tools or local process side effects.
- No endpoint or provider capability is required; the selected local CLI owns
  its provider/auth behavior.

## Tests

Start with failing repository, command-builder, and driver-boundary tests. Cover
at least:

1. Fresh Codex and Claude runs persist a stable local run/session association
   after session discovery, post-turn, final save, and process restart.
2. Codex emits `resume <uuid>` and Claude emits `--resume <uuid>` with the new
   prompt while fresh commands retain their current shape.
3. Missing/corrupt/mismatched transcript, wrong harness, unsupported schema,
   path traversal, outside-root symlink, and concurrent stale update all fail
   before process launch.
4. A transcript appearing after an early periodic save is discovered later;
   an absent early transcript does not terminate a healthy fresh run.
5. Resume preserves local model/profile/MCP/environment resolution and does not
   persist secret values or transcript contents in the index.
6. Restart round-trip works with Warp domains blocked and captures zero
   `/harness_support`, auth, telemetry, snapshot, or upload requests.
7. Default and `local_only` behavior is identical.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_harness_resume -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp_cli harness_resume -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_harness_resume -- --nocapture
```
