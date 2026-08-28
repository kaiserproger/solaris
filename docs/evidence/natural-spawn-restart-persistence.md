# Natural-spawn restart and identity persistence

Status: automated Phase-4 acceptance evidence for natural-spawn restart identity and retained entity state. This does **not** replace the separate 20-minute real-client natural-spawn visibility gate.

## Ownership boundary

Periodic natural-spawn candidates are planned in `mc-entity::natural_spawn_26_1_2`, receive deterministic attempt identities, commit through the regional entity owner, and are published/indexed by `mc-net::SessionRegistry`. Entity saves cross the simulation save barrier into `PersistedEntityCheckpoint`; `entities.dat` is the disk representation of that checkpoint.

Restart therefore has two independent correctness requirements:

1. checkpoint restore must preserve the authoritative snapshots and must not allow the same deterministic natural-spawn attempt to create duplicate identities;
2. the `entities.dat` codec must preserve the identity and retained state carried by a natural-spawn snapshot.

## Checkpoint/restart identity regression

Test:

```text
play::session::herd_spawn_authority_tests::periodic_natural_spawn_restart_preserves_entities_and_rejects_replayed_identities
```

The test:

- creates one periodic friendly cow and one periodic hostile zombie from the production planner/commit path at the same deterministic attempt tick;
- advances the lifecycle epoch and captures the production entity save barrier checkpoint;
- restores that checkpoint into a new empty `SessionRegistry` and asserts the complete restored snapshots equal the pre-restart snapshots;
- verifies the restored simulation/lifecycle clock;
- moves the restored entities away from the template positions so collision rejection cannot hide an identity bug;
- registers the same templates and replays the same attempt tick;
- requires zero newly committed friendly/hostile entities;
- requires one duplicate/stale rejection in each category;
- requires the post-replay entity count and UUID set to remain exactly the restored set.

Focused result on 2026-08-18:

```text
cargo test -p mc-net periodic_natural_spawn_restart_preserves_entities_and_rejects_replayed_identities -- --nocapture
1 passed / 0 failed
```

## Disk persistence regression

Test:

```text
play::persistence::tests::natural_spawn_identity_and_retained_state_round_trip_entities_dat
```

The fixture uses a repo-owned natural-spawn deterministic herd UUID and a temporally valid persisted natural cow snapshot. It writes the production `entities.dat` format and loads it through the production entity loader. The loaded record must preserve:

- entity id and deterministic UUID;
- exact entity type/name;
- position and velocity;
- animal breeding state;
- retained `spawn_tick`;
- retained `fall_distance`;
- checkpoint lifecycle clock.

Focused result on 2026-08-18:

```text
cargo test -p mc-net natural_spawn_identity_and_retained_state_round_trip_entities_dat -- --nocapture
1 passed / 0 failed
```

The initial synthetic fixture intentionally failed closed because its record age did not match `lifecycle_clock - spawn_tick`; it was corrected to use `PersistedEntityRecord::from_snapshot_at_lifecycle_clock`, the same temporal derivation used by production save snapshots. No persistence validation was weakened.

## Existing complementary fences

The new tests sit on top of existing coverage for:

- periodic spawn category caps, terrain/fluid/collision admission and day/night/darkness;
- population refill after movement/despawn;
- legacy/restored herd UUID deduplication;
- persisted entity checkpoint temporal validation;
- retained item/villager/entity state codec coverage;
- generated-village disk restart preserving the same villager identity.

## Acceptance disposition

The Phase-4 natural-spawn acceptance row **“Restart does not duplicate deterministic identities or lose retained entities”** is closed by the paired checkpoint/replay and disk-codec evidence above.

Still open and intentionally separate:

- the manifest-backed **fresh 20-minute no-operator survival run** in which friendly mobs become visibly observable and hostiles appear naturally at night;
- the release-candidate restart/reconnect observations/screenshots/logs attached to that long graphical gate.

Those visual/soak requirements must not be inferred from these deterministic automated regressions.
