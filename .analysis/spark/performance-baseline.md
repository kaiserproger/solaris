# Performance Baseline (T00-05)

Generated: 2026-07-24T10:35:12Z (owner UTC)
Source tree: `/home/kaiserroman/solaris-spark-worktrees/tasks/T00-05`

## Provenance

- Commit: `6fb2212`
- Branch: `agent/t00-05-6fb22122`
- Base tree: `6fb221222a2c390a24015671dbca2536e2f57e6f`
- Rust: `1.94.1` (`rustc e408947bfd200af42db322daf0fadfe7e26d3bd1`)
- Cargo: `1.94.1`
- Runtime build: debug, unoptimized + debuginfo
- Test command used for discovery/evidence: `cargo test -p mc-test-harness --test load_scenarios -- --list`
- Hardware (owner): AMD Ryzen 5 7535HS, 12 logical CPUs, Linux kaiserpc, 7.0.0-28-generic

## Reproducible workload catalogue

### Non-ignored test workload

- `prompt01_workload_latency_percentiles_use_nearest_rank` (unit helper test).
- `checked_multiplayer_transaction_replay_is_deterministic_and_conservative` (ignored in default run; requires sidecars when run with `--ignored`).
- `duplicate_lethal_player_commands_drop_one_bundle_and_survive_restart` (ignored in default run; requires sidecars).

### Ignored/manual-load workloads in `load_scenarios.rs`

Observed via `cargo test -p mc-test-harness --test load_scenarios -- --ignored --list`:

- `bounded_multiplayer_survival_replay_covers_sequential_contention_and_slow_reader`
- `concurrent_same_target_placements_consume_exactly_one_stack`
- `concurrent_shared_chest_same_state_commits_one_cursor_transaction`
- `vd8_multi_client_stop_drains_and_flushes_disk_world_under_stream_load`
- `vd8_twenty_same_spawn_clients_drain_full_window_and_stop_without_duplicate_pressure`
- `multicore_login_chunk_stream_and_broadcast_stays_within_budgets`
- `paused_reader_does_not_stall_active_entity_broadcasts`
- `paused_reader_pressure_does_not_delay_healthy_observers`
- `prompt02_multiplayer_transaction_soak_short_preflight`
- `prompt02_four_active_one_slow_reader_transaction_soak_36000_ticks`
- `reports_spawn_exploration_block_entity_and_multi_client_load`

## Test matrix (exact constants from code)

- View distance in load helpers: `VIEW_DISTANCE = 1`.
- M52 baseline: 4 clients, 8 summons, 30s budget, lock budget 250_000 us, slow reader 256 summons, healthy observers 128 summons.
- M96 replay: 4 clients, elapsed budget 45s, cancel budget 64, outbound burst 192, lock budget 250_000 us.
- O2/VD8: 4 stop-clients, view distance 8, concurrent 20 clients, one full-window chunk expectation, first-chunk p99 budget 2500ms, tick p99 50_000us, entity physics max 50_000us.
- Prompt02 soak helpers: 4 active clients and configurable target tick duration.

## Required metrics and latest known limits (source of truth)

### Core metrics surfaced by load workloads

- Tick percentiles (`tick`, `world_time`, `entity_goals`, `entity_physics`, `entity_dispatch`, `campfire_tick`, `entity_save`, `random_tick`, `block_tick`, `fluid_tick`, `hostile_attacks`, `animal_breeding`) with `samples/p50/p95/p99/max` in microseconds.
- Runtime telemetry fields used in assertions:
  - `tick.sample` thresholds and `observer_submit_us`, `observer_compute_us`, `observer_skipped_windows`.
  - `active_sessions`, `ticketed_chunks`, memory fields (`memory_used_mb`, `memory_limit_mb`), persistence outcomes.
- Lock metrics:
  - `world_storage`, `session_registry`, `chunk_prepare`, `player_persistence`, `save_all_flush` with `wait_count/wait_us/max_wait_us/hold_count/hold_us/max_hold_us`.
- Outbound pressure:
  - `best_effort_animation_drops`, `reliable_command_drops`, `reliable_command_retries`, `reliable_command_retries_in_flight`, `max_reliable_command_retries_in_flight`, `slow_client_write_timeouts`, `slow_client_pressure_sheds`.
- Chunk resources:
  - `active_cpu/io`, `max_cpu_active`, `max_io_active`, stop reasons and cancellation snapshots.
- World storage stats (`world_storage_stats_json`) and autoscale report gaps.

### Latest numeric checkpoints already recorded in repo docs

- M52 notes define a 30s four-client baseline and 250ms lock hold budgets.
- M91 notes an M77 generated-world reference run: all 289 chunks, 17.3s, with repeated tick/lock warnings.
- M91/O2 debug marker reports one explicit 400-tick gate with
  `observer_submit_us = 12`, `observer_compute_us = 666`, `observer_skipped_windows = 0`.
- This card did not execute soak/scenario runs to refresh end-to-end p50/p95/p99/max values.

## Static metric-name verification

Verified metric keys/field names referenced in `docs/M52_OPERATOR_PERFORMANCE_NOTES.md` and `docs/milestones/M91.md` against code symbols using `rg` against `crates/mc-net` and `crates/mc-test-harness/tests/load_scenarios.rs`:

- session lock metrics (`session_registry.max_wait_us`, `session_registry.max_hold_us`) — present
- world storage lock metrics (`world_storage.max_wait_us`, `world_storage.max_hold_us`) — present
- chunk prepare/player persistence/save-all locks (`chunk_prepare.*`, `player_persistence.*`, `save_all_flush.*`) — present
- outbound pressure fields (`best_effort_animation_drops`, `reliable_command_drops`, `reliable_command_retries`, `reliable_command_retries_in_flight`, `slow_client_write_timeouts`, `slow_client_pressure_sheds`) — present

## Known gaps / uncovered profiles

- No run of `--ignored` workloads with local sidecars was executed in this task.
- No successful real-client soak, manual Oracle, or 20-client balanced production-like run was executed.
- `T00-05` currently establishes current workload+metric inventory and contract checks; evidence numbers remain from in-repo documentation and test contracts.
