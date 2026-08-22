# P1.1 provider capability contract — implementation report

## Scope and outcome

Implemented the local/custom-provider capability contract from
`plans/2026-08-23-warposs-p1-provider-capability-contract.md`.

The contract is provider-wide and persisted in `CustomProviderConfig`. Legacy
settings without a capability table keep the existing direct-provider behavior:
`chat=true`, `tools=true`, and `vision`, `embeddings`, and `transcription` are
false. API keys remain outside this value in secure storage or an environment
variable reference.

The direct OpenAI-compatible adapter carries an immutable capability snapshot in
`CustomProviderRoute` and exposes one typed effective-capability boundary. The
current adapter makes only configured `chat` and `tools` effective. Vision,
embeddings, and transcription remain unavailable until their local adapters are
implemented.

## Changed interfaces

### Persisted settings

`app/src/settings/ai.rs`

- Added serializable/schema-backed `CustomProviderCapabilities`:
  `chat`, `tools`, `vision`, `embeddings`, `transcription`, and optional
  `context_window_tokens`.
- Added compatibility defaults and validation. Context windows below 256
  tokens, including zero, return a local `CustomProviderConfigError`.
- Added UI construction helpers that normalize the context value and preserve
  the full capability value when a provider is renamed or otherwise edited.

### Route and transport

`app/src/ai/agent/api/direct_openai.rs`

- `CustomProviderRoute` now carries the configured capability snapshot.
- `effective_capabilities_for_config` is the single adapter-owned boundary.
- A route with chat disabled returns a local error before constructing or
  sending an HTTP request. `complete_text` uses the same guard.
- A route with tools disabled constructs a request with no `tools`,
  `tool_choice`, or `parallel_tool_calls` fields. Existing tool schemas and
  execution behavior remain unchanged when tools are enabled.
- Image input on the current adapter returns a clear local vision-unavailable
  error before HTTP, even if the provider declaration says vision is supported.
- Configured context tokens are converted conservatively to a character budget
  (`tokens * 3`) at the existing context truncation boundary. This bounds
  selected text, files, terminal output, rules, and MCP summaries. It is not
  whole-request token accounting and does not reserve output tokens; the legacy
  24,000-character budget remains the fallback.

### Model metadata

`app/src/ai/llms.rs`

- Added optional effective `LLMCapabilities` metadata to `LLMInfo`.
- Custom model metadata is derived from the direct adapter boundary and carries
  the configured context maximum.
- Custom context metadata deliberately sets `is_configurable=false`: the
  provider-level limit is consumed by the direct adapter, while the existing
  execution-profile slider is not yet threaded into `RequestParams`. This
  avoids advertising a profile control that would not affect direct requests.
- Hosted/default metadata remains backward-compatible (`capabilities=None`),
  and cached metadata without the field still deserializes.

### Settings UI

`app/src/settings_view/ai_page.rs`

- Added a local context-window input and persisted capability switches for each
  custom provider.
- Chat/tool switches describe the currently available local bridge.
- Vision/embeddings/transcription switches explicitly say that their local
  adapters are pending; configured intent is not advertised as effective
  availability.
- Compact model labels remain `provider / model`; no URL, key, or capability
  data was added to picker labels.

## TDD evidence

### Red phase

Before the production implementation, the new legacy compatibility test was
run with:

```text
cargo test -p warp legacy_custom_provider_config_uses_compatible_capability_defaults -- --nocapture
```

It failed at compilation because `CustomProviderCapabilities` and the new
minimum-context constant did not yet exist. This was the expected red state.

### Green phase

Focused tests passed after implementation:

```text
cargo fmt --check
```

Passed.

```text
cargo test -p warp custom_provider -- --nocapture
```

12 passed, 0 failed. Covers legacy defaults, explicit round-trip values,
invalid context values, UI parsing, local startup catalog, and route setup.

```text
cargo test -p warp direct_openai -- --nocapture
```

22 passed, 0 failed. Covers configured/effective capabilities, no-network
chat-disabled and vision-disabled paths, exact HTTP omission of tool fields,
supported tool behavior, configured and legacy context budgets, streaming,
model discovery, and completion requests.

```text
cargo test -p warp llm_info_ -- --nocapture
```

3 passed, 0 failed. Covers legacy LLM metadata deserialization and cache
round-trip behavior.

```text
cargo test -p warp provider_editor_save_preserves_capabilities_when_provider_is_renamed -- --nocapture
cargo test -p warp custom_model_metadata_reports_effective_capabilities_and_context_window -- --nocapture
```

1 passed and 1 passed, respectively. These cover provider-editor persistence
semantics and effective custom model metadata.

```text
cargo test -p warp --features local_only custom_provider_capabilities -- --nocapture
cargo test -p warp --features local_only direct_openai -- --nocapture
```

2 passed and 22 passed, respectively. The local-only feature path has the same
capability and direct-provider behavior.

`git diff --check` also passed. No broad build, install, application launch,
push, or hosted-service verification was performed in this task.

## Local-first and privacy checks

- No Warp URL, Warp protobuf AI route, hosted fallback, auth, telemetry,
  billing, incident-upload, or product-data call was added.
- HTTP regressions use only an in-process localhost mock endpoint.
- Capability persistence contains no key or token values. Direct API keys are
  still resolved through the existing secure-storage/environment-variable
  path.
- Invalid local provider configuration is rejected or skipped locally with a
  user-actionable error; it does not trigger remote capability discovery.

## Residual risk and follow-up

- `tokens * 3` is intentionally a conservative character-bound heuristic,
  not tokenizer-accurate accounting. P2 compaction should own exact token
  budgeting and output reservation.
- Execution-profile context limits are not yet part of the direct route. The
  custom model metadata therefore does not advertise the existing slider.
- Vision, transcription, and embeddings require their own local adapters before
  their configured declarations can become effective capabilities.
- Focused tests do not replace manual UI interaction testing of the provider
  editor; the pure rename/persistence path and settings serialization are
  covered.
