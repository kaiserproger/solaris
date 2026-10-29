//! Checkpoint/journal capture of entities and players.
//! Moved verbatim from `core.rs`.

use super::*;

/// Convert region/checkpoint NBT Motion to core units; journal images stay in
/// blocks/second. Vanilla Entity.java saves/loads delta movement unchanged.
pub(crate) fn velocity_from_vanilla_motion(motion: [f64; 3]) -> [f64; 3] {
    motion.map(|component| component * 20.0)
}

/// Restore sheep's canonical vanilla fields into its synchronized wool byte.
pub(crate) fn appearance_from_vanilla_entity(
    record: &mc_data::entity_nbt::VanillaEntity,
) -> crate::journal::EntityAppearanceImage {
    let mut image = crate::journal::EntityAppearanceImage {
        health: record.health,
        experience_orb: record.experience_orb,
        ..Default::default()
    };
    if let Some(wool) = record.sheep_wool {
        image.extra.push((
            crate::blueprint::SHEEP_WOOL_DATA_INDEX,
            crate::journal::EntityDataValueImage::Byte(wool as i8),
        ));
    }
    for (key, tag) in &record.extra_fields {
        match (key.as_str(), tag) {
            ("CustomName", mc_nbt::Tag::String(name)) => image.custom_name = Some(name.clone()),
            ("CustomNameVisible", mc_nbt::Tag::Byte(value)) => {
                image.custom_name_visible = *value != 0;
            }
            ("IsBaby", mc_nbt::Tag::Byte(value))
                if crate::entity_life::uses_is_baby(&record.id) =>
            {
                image.baby = *value != 0;
            }
            _ => {}
        }
    }
    image
}

/// Collect one checkpoint's full domain data from live tick state: player
/// records (same fold-back as the journal post-images), entity base (full
/// records WITH motion), and block-entity storage.
pub(crate) fn checkpoint_data_for(
    state: &TickState,
    tick: u64,
) -> (
    Vec<CheckpointPlayerData>,
    std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk>,
    crate::core::CheckpointBlockEntities,
) {
    let mut players: Vec<CheckpointPlayerData> = state
        .players
        .keys()
        .filter_map(|id| {
            let slot = state.entities.slot_of_any(*id)?;
            let uuid = state.entities.row(slot).uuid.to_string();
            let record = player_record_for(state, *id, tick)?;
            Some(CheckpointPlayerData { uuid, record })
        })
        .collect();
    // Offline players (disconnected since their last journal post-image):
    // include their pending images so the manifest never advances past
    // their only durable record before pruning.
    for (uuid, record) in &state.offline_players {
        let mut record = record.clone();
        record.tick = tick;
        players.push(CheckpointPlayerData {
            uuid: uuid.to_string(),
            record,
        });
    }
    let entities = checkpoint_entities(state, tick);
    let block_entities = state
        .block_entities
        .iter()
        .map(|(pos, live)| {
            (
                *pos,
                crate::journal::ReplayBlockEntity {
                    slots: live.slots.clone(),
                    furnace: live.furnace_image(),
                    tick,
                },
            )
        })
        .collect();
    (players, entities, block_entities)
}

/// Read-only inputs of one entity's checkpoint record. `TickState` owns channel
/// endpoints and is not `Sync`, so capture workers borrow only these (like the
/// row-preparation `PrepView`).
pub(super) struct EntityCaptureView<'a> {
    pub(super) entities: &'a crate::entities::EntityStore,
    pub(super) registry: &'a crate::registry::CoreRegistry,
    pub(super) tick: u64,
}

/// Fields whose owners need the whole `TickState`. Gathered serially (only
/// raiders, the ominous item spawner and End fight bodies have any) and spliced
/// at the positions the serial capture put them.
#[derive(Default)]
pub(super) struct EntityCaptureExtras {
    /// Raid membership, then the ominous item spawner's pending item.
    pub(super) before_allay: Vec<(String, mc_nbt::Tag)>,
    /// End fight dragon/crystal state.
    pub(super) end_fight: Vec<(String, mc_nbt::Tag)>,
}

