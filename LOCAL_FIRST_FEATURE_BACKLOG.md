# WarpOss Local-First Feature Backlog

Last verified: 2026-08-24

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
- Current local implementation tree: through `070f28a1`.
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
shared P1.5–P1.7 gate passed both all-target builds, bundle creation, executable
checks, and strict codesign.

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

**Delivered in code:** `67c086afb`, `68f477ba`. Managed AgentMode prompts use
stable UUID files and atomic local writes, preserve complete workflow and
argument metadata, support local editor create/update/delete and UUID-backed
pane restore, and resolve CLI `--saved-prompt` by UUID or unique exact name.
The semantic fix pass preserved identity for duplicate-content prompts, added
dirty-delete confirmation and actionable filesystem errors, and coalesced the
watcher to one effective refresh.

Focused repository/UI suites passed 11 tests in both default and `local_only`
modes; the CLI suite passed 4 tests. The shared P1.5–P1.7 all-target and bundle
gate passed.

Reuse the existing workflow editor but write `Workflow::AgentMode` YAML
atomically to the local workflows directory. Add stable local IDs and CLI
`--saved-prompt` resolution. Never use `CloudModel` or `UpdateManager`.

**Estimate / risk:** 3–5 days / medium.

### P1.7 Local rule CRUD

**Delivered in code:** `a29ee822b`, `bf50a37fd`. The Rules UI now edits
file-backed global and project rules through a no-follow, directory-FD anchored
repository with revision CAS, rollback-safe atomic publication, exact managed
targets, explicit delete confirmation, read-only error rows, multi-project Add,
and a local suggested-rule flow. Cloud fact, personal-drive, and UpdateManager
CRUD are absent from the visible path; no unsafe Rust was introduced.

Focused default and `local_only` UI suites passed 6 tests each; repository and
precedence suites passed 6 and 2 tests. The shared P1.5–P1.7 gate passed both
all-target builds, bundle creation, executable checks, and strict codesign. The
verified bundle binary SHA-256 was
`68eb252f38a380ec6ca9cfdd49b90b70712caad304ad03cbeb0ba95bac30f2da`.

Make file-backed global and project rules editable through the existing UI.
Write atomically, preserve rule precedence, and protect against accidental
overwrite and permission errors. Existing watchers and direct-provider context
injection remain the read path.

**Estimate / risk:** 3–7 days / medium.

### P1.8 Local custom model routers

**Delivered in code:** `16d987e15`, `91da4f64d`, `8ac3b8da4`. Local YAML
routers now have strict bounded parsing, stable file identity, secure no-follow
expected-revision CRUD, watcher reconciliation, deterministic complexity and
prompt resolution, conservative capability/context limits, concrete custom
model routing, and a full Add/Edit/Delete settings UI. Invalid routers fail
closed without substituting another provider or contacting Warp.

Focused router suites passed 13 tests in both default and `local_only` modes;
the editor suite passed 4 tests and the direct-provider suite passed 81 tests.
The shared P1.8–P1.11 gate passed default and `local_only` all-target builds,
bundle creation, executable checks, and strict codesign. The verified bundle
binary SHA-256 is
`da3f67c0169f5be75938b7bdf2a9aa3db130d9eead984b14189fce159c9fa3ea`.

Restore the upstream local YAML definitions, but resolve a router to a concrete
`custom/<provider>/<model>` before the direct request. Do not serialize router
definitions into Warp protobuf. Start with deterministic rules; leave an
optional model-based classifier for P2.

**Estimate / risk:** 5–7 days / medium-high.

### P1.9 Local named-agent bundles

**Delivered in code:** `fbe87792b`, `40aa33f85`. Named agents now use strict
UUID YAML bundles, secure no-follow expected-revision CRUD, watcher refresh,
secret-safe validation, deterministic config precedence, one-shot preflight,
local profile/skill/MCP/harness resolution, direct OpenAI-compatible Oz
execution, and UUID/revision/non-secret effective resume metadata. The local
management UI and CLI CRUD/run/list paths contain no hosted agent rows, owner,
environment, managed-secret, auth, or upload flow.

Focused default and `local_only` suites passed 11 tests each; the CLI suite
passed 2 tests. The shared P1.8–P1.11 all-target build and signed-bundle gate
passed with the bundle SHA-256 recorded under P1.8.

Persist named configurations containing prompt, model ID, execution profile,
MCP specs, and local harness. Store only env-var or keychain references for
secrets. Reuse the current one-shot JSON/YAML config merge path.

**Estimate / risk:** 3–7 days / high.

