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

const MAX_BRIDGE_ID_LEN: usize = 64;
const MAX_NAMESPACED_ID_LEN: usize = 128;

/// Client-mod bridge capabilities requested during extension handshakes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientBridgeManifest {
    bridge_id: String,
    requested_extension_api_version: ExtensionApiVersion,
    protocol_families: Vec<BridgeProtocolFamilySchemas>,
    custom_payload_channels: Vec<String>,
}

impl ClientBridgeManifest {
    /// Create a manifest for a client bridge and its supported protocol schemas.
    pub fn new(
        bridge_id: impl Into<String>,
        requested_extension_api_version: ExtensionApiVersion,
        protocol_families: impl IntoIterator<Item = BridgeProtocolFamilySchemas>,
    ) -> Self {
        Self {
            bridge_id: bridge_id.into(),
            requested_extension_api_version,
            protocol_families: protocol_families.into_iter().collect(),
            custom_payload_channels: Vec::new(),
        }
    }

    /// Replace the optional custom payload channel declaration.
    pub fn with_custom_payload_channels(
        mut self,
        channels: impl IntoIterator<Item = String>,
    ) -> Self {
        self.custom_payload_channels = channels.into_iter().collect();
        self
    }

    pub fn bridge_id(&self) -> &str {
        &self.bridge_id
    }

    pub const fn requested_extension_api_version(&self) -> ExtensionApiVersion {
        self.requested_extension_api_version
    }

    pub fn protocol_families(&self) -> &[BridgeProtocolFamilySchemas] {
        &self.protocol_families
    }

    pub fn custom_payload_channels(&self) -> &[String] {
        &self.custom_payload_channels
    }

    /// Validate this manifest into a canonical, migration-friendly DTO.
    pub fn validate(&self) -> Result<ValidatedClientBridgeManifest, ClientBridgeManifestError> {
        if !is_valid_bridge_id(&self.bridge_id) {
            return Err(ClientBridgeManifestError::InvalidBridgeId {
                bridge_id: self.bridge_id.clone(),
            });
        }

        if !supports_extension_api_version(self.requested_extension_api_version) {
            return Err(ClientBridgeManifestError::UnsupportedExtensionApiVersion {
                requested: self.requested_extension_api_version,
                supported: EXTENSION_API_VERSION,
            });
        }

        let mut validated_families = Vec::with_capacity(self.protocol_families.len());
        for family in &self.protocol_families {
            let normalized_family = normalize_protocol_family(&family.family)?;
            if validated_families
                .iter()
                .any(|existing: &ValidatedBridgeProtocolFamilySchemas| {
                    existing.family == normalized_family
                })
            {
                return Err(ClientBridgeManifestError::DuplicateProtocolFamily {
                    family: normalized_family,
                });
            }
            if family.schema_versions.is_empty() {
                return Err(ClientBridgeManifestError::MissingSchemaVersions {
                    family: normalized_family,
                });
            }

            let mut schema_versions = Vec::with_capacity(family.schema_versions.len());
            for version in &family.schema_versions {
                if *version == 0 {
                    return Err(ClientBridgeManifestError::InvalidSchemaVersion {
                        family: normalized_family,
                        version: *version,
                    });
                }
                if schema_versions.contains(version) {
                    return Err(ClientBridgeManifestError::DuplicateSchemaVersion {
                        family: normalized_family,
                        version: *version,
                    });
                }
                schema_versions.push(*version);
            }

            validated_families.push(ValidatedBridgeProtocolFamilySchemas {
                family: normalized_family,
                schema_versions,
            });
        }

        let mut custom_payload_channels = Vec::with_capacity(self.custom_payload_channels.len());
        for channel in &self.custom_payload_channels {
            if channel.contains('*') {
                return Err(ClientBridgeManifestError::UnboundedCustomPayloadChannel {
                    channel: channel.clone(),
                });
            }
            if !is_valid_namespaced_id(channel) {
                return Err(ClientBridgeManifestError::InvalidCustomPayloadChannel {
                    channel: channel.clone(),
                });
            }
            if custom_payload_channels.contains(channel) {
                return Err(ClientBridgeManifestError::DuplicateCustomPayloadChannel {
                    channel: channel.clone(),
                });
            }
            custom_payload_channels.push(channel.clone());
        }

        Ok(ValidatedClientBridgeManifest {
            bridge_id: self.bridge_id.clone(),
            requested_extension_api_version: self.requested_extension_api_version,
            protocol_families: validated_families,
            custom_payload_channels,
        })
    }
}

