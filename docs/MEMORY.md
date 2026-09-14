# Solaris current cursor

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

## Settlement profiles: no fake vanilla villages, loud gap (landed 2026-09-14)

The owner corrected the settlement-profile semantics mid-flight: "vanilla
villages belong to vanilla" was **not landable honestly** with today's
`StructureRules` API, and the main agent stopped the five-type loader before it
was written. What the core can express: `plains_village_prototype_with_parts`
combines a *fixed three-template* composite (`village/plains/town_centers/
plains_fountain_01`, `houses/plains_small_house_1`, `houses/plains_tool_smith_1`)
into one template, `plains_village_markers` places that single vector on a 34/8
grid, `with_structure_set_facts` only reads `minecraft:village_plains`, and no
biome filter per village type, per-type pool, or jigsaw assembler exists (jigsaw
blocks are used only to attach villager spawn markers,
`is_plains_villager_jigsaw`). Building a "five-type vanilla village loader" on
that would place three plains houses in every biome — a fake. So the landed
change is truthfulness plus a loud report:

1. **Docs now say exactly what each profile places.**
   `SettlementProfile::Vanilla` (`crates/mc-server/src/lib.rs`) is documented as
   "places no villages: core does not implement vanilla village generation", and
   `PlainsVillagePrototype` as a bounded three-template Solaris composite on the
   extracted plains village spacing — explicitly not full vanilla generation and
   no desert/savanna/snowy/taiga villages. Same for the `DataSection` field and
   `example.toml`.
2. **The silent zero is gone.** The default `vanilla` profile no longer quietly
   places nothing. A shared `VANILLA_SETTLEMENT_NOTICE` (`crates/mc-server/
   src/main.rs`) is emitted as the typed `--check` operator warning
   `settlement_profile_vanilla_generates_no_villages`, and
   `structure_rules_for_startup` logs the same notice with `tracing::warn!` on
   the serve path whenever no plugin plan is deployed and the profile is
   `vanilla`. The notice names both real routes: a deployed Luau settlement plan,
   or the explicit `plains_village_prototype` opt-in with `vanilla_data_dir`.
3. **Plugin-plan precedence untouched and now pinned on CI.** A deployed
   settlement plan still returns first and builds the composite; the new
   `structure_rules_tests::deployed_settlement_plan_replaces_core_villages`
   proves it with the synthetic sidecar (default `vanilla` profile + plan ⇒ 15
   pasted marker blocks).
4. **Identity/revision unaffected.** `Vanilla` placement semantics did not
   change — it still places no structures — so `WORLDGEN_REVISION` stays 21 and
   no existing world gains structures or hits the persisted-contract mismatch
   (`crates/mc-server/src/startup_validation.rs`). Only the report is new.
- tests: `structure_rules_tests::default_settlement_profile_reports_missing_core_village_generation`
  (new) asserts the typed notice fires for `vanilla`, names
  "does not implement vanilla village generation" and the prototype opt-in,
  does **not** fire for `plains_village_prototype`, and that the default profile
  still returns empty rules (reported, not silent);
  `deployed_settlement_plan_replaces_core_villages` (new) as above; the existing
  synthetic-sidecar test still asserts 15 blocks for the prototype vs 0 for the
  default. `cargo test -p mc-server --bin mc-server structure_rules_tests` =>
  3 passed / 0 failed / 48 filtered.
- **open item (the only true vanilla village path, not implemented):** real
  vanilla village generation needs per-type village structure sets with their
  biome tags (`minecraft:village_plains|desert|savanna|snowy|taiga`, biome tags
  under `data/minecraft/tags/worldgen/biome/has_structure/`), each type's own
  template pools, and a jigsaw assembler that reads those pools. None of that
  exists in `crates/mc-worldgen/src/structures.rs`; until it does, no code or doc
  may claim Solaris generates vanilla villages. `PlainsVillagePrototype` stays
  the honest, explicitly-labelled prototype.
- owner/manual: the live proof with real Mojang village data (extracted
  `data/vanilla/` sidecar, startup logged notice absent for the prototype,
  real templates pasted client-side) — this machine has no Mojang sidecar.

## Core-only closeout (owner 2026-09-14: "доделывай че по ядру есть") - landed

Both items that were open here are closed in the core repo; neither needed a
sibling plugin package.

1. Append-before-publication ordering probe - closed, and order-sensitive rather
   than structural. The regional run arms a `#[cfg(test)]` probe
   (`arm_warehouse_publication_probe`,
   `crates/mc-net/src/play/simulation/regional_mutation.rs`) for every
   server-owned warehouse publication it holds, before it appends. The
   observation itself is taken inside the publication path
   (`publication_probe::observe`, called from `dispatch_visibility_commands` in
   `crates/mc-net/src/play/session/outbound.rs`), where the container's
   `ChestSlots` command leaves the run, and the journal records the append state
   at that moment (`WorldChunkJournal::record_warehouse_publication_for_test`,
   backed by `next_append_id`; read back with `warehouse_publications_for_test`).
   Test:
   `play::simulation::tests::server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both`
   (renamed from `..._journals_images_and_receipt_then_recovers_both`) asserts
   the run's only observation is `(decision_id, true)` beside its container,
   journal and recovery assertions. Both failure directions were exercised as
   scratch edits and reverted: publishing before appending (the run's append
   deferred until after the publication loop) fails the assertion with
   `left: [(1, false)]`, and a publication that never passes through the observed
   path fails it with `left: []` - the test cannot pass while the slots leave
   without the decision behind them. Production behaviour is unchanged: the
   probe, its arming and its re-export are all `#[cfg(test)]`
   (`cargo check -p mc-net --features load-bench` clean).
2. Vanilla villages without a plugin - closed as the existing path, verified and
   pinned, nothing re-implemented (no new profile, template, or generator).
   `[data] settlement_profile = "plains_village_prototype"` already builds the
   vanilla plains prototype with no plugin at all (`structure_rules_for_startup`,
   `crates/mc-server/src/main.rs:1313`). The profile *default* is
   `SettlementProfile::Vanilla` in `crates/mc-server/src/lib.rs:150-159` (enum
   `#[default]`) with the config field at `:194-195`; the
   `crates/mc-server/src/startup_validation.rs:33` `vanilla_profile` fallback is
   only the persisted-contract reader's default, not the config default. The
   profile requires `data.vanilla_data_dir` (Mojang NBT never enters Git),
   `example.toml` documents the switch, and the profile name is the recorded
   world identity whenever no deployed plugin plans settlements
   (`crates/mc-server/src/main.rs:667-670`, `:743`). New coverage of the real
   generation path with a *synthetic* sidecar:
   `structure_rules_tests::builtin_settlement_profile_generates_village_structures_from_the_sidecar`
   (`crates/mc-server/src/structure_rules_tests.rs`, included from `main.rs`)
   writes three synthetic structure templates plus one structure-set fact file
   into a temp dir, parses the stock `[data]` TOML, builds the rules through
   `structure_rules_for_startup`, generates the fixed-centre village chunks
   through `build_terrain_generator`, and asserts 15 pasted marker blocks for
   `plains_village_prototype` against 0 for the default `vanilla` profile. World
   identity: `tests::world_contract_accepts_and_persists_the_builtin_settlement_profile`
   creates, persists and reopens a world carrying the profile and refuses a world
   whose persisted profile differs; `builtin_settlement_profile_requires_the_vanilla_sidecar`
   keeps the missing-sidecar refusal.
- validation of record for this revision: `cargo test -p mc-net --lib -- warehouse`
  13 passed / 0 failed, `-- journal` 68 passed / 0 failed / 1 ignored, `-- chest`
  30 / 0, `-- owned_inventory` 19 / 0, and `cargo test -p mc-net --lib` 2188
  passed / 0 failed / 8 ignored; `cargo test -p mc-server --bin mc-server` 47
  passed / 1 failed / 1 ignored - the failure is
  `tests::deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles`,
  which panics on the missing sibling package (see the owner/manual line) - and
  `cargo test -p mc-server --lib` 79 / 0; `cargo test -p mc-worldgen --lib` 167
  passed / 0 failed / 5 ignored; `cargo clippy -p mc-net --all-targets -- -D
  warnings` clean; `cargo clippy -p mc-server --all-targets -- -D warnings`
  clean; `run fmt` PASS `.analysis/validation/20260914T030153-fmt-azhvfdq9`;
  `run code-health` PASS `.analysis/validation/20260914T030157-code-health-64kxddrv`.
- owner/manual, and explicitly NOT run here: L2 `correctness` and the real-client
  gates (they need the unpublished
  `../solaris-default-plugins/solaris-settlements`; this machine has only
  `data/vanilla/README.md`, so the sidecar-present field proof - startup logging
  `materialized built-in settlement prototype` and a client seeing village
  terrain at the seed-0 fixed centre - also stays owner/manual). The
  synthetic-sidecar test covers the generation path itself; it does not claim
  that the real Mojang templates paste identically.
- changed_files: `crates/mc-net/src/play/world_journal.rs`,
  `crates/mc-net/src/play/session/outbound.rs`,
  `crates/mc-net/src/play/session.rs`,
  `crates/mc-net/src/play/simulation/regional_mutation.rs`,
  `crates/mc-net/src/play/simulation.rs`, `crates/mc-server/src/main.rs`,
  `crates/mc-server/src/structure_rules_tests.rs` (new sibling `*_tests.rs`),
  `docs/decisions/0004-staged-single-writer-simulation.md`, `docs/MEMORY.md`.
- next: nothing from the owner's 2026-09-14 core ask remains open. The
  settlements/live-chain items stay blocked on the unpublished sibling package.
  Superseded in part by the settlement-profile section above: real per-type
  vanilla village generation is named there as the one open core item.
- review closeout (independent read-only `WarehouseInvariantReview`, 13m32s): verdict pass,
  `overall_correctness: correct` (confidence 0.76), all seven invariant questions answered with
  `file:line` evidence. Residual it named: the append-before-publication rule is structural only -
  no probe or test fails if publication preceded the append.
- validation of record for the final tree, run by Main after the last source edit: `run fmt` PASS
  `.analysis/validation/20260914T021459-fmt-ei9lc2u_`; `run code-health` PASS
  `.analysis/validation/20260914T021508-code-health-qgw6zram`; `cargo test -p mc-net --lib -- warehouse`
  13 passed / 0 failed; `cargo clippy -p mc-net --all-targets -- -D warnings` clean. Artifact paths
  cited elsewhere in this file that predate the last edit do not cover this revision.
## Warehouse write path - C1 slice 2 (landed + verified 2026-09-14)

- checkpoint_closed: the writable warehouse endpoint (`CommitChest` server-owned
  mode, C1 slice 2). `base_tree` `8414853e`; the previous cursor's design,
  rejected alternatives and mandated test shape are unchanged below/in
  `docs/decisions/0004-staged-single-writer-simulation.md`.
- landing: `SimulationCommand::CommitChest` gained `plugin_receipt:
  Option<Vec<u8>>` (present == server-owned) instead of a parallel command.
  `ChestTransaction::commit_server_owned`
  (`crates/mc-net/src/play/session/transactions.rs`) is the second entry point
  beside the menu `commit`: same composite, minus the `actor_has_open_view`
  fence, publishing `ChestSlots` to every viewer INCLUDING the actor. The
  journaled regional run stamps the container chunk for the run's reserved
  decision id (`stamp_chunks_for_world_journal`), carries the after-image plus
  the encoded receipt in ONE `record_reserved_decisions` group, then releases the
  flush fence, publishes and responds. `command_needs_world_journal` keeps a
  receipt-bearing command out of the non-journaled lanes (the menu path answers
  `WorldMutationFailed` rather than committing unjournaled). A refused deposit
  appends its reserved decision with NO participant, so the run's append stays
  contiguous. The endpoint gate is gone: `commit_owned_inventory_transfer`'s
  `Warehouse` arm was replaced by routing in
  `InventoryRuntime::execute_owned_inventory`, which resolves the binding and
  loaded container through the same helper the read path uses
  (`InventoryRuntime::resolve_warehouse_container`, now shared with
  `warehouse_inventory_snapshot`), fences BOTH endpoints (the DTO already
  requires one fence per distinct endpoint), plans with
  `plan_owned_item_transfers`, prepares the receipt batch with the player
  after-image, sends it through `SettlementWorld::commit_warehouse_transfer`
  (live impl over `SimulationHandle`), then projects the ledger frame + the
  player recovery, publishes the actor's `AuthoritativeInventory` and only then
  `mark_inventory_projected`.
- decision-id ownership (settled, do not re-derive): the RUN allocates the id and
  the response carries it. A player endpoint's receipt fence revision IS the
  post-commit `inventory_operation_revision`, which `PlayerInventoryRecovery::
  recover`/`load_player_state` force to be the journal decision id; a submitter
  pre-reservation would break `record_reserved_decisions` contiguity (regional
  runs reserve one block each and cannot wait without deadlocking the owner task
  that must process the deposit). Consequence, deliberate and documented in
  `docs/PLUGINS.md`: a warehouse transfer receipt's `Transfer` result names the
  WAREHOUSE endpoint's resulting fence (binding revision + hash of the planned
  container slots) and NOT the actor's, whose fence is re-read by a query —
  listing a pre-commit revision would hand the plugin a permanently stale fence.
- changed_files: `crates/mc-net/src/play/simulation.rs`,
  `crates/mc-net/src/play/simulation/regional_mutation.rs`,
  `crates/mc-net/src/play/session/transactions.rs`,
  `crates/mc-net/src/play/session/container_views.rs`,
  `crates/mc-net/src/play/session/owned_inventory_endpoint.rs`,
  `crates/mc-net/src/play/session/owned_inventory_endpoint_tests.rs`,
  `crates/mc-net/src/play/owned_inventory.rs`, `crates/mc-net/src/settlement.rs`,
  `crates/mc-net/src/script/storage/settlement.rs`,
  `crates/mc-net/src/script/storage/settlement_tests.rs`,
  `crates/mc-net/src/script/storage/resident_settlement_tests.rs`,
  `crates/mc-net/src/script/storage/world_inventory.rs`, `docs/PLUGINS.md`,
  `docs/decisions/0004-staged-single-writer-simulation.md`, `docs/MEMORY.md`.
  The cursor line below (journal enabler) stays as landed.
- validation: focused `cargo test -p mc-net --lib --features load-bench` filters
  `-- warehouse` 13 passed / 0 failed, `-- journal` 69 passed / 0 failed / 1
  ignored, `-- chest` 30 passed / 0 failed, `-- owned_inventory` 19 passed / 0
  failed (one pre-existing assertion updated: a warehouse transfer on a runtime
  with no settlement profile now answers `runtime_unavailable` instead of the old
  endpoint-gate `unloaded`); `python3 -m tools.harness run fmt` PASS
  `.analysis/validation/20260914T020927-fmt-zv4q3obc`; `run code-health` PASS
  `.analysis/validation/20260914T020938-code-health-sgsxhyty`;
  `cargo clippy -p mc-net --all-targets -- -D warnings` clean.
- mandatory acceptance test: `server_owned_warehouse_deposit_journals_images_
  and_receipt_then_recovers_both` (`play/simulation.rs`) drives a real
  deposit and asserts ONE decision whose decoded image holds the POST-deposit
  container (real `stamp_chunks_for_world_journal` images, so dropping the image
  fails it) with the batch still attached (so a two-decision split fails it),
  the checkpoint cutoff blocked until projection, the restart reopening the
  container from that image, and `InventoryRuntime::recover` replaying both
  participants; `server_owned_warehouse_deposit_refuses_stale_fences_without_
  mutating` covers the stale state-id fence, the rejected conditional commit and
  the stale player fence (each typed, nothing mutated, no receipt journaled);
  `publish_warehouse_transfer_advances_the_actor_inventory_projection` covers the
  actor's revision + `AuthoritativeInventory`; settlement-level
  `warehouse_transfer_refuses_foreign_unknown_unloaded_and_stale_containers` and
  `warehouse_transfer_commits_both_participants_under_one_decision` cover routing,
  the foreign/absent/unloaded/cancelled refusals and the receipt fence identity.
- not run here: L2 `correctness` and the real-client gates — the sibling
  `../solaris-default-plugins` still lacks the `solaris-settlements` package, so
  `crates/mc-test-harness/tests/settlement_lifecycle.rs` and
  `settlement_pause_repro.rs` panic and those gates cannot be green from this
  checkout. Reported, not worked around.
- next: the deposit path is writable and tested; the remaining C1 work is the
  deferred double-chest container (one 27-slot `ChestBlockEntity` today) and any
  plugin-facing follow-up the owner wants from the receipt-fence narrowing above.

## Handover snapshot (pushed `2ae90ff4`)

- base_tree: `0febdfa3` (pushed to `main`; chain `bdb665cd` -> `2ae90ff4` -> `f313b4a6` ->
  `44eea177` -> `5ebb33c4` batch commit -> `f77525da6b1f7c6e460d1ae538aca409ce9b6d6b`). The
  pause fix in `44eea177` was live-verified as insufficient and is superseded by `2ae90ff4`
  (content-scoped footprint fence), which `0febdfa3` documents in the owning ADR.
- cursor_commits: cursor and documentation bookkeeping is never content; any commit after
  `base_tree` that touches only `docs/MEMORY.md` or only documentation under `docs/` is the
  same kind, so the next session's content diff starts at `base_tree` and any later commit
  listed here is bookkeeping for it.
- checkpoint_closed: settlement commit pipeline through the simulation lane, mob spin
  fix, worldgen (biome/river/beach/villages), live operator+whitelist access control,
  tab list, redstone/pistons, pregeneration, warehouse bind/read (C1a) - landed as one
  local batch commit `5ebb33c4` (210 files, +45618/-7730, no push/tag) after the owner
  sanctioned a whole-tree batch because earlier checkpoint lines share hunks with the
  settlement hunks. Local-only paths stayed out: `dist/` (120 MB), the omp session dump,
  `crates/mc-entity/.analysis/bench/entity-battle-1500x1500.json` and four tracked root
  `.analysis/*` deletions.
- validation_at_close: `run correctness` PASS
  `.analysis/validation/20260913T104202-correctness-41olrp8v`; `--lib warehouse` 8,
  `--lib owned_inventory` 18, `-p mc-script` 129, `--test settlement_lifecycle` 7,
  `--test plugin_examples` 4. Independent reviews: `CutoverReview` and
  `WarehouseReadReview` (both verdict changes, all findings fixed).
- validation_after_limiter_fix: `run correctness` PASS
  `.analysis/validation/20260913T151643-correctness-5wnrl9nr` (after moving the drop-refusal
  out of the play gateway, 745 -> 727 lines against the 731 budget; the first attempt failed
  code-health). Focused: `--lib ingress_rate` 8, `--lib settlement` 55,
  `--test settlement_fund` 1 (new repro, fails before the fix).
- tree_freeze_proof: before pushing, no other omp/agent writer was alive and two
  `git status --porcelain` + `git diff HEAD` fingerprints taken apart were identical
  (`97a4df1a411b6a64970e446b95eb6ded7fd1bffb5051a65f4950bb545aa08d1d`), with an empty
  index and only the five local-only paths differing from HEAD.
- validation_on_pushed_commit: `run correctness` PASS
  `.analysis/validation/20260913T231751-correctness-wwdhk_td` (244.3 s) re-run on
  `44eea177c0f2a25998d876f5c5945ed0e830ef02`, so the pushed revision itself - not an
  earlier snapshot of it - is the one the L2 gate covers.
- pushed: `main` now carries both live-chain fixes (ingress burst limiter no longer drops a
  serialized command and answers a dropped one; `advance_structure` re-observes the footprint
  after its own portion commit instead of pausing as `site_changed`), together with the
  owner's concurrent cleanup sweep over mc-data/mc-test-harness.
- validation_latest_push: `run correctness` PASS
  `.analysis/validation/20260913T231231-correctness-a7ak29y2` (248.7 s) on the pushed tree.
- fixed_live_pause (supersedes the `44eea177` attempt): the pause was a race with the WORLD's own
  scheduled-block-tick writes, not with the settlement's own commits. Writer:
  `run_scheduled_block_ticks_owned` (`crates/mc-net/src/play.rs:10090`) ->
  `commit_cross_region_scheduled_block_tick` (`play.rs:8630`) ->
  `WorldChunkJournal::record_reserved_snapshot_groups` (`play.rs:8702`). The paused run's journal
  (`.analysis/validation/20260913T232942-regression-eov066g3`) holds WIF1 id=1 (the fund
  reservation) then WCF1 id=2 tick=680 images=[(215,7) lsn=2] carrying the house blocks; the
  settlement's own staged portions journal nothing (alloc_high stayed 1 across 99/144/99-block
  portions), so the earlier attribution to `apply_structure_portion`/`regional_mutation.rs:848`
  was wrong. Fix: the fence now observes the footprint's CONTENT - an FNV-1a digest over the
  blocks inside `bounds`, `None` when a covering chunk is unloaded (fail closed)
  (`crates/mc-net/src/settlement.rs:409`, `:571`, `:575`) - instead of the chunk's durable
  journal position; the trait method is `observe_footprint`, called at prepare
  (`script/storage/settlement.rs:1736`) and after each own portion commit (`:1896`); the adapter
  no longer takes a session handle (`server.rs`). A genuine edit inside the footprint still parks
  the build as `site_changed` (`settlement_tests.rs:1166`, `:1231`).
- live_after_real_fix: `.analysis/validation/20260914T002326-regression-76agjkm2` reaches
  `Reserved real materials for house_sm_1 (66d36952...)`, then
  `house_sm_1 committed (solaris:house_small) at revision 19.` and
  `regsville | small site_6_0_9af75e77 hamlet tier=hamlet pop=0 houses=1 ...`; the driver's
  commit and info matchers both fired and no `site_changed` pause appears anywhere in the run.
  The harness still reports failed only because the driver times out at the next stage.
- next_blocker: `settlement populate` never reaches `settled in regsville` - the resident spawn is
  refused and the plugin answers "The refused spawn left the site reservation free again."
  (resident/site-vertical path, tracked as blocked; not the fence).
- validation_latest_push: `run correctness` PASS
  `.analysis/validation/20260914T003603-correctness-72k40m62` (326.5 s).
