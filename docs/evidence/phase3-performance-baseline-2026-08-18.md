# Phase 3 measured performance baseline — 2026-08-18

## Provenance

This checkpoint measures the current dirty public-alpha worktree before and after the accepted Phase-3 optimization slice. Results are not presented as a clean-commit comparison.

- Git HEAD: `7d378936a8c81905102fad52bae9d37869cdd930`.
- Worktree: dirty; Phase-2 and Phase-3 changes are present together.
- Host: AMD Ryzen 5 7535HS, 6 physical cores / 12 logical CPUs, x86_64 Linux.
- Memory: 32,846,467,072 bytes host RAM; 8,589,930,496 bytes swap.
- Rust: `rustc 1.94.1 (e408947bf 2026-03-25)`.
- LLVM: `21.1.8`.
- Worldgen affinity run: physical CPUs `0,2,4,6,8,10`.
- Vanilla sidecar reported by the load harness: Java `26.1.2`.

The old 2026-07-27 matrix is retained as historical evidence. All claims below come from fresh runs on this exact host/current worktree unless explicitly called historical.

## Fresh public-alpha baselines

### 200-action break/drop/pickup lock gate

Command:

```sh
cargo test -p mc-test-harness --test block_edit \
  two_hundred_torch_break_drop_pickups_stay_below_lock_and_tick_budgets \
  -- --ignored --exact --nocapture
```

Result: **PASS**.

| Metric | Fresh result |
| --- | ---: |
| Tick p50 | `3.937 ms` |
| Tick p95 | `4.845 ms` |
| Tick p99 | `5.305 ms` |
| Tick max | `9.267 ms` |
| Session max wait | `17 us` |
| Session max hold | `574 us` |
| Player-persistence max wait | `2 us` |
| Player-persistence max hold | `418 us` |

All 200 actions completed and the exact 1,200-tick telemetry window stayed below the existing 50 ms tick gate. This reproduces the M39 item-path closeout with materially more lock-headroom than the earlier final run.

### Revision-10 worldgen throughput

Command:

```sh
env \
  SOLARIS_WORLDGEN_WORKERS=6 \
  SOLARIS_WORLDGEN_MIN_CHUNKS_PER_SECOND=743.578 \
  taskset -c 0,2,4,6,8,10 \
  cargo test --release -p mc-server \
    tellus_seed_712816_spawn_window_reports_worker_scaling \
    -- --ignored --nocapture
```

Result: **PASS**.

| Workers | Median chunks/s | Range |
| ---: | ---: | ---: |
| 1 | `518.958` | `443.125–531.121` |
| runtime auto = 3 | `1,501.156` | `1,496.878–1,533.098` |
| explicit 6 | `2,689.834` | `2,673.322–2,737.248` |

The explicit-six-worker result is about 9.3% above the previous same-host recorded median `2,461.979 chunks/s` and remains far above the frozen public-alpha floor `743.578 chunks/s`.

### 20-client same-spawn VD8 release workload

The first fresh invocation failed before Play because the load harness opened all 20 localhost connections from one source address while production pre-auth admission intentionally permits only four concurrent pre-auth connections per IP. Production admission was not relaxed. The harness now binds the 20 concurrent clients to distinct `127.0.0.x` source addresses, preserving real concurrent login pressure while respecting the production anti-abuse contract.

Command:

```sh
cargo test --release -p mc-test-harness --test load_scenarios \
  vd8_twenty_same_spawn_clients_drain_full_window_and_stop_without_duplicate_pressure \
  -- --ignored --exact --nocapture
```

Result after harness correction: **PASS**.

| Metric | Fresh release result |
| --- | ---: |
| Clients | `20` |
| Chunks/client | `289/289` |
| First-chunk p99 | `32 ms` |
| Ring-1 p99 | `104 ms` |
| Ring-2 p99 | `221 ms` |
| Full-window p99 | `1,805 ms` |
| Tick p50 / p95 / p99 / max | `0.265 / 0.516 / 1.136 / 1.826 ms` |
| RSS | `406 MiB` |
| Result-queue max / capacity | `2 / 64` |
| CPU workers | `2 / 2` max |
| IO workers | `1 / 1` max |
| Reliable drops | `0` |
| Reliable retries | `0` |
| Write timeouts | `0` |
| Slow-client sheds | `0` |

Lock maxima:

| Lock | Max wait | Max hold |
| --- | ---: | ---: |
| `chunk_prepare` | `615 us` | `661 us` |
| `session_registry` | `638 us` | `353 us` |
| `world_storage` | `526 us` | `1,508 us` |
| `save_all_flush` | `209 us` | `790 us` |
| `player_persistence` | `0 us` | `2 us` |

Shutdown drained CPU/IO work and left zero dirty chunks. The fresh `1.805 s` full-window p99 is substantially below the historical 2026-07-27 release result `3.409 s` on the same host class.

### Multicore login/chunk/broadcast release workload

Command:

```sh
cargo test --release -p mc-test-harness --test load_scenarios \
  multicore_login_chunk_stream_and_broadcast_stays_within_budgets \
  -- --ignored --exact --nocapture
```

