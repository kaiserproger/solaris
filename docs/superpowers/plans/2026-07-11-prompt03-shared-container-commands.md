# Prompt 03 Shared Container Command Plan

Quality label: `stabilization`.

## Scope

Move network chest and furnace click commits through ADR 0004's bounded owner.
Keep player inventory/cursor ownership, furnace ticking, hopper/server-origin
writes, other block entities, and persistence explicitly legacy.

## Tasks

- [x] Add typed expected/new chest and furnace commit commands and outcomes.
- [x] Couple viewer state-id validation/increment with the world write under the
  established `world -> session` lock order.
- [x] Return authoritative state id and block-entity snapshot on stale input;
  roll back connection-local inventory/cursor and resync.
- [x] Fail closed on full/closed/busy/unavailable/storage failures and expose
  container-command plus world-failure telemetry.
- [x] Pair initial world snapshots with viewer state ids under one lock order.
- [x] Add chest/furnace legacy-vs-owner, duplicate snapshot, busy-world, and
  paired-read race tests.
- [x] Pass 467 `mc-net` tests, the 66-test `block_edit` target, explicit
  two-client chest contention, stale chest/furnace, and normal furnace-smelt.
- [x] Rerun checked transaction replay and the short 4+1 soak.
- [x] Run the full Cargo/format/code-health checkpoint after documentation.
- [ ] Run P4/P42 only after the remaining Prompt 03 authority slices close.
