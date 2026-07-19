# Play And Session Hard Split Wave 10

**Goal:** move block-placement rules and non-regional survival world-action
authority out of the remaining coordinators without changing snapshots,
mutation fences, inventory conservation, or publication order.

## Global Constraints

- Preserve behavior and existing packet layouts; this wave adds no gameplay.
- Use explicit imports and concrete types. Add no trait layer, wildcard parent
  import, async child function, direct send, sleep, polling, or new lock.
- Keep snapshot acquisition, simulation routing, durability, relight,
  recipient selection, and delivery in their current owners.
- Preserve world -> session -> entity/player-persistence lock order and release
  gameplay locks before publication.

## Task 1: Block Placement Rules

**Files:** `crates/mc-net/src/play.rs`, new
`crates/mc-net/src/play/block_placement.rs`

- [x] Move `PlannedBlockPlacement`, placement edit planning, cactus
  support/cascade/obstruction, sign state/editor rules, direction, sign
  rotation, and door-half helpers into `block_placement.rs`.
- [x] Keep `handle_block_item_placement`, loaded snapshot acquisition,
  simulation-owner commit, token fencing, relight, sign-editor publication,
  and packet writes in `play.rs`.
- [x] Preserve exact snapshot sets for signs, doors, ordinary placement, and
  cactus; continue using the existing vertical-plant survival rule.

## Task 2: Survival Action Authority

**Files:** `crates/mc-net/src/play/session.rs`, new
`crates/mc-net/src/play/session/survival_action_authority.rs`

- [x] Move survival break/placement/bucket direct commits and their regional
  transaction preparation into `survival_action_authority.rs`.
- [x] Keep TNT ignition with explosion authority, food use with player state,
  regional transaction commits in `transactions.rs`, and routing/publication
  in `simulation.rs`.
- [x] Preserve conditional world-edit-before-inventory order, exact drop
  materialization, stale rejection, requester-loss behavior, and lock order.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and boundary checks for both modules;
  update ADR 0006, memory, and append-only WAL.
- [x] Run focused placement and survival transaction tests, full `mc-net`,
  strict Clippy, fmt, code-health, and independent reviews.
