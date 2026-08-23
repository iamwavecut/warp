# WarpOss Local-First Feature Backlog

Last verified: 2026-08-23

## Purpose

This is the implementation backlog produced by the read-only audit of lost or
disabled upstream functionality that can be restored without mandatory Warp
Cloud, account/auth, billing, telemetry, or another imposed hosted service.

Allowed dependencies are local processes, SQLite, the filesystem, operating
system APIs, MCP, and user-configured OpenAI-compatible endpoints. A configured
endpoint may run locally or remotely, but Warp domains must never be a fallback.

## Audit Baseline

- Original audited fork tree: `master` / `fork/master` at `3f23a5a3d0a4`.
- Original audited upstream tree: `origin/master` at `e722ebeda286`.
- Current local implementation head: `d4b611c1`.
- Current fetched upstream: `origin/master` at `702aa106`, recorded as an
  ancestor of the local branch.
- `e2a08021` was merged semantically as `c66af8fb`: upstream type/API changes
  were reconciled while the fork's local `TeamContext` behavior stayed intact.
- The later range `19548aec..702aa106` consists only of Teams/TUI scope UI and
  hosted cloud-agent-environment cloning. It was recorded by the semantic
  rejection merge `b43125ed` with the local tree unchanged; accepting those
  surfaces would violate the explicit no-Teams/no-hosted-agent policy.

The upstream ref is already descended from the audited upstream ref, while the
fork contains the local-first delta. Comparisons therefore use final trees and
semantic gates, not only an unmerged-commit list.

## Priority Definitions

- **P0:** existing local state and no more than two engineering days per item.
- **P1:** one new adapter or storage boundary, approximately three to seven days.
- **P2:** a new lifecycle, scheduler, or substantial migration, more than a week.
- **Reject:** the feature's meaning requires a forbidden hosted service or data
  disclosure.

## Required Acceptance Matrix

Every implemented feature must be tested with:

1. a local OpenAI-compatible endpoint when an LLM capability is required;
2. a user-configured remote OpenAI-compatible endpoint;
3. no configured provider;
4. a provider/model without the required capability;
5. Warp domains blocked, with no auth, telemetry, incident, or hosted-AI request.

State described as local must survive application restart. Provider and model
errors must degrade locally and clearly, without switching to Warp.

## P0 — Do Now

Status: **complete** at `11868d1a`; the later upstream-recording merge
`b43125ed` leaves that verified tree unchanged. Focused default and
`local_only` tests, both all-target builds, bundle creation, strict codesign,
isolated keyless/no-provider/provider UI paths, restart persistence, and an
empty hosted-fallback proxy capture all passed.

### P0.1 Allow OpenAI-compatible endpoints without an API key

**Delivered:** `59699a0d`, `7cfe4190`, `ece1335e`.

**Evidence**

- `app/src/settings/ai.rs` allows a provider without a key.
- `app/src/ai/agent/api/direct_openai.rs` accepts `Option<String>` and omits
  `Authorization` when no key is configured.
- `app/src/settings_view/ai_page.rs` currently requires a direct key or an
  API-key environment variable before checking `/models`.

**Implementation**

- Make the provider connection signature support an absent key.
- Call `fetch_models(base_url, None)` for unauthenticated providers.
- Preserve explicit errors for an unset named environment variable.
- Do not add any provider-specific or Warp discovery request.

**Capabilities:** `chat`; optional `tools`.

**Estimate / risk:** 0.5–1 day / low.

**Regression tests**

- `/models` and `/chat/completions` receive no `Authorization` header.
- An unset configured env var remains an error.
- An unreachable endpoint reports a local connection error.
- No Warp request is made.

### P0.2 Replace the disabled-server `SearchCodebase` fallback

**Delivered:** `dec41b80`, `4786a545`.

**Evidence**

- `app/src/ai/get_relevant_files/controller.rs` builds a local repository
  outline, but for two or more candidates calls `ServerApi::get_relevant_files`.
- `app/src/server/server_api.rs` always returns `backend_disabled` from that
  method.
- `app/src/ai/blocklist/action_model/execute/search_codebase.rs` routes local
  `SearchCodebase` actions through this controller.

**Implementation**

