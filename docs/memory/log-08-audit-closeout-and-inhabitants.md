# Archived checkpoint log — part 8 of 8

Chronological checkpoint history moved out of `docs/MEMORY.md` so the live cursor
stays small. **Not startup context** (see `AGENTS.md`); read it when a question is
about this era, not to learn the current state.

## Sections in this part

- Core audit closeout + village inhabitants from piece markers (landed 2026-09-15)

---

## Core audit closeout + village inhabitants from piece markers (landed 2026-09-15)

Owner ask: "проверь что в ядре не осталось незакрытых пунктов включая
solaris-settlements. если что-то есть - доделай." The audit first: the two gates
that were owner-blocked on the unpublished sibling package are **green from this
checkout** now that `../solaris-default-plugins` carries `solaris-settlements` —
`cargo test -p mc-test-harness --test settlement_lifecycle` 7/0,
`--test settlement_pause_repro` 1/0, and
`cargo test -p mc-server --bin mc-server -- deployed_sibling_plugins_prepare_runtime_and_worldgen_profiles`
1/0. `crates/` has no `todo!`/`unimplemented!`/`TODO`/`FIXME` at all, the
`/settlement populate` item is obsolete (the shipped package has no such
subcommand), and the core-side hardening the queue asked for — refusing a release
of a *consumed* spawn site — is already implemented
(`crates/mc-net/src/script/storage/settlement.rs:1642-1660`). One red gate was
found and closed: `settlement_tests::shipped_settlement_package_is_accepted`
loaded its block report from `data/vanilla/reports/blocks.json`, which this
machine does not have (that directory holds only `README.md`; the sidecar lives
in the managed cache now), so the test panicked before validating anything. It
now resolves the report the way the server resolves its content cache —
`SOLARIS_CONTENT_CACHE`, then the workspace sidecar, then the standard managed
cache — and passes against the real 6 MB report; a machine with no cache at all
is told so by name rather than failing on a missing path. The one open core item
the live cursor named as the next outcome was the village population — a feature
slice, not a closeout item, and the page's other "Not implemented" bullets are
deliberate fail-closed fences rather than outstanding work — and that is what
landed after the gates were established.

**checkpoint_closed:** a generated vanilla village now spawns its villagers.

- **Template entities are read.** `StructureTemplate` gained `entities`:
  `crates/mc-worldgen/src/structures.rs` reads the NBT `entities` list the way
  `StructureTemplate.load` does — `pos` (doubles) and `blockPos` (ints) default to
  zero when absent, an entry without `nbt` is dropped, `id` is required, and
  `VillagerData{type,profession,level}` plus `Age` and `Rotation[0]` are read for
  a villager. `level` outside 1..=5 fails the load by name instead of clamping.
- **Placement is vanilla's, pinned against the real classes.** The village lane's
  `piece.rs` now runs `StructureTemplate.placeEntities`: `transform(Vec3, ...)`
  (mirror `1.0 - c`, `+ 1` in the rotated branches) plus the piece's world
  position for the entity's double `pos`, the block transform for the
  bounding-box test, `entity.rotate(rotation) + entity.mirror(mirror) -
  entity.getYRot()` for the yaw, and the authored `Rotation[1]` verbatim for the
  pitch (vanilla's `snapTo` passes the entity's own xRot; `entity_pitch` in
  `mc-net` applies `Entity.setXRot`'s `clamp(% 360, -90, 90)`). Both transform
  functions are pinned to numbers a scratch probe over the bundled 26.1.2 jar
  printed (3 points × 3 mirrors × 4 rotations;
  `village_piece_tests::template_entity_positions_transform_like_vanilla`), the
  yaw's raw-vs-wrapped asymmetry (`Entity.load` sets the authored yaw unwrapped;
  `rotate`/`mirror` wrap) is pinned too, and the yaw/pitch reading was corrected
  by the review below: the baby templates author `Rotation = [0.0, -25.827711]`,
  so that value is a *pitch*, and the first version of this checkpoint treated it
  as a yaw and dropped the pitch entirely.
- **The entity leaves as a chunk inhabitant marker**, not as a runtime spawn:
  `ChunkPieceWriter` collects the villagers of the pieces it places in *this*
  chunk (the same per-chunk clip the blocks go through, so the entity whose block
  belongs to the neighbouring chunk is placed when that chunk is generated) and
  `write_plans` writes them once per chunk via `set_settlement_inhabitants` —
  the handoff the settlement lane already used, so `entities.dat`, the claim-
  derived UUID and the chunk extras round-trip are unchanged.
- **Marker fields** gained `age` and `yaw` (`mc_world::SettlementInhabitantMarker`,
  optional in NBT so a marker written without them reads as an adult yaw 0).
