# Movement-rules crate-boundary evidence

Date: 2026-08-17

## Ownership cutover

The movement vertical now keeps pure geometry and coordinate rules below `mc-net` while preserving session, game-mode, world-residency, mutation, and publication authority in the network/simulation layer.

`mc-physics` owns:

- Euclidean per-step displacement admission with invalid budgets failing closed;
- authoritative pose finite-value and absolute coordinate-limit validation;
- bounded linear sweep sample-count calculation;
- finite yaw/pitch validation;
- finite world-position clamping to caller-supplied horizontal and vertical limits;
- strict body-vs-obstacle AABB overlap with explicit deflation;
- retained fall-start height across ground, water, and airborne transitions.

`mc-world` owns bounded conversion from an inclusive world-space X/Z rectangle to the touched `ChunkPos` set. The helper takes an explicit `max_chunks` ceiling and rejects non-finite, reversed, out-of-range, or oversized rectangles before allocating the result.

`mc-net::play::movement` deliberately retains:

- movement-limit policy selected from the authoritative game mode;
- `PlayerPose`/protocol adaptation and typed connection errors;
- loaded-destination and resident-snapshot checks;
- world-height validation against the actual chunk geometry;
- player collision semantics, powder-snow context, embedded escape policy, and swept collision queries;
- corrective teleport/publication behavior after rejection.

This cutover does not move world mutation or session state into lower crates and introduces no reverse dependency on `mc-net`.

## Correctness fences

- `mc-physics` tests prove exact Euclidean boundary behavior, non-finite rejection, coordinate-limit edges, invalid limit rejection, sweep sample counts, rotation validation, and position clamping.
- `mc-world` tests prove chunk coverage across positive and negative boundaries, inclusive edge handling, invalid input rejection, and allocation-cap rejection.
- Existing `mc-net` movement tests continue to prove game-mode displacement policy, unloaded/resident-world rejection, no tunneling, embedded escape without collision bypass, in-place non-expanding pose updates, and corrective resynchronization without local pose advancement.
- `xtask code-health` requires the displacement, pose-validation, sweep-sampling, rotation/clamp primitives to remain in `mc-physics`; requires movement to consume those primitives; requires bounded chunk coverage to remain in `mc-world`; and rejects direct lower-crate Cargo dependencies on `mc-net`.
- Giant gateway size ceilings remain unchanged; this checkpoint narrows a touched vertical domain rather than mechanically splitting unrelated orchestration.

Benchmark: not applicable. The algorithms preserve the existing movement limits and sweep spacing; the change is an ownership cutover plus bounded allocation/fail-closed validation.

## Validation

- `cargo test -p mc-physics`: 75 passed.
- `cargo test -p mc-world`: 255 passed, 15 ignored.
- `cargo test -p mc-net movement_tests`: 9 passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace --quiet`: passed; `mc-net` 1,932 passed and 5 ignored, `mc-physics` 73 passed, `mc-world` 255 passed and 15 ignored.
- `cargo fmt`: passed.

No manual graphical/client or performance gate is claimed by this checkpoint. `SOL-042` giant-gateway extraction remains separately blocked by the CodexPro source write guard; this movement cutover does not depend on bypassing that policy.
