//! Player input application, reach checks, respawn and per-tick player upkeep.
//! Moved verbatim from `tick.rs`.

use super::*;

/// Keep the key-down flag distinct from crouching forced by a low ceiling.
pub(super) fn update_player_crouching(
    state: &mut TickState,
    entity: EntityId,
    slot: EntitySlot,
    sneaking: bool,
) {
    // Sleeping, swimming, flying, and other poses keep their own dimensions.
    if state.sleepers.contains_key(&entity) {
        return;
    }
    let row = state.entities.row(slot);
    if !matches!(row.appearance.pose, 0 | CROUCHING_POSE) {
        return;
    }
    let half_width = state.entities.body(slot).half_width;
    let position = state.entities.pos(slot);
    // Expanding a crouching box to standing occupies the same space
    // as sweeping the crouching box upward by the height difference.
    let crouching = sneaking
        || (row.appearance.pose == CROUCHING_POSE
            && !mc_physics::resolve_collision_displacement(
                &TickSampler {
                    world: &state.world,
                    registry: &state.registry,
                },
                mc_physics::Vec3::new(position[0], position[1], position[2]),
                mc_physics::Aabb {
                    half_width,
                    height: 1.5,
                },
                mc_physics::Vec3::new(0.0, 0.3, 0.0),
                false,
                0.0,
            )
            .is_some_and(|resolved| (resolved.delta.y - 0.3).abs() < 1e-7));
    let (body, row) = state.entities.parts_mut(slot);
    body.height = if crouching { 1.5 } else { 1.8 };
    let appearance = Arc::make_mut(&mut row.appearance);
    let flags = if sneaking {
        appearance.shared_flags | SNEAKING_FLAG
    } else {
        appearance.shared_flags & !SNEAKING_FLAG
    };
    let pose = if crouching { CROUCHING_POSE } else { 0 };
    if (flags, pose) != (appearance.shared_flags, appearance.pose) {
        appearance.shared_flags = flags;
        appearance.pose = pose;
        state.appearance_changed.push(entity);
    }
}

/// Eye heights for vanilla player poses, independent of the sneak key.
pub(crate) fn player_eye_height(pose: u8) -> f64 {
    match pose {
        2 => 0.2,         // sleeping
        1 | 3 | 4 => 0.4, // fall flying, swimming, spin attack
        5 => 1.27,        // crouching (including forced crouch)
        _ => 1.62,
    }
}

/// Vanilla's block interaction gate measures eye distance to the block AABB,
/// with the server packet handlers allowing a one-block prediction buffer.
pub(crate) fn within_block_reach(state: &TickState, entity: EntityId, target: BlockPos) -> bool {
    let Some(slot) = state.entities.slot_of(entity) else {
        return false;
    };
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    let position = state.entities.pos(slot);
    let eye_height = player_eye_height(state.entities.row(slot).appearance.pose);
    let eye = [position[0], position[1] + eye_height, position[2]];
    let minimum = [
        f64::from(target.x),
        f64::from(target.y),
        f64::from(target.z),
    ];
    let distance_squared: f64 = eye
        .into_iter()
        .zip(minimum)
        .map(|(coordinate, minimum)| {
            let delta = coordinate - coordinate.clamp(minimum, minimum + 1.0);
            delta * delta
        })
        .sum();
    let range = player
        .effective_attribute("block_interaction_range")
        .unwrap_or(4.5)
        + if player.creative() { 0.5 } else { 0.0 }
        + 1.0;
    distance_squared < range * range
}

