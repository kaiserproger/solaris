# Architecture Route

Read `docs/decisions/README.md`, then only the owning ADR. Use
`docs/CORE_INTERNALS_FOR_OWNER.md` when a deeper current-code map is required.

Current runtime facts:

- Solaris is a modular monolith, but `mc-net` orchestration remains only partly
  extracted. `play.rs`, `simulation.rs`, and related roots still contain real
  behavior; ADR 0006 records the staged boundary rather than completed purity.
- `EntityStore` is the production ECS authority. Regional ownership and
  transfers are staged; gameplay-significant side maps and global coordination
  still exist. ADR 0004/0005 are authoritative.
- Migrated world mutation and persistence paths use owned snapshots,
  prepare/commit/finalize, and explicit publication fences. Legacy/staged paths
  remain; verify the exact caller against ADR 0004. Do not hold locks across
  socket progress or guessed waits.
- Runtime capacity is derived from the machine. Autoscaling uses measurements
  and bounded admissions; operator worker-percentage knobs are forbidden.
- SIMD and fast paths require a measured bottleneck, a scalar/correctness
  fence, and evidence for only the measured workload.
- Overworld terrain shape is owned by `terrain::overworld::OverworldRouter`;
  route worldgen topology, climate, river, cave, or stage extraction work
  through ADR 0008.
- Worldgen revision 4 owns warped continents, erosion, ridges, rivers, climate,
  and narrow neighbour-connected tunnel caves. Caves keep a 32-block solid
  surface shell; trees require their exact planned support and a stable 5x5
  footprint. Generation order is terrain/caves/ores, structures, decorations.
  `solaris/world.json` fences revision, seed, mode, and geometry before Anvil
  open. Unversioned Anvil worlds open as vanilla imports without Solaris
  fallback generation. Evaluate revision 4 in `.analysis/test-world-v4`.
- Regional entity owners commit authoritative kinematics. Movement wire plans
  are prepared outside the global session lock and use tracker CAS plus a
  visibility recheck at publication. Visibility indexes and outbound session
  queues remain centralized, so do not describe publication as lock-free or
  fully regional; ADR 0005 records the remaining boundary.
- Warm ordinary entity reads validate monotonic versions on only the owner
  lanes they touch, so an unrelated regional writer no longer forces them
  through the coordinator. Versioned referenced-goal reads carry the same
  per-lane version vector; their CAS locks selected lanes in stable lane-id
  order while unrelated direct lane operations remain independent. The
  exclusive topology gate remains for actor fallbacks that change ownership or
  global indexes and for reconfiguration; ADR 0005 owns the distinction.
- Actor fallbacks for lane-local goal, animal, item, velocity, damage, and
  effect mutations use shared topology plus successfully resolved touched-lane
  admissions in stable lane-id order. Malformed ownership routes and actor
  operations that can change region ownership or global indexes remain
  exclusive.
- Cold point and ID-filtered actor reads use shared topology plus touched-lane
  admissions. Full snapshots still take every lane admission under shared
  topology. Long-running goal preparation holds shared topology only; each lane
  admission is held briefly while its read message is enqueued. Owner queues
  order local reads and exact-snapshot apply rejects stale plans.
- Goal selection publishes the exact current simulation-active entity IDs
  through `ArcSwap`; breeding reads that immutable set and requests only those
  IDs from regional owners. Unobserved regions no longer join the tick or age
  animals, and the removed all-lane breeding command must not be restored.
- Item despawn uses a simulation-tick deadline index. Ordinary physics turns do
  not scan the full entity store; only due item ids reach their owner lanes.
  Restored items retain the deadline derived from their persisted `spawn_tick`;
  cancellation and deduplication keep stale ids outside the live sweep budget.
- Prepared-goal apply holds shared topology and only the admissions named by
  its active inputs, follow targets, lease/batch regions, and requested
  post-apply kinematics IDs. Multi-lane apply remains atomic across those
  participants without taking the exclusive topology gate.
- Physics owner CAS returns its committed kinematics batch directly. The
  network adapter must not restore the removed immediate owner reread; it keeps
  one current-state read at the publication boundary.
- Movement and pickup planning use copied tracker and player-position inputs;
  neither ECS access nor the global session registry may be held across that
  pure computation.
- Movement recipient discovery reads an `ArcSwap` session index and per-session
  immutable visibility publications. Connect/disconnect rebuild the index;
  visibility writers publish only after reserving ordered spawn/despawn. Do not
  restore per-tick session/visibility traversal under `SessionRegistry.inner`.
- Wire tracker state is split across 64 shards. Final movement publication CASes
  those shards, reloads the `ArcSwap` recipient index and per-session visibility,
  and records metrics atomically without entering `SessionRegistry.inner`.
  Unregister closes that session's ordered queue before publishing removal, so
  a movement already past recipient validation cannot dispatch after removal.
  Visibility mutation and each session's ordered outbound queue still use locks,
  so this is not fully lock-free or fully regional publication.
- Empty/all-dead server entity ticks use the published atomic live-session
  count and perform no session-registry or owner-lane read. A transition to zero
  live players pushes generation-fenced hostile-target reconciliation after
  releasing the session lock; do not restore per-tick empty-player ECS scans.
- Hostile attack planning copies live target poses and immutable visibility
  sets, then releases `SessionRegistry.inner`. Creeper fuse CAS, arrow spawn,
  and batched melee-attacker validation execute on regional owners. Final melee
  admission reads the per-session `ArcSwap` combat-target and visibility
  snapshots and reserves output through the ordered session queue under one
  shared odd/even publication epoch. Target/visibility mutation opens the epoch
  before changing state; admission rejects an odd or changed epoch. Disconnect
  publishes non-targetable before queue close. Final melee does not reacquire
  the global registry. Never put an owner request or melee publication back
  under the session lock. This is an ordered regional boundary with lock-free
  reads, not global lock-free state.
- Uncontrolled heavy host load invalidates performance attribution. Record the
  build, workload, host contention, p95/p99, and maximum; repeat the same gate
  on a clean host. A contaminated run may retain functional evidence only.

Routing:

- Protocol ids/layouts: ADR 0002 plus local `wire-probe`/`javap` evidence.
- Authority or persistence ordering: ADR 0004.
- Regional ownership/transfers/ECS interaction: ADR 0005.
- `mc-net` extraction and publication adapters: ADR 0006.
- Performance claims: the exact milestone and documented metric definition;
  narrow benchmarks never prove the full scaling envelope.

Desired migration is not runtime truth. Verify callers, mutation authority,
tests, and publication before claiming a boundary has moved.
