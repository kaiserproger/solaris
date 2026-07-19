# Prompt 03 Packet Block-Entity Command Plan

Quality label: `stabilization`.

## Scope

Route packet-authored sign text and campfire insertion commits through ADR
0004's bounded simulation owner. Preserve block-state plus mutation-token CAS,
opaque NBT bytes, campfire cooking state, inventory debit ordering, viewer
updates, flush, and restart behavior.

This slice does not migrate the server-origin campfire cooking tick, furnace
tick, hopper transfers, scheduled world updates, entity-drop creation, or save
IO. Those paths remain explicit input to Prompt 06; they are not evidence for
whole-world single-writer ownership.

## Tasks

- [x] Add a typed opaque block-entity CAS command and runtime telemetry.
- [x] Route sign updates through the owner and fail closed with block resync.
- [x] Add direct-vs-owner and stale ABA regressions for opaque NBT.
- [x] Add an atomic campfire world-NBT/cooking-state command.
- [x] Debit held inventory only after the campfire command commits.
- [x] Add direct-vs-owner and duplicate-snapshot campfire regressions.
- [x] Pass focused sign update/reopen and campfire cook/flush/reopen TCP gates.
- [x] Pass the complete `block_edit` target, checked replay, and short 4+1 soak.
- [ ] Run P4/P42 after the remaining Prompt 03 source audit closes.
- [x] Run the full Cargo/format/code-health checkpoint after documentation.

## Residual Authority Audit

The following production mutations intentionally remain outside this slice:

| Aggregate | Direct producer | Next owner |
|---|---|---|
| Furnace block entity | active-container server tick | Prompt 06 world owner |
| Chest/furnace/campfire block entities | scheduled hopper transfer | Prompt 06 world owner |
| Campfire cooking state/NBT/drop | entity-ticker campfire phase | Prompt 06 world/entity owners |
| Block/fluid/random tick state | scheduled simulation phases | Prompt 06 world owner |
| Falling-block landing | entity/world coupling phase | Prompts 05-06 |
| Generic item/XP creation | player, loot, and server tick paths | Prompt 05 entity ECS owner |
| Player inventory/cursor/XP | connection task | later Prompt 03/05 transaction slice |
| Chunk/entity persistence | save and flush workers | Prompt 06 save epochs/barriers |

These adapters remain compatible with the staged ADR, but prevent claims that
the full runtime, world, campfire aggregate, or persistence path is already
single-writer.
