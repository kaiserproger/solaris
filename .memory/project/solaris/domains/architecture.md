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
- Overworld terrain shape is owned by `terrain::overworld::DensityRouter`;
  route worldgen topology, climate, river, cave, or stage extraction work
  through ADR 0008.
- The second-generation Overworld router owns continents, mountain provinces,
  rivers, climate, and per-block 3D caves. Caves keep a 32-block solid surface
  shell; generated trees require a stable 3x3 terrain footprint. Generation
  order is terrain/caves/ores, then structures, then decorations. Persisted
  chunks are never silently regenerated, so evaluate the new router only in a
  fresh world (`playable.toml` uses `.analysis/test-world-v2`).
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
  admissions. Full snapshots and breeding scans currently take every lane
  admission under shared topology. Long-running goal preparation holds shared
  topology only; each lane admission is held briefly while its read message is
  enqueued. Owner queues order local reads and exact-snapshot apply rejects
  stale plans.
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
- The final tracker CAS and current-visibility recheck still use the global
  session registry. The movement read path is lock-free with respect to that
  mutex, but the complete publication path is not yet lock-free or regional.
- Empty/all-dead server entity ticks use the published atomic live-session
  count and perform no session-registry or owner-lane read. A transition to zero
  live players pushes generation-fenced hostile-target reconciliation after
  releasing the session lock; do not restore per-tick empty-player ECS scans.
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
