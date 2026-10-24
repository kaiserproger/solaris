//! Cancellation-safe native admission: one FIFO queue, including reserved Leave.
use tokio::sync::mpsc::{OwnedPermit, Sender, error::TrySendError};

use crate::boundary::{ConnectionId, PlayerInput};
use crate::config::CoreError;

/// An admitted edge session. Dropping it queues its native Leave exactly once.
/// The reserved slot keeps cleanup nonblocking even when ordinary input is full.
#[derive(Debug)]
pub struct PlayerSession {
    connection: ConnectionId,
    leave: Option<OwnedPermit<PlayerInput>>,
}

impl PlayerSession {
    pub(super) fn admit(
        sender: &Sender<PlayerInput>,
        connection: ConnectionId,
        uuid: uuid::Uuid,
        name: String,
        permission_level: u8,
        view_distance: u8,
        locale: String,
    ) -> Result<Self, CoreError> {
        let leave = sender
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                TrySendError::Full(_) => CoreError::InputFull,
                TrySendError::Closed(_) => CoreError::TickStopped,
            })?;
        // Reserve all initial inputs before publishing any. A refused join
        // leaves no radius row behind, and no caller waits while holding credit.
        let mut inputs = sender.try_reserve_many(3).map_err(|error| match error {
            TrySendError::Full(_) => CoreError::InputFull,
            TrySendError::Closed(_) => CoreError::TickStopped,
        })?;
        inputs
            .next()
            .expect("three reserved inputs")
            .send(PlayerInput::SetViewDistance {
                connection,
                view_distance,
            });
        inputs
            .next()
            .expect("second reserved input")
            .send(PlayerInput::SetLocale { connection, locale });
        inputs
            .next()
            .expect("third reserved input")
            .send(PlayerInput::Join {
                connection,
                uuid,
                name,
                permission_level,
            });
        Ok(Self {
            connection,
            leave: Some(leave),
        })
    }
}

impl Drop for PlayerSession {
    fn drop(&mut self) {
        if let Some(permit) = self.leave.take() {
            let sender = permit.send(PlayerInput::Leave {
                connection: self.connection,
            });
            if sender.is_closed() {
                tracing::debug!(
                    connection = self.connection.0,
                    "native session ended after core stopped"
                );
            }
        }
    }
}
