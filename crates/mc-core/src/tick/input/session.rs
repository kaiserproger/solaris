use crate::boundary::{ChangeSet, ConnectionId, EntityAppearance, PlayerChange, PlayerInput};
use crate::entities::{ColdFields, EntityKind};
use crate::tick::{
    TickState, apply_player_effects, hydrate_player, respawn_player, spawn_item_entity,
};
use mc_data::statistics::CustomStat;
use std::sync::Arc;

pub(in crate::tick) fn plugin_client_ready(state: &mut TickState, connection: &ConnectionId) {
    crate::plugin_player::note_loader_ready(state, *connection);
}

pub(in crate::tick) fn permission_level(
    state: &mut TickState,
    (connection, level): (&ConnectionId, &u8),
) {
    if state.connections.entity_of(*connection).is_some() {
        state.plugin_players.observe_permission(*connection, *level);
    }
}

pub(in crate::tick) fn set_view_distance(
    state: &mut TickState,
    (connection, view_distance): (&ConnectionId, &u8),
) {
    // Pre-Join friendly: the edge submits the negotiated radius before Join,
    // so it must record even with no player entity yet. Dimension travel
    // moves it with the connection; Leave clears it.
    state
        .connections
        .set_view_distance(*connection, *view_distance);
}

pub(in crate::tick) fn set_locale(
    state: &mut TickState,
    (connection, locale): (&ConnectionId, &String),
) {
    // Pre-Join friendly, like the view radius: the edge records the configured
    // locale before Join and every Play ClientInformation updates it in place,
    // so the next dialog this connection opens reads the new language.
    state.connections.set_locale(*connection, locale.clone());
}

/// Vanilla's yellow `multiplayer.player.joined` / `multiplayer.player.left`
/// announcement, delivered reliably to every joined connection and relayed
/// across local dimensions.
pub(crate) fn announce_player(
    state: &mut TickState,
    translate: &str,
    name: &str,
    skip: Option<ConnectionId>,
) {
    use mc_nbt::{ListTag, Tag, tag_type};
    let content = Tag::Compound(vec![
        ("translate".to_owned(), Tag::String(translate.to_owned())),
        (
            "with".to_owned(),
            Tag::List(ListTag {
                element_type: tag_type::STRING,
                elements: vec![Tag::String(name.to_owned())],
            }),
        ),
        ("color".to_owned(), Tag::String("yellow".to_owned())),
    ]);
    let recipients: Vec<ConnectionId> = state.connections.all_connections().collect();
    for recipient in recipients {
        if skip == Some(recipient) {
            continue;
        }
        state.push_player(
            recipient,
            PlayerChange::StyledChat {
                content: content.clone(),
            },
        );
    }
    if state.dimension_globals.active {
        state
            .dimension_globals
            .broadcasts
            .push(crate::dimensions::globals::Broadcast::Chat(content));
    }
}

/// Vanilla `multiplayer.disconnect.duplicate_login`.
const DUPLICATE_LOGIN: &str = "You logged in from another location";