/// Supported schema versions for one extension protocol family.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BridgeProtocolFamilySchemas {
    family: String,
    schema_versions: Vec<u16>,
}

impl BridgeProtocolFamilySchemas {
    pub fn new(family: impl Into<String>, schema_versions: impl IntoIterator<Item = u16>) -> Self {
        Self {
            family: family.into(),
            schema_versions: schema_versions.into_iter().collect(),
        }
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn schema_versions(&self) -> &[u16] {
        &self.schema_versions
    }
}

/// Validated client-mod bridge manifest with canonical protocol family ids.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidatedClientBridgeManifest {
    bridge_id: String,
    requested_extension_api_version: ExtensionApiVersion,
    protocol_families: Vec<ValidatedBridgeProtocolFamilySchemas>,
    custom_payload_channels: Vec<String>,
}

impl ValidatedClientBridgeManifest {
    pub fn bridge_id(&self) -> &str {
        &self.bridge_id
    }

    pub const fn requested_extension_api_version(&self) -> ExtensionApiVersion {
        self.requested_extension_api_version
    }

    pub fn protocol_families(&self) -> &[ValidatedBridgeProtocolFamilySchemas] {
        &self.protocol_families
    }

    pub fn custom_payload_channels(&self) -> &[String] {
        &self.custom_payload_channels
    }
}

/// Validated schema versions for one canonical extension protocol family.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidatedBridgeProtocolFamilySchemas {
    family: String,
    schema_versions: Vec<u16>,
}

impl ValidatedBridgeProtocolFamilySchemas {
    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn schema_versions(&self) -> &[u16] {
        &self.schema_versions
    }
}

/// Reason a client bridge capability manifest failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientBridgeManifestError {
    InvalidBridgeId {
        bridge_id: String,
    },
    UnsupportedExtensionApiVersion {
        requested: ExtensionApiVersion,
        supported: ExtensionApiVersion,
    },
    InvalidProtocolFamily {
        family: String,
    },
    DuplicateProtocolFamily {
        family: String,
    },
    MissingSchemaVersions {
        family: String,
    },
    InvalidSchemaVersion {
        family: String,
        version: u16,
    },
    DuplicateSchemaVersion {
        family: String,
        version: u16,
    },
    InvalidCustomPayloadChannel {
        channel: String,
    },
    DuplicateCustomPayloadChannel {
        channel: String,
    },
    UnboundedCustomPayloadChannel {
        channel: String,
    },
}

fn normalize_protocol_family(family: &str) -> Result<String, ClientBridgeManifestError> {
    let normalized = family.trim().to_ascii_lowercase();
    if !is_valid_namespaced_id(&normalized) {
        return Err(ClientBridgeManifestError::InvalidProtocolFamily {
            family: family.to_owned(),
        });
    }
    Ok(normalized)
}

fn is_valid_bridge_id(bridge_id: &str) -> bool {
    if bridge_id.is_empty() || bridge_id.len() > MAX_BRIDGE_ID_LEN {
        return false;
    }

    let bytes = bridge_id.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| is_ascii_lower_alnum(*byte))
        && bytes.last().is_some_and(|byte| is_ascii_lower_alnum(*byte))
        && bytes
            .iter()
            .all(|byte| is_ascii_lower_alnum(*byte) || matches!(*byte, b'.' | b'-' | b'_'))
}

