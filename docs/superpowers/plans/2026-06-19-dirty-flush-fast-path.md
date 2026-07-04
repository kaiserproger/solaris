# Dirty Flush Snapshot Fast-Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bounded `mc-world` dirty-flush fast path that clears unchanged nonzero-generation chunks without re-encoding, while preserving the existing correctness guards for payload mismatch, generation mismatch, legacy zero-generation chunks, and post-plan untracked mutations.

**Architecture:** Keep the slice inside `crates/mc-world/src/storage.rs`. Extend dirty-flush planning/commit metadata with snapshot identity and a planned payload digest, then let `commit_dirty_flush()` use a fast-clean eligibility check before falling back to the current payload-compare path. Existing storage tests remain the regression net for correctness-sensitive cases; new helper-level tests drive the internal fast-path logic with honest RED/compile-fail cycles.

**Tech Stack:** Rust 1.94 workspace, `mc-world` storage/anvil codec path, built-in unit tests under `crates/mc-world/src/storage.rs`, workspace verification via `cargo fmt`, `cargo test`, `cargo clippy`, and `xtask code-health`.

---

## File Structure

- Modify: `crates/mc-world/src/storage.rs`
  Responsibility: dirty-flush plan/commit metadata, internal snapshot/digest helpers, commit fast-path eligibility, and focused storage tests.
- Reuse: `crates/mc-world/src/chunk.rs`
  Responsibility: existing `Chunk`/`dirty_generation` semantics only; do not edit in this slice.
- Reuse: `crates/mc-test-harness/tests/persistence_inventory.rs`
  Responsibility: existing persistence proof only; do not edit unless the storage unit slice proves insufficient.

No docs or protocol files should change in this implementation slice unless a proved invariant forces a wording correction.

### Task 1: Add the Failing Storage Tests

**Files:**
- Modify: `crates/mc-world/src/storage.rs` inside `#[cfg(test)] mod tests`
- Test: `crates/mc-world/src/storage.rs`

- [ ] **Step 1: Write the failing helper-level REDs for snapshot metadata and fast-path eligibility**

```rust
    #[test]
    fn dirty_flush_plan_tracks_snapshot_token_and_payload_digest() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned = &plan.regions[0].dirty_payloads[0];
        let snapshot = world.cache.get(&cpos).unwrap();

        assert_eq!(planned.snapshot_token, chunk_snapshot_token(snapshot));
        assert_eq!(
            planned.payload_digest,
            payload_digest(&planned.payload.uncompressed_nbt)
        );
    }

    #[test]
    fn dirty_flush_write_carries_snapshot_fast_path_metadata_into_commit() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let commit = plan.write().unwrap();
        let committed = &commit.regions[0].chunks[0];

        assert_ne!(committed.snapshot_token, 0);
        assert_eq!(
            committed.payload_digest,
            payload_digest(&committed.uncompressed_nbt)
        );
    }

    #[test]
    fn dirty_flush_fast_path_requires_matching_snapshot_generation_and_digest() {
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let snapshot = Arc::new(chunk);
        let committed = b"payload".to_vec();
        let digest = payload_digest(&committed);

        assert!(can_fast_clean_chunk(
            &snapshot,
            snapshot.dirty_generation,
            chunk_snapshot_token(&snapshot),
            digest,
            &committed,
        ));

        assert!(!can_fast_clean_chunk(
            &snapshot,
            snapshot.dirty_generation,
            chunk_snapshot_token(&snapshot) ^ 1,
            digest,
            &committed,
        ));
        assert!(!can_fast_clean_chunk(
            &snapshot,
            snapshot.dirty_generation,
            chunk_snapshot_token(&snapshot),
            digest ^ 1,
            &committed,
        ));
        assert!(!can_fast_clean_chunk(
            &snapshot,
            snapshot.dirty_generation + 1,
            chunk_snapshot_token(&snapshot),
            digest,
            &committed,
        ));
    }
```

- [ ] **Step 2: Run the new REDs and verify they fail for the right reason**

Run:

```bash
cargo test -p mc-world dirty_flush_plan_tracks_snapshot_token_and_payload_digest
cargo test -p mc-world dirty_flush_write_carries_snapshot_fast_path_metadata_into_commit
cargo test -p mc-world dirty_flush_fast_path_requires_matching_snapshot_generation_and_digest
```

