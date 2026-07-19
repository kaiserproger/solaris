# Prompt 03B Atomic Item Pickup Plan

Quality label: `stabilization`.

## Scope

Move item-entity claim plus player inventory credit into one simulation-owner
transaction. The registered `PlayerPersistedState` inventory is authoritative
for this command; the connection keeps a mirror for untouched legacy actions
and wire encoding. A requester that disappears after owner application must
not lose the credited item.

This slice does not migrate XP, arrows, cursor/container state, crafting,
break/tool damage, placement debit, combat, pose, or all inventory mutations.

## Tasks

- [x] Add RED conservation coverage for requester loss after owner application,
  partial capacity, and full-inventory rejection.
- [x] Replace production claim-only item pickup with a session-fenced owner
  command that validates item identity and computes capacity from owner state.
- [x] Commit entity remainder/removal and player inventory credit atomically,
  returning an immutable inventory snapshot plus semantic visibility events.
- [x] Make the connection mirror the returned snapshot without applying a
  second inventory credit.
- [x] Preserve legacy dual-path tests under `cfg(test)` and existing pickup wire
  behavior.
- [x] Run focused unit/wire/conservation gates, Prompt 02 replay/soak, workspace
  baseline, and the embedded MCP real-client pickup/crafting scenario.

## Evidence

- RED/GREEN owner tests prove exact partial credit, full-inventory no-op, stale
  session rejection, and credit plus visibility dispatch surviving a requester
  that never reads the applied response.
- The production connection mirrors the returned inventory snapshot and sends
  slot updates; it never applies a second pickup merge.
- The 66-test wire suite passes, including survival break/drop/pickup/place.
- Prompt 02 checked replay passed three consecutive post-fix runs. Its short
  four-active-plus-one-paused-reader soak passed with five transaction samples.
- The embedded MCP exposed 17 tools and passed
  `playable-02a-natural-log-to-planks` on a fresh seed-0 debug world: a real
  NeoForge client broke a natural birch log, observed and picked up the drop,
  and crafted four birch planks. Solaris performed its final save and exited
  with status 0.
- `cargo test --workspace`, workspace clippy with `-D warnings`, format check,
  and `xtask code-health` all pass; code-health reports `0 fail` / `KEEP`.
