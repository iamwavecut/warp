# Warp Local-First Fork

> Original upstream README: [warpdotdev/warp README.md](https://github.com/warpdotdev/warp/blob/master/README.md)

Experimental local-first fork of [Warp](https://github.com/warpdotdev/warp) for evaluation, source-code research, and local AI workflows.

This fork is **not** intended for production, commercial redistribution, or hosted/managed terminal use. Review the upstream licenses before using or distributing it:

- [AGPL v3](LICENSE-AGPL) - most of this repository.
- [MIT](LICENSE-MIT) - Warp UI crates (`warpui_core`, `warpui`).

## Current Direction

This fork no longer treats local mode as an optional product branch. The app is expected to run as a local-first terminal by default:

- no required registration, browser login, device auth, or Firebase anonymous user;
- no manufacturer data collection, event upload, or remote incident upload;
- no Warp account, billing, subscription, Stripe, upgrade, quota, paywall, referral, Teams, Shared Blocks, or Warp Drive cloud UI;
- no Warp cloud/protobuf endpoint for custom AI provider calls;
- no proprietary hosted Warp agent/server auth flow;
- preserved local terminal UX, MCP, local tooling, vertical tabs/sidebar, and local agent surfaces where they can work locally;
- BYOK and OpenAI-compatible custom providers are the primary AI provider path.

The historical `local_only` Cargo feature remains only as a compatibility alias for older commands. New code should not introduce behavior forks on `local_only`; if behavior belongs in this fork, make it the default.

Some upstream cloud/server types still exist in the source tree because local UI and history code reuse internal Rust types. Their presence is not a promise that the hosted Warp service path is supported in this fork.

## User And Settings UI

The former account surface is now a local `User` surface. It shows the local system user's display name when available and falls back to the system username. It must not show Warp test-user strings, plan labels, billing state, upgrade prompts, logout, settings sync, referrals, invites, Slack, feedback, Teams, Cloud Platform, Environments, Shared Blocks, or Warp Drive entry points.

Settings exposes `LLM providers` for custom OpenAI-compatible providers. The old proprietary key-only provider UI is intentionally removed from the visible settings surface.

## Configure LLM Providers

Use Settings > Agents > LLM providers, or edit the settings TOML directly:

```toml
[[agents.custom_providers]]
name = "local-openai-compatible"
base_url = "http://localhost:1234/v1"
models = ["qwen3-coder", "llama-local"]
api_type = "open_ai_compatible"

# Optional. If omitted, Warp first tries a custom key stored in secure storage.
# If neither exists, it sends the request without an Authorization header.
api_key_env_var = "LOCAL_OPENAI_API_KEY"
```

In the UI, the environment-variable field may be entered as either `LOCAL_OPENAI_API_KEY` or `$LOCAL_OPENAI_API_KEY`; the leading `$` is stripped before lookup.

Model IDs are exposed internally as:

```text
custom/<provider-name>/<model-id>
```

For example:

```text
custom/local-openai-compatible/qwen3-coder
```

Custom provider requests go directly to:

```http
POST <base_url>/chat/completions
Authorization: Bearer ***   # only when a key is configured
```

They do not use Warp's `/ai/multi-agent` cloud endpoint.

## Build

Common setup:

```bash
git clone https://github.com/iamwavecut/warp.git
cd warp
git lfs install
git lfs pull
```

Recommended checks:

```bash
cargo fmt --check
cargo build --all-targets
cargo build --features local_only --all-targets
```

Run the debug binary:

```bash
./target/debug/warp-oss
```

Build a macOS `.app` bundle:

```bash
cargo install cargo-bundle --git=https://github.com/burtonageo/cargo-bundle --rev ae4c76e92c08774bf54ff077b1c52e3d1cd6c16d
TERM=xterm-256color NO_COLOR=1 CLICOLOR=0 ./script/run --features local_only --dont-open
```

Bundle path:

```text
target/debug/bundle/osx/WarpOss.app
```

Launch from the repository root:

```bash
open target/debug/bundle/osx/WarpOss.app
```

## Verification

The current local-first fork is typically verified with:

```bash
cargo fmt --check
cargo test -p warp --features local_only custom_provider -- --nocapture
cargo test -p warp --features local_only direct_openai -- --nocapture
cargo test -p warp --features local_only defaults_to_true -- --nocapture
cargo test -p warp --features local_only local_only_account_section_is_user_and_cloud_sections_are_hidden -- --nocapture
cargo build --all-targets
cargo build --features local_only --all-targets
TERM=xterm-256color NO_COLOR=1 CLICOLOR=0 ./script/run --features local_only --dont-open
```

Warnings from dead hosted auth/cloud paths may appear while those upstream modules still exist. Treat build failures as blockers; warnings should be evaluated case by case and removed when deleting the underlying hosted code is safe.

## Upstream Docs

- [Original README](https://github.com/warpdotdev/warp/blob/master/README.md)
- [Original repository](https://github.com/warpdotdev/warp)
- [CONTRIBUTING.md](CONTRIBUTING.md)
- [WARP.md](WARP.md)
