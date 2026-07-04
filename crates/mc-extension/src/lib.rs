//! # mc-extension
//!
//! Safe extension boundary primitives.
//!
//! This crate intentionally does not run plugins. It defines the lock-free DTOs
//! and bounded queues that server code can use to hand immutable event snapshots
//! to a future extension host and receive bounded command requests back.

use std::num::NonZeroUsize;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

use bytes::Bytes;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Semantic version of the stable extension boundary contract.
pub const EXTENSION_API_VERSION: ExtensionApiVersion = ExtensionApiVersion::new(0, 1, 0);

/// Default maximum custom payload body accepted for extension forwarding.
pub const DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES: usize = 32 * 1024;

/// Version requested by an extension host or supported by the server boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExtensionApiVersion {
    major: u16,
    minor: u16,
    patch: u16,
}

impl ExtensionApiVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub const fn major(&self) -> u16 {
        self.major
    }

    pub const fn minor(&self) -> u16 {
        self.minor
    }

    pub const fn patch(&self) -> u16 {
        self.patch
    }

    pub const fn is_supported_by(&self, host: Self) -> bool {
        if self.major != host.major {
            return false;
        }
        if self.minor < host.minor {
            return true;
        }
        self.minor == host.minor && self.patch <= host.patch
    }
}

pub const fn supports_extension_api_version(requested: ExtensionApiVersion) -> bool {
    requested.is_supported_by(EXTENSION_API_VERSION)
}

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

/// Server-owned side of the extension boundary.
#[derive(Debug)]
pub struct ExtensionBoundary {
    event_tx: SyncSender<InboundEvent>,
    command_rx: Receiver<OutboundCommand>,
}

impl ExtensionBoundary {
    /// Try to enqueue one inbound event without blocking server tasks.
    pub fn try_enqueue_event(&self, event: InboundEvent) -> Result<(), QueueError<InboundEvent>> {
        self.event_tx.try_send(event).map_err(QueueError::from)
    }

    /// Try to receive one outbound command emitted by the extension side.
    pub fn try_recv_command(&self) -> Result<OutboundCommand, QueueRecvError> {
        self.command_rx.try_recv().map_err(QueueRecvError::from)
    }
}

/// Extension-host side of the boundary.
#[derive(Debug)]
pub struct ExtensionEndpoint {
    event_rx: Receiver<InboundEvent>,
    command_tx: SyncSender<OutboundCommand>,
}

impl ExtensionEndpoint {
    /// Try to receive one inbound event snapshot without blocking.
    pub fn try_recv_event(&self) -> Result<InboundEvent, QueueRecvError> {
        self.event_rx.try_recv().map_err(QueueRecvError::from)
    }

    /// Try to submit one outbound command without blocking extension workers.
    pub fn try_submit_command(
        &self,
        command: OutboundCommand,
    ) -> Result<(), QueueError<OutboundCommand>> {
        self.command_tx.try_send(command).map_err(QueueError::from)
    }
}

/// Construct paired server and extension handles with fixed queue capacities.
pub fn boundary_pair(
    event_capacity: NonZeroUsize,
    command_capacity: NonZeroUsize,
) -> (ExtensionBoundary, ExtensionEndpoint) {
    let (event_tx, event_rx) = sync_channel(event_capacity.get());
    let (command_tx, command_rx) = sync_channel(command_capacity.get());

    (
        ExtensionBoundary {
            event_tx,
            command_rx,
        },
        ExtensionEndpoint {
            event_rx,
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

    #[test]
    fn extension_api_version_accepts_current_and_older_minor_only() {
        assert_eq!(EXTENSION_API_VERSION, ExtensionApiVersion::new(0, 1, 0));
        assert!(supports_extension_api_version(EXTENSION_API_VERSION));
        assert!(supports_extension_api_version(ExtensionApiVersion::new(
            0, 0, 0
        )));
        assert!(!supports_extension_api_version(ExtensionApiVersion::new(
            0, 1, 1
        )));
        assert!(!supports_extension_api_version(ExtensionApiVersion::new(
            0, 2, 0
        )));
        assert!(!supports_extension_api_version(ExtensionApiVersion::new(
            1, 0, 0
        )));
    }
}
