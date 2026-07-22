# ADR 0008 - Overworld density router

**Date:** 2026-07-22
**Status:** Accepted, staged extraction

## Context

The old generator calculated terrain shape through separate formulas for
height, continents, rivers, mountains, climate, and caves. Those formulas could
disagree: a column could be classified as river without being carved below the
water line, spawn protection did not affect biome selection, and hand-authored
surface cave mouths produced shafts and unsupported decorations.

## Decision

`terrain::overworld::DensityRouter` is the single authority for terrain shape.
One coordinate sample returns surface height, continentalness, ridge strength,
river distance, temperature, and moisture. The same router owns cave density.
Underground shape uses true three-dimensional domain-warped value-noise fields
for intersecting tunnels and chambers. A higher-frequency vertical perturbation
adds local variation without a post-generation repair pass.

The router uses continuous domain-warped fields for broad continents, erosion,
mountain chains, local detail, and river valleys. Spawn land is a smooth field
constraint, not a later block rewrite. River suppression near spawn is reflected
in the sampled river field, so terrain and biome routing agree.

Chunk assembly, ore rules, structures, and decorations remain deterministic
consumers. They may not calculate an alternative terrain surface. Occupied
structure blocks are emitted before vegetation is placed, and decorations place
only into air, so a template cannot replace the bottom of an existing trunk or
be overwritten by a plant. Generated trees require solid support and caves
retain a 24-block surface-clearance fence.

Generation remains stateless and coordinate-derived. Parallel generation of the
same chunk or neighboring chunks in any order must produce identical output.
Changing this algorithm intentionally changes newly generated terrain; persisted
vanilla-format chunks remain authoritative and are not regenerated.

## Staged boundary

Density routing is isolated now. Surface composition, carvers, ores, features,
and structures still reside in the larger `terrain.rs` assembly and should move
into focused sibling modules as each stage is changed. This ADR does not claim
vanilla worldgen parity or Tectonic/Tellus feature completeness.

## Verification

- deterministic generation for repeated calls and explicit geometry;
- bounded adjacent-column steps and non-grid biome transitions;
- dry land at the origin across sampled seeds;
- broad water-filled river sections;
- sparse underground caves with no surface-mouth rewrite;
- no fully open sampled vertical columns, few isolated sampled cave cells, and
  a solid protected surface shell across sampled seeds;
- supported generated trees and order-independent ore placement;
- `cargo test -p mc-worldgen`.