/// Entity base as vanilla records grouped by chunk. The Solaris wire id rides
/// as `solaris:WireId` so journal tail entries (keyed by wire id) still match
/// after recovery. Records are built in parallel (about 10 us each: 110-137 ms
/// serial at 11.5k entities, on the tick thread) and grouped serially in slot
/// order, so the output is identical to a serial slot-order capture.
pub(super) fn checkpoint_entities(
    state: &TickState,
    tick: u64,
) -> std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> {
    use rayon::prelude::*;
    let slots: Vec<crate::entities::EntitySlot> = state.entities.slots().collect();
    let extras = entity_capture_extras(state, &slots);
    let view = EntityCaptureView {
        entities: &state.entities,
        registry: &state.registry,
        tick,
    };
    let mut records = Vec::new();
    // Indexed collection keeps slot order without a sort.
    slots
        .par_iter()
        .enumerate()
        .with_min_len(256)
        .map(|(index, &slot)| {
            entity_capture_record(&view, slot, capture_extras_for(&extras, index))
        })
        .collect_into_vec(&mut records);
    group_entity_records(records)
}

/// Serial gather of the state-dependent fields, keyed by slot index.
pub(super) fn entity_capture_extras(
    state: &TickState,
    slots: &[crate::entities::EntitySlot],
) -> Vec<(usize, EntityCaptureExtras)> {
    use crate::entities::EntityKind;
    let mut gathered = Vec::new();
    for (index, &slot) in slots.iter().enumerate() {
        let type_id = match state.entities.body(slot).kind {
            EntityKind::Mob { type_id }
            | EntityKind::Item { type_id }
            | EntityKind::ExperienceOrb { type_id } => type_id,
            EntityKind::Player { .. } => continue,
        };
        let mut before_allay = crate::raids::entity_fields(state, slot);
        if mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
            .is_some_and(|contract| contract.name == "minecraft:ominous_item_spawner")
        {
            before_allay.extend(crate::trial_spawner::entity_fields(
                state,
                state.entities.row(slot).uuid,
            ));
        }
        let end_fight = crate::end_fight::entity_fields(state, slot);
        if !before_allay.is_empty() || !end_fight.is_empty() {
            gathered.push((
                index,
                EntityCaptureExtras {
                    before_allay,
                    end_fight,
                },
            ));
        }
    }
    gathered
}

pub(super) fn capture_extras_for(
    extras: &[(usize, EntityCaptureExtras)],
    index: usize,
) -> Option<&EntityCaptureExtras> {
    extras
        .binary_search_by_key(&index, |(slot_index, _)| *slot_index)
        .ok()
        .map(|position| &extras[position].1)
}

/// Group records by chunk in the order given.
pub(super) fn group_entity_records(
    records: impl IntoIterator<Item = Option<((i32, i32), mc_data::entity_nbt::VanillaEntity)>>,
) -> std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> {
    let mut entities: std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> =
        std::collections::BTreeMap::new();
    for ((cx, cz), record) in records.into_iter().flatten() {
        entities
            .entry((cx, cz))
            .or_insert_with(|| mc_world::anvil::EntityChunk {
                x: cx,
                z: cz,
                entities: Vec::new(),
            })
            .entities
            .push(record);
    }
    entities
}

/// Read one live entity through the same authoritative projection as checkpoints.
pub(crate) fn entity_data_record(
    state: &TickState,
    slot: crate::entities::EntitySlot,
) -> Option<mc_data::entity_nbt::VanillaEntity> {
    let extras = entity_capture_extras(state, &[slot]);
    let view = EntityCaptureView {
        entities: &state.entities,
        registry: &state.registry,
        tick: crate::command_context::execution_tick(state),
    };
    entity_capture_record(&view, slot, capture_extras_for(&extras, 0)).map(|(_, record)| record)
}

