# ADR 0008 - Overworld generation pipeline

**Date:** 2026-07-22
**Status:** Accepted, worldgen revision 18

## Context

Earlier routers changed terrain formulas without fencing persisted
chunks by generator revision, seed, and mode. A server could therefore join old
and new terrain inside one Anvil world. That creates hard borders which no local
height or decoration fix can remove. Tree placement also accepted any non-fluid
block as support instead of the surface planned for that column.

## Decision

Worldgen revision 18 selects ocean temperature variants and snowy beaches from
the same temperature and transition-domain field as inland and riparian biomes.
Deep oceans retain their depth thresholds and have no invented deep-warm variant;
their warmest available vanilla variant is deep lukewarm ocean. The separate
deep-ocean region-noise picker is removed. Both modes share coastal selection.
Ocean thresholds are Solaris policy, not vanilla-captured values: -0.25 reuses
the inland/riparian cold boundary and 0.12 reuses the inland warm boundary.
The added -0.12 and 0.25 thresholds mirror those magnitudes to separate cold
from neutral water and warm from lukewarm water without another climate field.
All four use the existing 0.08 domain-adjusted transition margin. If a named
variant is absent from supplied biome rules, selection retains the first entry
of that same ocean/shore bucket; the generic default applies only to an empty
bucket. These choices do not establish vanilla ocean-temperature parity.

Within the existing two-block shoreline band, the already computed erosion field
selects exposed rocky shores below 0.35; cold shores retain snowy-beach routing.
The mountain ridge field cannot select coastal rock because its continental
mask is zero there. Stony shores now use gravel over stone instead of being
captured by the general sandy-beach material branch. Raised inland banks remain
outside this band. No height, drainage, wetland, climate-noise or cache algorithm
changes; existing terrain fields are reused without extra noise evaluation.
The persisted revision fence requires fresh generated worlds and leaves the
frozen field-test archive unchanged. This is climate coherence, not full vanilla
ocean feature or frozen-water parity.

A follow-up coastal replay found a streaming metadata mismatch, not a reason
to alter revision 18: Login advertised radius 8 while the owner views loaded
radius 6. Chunk streaming now publishes the effective client-cache radius
initially and on changes, keeping distance fog aligned with the delivered view.
The matched owner footprint and nearby occupancy/fluid samples are unchanged;
one live dirt/grass material difference is recorded separately. All nine
retained ground checks and three varied-seed graphical routes pass. The older
large rectangular patch did not reproduce in the fresh pre-fix replay, so its
original transient cause is not claimed. This is scoped rendering evidence,
not global terrain or frozen-water parity. Exact reproduction, wire authority,
images and gates:
`.analysis/codex-logs/owner-field-5617830-2026-09-06/coastal-boundaries/receipt.json`.

Worldgen revision 17 replaces the broad humid-lowland swamp classification
with a shared riparian wetland field. It uses the existing drainage strength
and variable-width shoulders, coast connection, low relief and warm/moist
climate. Weak reaches whose full-strength bed would remain above the local
water table do not create wetlands. Existing broad detail and hill fields
form shallow pockets and dry hummocks; there is no new noise field,
fixed-width ring, origin override or second surface-material authority.
Wetland biomes occupy connected shoulders between two blocks below and one
block above sea level; dry inland lowlands retain their climate biome.

Both modes share riparian climate selection while retaining their existing
river-width thresholds: cold rivers select frozen river, warm wetlands select
mangrove swamp, and temperate wetlands select swamp. Mangroves now have mud,
native logs and leaves, and variable root arms attached to actual neighbouring
soil. Submerged roots retain their waterlogged state. Temperate swamps retain
oak trees, including shallow-water sites, and use blue orchids and grass.
The existing leaf-support traversal resolves mangrove leaf distances too;
natural foliage does not receive a permanent-leaf override.

`terrain::trees` owns ordinary and wetland tree placement together rather than
adding another tree mechanism to chunk orchestration. These are custom
deterministic habitat shapes, not a claim of exact vanilla feature parity.
Changed terrain, materials and vegetation retain the persisted revision fence;
revision-16 generated worlds must not silently mix with revision 17. The frozen
field-test archive remains unchanged.

Worldgen revision 16 removes raised beach strips by using the existing
two-block shoreline height in both modes. The Tellus-only six-block band is
gone; climate fields, river topology and terrain heights do not change.
Altitude and climate routing now lives in `terrain::biome_routing`, alongside
its thresholds. Chunk assembly and structure placement consume the same shared
shoreline bound rather than retaining a second height policy. Surface materials
still come from the selected biome, and changed surfaces retain the persisted
worldgen-revision fence.