- Rank local outline candidates deterministically by partial-path match,
  filename/symbol match, and local text search.
- Keep the existing semantic-index branch when a future local `StoreClient` is
  available.
- Never invoke `ServerApi` for a local repository.

**Capabilities:** none; optional `chat` only for a later reranker.

**Estimate / risk:** 1–2 days / medium.

**Regression tests**

- Multi-file search succeeds with Warp domains blocked.
- Partial paths and symbol/content matches produce stable results.
- Empty, missing, and invalid repositories produce explicit local failures.

### P0.3 Restore local conversation rename and title persistence

**Delivered:** `ca42d68e`, `75278411`, `11868d1a`. The final semantic
pane route was manually verified live and after restart; the root task contains
the new title while the cosmetic vertical-tab override is absent.

**Evidence**

- Upstream `app/src/ai/conversation_rename.rs` and the conversation-list view
  contain inline and slash-command rename behavior but require a server token
  and server rename call.
- `app/src/ai/agent/conversation.rs` already derives a title from the root task
  description and persists conversation tasks through SQLite.
- The fork removed the local title mutator and rename UI/actions.

**Implementation**

- Restore validation, inline rename, and `/rename-conversation` without the
  server-token gate or API call.
- Update the root task description, persist through the existing conversation
  writer, and emit a local title-change event for list/pane metadata.
- Optionally generate a concise title after the first response with the existing
  direct `complete_text` path; failure leaves the initial query title intact.

**Capabilities:** none for manual rename; `chat` for optional generation.

**Estimate / risk:** 1–2 days / medium.

**Regression tests**

- Inline and slash rename update every visible title surface.
- Empty and over-500-character titles are rejected.
- Rename survives restart and works with no provider.
- Generated-title failure preserves the existing title and never calls Warp.

### P0.4 Show local prompts and skills outside CloudMode

**Delivered:** `505cb74c`, `992815cd`, `77d1b98d`; startup custom-model
catalog integration was completed by `5e260010`, `9c6d326f`, and `8f67b28d`.

**Evidence**

- `app/src/workflows/workflow.rs` already supports `Workflow::AgentMode`.
- `app/src/workflows/local_workflows.rs` and `app/src/user_config/native.rs`
  already load and watch local workflow files.
- `app/src/terminal/input/slash_commands/data_source/zero_state.rs` currently
  exposes local prompts and zero-state skills only when `is_cloud_mode_v2`.

**Implementation**

- Remove the CloudMode condition from local prompt and local skill discovery.
- Keep all cloud workflow and saved-prompt sources disabled.
- Preserve the current filesystem watcher and local prompt action.

**Capabilities:** none for discovery/editing; `chat` and optional `tools` when
the prompt is executed.

**Estimate / risk:** 0.5–1 day / low.

**Regression tests**

- Local prompts and skills are visible with CloudMode disabled.
- A file watcher update refreshes the menu.
- Editing/discovery works without a provider and survives restart.

## P1 — Provider And Context Foundation

### P1.1 Add a provider capability contract

**Delivered in code:** `86795248`, `00d3e8f9`, `c901b93e`, `0f168bf4`,
`b839b007`, `936d30c3`. Independent review found no remaining findings.
Focused default and `local_only` tests, both all-target builds, bundle creation,
and strict codesign passed. Isolated screen-level acceptance remains a final
gate because macOS was locked when the new bundle became available.

Introduce a local capability definition for `chat`, `tools`, `vision`,
`embeddings`, `transcription`, and context-window size. User configuration is
the source of truth; safe endpoint probes may assist but must not contact Warp.

Current evidence:

- `CustomProviderConfig` contains only name, URL, models, key reference, and API
  type.
- `app/src/ai/llms.rs` marks custom models non-vision and uses an empty context
  window.
- `direct_openai.rs` truncates context using a hard-coded 24,000-character cap.

**Estimate / risk:** 4–7 days / medium-high.

### P1.2 Complete the direct OpenAI tool adapter

**Delivered in code:** `953075f0`, `cf6f97db`, `279ed8a3`, `87cb76e2`,
`0123d2ab`, `b5083388`. Five semantic fix cycles resolved the independent
review findings; the final reviewer approved the complete adapter without
remaining Critical, Important, or Minor findings.

