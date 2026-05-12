//! Login state handler — offline-mode only.
//!
//! M1.d: drive the client from `LoginStart` through `LoginSuccess` and
//! `LoginAcknowledged`. After acknowledgement the connection is in the
//! Configuration state, which is M1.e; for now we just drop the
//! connection, which surfaces to the vanilla client as a generic
//! "connection lost" message.
//!
//! Online-mode (encryption + Mojang session-server authentication) is a
//! deliberate later milestone.

use bytes::BytesMut;
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::info;
use uuid::Uuid;

use crate::connection::{read_packet, write_packet};
use crate::error::ConnectionError;

/// Outcome of a successful login. Returned so the caller (`server.rs`)
/// has the information it needs to proceed into the Configuration state
/// once that lands in M1.e.
#[derive(Debug, Clone)]
pub(crate) struct LoggedInProfile {
    pub uuid: Uuid,
    pub name: String,
}

/// Java's `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`, byte-for-byte.
///
/// This is the de-facto standard offline-mode UUID derivation. Reproducing
/// it exactly means a player who connects to a vanilla offline server and
/// later to a Solaris offline server keeps the same UUID, so any external
/// persistence (whitelists, permissions, claims) survives the migration.
#[must_use]
pub fn offline_uuid(name: &str) -> Uuid {
    let mut hasher = Md5::new();
    hasher.update(b"OfflinePlayer:");
    hasher.update(name.as_bytes());
    let mut bytes: [u8; 16] = hasher.finalize().into();
    // Set version 3 (name-based MD5) and the RFC 4122 variant, matching
    // `UUID.nameUUIDFromBytes`.
    bytes[6] = (bytes[6] & 0x0F) | 0x30;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    Uuid::from_bytes(bytes)
}

pub(crate) async fn handle<R, W>(
    mut reader: R,
    mut writer: W,
    mut buf: BytesMut,
) -> Result<LoggedInProfile, ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let login_start =
        read_packet::<LoginStart, _>(&mut reader, &mut buf, Compression::Disabled, State::Login)
            .await?;
    // Offline mode: ignore the UUID the client just sent us and stamp our
    // own derived one. This is what vanilla does too — clients always send
    // *some* UUID (since 1.20.2 it is mandatory in LoginStart) but it is
    // not authoritative.
    let uuid = offline_uuid(&login_start.name);
    let name = login_start.name;
    info!(player = %name, %uuid, "offline login");

    let success = LoginSuccess {
        uuid,
        name: name.clone(),
        properties: Vec::new(),
    };
    write_packet(&mut writer, &success, Compression::Disabled).await?;

    let _ack = read_packet::<LoginAcknowledged, _>(
        &mut reader,
        &mut buf,
        Compression::Disabled,
        State::Login,
    )
    .await?;

    Ok(LoggedInProfile { uuid, name })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard against accidentally changing the offline UUID
    /// derivation. The values pinned here were captured from this
    /// implementation, which faithfully implements
    /// `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)` (MD5, then
    /// version/variant bit massage). If `md-5` ever produced a different
    /// digest, or someone "improved" the prefix here, every existing
    /// player's UUID would silently change — these pins surface that as
    /// a test failure.
    #[test]
    fn offline_uuid_is_pinned() {
        let cases: &[(&str, &str)] = &[
            ("Notch", "b50ad385-829d-3141-a216-7e7d7539ba7f"),
            ("Player", "a01e3843-e521-3998-958a-f459800e4d11"),
            ("jeb_", "a762f560-4fce-3236-812a-b80efff0b62b"),
            ("Steve", "5627dd98-e6be-3c21-b8a8-e92344183641"),
            ("Dinnerbone", "4d258a81-2358-3084-8166-05b9faccad80"),
        ];
        for (name, expected) in cases {
            let got = offline_uuid(name);
            assert_eq!(
                got,
                Uuid::parse_str(expected).unwrap(),
                "name {name:?}: expected {expected}, got {got}",
            );
        }
    }

    #[test]
    fn offline_uuid_is_version_3_variant_rfc4122() {
        let uuid = offline_uuid("anyone");
        let bytes = *uuid.as_bytes();
        assert_eq!(bytes[6] >> 4, 3, "version nibble");
        assert_eq!(bytes[8] >> 6, 0b10, "RFC 4122 variant");
    }

    #[test]
    fn offline_uuid_is_deterministic() {
        assert_eq!(offline_uuid("repeat"), offline_uuid("repeat"));
        assert_ne!(offline_uuid("a"), offline_uuid("b"));
    }
}
