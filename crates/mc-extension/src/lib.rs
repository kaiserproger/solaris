//! # mc-extension
//!
//! Safe extension boundary primitives.
//!
//! This crate intentionally does not run plugins. It defines the lock-free DTOs
//! and bounded queues that server code can use to hand immutable event snapshots
//! to a future extension host and receive bounded command requests back.

use std::num::NonZeroUsize;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::sync::Notify;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default maximum custom payload body accepted for extension forwarding.
pub const DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES: usize = 32 * 1024;

/// Stable player/session identifier snapshot for extension DTOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct PlayerId(u64);

impl PlayerId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Protocol phase that produced a custom payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProtocolPhase {
    Configuration,
    Play,
}

/// Immutable inbound event snapshots visible to extension code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InboundEvent {
    PlayerJoined {
        player_id: PlayerId,
        username: String,
    },
    PlayerLeft {
        player_id: PlayerId,
        reason: String,
    },
    ClientBrand {
        player_id: PlayerId,
        brand: String,
    },
    CustomPayload(CustomPayloadEvent),
}

/// Bounded custom payload snapshot allowed through the extension boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CustomPayloadEvent {
    pub player_id: PlayerId,
    pub phase: ProtocolPhase,
    pub channel: String,
    pub payload: Bytes,
}

/// Outbound command requests emitted by extension code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutboundCommand {
    SendCustomPayload {
        player_id: PlayerId,
        channel: String,
        payload: Bytes,
    },
    DisconnectPlayer {
        player_id: PlayerId,
        reason: String,
    },
}

/// Limits and allow-list for forwarding serverbound custom payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomPayloadPolicy {
    max_payload_bytes: usize,
    allowed_channels: Vec<String>,
}

impl CustomPayloadPolicy {
    /// Create a policy that only forwards channels explicitly listed in
    /// `allowed_channels` and rejects bodies larger than `max_payload_bytes`.
    pub fn new(
        max_payload_bytes: usize,
        allowed_channels: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            max_payload_bytes,
            allowed_channels: allowed_channels.into_iter().collect(),
        }
    }

    pub fn max_payload_bytes(&self) -> usize {
        self.max_payload_bytes
    }

    pub fn allowed_channels(&self) -> &[String] {
        &self.allowed_channels
    }

    pub fn allows_channel(&self, channel: &str) -> bool {
        self.allowed_channels
            .iter()
            .any(|allowed| allowed == channel)
    }

    /// Validate and copy a serverbound custom payload into a bounded event.
    ///
    /// Callers should pass borrowed packet body bytes here before retaining an
    /// unknown payload. Unknown channels and oversized bodies are rejected before
    /// constructing an owned `Bytes` value for extension dispatch.
    pub fn build_event(
        &self,
        player_id: PlayerId,
        phase: ProtocolPhase,
        channel: &str,
        payload: &[u8],
    ) -> Result<CustomPayloadEvent, CustomPayloadRejection> {
        if !self.allows_channel(channel) {
            return Err(CustomPayloadRejection::UnknownChannel {
                channel: channel.to_owned(),
            });
        }

        if payload.len() > self.max_payload_bytes {
            return Err(CustomPayloadRejection::PayloadTooLarge {
                len: payload.len(),
                max: self.max_payload_bytes,
            });
        }

        Ok(CustomPayloadEvent {
            player_id,
            phase,
            channel: channel.to_owned(),
            payload: Bytes::copy_from_slice(payload),
        })
    }
}

impl Default for CustomPayloadPolicy {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES, std::iter::empty())
    }
}

/// Reason a custom payload was not forwarded to the extension boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CustomPayloadRejection {
    UnknownChannel { channel: String },
    PayloadTooLarge { len: usize, max: usize },
}

/// Error returned when a bounded extension queue cannot accept an item.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueueError<T> {
    Full(T),
    Closed(T),
}

/// Error returned when a bounded extension queue has no item to receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueueRecvError {
    Empty,
    Closed,
}

impl<T> From<TrySendError<T>> for QueueError<T> {
    fn from(value: TrySendError<T>) -> Self {
        match value {
            TrySendError::Full(item) => Self::Full(item),
            TrySendError::Disconnected(item) => Self::Closed(item),
        }
    }
}

impl From<TryRecvError> for QueueRecvError {
    fn from(value: TryRecvError) -> Self {
        match value {
            TryRecvError::Empty => Self::Empty,
            TryRecvError::Disconnected => Self::Closed,
        }
    }
}

#[derive(Debug)]
struct SharedQueueSender<T> {
    sender: Mutex<Option<SyncSender<T>>>,
    ready: Arc<Notify>,
}

