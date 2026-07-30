# `mc-server` ignored-test classification

Scope: Phase 1 test inventory for `crates/mc-server`.

The crate has exactly four ignored tests, all in
`tests/generated_world_startup.rs`. Every gate starts the real `mc-server`
binary against a temporary disk-backed world and requires the local 26.1.2
`data/vanilla` sidecars. Missing prerequisites fail immediately through
`assert_required_sidecars`; none of these tests self-skips as a green result.

## Inventory

| Ignored test | Boundary and prerequisite | Existing executable coverage | Owner and exact close condition |
| --- | --- | --- | --- |
| `disk_backed_generated_world_startup_stream_budget` | Fresh generated world followed by a warmed restart; both clients receive the complete 289-chunk view-distance-8 window. The fresh startup-to-listener limit is 10 seconds. | Ordinary `mc-world` storage tests cover dirty flushing and reopen, while `mc-net` and harness tests cover chunk preparation, streaming, save, restart, and shutdown separately. The complete binary/startup timing composition exists only in this opt-in gate. | Phase 3 startup/streaming performance and release closeout. Run on the exact candidate and declared host with 26.1.2 sidecars; require fresh startup at or below 10 seconds, complete unique first and warmed streams, a drained 361-chunk startup checkpoint, quiescent shutdown saves, and a recorded benchmark artifact. |
| `disk_backed_generated_world_startup_checkpoint_survives_kill` | Fresh generation must finish the startup dirty checkpoint before the server process is killed without `stop`; reopening must find all 361 spawn-window chunks and valid baked light for the 289-chunk spawn view. | Ordinary storage and dirty-flush tests cover write/reopen behavior and stale-plan fences, but not the complete binary crash boundary after startup pre-generation. | Phase 1 persistence integration, owned by `mc-server` startup and `mc-world` storage. Run after a material startup-checkpoint/durability change and on the release candidate; close only when the killed process exits unsuccessfully and every expected chunk/light record reopens from disk. |
| `disk_backed_existing_world_missing_light_startup_stream_budget` | An existing unbaked world must defer startup writes, stream the complete 289-chunk view, drain exactly 289 dirty chunks asynchronously, and reach the listener within 10 seconds. | Ordinary world/light tests cover light bake persistence and dirty flushes; chunk-stream tests cover the wire window. Their full existing-world startup composition and timing remain opt-in. | Phase 3 startup/light performance and release closeout. Run on the exact candidate and declared host with 26.1.2 sidecars; require startup at or below 10 seconds, the complete unique stream, the expected deferred-flush summary, a drained 289-chunk checkpoint, quiescent shutdown, and a recorded benchmark artifact. |
| `disk_backed_generated_world_console_stop_drains_stream_load` | Four clients each reach play and receive nine unique chunks before `stop` is sent through console stdin; shutdown must quiesce before the final save and every observed streamed chunk must reopen from disk. Autoscaling is enabled. | Ordinary server tests cover shutdown notification and save ordering; harness tests cover multi-client streaming and persistence. The stdin-to-process-exit path under concurrent generation exists only in this opt-in gate. | Phase 1 shutdown integration, owned by `mc-server` process orchestration. Run after a material console/shutdown/drain change and on the release candidate; require successful process exit, shutdown-before-save ordering, a zero-dirty final save, and all streamed chunks present after reopen. |

## Current disposition

The bounded inventory command compiled the integration target and listed these
four tests and no others. The available local sidecar reports identify
Minecraft `26.1.2`, world version `4790`, and protocol version `775`.

No ignored gate was executed during this classification checkpoint. The two
startup budget tests are feature-boundary/release performance evidence, so
classification alone is not a reason to reproduce them. The crash and console
shutdown gates remain explicit opt-in integration evidence because the ordinary
workspace suite cannot depend on local Mojang sidecars or spawn child server
processes for every run.

`benchmark: not applicable`: this checkpoint changes only the inventory and
ownership record; it does not change a measured startup, streaming, light, or
shutdown path.

## Reproduction

List the exact ignored inventory without executing it:

```sh
cargo test -p mc-server --test generated_world_startup -- --list --ignored
```

Run one mapped gate explicitly. Do not use an unfiltered `--ignored` command
because it would combine both performance workloads and both process-level
integration gates:

```sh
cargo test -p mc-server --test generated_world_startup \
  disk_backed_generated_world_startup_checkpoint_survives_kill \
  -- --exact --include-ignored

cargo test -p mc-server --test generated_world_startup \
  disk_backed_generated_world_console_stop_drains_stream_load \
  -- --exact --include-ignored
```
