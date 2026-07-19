# Prompt 03B Atomic Arrow Pickup Plan

Quality label: `stabilization`.

## Scope

Move grounded-arrow removal plus arrow inventory credit into one
simulation-owner transaction. Validate grounded/stationary arrow state and
inventory capacity before either aggregate changes. Dispatch visibility from
the owner and return an immutable inventory snapshot to the connection.

This slice does not migrate projectile spawn/debit, bow durability, other
inventory actions, combat state, pose, containers, or save barriers.

## Tasks

- [x] Add RED requester-loss, full-inventory, stale-session, and concurrent
  conservation tests.
- [x] Add a session-fenced owner command that commits arrow removal and one-item
  inventory credit together.
- [x] Make the connection mirror owner inventory without a second local merge.
- [x] Keep claim-only arrow helpers available only to equivalence tests.
- [x] Run focused simulation/connection tests, wire/client regression,
  replay/soak, and workspace baseline.

## Evidence

- RED/GREEN owner tests prove requester-loss durability, full-inventory no-op,
  stale-session fencing, and one exact winner across two sessions.
- A connection-level test drives `pickup_nearby_arrows`, verifies one credited
  arrow, entity removal, and exactly one decoded inventory slot update.
- `mc-net` passes 489/489 tests; focused clippy passes with warnings denied.
- Prompt 02 checked replay and the short four-active-plus-one-paused-reader soak
  pass after the cutover.
- The 17-tool embedded MCP passed `playable-01-join-generated-spawn`; Solaris
  performed its final save and exited with status 0.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo fmt --all -- --check`, and `cargo run -p xtask --
  code-health` pass. Oracle/load rows marked ignored by the workspace suite were
  not promoted by this baseline.
- No MCP client/server process or listener remained on ports 25565 or 39095
  after the live gate.
