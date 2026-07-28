# ADR 0008 - Overworld generation pipeline

**Date:** 2026-07-22
**Status:** Accepted, worldgen revision 10

## Context

Earlier routers changed terrain formulas without fencing persisted
chunks by generator revision, seed, and mode. A server could therefore join old
and new terrain inside one Anvil world. That creates hard borders which no local
height or decoration fix can remove. Tree placement also accepted any non-fluid
block as support instead of the surface planned for that column.

## Decision

Worldgen revision 10 removes every production starter fixture and every
origin-based terrain deformation. Fixed surface stone/iron, the forced tree
anchor, dry-land blending, mountain suppression, and river suppression around
`(0,0)` are gone. A bounded deterministic locator searches the actual seeded
terrain for dry, low-relief inland land. Its block coordinates are persisted in
world-contract schema 3, startup generation/light are centered on its chunk, and
the network's final support/body-space scan is centered on the same published
`WorldSpawn`. The public config default and `example.toml` now select
`tellus_like`; `vanilla_like` remains an explicit compatibility profile.

Surface vegetation consumes one deterministic 192-block regional field blended
with the routed moisture value. Per-biome thresholds turn the existing exact
column hash into sparse candidates inside coherent patches rather than uniform
salt-and-pepper placement. Jungle, forest, taiga/grove, grassland, and savanna
use different admission thresholds and spacing. Savannas resolve acacia blocks;
desert, snowy plains, and ice spikes admit no trees. The field is skipped for
non-vegetated columns so ocean and exposed-rock generation pay no extra noise
cost. Multi-seed tests cover spatial coherence, biome dominance, feature
fingerprints, acacia material/canopy, open-cold gaps, and existing tree support.

Worldgen revision 9 removes the filled 3x3 upper leaf boxes left by revision 8.
Oak and jungle trees retain a broad main canopy but use a connected,
deterministically rotated irregular crown above it. This changes generated
chunks, so the persisted revision advances instead of mixing both silhouettes
inside one Solaris world.

Worldgen revision 8 retains the revision-7 pipeline and corrects terrain that a
real 26.1.2 client exposed as broad flat gravel plateaus. Long rolling relief
has more vertical range, while a rotated 520x210-block detail field shapes the
inside of the existing long mountain masks. Low coastal ridge masks no longer
select mountain surfaces, high peaks use snow, low shelves retain their
coastal/lowland surfaces, and a
smooth explicit floor keeps the spawn region dry across seeds. The persisted
revision fences these changed columns from revision-7 worlds.

Worldgen revision 7 retained the revision-6 density router and added an explicit
ore profile to the persisted world contract. The default `vanilla` profile uses
the embedded 26.1.2 ore passes. A validated plugin manifest may instead declare
`geological_deposits`; that disables the vanilla pass and uses large
deterministic cross-chunk deposits. Conflicting declarations fail startup, and
Lua receives no generator state or locks.

The optional `plains_village_prototype` settlement profile is an independent
startup-only plugin declaration. It loads one fountain, one small house, and
one toolsmith directly from the local vanilla NBT sidecar, combines them with
stable offsets, and consumes the extracted village spacing, separation, and
salt. Seed zero uses a fixed near-spawn center; other seeds keep deterministic
grassland placement. The settlement selection joins the persisted plugin
worldgen profile fence. Lua receives no generator, chunk, lock, or worker
handle. The bounded plan selects building parts and roles, inhabitants, jobs,
and plugin-owned extension records. Vanilla villager jigsaw positions become
persisted chunk markers; runtime installation routes them to the dedicated
system-owned simulation command rather than ambient-herd admission.

