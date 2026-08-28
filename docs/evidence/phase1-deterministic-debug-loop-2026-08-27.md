# Phase 1 deterministic feature-debug loop — 2026-08-27

## Scope

This checkpoint advances Phase-1 item 5 in `docs/PUBLIC_ALPHA_PLAN.md` with one complete active-feature route for bucket/block-resync behavior. The route deliberately crosses the four validation layers named by the plan instead of relying on whichever test or real-client scenario happened to run most recently.

The new entrypoint is:

```sh
bash tools/run-bucket-resync-debug-loop.sh --automated
```

It runs, in order:

1. the exact affected-crate production-path regression `play::bucket_interactions::tests::committed_bucket_response_orders_block_ack_before_inventory_update`;
2. the exact raw-TCP/harness regression `rejected_occupied_bucket_use_item_on_resyncs_blocks_and_held_slot_before_ack` with its explicit local-sidecar opt-in;
3. the exact save/restart regression `water_bucket_scheduled_spread_survives_save_restart_without_duplicate_tick` with the same explicit opt-in;
4. the common real-client runner's fail-closed preflight for manifest `docs/real-client-regression/manifests/m94-regression-pack.json` and scenario `m94-02b-rejected-block-resync`.

Every Cargo test is selected with `--exact`. The real-client route is fixed to a fresh world and server seed `0`; the common runner still validates that the scenario exists exactly once in the selected evidence manifest before launch.

The graphical continuation is explicit rather than implicit:

```sh
bash tools/run-bucket-resync-debug-loop.sh --real-client
```

`--all` runs the automated layers followed by that exact graphical scenario.

## Graphical prerequisite failure found during the checkpoint

Before the fail-fast fix, a real graphical attempt produced:

`.analysis/real-client-runs/20260827T073109Z-m94-regression-pack-Lmn0EK`

The runner spent about 202 seconds waiting for primary bridge readiness. This was not a gameplay failure: the client log reached NeoForge early display initialization, then repeatedly reported `ERROR DISPLAY`, `Failed to initialize the mod loading system and display`, and `glfwInit failed`. The CodexPro execution environment has neither `DISPLAY` nor `WAYLAND_DISPLAY`.

The common `tools/run-real-client-regression.sh --run` path now checks that graphical prerequisite after manifest/scenario validation and before creating a run directory or starting server/client work. Missing both variables fails immediately with:

```text
error: --run requires a graphical display; set DISPLAY or WAYLAND_DISPLAY
```

An executable `real_client_manifest` regression removes both environment variables and proves that the runner fails before printing `running real-client regression into`, so this cannot silently regress back to a multi-minute readiness timeout.

## Validation

Final-tree checks for this checkpoint:

- `bash -n tools/run-real-client-regression.sh` — PASS;
- `bash -n tools/run-bucket-resync-debug-loop.sh` — PASS;
- `real_client_runner_run_fails_fast_without_graphical_display` — 1/1 PASS; observed test duration about 0.03 s;
- `bucket_resync_debug_loop_preflight_uses_declared_real_client_scenario` — 1/1 PASS;
- `bash tools/run-bucket-resync-debug-loop.sh --automated` — PASS:
  - affected-crate bucket response-order test 1/1;
  - TCP/harness occupied-bucket rejection/resync test 1/1;
  - save/restart scheduled-water test 1/1;
  - exact real-client scenario preflight PASS;
- `cargo test -p mc-test-harness --test real_client_manifest --quiet` — 50/50 PASS;
- `cargo run -p xtask -- code-health` — `0 fail / KEEP`;
- `cargo fmt --all -- --check` — PASS;
- post-fix `bash tools/run-bucket-resync-debug-loop.sh --real-client` in the current non-graphical CodexPro environment — correctly fails in about 0.06 s on the explicit display prerequisite.

Benchmark: not applicable. This checkpoint changes development/test routing and prerequisite handling, not a measured production hot path.

## Graphical closeout — 2026-08-28

The fixed route was then run on an isolated local Xvfb display, which is an established Solaris graphical-client evidence path:

```sh
env LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s "-screen 0 1280x720x24" \
  bash tools/run-bucket-resync-debug-loop.sh --real-client
```

The command exited zero in about 34 seconds and the common runner validated:

`.analysis/real-client-runs/20260828T005009Z-m94-regression-pack-HL7nZB`

`observations.json` records `client_gate = agent-run-real-client`, `result = passed`, and exact scenario `m94-02b-rejected-block-resync`. The client reached Play in `minecraft:overworld`. The scenario observed:

- occupied solid placement rejected with both authoritative blocks stable, no fluid ghost, and held dirt unchanged;
- out-of-reach solid placement rejected with both authoritative blocks stable, no fluid ghost, and held dirt unchanged;
- occupied water-bucket fallback returned `Pass[]` while both blocks stayed authoritative, no fluid appeared, and the held water bucket remained unchanged.

The final server-log scan found no warning/error/disconnect/timeout/slow-tick match; the only matching line was the normal informational chunk-stream completion record.

## Evidence boundary and disposition

The earlier 202-second no-display run remains diagnostic evidence only and is not counted as gameplay evidence. The Xvfb artifact above is the passing graphical Minecraft 26.1.2 result for the same fixed manifest/scenario route and was accepted by the existing fail-closed run validator.

Phase-1 item 5 is **closed**: one deterministic active-feature route now spans affected-crate, TCP/harness, persistence/restart, and exact graphical real-client evidence without relying on latest/default test selection.

Independent read-only review: **PASS** with no findings. The reviewer checked the scoped route, common graphical-prerequisite change, executable regressions, evidence boundary, and negative-code/scope risk before the final runtime-only graphical closeout; no implementation changed after that review.
