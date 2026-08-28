# Phase 5 Luau safe reload boundary — 2026-08-26

## Scope

This checkpoint implements the reload portion of Phase 5 item 2 while preserving the already deployed `ScriptBoundary` used throughout `mc-net`. It includes both the host-level atomic replacement boundary and the explicit Unix production trigger in `mc-server`.

The implementation deliberately does **not** add a filesystem watcher, polling cadence, client-bundle hot swap, worldgen hot swap, or a new mutable plugin/world authority.

## Why reload stays inside the existing host boundary

`ScriptEventSink` / `ScriptBoundary` is cloned into connection, simulation, storage, zone, inventory, teleport, villager and other adapters. Replacing that boundary from the composition root would require a new mutable indirection across transport/runtime authority.

Instead, `mc-script` keeps the public boundary unchanged and changes only the private host-channel payload to an internal `ScriptHostInput` envelope. Public server-facing APIs still accept immutable `ScriptEvent` values and return the same host-attested `ScriptCommand` values. Reload control frames never leave `mc-script`.

`LuaHost` stores only a weak sender to the host input queue. The reload handle therefore cannot keep the runtime alive after `ScriptBoundary` closes or is dropped.

## Restart-only contract

`LuaHost::reload(PreparedLuaPlugins)` accepts only a candidate compatible with the active startup contract:

- same ordered plugin ids;
- same normalized player-command roots and operator-command roots;
- same worldgen ore contribution;
- same worldgen settlement-plan contract;
- same client bundle owner/id/version/hash/size/loaders/content/permissions.

Source, `config.toml`, subscriptions and command capabilities may change because they are runtime-local host behavior. A restart-only contract change returns `LuaReloadError::StartupContractChanged` before the request enters the host queue.

This prevents reload from silently changing world generation, Solaris Loader/client content, or command-root topology that other server components may have snapshotted at startup.

## Candidate readiness and atomic commit

After a compatible request is admitted to the private host FIFO:

1. every candidate plugin is fully constructed as a new isolated Luau VM on the dedicated host thread;
2. all candidate plugins subscribed to `server.started` execute that handler under the normal instruction, memory, per-plugin wall-clock and aggregate host-event budgets;
3. commands emitted by candidate `server.started` handlers remain staged and produce no server-owned side effect yet;
4. the host validates the entire staged command set against the candidate capabilities and reserves capacity for the complete command batch set;
5. host-attested command provenance is allocated while the commands remain unpublished;
6. the complete player-command ownership map is validated and replaced under one write lock;
7. only then is the plugin generation swapped and the already-reserved startup commands published.

Any candidate compile failure, `server.started` trap/budget failure, command-queue pressure, command admission failure, or command-ownership failure before step 7 leaves the current plugin generation authoritative. Issued-but-unpublished host admission tickets are cancelled by normal `HostAttached` drop semantics if commit aborts.

A closed command consumer is a non-normal host condition: the reload caller receives `HostClosed` and the host exits with `command_queue_closed` rather than pretending the old generation is still serviceable.

## Reload state and cancellation

On successful replacement:

- previous-generation runtime-disable diagnostics move exactly once into `LuaReloadReport`;
- the new generation starts with a fresh fault set;
- runtime-local state such as plugin timers is intentionally reset;
- a previously faulted plugin can regain the **same declared** player-command root through the atomic ownership replacement;
- host commands already emitted by the previous generation remain valid committed output and their admission tickets are still accepted after the swap.

Successful enqueue of the reload control frame is the commit-intent boundary. Cancelling the caller after that admission does not create a scheduler-dependent maybe-cancel/maybe-commit race: the host finishes the already-admitted replacement attempt and simply drops the result if nobody is waiting.

## Ordering

Ordinary queued events and reload controls share one host input FIFO. An ordinary event processed before the reload uses the old generation; a later ordinary event uses the new generation.

Coalesced `server.tick` retains its pre-existing latest-value semantics. It is not relabeled as a strict FIFO record merely to simplify the reload story; ADR 0009 records this explicitly.

## Production SIGHUP trigger

On Unix, `mc-server` installs `SIGHUP` as the explicit plugin reload trigger.

When received:

