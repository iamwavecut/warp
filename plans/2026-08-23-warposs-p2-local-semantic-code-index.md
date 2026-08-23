# WarpOss P2.2 Local Semantic Code Index

## Goal

Implement the existing full-source-code indexing pipeline against a local
vector/Merkle store and an explicitly configured OpenAI-compatible
`/embeddings` endpoint. Semantic retrieval is an optional acceleration layer;
the shipped lexical/outline search remains the provider-free fallback.

## Provider Contract

- Extend custom-provider configuration with an explicit embedding model ID and
  optional expected vector dimension. A generic chat model ID is never assumed
  to be an embedding model.
- Resolve one stable provider local ID, base URL, optional key, embedding model,
  and effective `embeddings` capability before an indexing batch. Send only
  `POST <base_url>/embeddings`; keyless endpoints omit `Authorization`.
- Validate response count/order/indexes, finite numeric values, uniform non-zero
  dimensions, configured dimension, and bounded payload/response sizes. Never
  log source chunks, vectors, or keys.
- Provider/model/dimension identity is part of the index generation key. A
  configuration change creates a new generation and keeps the last valid one
  readable until replacement commits.

## `LocalStoreClient`

- Implement the existing `StoreClient` trait with local SQLite metadata and
  content-addressed files under the app data directory. No GraphQL or
  `ServerApiProvider` is reachable from the local manager.
- Persist repository identity, canonical root, ignore/config fingerprint,
  Merkle root/nodes, content hash, fragment path/range/language, embedding
  generation, vector blob, timestamps, and schema version.
- Use transactions for intermediate-node, fragment, and generation commits.
  A cancelled/failed batch leaves the prior committed generation intact; stale
  partial rows are garbage-collected only after recovery proves they are
  unreferenced.
- Compute cosine similarity in bounded Rust batches with deterministic
  path/range/hash tie-breaking. `rerank_fragments` is deterministic semantic +
  lexical scoring; an optional chat reranker is out of the required path.
- `sync_merkle_tree` and `populate_merkle_tree_cache` become local consistency
  checks over the committed node store, not network operations.

## Indexing Lifecycle

- Reuse the existing chunker, Merkle tree, changed-file diff, snapshot,
  `CodebaseIndexManager`, watcher, and bounded build queue. Replace its
  `ServerApi` store at app startup with `LocalStoreClient` for local repos.
- Canonicalize roots and open source files under the validated repository with
  no-follow protection. Honor gitignore/existing exclusions, file count/size,
  binary/generated/secret path filters, and cancellation between batches.
- Debounce file changes, recompute only affected chunks/nodes, and atomically
  advance the root when all required embeddings are durable. Deleted/renamed
  files disappear from the next committed generation.
- Bound CPU, concurrent HTTP batches, database size, per-repository generations,
  and total indices. Indexing must yield to interactive requests and expose
  local progress/pause/rebuild/delete controls.

## Retrieval And Degradation

- Embed the query with the same generation identity, retrieve top-K candidates,
  validate current file identity/containment, reconstruct bounded context with
  existing search-shaping helpers, and merge/dedupe by canonical path/range.
- If no embedding provider is configured, capability is disabled, a generation
  is missing/stale, or the endpoint fails, immediately use the existing local
  lexical/outline search. The UI may report semantic-index state, but the tool
  itself remains useful.
- Never send repository paths as provider metadata beyond the source text that
  the user explicitly chose to embed. Never contact Warp for limits, config,
  embeddings, retrieval, reranking, or cache population.

## Tests

Start with failing local-store and endpoint-contract tests. Cover at least:

1. Initial build, incremental edit/add/delete/rename, restart restore, and
   deterministic retrieval over a temporary repository.
2. Keyless/keyed `/embeddings`, batching/order, model selection, dimension and
   finite-value validation, cancellation, timeout, and response bounds.
3. Failed/cancelled rebuild retains the prior committed generation; crash
   recovery ignores/removes incomplete rows without corrupting Merkle state.
4. Provider/model/dimension change creates a separate generation and cannot mix
   incompatible vectors.
5. Symlink/path replacement, outside-root path, binary/oversized/ignored/secret
   file, and final-open races are rejected.
6. Missing provider/capability/index and endpoint errors fall back to lexical
   search with stable results and no Warp request.
7. Database/storage bounds, pruning, pause/rebuild/delete, and default versus
   `local_only` behavior.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p ai local_store_client -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_embedding_endpoint -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_semantic_code_index -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_semantic_code_index -- --nocapture
```
