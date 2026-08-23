# WarpOss P1.7 Local Rule CRUD

## Goal

Make the existing file-backed global and project rules editable from the Rules
UI while retaining the current watcher, precedence, and direct-provider context
path. Remove cloud rule CRUD from the visible local-first surface.

## Repository Boundary

- Add a local rule repository whose identity is the validated canonical file
  path plus a content revision/hash.
- Managed global creation targets exactly `~/.agents/AGENTS.md`. Managed project
  creation targets the selected indexed project root and an explicit supported
  filename (`WARP.md` by default, `AGENTS.md` only when selected).
- Edit and delete accept only paths already surfaced by
  `ProjectContextModel`; creation accepts only the two resolved managed targets.
  Reject traversal, symlink escapes, non-regular files, and roots that change
  during the operation.
- Use compare-and-swap semantics: fail with a conflict if content/metadata no
  longer matches the revision opened in the editor. Never overwrite a newer
  external edit.
- Write through a same-directory temporary file, preserve reasonable existing
  permissions, flush/sync, and atomically rename. A failed write leaves the old
  file untouched.
- Deletion requires explicit UI confirmation, removes only the exact managed
  file, and reports permission/race errors locally. Do not delete parent dirs.

## UI And Context

- Replace visible `CloudAIFact`/personal-drive/`UpdateManager` rule actions with
  file-backed rows and the existing editor adapted to local content/path/revision.
- Show Add/Edit/Delete for writable local rows and a clear read-only state for
  unwritable files. Keep Open File.
- Save/close state changes only after repository success. Watcher events refresh
  the row and `ProjectContextModel`; avoid duplicate success events.
- Preserve precedence exactly as implemented:
  global rules first, project `WARP.md` shadows project `AGENTS.md` in the same
  directory, and applicable ancestor project rules remain ordered by the model.
- The existing `AIAgentContext::ProjectRules` → direct OpenAI prompt path remains
  the only LLM integration. Editing works with no provider.

## Boundaries

- No cloud facts, owner/space, Warp Drive, sync IDs, auth, telemetry, or server
  privacy/settings calls.
- Do not invent a second rule format or database copy; files are the source of
  truth.
- Do not silently merge concurrent Markdown edits. Report the conflict and let
  the user reload/copy their draft.

## Tests

Start with failing repository and UI-model tests. Cover at least:

1. Create/edit/delete global and project rules, watcher refresh, and restart
   persistence.
2. Concurrent external edit, permission failure, symlink replacement, path
   traversal, missing root, and atomic-rename failure preserve existing data.
3. Writability/action state and dirty editor behavior on success/failure.
4. Global/project precedence and `WARP.md`/`AGENTS.md` shadowing remain exact.
5. Direct-provider context includes the saved content after watcher refresh;
   provider absence does not block CRUD and no Warp request occurs.
6. Default and `local_only` behavior is identical.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_rule -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p ai project_rule -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_rule -- --nocapture
```

