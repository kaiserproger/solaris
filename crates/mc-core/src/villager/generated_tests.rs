use super::{VillagerKind, VillagerProfession, conversion_tests::state, generated};
use crate::boundary::{ChangeSet, ChunkPos};
use std::sync::Arc;
struct MarkerGenerator(mc_worldgen::TerrainGenerator, &'static str, bool);
impl mc_world::ChunkGenerator for MarkerGenerator {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::ChunkGenerator::generate(&self.0, pos);
        chunk.set_generated_entities(&[mc_world::GeneratedEntityMarker {
            claim: if self.1 == "minecraft:item_frame" {
                "minecraft:end_city/ship@0:80:0#elytra"
            } else if self.1 == "minecraft:chest_minecart" {
                "mineshaft:0:32:0#0"
            } else {
                "village:0:0:resident:1"
            }
            .into(),
            entity_type: self.1.into(),
            position: [0.5, 80., 0.5],
            persistent: self.2,
            villager_kind: "minecraft:savanna".into(),
            profession: "minecraft:farmer".into(),
            level: 2,
            age: -12000,
            yaw: if self.1 == "minecraft:item_frame" {
                180.0
            } else {
                93.0
            },
            pitch: 7.,
        }]);
        chunk
    }
}
fn fixture() -> crate::tick::TickState {
    fixture_for("minecraft:villager", false)
}