/// Vanilla entity interaction range (3, or 5 in creative), plus the packet
/// handler's three-block prediction buffer, measured eye-to-target AABB.
pub(super) fn within_entity_reach(state: &TickState, entity: EntityId, target: EntityId) -> bool {
    let (Some(actor_slot), Some(target_slot), Some(player)) = (
        state.entities.slot_of(entity),
        state
            .entities
            .slot_of(crate::end_fight::attack_target(state, target).0),
        state.players.get(&entity),
    ) else {
        return false;
    };
    let position = state.entities.pos(actor_slot);
    let eye = [
        position[0],
        position[1] + player_eye_height(state.entities.row(actor_slot).appearance.pose),
        position[2],
    ];
    let position = state.entities.pos(target_slot);
    let body = state.entities.body(target_slot);
    let (minimum, maximum) = crate::end_fight::part_bounds(state, target).unwrap_or((
        [
            position[0] - body.half_width,
            position[1],
            position[2] - body.half_width,
        ],
        [
            position[0] + body.half_width,
            position[1] + body.height,
            position[2] + body.half_width,
        ],
    ));
    let distance_squared: f64 = eye
        .into_iter()
        .zip(minimum.into_iter().zip(maximum))
        .map(|(coordinate, (minimum, maximum))| {
            let delta = coordinate - coordinate.clamp(minimum, maximum);
            delta * delta
        })
        .sum();
    let range = player
        .effective_attribute("entity_interaction_range")
        .unwrap_or(3.0)
        + if player.creative() { 2.0 } else { 0.0 }
        + 3.0;
    distance_squared < range * range
}

