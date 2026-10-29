//! Startup/WAL recovery of dimension and tick state; entity restore.
//! Moved verbatim from `core.rs`.

use super::*;

pub(super) fn load_dimension_state(
    config: &CoreConfig,
    registry: Arc<CoreRegistry>,
    dimension: crate::dimensions::Dimension,
    root_manifest: &CheckpointManifest,
    replay: Replay,
    store: Arc<mc_world::native::NativeStore>,
    settlement_globals: &std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
) -> Result<TickState, CoreError> {
    let manifest_now = if dimension == crate::dimensions::Dimension::Overworld {
        root_manifest.clone()
    } else {
        root_manifest
            .dimensions
            .get(&dimension)
            .map(|manifest| (**manifest).clone())
            .unwrap_or_default()
    };
    let manifest_tick = root_manifest.tick;
    crate::mounts::persistence::validate_replay(&replay)?;
    let recovered_tick = replay.tick.max(manifest_tick);

    // Worldgen: load the same vanilla router/density cache used by the
    // production v1 startup when mc-edge supplies its content directory.
    let terrain = if dimension == crate::dimensions::Dimension::Overworld {
        Some(overworld_terrain(config, &registry)?)
    } else {
        None
    };
    let world_spawn = if let Some(spawn) = manifest_now.world_spawn {
        spawn
    } else if let Some(terrain) = &terrain {
        let spawn = terrain
            .locate_safe_spawn()
            .ok_or_else(|| CoreError::Worldgen("no bounded dry land spawn found".into()))?;
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z]
    } else {
        [0, 80, 0]
    };
    let generator: Arc<dyn ChunkGenerator> = match dimension {
        crate::dimensions::Dimension::Overworld => Arc::new(terrain.expect("overworld terrain")),
        crate::dimensions::Dimension::Nether => Arc::new(nether_terrain(config, &registry)?),
        crate::dimensions::Dimension::End => Arc::new(end_terrain(config, &registry)?),
    };
    let mut world = ChunkWorld::new_native(
        config.seed,
        store,
        crate::native_checkpoint::dimension_id(dimension),
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        world_spawn,
        dimension.geometry(),
    );
    world.set_world_spawn(world_spawn, manifest_now.world_spawn_rotation);
    world.restore_inhabited_time(&replay, Some(&manifest_now));
    world.restore_sim_debt(&replay, Some(&manifest_now))?;

    // Replay absolute block post-images into the overlay, then fold once
    // chunks load (fold keeps homeless writes in the overlay).
    for ((x, y, z), state) in &replay.blocks {
        world.set_block_catch_up(*x, *y, *z, *state);
    }
    if !replay.blocks.is_empty() {
        let want = replay
            .blocks
            .keys()
            .map(|(x, _, z)| {
                (
                    crate::boundary::ChunkPos {
                        x: x.div_euclid(16),
                        z: z.div_euclid(16),
                    },
                    0,
                )
            })
            .collect();
        world.ensure_blocking(&want);
        world.fold_overlay();
    }
    // Fold replayed blocks before installing their opaque entities:
    // replacing a block clears any old sign NBT in the base chunk.
    for ((x, y, z), nbt) in &replay.opaque_block_entities {
        if let Some(bytes) = nbt {
            let tag = mc_nbt::read_network(&mut bytes.as_slice()).map_err(|error| {
                CoreError::Storage(format!("storage block entity at ({x},{y},{z}): {error}"))
            })?;
            if let mc_nbt::Tag::Compound(fields) = tag {
                mc_data::storage_blocks::validate(&fields, &registry.items).map_err(|error| {
                    CoreError::Storage(format!("storage block entity at ({x},{y},{z}): {error}"))
                })?;
            }
        }
        let chunk = crate::boundary::ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        world.ensure_blocking(&[(chunk, 0)].into_iter().collect());
        world.fold_overlay();
        world.set_opaque_block_entity(
            crate::boundary::BlockPos {
                x: *x,
                y: *y,
                z: *z,
            },
            nbt.clone(),
        );
    }
    // Replay adoption is already durable. Do not re-journal its replacement
    // removals on the first live tick after installing the final opaque images.
    let _ = world.take_block_entity_removals();

    let mut tick_state = TickState::new(
        Arc::clone(&registry),
        world,
        i32::from(config.simulation_distance),
        config.player_outbox_cap,
    );
    tick_state.require_plugin_login = config.require_plugin_login;
    tick_state.max_airtime_ticks = config.max_airtime_ticks;
    tick_state.enable_command_block = config.enable_command_block;
    tick_state.spawn_protection = config.spawn_protection;
    tick_state.mob_griefing = config.mob_griefing;
    tick_state.config_view_distance = i32::from(config.view_distance);
    tick_state.dimension = dimension;
    tick_state.tick = recovered_tick;
    // Settlement registries are per-dimension authorities: every dimension
    // hydrates its own rows (matched by home dimension) folded with its
    // dimension-routed WAL tail. Membership columns hydrate with entities below.
    {
        let sequence = match crate::settlements::persistence::checkpoint_sequence(
            settlement_globals,
            root_manifest.tick,
        ) {
            Ok(sequence) => sequence,
            // Fresh worlds and unit fixtures predate the sequence row and
            // hold no settlement rows either; anything else is corruption.
            Err(_)
                if !settlement_globals
                    .keys()
                    .any(|key| key == "settlement_sequence" || key.starts_with("settlement/")) =>
            {
                0
            }
            Err(error) => return Err(error),
        };
        crate::settlements::persistence::hydrate_dimension(
            &mut tick_state,
            settlement_globals,
            &replay,
            sequence,
        )?;
    }
    tick_state.plugins = replay.plugins;
    tick_state.plugins.validate()?;
    tick_state.block_ticks = crate::block_ticks::BlockTicks::from_image(
        replay.block_ticks,
        Some(&manifest_now.block_ticks),
    );
    tick_state.fluid_ticks =
        crate::fluids::FluidTicks::from_image(replay.fluid_ticks, Some(&manifest_now.fluid_ticks));
    // Startup chunks predate normal admission callbacks. Import their
    // native delays only when no manifest/WAL coverage already owns them.
    for chunk in tick_state.world.chunks() {
        tick_state.fluid_ticks.hydrate(chunk, recovered_tick);
    }
    tick_state.gamerules = replay.gamerules.into_iter().collect();
    if let Some(value) = tick_state.gamerules.get_mut("random_tick_speed")
        && let Ok(speed) = value.parse::<i32>()
        && speed > crate::command_rules::MAX_RANDOM_TICK_SPEED
    {
        tracing::warn!(
            speed,
            maximum = crate::command_rules::MAX_RANDOM_TICK_SPEED,
            "clamping persisted randomTickSpeed"
        );
        *value = crate::command_rules::MAX_RANDOM_TICK_SPEED.to_string();
        tick_state.gamerules_dirty = true;
    }
    replay.border.validate()?;
    tick_state.border = replay.border;
    tick_state.difficulty = replay.difficulty;
    if let Some(climate) = replay.climate {
        climate.restore(&mut tick_state)?;
    }
    tick_state.scoreboard = replay.scoreboard.unwrap_or_default();
    tick_state.command_storage = match replay.command_storage {
        Some(bytes) => crate::command_storage::decode(&bytes).map_err(CoreError::Storage)?,
        None => Default::default(),
    };
    tick_state.maps = crate::maps::Maps::restore(
        replay.saved_maps,
        crate::maps::sequence(settlement_globals)?,
    )?;
    tick_state.bossbars = replay
        .bossbars
        .as_deref()
        .map(crate::bossbar::BossBars::from_bytes)
        .transpose()
        .map_err(crate::config::CoreError::Storage)?
        .unwrap_or_default();
    tick_state.generated_entity_claims = replay.generated_entity_claims.clone();
    tick_state.end_fight = replay.end_fight.unwrap_or_default();
    tick_state.raids = replay.raids.unwrap_or_default();
    tick_state.raids.validate()?;
    tick_state.trader_spawner = replay.wandering_trader.unwrap_or_default();
    tick_state.datapacks.restore(replay.scheduled_functions)?;
    if let Some(selection) = replay.datapack_selection {
        tick_state.datapacks.restore_selection(selection)?;
    }
    tick_state.block_log = crate::block_log::BlockLog::restore(config.block_log, replay.block_log);

    // Replay player + entity records. Full player state (inventory,
    // vitals, xp, mode) restores here once the inventory/survival modules
    // wire into TickState (step 2); today position/health/food restore and
    // the rest rides the replay record.
    for (uuid_str, player) in &replay.players {
        if player.dimension != dimension {
            continue;
        }
        crate::ender_chest::contents(&player.opaque_player_fields, &registry.items)?;
        let uuid = uuid_str.parse::<uuid::Uuid>().map_err(|err| {
            CoreError::Storage(format!(
                "{}: recovered player UUID {uuid_str}: {err}",
                dimension.name()
            ))
        })?;
        tick_state.offline_players.insert(uuid, player.clone());
    }
    let mut max_wire_id: i32 = 0;
    // Drain the replay owner as live rows are restored: retaining both complete
    // populations multiplies peak memory during cold-column growth.
    for (id, record) in replay.entities {
        max_wire_id = max_wire_id.max(id);
        crate::vehicles::validate_replay(&registry, &record)?;
        restore_entity(&mut tick_state, id, &record)?;
    }
    for (pos, record) in &replay.block_entities {
        tick_state
            .block_entities
            .hydrate(*pos, || crate::tick::BlockEntityState {
                slots: record.slots.clone(),
                furnace: record.furnace.as_ref().map(|image| {
                    crate::inventory::furnace::FurnaceState {
                        lit_remaining: image.lit_remaining,
                        lit_total: image.lit_total,
                        cook_progress: image.cook_progress,
                        cook_total: image.cook_total,
                        recipes_used: Arc::clone(&image.recipes_used),
                    }
                }),
            });
    }
    let _ = max_wire_id;
    for (owner, (chunk, (_, radius))) in &replay.tickets {
        tick_state.tickets.insert(
            owner.clone(),
            (
                crate::boundary::ChunkPos {
                    x: chunk[0],
                    z: chunk[1],
                },
                *radius,
            ),
        );
    }
    crate::end_fight::restore(&mut tick_state)?;
    crate::raids::restore(&mut tick_state)?;
    crate::end_portals::validate_gateways(&tick_state).map_err(CoreError::Storage)?;
    crate::conduit::validate_loaded(&tick_state).map_err(CoreError::Storage)?;
    crate::sculk::validate(&tick_state).map_err(CoreError::Storage)?;
    crate::shrieker::validate_loaded(&tick_state).map_err(CoreError::Storage)?;
    if let Some(error) = tick_state.world.load_errors().next() {
        return Err(CoreError::Storage(error.to_string()));
    }
    Ok(tick_state)
}

