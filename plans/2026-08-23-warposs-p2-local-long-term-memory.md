# WarpOss P2.3 Scoped Local Long-Term Memory

## Goal

Provide explicit, inspectable, local long-term memory for agent conversations.
Manual CRUD and deterministic keyword recall are the baseline. Semantic recall
and model-assisted extraction are optional layers that require declared
`embeddings` or `chat` capabilities and never contact Warp.

## Data Model And Repository

- Add a dedicated SQLite repository with versioned migrations and rows for:
  memory UUID, scope, canonical scope owner, title, body, normalized keywords,
  source/provenance, created/updated timestamps, revision, pinned/enabled state,
  optional expiry, sensitivity marker, and tombstone.
- Scopes are explicit: global local user, canonical project root, named-agent
  bundle, or conversation. A request may read only the ordered union allowed by
  its active scopes; conversation/project memories never leak across roots.
- Keep immutable version history for edits and delete via tombstone first.
  Create/update/delete use optimistic revision checks and one transaction.
- Store no provider keys, environment values, raw terminal secrets, hidden
  reasoning, or arbitrary full transcripts. Reject or redact common credential
  patterns before persistence and let the user mark a memory sensitive/disabled.
- Export/import uses a documented local JSON/YAML schema with stable UUIDs,
  explicit scopes, checksums, dry-run validation, and collision policy.

## Manual CRUD And Visibility

- Add local CLI/UI list/show/create/edit/move/enable/disable/delete/history
  operations. Every memory shown to an agent is visible and editable to the
  user; provide per-request `why recalled` evidence.
- UI scope selectors resolve canonical local projects and named agents only.
  Do not expose Teams, cloud owners, sharing, sync, or account controls.
- Deleting a project/conversation does not silently delete broader memories.
  Offer an explicit scoped cleanup with count/preview and retain tombstones for
  recovery until local retention expires.

## Deterministic Recall

- Normalize Unicode and tokenize query, memory title/body/keywords. Rank with a
  lexicographic score for pinned/exact phrase/title/keyword/body matches,
  scope proximity, recency, and stable UUID tie-break.
- Enforce strict limits on candidate count, per-memory characters, total memory
  context, and number of scopes. Include source UUID/revision in internal
  context metadata, but send only selected body text to the provider.
- Inject recalled memories through a dedicated `AIAgentContext` variant before
  the current user request. Historical persisted requests retain the selected
  memory revision so replay/export remains truthful.
- No provider is required for manual CRUD or keyword recall. Empty/no match is a
  normal result, not a reason to query a hosted service.

## Optional Semantic Recall

- Reuse the local embedding endpoint adapter and store one vector generation
  per memory revision/provider/model/dimension. Never mix generations.
- Combine semantic candidates with deterministic keyword candidates, then
  apply local bounded reranking. If embeddings are missing/stale/fail, use only
  keyword recall and surface semantic-index status non-fatally.
- Memory edits/tombstones invalidate old vectors transactionally; background
  embedding is cancellable, bounded, and never blocks manual retrieval.

## Optional Extraction

- Default automatic extraction to off. When explicitly enabled, ask the active
  chat model for strict candidate objects only after a successful conversation
  turn. The model cannot write the repository directly.
- Validate scope, provenance, size, sensitivity, duplication, and confidence.
  Present candidates for confirmation unless the user separately enables a
  narrowly scoped auto-save policy.
- Extraction failure, cancellation, malformed output, or provider absence
  leaves memory unchanged. Never retry after any candidate has been committed.

## Tests

Start with failing repository/scope/ranking tests. Cover at least:

1. CRUD/version/tombstone/import/export/restart round-trips and stale-revision
   rejection with no partial writes.
2. Global/project/named-agent/conversation scope isolation, canonical root
   aliases, deleted owners, and deterministic precedence.
3. Unicode keyword ranking, exact ties, total/per-item limits, pinned/disabled/
   expired rows, and `why recalled` provenance.
4. Direct-provider context contains only selected memory bodies/revisions and
   never unrelated scopes, secrets, tombstones, or repository metadata.
5. Semantic generation, provider/model change, stale vectors, endpoint failure,
   and missing capability all preserve keyword fallback.
6. Extraction requires explicit policy/confirmation, rejects secrets and
   malformed/duplicate candidates, and leaves state unchanged on failure.
7. Blocked Warp domains and default/`local_only` runs perform zero hosted,
   auth, telemetry, billing, or memory-sync requests.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_memory_repository -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_memory_recall -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_memory_context -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_memory -- --nocapture
```
