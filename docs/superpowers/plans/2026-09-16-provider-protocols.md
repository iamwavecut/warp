# Provider Protocols Implementation Plan

> Use subagent-driven-development for the protocol adapter and independent review. Repository instructions override approval, commit, worktree and interleaved-testing defaults.

**Goal:** Support direct OpenAI Chat Completions, OpenAI Responses and Anthropic Messages per endpoint, with a mutable display alias and prompt caching enabled by default.

**Architecture:** Preserve custom/provider/model identity and existing local agent/tool orchestration. Convert the existing internal request into the selected wire protocol and normalize responses before existing tool validation. Keep aliases independent of name-keyed model IDs and credentials.

**Constraints:** No hosted Warp path, new dependencies or remote calls with real keys. Delivery is authorized on `master` to `fork/master`, with the verified bundle installed as `/Applications/WarpOss.app`. Default legacy protocol remains `open_ai_compatible`. Tests/builds only after all code changes. Work on clean master as instructed. Caching means prompt-prefix reuse, never replaying completed answers. Responses uses store=false. Never silently claim a universal server-cache disable where the protocol does not support one.

## Task 1: Protocol transport

- [x] Add Responses and Messages request/response adapters, SSE and non-streaming decoding, auth, vision, tool history, tool definitions/results, completion/error validation.
- [x] Add default-on per-route caching from config. Anthropic uses top-level ephemeral cache_control; disabled omits it. Responses implicit default; explicit-only mode without breakpoints disables supported endpoints. Document older model limits. Chat-compatible caching remains provider-managed.
- [x] Keep all agent, plain-text and compaction consumers on selected protocol. Add mock-wire regression coverage (success, tool turn, malformed/truncated/error, caching on/off, auth, stream boundaries).

## Task 2: Settings and UI

- [x] Extend CustomApiType with OpenAiResponses and AnthropicMessages; fields alias: Option<String>, prompt_caching: bool default true; display_name falls back to name.
- [x] Expose alias, protocol and caching per endpoint; retain edits through saves and rebuilds; use alias in model labels without changing routing or keys.
- [x] Adapt discovery authentication and parsing to selected protocol. Add configuration/editor/label tests; update AGENTS.md and focused provider documentation.

## Task 3: Review and verification

- [x] Independently review combined diff and fix concrete defects.
- [x] cargo fmt --check, targeted provider/protocol/editor tests, default and compatibility all-target builds, debug bundle.
- [x] Clean task-owned Cargo cache after the verification batch, preserving and verifying bundle. Interactive UI inspection was deferred to avoid restarting the running app; editor tests and both all-target builds passed.

## Verification result

All checks passed using per-process `DEVELOPER_DIR=/Library/Developer/CommandLineTools`; the system Xcode selection was unchanged. Test filters: `direct_openai` (116 passed), `custom_provider` (29), `settings_view::ai_page::tests` (28), and `local_compaction` (9). Both all-target build variants passed. The debug bundle was created without launching, passed strict deep codesign verification, and retained the same executable SHA-256 after the prescribed Cargo cleanup. `git diff --check` passed. Live provider calls with real credentials were not performed.

## Installation

Installed the verified debug bundle as `/Applications/WarpOss.app` and registered it with Launch Services. All 115 file, directory and symlink manifest entries matched the source; strict deep codesign verification passed. Executable SHA-256: `52dfbcf9abf5d421d7eba76c46b7227892ada5808faf6aa70b1cf56a1d425bfc`. Existing terminal sessions were left running.
