# Prompt 03 Entity Lifecycle Command Plan

Quality label: `stabilization`.

## Scope

Extend ADR 0004's bounded owner without claiming whole-runtime single-writer
status. Migrate grounded-arrow claims, player melee combat, `/summon`, and bow
projectile spawn. Keep player inventory/XP, generic drops, passive herds,
falling blocks, ticker/physics, blocks, containers, world storage, and
persistence explicitly legacy.

## Tasks

- [x] Route grounded-arrow claim/removal through `SimulationHandle` and private
  `SimulationAuthority`.
- [x] Commit melee damage, optional knockback, lethal removal, and planned
  item/XP rewards in one ordered owner command.
- [x] Route command entity spawn and bow projectile spawn through typed owner
  commands.
- [x] Consume bow ammunition and durability only after successful projectile
  creation.
- [x] Keep direct migrated mutation helpers available only under `cfg(test)`.
- [x] Add legacy-vs-owner normalization for claims, combat, command spawn, and
  projectile spawn, including duplicate lethal and full-queue no-damage cases.
- [x] Route passive herd spawn as detached bounded owner work and authority-gate
  lifecycle clock, goals/physics, arrow-hit resolution, and startup restore.
- [x] Run all 459 `mc-net` library tests, the six-test parallel mob gate, focused
  TCP bow/pickup tests, and focused
  clippy with `-D warnings`.
- [x] Rerun Prompt 02 checked replay and short soak after the complete entity
  lifecycle authority slice.
- [x] Run full workspace tests/clippy/format/code-health after that slice.
- [ ] Run P4/P42 after all Prompt 03 authority slices.