Focused default and `local_only` suites each passed all 58 direct-provider
tests. Both all-target builds, bundle creation, executable and directory
checks, and strict codesign also passed. The verified bundle binary SHA-256 was
`175803bef64d024f2e61b6cfafe05d2af4b9a411ee41daea8e1c229c02b49eae`.

Existing local executors support more actions than the direct adapter can
advertise or parse. Add schemas/parsers for local documents and plans, code
review comments, `ask_user`, conversation fetch, computer use, long-running
shell control, and new-conversation suggestions.

Do not retry a request automatically after a side-effectful shell or MCP action
has started.

**Capabilities:** `chat + tools`.

**Estimate / risk:** 4–7 days / medium-high.

### P1.3 Deliver vision attachments to custom providers

**Delivered in code:** `20fe7848`, `4437101c`, `a69bba1b`. Two semantic
fix cycles resolved the independent review findings around persisted context,
aggregate text budgeting, MIME validation, parallel tool chronology, context
deduplication, and ordered local history. The final reviewer approved the
cumulative implementation without remaining Critical, Important, or Minor
findings.

The final combined P1.3/P1.4 gate passed 81 direct-provider tests in both
default and `local_only` modes, both all-target builds, bundle creation, and
strict codesign verification.

Replace string-only message content with OpenAI-compatible content parts. Send
validated images as bounded `data:` payloads, read text attachments locally,
and reject unsupported binary content without fallback.

**Capabilities:** `chat + vision`.

**Estimate / risk:** 3–5 days / medium-high.

### P1.4 Add local transcription adapters

**Delivered in code:** `4b8e0271`, `6e893710`, `7eebf5d9`, `67e92af5`,
`099988b0`. Three semantic fix cycles removed the remaining hosted entitlement
gate, made provider selection and configured key environments fail closed,
hardened WAV validation, refreshed existing voice views on route changes, and
surfaced validation and persistence failures in the provider UI. The final
independent reviewer approved the cumulative implementation without remaining
blocking findings.

Focused transcription suites passed all 8 tests in both default and
`local_only` modes. The shared P1.3/P1.4 gate also passed both all-target builds,
bundle creation, executable checks, and strict codesign. The verified bundle
binary SHA-256 was
`3399d197e0653b837d89d53b22d31fd96d94d82a7074a3b9aa55bc236a8c11c6`.

Use the existing `Transcriber` boundary and voice capture UI with either a
user-configured `/audio/transcriptions` endpoint or an explicitly configured
local process. Do not bundle or silently download a model.

**Capabilities:** `transcription`.

**Estimate / risk:** 3–5 days / medium.

### P1.5 Persist and enable the prompt queue

**Delivered in code:** `bdd6b8357`, `71f73b3c4`, `e3ed1578b`, `d4b611c1`.
The local SQLite queue now persists prompt and command rows, attachments,
ordering, retries, lock and dispatch state, and per-conversation settings.
Semantic fixes made persistence fail closed, isolated queued attachments from
the current draft, correlated dispatch/completion with the owning conversation
and shell block, preserved uncertain rows for explicit retry, and kept pane
closure distinct from conversation deletion.

Focused default and `local_only` suites passed 35 queue tests and 8 panel tests
in each mode; the final conversation-scoped shell regression also passed. The
all-target and bundle gate is batched with P1.6 and P1.7.

The queue model, panel, reorder/edit behavior, and `/queue` path already exist,
but the feature is not default and its state lives only in memory. Add a local
repository for prompt rows, attachments, lock state, origin, and attempts.

The next row may run only after the current local command/tool result reaches a
terminal state.

**Capabilities:** none for queueing; `chat`, `tools`, or `vision` depend on the
queued item.

**Estimate / risk:** 3–7 days / medium.

## P1 — Local Productivity

### P1.6 Local saved-prompt/workflow CRUD

Reuse the existing workflow editor but write `Workflow::AgentMode` YAML
atomically to the local workflows directory. Add stable local IDs and CLI
`--saved-prompt` resolution. Never use `CloudModel` or `UpdateManager`.

**Estimate / risk:** 3–5 days / medium.

### P1.7 Local rule CRUD