pub(in crate::tick) fn join(
    state: &mut TickState,
    (connection, uuid, name, permission_level): (&ConnectionId, &uuid::Uuid, &String, &u8),
    changes: &mut ChangeSet,
) {
    if state.connections.by_connection.contains_key(connection) {
        return;
    }
    // Vanilla duplicate login: the newer login wins. The incumbent's ordinary
    // Leave capture (menus closed, image saved) is restored below, so its
    // inventory moves rather than copies. An unverified plugin-login claimant
    // never kicks a verified session; authentication arbitrates those.
    if !state.require_plugin_login
        && let Some(incumbent) = state
            .connections
            .uuids
            .iter()
            .find_map(|(other, joined)| (joined == uuid).then_some(*other))
    {
        leave(state, &incumbent, changes);
        state.push_player(
            incumbent,
            PlayerChange::Disconnect {
                reason: DUPLICATE_LOGIN.to_owned(),
            },
        );
        state.disconnects.push((incumbent, state.tick));
    }
    if state.entities.slot_of_uuid(*uuid).is_some()
        || state
            .connections
            .uuids
            .iter()
            .any(|(incumbent, joined)| joined == uuid && !state.session_locked(*incumbent))
    {
        state.push_player(
            *connection,
            PlayerChange::Disconnect {
                reason: "UUID is already joined".to_owned(),
            },
        );
        state.disconnects.push((*connection, state.tick));
        return;
    }
    let id = state.entities.alloc_id();
    // Unverified claimants never take custody of the saved player. Multiple
    // claimants may authenticate; native unlock chooses one live owner.
    let offline = if state.require_plugin_login {
        None
    } else {
        state.offline_players.remove(uuid)
    };
    let attributes = match offline
        .as_ref()
        .map(|record| {
            crate::entity_loot::player_attributes_from_opaque(&record.opaque_player_fields)
        })
        .transpose()
    {
        Ok(attributes) => attributes.unwrap_or_default(),
        Err(error) => {
            if let Some(record) = offline {
                state.offline_players.insert(*uuid, record);
            }
            state.push_player(
                *connection,
                PlayerChange::Disconnect {
                    reason: format!("Invalid player attributes: {error}"),
                },
            );
            state.disconnects.push((*connection, state.tick));
            return;
        }
    };
    let spawn = offline
        .as_ref()
        .map(|record| record.pos)
        .unwrap_or_else(|| {
            crate::natural_spawn::player_spawn(state, u64::from(id.0 as u32) ^ state.tick)
        });
    let spawn_rot = offline
        .as_ref()
        .map(|record| (record.yaw, record.pitch))
        .unwrap_or((0.0, 0.0));
    let fall_flying = offline.as_ref().is_some_and(|record| record.fall_flying);
    let appearance = Arc::new(EntityAppearance {
        health: offline.as_ref().map(|record| record.health).unwrap_or(20.0),
        custom_name: Some(name.clone()),
        pose: if fall_flying { 1 } else { 0 },
        shared_flags: if fall_flying { 0x80 } else { 0 },
        ..Default::default()
    });
    let slot = state.entities.spawn(
        id,
        if state.require_plugin_login {
            uuid::Uuid::from_u128((u128::from(connection.0) << 64) | 0x534f_4c41_5249_5300)
        } else {
            *uuid
        },
        EntityKind::Player {
            connection: *connection,
        },
        ColdFields {
            pos: spawn,
            vel: [0.0; 3],
            yaw: spawn_rot.0,
            pitch: spawn_rot.1,
            ground: !fall_flying,
            health: 20.0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance,
            movement: crate::blueprint::MovementClass::Player,
            half_width: 0.3,
            height: if fall_flying { 0.6 } else { 1.8 },
            speed: 4.3,
            attack_damage: 1.0,
            sight_range: 0.0,
            attack_reach: 3.0,
        },
    );
    state.entities.row_mut(slot).loot.attributes = attributes;
    state.entities.row_mut(slot).protected_until =
        state.tick + crate::entities::SPAWN_INVULNERABLE_TICKS;
    state
        .connections
        .insert(*connection, id, *uuid, name.clone());
    state
        .players
        .insert(id, hydrate_player(*connection, offline.as_ref()));
    if state.require_plugin_login {
        state.locked_sessions.insert(*connection);
        // Empty, nonpersistent authentication presentation.
    }
    if let Some(player) = state.players.get_mut(&id) {
        player.statistics.advance_tick(state.tick);
        player
            .statistics
            .set_active(true, !player.vitals.is_dead(), false, false);
        player.warden_warning.advance_tick(state.tick);
        player.warden_warning.set_active(true);
    }
    crate::mounts::join(state, id);
    if let Some(record) = offline.as_ref() {
        crate::contact_hazards::ignite(state, id, record.burn_ticks);
        state.entities.row_mut(slot).entity_life.portal = record.portal;
    }
    // Custody transaction on live state: cursor + grid stacks go
    // back through the real `inventory.add`; whatever does not
    // fit spawns as item entities at the player (journaled like
    // any drop). Nothing is merged or lost.
    if !state.session_locked(*connection)
        && offline
            .as_ref()
            .is_some_and(|record| !record.carried.is_empty() || !record.grid.is_empty())
    {
        let record = offline.as_ref().expect("checked");
        let mut returning = vec![record.carried.clone()];
        returning.extend(record.grid.iter().cloned());
        let rules = state.registry.item_rules();
        let mut leftover = Vec::new();
        if let Some(player) = state.players.get_mut(&id) {
            for mut stack in returning {
                if stack.is_empty() {
                    continue;
                }
                player.inventory.add(&rules, &mut stack);
                if !stack.is_empty() {
                    leftover.push(stack);
                }
            }
        }
        for stack in leftover {
            let item = spawn_item_entity(state, spawn, stack);
            changes.entities_spawned.push(item);
        }
    }
    state.outbox.insert(*connection, Vec::new());
    state
        .plugin_players
        .observe_permission(*connection, *permission_level);
    let game_mode = state
        .players
        .get(&id)
        .map_or(0, |player| player.game_mode as u8);
    state
        .players
        .get_mut(&id)
        .expect("joined player")
        .begin_teleport();
    state.push_player(
        *connection,
        PlayerChange::Joined {
            entity: id,
            dimension: state.dimension,
            position: state.entities.pos(slot),
            yaw: spawn_rot.0,
            pitch: spawn_rot.1,
            game_mode,
        },
    );
    crate::scoreboard::sync_join(state, *connection);
    crate::tick_control::sync_join(state, *connection);
    crate::bossbar::sync_join(state, *connection);
    crate::effects::sync_join(state, id);
    crate::advancements::sync_join(state, id);
    crate::dialog::probe::offer(state, *connection);
    let raining = state.weather.0 != crate::commands::Weather::Clear;
    state.push_player(
        *connection,
        PlayerChange::WeatherChanged {
            raining,
            rain_level: if raining { 1.0 } else { 0.0 },
            thunder_level: if state.weather.0 == crate::commands::Weather::Thunder {
                1.0
            } else {
                0.0
            },
        },
    );
    if state.difficulty != crate::survival::Difficulty::Easy {
        state.push_player(
            *connection,
            PlayerChange::DifficultyChanged {
                difficulty: state.difficulty.id(),
            },
        );
    }
    // The edge's join burst assumes survival: a restored other
    // mode must reach the client like a `/gamemode` change.
    if game_mode != 0 {
        state.push_player(*connection, PlayerChange::GameModeChanged { game_mode });
    }
    let burst = state.players.get(&id).map(|player| {
        (
            player.inventory_contents(),
            player.health_change(),
            player.air_change(),
            player.experience_change(),
        )
    });
    if let Some((inventory, health, air, experience)) = burst {
        state.push_player(*connection, inventory);
        state.push_player(*connection, health);
        state.push_player(*connection, air);
        state.push_player(*connection, experience);
    }
    // A player who left dead rejoins dead (vanilla death screen):
    // an alive row at 0 health would act, e.g. pick up its own
    // death drop, before respawning.
    if offline.as_ref().is_some_and(|record| record.health <= 0.0)
        && let Some(slot) = state.entities.slot_of(id)
    {
        let (body, row) = state.entities.parts_mut(slot);
        body.alive = false;
        row.health = 0.0;
        if crate::command_rules::boolean(state, crate::command_rules::BoolRule::IMMEDIATE_RESPAWN) {
            respawn_player(state, *connection, changes);
        } else if crate::command_rules::boolean(
            state,
            crate::command_rules::BoolRule::SHOW_DEATH_MESSAGES,
        ) {
            state.push_player(
                *connection,
                PlayerChange::Died {
                    message: mc_nbt::Tag::String(String::new()),
                },
            );
        }
    }
    changes.entities_spawned.push(id);
    if !state.session_locked(*connection) {
        announce_player(state, "multiplayer.player.joined", name, None);
    }
}