/// One non-player entity's vanilla checkpoint record and its chunk, or `None`
/// for players and entity types without a vanilla contract.
pub(super) fn entity_capture_record(
    view: &EntityCaptureView<'_>,
    slot: crate::entities::EntitySlot,
    extras: Option<&EntityCaptureExtras>,
) -> Option<((i32, i32), mc_data::entity_nbt::VanillaEntity)> {
    use crate::entities::EntityKind;
    let body = view.entities.body(slot);
    let row = view.entities.row(slot);
    let type_id = match body.kind {
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => type_id,
        EntityKind::Player { .. } => return None,
    };
    let name = view.registry.entity_type_name(type_id)?.to_owned();
    let tick = view.tick;
    let rot = view.entities.rot(slot);
    let pos = view.entities.pos(slot);
    let vel = view.entities.vel(slot);
    let sheep_wool = (name == "minecraft:sheep").then(|| crate::tick::wool_of(&row.appearance));
    let ageable = view.registry.blueprint(type_id).is_some_and(|blueprint| {
        blueprint.capabilities.ageable
            || blueprint
                .goals
                .iter()
                .any(|goal| goal.kind == crate::blueprint::GoalKind::Breed)
    });
    let mut extra_fields = row
        .villager
        .as_ref()
        .map(|villager| {
            crate::villager::persistence::villager_to_fields(&view.registry.items, villager, tick)
                .expect("live villager state satisfies persistence invariants")
        })
        .unwrap_or_default();
    if crate::entity_life::uses_is_baby(&name) && row.villager.is_none() {
        extra_fields.push((
            "IsBaby".into(),
            mc_nbt::Tag::Byte(i8::from(row.appearance.baby)),
        ));
    }
    extra_fields.extend(
        crate::journal::EntityStateImage::from_row(body, row).to_fields(&view.registry.blocks),
    );
    if type_id & crate::blueprint::CUSTOM_TYPE_FLAG != 0 {
        let presentation = view
            .registry
            .entity_type_name(crate::blueprint::wire_type_id(type_id))?;
        extra_fields.push((
            "solaris:Presentation".into(),
            mc_nbt::Tag::String(presentation.into()),
        ));
    }
    if let Some(extras) = extras {
        extra_fields.extend(extras.before_allay.iter().cloned());
    }
    if let Some(allay) = &row.allay {
        extra_fields.extend(crate::allay::to_fields(allay, &view.registry.items));
    }
    if let Some(vehicle) = &row.entity_life.vehicle {
        extra_fields.extend(crate::vehicles::to_fields(vehicle, &view.registry.items));
    }
    extra_fields.extend(crate::mounts::persistence::cargo_to_fields(
        &row.entity_life.mount,
        &view.registry.items,
        &name,
    ));
    extra_fields.extend(
        crate::entity_loot::equipment_to_fields(
            &view.registry.items,
            &row.appearance.equipment,
            &row.loot,
        )
        .expect("live entity equipment satisfies persistence invariants"),
    );
    if let Some(name) = &row.appearance.custom_name {
        extra_fields.push(("CustomName".into(), mc_nbt::Tag::String(name.clone())));
    }
    extra_fields.push((
        "CustomNameVisible".into(),
        mc_nbt::Tag::Byte(i8::from(row.appearance.custom_name_visible)),
    ));
    if let Some(extras) = extras {
        extra_fields.extend(extras.end_fight.iter().cloned());
    }
    if !row.opaque_entity_fields.is_empty() {
        let (_, opaque) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice())
            .expect("validated retained entity fields");
        if let mc_nbt::Tag::Compound(fields) = opaque {
            for (key, value) in fields {
                if !(row.projectile.as_ref().is_some_and(|projectile| {
                    crate::projectiles::persistence::owns_field(projectile.kind, &key)
                }) || row.villager.is_some() && crate::villager::persistence::owns_field(&key)
                    || extra_fields.iter().any(|(known, _)| known == &key))
                {
                    extra_fields.push((key, value));
                }
            }
        }
    }
    let tadpole = name == "minecraft:tadpole";
    let mut record = mc_data::entity_nbt::VanillaEntity {
        id: name,
        uuid: row.uuid,
        pos,
        // Core velocities are blocks/second; vanilla NBT Motion is blocks/tick.
        motion: vel.map(|component| component / 20.0),
        yaw: rot[0],
        pitch: rot[1],
        on_ground: view.entities.ground(slot),
        health: row.health,
        age: if tadpole {
            row.entity_life.age_lock.tadpole_age.min(i32::MAX as u32) as i32
        } else if ageable {
            if row.entity_life.child_ticks != 0 {
                -(row.entity_life.child_ticks.min(i32::MAX as u32) as i32)
            } else {
                row.breed_cooldown.min(i32::MAX as u32) as i32
            }
        } else {
            row.age.min(i32::MAX as u32) as i32
        },
        breed_cooldown: row.breed_cooldown as i32,
        pickup_delay: body.pickup_delay as i32,
        item: row
            .item
            .as_deref()
            .map(crate::entities::StackRow::full_stack),
        experience_orb: row.appearance.experience_orb,
        sheep_wool,
        active_effects: crate::journal::active_effects_to_nbt(&row.active_effects),
        absorption_amount: row.active_effects.absorption,
        wire_id: entity_id_int(body.id),
        extra_fields,
    };
    crate::item_frame::capture(&mut record, &row.appearance);
    crate::painting::capture(&mut record, &row.appearance, view.registry);
    if let Some(projectile) = &row.projectile {
        record
            .extra_fields
            .retain(|(key, _)| !crate::projectiles::persistence::owns_field(projectile.kind, key));
        record.age = projectile.age.min(i32::MAX as u32) as i32;
        for (key, value) in
            crate::projectiles::persistence::to_fields(projectile, &view.registry.items)
        {
            if key == "Item" {
                record.item = Some(projectile.stack.clone());
            } else {
                record.extra_fields.push((key, value));
            }
        }
    }
    // Floor division: truncation files negative-coordinate entities
    // under the wrong chunk/region (vanilla keys by floor).
    let (cx, cz) = (
        (pos[0].floor() as i32).div_euclid(16),
        (pos[2].floor() as i32).div_euclid(16),
    );
    Some(((cx, cz), record))
}

