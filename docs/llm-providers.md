# LLM connections

Open **Settings → AI → LLM providers** to configure each connection separately:

- **Provider ID**: the unique name of the connection, shown in its settings card and in the model picker as `provider / model-id`. Saved model selections and secure keys also use this name.
- **Protocol**: OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages.
- **Base URL**: the API root, usually ending in `/v1`; omit `/chat/completions`, `/responses`, or `/messages`.
- **Prompt caching**: enabled by default for new and existing connections. See the protocol differences below.

API keys remain in secure storage. Alternatively, configure an API-key environment variable; a leading `$` is accepted. Keyless local endpoints are supported. Use **Check models** to retrieve the model pool with the selected protocol's authentication.

## Configuration

Each entry under `agents.custom_providers` represents one connection. Multiple entries can share a URL while using different names, protocols, model lists and caching settings.

```toml
[[agents.custom_providers]]
name = "work-responses"
base_url = "https://api.openai.com/v1"
models = ["your-model-id"]
api_type = "open_ai_responses"
prompt_caching = true
api_key_env_var = "OPENAI_API_KEY"

[[agents.custom_providers]]
name = "anthropic"
base_url = "https://api.anthropic.com/v1"
models = ["your-claude-model-id"]
api_type = "anthropic_messages"
prompt_caching = true
api_key_env_var = "ANTHROPIC_API_KEY"
```

Omitting `api_type` preserves the legacy `open_ai_compatible` protocol. Model IDs remain `custom/<name>/<model-id>`. Older settings with an `alias` field still load; that field is ignored, and the existing provider name, model selections and secure-key lookup remain unchanged.

## Prompt caching

Caching reuses a provider's computation for a matching prompt prefix. Every request still generates a new answer and executes its own tools. Cache hits depend on the endpoint, model, minimum prefix length and expiration; turning caching on cannot guarantee a hit.

| Protocol | Caching enabled | Caching disabled |
| --- | --- | --- |
| OpenAI Responses | Uses the endpoint's default implicit caching | Sends `prompt_cache_options = { mode = "explicit" }` without breakpoints; requires an endpoint/model supporting explicit-only caching (OpenAI GPT-5.6+). Unsupported endpoints may reject the request; Warp does not silently retry with caching enabled. |
| Anthropic Messages | Sends top-level `cache_control = { type = "ephemeral" }` for automatic prefix caching | Omits cache control |
| OpenAI Chat Completions | Provider-managed automatic caching | This protocol has no universal cache-off field; the endpoint remains responsible for its caching policy |

Responses requests set `store = false` independently of caching. Conversation history is supplied from the local client; Warp hosted conversation storage is not used. Responses assistant phases and encrypted reasoning items, plus Anthropic thinking blocks and signatures, are retained locally for continuation and bound to the originating protocol, connection and model. Anthropic thinking also depends on the preceding system prompt, tool definitions and messages. If that prefix changes (for example during local compaction), incompatible thinking is omitted while ordinary text and tool history remain.

Protocol references: [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching), [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching), [Anthropic thinking continuity](https://platform.claude.com/docs/en/build-with-claude/thinking#preserving-thinking-blocks).