impl<T> SharedQueueSender<T> {
    fn new(sender: SyncSender<T>, ready: Arc<Notify>) -> Self {
        Self {
            sender: Mutex::new(Some(sender)),
            ready,
        }
    }

    fn try_send(&self, item: T) -> Result<(), QueueError<T>> {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(sender) = sender.as_ref() else {
            return Err(QueueError::Closed(item));
        };
        sender
            .try_send(item)
            .map(|()| self.ready.notify_one())
            .map_err(QueueError::from)
    }

    fn close(&self) {
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        drop(sender);
        self.ready.notify_waiters();
    }
}

impl<T> Drop for SharedQueueSender<T> {
    fn drop(&mut self) {
        let sender = self
            .sender
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        drop(sender);
        self.ready.notify_waiters();
    }
}

/// Server-owned side of the extension boundary.
#[derive(Debug, Clone)]
pub struct ExtensionBoundary {
    event_tx: Arc<SharedQueueSender<InboundEvent>>,
    command_rx: Arc<Mutex<Receiver<OutboundCommand>>>,
    command_ready: Arc<Notify>,
}

impl ExtensionBoundary {
    /// Try to enqueue one inbound event without blocking server tasks.
    pub fn try_enqueue_event(&self, event: InboundEvent) -> Result<(), QueueError<InboundEvent>> {
        self.event_tx.try_send(event)
    }

    /// Close event production for every boundary clone.
    pub fn close_event_queue(&self) {
        self.event_tx.close();
    }

    /// Try to receive one outbound command emitted by the extension side.
    pub fn try_recv_command(&self) -> Result<OutboundCommand, QueueRecvError> {
        let command_rx = self
            .command_rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        command_rx.try_recv().map_err(QueueRecvError::from)
    }

    /// Wait for one outbound command without polling the queue.
    pub async fn recv_command(&self) -> Result<OutboundCommand, QueueRecvError> {
        loop {
            let command_ready = self.command_ready.notified();
            tokio::pin!(command_ready);
            command_ready.as_mut().enable();
            match self.try_recv_command() {
                Err(QueueRecvError::Empty) => command_ready.await,
                result => return result,
            }
        }
    }
}

/// Extension-host side of the boundary.
#[derive(Debug)]
pub struct ExtensionEndpoint {
    event_rx: Arc<Mutex<Receiver<InboundEvent>>>,
    event_ready: Arc<Notify>,
    command_tx: Arc<SharedQueueSender<OutboundCommand>>,
}

impl ExtensionEndpoint {
    /// Try to receive one inbound event snapshot without blocking.
    pub fn try_recv_event(&self) -> Result<InboundEvent, QueueRecvError> {
        let event_rx = self
            .event_rx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        event_rx.try_recv().map_err(QueueRecvError::from)
    }

    /// Wait for one inbound event without polling the queue.
    pub async fn recv_event(&self) -> Result<InboundEvent, QueueRecvError> {
        loop {
            let event_ready = self.event_ready.notified();
            tokio::pin!(event_ready);
            event_ready.as_mut().enable();
            match self.try_recv_event() {
                Err(QueueRecvError::Empty) => event_ready.await,
                result => return result,
            }
        }
    }

    /// Try to submit one outbound command without blocking extension workers.
    pub fn try_submit_command(
        &self,
        command: OutboundCommand,
    ) -> Result<(), QueueError<OutboundCommand>> {
        self.command_tx.try_send(command)
    }

    /// Close command production for this extension endpoint.
    pub fn close_command_queue(&self) {
        self.command_tx.close();
    }
}

