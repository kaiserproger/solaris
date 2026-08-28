# Phase 5 Luau runtime lifecycle diagnostics — 2026-08-26

## Scope

This checkpoint advances Phase 5 item 2 in `docs/PUBLIC_ALPHA_PLAN.md` without claiming that the whole item is complete. The existing API `0.6.0` runtime already had one sandboxed Luau VM per plugin, fixed memory and instruction limits, wall-clock interruption, capability-gated command admission, bounded event/command queues, and per-plugin failure isolation. The missing boundary addressed here was lifecycle observability: a plugin could be disabled correctly while the composition root later learned only whether the host thread panicked.

The slice is intentionally limited to `mc-script` lifecycle reporting and `mc-server` shutdown diagnostics. It does not add hot reload, new gameplay authority, direct world/entity handles, or a new Luau API surface.

## Runtime contract

`LuaHost::join()` now returns a typed `LuaHostExitReport` on a non-panicking host thread.

The report contains:

- number of plugins accepted at host startup;
- number still enabled when the host exits;
- a deterministic list of plugins disabled during runtime;
- for every disabled plugin, its plugin id, disable stage and bounded diagnostic message;
- the host exit reason.

Disable stages are closed host-side values:

- `handler` — the subscribed or targeted Luau handler trapped or exceeded a runtime budget;
- `batch_rejection_handler` — `on_command_batch_rejected` itself failed;
- `command_admission` — a completed Luau handler produced a batch that the host authority rejected.

Diagnostic text is UTF-8-safe and capped at 4 KiB so a sandboxed plugin cannot turn an error string into unbounded host-owned terminal-report memory.

Exit reasons distinguish:

- `event_queue_closed` — normal drained host shutdown;
- `command_queue_closed` — the server-side command consumer disappeared while the host was still processing work;
- `player_command_authority_unavailable` — player-command ownership authority could not be used at startup;
- `progress_observer_closed` — bounded test/diagnostic progress observer ended;
- `startup_aborted` — strict startup failed before a runnable host was admitted.

Runtime disablement still unregisters the plugin's player-command roots before later host progress. The report is additive observability; it does not weaken the existing isolation behavior or change event/command ordering.

## Composition-root shutdown behavior

`mc-server` now consumes the typed report when joining the Luau host. It emits:

- one structured warning for each plugin that remained disabled at shutdown, including plugin id, disable stage and message;
- a warning when the host exited by a non-normal reason;
- one structured terminal summary with loaded, active, disabled and exit-reason fields.

The normal server lifecycle remains unchanged: admitted committed script events drain first, `server.stopping` is enqueued through the required event path, event admission is closed, the host drains the queue and exits with `event_queue_closed`, and the composition root joins it.

## Current reload semantics

API `0.6.0` still does **not** provide in-place live reload. `plugin.toml`, `config.toml`, and `main.lua` form the startup snapshot; changing them requires replacing the host through server restart. This checkpoint documents that limitation explicitly rather than treating restart-only behavior as atomic hot reload.

Phase 5 item 2 therefore remains open. A later bounded slice must define and prove a safe replacement/reload operation that preserves the last valid runtime when new configuration or source fails validation/startup.

## Verification

Focused feature verification on the changed tree:

```text
cargo test -p mc-script --features lua-runtime --quiet
running 186 tests
...
test result: ok. 186 passed; 0 failed; 0 ignored
```

The existing suite includes the sandbox, capability, resource-budget and failure-isolation gates such as:

- `lua_runtime_does_not_expose_filesystem_process_or_debug_libraries`;
- `lua_infinite_handler_is_stopped_by_instruction_budget`;
- `runtime_timeout_stops_an_infinite_lua_handler_before_the_fuel_ceiling`;
- `lua_memory_exhaustion_fails_the_invocation_without_returning_its_partial_batch`;
- `lua_rejects_an_undeclared_storage_capability_before_queuing_a_command`;
- `failed_handler_is_disabled_without_stopping_other_plugins`.

The failure-isolation test now additionally proves the terminal report contains `loaded=2`, `enabled_at_exit=1`, the disabled plugin id, `handler` stage, the original failure text, and normal `event_queue_closed` shutdown. A separate zero-plugin host test proves a clean event-queue close reports no disabled plugins. Focused lifecycle regressions also prove `command_queue_closed` and `player_command_authority_unavailable` remain distinct terminal classifications, and that both `batch_rejection_handler` and `command_admission` disable paths retain exactly one diagnostic even when a second event is processed after disablement. The diagnostic-bound test uses multi-byte UTF-8 input and proves the retained message remains within the 4 KiB cap.

Benchmark: not applicable. This checkpoint adds bounded terminal diagnostics on disable/shutdown paths and does not change a mapped hot-path performance contract.

## Independent review

Exactly one independent read-only reviewer returned `CHANGES` with three finite findings: direct coverage was missing for the `command_queue_closed` and `player_command_authority_unavailable` exit reasons; exactly-once disable reporting was proven only for the normal handler-failure path rather than `batch_rejection_handler` and `command_admission`; and the evidence still quoted the earlier 181-test count. The four lifecycle regressions described above were added, the suite now passes 186/186, and the evidence count was corrected. Post-fix strict `mc-script` Clippy, formatter, code-health and scoped diff-check all pass. Per repository policy, no second reviewer was run after addressing those bounded findings.

## Disposition

Lifecycle diagnostics/failure-reporting sub-slice: **PASS**.

At this historical sub-checkpoint Phase 5 item 2 was still open. Safe prepared reload and the Unix production trigger were implemented subsequently in [`phase5-luau-safe-reload-boundary-2026-08-26.md`](phase5-luau-safe-reload-boundary-2026-08-26.md), whose combined checkpoint later received terminal independent `PASS`; Phase 5 item 2 is therefore now closed. The sandbox, capability, budget and failure-isolation behavior remains existing validated functionality rather than being reimplemented here.
