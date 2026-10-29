//! Once-only structure template inhabitants. Claims outlive entity death and
//! share the entity's journal/checkpoint transaction, so reload cannot duplicate.
use super::{VillagerKind, VillagerProfession};
use crate::{
    boundary::{ChangeSet, ChunkPos},
    tick::TickState,
};

pub(crate) fn load(state: &mut TickState, chunks: &[ChunkPos], changes: &mut ChangeSet) {
    let initial;
    let chunks = if !state.generated_entity_initial_scan {
        state.generated_entity_initial_scan = true;
        initial = state.world.resident_positions();
        initial.as_slice()
    } else {
        chunks
    };
    for pos in chunks {
        let Some(chunk) = state.world.chunk(*pos) else {
            continue;
        };
        let markers = chunk.generated_entities();
        for marker in markers {
            if state.generated_entity_claims.contains(&marker.claim) {
                continue;
            }
            if marker.entity_type == "minecraft:item_frame"
                && marker.claim.starts_with("minecraft:end_city/ship@")
                && marker.claim.ends_with("#elytra")
            {
                let elytra = state
                    .registry
                    .items
                    .id_of(
                        &mc_data::Identifier::parse("minecraft:elytra").expect("static identifier"),
                    )
                    .expect("vanilla elytra item");
                let position = crate::boundary::BlockPos {
                    x: marker.position[0].floor() as i32,
                    y: marker.position[1].floor() as i32,
                    z: marker.position[2].floor() as i32,
                };
                let face =
                    [3, 4, 2, 5][((marker.yaw / 90.0).round() as i32).rem_euclid(4) as usize];
                let Some(id) = crate::item_frame::spawn(
                    state,
                    position,
                    face,
                    false,
                    mc_data::ItemStack::new(elytra, 1),
                ) else {
                    continue;
                };
                let slot = state.entities.slot_of(id).expect("new ship frame");
                state.entities.set_uuid(slot, generated_uuid(&marker.claim));
                state.generated_entity_claims.insert(marker.claim.clone());
                state.new_generated_entity_claims.push(marker.claim);
                continue;
            }
            let Some(type_id) = mc_data::Identifier::parse(&marker.entity_type)
                .ok()
                .and_then(|id| state.registry.entity_types.id_of(&id))
            else {
                continue;
            };
            let Some(blueprint) = state.registry.blueprint(type_id) else {
                continue;
            };
            if !matches!(
                marker.entity_type.as_str(),
                "minecraft:villager" | "minecraft:zombie_villager"
            ) {
                let health = blueprint.params.max_health;
                let id = state.spawn_mob(type_id, marker.position, health);
                let slot = state.entities.slot_of(id).expect("new entity");
                state.entities.set_uuid(slot, generated_uuid(&marker.claim));
                state
                    .entities
                    .set_rot(slot, marker.yaw, marker.pitch.clamp(-90.0, 90.0));
                state.entities.row_mut(slot).persistent |= marker.persistent;
                if marker.entity_type == "minecraft:chest_minecart"
                    && marker.claim.starts_with("mineshaft:")
                {
                    let vehicle = state
                        .entities
                        .row_mut(slot)
                        .entity_life
                        .vehicle
                        .as_mut()
                        .expect("chest minecart blueprint has a vehicle");
                    vehicle.loot_table = Some(
                        mc_data::Identifier::parse("minecraft:chests/abandoned_mineshaft")
                            .expect("static loot table identifier"),
                    );
                    vehicle.loot_seed = generated_uuid(&marker.claim).as_u128() as u64 as i64;
                }
                changes.entities_spawned.push(id);
                state.generated_entity_claims.insert(marker.claim.clone());
                state.new_generated_entity_claims.push(marker.claim);
                continue;
            }
            let Some(kind) = VillagerKind::from_name(&marker.villager_kind) else {
                continue;
            };
            let Some(profession) = VillagerProfession::from_name(&marker.profession) else {
                continue;
            };
            let id = state.spawn_mob(type_id, marker.position, 20.0);
            let slot = state.entities.slot_of(id).expect("new entity");
            state.entities.row_mut(slot).persistent |= marker.persistent;
            state.entities.set_uuid(slot, generated_uuid(&marker.claim));
            state
                .entities
                .set_rot(slot, marker.yaw, marker.pitch.clamp(-90.0, 90.0));
            let villager = state
                .entities
                .row_mut(slot)
                .villager
                .as_mut()
                .expect("villager blueprint");
            villager.data.kind = kind;
            villager.data.profession = profession;
            villager.data.level = marker.level.clamp(1, 5);
            villager.age = marker.age;
            villager.data_finalized = true;
            villager.activity = super::brain::schedule(marker.age, profession, state.day_time);
            super::trades::initialize_offers(villager, &state.registry);
            super::sync_appearance(state, slot);
            changes.entities_spawned.push(id);
            state.generated_entity_claims.insert(marker.claim.clone());
            state.new_generated_entity_claims.push(marker.claim);
        }
    }
}

// Same source identity policy as the old template authority: two FNV-1a
// offset bases make a stable 128-bit placement identity without allocation.
pub(crate) fn generated_uuid(claim: &str) -> uuid::Uuid {
    let hash = |seed: u64| {
        claim.bytes().fold(seed, |value, byte| {
            (value ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    };
    uuid::Uuid::from_u128(
        (u128::from(hash(0xCBF2_9CE4_8422_2325)) << 64) | u128::from(hash(0x8422_2325_CBF2_9CE4)),
    )
}
