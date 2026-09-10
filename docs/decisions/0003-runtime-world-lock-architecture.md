# ADR 0003 - Runtime world lock architecture

**Date:** 2026-06-01
**Status:** Accepted for legacy paths; staged supersession by ADR 0004
**Context:** M68.d architecture drift note

## Context

`PROJECT_SPEC.md` describes a target server architecture with clear ownership
boundaries between networking, simulation, persistence, and world mutation. The
current M68 codebase has intentionally drifted while milestones prioritized
client-visible gameplay and wire compatibility:

- `WorldHandle` is `Arc<tokio::sync::Mutex<WorldStorage>>`.
- Each play connection owns an `InteractionState` with session-local inventory,
  carried item, active container, pending break/use state, shield state, and a
  per-connection light cache.
- Many interaction handlers briefly lock world storage to read or mutate blocks,
  block entities, and scheduled ticks.
- Long-running broadcast, packet writes, entity visibility dispatch, and most
  inventory mutation are kept outside the world lock.

This shape is not the final single-writer simulation model from the project
spec, but it is the architecture Solaris has shipped through M68.

## Decision

Keep the current shared world lock model for the near-term milestone track. Treat
it as an explicit transitional architecture, not accidental technical debt.

Acceptable world-lock usage today:

- Short storage reads for interaction target checks, block facts, block entity
  state, collision samples, water overlap, and spawn/material probes.
- Short storage mutations for player-authored block edits, block entity updates,
  scheduled ticks, and persistence-backed container state.
- Building a local snapshot under the lock, dropping the guard, then writing
  packets or dispatching visibility commands.
- Relight planning that locks only while reading chunks or applying already
  chosen edits.

Code should avoid:

- Holding `world.lock().await` across network writes, session broadcasts, sleeps,
  recipe work that does not need storage, or expensive scans that can use a
  snapshot.
- Mutating `InteractionState` inventory/container state while also performing
  unrelated world I/O unless the operation must be atomic from the client action
  perspective.
- Introducing additional global mutable state that competes with `WorldHandle`
  without documenting ownership.

Remain single-writer or main-loop owned:

- Per-session connection state in `InteractionState`.
- `SessionRegistry` visibility and outbound command ownership.
- Entity lifecycle dispatch decisions before they become storage persistence.
- Packet encode/write ordering for one client connection.

Future milestones may replace `WorldHandle` with a simulation actor or command
queue, but that is a separate concurrency redesign. Until then, cleanup should
make lock spans smaller and more obvious rather than pretending the final model
already exists.

ADR 0004 starts that redesign. Its typed simulation commands supersede this
ADR only for domains explicitly migrated behind `SimulationHandle`; all other
world, session, container, and entity paths remain governed by ADR 0003 until a
later slice transfers their authority.

## Consequences

Positive:

- Documents the real M68 architecture so new cleanup work does not chase the
  original spec blindly.
- Gives reviewers a concrete rule for world-lock changes: short critical
  sections are acceptable; lock-spanning I/O is not.
- Lets gameplay cleanup continue without forcing an M39-scale runtime rewrite.

Negative:

- Solaris still has coarse world-storage serialization during concurrent player
  interactions.
- Some interaction handlers still mix protocol, inventory, world I/O, and
  visibility dispatch; M68 cleanup reduces this but does not eliminate it.
- The eventual actor/single-writer design will need migration work and tests for
  ordering-sensitive interactions.

## Implementation Notes

- `crates/mc-net/src/play.rs` is the main drift point: `InteractionState` owns
  per-connection state and locks `WorldHandle` for interaction-driven storage
  work.
- `crates/mc-net/src/server.rs` defines `WorldHandle` as the shared async mutex
  over `WorldStorage`.
- M68.b and M68.c are examples of acceptable transitional cleanup: they clarify
  control flow without changing world ownership.
- Dirty-cache pressure has one production persistence authority: the
  server-owned `DirtyFlushCoordinator`. Chunk preparation publishes a
  coalesced request and waits for the exact accepted worker action; a stream
  generation change wakes and cancels stale waiters. It must not start a
  competing full flush. Tests without the server worker may use the bounded
  eight-chunk fallback, but that fallback is not a second production owner.
