# WarpOss P1.2 Direct OpenAI Tool Adapter

## Goal

Make every already-existing, non-orchestration local agent executor that is
advertised for a custom OpenAI-compatible model round-trip through the direct
adapter. The path must remain fully local apart from the endpoint selected by
the user, and must never fall back to Warp services.

## Scope

Add OpenAI tool schemas, incoming-call parsing, and history serialization for:

- `init_project` and `open_code_review`;
- `suggest_new_conversation`;
- `read_documents`, `create_documents`, and `edit_documents`;
- `insert_review_comments`;
- `fetch_conversation` when its existing feature gate enables it;
- `ask_user_question` when its existing feature and request gates enable it;
- `use_computer` and `request_computer_use` when their existing gates enable
  them;
- the complete long-running shell family:
  `write_to_long_running_shell_command`, `read_shell_command_output`, and
  `transfer_shell_command_control_to_user`.

Use the current protobuf messages and their existing conversions/executors as
the contract. Keep naming and JSON shapes simple and OpenAI-compatible.

## Boundaries

- Do not add, advertise, or parse orchestration tools in this stage:
  `subagent`, `start_agent`, `start_agent_v2`, `run_agents`, or
  `send_message_to_agent`.
- Do not revive deprecated hosted plan tools or introduce cloud document sync.
- Do not retry automatically after any shell, MCP, file-edit, document-edit,
  computer-use, or other side-effectful action has begun.
- Keep `run_shell_command.wait_until_completion` forced to `true` unless the
  same request exposes and supports the complete long-running shell family.
  Once complete, preserve the model's explicit `false` and round-trip command
  IDs/results without reordering history.
- Advertise only tools present in `supported_tools`; feature/request gates in
  `get_supported_tools` remain authoritative.
- Malformed arguments and unsupported tool calls must fail locally with a
  useful error and no Warp request.

## Tests

Start with failing focused tests, then implement. Cover at least:

1. The full non-orchestration supported set produces exactly the expected
   OpenAI function names, with no orchestration names.
2. Every new function parser produces the intended protobuf tool variant and
   validates required fields/IDs/enums.
3. Every corresponding historical protobuf tool call serializes back to the
   same OpenAI function name and semantically equivalent JSON arguments.
4. Feature-gated tools are omitted when disabled.
5. `wait_until_completion=false` is preserved only when all long-running shell
   control tools are advertised; otherwise it remains forced to `true`.
6. Malformed and unknown calls return local errors.
7. Existing direct-provider tests continue to pass with and without
   `local_only`.

## Verification

Run after implementation is complete:

```sh
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"
CARGO_INCREMENTAL=0 cargo fmt --check
CARGO_INCREMENTAL=0 cargo test -p warp direct_openai -- --nocapture
CARGO_INCREMENTAL=0 cargo test -p warp --features local_only direct_openai -- --nocapture
```

