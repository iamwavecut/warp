# Upstream intake: 2026-10-23

Upstream: `1e6c3ae465d6a925a9b97ae03877aaadd159b8f1` (seven commits).
Baseline: `5971d28ab4fc3aad3f15d38ac4b5f1f1b28dd1b3`.

The merge uses the fork tree as its base and applies reviewed changes explicitly.
No hosted module or new source-level `local_only` branch is imported.

## Local adaptations

- `a36b76b92`: the local skills checkout consumer already has a full Git cache
  and independent clones. Narrow new bare clones and refresh refspecs to heads
  and tags, migrate old broad refspecs before fetching, and use parallel Git
  checkout workers. Preserve complete history, local credential helpers,
  no-hardlinks clones and redacted diagnostics. Regression coverage uses real
  repositories, branches, tags, pull-request refs and two cached checkouts.
  The hosted environment checkout module is not needed by this consumer.
- `d1d6200bf`: adapt discovery fallback to the fork's shared MCP negotiator.
  Only HTTP 400, INVALID_REQUEST, synthetic `server-error`, and an unsupported
  version message naming the sent revision bypass error-ID correlation.
  Legacy initialization cannot repeat the exception. Real HTTP/SSE tool calls
  cover header preservation; rejected IDs, versions, codes and statuses remain
  rejected. User-configured MCP authentication is not Warp account auth.
- `1e6c3ae46`: ACP tool display uses the configured local MCP server name,
  normalized qualified tool name and late input. Update the original message
  and action IDs rather than executing the tool again. Local server names are
  display metadata, not managed integration/account identities. Existing native
  conversation persistence consumes the updates. Mapper and controller tests
  cover the displayed input and stored message.
- `07c5e048d`: add an unconditional local submission setting with Interrupt,
  Steer and Queue. Steer persists input and attachments in the existing SQLite
  queue and injects its ready head at the next native model request. It does not
  cancel a running tool or send another request concurrently. Command, edit,
  locked and uncertain-dispatch rows remain FIFO barriers. The conversation
  auto-queue toggle forces Queue. Errors retain uncertain rows for explicit
  recovery. A matching completed model response acknowledges a steering row even
  while its tools continue, allowing the next correction at the next request;
  ordinary full-turn queue rows still wait. External ACP
  lifecycle remains adapter-owned. No hosted injection metadata, telemetry,
  steering feature flag or cloud settings sync is added.
- `3cee055a9`: pending sharing state must not block a local shell command.
  Preserve the narrow runtime safeguard with a bootstrapped terminal test;
  creating/viewing shared sessions remains disabled and hidden.
- `edf27fd58`: update the fork's existing TLS verifier and certificate roots.
  The unused upstream `webpki-roots` dependency is not added; keep the fork's
  existing protobuf pin and direct provider routing.

## Existing equivalents and remote-only implementation

- `692f75fd9`: installed local Claude/Codex CLI harnesses, ACP transport selection,
  native local history and local resume already provide the desktop result.
  The changed `standby`, `prewarmed` and assignment consumers require Warp
  sandbox allocation, server task configuration, team metadata and hosted
  credentials. Those modules are absent in this fork. Importing their lifecycle
  adds no independent local startup or resume operation.
- Upstream telemetry events, hosted steering/injection metadata and prewarmed
  managed MCP/credential plumbing are excluded. Local UI, queued input and
  direct providers have consumers independent of these transports, as above.

## Verification

After all edits: formatting, MCP protocol tests, both all-target builds, and a
single app test binary reused for ACP, local steering, queue persistence,
checkouts, pending local commands and direct/custom-provider filters. Temporary
targets and debug outputs have an unconditional finalizer. Production build and
installation follow only verified push. Observed results are recorded in the
automation memory, not inferred from this planned scope.
