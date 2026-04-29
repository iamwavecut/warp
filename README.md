# Warp Local-Only BYOK Fork

> Original upstream README: [warpdotdev/warp README.md](https://github.com/warpdotdev/warp/blob/master/README.md)

This repository is a local-only experimental fork of [Warp](https://github.com/warpdotdev/warp). It keeps the open-source Warp client codebase, but changes the default product assumptions: no registration, no login flow, no Firebase-backed anonymous user, no Warp cloud dependency, and BYOK-oriented AI plumbing.

## Notice

This fork is provided **only for evaluation, research, education, and source-code review**.

In accordance with the licensing model of the original Warp project, this fork is **not intended for production use, commercial redistribution, or use as a hosted/managed terminal product**. Review the upstream licenses before using, modifying, or distributing this code:

- [AGPL v3 license](LICENSE-AGPL) — applies to most of this repository.
- [MIT license](LICENSE-MIT) — applies to Warp's UI framework crates (`warpui_core` and `warpui`).

This notice is not legal advice. If you want to use this fork beyond local evaluation, review the upstream license obligations and consult qualified counsel.

## What this fork changes

### Local-only application mode

- Extends the existing `local_only` Cargo feature.
- Runs without login, registration, or browser/device auth.
- Adds `Credentials::Local` for local-only builds.
- Skips persisted server credentials.
- Starts directly in the terminal workspace instead of the auth/onboarding UI.

### No cloud-gated product flow

When built with `local_only`, cloud-dependent feature flags are forcibly disabled after feature initialization, including:

- Cloud Mode flags (`CloudMode`, `CloudModeFromLocalSession`, `CloudModeHostSelector`, `CloudModeImageContext`)
- Cloud Conversations
- Warp Managed Secrets
- Warp Environments / environment slash command
- Orchestration cloud event/push flags
- Oz handoff / sync ambient plans
- Force-login behavior

### BYOK-oriented AI groundwork

- Adds `LLMProvider::Custom(String)`.
- Adds custom API-key storage: `ApiKeys.custom: HashMap<String, String>`.
- Adds `ApiKeyManager::set_custom_key()`.
- Adds `CustomProviderConfig` to AI settings:
  - provider name
  - OpenAI-compatible base URL
  - model ID list
  - API type (`open_ai_compatible`)
- Adds `agents.custom_providers` TOML settings support.
- Builds local `ModelsByFeature` choices from `AISettings.custom_providers` instead of fetching model lists from Warp servers in `local_only` mode.

### Unlimited local AI quota checks

- Disables server quota refresh in `local_only`.
- Initializes request limits as effectively unlimited.
- Makes AI availability checks pass locally.

### Current limitation

The UI/settings/model-list groundwork for custom providers is present, but direct request routing to a custom OpenAI-compatible endpoint still needs a dedicated local HTTP path.

Upstream Warp AI requests normally pass through `warp_multi_agent_api` / `warp-proto-apis`. That protobuf request settings type does not currently carry arbitrary custom provider base URLs. The remaining work is to bypass that cloud-oriented API path for `LLMProvider::Custom(_)` and route directly to the configured endpoint.

## Repository and branch

- Fork: `https://github.com/iamwavecut/warp`
- Branch: `local-byok`
- Primary feature flag: `local_only`

## Custom provider configuration

Example TOML shape for a local settings file:

```toml
[[agents.custom_providers]]
name = "local-openai-compatible"
base_url = "http://localhost:1234/v1"
models = ["qwen3-coder", "llama-local"]
api_type = "open_ai_compatible"
```

API keys for custom providers are stored separately by the API-key manager, keyed by provider name.

## Build instructions

The commands below build the local-only OSS binary with the GUI feature enabled:

```bash
cargo build --features gui,local_only -p warp --bin warp-oss
```

The resulting debug binary is usually:

```text
target/debug/warp-oss
```

For a faster compiler-only check without GUI bundling:

```bash
cargo build --features local_only -p warp
```

### Common prerequisites

All platforms need:

1. Git
2. Git LFS
3. Rust/Cargo via rustup
4. A checked-out clone of this fork and branch

```bash
git clone https://github.com/iamwavecut/warp.git
cd warp
git checkout local-byok
git lfs install
git lfs pull
```

### macOS

Prerequisites:

- macOS with Xcode installed
- Xcode command-line tools selected
- Homebrew
- Rust/Cargo
- Git LFS

Bootstrap using upstream scripts if needed:

```bash
./script/bootstrap
```

Build the local-only OSS binary:

```bash
cargo build --features gui,local_only -p warp --bin warp-oss
```

Run the built binary directly:

```bash
./target/debug/warp-oss
```

Optional: build a macOS `.app` bundle. This requires `cargo-bundle` and signing tooling installed by the bootstrap script:

```bash
./script/run --features local_only --dont-open
```

The app bundle is created under:

```text
target/debug/bundle/osx/WarpOss.app
```

### Linux

Prerequisites:

- A recent Linux distribution with development packages
- Rust/Cargo
- Git LFS
- System libraries required by the upstream Warp Linux build

Bootstrap on Debian/Ubuntu-like systems:

```bash
./script/bootstrap
```

Build the local-only OSS binary:

```bash
cargo build --features gui,local_only -p warp --bin warp-oss
```

Run:

```bash
./target/debug/warp-oss
```

Optional packaging helpers exist under `script/linux/`:

```bash
./script/linux/bundle
./script/linux/bundle_appimage
./script/linux/bundle_deb
./script/linux/bundle_rpm
```

Those package scripts may require additional platform packages and should be treated as upstream experimental tooling.

### Windows

Prerequisites:

- Windows 10/11
- Git for Windows
- Rust/Cargo via rustup
- Git LFS
- CMake
- Inno Setup if building an installer

Bootstrap from PowerShell:

```powershell
.\script\windows\bootstrap.ps1
```

Build the local-only OSS binary:

```powershell
cargo build --features gui,local_only -p warp --bin warp-oss
```

Run:

```powershell
.\target\debug\warp-oss.exe
```

Optional installer build:

```powershell
cargo build --features gui,local_only -p warp --bin warp-oss --release
iscc .\script\windows\windows-installer.iss /DMyAppExeName=warp-oss.exe /DTargetProfileDir=release
```

See `script/windows/README.md` for upstream Inno Setup details.

## Verification performed for this fork

On the current macOS development machine, the following checks were run successfully:

```bash
cargo fmt --check
cargo build --features local_only --all-targets
cargo build --features gui,local_only -p warp --bin warp-oss
```

The repository scripts referenced by the Linux/macOS paths were syntax-checked with `bash -n`, and the Windows build scripts were inspected for the documented PowerShell entrypoints.

## Upstream documentation

For the original product documentation and contribution guide, see:

- [Original README](https://github.com/warpdotdev/warp/blob/master/README.md)
- [Original repository](https://github.com/warpdotdev/warp)
- [CONTRIBUTING.md](CONTRIBUTING.md)
- [WARP.md](WARP.md)