- **The runtime spawns the villagers it implies.** `mc-entity` carries the village
  types (`Desert`/`Plains`/`Savanna`/`Snow`/`Taiga`, wire ids 0/2/3/4/6 from the
  registry report) and `Nitwit` (11), plus
  `VillagerPopulationState::village_baby(age, claimed_home)`: the authored `Age`
  (not `VILLAGER_BABY_START_AGE_TICKS`) and the villager's own placement claim as
  the home the storage contract requires of a baby — caught by the independent
  read-only review before it shipped: the first version passed `None`, which
  `crates/mc-net/src/play/persistence.rs:1981` rejects, so the first generated
  village baby would have made `entities.dat` unwritable. `mc-net`'s
  `settlement_inhabitant_spawn` (new, and now the one place the marker's names
  become enums) resolves a marker; `settlement_candidate` spawns it with the
  authored yaw on the entity rotation and the baby schedule when `age < 0`, and
  the mc-net test now drives the spawned baby through a real
  `save_persisted_entity_records` / `load_persisted_entities` round-trip — the
  pre-fix receipt (scratch revert, `.analysis/codex-logs/closeout/baby-prefix-receipt.log`)
  fails on the missing home claim, so the test is not vacuous.
- **The gap that remains is reported, not hidden.** The lane spawns villagers
  only: the meeting-point iron golem, the animal pens' livestock and cats, a
  desert camel, a butcher shop's animals, an armour stand and the zombie
  villagers of the weight-1 zombie town centres are *not* spawned (each needs its
  own entity state). `VillageClosure::unspawned_piece_mobs()` reports exactly the
  ids the loaded closure reaches, and startup logs them as
  `village_piece_mobs_other_than_villagers_are_not_spawned`. The templates author
  **no POIs**, and vanilla's POI acquisition (claiming a bed/workstation/bell) is
  not modelled, so a marker carries the core's existing answer for that case
  (`default_villager_pois`, the same shape the settlement lane fills from its
  plan and the runtime applies to a brain-less villager): the entity's own placed
  position is its home, its meeting point, and a working profession's job site —
  so a village keeps the shape of a day, on placement-derived POIs rather than
  the furniture next to it.
- **`WORLDGEN_REVISION` 23 → 24** (`crates/mc-worldgen/src/lib.rs`): revision-23
  chunks carry no inhabitant markers, so a reused chunk would generate a village
  with no population. `docs/VILLAGE_GENERATION.md` gained a "Village inhabitants"
  section, its world-identity section and its "Not implemented" list now say what
  is and is not placed, and the "core places no villages" / notice text is
  unchanged (the analogue notice is still the one a village world carries, now
  beside the unspawned-mobs one).

**Independent review (read-only `VillageInhabitantsReview`, 18m11s, verdict
`changes`).** Three findings, all resolved here:

- minor — the entity's authored pitch (`Rotation[1]`) was parsed nowhere and the
  spawn hardcoded `pitch: 0.0`; the baby templates author
  `Rotation = [0.0, -25.827711]`, so every village baby would have spawned 25.8°
  off, and the first version of the *test* read that value as a yaw. Fixed:
  `TemplateEntity.pitch`, `PlacedEntity.pitch`, the marker's `Pitch`,
  `entity_pitch`'s `Entity.setXRot` clamp in `mc-net`, and tests that use the
  real authored values — including a `place_piece` test across all four rotations
  whose expectation is the pinned `Vec3` reference row, and a real-cache test
  that asserts the three villager templates' own `Rotation`/`Age`/`VillageData`.
- minor — the end-to-end marker test's position expectation was rotation-invariant
  (seed 7 draws `Rotation::None`), so the rotated path of the new `Vec3` transform
  was not exercised through the plan source. Fixed: the fixture's entity now sits
  at the `[2.25, 3.0, -1.5]` pinned point, whose four rotated images differ, and
  the plan-source test asserts the rotation-specific image.
- note — the repaired `shipped_settlement_package_is_accepted` report lookup had a
  different order than the server's `ContentSearch` and its docstring claimed to
  be the server's discovery. Fixed: the order is now env → managed cache →
  workspace sidecar, matching `ContentSearch::candidates`, and the docstring says
  what the helper does and does not re-check.

**Evidence.** `SOLARIS_CONTENT_CACHE=/home/user/.local/share/solaris/content/26.1.2
cargo test -p mc-worldgen --lib` 261 passed / 0 failed / 5 ignored — the live
layer really ran, and its new proof prints `13 pieces carry template entities; 8
villagers placed` (`village_plan_source_tests::live_village_populates_its_pieces_villagers`,
which also asserts that `minecraft:cat`, `minecraft:iron_golem` and
`minecraft:zombie_villager` are the *reported* unspawned mobs of the real
closure); `cargo test -p mc-world --lib` 288/0/15;
`cargo test -p mc-net --lib` 2190 passed / 0 failed / 8 ignored (the one red it
started with, `settlement_tests::shipped_settlement_package_is_accepted`, is the
block-report path above); `cargo test -p mc-entity --lib` 625/0/6; the settlement
harness gates 7/0 and 1/0 and the deployed-sibling test 1/0; and the full L2 gate
`python3 -m tools.harness run correctness` **PASS on the final tree**
`.analysis/validation/20260915T023733-correctness-xx5jvgrn` (fmt, code-health,
strict workspace clippy, `cargo test --workspace --all-targets`: 71 test binaries,
0 failed suites, 312 s) on the revision whose markers also carry the
placement-derived POIs; two earlier revisions of the same tree passed as
`20260915T022806-correctness-v6jjgxrs` (271 s) and
`20260915T015216-correctness-3x66szv1`, and `cargo fmt` was re-run green
(`20260915T023329-fmt-de9ay4_7`) after the last doc edit.

