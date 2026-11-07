# Local-first intake

Upstream target: f7fc2f2a0. Preserve the fork tree and apply reviewed changes.

Accepted local changes: MCP initialize negotiates without an early HTTP version
header; discovery retains its version header. Strict legacy counteroffer coverage
also invokes a tool through the negotiated transport. CLI login/logout/auth
commands are not interactive agent sessions; normal CLI sessions remain detected.

Existing local equivalents: credit availability is unconditional in
AIRequestUsageModel::has_any_ai_remaining. Local ACP steering already cancels the
current turn and queues corrections without replay. Custom providers use direct
requests and local keys; Pi uses installed pi-acp configuration, not managed keys.
Local skills use the filesystem and direct Git checkout, not Factory run policies.

Excluded transports: shared-session reconnect services, preview retention polling
by hosted task/execution ID, managed-secret GraphQL delivery, cloud skill-reference
reporting and prewarmed sandbox lifecycle. Their incoming consumers operate on
Warp-hosted runs, not the live local terminal/channel. Preserve local channels and
do not replace ACP history with a PTY transcript or fallback provider.

Private changelog/release authentication and signing configuration have no consumer
in the absent release workflow. Channel-config installation remains an explicit
local no-op. Do not restore private tokens, IAP routing, platform plugins, hosted
completion skills or upstream Debian repository enrollment to ship the local app.

Queued shared-session startup dispatch has no corresponding startup_queue module
in this fork. Do not import remote orchestration to reproduce its callers.

Delivery requires formatting, both all-target builds and focused MCP/CLI/provider
tests, followed by push, release packaging, installation and owned-output cleanup.
