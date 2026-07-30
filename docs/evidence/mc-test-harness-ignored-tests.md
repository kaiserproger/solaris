# `mc-test-harness` ignored-test classification

Scope: Phase 1 test inventory for `crates/mc-test-harness`.

The crate has exactly 27 ignored integration tests across five test targets.
They split into local vanilla-oracle parity gates, sidecar-backed behavioral
compositions, load/reliability gates, and explicit performance reports. Every
ignore now has a reason, and an explicitly requested gate fails when its named
prerequisite is unavailable instead of returning a false-green result.

## Inventory summary

| Target | Count | Classification |
| --- | ---: | --- |
| `block_edit` | 1 | Local 26.1.2 sidecar-backed stale-break composition |
| `chunk_stream` | 1 | Local 26.1.2 generated-world chunk/light composition |
| `entity_parity_26_1_2` | 1 | Strict local vanilla/Solaris entity comparison |
| `parity_oracle` | 10 | Strict local vanilla/Solaris protocol and behavior comparisons |
| `load_scenarios` | 14 | Six behavioral compositions, six load/reliability gates, and two explicit performance reports |

## Behavioral sidecar compositions

| Ignored test | Unique boundary | Executable fence and exact close condition |
| --- | --- | --- |
| `stale_survival_break_cannot_break_peer_replacement` | Two real TCP clients race a survival break against a peer replacement and require the stale completion to resync without replacing the peer's block | The ordinary `commands` stale-break wire gate and `mc-net` owner stale-root tests cover the authority rule. This opt-in composition belongs to Phase 1; run after a material break-authority change and on the release candidate with 26.1.2 blocks/registries sidecars, and require the replacement block plus acknowledgements to remain exact. |
| `vanilla_client_receives_spawn_view_distance_window` | A vanilla-shaped TCP login receives the complete unique 289-chunk view-distance-8 spawn window with exact chunk/light invariants and bounded lock pressure | Ordinary chunk-pipeline and reconnect tests cover preparation, visibility, cancellation, and publication. This Phase 1 integration gate closes on an explicit 26.1.2 blocks/light-sidecar run after a generated-world, light, or spawn-stream change and on the release candidate. |
| `checked_multiplayer_transaction_replay_is_deterministic_and_conservative` | The checked multiplayer manifest must normalize identically across two runs and conserve every concurrent transaction group | Phase 1 deterministic replay gate. Run after a material multiplayer transaction change and on the release candidate; both normalized runs must match and every checked group must satisfy its manifest state. |
| `duplicate_lethal_player_commands_drop_one_bundle_and_survive_restart` | Duplicate lethal commands must create exactly one inventory/XP death bundle and preserve it across save/restart | Ordinary player-death owner, wire, and persistence tests cover idempotency and conservation. Run this Phase 1 composition after a material death/save change and on the release candidate; require one exact bundle before and after restart. |
| `bounded_multiplayer_survival_replay_covers_sequential_contention_and_slow_reader` | Bounded multi-client replay combines sequential actions, live contention, reconnect/cancellation, and a paused reader | Phase 1 deterministic integration gate. Run after a material session/transaction/outbound change and on the release candidate; require the contention, reconnect/cancellation, slow-reader, and in-test budget fences to pass. |
| `concurrent_same_target_placements_consume_exactly_one_stack` | Two clients contend for one placement target over eight rounds and exactly one inventory debit may commit per round | Ordinary placement mutation-token and inventory-CAS tests cover the rule. Run this Phase 1 composition after a material placement-authority change and on the release candidate; require one world winner, one debit, and no stale publication in every round. |
| `concurrent_shared_chest_same_state_commits_one_cursor_transaction` | Two clients submit the same shared-chest state and only one cursor transaction may commit | Ordinary chest/container stale-state tests cover conservation. Run this Phase 1 composition after a material container-authority change and on the release candidate; require one commit, an authoritative loser resync, and conserved chest/player items. |
| `vd8_multi_client_stop_drains_and_flushes_disk_world_under_stream_load` | Concurrent view-distance-8 streaming must drain before shutdown and reopen all observed disk-backed chunks | Ordinary chunk-stream, shutdown, and persistence tests cover the individual paths. Run this Phase 1 process composition after a material stream/shutdown/save change and on the release candidate; require successful drain, quiescent save, and exact disk reopen. |

## Vanilla-oracle parity gates

All eleven tests in this section belong to Phase 4 parity. They are opt-in
because `.analysis/server.jar` and Java 25 or newer are local prerequisites.
The configuration-phase and full-registry-fallback comparisons plus the two
checked block/container manifest gates also require the local 26.1.2 sidecars.
Their common close condition is an explicit run on the exact candidate against
the recorded local 26.1.2 oracle, with no degraded observation and the compared
normalized facts equal. A failed comparison remains a parity finding rather
than a test infrastructure success.

