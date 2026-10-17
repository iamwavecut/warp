# Local-first upstream intake plan — 2026-10-17

> Execute inline with the main agent. User instructions override delegated review and early test runs.

**Goal:** Intake upstream b80b4e3e0 without hosted dependencies.
**Architecture:** Retain the fork tree and adapt local ACP manifests and cancellation history. Preserve local child panes and MCP startup rather than importing hosted discovery.
**Tech Stack:** Rust, local JSON files, native history, existing ACP bridge.
**Spec:** AGENTS.md and automation instructions.

- [x] Add manifest discovery/validation regressions, then local manifest dispatch and owned runtime-error patterns.
- [x] Add steering versus Stop history regressions, then settle cancelled streams with the correct reason.
- [x] Trace MCP and remote-child pane consumers. Record existing local equivalents and exclude only hosted transports.
- [x] Review added lines, provider routing, feature flags and preserved instructions.
- [x] Final-only formatting, both all-target builds, focused ACP/history/provider/MCP tests. No observed RED claim.

Delivery follows the verified source commit: push and independently verify the
fork SHA, then build/install a signed release without relaunch. Record terminal
delivery and exact temporary/debug cleanup proof in automation memory, outside
the source commit that the bundle represents.

Review focus: invalid explicit manifests never fall through, local credentials stay local, dynamic diagnostic patterns retain their owner, steering does not emit terminal cancellation, and local child controllers are not made lazy.