fn is_valid_namespaced_id(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_NAMESPACED_ID_LEN {
        return false;
    }

    let Some((namespace, path)) = value.split_once(':') else {
        return false;
    };
    if namespace.is_empty() || path.is_empty() {
        return false;
    }

    namespace
        .bytes()
        .all(|byte| is_ascii_lower_alnum(byte) || matches!(byte, b'.' | b'-' | b'_'))
        && path
            .bytes()
            .all(|byte| is_ascii_lower_alnum(byte) || matches!(byte, b'.' | b'-' | b'_' | b'/'))
}

fn is_ascii_lower_alnum(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
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
#[derive(Debug, Clone)]
pub struct ExtensionBoundary {
    event_tx: SyncSender<InboundEvent>,
    event_ready: Arc<Notify>,
    command_rx: Arc<Mutex<Receiver<OutboundCommand>>>,
    command_ready: Arc<Notify>,
}

impl ExtensionBoundary {
    /// Try to enqueue one inbound event without blocking server tasks.
    pub fn try_enqueue_event(&self, event: InboundEvent) -> Result<(), QueueError<InboundEvent>> {
        self.event_tx
            .try_send(event)
            .map(|()| self.event_ready.notify_one())
            .map_err(QueueError::from)
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
            match self.try_recv_command() {
                Err(QueueRecvError::Empty) => command_ready.await,
                result => return result,
            }
        }
    }
}

impl Drop for ExtensionBoundary {
    fn drop(&mut self) {
        self.event_ready.notify_one();
    }
}

/// Extension-host side of the boundary.
#[derive(Debug)]
pub struct ExtensionEndpoint {
    event_rx: Arc<Mutex<Receiver<InboundEvent>>>,
    event_ready: Arc<Notify>,
    command_tx: SyncSender<OutboundCommand>,
    command_ready: Arc<Notify>,
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
        self.command_tx
            .try_send(command)
            .map(|()| self.command_ready.notify_one())
            .map_err(QueueError::from)
    }
}

