# Upstream intake: 2026-10-22

Upstream: `f8a9c719f387e750190b5fbc98ec07aedbd48e4c` (eight commits).
Baseline: `b6cd4311c33e990354146c5886ca864b03c0cb4d`.

The merge records upstream ancestry with the local tree as its base. The
reviewed changes below are applied explicitly, rather than restoring hosted
modules deleted by this fork. No new source-level `local_only` branches.

## New local adaptations

- `f3bb1306b`: ACP batches ready corrections in FIFO groups of at most 32.
  Prompt requests are flushed before cancellation can be sent. Follow-up
  cancellation retries twice (after one and two seconds); its grace expiry
  leaves corrections queued until the active turn responds. Explicit stop
  retains its hard deadline and clears queued input. Completed turns do not
  receive a false interruption note. Local native conversation/adapter routing,
  permission policy and user-owned process lifecycle remain unchanged.
  Regression coverage exercises the real RPC pipe/flush ordering, bounded FIFO
  delivery, grace-expiry queue preservation and explicit-stop discard.
- `6ce705ac5`: V4A matching and local persisted/restored previews preserve explicit
  blank lines, empty repeated lists and CR-bearing lines. Embedded LF in a
  repeated line is rejected without legacy fallback. Match failures and no-op
  hunks remain independent local diagnostics, with counters large enough for
  more than 255 hunks. Fixture tests cover matching and actual editor bytes.
  The fork's existing protobuf pin and legacy wire conversion stay unchanged;
  optional line data is local serialized state, not a new hosted request path.
- `f8a9c719f`: the useful desktop behavior is restoring history into an existing
  terminal without attaching AI exchanges to older commands with matching
  timestamps. The fork's existing `load_ai_conversation` consumer now limits
  matching to newly restored command blocks. Coverage includes equal-timestamp
  prefixes and an empty restored tail. No standby driver is needed.

## Existing local equivalents

- `349cdd6f5`: `harness_kind_with_transport` already selects local ACP adapters
  unconditionally; Pi uses its installed manifest. There is no release/dogfood
  gate to promote, and no reason to add a Cargo feature.
- `c6c62e2fe`: `AgentDriver::resolve_mcp_specs` parses UUID/inline JSON locally
  before startup; `resolve_mcp_specs_to_json` reads configured installations and
  applies local secrets/templates. There are no managed-client config network
  operations to parallelize. Actual MCP readiness already starts servers and
  uses concurrent waits. Startup subscription ownership explicitly requires
  its three phases to remain sequential. Adding managed requests would regress
  local operation rather than accelerate this consumer.
- Local continuation already uses `harness/local_resume.rs`, local conversations
  and CLI-owned transcript locators, not server conversation loading or hosted
  standby assignments. The independent restoration-boundary fix is adapted above.

## Excluded remote-only implementation

- `4ff5753e2`: the control-origin override routes a Warp prewarmed sandbox's
  `/internal/prewarmed-sandboxes/.../ws` websocket separately from its server
  status endpoint. Its only consumer is the absent hosted standby lifecycle;
  it supplies no local terminal, provider or MCP operation.
- `0616e9d38`: standby credentials/author replacement fetches task Git credentials
  and overwrites a container's preceding assignment identity. This fork has no
  standby assignment or managed Git credential writer. Local checkouts use the
  user's Git configuration/credential helpers, so importing this writer would
  change user-owned identity rather than provide a missing local result.
- `96d816ad2`: AWS/GCP credential preparation and cleanup belongs to federated
  cloud sandbox assignments, task-scoped managed secrets and team metadata.
  Local CLI adapters inherit their own local configuration/environment; there
  is no task server or hosted credential allocation to prepare or replace.
- `f8a9c719f`: standby assignment restoration still requires task configuration,
  team scope, server conversation loading and hosted sharing. These consumers
  are absent. Local continuation and the independent terminal fix cover the
  useful desktop result without those transports.
- `6ce705ac5`: aggregate `AgentMode.Code.V4AMatchFinished` telemetry and its
  identifiers/persistence/upload path are not imported. Local callers already
  consume deltas, failure details and `warrants_failure`; user-visible matching
  does not need a manufacturer event. No telemetry stub is introduced.

## Verification scope

After all code changes: formatting, AI matcher tests, both required all-target
builds, and one app test binary reused for ACP, restoration, diff persistence,
editor bytes, custom-provider and direct-provider regression filters. Production
build/install follows only a verified push. Temporary targets/tools/logs are
agent-owned and finalized on failure as well as success. See automation memory
for observed command results and installed artifact proof.

Observed verification: formatting and both all-target builds passed. The AI
matcher suite passed 40 tests. Reused app-binary filters passed 58 ACP, 17
restoration, two restored-preview, four diff-persistence, 31 diff-application,
one editor-byte fixture test, 122 direct-provider, 28 custom-provider, three
default-setting and one local User/hidden-cloud-section tests. Filters can
overlap; these are per-filter counts, not a unique total. The batch's private
19 GiB target and temporary docktile plugin were removed.

Test compatibility repairs: omit a redundant upstream line-stat assertion
because this fork's `FileDiff` exposes no such method; exact preview content,
range and insertion assertions remain. Update the fork-only permission-order
test to the same RPC read barrier as upstream instead of the removed polling
helper. No production behavior or permission assertion was weakened.

Privacy search classified existing UI event subscriptions as local callbacks,
test `AuthState` as deterministic local setup, adapter auth as the installed
third-party CLI's own configuration, and `CloudConversationData` as an existing
restoration data type (not newly added transport). No telemetry collector,
upload, billing/account entry point or Warp endpoint was added. Forced-off
flags, direct-provider routing, `local_only = []`, `AGENTS.md` and dependency
pins remain unchanged.
