# Local-first upstream intake — 2026-10-21

Upstream `9b499e02961ba2ea48f1e3837b7655589920aff0` contains two commits.
Base fork: `f0463b9bdd4503c63d54b2b4dba4b65634a45795`.

## Accepted local adaptation

`9b499e029` fixes MCP protocol preflight and SDK lifecycle selection. The
fork does not contain upstream's crates/mcp runtime: its live consumer is
app/src/ai/mcp/templatable_manager/native.rs. Put protocol negotiation in
crates/mcp/src/protocol.rs and call it from that existing desktop path.

Connect directly to the user's configured MCP endpoint. Prefer server/discover,
negotiate a common known revision, and use the matching Discover or Initialize
SDK lifecycle. Preserve legacy SSE and stdio compatibility. Bound HTTP probes
to ten seconds and response bodies to 1 MiB. Do not retry permission failures,
server failures or uncorrelated/version-exhausted replies as legacy negotiation.
Preserve configured headers and MCP credential challenges without adding Warp
login, hosted transport, telemetry, managed secrets or a local_only branch.

Loopback regression tests are added before implementation and use real HTTP
and SDK connections. Cover modern discovery, older revisions, legacy initialize
counteroffers, SSE replies, static headers, invalid revisions, correlation,
HTTP errors and oversized bodies. Existing desktop credential classification
and local capability-query tests remain. No pre-fix RED execution is claimed:
repository policy runs verification only after all business/code edits.

Final verification also exposed an existing test-collector defect: two local
agent-environment scan tests expected completion records, but their shared
collector ignored the production completion event. Capture its real fields in
the collector instead of weakening assertions or changing runtime behavior.

## Existing local equivalent and excluded remote surface

`831f4fa9f` treats runs:null as an empty hosted agent-run list. Its only changed
consumer is the server API ListRunsResponse decoder and its fixtures. Neither
ListRunsResponse nor list_runs exists in the fork. The desktop conversation
manager's fetch_tasks_for_filters instead syncs local conversations and emits
TasksUpdated. The user result (an empty local task list) already exists without
decoding a server response. Do not recreate hosted run downloads or add a null
wire type without a consumer. No local feature is removed.

## Verification and delivery

Run final formatting, both all-target build variants, all MCP crate tests and
focused desktop MCP/direct-provider tests. Compile the app test binary once and
reuse it for filters. Record actual counts, merge/push SHA, signed production
installation proof and owned cleanup in automation memory. Keep protected
release apps and active user-owned backups. Do not launch the app for this run.
