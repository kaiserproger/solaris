# M67 Test Coverage Review

This review records where current coverage is sufficient for scoped Solaris
behavior and where M68+ should add harness or oracle-backed tests before more
features land.

## Strong Coverage

| Area | Evidence | Notes |
|---|---|---|
| Login/config/play smoke | `crates/mc-test-harness/tests/login.rs`, `configuration.rs`, and `play.rs` cover basic client entry and keepalive behavior. | Good baseline for protocol liveness. |
| Chunk streaming and visibility | `chunk_stream.rs`, `player_presence.rs`, and `load_scenarios.rs` cover spawn chunks, movement replanning, two-client visibility, and load pressure. | Keep these as regression gates for future networking cleanup. |
| Core block-edit path | `block_edit.rs` includes break/place/crafting/furnace/chest/campfire/sapling/wheat survival flows. | Good coverage for ack, block update, relight, inventory, and item entity seams. |
| Physics-facing behavior | `physics_validation.rs` covers collision, fall damage, water entry, falling blocks, and sugar cane support-break observations. | Useful bridge between unit physics and wire-visible behavior. |
| Combat smoke | `mob_presence.rs` covers mob visibility, damage, drops, and shield blocking. | Enough for current partial combat scope; not full vanilla parity. |

## Coverage Gaps

| Gap | Current Coverage | Risk | Suggested Action |
|---|---|---|---|
| Recent plant lifecycle slices | M60 oak sapling and M57/M58 wheat have harness coverage; M61-M66 plant slices are mostly focused unit tests. | Unit tests prove helpers but not use-on/break/random-tick integration through a vanilla-like client path. | Add one stable harness scenario for deterministic use-on plant behavior, likely sweet berry harvest or cocoa placement. Keep random-tick-only behavior unit-covered unless timing can be controlled. |
| Container click refactor safety | Containers have harness coverage, but duplicated code paths make it hard to know if every click mode is covered equally. | M68 container cleanup could regress a mode not directly asserted by harness. | Before refactoring, map pickup/quick-move/swap/throw coverage per inventory/crafting/chest/furnace and add missing unit tests rather than more broad harness tests. |
| Sign editing and metadata paths | Deferred because packet layouts need oracle evidence. | Implementing without oracle would violate ADR 0002 and risk client disconnects/desync. | Add oracle capture tasks before any sign-edit or visual metadata implementation. |
| Manual PrismLauncher gate | M60-M66 closeouts explicitly did not run manual client gates. | Cargo tests can miss visual desync, missing animations, or client prediction mismatches. | Run one manual gate before or during M68 and record results in the cleanup plan. |
| Drop policy | Crop/cocoa/sweet berry drops are unit-covered; mature wheat has harness coverage. | More deterministic drop tables can drift from block-break integration expectations. | If deterministic drops remain policy, add a small table-driven unit suite and one additional harness break scenario for a non-wheat plant only if stable. |
| Vanilla parity oracle scenarios | `parity_oracle.rs` has ignored vanilla comparison tests requiring `.analysis/server.jar`. | Useful but not part of routine gate; parity claims can silently drift if not run. | Keep ignored tests explicit; run selected oracle tests before any claim that behavior matches vanilla. |

## M68 Candidates From This Review

- Add a deterministic plant harness scenario for sweet berry harvest or cocoa
  placement after plant policy extraction starts.
- Build a container click coverage matrix before refactoring shared menu logic.
- Add unit coverage around the future plant policy module rather than only around
  current helper functions.
- Run a manual PrismLauncher pass and paste outcomes into M68 closeout or an
  operator note.
- Keep oracle tests ignored by default, but require explicit oracle runs for new
  packet/metadata parity claims.