impl Drop for ExtensionEndpoint {
    fn drop(&mut self) {
        self.command_ready.notify_one();
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

    (
        ExtensionBoundary {
            event_tx,
            event_ready: Arc::clone(&event_ready),
            command_rx: Arc::new(Mutex::new(command_rx)),
            command_ready: Arc::clone(&command_ready),
        },
        ExtensionEndpoint {
            event_rx: Arc::new(Mutex::new(event_rx)),
            event_ready,
            command_tx,
            command_ready,
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

    #[test]
    fn client_bridge_manifest_validates_and_normalizes_protocol_families() {
        let manifest = ClientBridgeManifest::new(
            "solaris-client",
            EXTENSION_API_VERSION,
            [BridgeProtocolFamilySchemas::new(" Solaris:Loader ", [2, 1])],
        )
        .with_custom_payload_channels(["solaris:hello".to_owned(), "minecraft:brand".to_owned()]);

        let validated = manifest.validate().unwrap();

        assert_eq!(validated.bridge_id(), "solaris-client");
        assert_eq!(
            validated.requested_extension_api_version(),
            EXTENSION_API_VERSION
        );
        assert_eq!(validated.protocol_families().len(), 1);
        assert_eq!(validated.protocol_families()[0].family(), "solaris:loader");
        assert_eq!(validated.protocol_families()[0].schema_versions(), &[2, 1]);
        assert_eq!(
            validated.custom_payload_channels(),
            &["solaris:hello".to_owned(), "minecraft:brand".to_owned()]
        );
    }

    #[test]
    fn client_bridge_manifest_rejects_invalid_bridge_id_and_api_version() {
        let invalid_id = ClientBridgeManifest::new("Solaris Client", EXTENSION_API_VERSION, []);
        assert_eq!(
            invalid_id.validate(),
            Err(ClientBridgeManifestError::InvalidBridgeId {
                bridge_id: "Solaris Client".to_owned()
            })
        );

        let unsupported_api = ClientBridgeManifest::new(
            "solaris-client",
            ExtensionApiVersion::new(EXTENSION_API_VERSION.major() + 1, 0, 0),
            [],
        );
        assert_eq!(
            unsupported_api.validate(),
            Err(ClientBridgeManifestError::UnsupportedExtensionApiVersion {
                requested: ExtensionApiVersion::new(EXTENSION_API_VERSION.major() + 1, 0, 0),
                supported: EXTENSION_API_VERSION,
            })
        );
    }

    #[test]
    fn client_bridge_manifest_rejects_duplicate_protocol_families_after_normalization() {
        let manifest = ClientBridgeManifest::new(
            "solaris-client",
            EXTENSION_API_VERSION,
            [
                BridgeProtocolFamilySchemas::new("solaris:loader", [1]),
                BridgeProtocolFamilySchemas::new(" SOLARIS:LOADER ", [2]),
            ],
        );

        assert_eq!(
            manifest.validate(),
            Err(ClientBridgeManifestError::DuplicateProtocolFamily {
                family: "solaris:loader".to_owned()
            })
        );
    }

    #[test]
    fn client_bridge_manifest_rejects_invalid_protocol_family() {
        let manifest = ClientBridgeManifest::new(
            "solaris-client",
            EXTENSION_API_VERSION,
            [BridgeProtocolFamilySchemas::new("solaris loader", [1])],
        );

        assert_eq!(
            manifest.validate(),
            Err(ClientBridgeManifestError::InvalidProtocolFamily {
                family: "solaris loader".to_owned()
            })
        );
    }

    #[test]
    fn client_bridge_manifest_rejects_invalid_protocol_family_schema_versions() {
        let empty_versions = ClientBridgeManifest::new(
            "solaris-client",
            EXTENSION_API_VERSION,
            [BridgeProtocolFamilySchemas::new("solaris:loader", [])],
        );
        assert_eq!(
            empty_versions.validate(),
            Err(ClientBridgeManifestError::MissingSchemaVersions {
                family: "solaris:loader".to_owned()
            })
        );

        let zero_version = ClientBridgeManifest::new(
            "solaris-client",
            EXTENSION_API_VERSION,
            [BridgeProtocolFamilySchemas::new("solaris:loader", [0])],
        );
        assert_eq!(
            zero_version.validate(),
            Err(ClientBridgeManifestError::InvalidSchemaVersion {
                family: "solaris:loader".to_owned(),
                version: 0,
            })
        );

        let duplicate_version = ClientBridgeManifest::new(
            "solaris-client",
            EXTENSION_API_VERSION,
            [BridgeProtocolFamilySchemas::new("solaris:loader", [1, 1])],
        );
        assert_eq!(
            duplicate_version.validate(),
            Err(ClientBridgeManifestError::DuplicateSchemaVersion {
                family: "solaris:loader".to_owned(),
                version: 1,
            })
        );
    }

    #[test]
    fn client_bridge_manifest_rejects_duplicate_and_invalid_custom_payload_channels() {
        let duplicate = ClientBridgeManifest::new("solaris-client", EXTENSION_API_VERSION, [])
            .with_custom_payload_channels(["solaris:hello".to_owned(), "solaris:hello".to_owned()]);
        assert_eq!(
            duplicate.validate(),
            Err(ClientBridgeManifestError::DuplicateCustomPayloadChannel {
                channel: "solaris:hello".to_owned()
            })
        );

        let invalid = ClientBridgeManifest::new("solaris-client", EXTENSION_API_VERSION, [])
            .with_custom_payload_channels(["Solaris:Hello".to_owned()]);
        assert_eq!(
            invalid.validate(),
            Err(ClientBridgeManifestError::InvalidCustomPayloadChannel {
                channel: "Solaris:Hello".to_owned()
            })
        );
    }

    #[test]
    fn client_bridge_manifest_rejects_unbounded_wildcard_custom_payload_channels() {
        let manifest = ClientBridgeManifest::new("solaris-client", EXTENSION_API_VERSION, [])
            .with_custom_payload_channels(["solaris:*".to_owned()]);

        assert_eq!(
            manifest.validate(),
            Err(ClientBridgeManifestError::UnboundedCustomPayloadChannel {
                channel: "solaris:*".to_owned()
            })
        );
    }
}