Expected: compile failure or test failure mentioning missing `snapshot_token`, `payload_digest`, `chunk_snapshot_token`, or `can_fast_clean_chunk`.

- [ ] **Step 3: Commit the test-only RED state if working in a commit-by-commit execution flow**

```bash
git add crates/mc-world/src/storage.rs
git commit -m "test: add dirty flush snapshot fast-path guards"
```

### Task 2: Add Snapshot and Digest Plumbing

**Files:**
- Modify: `crates/mc-world/src/storage.rs`
- Test: `crates/mc-world/src/storage.rs`

- [ ] **Step 1: Add the internal token and digest helpers near the dirty-flush types**

```rust
type ChunkSnapshotToken = usize;

fn chunk_snapshot_token(chunk: &ChunkSnapshot) -> ChunkSnapshotToken {
    Arc::as_ptr(chunk) as ChunkSnapshotToken
}

fn payload_digest(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn can_fast_clean_chunk(
    chunk: &ChunkSnapshot,
    planned_generation: u64,
    planned_snapshot: ChunkSnapshotToken,
    planned_payload_digest: u64,
    committed_bytes: &[u8],
) -> bool {
    planned_generation != 0
        && chunk.dirty_generation == planned_generation
        && chunk_snapshot_token(chunk) == planned_snapshot
        && payload_digest(committed_bytes) == planned_payload_digest
}
```

- [ ] **Step 2: Extend the dirty-flush metadata structs with the new fields**

```rust
struct PlannedChunkPayload {
    pos: ChunkPos,
    dirty_generation: u64,
    snapshot_token: ChunkSnapshotToken,
    payload_digest: u64,
    payload: ChunkPayload,
}

struct CommittedChunkPayload {
    pos: ChunkPos,
    dirty_generation: u64,
    snapshot_token: ChunkSnapshotToken,
    payload_digest: u64,
    uncompressed_nbt: Vec<u8>,
}
```

- [ ] **Step 3: Populate the metadata in `plan_dirty_flush()`**

```rust
                let payload_digest = payload_digest(&payload.uncompressed_nbt);
                dirty_payloads.push(PlannedChunkPayload {
                    pos: cpos,
                    dirty_generation: chunk.dirty_generation,
                    snapshot_token: chunk_snapshot_token(chunk),
                    payload_digest,
                    payload,
                });
```

- [ ] **Step 4: Carry the metadata through `DirtyFlushPlan::write()` into the commit**

```rust
                    .map(|planned| CommittedChunkPayload {
                        pos: planned.pos,
                        dirty_generation: planned.dirty_generation,
                        snapshot_token: planned.snapshot_token,
                        payload_digest: planned.payload_digest,
                        uncompressed_nbt: planned.payload.uncompressed_nbt,
                    })
```

- [ ] **Step 5: Run the focused tests and verify they now pass**

Run:

```bash
cargo test -p mc-world dirty_flush_plan_tracks_snapshot_token_and_payload_digest
cargo test -p mc-world dirty_flush_write_carries_snapshot_fast_path_metadata_into_commit
cargo test -p mc-world dirty_flush_fast_path_requires_matching_snapshot_generation_and_digest
```

Expected: PASS.

- [ ] **Step 6: Commit the metadata plumbing**

```bash
git add crates/mc-world/src/storage.rs
git commit -m "fix: carry dirty flush snapshot metadata"
```

### Task 3: Wire the Commit Fast Path and Preserve Fallback Correctness

**Files:**
- Modify: `crates/mc-world/src/storage.rs`
- Test: `crates/mc-world/src/storage.rs`

- [ ] **Step 1: Add commit-path regression coverage for the unchanged and derived-only cases**

```rust
    #[test]
    fn dirty_flush_commit_cleans_unchanged_nonzero_generation_snapshot_fast_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let commit = world.plan_dirty_flush().unwrap().write().unwrap();

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
    }

    #[test]
    fn dirty_flush_commit_falls_back_to_payload_compare_for_derived_only_snapshot_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        world
            .get_chunk_mut(cpos)
            .unwrap()
            .unwrap()
            .highest_opaque
            .set(0, 0, 0);

        let commit = plan.write().unwrap();

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
    }
```

- [ ] **Step 2: Run the new commit-path regressions plus the existing guards**

