# Natural-spawn crate-boundary evidence

Date: 2026-07-30

Checkpoint base: `9319fa75430a03a037c65df49b9842e0f2dc30d1`

## Ownership cutover

The production natural-spawn scheduler and planner previously lived under
`mc-net::play::session::herd_spawn_authority`. The cutover moves the
caller-neutral herd template, deterministic identity, caps, rotating scheduler,
metrics, distance, terrain, darkness, collision, and candidate rules into
`mc_entity::natural_spawn_26_1_2`.

`mc-net` still owns only the live adapters that must see runtime authority:

- selected session/player and active-chunk snapshots;
- read-only world snapshot acquisition;
- regional entity-owner commit and stale active-chunk rollback;
- visibility installation, recipient dispatch, and metric logging;
- the server ticker and operator interval normalization.

The existing spawn-fact, default-goal, geometry, and aquatic-classification
call sites delegate to the lower-crate authority instead of retaining duplicate
formulas in `mc-net`. No lock, channel, async task, persistence order, commit
owner, or outbound DTO moved.

`mc-entity` now directly depends on `mc-physics` and `mc-world` for collision
geometry and read-only terrain/light planning. Neither crate depends on
`mc-entity`, so the dependency remains one-way. This concrete dependency
change is the reason `Cargo.lock` changes in this checkpoint.

## Correctness fences

- Lower-crate tests cover bounded independent scheduler cursors, cumulative
  metric intervals, exact player-distance rejection, and per-chunk friendly
  and hostile caps.
- Existing `mc-net` periodic-spawn tests remain the authority for cadence,
  selected-chunk snapshot bounds, terrain/fluid/darkness admission, population
  refill, owner commit, and publication.
- `xtask code-health` pins the scheduler/planner/DTO owner in `mc-entity` and
  rejects session, outbound, async, channel, packet, and reverse `mc-net`
  dependencies from that domain.

Benchmark: not applicable. This is a mechanical ownership cutover with no
algorithm or feature performance contract change; the mapped natural-spawn and
release benchmark gates remain at their existing feature/release boundaries.

## Validation

- `cargo test -p mc-entity natural_spawn_26_1_2`: 3 passed.
- `cargo test -p mc-net periodic_`: 12 passed.
- `cargo test -p mc-entity`: 573 passed, 6 ignored; production ECS integration:
  8 passed.
- `cargo test -p mc-net`: 1,849 passed, 5 ignored; doc tests: 3 passed.
- `cargo run -p xtask -- code-health`: `0 fail`, `KEEP`.
- `cargo test --workspace`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- Independent read-only review against the checkpoint base: `pass`, no
  findings.

The PrismLauncher graphical/client gate was not run. This checkpoint changes
crate ownership without claiming new gameplay, graphical, performance, or
release readiness evidence.
