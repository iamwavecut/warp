# Repository Agent Instructions

## Scope

These instructions apply to the whole repository. They specialize the global
agent instructions for this local-first fork of Warp.

Use `master` as the canonical working branch for this fork. Do not create or
revive the old `local-byok` branch.

Use paths relative to the repository root in this file. Do not add local
absolute paths to `AGENTS.md`.

## Context Maintenance

When the user gives durable repository-specific context, workflow constraints,
or fork invariants, update this file in the same turn when that context should
guide future work. Keep updates concise and scoped to this repository.

When the user asks for a repo-wide cleanup or migration, do not arbitrarily
split the work into future phases. Continue within the current session until the
requested end state is reached or a concrete blocker is found. Keep the build
green as the boundary for safe progress.

Do not run verification commands such as tests, `cargo fmt`, `cargo fmt --check`,
`cargo check`, builds, or linting until all requested business/code changes for
the current work item are complete. Verification is the final phase, not an
interleaved step, unless the user explicitly asks for an early diagnostic run.

At the end of every work iteration on this fork, include a copyable launch
command relative to the repository root:

```sh
open target/debug/bundle/osx/WarpOss.app
```

## Fork Goal

This fork is an experimental local-first build of Warp. Local behavior is the
product default, not an optional mode. New code should not introduce a product
split between local and hosted/cloud workflows.

The app must work:

- without mandatory login or registration;
- without Firebase anonymous user creation;
- without manufacturer data collection, event upload, or remote incident upload;
- without Warp cloud/protobuf endpoints for custom AI providers;
- without Warp account, cloud, billing, subscription, quota, paywall, referral,
  Teams, Shared Blocks, or Warp Drive UI;
- without proprietary hosted Warp agent/server auth flows;
- with MCP and local tooling preserved;
- with local and non-cloud AI UX available where possible;
- with BYOK and custom OpenAI-compatible providers as the primary provider path.

## Core Local-First Policy

Keep these surfaces disabled, hidden, or removed as appropriate:

- login, account registration, browser auth, and device auth;
- Firebase anonymous user creation;
- Warp cloud and Warp Drive cloud sync;
- manufacturer/product data collection, event persistence, event upload, remote
  incident upload, and related placeholders must be removed, not stubbed;
- billing, subscription, Stripe, upgrade, usage pricing, quotas, and paywalls;
- cloud conversation storage;
- hosted/proprietary Warp agent server auth flows;
- Warp server API-key authentication and management surfaces;
- Teams, Referrals, Shared Blocks, Warp Drive, Cloud Platform, Cloud
  Environments, Slack/community links, feedback links, invite-a-friend links,
  logout, settings sync, and plan/free labels in visible UI.

Preserve or re-enable these surfaces through local workflows:

- terminal UX and local session management;
- vertical tabs/sidebar and local agent status surfaces;
- MCP;
- local tools;
- BYOK/custom-provider key configuration;
- OpenAI-compatible custom providers;
- local inference, local storage, local tools, MCP, or direct provider flows
  whenever they can reasonably replace a hosted dependency.

Do not spoof paid-plan state globally. Unlock local/non-cloud AI UX directly at
the relevant gates instead of making billing metadata appear paid, because that
can reintroduce billing and upgrade surfaces.

Keep the distinction clear:

- `api_key_env_var` for custom providers is allowed and expected.
- `FeatureFlag::APIKeyAuthentication` and `FeatureFlag::APIKeyManagement` are
  Warp server/API auth surfaces and must stay off.

## Local User And UI Rules

The former account settings surface is a local `User` surface. It should show
the local system user's display name when available and fall back to the system
username. It must never fall back to Warp test-user identity strings.

User-visible account/cloud UI should be removed from the visible interface
rather than merely made inert. Do not expose Account, Plan, Free, upgrade,
billing, Settings Sync, Logout, referrals, invite-a-friend, Slack, feedback,
Teams, Shared Blocks, Warp Drive, Cloud Platform, Environments, or Warp
server/API-key management entry points.

AI provider settings should expose only `LLM providers` for custom/local
provider configuration:

- provider name;
- OpenAI-compatible base URL;
- model IDs;
- optional direct API key stored securely;
- optional API-key environment variable.

If the env-var field starts with `$`, strip the `$` and resolve that environment
variable at request time. Do not restore the legacy proprietary key-only
provider UI.

## Cargo Features

File: `app/Cargo.toml`

Default builds should be local-first. Keep `skip_login`,
`skip_firebase_anonymous_user`, and `solo_user_byok` in the default feature set
unless their behavior is made unconditional in source.

The `local_only` feature may remain only as a compatibility alias for older
commands:

```toml
local_only = ["skip_login", "skip_firebase_anonymous_user", "solo_user_byok"]
```