1. file preparation runs on `spawn_blocking`, so the main `BoundServer::serve_and_save()` future continues to be polled while TOML/plugin files are read and validated;
2. the current config file is re-read;
3. the server requires that the running process originally started with `plugins.strict = true` and that the current config still has strict mode;
4. the existing `prepare_configured_luau_plugins()` path reruns external/bundled discovery, dependency ordering, package validation and `plugins.expected` exact-set validation;
5. the resulting `PreparedLuaPlugins` is passed to `LuaHost::reload`;
6. successful replacement diagnostics and loaded count are logged; failure is logged without claiming success.

SIGHUP applies only the validated plugin snapshot. Other server configuration remains the startup snapshot. With no Luau host or with a server started in permissive plugin mode, the signal is handled but the plugin reload is explicitly rejected. Non-Unix builds have no SIGHUP reload trigger.

Ctrl-C/server shutdown remains authoritative while preparation/reload is in progress; the network future continues polling and the existing drain/final-save lifecycle is unchanged.

## Executable evidence

The `mc-script` feature suite covers the reload boundary directly:

- `reload_swaps_generation_at_fifo_boundary` — old queued event, reload barrier, then new event; old and new outputs both pass the real `accept_host_command` provenance gate.
- `reload_reinitializes_candidate_before_publishing_new_generation` — replacement `server.started` emits staged output; it is visible only after successful swap, followed by new-generation tick behavior.
- `reload_reinitialization_failure_keeps_previous_generation` — candidate `server.started` trap rejects reload and the old generation handles the next event.
- `reload_startup_queue_pressure_keeps_previous_generation` — a full command queue rejects staged startup publication with `CommandQueueFull`; after draining, the old generation is still active.
- `failed_reload_keeps_previous_generation_authoritative` — syntax-invalid candidate returns `CandidatePlugin`; old generation remains active.
- `incompatible_reload_contract_is_rejected_before_swap` — changed command root returns `StartupContractChanged` before host admission.
- `reload_recovers_faulted_plugin_and_restores_command_ownership` — a faulted old plugin loses its root; repaired compatible reload returns the old diagnostic once, restores the same root and starts clean.
- `admitted_reload_commits_even_when_response_waiter_is_cancelled` — caller cancellation after host admission does not cancel the replacement attempt.
- `closed_boundary_rejects_reload_without_keeping_host_alive` — weak reload ownership does not prevent normal host shutdown.

The `mc-server` binary tests add production-path evidence:

- `strict_config_file_reload_reprepares_and_swaps_luau_generation` — writes a strict config with exact expected set, starts the real host, edits disk `main.lua`, rereads the config through the production preparation helper, commits reload, accepts the staged `server.started` host command, then proves the next tick runs the new generation.
- `luau_reload_requires_strict_startup` — a process that started permissive cannot enable production hot reload later.

Current focused results on this tree:

```text
cargo test -p mc-script --features lua-runtime --quiet
195 passed; 0 failed; 0 ignored

cargo test -p mc-script --quiet
87 passed; 0 failed; 0 ignored

cargo test -p mc-net --lib --quiet
1961 passed; 0 failed; 5 ignored

cargo test -p mc-server --quiet
all runnable test binaries passed; main binary set: 60 passed; 3 ignored
```

`cargo fmt --all -- --check` and `cargo run -p xtask -- code-health` are also clean at this checkpoint (`0 fail / KEEP`).

Benchmark: not applicable. Reload is an explicit operator control path; steady-state script/event and simulation hot loops keep the same bounded queues and authority path.

## Review status and disposition

An earlier read-only review attempt of the **host-only** reload slice was terminated by the local command timeout before producing a verdict; the ephemeral Codex session could not be resumed. It remains recorded as **BLOCKED / no verdict**, not retroactively promoted to a pass.

The later combined host + atomic `server.started` reinitialization + Unix SIGHUP production-trigger checkpoint received exactly one fresh bounded independent read-only review. The reviewer was restricted to the current reload/lifecycle code and evidence ranges, did not run tests or edit files, and returned terminal verdict:

```text
PASS
```

No findings required follow-up. The primary self-check on the reviewed tree was the 195/195 Luau feature suite, 87/87 default `mc-script`, 1961/5 `mc-net` lib result, all runnable `mc-server` tests, strict workspace Clippy, formatter, code-health `0 fail / KEEP`, and scoped diff-check.

Host + production safe-reload boundary: **PASS**.

Phase 5 item 2: **CLOSED**.
