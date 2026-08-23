# WarpOss P2.5 Local File And Screenshot Artifacts

## Goal

Make file and screenshot artifacts durable, viewable, and exportable from local
conversation state. Artifact bytes live in a content-addressed local store;
buttons and lightbox resolve local handles, never signed URLs or hosted download
APIs.

## Artifact Repository

- Store immutable blobs under the app data directory by SHA-256 and keep
  versioned SQLite metadata for artifact UUID, kind, MIME type, size, checksum,
  managed relative locator, optional sanitized display/original path,
  description, dimensions, owner conversation/run, created timestamp,
  reference count, retention state, and tombstone.
- Never persist an absolute managed-store path; derive it from the validated
  app-owned root. Original paths are display/provenance only and are never
  blindly reopened for artifact content.
- Import files by opening a user/tool-produced path through canonical root and
  no-follow validation, reading with strict size/time limits, hashing while
  copying to a temp file, fsyncing, then atomically installing the blob and
  metadata transaction. Never modify or delete the original file.
- Import screenshots directly from validated in-memory bytes. Decode/verify
  supported image MIME, dimensions, and pixel/byte limits before commit.
- Deduplicate blobs by checksum while retaining separate ownership metadata.
  Forking a conversation increments references transactionally; deletion
  tombstones ownership and garbage-collects only unreferenced blobs after a
  recovery/retention window.

## Local Producers

- Convert successful local document/file creation and explicit artifact tool
  results into repository imports only when the output path belongs to the
  active local run/conversation and passes validation.
- Persist computer-use screenshots already returned in local action results as
  screenshot artifacts. Do not capture additional screens or background windows
  merely because artifact storage exists.
- Add a narrow explicit `SaveLocalArtifact` action/tool only if existing local
  result types cannot express the producer. It accepts a validated local result
  handle, not arbitrary base64/path chosen by the model.
- Plan notebooks continue using their existing local notebook identity. Pull
  request artifacts remain ordinary external links and are never mirrored or
  required for file/screenshot operation.

## UI And File Operations

- Resolve screenshot buttons to local blob asset sources and reuse the existing
  multi-image lightbox in stable conversation order. Missing/corrupt blobs show
  a per-image local error without a network request.
- Replace `DownloadFile` hosted semantics with `Open`, `Reveal in Finder`, and
  `Save a copy`. Open/reveal resolves the managed file; save-copy uses an OS
  picker and never overwrites without explicit confirmation.
- Display sanitized filename, MIME, size, description, checksum prefix, and
  local-only badge. Never show storage roots, signed URLs, server IDs, or auth
  errors.
- Verify checksum and regular-file identity before every open/copy/lightbox
  load. Refuse symlink/device/directory/path-traversal substitutions and mark a
  corrupt artifact for repair/removal.

## Persistence And Migration

- Introduce a local artifact variant/locator while continuing to deserialize
  legacy hosted artifact UUID rows. Legacy hosted-only rows remain visible as
  unavailable historical metadata; they never trigger a fetch.
- Conversation SQLite persistence stores artifact UUID/reference metadata, not
  raw blobs. Repository commit must complete before emitting
  `UpdatedConversationArtifacts`; failed imports leave conversation artifacts
  unchanged.
- On startup reconcile temp files, metadata/blob checksum, orphan references,
  and interrupted writes. Do not garbage-collect user originals or unknown
  files under an unexpectedly changed root.
- Enforce per-artifact, per-conversation, and total-store quotas with local
  settings and a previewable cleanup screen. Quota failure is explicit and
  cannot upload elsewhere.

## Tests

Start with failing repository/import/resolver tests. Cover at least:

1. File and screenshot import, checksum/dedup/reference counts, conversation
   restart, fork, delete/tombstone, retention, and garbage collection.
2. Atomic-write interruption, SQLite failure, corrupt/truncated blob, missing
   blob, stale metadata, and startup recovery without losing valid artifacts.
3. Path traversal, outside-root and swapped symlink, directory/device/FIFO,
   oversized file/image, unsupported MIME, malformed image, and pixel bomb are
   rejected before conversation mutation.
4. Lightbox resolves ordered local screenshots; open/reveal/save-copy operate on
   the verified managed blob and never overwrite silently.
5. Local document and computer-use producers create artifacts only after
   successful owned results; failed/cancelled/unowned actions create none.
6. Legacy hosted UUID rows remain inert and produce no signed-URL/download/auth
   request with Warp domains blocked.
7. Quotas/cleanup are deterministic and default/`local_only` behavior is
   identical.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_artifact_repository -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_file_artifact -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp local_screenshot_artifact -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_artifact -- --nocapture
```
