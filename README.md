# Warp Local-Only BYOK Fork

> Original upstream README: [warpdotdev/warp README.md](https://github.com/warpdotdev/warp/blob/master/README.md)

Experimental local-only fork of [Warp](https://github.com/warpdotdev/warp) for evaluation and source-code research.

This fork is **not** intended for production, commercial redistribution, or hosted/managed terminal use. Review the upstream licenses before using or distributing it:

- [AGPL v3](LICENSE-AGPL) — most of this repository.
- [MIT](LICENSE-MIT) — Warp UI crates (`warpui_core`, `warpui`).

## What changed

When built with `local_only`, this fork changes Warp from a cloud/account-oriented product into a local-first client:

- No registration, login, browser auth, device auth, or Firebase anonymous user.
- Starts directly in the terminal workspace.
- Telemetry macros are no-ops.
- Telemetry collector, app focus telemetry, shutdown telemetry flush, crash reporting, and Sentry are not initialized.
- Cloud/account UI entry points are hidden or disabled, including Warp Drive, data-management/delete-account UI, Agent Management, Ambient Agents RTC, Cloud Mode, Cloud Conversations, cloud environments, managed secrets, orchestration cloud flags, and force-login behavior.
- AI quotas are local/unlimited; server quota refresh is skipped.
- Local/non-cloud AI UX gates are unlocked in `local_only`, including BYOK/custom providers, active AI, prompt/code suggestions toggles, next-command availability, AI autonomy gates, and codebase-context limits.
- Billing, subscription, upgrade, Stripe, cloud-agent capacity, and other hosted/cloud-only surfaces stay hidden or no-op in `local_only`.
- AI model list is populated from local custom provider settings.
- Agent requests for custom models go directly to OpenAI-compatible `/chat/completions` endpoints using a normal HTTP client. They do **not** use Warp's `/ai/multi-agent` cloud endpoint or try to pass custom provider data through Warp protobuf request settings.

Some upstream cloud/protobuf types remain in the source tree because existing UI/history code uses those internal Rust types for local event handling. The local-only custom-provider execution path does not send requests to Warp's cloud AI endpoint.

## Branch

The maintained branch is:

```text
master
```

Older development branch names such as `local-byok` are no longer used for new work.

## Configure custom providers

Add OpenAI-compatible providers to the Warp settings TOML:

```toml
[[agents.custom_providers]]
name = "local-openai-compatible"
base_url = "http://localhost:1234/v1"
models = ["qwen3-coder", "llama-local"]
api_type = "open_ai_compatible"

# Optional. If omitted, Warp first tries a custom key stored in secure storage;
# if neither exists, it sends the request without an Authorization header.
api_key_env_var = "LOCAL_OPENAI_API_KEY"
```

Model IDs are exposed internally as:

```text
custom/<provider-name>/<model-id>
```

For example:

```text
custom/local-openai-compatible/qwen3-coder
```

The direct client sends:

```http
POST <base_url>/chat/completions
Authorization: Bearer ***   # only when a key is configured
```

with `stream = false` for the current implementation.

## Build

Common setup:

```bash
git clone https://github.com/iamwavecut/warp.git
cd warp
git lfs install
git lfs pull
```

Compiler/build checks:

```bash
cargo fmt --check
cargo build --features local_only --all-targets
cargo build --features gui,local_only -p warp --bin warp-oss
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

## Verification

The current macOS development checkout is verified with:

```bash
cargo fmt --check
cargo test -p warp --features local_only parses_custom_model_ids_into_provider_and_model
cargo test -p warp --features local_only builds_chat_completions_url_from_base_url
cargo build --all-targets
cargo build --features local_only --all-targets
cargo build --features gui,local_only -p warp --bin warp-oss
TERM=xterm-256color NO_COLOR=1 CLICOLOR=0 ./script/run --features local_only --dont-open
```

`cargo build --features local_only --all-targets` is warning-free.

## Upstream docs

- [Original README](https://github.com/warpdotdev/warp/blob/master/README.md)
- [Original repository](https://github.com/warpdotdev/warp)
- [CONTRIBUTING.md](CONTRIBUTING.md)
- [WARP.md](WARP.md)
