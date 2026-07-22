# ADR 0008 - Overworld density router

**Date:** 2026-07-22
**Status:** Accepted, second-generation router

## Context

The old generator calculated terrain shape through separate formulas for
height, continents, rivers, mountains, climate, and caves. Those formulas could
disagree. The first unified router removed those duplicate authorities but
still produced over-wide mountain ridges, vertically stretched caves, and tree
anchors that looked unsupported on sharp terrain.

## Decision

`terrain::overworld::DensityRouter` is the single authority for terrain shape.
The first router was replaced instead of extended. One coordinate sample derives
warped continents, coast detail, erosion, mountain provinces, ridges, hills,
rivers, temperature, and moisture from one field family. Mountains only rise
inside a broad province mask. Rivers flatten land toward a water floor; they do
not subtract vertical shafts from terrain density.

Spawn land and river suppression are smooth field constraints, not later block
rewrites. River availability is part of the returned field, so biome routing
cannot label an uncarved coast as a river.

Underground shape is sampled per block from two intersecting, vertically bounded
3D tunnel fields plus rare deep chambers. Carvers never operate in the top 32
solid blocks of a column and no longer rewrite two-block vertical pairs. This
keeps caves locally coherent without surface mouths or long vertical voids.

Chunk assembly, ore rules, structures, and decorations remain deterministic
consumers. They may not calculate an alternative terrain surface. Structures
are emitted before vegetation. Generated trees require solid support and every
neighboring terrain column in a 3x3 footprint to be within one block of their
base, preventing visually floating trees on narrow ridges.

Generation remains stateless and coordinate-derived. Parallel generation of the
same chunk or neighboring chunks in any order must produce identical output.
Changing this algorithm intentionally changes newly generated terrain; persisted
vanilla-format chunks remain authoritative and are not regenerated. The local
playable profile therefore uses `.analysis/test-world-v2`; the previous local
world is retained but cannot be evidence for this router.

The hot path samples each surface column once and reuses its biome result for
vertical biome cells. Cave noise exits after the first rejected tunnel field
and evaluates rare chamber fields only underground. On the same development
build probe this changed a 25-chunk spawn window from 7.3 to 20.6 chunks/s. It
is a narrow diagnostic result, not a release or full-server throughput claim.

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
- sparse locally coherent caves with no surface mouth or long vertical shaft;
- a 32-block solid protected surface shell across sampled seeds;
- supported generated trees on stable terrain;
- order-independent ore placement;
- `cargo test -p mc-worldgen`.