Result: **PASS**.

- 4 clients, 8 summons, observer saw 11 spawn packets.
- elapsed: `5.083 s`.
- session-registry max hold: `494 us`.
- world-storage max hold: `6 us`.
- global chunk CPU/IO permit ceilings were respected.

## Autoscale startup-hysteresis investigation

The prior public-alpha observation recorded 18 CPU-admission changes during the first ten seconds of Play. A snapshot containing only the latest decision cannot distinguish stable hysteresis from oscillation, so this checkpoint adds monotonic `scale_down_decisions` / `scale_up_decisions` telemetry to `RuntimeControlSnapshot`; it does not alter policy or thresholds.

Focused controller coverage remains green (`cargo test -p mc-net autoscale --quiet`: 20/20).

A live release workload with runtime view distance allowed to change:

```sh
cargo test --release -p mc-test-harness --test load_scenarios \
  vd8_multi_client_stop_drains_and_flushes_disk_world_under_stream_load \
  -- --ignored --exact --nocapture
```

reported **before drain**:

- `scale_down_decisions = 2`;
- `scale_up_decisions = 0`;
- effective limits reached VD6 / send8 / load16 / generate8;
- the next queue-pressure observation was `Hold` with `pressure_ticks=1`, i.e. it was waiting for the next hysteresis threshold rather than scaling again immediately;
- no application-stop reason was present.

The workload then passed stop/drain/flush with zero dirty chunks. This closes the startup-hysteresis investigation: the current controller degrades under sustained source pressure without the old rapid down/up oscillation. Long recovery/memory-pressure soaks remain separate profile-envelope work.

## Ranked profile and explosion P0

The fresh O3 explosion benchmark initially reproduced a severe regression:

```sh
cargo test --release -p mc-net \
  explosion_authority_load_benchmark_report -- --ignored --nocapture
```

Initial current-tree result:

- 4,096 background entities;
- 64 queued TNT explosions;
- 1 explosion per tick;
- burst p50 `155.964 ms`;
- p95 `225.346 ms`;
- p99/max `234.639 ms`;
- frozen p99 threshold `< 50 ms`: **FAIL**.

Kernel sampling with `perf` was unavailable because this host has `perf_event_paranoid=4`; no sysctl was changed. The ignored benchmark was therefore extended with release-mode stage measurements.

The stage profile disproved several plausible causes:

- 13 nearby entity targets;
- entity-target snapshot p99 roughly `2–3 ms`;
- explosion ray/candidate planner against real `WorldStorage`: roughly `2–3 ms` p99;
- entity exposure against real collision data: below `1 ms` p99;
- about 453–471 explosion candidates but exactly 27 actual block edits;
- block mutation preparation below `0.1 ms`;
- conditional block batch application below `0.1 ms`.

### Accepted optimization 1 — batch explosion item-drop ownership

Explosion block drops used `spawn_item_drop_owned` once for each destroyed block. The block benchmark destroys 27 blocks, so one explosion performed 27 separate entity-owner/publication transactions.

The explosion path now uses one `spawn_item_drop_batch_owned` transaction, reusing the same owner batch insertion/publication model already used by other item-drop paths. The single-drop API delegates to the batch API.

Measured effect after this change:

- burst p50 fell from about `156 ms` to about `31 ms`;
- p99 fell from about `235 ms` to about `62 ms`.

Correctness fences include the existing item-drop batch tests and the full explosion semantics suite.

### Accepted optimization 2 — configured registries instead of embedded JSON rebuilds

`tick_primed_tnt` rebuilt the embedded fallback `ItemRegistry` and `EntityTypeRegistry` inside the hot tick. Those helper functions parse embedded JSON and construct registry objects by value.

The simulation owner now receives the authoritative configured item/entity registries from `ServerConfig` and resolves explosion drop/TNT entity IDs from those registries. This both removes repeated JSON/registry construction and makes the runtime path use the same configured data authority as the rest of the server.

Measured effect after this change moved explosion p99 from roughly `62 ms` to roughly `58 ms`.

### Accepted optimization 3 — one passenger-index scan per region/batch

The remaining impact profile showed accepted entity-damage batches taking up to roughly `25 ms`, proportional to the number of damaged targets. The regional lane validator performed this check for **every** `DamageIfCurrent` mutation:

```text
scan every snapshot in the region to determine whether expected.id is referenced as a passenger
```

With ~12 accepted targets and ~4,096 background entities, this repeated the same immutable pre-state scan tens of thousands of times per explosion.

`prepare_region_owner_batch` now lazily builds one `HashSet<EntityId>` of passenger ids per touched region/batch and performs O(1) membership checks for each `DamageIfCurrent`. The cache is scoped only to batch validation and therefore observes the same immutable pre-commit state as the original repeated scans.

A dedicated regression now proves that batch damage to an entity currently referenced as a passenger remains unapplied and leaves its health unchanged. Existing vehicle tests remain green.

After this change the measured damage-batch phase fell to roughly `1–3 ms` on accepted-hit ticks.

Neutral experiments were removed: batching explosion knockback and a special single-lane thread-spawn bypass did not materially improve the frozen benchmark and are not part of the accepted optimization set.

