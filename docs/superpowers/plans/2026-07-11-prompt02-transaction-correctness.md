# Prompt 02: Multiplayer Transaction Correctness Plan

**Quality label:** `stabilization`.

**Goal:** Make shared multiplayer mutations linearizable, replayable, and
conservative under stale actions before changing runtime ownership.

## Slice 1: Block mutation preconditions

- [x] Add a two-session TCP regression where one player starts mining, a peer
  breaks and replaces the target, and the stale stop must resync without
  breaking the replacement or producing a drop.
- [x] Capture the authoritative target state at mining start and require it as
  an atomic precondition on completion.
- [x] Apply the same conditional mutation primitive to block placement so a
  target validated as air cannot overwrite a peer mutation after an await.
- [x] Add repeated concurrent same-target placement conservation coverage:
  one winner, one consumed stack, one final authoritative block.

## Slice 2: Shared container transactions

- [x] Extend chest/furnace state ids into explicit compare-and-mutate results
  for concurrent valid clicks, not only stale sequential clicks.
- [x] Cover chest concurrent cursor conservation, furnace tick/click ordering,
  and hopper-origin state publication with reproducible ordering.
- [x] Save/restart after shared-container contention and compare exact storage,
  player inventory, and cursor outcomes after reconnect.

## Slice 3: Item, XP, death, and combat claims

- [x] Drive simultaneous item and XP pickup through real session tasks and
  assert one claimant plus exact stack/XP conservation.
- [x] Cover simultaneous lethal mob damage through real attack tasks and assert
  one removal, one loot entity, and one XP reward.
- [x] Cover player death inventory/XP drops plus save/restart without duplicate
  rewards or lost authoritative state.

## Slice 4: Session and chunk cancellation

- [x] Prove duplicate login ordering and reconnect after session release.
- [x] Cancel or reject stale chunk results after disconnect/replan and expose
  bounded cancellation/drain counters.
- [x] Replace the M96 paused-reader probe with deterministic bounded pressure
  evidence. A bounded operator-only burst now saturates the normal reliable
  outbound queue deterministically; this is queue-pressure evidence, not a
  natural TCP slow-reader soak.

## Slice 5: Replay and duration gates

- [x] Extend the checked replay contract with concurrent action groups and
  state-conservation observations; persist minimal failing seeds.
- [x] Run repeated deterministic contention seeds, then the four-active-plus-
  one-slow-reader 30-minute fallback workload.
- [x] Save/restart after the mixed workload and compare final authoritative
  state.
- [x] Record focused two-real-client shared edit/container/pickup evidence.
- [x] Run the full Cargo baseline and document every skipped oracle/client/
  performance/soak gate without promoting readiness rows.
