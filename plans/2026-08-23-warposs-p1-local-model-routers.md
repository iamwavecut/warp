# WarpOss P1.8 Local Custom Model Routers

## Goal

Restore upstream's local YAML custom-router definitions and editor, but resolve
every router to a concrete configured `custom/<provider>/<model>` before the
direct request. Router definitions, prompts, and decisions must never be sent to
Warp protobuf or a classifier service.

## Semantic Upstream Intake

Port the local-only parts of upstream `custom_model_routers`:

- strict one-router-per-YAML parsing and validation;
- stable filename-derived local identity;
- filesystem watcher, picker entry, create/edit/delete UI, and actionable parse
  errors;
- complexity buckets and ordered prompt rules.

Remove/replace:

- `to_proto` and `Request.Settings.custom_model_routers` serialization;
- cloud/team router prefixes and server-synced router handling;
- auth-personalized text, upgrade/billing affordances, telemetry, and Warp
  discovery.

Harden upstream's raw write/delete helpers with the same atomic, canonical-path,
compare-and-swap rules used by other local repositories.

## Local Resolution

- Allow only concrete target IDs that resolve through current custom provider
  settings. Reject built-in auto IDs, nested routers, missing providers/models,
  duplicate provider names, invalid provider configs, and unavailable required
  capabilities.
- Resolve the router in-process before `resolve_custom_provider_route`; the
  resulting request uses the ordinary direct OpenAI adapter and contains only
  the concrete target model ID.
- Compute the router's catalog capabilities conservatively as the intersection
  of its reachable targets; context-window size is the minimum declared target
  limit. Disable the picker row with a local reason when any required/default
  target is invalid.
- Complexity routing is deterministic and documented: derive an easy/medium/hard
  bucket from bounded request facts already present locally (prompt/context
  size, attachments, code-review/edit/tool requirements), with missing buckets
  falling back to `default`.
- Prompt routing is deterministic: normalize/tokenize the current user request,
  choose the first ordered rule with a meaningful token match, otherwise use
  `default_model`. No LLM classifier in P1.
- Log only router ID, selected bucket/rule index, and concrete model ID; do not
  log prompt contents.

## UI And Persistence

- Store routers under the existing local custom-router directory, one UUID- or
  stable-filename-backed YAML file each. Display-name changes preserve ID.
- Save atomically and fail on concurrent external edits. Delete only a validated
  file in the router directory after confirmation.
- Editor model dropdowns list concrete custom models only. No Warp models,
  hosted auto targets, account, or upgrade footer.
- Watcher updates reconcile active selections: a removed/invalid router falls
  back to a concrete configured custom model or the existing no-provider state,
  never to hosted AI.

## Tests

Start with failing parser/resolver/repository/catalog tests. Cover at least:

1. Strict YAML parse/serialize round-trip, stable ID across display rename, and
   atomic CRUD/restart/watch behavior.
2. Deterministic complexity buckets, optional-bucket fallback, ordered prompt
   matching, tie behavior, Unicode normalization, and default fallback.
3. Nested/auto/missing/duplicate/invalid/capability-incompatible targets fail
   locally and make no HTTP/Warp request.
4. Catalog capability intersection/context minimum and active-selection
   reconciliation after edit/delete.
5. The HTTP request reaches only the selected concrete custom endpoint and
   contains no router definition or Warp URL.
6. Default and `local_only` behavior is identical.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp custom_model_router -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only custom_model_router -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp direct_openai -- --nocapture
```

