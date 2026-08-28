# Chunk-stream planning crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

The first bounded chunk-stream extraction moves protocol-neutral coordinate and prewarm planning from `mc-net::play::chunk_stream` into `mc-world::chunk_stream_plan_26_1_2`.

`mc-world` now owns:

- Chebyshev-ring spiral coverage and look-direction-prioritized ordering with caller-supplied view-distance ceilings;
- Minecraft-yaw forward-vector, directional score, and lateral-distance math;
- one-ring-beyond-view prewarm ordering;
- bounded prewarm batch sizing and near-edge batch planning;
- distance-to-signed-chunk-edge calculation;
- initial stream-window target sizing.

`mc-net` deliberately retains:

- protocol/server policy constants such as `MAX_VIEW_DISTANCE`, minimum initial ring, and prewarm batch ceiling;
- `PlayerPose` adaptation to protocol-neutral X/Z inputs;
- stream generation ids, scheduler queues, cancellation, worker admission, and in-flight state;
- chunk generation/load/light work, cache/backpressure, packet preparation, and publication.

This keeps policy and runtime authority in `mc-net` while making deterministic chunk-coordinate planning reusable and directly testable without network/session dependencies.

## Correctness fences

- `mc-world` tests cover bounded spiral uniqueness/coverage, full prewarm-ring uniqueness, near-edge batch selection and limit behavior, yaw direction, signed-edge distance, and initial-window sizing.
- Existing `mc-net` tests continue to prove spiral ordering, look-direction priority, untrusted view-distance capping, full prewarm edge ordering, and near-east/near-west batch behavior through the compatibility adapters.
- `xtask code-health` requires the planner primitives to remain in `mc-world` and requires the `mc-net` adapter to consume them.
- The general lower-crate transport/session and reverse-dependency guards apply to the new module.

Benchmark: not claimed. This checkpoint preserves existing ordering and limits and changes ownership only.

## Validation

- `cargo test -p mc-world chunk_stream_plan_26_1_2`: 5 passed.
- `cargo test -p mc-net spiral`: 5 passed.
- `cargo test -p mc-net prewarm_edge`: 2 passed.
- `cargo test -p mc-net prewarm_batch_`: 2 passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --check`: passed.
- `cargo test --workspace --quiet`: passed; `mc-data` 234 passed/25 ignored, `mc-entity` 586 passed/6 ignored, `mc-net` 1,932 passed/5 ignored, `mc-physics` 75 passed, `mc-world` 261 passed/15 ignored, and all executable integration groups completed without failure.

No graphical/client or throughput claim is made by this evidence.
