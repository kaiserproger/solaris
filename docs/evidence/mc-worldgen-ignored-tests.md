# `mc-worldgen` ignored-test classification

Scope: Phase 1 test inventory for `crates/mc-worldgen`.

This inventory covers the crate's two explicit debug-build measurement probes
over a seed-42, 5×5 chunk window using the synthetic test registry. Neither is
a hidden behavioral regression or a manual gameplay gate. The crate now has
three additional ignored local-artifact structure gates, classified separately
in
[`mc-worldgen-structure-local-artifact-tests.md`](mc-worldgen-structure-local-artifact-tests.md).

## Inventory

| Ignored test | Measured boundary and correctness fence | Owner and exact close condition |
| --- | --- | --- |
| `terrain::tests::generated_spawn_window_debug_stage_profile` | Reconstructs the production generation stages for 25 chunks and reports elapsed time and percentage for column planning, fill, caves, ores, biomes, structures, and decorations. It has no timing threshold or behavioral assertion; successful execution only proves that the measured diagnostic workload completed. | Phase 3 worldgen profiling. Re-run only after a material generation-pipeline feature boundary or while investigating a measured worldgen bottleneck. Record the exact tree, debug profile, host, workload, total time, and every stage measurement. It closes the requested diagnostic run only; it cannot close correctness or the release throughput gate. |
| `terrain::tests::generated_spawn_window_debug_budget_reports_throughput` | Calls the complete generator for the same 25 chunks, requires every result to have `minecraft:full` status, reports chunks per second, and enforces a debug-only wall-time ceiling of 10 seconds. | Phase 3 M31/worldgen performance evidence. Re-run after a material worldgen performance change and on the release candidate with the exact tree and host recorded; the probe must complete all 25 chunks below 10 seconds. Release closeout additionally requires the public plan's same-host 225-chunk comparison against the recorded `929.473 chunks/s` baseline, with no more than 20% regression. |

## Executable correctness fences

The ordinary `mc-worldgen` suite covers the behavior exercised by the probes:

- repeated generation is deterministic and different seeds change chunks;
- columns retain bedrock, surface shells, heightmaps, and explicit geometry
  across short, tall, and extreme valid dimensions;
- caves, ores, biome assignment, structures, and decorations are individually
  reachable and constrained;
- biome/feature fingerprints remain distinct and bounded across multiple seeds;
- generated overlays and configured structures survive flush and reopen.

Those tests remain the behavioral authority. The manual stage reconstruction is
useful only for attribution, and the 25-chunk budget probe is a local debug
guard rather than evidence for graphical quality, production startup, or the
release-host throughput comparison.

## Current disposition

The original bounded inventory command compiled `mc-worldgen` and listed these
two probes. The current ignored inventory contains both probes plus the three
separately classified structure sidecar gates. Neither performance probe was
executed during this classification checkpoint.

The public plan's `929.473 chunks/s` result is historical 225-chunk evidence,
not a result from either ignored probe on this tree. The revision-10 visual,
seed-`712816` owner-play, restart, and release-host throughput gates remain
separate and open.

`benchmark: not applicable`: this checkpoint changes only the inventory and
ownership record; it does not change a measured worldgen path.

## Reproduction

List the current ignored inventory:

```sh
cargo test -p mc-worldgen -- --list --ignored
```

Run only the mapped probe:

```sh
cargo test -p mc-worldgen \
  terrain::tests::generated_spawn_window_debug_stage_profile \
  -- --exact --include-ignored --nocapture

cargo test -p mc-worldgen \
  terrain::tests::generated_spawn_window_debug_budget_reports_throughput \
  -- --exact --include-ignored --nocapture
```
