//! The world observations of `world-events.wit`, mapped from the server's own
//! committed events.
//!
//! Every variant here is built from a `ScriptEventKind` the server's gameplay
//! owners publish *after* they committed something: a block removal or placement,
//! a craft, an item credit, a lethal blow, an accepted interaction, a death. This
//! module translates those snapshots into the contract's records and adds nothing:
//! it reads no world, resolves no session, and invents no field.
//!
//! Three rules of the boundary are visible in the mapping and are the reason this
//! is a translation rather than a copy:
//!
//! - The player is the event's own immutable context, not a live lookup. A
//!   reconnect after the observation cannot change who it says acted.
//! - The session is the runtime player id the event was built with. The server
//!   never keeps a session -> identity map for script events, so a session a
//!   reconnect has replaced is still reported as the one the event happened on.
//! - The entity's runtime id is dropped. It is an unversioned simulation
//!   identifier the owner reuses, and no consumer of these observations reads it;
//!   the entity's type id is what the contract carries.
//!
//! A kind this contract does not carry - including a kind a future server adds -
//! answers `None`. The caller drops it rather than reporting it as one of these
//! observations: telling a plugin that a block was broken when the server said
//! something else would be telling it something no owner said. `ServerTick` is the
//! same case, and deliberately so: it is the clock the host stamps every batch's
//! `EventContext.tick` with, not an observation a plugin is called for, so a
//! consumer that needs the tick it is recording against reads the context of the
//! batch that carried its observation.

use mc_script::{ScriptEventKind, ScriptPlayerId};

use crate::bindings::exports::solaris::plugin::events::Event;
use crate::bindings::solaris::plugin::types::Position;
use crate::bindings::solaris::plugin::world_events::{
    BlockPosition, PlayerBlockBroken, PlayerBlockPlaced, PlayerDied, PlayerEntityInteracted,
    PlayerEntityKilled, PlayerItemCrafted, PlayerItemPickedUp,
};

/// The contract's event for one committed gameplay observation, or `None` when
/// this contract carries no observation of that kind.
pub(crate) fn map_event(kind: &ScriptEventKind) -> Option<Event> {
    match kind {
        ScriptEventKind::PlayerBlockBroken {
            player_id,
            context,
            dimension,
            block_id,
            x,
            y,
            z,
            ..
        } => Some(Event::PlayerBlockBroken(PlayerBlockBroken {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            dimension: dimension.clone(),
            block: block_id.clone(),
            at: BlockPosition {
                x: *x,
                y: *y,
                z: *z,
            },
        })),
        ScriptEventKind::PlayerBlockPlaced {
            player_id,
            context,
            dimension,
            block_id,
            x,
            y,
            z,
            ..
        } => Some(Event::PlayerBlockPlaced(PlayerBlockPlaced {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            dimension: dimension.clone(),
            block: block_id.clone(),
            at: BlockPosition {
                x: *x,
                y: *y,
                z: *z,
            },
        })),
        ScriptEventKind::PlayerItemCrafted {
            player_id,
            context,
            dimension,
            item_id,
            count,
            ..
        } => Some(Event::PlayerItemCrafted(PlayerItemCrafted {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            dimension: dimension.clone(),
            item: item_id.clone(),
            count: *count,
            position: Position {
                x: context.x(),
                y: context.y(),
                z: context.z(),
            },
        })),
        ScriptEventKind::PlayerItemPickedUp {
            player_id,
            context,
            dimension,
            item_id,
            count,
            ..
        } => Some(Event::PlayerItemPickedUp(PlayerItemPickedUp {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            dimension: dimension.clone(),
            item: item_id.clone(),
            count: *count,
            position: Position {
                x: context.x(),
                y: context.y(),
                z: context.z(),
            },
        })),
        ScriptEventKind::PlayerEntityKilled {
            player_id,
            context,
            dimension,
            entity_type,
            ..
        } => Some(Event::PlayerEntityKilled(PlayerEntityKilled {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            dimension: dimension.clone(),
            entity_type: entity_type.clone(),
            position: Position {
                x: context.x(),
                y: context.y(),
                z: context.z(),
            },
        })),
        ScriptEventKind::PlayerEntityInteracted {
            player_id,
            context,
            dimension,
            entity_type,
            ..
        } => Some(Event::PlayerEntityInteracted(PlayerEntityInteracted {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            dimension: dimension.clone(),
            entity_type: entity_type.clone(),
            position: Position {
                x: context.x(),
                y: context.y(),
                z: context.z(),
            },
        })),
        // The pose is the one the death committed at, from the same immutable
        // context every other observation of this module takes its actor from: a
        // later respawn pose is a different value and never reaches this mapping.
        ScriptEventKind::PlayerDied {
            player_id,
            context,
            dimension,
            ..
        } => Some(Event::PlayerDied(PlayerDied {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            dimension: dimension.clone(),
            position: Position {
                x: context.x(),
                y: context.y(),
                z: context.z(),
            },
        })),
        _ => None,
    }
}

/// The connection one script-visible event was produced on.
///
/// The runtime player id *is* the live connection's id on this boundary - the same
/// rule the host applies when it maps a command-invoked or menu-clicked event - and
/// it is taken from the event itself rather than resolved from the stable identity,
/// which a reconnect may have moved to a different connection.
fn session_of(player_id: ScriptPlayerId) -> u64 {
    player_id.value()
}

#[cfg(test)]
mod tests {
    use mc_script::{
        ScriptCraftingSource, ScriptEvent, ScriptGameMode, ScriptPlayerContext, ScriptPlayerId,
    };

    use super::{Event, map_event};

    #[test]
    fn crafted_observation_carries_the_committed_actor_pose() {
        let event = ScriptEvent::try_player_item_crafted_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::new(
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "Ada",
                false,
                12.9,
                64.2,
                -4.7,
            ),
            "minecraft:overworld",
            "minecraft:bread",
            1,
            1,
            ScriptCraftingSource::Inventory,
            ScriptGameMode::Survival,
        )
        .expect("committed craft observation");
        let Some(Event::PlayerItemCrafted(mapped)) = map_event(event.kind()) else {
            panic!("craft must map to the component contract");
        };

        assert_eq!(mapped.session, 7);
        assert_eq!(mapped.position.x, 12.9);
        assert_eq!(mapped.position.y, 64.2);
        assert_eq!(mapped.position.z, -4.7);
    }
}
