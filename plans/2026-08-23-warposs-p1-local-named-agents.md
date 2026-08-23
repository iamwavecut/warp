# WarpOss P1.9 Local Named-Agent Bundles

## Goal

Provide persistent named agent configurations that compose the local provider,
execution profile, MCP, skills, and local harness paths already present in the
client. CRUD and execution must be entirely file-backed and must never use Warp
agent-management APIs or managed secrets.

## Local Schema And Repository

- Store one strict YAML document per agent under a dedicated local
  `agents/` directory. Use a UUID filename as the stable ID; display-name edits
  do not change identity.
- Schema fields: `name`, optional `description`, `base_prompt`, concrete custom
  `model_id`, optional local execution `profile_id`, ordered local skill
  references, local/configured MCP specs, local harness, optional computer-use
  enablement, and optional secret references.
- Secret references may name an environment variable or an existing secure
  storage/keychain entry. Reject literal secret values, inline credentials,
  provider keys, and secret-bearing MCP environment values from managed files.
- Parse with `deny_unknown_fields`, validate all references locally, and retain
  per-file actionable errors without suppressing valid agents.
- Create/update with atomic compare-and-swap writes; delete only a validated
  UUID file after confirmation. Preserve existing data on any failure.

## Resolution And Execution

- Reuse `AgentConfigSnapshotFile`, `merge_with_precedence`, profile resolution,
  skill resolution, MCP construction, and the local `AgentDriver`.
- Merge precedence is deterministic:
  named bundle < optional one-shot config file < CLI/UI overrides < explicitly
  invoked skill instructions. Do not mutate the stored bundle at run time.
- Resolve the model to a concrete local custom model/router and validate required
  capabilities before spawning. Missing provider/profile/skill/MCP/harness
  yields a local error and no partial process launch.
- Allow only local harnesses and local terminal execution. Do not add a worker
  host, cloud environment, remote runner, server conversation, or transcript
  upload.
- Store only references in persisted conversation metadata; snapshot the
  effective non-secret config needed for truthful history/resume.

## CLI And UI

- Extend `warp agent` with local `create`, `show`, `update`, `delete`, and
  `run --agent <id-or-name>` behavior. UUID wins; name resolution must be unique.
- Make `warp agent list` distinguish named agents from discovered skills without
  labeling skills as agents. Never print base prompts or secret references in a
  default listing; `show` may print non-secret config explicitly.
- Reuse the agent-management list/details surfaces with local rows/status only.
  Remove Warp owner/environment/managed-secret/auth/upgrade affordances.

## Tests

Start with failing repository/resolver/CLI tests. Cover at least:

1. CRUD/rename/restart with stable IDs, watcher refresh, strict parse errors,
   atomic failure, concurrent edit, path traversal, and safe deletion.
2. Merge precedence across bundle/config/CLI/skill and no mutation of stored
   input.
3. Rejection of literal secrets and redaction-safe list/show/error/log output.
4. Missing/ambiguous model, profile, skill, MCP, or harness fails before process
   or HTTP dispatch.
5. A valid named agent runs through the selected local harness/direct provider
   with the expected profile/MCP/skills and no Warp request.
6. Default and `local_only` behavior is identical and state survives restart.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_named_agent -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp_cli named_agent -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_named_agent -- --nocapture
```

