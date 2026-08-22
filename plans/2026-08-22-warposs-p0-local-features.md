# WarpOss P0 Local-First Features Implementation Plan

> Status: ready to execute after the pending `origin/master` merge is explicitly authorized and resolved under the local-first policy. The merge is not part of this plan's code changes.

**Goal:** Implement every P0 item in `LOCAL_FIRST_FEATURE_BACKLOG.md` without Warp Cloud, account/auth, telemetry, billing, or an implicit hosted-provider fallback.

**Architecture:** Keep the existing direct OpenAI-compatible adapter, SQLite conversation writer, repository outline, filesystem workflow watcher, and skill manager as the boundaries. Remove hosted gates at their consumers, add deterministic local behavior where the fork currently calls a disabled server method, and restore only the local half of upstream conversation rename.

**Tech stack:** Rust, WarpUI model/view actions, Tokio, SQLite model events, existing `mockito` HTTP tests, repository outline/index types.

**Execution rule:** Follow red-green-refactor for each task. Do not run broad verification until all P0 production changes are complete. Checkpoint diffs locally; create commits only after separate explicit authorization.

---

## Prerequisite: synchronize the screened upstream commit

**Files affected by the upstream conflict:**

- Modify during conflict resolution: `app/src/workspaces/user_workspaces.rs`
- Verify incoming tests: `app/src/workspaces/user_workspaces_tests.rs`

1. Confirm the worktree contains only the backlog and this plan.
2. After explicit authorization, create the required `backup/pre-upstream-merge-<timestamp>` branch and merge `origin/master` with `--no-ff`.
3. Resolve the `TeamContext` conflict by accepting upstream type/API terminology only where it does not restore Teams, account metadata, cloud sync, or hosted authorization behavior.
4. Inspect the final merged diff for forbidden network/auth/product surfaces before starting P0 edits.
5. Do not build or test yet; verification remains the final phase after all P0 changes.

## Task 1: Allow unauthenticated OpenAI-compatible endpoints

**Files:**

- Modify: `app/src/settings_view/ai_page.rs`
- Modify tests: `app/src/settings_view/ai_page_tests.rs`
- Modify tests: `app/src/ai/agent/api/direct_openai.rs`

### Step 1: Write failing connection-resolution tests

Extract the editor-independent connection resolution into a small private helper and test these cases in `ai_page_tests.rs`:

- a valid base URL with no direct key and no environment-variable name returns `api_key: None`;
- a direct key returns `Some(key)` and a stable keyed signature;
- a configured but unset environment variable is an error;
- an empty or invalid base URL is an error.

Run only the new test filter and confirm the keyless case fails before production changes:

```sh
cargo test -p warp provider_connection_without_key -- --nocapture
```

### Step 2: Represent an absent key in the validation signature

Change `ProviderConnectionSignature` and connection-resolution output so absence is distinct from an empty secret. Fingerprint only `Some(api_key)`. Preserve `$NAME` normalization and the explicit error when the user names an environment variable that is unset or empty.

### Step 3: Pass the optional key to `/models`

Change `LLMProviderModelsPicker::validate_provider` to call:

```rust
direct_openai::fetch_models(&base_url, api_key.as_deref())
```

Make `can_add_models` compare the new signature without requiring a key.

### Step 4: Add the HTTP regression test

In the existing inline `direct_openai` tests, add a mock `/v1/models` endpoint that succeeds only when no `Authorization` header is sent. Keep the existing keyed test.

Run the two focused filters:

```sh
cargo test -p warp provider_connection_without_key -- --nocapture
cargo test -p warp fetches_openai_compatible_model_ids -- --nocapture
```

Expected result: keyless and keyed model discovery both pass, and no Warp client is involved.

## Task 2: Replace the hosted `SearchCodebase` fallback

**Files:**

- Modify: `app/src/ai/get_relevant_files/controller.rs`
- Modify: `app/src/ai/get_relevant_files/api.rs` only if a local ranking result type is needed
- Add or modify tests: `app/src/ai/get_relevant_files/controller_tests.rs`
- Modify module declaration: `app/src/ai/get_relevant_files/mod.rs` if the test module is separate