/// Full player record for journal/checkpoint from live state: fold the open
/// menu back (as if closed) so carried/grid rejoin the inventory.
/// Full player record for journal/checkpoint from live state: real
/// custody, never folded. Inventory 41 + cursor + open crafting grid ride as
/// separate stacks; nothing is merged or dropped during serialization.
pub(crate) fn player_record_for(
    state: &TickState,
    id: EntityId,
    tick: u64,
) -> Option<crate::journal::ReplayPlayer> {
    player_record(state, id, tick, true)
}

pub(crate) fn player_data_tag(state: &TickState, id: EntityId) -> Result<mc_nbt::Tag, CoreError> {
    let record = player_record(
        state,
        id,
        crate::command_context::execution_tick(state),
        false,
    )
    .ok_or_else(|| CoreError::Storage("Player is unavailable".into()))?;
    crate::journal::player_query_tag(&record, &state.registry.items)
}

pub(super) fn player_record(
    state: &TickState,
    id: EntityId,
    tick: u64,
    include_statistics: bool,
) -> Option<crate::journal::ReplayPlayer> {
    if state.entity_session_locked(id) {
        return None;
    }
    let slot = state.entities.slot_of_any(id)?;
    let player = state.players.get(&id)?;
    let rot = state.entities.rot(slot);
    // Window 0 lives in `inventory_menu`; container windows in `open`.
    let menu = player.open.as_ref().unwrap_or(&player.inventory_menu);
    let (carried, grid) = (menu.carried.clone(), menu.grid.clone());
    Some(crate::journal::ReplayPlayer {
        opaque_player_fields: crate::entity_loot::player_attributes_to_opaque(
            &player.opaque_player_fields,
            &state.entities.row(slot).loot.attributes,
        )
        .ok()?,
        statistics: if include_statistics {
            player.statistics.at_tick(tick)
        } else {
            Default::default()
        },
        advancement_progress: player.advancement_progress.clone(),
        unlocked_recipes: player.unlocked_recipes.clone(),
        dimension: state.dimension,
        pos: state.entities.pos(slot),
        yaw: rot[0],
        pitch: rot[1],
        health: player.vitals.health,
        food: player.vitals.food,
        saturation: player.vitals.saturation,
        exhaustion: player.vitals.exhaustion,
        air: player.vitals.air,
        experience_level: player.vitals.experience.level,
        experience_progress: player.vitals.experience.progress,
        experience_total: player.vitals.experience.total,
        game_mode: player.game_mode as u8,
        selected: player.inventory.selected,
        inventory: player.inventory.slots.clone(),
        carried,
        grid,
        respawn: player.respawn_point,
        respawn_dimension: player.respawn_dimension,
        respawn_forced: player.respawn_forced,
        respawn_yaw: player.respawn_yaw,
        respawn_pitch: player.respawn_pitch,
        active_effects: player.active_effects.clone(),
        current_impulse_impact_pos: player.current_impulse_impact_pos,
        fall_flying: player.fall_flying,
        enchantment_seed: player.enchantment_seed,
        enchantment_offer_seed: player.enchantment_offer_seed,
        warden_warning: player.warden_warning,
        burn_ticks: state.entities.body(slot).burn_ticks,
        portal: state.entities.row(slot).entity_life.portal,
        tick,
    })
}