Run:

```bash
cargo test -p mc-world dirty_flush_commit_cleans_unchanged_nonzero_generation_snapshot_fast_path
cargo test -p mc-world dirty_flush_commit_falls_back_to_payload_compare_for_derived_only_snapshot_change
cargo test -p mc-world dirty_flush_commit_keeps_matching_nonzero_generation_dirty_on_payload_mismatch
cargo test -p mc-world dirty_flush_commit_keeps_post_plan_unmarked_chunk_mutation_dirty
cargo test -p mc-world dirty_flush_commit_keeps_nonzero_generation_mismatch_dirty_even_if_payload_matches
cargo test -p mc-world dirty_flush_commit_uses_payload_fallback_for_legacy_zero_generation_dirty
```

Expected: these regression tests may already pass under the current payload-compare implementation. That is acceptable. The optimization RED is still driven by the helper-level tests from Task 1; these commit-path tests exist to keep the storage contract honest while the fast path is wired in, and the existing guards should remain green once the implementation lands.

- [ ] **Step 3: Update `commit_dirty_flush()` to try the fast path before re-encoding**

```rust
    pub fn commit_dirty_flush(&mut self, commit: DirtyFlushCommit) -> Result<usize, WorldError> {
        let mut clean = Vec::new();
        for region in &commit.regions {
            for planned in &region.chunks {
                let Some(chunk) = self.cache.get(&planned.pos) else {
                    continue;
                };
                if !chunk.dirty {
                    continue;
                }
                if planned.dirty_generation != 0
                    && chunk.dirty_generation != planned.dirty_generation
                {
                    continue;
                }
                if can_fast_clean_chunk(
                    chunk,
                    planned.dirty_generation,
                    planned.snapshot_token,
                    planned.payload_digest,
                    &planned.uncompressed_nbt,
                ) {
                    clean.push(planned.pos);
                    continue;
                }
                let current = chunk_to_payload_with_items(
                    chunk,
                    &self.registry,
                    self.item_registry.as_deref(),
                    0,
                )?;
                if current.uncompressed_nbt == planned.uncompressed_nbt {
                    clean.push(planned.pos);
                }
            }
        }

        for cpos in &clean {
            if let Some(chunk) = self.cache.get_mut(cpos) {
                let chunk = Arc::make_mut(chunk);
                chunk.dirty = false;
            }
        }
        for region in commit.regions {
            self.regions.remove(&region.region);
            self.region_lru.retain(|&k| k != region.region);
        }

        Ok(clean.len())
    }
```

- [ ] **Step 4: Run the targeted dirty-flush test group**

Run:

```bash
cargo test -p mc-world dirty_flush_commit_
```

Expected: all `dirty_flush_commit_*` tests PASS.

- [ ] **Step 5: Commit the fast-path slice**

```bash
git add crates/mc-world/src/storage.rs
git commit -m "fix: add dirty flush snapshot fast-path"
```

### Task 4: Run Final Verification for the Slice

**Files:**
- Modify: none expected
- Test: workspace commands only

- [ ] **Step 1: Run the focused module verification**

Run:

```bash
cargo test -p mc-world dirty_flush_
```

Expected: PASS with all dirty-flush storage tests green.

- [ ] **Step 2: Run the repository formatting and architecture gate**

Run:

```bash
cargo fmt --all -- --check
cargo run -p xtask -- code-health
```

Expected: `rustfmt` clean, `verdict: KEEP`.

- [ ] **Step 3: Run the final workspace baseline**

Run:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 4: Check the final diff is limited to the storage slice**

Run:

```bash
git diff -- crates/mc-world/src/storage.rs docs/superpowers/specs/2026-06-19-dirty-flush-fast-path-design.md docs/superpowers/plans/2026-06-19-dirty-flush-fast-path.md
```

Expected: only the planned storage fast-path changes plus the spec/plan files.

- [ ] **Step 5: Record the evidence honestly in the final handoff**

```text
Label: stabilization
Focused tests: dirty-flush storage guards + new snapshot fast-path tests
Vanilla oracle: not run
Client/manual: not run
Performance/concurrency: internal commit-path optimization only; no new green budget claim
Known gaps: no broader Chunk mutation accounting, no SessionRegistry lock trimming, no real-client or oracle evidence
```
