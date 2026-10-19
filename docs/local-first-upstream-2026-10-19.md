# Local-first upstream intake — 2026-10-19

Upstream `ae60375ef1d3895908756878fe4a07cbe703ce1a` changes only the hosted
standby callback contract. This selective ancestry merge retains the fork's
application tree, provider path and instructions without code or dependency edits.

## Localization assessment

The user-visible result is keeping a server-allocated sandbox ready for a future
hosted run. Callers are `standby::prepare_and_wait`,
`report_preparation_outcome` and `maintain_sandbox`: they fetch configuration,
send Ready/Failed and heartbeat callbacks, and wait for server-owned expiry.
The new header selection and `generation/preparation` to `fingerprint/spec`
wire-field changes serve only those calls and their GraphQL types/tests.

The fork has no `PrewarmedSandboxClient`, corresponding schema or CLI command.
Its existing local `AgentDriver` already prepares configured repositories,
terminal, skills and MCP before a prompt, without a lease or managed credentials.
There is no independent local consumer of the changed wire fields or callback
headers, and no new local user result to implement. Keep local preparation;
exclude the hosted GraphQL/REST contract and its fixtures. Do not create a
standby pool, telemetry, credential lookup or no-op client for absent consumers.

## Verification and delivery

No behavior changed, so no new regression test is added. Run the automation's
required final formatting check and both all-target build variants. Record
actual results, pushed merge SHA, release installation proof and exact owned
cleanup in automation memory. No upstream code is accepted in this iteration.

Final gates passed: `cargo fmt --check`, `cargo build --all-targets` and
`cargo build --features local_only --all-targets`. No runtime tests were run
for this ancestry/documentation-only intake. The immutable batch exited zero
at its complete stage, then removed its private verification target.
