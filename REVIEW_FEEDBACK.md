# Current review feedback

Curated against the working tree on 2026-07-20. This replaces the stale 7.4/10
snapshot scorecard with an actionable owner queue. The prior file described an
older Repomix snapshot, so its file counts, module sizes, ledger numerator, and
product scores are not current evidence.

## Top priorities

### High - world authority and durability

`WorldHandle` is still `Arc<Mutex<WorldStorage>>`
(`crates/mc-net/src/server.rs:220`) even though resident mutations now have
sorted regional locks (`crates/mc-world/src/resident.rs`). Finish the resident
cutover on dominant production paths and prove that disk/LRU coordination is
not a second mutable authority.

World and entity journals exist, but compound gameplay outcomes still need
fault-injected proof across journal append, fsync/rename, checkpoint compaction,
restart replay, and publication. Campfire D1/entity/D2, chained/simultaneous TNT,
and cross-region hopper commits are the highest-value cases.

### High - current entity cutover

Checkpoint `feba79a` replaced the legacy entity/shadow surface with 26.1.2 runtime,
attributes, effects, equipment, living, navigation, projectile, and synced-data
modules. Require one version-fenced authoritative snapshot across ECS owner,
wire metadata, persistence, collision, AI, damage, and equipment. Stale CAS,
death/despawn, restart/replay, and observer publication are the critical failure
branches.

### High - external security qualification

Online-mode RSA/AES/Mojang session verification is implemented; the old claim
that public authentication is absent is resolved. The remaining release gate is
a paid real client against the real endpoint under reconnect/load, including
signed properties and compression transition. Expose
IP-bound verification through `AuthSection` is now implemented; the external
qualification remains open.

### High - parity claims and current evidence

Do not infer broad vanilla parity from supported slices. The dominant open
families are loot context/random sequences, effects/attributes/enchantments,
vehicles/stations, species AI/pathing, sleep/dimension behavior, placement
neighbour/support rules, and deliberate non-vanilla worldgen.

The exact `feba79a` tree passed the full baseline and both parallel and
sequential 94/94 `block_edit` runs. Exact oracle, real-client, crash,
performance, dedicated concurrency, and soak gates remain unrefreshed; enumerate
ignored/skipped rows before readiness language.

## Medium priorities

- `play.rs` is about 13.1k lines, `simulation.rs` 15.9k, `server.rs` 8.4k, and
  `chunk_stream.rs` 8.2k. Continue bounded domain ownership extraction;
  `session.rs` is already reduced to about 1.5k, so the old 8.4k claim is stale.
- Stair/slab orientation, matching slab merge, waterlogging, and common standing
  and wall-torch placement exist with current wire coverage. Stair neighbour
  shapes and complete sturdy-face semantics remain open.
- Worldgen now consumes explicit `ChunkGeometry` end to end and checks extreme
  vertical arithmetic. Its terrain/ore algorithms still deliberately differ
  from Mojang NoiseRouter and placement, so algorithm parity remains open.
- Existing-region atomic replacement remains unsupported on Windows
  (`crates/mc-world/src/storage/dirty_flush.rs:240-251`).
- The Lua host is real and bounded. `mc-extension` still provides only queues
  and DTOs, not a native/WASM plugin host.
- Dynamic live `minecraft:scale` geometry is not wired through physics, reach,
  explosion exposure, wire state, and persistence.

## Resolved stale feedback

- Removed: "online-mode authentication and encryption are absent." Current
  implementation performs RSA challenge, AES-CFB8 transport, signed SHA-1 hash,
  and Mojang `hasJoined` verification.
- Removed: "NBT does not implement strict Modified UTF-8." Current codec handles
  NUL and surrogate-pair encoding and rejects invalid forms.
- Removed: "full RegistryData payloads are not implemented." Current
  configuration path sends sidecar Network-NBT fallback or fails closed.
- Removed: "mc-script has no VM." The server starts a bounded `mlua` plugin host.
- Collapsed: old exact Rust/test/ignored counts, 0/46 ledger claim, and obsolete
  module sizes. None describe the current working tree.
- Narrowed: "all chunk height is Overworld-only" to the remaining worldgen and
  dimension-rule gaps; chunk storage itself now has `ChunkGeometry`.

## Product boundaries retained

The server still deliberately exposes unsupported station families in
`crates/mc-net/src/play/containers.rs:198-219`, uses non-Mojang worldgen rules,
and keeps native extensions as a future boundary. These are product-scope gaps,
not hidden runtime bugs. Public/replacement-ready language remains inappropriate
until current oracle, real-client, crash, performance, and soak evidence exists.

## Curation counts

- Removed six stale narrative sections and ten scorecard subsections from the
  previous review.
- Retained and restructured 14 actionable invariants in the detailed queue:
  six High and eight Medium, organized by subsystem/severity. This executive
  review combines related invariants into four High groups and six Medium
  bullets.
- Collapsed six demonstrably resolved claims into the resolved section.
- The detailed provenance and exact active queue live in
  `.analysis/junior-readonly-wal.md`.
