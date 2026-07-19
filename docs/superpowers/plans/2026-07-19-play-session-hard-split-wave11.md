# Play And Session Hard Split Wave 11

**Goal:** extract player movement rules and generic server-entity lifecycle
authority from the two remaining coordinators without changing wire order,
collision correction, entity indexes, or visibility publication.

## Global Constraints

- Preserve behavior and packet layouts; this wave adds no gameplay.
- Use explicit imports and concrete types. Add no trait layer, parent wildcard
  import, sleep, polling, new lock, or direct channel send.
- Keep async packet I/O and loaded-world acquisition in `play.rs`.
- Keep registry fields and guard acquisition in `session.rs`; actual visibility
  delivery remains in the outbound owner.
- Preserve current mutation, lock, index, and publication order exactly.

## Task 1: Player Movement Rules

**Files:** `crates/mc-net/src/play.rs`, new
`crates/mc-net/src/play/movement.rs`

- [x] Move accepted absolute movement validation/normalization, movement
  exhaustion, water/contact and collision predicates, fall-damage planning,
  farmland landing rules, and pending-teleport state transitions into
  `movement.rs` where those rules are synchronous.
- [x] Keep packet decoding, async world reads, collision correction packet
  writes, teleport sync/resend writes, simulation commits, and player/session
  publication in `play.rs`.
- [x] Preserve non-finite rejection, coordinate bounds, yaw/pitch normalization,
  pending-confirm movement gate, teleport ID wrap, sprint/jump exhaustion,
  vanilla farmland walk height, and fall-damage thresholds.

## Task 2: Generic Entity Lifecycle

**Files:** `crates/mc-net/src/play/session.rs`, new
`crates/mc-net/src/play/session/entity_lifecycle.rs`, and only the existing
session children whose explicit imports must follow the moved helpers.

- [x] Move falling-block and command-entity spawn authority, dying-entity
  completion, nearby candidate/snapshot queries, generic removal cleanup, and
  entity chunk-index maintenance into `entity_lifecycle.rs`.
- [x] Keep registry fields, guard acquisition, specialized pickup/projectile/
  explosion authority, simulation routing, and actual delivery in their
  existing owners.
- [x] Preserve EntityStore -> SessionRegistry lock order, stable entity-ID
  ordering, current-snapshot conditional removal, all auxiliary-map cleanup,
  visibility reservation before delivery, and chunk-index consistency.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and boundary scans for both modules;
  update ADR 0006, memory, and append-only WAL.
- [x] Run focused movement/teleport/fall/collision and entity lifecycle/index
  tests, full `mc-net`, strict Clippy, fmt, code-health, and independent reviews.