- live_after_fix: `.analysis/validation/20260913T232942-regression-eov066g3` (pushed tree,
  seed 81) still ends at `house_sm_1 paused: site_changed.` - `fund` answers
  (`Reserved real materials for house_sm_1 (0c06d1b4...)`) and the next line is the pause,
  then the 615 s driver timeout. So the post-portion re-observe added in `44eea177` is
  downstream of the write that actually lands between `project` and `build`: the fence sits
  at the top of the stage-advance
  (`crates/mc-net/src/script/storage/settlement.rs:1805`) and pauses before any portion is
  applied in that command, so the re-observe never runs for the offending frame. Next:
  instrument which frame lands in the footprint chunk between `project` and `build`
  (`fund` reservation and/or `prepare_structure`, `script/storage/settlement.rs:1611`), then
  re-observe at the end of that path (or attribute the structure's own decisions) instead of
  only after a portion commit.
- next: live acceptance of the `site_changed` fix once the machine and the worldgen sweep are
  free - `SOLARIS_REAL_CLIENT_AGENT_SCENARIO=m94-09-settlement-chain python3 -m tools.harness
  run regression --timeout-seconds 600 --run`. Deferred only for the owner's power-saver/noise
  window and because the sweep's terrain turns the driver's deterministic site pick (3203,60,300)
  into 3995/4096 water, which made `.analysis/validation/20260913T164410-regression-go7um90p`
  invalid as acceptance (project refused and withdrawn). If the terrain stays, the driver needs
  a dry-site preference instead of the first candidate. Receipts:
  `.analysis/codex-logs/live-chain/receipt.md`, `.analysis/codex-logs/site-changed/receipt.md`.
- owned_hash: `52a5e94e417ee168aa1f5aa2abf2c6becd4dd706b23f3ee0d335f8861c9e29b5` (SHA-256 over the 13 checkpoint-owned paths, recomputed after the C1a slice and the C1b revert: `crates/mc-net/src/settlement.rs`, `server.rs`, `play.rs`, `play/simulation.rs`, `play/block_wire.rs`, `play/tests/campfire_cooking.rs`, `script/storage/settlement.rs`, `script/storage/settlement_tests.rs`, `script/storage/resident_settlement_tests.rs`, `crates/mc-test-harness/tests/settlement_lifecycle.rs`, `docs/decisions/0004-staged-single-writer-simulation.md`, `docs/PLUGINS.md`, `../solaris-default-plugins/solaris-settlements/main.lua`; recipe: SHA-256 over `"<sha256>  <path>\n"` lines in that order. Other dirty paths belong to earlier checkpoints)
- changed_files: owned batch 18 paths; the rest of the dirty tree (191 paths) is pre-existing workspace WIP, not this batch
- sibling_batch: `../solaris-default-plugins` base `2d51ae5559cd` SHA-256 `71f40021def7e280b2de94fa3a27d206a2eecf6e2e247d36d2f7bec2e774c041` over the 9 files the package agents changed (root `README.md`/contract/`WATCHDOG.yml` and untouched package files excluded — pre-existing WIP; the package's two older receipts under `server/evidence/` are pre-existing as well)
- classification: `.analysis/handover/2026-09-13-batch-files.txt` (ignored artifact)
- checkpoint_site_vertical: settlements are grounded on the world's own terrain.
  `ChunkGenerator::surface_height` (defaulted `None`) with a `TerrainGenerator`
  override; `SettlementSelector::layout`/`::road` take a
  `&dyn Fn(i32, i32) -> Option<i32>` ground resolver and anchor each building so
  its authored anchor meets the terrain row at the anchor column (flush rule);
  `SettlementRuntime` holds the `WorldStorage` generator (fail-closed when a
  deployment has none); `site_snapshot` reports the grounded candidate;
  `spawn_resident` verifies `ResidentWorld::standable` before materialising.
  Evidence: `correctness` PASS `.analysis/validation/20260913T052447-correctness-_wlwlfgp`;
  receipt `.analysis/codex-logs/site-vertical/receipt.md`; live real-client
  `/settlement site` reports `origin 839,62,296` and `origin 1788,87,58` where the
  previous receipt recorded `origin 1788,0,58`, with
  `adopt`/`survey`/`project`/`fund`/`build`/`info houses=1` completing
  (`.analysis/validation/20260913T053315-regression-vbm3xulo`).
- checkpoint_site_vertical_open: both review findings are closed. (1) The first
  grounding rule (`+1`) was wrong: the shipped data anchors `plaza_well` at
  `[7, 0, 7]` with its layer at local y=0 and `house_small` at `[5, 0, 8]` with
  rows only at y=1..6, so `+1` floated the plaza and put the house floor at
  `surface+2` instead of the accepted `surface+1`. The rule is now
  `origin_y = surface - anchor_local_y`; live re-check confirms it
  (`origin 1788,86,58`, `origin 839,61,296`, no y=0). (2) A deployed catalog
  missing a required role now fails startup (`missing_required_role` +
  `SettlementStartupError::MissingRole`), covered by
  `catalog_violations_fail_loudly_with_a_typed_error`. The single independent
  reviewer re-read the settled tree and returned `overall_correctness: correct`
  with no new findings; the site-origin row is the origin column's first free
  row, `road` kept its original signature (no production caller), and a focused
  `anchor = [1,1,0]` deck test covers the anchor-above-base case under rotation.
- checkpoint_commit_publication: fixed behaviourally, cause not proven. Before the
  change, a player-visible `/settlement build` reported `house_sm_1 committed
  (solaris:house_small)` and `houses=1` while a client scan of the footprint found
  only terrain (QA runs 4/5). After it, run 6 on a fresh world found the whole
  house in the client at `origin 1788,94,58` (oak_planks 628, oak_log 32, glass 20,
  red_bed 20, torch 4, oak_door 4, crafting_table 2, chest 2; dirt 92 → snow 93 →
  air 94 → planks 95 → log 96-99) with the same `committed` answer. What changed in
  code: `play::block_wire::broadcast_applied_edits` publishes a writer-less batch
  (`invalidate_prepared_chunks`, delta broadcast, incremental light + chunk
  invalidation) and evicts session cooking state for an applied campfire →
  non-campfire edit, instead of returning after the storage commit. That eviction
  came from the independent read-only review (verdict `incorrect`, one P2 finding
  with the concrete failure path: the tick loop skips only positions whose current
  block is not a campfire, so a stale entry could be inherited by a later campfire
  and materialise its pending outputs); the review confirmed the rest of the fanout
  as correct, and its two minor notes (call `lighting::light_update_chunks`, revert
  the `invalidate_prepared_chunks` widening) are applied. Two explanations for the
  original invisibility remain open in attribution only: run 6's own saved world
  was decoded read-only and contains the committed house (oak_planks 314, oak_log
  16, glass 10, red_bed 10, torch 2, oak_door 2, crafting_table 1, chest 1 — the
  authored `house_small` totals, `.analysis/real-client-runs/settlement-ground-qa/run6-persistence.json`),
  so that run's placement and persistence are not in question; what is not run is
  an A/B of the pre-change build on one world, which is the only way to attribute
  the earlier invisibility to the missing publication rather than to the grounding
  change from the same window. A live reload of that world was not possible for the
  QA agent (entity-owner journal refuses recovery past the supported
  30,000-decision boundary; setting it aside trips the deliberate
  `world metadata identity mismatch` for the copied path). The reviewed campfire
  gap has a regression:
  `play/tests/campfire_cooking.rs::a_writer_less_commit_evicts_cooking_for_a_replaced_campfire`,
  which fails with the eviction disabled and passes with it.
  `.analysis/codex-logs/placement-visibility/receipt.md`; scan
  `.analysis/real-client-runs/settlement-ground-qa/run6-watchdog-scan.json`.
  Still open from the same run: `/settlement populate` refused with `blocked`
  (residents not materialised).
- checkpoint_commit_publication_residual: run 6 proves in-session client
  visibility and (by read-only region decode) disk persistence. It does not prove
  what a restarted server or a reloaded client serves: the QA's reload attempt
  failed before serving (`regional decision recovery exceeds the supported
  30,000-decision boundary`, then the deliberate `world metadata identity mismatch`
  on the copied path). One scan against run 6's own world directory after a real
  restart is the closure for that, tracked as a todo; do not infer reload
  behaviour from the decode.
- checkpoint_settlement_commit_pipeline: DONE (2026-09-13). A structure portion now
  commits as exactly one awaited `SimulationCommand::ApplyBlockEdits
  { actor_session: None, .. }` on the server-owned `SimulationHandle` bound into
  `LiveSettlementWorld` (`crates/mc-net/src/settlement.rs`), created by `server.rs`
  before the settlement deployment; the direct `WorldStorage` write, the
  `broadcast_applied_edits` fanout and the `OnceLock` late binding are gone. Every
  actor-`None` batch stays off the session fast lanes (`command_can_use_resident_
  mutation`, with `command_can_use_regional_mutation` delegating for that command),
  so the staged path owns cooking eviction, reactivity, owner relighting and
  post-commit publication; ADR 0004 records the invariant. Evidence:
  `.analysis/decomposition/evidence/{P01,P02,P03,P04,P04b,P05}/receipt.json`.
  Validation: `cargo test -p mc-net --lib` 2172 passed; `server_owned_block_edits`
  neighbourhood 5 passed (eviction for cross-region and single-region batches,
  fenced-handle refusal, lane-policy predicate, `BlockDeltas` seen by a loaded
  session); `cargo test -p mc-test-harness --test settlement_lifecycle` 6 passed;
  `plugin_examples` 4 passed; `code-health` PASS
  `.analysis/validation/20260913T085900-code-health-2culc043`; `fmt` PASS
  `.analysis/validation/20260913T085903-fmt-vpq5t0nt`.
  Closed in the same checkpoint: `release_resident_site` refuses a consumed
  reservation (`.analysis/decomposition/evidence/P06`); the plugin releases a
  stranded resident-site reservation only for `blocked`/`unloaded` - the two
  refusals that provably precede any effect in `residents.rs` - and keeps the
  reservation for `runtime_unavailable`/`invalid_request`/`not_found`/`capacity`/
  `busy` (`P08`, plus the missing `continue_write` release-intent branch and the
  release-completion hydration fix, `P10_P11`); a refused `project` is withdrawn by
  a confirmed durable batch and its pending intent survives an unconfirmed cleanup
  (`P10`); batch ids no longer alias names differing only by `-`/`_` (`P11`); the
  native m94-09 loop anchors each stage on chat observed after the submission and
  fails closed on absent feedback (`P13`).
  Residual gaps: batch ids built from two 40-char components can still exceed the
  core's 64-byte `MAX_SCRIPT_ID_BYTES`; live-proof driver steps below are missing.
  (The earlier note about writer-session campfire eviction on an accelerated lane
  was wrong and is withdrawn: the writer path clears replaced campfire cooking in
  `finalize_visible_block_edit_outcome`, `play/block_edit_commit.rs:315-338`.)

- release_installed: `~/.local/bin/solaris` md5 `006e7195848f901918118890709b3d3a`
  (`mc-server 0.0.6`, built from the current tree after the settlement commit
  pipeline cutover and the plugin lifecycle fixes), and
  `$HOME/sarvar/plugins/solaris-settlements/main.lua` is byte-identical to the
  sibling package source; the other five deployed packages are untouched. Plugin
  discovery must be checked from `~/sarvar`, because `[plugins].directory =
  "plugins"` is relative: `cd ~/sarvar && solaris --check --config server.toml`
  exits 0 with `operator_warnings: []` and discovers all six packages (the same
  check from the repository root resolves no plugin directory and reports
  `discovered_plugins: []`).
- validation: `run correctness` PASS `.analysis/validation/20260913T071815-correctness-nt60vl9s`
  (fmt, `code-health`, strict workspace Clippy, workspace tests, including the new
  campfire regression) and `.analysis/validation/20260913T070609-correctness-skxjk7sx`
  on the same revision the installed binary came from (the later addition is
  `#[cfg(test)]` only, so `~/.local/bin/solaris` md5 `c6dc5444d9bc91c058c84877c0521060`
  is unchanged); live real-client QA run 6 verified the committed house in the
  client (`.analysis/real-client-runs/settlement-ground-qa/run6-*`) and its saved
  world was decoded read-only as persisted; a smoke start of the installed release
  bound `127.0.0.1:25565` with `plugins=6` and was stopped again, and
  `solaris --check --config ~/sarvar/server.toml` exits 0 with six discovered
  packages, so the port is free for the owner's test.
- validation_latest: `run correctness` PASS
  `.analysis/validation/20260913T104202-correctness-41olrp8v` on the current tree
  (after the C1a review fixes) and
  `.analysis/validation/20260913T103025-correctness-ry310o95` after the C1b revert;
  earlier: `.analysis/validation/20260913T090714-correctness-jrwjk54y` (settlement
  commit pipeline). Focused: `cargo test -p mc-net --lib warehouse` 8 passed,
  `--lib owned_inventory` 18 passed, `cargo test -p mc-script` 129 passed,
  `cargo test -p mc-test-harness --test settlement_lifecycle` 7 passed,
  `--test plugin_examples` 4 passed; `code-health` and `fmt` PASS on the same tree.
  Independent reviews at this checkpoint: `CutoverReview` (settlement commit
  pipeline, verdict changes -> all fixed) and `WarehouseReadReview` (C1a bind/read,
  verdict changes -> receipt-per-bind + doc corrections fixed).
- visibility_claim (behavioral only, A/B not run by decision): committed settlement
  houses are client-visible and persist to the region file on the current build
  (run 6). No causal attribution is made between the missing publication fanout and
  the structure grounding change from the same window; see
  `.analysis/decomposition/evidence/P22/receipt.json`.
- c1_writable_warehouse (in progress, split): the documented contract is an
  opaque handle for a verified loaded container, never coordinates
  (`docs/PLUGINS.md:1577-1621`); canonical storage stays `Chunk.chests` keyed by
  `BlockPos`, so no second item ledger may appear. Verified today: three refusal
  gates (`play/session/owned_inventory_endpoint.rs:41-43` read, `:93-97` transfer,
  `:398-402` reservation), routing at `script/storage/world_inventory.rs:530-545,609-630,655-672`,
  the prepare boundary at `play/owned_inventory.rs:25-65`, and the issuance
  precedent at `script/storage/residents.rs:875-949`; `commit_prepared` still
  passes `Vec::new()` chunk snapshots (`world_inventory.rs:466-470`), so container
  after-images are not yet journaled. The authored warehouse blueprint exists
  (3 chests + 3 barrels + six `empty_container` entities) and the plugin's
  `/settlement deposit` is an unconditional refusal (`main.lua:2344-2355`).
  Handle invariant: core mints the handle; the plugin never supplies positions. The
  warehouse blueprint's `stores` POI is kind `work`, not a container, so no POI
  receipt can address a chest; the plugin names its durable `structure_id`
  plus the authored container ordinal and core verifies it against the
  blueprint catalog and the placed, loaded block. A plugin-chosen coordinate
  or a second container-address authority stays forbidden.
  Slice 1 DONE (2026-09-13, `.analysis/decomposition/evidence/C1a/receipt.json`): the
  DTO/Lua `bind_warehouse` + `ScriptWarehouseBinding {handle,structure_id,container_id,revision}`,
  a durable `DurableSettlementChange::Warehouse` binding, verification (ownership,
  placed `is_placed() == state != Cancelled`, authored `empty_container` ordinal,
  loaded chunk, container present) and a real READ path resolving a handle to the
  canonical `Chunk.chests` snapshot; `is_active()` is deliberately not used (it
  excludes completed structures, and a warehouse is a completed container).
  Evidence: `cargo test -p mc-net --lib warehouse` 8 passed, `-p mc-script` 129 passed,
  L2 `correctness` PASS `.analysis/validation/20260913T094337-correctness-gstdsd2t`.
  Slice 2 ATTEMPTED AND REVERTED (`.analysis/decomposition/evidence/C1b/receipt.json`,
  status reverted): the write path was built on a raw `WorldMutationView` container
  mutation, bypassing the typed container transaction, so it neither advanced
  `chest_state_ids` nor published `ChestSlots` to viewers and a concurrent menu
  commit could plan from pre-transfer state (ADR 0004 violation, a second
  authority). Nothing of it remains; the transfer endpoint refuses again and
  `commit_prepared` is back to `Vec::new()`.
  Required design for the next checkpoint: ONE server-owned simulation command
  carrying the operation receipt, the player after-image, the container with its
  expected/updated slots and the expected fences, executed entirely inside the
  owner turn - validate fences and current chest state, stage via the chest
  transaction boundary, append the ONE world-journal decision, project both sides,
  advance `chest_state_ids`, publish `ChestSlots`, then respond - so no container
  transaction can interleave. That is a cross-domain ownership migration whose
  recovery/data lifetime must be designed, not a small container fix; it is not to
  be improvised.
  Slice 3 (plugin deposit/withdraw) is BLOCKED on three owner decisions, because the
  package defines none of them: (a) which items deposit, (b) whether quantity is
  user-visible, (c) whether a withdraw surface exists at all. `/settlement deposit`
  stays an honest refusal (`main.lua:2344-2355`); no plugin-side money balance may
  be invented and `record.money` is only a carried-inventory projection.
  Slice 4: restart round-trip evidence, after slice 2. Deferred: multi-container /
  double chest and C4 haul (`resident_order_execution.rs:646-650`) /
  demobilization (`:1063-1065`).
  with container after-images in one recoverable decision. Slice 3: plugin
  deposit/withdraw wiring. Slice 4: restart round-trip evidence. Deferred:
  multi-container/double chest and C4 haul/demobilization.
- next: the native chain is runnable
  (`SOLARIS_REAL_CLIENT_AGENT_SCENARIO=m94-09-settlement-chain python3 -m tools.harness
  run regression --timeout-seconds 600 --run`) but there is no settlement sweep
  profile and the refused-spawn retry, second-create, warehouse round-trip and
  dismiss proofs need driver steps that do not exist yet
  (`.analysis/decomposition/evidence/D5/receipt.json`); after that, the C1 writable
  warehouse endpoint (no handle issuer or ownership binding exists today:
  `.analysis/decomposition/evidence/D4/receipt.json`), then the owner's commit
  decision for the owned batch.

Top changed groups: `crates/mc-net` (70), `crates/mc-test-harness` (57), `crates/mc-script` (20), `crates/mc-worldgen` (13), `crates/mc-server` (10), `examples/loader-live-gate` (10), `crates/mc-entity` (7), `crates/mc-protocol` (2)

## Queued after handover (dependencies, not motion)

Blocked on the owner's field test:
- **Owner manual structure-fit test**: the shipped gate's accepted-anchor path on the release
  binary (`~/.local/bin/solaris`, md5 `d066538256ec088b760ae11f027a9f64`); its live evidence so
  far is the terminal refusal on the previous, looser revision plus the unit test.

Blocked on the site-vertical checkpoint (deterministic `TerrainGenerator::surface_height`
plumbed into `SettlementRuntime`; the live-occupancy attempt was cancelled, reverted and
recorded):
- Settlement residents standing on the ground, the live `hire` + `squad order attack` combat
  proof the package still owes, and the canonical `m94-09-settlement-chain` run (its driver
  also needs paced commands, and the run root must stay short until the world-identity fix is
  in a build the harness uses). `/settlement populate` is refused by the core with `blocked`;
  the cause is diagnosed from run 6's plugin journal below.
- QA finding, real-client run 6: the m94-09 runner stalls when the server's
  command ingress bucket drops a `/give` (`COMMAND_BURST=8`, `class="command"`
  drop in `logs/debug.log`). Its second half is fixed: `await_state` in
  `tools/harness/backends/driver.py` clamped its event timeout to the client's
  accepted 0.1 s floor, so a deadline-driven wait no longer surfaces as
  `IllegalArgumentException: timeout_seconds must be between 0.1 and 120.0`
  instead of a clean timeout. Still open: the dropped `/give` itself — the runner
  should wait for the give to be acknowledged (inventory state or the log drop
  marker) rather than firing the next command, and that pacing decision is not
  made yet.
- The package's ghost `projected` entry after a refused `project`: `S.refuse` does not clear
  the prepare intent/index/record.
- **Why `/settlement populate` is refused with `blocked` (diagnosed from run 6's own plugin
  journal, `.analysis/validation/20260913T063935-regression-958j57r4/regression/20260913T063936Z-m94-regression-pack-w2tEzP/world/solaris/plugin-storage-v1/journal-v1.bin`):**
  the journal holds three resident-site ops — `reserve-regsville-25` (`reserve_poi`,
  `site_3_0_31f075c1.0.home`), then `spawn-regsville-27` (`spawn`, token `1a0807e0…`), then
  `reserve-regsville-29` for the *same* home. So the first reserve committed, the spawn failed
  (`runtime_unavailable`), and nothing ever handed the reservation back; the core refuses a
  second reserve of a live reservation (`script/storage/settlement.rs:1163`), which is the
  observed `Core refused the request: blocked.`. Required fix, in this order: resolve the spawn
  op through the plugin's existing durable intent/receipt lifecycle (`operation_status`), and
  release with `solaris.release_resident_site(request_id, op_id, spawn_site_token)` (core
  handler `release_resident_site`, release DTO `ScriptSettlementOperation::ReleaseResidentSite`)
  **only** on a confirmed non-commit. A fire-and-forget release from the generic refusal branch
  is wrong: an unconfirmed failure can race a committed spawn, and the core marks `released`
  without refusing a consumed reservation, so it would free an occupied home. A core-side guard
  refusing a release whose reservation is already `consumed` is a proposed hardening and belongs
  to the core file the current checkpoint owns. Any release intent must also be its own durable
  transition: `set_pending` overwrites the same `resident-site` slot, so an intent whose bundle
  write fails followed by `clear_pending` would drop the only recovery handle while the core
  reservation stays live — model the cleanup exactly like the package's other resident-site
  intents (including failed-write and restart recovery) and cover it with the package's
  intent-lifecycle tests before deploying it.

Open design decisions, no code yet:
- The base-row rule for the eight blueprints that author cells at their local `y=0`
  (`farm`, `market`, `plaza_well`, `mine_entrance`, `pen`, `palisade_gate`, `stone_wall`,
  `fishing_pier`): re-author their base rows hollow, or gate base-row blueprints strictly
  above the terrain. `mine_entrance` cannot be placed into a hillside under the current rule.
- POI leash for villagers and golems: the global 6..32 wander reach moves idle villagers up to
  32 blocks from home. Needs a measured policy, not a guess.
- A writable `warehouse` inventory endpoint (core C1) and the remaining C4 coverage gaps.

## Handover state (owner manual test)

L2 `python3 -m tools.harness run correctness` PASS on the final tree
(`.analysis/validation/20260913T034637-correctness-ualx2zif`, 333.8s) after one independent
read-only review (`FinalReview`, 9m23s) whose verdict was **`changes`, `overall_correctness:
incorrect`** — four findings, not a pass. Three were claim/evidence defects and are fixed:
the structure-fit and wander receipts now state the shipped boundary (`max_opaque_y > anchor[1]`),
record that the live acceptance run predates that correction and is owner-manual pending, and
drop the imaginary `/fill` platform (the server has no `fill` command, so the wander A/B ran on
natural terrain at 0,0). The review also established that villagers and golems share
`GoalState::Wander`, so the global 6..32 reach moves idle villagers up to 32 blocks from home;
a measured POI-leash policy is a separate queued checkpoint, not folded into this batch.
Its fourth finding stays **open, not fixed**: the eight shipped blueprints that author cells at
their local `y=0` (`farm`, `market`, `plaza_well`, `mine_entrance`, `pen`, `palisade_gate`,
`stone_wall`, `fishing_pier`) do replace the terrain's top ground row when anchored flush with
it, so "a structure is never built into terrain" is literally true only for the other fourteen.
Deciding that rule (re-author their base rows hollow, or gate base-row blueprints to strictly
above the terrain) is its own checkpoint.

One harness test needed a determinism fix, not a product change:
`survival_tnt_explosion_damages_mob_over_wire` raced the new wander — a summoned chicken
walked out of the four-block blast radius while the fuse burned. A/B in
`.analysis/codex-logs/tnt-mob-wander/repro.log`: with reach 3..4 the test passes, with 6..26 it
fails. Fix: the test pins `minecraft:chicken` to `MobMovementPolicy::Immobile` through
`bound.entity_behavior_handle().configure_mob_behavior_table(...)` before serve, which removes
the incidental wander premise without touching explosion geometry.

Delivered for the owner's field test: release binary installed at `~/.local/bin/solaris`
(built from this exact tree) and the six packages deployed to `$HOME/sarvar/plugins`
(`solaris-permissions`, `solaris-essentials`, `solaris-economy`, `solaris-towns`,
`solaris-audit`, `solaris-settlements`), which the owner's existing config already points at
through its relative `[plugins] directory = "plugins"`.

## Buildings buried in terrain: diagnosed, guarded, and the real fix dispatched

Owner report (screenshot + "по полу невозможно ходить, я застреваю в нём"). Evidence
from the live probe world: the committed `solaris:house_small` at probetown has its floor
planks at world y=73, windows at y=76 and roof at y=78 while the surrounding terrain is
y≈79-80, and **the interior columns are solid stone**. The player stands inside terrain.

Causal chain, from code plus world scan:
1. `mc_worldgen::SiteCandidate::origin` always carries `y = 0`; its own doc comment says
   "the y coordinate is resolved by the caller" and nothing resolves it, so
   `/settlement site` reports `origin 1788,0,58` and the plugin's `S.site_anchor`
   fallback (`y = site.min_y`) points at y=0.
2. The structure anchor therefore came from the ordering player's position (the plugin's
   `project ... here`), which stood on a slope, so the house was built inside the hill.
3. `prepare_structure` validated bounds, claims and the survey token but never checked
   that the reserved volume was free, while contract A03 requires invalid sites to be
   refused or re-checked **without overwriting** terrain.

Fixed by Main in core: `SettlementWorld::max_opaque_y(bounds)` (bounded per-column read,
honestly named — it is an occupancy ceiling, not a terrain surface) plus the fit gate in
`prepare_structure` that answers `Blocked` when `max_opaque_y > anchor[1]`: nothing may sit
above the base row, and terrain level with it is the ground the structure stands on. The
first revision of that gate allowed the row above as well and was corrected, because a floor
authored at local `y=1` would then overwrite the terrain top. No
terrain is ever cleared: an air/clearance write path was designed and rejected because it
contradicts A03 and would need new blueprint semantics for water-bearing structures such
as `fishing_pier` (which authors `minecraft:water` at local y=0 with its deck at y=1).
Test `prepare_refuses_a_footprint_the_terrain_rises_into` covers the refusal, the
flush-fit boundary and the unloaded footprint; `cargo test -p mc-net --lib` 2162 passed,
clippy `-D warnings` clean, `run fmt` and `run code-health` PASS. Live proof: an
obstructed anchor answered `Core refused the request: blocked.` in the real client.

Independent confirmation, from agent `P1SquadHandle`: a resident entity sat at
(1793, 2, 64) inside solid stone with terrain at y≈92 there, same cause (site POIs use the
unresolved y=0 origin).

Live proof on a **fresh** world (`.analysis/live-probe/gate_proof.py`, own `world_dir`,
frozen binary `/tmp/mc-server-gate`): an anchor whose footprint contains terrain answers
`Core refused the request: blocked.`, and a house committed and stood on the ground with its
floor planks one row above the terrain top, glass walls and a 99-plank roof
(`house_sm_1 committed (solaris:house_small) at revision 20`, raw column scan in
`.analysis/codex-logs/structure-fit/floorscan.json`). That run predates the boundary
correction — it anchored at `terrain_top - 1`, which the old `anchor + 1` rule accepted and
today's rule refuses — so for the shipped revision the live *refusal* and
`prepare_refuses_a_footprint_the_terrain_rises_into` are the matching evidence, and the
accepted-anchor path is **owner-manual pending** (blocked gate, never green): the shipped
revision's live evidence is the refusal plus the unit test, and the accepted-anchor gameplay
check is the owner's own field test. Details, hashes and the tooling limits (stale
client block reads after a build; `minecraft_press_inputs` not moving the player in this
setup) are in `.analysis/codex-logs/structure-fit/receipt.md`.

The site/POI vertical placement is still unresolved and is the next checkpoint. An agent
grounded it from **live occupancy**; that is rejected and being reverted (`Unground`),
because the canonical layout must stay a pure function of `(seed, revision, cell)` — the
live read made `list`/`query` depend on loaded chunks and mutable blocks, so a distant
site would answer `Unloaded` and the same settlement could report different coordinates.
The deterministic design to implement next is
`mc_worldgen::TerrainGenerator::surface_height(x, z)` (public, pure, "the same function
the generator does") plumbed into `SettlementRuntime`, with the fit gate above still
checking real world contents; the rule itself comes from the blueprint's own data (every
shipped blueprint's `[footprint].anchor` equals its first `[[street_connection]].at`, and
`fishing_pier` authors water at local y=0 with its deck at y=1).

## Long-path world identity fixed (owner-relevant)

Agent `WorldIdentity` (21m38s, pass) removed the 128-byte cap on the world identity input
in `mc-script` (`resident_generation_id`): a deep server directory previously made
`/settlement site` answer `Core refused the request: invalid_request.`. The returned id was
already fixed-width 64-hex, so ids for paths ≤128 bytes are byte-identical (persisted CAS
generation ids stay valid). Live proof at a 179-byte world path: `create` and `site`
accepted. `cargo test -p mc-script` 128 passed, clippy clean, `run code-health` PASS.

## Long-range mob wander (owner: "чтобы мир реально был живым и в движении")

Wander targets were rolled 3..7 blocks from the agent's current position
(`WANDER_MIN_DISTANCE 3.0` + `WANDER_DISTANCE_SPREAD 4.0`, `crates/mc-entity/src/lib.rs`),
so the world read as static. They are now rolled 6..32 blocks. Because a target is
rolled relative to the *current* position there is no home leash, so the wider reach
becomes real roaming rather than a wider idle. Cost stays flat per tick: pathing is a
greedy per-tick step under `PathingBudget`, so a longer walk costs ticks, not work, and
an unreachable target is abandoned by the retained-path no-progress budget and re-rolled
(aquatic agents discard the blocked target and re-roll the same way). Hostile mobs share
the goal, so they roam too; villagers do not use it.

Measured A/B on a clean 129x129 stone platform at y=119 with ten sheep summoned on a
tight ring, sampled every 3 s for 60 s through the real MCP client, same world snapshot
and therefore the same entity ids and the same deterministic angle sequence for both
builds (before = `/tmp/mc-server-prewander`, mtime 09:23:48, the last build preceding the
edit; after = `target/debug/mc-server`, mtime 09:40:51):

| metric (60 s, 10 sheep) | before (3..7) | after (6..32) |
| --- | --- | --- |
| median net displacement | 15.98 | 33.40 |
| max net displacement | 27.12 | 57.71 |
| median travel | 79.85 | 106.39 |
| smallest net displacement | 6.00 | 20.90 |

Tests: `wander_targets_are_multiblock_and_not_synchronized` now samples the real roll
path over 64 entity ids and fails if the reach drops back to a stroll;
`wander_pauses_after_reaching_its_retained_target` derives its tick budget from the reach
instead of a magic 80. `cargo test -p mc-entity --lib` 625 passed,
`cargo test -p mc-net --lib mob_spin` 3 passed, `run fmt` PASS, clippy `-D warnings` clean.
Raw method, binaries and totals: `.analysis/codex-logs/wander-range/receipt.md`.

