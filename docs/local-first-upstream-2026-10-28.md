# Upstream intake: 2026-10-28

Upstream: `eafc6d18d18bea8c6cf78dc21a4e0eab59f4c6c6` (three commits).
Fork baseline: `9533eed9e17c41284ff52173f589621526547741`.

The merge retains the fork tree and applies the reviewed changes explicitly.
An initial ordinary merge refused to overwrite the imported regression test;
no changes were lost. The final merge has no textual conflicts.

## Accepted desktop behavior

- `eafc6d18d`: keep one lifetime history subscription in the agent status bar.
  Resolve the current exchange when each output event arrives instead of
  registering another view-owned callback whenever the exchange changes.
  Output from an old exchange must not close the current summarization dialog.
  Current-exchange output still works across terminal surfaces. Local
  same-conversation invalidation remains available. Do not import the upstream
  `ConversationUsageMetadataUpdated` arm: that hosted usage event does not exist
  in this fork, and restoring it adds no local history consumer.
  These consumers read local history and need no Warp transport or account.
  Import the focused restored-conversation regression before the implementation.
  Other block subscribers remain unchanged.

## Repository automation

- `65229eb52`: preserve the fork's CI file filters and runner configuration.
  Grant the parameter job only repository/PR read permissions. Keep explicit
  read/comment permissions on the existing owner-guarded approval workflow.
  Its cache caller needs repository/PR read access, not upstream GCP OIDC.
  Remove the two obsolete hosted-agent fix-PR cleanup workflows as upstream does.
- Do not restore the deleted upstream release and repo-sync workflows merely
  to add their token permissions. Their consumers are upstream release/tag
  publication and `warpdotdev/repo-sync` jobs, not desktop behavior. The fork's
  local production bundle/install path already supplies its release result.
- `1ed604025`: do not add the Octo STS channel-version trust policy. Its exact
  subject is `warpdotdev/channel-versions` and its `update_channels` and
  `create_version_bump_pr` workflows.
  It delegates repository access to that separate identity and has no local
  client consumer or independent desktop result to adapt.

## Local-first boundary and verification

No new hosted transport, telemetry, authentication, billing, source-level
`local_only` branch or provider fallback is added. Preserve AGENTS, forced-off
flags, direct provider routing and local CLI/MCP behavior. Search changed files
for prohibited surfaces and distinguish GitHub workflow events and local
history events from product telemetry.

After all changes, run formatting, both required all-target builds and focused
status-bar/direct/custom-provider tests. Use a private target with an
unconditional cleanup finalizer. Push only after verification. Build and install
the production bundle after the verified push, without launching the app.
Record observed results and cleanup in automation memory.
