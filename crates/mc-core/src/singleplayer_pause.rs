//! Whole-world pause requested by the local singleplayer operator.

use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
use crate::tick::TickState;

#[derive(Default)]
pub(crate) struct SingleplayerPause {
    owner: Option<ConnectionId>,
}

/// Count players across every dimension, including for empty-server pause.
pub(crate) fn online_players(state: &TickState) -> usize {
    std::iter::once(state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .map(|world| world.players.len())
        .sum()
}

impl SingleplayerPause {
    /// Consume pause requests and return whether a whole world tick should run.
    pub(crate) fn filter_inputs(
        &mut self,
        state: &TickState,
        inputs: &mut Vec<PlayerInput>,
        outbox: &std::sync::mpsc::Sender<Vec<(ConnectionId, Vec<PlayerChange>)>>,
    ) -> bool {
        let joining = inputs
            .iter()
            .any(|input| matches!(input, PlayerInput::Join { .. }));
        let leaving = inputs
            .iter()
            .any(|input| matches!(input, PlayerInput::Leave { .. }));
        if joining
            || inputs.iter().any(|input| {
                matches!(input,
                    PlayerInput::Leave { connection } if Some(*connection) == self.owner
                )
            })
        {
            self.owner = None;
        }
        inputs.retain(|input| {
            let PlayerInput::Command {
                connection,
                command,
                permission_level,
            } = input
            else {
                return true;
            };
            let requested = match command.as_str() {
                "solaris_pause on" => true,
                "solaris_pause off" => false,
                _ => return true,
            };
            let owner_online = std::iter::once(state)
                .chain(state.other_dimensions.values().map(|world| &**world))
                .any(|world| {
                    world.connections.entity_of(*connection).is_some()
                        && !world.session_locked(*connection)
                });
            let allowed = *permission_level >= 4
                && owner_online
                && online_players(state) == 1
                && !joining
                && !leaving
                && (requested || self.owner == Some(*connection));
            if allowed {
                self.owner = requested.then_some(*connection);
            } else {
                let _ = outbox.send(vec![(
                    *connection,
                    vec![PlayerChange::Chat {
                        message: "Cannot pause: one online operator is required.".into(),
                    }],
                )]);
            }
            false
        });
        if self.owner.is_some() {
            // The edge handles keepalives independently of world ticks.
            inputs.clear();
        }
        self.owner.is_none()
    }
}

#[cfg(test)]
#[path = "singleplayer_pause_tests.rs"]
mod tests;
