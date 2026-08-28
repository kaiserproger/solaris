# Phase 4 oracle provenance and deliberate divergence ledger — 2026-08-21

## Scope and closeout rule

Phase 4 common-play packet layouts and gameplay rules must come from repository-local
Minecraft Java Edition 26.1.2 evidence: the local unobfuscated/decompiled jar,
`GameProtocols`/`javap` protocol inspection, checked Mojang reports and sidecars,
vanilla captures, or the side-by-side real-client/oracle harness. Internet-derived
constants are not accepted as authoritative implementation input.

This ledger consolidates the provenance already distributed across source headers,
fact sheets, captures and executable parity gates. It also records deliberate Solaris
behavior boundaries so that deterministic or bounded substitutes are not mistaken for
unqualified vanilla parity.

Independent reviewer status is separate from this provenance closeout. The existing
read-only reviewer attempts that did not return a terminal verdict remain **BLOCKED**;
this document does not convert them into a review pass.

## Common-play provenance matrix

| Domain | Strongest repository-local source(s) | Phase-4 disposition |
| --- | --- | --- |
| Movement / physics | `crates/mc-data/src/collision_shapes.rs` embeds the checked 26.1.2 block-state shape report; `crates/mc-physics`; [`mc-net-oracle-aabb-deflation-test-extraction.md`](mc-net-oracle-aabb-deflation-test-extraction.md); `mc-test-harness/tests/physics_validation.rs`; side-by-side parity gates in `parity_oracle.rs` / `entity_parity_26_1_2.rs` | Common rules traced to local report/oracle/harness evidence. |
| Block interaction | `crates/mc-data/src/block_facts.rs` derives facts from the local block report; checked break/place/use-item-on tests and [`mc-net-use-item-on-preflight-test-extraction.md`](mc-net-use-item-on-preflight-test-extraction.md); accepted vanilla TCP observations recorded by the playable evidence | Common interaction rules traced. |
| Inventory / crafting / containers | `crates/mc-data/src/recipes.rs` consumes local recipe JSON sidecars; chest/furnace/crafting-table/merchant/enchanting executable evidence under `docs/evidence/`; checked container-state vanilla/Solaris oracle gates | Common recipe/container layouts and transitions traced. |
| Combat | `crates/mc-entity/src/living_26_1_2/` carries verified 26.1.2 damage/lifecycle policy; shield/axe cooldown, reach and armor paths are pinned by local packet/item-component oracles and real-client/TCP tests | Common combat rules traced. |
| Projectiles | Local decompiled `AbstractHurtingProjectile`, `LargeFireball`, `SmallFireball`, `WitherSkull`, `DragonFireball`, `ShulkerBullet` and species attack goals; projectile owner/collision tests; [`mc-net-arrow-launch-test-extraction.md`](mc-net-arrow-launch-test-extraction.md) | Common projectile ordering, damage and lifecycle traced. |
| Fluids | `mc_data::block_facts::{FluidKind, FluidStateFacts}` from checked 26.1.2 reports; real-client fluid observations in `docs/playable/OWNER_PLAYTEST_2026-07-22.md`; [`mc-net-fluid-runtime-test-extraction.md`](mc-net-fluid-runtime-test-extraction.md) | Common fluid state/collision behavior traced. |
| Redstone essentials | Inline local 26.1.2 references in `crates/mc-net/src/play/scheduled_blocks.rs` including hopper move speed and comparator delay; scheduled button/hopper/comparator TCP tests | Current essential scheduled behavior traced. |
| Status / effects | `crates/mc-entity/src/effects_26_1_2/` and `attributes_26_1_2/`; entity-parity attributes/equipment/effects scenario; species effect facts such as Shulker Levitation pinned against local decompiled source | Common effect application/publication rules traced. |
| Death / respawn | [`mc-net-death-xp-test-extraction.md`](mc-net-death-xp-test-extraction.md), death-drop/XP conservation suites, real-client respawn/load gates and persistence evidence | Common death/respawn lifecycle traced. |
| Weather / time | [`world-clock-26.1.2.md`](world-clock-26.1.2.md) from local `ClientboundSetTimePacket`/clock inspection plus rendered 24,000-tick evidence; [`weather-26.1.2-authority-persistence-oracle.md`](weather-26.1.2-authority-persistence-oracle.md) from the local Mojang 26.1.2 oracle and side-by-side packet sequence | Covered bounded weather/time rules traced. |
| Persistence | `mc-world` Anvil/NBT codecs against local vanilla-generated worlds; [`natural-spawn-restart-persistence.md`](natural-spawn-restart-persistence.md); weather/world-clock persistence gates; entity restart-identity evidence | Current persisted common-play state traced. |
| Common entity AI | [`phase4-entity-ai-matrix-2026-08-21.md`](phase4-entity-ai-matrix-2026-08-21.md); local decompiled species goals/brains/projectiles; `crates/mc-data/src/entity_contract_26_1_2.rs` from checked registry report; villager gossip oracle sheets | All formerly `UnsupportedSpecial` primary-combat verticals and the named common entity behaviors have local oracle/evidence provenance. |

## Protocol-layout provenance boundary

`crates/mc-protocol/src/packets/play.rs` is governed by the local 26.1.2
`GameProtocols`/unobfuscated-jar workflow and `.analysis/protocol-dump.txt` as recorded
by ADR 0002 and the module documentation. A minority of individual `const ID` sites do
not repeat the source citation immediately adjacent to the constant; that is a source
annotation/style gap, not evidence that those ids were derived externally. New packet
layouts must continue to use the same local workflow rather than copying online tables.

## Active deliberate Solaris divergences

