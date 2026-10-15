# Local-first intake: 2026-10-15

Upstream: `522481cd042ce8c48eed04dbff8e0313f174c329`.
Previous fork: `e2e9e358f971033638f508b5fa2d89f95319b7f2`.

The merge preserves the fork tree and imports the following independent local results.

## Local adaptations

- `9e57f4f6a`: Pi can run through an installed `pi-acp` executable. The native
  driver uses its existing private host-local bridge, permission policy and local
  conversation history. Pi uses its own local provider configuration. No adapter
  download, account secret broker, Factory environment or Warp endpoint is added.
  Pi defaults to ACP even when the CLI transport argument retains its PTY default.
  It does not claim terminal-transcript resume or local child-pane support.
- `bf12566b4`: failure to create the first native turn closes the ACP connection
  and interrupts the app-owned adapter terminal. Existing handshake diagnostics
  already preserve the local error chain. Setup metrics and remote incident
  reporting are not imported.

Regression coverage checks Pi config-name persistence, its exact local launch
command, default ACP dispatch, explicit ACP dispatch, and absence of an auth
preflight. Existing ACP bridge, permissions, protocol and follow-up tests cover
the shared runtime. A live Pi adapter session is a separate, unavailable check
when `pi-acp` is not installed.

The broader harness suite exposed two pre-existing fixture assumptions. Child
history now checks that both children retain the parent's local run ID rather
than a server token. Approval-config tests explicitly enable their thread-local
CLI harness gate before checking preservation of the chosen model and harness.
These repairs do not restore hosted identifiers or change production gates.

## Existing local equivalents and remote-only changes

- `1b76b6f3e`: the added queue selects `shared_session_prompt()` rows and copies
  Warp-server `BaseUserQuery` participant attribution. That queue has no local
  producer after shared sessions are removed. Local follow-up prompts already
  use `ExternalHarnessPrompt` and the local ACP channel. Preserve that path,
  not participant identity or server injection.
- `af9a6a956`: remote-child validation, snapshots and Factory Pi gates consume
  hosted orchestration state. Local Pi execution is implemented above instead.
  No cloud child route, snapshot transport or hosted support skills are imported.
- `dc0b890ba` and `522481cd0`: their repository inventory comes from a Factory
  task snapshot, execution bootstrap and exported Oz checkout contract. The fork
  has none of those producers. Local skill repositories already use native Git
  mirrors and credential helpers. Preserve that independent local checkout
  consumer. Do not introduce task-server inventory, hosted credentials or a
  fabricated Factory environment to obtain ordinary Git checkouts.

The source enum and local UI labels recognize Pi. Native named-agent and child
pane validation still reject unsupported Pi launch modes rather than silently
running another provider. Cloud, telemetry, account, billing and hosted AI paths
remain disabled. Custom providers still use their configured direct endpoints.