pub(in crate::tick) fn leave(
    state: &mut TickState,
    connection: &ConnectionId,
    changes: &mut ChangeSet,
) {
    // Disconnect closes transient menus before freezing custody.
    // Return only real inputs/cursor; result slots are projections.
    if let Some(entity) = state.connections.entity_of(*connection) {
        if let Some(player) = state.players.get_mut(&entity) {
            player.statistics.increment_custom(CustomStat::LeaveGame, 1);
            player.statistics.advance_tick(state.tick);
            player.statistics.set_active(false, false, false, false);
            player.warden_warning.set_active(false);
        }
        if !state.session_locked(*connection) {
            let rules = state.registry.item_rules();
            let effects = state
                .players
                .get_mut(&entity)
                .map(|player| player.close(&rules))
                .unwrap_or_default();
            apply_player_effects(state, *connection, entity, effects, changes);
        }
        state.open_containers.remove(&entity);
    }
    // Vanilla announces a departure to everyone still connected; the leaver's
    // own connection is closing, so it is the one recipient that is skipped.
    let leaving_name = state
        .connections
        .entity_of(*connection)
        .and_then(|entity| state.connections.name_of(entity))
        .filter(|_| !state.session_locked(*connection))
        .map(str::to_owned);
    // Capture the final player image BEFORE removing the live
    // player: Join restores only from `offline_players`, so a
    // disconnect/reconnect without this image resets inventory,
    // custody, position and XP.
    let uuid = state.connections.uuid_of(*connection);
    if let (Some(uuid), Some(entity)) = (
        uuid,
        state.connections.by_connection.get(connection).copied(),
    ) && let Some(record) = crate::core::player_record_for(state, entity, state.tick + 1)
    {
        state.offline_players.insert(uuid, record);
        state.left_players.push(uuid);
    }
    state.locked_sessions.remove(connection);
    // A disconnected editor releases its sign (vanilla SignBlockEntity
    // clears editors whose player is missing); other players can edit it.
    state.sign_editors.remove(connection);
    if let Some(entity) = state.connections.remove_by_connection(*connection) {
        state.sleepers.remove(&entity);
        // `slot_of_any`: a player who leaves while dead must
        // not leak its row (chunks kept resident, corpse kept).
        if let Some(slot) = state.entities.slot_of_any(entity) {
            state.remove_entity(slot);
        }
        state.outbox.remove(connection);
        changes.entities_removed.push(entity);
    }
    if let Some(name) = leaving_name {
        announce_player(state, "multiplayer.player.left", &name, Some(*connection));
    }
}

pub(in crate::tick) fn confirm_teleport(
    state: &mut TickState,
    (connection, teleport_id): (&ConnectionId, &i32),
) {
    if let Some(entity) = state.connections.entity_of(*connection)
        && let Some(player) = state.players.get_mut(&entity)
        && player.awaiting_teleport == Some(*teleport_id)
    {
        player.awaiting_teleport = None;
    }
}

pub(in crate::tick) fn respawn(
    state: &mut TickState,
    connection: &ConnectionId,
    changes: &mut ChangeSet,
) {
    if !respawn_player(state, *connection, changes)
        && !state.deferred_uses.iter().any(|input| {
            matches!(
                input, PlayerInput::Respawn { connection: queued } if queued == connection
            )
        })
    {
        state.deferred_uses.push(PlayerInput::Respawn {
            connection: *connection,
        });
    }
}