### P1.10 Resume local CLI harnesses

**Delivered in code:** `6f73fa136`, `bcac94830`. Codex and Claude Code runs
now have stable local run IDs, a versioned secret-free session index, secure
no-follow transcript binding, advisory locking, atomic revision updates,
capability preflight, canonical Claude index/cwd handling, fatal final-save
semantics, CLI `--resume`, and child-run task association. Resume uses only
third-party CLI files and never calls Warp harness-support, auth, snapshot, or
upload APIs.

Focused default and `local_only` lifecycle suites passed 14 tests each; the CLI
suite passed 1 test. The shared P1.8–P1.11 all-target build and signed-bundle
gate passed with the bundle SHA-256 recorded under P1.8.

Reuse only the local JSONL discovery and rehydration portions of upstream Codex
and Claude transcript code. Maintain a local conversation-to-session index and
never upload transcripts or block snapshots.

**Estimate / risk:** 4–7 days / medium-high.

### P1.11 Same-process local child agents

**Delivered in code:** `44641eff2`, `43db79a65`, `be8b3513a`. The direct
OpenAI-compatible adapter now exposes bounded local-only `StartAgent`,
`RunAgents`, and `SendMessageToAgent` schemas only when chat/tools and a live
local controller are available. Stable local run IDs, topology, ownership,
status, cancellation, ordered message queues, idempotent action replay,
concurrent fan-out, timeout cleanup, child pills, CLI-harness launch/resume,
and restart-safe stopped-history restoration are all process-local. Warp
orchestration tokens, hosted runners, SSE, remote handoff, and server message
delivery are not used.

Focused suites passed 10 registry tests, 8 StartAgent tests, 2 RunAgents tests,
1 SendMessageToAgent test, 9 orchestration-control tests, 85 direct-provider
tests, a terminal bootstrap regression, and 10 `local_only` registry tests.
The reproducible Cargo test cache was removed immediately afterward while the
existing `.app` bundle was preserved. The shared P1.8–P1.11 gate then passed
default and `local_only` all-target builds, bundle creation, executable checks,
and strict codesign. The bundle SHA-256 is recorded under P1.8; cleanup reduced
`target` to 990 MiB without removing the app bundle.

Expose `StartAgent`, `RunAgents`, and `SendMessageToAgent` through the direct
tool adapter and existing local executors. Add local ownership, cancellation,
topology/status, and notification handling. Durable cross-process delivery is a
separate P2 lifecycle.

**Estimate / risk:** 5–7 days / high.

## P2 — Local Redesigns

Status: **complete** through P2.5. The focused P2.5 suite, `cargo fmt --check`,
and default plus `local_only` all-target builds passed on 2026-08-24.

### P2.1 Real local `/compact`

**Delivered:** `37a68613`.

The direct OpenAI-compatible path now captures a bounded immutable snapshot,
preserves the four newest messages and complete tool-call chronology, rejects
active long-running command state, and requests a strict typed JSON summary.
It retries once with a smaller snapshot only for an explicit provider context
overflow. The validated result becomes one structural
`MoveMessagesToNewTask` transaction; its compare-and-swap metadata binds the
conversation, source range and checksum, complete task graph, and current
provider configuration. SQLite acknowledgement is required before success and
in-memory state is restored if persistence fails. Later direct-provider
requests receive only the structural summary, while archived raw messages stay
outside active context. Provider errors, malformed responses, cancellation,
configuration changes, or stale state produce no history mutation and never
fall back to Warp.

The focused `local_only` compaction suite passed 11 tests, including message
boundaries, tool chronology, strict schema validation, stale-state rejection,
provider-route changes, successful SQLite reload, paused-writer failure, direct
endpoint success/failure, summary projection, and context-overflow retry
classification.

Generate a typed summary with `chat`, validate it, then use the existing
message-splice/task machinery and SQLite persistence to replace old context.
Never mutate history before the summary succeeds, and preserve tool chronology.

### P2.2 Local semantic code index

**Delivered:** `d488ac31`.

The upstream full-source chunker, content-addressed Merkle tree, index manager,
and persisted-workspace restore path now use a local `StoreClient` in app and
CLI launch modes. Merkle nodes, raw source chunks, vectors, and repository roots
are stored in scoped SQLite and survive process restart. Remote-server daemon
mode remains isolated from the local route lifecycle.