The revision-6 router replaced the revision-5 router instead of tuning it.
`terrain::overworld::landforms` owns a new coordinate field: domain-warped
continents establish shelves and land, erosion and uplands shape broad relief,
and two differently oriented ridge fields form long branching mountain ranges.
River valleys use warped zero contours, are suppressed only by mountain relief,
and become river biomes only after their valley is substantially carved. Broad
coordinate scales and a tested three-block adjacent-column slope
budget keep interior columns and chunk borders continuous. A separate
sampled four-block neighbourhood invariant detects isolated terrain craters.

No spawn-specific terrain constraint remains. River availability is part of the
returned field, so biome routing cannot label an uncarved coast as a river; spawn
selection consumes the finished field without changing it.

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
seed, mode, ore profile, settlement profile, geometry, and selected spawn block
coordinates. A mismatched contract is rejected before Anvil open.
An existing unversioned Anvil world is treated as a vanilla import and opens
without Solaris fallback generation, so missing chunks cannot mix both terrain
authorities. Existing worlds are never rewritten. The local playable profile
uses `.analysis/test-world-v10`.

Anvil root metadata belongs to the chunk serialization boundary, not a concrete
terrain generator. The encoder emits one `DataVersion`, `LastUpdate`, and
`InhabitedTime` field for every saved chunk. It preserves imported data and
inhabited values, supplies the pinned 26.1.2 data version when absent, and uses
the explicit simulation tick for `LastUpdate`. The tick owner follows vanilla's
strict 128-block chunk-center range around non-spectator players and counts
every spawning chunk once per game tick. It accumulates those ticks in a small
coordinate map, publishes resident metadata every 20 ticks or when a chunk
leaves the range, and drains a partial interval before shutdown. A resident
miss retains its delta for retry; shutdown loads that chunk without generation
before the final save. This preserves vanilla elapsed-tick semantics without
republishing hundreds of chunks on every tick.

The hot path samples each surface column once and reuses its biome result for
vertical biome cells. Cave noise exits after its region mask or first tunnel
field rejects the cell. No revision-8 performance claim exists until a release
benchmark runs on a clean host.

## Staged boundary

Landforms and caves are isolated sibling stages. Surface composition, ores,
features, and structures still reside in the larger `terrain.rs` assembly and
should move into focused sibling modules when each stage is changed. This ADR
does not claim vanilla NoiseRouter parity or complete Tectonic/Tellus feature
coverage. The bounded client pass verifies representative shapes, not complete
seed coverage or owner-approved visual parity.

## Verification

- deterministic generation for repeated calls and explicit geometry;
- bounded adjacent-column and chunk-border steps plus non-grid biome transitions;
- no isolated four-block-scale terrain craters across sampled seeds;
- representative high-relief windows have visible shape without vertical walls;
- dry walkable land throughout a 193x193 spawn window across sampled seeds;
- broad water-filled river sections;
- sparse locally coherent tunnel caves with no chamber field, surface mouth, or
  long vertical shaft;
- a 32-block solid protected surface shell across sampled seeds;
- exact-surface tree support over a stable 5x5 footprint and an irregular
  raised crown;
- vanilla-import isolation plus rejection of mismatched revision/seed/mode/geometry;
- canonical vanilla root metadata through an actual Anvil write/read at a
  nonzero simulation tick;
- exact active-tick `InhabitedTime` accumulation through an actual Anvil
  flush/reopen, including a chunk active for only part of a batch;
- rejection of a changed persisted ore profile;
- manifest admission, conflicts, sidecar requirement, exact extracted-template
  loading, and deterministic block-for-block village regeneration;
- order-independent ore placement;
- geological deposits crossing chunk boundaries while default generation stays vanilla;
- agent-run 26.1.2 MCP inspection with seed `918273645` and `tellus_like` mode
  over forest, coast, ocean, and high-relief terrain;
- agent-run 26.1.2 MCP inspection of the exact shipped `playable.toml` seed-0
  `tellus_like` forest spawn;
- agent-run revision-9 inspection of an isolated raised tree crown and the
  seed-918273645 long snow slope at `(-78080,215,-28928)`;
- `cargo test -p mc-worldgen`.
