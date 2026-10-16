# Local-First Upstream Intake Implementation Plan

> **For agentic workers:** Use `superpowers:executing-plans` inline. The user requires the main agent and final-phase verification, not delegated workers or intermediate test runs.

**Goal:** Intake upstream through `3e2b62975` without restoring Warp hosted services.

**Architecture:** Preserve the fork tree. Adapt ACP protocol and local controller paths, retain native filesystem attachments and local request history, and accept local terminal fixes. Do not import remote setup infrastructure.

**Tech Stack:** Rust, existing futures/Tokio background executor, ACP JSON-RPC, SQLite history, native macOS bundle tools.

**Spec:** `AGENTS.md` and the current automation instructions.

## Global Constraints

- Work on `master`; backup before the actual merge.
- No telemetry, hosted attachments, shared identity, billing, account or Warp API fallback.
- Write focused tests before implementation. Run verification only after all business changes.
- Push before production build/install. Preserve the current user session and protected apps.
- Use a private temporary target, finally-style cleanup and pinned cargo-bundle 0.11.0.

## Review Focus

- Slow permission handlers must not block updates, responses or cancellation.
- Cancel and steering must preserve adapter process, stream ownership and prompt order.
- Unsupported attachment capabilities must fall back to local links, with bounded reads.
- Interleaved task inputs must retain explicit order and all exchange registrations.
- Hosted metadata must not become a prerequisite for local CLI events or Git checkout.

## Tasks

- [x] ACP connection/protocol: asynchronous local handlers, numeric/string IDs, single answers and cancellation isolation.
- [x] Local attachments: bounded images/text resources for capable adapters, local links otherwise. Drive downloads remain rejected.
- [x] ACP steering: queued corrections and protocol Stop routing. Wire cancellation, pending permissions, notification ordering, controller routing and queue discard have focused coverage. No live adapter session is claimed.
- [x] Input ordering: ordered entries, grouped task history and every exchange registered for stream updates.
- [x] Local terminal intake: Antigravity event/command/submit tests and Codex local notification compatibility.
- [x] Remaining commits assessed by callers in `docs/local-first-upstream-2026-10-16.md`.
- [x] Final formatting, both all-target builds and focused tests passed. The finalizer removed temporary debug outputs.

## Delivery procedure

After committing this verified tree, push and verify the remote SHA. Build and
install a signed release with matching raw/bundled UUID and source/installed
hashes. Do not restart the app. Record the terminal delivery result in automation
memory, not by modifying this already verified source revision. Remove exact
unused temporary/plugin artifacts and report protected active-owner outputs and
measured free space.
