# Play And Session Hard Split Wave 9

**Goal:** move incremental relighting and session chunk-view authority out of the remaining coordinators without changing world-lock order, light publication, prepared-cache fences, or recipient ordering.

## Task 1: Incremental Lighting

**Files:** `crates/mc-net/src/play.rs`, `crates/mc-net/src/play/simulation.rs`, new `crates/mc-net/src/play/lighting.rs`

- [x] Move incremental source capture/currentness, light computation, full fallback collection, outbound light DTO construction, cache seeding, and baked-light persistence into `lighting.rs`.
- [x] Keep async world lock release/reacquire, stale-source fallback orchestration, mutation commits, prepared invalidation, recipient selection, command publication, and packet writes in their current owners.
- [x] Preserve final-state encoding, neighbourhood capture, conditional publication, cache persistence, and writer-release behavior; add no async/await, direct send, lock acquisition, wildcard import, sleep, or polling to the child.

## Task 2: Chunk View Authority

**Files:** `crates/mc-net/src/play/session.rs`, new `crates/mc-net/src/play/session/chunk_view_authority.rs`

- [x] Move block-entity dispatch planning, view replacement, loaded/unloaded revision fences, loaded recipient planning, and sorted ticket/load snapshots into `chunk_view_authority.rs`.
- [x] Keep registry fields/guards, registration teardown, shared ticket/index helpers, prepared-cache ownership, visibility implementation, persistence, and actual delivery in their current owners.
- [x] Preserve literal session -> prepared-cache lock order, revision fencing, subscriber/frontier cleanup, visibility refresh order, and ordered recipient construction; add no direct send, async/await, new lock, wildcard import, sleep, or polling.

## Task 3: Fence And Validate

- [x] Add multiple ownership anchors and explicit-import checks for both domains; update ADR 0006 and memory.
- [x] Run focused lighting/chunk-view tests, full `mc-net`, strict Clippy, fmt, code-health, and independent review.
