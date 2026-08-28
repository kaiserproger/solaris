//! Login state handler for offline and Mojang-authenticated online mode.
//!
//! Online mode performs the RSA challenge, enables the continuous AES-CFB8
//! transport, verifies the account with the session service, and only then
//! enables compression and enters Configuration.

use std::net::IpAddr;
use std::sync::Arc;

use bytes::BytesMut;
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::login::{
    GameProfileProperty, LoginAcknowledged, LoginDisconnect, LoginStart, LoginSuccess,
    SetCompression,
};
use md5::{Digest, Md5};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::info;
use uuid::Uuid;

use crate::connection::{
    ConnectionReader, ConnectionWriter, PRE_PLAY_READ_TIMEOUT, PrePlayBudget,
    read_packet_with_timeout_budgeted, write_packet,
};
use crate::error::ConnectionError;
use crate::session_auth::{
    RsaIdentity, SessionVerifier, VerifiedSession, VerifySession, VerifySessionError,
    minecraft_server_hash,
};

pub(crate) const LOGIN_COMPRESSION_THRESHOLD: i32 = 256;

/// Outcome of a successful login. Returned so the caller (`server.rs`)
/// has the information it needs to proceed into the Configuration state
/// once that lands in M1.e.
#[derive(Debug, Clone)]
pub(crate) struct LoggedInProfile {
    pub uuid: Uuid,
    pub name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct LoginOutcome {
    pub profile: LoggedInProfile,
    pub properties: Vec<GameProfileProperty>,
}

#[derive(Debug, Clone, Default)]
pub struct LoginAccessConfig {
    pub online_mode: bool,
    pub whitelist_enabled: bool,
    pub whitelist: std::collections::BTreeSet<String>,
    pub banned_players: std::collections::BTreeSet<String>,
    session_verifier: Option<Arc<dyn SessionVerifier>>,
    prevent_proxy_connections: bool,
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
            session_verifier: None,
            prevent_proxy_connections: false,
        }
    }

    #[must_use]
    pub fn with_session_verifier(mut self, verifier: Arc<dyn SessionVerifier>) -> Self {
        self.session_verifier = Some(verifier);
        self
    }

    #[must_use]
    pub fn with_prevent_proxy_connections(mut self, enabled: bool) -> Self {
        self.prevent_proxy_connections = enabled;
        self
    }

    pub(crate) fn session_verifier(&self) -> Option<Arc<dyn SessionVerifier>> {
        self.session_verifier.clone()
    }

    pub(crate) fn prevent_proxy_connections(&self) -> bool {
        self.prevent_proxy_connections
    }
}

#[derive(Debug)]
pub(crate) struct OnlineAuthentication {
    identity: RsaIdentity,
    verifier: Arc<dyn SessionVerifier>,
    prevent_proxy_connections: bool,
}