/// Dig/place/attack/breed emit intents; the rest apply directly.
pub(super) fn apply_inputs<'a>(
    state: &mut TickState,
    inputs: impl IntoIterator<Item = &'a PlayerInput>,
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    for input in inputs {
        if let PlayerInput::BuildPreviewFence { fence } = input {
            input::interact::build_preview_fence(fence);
            continue;
        }
        if auth::refuse_input(state, input) {
            continue;
        }
        if input.is_communication()
            && !input::social::admit_communication(state, input.connection())
        {
            continue;
        }
        if let PlayerInput::AuthorizedBuild { authorization } = input {
            crate::build_precommit::apply_authorized(state, authorization, intents);
            continue;
        }
        if let PlayerInput::BuildResync { request } = input {
            crate::build_precommit::apply_resync(state, request);
            continue;
        }
        if crate::build_precommit::requires_approval(state, input) {
            crate::build_precommit::resync_input(state, input);
            continue;
        }
        crate::plugin::observe_input(state, input);
        let (handled, event) = crate::plugin_player::menu_input(state, input);
        crate::plugin::enqueue_result(state, event);
        if handled {
            continue;
        }
        if let PlayerInput::PluginClientReady { connection } = input {
            input::session::plugin_client_ready(state, connection);
            continue;
        }
        if let PlayerInput::LoaderViewRequest {
            connection,
            owner,
            request_kind,
        } = input
        {
            crate::plugin_player::loader_view_request(
                state,
                *connection,
                owner,
                request_kind.clone(),
            );
            continue;
        }
        match input {
            PlayerInput::BuildPreviewFence { .. }
            | PlayerInput::AuthorizedBuild { .. }
            | PlayerInput::BuildResync { .. } => {}
            PlayerInput::SeenAdvancements { connection, tab } => {
                input::social::seen_advancements(state, (connection, tab))
            }
            PlayerInput::PluginClientReady { .. } => {}
            PlayerInput::LoaderViewRequest { .. } => {}
            PlayerInput::PermissionLevel { connection, level } => {
                input::session::permission_level(state, (connection, level))
            }
            PlayerInput::SpawnProtectionOperators { operators } => {
                state.spawn_operators = operators.iter().copied().collect();
            }
            PlayerInput::SetViewDistance {
                connection,
                view_distance,
            } => input::session::set_view_distance(state, (connection, view_distance)),
            PlayerInput::SetLocale { connection, locale } => {
                input::session::set_locale(state, (connection, locale))
            }
            PlayerInput::Join {
                connection,
                uuid,
                name,
                permission_level,
            } => input::session::join(state, (connection, uuid, name, permission_level), changes),
            PlayerInput::Leave { connection } => input::session::leave(state, connection, changes),
            PlayerInput::RequestStats { connection } => {
                input::social::request_stats(state, connection)
            }
            PlayerInput::ConfirmTeleport {
                connection,
                teleport_id,
            } => input::session::confirm_teleport(state, (connection, teleport_id)),
            PlayerInput::Move { connection, .. }
            | PlayerInput::MoveRotation { connection, .. }
            | PlayerInput::MoveStatus { connection, .. } => {
                input::movement::move_input(state, connection, input, intents)
            }
            PlayerInput::SelectSlot { connection, slot } => {
                input::inventory::select_slot(state, (connection, slot))
            }
            PlayerInput::PickItemFromBlock {
                connection,
                position,
            } => input::inventory::pick_item_from_block(state, (connection, position), changes),
            PlayerInput::PickItemFromEntity { connection, target } => {
                input::inventory::pick_item_from_entity(state, (connection, target), changes)
            }
            PlayerInput::Dig {
                connection,
                position,
                action,
                sequence,
            } => input::interact::dig(
                state,
                (connection, position, action, sequence),
                intents,
                changes,
            ),
            PlayerInput::UseItemOn {
                connection,
                position,
                hand,
                face,
                cursor,
                sequence,
            } => input::interact::use_item_on(
                state,
                (connection, position, hand, face, cursor, sequence),
                input,
                intents,
                changes,
            ),
            PlayerInput::Attack { connection, target } => {
                input::combat::attack(state, (connection, target), intents, changes)
            }
            PlayerInput::InteractEntity {
                connection,
                target,
                hand,
                location,
            }
            | PlayerInput::InteractEntitySecondary {
                connection,
                target,
                location,
                hand,
            } => input::interact::interact_entity(
                state,
                (connection, target, hand, location),
                input,
                intents,
                changes,
            ),
            PlayerInput::ContainerClick {
                connection,
                window,
                state_id,
                slot,
                button,
                mode,
            } => input::inventory::container_click(
                state,
                (connection, window, state_id, slot, button, mode),
                changes,
            ),
            PlayerInput::SelectTrade { connection, offer } => {
                input::inventory::select_trade(state, (connection, offer), changes)
            }
            PlayerInput::ContainerSlotState {
                connection,
                window,
                slot,
                enabled,
            } => input::inventory::container_slot_state(state, (connection, window, slot, enabled)),
            PlayerInput::ContainerButton {
                connection,
                window,
                button,
            } => input::inventory::container_button(state, (connection, window, button), changes),
            PlayerInput::SetBeacon {
                connection,
                primary,
                secondary,
            } => crate::beacon::configure(state, *connection, *primary, *secondary),
            PlayerInput::RenameItem { connection, name } => {
                input::inventory::rename_item(state, (connection, name), changes)
            }
            PlayerInput::ContainerClose { connection, .. } => {
                input::inventory::container_close(state, connection, changes)
            }
            PlayerInput::PlaceRecipe {
                connection,
                window,
                recipe,
                make_all,
            } => input::inventory::place_recipe(
                state,
                (connection, window, recipe, make_all),
                changes,
            ),
            PlayerInput::SwapHands { connection } => {
                input::inventory::swap_hands(state, connection, changes)
            }
            PlayerInput::CreativeSlot {
                connection,
                slot,
                stack,
            } => input::inventory::creative_slot(state, (connection, slot, stack), changes),
            PlayerInput::Sneak {
                connection,
                sneaking,
            } => input::movement::sneak(state, (connection, sneaking)),
            PlayerInput::VehicleInput { connection, flags } => {
                input::movement::vehicle_input(state, (connection, flags))
            }
            PlayerInput::MoveVehicle {
                connection,
                position,
                yaw,
                pitch,
                on_ground,
            } => {
                input::movement::move_vehicle(state, (connection, position, yaw, pitch, on_ground))
            }
            PlayerInput::Sprint {
                connection,
                sprinting,
            } => input::movement::sprint(state, (connection, sprinting)),
            PlayerInput::StartFallFlying { connection } => elytra::start(state, *connection),
            PlayerInput::EditBook {
                connection,
                slot,
                pages,
                title,
            } => {
                crate::books::edit(state, *connection, *slot, pages, title.as_deref());
            }
            PlayerInput::SignUpdate {
                connection,
                position,
                front,
                lines,
            } => input::social::sign_update(state, (connection, position, front, lines)),
            PlayerInput::DropSelected {
                connection,
                whole_stack,
            } => input::inventory::drop_selected(state, (connection, whole_stack), changes),
            PlayerInput::UseItem {
                connection,
                hand,
                yaw,
                pitch,
                sequence,
            } => input::interact::use_item(
                state,
                (connection, hand, yaw, pitch, sequence),
                intents,
                changes,
            ),
            PlayerInput::ReleaseUseItem { connection } => {
                input::interact::release_use_item(state, connection)
            }
            PlayerInput::Chat {
                connection,
                message,
            } => input::social::chat(state, (connection, message)),
            PlayerInput::DialogClick {
                connection,
                id,
                payload,
            } => crate::dialog::route_click(state, *connection, id, payload.as_ref(), changes),
            PlayerInput::Command {
                connection,
                command,
                permission_level,
            } => {
                crate::command_console::execute(
                    state,
                    *connection,
                    command,
                    *permission_level,
                    changes,
                );
            }
            PlayerInput::ConsoleCommand(command) => {
                command.execute(state, changes);
            }
            #[cfg(test)]
            PlayerInput::TestGive {
                connection,
                slot,
                stack,
            } => input::inventory::test_give(state, (connection, slot, stack), changes),
            PlayerInput::Respawn { connection } => {
                input::session::respawn(state, connection, changes)
            }
            PlayerInput::LeaveBed { connection } => input::movement::leave_bed(state, connection),
            PlayerInput::OpenMountInventory { connection } => {
                if let Some(actor) = state.connections.entity_of(*connection)
                    && let Some(vehicle) = crate::mounts::vehicle(state, actor)
                {
                    let target = state.entities.body(vehicle).id;
                    crate::mounts::interact(
                        state,
                        actor,
                        target,
                        crate::boundary::Hand::Main,
                        true,
                        intents,
                    );
                }
            }
        }
    }
}

