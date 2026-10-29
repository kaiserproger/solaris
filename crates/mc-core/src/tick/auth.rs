//! The native session fence runs before gameplay, menus and plugin observation.
use super::{TickState, apply_player_effects};
use crate::boundary::{ChangeSet, ConnectionId, PlayerInput};

pub(super) fn refuse_input(state: &mut TickState, input: &PlayerInput) -> bool {
    let connection = input.connection();
    if !state.session_locked(connection) {
        return false;
    }
    match input {
        // Connection lifecycle and presentation acknowledgements are not gameplay.
        PlayerInput::Join { .. }
        | PlayerInput::Leave { .. }
        | PlayerInput::PluginClientReady { .. }
        | PlayerInput::PermissionLevel { .. }
        | PlayerInput::SpawnProtectionOperators { .. }
        | PlayerInput::SetViewDistance { .. }
        | PlayerInput::SetLocale { .. }
        | PlayerInput::ConfirmTeleport { .. }
        | PlayerInput::BuildResync { .. }
        | PlayerInput::BuildPreviewFence { .. } => return false,
        PlayerInput::Command {
            command,
            permission_level,
            ..
        } => {
            let root = command
                .trim_start()
                .trim_start_matches('/')
                .split_whitespace()
                .next();
            if matches!(root, Some("register" | "login" | "changepassword")) {
                // route_command itself requires a declared plugin root. Never fall
                // through to native commands, echo text or log credential arguments.
                crate::plugin::route_command(state, connection, command, *permission_level);
            } else {
                refuse_with_hint(state, connection);
            }
        }
        PlayerInput::Chat { .. } => refuse_with_hint(state, connection),
        // Fence only a real displacement, once per teleport. The client answers every
        // teleport with accept + its pose, so correcting each input (or an echoed
        // pose) is a ping-pong that floods the connection until a queue overflows.
        PlayerInput::Move { position, .. } | PlayerInput::MoveVehicle { position, .. } => {
            if let Some(entity) = state.connections.entity_of(connection)
                && state
                    .players
                    .get(&entity)
                    .is_some_and(|player| player.awaiting_teleport.is_none())
                && let Some(slot) = state.entities.slot_of_any(entity)
            {
                let home = state.entities.pos(slot);
                let drift: f64 = (0..3).map(|i| (position[i] - home[i]).powi(2)).sum();
                // NaN drift compares false; a non-finite pose is fenced like any other.
                if drift > 1e-6 || !drift.is_finite() {
                    let [yaw, pitch, _] = state.entities.rot(slot);
                    state.correct_position(connection, home, yaw, pitch);
                }
            }
        }
        // Rotation/status carry no position, so nothing can diverge.
        PlayerInput::MoveRotation { .. } | PlayerInput::MoveStatus { .. } => {}
        _ => crate::build_precommit::resync_input(state, input),
    }
    true
}

/// Locked sessions get one fixed line, never an echo of what they typed.
fn refuse_with_hint(state: &mut TickState, connection: ConnectionId) {
    state.push_player(
        connection,
        crate::boundary::PlayerChange::Chat {
            message: LOCKED_HINT.into(),
        },
    );
}

const LOCKED_HINT: &str = "Сначала войдите: /login <пароль> или /register <пароль> <пароль> / \
    Log in first: /login <password> or /register <password> <password>";

