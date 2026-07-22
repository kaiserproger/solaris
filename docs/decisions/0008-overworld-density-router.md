# ADR 0008 - Overworld generation pipeline

**Date:** 2026-07-22
**Status:** Accepted, worldgen revision 7

## Context

Earlier routers changed terrain formulas without fencing persisted
chunks by generator revision, seed, and mode. A server could therefore join old
and new terrain inside one Anvil world. That creates hard borders which no local
height or decoration fix can remove. Tree placement also accepted any non-fluid
block as support instead of the surface planned for that column.

## Decision

Worldgen revision 7 retains the revision-6 density router and adds an explicit
ore profile to the persisted world contract. The default `vanilla` profile uses
the embedded 26.1.2 ore passes. A validated plugin manifest may instead declare
`geological_deposits`; that disables the vanilla pass and uses large
deterministic cross-chunk deposits. Conflicting declarations fail startup, and
Lua receives no generator state or locks.

The revision-6 router replaced the revision-5 router instead of tuning it.
`terrain::overworld::landforms` owns a new coordinate field: domain-warped
continents establish shelves and land, erosion and uplands shape broad relief,
and two differently oriented ridge fields form long branching mountain ranges.
River valleys use warped zero contours, are suppressed in mountains and the safe
spawn plateau, and become river biomes only after their valley is substantially
carved. Broad coordinate scales and a tested three-block adjacent-column slope
budget keep interior columns and chunk borders continuous. A separate
sampled four-block neighbourhood invariant detects isolated terrain craters.

Spawn land and river suppression are smooth field constraints, not later block
rewrites. River availability is part of the returned field, so biome routing
cannot label an uncarved coast as a river.

`terrain::overworld::caves` independently owns underground shape as the
vertically bounded intersection of two anisotropic 3D tunnel fields. Carvers
retain a 32-block solid surface shell; carving requires a horizontal tunnel
neighbour, and tests bound shafts, isolated cells, total cave density, and open
cells in 9x9 slices.

Chunk assembly, ore rules, structures, and decorations are deterministic
consumers. Structures are emitted before vegetation. A generated tree now
requires the exact planned surface block under its trunk plus a stable 5x5
terrain footprint; structures or earlier stages cannot become accidental tree
support.

Generation remains stateless and coordinate-derived. Parallel generation of the
same chunk or neighbouring chunks in any order produces identical output. Every
new Solaris world persists `solaris/world.json` with schema, worldgen revision,
seed, mode, ore profile, and geometry. A mismatched contract is rejected before
Anvil open.
An existing unversioned Anvil world is treated as a vanilla import and opens
without Solaris fallback generation, so missing chunks cannot mix both terrain
authorities. Existing worlds are never rewritten. The local playable profile
uses `.analysis/test-world-v7`.

Anvil root metadata belongs to the chunk serialization boundary, not a concrete
terrain generator. The encoder emits one `DataVersion`, `LastUpdate`, and
`InhabitedTime` field for every saved chunk. It preserves imported data and
inhabited values, supplies the pinned 26.1.2 data version when absent, and uses
the explicit simulation tick for `LastUpdate`. Runtime accumulation of
`InhabitedTime` is a separate ownership task; generated chunks currently begin
at zero.

The hot path samples each surface column once and reuses its biome result for
vertical biome cells. Cave noise exits after its region mask or first tunnel
field rejects the cell. No revision-7 performance claim exists until a release
benchmark runs on a clean host.

## Staged boundary

Landforms and caves are isolated sibling stages. Surface composition, ores,
features, and structures still reside in the larger `terrain.rs` assembly and
should move into focused sibling modules when each stage is changed. This ADR
does not claim vanilla NoiseRouter parity or complete Tectonic/Tellus feature
coverage. Real-client visual inspection remains required.

## Verification

- deterministic generation for repeated calls and explicit geometry;
- bounded adjacent-column and chunk-border steps plus non-grid biome transitions;
- no isolated four-block-scale terrain craters across sampled seeds;
- dry walkable land throughout a 193x193 spawn window across sampled seeds;
- broad water-filled river sections;
- sparse locally coherent tunnel caves with no chamber field, surface mouth, or
  long vertical shaft;
- a 32-block solid protected surface shell across sampled seeds;
- exact-surface tree support over a stable 5x5 footprint;
- vanilla-import isolation plus rejection of mismatched revision/seed/mode/geometry;
- canonical vanilla root metadata through an actual Anvil write/read at a
  nonzero simulation tick;
- rejection of a changed persisted ore profile;
- order-independent ore placement;
- geological deposits crossing chunk boundaries while default generation stays vanilla;
- `cargo test -p mc-worldgen`.