#[cfg(test)]
pub(crate) fn recover_dimension_state(
    config: &CoreConfig,
    dimension: crate::dimensions::Dimension,
) -> Result<TickState, CoreError> {
    let store = crate::native_checkpoint::tests::fixture_store(config)?;
    let registry = Arc::new(CoreRegistry::load(config.vanilla_data_dir.as_deref())?);
    let reader = store
        .checkpoint_reader()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let mut recovered =
        crate::native_checkpoint::recover_with_reader(config, registry.clone(), reader)?;
    let replay = recovered
        .dimensions
        .remove(&dimension)
        .ok_or_else(|| CoreError::Storage(format!("missing recovered dimension {dimension:?}")))?;
    let mut state = load_dimension_state(
        config,
        registry,
        dimension,
        &recovered.manifest,
        replay,
        store,
        &recovered.checkpoint.globals,
    )?;
    state.native_globals = recovered.checkpoint.globals;
    crate::settlements::persistence::refuse_legacy_plugin_state(&state)?;
    crate::settlements::persistence::finish_recovery(&mut state)?;
    Ok(state)
}

/// Recover the three dimension worlds and the global native state from the
/// store: the checkpoint-recovery half of [`Core::open`], shared with the
/// input-log replayer. No thread is started.
pub(crate) fn recover_tick_state(
    config: &CoreConfig,
    registry: &Arc<CoreRegistry>,
    store: &Arc<mc_world::native::NativeStore>,
    fresh: bool,
    plugin_runtime: Option<crate::plugin::PluginRuntime>,
    build_queries: Option<crate::build_precommit::PreviewQueries>,
) -> Result<(TickState, CheckpointManifest, bool), CoreError> {
    let (root_manifest, mut replays, globals) = if fresh {
        store
            .initialize_dimensions(
                &crate::native_checkpoint::generation_records(config)?["dimensions"],
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut manifest = CheckpointManifest {
            domain_versions: crate::native_checkpoint::domain_versions(),
            ..Default::default()
        };
        for dimension in [
            crate::dimensions::Dimension::Nether,
            crate::dimensions::Dimension::End,
        ] {
            manifest
                .dimensions
                .insert(dimension, Box::new(CheckpointManifest::default()));
        }
        let mut globals = crate::native_checkpoint::generation_records(config)?;
        for key in ["level_metadata", "conversion_extras"] {
            globals.insert(
                key.into(),
                crate::native_checkpoint::tag_record(0, &mc_nbt::Tag::Compound(Vec::new()))?,
            );
        }
        globals.insert(
            "settlement_sequence".into(),
            crate::native_checkpoint::record(
                0,
                serde_json::to_vec(&0u64).map_err(|e| CoreError::Storage(e.to_string()))?,
            ),
        );
        (
            manifest,
            [
                crate::dimensions::Dimension::Overworld,
                crate::dimensions::Dimension::Nether,
                crate::dimensions::Dimension::End,
            ]
            .into_iter()
            .map(|dimension| (dimension, Replay::default()))
            .collect::<std::collections::BTreeMap<_, _>>(),
            globals,
        )
    } else {
        // Opening the sole writer may repair redb's allocator bookkeeping.
        // Semantic refusal below never publishes a new checkpoint transaction.
        let recovered = crate::native_checkpoint::recover_with_reader(
            config,
            Arc::clone(registry),
            store
                .checkpoint_reader()
                .map_err(|e| CoreError::Storage(e.to_string()))?,
        )?;
        (
            recovered.manifest,
            recovered.dimensions,
            recovered.checkpoint.globals,
        )
    };
    let mut tick_state = load_dimension_state(
        config,
        Arc::clone(registry),
        crate::dimensions::Dimension::Overworld,
        &root_manifest,
        replays
            .remove(&crate::dimensions::Dimension::Overworld)
            .expect("root replay"),
        Arc::clone(store),
        &globals,
    )?;
    for dimension in [
        crate::dimensions::Dimension::Nether,
        crate::dimensions::Dimension::End,
    ] {
        let child = load_dimension_state(
            config,
            Arc::clone(registry),
            dimension,
            &root_manifest,
            replays.remove(&dimension).expect("child replay"),
            Arc::clone(store),
            &globals,
        )?;
        tick_state
            .other_dimensions
            .insert(dimension, Box::new(child));
    }
    tick_state.native_globals = globals;
    let salt_minted = crate::entity_uuid::install_salt(&mut tick_state)?;
    crate::settlements::persistence::refuse_legacy_plugin_state(&tick_state)?;
    crate::settlements::persistence::finish_recovery(&mut tick_state)?;
    tick_state.plugin_runtime = plugin_runtime;
    tick_state.build_queries = build_queries;
    crate::plugin_player::restore(&mut tick_state)?;
    crate::plugin::validate_startup(
        &mut tick_state,
        fresh || !root_manifest.domain_versions.contains_key("plugins"),
    )?;
    crate::dimensions::validate_identity(&mut tick_state)?;
    Ok((tick_state, root_manifest, salt_minted))
}

/// Restore one journaled entity record.
pub(crate) fn restore_entity(
    state: &mut TickState,
    id: i32,
    record: &crate::journal::ReplayEntity,
) -> Result<(), CoreError> {
    let entity_id = EntityId(id);
    let uuid = crate::dimensions::recovery::validate_entity(state, id, record)?;
    state.entities.set_next_wire_id(id);
    let kind = if record.type_id == state.registry.item_type_id {
        EntityKind::Item {
            type_id: record.type_id,
        }
    } else if record.appearance.experience_orb.is_some() {
        EntityKind::ExperienceOrb {
            type_id: record.type_id,
        }
    } else {
        EntityKind::Mob {
            type_id: record.type_id,
        }
    };
    let mut appearance = restore_appearance(record);
    // Item entities mirror their stack on the appearance (as live spawns do):
    // views read the stack from there.
    if matches!(kind, EntityKind::Item { .. }) {
        appearance.item = record.item.clone();
        appearance.health = record.health;
    }
    let appearance = Arc::new(appearance);
    let (movement, half_width, height, speed, damage, sight, reach) =
        if let Some(blueprint) = state.registry.blueprint(record.type_id) {
            (
                blueprint.movement,
                blueprint.half_width,
                blueprint.height,
                blueprint.params.speed,
                blueprint.params.attack_damage,
                blueprint.sensors.sight_range,
                blueprint.sensors.attack_reach,
            )
        } else {
            // Non-mob owners (including falling blocks and TNT) do not have
            // an AI blueprint. Their physical size still comes from the
            // complete validated registry, and their controller restores
            // its movement state below.
            let contract =
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(record.type_id)
                    .expect("retained entity type validated before recovery");
            let dimensions = mc_data::entity_types::canonical_dimensions(contract);
            (
                crate::blueprint::MovementClass::Stationary,
                dimensions.half_width(),
                dimensions.height,
                0.0,
                0.0,
                0.0,
                0.0,
            )
        };
    state.entities.spawn(
        entity_id,
        uuid,
        kind,
        crate::entities::ColdFields {
            // Ordinary motion is checkpoint-durable; existing crossing images
            // carry position so replay keeps the right chunk-clock authority.
            pos: record.pos,
            vel: record.vel,
            yaw: record.yaw,
            pitch: record.pitch,
            ground: record.ground,
            health: record.health,
            breed_cooldown: record.breed_cooldown,
            pickup_delay: record.pickup_delay,
            item: record.item.as_ref().map(|stack| crate::entities::StackRow {
                item_id: stack.item_id,
                count: stack.count,
                stack: Some(stack.clone()),
            }),
            appearance,
            movement,
            half_width,
            height,
            speed,
            attack_damage: damage,
            sight_range: sight,
            attack_reach: reach,
        },
    );
    if let Some(slot) = state.entities.slot_of(entity_id) {
        let baby = record.appearance.baby
            || (record.state.entity_life.child_ticks != 0
                && crate::entity_life::age_has_baby_model(
                    state
                        .registry
                        .entity_type_name(record.type_id)
                        .expect("restored entity type"),
                ));
        if baby {
            state.entities.set_half_width(slot, half_width * 0.5);
        }
        state
            .entities
            .set_entity_life(slot, record.state.entity_life.clone());
        let (body, row) = state.entities.parts_mut(slot);
        body.alive = record.alive;
        row.age = record.age;
        row.protected_until = record.state.protected_until;
        row.active_effects = record.active_effects.clone();
        row.villager = record.villager.clone().map(Box::new);
        row.persistent = record.state.persistent;
        row.no_action_time = record.state.no_action_time;
        body.air_supply = record.state.air_supply;
        body.burn_ticks = record.state.burn_ticks;
        row.love = record.state.love;
        row.loot = record.state.loot.clone();
        row.controls = record.state.controls;
        row.falling = record.state.falling;
        row.tnt = record.state.tnt;
        row.special_mob = record.state.special_mob.clone().map(Box::new);
        row.bee = record.state.bee.clone().map(Box::new);
        row.allay = record.state.allay.clone().map(Box::new);
        row.opaque_entity_fields = record.state.opaque_entity_fields.clone();
        crate::entity_life::restore_appearance(body, row);
        if body.burn_ticks != 0 {
            Arc::make_mut(&mut row.appearance).shared_flags |= 1;
        }
        if baby {
            Arc::make_mut(&mut row.appearance).baby = true;
            body.height *= 0.5;
        }
        crate::entity_loot::restore(state, slot);
        crate::armor_stand::restore(state, slot);
        crate::item_frame::restore(state, slot);
        crate::painting::restore(state, slot);
        crate::vehicles::restore(state, slot);
        crate::bees::appearance(state, slot);
        crate::allay::appearance(state, slot);
        crate::special_mobs::restore(state, slot);
        crate::leash::initialize(state, slot);
        if record.state.falling.is_some() {
            crate::falling::configure(state, entity_id);
        }
        if record.state.tnt.is_some() {
            crate::tnt::restore(state, slot);
        }
        let mut projectile = record.projectile.clone();
        if projectile.is_none() {
            // Initial v2 skeleton arrows had only metadata/age. Preserve their
            // stuck state and disallowed pickup rather than minting an item.
            if let Some(kind) = state
                .registry
                .blueprint(record.type_id)
                .and_then(|blueprint| {
                    crate::projectiles::ProjectileKind::from_name(&blueprint.name)
                })
                .filter(|kind| kind.arrow())
            {
                let mut migrated =
                    crate::projectiles::ProjectileState::new(kind, None, mc_data::ItemStack::EMPTY);
                migrated.age = record.age;
                migrated.mob_damage = true;
                migrated.stuck = record.appearance.extra.iter().any(|(index, value)| {
                    *index == crate::projectiles::ARROW_IN_GROUND_INDEX
                        && matches!(value, crate::journal::EntityDataValueImage::Boolean(true))
                });
                projectile = Some(migrated);
            }
        }
        if let Some(projectile) = &mut projectile {
            crate::projectiles::persistence::restore_clock(projectile, state.tick);
        }
        state.entities.row_mut(slot).projectile = projectile.map(Box::new);
        crate::villager::sync_appearance(state, slot);
        let pos = state.entities.pos(slot);
        let chunk = crate::boundary::ChunkPos {
            x: (pos[0].floor() as i32).div_euclid(16),
            z: (pos[2].floor() as i32).div_euclid(16),
        };
        state.journaled_entity_state.insert(
            entity_id,
            (
                state.tick,
                chunk,
                Arc::new(crate::journal::EntityStateImage::from_row(
                    state.entities.body(slot),
                    state.entities.row(slot),
                )),
            ),
        );
        // Seed the villager baseline too: without it the first journal batch
        // re-images every restored villager (500k: ~2.6 GB, a 5.7 s first tick).
        // A row whose live state differs from the restored one still journals.
        if let Some(villager) = &state.entities.row(slot).villager {
            state.journaled_villagers.insert(
                entity_id,
                (
                    state.tick,
                    Box::new(crate::villager::persistence::journal_image(villager)),
                ),
            );
        }
    }
    Ok(())
}

/// Restore an [`EntityAppearance`] from its journal image.
pub(super) fn restore_appearance(
    record: &crate::journal::ReplayEntity,
) -> crate::boundary::EntityAppearance {
    let image = &record.appearance;
    let mut equipment: [Option<mc_data::ItemStack>; 6] = Default::default();
    for (slot, stack) in equipment.iter_mut().zip(image.equipment.iter()) {
        *slot = stack.clone();
    }
    crate::boundary::EntityAppearance {
        item: image.item.clone(),
        experience_orb: image.experience_orb,
        spawn_data: image.spawn_data,
        equipment,
        body_armor: record.state.loot.body_armor.as_deref().cloned(),
        saddle: record.state.loot.saddle.as_deref().cloned(),
        passengers: Vec::new(),
        leash_holder: None,
        baby: image.baby,
        pose: image.pose,
        shared_flags: image.shared_flags,
        health: record.health,
        custom_name: image.custom_name.clone(),
        custom_name_visible: image.custom_name_visible,
        extra: image
            .extra
            .iter()
            .map(|(index, value)| (*index, restore_data_value(value)))
            .collect(),
        effect_particles: Vec::new(),
        effects_ambient: false,
        arrow_count: 0,
        stinger_count: 0,
        absorption: record.active_effects.absorption,
    }
}

/// Restore one [`EntityDataValue`](crate::boundary::EntityDataValue) from its image.
pub(super) fn restore_data_value(
    value: &crate::journal::EntityDataValueImage,
) -> crate::boundary::EntityDataValue {
    match value {
        crate::journal::EntityDataValueImage::WeatheringCopper(v) => {
            crate::boundary::EntityDataValue::WeatheringCopper(*v)
        }
        crate::journal::EntityDataValueImage::CopperGolemState(v) => {
            crate::boundary::EntityDataValue::CopperGolemState(*v)
        }
        crate::journal::EntityDataValueImage::Direction(v) => {
            crate::boundary::EntityDataValue::Direction(*v)
        }
        crate::journal::EntityDataValueImage::PaintingVariant(v) => {
            crate::boundary::EntityDataValue::PaintingVariant(*v)
        }
        crate::journal::EntityDataValueImage::VillagerData {
            kind,
            profession,
            level,
        } => crate::boundary::EntityDataValue::VillagerData {
            kind: *kind,
            profession: *profession,
            level: *level,
        },
        crate::journal::EntityDataValueImage::Byte(v) => crate::boundary::EntityDataValue::Byte(*v),
        crate::journal::EntityDataValueImage::VarInt(v) => {
            crate::boundary::EntityDataValue::VarInt(*v)
        }
        crate::journal::EntityDataValueImage::VarLong(v) => {
            crate::boundary::EntityDataValue::VarLong(*v)
        }
        crate::journal::EntityDataValueImage::Float(v) => {
            crate::boundary::EntityDataValue::Float(*v)
        }
        crate::journal::EntityDataValueImage::Rotations(v) => {
            crate::boundary::EntityDataValue::Rotations(*v)
        }
        crate::journal::EntityDataValueImage::Boolean(v) => {
            crate::boundary::EntityDataValue::Boolean(*v)
        }
        crate::journal::EntityDataValueImage::String(v) => {
            crate::boundary::EntityDataValue::String(v.clone())
        }
        crate::journal::EntityDataValueImage::BlockState(v) => {
            crate::boundary::EntityDataValue::BlockState(*v)
        }
        crate::journal::EntityDataValueImage::Item(v) => {
            crate::boundary::EntityDataValue::Item(v.clone())
        }
        crate::journal::EntityDataValueImage::OptionalBlockPos(v) => {
            crate::boundary::EntityDataValue::OptionalBlockPos(*v)
        }
        crate::journal::EntityDataValueImage::EntityEffectParticle(v) => {
            crate::boundary::EntityDataValue::EntityEffectParticle(*v)
        }
        crate::journal::EntityDataValueImage::OptionalUnsignedInt(v) => {
            crate::boundary::EntityDataValue::OptionalUnsignedInt(*v)
        }
        crate::journal::EntityDataValueImage::OptionalLivingEntityReference(v) => {
            crate::boundary::EntityDataValue::OptionalLivingEntityReference(*v)
        }
    }
}
