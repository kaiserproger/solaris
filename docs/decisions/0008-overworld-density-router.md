# ADR 0008 - Overworld generation pipeline

**Date:** 2026-07-22
**Status:** Accepted, worldgen revision 5

## Context

Three previous routers changed terrain formulas without fencing persisted
chunks by generator revision, seed, and mode. A server could therefore join old
and new terrain inside one Anvil world. That creates hard borders which no local
height or decoration fix can remove. Tree placement also accepted any non-fluid
block as support instead of the surface planned for that column.

## Decision

Worldgen revision 5 replaces the previous mixed height formula with a layered
`terrain::overworld::OverworldRouter`. Continents first choose ocean or land;
erosion then chooses plains or uplands; ridges add mountains only on established
land; rivers carve only low relief. Every layer has a bounded vertical effect,
so no late mask can turn an ordinary surface column into a deep pit. Broad
coordinate scales and a tested three-block adjacent-column slope budget keep
interior columns and chunk borders continuous.

Spawn land and river suppression are smooth field constraints, not later block
rewrites. River availability is part of the returned field, so biome routing
cannot label an uncarved coast as a river.

Underground shape is a vertically bounded intersection of two narrow 3D tunnel
fields. A cell is carved only when a horizontal neighbour belongs to the same
tunnel. Carvers retain a 32-block solid surface shell and tests bound shafts,
isolated cells, total cave density, and open cells in 9x9 slices.

Chunk assembly, ore rules, structures, and decorations are deterministic
consumers. Structures are emitted before vegetation. A generated tree now
requires the exact planned surface block under its trunk plus a stable 5x5
terrain footprint; structures or earlier stages cannot become accidental tree
support.

Generation remains stateless and coordinate-derived. Parallel generation of the
same chunk or neighbouring chunks in any order produces identical output. Every
new Solaris world persists `solaris/world.json` with schema, worldgen revision,
seed, mode, and geometry. A mismatched contract is rejected before Anvil open.
An existing unversioned Anvil world is treated as a vanilla import and opens
without Solaris fallback generation, so missing chunks cannot mix both terrain
authorities. Existing worlds are never rewritten. The local playable profile
uses `.analysis/test-world-v5`.

The hot path samples each surface column once and reuses its biome result for
vertical biome cells. Cave noise exits after its region mask or first tunnel
field rejects the cell. No revision-5 performance claim exists until a release
benchmark runs on a clean host.

## Staged boundary

Overworld routing is isolated. Surface composition, carvers, ores, features, and
structures still reside in the larger `terrain.rs` assembly and should move into
focused sibling modules when each stage is changed. This ADR does not claim
vanilla NoiseRouter parity or complete Tectonic/Tellus feature coverage.

## Verification

- deterministic generation for repeated calls and explicit geometry;
- bounded adjacent-column and chunk-border steps plus non-grid biome transitions;
- dry walkable land throughout a 193x193 spawn window across sampled seeds;
- broad water-filled river sections;
- sparse locally coherent tunnel caves with no chamber field, surface mouth, or
  long vertical shaft;
- a 32-block solid protected surface shell across sampled seeds;
- exact-surface tree support over a stable 5x5 footprint;
- vanilla-import isolation plus rejection of mismatched revision/seed/mode/geometry;
- order-independent ore placement;
- `cargo test -p mc-worldgen`.
