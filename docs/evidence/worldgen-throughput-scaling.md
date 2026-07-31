# Revision-10 worldgen throughput and scaling

Date: 2026-07-31

Checkpoint base: `e235028` (`perf(play): tighten item transaction locks`)

## Scope

This checkpoint turns the public-alpha generation comparison into a reproducible, topology-explicit release test. The ignored `mc-server` test `tellus_seed_712816_spawn_window_reports_worker_scaling` uses the production startup path and:

1. builds the production `tellus_like` generator with seed `712816` and default settings;
2. locates the same deterministic natural spawn used by startup;
3. generates the exact 225-chunk view-distance-6 plus light-border square;
4. inserts every generated chunk through `WorldStorage::insert_generated_chunk`;
5. runs three samples for one worker, runtime-auto workers, process-visible workers, and an optional explicit worker count;
6. reports minimum, median, and maximum chunks per second.

The benchmark excludes light baking so it measures the historical `empty world pre-generation finished` boundary rather than combining two independently reported startup stages. It requires a release build. `SOLARIS_WORLDGEN_WORKERS` adds an explicit topology point; `SOLARIS_WORLDGEN_MIN_CHUNKS_PER_SECOND` is accepted only together with that explicit worker count, preventing a threshold from silently changing CPU topology.

The repository owner explicitly waived the old same-host requirement on 2026-07-31. Closeout therefore records the exact host, affinity, worker topology, build, and tree instead of presenting different machines as directly controlled hardware comparisons.

## Measured bottleneck

The first local release stage profiles for 25 chunks were `98-102 ms`. The stable dominant stages were:

| Stage | Initial range |
| --- | ---: |
| Column planning | `10-11 ms` |
| Fill | `10-11 ms` |
| Caves | `32-33 ms` |
| Ores | `42-44 ms` |
| Biomes, structures, decorations | `3-4 ms` |

`perf` attributed the original hot path primarily to repeated 3D cave value noise, repeated 2D landform/biome planning for neighbouring ore halos, and ore exposure reconstruction. The optimization work rejected two neutral experiments: a scalar `floor` fast path and a generated-cell classification cache. Neither remains in the tree.

## Accepted optimization

The production generator now:

- keeps a bounded 64-shard cache of immutable terrain/biome/vegetation diagnostics, capped at 2,048 entries per shard and reset whenever geometry or worldgen mode changes;
- reuses the cave raw-sample cache between cave carving and vanilla ore exposure checks inside one chunk;
- evaluates one cave Y layer through a batched two-octave 3D value-noise grid, reusing lattice corners across adjacent X/Z samples;
- preserves a scalar fallback at the extreme i32 block-coordinate boundary;
- keeps hot 2D value-noise and fBm functions inline after measured improvement.

The batched 3D sampler has a bit-for-bit regression against scalar `fbm_3d` over negative and positive coordinates, three cave scales, and multiple seeds. Existing deterministic chunk bytes, cross-border ore veins, discard-on-air-exposure, extreme-coordinate generation, cave shell, drainage, mosaic, and order-independence tests remain unchanged and green.

## Final stage profile

Repeated release profiles after the accepted changes completed in `65-70 ms` for 25 chunks. Representative final stages:

| Stage | Final range | Change from initial |
| --- | ---: | ---: |
| Column planning | `7 ms` | about `-30%` |
| Fill | `10 ms` | unchanged |
| Caves | `10-11 ms` | about `-67%` |
| Ores | `33-35 ms` | about `-21%` |
| Total | `65-70 ms` | about `-32%` |

The next measured optimization target is the ore path, now roughly half of serial generation time. A stretch target of 5,000 chunks/s on six workers was discussed but is not claimed or substituted for the accepted public-alpha threshold.

## Host and toolchain

- CPU: AMD Ryzen 5 7535HS, 6 physical cores / 12 logical CPUs, boost enabled.
- Physical-core affinity: CPUs `0,2,4,6,8,10`.
- Rust: `rustc 1.94.1 (e408947b 2026-03-25)`, LLVM `21.1.8`.
- Build: Cargo release test profile.

## Scaling results

Unrestricted process-visible topology (`12` logical CPUs):

| Requested workers | Median chunks/s | Sample range |
| ---: | ---: | ---: |
| 1 | `525.311` | `448.748-527.865` |
| runtime auto = 6 | `2,669.869` | `2,549.781-2,675.684` |
| available = 12 | `2,958.300` | `2,908.174-2,988.828` |

Six physical CPUs only (`taskset -c 0,2,4,6,8,10`):

| Requested workers | Median chunks/s | Sample range |
| ---: | ---: | ---: |
| 1 | `492.639` | `425.779-494.652` |
| runtime auto = 3 | `1,408.406` | `1,364.904-1,414.186` |
| explicit 6 | `2,461.979` | `2,456.894-2,486.200` |

The explicit six-worker result passes the historical public-alpha floor of `743.578 chunks/s`, which is 80% of the recorded `929.473 chunks/s` baseline. The current value is not described as a controlled same-host speedup; it is a current-tree closeout on fully recorded hardware.

Exact six-physical-core command:

```sh
env \
  SOLARIS_WORLDGEN_WORKERS=6 \
  SOLARIS_WORLDGEN_MIN_CHUNKS_PER_SECOND=743.578 \
  taskset -c 0,2,4,6,8,10 \
  cargo test --release -p mc-server \
    tellus_seed_712816_spawn_window_reports_worker_scaling \
    -- --ignored --nocapture
```

Default scaling-matrix command:

```sh
cargo test --release -p mc-server \
  tellus_seed_712816_spawn_window_reports_worker_scaling \
  -- --ignored --nocapture
```

## Evidence boundary

This closes the revision-10 generation-throughput row and proves scaling across one, automatic, and available worker counts on the recorded host. It does not replace the seed-`712816` owner traversal/disposition, restart/rejoin, graphical natural-spawn soak, light-bake benchmark, or broader release profile matrix.

Final package tests, workspace Clippy, formatter, code-health, diff check, and independent review are recorded at checkpoint close.
