# ADR 0008 - Overworld density router

**Date:** 2026-07-22
**Status:** Accepted, third-generation router

## Context

The old generator calculated terrain shape through separate formulas for
height, continents, rivers, mountains, climate, and caves. Those formulas could
disagree. Two later routers retained too much domain warping, broad ridge masks,
and a second Tellus-only mountain authority. The result was hard to reason about
and could produce visually abrupt terrain and decorations over unsupported
ground.

## Decision

`terrain::overworld::DensityRouter` is the single authority for terrain shape.
The third router replaces the prior formulas instead of layering more fixes over
them. One coordinate sample derives continents, coasts, tectonic plates, erosion,
mountain ridges, hills, rivers, temperature, and moisture. The same sample drives
both vanilla-like and Tellus-like terrain and biome routing. Mountains only rise
where a plate mask and narrow ridge field overlap. Rivers flatten land toward a
water floor; they never subtract vertical shafts from terrain density.

Spawn land and river suppression are smooth field constraints, not later block
rewrites. River availability is part of the returned field, so biome routing
cannot label an uncarved coast as a river.

Underground shape is sampled per block from two intersecting, vertically bounded
3D tunnel fields. The chamber field was removed. Carvers never operate in the
top 32 solid blocks of a column. This keeps caves locally coherent without
surface mouths, giant chambers, or long vertical voids.

Chunk assembly, ore rules, structures, and decorations remain deterministic
consumers. They may not calculate an alternative terrain surface. Structures
are emitted before vegetation. Generated trees require solid support and every
terrain column under their full 5x5 canopy footprint to be within one block of
the trunk base, preventing visually floating trees on narrow ridges.

Generation remains stateless and coordinate-derived. Parallel generation of the
same chunk or neighboring chunks in any order must produce identical output.
Changing this algorithm intentionally changes newly generated terrain; persisted
vanilla-format chunks remain authoritative and are not regenerated. The local
playable profile therefore uses `.analysis/test-world-v3`; previous local worlds
are retained but cannot be evidence for this router.

The hot path samples each surface column once and reuses its biome result for
vertical biome cells. Cave noise exits after its region mask or first tunnel
field rejects the cell. The ignored 25-chunk debug probe generated 24.1 chunks/s
on the development host. This is a narrow diagnostic, not a release or full
server throughput claim.

## Staged boundary

Density routing is isolated. Surface composition, carvers, ores, features, and
structures still reside in the larger `terrain.rs` assembly and should move into
focused sibling modules when each stage is changed. This ADR does not claim
vanilla NoiseRouter parity or complete Tectonic/Tellus feature coverage.

## Verification

- deterministic generation for repeated calls and explicit geometry;
- bounded adjacent-column steps and non-grid biome transitions;
- dry land at the origin across sampled seeds;
- broad water-filled river sections;
- sparse locally coherent tunnel caves with no chamber field, surface mouth, or
  long vertical shaft;
- a 32-block solid protected surface shell across sampled seeds;
- supported generated trees over their full 5x5 canopy footprint;
- order-independent ore placement;
- `cargo test -p mc-worldgen`.
