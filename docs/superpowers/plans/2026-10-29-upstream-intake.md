# Local-first upstream intake implementation plan

> Execute inline. User instructions require primary master, no subagents,
> test-first edits but verification only after all business/code changes.

**Goal:** Intake upstream `5a55a2aca77339ad75637bed650352369ee7fcb9`
without restoring hosted product behavior.

**Architecture:** Keep the fork tree and adapt terminal/runtime changes at
their existing local consumers. Retain existing direct-provider and ACP paths.
The heartbeat instructions and AGENTS.md are the specification.

## Constraints and review focus

- Preserve custom providers, MCP, local history and user-owned CLI lifecycles.
- Pending commands must wait for final bootstrap precmd, then dispatch once.
- A command that never starts and one that never exits need one bounded wait.
- Timeout must not type into or terminate a user-owned terminal process.
- Tab classification must distinguish completion, autosuggestion and custom keys.
- Do not restore Warp TUI detection for the ACP socket bridge.
- Do not import CI analytics, upstream private auth or managed integrations.
- Use only private temporary paths, remove them even on errors, preserve apps.

## Tasks

1. Add regression tests before implementation:
   - `app/src/settings_view/features_page_tests.rs`: Tab for completions,
     Tab for autosuggestions, and no Tab produce the corresponding setting.
   - `app/src/terminal/input_test.rs`: pending command waits across the
     Bootstrapped/final-precmd gap and dispatches once after the final event.
   - `app/src/ai/agent_sdk/driver/environment_tests.rs`: deadline covers
     command start and exit, preserves completion/errors, drops pending waits.
   - `app/src/terminal/cli_agent_tests.rs`: existing fork behavior does not
     treat Warp ACP bridge commands as interactive CLI agents.
2. Implement the Tab classifier in `features_page.rs`, route refresh through it.
   Add the precmd guard to `Input::execute_pending_command` and a 30-minute
   `await_environment_command` around the local environment command lifecycle.
3. Retain local CI configuration; update its existing pinned install-action.
   Record every incoming commit and its local/remote-only consumer assessment.
4. Review the final diff and local-first boundaries, then run formatting, both
   all-target builds and nonempty focused app-test filters. Do not restore
   legacy hosted integration fixtures to compile unrelated test modules.
5. Commit/push, independently verify fork/master, then build production with
   private pinned cargo-bundle. Verify UUID, signatures, dependency paths and
   source/installed equality. Do not launch the app. Remove owned artifacts.

## Execution rulings

- User verification ordering overrides an early RED run; do not claim a local
  RED result. User master/no-subagent rules override generic skill defaults.
- Automation memory is the durable execution ledger; the external mktemp
  directory owns transient logs/scripts/targets. Do not create another worktree.
- Review ruling: ModelSpawner executes queued callbacks even when their waiter
  is dropped. Guard the queued environment callback with oneshot liveness and
  test expiry while holding the main thread, before any terminal buffer write.
- Existing Linux PATH append runs after profiles. The incoming private fallback
  directory is produced only by the absent hosted entrypoint; assess that
  transport separately from ordinary local command availability.
