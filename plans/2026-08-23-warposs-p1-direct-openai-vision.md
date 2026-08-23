# WarpOss P1.3 Direct OpenAI Vision Attachments

## Goal

Deliver locally prepared image context to a user-selected OpenAI-compatible
`/chat/completions` endpoint using standard multimodal content parts. Requests
must fail locally when vision is disabled or image data is invalid, with no Warp
fallback and no hidden upload service.

## Design

- Replace string-only `ChatMessage.content` with an untagged representation
  that can serialize either the existing string or an array of OpenAI content
  parts.
- Keep system, assistant, and tool-result content as strings. Build user
  messages with ordered `text` and `image_url` parts when an input contains
  `AIAgentContext::Image`.
- Encode each image URL as
  `data:<validated-mime-type>;base64,<validated-base64-data>`. Never write image
  bytes to a remote artifact store or log their payload.
- Accept only MIME types already supported by `util::image`, decode base64 before
  dispatch, validate the decoded image, and enforce the existing image count and
  byte limits even for restored/programmatically constructed context.
- Keep textual file/selected-text/rules context local and include it as text.
  Reject binary `AIAgentContext::File` content explicitly before HTTP rather
  than silently omitting it or switching providers.
- Preserve the relative order of text context and images within each user turn,
  and preserve image-bearing historical user messages when the local persisted
  conversation is rebuilt for the endpoint.
- Once this adapter is complete, `EffectiveCustomProviderCapabilities.vision`
  may reflect the user's configured `vision` value. It must remain false for
  providers that do not opt in.

## Boundaries

- Use only the direct endpoint selected in local settings.
- No signed URLs, Warp artifact upload, cloud attachment metadata, auth, or
  telemetry.
- Do not read arbitrary paths for an image part. Only use image bytes already in
  `ImageContext`; ordinary text file context follows the existing bounded read
  path before request construction.
- Do not enable embeddings, transcription, or unrelated tool behavior.
- Error text must name the local capability/data problem without claiming a
  provider failure when no HTTP request was made.

## Tests

Start with failing tests. Cover at least:

1. A vision-enabled provider receives ordered OpenAI `text` + `image_url`
   content parts with the expected data URL and no Warp request.
2. Multiple images retain their order and remain within the existing count and
   size limits.
3. Vision-disabled providers, unsupported MIME types, malformed base64,
   oversized images, and binary file context fail before HTTP.
4. Text-only requests keep the prior string content shape and behavior.
5. Persisted historical image context is serialized as multimodal user content,
   not as the old placeholder.
6. Request/error logs never contain base64 image data.
7. Existing direct-provider tests pass with and without `local_only`.

## Verification

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp direct_openai -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only direct_openai -- --nocapture
```