fn fixture_for(entity_type: &'static str, persistent: bool) -> crate::tick::TickState {
    let mut state = state();
    state.world = crate::world::ChunkWorld::new(
        42,
        Arc::new(MarkerGenerator(
            mc_worldgen::TerrainGenerator::new(42, Arc::clone(&state.registry.blocks)),
            entity_type,
            persistent,
        )),
        &state.registry,
        mc_world::BlockStateId(state.registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let chunk = ChunkPos { x: 0, z: 0 };
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    if entity_type == "minecraft:item_frame" {
        state.world.set_block(0, 80, 0, state.registry.states.air);
        state.world.set_block(0, 80, 1, state.registry.states.stone);
    }
    state
}

#[test]
fn preloaded_village_resident_spawns_once_and_claim_survives_death() {
    let mut state = fixture();
    let chunk = ChunkPos { x: 0, z: 0 };
    let mut changes = ChangeSet::default();
    generated::load(&mut state, &[], &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    let id = changes.entities_spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    let uuid = state.entities.row(slot).uuid;
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(slot));
    let villager = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(villager.data.kind, VillagerKind::Savanna);
    assert_eq!(villager.data.profession, VillagerProfession::Farmer);
    assert_eq!(villager.data.level, 2);
    assert_eq!(villager.age, -12000);
    assert!(
        !villager.offers.is_empty(),
        "template babies retain merchant offers"
    );
    assert!(
        !super::trades::can_trade(villager),
        "babies cannot open a merchant"
    );
    assert_eq!(
        villager.activity,
        super::brain::schedule(villager.age, villager.data.profession, state.day_time)
    );
    assert_eq!(state.entities.rot(slot)[..2], [93., 7.]);
    assert_eq!(state.new_generated_entity_claims.len(), 1);
    state.remove_entity(slot);
    changes.entities_spawned.clear();
    generated::load(&mut state, &[chunk], &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(state.generated_entity_claims.len(), 1);
}

#[test]
fn generated_mineshaft_cart_has_unopened_loot_and_is_claimed_once() {
    let mut state = fixture_for("minecraft:chest_minecart", true);
    let mut changes = ChangeSet::default();
    generated::load(&mut state, &[], &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    let slot = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let vehicle = state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap();
    assert_eq!(
        vehicle.loot_table.as_ref().unwrap().as_str(),
        "minecraft:chests/abandoned_mineshaft"
    );
    changes.entities_spawned.clear();
    generated::load(&mut state, &[ChunkPos { x: 0, z: 0 }], &mut changes);
    assert!(changes.entities_spawned.is_empty());
}

#[test]
fn generated_end_city_guard_is_persistent_and_never_reissued_after_death() {
    let mut state = fixture_for("minecraft:shulker", true);
    let mut changes = ChangeSet::default();
    generated::load(&mut state, &[], &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    let slot = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    assert!(state.entities.row(slot).persistent);
    let expected_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:shulker").unwrap())
        .unwrap();
    assert!(matches!(state.entities.body(slot).kind,
        crate::entities::EntityKind::Mob { type_id } if type_id == expected_type));
    assert_eq!(
        state.entities.row(slot).uuid,
        generated::generated_uuid("village:0:0:resident:1")
    );
    state.remove_entity(slot);
    changes.entities_spawned.clear();
    generated::load(&mut state, &[ChunkPos { x: 0, z: 0 }], &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(state.generated_entity_claims.len(), 1);
}

#[test]
fn generated_uuid_lookup_and_claim_survive_checkpoint_restart() {
    checkpoint_roundtrip("minecraft:villager");
}

#[test]
fn end_city_guard_claim_and_persistence_survive_checkpoint_restart() {
    checkpoint_roundtrip("minecraft:shulker");
}

#[test]
fn end_ship_elytra_and_claim_survive_checkpoint_restart() {
    checkpoint_roundtrip("minecraft:item_frame");
}

#[test]
fn generated_end_ship_reward_is_obtainable_once() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let mut state = fixture_for("minecraft:item_frame", true);
    let mut changes = ChangeSet::default();
    generated::load(&mut state, &[], &mut changes);
    let frame = state.hostile.spawned[0];
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "end-explorer".into(),
            permission_level: 0,
        }],
    );
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let mut drops = ChangeSet::default();
    assert!(crate::item_frame::attack(
        &mut state, actor, frame, &mut drops
    ));
    assert_eq!(drops.entities_spawned.len(), 1);
    let dropped = state
        .entities
        .row(state.entities.slot_of(drops.entities_spawned[0]).unwrap())
        .item
        .as_ref()
        .unwrap()
        .full_stack();
    assert_eq!(
        state
            .registry
            .items
            .name_of(dropped.item_id)
            .unwrap()
            .as_str(),
        "minecraft:elytra"
    );
    assert_eq!(dropped.count, 1);
    assert!(crate::item_frame::attack(
        &mut state, actor, frame, &mut drops
    ));
    assert!(state.entities.slot_of(frame).is_none());
    changes.entities_spawned.clear();
    state.hostile.spawned.clear();
    generated::load(&mut state, &[ChunkPos { x: 0, z: 0 }], &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert!(state.hostile.spawned.is_empty());
}

fn checkpoint_roundtrip(entity_type: &'static str) {
    let mut source = fixture_for(entity_type, entity_type != "minecraft:villager");
    let mut changes = ChangeSet::default();
    generated::load(&mut source, &[], &mut changes);
    changes.entities_spawned.append(&mut source.hostile.spawned);
    let id = changes.entities_spawned[0];
    let uuid = source
        .entities
        .row(source.entities.slot_of(id).unwrap())
        .uuid;
    let (_, regions, _) = crate::core::checkpoint_data_for(&source, source.tick);
    let dir = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(dir.path().to_path_buf(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut checkpoint = store.read_checkpoint().unwrap();
    let tick = source.tick.max(1);
    let entities = crate::native_checkpoint::encode_entities(
        crate::dimensions::Dimension::Overworld,
        &regions,
        &source.registry.items,
        tick,
    )
    .unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&checkpoint.globals["checkpoint"].payload).unwrap();
    manifest.tick = tick;
    manifest.generated_entity_claims = source.generated_entity_claims.clone();
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    for record in checkpoint.globals.values_mut() {
        record.captured_tick = tick;
    }
    checkpoint.globals.get_mut("checkpoint").unwrap().payload =
        serde_json::to_vec(&manifest).unwrap();
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick,
                previous_tick: checkpoint.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &checkpoint.globals,
            },
            &source.registry.blocks,
            &source.registry.items,
        )
        .unwrap();
    drop(store);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    let mut restored = fixture_for(entity_type, entity_type != "minecraft:villager");
    restored.tick = recovered.manifest.tick;
    let replay = &recovered.dimensions[&crate::dimensions::Dimension::Overworld];
    restored.generated_entity_claims = replay.generated_entity_claims.clone();
    for (&id, record) in &replay.entities {
        crate::core::restore_entity(&mut restored, id, record).unwrap();
    }
    assert!(restored.entities.slot_of_uuid(uuid).is_some());
    let mut changes = ChangeSet::default();
    generated::load(&mut restored, &[], &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(restored.entities.len(), 1);
    let slot = restored.entities.slot_of_uuid(uuid).unwrap();
    assert_eq!(restored.entities.row(slot).uuid, uuid);
    if entity_type == "minecraft:item_frame" {
        assert!(restored.entities.row(slot).persistent);
        assert!(restored.entities.row(slot).controls.no_ai);
        assert_eq!(
            restored.entities.body(slot).movement,
            crate::blueprint::MovementClass::Stationary
        );
        assert_eq!(
            restored.entities.row(slot).appearance.extra,
            source
                .entities
                .row(source.entities.slot_of(id).unwrap())
                .appearance
                .extra
        );
        assert_eq!(restored.generated_entity_claims.len(), 1);
        assert!(restored.hostile.spawned.is_empty());
        return;
    }
    if entity_type == "minecraft:shulker" {
        assert!(restored.entities.row(slot).persistent);
        return;
    }
    let original = source
        .entities
        .row(source.entities.slot_of(id).unwrap())
        .villager
        .as_ref()
        .unwrap();
    let loaded = restored.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(loaded.offers, original.offers);
    assert_eq!(loaded.age, original.age);
    assert_eq!(loaded.activity, original.activity);
}

#[test]
fn uuid_rekey_removes_old_alias_and_swap_remove_preserves_new_lookup() {
    let mut state = fixture();
    let first = state.spawn_mob(state.registry.cow_type_id, [0.5, 80.0, 0.5], 20.0);
    let second = state.spawn_mob(state.registry.cow_type_id, [1.5, 80.0, 0.5], 20.0);
    let slot = state.entities.slot_of(second).unwrap();
    let old = state.entities.row(slot).uuid;
    let uuid = uuid::Uuid::from_u128(42);
    state.entities.set_uuid(slot, uuid);
    assert!(state.entities.slot_of_uuid(old).is_none());
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(slot));
    state.remove_entity(state.entities.slot_of(first).unwrap());
    let moved = state.entities.slot_of(second).unwrap();
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(moved));
    state.entities.set_uuid(moved, uuid);
    assert_eq!(state.entities.slot_of_uuid(uuid), Some(moved));
}

#[test]
fn generated_brain_schedule_and_merchant_cover_adult_baby_and_unemployed() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    for age in [-12000, 0] {
        for profession in VillagerProfession::ALL {
            let mut resident = super::VillagerState::default();
            resident.data.profession = profession;
            resident.data.level = 3;
            resident.age = age;
            resident.activity = super::brain::schedule(age, profession, 4000);
            super::trades::initialize_offers(&mut resident, &registry);
            let employed = !matches!(
                profession,
                VillagerProfession::None | VillagerProfession::Nitwit
            );
            assert_eq!(
                !resident.offers.is_empty(),
                employed,
                "{profession:?}, age {age}"
            );
            assert_eq!(
                resident.activity,
                if age < 0 {
                    super::Activity::Play
                } else if profession == VillagerProfession::Nitwit {
                    super::Activity::Idle
                } else {
                    super::Activity::Work
                }
            );
            let offers = resident.offers.clone();
            super::trades::initialize_offers(&mut resident, &registry);
            assert_eq!(resident.offers, offers, "initialization is idempotent");
        }
    }
}

