# Prompt 03 Visible Block Command Plan

Quality label: `stabilization`.

## Scope

Route every production `apply_visible_block_edit_batch` call through ADR
0004's owner. Preserve stronger mutation-token CAS for survival break/place;
do not claim CAS for planners that currently provide only unconditional edits.

## Tasks

- [x] Route conditional and unconditional visible block batches through the
  typed owner command in production.
- [x] Preserve sequence ordering and add a command-kind telemetry counter.
- [x] Replace the unconditional `expect` with fail-closed empty outcome plus
  authoritative loaded-block resync.
- [x] Add owner-order and authoritative-resync regressions.
- [x] Pass all 469 `mc-net` tests and all 66 `block_edit` TCP tests.
- [x] Observe one block command plus one pickup command in the focused TCP
  break/drop/pickup gate with no queue/world failures.
- [x] Rerun checked transaction replay and the short 4+1 soak.
- [x] Run the full Cargo/format/code-health checkpoint after documentation.
- [ ] Run P4/P42 only after the remaining Prompt 03 authority slices close.