Do not add new `local_only` source-level branches. If behavior belongs in this
fork, make it unconditional. If compatibility commands still use
`--features local_only`, they should exercise the same local-first behavior.

## Feature Flags

File: `app/src/lib.rs`

`init_feature_flags()` should force cloud, hosted, account, billing, auth,
manufacturer data collection, and remote incident upload flags off
unconditionally for this fork.

Keep local agent-management and vertical-tab/sidebar surfaces available when
they are backed by local state. Do not disable MCP wholesale.

Cloud/hosted/account flags that should stay off include:

- `CloudObjects`
- `CloudMode*`
- `CloudConversations`
- `CloudEnvironments`
- `ScheduledAmbientAgents`
- hosted-only `AmbientAgents*` flags such as RTC or image upload
- `SshRemoteServer`
- shared-session flags
- `ForceLogin`
- `APIKeyAuthentication`
- `APIKeyManagement`
- `UsageBasedPricing`
- remote incident upload flags
- manufacturer data collection flags

## Custom Provider Path

Relevant files:

- `app/src/ai/agent/api/direct_openai.rs`
- `app/src/ai/agent/api.rs`
- `app/src/ai/agent/api/impl.rs`
- `app/src/settings/ai.rs`
- `app/src/settings_view/ai_page.rs`

Custom model IDs use:

```text
custom/<provider-name>/<model-id>
```

Custom providers must send requests directly to:

```text
POST <base_url>/chat/completions
Authorization: Bearer ***   # only when a key is configured
```

This path must bypass Warp cloud/protobuf `/ai/multi-agent`.

The direct OpenAI-compatible provider path must wait for `run_shell_command`
results to complete before returning a tool result. Do not send
`LongRunningCommandSnapshot` results to OpenAI-compatible providers unless that
same path also exposes the matching long-running command polling/control tools.

Example settings shape:

```toml
[[agents.custom_providers]]
name = "local-openai-compatible"
base_url = "http://localhost:1234/v1"
models = ["qwen3-coder", "llama-local"]
api_type = "open_ai_compatible"
api_key_env_var = "LOCAL_OPENAI_API_KEY"
```

Never store real API keys in the repo, chat, logs, tests, or docs.

## Model List

File: `app/src/ai/llms.rs`

The model list should come from `AISettings.custom_providers`, not from the Warp
server. Custom providers should be available without fetching hosted model
metadata.

## Usage, Paywall, And Entitlements

Relevant files:

- `app/src/workspaces/user_workspaces.rs`
- `app/src/ai/request_usage_model.rs`
- `app/src/settings_view/ai_page.rs`
- `app/src/settings_view/billing_and_usage_page.rs`
- `app/src/workspace/view.rs`
- `app/src/search/command_search/view.rs`

Policy:

- unlock local/non-cloud AI UX gates;
- hide or no-op cloud/server-only features;
- avoid global paid-plan spoofing that brings billing UI back;
- keep local provider and MCP workflows usable.

## Privacy And Local Diagnostics

Relevant files:

- `app/src/settings/privacy.rs`
- `app/src/settings_view/privacy_page.rs`
- `app/src/lib.rs`

Policy:

- manufacturer/product data collection is not acceptable in this fork;
- event collection code, collectors, persistence, uploads, settings, UI, and
  imports should be removed when encountered rather than merely hidden;
- remote incident upload code and placeholders are removed when encountered;
- cloud conversation storage is disabled;
- server privacy fetch/update code for these removed surfaces should not be
  kept as placeholders;
- setters for cloud conversation storage force `false`;
- app startup should not register manufacturer event collectors or remote
  incident uploaders.

## Startup And Auth

Relevant files:

- `app/src/root_view.rs`
- `app/src/auth/*`
- `app/src/auth/auth_manager/*`

Startup should go directly to the terminal workspace. Auth state should be local
and deterministic. Login, logout, browser auth, device auth, anonymous-user
creation, persisted hosted credentials, and custom-token flows should be no-op
or unreachable from visible UI.

## Upstream Sync

At the start of each repo work session, check whether `origin/master` has new
commits compared with local `master` before feature work or fixes:

```sh
git status --short --branch
git fetch origin master
git fetch fork master
git log --oneline master..origin/master
```

If upstream has new commits and the worktree is clean or the active changes are
already saved, merge upstream before starting new feature work:

```sh
ts=$(date +%Y%m%d-%H%M%S)
git branch backup/pre-upstream-merge-$ts
git merge --no-ff origin/master -m "Merge upstream master into local-first fork"
```

If the worktree contains substantial user or agent changes, do not mix an
upstream merge into the same edit without an explicit decision. Report the
pending upstream commits and continue only when doing so will not risk the
current work.

Conflict policy:

1. Prefer upstream names, renames, fields, modules, and API terminology when
   upstream changed them.