- Indirect `ChunkSection` palettes and packed indices share an immutable
  `Arc` payload across chunk snapshots. Editing detaches only the changed
  section; no-op writes do not detach. Single-valued sections stay
  allocation-free. The chunk mutation/replacement fences, Anvil encoding,
  persistence authority and visible block values are unchanged.
  Per-chunk heap budgeting conservatively charges the shared payload and its
  reference-count header; it is not a deduplicated process-heap measurement.
  This removes full-section copies during snapshot overlap, not the need to
  retain genuinely live chunks or buffers. Reverting the private section payload
  representation removes the optimization without a world-format migration.
  The fixed 1,089-chunk allocation probe measured 1.7% less resident payload and
  35.5% less live heap during edits with an old snapshot held. Its setter-heavy
  workload was 18.5% slower by the median of four alternating-order run pairs;
  this is not a TPS or whole-server memory comparison. Reproduction sources,
  requested allocation bytes and RSS are in
  `.analysis/codex-logs/chunk-memory/measurements.json`.
- A writable world keeps its process-exclusive root lease until the final
  `WorldRootLease` owner is dropped. That drop explicitly unlocks the file:
  close-on-exec alone can leave the open-file-description lock alive in a
  concurrently spawned child before exec. Private duplicated descriptors are
  not additional world owners; supported same-process storage handles retain
  the shared `Arc` lease. A duplicate-description regression preserves LSN 73
  through flush/drop/reopen, while the independent-process exclusion and crash
  recovery tests retain their separate contract. This demonstrates a concrete
  lifetime defect, not the exact cause of a historical CI run.
- The region LRU retains validated Anvil headers/location indexes and shared
  open readers, not decompressed NBT for every chunk in a visited region.
  Requested slots reuse bounded decoding; the reader mutex protects seek/read
  only and is released before decompression. Flush replacement invalidates the
  cache as before, and captured disk plans retain their existing reader lifetime.
  Location-table corruption remains eager; payload corruption is reported when
  that slot is requested. Whole-region operations retain aggregate decode limits.
  On 1,811 stored owner chunks, standalone live heap fell from 198,570,425 to
  100,825,394 bytes with identical resident checksum. Four alternating run pairs
  measured median load time 0.593 versus 0.557 seconds. Evidence:
  `.analysis/codex-logs/aquatic-ram/region-cache-comparison.json`.
- Light layers now store repeated packed bytes inline and mixed 2,048-byte
  arrays behind `Arc`, detaching on mutation. Unknown light remains distinct
  from computed zero; fully computed zero persists explicitly and uses the
  vanilla empty-light wire mask. Indirect block palettes use 1–3 bits in RAM
  when possible; disk and wire repack to their minimum four-bit representation
  without changing state values. The 256-to-257-state direct-wire boundary and
  projected palette encoding have independent decoder regressions.
  The same 1,811 stored chunks measured 100,825,603→42,859,987 requested live
  bytes (57.5% less) with the frozen versus compact builds. Both released to
  244 bytes after world drop while RSS retained its high-water allocation.
  The probe's `checksum` is an estimated-byte sum, not a semantic hash.
  Evidence: `.analysis/codex-logs/compact-profile/receipt.json`.
- Explicit `profile` capture runs on a blocking worker. Published chunk
  snapshots are cloned under short shard locks and traversed afterwards;
  private pointer sets deduplicate shared payloads only inside the captured
  publication set. Registry, worker scratch and other owner capacities are
  separate estimates, not another world-budget authority. The system allocator
  is unchanged; `stats_alloc` adds atomic requested-allocation counters, not
  trimming, arena tuning or allocation stack tracing. CPU scopes use thread CPU
  clocks, subtract nested scopes and poll async futures separately, so suspension
  is not billed as CPU. Uninstrumented work, interval-boundary skew and unowned
  heap remain explicit. No persistence ordering, mutation fence or worldgen
  revision change is required by these storage/profile changes.
