# `mc-entity` ignored-test classification

Scope: Phase 1 test inventory for `crates/mc-entity`.

The current crate has exactly six ignored tests. Every ignore attribute names an
explicit debug or release benchmark, every test emits timing data, and none is a
manual gameplay or behavioral regression gate.

## Inventory

| Ignored test | Measured boundary | Correctness fence inside the workload | Classification |
| --- | --- | --- | --- |
| `tests::active_subset_ecs_density_benchmark_report` | Debug ECS goal/pathing work for 32 active entities among 10,000 over 200 ticks | None beyond completing the measured workload | Opt-in sparse active-subset timing report |
| `tests::dense_active_set_ecs_benchmark_report` | Debug comparison of full and indexed goal/pathing passes for 1,000 active entities over 200 ticks | Both paths must finish with equal snapshots so unlike work is not compared | Opt-in dense ECS timing comparison |
| `regional::tests::persistent_owner_lane_scaling_benchmark_report` | Debug serial/parallel regional-owner timing for 2,048 entities, then a 512-entity active subset | Serial and parallel runs must finish with equal snapshots; Linux runs require distinct physical cores | Opt-in owner-lane scaling report |
| `regional::tests::regional_coordinator_actor_overhead_benchmark_report` | Debug raw-ECS, direct-coordinator, and actor-runtime timing for 512 entities | All three routes must finish with equal snapshots | Opt-in coordinator/actor overhead report |
| `regional::tests::cached_animal_mutation_benchmark_report` | Debug cached-lane versus forced-actor animal CAS timing, including two concurrent regional callers | Cached and actor variants must finish with equal snapshots | Opt-in selected-route mutation report |
| `regional::tests::regional_owner_1500v1500_battle_benchmark_report` | Release 1,500-versus-1,500 regional battle workload with bounded active actors | The workload must exercise off-cohort targets, attacks, deaths, and retargeting without missing live follow targets | Opt-in release workload; writes its JSON report under `.analysis/bench/` unless `SOLARIS_BATTLE_REPORT` overrides it |

The equality and progress assertions above are benchmark validity fences: they
prevent timing unlike or inert work. The underlying behavior remains in the
ordinary executable suite. Representative non-ignored coverage includes:

- `dense_ecs_pathing_requests_are_stably_ordered` and the focused prepared-goal
  tests for all-ID and selected-ID ECS goal/pathing work;
- `owner_coordinator_moves_physical_stores_to_lanes_and_round_trips_them`,
  `production_kinematics_parallelism_admits_dense_regions`, and
  `owner_runtime_routes_goal_and_conditional_physics_commands` for direct,
  parallel, and actor-owned movement;
- `conditional_animal_batch_rejects_stale_snapshot_without_partial_mutation`,
  `owner_coordinator_rejects_cross_lane_animal_cas_when_one_parent_is_stale`,
  `cached_single_animal_cas_bypasses_the_coordinator_actor`, and
  `cached_same_lane_animal_batch_bypasses_the_coordinator_actor` for animal
  mutation authority and cached routing;
- `owner_goal_apply_handles_mutual_cross_lane_follow_targets_without_readmission_deadlock`,
  `owner_damage_batch_applies_independent_cross_lane_hits`, and
  `owner_coordinator_damage_uses_snapshot_cas_and_reports_lethal_result` for the
  goal, damage, and death behavior exercised by the battle workload.

Therefore no mandatory behavioral regression is available only through
`--ignored`; the large-scale comparison itself is performance evidence.

## Owner and close conditions

The five debug reports belong to Phase 3 performance work and the Entity/ECS
section of
[`../performance/2026-07-27-benchmark-matrix.md`](../performance/2026-07-27-benchmark-matrix.md).
Reproduce only the mapped report at a completed feature boundary that materially
touches its measured path, using the candidate tree and a recorded environment,
workload, result, and artifact.

The 1,500-versus-1,500 release workload belongs to release-candidate performance
closeout unless an earlier measured feature boundary explicitly maps it. Its
close condition is a current-candidate run with the report artifact and the
applicable threshold recorded in the benchmark matrix.

The 2026-07-27 matrix is prior measured evidence, not proof that these reports
were reproduced on this checkpoint's tree. This classification checkpoint did
not run any ignored benchmark.

## Reproduction of the inventory

The bounded inventory command is:

```sh
cargo test -p mc-entity -- --list --ignored
```

It lists the six names above and no other ignored unit, integration, or doctest
in `mc-entity`.

## Current-tree revalidation

The wrapper later replayed this checkpoint from its original
`5328f9dcc550ffb9f5499b40c4806ade4ea1300a` resume base even though commit
`8add5e32f0eff83ec6080983b37e89b57ec836f7` had already closed it. The
2026-07-30 revalidation used current tree
`10d542976dd688b2b98c548601e80996af2d089e` and confirmed:

- `cargo test -p mc-entity -- --list --ignored` still lists exactly the six
  benchmark reports in the inventory and no integration or doctest ignores;
- the always-executable
  `dense_ecs_pathing_requests_are_stably_ordered`,
  `cached_single_animal_cas_bypasses_the_coordinator_actor`, and
  `owner_coordinator_damage_uses_snapshot_cas_and_reports_lethal_result`
  regressions each pass;
- no ignored benchmark was executed or claimed as reproduced.
- one independent read-only review passed with no findings after checking the
  source annotations, captured inventory, focused regressions, policy, links,
  scope, and next cursor.

This is a documentation/test-policy checkpoint. Affected-package, workspace,
Clippy, and graphical gates are not mapped to it and were not rerun. The next
Phase 1 cursor is one bounded `mc-net` wall-clock-wait test class under item 2;
inventory and replace/classify the first real polling or sleep dependency
without combining unrelated gameplay work.