- `local_vanilla_and_solaris_entity_scenarios_compare_side_by_side`
- `vanilla_and_solaris_configuration_phase_can_be_diffed`
- `vanilla_and_solaris_full_registry_fallback_match`
- `vanilla_and_solaris_spawn_smoke_can_be_diffed`
- `vanilla_and_solaris_seeded_core_actions_can_be_diffed`
- `checked_manifest_vanilla_and_solaris_protocol_observations_can_be_diffed`
- `vanilla_and_solaris_container_held_slot_can_be_diffed`
- `vanilla_and_solaris_entity_lifecycle_can_be_diffed`
- `vanilla_and_solaris_timed_action_can_be_diffed`
- `checked_block_transaction_manifest_matches_vanilla_oracle_and_solaris`
- `checked_container_state_manifest_matches_vanilla_oracle_and_solaris`

The ordinary suite still exercises each Solaris scenario, deterministic
manifest replay, observation normalization, oracle availability, and the block
and container oracle adapters without launching Mojang's server. Those tests
are correctness fences for the harness; they do not substitute for the
side-by-side parity result.

Eight `parity_oracle` gates previously printed the missing-oracle message and
returned success. They now match the two checked-manifest gates and the entity
parity gate by failing the explicit opt-in run when the oracle is unavailable.

## Load, soak, and performance gates

| Ignored test | Classification | Owner and exact close condition |
| --- | --- | --- |
| `prompt02_multiplayer_transaction_soak_short_preflight` | 400-tick transaction/slow-reader preflight | Phase 3 reliability preflight. Run before the long soak after a material transaction/outbound change; require the exact tick target, transaction samples, entity broadcasts, zero reliable drops, drained workers, successful save, and published tick provenance. |
| `prompt02_four_active_one_slow_reader_transaction_soak_36000_ticks` | 36,000-tick fallback soak | Release reliability closeout. Run on the exact candidate after the short preflight; require the same fences through all 36,000 ticks and record host, tree, elapsed time, tick percentiles, pressure, and save results. |
| `vd8_twenty_same_spawn_clients_drain_full_window_and_stop_without_duplicate_pressure` | Concurrent 20-client view-distance-8 streaming/load gate | Phase 3 chunk-stream performance. Run at the mapped streaming feature boundary and on the release candidate; require all 20 unique 289-chunk windows, shared preparation within the test's frozen budgets, drained workers, zero dirty tail, and recorded debug/release results. |
| `multicore_login_chunk_stream_and_broadcast_stays_within_budgets` | Multicore login, chunk-stream, broadcast, and tick-budget gate | Phase 3 runtime performance. Run after a material runtime/streaming change and on the release candidate; every in-test latency and tick budget must pass with environment and percentiles recorded. |
| `paused_reader_does_not_stall_active_entity_broadcasts` | Slow-reader isolation under entity publication pressure | Phase 3 outbound reliability. Run after a material outbound/backpressure change and on the release candidate; the active observer must keep receiving bounded broadcasts with no reliable-command loss. |
| `paused_reader_pressure_does_not_delay_healthy_observers` | One paused reader plus two healthy observers under deterministic pressure | Phase 3 outbound reliability. Run after a material delivery/backpressure change and on the release candidate; both healthy observers must satisfy the in-test latency fences while pressure remains observable and reliable drops remain unchanged. |
| `reports_spawn_exploration_block_entity_and_multi_client_load` | M37 opt-in aggregate load report | Phase 3 performance evidence. Reproduce only at the mapped feature boundary or release candidate and record the exact tree, host, workload, emitted measurements, and artifact; report generation alone is not a release pass. |
| `entity_scale_40k_hostiles_60_clients_profile` | Explicit 40,000-hostile/60-client release profiling workload | Phase 3 entity/runtime profiling and release closeout. Run only on the declared release-capable environment with its configured workload; require all correctness fences and persist the provenance, phase timings, tick/memory results, and artifact before making a capacity claim. |

The load target checks for the blocks and registries sidecars before starting a
server and fails with an explicit degraded-coverage message if they are absent.
Optional light/biome inputs remain visible through the workload configuration;
release evidence must use the complete declared 26.1.2 sidecar set.

## Current disposition

The bounded inventory command compiled every `mc-test-harness` target and
listed exactly 27 ignored tests. No ignored parity, soak, or benchmark workload
was executed during this classification checkpoint. Historical load results in
[`../performance/2026-07-27-benchmark-matrix.md`](../performance/2026-07-27-benchmark-matrix.md)
remain prior-tree evidence, not proof for this tree.

`benchmark: not applicable`: this checkpoint changes prerequisite visibility
and ignored-test classification only; it does not change a measured gameplay,
streaming, outbound, or entity-runtime path.

## Reproduction

List the complete ignored inventory without executing it:

```sh
cargo test -p mc-test-harness -- --list --ignored
```

Run only the mapped gate. Do not use an unfiltered `--ignored` invocation,
which would combine strict parity comparisons, long soaks, and release-scale
profiling:

```sh
cargo test -p mc-test-harness --test parity_oracle \
  vanilla_and_solaris_spawn_smoke_can_be_diffed \
  -- --exact --include-ignored

cargo test -p mc-test-harness --test load_scenarios \
  checked_multiplayer_transaction_replay_is_deterministic_and_conservative \
  -- --exact --include-ignored
```