/// Commit the first verified claim, then restore private saved state and finish
/// cursor/grid custody. Locked presentations never replace durable player data.
pub(crate) fn unlock_session(state: &mut TickState, connection: ConnectionId) {
    if !state.require_plugin_login
        || !state.locked_sessions.contains(&connection)
        || state
            .disconnects
            .iter()
            .any(|(closing, _)| *closing == connection)
    {
        return;
    }
    let Some(entity) = state.connections.entity_of(connection) else {
        return;
    };
    let Some(uuid) = state.connections.uuid_of(connection) else {
        return;
    };
    if state.connections.uuids.iter().any(|(other, claimed)| {
        *other != connection && *claimed == uuid && !state.session_locked(*other)
    }) {
        state.push_player(
            connection,
            crate::boundary::PlayerChange::Disconnect {
                reason: "UUID is already authenticated".into(),
            },
        );
        state.disconnects.push((connection, state.tick));
        return;
    }
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return;
    };
    state.locked_sessions.remove(&connection);
    state.entities.set_uuid(slot, uuid);
    let competing: Vec<_> = state
        .connections
        .uuids
        .iter()
        .filter_map(|(other, claimed)| (*other != connection && *claimed == uuid).then_some(*other))
        .collect();
    for other in competing {
        state.push_player(
            other,
            crate::boundary::PlayerChange::Disconnect {
                reason: "Another session authenticated this identity".into(),
            },
        );
        state.disconnects.push((other, state.tick));
    }
    if let Some(record) = state.offline_players.remove(&uuid) {
        let attributes =
            match crate::entity_loot::player_attributes_from_opaque(&record.opaque_player_fields) {
                Ok(attributes) => attributes,
                Err(error) => {
                    state.offline_players.insert(uuid, record);
                    state.locked_sessions.insert(connection);
                    state.push_player(
                        connection,
                        crate::boundary::PlayerChange::Disconnect {
                            reason: format!("Invalid player attributes: {error}"),
                        },
                    );
                    state.disconnects.push((connection, state.tick));
                    return;
                }
            };
        state.entities.row_mut(slot).loot.attributes = attributes;
        let mut player = super::hydrate_player(connection, Some(&record));
        player.inventory_menu.carried = record.carried;
        player.inventory_menu.grid = record.grid;
        player.inventory_menu.grid.resize(
            player.inventory_menu.grid.len().max(4),
            mc_data::ItemStack::EMPTY,
        );
        player.statistics.advance_tick(state.tick);
        player
            .statistics
            .set_active(true, !player.vitals.is_dead(), false, false);
        player.warden_warning.advance_tick(state.tick);
        player.warden_warning.set_active(true);
        state.players.insert(entity, player);
        state.entities.set_pos(slot, record.pos);
        state.entities.set_rot(slot, record.yaw, record.pitch);
        state.entities.set_ground(slot, !record.fall_flying);
        let (body, row) = state.entities.parts_mut(slot);
        body.alive = record.health > 0.0;
        body.height = if record.fall_flying { 0.6 } else { 1.8 };
        row.health = record.health;
        row.entity_life.portal = record.portal;
        let appearance = std::sync::Arc::make_mut(&mut row.appearance);
        appearance.health = record.health;
        appearance.pose = if record.fall_flying { 1 } else { 0 };
        appearance.shared_flags = if record.fall_flying { 0x80 } else { 0 };
        crate::contact_hazards::ignite(state, entity, record.burn_ticks);
        state.correct_position(connection, record.pos, record.yaw, record.pitch);
    }
    let effects = {
        let rules = state.registry.item_rules();
        state.players.get_mut(&entity).map(|player| {
            let effects = player.close(&rules);
            // Any restored larger crafting menu is now empty; window zero
            // returns to its ordinary two-by-two input grid.
            player.inventory_menu.grid.truncate(4);
            effects
        })
    };
    if let Some(effects) = effects {
        let mut changes = ChangeSet::default();
        apply_player_effects(state, connection, entity, effects, &mut changes);
        state.plugin_spawned.extend(changes.entities_spawned);
    }
    if let Some(player) = state.players.get(&entity) {
        let burst = [
            player.inventory_contents(),
            player.health_change(),
            player.air_change(),
            player.experience_change(),
            crate::boundary::PlayerChange::GameModeChanged {
                game_mode: player.game_mode as u8,
            },
        ];
        let dead = player.vitals.is_dead();
        for change in burst {
            state.push_player(connection, change);
        }
        if dead {
            state.push_player(
                connection,
                crate::boundary::PlayerChange::Died {
                    message: mc_nbt::Tag::String(String::new()),
                },
            );
        }
    }
    crate::effects::sync_join(state, entity);
    crate::advancements::sync_join(state, entity);
    if let Some(name) = state.connections.name_of(entity).map(str::to_owned) {
        super::input::session::announce_player(state, "multiplayer.player.joined", &name, None);
    }
}
