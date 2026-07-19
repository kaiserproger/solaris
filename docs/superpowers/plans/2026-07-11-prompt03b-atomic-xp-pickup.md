# Prompt 03B Atomic XP Pickup Plan

Quality label: `stabilization`.

## Scope

Move experience-orb removal plus player XP credit into one simulation-owner
transaction. The connection mirrors the returned `XpState` for packet output;
requester loss after owner application must preserve both the credited XP and
the removed orb.

This slice does not migrate arrow pickup, inventory actions beyond item pickup,
health/hunger, combat/death, pose, cursor/container state, or save barriers.

## Tasks

- [x] Add RED requester-loss, stale-session, and exact concurrent-credit tests.
- [x] Add a session-fenced owner command that removes one XP orb and credits the
  registered player snapshot in the same critical section.
- [x] Dispatch entity visibility from the owner and return an immutable XP
  snapshot to the connection.
- [x] Keep claim-only XP helpers available only to legacy equivalence tests.
- [x] Run focused simulation and concurrent connection tests, wire suite,
  checked replay/soak, MCP client gate, and workspace baseline.

## Evidence

- RED/GREEN owner tests prove requester-loss durability, stale-session no-op,
  and exactly one five-point winner across two concurrent sessions.
- The existing two-connection item/XP contention test and concurrent lethal
  attack reward test pass through the new owner credit path.
- The survival passive-mob TCP test now requires both the configured item drop
  and a positive `ClientboundSetExperience` packet after the kill.
- Prompt 02 checked replay and the short four-active-plus-one-paused-reader soak
  pass after the cutover.
- The embedded MCP exposes 17 tools and passed
  `playable-01-join-generated-spawn`; the debug server saved and exited with
  status 0.
- `cargo test --workspace`, workspace clippy with `-D warnings`, format check,
  and `xtask code-health` pass; code-health reports `0 fail` / `KEEP`.
