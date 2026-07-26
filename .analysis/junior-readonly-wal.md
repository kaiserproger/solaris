# Solaris current audit queue

Owner-local, untracked audit artifact. Curated against the working tree on
2026-07-20. This is a queue, not a readiness ledger and not a historical test
log.

## Curation record

- Previous shape: 28,956 lines, 1,399 level-2 sections, 1,065 `WRITE AHEAD`
  entries, 1,066 result/final entries, and 152 front-loaded closed bullets.
- Current shape: five active subsystem queues plus one collapsed resolved
  index. The per-command chronology was removed because later entries repeatedly
  superseded it and left stale blockers searchable as if they were current.
- Evidence rule: an item is removed as fixed only when the current implementation
  contains the required path. Test names, plans, checkpoint prose, and historical
  counts are not sufficient.
- Workspace tests, workspace all-target strict Clippy, fmt, code-health, and
  diff-check were refreshed on the exact `feba79a` tree. Client, oracle,
  performance, crash-injection, dedicated concurrency, and soak gates were not.

Severity: **High** means an authority, durability, security, or release-evidence
invariant. **Medium** means a material product/parity or maintainability gap that
does not presently imply corruption.

## World, storage, and durability

### High - remove the global world handle from resident gameplay paths

The original finding remains materially open: `WorldHandle` is still
`Arc<Mutex<WorldStorage>>` (`crates/mc-net/src/server.rs:220`), and production
paths in `play.rs`, `simulation.rs`, `chunk_stream.rs`, and `server.rs` still
await it. This is no longer the whole story: `ResidentChunkStore` now owns
sorted regional mutation locks and conditional resident block/entity commits
(`crates/mc-world/src/resident.rs:33`, `:316`, `:642`, `:1014`).

Actionable invariant: a resident gameplay mutation must acquire only the sorted
affected regional owners; the global storage mutex may coordinate disk-backed
misses, LRU/cache admission, generation, Anvil IO, and flush, but must not be a
second mutable authority for an already resident chunk. Profile the remaining
production lock sites and move the dominant ones, rather than deleting the type
before its IO role is replaced.

### High - prove cross-authority crash outcomes, not only component WAL replay

World and entity durability use separate journals
(`crates/mc-net/src/play/world_journal.rs:13` and
`crates/mc-net/src/play/persistence.rs:30`). Startup replays both, and save
checkpoints clear them only after corresponding persistence work
(`crates/mc-net/src/server.rs:3422`, `:3520`, `:3939`, `:3973`). The old generic
"durable journal missing" finding is fixed, but the stronger invariant remains:
operations spanning world, entity, inventory, and publication must recover to
exactly one committed outcome across failures during append, rename/fsync,
checkpoint compaction, and publication.

Prioritize fault injection for campfire D1/entity/D2 recovery, simultaneous or
chained TNT world/entity outcomes, cross-region hopper commits, and shutdown
while either journal outcome is unknown. A timeout may only fail the gate; it
must not establish success.

### Medium - existing-region replacement is still unsupported on Windows

`replace_region_file` explicitly rejects replacement of an existing region on
Windows (`crates/mc-world/src/storage/dirty_flush.rs:240-251`). Keep this as a
platform limitation until an atomic, stale-version-fenced install strategy is
implemented and exercised on Windows.

## Entity runtime and simulation

### High - finish the current entity-contract cutover without split authority

The working tree deletes the tracked shadow runtime and introduces new
26.1.2-specific runtime, attributes, effects, equipment, living, navigation,
projectile, and synced-data modules. This makes the old "global entity mutex"
finding stale: production sessions use `RegionalOwnerRuntime` through
`session/entity_owner.rs`, and no `Mutex<RegionalEntityAuthority>` remains.

The active invariant is now stricter: ECS/regional-owner state, wire metadata,
persistence, collision geometry, AI, damage/effects, and equipment must all
derive from the same complete entity snapshot and version fence. Do not leave a
legacy side map or adapter as a second authority after callers move. The cutover
is checkpointed at `feba79a` with a fresh full baseline. Focused stale-CAS,
restart/replay, despawn/death, and observer-publication coverage still needs a
current oracle/client fence before broad sole-authority claims.

### Medium - dynamic entity scale is still not modeled end to end

The current code has exact base dimensions and age-aware livestock geometry,
but searches find no runtime `minecraft:scale` attribute application. The new
entity contract records `spawn_dimensions_scale`
(`crates/mc-data/src/entity_types.rs:109-124`), not a general live SCALE
attribute pipeline. Preserve the prior finding: broad phase, narrow collision,
eye height, reach, projectile hits, explosion exposure, wire state, and
persistence must observe the same scaled geometry.

### Medium - simulation orchestration remains concentrated

Current sizes are approximately `play.rs` 13.1k lines, `simulation.rs` 15.9k,
`server.rs` 8.4k, and `chunk_stream.rs` 8.2k.
`session.rs` has been reduced to about 1.5k and many domains are extracted, so
the old review's exact size table and claim that `session.rs` is 8.4k are fixed.

Continue only bounded ownership extractions from the touched root path. The
correctness fence is that domain modules own policy/state machines while root
files retain orchestration; extraction alone is not evidence of behavior.

## Protocol, authentication, and hostile input

### High - qualify online mode with a real external gate and operator surface

The old "online-mode authentication and encryption are absent" finding is
fixed in implementation. `login.rs:202-259` performs RSA challenge, enables
AES-CFB8, computes the Java signed server hash, and verifies `hasJoined` through
`session_auth.rs`; public bind now permits online mode.

