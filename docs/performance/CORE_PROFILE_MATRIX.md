# Solaris core performance profiles

Status: frozen test contract for the M91/T07 performance campaign. These limits are selected before profile runs and must not be relaxed after seeing a result. A failed profile remains `degraded` or `blocked` until code or the declared hardware envelope changes in a separately reviewed commit.

## Provenance

- Contract base: `0cc648826a14f25f1aeae759f379d463337bd3bd` plus the T07-01 autoscale change in the same campaign checkpoint.
- Rust/Cargo: `1.94.1`.
- Calibration host: AMD Ryzen 5 7535HS, 6 cores / 12 threads, 32,076,632 KiB host RAM, Linux.
- Runtime build for the recorded calibration: debug, unoptimized + debuginfo.
- Vanilla sidecar: owner-local `data/vanilla`, mounted read-only into the campaign run.
- Calibration artifact: `.analysis/runs/t07-01-a/m52-debug-baseline.log`.
- Calibration result: 4 clients, 8 summons, 5,754 ms elapsed; chunk CPU peak 2, IO peak 1, result queue peak 1; `session_registry max_wait_us=121`, `max_hold_us=5,189`; `world_storage max_wait_us=11`, `max_hold_us=50`.

The calibration is a bounded balanced-profile checkpoint, not proof of the 20-client VD8 or VD32 profiles.

## Runtime profile defaults

These values are the authoritative defaults in `mc_net::AutoscalePolicy::for_profile`.

| Profile | VD range | Chunk send | Chunk load | Chunk generate | Tick target | First chunk target | Queue pressure | Memory pressure | Down/up hysteresis |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `low_end` | 4..8 | 4..8 | 8..16 | 4..12 | 50 ms | 2,500 ms | 70% | 85% | 2 / 8 observations |
| `balanced` | 6..10 | 8..16 | 16..64 | 8..32 | 50 ms | 1,500 ms | 75% | 85% | 3 / 10 observations |
| `high_end` | **8..32** | 16..32 | 32..96 | 16..64 | 50 ms | 1,000 ms | 80% | 90% | 4 / 12 observations |

Tick, queue, memory, first-chunk SLA, or slow-client pressure first yields deferred/random-tick work and then reduces bounded chunk throughput and view distance. Healthy observations restore one view-distance step at a time. `high_end` therefore supports a real `32 -> 8` degradation path and an `8 -> 32` recovery path without dropping below VD8.

## Frozen measurement envelopes

CPU and memory entries are harness/cgroup requirements, not TOML keys. Client/entity limits are workload inputs, not claims about production capacity.

| Profile | CPU quota | Memory limit | Clients | Initial VD / simulation distance | Active entity ceiling | Duration | Current evidence status |
|---|---:|---:|---:|---:|---:|---:|---|
| low | 2 vCPU | 4 GiB | 4 | 8 / 4 | 128 | 10 min | `degraded`: frozen, not run on the declared quota |
| balanced | 6 physical / 12 logical CPUs | 8 GiB | 20 | 8 / 8 | 256 | 20 min | `degraded`: focused 20-client VD8 debug/release gates passed on 2026-07-27, but the exact 20-minute and 8 GiB envelope was not run |
| high | 12 vCPU | 16 GiB | 20 | **32 / 8** | 512 | 20 min | `degraded`: VD32 runtime contract tested; end-to-end VD32 load not yet run |

The entity ceiling counts non-item server entities at the measurement checkpoint. Item drops, XP orbs, projectiles, and primed TNT are reported separately so a transient burst cannot hide inside the mob count.

## Valid profile configuration

The production example intentionally starts at VD32 with conservative initial chunk rates and lets runtime control raise or lower throughput independently.

```toml
[server]
max_players = 20
view_distance = 32
simulation_distance = 8

[chunk_pipeline]
chunk_send_rate = 8
chunk_load_rate = 16
chunk_generate_rate = 16
chunk_prepare_budget_ms = 0
chunk_prepare_batch_size = 8
chunk_result_queue_size = 64
region_cache_size = 9

[simulation]
random_tick_speed = 5
save_interval_ticks = 1200
friendly_spawn_interval_ticks = 400
hostile_spawn_interval_ticks = 20

[autoscale]
enabled = true
profile = "high_end"
min_view_distance = 8
max_view_distance = 32
target_tick_ms = 50
target_first_chunk_ms = 1000
scale_down_after_ticks = 4
scale_up_after_ticks = 12
```

`simulation_distance` stays at 8 for the VD32 profile. Raising simulation distance to 32 is a different workload and must not be silently included in a view-distance result.

## Required metrics

Every profile artifact must include:

- commit, dirty flag, build mode, Rust version, hardware, CPU quota, memory limit, seed, duration, clients, configured/effective VD, simulation distance, entity counts;
- tick and stage `samples/p50/p95/p99/max` for total tick, world time, entity goals, entity physics, entity dispatch, random tick, block tick, fluid tick, entity save, campfire, hostile attacks, and breeding;
- first/ring/full chunk latency p50/p95/p99/max and per-stage fetch, encode, light, compression/frame, and socket-write maxima;
- `active_cpu`, `active_io`, permit maxima, result queue depth/capacity, stop reasons and cancellation counts;
- lock wait/hold counts, totals and maxima for `world_storage`, `session_registry`, `chunk_prepare`, `player_persistence`, and `save_all_flush`;
- RSS/effective memory limit and percentage, dirty chunks/flush timings, prepared/region cache bytes and evictions;
- reliable drops/retries/in-flight maximum, best-effort drops, write timeouts, and slow-client pressure sheds.

## Pass, degraded, and blocked rules

A profile is `pass` only when all mandatory metrics exist and:

- sustained TPS is greater than 18 and tick p95/p99 are at or below 50,000 us;
- no unexplained tick exceeds 150,000 us; the stricter O2 VD8 gate keeps tick max at or below 50,000 us;
- first-chunk p99 is within the profile target; full-window latency is recorded and monotonically ordered after first/ring milestones;
- normal-reader workloads have zero reliable command drops, zero write timeouts, and zero slow-client sheds;
- result queue depth remains below the profile pressure threshold and CPU/IO permits are never exceeded;
- memory remains below the profile pressure threshold and no allocation/queue success relies on OOM recovery;
- `session_registry` and `world_storage` max hold remain below 250,000 us; any 150,000 us or larger tick/lock event is individually explained;
- shutdown drains CPU/IO work, flushes dirty chunks, and leaves no duplicate-pressure or stuck retry state.

A run is `degraded` when it completes but misses a budget, lacks one non-authoritative metric, uses debug build where release evidence is required, or runs below the declared clients/VD/entity/duration envelope. It is `blocked` when required sidecars are absent, the process fails, a required metric/artifact is missing, reliable state is lost, or shutdown does not drain.

## Fixed execution order

1. Validate config and sidecar paths.
2. Record idle and solo baselines.
3. Run the exact profile envelope without changing budgets.
4. On failure, preserve the artifact and optimize the measured dominant stage only.
5. Re-run the same profile and compare against the frozen contract.
6. Never lower clients, VD, entities, duration, or percentile budgets in the same change that claims an optimization.