## Final explosion result

Final clean release run, without profiling environment variables:

| Metric | Result |
| --- | ---: |
| Burst p50 | `28.321 ms` |
| Burst p95 | `39.432 ms` |
| Burst p99/max | `41.300 ms` |
| Frozen p99 budget | `< 50 ms` |
| Status | **PASS** |

This is roughly an 82% reduction from the fresh current-tree `234.639 ms` p99 baseline and closes the previously failing O3 explosion-authority threshold without changing the budget.

## Fresh neighboring O3 results

After the explosion optimization, the neighboring ranked workloads were rerun:

### Mob combat / death cleanup

```sh
cargo test --release -p mc-net mob_combat_load_benchmark_report -- --ignored --nocapture
```

- 4,096 entities;
- lethal p99 `16.300 ms`;
- cleanup p99 `30.523 ms`;
- **PASS**.

### Regional decision journal fsync

```sh
cargo test --release -p mc-net regional_decision_journal_fsync_latency_report \
  -- --ignored --nocapture
```

- 40 iterations;
- record p99 `5.985 ms`;
- total p99/max `6.003 ms`;
- **PASS**.

The old matrix's explicit explosion failure is therefore no longer the highest-priority measured O3 blocker.

## Fresh ranked ordinary-play hotspot closeout

The closeout ranking uses frozen profile targets where they exist. For per-tick/component authority workloads without a narrower pre-existing budget, the conservative threshold is the frozen `50 ms` tick p99 target from `CORE_PROFILE_MATRIX.md`; this is now asserted directly by the regional battle/scaling and durability ignored benchmarks where applicable.

| Rank | Hotspot / workload | Metric and explicit threshold | Fresh result | Status |
| ---: | --- | --- | ---: | --- |
| 1 | Explosion authority, 4,096 background entities / 64 TNT | burst p99 `< 50 ms` | `41.300 ms` | PASS / watch |
| 2 | Mob death cleanup, 4,096 entities | cleanup p99 `< 50 ms` | `30.523 ms` | PASS / watch |
| 3 | Integrated 1,500×1,500 regional battle | tick p99 `<= 50 ms` | `25.706 ms` | PASS / watch |
| 4 | Regional owner, 2,048 full-set / four lanes | mutation p99 `<= 50 ms` | `12.195 ms` | PASS |
| 5 | Lighting, emissive full recompute | invocation upper estimate `< 50 ms` | `11.073 ms` | PASS |
| 6 | Regional decision journal fsync | total p99 `<= 50 ms` | `6.003 ms` | PASS |
| 7 | 200-action break/drop/pickup | tick p99 `<= 50 ms`; authoritative lock max hold `< 250 ms` | `5.305 ms`; `0.574/0.418 ms` session/player-persistence | PASS |
| 8 | Regional actor overhead, 512 entities | mutation p99 `<= 50 ms` | `3.004 ms` | PASS |
| 9 | 20-client balanced VD8 streaming | first-chunk p99 `<= 1,500 ms`; tick p99 `<= 50 ms`; zero reliable drops/timeouts/sheds | `32 ms`; `1.136 ms`; zero | PASS focused |
| 10 | Dense ECS, 1,000 active | per-tick work `< 50 ms` | `0.498 ms` indexed | PASS |
| 11 | Sparse ECS, 10,000 total / 32 active | per-tick work `< 50 ms` | `0.018 ms` | PASS |
| throughput | Revision-10 worldgen, six physical CPUs | `>= 743.578 chunks/s` | `2,689.834 chunks/s` | PASS |
| control | Autoscale startup under sustained pressure | no scale-up oscillation before pressure clears; down decisions remain hysteresis-bounded | `2 down / 0 up` before drain | PASS |

The fresh light-engine rerun produced `3.034 ms` flat, `3.800 ms` noisy and `11.073 ms` emissive full-recompute upper estimates; incremental edit-storm upper estimates were `1.326 ms` flat and `0.723 ms` noisy. Fresh entity/ECS reruns produced `18 us/tick` for 10,000/32 active, `472/498 us/tick` for the dense 1,000-entity full/indexed paths, actor p99 `3.004 ms`, and four-lane active-512 p99 `1.822 ms`.

No ordinary-play hotspot in this fresh ranked set remains over its mapped threshold. The top three rows remain explicit watch metrics because they consume the largest fraction of the frozen tick budget.

## Evidence boundary

This checkpoint supports Phase-3 baseline/profiling claims on this exact host/current dirty tree. It does **not** claim:

- the frozen low `2 vCPU / 4 GiB / 10 min` envelope;
- the full balanced `20 min / 8 GiB` envelope;
- the high VD32 `12 vCPU / 16 GiB / 20 min` envelope;
- fresh 30-minute or multi-hour slow-reader/transaction soak;
- slow-disk, memory-pressure, reconnect-storm, or real-client performance parity.

Those remain release/profile-envelope evidence gaps and must not be inferred from these bounded gates. They do not represent an unnamed ordinary-play hotspot that failed the Phase-3 ranked closeout; each must still be run before any later claim that its exact frozen profile envelope is `pass`.