Worldgen revision 15 replaces the two straight halves of each river reach with
an eight-segment approximation of a smooth parabolic bend. The same jittered
endpoints, downstream topology and runoff remain; continental, mountain and
climate scales do not change. The curve passes through both shared endpoints
and reaches its existing lateral displacement smoothly at mid-reach.
Seeded base widths remain, but taller banks widen according to the masked
carving relief and the quintic falloff's maximum derivative instead of cutting
steeper cliffs. A first curved-reach attempt failed the existing three-block
terrain-step check; relief-dependent width fixes that failure without relaxing
the check. No second generator, persistent cache or compatibility switch is added.

River sampling covers the complete curve and width bounds, including origins
two cells away at small supported world scales and farther when bank relief
requires it. Cheap convex-hull and per-reach bounds reject irrelevant candidates
before detailed distance evaluation. A wide unpruned reference guards
cell-boundary contributions; the previous 3x3
search clipped valid river weights at scale 0.25. Changed terrain remains behind
the existing persisted revision mismatch fence and requires fresh generated worlds.

Both biome routes now restrict beaches to the continental coast band instead
of assigning every low inland river bank a beach biome. Inland banks retain
their local climate, ground and vegetation. Surface materials still come from
the selected biome; no per-column override or second surface authority is added.

Worldgen revision 14 adds a deterministic jungle undergrowth layer: one-block
jungle-log bushes with oak leaves are mixed with ordinary jungle trunks of
4–12 blocks. Jungle candidates use roughly 50 columns per chunk before the
existing moisture, density, exact-surface, stable-5x5 and chunk-margin fences.
This is not full vanilla jungle feature parity; mega trees and vines are not
part of this change.

Generated natural leaves start at distance 7 with `persistent=false`. Before
publishing a decorated chunk, a bounded six-neighbour traversal resolves their
nearest in-chunk log distances through distance 6. State palettes and support
classification are cached per generator; the existing `mc-world` plant rules
own the log/leaf support classification used by both generation and scheduled
runtime updates. Later block edits retain ordinary leaf-distance propagation
and decay. There is no permanent-leaf override or separate CPU admission path.

Generated revisions remain behind the persisted world-contract mismatch fence. The
old packaged alpha archive is unchanged. After correcting chunk-queue CPU
backpressure, real clients load the full 9x9 destination window within 30 seconds
on seeds `5617830` and `-17711`; inspected screenshots show trees and undergrowth,
including the owner's reported site. After removing whole-resident byte-accounting
rescans, both full graphical harnesses pass without chunk-publication lock-wait
warnings. Slow scheduled-block ticks remain a separate unresolved performance
finding; this is not complete terrain, population or performance acceptance.

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
the canonical `realistic_deposits` profile; that disables the vanilla pass and
uses large deterministic cross-chunk deposits. Conflicting declarations fail
startup, and Lua receives no generator state or locks. Earlier planning called
this profile `geological_deposits`; the public manifest and persisted contract
now use only the canonical `realistic_deposits` spelling.

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

The hot path reuses each planned surface column's biome for vertical biome cells
and biome-restricted ore anchors. Ore admission reads the existing chunk-local
`OreColumnCache`; it does not rerun height and climate routing for each Y anchor.
The cache halo covers the same neighbouring anchors as vein placement, so
chunk-order independence and the ore biome/height fences are unchanged.
Cave noise exits after its region mask or first tunnel field rejects the cell.
The 2026-09-05 debug comparison preserves serialized fingerprints across 48
chunks, three seeds, and both modes; it is not a release throughput claim and
does not change the generator revision.

## Staged boundary

Landforms, caves and biome routing are isolated sibling stages. Surface
composition, ores, features and structures still reside in the larger `terrain.rs`
assembly and should move into focused siblings when each stage is changed.
This ADR does not claim vanilla NoiseRouter parity or complete Tectonic/Tellus feature
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
- canonical `realistic_deposits` deposits crossing chunk boundaries while default generation stays vanilla;
- order-independent ore placement;
- agent-run 26.1.2 MCP inspection with seed `918273645` and `tellus_like` mode
  over forest, coast, ocean, and high-relief terrain;
- agent-run 26.1.2 MCP inspection of the exact shipped `playable.toml` seed-0
  `tellus_like` forest spawn;
- agent-run revision-9 inspection of an isolated raised tree crown and the
  seed-918273645 long snow slope at `(-78080,215,-28928)`;
- `cargo test -p mc-worldgen`.
