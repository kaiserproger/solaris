# Archived checkpoint log — part 1 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- Checkpoint A scope pinned; implementation NOT started (2026-09-14)
- Vanilla villages: is there any vanilla feature execution in core? (2026-09-14)
- Vanilla villages: phase-1b decompile findings (2026-09-14) — hard boundary hit
- Vanilla villages: phase-1 semantics pinned from the derived data (2026-09-14)
- Vanilla content source: managed cache + packaged importer (stage 1, landed)

---

## Checkpoint A scope pinned; implementation NOT started (2026-09-14)

Owner decision relayed by Main: the `feature_pool_element` boundary is **not**
accepted — villages must ship with no decor gap, so core needs a data-driven
vanilla placed-feature executor first. This section pins checkpoint A's exact
scope from the derived cache; **no implementation code has been written yet**,
and nothing below is approximated in the tree.

Working set = the 13 distinct village decor features, resolved to their
configured features and placement modifiers (quoted from
`worldgen/placed_feature/*.json` + `worldgen/configured_feature/*.json`):

- `minecraft:tree` ×4 — `oak` (straight_trunk_placer + blob_foliage_placer),
  `spruce` (straight + spruce_foliage_placer), `pine` (straight +
  pine_foliage_placer), `acacia` (forking_trunk_placer + acacia_foliage_placer).
  Each also carries a `below_trunk_provider` (`rule_based_state_provider` over
  `not`/`matching_block_tag`/`simple_state_provider`, e.g.
  `minecraft:cannot_replace_below_tree_trunk` → `minecraft:dirt`); decorators are
  empty for these four.
- `minecraft:simple_block` ×3 — `flower_plain` (to_place =
  `noise_threshold_provider`: default dandelion, high states poppy/azure_bluet/
  oxeye_daisy/…), `berry_bush`, `taiga_grass`.
- `minecraft:block_pile` ×5 — `pile_hay` (`rotated_block_provider`), `pile_ice`
  and `pile_pumpkin` (`weighted_state_provider`), `pile_melon` and `pile_snow`
  (`simple_state_provider`).
- `minecraft:block_column` ×1 — `cactus` (the `patch_cactus` configured feature).

Placement modifiers actually referenced: `minecraft:count`,
`minecraft:random_offset`, `minecraft:block_predicate_filter` (8/4/4 uses). The
trees and `patch_*` entries use only these three; no `in_square`, `heightmap`,
`biome`, `rarity_filter` or `noise_based_count` appears in this working set (they
exist in the cache and must fail loudly if a referenced entry ever pulls them in).

Therefore checkpoint A = one dispatch-by-type-id executor over the derived data
covering exactly those 4 configured types, 4 trunk placers, 4 foliage placers,
the state/provider set above, and those 3 placement modifiers — living beside the
existing worldgen modules (no parallel generator, no hardcoded per-biome tree
code), with an unsupported type or modifier failing loudly by name.

Decompile reading list already extracted for the semantics
(`/tmp/vanclasses`, Vineflower): `structure/pools/FeaturePoolElement` (done),
`worldgen/feature/{TreeFeature,BlockPileFeature,BlockColumnFeature,SimpleBlockFeature,RandomPatchFeature}`,
`worldgen/feature/treedecorators/*`, `worldgen/feature/stateproviders/*`,
`worldgen/placementmodifier/*`, `worldgen/heightprovider/*`, plus the tree
placers (`worldgen/feature/treedecorators`, `level/levelgen/feature/trunkplacers`,
`foliageplacers`). None of these bodies have been read yet.

**Binding refinement (owner, before any loader code): resolve by reference,
never enumerate.** The loud-error rule applies only to *transitively reachable*
entries. The loader must start from the village `feature_pool_element` entries
(the 19 reachable entries / 13 distinct features) and follow
placed_feature → its configured_feature → every referenced block-state provider,
heightmap provider and modifier, loading **only that closure**. An unknown or
unsupported type *inside* the closure fails closed, naming the type id and the
referring entry; anything outside the closure is never loaded and can never
error. The same discipline applies to the jigsaw side: walk from the village
structure sets and their pools, and do not enumerate `template_pool/**`,
`processor_list/**`, `configured_feature/**` or `placed_feature/**` as a whole —
the vanilla registries contain many types core does not implement, and failing on
those would make every server start depend on full vanilla feature parity.

**Binding constraint on A1 (owner, from review): land it as an explicitly
PARTIAL internal layer, never as an activated generation path.** The reachable
village closure still contains the four `minecraft:tree` entries, so switching
`settlement_profile = "vanilla"` onto the new executor before A2 would either
fail closed on the first tree at an ordinary startup or produce an incomplete
village — both forbidden. Concretely:
- do **not** enable the new path for `settlement_profile = "vanilla"`, do not
  flip any default, and do not delete `PlainsVillagePrototype` or its tests in A1;
  ordinary server starts must behave exactly as they do today after A1 lands;
- the A1 executor and its tests land as an internal, tested layer reachable from
  tests and an explicit non-default entry point only, with the partial state
  marked honestly here and in code/doc comments;
- the A1 live proof must come through that internal/non-default path (a test or
  explicit probe), not by changing profile behaviour: show the executor placing
  one of the A1 feature types in a village context, with coordinates and a
  determinism repeat;