/// Shared manual/immediate respawn transition; live rows cannot respawn twice.
pub(crate) fn respawn_player(
    state: &mut TickState,
    connection: ConnectionId,
    changes: &mut ChangeSet,
) -> bool {
    let Some(entity) = state.connections.by_connection.get(&connection).copied() else {
        return true;
    };
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return true;
    };
    if state.entities.body(slot).alive {
        return true;
    }
    crate::mounts::reset_player(state, entity);
    let destination =
        state
            .players
            .get(&entity)
            .map_or(crate::dimensions::Dimension::Overworld, |player| {
                if player.respawn_point.is_some() {
                    player.respawn_dimension
                } else {
                    crate::dimensions::Dimension::Overworld
                }
            });
    if destination != state.dimension {
        state.dimension_respawns.push(entity);
        return true;
    }
    if !crate::sleep::respawn_chunks_loaded(state, entity) {
        return false;
    }
    let personal = crate::sleep::respawn_position(state, entity);
    if personal.is_none() {
        if let Some(player) = state.players.get_mut(&entity) {
            if player.respawn_point.take().is_some() {
                state
                    .outbox
                    .entry(connection)
                    .or_default()
                    .push(PlayerChange::Chat {
                        message:
                            "You have no home bed or charged respawn anchor, or it was obstructed"
                                .to_owned(),
                    });
            }
            player.respawn_dimension = crate::dimensions::Dimension::Overworld;
        }
        if state.dimension != crate::dimensions::Dimension::Overworld {
            state.dimension_respawns.push(entity);
            return true;
        }
    }
    let [yaw, pitch] = crate::sleep::respawn_rotation(state, entity);
    if personal.is_some()
        && let Some([x, y, z]) = state
            .players
            .get(&entity)
            .and_then(|player| player.respawn_point)
        && let Some(current) = state.world.block_at(x, y, z)
        && let Some(charges) = crate::dimensions::rules::anchor_charge(state, current)
        && charges > 0
        && let Some(next) = crate::dimensions::rules::anchor_state(state, current, charges - 1)
    {
        let position = BlockPos { x, y, z };
        state.world.set_block(x, y, z, next);
        changes.blocks.push(crate::boundary::BlockChange {
            position,
            state: next,
        });
        for cell in crate::plants::neighbours(position) {
            crate::redstone::schedule_cell(state, cell, 1, 0);
        }
    }
    crate::sleep::wake(state, entity);
    let position = personal.unwrap_or_else(|| {
        crate::natural_spawn::player_spawn(state, u64::from(entity.0 as u32) ^ state.tick)
    });
    let (body, row) = state.entities.parts_mut(slot);
    body.alive = true;
    body.burn_ticks = 0;
    row.health = 20.0;
    row.protected_until = state.tick + crate::entities::SPAWN_INVULNERABLE_TICKS;
    row.embedded_projectile_timers = [0; 2];
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.health = 20.0;
    appearance.arrow_count = 0;
    appearance.stinger_count = 0;
    appearance.shared_flags &= !1;
    state.appearance_changed.push(entity);
    state.entities.set_pos(slot, position);
    state.entities.set_rot(slot, yaw, pitch);
    state.entities.set_vel(slot, [0.0; 3]);
    let effects = state
        .players
        .get_mut(&entity)
        .map(|player| player.respawn(position, yaw, pitch, state.dimension))
        .unwrap_or_default();
    apply_player_effects(state, connection, entity, effects, changes);
    crate::bossbar::sync_dimension(state, connection);
    true
}

