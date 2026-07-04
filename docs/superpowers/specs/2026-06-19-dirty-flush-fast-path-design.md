# Dirty Flush Snapshot Fast-Path Design

Date: 2026-06-19
Quality label: `stabilization`
Target rows: `O2` primarily, with secondary impact on `O1`

## Problem

`WorldStorage::commit_dirty_flush()` currently re-encodes every matching-generation
dirty chunk before it can clear the `dirty` flag. This is correct but expensive on
the normal "planned and untouched" path. The current guard also cannot trust
`dirty_generation` alone because `get_chunk_mut()` exposes a raw `&mut Chunk`, and
some payload-affecting edits can happen without bumping the token.

The result is a correctness-first implementation that keeps the tree safe but still
does extra work under `SaveAllFlush` and dirty-pressure commit paths. The
validation ledger already treats this as a correctness guard, not as the intended
fast path.

## Goal

Add a bounded fast path inside `mc-world::storage` that clears a dirty chunk after
flush commit without re-encoding when the chunk is provably unchanged since
planning.

The fast path must never weaken the existing correctness invariants:

- post-plan payload mutation without token bump must stay dirty
- nonzero generation mismatch must stay dirty
- legacy `dirty_generation == 0` behavior must continue to use payload comparison
- region staleness checks and disk-write behavior must remain unchanged

## Non-Goals

- no public API redesign of `Chunk` or `get_chunk_mut()`
- no lock-ownership rewrite in `mc-net`
- no semantic change to region planning, file replacement, or cache eviction
- no readiness/performance claim beyond "less unnecessary re-encode work on the
  unchanged path"

## Recommended Approach

Store snapshot identity in the dirty-flush plan alongside the existing
`dirty_generation` and encoded payload.

At planning time, each `PlannedChunkPayload` will capture:

- `pos`
- `dirty_generation`
- `planned_snapshot`: identity token derived from the planned `Arc<Chunk>`
- encoded `payload`

At write/commit hand-off, each `CommittedChunkPayload` will retain:

- `pos`
- `dirty_generation`
- `planned_snapshot`
- `uncompressed_nbt`

At commit time, `WorldStorage::commit_dirty_flush()` will use this order:

1. Skip non-dirty or missing chunks.
2. If `dirty_generation != 0` and current generation differs from the planned
   generation, keep dirty.
3. If `dirty_generation != 0` and the current cached `Arc<Chunk>` still has the
   same snapshot identity as the planned snapshot, clear dirty immediately
   without re-encoding.
4. Otherwise, fall back to the current payload-compare path and clear dirty only
   when the current encoded NBT exactly matches the committed payload.

This keeps the unchanged path fast while preserving the existing payload fallback
for untracked mutations and legacy zero-generation chunks.

## Snapshot Identity

The snapshot token is internal to `storage.rs`. It should be derived from the
resident `Arc<Chunk>` identity rather than from serialized content. The simplest
bounded form is a raw pointer token derived from `Arc::as_ptr()` and stored as a
copyable integer-sized value.

This token is only used as an equality check within one process between
`plan_dirty_flush()` and `commit_dirty_flush()`. It is not persisted, logged as a
semantic value, or exposed outside `mc-world`.

## Why This Boundary

This slice stays inside `mc-world/src/storage.rs`, where the dirty-flush contract
already lives. It does not require:

- making `Chunk` fields private
- changing every mutation site to mandatory token bumps
- introducing new synchronization
- coupling `mc-net` to `mc-world` internals

If later work wants broader mutation accounting, it can build on top of this
without invalidating the fast path.

## Test Plan

Add focused storage tests around the new frontier:

1. `dirty_flush_commit_cleans_unchanged_nonzero_generation_snapshot_fast_path`
   proves that a normal dirty chunk with unchanged generation and unchanged
   snapshot clears cleanly.
2. Keep
   `dirty_flush_commit_keeps_post_plan_unmarked_chunk_mutation_dirty`
   unchanged as the guard for payload mutation without token bump.
3. Add a derived-only-mutation guard where the snapshot changes after planning
   but serialized payload still matches, proving commit falls back to payload
   compare instead of blindly trusting generation.
4. Keep the existing guards for:
   - payload mismatch on matching nonzero generation
   - nonzero generation mismatch with matching payload
   - legacy zero-generation payload fallback

Focused verification after implementation:

- targeted `mc-world` storage tests for the new and existing dirty-flush cases
- `cargo test -p mc-world storage::tests::...` or equivalent filtered runs
- workspace baseline after the final slice

## Risks

- Pointer-identity handling must stay internal and simple; avoid threading it
  through unrelated modules.
- A derived-only mutation may replace the cached `Arc<Chunk>` while leaving the
  serialized payload unchanged. This is acceptable: the design intentionally
  falls back to payload comparison in that case.
- If a test cannot produce a reliable RED for the fast-path distinction, do not
  fake a performance claim. Keep the correctness guards and stop at the proven
  boundary.

## Acceptance

This slice is successful when:

- unchanged dirty chunks can clear without forced re-encode on the proven path
- all current correctness guards remain green
- no public API surface changes
- the final report still labels the work `stabilization`
- no vanilla-oracle, real-client, or release-readiness claim is added

## Open Frontier After This Slice

If this lands cleanly, the next bounded follow-up is one of:

- broader mutation accounting for `Chunk` write paths
- `chunk_stream` fail-closed handling for `pressure_abandoned`
- `SessionRegistry` hot-path lock trimming around prepared-cache/load bookkeeping

Those are intentionally out of scope for this design.