- activation, `PlainsVillagePrototype` deletion and any worldgen identity bump
  happen only in B, after the whole closure (trees included) is implemented and a
  live village with its decor is verified on the real cache.

Order therefore reads: **A1 (partial internal layer) → A2 (trees) → B (villages +
activation + prototype deletion + identity bump)**.

Status: A1 **not implemented** — this revision adds no production code; the
closure, the resolve-by-reference discipline, the A1/A2 split and this partial-
layer constraint are pinned here so the dedicated run can execute directly.
Interim truth in the tree is unchanged: core generates no vanilla villages,
`plains_village_prototype` is still the only core path placing village-like
structures, and the default `vanilla` profile still reports that core places no
villages.

## Vanilla villages: is there any vanilla feature execution in core? (2026-09-14)

Facts for the owner's scope decision on the `feature_pool_element` boundary.

**No. Core has no data-driven vanilla feature execution.**
The only consumer of `placed_feature`/`configured_feature` data is
`mc-data::worldgen_ores::load_ore_features` (`crates/mc-data/src/worldgen_ores.rs:99`).
It is ore-only — it globs placed features whose file stem starts with `ore_`
(`:118`) and returns `OreFeature` specs — and its module doc states it "does not
imply vanilla terrain parity; `mc-worldgen` still decides how to translate these
specs into its own generator rules". There is **no dispatch by feature type id**
for `minecraft:tree`, `minecraft:flower`, `minecraft:block_pile`,
`minecraft:random_patch` or any other vanilla configured feature. Ordinary
terrain vegetation is entirely Solaris-authored and hardcoded, decided in
`TerrainGenerator::apply_decorations`
(`crates/mc-worldgen/src/terrain.rs:1565`) through `place_tree`,
`tree_blocks_for_biome`/`tree_spacing_for_biome` (`:1823`),
`tree_density_allows` (`:1783`), `tree_site_is_stable` (`:1756`),
`place_cactus` (`:1862`), `place_sugar_cane` (`:1889`),
`plant_spacing_for_biome` (`:1846`), `ground_cover_density_allows` (`:1806`) and
`place_single` (`:1971`), over a hardcoded `DecorationBlocks` vocabulary.

**The 13 distinct village `feature_pool_element` features**
(quoted from `worldgen/template_pool/village/*/decor.json`):
`minecraft:acacia` (savanna), `minecraft:flower_plain` (plains),
`minecraft:oak` (plains), `minecraft:patch_berry_bush` (taiga),
`minecraft:patch_cactus` (desert), `minecraft:patch_taiga_grass` (taiga),
`minecraft:pile_hay` (desert, plains, savanna), `minecraft:pile_ice` (snowy),
`minecraft:pile_melon` (savanna), `minecraft:pile_pumpkin` (taiga),
`minecraft:pile_snow` (snowy), `minecraft:pine` (taiga),
`minecraft:spruce` (snowy, taiga).

What core already has, by block vocabulary in `terrain.rs`:
- *Partial, non-vanilla-shaped equivalents*: `oak`/`spruce`/`pine`/`acacia`
  (hardcoded per-biome tree placement with `oak_log`/`spruce_log`/`acacia_log`
  and leaves — not the vanilla configured tree), `flower_plain` (hardcoded
  `dandelion`/`poppy`/`blue_orchid` spacing), `patch_cactus` (`place_cactus`),
  `patch_taiga_grass` (`short_grass` in cold biomes), `pile_pumpkin` (a rare
  single `pumpkin` decoration, not a pile).
- *No equivalent at all*: `pile_hay` (no `hay_block`), `pile_ice` (no ice
  blocks), `pile_melon` (no `melon`), `pile_snow` (`snow_block` exists only as a
  surface material, no pile generator), `patch_berry_bush` (no
  `sweet_berry_bush`).

So option (c) would still not be vanilla parity for any of the 13 — it would
substitute Solaris-authored decoration — and 5 features have no candidate at all;
option (b) is a real engine project (vanilla configured-feature dispatch plus
placement modifiers); option (a) keeps villages vanilla except those entries,
which must then be a loud, documented gap.

## Vanilla villages: phase-1b decompile findings (2026-09-14) — hard boundary hit

Sources for every fact below: the decompiled classes of the bundled 26.1.2 server
jar (`.analysis/server.jar` → `META-INF/versions/26.1.2/server-26.1.2.jar`,
extracted and decompiled with Vineflower), plus the derived cache JSON already
quoted in the phase-1 section above. Confidence: high where a method body is
quoted; explicitly marked where not established.