These are intentional bounded substitutes or deterministic policies that remain active
on the Phase-4 tree. They are recorded here so their provenance and non-vanilla boundary
are explicit.

### Ender Dragon D1 air-combat orbit

- Owner: `crates/mc-entity/src/dragon_26_1_2.rs` and
  `crates/mc-net/src/play/session/dragon_authority.rs`.
- Vanilla source: local decompiled
  `net/minecraft/world/entity/boss/enderdragon/EnderDragon.java`,
  `phases/DragonHoldingPatternPhase.java`,
  `phases/DragonStrafePlayerPhase.java`,
  `phases/DragonChargePlayerPhase.java`, `DragonFlightHistory.java`, and
  `entity/projectile/hurtingprojectile/DragonFireball.java`.
- Deliberate boundary: D1 uses a deterministic eight-point air orbit
  (`D1_ORBIT_POINTS = 8`, radius `20`, height offset `5`) instead of the End fight's
  vanilla 24-node A* ring. Landing/perch transitions are deliberately suppressed so
  the bounded flying state machine is closed under its implemented transitions.
- What remains vanilla-derived inside D1: part sizes/damage divisor, strafe
  range/charge/cone, charging recovery, steering constants, DragonFireball breath
  cloud semantics and the fight-less 500-XP/200-tick death schedule.
- Evidence: Ender Dragon row in
  [`phase4-entity-ai-matrix-2026-08-21.md`](phase4-entity-ai-matrix-2026-08-21.md).

### Villager gossip RNG boundary

- Owner/evidence: [`villager-gossip-transfer-26.1.2.md`](villager-gossip-transfer-26.1.2.md)
  and [`villager-positive-gossip-26.1.2.md`](villager-positive-gossip-26.1.2.md).
- Vanilla source: fingerprinted local decompiled 26.1.2 villager gossip implementation.
- Deliberate boundary: Solaris ports the Java legacy bounded-random behavior required by
  the transfer rule but does not claim to reproduce the complete per-entity vanilla
  `RandomSource` stream across unrelated systems.

### Attribute deterministic ordering

- Owner: `crates/mc-entity/src/attributes_26_1_2/`.
- Vanilla source: local 26.1.2 attribute implementation and the documented fastutil
  ordering behavior used by the clean-room kernel.
- Deliberate boundary: Solaris publishes/persists in deterministic `AttributeId` order
  instead of depending on JVM identity-hash iteration order. This is an intentional
  reproducibility/stability fix and is documented by the module's semantic-order
  boundary.

### Deterministic loot/drop policy still in scoped paths

- Historical review: `docs/M67_VANILLA_DIVERGENCES.md`.
- Owners: current block/crop/mob drop helpers and the repo-owned/fallback loot tables in
  `mc-data`/`mc-net`.
- Deliberate boundary: remaining scoped crop/cocoa/sweet-berry/mob/block fallback paths
  use deterministic local yields where full vanilla loot-table/RNG/tool-predicate
  execution is not yet claimed. This is a known bounded behavior choice, not an
  oracle-derived claim of exact vanilla random distribution.

### Deterministic plant/tree lifecycle in scoped paths

- Historical review: `docs/M67_VANILLA_DIVERGENCES.md`.
- Owners: current plant support/growth helpers and generated-tree logic.
- Deliberate boundary: supported saplings use Solaris-owned deterministic tree shapes;
  several crop/vertical-plant/stem/cocoa/sweet-berry growth and yield paths intentionally
  use deterministic helpers rather than the full vanilla RNG/event/feature-placement
  machine. Exact unsupported richness is not claimed by the Phase-4 common-play matrix.

## Superseded M67 rows

`docs/M67_VANILLA_DIVERGENCES.md` is a historical review and must not be read as the
current Phase-4 blocker list. In particular:

- the old **Beds: sleeping/time-skip absent** row is superseded by later sleep/time
  authority and real-client/TCP evidence;
- the old **Shields: axe disable absent** statement is superseded by the implemented
  five-second axe-disable/cooldown path; other shield presentation/durability residuals
  remain separately bounded;
- the old broad **Bows/arrows** scope predates the later projectile owner, collision,
  damage, pickup/despawn and TCP work; tipped/spectral/enchantment/presentation residuals
  remain, but the old row is not the current common-projectile disposition.

Other active M67 deterministic-growth/drop/tree observations are consolidated above
rather than silently discarded.

## Captured vanilla behavior intentionally preserved

Not every unusual rule is a divergence. The weather side-by-side oracle observed a
vanilla duplicate-snapshot ordering and Solaris reproduces that captured sequence
rather than normalizing it away. It remains documented in
[`weather-26.1.2-authority-persistence-oracle.md`](weather-26.1.2-authority-persistence-oracle.md)
as capture-driven fidelity.

## Ender Dragon D1 provenance check

The D1 implementation was checked against repository-local decompiled source under
`.analysis/decompiled/server-26.1.2/net/minecraft/world/entity/boss/enderdragon/` and
`.../entity/projectile/hurtingprojectile/DragonFireball.java`. The production TCP gate
proves Dragon movement → `dragon_fireball` → `area_effect_cloud` → player damage →
fireball removal. Focused owner tests prove the reserved vanilla multipart id span,
part-aware player damage, lethal `Dying` transition and fight-less XP/removal schedule.
The deterministic eight-point orbit is the deliberate boundary above; it is not
presented as the vanilla 24-node path graph.

## Closeout disposition

For Phase 4 item 5, every sampled common-play packet/rule family has an allowed local
provenance path and the active deliberate divergences are now consolidated in this
ledger. Rare/boss residuals listed by the common-play/entity matrices remain outside
the parity claim rather than becoming undocumented source assumptions.