The first configured provider with an effective `embeddings` capability and an
explicit embedding model is routed directly to its OpenAI-compatible
`/embeddings` endpoint. Keyless endpoints and keys from secure storage or an
explicit environment variable are supported. Responses must contain one
finite, non-empty, consistently sized vector for every input. Endpoint, model,
format, and provider identity define an isolated vector space; credential or
route changes trigger a fresh sync without persisting credentials.

Queries embed locally, walk only chunks reachable from the requested Merkle
root, rank them by cosine similarity, and apply a deterministic local lexical
reranker. Missing providers, missing capabilities, endpoint failures, and empty
semantic results fall back to the existing local outline/content search. No
Warp endpoint is consulted.

Focused `local_only` tests passed durable SQLite restart and ranking, vector
space isolation, keyless routing, explicit capability/model routing, invalid
float rejection, legacy capability compatibility, and asynchronous semantic
failure fallback to local lexical results.

### P2.3 Local long-term memory

**Delivered:** `72b4c299`.

User-managed memories now have explicit create, edit, and delete flows in the
local Knowledge pane, with global or canonical project scope. The scoped
SQLite repository uses compare-and-swap revisions, retains immutable version
history including deletion tombstones, validates size/count limits, and
survives application restart. The enablement setting is local-only and memory
management remains usable without any configured provider.

Recall is deterministic and provider-free: normalized keyword matches are
ranked across global entries and only the project entries applicable to the
current local working directory. At most eight results and 6,000 content
characters are attached to a request. The direct OpenAI-compatible adapter
validates, persists, restores, redacts, and projects this typed context; invalid
or oversized restored data fails closed. Storage/retrieval failures continue
without memory and never consult Warp. Semantic recall and automatic extraction
remain optional future refinements rather than dependencies of the feature.

The focused `local_only` suite passed six tests covering durable CRUD/history,
stale-revision rejection, ranked scope-aware recall, validation bounds, local
task-storage roundtrip, and restored-context revalidation.

Add scoped SQLite memory with explicit CRUD/versioning and bounded keyword
retrieval. Semantic recall and automatic extraction are optional layers using
`embeddings` and `chat`; both must degrade to explicit/manual memory.

### P2.4 Local scheduler and durable background lifecycle

**Delivered:** `42e76b55`.

A local SQLite schedule repository now owns CRUD, optimistic revisions,
timezone-aware interval and daily cadences, missed-run policies, bounded event
journals, durable cursors, manual runs, cancellation, and interrupted-run
recovery. The app-only supervisor claims due work and launches the existing
local named-agent CLI path as a child process, records terminal output and
status locally, and optionally posts an OS notification. Storage failure
disables scheduling without blocking terminal startup or consulting Warp.

The agent action stack now supports a local `wait_for_events` lifecycle. It
waits on the in-process agent registry with a bounded watchdog, wakes for local
child status and message events, cancels cleanly before event injection, and is
available to direct OpenAI-compatible providers only when local orchestration
is ready. No hosted ambient-agent spawn, polling, authentication, or event
transport is used.

The `oz schedule` CLI exposes create/list/show/update/pause/resume/delete,
run/cancel, and cursor-based event inspection. Focused `local_only` tests passed
seven cases covering durable CRUD/journal/cursors, missed-run handling,
restart recovery, cancellation, timezone validation, watchdog bounds, provider
readiness, and direct-provider schema parsing.

Add a schedule repository, local supervisor, durable event journal/cursors,
timezone and missed-run policy, cancellation, `WaitForEvents`, and OS
notifications. Hosted ambient spawn/poll code is not reusable.

### P2.5 Local file and screenshot artifacts

**Delivered:** `070f28a1`.

Files and screenshots selected by a local agent are copied into a private,
application-managed artifact root. A scoped SQLite repository persists the
artifact ID, canonical managed path, MIME type, size, SHA-256 checksum, creation
time, and explicit owners. Owner attachment is transactional; releasing the
last owner removes both metadata and the managed copy, while shared artifacts
remain available until their final owner is released.

The direct OpenAI-compatible adapter advertises and parses the local
`upload_file_artifact` tool only for a local conversation executor. Execution
enforces permissions and size bounds, canonicalizes the source to prevent
symlink path escapes, and attaches the resulting local file or screenshot to
the conversation without a network request. Screenshot lightbox display and
Finder reveal resolve only canonical managed paths and verify the checksum
before opening. Conversation deletion releases its artifact ownership. There
are no signed URLs, hosted-upload fallback, Warp auth, or telemetry calls.

The focused `local_only` suite passed four tests covering direct-provider tool
schema and parsing, restart persistence of metadata and bytes, shared-owner
lifecycle cleanup, and checksum-tamper rejection.

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