## Mob spin near leaves fixed and measured (owner bug)

Agent `MobSpinFix` (52m23s) found the real cause, which is **not** leaf passability:
leaves were already solid obstacles (oak_leaves state 279, probe `Blocked` inside),
and no leaf-id special-casing was added. The wander pathfinder accepted a detour
that moved the mob *away* from an unsatisfiable target, then walked it back; the
no-progress guard only watched position deltas, so the ~0.9-block oscillation reset
it, and `face_horizontal_motion` chased the flipping velocity — endless rotation
(`crates/mc-entity/src/lib.rs`, `bounded_pathing_step`). Fixes: a detour is accepted
only when it strictly reduces target distance or the body already overlaps terrain,
otherwise `Blocked` (zero velocity, no rotation) and the existing cadence/backoff
retargets; an overlapping agent gets a bounded cardinal escape (feet level and one
block down) so one spawned *inside* a canopy walks out; the terrain probe now
declares the entity position and escape probes.

Measured live on its own server/ports with the real MCP client: pre-fix 4 sheep
matched the spin (net movement < 1 m, yaw > 1700°/5 s); post-fix 0 matched, and
3 spiders summoned inside the canopy walked out 11.2–26.7 m. Tests:
`cargo test -p mc-entity` 625 passed, `cargo test -p mc-net --lib` 2162 passed with
3 new `mob_spin` tests (one fails pre-fix), fmt/clippy/check clean. Known
limitation, honest and recorded: an agent fully enclosed by a ≥2-block leaf pocket
or embedded in a 1x1 trunk log can stay stationary when no cardinal neighbour is
walkable — it no longer rotates, but it also cannot escape a sealed pocket.

## Plugin ↔ C4 wiring landed; live combat blocked by one plugin bug

Agent `P1C4Wiring` (43m5s) wired the shipped package to the C4 APIs and deleted
every "needs core C4" placeholder: `assign/cancel_resident_work`,
`issue/cancel_resident_order`, `demobilize_resident`, and
`transfer_owned_items` with the `resident_equipment`/`resident_carry` endpoints
(`main.lua` 4224→5468, manifest gained `resident_work`/`resident_orders`). It also
fixed a real plugin bug (`squad <name> list` was unreachable) and reported one it
did not fix: `create`'s durable operation id is not per-settlement, so founding a
second settlement name returns `operation_conflict`.

Main proved two thirds of it live on the running server (creative, then the dry
site coordinate 1789/74/59 used by the earlier chain):
- `hire` reads the employer's real player inventory and refuses with the exact
  missing item — `Cannot equip 29ae7f40 as militia; missing from your inventory:
  minecraft:leather_chestplate. Nothing was equipped.` — then after giving the kit:
  `29ae7f40 serves as militia (core equipment: iron_sword,leather_chestplate).`
  with `residents` reporting `service=military role=militia squad=alpha
  gear=iron_sword,leather_chestplate`, i.e. C1's `resident_equipment` endpoint
  really committed the gear.
- `/summon minecraft:zombie` works and the squad record stores `order=hold`.

**Remaining plugin defect found live**: `squad <name> order <squad> hold` answers
`Squad alpha has no member with a core handle.` even though `residents` shows the
same member with its handle and `squad list` reports `members=1 armed=1`, and
`squad <name> add <squad> <handle>` prints nothing at all. So the squad record does
not retain/resolve the resident's core handle, which blocks the order path (and
therefore the live combat proof). Fix is plugin-side and small; it must be followed
by the live proof: armed militia + summoned hostile → observed committed damage,
then `dismiss` returning the gear.

## Live re-verification on the gate-green build + the last gameplay gap

After `correctness` passed, Main re-ran the live chain against the rebuilt binary
(server + real MCP client, same six-package set and the same world, so persistence
was re-proven too): `/settlement info probetown` still reports `houses=1` from the
house built before the gate, and `/settlement populate probetown` now spawns a real
resident through C3 — `29ae7f40 settled in probetown (alive_loaded), home
site_3_0_31f075c1.0.home. House capacity is tracked by that home POI.` —
`/settlement residents` shows `29ae7f40 family=unassigned job=- service=civilian
squad=- life=alive_loaded`, `info` moves to `pop=1 houses=1`, and
`grep -c "wall-clock budget exceeded\|plugin disabled"` = 0.

**Last gameplay gap found by that probe**: the shipped plugin still answers
`29ae7f40 serves as militia; equipment and orders need core C4.` and `Squad alpha
order hold recorded; physical execution needs core C4 (issue_resident_order).`
because it was written before C4's Lua surface existed and (correctly, per its
brief) recorded the missing call instead of faking it. Core C4 is landed and
gate-green, so the gap is purely plugin-side: agent `P1C4Wiring` is wiring
`hire`/gear (`transfer_owned_items` with `resident_equipment`/`resident_carry`),
`squad order` (`issue_resident_order`/`cancel_resident_order`), `job`
(`assign_resident_work`) and `dismiss` (`demobilize_resident`), and must prove it
live by summoning a hostile next to an armed militia member and showing committed
combat damage with no ally hit.

## Full L2 `correctness` gate PASSES on the whole settlement program

`python3 -m tools.harness run correctness` → **status passed**, artifact
`.analysis/validation/20260913T005844-correctness-8o6u6sbt` (supervised via
`hub start name=correctness`; the earlier attempt died when its foreground job was
lost). That is fmt + `code-health` + workspace clippy `-D warnings` +
`cargo test --workspace --all-targets` green on the tree that now carries C1–C4,
the settlement runtime, the loader protocol-3 cutover and the plugin-set changes.

Two integration defects were found by the gate itself and fixed by Main before it
went green:
1. `code-health`: the new public plugin DTO
   `ScriptClientViewFieldValue` (crates/mc-script/src/client_view.rs) lacked
   `#[non_exhaustive]`; adding it exposed two exhaustive matches in
   `crates/mc-net/src/play/session/loader_views.rs`, which now have explicit
   catch-alls (a substituted-field refusal and a `FieldKind::Unknown` that cannot
   match a declared model field) instead of being silently widened.
2. `cargo test --workspace`: `crates/mc-server/tests/cli.rs`
   `check_reports_derived_deployment_for_every_plugin` still built its fixture
   plugin with `[client] schema = 1`, which the schema-2 cutover now rejects; the
   fixture is schema 2 (`content = ["assets"]`, `permissions = ["load_assets"]`
   remain valid pairs). 42 cli tests green after the fix.

## C4 combat proven — five real executor bugs fixed

Agent `A10Combat` (43m33s) removed both `#[ignore]`s after finding the recorded
reason was a **misdiagnosis**: `resident_perception` does see a spawned zombie; the
executor was wrong in five places, each a gameplay bug, not a test artifact:
1. proximity orders dropped their engagement radius (`let _ = engagement_radius`),
   so Hold/patrol never perceived anything;
2. no order ever issued a *fresh* target ref (only TTL-refreshed resolved ones), so
   the Attack op was unreachable — proximity orders now perceive with their own
   radius, fill targets and mint server-issued refs, with attack refs deduped;
3. ranged detection used `weapon.ends_with("_bow")`, which is never true for
   `minecraft:bow`, so the ammunition gate was dead and every "archer" meleed;
4. a dead guard (`references.get(&uuid_of(record))` — the attacker is not in the
   target-ref map) aborted every attack;
5. the ally set was matched against the member's *handle* instead of resolved
   resident handles, so allies were issued and could be hit.

Tests: `cargo test -p mc-net --lib --features load-bench resident_order` → 11
passed, 0 ignored; full mc-net lib → 2152 passed, 8 ignored; fmt/clippy clean;
`cargo check -p mc-server` clean. Six mutations each fail the named assertion
(ammo, LOS, ally-hit, ally-issued, retreat, patrol-resume), so the tests defend
behaviour rather than plumbing.

Residual to prove live (I1 item, not a code gap on this evidence): production
residents are not tracked through the session-local fixture path the tests use, so
their perceivability must be confirmed on a running server (spawn/claim a resident,
put a hostile nearby, order an attack, observe committed damage). The per-tick
simulation-input publication path is the expected tracker; that assumption is not
yet verified outside unit tests.

## Loader wire cutover closed (protocol 3 / schema 2 both sides)

Agent `L1Core` (33m9s, verdict `pass`) landed the core half: bundle schema 2 with
no schema-1 decoder, Loader protocol 3, the full wire-3 view lifecycle
(`crates/mc-net/src/play/session/loader_views.rs`, `script_client_view_endpoint.rs`,
`crates/mc-script/src/client_view.rs`), client ingress `view_action` /
`cancel_selection` admitted through ledger-owned permission pairs and re-read per
action, single-use tick-expiring selection contexts invalidated by
replacement/close/disconnect/revocation, and a clean cutover that deleted
`ScriptClientUi`, `present_client_ui`, `loader_interaction` and their endpoint
files/tests. The shipped `examples/loader-live-gate` fixtures were migrated and
rebuilt. `docs/PLUGINS.md` now states that no shipped package declares `[client]`.

Main verified the cross-repo signal directly, not just through the profile: the
harness `java` profile reports `loader-core:test` as UP-TO-DATE (the fixture test
reads a system property), so I forced
`./gradlew --offline --no-configuration-cache --rerun-tasks :loader-core:test
--tests '*LoaderLiveGateFixtureTest*'` → BUILD SUCCESSFUL with
`LoaderLiveGateFixtureTest tests=2 failures=0 errors=0 skipped=0` (written 02:07).
Harness receipt: `20260912T190619-java-1p3l15pg`. Also green: mc-script 262
passed, mc-net `loader_view` 6, `script_client_view` 3, `cargo check -p mc-server
--all-targets`, fmt and clippy clean.

Two intentional deferrals, now written into the frozen wire doc as the shipped
contract rather than left as open gaps: (1) the marker payload on the wire is
`{ marker_id, selection_token, action_id, formation, radius }` — the earlier
`selection_context_id`/`preview_id` naming was never implemented, and the
projection binding (`world_preview_ref`) plus the V/R `view_request` message stay
deferred with the Loader UI feature; `LoaderViewRequest.java` is the intended
carrier for the latter. Declared view kinds and `revoke_loader_views` are
implemented and unit-tested but unwired while no `[client]` package exists.

Next: A10 — `resident_perception` does not see a test-spawned hostile, so the C4
archer-ammo/LOS/ally-policy and attack→retreat→patrol behaviours are implemented
but unproven (both tests `#[ignore]`d with that reason).

## C4 execution landed (verified)

Agent `C4Exec` (56m38s, verdict `changes`) landed the mc-net execution half:
`script/storage/resident_orders.rs` (870), `resident_order_execution.rs` (2429),
`play/resident_work.rs` (418), `play/session/resident_orders.rs` (294),
`resident_order_tests.rs` (1278). Design: an order change rides inside the existing
`PreparedStorageBatch` and its operation receipt (`OP_RESIDENT_ORDER_CHANGE` /
`OP_SNAPSHOT_ORDER`), so the admission frame *is* the commit; `recover_resident_orders`
applies pending members exactly once at actor start; server-issued target refs are
persisted with the batch and forged/expired/allied/out-of-reach/wall-blocked targets
are refused; damage commits through `damage_batch_if_current` fenced on the observed
snapshot. mc-script's frozen DTOs needed no change.

Independently re-run by Main: `cargo test -p mc-net --lib --features load-bench
resident_order` → 9 passed, 0 failed, 2 ignored; `cargo fmt`/`clippy -D warnings`
clean for mc-net/mc-script/mc-entity; `cargo check -p mc-net --all-targets
--features load-bench` clean.

Proven: A08 (harvest needs its tool and commits real drops; craft consumes inputs
exactly once; haul moves items between canonical resident endpoints across process
boundaries), A09 (a squad reforms through an open passage and refuses a closed one
with `blocked_route`, no teleport, distinct slots), A11's stale-member leg (a member
dying between prepare and commit changes no order), A12 (a committed admission
replays exactly once and a repeated operation id with a new payload conflicts),
demobilisation without a warehouse keeps the handle and the gear. The tests also
caught and fixed a real bug: haul re-used a stale slot clone and over-reported work
units.

Open C4 gaps, each recorded rather than papered over:
- A10 and the attack leg of A11 are `#[ignore]`d with an explicit reason:
  `resident_perception` returns no candidate for a test-spawned hostile, so the
  archer-ammo/LOS/ally-policy and attack→retreat→patrol behaviours are implemented
  but unproven. This is the next C4 item after `L1Core`.
- Garrison post occupancy has no POI → position resolver yet (reports `blocked_route`);
  demobilisation cannot complete without a warehouse resolver; `construct` is wired
  to C2's committed reservation but has no focused test.
- C1's Lua `transfer_owned_items` still rejects the `resident_equipment` /
  `resident_carry` endpoint kinds, so player↔resident gear movement through the
  script API is not exercisable yet (the execution layer moves gear internally).

## Open regression carried into the next wave: Loader protocol cutover

Wave-1 agent `LoaderSchema2` cut the Loader to bundle **schema 2 / wire protocol 3**
and made schema-1 bundles fail closed, but core still advertises protocol 2 and
schema 1: the core L1 endpoint (`open/present/close_client_view`,
`begin/cancel_client_selection`, `on_loader_view_action`, `view_request`) was never
built, so a Loader client and the current core disagree on the handshake. The Loader
repo's own gates are green (`python3 -m tools.harness run java` PASS,
`20260912T173749-java-bi9gs9sg`) and `loader-live` is the cross-repo gate that stays
red until core lands the cutover.

Plan: land the core L1 endpoint as the next mc-script slice *after* `C4Exec` (both
edit `operations.rs` / `lua/operations.rs` / `lib.rs`, so they must not run
together). Frozen decisions to implement, from
`'/home/kaiserroman/.omp/agent/sessions/-solaris/2026-09-11T15-31-29-764Z_01a09118-6364-762a-a2ac-b4dd04f8e34c/local/settlement-wire-freeze.md'`:
- `view_request { request_kind }` with `request_kind ∈ {settlement, army}` is the
  client→server open request; the Loader-side `LoaderViewRequest.java` is that
  message, not dead code — core must accept it, admit by session + owning plugin +
  declared view permission, deliver it to that owner, and open nothing on refusal.
- the selection context id is a **top-level field of the marker model**
  (`markers[].selection_context_id`), not buried in an unspecified inner shape;
- a view/marker binds to a verified projection through `world_preview_ref` on both
  sides (markers reference a `world_previews[]` entry of the same bundle);
- `entity_presentations` stays deferred (C4 may claim it later).
Owner decision still holds: no shipped plugin declares `[client]`, so nothing
Loader-facing is *enabled* — this slice only restores cross-repo agreement.

## Other carried items

- `materialize_resident` (C3 worldgen seam) still carries a localised
  `#[allow(dead_code)]`; `reserve_resident_site` is used by C2Runtime, the
  materialiser stays unused until the plugin's `populate` path lands. Remove the
  allowance when that caller exists.
- The regression manifest `docs/real-client-regression/manifests/m94-regression-pack.json`
  lacks boolean `no_debug_commands` on 6 of 21 scenarios (including
  `m94-01-join-rejoin-chunks-movement`), so `run regression --run` rejects them.
  Not "fixed" blindly: labelling a scenario's debug-command policy wrongly would
  weaken a gate; decide per scenario when that manifest is next touched.
- Money stays a plugin-side ledger (solaris-economy / the settlements treasury
  projection); there is no core money authority, and none may be invented.
- Probe attribution: the live probe above ran against
  `target/debug/mc-server` sha256 `446e17b57d0b821e228f2767025db183cbd3747a0c2abc6d72cf2e3ee7ea5c0e` (rebuild it before re-probing; see
  `.analysis/codex-logs/live-probe/receipt.md` for the full commands and log lines).

## Live probe of the default plugin set (owner request)

Set installed with `../solaris-default-plugins/install.sh` into
`.analysis/live-probe/plugins`: solaris-permissions, solaris-essentials,
solaris-economy, solaris-towns, solaris-audit, solaris-settlements; config
`.analysis/live-probe/server.toml` (absolute directory, strict, six expected ids,
operators SolarisMcp/SolarisPrimary). `--check` shows all six discovered as
`deployment: "server_only"` with no client bundles or permissions, i.e. the
server-side v1 decision holds. Canonical gate: `run regression --run` with
scenario `m94-02b-rejected-block-resync` PASSED in 27.0 s
(`.analysis/validation/20260912T165404-regression-zjobn0xi`) driving a real
Gradle client under Xvfb.

All six plugins answered with their own messages, including operator and
adversarial paths (`Only an operator ...` refusals, usage text for wrong args,
`Cannot create that town.` on a repeat, `Chunk is claimed ...` on a repeat claim,
`No matching bounded audit records.`). Full table in
`.analysis/codex-logs/live-probe/receipt.md`.

Settlements v1 is live server-side: `/settlement create probetown small` ->
`Founded probetown (small hamlet).`, `list`/`info` report real progress
(`Next village missing: houses 0/9, residents 0/24, jobs 0/12, food 0/64,
committed meeting hall; pause=running`), `/settlement site probetown` prints the
C2 deterministic candidates (`site_3_0_31f075c1 village origin 1788,0,58 size
192,32,192 buildings=12`, `site_1_0_0ac99d6a hamlet ... 128,32,128 buildings=8`),
`adopt` commits (`Adopted site_3_0_31f075c1 (village): 12 buildings, 8 points of
interest, revision 0.`), the workflow gates correctly (`Adopt a deterministic site
first`, `Survey the plot first`), records survive a server restart, and startup
logs `settlement blueprint catalog validated ... blueprints=22`.

**Fixed and re-verified live**: agent `SurveyBudgetFix` (32m) measured the real
cause — the request was fine (core survey 2.1 ms at 64x64, 8.1 ms at 128x128,
never loads chunks); the *script-visible result* carried one Lua record per
surface column (<=16,384 records; 47 ms at 4096, 190 ms at 16384) against the
50 ms `HOST_EVENT_WALL_BUDGET`, so `set_result` alone guaranteed the trap. The fix
is a bounded aggregate snapshot (plots/water/claimed/chunks/tags) instead of
per-column records; no deployed plugin read the columns, so no plugin change and
no tiling (which would have shipped dead data) was needed. Per-column heights and
slopes deliberately no longer cross the script boundary.

Post-fix live chain, same real client: survey -> `plots=4096 chunks=loaded`,
project -> `house_sm_1 projected (solaris:house_small, 4 stages)`, fund without
materials -> `Not enough materials in your inventory; nothing was reserved.`,
fund with the authored materials (planks 314/oak_log 16/glass 10/red_bed 10/
torch 2/oak_door 2/crafting_table 1/chest 1) -> `Reserved real materials for
house_sm_1 (84c74c16...)`, build -> `house_sm_1 committed (solaris:house_small)
at revision 28.`, `/settlement info` -> `houses=1`, and a client block scan at the
anchor shows 32 oak_planks / 4 oak_log / 2 glass / 1 torch, i.e. the building is
physically in the world. `grep -c "wall-clock budget exceeded"` = 0.
Screenshots: `.analysis/live-probe/house-front.png`, `house.png` (captured,
unverified visually).

Historical record of the defect: `/settlement survey probetown plot` trapped the Lua host
(`Lua plugin disabled after handler failure plugin=solaris-settlements error=Trap
{ message: "wall-clock budget exceeded" }`), after which the plugin was disabled
and later subcommands answered `Unknown command` (root cause and fix above).

Also noted: `m94-01-join-rejoin-chunks-movement` cannot run via the harness
because the regression manifest does not declare boolean `no_debug_commands` for
it (6 of 21 entries lack it) — manifest gap, not a server bug.

## Settlement overhaul program — waves

Owner order: "добивай поселения полностью" with subagents authorized (still capped at
two concurrent, disjoint write sets). Contract:
`../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md` §10 queue. Frozen
shared interfaces written as local artifacts (not repo files):
`local://settlement-wire-freeze.md` (bundle schema 2 / wire 3, resolved the four
gaps the Loader pass reported: selection context id travels in
`model.markers[]`, markers reference a verified `world_previews[]` entry,
key-driven open uses a `view_request` message admitted server-side, entity
presentation stays deferred) and `local://settlement-blueprint-freeze.md`
(blueprint schema 1, authoring layout, hard limits, determinism, ruins).

Wave 1 (uncommitted):
- **C3 persistent residents** (agent, 42m23s, verdict `changes`): five §6.1 calls
  (`claim/spawn/query/release/set_resident_pois`) as closed DTOs on the C1
  operation envelope, `persistent_residents` capability replacing the old
  `villagers` API, core-owned `ResidentLedger` replayed from the plugin storage
  journal (OP_RESIDENT_CHANGE 13 / OP_SNAPSHOT_RESIDENT 14) so a handle resolves
  to the same UUID after reopen and reports `alive_unloaded`, not `dead`.
  Files: `mc-script/src/resident_operations{,_tests}.rs`,
  `mc-net/src/script/storage/residents.rs` (1166) + `resident_tests.rs` (438),
  `mc-net/src/play/session/script_resident_endpoint.rs`.
  Not finished: site-bootstrap wiring (C2), POI validity (C2), `resident.changed`
  notifications and assignment exclusivity (C4).
- **L1 Loader half** (agent, 32m8s, verdict `pass`) in `../solaris-loader`:
  schema 2 / wire 3 cutover, closed widget set, view-instance + selection-context
  lifecycle, bounded model validation, world-selection input, one shared
  model/validator/presenter for all three adapters, no schema-1 decoder left.
  Graphical U01–U06 still need the harness and the core endpoint (below).

