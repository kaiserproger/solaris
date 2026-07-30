# `mc-net` ignored and sidecar-test classification

Scope: Phase 1 test inventory for `crates/mc-net`.

The crate now has exactly five explicit ignored tests. Three are mapped
performance reports. Two are local 26.1.2 sidecar parity gates that previously
returned success without assertions when `data/vanilla` was absent; they now
require an explicit opt-in run and fail if their prerequisite is missing.

## Inventory

| Ignored test | Boundary and prerequisite | Behavioral coverage | Owner and exact close condition |
| --- | --- | --- | --- |
| `play::persistence::tests::regional_decision_journal_fsync_latency_report` | Forty local-filesystem append/fsync/clear iterations; reports record, clear, and total percentiles without a source-level threshold | Ordinary tests cover round-trip, grouped append, compaction, exact durable identity, crash-prefix recovery, corruption rejection, and unknown append outcomes | Phase 3 performance/release evidence. Re-run on the exact candidate and declared storage environment; record command, tree, environment, record/total p99 and max in the benchmark matrix |
| `play::session::combat_load_tests::mob_combat_load_benchmark_report` | O3 workload: 4,096 entities, 1,000 idle ticks, four lethal attacks per tick, and bounded death cleanup | Non-ignored deadline tests cover killed-only indexing, lethal effect damage, duplicate scheduling, stale owner revision, bounded removal, and final entity removal | Phase 3 performance/release evidence. Re-run after a material combat/death-index change or on the release candidate; both lethal and cleanup p99 must remain below `50,000 µs` |
| `play::simulation::explosion_load_tests::explosion_authority_load_benchmark_report` | O3 workload: 4,096 background entities and 64 queued explosions with bounded explosions per tick | Non-ignored simulation/session tests cover exact TNT expiry, simultaneous explosion drops/publication, delayed spawn, entity geometry, damage, and velocity publication | Phase 3 performance blocker. Compare base and candidate on the same workload and environment; close only when explosion burst p99 is below `50,000 µs`, then record the current-candidate artifact |
| `play::tests::real_door_states_plan_hand_toggle_when_sidecar_is_present` | Local `data/vanilla/reports/blocks.json`; exact real oak-door states must produce the two-half hand-toggle plan | Always-executable synthetic-registry tests cover property preservation, hand-openable materials, two-half planning, power interaction, and release scheduling | Phase 4 parity gate. Run explicitly with the exact local 26.1.2 blocks report after a material door/registry change and on the release candidate; record tree, sidecar version, command, and result |
| `play::session::tests::sheep_recipe_mix_matches_all_local_vanilla_two_dye_recipes` | Local `data/vanilla/data/minecraft/recipe`; the complete two-dye recipe projection must equal Solaris' mix table | `sheep_recipe_mix_matches_checked_in_26_1_2_table` now runs in the ordinary suite and fixes the exact nine unordered mixes without requiring Mojang bytes | Phase 4 parity gate. Run explicitly with the exact local 26.1.2 recipe sidecar after a material sheep/recipe change and on the release candidate; record tree, sidecar version, command, and result |

## Current disposition

The two sidecar parity gates passed in this checkpoint against the available
local 26.1.2 blocks and recipe data. This is focused parity evidence, not a
graphical or real-client gate.

The mapped 2026-07-27 performance evidence remains historical:

- mob combat/death cleanup passed at lethal p99 `16.301 ms` and cleanup p99
  `27.681 ms`;
- regional journal fsync passed at record p99 `5.592 ms` and total p99/max
  `5.602 ms`;
- explosion authority failed twice and most recently measured p99/max
  `55.941 ms` against the frozen `50 ms` budget.

Those numbers come from
[`../performance/2026-07-27-benchmark-matrix.md`](../performance/2026-07-27-benchmark-matrix.md).
No ignored performance report was reproduced during this classification
checkpoint, and the explosion result remains an open Phase 3 blocker.

Workspace validation also exposed two ordinary regional-owner boundary
regressions. A finite villager/item geometric miss now returns `false` without
mutation instead of panicking the server, while a simulation-projection read
omits an entity removed after interest selection instead of rejecting the whole
read. The generated-village fixture now places its carrot drop inside the
authoritative pickup bounds rather than relying on item drift. Villager-shared
food now carries its exact persisted recipient, preventing the donor from
recollecting and rethrowing the same stack. Pending parents keep reciprocal
follow goals until birth or no-bed resolution instead of wandering out of the
courtship range. The generated-village drop, share, heart, birth, wire, save,
and restart gate passed with the same child UUID after restart.

`benchmark: not applicable`: no mapped performance contract covers these test
visibility and stale-boundary corrections. No benchmark was reproduced.

## Reproduction

The bounded inventory command is:

```sh
cargo test -p mc-net -- --list --ignored
```

Run the two sidecar gates individually; do not use an unfiltered `--ignored`
command because that would also execute all three heavy performance reports:

```sh
cargo test -p mc-net sheep_recipe_mix_matches -- --include-ignored
cargo test -p mc-net real_door_states_plan_hand_toggle_when_sidecar_is_present -- --include-ignored
```

The focused regressions are:

```sh
cargo test -p mc-entity regional::tests::villager_inventory_pickup_returns_false_for_stale_or_out_of_reach_without_mutation -- --exact
cargo test -p mc-entity regional::tests::villager_inventory_pickup_accepts_targeted_shared_food_within_search_radius -- --exact
cargo test -p mc-entity regional::tests::owner_coordinator_simulation_projection_skips_removed_requested_ids -- --exact
cargo test -p mc-test-harness --test worldgen generated_village_food_share_birth_wire_and_restart_are_stable -- --exact
```