/// Live player bodies in dense-slot order. A player-kind row without a
/// `players` entry has no tick work, so enumerate the entries instead of
/// reading every resident row; stale entries fail the kind/alive gate.
pub(crate) fn live_players(state: &TickState) -> Vec<(EntitySlot, EntityId, ConnectionId)> {
    let mut found: Vec<(EntitySlot, EntityId, ConnectionId)> = state
        .players
        .keys()
        .filter_map(|id| {
            let slot = state.entities.slot_of(*id)?;
            let body = state.entities.body(slot);
            match body.kind {
                EntityKind::Player { connection } if body.alive => {
                    Some((slot, body.id, connection))
                }
                _ => None,
            }
        })
        .collect();
    found.sort_unstable_by_key(|(slot, ..)| slot.0);
    found
}

/// The previous dense scan, kept as the differential oracle for `live_players`.
#[cfg(test)]
pub(crate) fn live_players_by_scan(state: &TickState) -> Vec<(EntitySlot, EntityId, ConnectionId)> {
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            match body.kind {
                EntityKind::Player { connection } if body.alive => {
                    Some((slot, body.id, connection))
                }
                _ => None,
            }
        })
        .collect()
}

/// Per-tick player maintenance: eating progress, vitals (regen, starvation,
/// drowning). Damage routes through merge like combat; deaths produce drops +
/// Died via commit.
pub(super) fn tick_players(
    state: &mut TickState,
    changes: &mut ChangeSet,
    intents: &mut Vec<Intent>,
) {
    let difficulty = state.difficulty;
    let natural_regeneration = crate::command_rules::boolean(
        state,
        crate::command_rules::BoolRule::NATURAL_HEALTH_REGENERATION,
    );
    crate::statistics::tick(state, state.tick + 1);
    // Snapshot player entities to avoid borrow issues.
    let players = live_players(state);
    for (_, entity, connection) in players {
        if state.session_locked(connection) {
            continue;
        }
        let Some(slot) = state.entities.slot_of(entity) else {
            continue;
        };
        elytra::tick(state, entity);
        // Eating progress.
        let consumable_refused = state
            .players
            .get(&entity)
            .and_then(|player| player.finishing_consumable_debit())
            .is_some_and(|debit| {
                crate::plugin_inventories::below_player_debit_floors(state, entity, &[debit], None)
            });
        let consumable_event = (!consumable_refused)
            .then(|| {
                state.players.get(&entity).and_then(|player| {
                    player.finishing_consumable_event(&state.registry.item_rules())
                })
            })
            .flatten();
        let effects = if consumable_refused {
            crate::player::PlayerEffects::default()
        } else {
            let rules = state.registry.item_rules();
            state
                .players
                .get_mut(&entity)
                .map(|player| player.tick_item_use(&rules))
                .unwrap_or_default()
        };
        if let Some(event) = consumable_event {
            let pos = state.entities.pos(slot);
            crate::vibration::emit(state, pos, event, Some(entity));
        }
        if let Some(item) = &effects.consumed_item {
            let item = crate::advancements::item_snapshot(state, item);
            crate::advancements::trigger(
                state,
                entity,
                "minecraft:consume_item",
                serde_json::json!({"item":item}),
            );
            crate::effects::sync_appearance(state, entity);
        }
        // Apply eating effects inline (drops spawn immediately).
        if effects.chorus_consumed {
            crate::consumables::finish_chorus(state, entity);
        }
        for change in effects.changes {
            state.push_player(connection, change);
        }
        let pos = state.entities.pos(slot);
        for stack in effects.dropped {
            let id = spawn_item_entity(state, pos, stack);
            changes.entities_spawned.push(id);
        }
        // Eye-in-water for drowning.
        let pose = state.entities.row(slot).appearance.pose;
        let eye_in_water = crate::fluids::water_at_point(
            state,
            [pos[0], pos[1] + player_eye_height(pose), pos[2]],
        );
        let vitals_tick = state
            .players
            .get_mut(&entity)
            .map(|player| {
                let can_drown = !player.creative()
                    && !player.spectator()
                    && !player
                        .active_effects
                        .has(crate::effects::EffectId::WaterBreathing)
                    && !player
                        .active_effects
                        .has(crate::effects::EffectId::ConduitPower)
                    && !player
                        .active_effects
                        .has(crate::effects::EffectId::BreathOfTheNautilus);
                let consume_air = !eye_in_water
                    || !can_drown
                    || crate::enchantments::consumes_air(
                        &player.inventory.slots[39],
                        &mut player.enchantment_seed,
                    );
                let max_health = player.effective_attribute("max_health").unwrap_or(20.0) as f32;
                player.vitals.tick_with_max_health(
                    difficulty,
                    eye_in_water,
                    can_drown,
                    consume_air,
                    natural_regeneration,
                    max_health,
                )
            })
            .unwrap_or_default();
        if vitals_tick.air_changed
            && let Some(player) = state.players.get(&entity)
        {
            let change = player.air_change();
            state.push_player(connection, change);
        }
        // Vitals damage through merge (footprinted on the player).
        for (kind, amount) in vitals_tick.damage {
            let seq = state.next_seq(entity);
            intents.push(Intent {
                actor: entity,
                seq,
                kind: IntentKind::DamageEntity {
                    target: entity,
                    amount,
                    kind,
                    source: None,
                },
                footprint: Footprint {
                    entity_reads: vec![entity],
                    entity_writes: vec![entity],
                    ..Default::default()
                },
            });
        }
        if vitals_tick.health_changed {
            let (health, food, saturation) = state
                .players
                .get(&entity)
                .map(|player| {
                    (
                        player.vitals.health,
                        player.vitals.food,
                        player.vitals.saturation,
                    )
                })
                .unwrap_or((20.0, 20, 5.0));
            state.push_player(
                connection,
                PlayerChange::Health {
                    health,
                    food: food.clamp(0, 20) as u8,
                    saturation,
                },
            );
        }
    }
}