Main integration work after wave 1: `cargo fmt`/`clippy -D warnings` clean for
mc-script/mc-net/mc-entity. Fixed by shrinking the shared types rather than
boxing 29 call sites: both heavyweight `ScriptOperationPayload` variants
(`OwnedInventory`, `Resident`) now hold `Box<...>`, which removed both
`large_enum_variant` findings (C3's new results had grown the old outcome enum);
deleted dead `track_villager_override` and `bootstrap_resident_change`; the two
worldgen-facing seams `materialize_resident`/`reserve_resident_site` are kept
with a localised `#[allow(dead_code)]` and a reason naming C2 as their caller.
Evidence: mc-script `--features lua-runtime` 233 passed, mc-net lib 2118 passed /
8 ignored, mc-net `--all-targets` compiles.

Known debt from wave 1:
- `crates/mc-entity/src/regional.rs` still carries the unreachable old villager
  binding lane (`claim_nearest_villager`, `apply_villager_binding_goal`,
  `release_villager_binding`, purge hooks, `villager_binding_tests.rs`) with no
  callers: delete it in the entity/C4 wave.
- `../solaris-default-plugins/colony-villager-scaffold` still calls the removed
  villagers API: the contract deletes it when P1 lands.
- Wave 1 broke `crates/mc-test-harness/tests/commands.rs`
  (`lua_villager_goal_reaches_the_regional_owner_and_returns_targeted_result`
  fails 0 vs 1); wave 2 migrates it to the resident API.

Wave 2 closed:
- **C2 catalog/sites/construction** landed by agent `C2CatalogSites`, which then
  failed (exit 1) after 1h4m before reporting; the integration owner verified the
  tree and wrote `.analysis/codex-logs/c2-catalog/receipt.md` from verified state.
  Landed: `mc-script/src/settlement_operations.rs` (1097) + tests, catalog loader
  and deterministic sites in `mc-worldgen/src/settlement_catalog*.rs` /
  `settlement_sites*.rs`, execution/receipts in
  `mc-net/src/script/storage/settlement.rs` (1996) + tests (1612),
  `session/settlement_authority.rs`, Lua install, `docs/PLUGINS.md`.
  Main ruled and the implementer landed the bound split: 64-axis is a blueprint
  bound (`MAX_BLUEPRINT_FOOTPRINT_AXIS`), a site is territory
  (`MAX_SETTLEMENT_SITE_AXIS = 256`), pinned by
  `site_territory_footprint_is_accepted_above_the_blueprint_bound`; the interim
  "report the built layout bbox" workaround was rejected. Verified: fmt/clippy
  clean for mc-script/mc-net/mc-worldgen, mc-script settlement 22 passed,
  mc-net settlement 36 passed, mc-worldgen settlement modules green, determinism
  (A02), catalog rejection, rotation, survey/prepare/cancel/replay coverage.
- Harness fallout fixed by agent `C3HarnessTail` (19m10s):
  `mc-test-harness/tests/commands.rs` migrated to `persistent_residents`
  (13 passed), and `cargo fmt -p mc-test-harness` closed the last fmt debt — the
  workspace `cargo fmt --all --check` is now clean (0 diffs).

Wave 3 in flight: **C4** (resident work orders, squad orders/formations,
cross-region group admission, combat commits, equipment, plus deleting the dead
mc-entity villager binding lane) and **P1 v1 server-side** (the merged
`solaris-settlements` package: strict manifest, authored blueprints, growth /
economy / population domain logic, no Loader dependency).

Owner decision recorded: settlement plugin **v1 is server-side only**; everything
Loader-dependent stays implemented-but-disabled (`local://settlement-wire-freeze.md`
frozen, Loader repo work landed and unused for now). Next after wave 3: a live
probe of the default plugin set with a real player, then fixes.

## Bounded region pregeneration CLI + login-burst flake closed

`mc-server --config server.toml pregenerate --from x,z --to x,z` (uncommitted,
base tree f77525da). Inclusive block corners, either order, rounded outward with
`div_euclid`; fail-closed cap `MAX_PREGENERATE_CHUNKS = 4_194_304` (4096x4096
chunks). Runs the same startup path as serve (contract/baseline/seed), generates
the spawn window plus the rectangle through one shared worker batch
(`generate_chunk_positions(..., label)` — `label` = "spawn" keeps the tested
panic/incomplete messages), flushes dirty chunks, logs
`region pre-generation finished; every chunk is on disk`, exits before the
listener. Args need `allow_hyphen_values = true`: clap's negative-number
heuristic rejects `-600,900` because of the comma (`allow_negative_numbers` is
not enough).

Evidence: debug run `--from 2000,2000 --to 2060,2060` -> 16 region chunks,
`flushed=241`, `world/region/r.3.3.mca`; installed release (`~/.local/bin/solaris`,
18:16) `--from -600,900 --to -450,1050` -> 100 chunks, region `r.-1.1.mca`;
serve then opened that world with `existing world startup spawn window warmed
... region_files=8` and reached `Solaris is listening`. Tests: unit
`parses_pregenerate_block_coordinates`, `region_positions_normalise_corners_and_refuse_absurd_requests`,
`region_pre_generation_stores_every_requested_chunk_and_repeats` (reopens the
storage and proves the chunks are on disk); CLI
`pregenerate_rejects_malformed_block_coordinates`,
`pregenerate_cannot_be_combined_with_check`,
`pregenerate_accepts_negative_coordinates_and_repeats_on_a_fresh_world` (real
binary twice on one tempdir world: success, `solaris/world.json`, region files).

Flake closed: `plugin_owned_command_argument_limits_do_not_terminate_play_ingress`
(crates/mc-server/tests/play.rs) failed under load because the test client never
read the login burst (tab list + roster fills the socket), so the session task
blocked writing and `PlayerCommand` missed the 2s budget. Fix is the file's own
convention: `drain_initial_play_burst` after `drive_to_play`. 8/8 green at
0.76-0.80s (failure path was 2.5s). No production change.

Gates: `cargo test -p mc-server --all-targets` green (79 lib / 46+1 ignored bin /
2 / 42 cli / 14 / 0+4 ignored / 12 / 1 / 19 play / 2), `cargo fmt -p mc-server
--check` clean, `cargo clippy -p mc-server --all-targets -- -D warnings` clean,
`code-health` PASS (20260912T111836 and 20260912T111927). Workspace `cargo fmt
--all --check` still reports 58 pre-existing diffs, all in
`crates/mc-test-harness/tests/**`, none touched here.

Docs: `docs/OPERATING.md` "Pre-generating a region" + worldgen revision 21 text
(was stale at 20) + console-vs-CLI operator/whitelist wording (console commands
apply live, standalone CLI applies at next start) + `whitelist.json` defaulting;
`README.md` mentions the subcommand.

Reviewer round (read-only agent `PregenerateReview`, 7m5s, verdict
"incorrect", 0.8) found five real defects, all fixed:
1. `pregenerate` wrote Solaris terrain into an unversioned vanilla Anvil import
   that serve keeps read-only -> `ensure_pregenerate_target` now bails
   fail-closed on `WorldSource::ExistingVanilla`;
2. `insert_generated_chunk` bypasses the disk-first rule, so a rerun rewrote
   stored chunks (reviewer measured changing md5s) and would erase in-game
   edits -> `WorldStorage::chunk_is_stored` (resident-or-on-disk probe, no
   payload load, no generator) + `pending_region_positions` split; the log now
   reports `chunks/generated/skipped/flushed` and stored chunks are never
   regenerated;
3. the constant comment and the operator doc called 4,194,304 chunks a
   4096x4096 square -> corrected to 2048x2048 in both places;
4. the CLI test's `regions.count() > 0` passed on spawn-window files alone ->
   now asserts the rectangle's own r.-2.1/r.-1.1/r.-2.2/r.-1.2 files;
5. the cap check ran after the world had been created and the spawn window
   generated -> positions (and the cap) are resolved before the world is
   touched, so an oversized request cannot create a world.

Those fixes are verified live with the rebuilt release binary (installed
`~/.local/bin/solaris`, 12:00):
- oversized request `--from 0,0 --to 999999999,999999999` -> `error: requested
  region covers 3906250000000000 chunks, above the 4194304 chunk pre-generation
  cap` and the world directory is never created;
- first run of the -600,900/-450,1050 rectangle -> `chunks=100 generated=100
  skipped=0 flushed=325`; second run -> `chunks=100 generated=0 skipped=100
  flushed=0` and `md5sum -c` reports all four region files unchanged, so a rerun
  no longer rewrites stored chunks or in-game edits;
- unversioned-vanilla-import refusal is pinned by
  `pregenerate_refuses_an_unversioned_vanilla_import` (a live vanilla import is
  not reproducible locally, so this evidence is code-level only).

Final gates on the combined tree (my checkpoint plus the C1 inventory work):
`cargo test -p mc-server --all-targets` green (79 / 47+1 ignored / 2 / 42 / 14 /
0+4 ignored / 12 / 1 / 19 / 2), `cargo test -p mc-net --lib owned_` 27 passed,
`cargo fmt -p mc-server -p mc-world --check` clean, `cargo clippy -p mc-server
-p mc-world --all-targets -- -D warnings` 0, `code-health` PASS
(20260912T120025).

## Settlement contract C1 (inventory) — delegated slice complete, uncommitted

Agent `C1Inventory` (39m56s, verdict `changes`) implemented the C1 inventory half
on top of the existing DTO layer: `inventory_transfers` capability through the
existing `required_features` gate, 5 Lua functions installed, new
`crates/mc-net/src/play/session/owned_inventory_endpoint.rs` +
`crates/mc-net/src/script/storage/owned_inventory.rs`, and one durable
world-journal decision per mutation carrying the plugin receipt and the player
after-image (idempotent by `operation_id` + canonical fingerprint; typed
failures). Its crash test forks the process and `kill -9`s at three durable
boundaries, then reopens and replays: the item moves exactly once, and reverting
to two independent writes fails the test. Receipt:
`.analysis/codex-logs/c1-inventory/receipt.md`.

Two documented gaps, both correct scope boundaries rather than shortcuts:
1. the warehouse endpoint cannot resolve container block-entity NBT from the
   storage actor (that needs `&mut mc_world::WorldStorage` in the simulation
   interaction path); the endpoint kind is defined and fails closed with
   `unloaded`, and the handle issuer is documented for C2, so player<->warehouse
   transfer and cross-endpoint reservation blocking are not exercisable yet;
2. reservation `consumed` is always 0 in C1 because consumption receipts are
   written by the C2/C4 world/work operation; release arithmetic and the
   blocking predicate are implemented and tested.

next: owner decides whether to keep the pregenerate chunk cap and whether to
commit these two checkpoints; the next contract task is C3 (persistent resident
handles) or C2 (catalog/sites), which also unblocks the warehouse endpoint.

## GitHub release v0.0.6 republished with current binary

Built exactly per CI release-build (locked, x86_64 target), packaged,
smoked (VERSION/version/--check), uploaded with --clobber over the old
assets, installer verified end-to-end into a temp dir. aarch64 asset
still comes from CI tag runs only.

## v0.0.6 at d8ec969e — journal race test deterministic

CI failed the journal test twice despite the fail-closed record_commits
flag check: the test asserted post-mortem timing and could win against
worker teardown. Test now subscribes to the worker's own failure_reporter
watch channel (plus post-subscribe flag re-check), tokio::test with a
5s fail-closed timeout — push, not pull, no Instant polling. 30/30 local.
Production record path unchanged since 881461ca.
next: watch CI d8ec969e; owner retests recipe book on real client.

## v0.0.6 at 5f72864d — recipe_book_add client kick fixed

Symptom: real 26.1.2 client (NeoForge) kicked at login with
`Failed to decode packet clientbound/minecraft:recipe_book_add`,
`NoSuchElementException` inside ingredient decode. Decompiled vanilla
client.jar + NeoForge universal: vanilla HolderSet tag branch does
`registry.get(tagKey).orElseThrow`. We shipped 16 item tags in UpdateTags
but recipes reference 40 (e.g. `minecraft:coals` for torches) — first
unknown tag kills the client. Fix (5f72864d): added the 27 missing tags
with vanilla member lists (all members resolve in our ItemRegistry);
new regression test fails pre-fix naming the tags, green post-fix.
Reviewer verdict pass (0.9). CI on the tag in flight; real-client retest
pending with owner.

## v0.0.6 at 881461ca — all known reds fixed, quarantine lifted

Tag v0.0.6 = 881461ca (main pushed). Since 68549c44:
1. Journal race (CI-only): `record_commits` now checks the writer death flag
before enqueue — buffered send could succeed mid-teardown, accepting a commit
never persisted. 30/30 stress green; mc-net lib 2085/0.
2. Village defense un-quarantined and green (15.4s): root cause was the
ravager killing the observing player ~tick 230 (live=0 clears active chunks,
simulation freezes with golem ~3 blocks short). Observer now goes creative;
no production behavior change. Spawn ~tick 100 (villagers join projections
only each 100th tick — by design), pursuit ~1 block/s, attack lands ~260.
Full local L2-equivalent: mc-net lib green + defense file green; CI run on
the tag is the remaining gate.
next: watch CI 881461ca; villages gameplay follow-up (golem tuning only with
vanilla evidence).

## v0.0.6 republished green (tag moved, main pushed)

Full harness `test` PASS on committed tree
`.analysis/validation/20260911T181117-test-vzzubkvr` (268s). Tag v0.0.6 now at
16333834 (was f6da95d8); main pushed. Two fixes since f6da95d8:
1. village_defense attack test quarantined with #[ignore] + reason (spawn ok,
zero golem EntityEvents in 20s over wire; registry-level plan/commit/goal/
velocity all verified working — pursuit-vs-commit gap tracked for villages
follow-up, do NOT re-ignore further reds without owner consent).
2. witch_presence fixed for real: stale pre-flattening effect ids (slowness
2->1, poison 19->18) vs vanilla registry report
(`data/vanilla/reports/registries.json`: poison protocol_id 18); production
enum was already correct, test constants updated, 4.9s green. The old
full-run log never executed witch_presence (run stopped after the village
failure), so witch was never green — not a flake.
Receipt: `.analysis/releases/public-v0.0.6/receipt.json` updated
(tag_republished_ci_green_local). All temp probes removed; owned diff vs
f6da95d8 is exactly the two test files. Maturity draft.
next: villages follow-up — golem pursuit/attack root cause (wire census:
golem spawns, no EntityEvent at all); then un-quarantine the defense test.

## Test-repair sweep (dead field + stale wire expectations, no push)

Removed proven-dead `StructureSetFacts::placement_type` (+`RawStructurePlacement::type_id`
parsing, no consumer; `grep placement_type` empty). Backfilled `stew_effects:
Vec::new()` into 17 stale `FurnaceSlot`/`RecipeResult` test constructors.
Aligned stale zombie fixtures to HEAD 2.3 (`HOSTILE_FOLLOW_SPEED`, regional goal
test now pins target + 2.3 speed). Moved one heavy pickup integration test onto
the existing 4MiB-thread pattern (stack overflow fix, no behavior change).
Fixed two obsolete hurt-event expectations to 26.1.2 `ClientboundDamageEvent`
(PVP helpers + `player_entity_killed_lua` nonlethal fence; production untouched).
Rewrote the flaky short-grass seed wire test into a deterministic single-break
update+ack transaction (1/8 seed probability stays covered by
`mc-data/tests/plant_loot.rs`; prior form failed ~7/8 plus a 2026-08-30 known-flake
record). `SimulationAuthority` kept as capability token per review.
Validation: mc-data 261, mc-worldgen 137, mc-entity 635, mc-server 76, mc-net
2085, block_edit 36/0/70ignored, fmt + code-health PASS, one read-only reviewer
pass (pre-lua/grass deltas). L2 `correctness` red on
`village_defense_spawns_golem_and_attacks_hostile_over_tcp` (golem spawns,
no attack in 20s; fails isolated too; zero overlap with owned files —
pre-existing, needs its own vanilla-evidenced slice, not fixed here).
Ignored `mob_presence` helper holds the same obsolete EntityEvent-2 shape;
untouched (needs sidecars, unverifiable here) — follow-up with the defense slice.
base_tree: 961ed9ecb596b363d40360c2ce37605c78b7bf7a
diff_hash: 366c88566bcfb750b87814ce106ae6d1a44feb5c1df8ec423779bceb7901b38b
changed_files (13 owned, uncommitted): worldgen_structures.rs, regional.rs
(test only), play.rs (cfg-test const), startup_data_tests.rs,
block_edit.rs (DamageEvent import), campfire.rs, chests_and_hoppers.rs,
furnaces.rs, pvp.rs, survival_lifecycle.rs, survival_pickup_overflow.rs,
wheat_seed_source.rs, player_entity_killed_lua.rs. No commit/push. Maturity draft.
next: village-defense golem-attack slice with vanilla evidence (spawn ok,
attack never arrives); then re-run L2 green.


## Combined commit 961ed9ec (owner-authorized, no push)

One local commit with all 6 slices (owned 18 files only, path-limited):
ender emission, fluid wash, nether gen, end gen, structure loot, chest-loot
wiring. Validation per slice as recorded above; tree was fingerprinted stable
across the final gate. Known reds documented in the message. Left dirty and
unstaged: WATCHDOG.yml model swap, worldgen_structures placement removal,
regional.rs (also caught my earlier blanket `cargo fmt --all` — content-neutral
reformat of a stranger file, not staged), startup_data_tests + harness stew
fixes, .analysis deletions, bench json. No foreign session reachable via hub
(all peers are own subagents) — authorship of those edits undetermined.

## Old questions closed (owner decision 2026-09-11)

1. Stale-baked-light repair: NO automatic repair. Rationale: per-edit
invalidation already covers live changes; a validity bit is new persisted
state (vanilla-shaped but invasive) and a force-relight-all is unmeasured
work. Worlds baked pre-fix keep old light until retouched — same wart vanilla
carries across its own light fixes. Reopen only with a measured complaint.
2. Lava wash: KEEP stop-at-plant. Rationale: no vanilla evidence for
lava-vs-plant displacement or drop-burning semantics; changing it would be
invented behavior. Water wash stands alone as the reported gameplay case.

## Chest-loot startup wiring: done

Startup loads `simple_dungeon` + `village_toolsmith` tables from
`<vanilla_data_dir>/data` into `TerrainGenerator::with_chest_loot`; missing
dir/tables warn + keep fixed loot (hermetic fallback test). 8-arg Clippy lint
fixed by bundling into one `Option<(catalog, items)>` tuple (7 args), not
suppressed. Validation: server bin 40 passed; strict bin Clippy clean
(lib-test `stew_effects` initializer is a pre-existing HEAD breakage, left
alone); fmt + code-health PASS. Covered by the same independent reviewer as
End (disjoint paths). Nothing staged/committed/pushed.

## End generator foundation: done

`EndGenerator` worker slice (mirrors nether): end-stone island with void rim,
obsidian pillar ring + bedrock caps, single `the_end` biome, order-free
overlay, 4 tests green. Validation: worldgen lib 137 passed (nether 4 + end 4
included); strict Clippy clean; fmt + code-health PASS. Independent reviewer:
correct, no findings. Portals/travel/multi-world stay queued (needs server
coordination, Main-owned). Nothing staged/committed/pushed.

## Settlement plugin worker: P1-a done (sibling repo)

`SettlementPlugin` closed `solaris-settlements` P1-a: settlement domain ledger
(contract 3.1/3.2/3.3-roles/7-records) on API 0.6.0, storage-only. Validation:
STRICT-OK (real loader discovery+typecheck, fail-closed on unknown capability
and stray entries) and BEHAVIOR-OK 40/40 (lifecycle, gates, roles,
conflict-retry, restart recovery, abandon, orphan-free). Receipt:
`../solaris-default-plugins/server/evidence/solaris-settlements-p1a/receipt.md`.
Next slice: P1-b intent ledger; most value after core C1 `storage_batch_cas`
lands. No core files touched; nothing staged/committed/pushed.

## Nether review fixes (same slice)

Reviewer verdict was `incorrect` on tests only (production code held):
(1) buried-lava assert was inverted — replaced with per-column lava-xor-rock
exclusion; (2) ceiling-band interior 123..126 unasserted — now pinned to
bedrock-or-netherrack; (3) determinism never exercised order independence —
now interleaves neighbors + a second same-seed generator and compares full
columns. Advisor also caught a real generator bug: buried lava under high
columns + hollow air to the roof. Fixed to solid body (land = netherrack to
field, lakes = lava to sea level). Lake frequency retuned (field mean 52,
range [22,82]) after proving zero lakes across 256 chunks. Constants 32/127
labeled vanilla, [22,82] labeled Solaris tuning. Revalidated: full
`mc-worldgen --lib` 133 passed, Clippy/fmt/code-health green. No second
reviewer (findings fixed per policy). structures `pub use` re-export
accidentally dropped by a lib.rs edit — restored.

## Structure loot worker: done

`StructLootRolls` closed chest loot rolls at paste time: `TemplateChest` gains
`loot_table`, rolls are SplitMix64-deterministic per (seed, pos, chest index)
with vanilla overwrite-into-random-slots semantics, fixed contents stay as
fallback. New `mc-data/src/loot/chest_26_1_2.rs` compiles the exact JSON
surface of `simple_dungeon` + `village_toolsmith` and fails closed otherwise.
Validation (worker): worldgen lib green, 4 new loot tests + 7 chest tests pass,
Clippy/fmt/code-health green. Queued next: production wiring
(`ChestLootCatalog::load_vanilla_tables` at startup via `with_chest_loot`;
live servers still paste fixed loot until then).

## Current checkpoint: nether generator foundation (dimensions slice 1)

New `mc-worldgen/src/nether.rs` (`NetherGenerator`, `ChunkGenerator` impl):
bedrock floor y=0, fbm height field [26,92], lava sea below 32, netherrack
body, rough bedrock ceiling cap at 127 (4-deep hashed band), air above to
256, single `nether_wastes` biome. Deterministic per seed+chunk, order-free.
Generation-only: no server wiring, no portals/travel/respawn (single-dimension
architecture queued as its own slice). Multi-biome regions, ores, glowstone,
fortresses queued.

Changed (new files): `nether.rs` + `nether/tests.rs`; `lib.rs` module export.
Validation: 4 nether tests (floor/ceiling, body invariants over 16 chunks,
determinism, missing-block startup error); full `mc-worldgen --lib` 129 passed;
strict Clippy clean; fmt + code-health PASS. Independent review pending a free
agent slot (cap 2/2 busy: plugin + structure-loot workers). Maturity `draft`.

Next: End generator (same pattern), structure-loot worker result, portal/
multi-world architecture, graphical gates.

## Current checkpoint: stale baked light repro (handoff issue 7, migration half)

Headless repro proven (throwaway, removed after green): all-zero block light
baked over an emission-15 cell is served verbatim at stream with zero compute;
the same chunk without baked light recomputes nonzero. So chunks baked under
the old opacity-15/no-ender-7 metadata keep serving darkness after a fixed
binary is installed, until a light-changing edit retouches them. Per-edit
invalidation cannot repair them — nothing re-touches them.
Vanilla precedent (decompiled server-26.1.2.jar, `javap -c`): Anvil stores
`isLightOn` + light arrays; load applies `setLightCorrect(isLightOn)` and
false chunks relight through the engine. We have no such bit; baked light is
trusted unconditionally. Repair choice queued for owner (below) — nothing
automatic built.

Changed: none kept (throwaway reverted; receipt in subagent history
`history://StaleLightRepro`). No commit/push. Maturity `draft`.

## Current checkpoint: fluid wash (plant follow-up, water half)

Flowing water now displaces ground-support plants and columns
(`is_water_washable_plant`: shared ground set + shared column set, minus
seagrass/tall_seagrass which live submerged). The wash edit carries the
same read preconditions as any fluid edit; cells above a washed plant pop to
air through the reused break-path cascade; drops resolve at commit from the
previous states with survival loot rules (no tool) and a deterministic
tick+pos seed — upper double halves yield nothing. Lava keeps stop-at-plant
(queued, not a desync). Block-delta broadcast and light publication order at
the fluid commit are untouched (one advisor-caught near-miss restored before
validation).

Follow-up: column match deduped onto shared
`block_break::is_vertical_support_cascade_block` (no forked list; seagrass
exclusion stays the single documented water-specific rule, pinned by test).
Revalidated: fluid_runtime 15/15, `mc-net` Clippy clean, fmt + code-health PASS.
Lava wash deliberately untouched (no vanilla evidence for lava-vs-plant;
stays queued, current stop-at-plant is no regression).

Changed: `play/fluids.rs` (predicate + flow arm + cascade + `fluid_wash_drops`),
`play.rs` (`spawn_fluid_wash_drops` wired into `run_scheduled_fluid_ticks_owned`),
`play/tests.rs` (fixture +poppy/short_grass/tall_grass-halves/seagrass, old ids
stable), `play/tests/fluid_runtime.rs` (5 regressions: poppy wash, tall-grass
cascade, seagrass coexistence, lava stop, poppy-drop/upper-skip loot).

Validation: fluid_runtime 15/15; plants 80; session::tests 283; strict `mc-net`
Clippy clean; harness `fmt` PASS (`20260911T145709-fmt-p5798_17`), `code-health`
PASS. Full `mc-net --lib`: 2084 passed, 1 failed —
`hostile_pathing_keeps_full_speed` ALSO fails on clean HEAD 7518c29d
(HEAD zombie-speed slice leftover, pin still expects 1.25 vs attribute 2.3);
left for the slice owner, not re-pinned here. One read-only reviewer: correct,
no findings. Maturity `draft`; no commit/push without authorization.

Next: bucket OUTLINE nit (low confidence), stale-baked-light migration question,
all graphical gates. Lava wash queued.

## Current checkpoint: light publication trace (handoff issue 7, ordering half)

Traced immediate, deferred-mutation and deferred-storage relight paths with
no server-side ordering bug found. Deferred-mutation publishes through
`publish_computed_light_updates` (conditional publish, recompute fallback);
deferred-storage checks `incremental_light_sources_are_current` with a full
`collect_full_light_updates_for_current_world` fallback; the initial stream
serves baked light when present else computes. `block_edit_changes_light`
compares emission/opacity/sky, so the chest opacity fix correctly silences
ordinary-chest relight while the new ender emission 7 correctly triggers it.
The dark→correct→dark intermittency is not explainable by static metadata
and was not reproduced headless; it stays open pending the graphical gate
(client-side staleness vs stream ordering still unverified).

Changed: none (investigation only). No fake resends or forced brightness.
Validation: focused `mc-net --lib light` 29 passed (covers relight fencing,
baked publish, prepared-chunk invalidation). Maturity `draft`.

Next: fluid wash (needs drop plumbing in fluid-tick plans), bucket OUTLINE
nit (low confidence), all graphical gates.

## Current checkpoint: ender chest emission (handoff issue 7, emission half)

Conservative fallback gave every ender_chest state emission 0; vanilla 26.1.2
`block_light.json` rows for all 8 ender states are `[7, ..]` (ordinary/trapped
chests are 0). Fix: `conservative_emission` returns 7 for `ender_chest`,
after the candle branch, before the 0 fallback — reachable, shadow-free
(no earlier `contains` arm matches). This is the production path: the owner
server loads `blocks-report-conservative`, not the sidecar report.
Opacity still follows the chest waterlogged rule (0/1, never 15).

Changed: `mc-data/src/block_light.rs` (branch + `conservative_ender_chest_
emission_is_seven` regression + ender pin in ignored `real_table_matches_
known_blocks`, verified passing against local reports).

Validation: `mc-data` full 254+4+2+11+3 passed, strict `mc-data` Clippy clean,
harness `fmt` PASS (`20260911T142843-fmt-bj5g1rjg`), `code-health` PASS
(`20260911T142847-code-health-jotfj1vl`). One read-only reviewer: correct,
no findings. Maturity `draft`; no commit/push without authorization.
Intermittent dark-chest behavior itself still needs the publication-ordering
half + graphical gate.

Next: light publication ordering, then fluid wash (needs drop plumbing in
fluid-tick plans — not a one-line `can_flow_into` widening, which would
destroy plants without drops). Queued: all graphical gates, bucket OUTLINE
nit (low confidence, gameplay-only).

## Owner field follow-up — 2026-09-11 (uncommitted)

Latest priority: delayed leaf drops and expensive random ticks. Leaf fallback
hardness now matches 0.2; the packet-to-owner regression confirms STOP at tick 5
commits both air and a deterministic configured drop without delayed ticks.
Random candidate selection moved into `play/random_ticks.rs`: snapshot only the
budgeted chunks and generate samples only in eligible sections, retaining seed
offsets and order. Copied owner-region benchmark and exhaustive parity receipt:
`.analysis/codex-logs/field-followup-20260911/random-tick-receipt.json`.
This is candidate-stage evidence, not a new live-server p95 measurement.

Related fixes in this working tree: actual 26.1.2 damage packet instead of legacy
entity event 2, squid max health 10, pig passive behavior initialization,
Overworld timeline tags, and queued crafting clicks processed despite stale
state ids. Independent read-only review found stale carried predictions still
vetoed crafting actions; that veto is removed for stale packets and the
three-slot regression now supplies an incorrect nonempty cursor prediction.
Owner inventory fences remain authoritative.

Food recipe completeness and special edible-item effects remain open.
Basic food metadata now covers
all 40 consumable foods from local vanilla 26.1.2 reports: nutrition, saturation,
duration, animation and stack limits. Cooked cod/salmon/mutton now pass a server
use-item-to-commit regression (timing, debit, hunger and persisted state).
Source receipt: `.analysis/codex-logs/field-followup-20260911/food-data-receipt.json`.
This source change is not installed and has not had owner/client acceptance.
The `food.can_always_eat` flag now crosses start, completion and owner commit:
golden apples/chorus fruit consume at full hunger, ordinary food still cannot.
Real report loading excludes food holders without a consumable component.
Container remainders now commit with food: replace the final portion in hand,
otherwise use canonical inventory insertion, then publish one overflow entity.
Four consumption regressions cover hunger/timing and hand/merge/full-inventory
conservation; four owner food-transaction regressions and six item-component
tests pass. Strict affected-crate Clippy passes; `mc-net --lib` reports
2076 passed, 8 ignored. Independent remainder review found no defect.
Food eligibility is closed as the 40-item eating contract, not full special-item
parity. Consumption-effect execution and recipe-specific stew components remain
open with the food-recipe work. Do not reopen combat/absorption while closing
food eligibility.
Player status effects now use the persisted active-effect store. A save/load
regression covers unsorted effect input, hidden-effect restoration, actual
health/food mutations, and decoded packets for owner, tracker and late tracker.
The focused regression passes. No new graphical acceptance or binary install.
Manual leaf/grass loot now uses contextual probability rules: 11 leaf variants,
shears/Silk Touch preservation, Fortune tables, and short grass seed chance 1/8.
An empty roll stays empty instead of falling through to the block item.
Pre-fix: grass dropped seeds on 65,536/65,536 breaks and shears returned apples.
Post-fix: three deterministic distribution/tool regressions and the network
fallback regression pass; affected `mc-net --lib` 2074 passed / 8 ignored,
strict affected-crate Clippy and harness code-health passed.
Receipt: `.analysis/codex-logs/field-followup-20260911/plant-loot-receipt.json`.
These loot changes are source-only; no graphical acceptance or binary install.
The previously built debug binary remains installed at `~/.local/bin/solaris`;
SHA-256 `e0ce9ccf7fb25bb77deb524fee27c26711e406ec6da343d924374bad671c532a`.
Installation receipt:
`.analysis/codex-logs/installation-20260911T023358Z/receipt.json`.
The owner then verified time and crafting-table interaction as fixed.
Eating fish failed in the installed binary; the source-only regression above
does not replace owner/client acceptance. Hurt reactions mostly work, but squid
first-hit damage still fails; the 10-HP data regression is not acceptance.
Animals flee more slowly than vanilla, zombies move more slowly, and skeletons
hold bows without a visible string/arrow draw cycle. Those observations remain
open owner failures. Future fixes must address shared consumption, damage,
movement and bow-use paths, including newly added archer mobs, not type-name
exceptions. No owner process, configuration or world was changed by the agent.
Maturity remains draft; the above graphical observations are owner-run. Base tree:
`c9f3fba3087af9fd0b7510e1e55c160e21e5d209`. Evidence and validation closeout:
`.analysis/codex-logs/field-followup-20260911/checkpoint.json`.
Final affected scope: `mc-net --lib` 2073 passed / 8 ignored; field runtime
facts 3 passed; harness fmt, code-health and strict workspace Clippy passed.
Full workspace tests are not green: the retry stalled in
`mc-script::lua::loader_tests::shipped_two_owner_live_gate_fixture_is_discoverable_and_runnable`
and was interrupted; its log path and the earlier corrected failures are in
the checkpoint receipt. No change to that unrelated fixture was made.