**That gate is load-sensitive on this machine, and the runs that failed are
recorded rather than hidden.** Four runs (and one direct `cargo test --workspace
--all-targets`) failed on *timing-deadline integration tests*, a different set
each time, every one of them green when the same binary was rerun alone on the
same tree: `mc-server/tests/play.rs`'s 2 s Lua-over-the-wire deadlines
(`lua_script_payload_round_trip_reaches_player`,
`lua_plugin_loaded_from_disk_replies_to_join_and_chat_over_the_wire`,
`lua_script_oversized_payload_is_rejected_before_the_wire`,
`lua_player_command_context_distinguishes_operator_and_exposes_identity_and_position`,
`lua_disk_plugin_spawns_allowlisted_entity_over_the_wire` — 19/19 standalone in
0.9 s, twice, and once as two concurrent processes),
`mc-test-harness/tests/player_entity_interacted_lua.rs`'s 957 ms frame wait
(1/1 standalone, twice), `mc-test-harness/tests/commands.rs`'s inventory snapshot
(1/1 standalone), `mc-script`'s
`batch_rejection_handler_disable_is_reported_exactly_once` (266/266 standalone,
twice) and `mc-test-harness/tests/settlement_pause_repro.rs` (1/1 standalone:
the run's server answered `Unknown command`, i.e. the plugin was not ready when
the deadline-driven command fired). None of them touches a path this checkpoint
changed, and the machine reported load ~4-6 throughout. The transform and yaw
tables come from the bundled jar
(`.analysis/server.jar` → `META-INF/versions/26.1.2/server-26.1.2.jar` + its
`META-INF/libraries/*`, run with Vineflower installed at
`~/.local/share/vineflower/vineflower.jar`).

**Still open in core, deliberately not in this checkpoint** (audit result, each
with its owner): the mobs above; villager POI acquisition/breeding/trading; the
`plains_village_prototype` composite and profile, whose removal waits on a
plugin-side placement mechanism for a deployed settlement plan; `spawn_overrides`
parsed and consumed by nothing; start heights other than a constant absolute,
vanilla's `dimension_padding` and terrain adaptations other than `beard_thin`
(all fail closed by name); the POI leash for villagers/golems (wander reach is
still 6..32 with no home leash); the deferred double-chest container; the
base-row blueprint rule for the eight plugin blueprints that author cells at
their local `y=0`; and the canonical `m94-09-settlement-chain` real-client run,
which needs the harness and a client (owner/manual or agent-run through the
approved client MCP) — no client was launched here.

**Checkpoint state (no commit authorization).** base_tree `78b4f39d`;
diff_hash `1ea005139d2f50db9ec10d94fb241ab691b9fce86551cbbf76971d54b6023806` (SHA-256 over `git diff -- crates/`, i.e. the validated source revision — 19 files, +1750/-63; the doc edits that follow the L2 receipt do not move it. Whole uncommitted change: 21 files, +2063/-88); changed_files:
`crates/mc-entity/src/lib.rs`, `villager_population_26_1_2.rs`,
`crates/mc-net/src/play.rs`, `play/chunk_stream.rs`, `play/session.rs`,
`play/session/settlement_authority.rs`, `play/wire_entities.rs`,
`crates/mc-net/src/settlement_tests.rs`, `crates/mc-server/src/main.rs`,
`crates/mc-world/src/chunk.rs`, `crates/mc-worldgen/src/lib.rs`,
`structures.rs`, `terrain.rs`, `village/{closure,mod,piece,plan_source}.rs`,
`village_piece_tests.rs`, `village_plan_source_tests.rs`,
`docs/VILLAGE_GENERATION.md`, `docs/MEMORY.md`. Nothing staged, committed or
pushed; no sibling repository touched.

**Manual/client gate: not run.** Revision 24's evidence is Rust gates plus the
Java probe, exactly as revision 23's was; a village walk is what would show the
population.

**Next.** The game-visible half of this change: run the real-client village walk
(fresh `world_dir`, default `settlement_profile`, a generated village in view)
and confirm the villagers stand in their houses, or take up villager POI
acquisition if the walk shows wandering villagers as the bigger gap.