/// Construct paired server and extension handles with fixed queue capacities.
pub fn boundary_pair(
    event_capacity: NonZeroUsize,
    command_capacity: NonZeroUsize,
) -> (ExtensionBoundary, ExtensionEndpoint) {
    let (event_tx, event_rx) = sync_channel(event_capacity.get());
    let (command_tx, command_rx) = sync_channel(command_capacity.get());
    let event_ready = Arc::new(Notify::new());
    let command_ready = Arc::new(Notify::new());
    let event_tx = Arc::new(SharedQueueSender::new(event_tx, Arc::clone(&event_ready)));
    let command_tx = Arc::new(SharedQueueSender::new(
        command_tx,
        Arc::clone(&command_ready),
    ));

    (
        ExtensionBoundary {
            event_tx,
            command_rx: Arc::new(Mutex::new(command_rx)),
            command_ready,
        },
        ExtensionEndpoint {
            event_rx: Arc::new(Mutex::new(event_rx)),
            event_ready,
            command_tx,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonzero(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).expect("test capacities are non-zero")
    }

    fn joined(name: &str) -> InboundEvent {
        InboundEvent::PlayerJoined {
            player_id: PlayerId::new(7),
            username: name.to_owned(),
        }
    }

    fn disconnect(reason: &str) -> OutboundCommand {
        OutboundCommand::DisconnectPlayer {
            player_id: PlayerId::new(9),
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn inbound_event_queue_reports_full_without_dropping_existing_event() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));

        boundary.try_enqueue_event(joined("first")).unwrap();
        let rejected = joined("second");

        assert_eq!(
            boundary.try_enqueue_event(rejected.clone()),
            Err(QueueError::Full(rejected))
        );
        assert_eq!(endpoint.try_recv_event().unwrap(), joined("first"));
        assert_eq!(endpoint.try_recv_event(), Err(QueueRecvError::Empty));
    }

    #[test]
    fn cloned_boundary_handles_share_the_same_event_queue() {
        let (boundary, endpoint) = boundary_pair(nonzero(2), nonzero(1));
        let cloned = boundary.clone();

        boundary.try_enqueue_event(joined("first")).unwrap();
        cloned.try_enqueue_event(joined("second")).unwrap();

        assert_eq!(endpoint.try_recv_event().unwrap(), joined("first"));
        assert_eq!(endpoint.try_recv_event().unwrap(), joined("second"));
        assert_eq!(endpoint.try_recv_event(), Err(QueueRecvError::Empty));
    }

    #[test]
    fn outbound_command_queue_reports_full_without_dropping_existing_command() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let first = OutboundCommand::DisconnectPlayer {
            player_id: PlayerId::new(9),
            reason: "first".to_owned(),
        };
        let second = OutboundCommand::DisconnectPlayer {
            player_id: PlayerId::new(9),
            reason: "second".to_owned(),
        };

        endpoint.try_submit_command(first.clone()).unwrap();

        assert_eq!(
            endpoint.try_submit_command(second.clone()),
            Err(QueueError::Full(second))
        );
        assert_eq!(boundary.try_recv_command().unwrap(), first);
        assert_eq!(boundary.try_recv_command(), Err(QueueRecvError::Empty));
    }

    #[test]
    fn try_recv_event_uses_api_owned_empty_error() {
        let (_boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));

        assert_eq!(endpoint.try_recv_event(), Err(QueueRecvError::Empty));
    }

    #[test]
    fn try_recv_command_uses_api_owned_empty_error() {
        let (boundary, _endpoint) = boundary_pair(nonzero(1), nonzero(1));

        assert_eq!(boundary.try_recv_command(), Err(QueueRecvError::Empty));
    }

    #[tokio::test]
    async fn recv_command_wakes_when_endpoint_submits_command() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let expected = OutboundCommand::DisconnectPlayer {
            player_id: PlayerId::new(9),
            reason: "event-driven".to_owned(),
        };
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            boundary.recv_command().await
        });
        started_rx.await.unwrap();

        endpoint.try_submit_command(expected.clone()).unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("submitted command must wake receiver")
            .unwrap();
        assert_eq!(received, Ok(expected));
    }

    #[tokio::test]
    async fn recv_command_wakes_when_endpoint_closes() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            boundary.recv_command().await
        });
        started_rx.await.unwrap();

        drop(endpoint);

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("closed command channel must wake receiver")
            .unwrap();
        assert_eq!(received, Err(QueueRecvError::Closed));
    }

    #[tokio::test]
    async fn recv_event_wakes_when_boundary_enqueues_event() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let expected = joined("event-driven");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            endpoint.recv_event().await
        });
        started_rx.await.unwrap();

        boundary.try_enqueue_event(expected.clone()).unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("enqueued event must wake receiver")
            .unwrap();
        assert_eq!(received, Ok(expected));
    }

    #[tokio::test]
    async fn recv_event_wakes_when_boundary_closes() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            started_tx.send(()).unwrap();
            endpoint.recv_event().await
        });
        started_rx.await.unwrap();

        drop(boundary);

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("closed event channel must wake receiver")
            .unwrap();
        assert_eq!(received, Err(QueueRecvError::Closed));
    }

    #[tokio::test]
    async fn explicit_event_close_wakes_two_waiters() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let endpoint = Arc::new(endpoint);
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first = {
            let endpoint = Arc::clone(&endpoint);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                endpoint.recv_event().await
            })
        };
        let second = {
            let endpoint = Arc::clone(&endpoint);
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                endpoint.recv_event().await
            })
        };
        barrier.wait().await;
        boundary.close_event_queue();

        assert_eq!(first.await.unwrap(), Err(QueueRecvError::Closed));
        assert_eq!(second.await.unwrap(), Err(QueueRecvError::Closed));
    }

    #[tokio::test]
    async fn explicit_command_close_wakes_two_waiters() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let first = {
            let boundary = boundary.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                boundary.recv_command().await
            })
        };
        let second = {
            let boundary = boundary.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                boundary.recv_command().await
            })
        };
        barrier.wait().await;
        endpoint.close_command_queue();

        assert_eq!(first.await.unwrap(), Err(QueueRecvError::Closed));
        assert_eq!(second.await.unwrap(), Err(QueueRecvError::Closed));
    }

    #[tokio::test]
    async fn close_drains_filled_queues_before_reporting_closed() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let event = joined("queued");
        boundary.try_enqueue_event(event.clone()).unwrap();
        boundary.close_event_queue();
        assert_eq!(endpoint.recv_event().await, Ok(event));
        assert_eq!(endpoint.recv_event().await, Err(QueueRecvError::Closed));

        let command = disconnect("queued");
        endpoint.try_submit_command(command.clone()).unwrap();
        endpoint.close_command_queue();
        assert_eq!(boundary.recv_command().await, Ok(command));
        assert_eq!(boundary.recv_command().await, Err(QueueRecvError::Closed));
    }

    #[tokio::test]
    async fn repeated_close_is_idempotent_and_rejects_new_items() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        boundary.close_event_queue();
        boundary.close_event_queue();
        let event = joined("late");
        assert_eq!(
            boundary.try_enqueue_event(event.clone()),
            Err(QueueError::Closed(event))
        );
        assert_eq!(endpoint.recv_event().await, Err(QueueRecvError::Closed));

        endpoint.close_command_queue();
        endpoint.close_command_queue();
        let command = disconnect("late");
        assert_eq!(
            endpoint.try_submit_command(command.clone()),
            Err(QueueError::Closed(command))
        );
        assert_eq!(boundary.recv_command().await, Err(QueueRecvError::Closed));
    }

    #[tokio::test]
    async fn event_queue_closes_only_after_last_boundary_clone_drops() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        let clone = boundary.clone();
        drop(boundary);

        let event = joined("survives-clone-drop");
        clone.try_enqueue_event(event.clone()).unwrap();
        assert_eq!(endpoint.recv_event().await, Ok(event));
        drop(clone);
        assert_eq!(endpoint.recv_event().await, Err(QueueRecvError::Closed));
    }

    #[test]
    fn closed_extension_queue_maps_to_api_owned_closed_error() {
        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        drop(boundary);

        assert_eq!(endpoint.try_recv_event(), Err(QueueRecvError::Closed));

        let (boundary, endpoint) = boundary_pair(nonzero(1), nonzero(1));
        drop(endpoint);

        assert_eq!(boundary.try_recv_command(), Err(QueueRecvError::Closed));
    }

    #[test]
    fn custom_payload_policy_rejects_unknown_channel_before_copying_payload() {
        let policy = CustomPayloadPolicy::new(16, ["solaris:allowed".to_owned()]);

        assert_eq!(
            policy.build_event(
                PlayerId::new(1),
                ProtocolPhase::Play,
                "other:channel",
                b"small",
            ),
            Err(CustomPayloadRejection::UnknownChannel {
                channel: "other:channel".to_owned()
            })
        );
    }

    #[test]
    fn custom_payload_policy_rejects_oversized_payload() {
        let policy = CustomPayloadPolicy::new(4, ["solaris:allowed".to_owned()]);

        assert_eq!(
            policy.build_event(
                PlayerId::new(1),
                ProtocolPhase::Configuration,
                "solaris:allowed",
                b"12345",
            ),
            Err(CustomPayloadRejection::PayloadTooLarge { len: 5, max: 4 })
        );
    }

    #[test]
    fn allowed_custom_payload_builds_bounded_event() {
        let policy = CustomPayloadPolicy::new(4, ["solaris:allowed".to_owned()]);

        let event = policy
            .build_event(
                PlayerId::new(2),
                ProtocolPhase::Play,
                "solaris:allowed",
                b"1234",
            )
            .unwrap();

        assert_eq!(event.player_id, PlayerId::new(2));
        assert_eq!(event.player_id.value(), 2);
        assert_eq!(event.phase, ProtocolPhase::Play);
        assert_eq!(event.channel, "solaris:allowed");
        assert_eq!(event.payload, Bytes::from_static(b"1234"));
    }
}
