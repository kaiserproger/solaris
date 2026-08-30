# Alpha-3 owner terrain review package (post-coherence-model)

Date: 2026-08-30
Audience: owner verdict only — no automated gate substitutes this decision.

## What this is

The owner rejected the pre-model seed-`712816` terrain for alpha-3
("repeated/banded endless savanna, no large-scale biome cohesion" —
`phase4-seed-712816-owner-disposition-package-2026-08-28.md`). The worldgen
line was then replaced with a measured multi-scale biome-domain model
(broad internally-coherent regions, climate-driven transitions,
rivers/coasts, local variation) with multi-seed striping/fragmentation
metrics. The plan's world-generation items 1–4 are closed; the only open
action is this explicit owner review.

## Current artifacts to review

1. `docs/evidence/worldgen-mosaics/seed-712816/biome.png` — biome mosaic,
   2048×2048 blocks at 8 blocks/pixel.
2. `docs/evidence/worldgen-mosaics/seed-712816/height.png` — height mosaic.
3. `docs/evidence/worldgen-mosaics/seed-712816/vegetation.png` — vegetation
   density mosaic.
4. `docs/evidence/worldgen-mosaics.md` — palette/derivation contract and the
   one-command reproduction.

Freshness proof: all three images were regenerated on the current committed
tree (`5b8b54c7`) on 2026-08-30 and are byte-identical (SHA-256) to the
checked-in artifacts — the review material matches the shipping generator.

Optional in-game pass: start the server with `playable.toml`, seed `712816`,
and traverse; the automated traversal/restart/survival/throughput evidence
for this generator remains green and must not be reopened unless it
regresses.

## What to evaluate (from the owner's original rejection)

- Large-scale biome cohesion: contiguous, credible biome regions instead of
  repeated banding.
- Transition width: climate-driven gradients rather than hard stripes.
- Regional identity: distinguishable broad areas (ocean/land, mountain
  systems, forest/savanna/desert belts).
- Local variation inside coherent regions without fragmentation.

## Verdict handling

Record the verdict verbatim in `docs/PUBLIC_ALPHA3_PLAN.md` (world
generation section). ACCEPT closes the section; REJECT names the concrete
field defect and becomes the next worldgen checkpoint's contract. Tagging
`v0.0.3-alpha.1` waits for this verdict.
