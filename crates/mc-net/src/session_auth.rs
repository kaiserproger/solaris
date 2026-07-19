use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;

use mc_protocol::packets::login::GameProfileProperty;
use num_bigint::BigInt;
use rsa::pkcs8::EncodePublicKey;
use rsa::rand_core::{OsRng, RngCore};
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest, Sha1};
use thiserror::Error;

const RSA_KEY_BITS: usize = 1024;
const MOJANG_HAS_JOINED_URL: &str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// RSA identity used by the Minecraft login encryption handshake.
pub struct RsaIdentity {
    private_key: RsaPrivateKey,
    public_key_der: Vec<u8>,
}

impl fmt::Debug for RsaIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RsaIdentity")
            .field("public_key_der_len", &self.public_key_der.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RsaIdentityError {
    #[error("RSA operation failed")]
    Crypto,
    #[error("encrypted login response contained the wrong challenge")]
    InvalidChallenge,
    #[error("shared secret must contain exactly 16 bytes, got {0}")]
    InvalidSharedSecretLength(usize),
}

impl RsaIdentity {
    pub fn generate() -> Result<Self, RsaIdentityError> {
        let private_key =
            RsaPrivateKey::new(&mut OsRng, RSA_KEY_BITS).map_err(|_| RsaIdentityError::Crypto)?;
        let public_key_der = RsaPublicKey::from(&private_key)
            .to_public_key_der()
            .map_err(|_| RsaIdentityError::Crypto)?
            .as_bytes()
            .to_vec();
        Ok(Self {
            private_key,
            public_key_der,
        })
    }

    pub fn public_key_der(&self) -> &[u8] {
        &self.public_key_der
    }

    pub fn new_challenge(&self) -> [u8; 4] {
        let mut challenge = [0; 4];
        OsRng.fill_bytes(&mut challenge);
        challenge
    }

    pub fn decrypt_response(
        &self,
        encrypted_secret: &[u8],
        encrypted_challenge: &[u8],
        expected_challenge: [u8; 4],
    ) -> Result<[u8; 16], RsaIdentityError> {
        let challenge = self
            .private_key
            .decrypt(Pkcs1v15Encrypt, encrypted_challenge)
            .map_err(|_| RsaIdentityError::Crypto)?;
        if challenge != expected_challenge {
            return Err(RsaIdentityError::InvalidChallenge);
        }

        let secret = self
            .private_key
            .decrypt(Pkcs1v15Encrypt, encrypted_secret)
            .map_err(|_| RsaIdentityError::Crypto)?;
        let secret_len = secret.len();
        secret
            .try_into()
            .map_err(|_| RsaIdentityError::InvalidSharedSecretLength(secret_len))
    }
}

/// Computes the vanilla login hash. `server_id` contains ISO-8859-1 bytes.
pub fn minecraft_server_hash(
    server_id: &[u8],
    shared_secret: &[u8; 16],
    public_key_der: &[u8],
) -> String {
    let mut digest = Sha1::new();
    digest.update(server_id);
    digest.update(shared_secret);
    digest.update(public_key_der);
    java_signed_hex(&digest.finalize())
}

fn java_signed_hex(bytes: &[u8]) -> String {
    BigInt::from_signed_bytes_be(bytes).to_str_radix(16)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifySession {
    pub username: String,
    pub server_id_hash: String,
    pub client_ip: Option<IpAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSession {
    pub uuid: uuid::Uuid,
    pub properties: Vec<GameProfileProperty>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum VerifySessionError {
    #[error("the session server did not verify this account")]
    Unverified,
    #[error("the session server is unavailable")]
    Unavailable,
}

pub type SessionVerifierFuture<'a> =
    Pin<Box<dyn Future<Output = Result<VerifiedSession, VerifySessionError>> + Send + 'a>>;

pub trait SessionVerifier: Send + Sync + fmt::Debug {
    fn verify(&self, request: VerifySession) -> SessionVerifierFuture<'_>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SessionVerifierBuildError {
    #[error("invalid session verification URL")]
    InvalidBaseUrl,
    #[error("failed to construct HTTP client")]
    HttpClient,
}

#[derive(Debug, Clone)]
pub struct MojangSessionVerifier {
    client: reqwest::Client,
    base_url: reqwest::Url,
}

impl MojangSessionVerifier {
    pub fn new() -> Result<Self, SessionVerifierBuildError> {
        Self::with_base_url(
            MOJANG_HAS_JOINED_URL,
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_REQUEST_TIMEOUT,
        )
    }

    pub fn with_base_url(
        base_url: &str,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, SessionVerifierBuildError> {
        let base_url =
            reqwest::Url::parse(base_url).map_err(|_| SessionVerifierBuildError::InvalidBaseUrl)?;
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .build()
            .map_err(|_| SessionVerifierBuildError::HttpClient)?;
        Ok(Self { client, base_url })
    }
}

impl SessionVerifier for MojangSessionVerifier {
    fn verify(&self, request: VerifySession) -> SessionVerifierFuture<'_> {
        Box::pin(async move {
            let mut query = vec![
                ("username", request.username),
                ("serverId", request.server_id_hash),
            ];
            if let Some(client_ip) = request.client_ip {
                query.push(("ip", client_ip.to_string()));
            }

            let response = self
                .client
                .get(self.base_url.clone())
                .query(&query)
                .send()
                .await
                .map_err(|_| VerifySessionError::Unavailable)?;
            let status = response.status();
            let body = response
                .bytes()
                .await
                .map_err(|_| VerifySessionError::Unavailable)?;

            if status.is_client_error() {
                return Err(VerifySessionError::Unverified);
            }

            let response: HasJoinedResponse =
                serde_json::from_slice(&body).map_err(|_| VerifySessionError::Unverified)?;
            if status.is_server_error() {
                return Err(VerifySessionError::Unavailable);
            }
            if !status.is_success() {
                return Err(VerifySessionError::Unverified);
            }

            let uuid = response
                .id
                .ok_or(VerifySessionError::Unverified)
                .and_then(|id| {
                    uuid::Uuid::parse_str(&id).map_err(|_| VerifySessionError::Unverified)
                })?;
            let properties = response
                .properties
                .unwrap_or_default()
                .into_iter()
                .map(HasJoinedProperty::into_game_profile_property)
                .collect();
            Ok(VerifiedSession { uuid, properties })
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct HasJoinedResponse {
    id: Option<String>,
    #[serde(default)]
    properties: Option<Vec<HasJoinedProperty>>,
}

#[derive(Debug, serde::Deserialize)]
struct HasJoinedProperty {
    name: String,
    value: String,
    #[serde(default)]
    signature: Option<String>,
}

impl HasJoinedProperty {
    fn into_game_profile_property(self) -> GameProfileProperty {
        GameProfileProperty {
            name: self.name,
            value: self.value,
            signature: self.signature,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use mc_protocol::packets::login::GameProfileProperty;
    use rsa::pkcs8::DecodePublicKey;
    use rsa::traits::PublicKeyParts;
    use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use uuid::Uuid;

    use super::{
        MojangSessionVerifier, RsaIdentity, RsaIdentityError, SessionVerifier, VerifySession,
        VerifySessionError, java_signed_hex, minecraft_server_hash,
    };

    async fn local_response(status: &str, body: &str) -> (String, oneshot::Receiver<String>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local HTTP listener");
        let address = listener.local_addr().expect("local HTTP address");
        let status = status.to_owned();
        let body = body.to_owned();
        let (request_tx, request_rx) = oneshot::channel();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept HTTP request");
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let read = stream.read(&mut chunk).await.expect("read HTTP request");
                assert!(read > 0, "client closed before completing HTTP headers");
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = request_tx.send(String::from_utf8(request).expect("HTTP request is ASCII"));

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write HTTP response");
        });

        (
            format!("http://{address}/session/minecraft/hasJoined"),
            request_rx,
        )
    }

    fn verifier(base_url: &str) -> MojangSessionVerifier {
        MojangSessionVerifier::with_base_url(
            base_url,
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .expect("build local session verifier")
    }

    fn request() -> VerifySession {
        VerifySession {
            username: "A B+?".to_owned(),
            server_id_hash: "-abc123".to_owned(),
            client_ip: Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9))),
        }
    }

    #[test]
    fn java_signed_hex_matches_big_integer_edges() {
        assert_eq!(java_signed_hex(&[0]), "0");
        assert_eq!(java_signed_hex(&[0, 0, 0x0f]), "f");
        assert_eq!(java_signed_hex(&[0x7f, 0]), "7f00");
        assert_eq!(java_signed_hex(&[0x80]), "-80");
        assert_eq!(java_signed_hex(&[0xff]), "-1");
        assert_eq!(java_signed_hex(&[0xff, 0]), "-100");
    }

    #[test]
    fn minecraft_server_hash_matches_positive_and_negative_vectors() {
        assert_eq!(
            minecraft_server_hash(
                b"",
                &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
                b"public-key-der"
            ),
            "5242c3c07c6d11c534151bb8c15f8481019b4a93"
        );
        assert_eq!(
            minecraft_server_hash(b"", &[0xff; 16], b"public-key-der"),
            "-600ab57a13a4f1476a78aaeec78b145aa9d5b960"
        );
    }

    #[test]
    fn rsa_identity_decrypts_valid_response() {
        let identity = RsaIdentity::generate().expect("generate RSA identity");
        let public_key = RsaPublicKey::from_public_key_der(identity.public_key_der())
            .expect("decode SPKI public key");
        assert_eq!(public_key.size() * 8, 1024);

        let challenge = identity.new_challenge();
        let secret = [0x5a; 16];
        let mut rng = rsa::rand_core::OsRng;
        let encrypted_secret = public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, &secret)
            .expect("encrypt secret");
        let encrypted_challenge = public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, &challenge)
            .expect("encrypt challenge");

        assert_eq!(
            identity
                .decrypt_response(&encrypted_secret, &encrypted_challenge, challenge)
                .expect("decrypt response"),
            secret
        );
    }

    #[test]
    fn rsa_identity_rejects_wrong_challenge() {
        let identity = RsaIdentity::generate().expect("generate RSA identity");
        let public_key = RsaPublicKey::from_public_key_der(identity.public_key_der())
            .expect("decode SPKI public key");
        let challenge = identity.new_challenge();
        let mut wrong_challenge = challenge;
        wrong_challenge[0] ^= 1;
        let mut rng = rsa::rand_core::OsRng;
        let encrypted_secret = public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, &[0x5a; 16])
            .expect("encrypt secret");
        let encrypted_challenge = public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, &wrong_challenge)
            .expect("encrypt challenge");

        assert_eq!(
            identity.decrypt_response(&encrypted_secret, &encrypted_challenge, challenge),
            Err(RsaIdentityError::InvalidChallenge)
        );
    }

    #[test]
    fn rsa_identity_rejects_non_aes_secret_length() {
        let identity = RsaIdentity::generate().expect("generate RSA identity");
        let public_key = RsaPublicKey::from_public_key_der(identity.public_key_der())
            .expect("decode SPKI public key");
        let challenge = identity.new_challenge();
        let mut rng = rsa::rand_core::OsRng;
        let encrypted_secret = public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, &[0x5a; 15])
            .expect("encrypt secret");
        let encrypted_challenge = public_key
            .encrypt(&mut rng, Pkcs1v15Encrypt, &challenge)
            .expect("encrypt challenge");

        assert_eq!(
            identity.decrypt_response(&encrypted_secret, &encrypted_challenge, challenge),
            Err(RsaIdentityError::InvalidSharedSecretLength(15))
        );
    }

    #[tokio::test]
    async fn verifier_returns_uuid_and_signed_or_unsigned_properties() {
        let (base_url, received_request) = local_response(
            "200 OK",
            r#"{"id":"12345678123456789abcdef012345678","name":"IgnoredName","properties":[{"name":"textures","value":"texture-value","signature":"texture-signature"},{"name":"rank","value":"builder"}]}"#,
        )
        .await;
        let verifier = verifier(&base_url);
        let _: &dyn SessionVerifier = &verifier;

        let verified = verifier.verify(request()).await.expect("verified session");

        assert_eq!(
            verified.uuid,
            Uuid::parse_str("12345678-1234-5678-9abc-def012345678").unwrap()
        );
        assert_eq!(
            verified.properties,
            vec![
                GameProfileProperty {
                    name: "textures".to_owned(),
                    value: "texture-value".to_owned(),
                    signature: Some("texture-signature".to_owned()),
                },
                GameProfileProperty {
                    name: "rank".to_owned(),
                    value: "builder".to_owned(),
                    signature: None,
                },
            ]
        );

        let raw_request = received_request.await.expect("receive HTTP request");
        let target = raw_request
            .lines()
            .next()
            .expect("request line")
            .split_whitespace()
            .nth(1)
            .expect("request target");
        let url = reqwest::Url::parse(&format!("http://localhost{target}"))
            .expect("parse request target");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("username").map(String::as_str), Some("A B+?"));
        assert_eq!(query.get("serverId").map(String::as_str), Some("-abc123"));
        assert_eq!(query.get("ip").map(String::as_str), Some("203.0.113.9"));
    }

    #[tokio::test]
    async fn verifier_accepts_dashed_uuid_and_null_properties() {
        let (base_url, _) = local_response(
            "200 OK",
            r#"{"id":"12345678-1234-5678-9abc-def012345678","properties":null}"#,
        )
        .await;

        let verified = verifier(&base_url)
            .verify(VerifySession {
                username: "Player".to_owned(),
                server_id_hash: "abc".to_owned(),
                client_ip: None,
            })
            .await
            .expect("verified session");

        assert!(verified.properties.is_empty());
    }

    #[tokio::test]
    async fn verifier_maps_null_and_client_errors_to_unverified() {
        for (status, body) in [
            ("200 OK", "null"),
            ("200 OK", "{}"),
            ("403 Forbidden", r#"{"error":"Forbidden"}"#),
        ] {
            let (base_url, _) = local_response(status, body).await;
            assert_eq!(
                verifier(&base_url).verify(request()).await,
                Err(VerifySessionError::Unverified)
            );
        }
    }

    #[tokio::test]
    async fn verifier_maps_malformed_success_to_unverified() {
        let (base_url, _) = local_response("200 OK", "not-json").await;
        assert_eq!(
            verifier(&base_url).verify(request()).await,
            Err(VerifySessionError::Unverified)
        );
    }

    #[tokio::test]
    async fn verifier_maps_parsed_server_error_to_unavailable() {
        let (base_url, _) = local_response(
            "503 Service Unavailable",
            r#"{"error":"ServiceUnavailable","errorMessage":"try later"}"#,
        )
        .await;
        assert_eq!(
            verifier(&base_url).verify(request()).await,
            Err(VerifySessionError::Unavailable)
        );
    }
}