#[test]
fn structure_monsters_use_native_blueprints_and_once_only_persistent_claims() {
    for name in [
        "minecraft:piglin",
        "minecraft:piglin_brute",
        "minecraft:witch",
        "minecraft:pillager",
        "minecraft:vindicator",
        "minecraft:evoker",
        "minecraft:elder_guardian",
        "minecraft:zombie_villager",
        "minecraft:shulker",
    ] {
        for persistent in [false, true] {
            let mut state = fixture_for(name, persistent);
            let mut changes = ChangeSet::default();
            generated::load(&mut state, &[], &mut changes);
            assert_eq!(changes.entities_spawned.len(), 1, "{name}");
            let id = changes.entities_spawned[0];
            let slot = state.entities.slot_of(id).unwrap();
            let row = state.entities.row(slot);
            assert_eq!(
                row.uuid,
                generated::generated_uuid("village:0:0:resident:1")
            );
            if persistent {
                assert!(row.persistent, "{name}");
            }
            assert_eq!(state.entities.rot(slot)[..2], [93.0, 7.0]);
            assert_eq!(state.new_generated_entity_claims.len(), 1);
            state.remove_entity(slot);
            changes.entities_spawned.clear();
            generated::load(&mut state, &[ChunkPos { x: 0, z: 0 }], &mut changes);
            assert!(changes.entities_spawned.is_empty(), "{name} resurrected");
        }
    }
}
