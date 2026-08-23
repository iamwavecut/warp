# WarpOss P1.6 Local Saved-Prompt CRUD

## Goal

Turn the existing `Workflow::AgentMode` editor and discovery path into complete
local create/read/update/delete behavior, and resolve saved prompts from the
local CLI without `CloudModel`, `UpdateManager`, Warp Drive, or auth.

## Repository

- Add a `LocalSavedPromptRepository` under the existing user workflows
  directory. Repository-managed prompts use one YAML document per file in a
  dedicated `local-prompts/` subdirectory.
- Use a UUID filename as the stable local ID; display name changes do not rename
  the file or identity. Never derive a path directly from user input.
- Serialize the existing `Workflow::AgentMode` schema so the current
  `WarpConfig` loader/watcher and zero-state discovery remain the read path.
- Create/update atomically with a same-directory temporary file, flush/sync,
  then rename. Reject collisions and preserve the prior file on serialization,
  permission, or rename failure.
- Delete only the exact repository-managed UUID file. Existing hand-written
  multi-document workflow files remain discoverable and read-only unless the
  user explicitly imports/copies an entry into managed storage.
- Surface parse and permission errors locally; never delete or overwrite an
  unparseable file automatically.

## UI

- Route Agent Mode create/edit/delete in the existing workflow editor through
  the local repository. Do not call `UpdateManager`, create cloud IDs, require an
  owner/space, or render Warp Drive/trash/access wording for local prompts.
- Preserve current secret detection, argument handling, dirty-state prompts,
  watcher refresh, and prompt execution.
- Local save must transition the editor to view mode only after the atomic write
  succeeds. A failed save remains dirty and displays an actionable toast.

## CLI

- Add `warp agent run --saved-prompt <id-or-name>` as mutually exclusive with an
  inline prompt/task source.
- Resolve UUID first, otherwise a unique exact local Agent Mode workflow name.
  Missing or ambiguous names fail locally and list no private prompt contents.
- Expand arguments through the existing workflow argument machinery, then feed
  the resulting query to the normal local harness/provider path.
- Do not resolve cloud workflow IDs and do not contact Warp.

## Tests

Start with failing repository, UI-model, watcher, and CLI tests. Cover at least:

1. Create/edit/rename/delete round-trip with stable UUID and restart reload.
2. Serialization/permission/rename failures preserve the previous file and
   leave UI dirty without a success event.
3. Malicious names cannot escape the managed directory; unmanaged and
   multi-document files cannot be overwritten/deleted through managed CRUD.
4. The existing watcher refreshes zero-state exactly once after effective
   create/update/delete.
5. CLI resolution by UUID and unique name, plus missing/ambiguous/conflicting
   argument errors.
6. No-provider editing works; execution reports the ordinary local provider
   error; blocked Warp domains receive no request.
7. Default and `local_only` behavior is identical.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp local_saved_prompt -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp_cli saved_prompt -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only local_saved_prompt -- --nocapture
```