**Blocker 1 — `feature_pool_element` is full vanilla placed-feature execution.
HARD BOUNDARY (stop and report, per the owner's instruction).**
`net.minecraft.world.level.levelgen.structure.pools.FeaturePoolElement.place`
(quoted) is `return this.feature.value().place(level, generator, random,
position);` and `fieldOf("feature")` is a `PlacedFeature`. Its `getSize` is
`Vec3i.ZERO` and it contributes one synthetic jigsaw block
(`name minecraft:bottom`, `final_state minecraft:air`, `pool minecraft:empty`,
`target minecraft:empty`, `joint rollable`). So a faithful village decor requires
running vanilla `PlacedFeature` from the derived data — placement modifiers,
heightmap-provider interaction, biome predicate and the configured feature's own
algorithm — inside chunk generation. Core decoration is Solaris's own and does
not execute vanilla placed features, so the 19 `feature_pool_element` entries
(oak, flower_plain, pile_hay, patch_cactus, spruce, pine, pile_snow/ice/melon/
pumpkin) cannot be reproduced without that engine. **This is a scope boundary,
not a detail: report it for the owner's decision rather than substituting
blocks.**

**Blocker 2 — processor execution (ESTABLISHED).**
`templatesystem.RuleProcessor.processBlock` (quoted) does:
`RandomSource random = RandomSource.create(Mth.getSeed(processedBlockInfo.pos()));`
then reads `locState = level.getBlockState(processedBlockInfo.pos())` and walks
`this.rules` **in order**, returning on the **first** rule whose `ProcessorRule.test`
passes — otherwise the block is returned unchanged. So the RNG is a **fresh
per-position** source seeded from the block's world coordinates (not the structure
seed, not a shared stream), rules are ordered and short-circuit, and location
predicates see the *current world* state. `ProcessorRule.test` (quoted) is
`inputPredicate.test(inputState, random) && locPredicate.test(locState, random) &&
posPredicate.test(inTemplatePos, worldPos, reference, random)` — predicate order
matters because only reached predicates consume randomness.
`RandomBlockMatchTest.test` (quoted) is
`blockState.is(this.block) && random.nextFloat() < this.probability`, which fixes
`mossify_*`/`zombie_*` exactly. Because the seed depends only on position, the
same block at the same coordinates always rolls the same value — reproducible
without the structure RNG. Other processors are decompiled in the same pass
(`BlockAgeProcessor`, `GravityProcessor`, `JigsawReplacementProcessor`,
`ProtectedBlockProcessor`, `BlackstoneReplaceProcessor`) and their bodies are
available for the implementation step; their interaction *ordering inside a
`StructureProcessorList`* was not yet read line-by-line and is **not established**.