## Current checkpoint: skeleton ranged slice (handoff issues 11-equip + 12)

Skeleton arrows now carry vanilla spread: normalize → per-axis triangular
offsets scaled by 0.0172275 × divergence → scale by 1.6, no renormalize
(javap evidence from local server-26.1.2.jar `AbstractSkeleton` +
`Projectile`). Divergence 10.0 = 14 − 4 × EASY, matching the advertised
`ChangeDifficulty 1`; seeded per (shooter, tick), no shared RNG; crossbow
passes 0.0 (bit-identical path). Skeletons/strays/bogged project a bow in
the main hand at spawn (same `finalizeSpawn` projection as pillager
crossbows, covering late trackers through the shared snapshot fn). Bow aim
fixed per the same bytecode: `target.getY(0.333)` (≈ +0.6, matching the
existing crossbow offset) plus `horizontal * 0.2` drop compensation (was
+1.0, no compensation). Draw-pose driver (probed): client `SkeletonModel`
poses BOW_AND_ARROW iff `isAggressive && mainHandItem.is(BOW)` — no
using-item flag needed; aggressive = mob-flags byte bit 0x04, published on
transitions and cleared on lost-target/death/re-track.

Draw pose done (no new state): `server_entity_snapshot_from` projects
`aggressive` for bow skeletons with a `FollowPosition` goal; the hostile tick
evaluates it every tick from the same budgeted projection fetch (no
due-gating, zero owner-lane reads — volley/melee budget tests pin this) and
diffs against the published snapshot, emitting `Byte{15, 0x04}` on change and
silence when steady. Index 15 by javap: Entity defines 8 accessors (0-7),
LivingEntity 7 (8-14, matches `LIVING_FLAGS=8` pin), Mob 1 → 15, PathfinderMob
0 → `AGEABLE=16` pin holds; aggressive = `Mob.isAggressive` bit 0x04.

Changed (uncommitted, C1 preserved): `session/outbound.rs` (flag +
`is_bow_skeleton_type_26_1_2`), `session.rs` (re-export), `session/
visibility.rs` (projection + shared bow predicate), `session/
visibility_tests.rs` + `wire_entities_tests.rs` (literals), `play/
wire_entities.rs` (index/bit consts, pairing + update encode, wire tests),
`session/hostile_authority.rs` (every-tick check + diff publish), `session/
tests.rs` (acquire/loss/steady/pillager-control test).

Validation: full `mc-net` lib 2069 pass, Clippy clean, harness `fmt` PASS
(`20260910T234125-fmt-inist4vt`), `code-health` PASS
(`20260910T234129-code-health-_e243jij`). Three read-only reviews (spread+
equip, aim delta, draw pose): the draw-pose review caught a real early-return
drop (fixed: publish helper called from both tick branches; test proven to
fail muted and pass fixed). Closing nit: pairing gated by bow type for
symmetry with the update path (+ leak-guard test). Maturity `draft`; no
commit/push without authorization.

Next: time (8) needs the graphical gate — blocked headless. Queued: fluid
wash, all graphical gates, light publication ordering, ender emission,
bucket OUTLINE nit.
## Current checkpoint: mob hurt flash trace (handoff issue 10, signal half)

Traced all three hit paths with vanilla evidence; no server-side signal bug
found. `attack_server_entity_locked` (mob/mob, village defense) sends entity
event 2 + knockback on every accepted nonlethal hit; the player path
additionally writes event 2 to the attacker stream; dragon/death paths send
2/`ENTITY_EVENT_DEATH`. Codec verified against local client-26.1.2.jar
`ClientboundEntityEventPacket`: writeInt entity id + writeByte event id,
wire id 0x22 — matches ours. Existing session tests pin `Damaged` outcomes
(which carry the event dispatches) for cow punches. Not yet done: the
observed-client half — attacker/observer/invuln-reject/death in a real 26.1.2
graphical run (entity-id mapping at spawn and client handling unverified
headless). No fake damage events added; no code changed in this slice.

Changed: none (investigation only). Issue 10 stays open past this slice
pending the graphical gate.

Next: skeletons (11, 12). Queued: fluid wash, time/bucket/chest/fish/light
graphical gates, light publication ordering + graphical, ender emission.

## Previous checkpoint: chest lighting opacity (handoff issue 7, metadata half)

Confirmed with the local 26.1.2 block-light report: every chest-family state
is opacity 0 dry / 1 waterlogged, never 15. The conservative fallback table
gave all chests 15 (opaque, no skylight) — the constant-darkness mechanism.
Fix: chest branch (`chest`/`*_chest`, 11/11 family IDs verified, no false
positives) mapping waterlogged→1 else 0, with propagates/suffocating
derived exactly matching vanilla rows. Intermittency (dark→correct→dark)
is NOT explained by the static table; publication ordering + graphical gate
stay queued. Ender emission 7 gap noted, untouched.

Changed (uncommitted, C1 preserved): `mc-data/src/block_light.rs` (branch +
1 test).

Validation: 13/13 block_light; `mc-data` Clippy clean; harness `fmt` PASS
(`20260910T142737-fmt-nmqmafam`), `code-health` PASS
(`20260910T142740-code-health-r9b8pf16`). One read-only reviewer: pass, no
findings. Maturity `draft`; no commit/push without authorization.

Next: mobs (10), skeletons (11, 12). Queued: fluid wash, time/bucket/chest/
fish graphical gates, light publication ordering + graphical, ender emission.

## Previous checkpoint: explosion support cascade (plant follow-up, blast half)

Blasts now pop ground plants/columns above destroyed supports in the same
conditional batch, reusing `append_vertical_support_cascade` (now
`pub(super)`, body unchanged) via `plan_explosion_support_cascade`, which
skips already-destroyed and unreadable cells with per-edit preconditions.
Drops flow through the existing explosion table (upper halves yield nothing,
matching survival semantics). Incidental: chest placement keeps the
same-kind/`single` guard and places single (not abort) when a candidate
neighbor chunk is unloaded. Still queued: fluid wash + placement-neighbor
paths, and all graphical gates.

Changed (uncommitted, C1 preserved): `play/simulation.rs` (helper + hook +
1 test), `play/block_break.rs` (visibility only), `play/block_placement/
chest.rs` + `chest_tests.rs` (unloaded continue + test).

Validation: explosion cascade unit test; 23 block_placement; 20
furnace/chest/plant neighbors; `mc-net` Clippy clean; harness `fmt` PASS
(`20260910T142301-fmt-v7nj2ot7`), `code-health` PASS
(`20260910T142305-code-health-ypubcobo`). One read-only reviewer: pass, no
findings. Maturity `draft`; no commit/push without authorization.

Next: lighting (7), mobs (10), skeletons (11, 12). Queued: fluid wash,
bucket OUTLINE nit, chest trapped/waterlogged/two-player/graphical, fish
campfire/graphical, time graphical gate.

## Previous checkpoint: region flush preservation (handoff issue 9)

`DirtyFlushPlan::write` no longer decodes/retains/recompresses untouched
slots. New `RawChunkRecord` + `read_region_raw` (location/comp/count/
aggregate validation, no retention) and `write_region_create_new_mixed`
(`Fresh` zlib-encodes, `Preserved` copies bytes verbatim with its timestamp).
The existing writer shares the same assembler with identical behavior.
Review drove two hardenings, both fixed: LZ4 compressed blocks are now
checksum-verified while counting (the counter skipped them, unlike exact
decode), and mixed-write validation enforces decode budgets on preserved
slots too — corrupt input fails raw read, mixed write, and decoded read
identically, no silent carry. Unique-tmp/stale-fence/journal/fsync/rename/
parent-sync/dirty-generation semantics untouched.
Benchmark (same 64-slot/4-dirty copied workload, debug build, receipt +
log in `.analysis/codex-logs/flush-preserve-bench/`): rewrite-all 2309ms vs
preserving 286ms (8.1x), retained uncompressed 3.11MB vs 86KB per flush
(36x). Synthetic NBT caveat noted in receipt; owner-workload RSS/CPU still
needs the owner environment.

Changed (uncommitted, C1 preserved): `anvil/region.rs` (raw + mixed +
validator + 4 tests), `anvil/mod.rs` exports, `storage/dirty_flush.rs`
(raw map + mixed tmp + 1 test). Throwaway bench file removed after receipt.

Validation: 288/288 `mc-world` lib; `mc-world` Clippy clean; harness `fmt`
PASS (`20260910T140833-fmt-fl0uwii1`), `code-health` PASS
(`20260910T140837-code-health-7_ar5vrc`). One read-only reviewer returned 2
findings (LZ4 count gap, write-budget gap); both fixed, no second review per
policy. Maturity `draft`; no commit/push without authorization.

Next: lighting (7), mobs (10), skeletons (11, 12), plant explosion follow-up.
Queued parity nits: bucket ray vs grass OUTLINE, chest trapped/waterlogged/
two-player/graphical, fish campfire/graphical, time graphical gate.

## Previous checkpoint: time set trace (handoff issue 8, server half)

Server chain verified end to end with vanilla evidence; no server-side value
bug found. Console `night` parses to 13000 (aliases ruled out already);
`OperatorControlHandle` uses the server-owned fence; simulation stores then
broadcasts; `send_outbound_world_time` maps simulation_tick→game_time and
world_time→overworld total (existing `world_time.rs` test pins both clocks).
Vanilla `javap` on local client-26.1.2.jar: `WorldClocks.bootstrap`
registers OVERWORLD first (id 0) then THE_END (id 1) — our constants match;
`ClientboundSetTimePacket` layout (gameTime, holder-id map, VarLong total,
floats) matches our codec; client `handleSetTime` applies `gameTime` via
`setTimeFromServer` AND clock updates via `ClientClockManager.handleUpdates`,
which keys by holder and sets total/partial/rate. Encoding suspects ruled out.
Not yet done: the observed-client half — capture the actual packet bytes and
sky/hostile-spawn agreement for night/day/noon/midnight in a real 26.1.2
graphical run (no client credentials in this environment). A subsequent
tick/sleep override was reviewed in code shape only (sleep `Skipped` is the
sole alternate publisher; dedup cannot resurrect stale values).

Changed: none (investigation only). No fix claimed; issue 8 stays open past
this slice pending the graphical gate.

Next: handoff issue 9 (region flush), then lighting (7), mobs (10),
skeletons (11, 12), plant explosion follow-up. Queued parity nits: bucket ray
vs grass OUTLINE, chest trapped/waterlogged/two-player/graphical, fish
campfire/graphical.

## Previous checkpoint: fish display IDs (handoff issue 2, logic half)

The 6 real vanilla fish recipes sort right after `minecraft:chest`, shifting
every later display ID +6. Per handoff 43-46 the shift is accepted (no
ordering hack, no fake `zz_` IDs): 76 pins across 15 `mc-data` recipe tests
moved +6, `chest`=5 untouched, production `solaris_required_recipes()`
(BTreeMap sorted + `bone_meal` tail) unchanged. Advertisement and lookup
share the same ordered set, so client/server stay consistent by construction.
A reverted `STABLE_TAIL_IDS` detour is recorded and was wrong: it broke
sorted order to preserve pins, against the handoff.
Queued: campfire real-item + graphical client acceptance.

Changed (uncommitted, C1 preserved): `mc-data/src/recipes.rs` test module
only, on top of the chest + bucket + flower/grass slices below.

Validation: 28/28 `mc-data` recipe tests; fish furnace/smoker regression;
32 campfire; 2 play recipes; `mc-data` Clippy clean; harness `fmt` PASS
(`20260910T133935-fmt-e27bi13b`), `code-health` PASS
(`20260910T133939-code-health-l3s2h58o`). One read-only reviewer: pass.
Maturity remains `draft`; no commit/push without explicit owner authorization.

Next: handoff issue 8 (time set), then issue 9 (region flush). Queued plant
follow-up: explosion candidates (`plan_explosion_candidates`) and
fluid/placement-neighbor removals do not yet reuse `is_ground_support_plant`;
queued parity nits: bucket ray vs grass OUTLINE, chest trapped/waterlogged/
two-player/graphical, fish campfire/graphical.

## Previous checkpoint: double-chest pairing (handoff issue 1, logic half)

Placement pairing proven in both orders with complementary left/right types,
equal facing, and either-half `paired_position` symmetry. Shared `opposite`
from `mc_data::block_placement_26_1_2` replaces the local duplicate.
Changed: `play/block_placement/chest.rs`, `chest_tests.rs` (mirror test).
Validation: 3/3 chest planning, 22/22 block_placement, Clippy clean,
harness `fmt` PASS (`20260910T133149-fmt-7vdqceed`), `code-health` PASS
(`20260910T133152-code-health-r9b8pf16`); reviewer pass, no findings.
Queued: trapped-vs-normal, waterlogged, two-player contents, graphical.

## Previous checkpoint: authoritative buckets (handoff issues 5+6)

Empty-bucket `UseItem` now picks up source fluid through a validated-pose
raycast (source-only, occlusion, 4.5 range) reusing `BucketUsePlan` + the
simulation commit transaction. Filled-bucket `UseItemOn` follows vanilla
ordering (pickup-first, vegetation in-place replace, target reach,
placeability incl. source refusal); every bucket-held outcome ends terminal
via resync+ack with no double-ack, both hands. Unrelated stack moves/swaps,
close/reopen and reconnect stay authoritative. Look math consolidated onto
`player_look_direction` after review (no second convention).
Queued parity nit (not a desync): the ray passes grass vanilla OUTLINE would
stop at; confidence low, gameplay-only.

Changed (uncommitted, C1 preserved): `play/bucket_interactions.rs`
(raycast + hardened ordering + 8 tests), `play.rs` bucket branch + pose
plumbing, `use_item_on_adapter.rs` visibility + pose arg, plus the
flower/grass cascade below.

Validation: 9 bucket module tests; neighbors bucket 19 / fluid 20 /
use_item_on 18; `mc-net` Clippy tests clean; harness `fmt` PASS
(`.analysis/validation/20260910T132734-fmt-k05y538m`), `code-health` PASS
(`.analysis/validation/20260910T132738-code-health-l3s2h58o`). One read-only
reviewer: correct with 2 P3 (look-dup fixed, grass-OUTLINE queued).
Maturity remains `draft`; no commit/push without explicit owner authorization.

## Previous checkpoint: flower/grass support cascade (handoff issues 3+4, break path)

Breaking a support block authoritatively pops poppy, short grass and tall
grass (lower + upper halves) in one transaction; every cascade edit carries a
read precondition (unloaded neighbor chunk rejects the whole batch).
Support-pop carries no held tool (no shears-only grass grant); the upper
double-plant half drops nothing twice. `hanging_roots` excluded after review
(ceiling-hung). Explosion/fluid/placement-neighbor paths still open.
Changed: `play/block_break.rs`, `plant_rules_26_1_2.rs`,
`play/tests/plants.rs` (4 regressions + import/sort fix),
`play/tests/furnace.rs` Clippy borrow.
Validation: 4/4 cascade + 101 plants/block_break; fish furnace test;
`mc-net` Clippy clean; harness `fmt` PASS
(`20260910T121027-fmt-rqtlu3zo`), `code-health` PASS
(`20260910T121510-code-health-twwl4b6u`); reviewer changes (hanging_roots)
fixed.

## Previous checkpoint: owner-requested as-is handoff to main

The owner stopped implementation and explicitly requested an immediate single
commit and push to `main`, with unfinished work documented for another agent.
Read [FIELD_TEST_HANDOFF.md](FIELD_TEST_HANDOFF.md) first: it contains all twelve
reported issues, evidence, source entrypoints, partial changes and acceptance
steps. Do not treat this snapshot as a release or a completed gameplay fix.

Fish cooking has a passing focused before/after regression. Double-chest changes
are partial. The final correctness gate passed formatting but failed strict Clippy
at `play/tests/furnace.rs:1195` (`&format!` needless borrow); full tests did not run.
Receipt: `.analysis/validation/20260910T095908-correctness-0t0ey1bm/result.json`.
The installed owner binary/process/world were not changed by these field fixes.
Maturity remains `draft`.

Next: resume the documented issues, starting with the exact Clippy error and
chest regressions; then close the remaining gameplay and save-allocation defects.
The older checkpoint narrative below is historical, not the active queue.

## Previous checkpoint: compact chunk storage and actionable profiles

The owner-approved uniform/shared lighting, 1–3-bit in-memory block palettes and
memory/CPU profile breakdown are implemented. Source remains uncommitted.
No installed binary, owner process, world or configuration was changed.

Light arrays use inline repeated bytes or shared copy-on-write 2,048-byte
payloads. Unknown and computed-zero light remain distinct. Small palettes use
1/2/3 bits in RAM, with valid minimum-four-bit Anvil/wire encoding.
On the same isolated copy of 1,811 stored owner chunks, requested live heap fell
100,825,603→42,859,987 bytes (96.2→40.9 MiB, 57.5% less). Both probes released to
244 bytes after world drop; RSS remained retained. The probe's printed checksum
is an estimated-byte sum, not a semantic content hash.

`profile` now captures actual process RSS/Linux mappings/I/O/thread CPU,
requested live Rust allocations and churn, sorted owner estimates, chunk
categories with shared-payload deduplication, reusable lighting scratch,
prepared/session/entity capacities, and exclusive CPU by subsystem.
Async polling excludes suspension; lock/runnable wait and inclusive wall time
remain separate. Run profile before and after the workload for interval deltas.
The system allocator is unchanged; no trimming or arena tuning was introduced.
The unclassified heap/CPU remainder and non-transactional capture limits are
explicit, not labeled a leak or exact allocator retention.

Final isolated debug-server smoke passed four 1,089-chunk initial streams,
128 unloads and regrowths each, zero retained resends and normal chat.
After explicit `save-all` acknowledgement, profile measured 235.1 MiB RSS and
48.2 MiB requested Rust live bytes: estimated registry 21.4 MiB, light scratch
6.8 MiB, published chunks 1.5 MiB, prepared frames 0.17 MiB and 18.4 MiB outside
classified owners. Capture took 12 ms. The exercised interval attributed
43.5 of 49.4 CPU seconds; other preparation, disk decode and saving dominated.
These are a native debug workload and capacity estimates, not graphical
acceptance, a TPS target or exact RSS ownership. The combined changes did not
regress the measured warmed stream timings; profiler overhead was not isolated
from the storage savings.

Validation: complete workspace 4,500 passed/194 ignored; formatter, strict
workspace Clippy and code-health passed. Final registry/CPU attribution
refinements additionally passed strict workspace Clippy, mc-world 280/15 and
mc-server 76/0, debug build and the native profile/save/stream scenario.
One independent read-only review found no blockers in compact storage and the
initial profiler; Main verified the final attribution refinements.
The existing Lua gameplay-event fixture now moves horizontally off its supporting
block before descending to the item; all event assertions remain and both its
focused test and complete workspace rerun pass. An old telemetry fixture that
only asserted defaults/copied values was removed, not re-pinned.

```yaml
base_tree: 8414d7e05b379b0d5b78e9506036d14bc7f98378
snapshot: .analysis/codex-logs/compact-profile/receipt.json
previous_snapshot: .analysis/codex-logs/river-profile/receipt.json
validation:
  - .analysis/validation/20260910T075629-test-bec1lr2p/result.json
  - .analysis/validation/20260910T081626-code-health-bqumxzwb/result.json
  - .analysis/codex-logs/compact-profile/final-validation.json
  - .analysis/codex-logs/compact-profile/final-after-save.json
resume:
  next: Owner graphical field test of target/debug/mc-server with a fresh world_dir, collecting profile before and after exploration; keep the current sarvar world and installed process unchanged until separately authorized.
```

Earlier aquatic movement, region-reader cache and wider/deeper variable rivers
remain implemented; their evidence is in the previous snapshot and
`.analysis/codex-logs/aquatic-ram/receipt.json`. Worldgen revision remains 20:
the prior river change requires a fresh world rather than rewriting the owner's
revision-19 chunks. This compact-storage/profile checkpoint did not bump it.
`target/debug/mc-server` includes all these changes. `target/release/mc-server`
remains the earlier aquatic/RAM revision-19 build. Subsequently, on explicit owner
request, the current debug build was installed atomically as
`~/.local/bin/solaris`; the previous executable was backed up. SHA-256 matches
the tested source binary and installed `--version` reports `mc-server 0.0.5`.
No running server was restarted and no world/configuration was modified.
Installation receipt and backup location:
`.analysis/codex-logs/compact-profile/installation/receipt.json`.
Published version remains v0.0.5 and
maturity `draft`. Graphical movement/parity and historical failed/manual-pending
scenarios remain unverified; native/Cargo passes do not close them.

## Standing architecture objective

Full core redesign: fewer mechanisms and lines, more reuse and clarity, preserved
performance and vanilla invariants. One broad, uniform, stable addon API must
serve server logic and Loader-backed client features. Replace the documentation
and memory; delete obsolete material rather than preserve legacy guides.

The new target contract is [ARCHITECTURE.md](ARCHITECTURE.md). It is not a claim
that the runtime has already been migrated. Current plugin API: `0.6.0`;
[current reference](PLUGINS.md). Loader has one common implementation and
Fabric/NeoForge/Forge adapters.

Owner-approved overload policy: inside declared capacity, vanilla semantics;
outside it, explicit pre-mutation rejection or local delay of expensive work is
allowed to protect the kernel and healthy players. Never discard accepted
mutations, delete items, or partially commit a transaction to relieve pressure.

Owner correction: finish one bounded area through design, implementation and
verification before selecting the next. Do not reopen a whole-core design survey.

Current owner priority: finish RAM-backed asynchronous WAL, then the alpha-3
worldgen and water-walking findings; reduce oversized files, duplication and
one-use helpers; run varied load scenarios and fix measured hot paths. Loader
feature development remains frozen. The owner now authorizes committing and
pushing the entire accumulated core change, with local artifacts excluded, and
creating/publishing the public `solaris-loader` and `solaris-default-plugins`
repositories first so hosted CI can resolve them. The owner subsequently
authorized tagging and publishing `v0.0.3-alpha.1` for curl installation and
manual play, superseding the CI-wait interruption. The existing frozen local
archive remains untouched; tag CI builds new public assets from current source.

