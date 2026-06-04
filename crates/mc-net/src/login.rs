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
use mc_protocol::packets::login::{
    LoginAcknowledged, LoginDisconnect, LoginStart, LoginSuccess, SetCompression,
};
use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::info;
use uuid::Uuid;

use crate::connection::{read_packet, write_packet};
use crate::error::ConnectionError;

pub(crate) const LOGIN_COMPRESSION_THRESHOLD: i32 = 256;

/// Outcome of a successful login. Returned so the caller (`server.rs`)
/// has the information it needs to proceed into the Configuration state
/// once that lands in M1.e.
#[derive(Debug, Clone)]
pub(crate) struct LoggedInProfile {
    pub uuid: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct LoginAccessConfig {
    pub online_mode: bool,
    pub whitelist_enabled: bool,
    pub whitelist: std::collections::BTreeSet<String>,
    pub banned_players: std::collections::BTreeSet<String>,
}

impl LoginAccessConfig {
    #[must_use]
    pub fn offline_only() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn normalized<I, J, S, T>(
        online_mode: bool,
        whitelist_enabled: bool,
        whitelist: I,
        banned_players: J,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: Into<String>,
        T: Into<String>,
    {
        Self {
            online_mode,
            whitelist_enabled,
            whitelist: normalize_access_set(whitelist),
            banned_players: normalize_access_set(banned_players),
        }
    }
}

fn normalize_access_set<I, S>(entries: I) -> std::collections::BTreeSet<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    entries
        .into_iter()
        .map(Into::into)
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginRejection {
    OnlineModeUnsupported,
    Banned,
    Whitelist,
}

impl LoginRejection {
    fn message(self) -> &'static str {
        match self {
            Self::OnlineModeUnsupported => {
                "Solaris M89 only supports offline-mode private/local authentication"
            }
            Self::Banned => "You are banned from this Solaris server",
            Self::Whitelist => "You are not whitelisted on this Solaris server",
        }
    }
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
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    compression_threshold: i32,
    compression: &mut Compression,
    compression_level: Option<u32>,
    access: &LoginAccessConfig,
) -> Result<Option<LoggedInProfile>, ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let login_start =
        read_packet::<LoginStart, _>(reader, buf, Compression::Disabled, State::Login).await?;
    // Offline mode: ignore the UUID the client just sent us and stamp our
    // own derived one. This is what vanilla does too — clients always send
    // *some* UUID (since 1.20.2 it is mandatory in LoginStart) but it is
    // not authoritative.
    let uuid = offline_uuid(&login_start.name);
    let name = login_start.name;
    if let Some(rejection) = access_rejection(access, &name, uuid) {
        write_login_disconnect(writer, rejection.message()).await?;
        info!(player = %name, %uuid, reason = ?rejection, "login rejected");
        return Ok(None);
    }
    info!(player = %name, %uuid, "offline login");

    let compression_threshold = compression_threshold.max(0);
    write_packet(
        writer,
        &SetCompression {
            threshold: compression_threshold,
        },
        Compression::Disabled,
    )
    .await?;
    *compression = match compression_level {
        Some(level) => Compression::Threshold(compression_threshold as usize).with_level(level),
        None => Compression::Threshold(compression_threshold as usize),
    };

    let success = LoginSuccess {
        uuid,
        name: name.clone(),
        properties: Vec::new(),
    };
    write_packet(writer, &success, *compression).await?;

    let _ack = read_packet::<LoginAcknowledged, _>(reader, buf, *compression, State::Login).await?;

    Ok(Some(LoggedInProfile { uuid, name }))
}

pub(crate) fn access_rejection(
    access: &LoginAccessConfig,
    name: &str,
    uuid: Uuid,
) -> Option<LoginRejection> {
    if access.online_mode {
        return Some(LoginRejection::OnlineModeUnsupported);
    }
    let name = name.to_ascii_lowercase();
    let uuid = uuid.to_string().to_ascii_lowercase();
    if access.banned_players.contains(&name) || access.banned_players.contains(&uuid) {
        return Some(LoginRejection::Banned);
    }
    if access.whitelist_enabled
        && !access.whitelist.contains(&name)
        && !access.whitelist.contains(&uuid)
    {
        return Some(LoginRejection::Whitelist);
    }
    None
}

async fn write_login_disconnect<W>(writer: &mut W, reason: &str) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &LoginDisconnect {
            reason_json: serde_json::json!({ "text": reason }).to_string(),
        },
        Compression::Disabled,
    )
    .await
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

    #[test]
    fn access_rejection_fails_closed_for_online_mode() {
        let access =
            LoginAccessConfig::normalized(true, false, ["notch"], std::iter::empty::<&str>());
        assert_eq!(
            access_rejection(&access, "Notch", offline_uuid("Notch")),
            Some(LoginRejection::OnlineModeUnsupported)
        );
    }

    #[test]
    fn access_rejection_prefers_ban_over_whitelist() {
        let access = LoginAccessConfig::normalized(
            false,
            true,
            ["notch"],
            [offline_uuid("Notch").to_string()],
        );
        assert_eq!(
            access_rejection(&access, "Notch", offline_uuid("Notch")),
            Some(LoginRejection::Banned)
        );
    }

    #[test]
    fn access_rejection_enforces_whitelist_by_name_or_uuid() {
        let denied =
            LoginAccessConfig::normalized(false, true, ["alex"], std::iter::empty::<&str>());
        assert_eq!(
            access_rejection(&denied, "Notch", offline_uuid("Notch")),
            Some(LoginRejection::Whitelist)
        );

        let allowed_by_name =
            LoginAccessConfig::normalized(false, true, ["notch"], std::iter::empty::<&str>());
        assert_eq!(
            access_rejection(&allowed_by_name, "Notch", offline_uuid("Notch")),
            None
        );

        let allowed_by_uuid = LoginAccessConfig::normalized(
            false,
            true,
            [offline_uuid("Notch").to_string()],
            std::iter::empty::<&str>(),
        );
        assert_eq!(
            access_rejection(&allowed_by_uuid, "Notch", offline_uuid("Notch")),
            None
        );
    }
}