**Blocker 3 — start geometry (PARTIALLY established).**
`structure.structures.JigsawStructure` + `pools.JigsawPlacement` show: the start
piece's Y comes from a `Types.WORLD_SURFACE_WG` heightmap at the start jigsaw
column; placement grows a `BoundingBox` limited by `maxDistance` (horizontal 80 /
vertical from `max_distance_from_center`), enforces `depth + 1 <= maxDepth`
(the structure's `size` = 6) and a free-space check
(`MutableObject<VoxelShape> free`, `tryPlacingChildren`), with an
`doExpansionHack` term. The exact per-step arithmetic of the free-space shape
growth and of `terrain_adaptation = beard_thin` is **not established**; the field
values are (`size 6`, `max_distance_from_center 80`, `start_height {absolute:0}`,
`project_start_to_heightmap WORLD_SURFACE_WG`, `beard_thin`).

**Blocker 4 — zombie variant (ESTABLISHED, data-only).**
`worldgen/structure/village_plains.json` has `"spawn_overrides": {}` and the
zombie variant is a separate template tree selected by weight: in
`plains/town_centers.json` the four normal town centres are `"weight": 50` and the
four `village/plains/zombie/town_centers/*` are `"weight": 1` (≈2%). The zombie
templates carry their own connectors — `zombie/town_centers/plains_fountain_01.nbt`
references only `minecraft:village/plains/zombie/streets` and
`minecraft:village/common/cats` (no `villagers`, no `common/iron_golem`), while
`zombie/houses/plains_small_house_1.nbt` references
`village/plains/zombie/streets` and `village/plains/zombie/villagers`. So the
zombie village needs **no extra code semantics**: honoring weights and pools plus
the zombie sub-pools, whose `zombie_*` processors (quoted in phase 1: 0.8 mossify,
doors/torches removed) produce the ruined look.

**Verdict for phase 2.** Blocker 1 is the named hard boundary (vanilla
`PlacedFeature` execution in core), so phase 2 is stopped before implementation
and the scope decision goes to the owner. If the owner accepts the boundary, the
remaining assembly path is implementable from established facts: read the one
`villages.json` set (salt 10387312, separation 8, spacing 34) and the five
weight-1 structures with their `has_structure/village_<type>` tags, resolve pools
and pieces from the cache, assemble by connectors with the phase-1 connector
names/joints, apply the position-seeded rule processors, and skip — loudly, not
silently — the `feature_pool_element` entries.

## Vanilla villages: phase-1 semantics pinned from the derived data (2026-09-14)

Binding amendment from the owner: no jigsaw mechanics may be implemented from a
guess. This section is the required phase-1 record — every fact below is quoted
from the cache derived by `content import` (the `26.1.2` cache at
`<cache>/data/minecraft/**`), with the file named. **Phase 2 (the data-driven
loader + assembler that replaces `PlainsVillagePrototype`) is NOT implemented**;
the "not established" list at the end is what blocks it, and none of it may be
approximated.

**Placement — there is ONE village structure set, not five.**
`worldgen/structure_set/villages.json`:
`"placement": {"type": "minecraft:random_spread", "salt": 10387312,
"separation": 8, "spacing": 34}` and
`"structures": [village_plains, village_desert, village_savanna, village_snowy,
village_taiga]`, each `"weight": 1`. So the brief's assumption of per-type
spacing/separation/salt is contradicted by the data: the five types share one
`random_spread` placement and differ only by the biome filter of the structure
that the weighted roll picks. Any implementation must read `spacing`/`separation`
/`salt` from this set, not hardcode per-type values.

**Per-type biome filter.** Each `worldgen/structure/village_<type>.json` declares
`"biomes": "#minecraft:has_structure/village_<type>"`, resolved from
`tags/worldgen/biome/has_structure/village_<type>.json`:
plains = [`minecraft:plains`, `minecraft:meadow`]; desert = [`minecraft:desert`];
savanna = [`minecraft:savanna`]; snowy = [`minecraft:snowy_plains`];
taiga = [`minecraft:taiga`].

**Jigsaw structure parameters** (identical across the five except `start_pool`),
quoted from each structure JSON: `type = minecraft:jigsaw`,
`start_pool = minecraft:village/<type>/town_centers`, `size = 6` (max depth),
`start_height = {"absolute": 0}`, `project_start_to_heightmap = "WORLD_SURFACE_WG"`,
`max_distance_from_center = 80`, `terrain_adaptation = "beard_thin"`,
`step = "surface_structures"`.

**Pools.** `worldgen/template_pool/village/<type>/{town_centers, houses, streets,
terminators, decor, trees, villagers, zombie/**}.json`. Across every village pool
the element types are exactly: `minecraft:legacy_single_pool_element` ×337,
`minecraft:feature_pool_element` ×19, `minecraft:empty_pool_element` ×12. An
element carries `weight`, `projection` (`rigid` for town centres/houses/decor,
`terrain_matching` for streets/terminators) and `processors` (a processor-list id
such as `minecraft:mossify_10_percent`, an inline `{"processors": [...]}` object,
or absent). Fallbacks chain to `minecraft:village/<type>/terminators` or
`minecraft:empty`.

**Connectors (canonical jigsaw data).** Proven by reading the template NBT:
jigsaw blocks carry a block entity with `name`, `target`, `pool`,
`joint` ∈ {`aligned`, `rollable`} and `final_state` (a concrete block state, or
`minecraft:structure_void`); the facing comes from the block state's
`orientation` property. Observed examples: `name/target = minecraft:street`
(pool `<type>/streets`, `final_state minecraft:structure_void`,
`joint aligned`) in `village/plains/town_centers/plains_fountain_01.nbt` and
`village/plains/streets/straight_01.nbt`; `minecraft:building_entrance`
(pool `<type>/streets`) and `minecraft:bottom` (`rollable`, pools
`<type>/villagers`, `<type>/decor`, `village/common/cats`,
`village/common/iron_golem`) in `village/plains/houses/plains_small_house_1.nbt`.

**Processors change blocks, so they are required for faithfulness.**
`worldgen/processor_list/<id>.json`, each one `minecraft:rule` processor:
`mossify_10_percent` = `random_block_match` on cobblestone, probability 0.1 →
`minecraft:mossy_cobblestone`; `street_plains` = `dirt_path` → planks when the
*location* block is water, plus random `dirt_path` → snowy grass; `zombie_plains`
= the 0.8 mossify plus `tag_match` on `minecraft:doors` → air and torch removal;
the pools reference 16 lists in total (`farm_*`, `mossify_*`, `street_*`,
`zombie_*`).

**Not established — blocks faithful implementation; do not approximate:**
1. `feature_pool_element` entries name configured/placed features
   (`minecraft:oak`, `flower_plain`, `pile_hay`, `patch_cactus`, `spruce`,
   `pine`, `pile_snow/ice/melon/pumpkin`) that vanilla places as features. Core
   decoration is Solaris's own and does not execute vanilla placed features, so
   those villager/decor entries cannot yet be reproduced.
2. Exact processor execution: rule ordering, the RNG derivation of
   `random_block_match` probabilities, and the other processor types
   (`block_age`, `gravity`, `jigsaw_replacement`, `protected_blocks`,
   `blackstone_replace`) are not established beyond the quoted JSON.
3. Placement math: how `project_start_to_heightmap = WORLD_SURFACE_WG` and
   `terrain_adaptation = beard_thin` transform the start piece's Y, and the
   piece bounding-box/overlap and `max_distance_from_center` rules beyond their
   quoted field values.
4. Zombie-village semantics (the `zombie/**` sub-pools appear in
   `town_centers` with weight 1 against 50, i.e. a rare variant) — the
   villager→zombie conversion is not established.

Until 1-4 are established and phase 2 lands, the interim truth stands: core
generates no vanilla villages, `plains_village_prototype` still exists (it is the
only core path that places village-like structures) and the default `vanilla`
profile still reports that core places no villages. Nothing in the tree claims
otherwise.

## Vanilla content source: managed cache + packaged importer (stage 1, landed)

Pinned values and integrity rule (observed live 2026-09-14; where any other line in this
file conflicts, these win):
- `version_manifest_v2.json` is the discovery document: its `26.1.2` entry carries
  `complianceLevel, id, releaseTime, sha1, time, type, url` and NO `size`, and the
  manifest itself is never pinned (a stale embedded hash would break version updates).
  Select by `id`, then verify the VERSION JSON against that entry's sha1
  `0d0f7a851642f3c81d1cff0a608aa048e5e8319f`.
- 26.1.2's version JSON `downloads` holds ONLY `client` and `server` (no mappings), so
  protocol/obfuscation-derived ids keep coming from the server jar plus javap/probes:
  `server` sha1 `97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51` size 60417480 (carries
  `data/minecraft/**` and the datagen entry point); `client` sha1
  `4e618f09a0c649dde3fdf829df443ce0b8831e65` size 38113927 (client resources only).
- `assetIndex` id 30, sha1 `25c0d18dd5ab0e3a7515f3704f401de547da4b1c`, size 548391,
  totalSize 456526210; the `e0b6…` hash appearing lower in this file is the piston URL
  path segment, not the artifact digest. Objects come from the resource CDN as
  `<first-two-hex>/<hash>` with per-object `{hash,size}` - the same size-then-SHA1
  validation Prism performs (`launcher/minecraft/AssetsUtils.cpp`).
- Integrity rule: verify the version JSON against the selected manifest entry's sha1,
  then each artifact against the hash and size declared in that version JSON entry;
  require size only where the fetched JSON supplies one.
- `reports/registry_network_nbt/**` is a real Configuration-state handshake capture
  (raw datapack JSON is not wire-equivalent, `tools/extract-vanilla-data.sh:24-27`;
  `crates/mc-net/src/configuration.rs:328-340` rejects a client that declines the core
  pack without full payloads), so the packaged importer runs it as one of its steps.

Owner direction (2026-09-14): Solaris needs the real vanilla **data**, and it must
not depend on a manual `data/vanilla` sidecar. Mojang bytes never enter Git, so
the one admissible path is a managed, launcher-style source: a distributable
`solaris content import` derives a complete cache from the operator's own
licensed artifact (local jar or downloaded from Mojang's public metadata into a
gitignored cache). Startup reuses an already-valid cache offline and, on a
missing or invalid cache, invokes that same packaged, staged, atomic importer
itself before binding the listener (the owner's "Автоматом"); a failure at any
step keeps the previous cache intact and fails with the concrete cause. Absence
of any licensed source fails loudly; there is no silent subset and no second
content path.

Sequence: (1) classification below, (2) `content import` + cache + startup
discovery, (3) per-type villages (village structure sets + jigsaw assembly) on
that same path. **Status: (1) landed and (2) landed + verified end to end. (3)
is the open item; nothing claims vanilla villages yet.**

What landed for (2):
- `crates/mc-server/src/content_cache.rs` — the single content source. Discovery
  order: `[data].vanilla_data_dir` override → `SOLARIS_CONTENT_CACHE` → the
  user-level cache (`$XDG_DATA_HOME|~/.local/share` + `solaris/content/<release>`)
  → `data/vanilla` under the working directory and its ancestors (so a cache in
  a source checkout is found from the repo root *and* from `crates/mc-server`,
  where `cargo test` puts the process). The override and the env var are authoritative: an invalid
  one is an error, never a silent fall-through to another location.
  `validate_content_cache` rejects a partial derivation by name (missing
  `reports/registry_network_nbt` is called out explicitly), and the failure text
  names the prerequisite, every searched path, the `content import` command and
  the JDK requirement.
- `crates/mc-server/src/content_import.rs` — the packaged importer, one code path
  shared by the subcommand and startup. Resolves the pinned release through
  Mojang's public version manifest (hash-only entry: no size is required there),
  verifies the downloaded VERSION JSON against the entry's `sha1`, then the
  artifact against the version JSON's own `downloads.server` `sha1`+`size`
  before anything is staged; `--from <jar>` stages an operator jar only after the
  same verification. Derivation = bundle unpack (inner
  `META-INF/versions/server-<id>.jar` + `META-INF/libraries/*.jar`) +
  `data/minecraft/**` subset + the bundle's own datagen
  (`-DbundlerMainClass=net.minecraft.data.Main … --server --reports`) + the three
  in-tree Java extractors (`LightExtractor`/`MiningExtractor`/`ExplosionExtractor`,
  embedded via `include_str!`, compiled against the bundle classpath — the repo
  shell scripts are never invoked) + the wire capture. The licensed jar lives in
  scratch `.work` and never lands in the cache. Validation is `mc_data::load`
  plus `has_full_registry_payloads`, then `publish` swaps staging in atomically
  (previous tree restored if the rename fails); `finish_import` funnels every
  derivation failure to "discard staging, keep the live cache".
- `crates/mc-server/src/main.rs` — `mc-server content import --version 26.1.2
  [--from <jar>|--download] [--cache <dir>]`, and `serve` calls
  `resolve_or_import` before `StartupData::load`: a valid cache is reused
  offline, a missing/invalid one triggers the same packaged importer
  automatically before the listener binds, and the launch fails with the
  concrete cause if that fails.
- One content path: the `load_effective_*` loaders (`crates/mc-server/src/
  startup_data.rs`) now take `&Path` instead of `Option<&Path>`; the
  embedded-subset startup selection is deleted, so there is no second mode and
  no silent subset. `startup_data` stays the only consumer. The minimal embeds
  still used for non-cache domains (blocks report, items, entity types, biome
  spawns, and the recipe/loot completion) are listed under "delta" below.
- Wire capture ownership: the step lives in `mc-test-harness`
  (`registry_capture::capture_registry_payloads_from_jar`, owned by
  `ContentCaptureStep`) and is called by the importer; there is exactly one
  Configuration capture implementation and it reuses the existing
  `registry-data-extract` path.

- JDK preflight (added after the live acceptance run): `preflight_java` runs the
  selected executable's `-version` before any derivation, requires the Java 25
  family, and fails fast naming the resolved path, the observed banner, the
  required major and the JAVA/JAVAC override; the JVM's stderr tail is folded
  into every datagen/extractor failure (`stderr_tail`). Verified supervised with
  the real binary, three runs, same cache path:
  (a) cold + PATH java (21.0.2): `error: automatic vanilla content import failed
  after: … Java 21 at `java` cannot derive content for 26.1.2: the server
  bundle's classes require Java 25 (observed `openjdk version "21.0.2"
  2024-01-16`). set JAVA=/path/to/jdk-25/bin/java and
  JAVAC=/path/to/jdk-25/bin/javac` — exit 1 in 66 ms, no JVM started, no
  download, the cache path was never created;
  (b) cold + JAVA/JAVAC = the 25.0.2-graalce JDK: automatic import logged
  (`no valid vanilla content cache found; importing automatically`,
  `running the server bundle's own datagen`,
  `capturing exact RegistryData payloads from a temporary vanilla server`,
  `vanilla content cache imported … registries=28 entries=382`) and the listener
  bound (hub readiness on the configured port, 32.6 s including the import);
  (c) warm on the same cache with only PATH java (no JAVA, i.e. no JDK 25 at
  all): 0 `importing automatically` lines, `registry index loaded registries=28
  entries=382`, `version.json` mtime unchanged from the import, listener bound
  in 2.3 s — Java is an import-time prerequisite only.

- validation of record for this revision:
  `cargo test -p mc-server --bin mc-server -- content_import::tests` 16 passed /
  0 failed (synthetic fixtures: manifest selection is hash-only, version
  metadata, incomplete-cache rejection by name, discovery priority/no
  fall-through, failure text, import target, publish swap, failed derivation
  keeps the previous cache, data-subset filter, version pin, and two tests that
  drive the real startup entry point `resolve_or_import` itself —
  `resolve_or_import_reuses_a_valid_cache_without_running_the_importer` proves
  positively that a complete cache is reused with no import attempt: the exact
  importer staging path is occupied by a tripwire file and the cache's parent
  must still hold only the cache plus that tripwire;
  `resolve_or_import_reports_the_missing_cache_and_publishes_nothing` asserts the
  cold-machine error carries the discovery failure (prerequisite, every searched
  location, `mc-server content import`, JDK 25) plus the concrete import cause,
  and publishes nothing. Mutation check: making `resolve_or_import` always enter
  the importer (scratch edit, since restored byte-identically) fails only the
  reuse test — that is the regression guard if startup ever goes back to always
  importing);
  real end-to-end import with the verified oracle jar
  `mc-server content import --version 26.1.2 --from .analysis/server.jar
  --cache /tmp/from-content` => "imported vanilla content 26.1.2 into
  /tmp/from-content: 28 registries, 382 entries" (42 MB cache, no jar staged,
  `reports/{blocks,registries,packets,block_light,block_mining,block_explosion}.json`
  + `reports/minecraft/components/item` + `reports/registry_network_nbt/**`);
  automatic cold-cache launch (`SOLARIS_CONTENT_CACHE=/tmp/cold-cache` with the
  cache deleted) logged, from a real `serve`:
  "no valid vanilla content cache found; importing automatically cache=/tmp/cold-cache",
  "downloading server bundle url=…", "running the server bundle's own datagen",
  "capturing exact RegistryData payloads from a temporary vanilla server",
  "vanilla content cache imported cache=/tmp/cold-cache registries=28 entries=382",
  then failed on the concrete cause at world open (a stale world lease from an
  earlier run of mine) — never a silent subset. Oracle artifact check:
  `.analysis/server.jar` is byte-identical to `downloads.server`
  (sha1 97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51, 60417480 bytes).
  `cargo test -p mc-server --bin mc-server` 59 passed / 1 failed / 1 ignored
  (the failure is `tests::deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles`,
  the known missing `../solaris-default-plugins/solaris-settlements`);
  `cargo test -p mc-server --lib` 76 passed / 0 failed;
  `cargo clippy -p mc-worldgen -p mc-server --all-targets -- -D warnings` clean;
  `run fmt` PASS `.analysis/validation/20260914T040313-fmt-v0qz3x2p`;
  `run code-health` PASS `.analysis/validation/20260914T040316-code-health-b9g0k_ga`;
  `cargo test -p mc-net --lib -- warehouse journal chest owned_inventory`
  125 passed / 0 failed / 1 ignored; `cargo test -p mc-server --test cli`
  42 passed / 0 failed (the server-starting `pregenerate_accepts_negative_…`
  test now reuses a discovered cache and skips without one, like the other
  sidecar-dependent tests; run with `SOLARIS_CONTENT_CACHE=/tmp/cold-cache` it
  executes for real and passes). Offline reuse proof: a fresh-world `serve`
  against the valid cache logged 0 auto-import attempts and
  "registry index loaded registries=28 entries=382" / "tags loaded tags=661
  entries=7736" before running until the harness timeout.
- changed_files: `crates/mc-server/src/content_cache.rs` (new),
  `crates/mc-server/src/content_import.rs` (new),
  `crates/mc-server/src/content_import_tests.rs` (new),
  `crates/mc-server/src/startup_data.rs`, `crates/mc-server/src/startup_data_tests.rs`,
  `crates/mc-server/src/main.rs`, `crates/mc-server/src/lib.rs`,
  `crates/mc-server/src/structure_rules_tests.rs`, `crates/mc-server/tests/cli.rs`,
  `crates/mc-server/Cargo.toml`,
  `crates/mc-test-harness/src/registry_capture.rs` (capture extracted from the
  bin; `ContentCaptureStep` then packaged it), `crates/mc-test-harness/src/lib.rs`,
  `crates/mc-test-harness/src/bin/registry_data_extract.rs`, `example.toml`,
  `docs/PLUGINS.md`, `docs/decisions/0001-vanilla-data-as-runtime-input.md`,
  `docs/decisions/0008-overworld-density-router.md`, `docs/MEMORY.md`.

### Classification: `tools/extract-vanilla-data.sh`, end to end

Inputs it consumes:
- `$1` = the Mojang **server bundle jar** (default `.analysis/server.jar`).
- A JDK matching the bundle's `javaVersion.majorVersion` (25 for this target)
  plus `unzip`/`find`/`cargo` at *extraction* time (not at server runtime).

Derivation steps and outputs (the whole cache):
1. Unzip the bundle: `META-INF/versions/server-<id>.jar` (inner server) and
   `META-INF/libraries/*.jar` (classpath for the Java extractors).
2. Read `version.json` from the bundle root → `id`, `protocol_version`
   (cross-checked against `mc_protocol::TARGET_RELEASE`/`PROTOCOL_VERSION`).
3. Unzip the inner jar's `data/minecraft/**` and keep the consumed subset:
   - 27 top-level Configuration registries (`banner_pattern`, …, `world_clock`,
     `zombie_nautilus_variant`) → `data/minecraft/<registry>/`;
   - `worldgen/{biome,configured_feature,placed_feature,structure,structure_set,template_pool,processor_list,multi_noise_biome_source_parameter_list}`;
   - `structure/**` (the village templates and other NBT pieces);
   - `tags/**`;
   - `recipe/**`, `loot_table/**`.
4. Datagen through the bundle itself —
   `java -DbundlerMainClass=net.minecraft.data.Main -jar <bundle> --server --reports`
   → `reports/{blocks,registries,packets}.json` and
   `reports/minecraft/components/item/**`.
5. `tools/extract-block-light.sh` → `reports/block_light.json` (compiles and runs
   `LightExtractor.java` against the inner jar + libraries; not available from
   datagen).
6. `tools/extract-block-mining.sh` → `reports/block_mining.json`.
7. `tools/extract-block-explosion.sh` → `reports/block_explosion.json`.
8. `cargo run -p mc-test-harness --bin registry-data-extract -- --jar <bundle>
   --out <cache>` → `reports/registry_network_nbt/**`: it launches a vanilla
   server from the same jar, performs the Configuration handshake, declines
   Known Packs, and captures the exact `RegistryData` payloads
   (`VanillaData::has_full_registry_payloads`, `crates/mc-data/src/lib.rs:249-267`,
   gates a real client login at `crates/mc-net/src/configuration.rs:329`).
9. Wipe-and-replace the generated subdirs; `README.md` and other tracked files
   stay in place.

### Observed launcher flow (Prism, this machine) — the evidence for "how a launcher does it"

Inspected an actual PrismLauncher installation at
`~/.local/share/PrismLauncher` rather than relying on generic knowledge. The
observed artifacts and the exact fields relied on:

- `meta/net.minecraft/index.json` (cached component index, 1016 versions). The
  `version = "26.1.2"` entry is
  `{"version":"26.1.2","type":"release","releaseTime":"2026-04-09T10:12:23+00:00",
  "sha256":"5f13ad46b6b82683305cf9c3c84f4f4a83ba3111415db74021acda7e6eab1e56","requires":[…lwjgl3…]}`.
  It carries Prism's own `sha256` of the per-version metadata, not a jar url.
- `meta/net.minecraft/26.1.2.json` (resolved per-version metadata, 24423 bytes).
  Keys observed: `assetIndex`, `libraries` (51 entries), `logging`, `mainClass`
  (`net.minecraft.client.main.Main`), `mainJar`, `minecraftArguments`,
  `compatibleJavaMajors: [25]`, `compatibleJavaName: java-runtime-epsilon`.
  - `mainJar.downloads.artifact = {"sha1":"4e618f09a0c649dde3fdf829df443ce0b8831e65",
    "size":38113927,"url":"https://piston-data.mojang.com/v1/objects/4e618f09…/client.jar"}`
    — i.e. the **client** jar; every `libraries[]` entry has the same
    `downloads.artifact.{sha1,size,url}` shape (e.g.
    `at.yawk.lz4:lz4-java:1.10.1` → `sha1 f541d7f9…`, `size 910232`,
    `https://libraries.minecraft.net/…`).
  - `assetIndex = {"id":"30","sha1":"e0b6126c05700238ee326e24af45f8f84c64dbf5",
    "size":548391,"totalSize":456504623,"url":"…/30.json"}` — these are Prism's
    NORMALIZED component values: `e0b6…` is the hash inside the piston URL path,
    not the artifact digest. The raw Mojang version JSON declares the index as
    sha1 `25c0d18dd5ab0e3a7515f3704f401de547da4b1c`, size 548391, totalSize
    456526210, and that is the value the importer verifies (see the pinned table
    at the top of this file).
  - **No `downloads.server` and no `downloads` blob at all**: Prism normalizes
    the client component, so a launcher-prepared instance cannot supply the
    artifact our extractor needs.
- `assets/indexes/30.json` (548391 bytes) is `{"objects": …}` with 4750 entries
  of the shape `"icons/icon_128x128.png": {"hash":"b62ca8ec…","size":9101}`, and
  `assets/objects/<first-two-hex>/<hash>` holds the files (e.g.
  `objects/00/000fc372f9d0533d29ef0cccd427402969181319`, 527842 bytes).
  Explicitly: this hashed `objects` cache holds **CLIENT resources**
  (icons/lang/sounds/textures), not server data — registries, reports and
  structure templates come from the server jar and its datagen, so the launcher
  asset cache cannot supply them.
- `assets/skins/` is player-skin storage, likewise client-side.

### Artifact mapping (each input → the manifest artifact it must come from)

The pinned target is **26.1.2**; hashes/sizes are what a launcher verifies before
anything is staged. Prism's own client component (above) does not carry the
server artifact, so the server jar is resolved from the upstream Mojang version
JSON (`https://piston-meta.mojang.com/v1/packages/<version-json-sha1>/26.1.2.json`),
whose field shapes match Prism's `downloads.artifact.{sha1,size,url}` exactly:
- Bundle jar → `downloads.server`:
  `https://piston-data.mojang.com/v1/objects/97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51/server.jar`,
  size 60417480, sha1 `97ccd4c0ed3f81bbb7bfacddd1090b0c56f9bc51`. Verified locally:
  the repo oracle `.analysis/server.jar` is byte-identical (sha1 matches), so the
  current cache is derivable from exactly this artifact.
- Version metadata → the per-version manifest JSON:
  `https://piston-meta.mojang.com/v1/packages/0d0f7a851642f3c81d1cff0a608aa048e5e8319f/26.1.2.json`,
  sha1 `0d0f7a851642f3c81d1cff0a608aa048e5e8319f` (the jar's own `version.json`
  is cross-checked against it). Java prerequisite: `javaVersion.component =
  java-runtime-epsilon`, `majorVersion = 25`.
- `downloads.client` (`sha1 4e618f09a0c649dde3fdf829df443ce0b8831e65`, size
  38113927): **not consumed** — every derived payload comes from the server jar
  and its datagen; no loader reads a client-only file.
- `assetIndex` (`id 30`, sha1 `25c0d18dd5ab0e3a7515f3704f401de547da4b1c`,
  totalSize 456526210): launcher `assets/objects` hold **client resources**
  (textures/sounds/lang), not server data. No loader consumes them today, so they
  are out of scope for the cache (lowest priority, revisit only if a domain
  needs them).
- `server_mappings`/`client_mappings`: 26.1.2 exposes none in `downloads`, and
  no consumed payload is obfuscation-derived (protocol ids come from datagen's
  `packets.json`). Not staged.

Consequence for the cache delta: everything the `startup_data` loaders read is
derived from the single `downloads.server` artifact, so `content import` needs
exactly one jar (+ Java at import time) and no client/asset download.

Minimal embeds that remain in `mc-data` until the cache is the only source in
practice (so the next stage knows the delta):
- `VanillaData::solaris_required_data` — the repo-owned required registry index
  (`crates/mc-data/src/lib.rs:328-330`, `required_registry_index.json`), used
  today when no cache is selected; it carries identifiers, **not** the
  data-pack definitions, so `has_full_registry_payloads()` is false
  (`crates/mc-data/src/lib.rs:249-267`).
- `blocks::solaris_required_blocks_report` — the minimal required block/state
  report (`crates/mc-data/src/blocks.rs:64-66`), still used for the block
  registry even when a cache is present (there is no full `reports/blocks.json`
  reader wired at startup today).
- `tags::solaris_required_client_tags`, `loot::builtin`,
  `recipes::solaris_required_recipes`,
  `item_components::solaris_required_item_facts`,
  `block_light::BlockLightTable::conservative_from_blocks_report`,
  `entity_types::solaris_required_entity_types`,
  `biomes::solaris_required_biome_spawn_rules` — the per-domain minimal
  fallbacks. `startup_data` keeps using the recipe/loot loaders' existing
  embedded *completion* (ADR 0001), but the fallback **selection** on a missing
  cache is what stage 1 removes.