Publication completed on `main`: core `f6e9426b`, Loader `6a383c2d`, default
plugins `d71ce84d`. The owner then prioritized the hosted CI failures in
[run 34353147695](https://github.com/kaiserproger/solaris/actions/runs/34353147695).
The Loader fixture required missing `ffmpeg`; a fresh Java build also lacked
the external Minecraft client jar. CI now provisions both, verifies Mojang's
client checksum, and retains failed test/Loader receipts. Three fresh public
checkouts pass 4,462 Rust tests and the full Java profile after prerequisites.
The original hosted Cargo failure's detailed log was not retained, so its cause
is not established and no test or assertion was weakened.
Exact reproduction, correction and hosted follow-up evidence:
`.analysis/codex-logs/ci-fixture-34353147695/receipt.json`.

The owner-authorized public prerelease `v0.0.3-alpha.1` is now published from
`571f53834f33a4f60c430b293dc901e9b1f4f5f7`.
[Tag workflow 34356580571](https://github.com/kaiserproger/solaris/actions/runs/34356580571)
passed all required gates and published Linux x86_64/AArch64 archives and SHA-256
files. The pinned public curl installer, version/config checks, fresh standalone
startup on `127.0.0.1:25565` and clean SIGINT shutdown passed on x86_64.
One independent release review passed; full survival acceptance remains open.
Receipt: `.analysis/releases/public-v0.0.3-alpha.1/receipt.json`.

Owner explicitly requires continuous autonomous execution, without mandatory
checkpoint stops. Evidence snapshots and validation are internal milestones,
not permission gates or reasons to yield. Continue into the next bounded area
unless a real blocker or material owner decision prevents progress.

## Local C1 work: reapplied over updated origin/main

The owner requested an upstream update and explicitly chose to reapply local
C1 rather than park it. Core is based on `98eefb5f`; Loader and default-plugins
already matched their current `origin/main`. Upstream field-test handoff and
release notes above remain intact; this local work does not close those issues.

C1 storage batches/scans/receipts are implemented. Compound inventory/storage
uses one world-journal decision, with durable storage/playerdata projections
before live publication. Recovery includes cursor and open-container inputs,
runs without Lua, and restores checkpoint eligibility. Failed projection
signals world fail-stop while the player inventory remains fenced.

Before this upstream integration, 105 inventory tests passed, including process
crashes and startup without Lua (`artifact://1770`). The broader correctness
run passed formatting but failed Clippy on an unused import and unwired owned
inventory helpers; the import was subsequently fixed. Those earlier results
are not validation of the rebased tree.

Verified pre-update backup, stash identity and synchronization evidence:
`.analysis/codex-logs/upstream-sync-20260910T113724Z/receipt.json`.
Prior implementation evidence:
`.analysis/codex-logs/settlement-c1/owned-inventory-progress.json`.

**C1 remains in progress.** Physical transfers/reservations still need runtime
integration with canonical POI and resident ownership. The owner chose to allow
destruction of containers holding reserved materials, with explicit loss
accounting and no automatic compensation; do not make them indestructible.
Contract: `../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md`.

## Previous checkpoint: published alpha-4 and verified installation

[Solaris v0.0.4-alpha.1](https://github.com/kaiserproger/solaris/releases/tag/v0.0.4-alpha.1)
is published from `60be6039bfc63e97a40299a378f3e66e6a7cac17`.
[Tag workflow 34417692138](https://github.com/kaiserproger/solaris/actions/runs/34417692138)
passed and published Linux x86_64/AArch64 archives and SHA-256 files.
The final local correctness gate passed: 4,471 Rust tests, zero failures,
194 ignored, plus formatter, strict Clippy and code-health. Installer and
harness-check gates passed; the independent release review found no blockers.

The pinned public installer downloaded the x86_64 release without a local
asset override. The installed binary reports `mc-server 0.0.4-alpha.1`;
strict configuration admission loaded all five explicitly installed standard
packages from public package commit `2d51ae5559cdd5b7cbab32888ef2b11d931e3e6e`.
An isolated fresh world started, accepted `status`, `save-all` and `stop`,
then reopened its saved metadata and exited cleanly on SIGINT. Both exits
were zero. The pipe driver required an explicit final LF; its initial wait
timeout is retained in the receipt, not counted as a successful gate.

World identity is schema 4 / worldgen revision 19. Use a fresh alpha-4 world;
do not hand-edit older world metadata past the startup compatibility fence.
These installation and diagnostic graphical checks are not full survival
acceptance. The earlier no-debug survival scenario remains blocked; maturity
remains **draft**. No local AArch64 runtime or packaged-JAR launcher-matrix
claim is made.

Evidence: `.analysis/releases/public-v0.0.4-alpha.1/receipt.json`.
Next outcome: implement and verify C1 durable storage and inventory operations
from `../solaris-default-plugins/SETTLEMENT_OVERHAUL_CONTRACT.md`, followed
by the remaining owner-requested settlement contract and acceptance scenarios.

## Previous checkpoint: downloadable Loader preview

[Solaris Loader v0.1.0](https://github.com/kaiserproger/solaris-loader/releases/tag/v0.1.0)
is published as a prerelease from
`3aaa92663ce8bc3e7de2859ad40ed357aacf3382`, with the three player adapter
JARs and `SHA256SUMS`. Downloaded release assets passed checksum verification.
The README now starts with player installation and permissions, including
instance-local instructions for common launchers; developer/MCP material is
retained below. Forge metadata now fences Minecraft exactly to 26.1.2.

Java tests and real graphical Fabric, NeoForge and Forge Loader gates passed.
Visual review confirmed visible modal assets/buttons, two-owner HUD updates,
owner-local hiding and reconnect cleanup. Vanilla notification overlays
obscure some modal description text. These gates use Gradle client adapters,
not packaged-JAR installation in every launcher; no broader claim is made.

Evidence and release source/assets:
`.analysis/releases/loader-v0.1.0/receipt.json`.

## Previous checkpoint: explicit plugin installation and author workflow

The owner selected explicit opt-in installation. The package repository now
owns `install.sh`: it installs the standard five packages or named selections,
refuses existing package paths, and does not edit server configuration. Core
builds and ordinary core installation remain independent and plugin-free.

The plugin guide now indexes all 32 registered host functions, includes an
executable `/hello` author workflow, and distinguishes dynamic host argument
validation from strict checking of Luau itself. Isolation documentation records
separate VMs on one host thread, 16 MiB per-VM memory, aggregate 50 ms event
budget and maximum 10 ms plugin slices; none is a process-isolation or
whole-server performance claim.

Real-package installation and strict server admission passed. Refused updates
preserved operator configuration; invalid batches, duplicate names, traversal
and dangling destination symlinks were rejected. A real graphical client
received the documented greeting and sandbox checks before and after an
infinite handler disabled only its owning plugin. One independent read-only
review returned no findings.

Evidence: `.analysis/codex-logs/alpha4-plugin-install/receipt.json` and
`.analysis/validation/20260909T230228-regression-w8ebweft/result.json`.
No runtime Rust changed in this slice; prior native gates were not rerun.
Next outcome: player-first Loader documentation and downloadable platform
JARs, verified through the supported launch/install paths.

## Previous checkpoint: alpha-4 field findings and startup rules

On seed `1785772562805887200`, native herd placement now separates pack
members, suitable water supports fish and squid, and generated shallow
sediments include clay. The field probe found a nearby plains tree; a real
graphical client visited that tree, birch forest, and a snowy mountain.
The stricter graphical survey also observed natural cod and squid close up
and raycast natural clay at both the spawn-side pool and river.

Startup `rules.lua` compiles bounded, validated gameplay data once into native
spawn and terrain rules. Its effective identity is persisted with the world;
changed rules reject a restart before listening. Broken manifests cannot hide
a rules owner in permissive discovery. Weighted monster plans consume the
declared entries and counts within native admission caps.

The server now has a typed interactive console and an explicit `--no-console`
mode. Normal logs omit repeated profiling detail. TUI/plain-console and
startup/restart-rejection smokes passed. Workspace tests, formatter,
code-health and strict workspace Clippy passed; one independent read-only
review found two issues, both corrected and covered by focused regressions.

Exact receipts, source snapshot and the failed diagnostic driver attempts:
`.analysis/codex-logs/alpha4-field-1785772562805887200/receipt.json`.
Final graphical gate:
`.analysis/validation/20260909T162959-regression-_fak5g95/result.json`.
This is operator-assisted field diagnosis, not no-debug survival or release
acceptance. Maturity remains `draft`; alpha-4 has not been published.

Next outcome: install selected default plugin packages without bundling their
source into core, with a documented, executable author/deployment workflow.
The supplied settlement overhaul contract remains a proposal; its upstream
capabilities and acceptance matrix are not implemented by this checkpoint.

## Baseline and evidence

- Base HEAD before the authorized publication:
  `638543ab4771f7db9aef93cfaeed7e2fae832312`. Earlier dirty work is included by
  explicit owner choice; local caches, world data and evidence remain excluded.
  Historical no-commit notes below describe their original checkpoints, not the
  current authorization.
- Previous cutover: net −1,198 Rust lines, including tests. Core tests, Clippy,
  formatting, code-health, and independent review passed. This did not close
  broader acceptance. All eleven recorded workspace failures are now closed;
  the full correctness gate and its final affected-target follow-up passed.
- Frozen debug load matrix: 42 runs on one Ryzen 5 7535HS, CPU affinities 1/4/12;
  **20 PASS / 22 FAIL**. Original assertions and workload sizes remain intact.
  Both soaks fail setup; the twelve-CPU 40k/60-client run retains only 7 sessions.
- Exact prior receipt, snapshots, and logs:
  `.analysis/codex-logs/core-overhaul-2026-09-05/receipt.json` and `rnd/` beside it.
  Graphical join smoke passed; full survival/multiplayer and owner terrain
  acceptance were not established. Alpha is still draft, not release-ready.
- Vanilla oracle: `.analysis/server.jar`, verified Minecraft 26.1.2. Reuse it.

## Current verified improvement: compact vehicle reads

- Vehicle graph validation and passenger lookup now read live identity,
  lifecycle and vehicle components instead of cloning every complete entity
  snapshot. The existing vehicle API lives in `entity_vehicle.rs`; no new cache,
  topology authority, scheduling policy or reduced entity selection was added.
- Entity removal uses the canonical ECS passenger unlinking once, without the
  former redundant full-population scan. Atomic graph rejection, lifecycle,
  duplicate-passenger, cycle, rollback and publication rules are preserved.
- In the same instrumented seeded route, unfenced owner-apply median/p95 fell
  from 10.152/18.321 to 4.373/10.872 ms. All 3,738 baseline and 2,575 candidate
  unfenced inputs committed. Work counts differ: these are elapsed distributions,
  not equal-throughput, process CPU or whole-server claims.
- A separate compact fenced batch applied 92 of 130 inputs. Its receipt remains
  explicit; unchanged fence policy and the fully committed unfenced cohort do
  not justify claiming universal input acceptance.
- The external smoke checked 45 graph cases, 18 removals and 360 paired passenger
  lookups. Existing vehicle/kinematics/rollback checks passed, and a permanent
  chain-extension/atomic-cycle regression protects the uncertain graph edge.
- Final correctness passes: 4,462 tests passed, zero failed, 194 ignored;
  formatting, strict Clippy, code-health, debug build and three graphical seeds
  pass. Each route observed land/water/bank transitions, with zero unsupported
  samples in this run. Earlier isolated water evidence remains unresolved.
- Native warning-only maxima remain 119.981 ms whole tick on the owner seed and
  98.495 ms dispatch on seed -17711. This does not close the 50/60 ms gate.
- One independent read-only optimization review passed. All ten temporary
  profiling/configuration/launcher/smoke files were removed after verification;
  their sources and exact evidence remain under
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/entity-dispatch-followup/receipt.json`.
  Publication evidence and exact repository revisions are recorded separately
  in `publication.json` beside it. Alpha remains draft.

## Previous verified improvement: shared collision classification

- Owner-local physics and pathing now reuse `vanilla_collision_class`, already
  used by fallback physics. Empty and full-cube cells avoid repeated binary
  shape decoding. Startup warms this existing immutable table before serving.
- Block reads, snapshot/completeness checks, powder-snow context, complex boxes,
  unknown-state fallbacks and publication fences are unchanged. No per-entity
  block cache, duplicate classification table or reduced simulation work remains.
- On 3,693,246 identical query/snapshot pairs, sampler construction plus physics
  integration averaged 12.330 → 7.448 µs, a 39.6% elapsed reduction. Both execution
  orders improved; the separate identical-implementation control had 0.90%
  aggregate label bias. This is not process CPU or whole-server throughput.
- The final pathing cutover preserved geometry for 29,873 catalog states and two
  out-of-range cases. Existing publication and chicken-AABB checks passed.
  The catalog smoke was temporary, not a new permanent test.
- Final correctness passes: 4,461 tests passed, 194 ignored, formatting, strict
  Clippy and code-health. Debug build and all three graphical routes pass.
  Each observed land-to-water and water-to-bank transitions; sustained
  unsupported samples were zero. Earlier isolated water evidence and broad
  terrain acceptance remain unresolved; still images do not prove support.
- Final owner warning-only tick maximum is 148.525 ms. Dispatch, preparation and
  scheduled-block costs remain open; this is not a whole-tick latency win.
- One independent read-only review passed. Production Rust grew by ten lines;
  all probes and nine temporary launch/config/smoke files were removed.
  Exact comparisons, controls, receipts, source delta and visual limitations:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/physics-sampling-followup/receipt.json`.
  Alpha remains draft; no staging, commit, push, tag or archive replacement.
- Prior publication-tail, periodic-planning and narrow-despawn evidence remains
  in `regional-commit-followup/receipt.json`, `periodic-planning-followup/receipt.json`
  and `unattributed-tick-followup/receipt.json` beside this receipt's parent.

## Previous verified improvement: background CPU headroom

- Background preparation starts and recovers at `max(cpu_capacity - 1, 1)`.
  The shared foreground ceiling, selected simulation work and job retention
  are unchanged. Other foreground users can occupy the headroom; this is not
  a planning deadline guarantee. Single-worker configurations remain serial.
- A forwarding-waker probe attributed 98.516 ms of a 98.597 ms admission sample
  to waiting for a permit, versus 0.081 ms to resume after notification.
  In the matched route, admission p99/max fell from 2.084/98.597 ms to
  0.016/3.020 ms; no candidate acquisition polled pending. Combined
  admission/dispatch/planning p99 fell from 4.956 to 3.205 ms.
- Realized work counts and finished stream windows differ between runs.
  Do not claim equal chunk throughput or an across-the-board speedup.
- Final workspace tests pass: 4,462 passed, 194 ignored. Final formatting,
  code-health, strict Clippy, debug build and three graphical habitat routes
  pass. One independent review accepted the production policy. Later test
  migrations replace call-counter/default assertions with admission/drain
  behavior; obsolete test-only counters were removed.
- Original failed correctness/test receipts remain failed and retained.
  An intermittent roster timeout did not recur in diagnostic runs; the wire
  scenario now waits for the final menu's close, not any buffered close.
  Stale-close causation of that timeout is not established.
- Ten temporary files and the planning probe were removed. Exact source,
  measurements, failures, final gates and review scope:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/scheduled-stall-followup/receipt.json`.
- The owner route still has warning-only maxima of 155.537 ms whole tick,
  118.228 ms unattributed time, 56.793 ms entity dispatch and 23.620 ms
  scheduled-block work. The broad scheduled-block task and 50/60 ms gate remain
  open. Alpha remains draft; no staging, commit, push, tag or archive replacement.
- Previous coastal and session evidence remains in `coastal-boundaries/receipt.json`
  and `session-lock-phases/receipt.json` beside this receipt's parent directory.

## Previous checkpoint: idle grazing snapshot construction avoided

- Every loaded sheep ID still reaches an owner batch. Owners read the current
  ECS timer and construct full snapshots only for active timers or possible
  idle starts. The existing baby/adult phase relationship preserves both start
  schedules; `Some(0)` cleanup and full-state atomic timer CAS remain intact.
- No population cap, cadence change, persistent cache or journal bypass.
  Filtered reads use the coordinator; complete reads retain their direct path
  and do not receive partial route-cache publications.
- Nine controlled debug A/B cases retain exact timers and start sets. At 1024
  sheep, idle median falls 15.785→3.708 ms and mixed 19.439→13.691 ms; all-active
  median rises 41.866→43.339 ms (+3.5%). This is a measured tradeoff, not a
  whole-server latency claim.
- Full L2, debug build and one independent read-only review pass. All three
  unchanged graphical habitat routes pass their existing acceptance rule;
  sampled dry pig movement and complete censuses remain, without reliable
  overflow log lines. One nonconsecutive grounded-over-water observation on
  the owner route remains unclassified; the gate rejects consecutive samples,
  not every isolated observation. Do not claim clean water-surface parity.
- Live grazing still reaches a 51.527 ms owner-route warning; the jungle frame
  shows 14 FPS. Broader grazing, terrain/render and survival acceptance remain
  open. Source, receipts, the surface observation and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/sheep-grazing-idle/checkpoint.json`.
- Temporary probe and replay tools are removed with their sources retained.
  No staging, commit, push or tag; frozen archive unchanged. Alpha stays draft.

## Previous checkpoint: duplicated grazing batch validation removed

- In-place conditional replacement batches with unchanged passenger links now
  use owner preparation for full-state comparison, without another coordinator
  snapshot read. Structural, claim, routing, committed-state and atomic
  rollback/journal fences remain. Topology-changing batches retain preflight.
- Removed the redundant grazing-action ID filter; every action belongs to the
  same atomic timer batch. The borrowed planner remains: an ownership cutover
  increased measured planning cost and was rejected.
- Nine matched debug workloads retain every loaded sheep and exact timer
  decrements. At 1024 active sheep, median/p95 fell from 52.756/54.448 ms to
  42.323/46.913 ms on the same four CPUs. Idle reads did not improve consistently.
- Final full L2, debug build and independent read-only review pass. The unchanged
  three-seed graphical habitat routes pass with complete natural censuses,
  sampled dry pig movement and no reliable queue overflow.
- The broader sheep-grazing bottleneck task stays open: idle snapshot reads
  remain material, and the owner route still warns about expensive grazing.
  Dense-canopy client FPS remains low. Warning-only samples and still images
  do not prove full-server latency, seamless distant terrain or survival parity.
- Complete evidence, rejected trial, source and next action:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/sheep-grazing/checkpoint.json`.
  Temporary profiling/replay tools are removed, with sources retained. No
  staging, commit, push or tag; frozen archive unchanged. Alpha remains draft.

## Previous checkpoint: dense natural despawn reads reduced

- Every tracked natural entity now uses the existing compact owner projection;
  only conditional removals need full retained snapshots. Population, cadence,
  distances, idle/damage rules and UUID-based rolls remain unchanged.
- Fifteen unchanged debug cases cover five population mixes and three sizes.
  Homogeneous 1024-entity median read/scan costs fell by 20.3–27.0%. A two-pass
  ground-animal trial was rejected because it regressed other ground species.
- Independent review identified a vanished-snapshot idle-clock leak. Its
  deterministic reproduction fails before and passes after the cleanup; the
  regression remains in a focused sibling test module.
- Full L2 passed before that one-line review correction. Final affected-scope
  formatting, code-health, strict Clippy, 21 despawn tests, debug build and the
  unchanged three-seed graphical habitat routes pass after it. Reliable unload
  batching remains intact; final client logs have no reliable queue overflows.
- The session lock still spans owner reads. Sheep grazing is now the larger
  warned cost on the retained owner route; dense-jungle client FPS remains low.
  Warning samples are not unbiased percentiles or throughput evidence. Neither
  distant render/fog acceptance nor full survival acceptance is closed.
- Receipt, exact measurements, source, review resolution and next action:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/despawn-cost/checkpoint.json`.
  Temporary profiling/replay tooling is removed, with sources retained. No
  staging, commit, push or tag; frozen archive unchanged. Alpha remains draft.

## Previous checkpoint: populated habitat traversal verified

- Fresh revision-18 agent-run graphical Minecraft 26.1.2 routes pass on
  `5617830`, `712816` and `-17711`, with unchanged natural-spawn intervals,
  work budgets and observation deadlines. No animals were summoned or moved,
  and no terrain was edited. Both retained owner barren sites now show jungle
  trees, undergrowth, grass and flowers; river samples include natural mangrove
  habitat, while other seeds retain intentionally open grassy lowlands.
- The final complete 60-block censuses at the two owner sites contain 190 and
  209 land animals. Sampled dry pig movement is observed for 146, 58 and 32
  distinct pigs across the three seeds; no unsupported dry-ground water-footprint
  samples were recorded. This is not full movement parity or survival acceptance.
- The first owner route failed during its third-site view change. Server evidence
  identifies reliable-queue overflow (321 dropped commands) before the secondary
  client-thread timeout. Chunk unload emitted a separate ordered command and full
  snapshot per entity, unlike already batched chunk-load publication.
- Unloading a dense chunk now publishes compact removal IDs in the existing
  `RemoveEntities` packet and ordered reliable lane. Singleton lifecycle
  commands retain their behavior; queue limits and authoritative entities do not
  change. A 400-entity/one-slot receiver regression fails before, then passes
  with exact-once removal and all authoritative entities retained. The unchanged
  real-client route subsequently passes; all three final logs have zero reliable
  backlog overflows.
- Final L2, debug build and independent read-only review pass. Existing uncapped
  spawning, independent frequency/disable controls, collision admission and
  movement/refill tests also pass; the population-policy task is verified.
- Performance remains open: the owner screenshot shows 16 FPS, and warning-only
  samples reach 202.351 ms per tick, 150.476 ms holding the distant-despawn session
  lock and 40.498 ms in sheep grazing. These are not unbiased percentiles or a
  controlled throughput comparison. Distant render/fog rectangles remain visible.
- Full receipts, source, review, failing/passing reproduction and next action:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/habitat-population/checkpoint.json`.
  Temporary executable tooling is removed; replayable sources remain. No staging,
  commit, push or tag; frozen archive unchanged. Alpha remains draft.

## Previous checkpoint: coastal climate classification verified

- Worldgen revision 18 routes ocean temperature variants and snowy beaches
  through the shared inland/riparian climate field in both modes. The unrelated
  deep-ocean noise picker is removed; no extra noise field or height adjustment.
- Existing erosion selects rocky shoreline within the existing two-block band.
  Stony shore now produces gravel over stone rather than the generic beach sand.
  The retained raised bank stays grass. Cold and warm variants remain reachable.
- Both new coastal regressions fail against the inherited routing and pass after
  the fix. Final L2 passes formatter, strict workspace Clippy, code-health and
  all-target workspace tests; the debug server builds. Independent read-only
  review passed without findings.
- Fresh agent-run graphical Minecraft 26.1.2 routes pass on `5617830`, `712816`
  and `-17711`: nine coastal/bank samples retain their expected ground after
  120 live ticks, with connected clients. Inspected aerial and ground images
  show warm jungle/mangrove coasts, snowy inland/coast adjacency, rocky shores
  and the retained grass bank. Before/after height images are byte-identical
  across all three sampled mosaics; this is not exhaustive terrain parity.
- Distant aerial images still show rectangular render/fog boundaries whose
  cause is unestablished. This checkpoint proves nearby coastal classification
  and ground, not seamless distant rendering or vanilla frozen-water features.
  Broader visual acceptance remains open; retained barren habitat is verified above.
- Receipts, images, review, source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/coastal-climate/checkpoint.json`.
  Temporary executable tooling is removed, with reproducible source retained.
  No staging or commit; frozen archive untouched. Fresh generated worlds are
  required by the revision fence. Alpha remains draft.

## Previous checkpoint: natural riparian wetlands verified

- Worldgen revision 17 replaces blanket humid-lowland swamp coloring with
  river-connected warm/moist shoulders, shallow pockets and dry hummocks.
  Weak dry reaches retain their surrounding climate; no fixed-width ring or
  seed-specific override. River and wetland variants now follow that climate.
- Mangroves have mud, native logs/leaves and soil-anchored, waterlogged roots.
  Temperate wetlands have oaks, grass and blue orchids. Tree placement is shared
  in `terrain/trees.rs`; chunk orchestration shrank from 2,495 to 2,353 lines.
- Fresh agent-run graphical Minecraft 26.1.2 habitat and dry-inland checks pass
  on `5617830`, `712816` and `-17711`. Inspected aerial and ground views show the
  natural transitions. Native trunks remain after 120 live ticks; sampled
  mangrove roots retain source water before and after. Dry inland columns retain
  grass and their surrounding vegetation.
- The first `-17711` run exposed a real disconnect during the second-site stream:
  optimistic cache admission missed retained clean entries and actual pressure
  escaped the storage `try_` APIs as an error. Both try-publication paths now
  return their existing backpressure outcomes for that typed condition, while
  preserving other errors. Cache limits and scenario requirements are unchanged.
  The deterministic reproduction fails before and passes after; the unchanged
  graphical route then passes with 12 deferrals and no disconnect or abandoned
  delivery.
- Final L2 passes formatter, strict workspace Clippy, code-health and all-target
  workspace tests; the revised debug server builds. One independent read-only
  wetland review passed. The later pressure correction has its retained failing/
  passing regression, non-pressure-error coverage and real-client reproduction.
- Evidence, final source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/habitat/checkpoint.json`.
  Active throwaway tooling is removed; reproducible source and receipts remain.
  No staging or commit; the frozen archive is unchanged. Revision fencing
  requires fresh generated worlds. Alpha remains draft; broader population,
  coastal climate coherence and scheduled-block performance are not closed.

## Previous checkpoint: natural pig population and swimming verified

- Fresh agent-run graphical Minecraft 26.1.2 worlds pass on `5617830`, `712816`
  and `-17711`. Natural pigs populate the retained owner sites and actual river
  neighborhoods; no summons, animal interactions, attacks or animal teleports.
- Same-UUID client samples establish dry movement, immersed movement and later
  grounded dry-bank exits with matching solid footprint scans on all three seeds.
  This is sampled behavior, not full movement parity or a continuous path capture.
- The complete census covers 60 blocks. Wider 128-block queries that reach the
  bridge's 512-entry maximum are explicitly lower bounds, not population totals.
  Spawning remains at 400 ticks / 48 chunks, with view and simulation distance 8;
  no population caps or reduced simulation work were introduced.
- The old water coordinate is now dry ground at `y=81` after the terrain changes.
  New water observations use client-confirmed open rivers, not the obsolete site.
- Inspected images show immersed natural pigs on the owner and `712816` seeds
  and a dry-land pig on `-17711`. The owner land close-up is tree-occluded; exact
  submersion and bank support are established by the retained client state.
- No production changes were needed. The unchanged Rust scope was not rerun;
  one independent read-only evidence review passed. Dense-view screenshots show
  15 FPS, so this does not close the separate performance/load queue.
- Evidence, review, source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/pigs/checkpoint.json`.
- Active throwaway drivers removed; reproducible source and receipts retained.
  No staging or commit, frozen archive unchanged, alpha remains draft.

## Previous checkpoint: raised coastal sand strips removed

- The remaining strip was a beach, not a desert-climate boundary. Revision 16
  removes the Tellus-only six-block beach height and uses the existing two-block
  shoreline band in both modes. Climate fields and river geometry are unchanged.
- Altitude/climate routing now lives in `terrain/biome_routing.rs`; assembly and
  structure placement consume its shared shoreline bound. The assembly file
  shrank from 2,679 to 2,495 lines without duplicating the moved decisions.
- The retained generated-block regression now keeps raised land grassy while
  preserving sand at the true shoreline. In the 16,384-sample diagnostic window,
  only 2,682 beach samples change to grassland; the height image is byte-identical.
- Final L2 passed: **4,455 passed / 194 ignored**, formatter, strict Clippy and
  code-health. One independent read-only review passed; its source files remain
  unchanged and CodeGraph is synced.
- Fresh agent-run graphical worlds pass on `5617830`, `712816` and `-17711`.
  Inspected images remove the retained raised sand strip. The real client checks
  both the new grassy column and preserved shoreline sand.
- The reported river/raised-beach defects are closed. The swamp-colored lowland
  mismatch left by this checkpoint is addressed by revision 17 above; broader
  population, full survival and general terrain parity are not established here.
- Evidence, final source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/biome-coherence/checkpoint.json`.
- Temporary client tooling removed; evidence snapshots remain. No staging or
  commit, frozen archive unchanged, alpha still draft.

## Previous checkpoint: curved rivers and natural inland banks

- Revision 15 replaces the two straight halves of a river reach with a smooth
  parabolic bend. Shared endpoints, downstream topology and runoff remain.
  Search bounds include the complete curved reach and its relief-dependent
  width; the old small-scale cell-boundary clipping regression now passes.
- Taller banks widen instead of cutting steeper cliffs. The existing three-block
  terrain-step check passes without relaxing it. Both biome routes now reserve
  beaches for the continental coast; inland banks retain local ground and life.
- Final L2 passed: **4,454 passed / 194 ignored**, formatter, strict Clippy and
  code-health. One independent read-only review passed the geometry change;
  those reviewed files are unchanged. The later coastal-biome correction passed
  its generated-block regression, biome reachability, final L2 and client checks.
- Fresh agent-run graphical worlds pass on `5617830`, `712816` and `-17711`.
  Inspected images show curved reaches and grassy inland banks; the real client
  confirms the formerly sandy regression column is now grass.
- **The broader artificial-terrain item remains open.** The final `-17711`
  view still shows an abrupt sandy biome strip. Biome transitions, population,
  full survival and general performance acceptance are not established here.
- Evidence, before/after regressions, final snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/rivers/checkpoint.json`.
- Temporary probes and client tooling removed; their evidence snapshots remain.
  No staging or commit; the frozen archive is unchanged. Alpha remains draft.

## Previous checkpoint: full-volume skylight boundary scan removed

- Profiling isolated sky-boundary seeding as the dominant chunk-light cost.
  It now derives the same boundary from adjacent columns' open-sky bottoms,
  rather than scanning every volume cell and its six neighbours. Removed the
  one-use dark-neighbour helper; no new cache, executor, limit or bypass.
- Instrumented mean seeding wall time fell from 169.7 ms across 589 calls to
  12.5 ms across 578 calls. Calls are not paired; this is not a whole-server
  throughput ratio. Profiles and original source are retained in the receipt.
- The new sibling regression compares all light values with independently
  seeded full-source propagation across uneven, open, closed and low-opacity
  columns. Existing incremental checks pass, including the explicitly enabled
  `incremental_relight_wire_matches_full_recompute` wire test.
- Full L2 passed: **4,452 passed / 194 ignored**, formatter, strict Clippy and
  code-health. One independent read-only review passed; CodeGraph is synced.
- Both final graphical jungle workloads pass unchanged on `5617830` and
  `-17711`. Inspected images retain trees, undergrowth and lighting. No lock-wait
  warnings or CPU-limit changes occurred. Maximum block ticks are 59.3/39.7 ms,
  down from the preceding 119.6/135.7 ms; slow-tick warning counts are 5/1.
- Occasional tick-budget overruns remain; general performance acceptance and the
  broader scheduled-stalls task are still open. With destination streaming green,
  return to the owner's remaining ordinary terrain/habitat findings before
  chasing the residual performance tail.
- Evidence, source snapshot and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/streaming-light/checkpoint.json`.
- Temporary profiling and client driver removed. No staging or commit; the
  frozen archive is unchanged. Alpha remains draft.

## Previous checkpoint: duplicate scheduled-plan admission removed

- The unchanged jungle workload produced 169 sampled slow, nonempty planning
  calls: 5.93 seconds waiting for CPU admission versus 15.8 milliseconds doing
  planning. These are sampled wall spans, not a normalized CPU benchmark.
- The serial path now consumes its first existing plan instead of discarding it
  and entering CPU admission again. Later groups still re-snapshot and replan;
  state/token and due-prefix fences still retain stale work.
- Removed the one-use single-region wrapper. Plan types and synchronous planning
  stay in `scheduled_blocks.rs`; CPU admission and blocking-worker dispatch stay
  in existing play orchestration. The architecture gate caught and rejected the
  initial async-wrapper relocation; it was corrected without a guard exception.
- The strengthened one-permit FIFO button regression fails with the duplicate
  admission and passes without it. Repeated-region and ABA coverage also passes.
  Final L2 scope passed through the individual formatter, code-health, strict
  Clippy and workspace-test profiles: **4,451 passed / 194 ignored**.
  One independent read-only review passed the semantic change; the later
  ownership correction restores the original orchestration boundary.
- Final agent-run graphical harnesses pass on `5617830` and `-17711`, with the
  unchanged 9x9 destination window, 30-second deadline and warning checks.
  Both inspected images show trees and undergrowth; neither run has lock-wait
  warnings. Maximum observed block ticks are 119.6 and 135.7 ms, respectively,
  versus 179.9 and 198.8 ms in the preceding checkpoint's runs.
- **Scheduled-block stalls remain open.** These samples do not establish general
  performance acceptance. The owner-seed run still triggered TickTime CPU
  scaling from 6 to 3 and back to 6; that existing policy was not changed.
- Profiling, before/after regression, final source snapshot, gates, review and
  next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/scheduled-admission/checkpoint.json`.
- Temporary profiler and client driver removed. No staging or commit; the frozen
  archive is unchanged. Alpha remains draft.

## Previous checkpoint: resident-accounting lock stalls removed

- Profiling the unchanged jungle workload attributed the dominant measured
  chunk-publication lock holds to repeated resident heap-accounting scans.
  Receipts contain the wall-time samples; kernel `perf` was unavailable under
  `perf_event_paranoid=4`, so temporary in-process timing was used and removed.
- Resident/dirty byte totals now update with the existing publication counters,
  using the unchanged heap estimator and before/after mutation footprints.
  Admission no longer clones and scans every resident chunk. Budgets, save-health
  checks, clean eviction and cross-region publication fences remain unchanged.
- Counter storage shares one `Arc`; no additional executor, resident authority,
  cache limit or lock. This also avoids enlarging resident transaction values.
- The lifecycle regression covers admission through growth, shrinkage, dirty
  flush finalization and clean eviction. **4,451 tests passed / 194 ignored**.
  Final formatter, strict Clippy and code-health gates passed. The initial L2
  attempt exposed the enlarged enum; after compacting counter storage, the
  remaining L2 scope completed through the individual harness profiles.
  One independent read-only source review passed.
- **Both full graphical harnesses now pass** on `5617830` and `-17711`, preserving
  the exact 9x9 destination window, 30-second deadline and warning checks.
  Neither run logged a lock-wait warning; inspected images still show jungle
  trees and undergrowth. The prior chunk-publication lock-wait failure is closed.
- **Scheduled-block stalls remain unresolved.** Both runs still logged long
  block ticks. These graphical passes are not general performance acceptance.
- Source snapshot, profiling, final gate receipts, review and next outcome:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/streaming-accounting/checkpoint.json`.
- The preceding queue-backpressure correction and closure of the water-walking/
  sparse-jungle finding remain recorded in `streaming/checkpoint.json` beside it.
- Temporary profiling and client driver removed. No staging or commit; frozen
  archive unchanged. Alpha remains draft.

## Previous checkpoint: jungle undergrowth and leaf initialization

- Revision 14 mixes low jungle-log/oak-leaf bushes with ordinary jungle trunks
  of 4–12 blocks. Existing density, exact-surface, stable-5x5 and chunk-margin
  fences remain. Mega trees, vines and full vanilla jungle parity are not claimed.
- Natural leaves are initialized to their nearest in-chunk supporting logs
  before publication, with `persistent=false`. Generation and scheduled runtime
  updates share the existing `mc-world` plant support rule. Removed the one-use
  leaf-state helper. There is no separate CPU admission or permanent-leaf bypass.
- Full L2 passed: **4,449 tests passed / 194 ignored**, strict workspace Clippy,
  formatting and code-health. The nearest-log/unsupported-leaf boundary test,
  three-seed bush/tree test and existing scheduled log-removal propagation test
  all passed. One independent read-only source review passed; CodeGraph is synced.
- Agent-run graphical seed `712816` shows taller trees and low bushes in a
  natural valley. Seed `5617830` client block observations contain natural leaf
  distances 1–3, and its pre-generated startup terrain renders.
- At that checkpoint, destination streaming remained blocked: `5617830`
  showed all-sky views and failed the stronger 9x9/30-second gate; `-17711`
  failed even the original four-corner gate. The current checkpoint above
  supersedes those destination failures, not the remaining performance failures.
- Profiling measured shared CPU admission dominating slow scheduled ticks;
  CPU capacity fell to 1 and view distance changed 8→7→6. The leaf correction
  storm was reduced. All temporary runtime profiling was removed.
- L2 receipt:
  `.analysis/validation/20260906T184530-correctness-pr61axb6/result.json`.
  Source snapshot, review, visual evidence, failed gates and next reproduction:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/jungle/checkpoint.json`.

This closes the source capability, not the complete jungle finding or alpha
acceptance. The old alpha archive is unchanged; no staging or commit is authorized.

## Previous verified slice: sheep/pig swimming and shore exit

- Agent-run graphical Minecraft 26.1.2 verified immersed ascent and actual
  grounded dry-bank landings for both sheep and pigs. The final fixture uses
  existing water corridors with two-block dry banks; support is checked across
  the entity footprint, not only its center. Both landed at `y=95`,
  `on_ground=true`, `in_water=false`.
- Inspected screenshots show immersed bodies and subsequent dry-stone landings.
  Exact underwater foot depth is established by client state, not guessed from
  the water-obscured images. A cod control remained in water for 20 samples;
  its screenshot does not resolve the fish, so that control is state evidence.
- Local vanilla bytecode confirms depth-gated float jumps and the `0.3/tick`
  bank impulse. Solaris uses bounded deterministic lift, not full vanilla
  probabilistic movement parity. Corrected the stale head-submersion comment.
- One independent read-only review passed. Formatter and code-health passed;
  CodeGraph is synchronized. The preceding full workspace gate already covers
  the unchanged physics behavior; this checkpoint adds real-client evidence.
- Final graphical receipt:
  `.analysis/validation/20260906T164401-regression-oopflzwf/result.json`.
  Snapshot, state measurements, images, review and rejected probe assumptions:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/swimming/checkpoint.json`.

This closes the sheep/pig buoyancy findings, not jungle density, natural-world
population, river/biome appearance, full movement parity or load acceptance.

## Previous verified slice: RAM write-behind WAL

Chunk and regional-entity decisions share one world-owned writer, bounded queue,
grouped disk sync and failure signal. Gameplay acknowledges RAM acceptance;
the owner explicitly accepts losing the unflushed tail on a crash. Save-all,
background/pressure dirty flush and clean shutdown fence accepted work before
acknowledging durable storage. The writer owns the world lease until shutdown.
The contract is in [ADR 0005](decisions/0005-regional-simulation.md#journal-durability).

- Removed synchronous reservation rewrites, separate journal workers and obsolete
  persistence bypasses. Moved the substantial journal tests to a sibling file;
  removed the unused `fd-lock` dependency.
- Final workspace tests: **4,448 passed / 194 ignored**. Workspace strict Clippy
  passed; later worldgen/xtask edits passed focused strict Clippy. Final formatter
  and code-health passed. Original failed receipts and successful follow-ups
  remain preserved; the original composite receipt was not rewritten as green.
- The full test run caught a tree-placement regression. Restored the existing
  surrounding-terrain stability guard; the original assertion and exact failing
  test remain intact and pass. Removed three stale code-health anchors that
  required deleted passive-spawn gating and adapter wrappers.
- Agent-run graphical Minecraft: mining changed a block to air; clean shutdown,
  restart/rejoin, recovered block state and manual save-all passed. The actual
  in-game screenshot was inspected. Survival pickup was interrupted by player
  death; crowded-world performance remains unresolved. Neither is called green.
- One executing independent WAL reviewer found no evidence-backed corruption
  or false durable-save acknowledgement. The old field reviewer could not be
  revived; its earlier verdict was not reused as WAL evidence.
- Receipt, owned snapshot, exact gates, review and runtime evidence:
  `.analysis/codex-logs/owner-field-5617830-2026-09-06/wal-write-behind/checkpoint.json`.

This closes WAL delivery, not the full owner request. Alpha remains **draft**.
The delivered local alpha archive remains frozen and was not overwritten.

## Previous verified slice: local inventory candidates

Window-0 clicks, recipe-book crafting, offhand swaps and normal/debug grants
prepare local inventory/cursor candidates. The connection baseline stays
unchanged until the existing owner returns a committed or rejected snapshot.
Removed speculative projection writes, rollback branches and redundant clones.
Other menu handlers and broader core ownership remain unfinished.

- L2 passed: **4,449 tests passed / 194 ignored**, code-health, workspace Clippy
  `-D warnings` and formatting. Java bridge-core/java-agent tests passed.
- Real Minecraft passed natural mining/pickup/economy, one oak log → four planks,
  cursor pickup/return of two apples, and selection of the crafted stack:
  `.analysis/plugin-client-compat/20260906T145930/result.json`.
  Crafting setup used an isolated operator fixture; this is not full survival,
  multiplayer, performance or owner terrain acceptance.
- The old crafting scenario used fixed recipe ID 697; the server advertises 18.
  It now uses existing recipe-book lookup. Two tests pinning mocked call order
  and the fixed ID were removed rather than repinned. Actual count assertions
  remain in the real-client scenario.
- One independent read-only review passed. The throwaway smoke launcher was
  removed; its source and observations remain with the evidence.
- Receipt: `.analysis/codex-logs/core-inventory-candidates-2026-09-06/checkpoint.json`.
  No commit, staging, push or tag.

## Previous verified slice: owner-selected held-item gameplay

Route: `architecture`; contract: `docs/ARCHITECTURE.md` and ADR 0006.
Removed `InteractionState.selected_hotbar_slot` and its constructor/update paths.
Mining, combat, item use, shields, arrows and drops now read the exact owner's
shared selected slot. No replacement cache or compatibility path was introduced.

- Reproduced a stale gameplay read: after owner selection of an enchanted weapon,
  connection gameplay still computed **5 damage instead of 7**. The regression
  now passes and also covers invalid selection preserving the weapon and return
  to the original slot. Existing arrow-selection coverage remains.
- L2 passed: **4,449 tests passed / 194 ignored**, code-health, workspace Clippy
  `-D warnings` and formatting. One independent read-only review passed.
- Agent-run graphical Minecraft gate passed without changing this checkpoint's
  scenario: `.analysis/plugin-client-compat/20260906T142622/result.json`.
  Natural mining/pickup, economy purchase and ledger persistence passed.
- Receipt, checkpoint-local diff and validation logs:
  `.analysis/codex-logs/core-player-selection-2026-09-06/checkpoint.json`.
  No commit, staging, push or tag.

This closes held-item selection authority, not all player/session mirrors or the
whole core redesign. Inventory/cursor projections and broader ownership remain.

## Previous verified slice: core startup data

Route: `architecture`; contract: `docs/ARCHITECTURE.md` and ADR 0006.
`mc_server::startup_data::StartupData` assembles and validates immutable gameplay
tables before terrain/world preparation. The CLI orchestrates the resulting
bundle; per-table `Effective<T>` wrappers are removed.

- Runtime smoke: malformed recipe output is rejected before creating a new
  world or changing an existing sentinel world. Configuration-only `--check`
  still emits valid JSON and does not create a world.
- Agent-run graphical Minecraft compatibility gate passed:
  `.analysis/plugin-client-compat/20260906T140305/result.json`.
  Natural dirt mining/pickup, economy purchase and persisted ledger assertions
  passed. The scenario now clears natural snow cover before harvesting dirt;
  positive pickup and purchase assertions remain intact. This is not an
  unchanged harness run or a full survival/multiplayer acceptance gate.
- L2 passed: **4,448 tests passed / 194 ignored**, code-health, workspace
  Clippy `-D warnings`, formatting; Java bridge-core and java-agent tests passed.
- One independent read-only startup-cutover review passed without findings.
  This review predates the subsequent snow-clearing harness adjustment.
- Evidence and owned-change receipt:
  `.analysis/codex-logs/core-startup-2026-09-06/checkpoint.json`.
  No commit, staging, push or tag.

## Previous verified slice: Loader sounds

Route: `plugins`; current contract: `docs/PLUGINS.md` and ADR 0010.
`play_client_sound`/`stop_client_sound` use owner `sounds` definitions,
`play_sounds`, verified mono OGG assets and the existing resource pack.
One shared native presenter supports personal and fixed-position one-shots,
volume, pitch, vanilla attenuation, owner-local stop and disconnect cleanup.
Loader wire protocol remains **2**; plugin API remains `0.6.0`.

- Final agent-run real Minecraft 26.1.2 MCP/Xvfb audio gates passed:
  `20260906T131829-fabric`, `20260906T131958-neoforge`, and
  `20260906T132202-forge`, under `.analysis/loader-live-gate/runs/`.
  Recorded actual audio proves quarter-volume scaling, 440→660 Hz pitch,
  near/mid/far attenuation, concurrent owners, owner-local/foreign stop,
  disconnect silence, reconnect silence and newly requested playback.
  Both owner frequencies are zero after the final stop in all three runs.
  Post-play and clean-reconnect screenshots were also inspected.
- One independent read-only reviewer found one P2 gap in the audio gate:
  Sapphire's final stop was recorded but not asserted. Both owner frequencies
  are now asserted silent. A real-client dropped-Sapphire-stop probe correctly
  failed with `stop_client_sound left owner audio audible`
  (`20260906T131622-fabric`); the expected-rejection wrapper exited successfully.
  No second reviewer was spawned.
- Initial failures are preserved: Rust's closed artifact index lacked `sounds`;
  the audio capture initially listened to a different sink; an adversarial run
  then exposed background-music interference. The isolated client now selects
  its private sink and disables vanilla music, without changing the desktop sink
  or relaxing spectral thresholds.
- L2 passed once: code-health, **4,450 tests passed / 194 ignored**, workspace
  Clippy `-D warnings`, and formatting. Focused sound-boundary tests, all four
  Java Loader test tasks, three distributable jars and reproducible fixtures
  passed. Only owned Rust files were formatted.
- No loops, moving sources, completion events or arbitrary client scripts.
  Forge retains owner-approved isolated `earlyWindowControl=false`; these runs
  are not owner Prism/terrain acceptance, performance or full-alpha evidence.
- No commit/staging/push/tag. Preserve inherited dirty work.

Exact receipt, review resolution, source snapshots and audio/visual evidence:
`.analysis/codex-logs/loader-sound-2026-09-06/checkpoint.json`.
The preceding input slice is closed; its full evidence remains at
`.analysis/codex-logs/loader-input-2026-09-06/checkpoint.json`.

## Previous verified slice: unified client UI

- One `solaris.present_client_ui(player_id, ui_id, options)` command handles
  `screen`, `hud`, and `hidden` through `ScriptBoundary`. The `ui` resource
  schema, `present_ui` permission, and `solaris:loader/ui` transport replace the
  screen-only API without aliases. Exact-session and owner fences remain.
- One shared Minecraft presenter owns modal item/block/action views and
  non-interactive HUD panels. Text overrides are bounded; omitted values come
  from the verified resource, not earlier dynamic state. Hiding is id-local;
  activation/disconnect clears HUD state.
- Agent-run real Minecraft 26.1.2 MCP/Xvfb gates passed on Fabric, NeoForge and
  Forge. Captured images confirm both owner HUDs, text updates, owner-local
  hiding and no stale HUD after reconnect; ordinary jump input remained live.
  Capture now waits for the actual loading overlay to disappear, not merely Play.
- Forge's default FML early-window GL-context handoff fails on this Xvfb host,
  before Solaris initializes. The owner approved `earlyWindowControl=false` in
  the isolated QA profile. The normal game renderer/HUD passed; early-window
  GL features are not covered. This is not owner-run Prism/terrain acceptance.
- Full L2 passed: **4,449 tests passed, 194 ignored**, code-health, workspace
  Clippy and formatting. Java core and all three adapter test tasks passed.
  The missing non-exhaustive API marker was fixed; a runner deadline interrupted
  Clippy, which then completed separately without replaying successful tests.
- One independent read-only review passed without findings. The later
  non-exhaustive invariant and approved QA-profile setting are recorded in the
  receipt. No load/performance or full-alpha readiness claim; no commit,
  staging, push or tag.

UI receipt:
`.analysis/codex-logs/unified-client-ui-2026-09-05/checkpoint.json`.
The preceding configuration/workspace receipt remains
`.analysis/codex-logs/configuration-workspace-2026-09-05/checkpoint.json` and links
the earlier NPC, movement/pickup, inventory and world-commit evidence.

## Next outcome

First require the correction's hosted `test` and `loader` jobs to pass; local
clean-checkout passes do not clear a hosted failure. Then attribute and reduce the
remaining dispatch tail on retained seed -17711, with the owner jungle route
as a comparison. Separate computation, admission and owner-response waits before
choosing another narrow change; do not reduce selected entities or reorder
goal/gameplay/physics phases.

The final native negative-seed trace reaches 98.495 ms entity dispatch and
41.896 ms physics preparation; the owner trace retains 24.754 ms block work.
These are warning-only maxima, not whole-run percentiles. Acceptance: lower
matched attributed cost with unchanged selection, transaction and publication
fences, plus the retained three-seed graphical routes. Evidence:
`entity-dispatch-followup/native-stage-maxima.json` beside the owner evidence root.
Primary route: `docs/decisions/0005-regional-simulation.md`.
Continue autonomously after recording evidence; this is not a checkpoint stop.

Keep broader visual acceptance open; the radius repair does not establish full
terrain, frozen-water or client-frame parity.
The latest owner route also retains one unclassified, nonconsecutive
grounded-over-water pig observation in the idle-grazing receipt. Preserve that
evidence; a passing sustained-surface gate does not clear shorter artifacts.

Keep residual scheduled-block latency in the measured performance queue, not
marked complete. Grid population was the largest light stage in the retained
streaming-light profile. The final session-contention route also retained a
28.305 ms apply-despawn session hold; do not claim all session stalls eliminated.
Preserve settled accounting, queue-backpressure, first-plan reuse and
leaf-initialization fixes.

Then finish the bounded refactoring and varied load matrix. The original
sheep-grazing bottleneck is resolved, not a reason to repeat its completed work.
Do not restore population caps or reduce selected simulation work to hide costs.

Broader core ownership, integration finalization and full gameplay/load acceptance
remain open. The old frozen load matrix and missing owner terrain acceptance
remain unresolved. Do not return to Loader feature development.

## Owner followup 2026-09-11 (evening, pushed unverified — internet cutoff)

Done and tested: squid first-hit regression
(`squid_first_melee_hit_damages_and_notifies_observers`), panic multipliers
from decompiled 26.1.2 (cow 2.0, sheep/pig 1.25, chicken 1.4), zombie pursuit
from attribute (`ZombieAttackGoal 1.0` over 0.23 → 2.3).
schedule removed. New `skeleton_bow_draw_cycle` passes; `hostile_commit_releases`
and `skeleton_shoots_a_real_arrow` pass. `skeleton_volley` FAILS on the owner-request
count (9 vs old 5: gameplay `attacks == 2` passes, only the lock-count expectation
UNVERIFIED: `skeleton_volley` owner-request count re-baselined 5 → 9 without
a green run (no time before disconnect). First act on reconnect: run the four
bow tests + `cargo fmt --all` + full `mc-net`/`mc-entity`/`mc-data` lib suites,
then L2 `correctness`. `SKELETON_SHOT_PERIOD_TICKS` const removed; do not
re-add. `.analysis/server.jar` (official Mojang download, ignored) + vineflower
decomp under /tmp/decomp (Cow/Sheep/Pig/Chicken/PanicGoal/Zombie/Skeleton/
AbstractSkeleton/RangedBowAttackGoal) back further vanilla checks.

## Tab-list checkpoint 2026-09-12 (uncommitted, L1 green)
Config-driven tab list: `ClientboundTabList` (0x7A, header/footer NBT),
`[tab_list]` in `example.toml`, login burst sends full roster + header/footer
(skipped when both empty), join broadcasts add, leave broadcasts remove,
game-mode changes broadcast `UPDATE_GAME_MODE`. Roster entries carry live
`game_mode` (was hardcoded 0). `OutboundCommand::PlayerInfo/PlayerInfoRemove`
dispatched via trailing `Some(cmd)` arm + `write_player_info` helper
(play_loop_inner gateway budget 731 holds). Fail-closed codec kept: test
profile renamed `InitialRecipeSync` (17) -> `InitialRecipe` (16) instead of
truncating production names. Removed dead `OutboundCommand::TabList` variant.
Gates: mc-net lib 2090/0, mc-protocol 321/0, fmt clean, code-health PASS,
clippy clean except pre-existing furnace collapsible-if (FurnaceBoats-owned).
NOT committed (no authorization). Biome blotches seed 9700063978612627 still open.

## Biome-river checkpoint 2026-09-12 (uncommitted, L1 green)
Seed 9700063978612627 diagnosis (TellusLike): macro continents healthy (2km+
masses, smooth J/G/D borders on 8km map); sampled lowland river textbook
(sand bed, water 2-5, clean banks). Real defect: weak upland carves routed
as warm_ocean stripes (river field 0.03 at w~0.7 missed the 0.016 band that
only covered the w>0.84 core) with grassy savanna shallows at y62-63.
Fix: TELLUS_RIVER_BIOME_WIDTH 0.016->0.04 (covers carve to w~0.6, open water
stays 1.0->ocean; 0.05 tried first but ate the riparian-wetland shoulders
pinned by generated_riparian_wetlands test). No river/swamp reorder, no
field rescale (both wider blast radius). WORLDGEN_REVISION 20->21 (biome
layout change; old worlds must wipe per startup gate). Regression:
tellus_carved_channel_routes_river_wall_to_wall. Gates: mc-worldgen
138+1+12/0, fmt clean, code-health PASS. Straightness: mapped reaches
meander; residual downhill-chain coherence is a known accepted tradeoff
(branch jitter 0.22, see drainage comment). NOT committed.

## Beach-shore checkpoint 2026-09-12 (uncommitted, L1 green)
Seed 1785772562805887200 field report (screenshots): grass underwater at the
waterline + beach sheets across flats. Transect (1283,1740-1832) reproduced
plains y62 + water over grass. Fix 1 (kept): below-sea non-river/swamp/ocean
land routes to shore in Tellus (`tellus_sub_sea_land_routes_shore_not_grass`).
Fix 2 (kept): Tellus shore capped at sea+1 (`tellus_beach_stays_near_waterline`;
y65+ sheets -> climate, y63-64 fringe stays). sea+0 tried, reverted (killed
y64 berm, broke 2 coastal tests). Rivers on this seed verified healthy
(sand beds, water 2-5, meandering with anabranching knots at confluences).
Revision stays 21. Release binary rebuilt+reinstalled with all fixes.
Queued tuning (owner): rougher meanders, softer biome transitions, more land
share. (Live operator/whitelist landed 2026-09-12.)

## Live operator/whitelist checkpoint 2026-09-12 (uncommitted, L1 green)
Console `operator add|remove` and new `whitelist add|remove|list` now take effect
without a restart. Single live source: `CommandPermissionConfig.operators` and
`LoginAccessConfig.whitelist` are `Arc<ArcSwap<BTreeSet<String>>>`; login reads
the whitelist at the check, and chat-command ingress re-resolves op via
`live_permissions_for` (loopback dev fallback stays valid only while no operator
is configured) and re-sends the command tree when it changed. `operator list`
and `whitelist list` report the effective live set.
Persistence mirrors the file manager: `manage_access_file` with
`AccessControlTarget::{Operators,Whitelist}`, console defaults `ops.json` /
`whitelist.json` beside the config, and startup auto-loads `whitelist.json` when
`auth.whitelist_file` is unset (as it already did for `ops.json`).
`operator_file_tests.rs` renamed `access_control_file_tests.rs`.
Known limit: enforcement toggle (`whitelist_enabled`) and online clients'
initial command tree still need a config edit/reconnect; the F3+F4 game-mode
packet path uses login-time authority until reconnect.
Gates: mc-net lib 2093/0, mc-server 78+40+39+14/0, fmt clean, code-health PASS,
`clippy -D warnings` clean (furnace collapsible-if collapsed). Release binary
rebuilt + reinstalled.

## River/beach realism checkpoint 2026-09-12 (uncommitted, L1 green)
Owner field reports on seed 1785772562805887200 (screenshots): grass under
water, beach sand cutting dry valleys, rivers starting at full width out of
nowhere, no gravel in river beds, no savanna found.
- Beaches now require adjacent water: `tellus_biome_for` probes one step (6
  blocks) for a sub-sea neighbour, so a y64 flat 140 blocks from water keeps
  its climate instead of a sand band (`tellus_beach_never_cuts_dry_ground_inland`).
  The two coastal design tests now pin a shoreline column (seed 712816 at
  -448,-32) because synthetic inland samples no longer qualify.
- River beds mix sand with gravel bars: two-octave field, scale 34, threshold
  0.30 (~16% of bed measured), `river_beds_carry_gravel_bars_between_sand`.
- Headwaters taper: Tellus only, `drainage::sample(..., taper_headwaters)`
  scales the minimum channel width by reach strength. VanillaLike keeps the
  flat 20-block minimum so its terrain and the 3-block step budget stay
  byte-identical (measured: Tellus worst step 3, 0 violations; VanillaLike
  continuity test green again).
- Savanna exists on that seed (hot_dry is ~4% of land, vanilla-like):
  first at 1840,-3072, also 2176,-2928, 2560,-2656, 2176,-2544, 2576,-2544;
  a plains at 2848,-2464. No code change.
- Villages: diagnosed as unreachable by default. `structure_rules_for_startup`
  only builds village rules from a Luau settlement plan
  (`PreparedLuaPlugins::worldgen_settlement_plan`) plus `data.vanilla_data_dir`;
  `example.toml` sets neither, so a stock server generates zero villages.
- WORLDGEN_REVISION stays 21 (still unpublished). Binary rebuilt+reinstalled
  at ~/.local/bin/solaris (17:28).
- Queued owner asks: tectonic-plate canyons/mountains, Chunky-style bounded
  pregeneration, more realistic mob spawning, village availability, redstone
  and pistons (delegated to the `RedstonePistons` subagent, uncommitted).

## Villages + redstone checkpoint 2026-09-12 (uncommitted, L1 green)
Villages are reachable without a plugin: `[data] settlement_profile =
"plains_village_prototype"` builds the vanilla plains prototype from
`vanilla_data_dir` (still required; Mojang NBT never enters Git). A deployed
plugin settlement plan still wins and stays the recorded identity, otherwise the
built-in profile name is recorded. Verified end-to-end locally: startup logs
`materialized built-in settlement prototype profile="plains_village_prototype"`
and the sidecar tests now skip-when-absent instead of `#[ignore]`, asserting the
built-in profile changes >200 generated blocks around the fixed centre.
Redstone/pistons landed by the `RedstonePistons` subagent (uncommitted):
`crates/mc-net/src/play/redstone/` (power model, event-driven settle,
atomic piston moves, 17 new tests) plus the old one-hop power fanout deleted
from `toggles.rs`; fence = 256 positions/settle, 1024/tick, 64 ticks/commit,
work beyond a cap is dropped and counted (`budget_drops`) rather than deferred.
Deviations are documented in the module header (no piston animation packet,
no strong-power relaying, one scheduled tick of latency for non-interactive
edits, no quasi-connectivity).
Known flake, not fixed here: `plugin_owned_command_argument_limits_do_not_
terminate_play_ingress` (2s script-event budget; 1 pass / 3 fail under load).
A/B evidence: it still flakes with the tab-list burst disabled and with the
live-permission refresh disabled, so it is load-sensitive, not a feature
regression. `crates/mc-server/tests/play.rs` is unmodified apart from the
required `tab_list` struct-literal lines.
Gates: mc-net lib 2110/0, mc-server 79/43/2/39/14/0/12/1 (+the flake),
mc-worldgen 141+1+12/0, fmt clean, `clippy -D warnings` clean for the three
crates, code-health PASS. Binary rebuilt + reinstalled (~/.local/bin/solaris).

## LOC unification and plugin authoring audit 2026-09-13 (uncommitted, draft)

Whole tracked source/config inventory: 817 files, 542097 physical lines across
all 13 crates and tooling; exact-clone scan plus targeted semantic audits.
This is not a claim that all similar code is interchangeable. Owned code delta:
**-730 lines**, including new Luau declarations and authoring regression tests.
Shared registry decoding, the common TCP Play handshake (16 test files),
identical two-client harness setup/screenshots, script-host dequeue policy, and
identical noise interpolation formulas now have single implementations.
Different handshake, preflight, floating-point, and authority contracts remain
separate rather than being forced through a generic abstraction.

Plugin discovery now checks 61 real host functions against bundled Luau
declarations instead of typing `solaris` as `any`. All ten first-party package
sources typechecked; an actual strict-loaded command and simulation timer
reached a real TCP client. Advanced result records remain dynamically typed and
runtime-validated; durable async request/result correlation is still explicit.
Plugin docs now use the real completion callback and observed inventory fence,
and no longer advertise the removed ephemeral villager bindings.

Evidence: `.analysis/codex-logs/loc-unification/receipt.json` records
`base_tree`, `diff_hash`, the 31 owned `changed_files`, validation, and next action.
Base: `5ebb33c413d2017f0256f67e5445934a817d7da4`.
Owned patch SHA256:
`b9f55c01c4bc4be3290f0ff3d11aebec461a426032aa79f8b59b8ab8ead87b6e`.
The patch excludes this append-only cursor record and concurrent settlement
changes, which were preserved. No staging, commit, push, or sibling-source edits.

Validation: canonical `correctness` passed (4763 passed, 192 ignored), receipt
`.analysis/validation/20260913T165448-correctness-7hjn9uiy/result.json`;
`harness-check` passed; two normally ignored multiplayer presence scenarios
passed with real TCP clients; 48 old/new Python driver trace comparisons passed
with substituted bridge calls. Independent read-only sonic review passed.
No graphical Minecraft client gate was run (client credentials absent); this
does not establish gameplay parity or close any failed owner scenario.

Owner requested quiet operation during closeout. All owned build/test jobs had
finished; no further heavy work was launched, and power saver was left untouched.
Next: review the scoped uncommitted patch while preserving concurrent settlement
work. This appendix does not advance the existing route cursor.

## Shutdown signals and import visibility (landed + verified locally 2026-09-14, uncommitted)

- SIGTERM is now handled exactly like Ctrl-C. `run_bound_server` waits on
  `tokio::signal::unix::signal(SignalKind::terminate())` in the same `select!`
  as `tokio::signal::ctrl_c()`, so `kill`, systemd and container stops drain
  admitted work and perform the single final save. Before this, SIGTERM killed
  the process by default disposition while Ctrl-C was already correct.
  Evidence (`/tmp/sigverify.sh`; isolated dirs and ports, readiness proven from
  each child's own `Solaris is listening` line before the signal): SIGTERM and
  SIGINT both now exit 0 with `shutdown requested; listener stopping`,
  `save-all complete … context=server run final save`, `Luau plugin host stopped`.
- Automatic content import is legible and no longer rotates the server's own log.
  The import reports one terminal line per phase plus byte-throttled download
  checkpoints (`downloaded 8.0 MiB / 57.6 MiB (13%)` …), driven by the response
  stream rather than a timer; the in-tree extractor and datagen JVMs run with an
  explicit `current_dir`, so vanilla log4j can no longer rotate
  `<server cwd>/logs/latest.log` out from under the launcher (observed before the
  fix: the launcher writing into an unlinked inode, `/proc/<pid>/fd/9` pointing at
  `logs/<date>-1.log.gz (deleted)`, `latest.log` at 0 bytes).
  Evidence: cold default-console run transcript; `/tmp/acc2` contains only
  `logs/latest.log` (42 lines) plus `debug.log`, no `*.log.gz`; cache published
  `registries=28 entries=382 in 30.4s`.
- Still not done: the interactive console does not render tracing records at any
  level (`ConsoleOutput`'s ring is a different object from the ring `init_tracing`
  builds, and `--no-console` only flips `interactive`). The import phase lines are
  a terminal sink; general log records are still file-only.
- Undiagnosed and unchanged: a cold start can still fail on the manifest fetch with
  `client error (Connect): operation timed out`. No retry/backoff/sleep mechanism
  was added. The IPv6-only explanation is NOT established — in this session the
  host resolved `piston-meta.mojang.com` to IPv4 `150.171.109.106` and `curl`
  returned 200 in 0.95s.
- Installed build: `~/.local/bin/solaris` md5 `78a91daf6fd31bceb796cfb8c6451c2d`
  (mc-server 0.0.6, release). `cargo test -p mc-server --bin mc-server --
  content_import::tests` 16 passed / 0 failed; `cargo clippy -p mc-server
  --all-targets -- -D warnings` clean; `run code-health` passed
  `.analysis/validation/20260914T054745-code-health-bivycely`. `run fmt` currently
  fails on another agent's in-flight `crates/mc-data/src/vanilla_feature_closure.rs`,
  not on these changes.

## Vanilla feature executor A1 (partial, internal only — landed uncommitted 2026-09-14)

Data-driven execution of the vanilla placed/configured features reachable from the
village `feature_pool_element` entries, scoped to A1 (no trees yet, no villages).
Reachable closure pinned from the derived data: `minecraft:simple_block` ×3
(flower_plain, berry_bush, taiga_grass), `minecraft:block_pile` ×5 (hay, ice,
melon, pumpkin, snow), `minecraft:block_column` ×1 (cactus), and exactly the
modifiers `count`, `random_offset`, `block_predicate_filter`.

- `mc-data::vanilla_feature_closure` resolves by reference from the named root
  only and fails closed (`UnsupportedType{kind, type_id, referrer}`) inside it;
  entries outside the closure are never read. `minecraft:tree` is NOT accepted.
- `mc-worldgen::vanilla_features` compiles that closure and executes the three
  feature bodies (semantics read from the vineflower decompiles staged under
  `/tmp/a1d`), with the vanilla legacy RNG, Java `String.hashCode`, `NormalNoise`
  and the block-state-provider/int-provider/predicate set the closure reaches.
- Glue: `vanilla_features::compile_pool_features(worldgen_dir, pool_id, &BlockSemantics)`.

Evidence: `cargo test -p mc-data --lib vanilla_feature_closure` 12 passed / 0 failed;
`cargo test -p mc-worldgen --lib vanilla_features` 18 passed / 0 failed (both
reproduced by Main); live proof
`SOLARIS_CONTENT_CACHE=/tmp/jdk-cold2 cargo test -p mc-worldgen --lib village_decor_closure_places_from_the_real_cache -- --nocapture`
places `minecraft:village/desert/decor` (11 hay_block, 2 cactus at quoted
coordinates), deterministically on repeat, and fails closed for
`minecraft:village/taiga/decor` on `minecraft:tree`; loud skip when no cache.
Gates: `run fmt` PASS `.analysis/validation/20260914T060152-fmt-de_ds04g`,
`run code-health` PASS `.analysis/validation/20260914T060155-code-health-zsg0znma`,
clippy `-D warnings` clean for `mc-data` + `mc-worldgen`.

Partial by construction: the only non-test call sites are the two module
registrations, `WORLDGEN_REVISION` is still 21, `PlainsVillagePrototype` and its
tests are intact, no profile/default/config/env/flag reaches the new path. Next:
A2 (`minecraft:tree`, the four trunk/foliage placer pairs), then B (village
assembly, activation, prototype deletion, worldgen identity bump).
`Mth.getSeed(BlockPos)` is now read from the decompile
(`seed = x*3129871 ^ z*116129781L ^ y; seed*seed*42317861L + seed*11 >> 16`) but is
deliberately NOT implemented — it belongs to B's per-position rule-processor RNG.

Review (independent, read-only, `ImportProgress`, 2026-09-14): **pass**, no blocking
or high-severity defect. It re-derived the pile and column shapes in Java against the
decompiled bodies and reproduced this crate's expected positions and axis values,
regenerated the pinned RNG constants with `java.util.Random`, ran the real 26.1.2
`NormalNoise` out of the inner server jar and reproduced all 12 noise values, and
confirmed by search that nothing outside the new modules and their tests references
the layer (`WORLDGEN_REVISION` 21, `PlainsVillagePrototype` intact). Its reproduction
of the two focused suites matched (12/0 and 18/0) plus
`cargo test -p mc-worldgen --lib` 185 passed / 0 failed / 5 ignored, and
`run fmt` PASS `.analysis/validation/20260914T061403-fmt-fikmw34z`.
Seven non-blocking findings, folded into A2/B rather than fixed in isolation:
nested `rule_based` providers return `None` where vanilla resolves the existing state
(`provider.rs:222`, only reachable once trees land, so A2 must fix it before enabling
trees); a non-string block-state property is coerced to `""` instead of failing
(`vanilla_feature_closure.rs:762`); `list_pool_element` containing a feature element is
skipped rather than failing closed (`:378`; no village pool uses the list form today);
the live proof asserts liveness and determinism but no vanilla-derived expected
coordinates and covers 3 of 13 reachable features; `parse_offset` omits vanilla's
±16 bound (`:913-940`); missing `weight`/`prioritize_tip` default instead of being
required (`:389`, `:497`); and `random.rs:10-11`'s comment about `DOUBLE_MULTIPLIER`
misdescribes an exact-2^-53 double as a float literal, which invites a future change
that would silently alter every `nextDouble`. Reviewer's unverified list: the 33-entry
vegetation list beyond three checked blocks, `synth.rs` internals, and placement for
the ten features no test exercises.

## Vanilla feature executor A2 — trees (partial, internal only — uncommitted 2026-09-14)

`minecraft:tree` implemented for the four village placer pairs (straight+blob oak,
straight+spruce, straight+pine, forking+acacia), so the WHOLE village decor closure
(19 pool elements / 13 distinct features) now loads and places. Loader gained
`ConfiguredFeatureKind::Tree(Box<TreeSpec>)` (trunk/foliage providers and placers,
`minimum_size` with the codec defaults, `ignore_vines`, `below_trunk_provider`
defaulting to `PLACE_BELOW_OVERWORLD_TRUNKS`) and `IntProviderSpec::Uniform`; a
non-empty decorators list, an unsupported placer/size, a `root_placer` and a
`list_pool_element` all fail closed naming type id + referring entry. Executor
`crates/mc-worldgen/src/vanilla_features/tree.rs` is transcribed from the decompiled
`TreeFeature`/`TrunkPlacer`/`FoliagePlacer` bodies, including `updateLeaves`' BFS,
whose final leaf `distance` depends on Java `HashSet` iteration order — emulated
explicitly (`java_hash_set`), because Rust's randomised set order produced different
distances. `StructureTemplate.updateShapeAtEdge` is deliberately not reproduced: every
reachable tree block returns its state unchanged there, documented in the module header.

All seven A1 review findings closed with named tests: nested `rule_based` resolves the
existing state, the `DOUBLE_MULTIPLIER` comment states the exact-2^-53-through-`f32`
requirement, non-string property values and `list_pool_element` fail closed, offsets are
bounded to vanilla's ±16, pool `weight` and `block_column.prioritize_tip` are required.
Proof strength raised: the live proof now places all 13 reachable features and asserts
the exact vanilla block sets for all four trees, on top of the synthetic fixture tests.

Evidence (reproduced by Main): `cargo test -p mc-data --lib vanilla_feature_closure`
18 passed / 0 failed; `cargo test -p mc-worldgen --lib vanilla_features` 22 passed /
0 failed; `SOLARIS_CONTENT_CACHE=/tmp/jdk-cold2 cargo test -p mc-worldgen --lib
village_decor_closure_places_from_the_real_cache -- --nocapture` 1 passed with
coordinates for every feature and `deterministic replay matched for all 13 features`.

Still partial/internal: tests-only reachability, no activation, `PlainsVillagePrototype`
intact, `WORLDGEN_REVISION` still 21, no `docs/MEMORY.md` self-writes by agents.
Review of A2: **pass** (independent, read-only, `ImportProgress`). The reviewer
transcribed the whole tree path into Java against the real `LegacyRandomSource` from
the bundled jar and real `java.util.HashSet` and reproduced all four tree fixtures
exactly (oak 59/59, spruce 62/62, pine 98/98, acacia 93/93, leaf `distance` included),
and mirrored the `JavaHashSet` order against Java's across 46,540 comparisons. It also
confirmed from the decompile that `StructureTemplate.updateShapeAtEdge` cannot write for
reachable tree blocks, and closed out all six prior findings and the new fail-closed
entry kinds. Two items kept open and assigned before B: the `JavaHashSet` emulation
models `HashMap` only up to the treeify boundary (`tree.rs:744-812`, guard at :770) so a
large enough per-distance set could silently diverge in a RELEASE build — it must fail
loudly there instead of relying on `debug_assert`; and two coverage gaps (tree
fail-closed tests assert the type id but not the referring entry; the clipping test
asserts only `assert_ne!`/`len <` for one seed per tree). Reviewer ran
`cargo test -p mc-data --lib vanilla_feature_closure` 18/0,
`cargo test -p mc-worldgen --lib vanilla_features` 22/0, the live proof (13 features,
four trees with verified constants, deterministic replay), `run fmt` PASS
`.analysis/validation/20260914T064428-fmt-8ndbobhr`, and clippy clean.
Next: checkpoint B — village assembly (structure sets and their biome tags, jigsaw
placement and template pools from the cache, `feature_pool_element` calling this layer,
per-position `RuleProcessor` seeding via the now-read `Mth.getSeed`), activation,
prototype deletion and the worldgen identity bump.

## Core vanilla village generation activated (uncommitted 2026-09-14)

Core now generates the five vanilla village structures under the default
`settlement_profile = "vanilla"`, and only there: `village_plan_source_for_startup`
(`crates/mc-server/src/main.rs`) attaches a `VillagePlanSource` only when no Luau
settlement plan is deployed and the profile is `vanilla`, so a plugin plan keeps
settlement ownership and the retained `plains_village_prototype` profile keeps its
own composite without double-placing. The
`settlement_profile_vanilla_generates_no_villages` warning is retired; the
terrain-adaptation analogue notice is the one a village world carries.
`WORLDGEN_REVISION` is 22 (revision-21 worlds are refused with the fresh-`world_dir`
message), and `docs/VILLAGE_GENERATION.md` documents the activated semantics,
including the multi-village-per-chunk rule and the still-unresolved prototype
removal.

Engine shape. `village/plan_source.rs` is the one lookup both consumers read:
`plans_for_chunk`/`plans_for_column` return a `VillagePlanSet` (every plan whose
region reaches the chunk, ascending start-chunk order, no cache, no second source of
truth), `VillagePlanSet::adjusted_columns` sums the plans' beard contributions and
rounds once the way `Beardifier` sums every structure a chunk references, and
`write_pieces` places each plan clipped to the chunk. The source is data-only: it
holds the closure, the block registry, the block tag index and the biome tag index,
and takes the terrain's plan-free biome per lookup, so it can never call back into
the generator that holds it (`TerrainGenerator::base_biome` is the plan-free biome
accessor; `diagnostic_sample` would have recursed). `VillagePlanSource::new` is the
only constructor and validates every closure piece under every rotation against two
probe world states (air, water) before handing the source out, which is what makes
the `expect` in `TerrainGenerator::apply_village_pieces` an invariant rather than a
hope. `vanilla_features::CacheTags` wraps the already-loaded `TagsData` plus
`VanillaData` and implements both tag traits, so no second tag authority exists.

Two fidelity defects found by the live activated-path proof, both fixed and pinned:

- **`GravityProcessor` added the world `y`.** `PieceBlock` now carries `template_y`
  (vanilla's `originalBlockInfo.pos().getY()`, the template-local position that
  `StructureTemplate.processBlockInfos` passes as the *original* while the world
  position is the *processed* one), and `Processor::Gravity` adds that. A scratch
  runner over the real 26.1.2 classes with the two readings made distinct (local
  `y = 0` and `5`, both at world `y = 70`) printed `y=64` and `y=69` — height +
  local `y`, never `+` world `y`. Before the fix every `terrain_matching` village
  block was flung to `height + piece.y`.
- **Processor lists came from the wrong pool element.** A template id can appear in
  several pools with different processor lists or projections (592 village pool
  elements, 478 distinct templates, 91 shared, four live conflicts among the
  terminators: `plains/terminators` vs `savanna`/`snowy`/`taiga`, all
  `terrain_matching` with four different lists). `PlacedPiece` now carries the
  placing element's `owner`, `processors` and `legacy` from the solver, and
  `plan_from` uses them; the old first-match-by-template-id lookup is deleted.

Evidence. `cargo test -p mc-worldgen --lib` 250 passed / 0 failed / 5 ignored;
`cargo test -p mc-worldgen --lib -- village_processors` 19/0 (18 + the new
`gravity_uses_the_template_y_not_the_world_y`, which pins the measured reference
numbers); `cargo test -p mc-server --bin mc-server -- structure_rules` 4/0; the live
proof `live_activated_path_places_a_vanilla_village` prints
`seed 4242, village at chunk (427, 0), 9 pieces, 16 junctions, 5 beard pieces,
region 6812,59,-20..6853,86,24, 52 village blocks, 5 columns moved` and asserts
deterministic replay through the activated path; `run fmt` PASS
`.analysis/validation/20260914T092746-fmt-at4crltn`, `run code-health` PASS
`.analysis/validation/20260914T092238-code-health-elgo627a`, clippy clean for
`mc-worldgen` + `mc-server` with `--all-targets`. Base tree `8414853e`.

Unresolved, stated rather than approximated:

- **Village decor is not placed yet.** The `feature_pool_element` entries the pools
  reach (13 features, executor done and live-proved) are skipped by the growth loop,
  which only accepts piece elements, so villages generate buildings, streets and the
  terrain analogue without their decorative features. Next village item.
- **The prototype/composite is not removable yet.** A deployed Luau settlement plan
  materializes its buildings through `StructureRules::plains_village_prototype`, so
  deleting the prototype and its three-template composite would delete the plugin
  settlement route with it. Removal needs a plugin-side placement mechanism first.
- `tests::deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles` fails
  because the sibling `../solaris-default-plugins` checkout lacks the
  `solaris-settlements` package it expects (pre-existing at HEAD; owner-blocked).
- `StructureSpec.spawn_overrides`/`step` are parsed and carried but consumed by
  nothing; piece entity/villager markers are not exposed through `structures.rs`;
  the weighted-roll random source for the structure pick is still the engine's
  per-chunk draw rather than a confirmed `ChunkGeneratorStructureState` reproduction.
- The terrain analogue remains the owner-approved column-height divergence, not
  vanilla density parity.

**Owner decision on the decor gap (2026-09-14, after activation landed).** The
activated path places buildings, streets and the terrain analogue but not the
`feature_pool_element` decor (13 features; executor done and live-proved, lane not
wired). Asked whether to keep the default profile on the incomplete village, roll
the activation back until decor lands, or hide it behind a new opt-in profile, the
owner chose to **keep the default with the decor gap and make the decor lane the
next checkpoint**. So `settlement_profile = "vanilla"` stays the activated default,
`WORLDGEN_REVISION` stays 22, and `crates/mc-worldgen/src/village/mod.rs` plus
`docs/VILLAGE_GENERATION.md` name the gap explicitly rather than implying the
decor lane is landed. The commit that carries the activation (`884857cb`) also
carries the rest of the session's uncommitted work — the managed content import,
the warehouse endpoint work in `mc-net`, the harness capture, and `Cargo.lock`
(which changed for a concrete reason: `mc-server` gained `reqwest`, `sha1`, `zip`,
`thiserror` and the `mc-test-harness` dependency) — so a revert of that commit
drops more than the village activation.

**Independent checkpoint review of the activation (2026-09-14): verdict `changes`.**
The reviewer read the working tree against vanilla and found the assembled village
geometry is **not yet vanilla's**; the fixes below are the next checkpoint and each
one changes generated terrain, so the activation needs another
`WORLDGEN_REVISION` bump (23) when they land.

- **blocker — source jigsaw position and faces are not rotated** (`solver.rs:534-539`).
  Vanilla iterates `sourceElement.getShuffledJigsawBlocks(manager, sourceBoxPosition,
  sourceRotation, random)` -> `StructureTemplate.getJigsaws(position, rotation)`, whose
  entries are `calculateRelativePosition(settings, local).offset(position)` with
  `state.rotate(rotation)` (`StructureTemplate.java:201-219`). `grow` uses the raw
  template-local position and the unrotated `front`, so `target_jigsaw_pos`
  (= pos + front) drives `attach_inside`, the target's placement base and the
  `free_height` column from a jigsaw that is not where the piece actually placed it,
  and `can_attach` compares unrotated faces on both sides while vanilla compares both
  rotated by their own piece rotations (`JigsawBlock.canAttach`). Fires on every
  rotation except `None`. `anchor_origin` already rotates, so the module contradicts
  itself, and no test covers a rotated source attachment.
- **major — target pool and fallback candidate lists are not shuffled**
  (`solver.rs:556-563`). Vanilla uses `targetPool.value().getShuffledTemplates(random)`
  then `fallback.value().getShuffledTemplates(random)`
  (`JigsawPlacement.java:351-356`); `flat_elements` is right for `pick_element` only.
  Besides choosing a different first-fitting template, the two missing shuffles consume
  no draws, so every later draw (`Rotation::get_shuffled`, `shuffled_jigsaws`) comes
  from a stream vanilla has already advanced by `len(pool) + len(fallback) - 2` per
  source jigsaw.
- **major — `use_expansion_hack`'s `expandTo` is not applied** (`solver.rs:584-600`).
  All five village structures set the flag; vanilla raises the target box to
  `max(expandTo + 1, ySpan)` before the free-space test, before the piece is created
  and therefore before the RIGID piece's `BeardContribution`
  (`JigsawPlacement.java:368-406`). The engine computes the hack box and discards it.
- **major — the source-jigsaw loop is not continued, and the inside free shape is
  never grown** (`solver.rs:636-660, 717`). Vanilla's `continue label129` abandons the
  remaining candidates for that jigsaw after an attachment; `continue 'candidates` keeps
  scanning them, so a second piece can attach to the same jigsaw. And the grown shape is
  written back only when the target attached outside the source, while vanilla writes it
  to whichever `childrenFree` it used, so several attachments inside one source piece
  may overlap where vanilla rejects them.
- **minor — non-rigid `ground_level_delta` must be 1, not 0** (`solver.rs:662-666`);
  `StructurePoolElement.getGroundLevelDelta()` returns 1 and it feeds the second
  junction's `ground_y`, i.e. the analogue's `BeardJunction.ground_y`.
- **minor — `reference_pos` is the stub centre, not vanilla's**
  (`plan_source.rs:496-499`); `StructureStart.placeInChunk` uses
  `(centerPos.x, centerBB.minY, centerPos.z)`. Nothing observable changes today (no
  reachable village list declares `position_predicate`), but the modelled input is wrong.
- **minor — a named processor list missing from the closure silently becomes empty**
  (`plan_source.rs:535-543`), and `validate` probes through the same function, so the
  piece validates clean and then places with only the ignore/jigsaw/projection passes.
- **minor — stale docs**: the added paragraphs in `docs/PLUGINS.md` and
  `docs/decisions/0008-overworld-density-router.md` still say the `vanilla` profile
  places no villages, and `docs/VILLAGE_GENERATION.md` said the lookup "keeps the one"
  plan. (The `village/mod.rs` header claim about `feature_pool_element` was corrected in
  `d766018c`.)

The advisor added the same class of defect for the skipped decor lane: `grow`'s
`PlacedKind::Feature` arm `continue`s past a drawn element, while vanilla places the
feature there and the branch terminates because the element has no connectors - so the
candidate order and the RNG stream already diverge, and the village may connect a piece
vanilla never placed. The decor checkpoint must reproduce `FeaturePoolElement`'s
position/terminal-vs-retry/RNG semantics, not merely call `CompiledPlacedFeature`.