2. Preserve this fork's local-first behavior.
3. Do not restore login, account, billing, manufacturer data collection,
   remote incident upload, Firebase anonymous user, Warp cloud, Warp Drive, or
   hosted server auth flows.
4. Do not disable MCP wholesale.
5. Do not disable local custom providers or BYOK/custom-provider key config.
6. Do not route custom providers through Warp protobuf/cloud endpoints.

When upstream adds features, classify whether they depend on Warp's proprietary
backend. Hide or no-op clearly cloud-backed features. Preserve or localize
features that can reasonably run through local inference, local storage, local
tools, MCP, or BYOK/custom providers.

After every upstream merge, inspect the incoming commits for new cloud-backed,
hosted, account, billing, manufacturer data collection, remote incident upload,
Warp Drive, or proprietary auth behavior before continuing feature work. Keep
features only when they are already local-first or can be adapted to local
inference, local storage, local tools, MCP, or BYOK/custom providers. Remove,
hide, or no-op features that require proprietary Warp services and cannot be
made local-first.

## Verification Commands

Run commands from the repository root.

Minimal useful check:

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cargo fmt --check
cargo build --all-targets
cargo build --features local_only --all-targets
```

Focused local-first checks:

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
cargo fmt --check
cargo test -p warp --features local_only custom_provider -- --nocapture
cargo test -p warp --features local_only direct_openai -- --nocapture
cargo test -p warp --features local_only defaults_to_true -- --nocapture
cargo test -p warp --features local_only local_only_account_section_is_user_and_cloud_sections_are_hidden -- --nocapture
cargo build --all-targets
cargo build --features local_only --all-targets
TERM=xterm-256color NO_COLOR=1 CLICOLOR=0 ./script/run --features local_only --dont-open
```

Warnings from disabled hosted auth/cloud paths may appear while those upstream
modules still exist. Treat build failures as blockers; evaluate warnings case
by case and remove them when deleting the underlying hosted code is safe.

## macOS Bundle

If `cargo-bundle` is missing:

```sh
cargo install cargo-bundle \
  --git=https://github.com/burtonageo/cargo-bundle \
  --rev ae4c76e92c08774bf54ff077b1c52e3d1cd6c16d
```

Build the bundle:

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
TERM=xterm-256color NO_COLOR=1 CLICOLOR=0 ./script/run --features local_only --dont-open
```

The `TERM`, `NO_COLOR`, and `CLICOLOR` environment variables avoid an older
`cargo-bundle` failure:

```text
Error(Term(ColorOutOfRange))
```

Bundle path:

```text
target/debug/bundle/osx/WarpOss.app
```

Launch:

```sh
open target/debug/bundle/osx/WarpOss.app
```

Launch with logs:

```sh
target/debug/bundle/osx/WarpOss.app/Contents/MacOS/warp-oss
```

Simple binary:

```text
target/debug/warp-oss
```

## Build Cache Cleanup

Avoid full `cargo clean` when the current `.app` bundle should be preserved.
After final successful local builds or bundle builds, clean reproducible Cargo
cache and debug-output leftovers before ending the work iteration. This repo's
debug artifacts can grow by tens of gigabytes per build because `incremental`,
hashed `deps` outputs, and top-level debug binaries keep multiple feature-set
generations side by side.

Safe cleanup after successful heavy builds:

```sh
rm -rf target/debug/incremental \
  target/debug/deps \
  target/debug/build \
  target/debug/examples \
  target/debug/.fingerprint
rm -f target/debug/libwarp*.rlib \
  target/debug/warp \
  target/debug/warp-oss \
  target/debug/stable \
  target/debug/dev 2>/dev/null || true
```

This cleanup removes the simple top-level `target/debug/warp-oss` binary. That
binary is reproducible; preserve and verify the `.app` bundle instead when the
goal is to keep a launchable local build.

Verify the bundle still exists:

```sh
test -d target/debug/bundle/osx/WarpOss.app && echo app-bundle-present
test -x target/debug/bundle/osx/WarpOss.app/Contents/MacOS/warp-oss && echo app-binary-present
du -sh target
df -h .
```

## Secrets And Sensitive Data

Never commit, print, or include in chat:

- API keys;
- tokens;
- passwords;
- credentials;
- connection strings.

If sensitive values appear, replace them with:

```text
[REDACTED]
```

## Hard No List

Do not:

- create or revive a second long-lived branch such as `local-byok`;
- re-enable login;
- re-enable Firebase anonymous user creation;
- re-enable Warp cloud or Warp Drive cloud sync;
- re-enable manufacturer data collection or event upload;
- re-enable remote incident upload;
- re-enable billing, subscription, Stripe, upgrade, quotas, or paywall surfaces;
- re-enable proprietary hosted agent/server auth flows;
- pass custom providers through the Warp protobuf/cloud endpoint;
- add absolute local machine paths to `AGENTS.md`.
