//! Settlement, build and army command execution.
//! Moved verbatim from `command_execution.rs`.

use super::*;

/// Native `/settlement` execution. Create is open to any authenticated actor;
/// every record mutation needs the owner or an operator. Reads need nothing.
#[allow(clippy::too_many_lines)]
pub(super) fn settlement(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &crate::settlements::commands::SettlementCommand,
) {
    use crate::settlements::commands::{SettlementCommand, lookup};
    let operator = state.plugin_players.is_operator(connection)
        || connection == crate::command_console::CONNECTION;
    // Reads resolve below without an actor; every mutation arm re-checks.
    let actor = actor_identity(state, connection);
    match command {
        SettlementCommand::Create { name, radius } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let anchor = crate::command_console::pose(state, executor)
                .map(|(position, _)| position.map(|coordinate| coordinate.floor() as i32));
            let Some(anchor) = anchor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            match state.settlements.create(
                &mut state.settlement_sequence,
                name.clone(),
                actor,
                anchor,
                *radius,
            ) {
                Ok(id) => {
                    crate::settlements::refresh_owner_names(state);
                    let record = state.settlements.get(id).expect("created record");
                    let revision = record.revision;
                    error(
                        state,
                        connection,
                        format!("Founded {name} (revision {revision})."),
                    );
                }
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Claim => {
            match crate::settlements::commands::claim(state, connection, None) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Menu { page } => {
            crate::settlements::ui::open_menu(state, connection, page.as_deref());
        }
        SettlementCommand::List => {
            let mut names: Vec<&str> = state
                .settlements
                .records()
                .map(|record| record.name.as_str())
                .collect();
            names.sort_unstable();
            let message = if names.is_empty() {
                "No settlements.".to_owned()
            } else {
                format!("Settlements: {}.", names.join(", "))
            };
            error(state, connection, message);
        }
        SettlementCommand::Info { name } => {
            let record = match name.as_deref() {
                Some(name) => lookup(state, name),
                None => state.settlements.records().next(),
            };
            match record {
                None => error(state, connection, "Settlement not found."),
                Some(record) => {
                    let members = crate::settlements::member_count(state, record.id);
                    let message = format!(
                        "{}\n{} | owner={} anchor={:?} radius={} members={} revision={}",
                        crate::settlements::presentation::summary(state, record),
                        record.name,
                        record.owner.as_deref().unwrap_or("autonomous"),
                        record.anchor,
                        record.radius,
                        members,
                        record.revision,
                    );
                    error(state, connection, message);
                }
            }
        }
        SettlementCommand::Rename { name, new_name } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let Some(record) = lookup(state, name) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator renames a settlement.",
                );
                return;
            }
            let id = record.id;
            match state.settlements.rename(id, new_name.clone()) {
                Ok(()) => error(state, connection, format!("Renamed {name} to {new_name}.")),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Remove { name } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let Some(record) = lookup(state, name) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator removes a settlement.",
                );
                return;
            }
            let id = record.id;
            if crate::settlements::remove(state, id) {
                error(state, connection, format!("Removed {name}."));
            } else {
                error(
                    state,
                    connection,
                    "Settlement removal refused: construction could not be cancelled.",
                );
            }
        }
        SettlementCommand::Relocate {
            settlement,
            household,
            destination,
        } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let (Some(source), Some(target)) =
                (lookup(state, settlement), lookup(state, destination))
            else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !source.can_manage(&actor, operator) || !target.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner of both settlements or an operator relocates a household.",
                );
                return;
            }
            let (source, target) = (source.id, target.id);
            match crate::settlements::households::relocate(state, source, household, target) {
                Ok(()) => error(
                    state,
                    connection,
                    format!(
                        "Relocated {household} to {destination}; native housing admission will reacquire reachable claims."
                    ),
                ),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Bind {
            settlement,
            villager,
        } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let Some(record) = lookup(state, settlement) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator binds residents.",
                );
                return;
            }
            let id = record.id;
            let (entity, uuid) = match settlement_resident(state, executor, villager.as_deref()) {
                Ok(resident) => resident,
                Err(message) => {
                    error(state, connection, message);
                    return;
                }
            };
            match crate::settlements::bind(state, id, entity) {
                Ok(()) => error(state, connection, format!("Bound {uuid} to {settlement}.")),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Unbind { villager } => {
            let Some(actor) = actor else {
                error(
                    state,
                    connection,
                    "Settlement command admission is unavailable.",
                );
                return;
            };
            let (entity, uuid) = match settlement_resident(state, executor, villager.as_deref()) {
                Ok(resident) => resident,
                Err(message) => {
                    error(state, connection, message);
                    return;
                }
            };
            let slot = state.entities.slot_of(entity).expect("resolved resident");
            let Some(villager) = state.entities.row(slot).villager.as_ref() else {
                error(state, connection, "Villager not found.");
                return;
            };
            let id = villager.settlement;
            let Some(id) = id else {
                error(state, connection, "Villager belongs to no settlement.");
                return;
            };
            let Some(record) = state.settlements.get(id) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            if !record.can_manage(&actor, operator) {
                error(
                    state,
                    connection,
                    "Only the owner or an operator unbinds residents.",
                );
                return;
            }
            match crate::settlements::unbind(state, entity) {
                Ok(()) => error(state, connection, format!("Unbound {uuid}.")),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Economy {
            domain,
            settlement,
            arguments,
        } => {
            let Some(record) = lookup(state, settlement) else {
                error(state, connection, "Settlement not found.");
                return;
            };
            let status = arguments.len() == 1 && arguments[0] == "status";
            if !status
                && actor
                    .as_ref()
                    .is_none_or(|actor| !record.can_manage(actor, operator))
            {
                error(
                    state,
                    connection,
                    "Only the owner or an operator changes settlement economics.",
                );
                return;
            }
            let id = record.id;
            match crate::settlements::economy_commands::execute(state, id, *domain, arguments) {
                Ok(message) => error(state, connection, message),
                Err(failure) => error(
                    state,
                    connection,
                    format!("Native settlement action refused: {failure:?}."),
                ),
            }
        }
        SettlementCommand::BuildingCertify { kind, min, max } => {
            match crate::settlements::building_commands::certify(
                state,
                executor,
                actor.as_deref(),
                operator,
                kind,
                *min,
                *max,
            ) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::BuildingUpgrade { id } => {
            match crate::settlements::building_commands::upgrade(
                state,
                actor.as_deref(),
                operator,
                *id,
            ) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::BuildingList => {
            match crate::settlements::building_commands::list(state, executor) {
                Ok(message) => error(state, connection, message),
                Err(message) => error(state, connection, message),
            }
        }
        SettlementCommand::Build { blueprint } => {
            build_command(
                state,
                connection,
                executor,
                actor.as_deref(),
                operator,
                blueprint,
            );
        }
        SettlementCommand::BuildUpgrade { id } => {
            build_upgrade_command(state, connection, executor, actor.as_deref(), operator, *id);
        }
        SettlementCommand::Army { command } => {
            army_command(state, connection, executor, command);
        }
    }
}

/// Native `/settlement build ...` execution. The target is always the
/// executor's enclosing settlement (closest anchor within its radius, like
/// building/army commands). Both variants need an authenticated actor and
/// `can_manage`, which also refuses ownerless settlements; operators of an
/// owned settlement pass per existing semantics. Orders start immediately when
/// funded and free, or replace the single pending owner order. `build cancel`
/// cancels only that pending order; active work and real stock stay unchanged.
pub(super) fn build_command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    actor: Option<&str>,
    operator: bool,
    blueprint: &str,
) {
    use crate::settlements::building_commands;
    let Some(id) = building_commands::containing(state, executor) else {
        return error(state, connection, building_commands::NO_SETTLEMENT);
    };
    let Some(actor) = actor else {
        return error(state, connection, building_commands::ADMISSION_UNAVAILABLE);
    };
    if state
        .settlements
        .get(id)
        .is_none_or(|record| !record.can_manage(actor, operator))
    {
        return error(
            state,
            connection,
            "Only the owner or an operator orders construction.",
        );
    }
    if blueprint == "cancel" {
        let message = crate::settlements::construction::cancel_pending(state, id).map_or_else(
            |message| message,
            |()| "Cancelled pending construction order.".into(),
        );
        return error(state, connection, message);
    }
    match crate::settlements::construction::request_build(state, id, blueprint) {
        Ok(outcome) => error(state, connection, outcome.message(blueprint)),
        Err(message) => error(state, connection, message),
    }
}

/// Native `/settlement build upgrade <buildingId>` execution. The building id
/// is global (`settlement << 32 | sequence`, like `/settlement building
/// upgrade`), but authorization runs against the executor's enclosing
/// settlement: the target must belong to it. Starts a funded free upgrade or
/// queues it without stock debit or placement.
/// Kept separate from the geometric `/settlement building upgrade` path.
pub(super) fn build_upgrade_command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    actor: Option<&str>,
    operator: bool,
    id: u64,
) {
    use crate::settlements::building_commands;
    use crate::settlements::buildings::BuildingId;
    let Some(enclosing) = building_commands::containing(state, executor) else {
        return error(state, connection, building_commands::NO_SETTLEMENT);
    };
    let Some(actor) = actor else {
        return error(state, connection, building_commands::ADMISSION_UNAVAILABLE);
    };
    if state
        .settlements
        .get(enclosing)
        .is_none_or(|record| !record.can_manage(actor, operator))
    {
        return error(
            state,
            connection,
            "Only the owner or an operator orders construction.",
        );
    }
    let building = BuildingId(id);
    let home = crate::settlements::SettlementId(id >> 32);
    if home != enclosing {
        return error(state, connection, "Building not found.");
    }
    let blueprint = state
        .settlements
        .get(enclosing)
        .and_then(|record| record.buildings.get(&building))
        .and_then(|target| {
            crate::settlements::blueprints::all(&state.registry)
                .iter()
                .find(|bp| bp.kind == target.kind)
        })
        .map(|bp| bp.id.clone());
    match crate::settlements::construction::request_upgrade(state, enclosing, building) {
        Ok(outcome) => {
            let mut label = blueprint.expect("accepted blueprint");
            if let Some(order) = state
                .settlements
                .get(enclosing)
                .and_then(|record| record.pending_order.as_ref())
            {
                use std::fmt::Write;
                write!(label, " L{}", order.level).expect("string write");
            }
            error(state, connection, outcome.message(&label));
        }
        Err(message) => error(state, connection, message),
    }
}
/// Native `/settlement army ...` execution. The target is always the
/// executor's enclosing settlement (closest anchor within its radius, like
/// building commands). Reads are open; every mutation re-checks
/// `can_manage`, which also refuses ownerless settlements. Follow uses the
/// executor's entity UUID; guard uses the executor's current block position.
pub(super) fn army_command(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &crate::settlements::commands::ArmyCommand,
) {
    use crate::settlements::commands::{ArmyCommand, ArmyOrderKind};
    use crate::settlements::{army, building_commands};
    let Some(id) = building_commands::containing(state, executor) else {
        return error(state, connection, building_commands::NO_SETTLEMENT);
    };
    if !matches!(command, ArmyCommand::List) {
        let operator = state.plugin_players.is_operator(connection)
            || connection == crate::command_console::CONNECTION;
        let Some(actor) = actor_identity(state, connection) else {
            return error(state, connection, building_commands::ADMISSION_UNAVAILABLE);
        };
        if state
            .settlements
            .get(id)
            .is_none_or(|record| !record.can_manage(&actor, operator))
        {
            return error(
                state,
                connection,
                "Only the owner or an operator commands soldiers.",
            );
        }
    }
    match command {
        ArmyCommand::List => {
            let rows = army::list(state, id);
            let message = if rows.is_empty() {
                "No soldiers.".to_owned()
            } else {
                rows.join("\n")
            };
            error(state, connection, message);
        }
        ArmyCommand::Recruit => {
            let result = army::recruit(state, id).map(|uuid| format!("Recruited {uuid}."));
            error(
                state,
                connection,
                result.unwrap_or_else(std::convert::identity),
            );
        }
        ArmyCommand::Dismiss { soldier } => {
            let Ok(uuid) = soldier.parse::<uuid::Uuid>() else {
                return error(state, connection, "Soldier must be a native entity UUID.");
            };
            let result = army::dismiss(state, id, uuid).map(|()| format!("Dismissed {uuid}."));
            error(
                state,
                connection,
                result.unwrap_or_else(std::convert::identity),
            );
        }
        ArmyCommand::Order { target, order } => {
            let uuid = if target == "all" {
                None
            } else {
                let Ok(uuid) = target.parse::<uuid::Uuid>() else {
                    return error(
                        state,
                        connection,
                        "Soldier must be a native entity UUID or all.",
                    );
                };
                Some(uuid)
            };
            let order = match order {
                ArmyOrderKind::Follow | ArmyOrderKind::Guard => {
                    let Some(slot) = state.entities.slot_of(executor) else {
                        return error(state, connection, "Your position is unavailable.");
                    };
                    if *order == ArmyOrderKind::Follow {
                        army::Order::Follow(state.entities.row(slot).uuid)
                    } else {
                        army::Order::Guard(
                            state.entities.pos(slot).map(|value| value.floor() as i32),
                        )
                    }
                }
                ArmyOrderKind::Patrol => army::Order::Patrol,
                ArmyOrderKind::Return => army::Order::Return,
            };
            let result = army::order(state, id, uuid, order)
                .map(|count| format!("Ordered {count} soldier(s)."));
            error(
                state,
                connection,
                result.unwrap_or_else(std::convert::identity),
            );
        }
    }
}

/// Explicit UUIDs retain the native target path. Without one, pick the nearest
/// living resident in this dimension, within eight blocks of the player.
pub(super) fn settlement_resident(
    state: &TickState,
    executor: EntityId,
    target: Option<&str>,
) -> Result<(EntityId, uuid::Uuid), &'static str> {
    if let Some(target) = target {
        let uuid = target
            .parse::<uuid::Uuid>()
            .map_err(|_| "Villager must be a native entity UUID.")?;
        let slot = state
            .entities
            .slot_of_uuid(uuid)
            .filter(|&slot| state.entities.row(slot).uuid == uuid)
            .ok_or("Villager not found.")?;
        return Ok((state.entities.body(slot).id, uuid));
    }
    let origin = state
        .entities
        .slot_of(executor)
        .map(|slot| state.entities.pos(slot))
        .ok_or("Stand within 8 blocks of a villager, or supply its native UUID.")?;
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            let villager = row.villager.as_ref()?;
            if !body.alive || villager.wandering || villager.conversion_time.is_some() {
                return None;
            }
            let distance: f64 = state
                .entities
                .pos(slot)
                .iter()
                .zip(origin)
                .map(|(at, origin)| (at - origin).powi(2))
                .sum();
            (distance <= 64.0).then_some((distance, body.id, row.uuid))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, entity, uuid)| (entity, uuid))
        .ok_or("No living villager within 8 blocks. Stand next to one, or supply its native UUID.")
}
