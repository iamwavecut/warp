# WarpOss P1.1 Provider Capability Contract Plan

**Goal:** Make local/custom-provider behavior capability-driven before adding
the remaining direct adapters. The contract must never probe or fall back to
Warp and must preserve existing custom-provider settings.

**Architecture:** `CustomProviderConfig` owns the configured provider-wide
contract. `CustomProviderRoute` carries an immutable snapshot into a request.
`LLMInfo` exposes only capabilities the current local transport can actually
deliver. Downstream adapters consume the same contract instead of adding
independent feature flags.

Provider-wide capability scope is intentional for the first boundary. A user
whose endpoint exposes models with different capabilities can define multiple
provider entries with the same base URL and different local names. Model-level
router overrides belong to P1.8 and must not complicate this migration.

## Compatibility and defaults

- `chat=true` and `tools=true` when the capability table is absent, preserving
  the already-shipped direct OpenAI-compatible path.
- `vision=false`, `embeddings=false`, and `transcription=false` unless the user
  explicitly configures them.
- Context-window size is optional; the existing conservative direct-adapter
  limit remains the fallback when it is absent.
- Deserialization of existing TOML and cached model metadata must remain valid.
- Capability configuration contains no secrets and may be persisted in normal
  settings. API keys remain in secure storage or an environment-variable
  reference.

## Effective capability rule

A feature is available only when both are true:

1. the provider configuration declares it; and
2. the local transport/adapter implements it.

The direct adapter currently implements `chat` and `tools`. P1.3 will add
`vision`, P1.4 will add `transcription`, and P2.2 will add `embeddings`.
Configured-but-not-yet-implemented features must degrade locally with a clear
message and must not become enabled UI promises.

## Task 1: Define and persist the contract

**Files:**

- `app/src/settings/ai.rs`
- `app/src/settings/ai_tests.rs`
- generated settings schema/input only if required by the existing generator

Add a serializable capability value with explicit compatibility defaults for
`chat`, `tools`, `vision`, `embeddings`, `transcription`, and optional
context-window tokens. Add it to `CustomProviderConfig` with `#[serde(default)]`.
Keep construction helpers and all struct literals source-compatible by updating
them deliberately rather than hiding missing fields behind ad hoc defaults.

Regression tests must cover legacy config without the new table, explicit
false/true values, round-trip persistence, and invalid zero/tiny context-window
values. Validation must return a local configuration error.

## Task 2: Propagate configured and effective capabilities

**Files:**

- `app/src/ai/llms.rs`
- `app/src/ai/llms_tests.rs`
- `app/src/ai/agent/api/direct_openai.rs`
- nearby test constructors that build `LLMInfo` or `CustomProviderConfig`

Carry the configured contract in `CustomProviderRoute`. Provide one small,
typed effective-capability boundary owned by the direct adapter. Populate custom
model metadata from that boundary, including vision availability and configured
context-window metadata, without changing hosted/default metadata semantics.

Do not mark vision, embeddings, or transcription effective until their local
adapters exist. Tests must prove that explicit configuration is retained while
effective availability remains false for an unimplemented adapter.

## Task 3: Enforce request behavior

**Files:**

- `app/src/ai/agent/api/impl.rs`
- `app/src/ai/agent/api/impl_tests.rs`
- `app/src/ai/agent/api/direct_openai.rs`

- A route without `chat` returns a clear local error before any HTTP request.
- A route without `tools` sends a chat request without `tools`, `tool_choice`,
  or `parallel_tool_calls`.
- A route with tools preserves the current schemas and execution behavior.
- The context-window setting replaces the hard-coded truncation decision at a
  single request-building boundary. Use a documented conservative conversion
  from configured tokens to a character budget until P2 compaction has exact
  accounting; preserve the current limit when no value is configured.
- Never retry automatically after a side-effectful tool begins and never route
  to hosted Warp AI.

HTTP tests must assert request absence for unsupported chat, exact omission of
tool fields for unsupported tools, existing tool behavior for supported tools,
and bounded context behavior for configured and legacy providers.

## Task 4: Expose editable settings without false promises

**Files:**

- `app/src/settings_view/ai_page.rs`
- `app/src/settings_view/ai_page_tests.rs`

Extend each local provider editor with capability controls and a context-window
input using existing settings UI primitives. Saving an editor must preserve all
new fields; renaming a provider must not lose them. Clearly distinguish
configured capabilities from capabilities unavailable because their local
adapter is pending.

The compact model picker remains `provider / model`; do not add URL or
capability noise to picker labels.

## Verification

Follow red-green-refactor with narrow filters first. After all source changes:

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cargo fmt --check
cargo test -p warp custom_provider_capabilities -- --nocapture
cargo test -p warp direct_openai -- --nocapture
cargo test -p warp --features local_only custom_provider_capabilities -- --nocapture
cargo build --all-targets
cargo build --features local_only --all-targets
```

Static acceptance must also prove no new Warp URL, `ServerApi`, auth,
telemetry, billing, or incident-upload call was introduced. Manual UI
acceptance is required for persistence of the capability controls and clear
unsupported-capability degradation.