Make file-backed global and project rules editable through the existing UI.
Write atomically, preserve rule precedence, and protect against accidental
overwrite and permission errors. Existing watchers and direct-provider context
injection remain the read path.

**Estimate / risk:** 3–7 days / medium.

### P1.8 Local custom model routers

Restore the upstream local YAML definitions, but resolve a router to a concrete
`custom/<provider>/<model>` before the direct request. Do not serialize router
definitions into Warp protobuf. Start with deterministic rules; leave an
optional model-based classifier for P2.

**Estimate / risk:** 5–7 days / medium-high.

### P1.9 Local named-agent bundles

Persist named configurations containing prompt, model ID, execution profile,
MCP specs, and local harness. Store only env-var or keychain references for
secrets. Reuse the current one-shot JSON/YAML config merge path.

**Estimate / risk:** 3–7 days / high.

### P1.10 Resume local CLI harnesses

Reuse only the local JSONL discovery and rehydration portions of upstream Codex
and Claude transcript code. Maintain a local conversation-to-session index and
never upload transcripts or block snapshots.

**Estimate / risk:** 4–7 days / medium-high.

### P1.11 Same-process local child agents

Expose `StartAgent`, `RunAgents`, and `SendMessageToAgent` through the direct
tool adapter and existing local executors. Add local ownership, cancellation,
topology/status, and notification handling. Durable cross-process delivery is a
separate P2 lifecycle.

**Estimate / risk:** 5–7 days / high.

## P2 — Local Redesigns

### P2.1 Real local `/compact`

Generate a typed summary with `chat`, validate it, then use the existing
message-splice/task machinery and SQLite persistence to replace old context.
Never mutate history before the summary succeeds, and preserve tool chronology.

### P2.2 Local semantic code index

Implement `LocalStoreClient` for chunks, Merkle state, vectors, query, and
optional reranking. Use a configured `/embeddings` endpoint or local process and
retain lexical search as the provider-free fallback.

### P2.3 Local long-term memory

Add scoped SQLite memory with explicit CRUD/versioning and bounded keyword
retrieval. Semantic recall and automatic extraction are optional layers using
`embeddings` and `chat`; both must degrade to explicit/manual memory.

### P2.4 Local scheduler and durable background lifecycle

Add a schedule repository, local supervisor, durable event journal/cursors,
timezone and missed-run policy, cancellation, `WaitForEvents`, and OS
notifications. Hosted ambient spawn/poll code is not reusable.

### P2.5 Local file and screenshot artifacts

Store local path, MIME type, checksum, and owner metadata. Implement safe
open/reveal/lightbox and lifecycle cleanup without signed URLs or remote upload.

Each P2 item is estimated at more than one week with high lifecycle, migration,
privacy, or correctness risk.

## Reject

- Shared sessions, Teams, ACLs, Shared Blocks, and Warp Drive sharing.
- Cloud environments, hosted runners, remote handoff, and hosted child agents.
- Warp-hosted Factory/well-known MCP catalogs and mandatory managed OAuth.
- Hosted memory synchronization, server-ranked retrieval, and hosted schedules.
- Account, auth, billing, subscription, quota, telemetry, and incident upload.

## Already Local — Do Not Reimplement

- Direct OpenAI-compatible `POST /chat/completions` routing.
- Local custom-provider model selection and key/env resolution.
- User-configured MCP resources/tools and local skills.
- SQLite conversation history/restoration and conversation export.
- Local Plan artifacts and execution-profile persistence.
- AI code review, commit/PR text generation, and command/workflow metadata via
  the direct endpoint.

The current `/export-to-file` implementation warns and then immediately
overwrites an existing file. That is a separate correctness issue, not a lost
feature restoration.

## Implementation Order

1. Complete all P0 items.
2. Add provider capabilities and direct tool parity.
3. Add vision, transcription, and durable queueing.
4. Add local prompts, rules, routers, named agents, and CLI resume.
5. Add same-process child agents before any P2 lifecycle work.
6. Implement P2 items independently behind local repositories and explicit
   capability checks.

## Completion Rule

A backlog item is complete only when its targeted regression tests pass, local
state survives restart where promised, failures do not contact Warp, and the
default and `local_only` builds retain identical local-first behavior.