Remaining invariant: prove a paid 26.1.2 client against the real Mojang session
endpoint, including signed profile properties, compression transition,
disconnect/error mapping, reconnect/load behavior, and public-bind startup.
`LoginAccessConfig` supports `prevent_proxy_connections`, but `AuthSection` has
now exposes the TOML field and forwards it into the login authority. The
remaining item is the external paid-client qualification above.

### Medium - retain full-registry custom overlay coverage

The old "full RegistryData payloads are not implemented" finding is fixed.
`configuration.rs:238-297` sends startup-loaded Network-NBT payloads when the
client does not acknowledge the core pack and fails closed when they are
unavailable. Still open: a real Java client explicitly returning empty Known
Packs and custom/mod registry overlays whose entries and dimension geometry do
not match the vanilla sidecar.

## Gameplay and product breadth

### High - do not convert supported-slice evidence into broad vanilla parity

Current implementations cover substantial slices, but the queue should retain
the original boundary verbatim enough to act on: gameplay breadth is larger
than demonstrated parity. High-value open families include exact loot context
(Looting, burning, Fortune/Silk Touch, nested/weighted/conditional tables and
random sequences), complete effects/attributes/enchantments, vehicles and
stations, species AI/pathing, sleep/dimension rules, and crash-safe compound
actions. Each parity claim still needs an oracle plus a real-path gate.

The server explicitly rejects or no-ops brewing stand, anvils, smithing table,
grindstone, loom, cartography table, composter, cauldrons, lectern, fletching
table, beacon, and crafter station use
(`crates/mc-net/src/play/containers.rs:198-219`). Bucket-cauldron interactions
exist elsewhere, so treat that entry as an incomplete station surface rather
than "all cauldrons absent."

### Medium - finish placement state and support-face semantics

Stair facing/half, slab top/bottom, matching-slab merge, waterlogging, standing
torches, and common wall-torch placement now exist in `block_placement.rs` with
current wire coverage. The implementation still uses an intentionally small
full-cube fallback instead of complete vanilla `isFaceSturdy`. Retain stair
neighbour-shape updates and irregular support faces as the concrete open tasks.

### Medium - worldgen remains a deliberate non-vanilla product boundary

The vertical-geometry part of the old finding is fixed: terrain, biome, ore,
and structure generation consume explicit `ChunkGeometry`; extreme valid
geometries use checked/wide arithmetic, and no production `mc-worldgen` path
uses global Overworld `MIN_Y/MAX_Y`. The default Overworld path has a
deterministic serialized-NBT fingerprint.

The remaining boundary is algorithmic. Ore placement and the wider terrain
pipeline explicitly are not Mojang's NoiseRouter/placement algorithms. Keep
custom-dimension support and vanilla worldgen parity as separate goals, and do
not describe the Solaris fingerprint as vanilla byte parity.

### Medium - native extension hosting is still only a boundary

The old claim that `mc-script` has no VM is fixed: `mc-script/src/lua.rs` loads
disk plugins into bounded `mlua` runtimes and `mc-server/src/main.rs:706-726`
starts and joins the host. `mc-extension`, however, still explicitly says it
does not run plugins and only exposes DTOs and bounded queues
(`crates/mc-extension/src/lib.rs:1-8`). Keep this only if a native/WASM extension
host remains an intended product requirement.

## Evidence and release claims

### High - refresh end-to-end gates after the current dirty entity/world slice

The exact tree committed as `feba79a` passed workspace tests, workspace
all-target strict Clippy, fmt, code-health `0 fail / KEEP`, diff-check, and both
parallel and sequential 94/94 `block_edit` runs. Real-client, vanilla oracle,
performance, crash-injection, dedicated concurrency, and soak evidence remains
unrefreshed. Report those skipped prerequisites explicitly; the fresh baseline
does not replace them.

### Medium - clean local artifact visibility before any staging operation

The worktree still exposes generated Fabric `run`/`run-mcp`, region files,
Repomix snapshots, Python bytecode, and other local artifacts as untracked.
These are not source findings, but they materially raise accidental staging and
Mojang-byte risk. Keep `.analysis/` and `REVIEW_FEEDBACK.md` uncommitted as
requested.

## Collapsed resolved index

Removed from the active queue after current implementation inspection:

- Strict Java Modified UTF-8, including NUL and supplementary characters, is
  implemented in `crates/mc-nbt/src/lib.rs:16-17` and `:222`.
- Online-mode RSA/AES/session verification is implemented in `login.rs` and
  `session_auth.rs`; only the external/operator qualification above remains.
- Full RegistryData fallback exists in `configuration.rs`; only custom-overlay
  and real-client coverage remains.
- The serverbound protocol allocation audit now bounds every decoded variable
  collection before allocation, applies a packet-wide Container Click budget,
  and preflights fallible encoders before writing output.
- A built-in bounded Lua VM host exists; only native/WASM extension hosting is
  still absent.
- The global entity mutex and legacy shadow runtime are superseded by the
  regional owner runtime/current cutover; split-authority verification remains.
- Dynamic chunk storage geometry exists; worldgen and several dimension rules
  remain Overworld-specific.
- Historical fixed gameplay, authority, persistence, relight, queueing,
  backpressure, and module-extraction entries were collapsed rather than kept
  as 152 separate closed bullets. Their old test counts are intentionally not
  promoted to current readiness evidence.