### Step 1: Write failing deterministic-ranking tests

Test a pure local ranking function with synthetic `FileContext` values. Cover:

- an exact partial-path segment outranks a filename substring;
- a filename match outranks a symbol-only match;
- all query tokens contribute deterministically;
- ties use normalized path order;
- an empty query or empty candidates returns no invented path;
- the result limit is stable and duplicate paths are removed.

Run and confirm the new filter fails because the ranker does not exist:

```sh
cargo test -p warp ranks_local_outline_candidates -- --nocapture
```

### Step 2: Implement the pure outline ranker

Normalize query, paths, filenames, and symbols with lowercase alphanumeric tokens. Use a documented integer score with this order:

1. partial-path or exact path-token match;
2. filename/stem match;
3. symbol token/prefix match;
4. remaining symbol substring match.

Sort by descending score and then ascending normalized path. Exclude zero-score candidates unless the query is empty and the caller has fewer than two outline files, preserving the current small-repository behavior.

### Step 3: Remove `ServerApi` from the local branch

In `send_local_request`, keep `FullSourceCodeEmbedding` first. When only the outline is available:

- map its `FileContext` candidates through the local ranker;
- join results to the outline base path;
- validate that each path exists and remains under the repository root;
- emit `GetRelevantFilesControllerResult::Locations` synchronously;
- never construct or call `ServerApi::get_relevant_files` for a local target.

Leave the remote-session branch unchanged; it is a separate target and remains subject to existing local-first visibility gates.

### Step 4: Add controller behavior tests

Use a temporary local repository tree and a complete outline fixture. Verify multi-file search returns stable, existing whole-file locations without creating an abort handle or requiring a backend. Verify missing/pending/failed outline states keep their explicit local errors.

Run the focused tests:

```sh
cargo test -p warp local_outline -- --nocapture
cargo test -p warp search_codebase -- --nocapture
```

## Task 3: Show local prompts and skills without CloudMode

**Files:**

- Modify: `app/src/terminal/input/slash_commands/data_source/zero_state.rs`
- Modify tests: `app/src/terminal/input/slash_commands/data_source/mod_tests.rs`
- Inspect affected behavior: `app/src/terminal/input/slash_commands/data_source/gui.rs`

### Step 1: Write failing gate tests

Add small behavior helpers used by `GuiZeroStateDataSource::run_query` and test that:

- local prompts are eligible when AI is enabled and `is_cloud_mode_v2` is false;
- local skills are eligible when AI and `ListSkills` are enabled and `is_cloud_mode_v2` is false;
- disabling AI still hides actions that cannot execute;
- disabling `ListSkills` hides only skills, not local prompts.

Run and confirm the CloudMode-disabled assertions fail:

```sh
cargo test -p warp local_zero_state_sources -- --nocapture
```

### Step 2: Remove only the hosted-mode condition

Remove `is_cloud_mode_v2` from the local prompt and local skill discovery conditions. Keep:

- `AISettings::is_any_ai_enabled` execution readiness;
- `FeatureFlag::ListSkills` for skills;
- CLI-provider compatibility filtering;
- the filesystem-backed `WarpConfig::local_user_workflows` source;
- existing compact-layout behavior.

Do not enable cloud workflows, saved cloud prompts, or remote skill sources.

### Step 3: Verify query and zero-state behavior

Run:

```sh
cargo test -p warp local_zero_state_sources -- --nocapture
cargo test -p warp slash_command -- --nocapture
```

Expected result: filesystem prompts and skills are discoverable in the normal local UI, while cloud-only sources remain absent.

## Task 4: Restore purely local conversation rename

**Files:**

