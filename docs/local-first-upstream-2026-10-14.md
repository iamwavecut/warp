# Local-first upstream intake, 2026-10-14

Snapshot: `1d32d02f0e9150866c52ef1b4e6c7a25aae809b4`.
Fork baseline: `af9ec48aa2aa857415faefb9c1532447c1256252`.
The merge retains the fork tree and selectively adapts incoming behavior.
Verification and delivery results belong in the automation memory, not in this assessment.

## Local adaptations

- `24e80bea8`: Markdown rendered/raw keyboard switching in file notebooks and code panes.
  Existing pane transitions remain the consumer; regression coverage routes real keystrokes.
- `7c3148819`: Muse identification, structured local session events, icon and bracketed-paste
  submission. No plugin installer or hosted integration is added.
- `626a4d4eb`: Claude local transcript directory encoding covers Windows, UTF-16,
  punctuation and the long-path hash. The existing local resume repository consumes it.
- `69c98cf99`: MouseDown can use the current pointer. The direct provider schema,
  action conversion and all OS backends preserve coordinate validation.
- `c79019b55`, `714c51f1e`: Rustls and multipart-parser dependency fixes.
- `721449fde`, `5c84259e2`: local transcript saves coalesce bursts; final saves bypass
  debounce. Existing local periodic checkpoints remain available. Hosted handoff promotion
  has no additional local consumer beyond these checkpoints.
- `b113a6c95`, `2055f0f48`: the active skill-repository checkout path uses a local full
  Git mirror with typed arguments and independent object storage. Existing terminal and
  skill workflows consume the checkout. Factory execution configuration, server-issued
  Git credentials, cache volumes and deferred hosted repository transport are not imported.
- `760d01e53`, `f386a4cc8`, `02668f9be`, `37fcf683f`, `cd328546a`: local ACP
  adapters render structured turns through native conversation history. Driver identity
  survives local SQLite restoration; externally executed tools are never executed again
  by the native executor. Attachments use local files/text. Permissions fail closed and
  choose at most AllowOnce. Host-local shell validation excludes WSL/container transport.
  Hosted native-conversation creation and transcript uploads are replaced by local history.
  PTY transcript resume is explicitly rejected for ACP instead of pretending to resume it.
- `21f068328`: batch and per-child string labels pass through confirmation and local
  dispatch, with child labels overriding batch labels. They are stored locally, not injected
  into environment variables or sent to a task server.
- `b2b7357dd`: local native-agent shell termination can spawn a replacement in the same
  terminal, retaining its channel, working directory and known environment. Interrupted
  native work fails explicitly; commands are not replayed. External CLI prompts and
  unbootstrapped or non-host shells retain their existing exit handling. No cloud flag or
  recovery telemetry is introduced.

## Existing equivalents and excluded transport

- `1d32d02f0`: the fork HTTP client already has no automatic Warp headers or workload
  token injection. Direct provider requests use the configured endpoint. Presigned hosted
  transfers have no local consumer because their upload/checkpoint services are removed.
- `c1ec372cb`: local permission checks already deny protected MCP/config paths
  unconditionally. The cloud-only bypass would weaken this invariant and is excluded.
- `c10fc07d7`: its only new installation calls Warp's preview-URL MCP endpoint with
  bearer credentials plus an ambient workload token. The result is a public URL for a
  hosted sandbox; publishing that URL requires the forbidden tunnel/backend. Local
  terminal processes and user-configured MCP servers remain available without this service.
- `b0a86afa7`, `8aa20c3ee`, `4f3360deb`: hosted execution configuration promotion,
  Warp startup-metadata retries and development workload audience selection have no
  local runtime consumer once hosted task startup and authentication are removed.
- `25e82210a`: OTLP parent-token propagation serves remote tracing and hosted checkout.
  Local checkout diagnostics use ordinary errors; its temporary directories are private.
- `d0d50955d`, `d23c22a4c`, `fc4b06fb4`: analytics upload and Namespace CI/cache
  transport have no independent desktop consumer. Local Git caching is implemented above.
- `6f120ee52`: the timeout targets two upstream tests absent from this fork; importing
  hosted test fixtures would not verify a local feature.

## Verification boundaries

Protocol compatibility repairs preserve inline images, local terminal-busy results and
64-bit token counters. Request metadata does not become user text. Unused Warp API-key
request construction was removed rather than extended with hosted subscription auth.
Saved external-harness history refuses follow-up prompts without its live local channel.
Markdown fixtures register the disabled local voice transcriber; AI documents use a
copyable export that preserves block separators without changing ordinary editor export.

Run formatting, all-target default and compatibility builds, and focused tests after
business changes. New tests cover ACP framing/mapping/socket authentication, local history
ownership, metadata, Git mirrors, Markdown focus routing, Muse, transcript paths and pointer
coordinates. Build success alone does not establish Windows/Linux execution or live-provider
GUI behavior. Preserve release and installed bundles during mandatory cleanup.