impl OnlineAuthentication {
    pub(crate) fn new(
        identity: RsaIdentity,
        verifier: Arc<dyn SessionVerifier>,
        prevent_proxy_connections: bool,
    ) -> Self {
        Self {
            identity,
            verifier,
            prevent_proxy_connections,
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
    Banned,
    Whitelist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginNameError {
    Length { bytes: usize },
    InvalidCharacter { index: usize, byte: u8 },
}

pub(crate) fn validate_login_name(name: &str) -> Result<(), LoginNameError> {
    if !(3..=16).contains(&name.len()) {
        return Err(LoginNameError::Length { bytes: name.len() });
    }
    for (index, byte) in name.bytes().enumerate() {
        if !byte.is_ascii_alphanumeric() && byte != b'_' {
            return Err(LoginNameError::InvalidCharacter { index, byte });
        }
    }
    Ok(())
}

impl LoginRejection {
    fn message(self) -> &'static str {
        match self {
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle<R, W>(
    reader: &mut ConnectionReader<R>,
    writer: &mut ConnectionWriter<W>,
    buf: &mut BytesMut,
    budget: &mut PrePlayBudget,
    compression_threshold: i32,
    compression: &mut Compression,
    compression_level: Option<u32>,
    access: &LoginAccessConfig,
    online_authentication: Option<&OnlineAuthentication>,
    peer_ip: IpAddr,
) -> Result<Option<LoginOutcome>, ConnectionError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let login_start = read_packet_with_timeout_budgeted::<LoginStart, _>(
        reader,
        buf,
        Compression::Disabled,
        State::Login,
        PRE_PLAY_READ_TIMEOUT,
        budget,
    )
    .await?;
    let requested_name = login_start.name;
    if let Err(reason) = validate_login_name(&requested_name) {
        write_login_disconnect(writer, "Invalid username").await?;
        info!(
            ?reason,
            name_bytes = requested_name.len(),
            "login rejected: invalid username syntax"
        );
        return Ok(None);
    }
    let (name, uuid, properties) = if access.online_mode {
        let authentication = online_authentication.ok_or(ConnectionError::OnlineAuthentication(
            "server authentication context is unavailable",
        ))?;
        let challenge = authentication.identity.new_challenge();
        write_packet(
            writer,
            &mc_protocol::packets::login::EncryptionRequest {
                server_id: String::new(),
                public_key: authentication.identity.public_key_der().to_vec(),
                verify_token: challenge.to_vec(),
                should_authenticate: true,
            },
            Compression::Disabled,
        )
        .await?;
        let response = read_packet_with_timeout_budgeted::<
            mc_protocol::packets::login::EncryptionResponse,
            _,
        >(
            reader,
            buf,
            Compression::Disabled,
            State::Login,
            PRE_PLAY_READ_TIMEOUT,
            budget,
        )
        .await?;
        let shared_secret = authentication
            .identity
            .decrypt_response(
                &response.encrypted_shared_secret,
                &response.encrypted_verify_token,
                challenge,
            )
            .map_err(|_| ConnectionError::OnlineAuthentication("invalid encryption response"))?;
        reader.enable_encryption(&shared_secret, buf)?;
        writer.enable_encryption(&shared_secret)?;

        let verified = authentication
            .verifier
            .verify(VerifySession {
                username: requested_name.clone(),
                server_id_hash: minecraft_server_hash(
                    b"",
                    &shared_secret,
                    authentication.identity.public_key_der(),
                ),
                client_ip: authentication.prevent_proxy_connections.then_some(peer_ip),
            })
            .await;
        let VerifiedSession {
            uuid,
            name,
            properties,
        } = match verified {
            Ok(profile) => profile,
            Err(VerifySessionError::Unverified) => {
                write_login_disconnect(writer, "Failed to verify username!").await?;
                info!(player = %requested_name, "online login rejected: unverified session");
                return Ok(None);
            }
            Err(VerifySessionError::Unavailable) => {
                write_login_disconnect(
                    writer,
                    "Authentication servers are down. Please try again later, sorry!",
                )
                .await?;
                info!(player = %requested_name, "online login rejected: session service unavailable");
                return Ok(None);
            }
        };
        if let Err(reason) = validate_login_name(&name) {
            write_login_disconnect(writer, "Failed to verify username!").await?;
            info!(
                ?reason,
                returned_name_bytes = name.len(),
                "online login rejected: verifier returned an invalid username"
            );
            return Ok(None);
        }
        if !name.eq_ignore_ascii_case(&requested_name) {
            write_login_disconnect(writer, "Failed to verify username!").await?;
            info!(
                requested_name_bytes = requested_name.len(),
                returned_name_bytes = name.len(),
                "online login rejected: verifier returned a different username"
            );
            return Ok(None);
        }
        info!(player = %name, requested = %requested_name, %uuid, "online login verified");
        (name, uuid, properties)
    } else {
        // Offline mode ignores the client UUID and derives the vanilla UUID.
        let uuid = offline_uuid(&requested_name);
        info!(player = %requested_name, %uuid, "offline login");
        (requested_name, uuid, Vec::new())
    };
    if let Some(rejection) = access_rejection(access, &name, uuid) {
        write_login_disconnect(writer, rejection.message()).await?;
        info!(player = %name, %uuid, reason = ?rejection, "login rejected");
        return Ok(None);
    }

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
        properties: properties.clone(),
    };
    write_packet(writer, &success, *compression).await?;

    let _ack = read_packet_with_timeout_budgeted::<LoginAcknowledged, _>(
        reader,
        buf,
        *compression,
        State::Login,
        PRE_PLAY_READ_TIMEOUT,
        budget,
    )
    .await?;

    Ok(Some(LoginOutcome {
        profile: LoggedInProfile { uuid, name },
        properties,
    }))
}

pub(crate) fn access_rejection(
    access: &LoginAccessConfig,
    name: &str,
    uuid: Uuid,
) -> Option<LoginRejection> {
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
    W: AsyncWrite + Unpin,
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
    use std::sync::Mutex;

    use super::*;
    use crate::connection::read_packet_with_timeout;
    use mc_protocol::packets::login::{
        EncryptionRequest, EncryptionResponse, GameProfileProperty, LoginAcknowledged, LoginStart,
        LoginSuccess, SetCompression,
    };
    use rsa::pkcs8::DecodePublicKey;
    use rsa::{Pkcs1v15Encrypt, RsaPublicKey};

    #[derive(Debug)]
    struct RecordingVerifier {
        requests: Mutex<Vec<VerifySession>>,
        result: VerifiedSession,
    }

    impl SessionVerifier for RecordingVerifier {
        fn verify(&self, request: VerifySession) -> crate::SessionVerifierFuture<'_> {
            self.requests.lock().unwrap().push(request);
            let result = self.result.clone();
            Box::pin(async move { Ok(result) })
        }
    }

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
    fn login_name_accepts_exact_boundaries_and_ascii_alphabet() {
        for name in ["abc", "A_1", "abcdefghijklmnop", "Player_123"] {
            assert_eq!(validate_login_name(name), Ok(()), "name {name:?}");
        }
    }

    #[test]
    fn login_name_rejects_length_controls_spaces_punctuation_and_unicode() {
        for name in [
            "",
            "ab",
            "abcdefghijklmnopq",
            "abc\n",
            "abc\r",
            "abc def",
            "abc-def",
            "abc.dev",
            "éclair",
            "玩家123",
        ] {
            assert!(validate_login_name(name).is_err(), "name {name:?}");
        }
    }

    #[tokio::test]
    async fn online_login_encrypts_before_compression_and_uses_verified_profile() {
        const SHARED_SECRET: [u8; 16] = *b"0123456789abcdef";
        let verified_uuid = Uuid::parse_str("12345678-1234-5678-9abc-def012345678").unwrap();
        let properties = vec![GameProfileProperty {
            name: "textures".to_owned(),
            value: "texture-value".to_owned(),
            signature: Some("texture-signature".to_owned()),
        }];
        let verifier = Arc::new(RecordingVerifier {
            requests: Mutex::new(Vec::new()),
            result: VerifiedSession {
                uuid: verified_uuid,
                name: "OnlinePlayer".to_owned(),
                properties: properties.clone(),
            },
        });
        let authentication =
            OnlineAuthentication::new(RsaIdentity::generate().unwrap(), verifier.clone(), true);
        let access = LoginAccessConfig::normalized(
            true,
            false,
            std::iter::empty::<&str>(),
            std::iter::empty::<&str>(),
        );
        let (server_io, client_io) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server_io);
        let (client_read, client_write) = tokio::io::split(client_io);
        let mut server_reader = ConnectionReader::new(server_read);
        let mut server_writer = ConnectionWriter::new(server_write);
        let mut client_reader = ConnectionReader::new(client_read);
        let mut client_writer = ConnectionWriter::new(client_write);
        let mut server_buf = BytesMut::new();
        let mut client_buf = BytesMut::new();
        let mut server_compression = Compression::Disabled;
        let mut budget = PrePlayBudget::new(
            crate::connection::MAX_PRE_PLAY_PACKETS,
            crate::connection::MAX_PRE_PLAY_BYTES,
        );

        let server = async {
            handle(
                &mut server_reader,
                &mut server_writer,
                &mut server_buf,
                &mut budget,
                LOGIN_COMPRESSION_THRESHOLD,
                &mut server_compression,
                None,
                &access,
                Some(&authentication),
                "203.0.113.9".parse().unwrap(),
            )
            .await
            .unwrap()
            .unwrap()
        };
        let client = async {
            write_packet(
                &mut client_writer,
                &LoginStart {
                    name: "onlineplayer".to_owned(),
                    player_uuid: Uuid::nil(),
                },
                Compression::Disabled,
            )
            .await
            .unwrap();
            let request = read_packet_with_timeout::<EncryptionRequest, _>(
                &mut client_reader,
                &mut client_buf,
                Compression::Disabled,
                State::Login,
                PRE_PLAY_READ_TIMEOUT,
            )
            .await
            .unwrap();
            let public_key = RsaPublicKey::from_public_key_der(&request.public_key).unwrap();
            let mut rng = rsa::rand_core::OsRng;
            let encrypted_shared_secret = public_key
                .encrypt(&mut rng, Pkcs1v15Encrypt, &SHARED_SECRET)
                .unwrap();
            let encrypted_verify_token = public_key
                .encrypt(&mut rng, Pkcs1v15Encrypt, &request.verify_token)
                .unwrap();
            write_packet(
                &mut client_writer,
                &EncryptionResponse {
                    encrypted_shared_secret,
                    encrypted_verify_token,
                },
                Compression::Disabled,
            )
            .await
            .unwrap();
            client_reader
                .enable_encryption(&SHARED_SECRET, &mut client_buf)
                .unwrap();
            client_writer.enable_encryption(&SHARED_SECRET).unwrap();

            let set_compression = read_packet_with_timeout::<SetCompression, _>(
                &mut client_reader,
                &mut client_buf,
                Compression::Disabled,
                State::Login,
                PRE_PLAY_READ_TIMEOUT,
            )
            .await
            .unwrap();
            let compression = Compression::Threshold(set_compression.threshold as usize);
            let success = read_packet_with_timeout::<LoginSuccess, _>(
                &mut client_reader,
                &mut client_buf,
                compression,
                State::Login,
                PRE_PLAY_READ_TIMEOUT,
            )
            .await
            .unwrap();
            write_packet(&mut client_writer, &LoginAcknowledged, compression)
                .await
                .unwrap();
            let expected_hash = minecraft_server_hash(b"", &SHARED_SECRET, &request.public_key);
            (success, expected_hash)
        };

        let (outcome, (success, expected_hash)) = tokio::join!(server, client);

        assert_eq!(outcome.profile.uuid, verified_uuid);
        assert_eq!(outcome.profile.name, "OnlinePlayer");
        assert_eq!(outcome.properties, properties);
        assert_eq!(success.uuid, verified_uuid);
        assert_eq!(success.name, "OnlinePlayer");
        assert_eq!(success.properties, properties);
        let requests = verifier.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].username, "onlineplayer");
        assert_eq!(requests[0].server_id_hash, expected_hash);
        assert_eq!(requests[0].client_ip, Some("203.0.113.9".parse().unwrap()));
    }

    #[test]
    fn access_rejection_allows_verified_online_profile() {
        let access =
            LoginAccessConfig::normalized(true, false, ["notch"], std::iter::empty::<&str>());
        assert_eq!(
            access_rejection(&access, "Notch", offline_uuid("Notch")),
            None
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