- Add: `app/src/ai/conversation_rename.rs`
- Modify: `app/src/ai/mod.rs`
- Modify: `app/src/ai/agent/conversation.rs`
- Modify: `app/src/ai/blocklist/history_model.rs`
- Modify tests: `app/src/ai/blocklist/history_model_tests.rs`
- Modify: `app/src/search/slash_command_menu/static_commands/commands.rs`
- Modify tests: `app/src/search/slash_command_menu/static_commands/commands_tests.rs`
- Modify: `app/src/terminal/input/slash_commands/mod.rs`
- Modify tests: `app/src/terminal/input/slash_commands/mod_tests.rs`
- Modify: `app/src/workspace/view/conversation_list/view.rs`
- Modify tests near the conversation-list view, or add focused view-model tests if direct rendering is too broad

### Step 1: Restore validation tests first

Port only the pure validation behavior from upstream and test:

- surrounding whitespace is trimmed;
- empty titles are rejected;
- 500 Unicode scalar values are accepted;
- 501 are rejected;
- an unchanged title is a no-op.

Run and confirm failure:

```sh
cargo test -p warp validate_conversation_title -- --nocapture
```

### Step 2: Add the local title mutation boundary

Restore `AIConversation::update_conversation_title`, using `TaskStore::modify_root_task` and `Task::update_description`, then call the existing `write_updated_conversation_state`. Do not mutate or require server metadata, server tokens, task IDs from Warp, or an in-flight server rename map.

Expose one `BlocklistAIHistoryModel::rename_conversation_locally` operation that:

- rejects missing or empty conversations;
- returns clean domain errors for validation/not-found conditions;
- updates the loaded conversation;
- refreshes cached local navigation metadata through the existing model event path;
- writes the existing SQLite conversation state event.

### Step 3: Prove mutation and persistence events

Add history-model tests based on the existing pin-persistence fixture. Assert that rename updates `conversation.title()` immediately and that the emitted `UpdateMultiAgentConversation` contains the modified root task source. Reconstruct a conversation from the persisted record and assert the title survives restoration.

Run:

```sh
cargo test -p warp local_conversation_rename -- --nocapture
```

### Step 4: Restore slash-command routing without a server call

Port the upstream `/rename-conversation <title>` command definition and terminal dispatch. Route it directly to `rename_conversation_locally`; show local success/error toasts. Do not obtain `ServerApiProvider` or spawn a network request.

Test command scoping, required argument, no active conversation, valid rename, and invalid title.

### Step 5: Restore inline list rename

Port the upstream conversation-list edit state and actions, but replace the hosted rename helper with the local operation. Test start/cancel/finish behavior and ensure the displayed row and active pane title observe the history-model update.

Do not add automatic title generation in P0. Manual rename must work with no provider; title generation remains an optional follow-up only after the manual path is stable.

## Task 5: Final privacy and regression verification

Run verification only after Tasks 1–4 production changes are complete.

### Step 1: Focused P0 tests

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cargo test -p warp provider_connection_without_key -- --nocapture
cargo test -p warp ranks_local_outline_candidates -- --nocapture
cargo test -p warp local_zero_state_sources -- --nocapture
cargo test -p warp local_conversation_rename -- --nocapture
```

### Step 2: Local-first suites and formatting

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cargo fmt --check
cargo test -p warp --features local_only custom_provider -- --nocapture
cargo test -p warp --features local_only direct_openai -- --nocapture
cargo test -p warp --features local_only local_first_account_section_is_user_and_cloud_sections_are_hidden -- --nocapture
```

### Step 3: Static forbidden-route proof

Inspect the focused diff and use targeted searches to prove the new paths do not reference:

- `ServerApiProvider` from local search or local rename;
- Warp `/ai/multi-agent`, conversation rename, auth, telemetry, billing, or incident endpoints;
- a fallback from a missing/incompatible custom provider to Warp.

### Step 4: Final builds

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cargo build --all-targets
cargo build --features local_only --all-targets
```

### Step 5: Manual acceptance matrix

Using the built app, check:

1. a local keyless OpenAI-compatible endpoint;
2. a user-configured remote keyed endpoint;
3. no configured provider;
4. an endpoint missing a requested capability;
5. Warp domains blocked.

Restart the app and verify conversation rename plus filesystem prompts/skills persist. Confirm endpoint errors remain local and readable. Record any acceptance item that cannot be exercised locally as an explicit residual risk rather than claiming it passed.
