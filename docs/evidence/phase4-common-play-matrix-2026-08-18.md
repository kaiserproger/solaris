# Phase 4 common-play evidence matrix — 2026-08-18

Status: Phase-4 item-2 inventory/evidence closeout. This matrix maps every common-play domain named in `PUBLIC_ALPHA_PLAN.md` to executable evidence and an explicit remaining boundary. A row being present here is **not** a claim that every vanilla mechanic in that category is complete.

Target: unmodified Minecraft Java Edition 26.1.2 behavior used in an ordinary multiplayer survival session.

## Matrix

| Domain | Strongest current executable evidence | Current disposition | Explicit remaining common-play gap |
| --- | --- | --- | --- |
| Movement | `physics_validation`: flat-ground movement without correction, wall/full-block rejection, shallow/deep-water movement, landing fall damage and water fall suppression; `player_presence::two_clients_spawn_move_and_despawn_visible_players`; validated M94 real-client join/move/rejoin artifact | **covered common path / broader parity open** | Broader sprint/sneak/jump edge combinations, exact swim feel across more terrain, uncommon movement states, and broad real-client movement matrix |
| Block interaction | `block_edit`: accepted break/update/ack/relight, survival timing, stale peer replacement rejection, survival placement/pickup, placement rejection/resync, signs, doors/stairs/plants/falling blocks; two-client break publication | **covered common path / broader parity open** | Long-tail block families, sounds/statistics/game events, uncommon placement shapes, and broader real-client visual parity |
| Inventory / crafting / containers | `survival_inventory`, `inventory_crafting`, `crafting_table`, `chests_and_hoppers`, `furnaces`; checked container-state vanilla/Solaris oracle; two-client chest/furnace stale-click convergence | **covered common path / broader parity open** | Recipe-book UX/sync, broader station families, remaining click-mode edges, and broad real-client concurrent-container evidence |
| Combat | `block_edit/pvp::melee_pvp_damages_only_the_observed_target_player_over_wire`; shield/axe TCP cases; hostile combat; armor/durability/player-damage tests; entity battle load gate | **covered common path / broader parity open** | Remaining exact attack timing/knockback/sound/particle rules, uncommon damage sources, and broader real-client two-player combat |
| Projectiles | survival bow release/spawn/motion, skeleton-arrow player damage over wire, grounded-arrow owner/pickup/despawn tests, projectile armor/shield paths | **covered common path / broader parity open** | Crossbows, tipped/spectral variants, broader arrow embedding/metadata/attribution, and wider oracle/client coverage |
| Fluids | water bucket scheduled spread, water/lava solidification, scheduled-fluid save/restart without duplicate tick, bucket progression, shallow/deep-water physics | **covered common path / broader parity open** | Broader source/flow update order, uncommon fluid/container interactions, current/swim feel breadth, and broader real-client fluid evolution |
| Redstone essentials | scheduled buttons driving two-half doors; hopper source/feed/output/campfire/double-chest paths; `server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp` | **partial but executable** | Broader lever/pressure-plate/wire propagation, comparator placement/persistence, update-order breadth; pistons/observers/quasi-connectivity remain below the current common-alpha contract |
| Status / effects | entity parity scenario `attributes-equipment-effects`; active-effect regional/entity tests; equipment/armor/shield status publication | **partial but executable** | Broader player-applied potion/effect lifecycle, effect-source gameplay, visual/icon/particle parity, and wider side-by-side oracle coverage |
| Death / respawn | real-client DeathScreen→respawn gate; `dead_survival_player_can_respawn_and_act_again`; `respawned_survival_player_rejoins_alive_after_saved_restart`; keep-inventory/death-drop/XP tests | **covered common path / broader parity open** | Contested player-death reward persistence, broader XP-orb lifecycle, and multiplayer restart after simultaneous death/reward races |
| Weather / time | typed world-clock map plus rendered 24,000-tick/restart evidence; multiplayer sleep/time tests; `/weather` raw-TCP gate; full Mojang 26.1.2 rain→thunder→clear `GameEvent` oracle; weather save/rebind | **covered bounded weather/time / scheduler open** | Autonomous random weather scheduler and `/weather ... <duration>` override lifecycle are deliberately not claimed; subjective bed-animation/wake parity remains open |
| Persistence | player/chunk/block-entity/container/entity/item/scheduled-fluid/campfire restart gates; natural-spawn deterministic identity checkpoint/disk evidence; weather kind + exact intermediate level persistence | **covered common restart slices / broad crash parity open** | Broad crash-window/fsync proof, open-container/cursor recovery, real-client weather/campfire/scheduled-tick restart breadth, and full fresh-world release restart gate |

## Evidence anchors

The matrix intentionally points to existing executable suites instead of creating duplicate scenario ownership. Main anchors include:

- `crates/mc-test-harness/tests/physics_validation.rs`
- `crates/mc-test-harness/tests/block_edit.rs` and its focused `block_edit/*` modules
- `crates/mc-test-harness/tests/player_presence.rs`
- `crates/mc-test-harness/tests/mob_presence.rs`
- `crates/mc-test-harness/tests/parity_oracle.rs`
- `crates/mc-test-harness/tests/entity_parity_26_1_2.rs`
- `crates/mc-test-harness/tests/persistence_inventory.rs`
- `crates/mc-test-harness/tests/load_scenarios.rs`
- [`world-clock-26.1.2.md`](world-clock-26.1.2.md)
- [`natural-spawn-restart-persistence.md`](natural-spawn-restart-persistence.md)
- [`weather-26.1.2-authority-persistence-oracle.md`](weather-26.1.2-authority-persistence-oracle.md)

## Placement conclusion

No named Phase-4 item-2 domain is now an unclassified hole. The matrix identifies two particularly visible bounded gaps rather than hiding them behind a generic parity label:

1. autonomous natural weather scheduling/duration override;
2. broader status/effect gameplay beyond the existing entity/equipment effect path.

Phase-4 item 3 has since closed against the dedicated entity/AI matrix in [`phase4-entity-ai-matrix-2026-08-21.md`](phase4-entity-ai-matrix-2026-08-21.md), including explicit production authority for every formerly `UnsupportedSpecial` primary-combat vertical. The broader-parity boundaries above remain scoped residuals rather than reopening that common entity/AI closeout.
