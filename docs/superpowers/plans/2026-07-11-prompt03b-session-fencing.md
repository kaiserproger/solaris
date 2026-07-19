# Prompt 03B Session Fencing Plan

Quality label: `stabilization`.

## Scope

Bind every packet-authored simulation request to the connection's monotonic
`SessionId`. Reject a queued command when that session has been unregistered,
before the command can mutate migrated entity, world, container, or block-entity
state. Keep server-owned herd work unfenced.

This is the first Prompt 03B prerequisite. It does not move inventory, cursor,
XP, health, pose, persistence, or complete player transactions into the owner.

## Tasks

- [x] Add a per-session simulation handle that cannot enqueue unfenced player
  commands from production packet code.
- [x] Reject stale-session envelopes before command application and expose a
  dedicated queue telemetry counter.
- [x] Add disconnect/reconnect tests proving an old session cannot mutate state
  and a newly registered session can execute the same command.
- [x] Preserve detached server-owned simulation commands and existing Prompt 03
  normalized replay outcomes.
- [x] Run focused simulation/server tests, `mc-net` library tests, format,
  code-health, and an MCP-backed real-client gate.

## Evidence

- Packet-authored handles are bound with `SimulationHandle::for_session`; an
  unbound production handle cannot enqueue a player command.
- The owner rejects an unregistered fence as `StaleSession` before dispatch and
  increments `simulation_commands_rejected_stale_session`.
- Unit coverage proves stale item claims and conditional block edits cannot
  mutate state after disconnect, while a newly registered session can proceed.
- Prompt 02 checked replay and short 4+1 transaction soak passed unchanged.
- The embedded Minecraft MCP ran
  `playable-02a-natural-log-to-planks` against the updated debug server: the
  real client joined, broke a generated birch log, picked up its drop, crafted
  four birch planks, and reported `passed` through
  `minecraft_run_scenario`.

The checkpoint does not claim full player ownership or replacement readiness.
