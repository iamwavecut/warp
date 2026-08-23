# WarpOss P1.4 Local OpenAI-Compatible Transcription

## Goal

Reconnect the existing voice-capture UI to a user-configured
OpenAI-compatible `/audio/transcriptions` endpoint. The endpoint may be a local
process or a user-chosen remote server; Warp auth, quota, telemetry, and hosted
transcription must never participate.

## Configuration And Routing

- Extend the local provider capability configuration with an optional explicit
  transcription model ID. Enabling `transcription` without a non-empty model ID
  is invalid and must produce an actionable settings error.
- Select the transcription route from the currently selected custom provider
  when it declares transcription and has a transcription model. Do not select a
  different provider silently. If no usable route exists, voice input is
  disabled with a local explanation.
- Reuse the provider's `base_url`, optional secure key/env-var resolution,
  duplicate-name fail-closed rule, and key-hydration readiness. Key values must
  never enter settings, logs, or tests.

## Adapter

- Implement the existing `Transcriber` trait with an OpenAI-compatible adapter.
- Decode the captured base64 WAV locally, validate a bounded non-empty WAV
  payload, and send multipart form data to
  `POST <base_url>/audio/transcriptions` with `model` and `file` fields.
- Add `Authorization: Bearer ...` only when the chosen provider has a resolved
  key. Keyless local endpoints must work.
- Parse the standard JSON `text` response and report transport, HTTP, malformed
  payload, and missing-text errors locally.
- Do not retry transcription automatically; the recorded audio is user data and
  a request may already have completed server-side.

## App Lifecycle And UI

- Enable the existing `voice_input` Cargo feature for the default/local-first
  app and always register a `VoiceTranscriber` singleton (disabled when no route
  is available).
- Replace the fork's hard-coded voice entitlement/getter `false` values with
  local configuration checks. Preserve the user's existing
  `agents.voice.voice_input_enabled` setting.
- Refresh the transcriber route when provider settings, selected model, or
  secure-key readiness changes. A stale route must not survive provider removal
  or rename.
- Keep microphone permission and existing recording/cancellation behavior
  unchanged. Do not add model downloads or spawn an undeclared process.

## Tests

Start with failing tests. Cover at least:

1. Keyless and keyed mock endpoints receive one multipart request at
   `/audio/transcriptions`, the configured model, a WAV file, and the correct
   presence/absence of `Authorization`.
2. Invalid base64/WAV, oversized audio, non-2xx responses, malformed JSON, and
   missing `text` fail locally without retry or Warp traffic.
3. Missing provider, transcription disabled, missing model, duplicate provider
   name, keys loading, and unset key env var disable/fail the route clearly.
4. Settings/model/key changes replace or clear the live transcriber route.
5. Default and `local_only` builds expose identical voice behavior.
6. The configured transcription model and enablement survive restart; no secret
   value is persisted.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp transcrib -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only transcrib -- --nocapture
CARGO_INCREMENTAL=0 cargo build --all-targets
CARGO_INCREMENTAL=0 cargo build --features local_only --all-targets
```

