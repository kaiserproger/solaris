//! Exact `RegistryData` payload capture — the packaged content-import step.
//!
//! A vanilla server only emits the full registry payloads a client needs when
//! the client declines Known Packs, and those bytes are codec-produced wire
//! data: the raw data-pack JSON is not equivalent. This module is the one
//! capture implementation. It drives a real server through the Configuration
//! handshake and writes each payload under `mc_data::NETWORK_REGISTRY_PAYLOAD_DIR`
//! (`reports/registry_network_nbt/**`); do not add a second Configuration
//! decoder or a derived-JSON substitute.
//!
//! # Entry points
//!
//! * [`capture_registry_payloads_from_jar`] — the whole step. Given the staged
//!   content directory that receives the payloads, a vanilla server jar, a
//!   scratch directory, a Java executable and a readiness timeout, it starts a
//!   temporary server from that jar, captures the payloads, checks the captured
//!   registry index against the staged content, publishes the payload tree into
//!   the staged directory, and stops the server. It never reads the repository
//!   layout, never runs `cargo`, and never shells out to `tools/*.sh`.
//! * [`capture_registry_payloads`] — the same capture against an
//!   already-started server endpoint, for callers that own the server.
//!
//! ```text
//! capture_registry_payloads_from_jar(
//!     server_jar:   &Path,   // Mojang bundle jar for the pinned release
//!     scratch_dir:  &Path,   // scratch space for the temporary server
//!     content_root: &Path,   // staged content; payloads land under
//!                            // content_root/reports/registry_network_nbt/**
//!     java:         &Path,   // java executable (absolute path or a PATH name)
//!     startup_timeout: Duration,
//! ) -> Result<BTreeMap<String, BTreeSet<String>>, RegistryCaptureError>
//! ```
//!
//! The returned index is `registry id -> captured entry names`. Readiness is
//! the server's own "Done" log line and the handshake is driven frame by frame;
//! nothing polls or sleeps, and the temporary server's scratch tree is removed
//! on every path, including failures.
//!
//! # Failure modes
//!
//! Every failure is a [`RegistryCaptureError`] variant, so a caller can name the
//! concrete cause without parsing messages:
//!
//! | variant | cause |
//! |---|---|
//! | [`RegistryCaptureError::JavaUnavailable`] | the Java executable does not exist |
//! | [`RegistryCaptureError::ServerJarMissing`] | the server jar is absent |
//! | [`RegistryCaptureError::ContentRootMissing`] | the staged content directory is absent |
//! | [`RegistryCaptureError::ContentNotIndexable`] | the staged content has no valid raw registry index |
//! | [`RegistryCaptureError::ServerNotReady`] | the server never announced readiness in time |
//! | [`RegistryCaptureError::ServerExited`] | the server died before readiness |
//! | [`RegistryCaptureError::ServerProcess`] | the temporary server's process lifecycle failed |
//! | [`RegistryCaptureError::HandshakeRefused`] | connect/login/Configuration exchange failed |
//! | [`RegistryCaptureError::UnexpectedFrame`] | an unknown Configuration frame arrived mid-capture |
//! | [`RegistryCaptureError::PayloadMissing`] | vanilla omitted a payload after Known Packs was declined |
//! | [`RegistryCaptureError::DuplicateData`] | a registry or entry was sent twice |
//! | [`RegistryCaptureError::PayloadPath`] | a captured identifier has no valid payload path |
//! | [`RegistryCaptureError::IndexMismatch`] | captured registries differ from the staged content index |
//! | [`RegistryCaptureError::InstalledPayloadsIncomplete`] | the published payload set does not cover the index |
//! | [`RegistryCaptureError::Io`] | staging, publish or scratch I/O failed |

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

use bytes::Buf;
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateTags,
};

use crate::client::Client;
use crate::parity::{VanillaLaunchError, VanillaServerProcess};

/// Typed failure of the vanilla RegistryData capture step.
///
/// Variants name the concrete cause; the four a caller switches on are
/// [`Self::JavaUnavailable`], [`Self::ServerNotReady`], [`Self::HandshakeRefused`]
/// and [`Self::PayloadMissing`].
#[derive(Debug, thiserror::Error)]
pub enum RegistryCaptureError {
    #[error(
        "java executable is unavailable at {}; install a JDK or pass its path",
        java.display()
    )]
    JavaUnavailable { java: PathBuf },

    #[error("vanilla server jar missing: {}", jar.display())]
    ServerJarMissing { jar: PathBuf },

    #[error("staged content directory missing: {}", root.display())]
    ContentRootMissing { root: PathBuf },

    #[error("staged content at {} cannot be indexed: {source}", root.display())]
    ContentNotIndexable {
        root: PathBuf,
        #[source]
        source: mc_data::DataError,
    },

    #[error(
        "vanilla server did not become ready within {timeout:?} while capturing RegistryData: {log}"
    )]
    ServerNotReady { timeout: Duration, log: String },

    #[error("vanilla server exited before ready with {status} while capturing RegistryData: {log}")]
    ServerExited { status: ExitStatus, log: String },

    #[error("vanilla server process failure: {detail}")]
    ServerProcess { detail: String },

    #[error("registry handshake with {addr} was refused: {detail}")]
    HandshakeRefused { addr: SocketAddr, detail: String },

    #[error(
        "unexpected configuration frame 0x{packet_id:02X} from {addr} while capturing RegistryData"
    )]
    UnexpectedFrame { addr: SocketAddr, packet_id: i32 },

    #[error(
        "vanilla omitted the Network-NBT payload for {entry} in {registry} after Known Packs was declined"
    )]
    PayloadMissing { registry: String, entry: String },

    #[error("duplicate RegistryData content for {registry}: {detail}")]
    DuplicateData { registry: String, detail: String },

    #[error("cannot stage the payload for {entry} in {registry}: {detail}")]
    PayloadPath {
        registry: String,
        entry: String,
        detail: String,
    },

    #[error("captured registry index does not match the staged content index: {detail}")]
    IndexMismatch { detail: String },

    #[error("installed RegistryData payloads at {} are incomplete", root.display())]
    InstalledPayloadsIncomplete { root: PathBuf },

    #[error("capture I/O failed at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Run the complete wire capture against a temporary vanilla server.
///
/// Starts `server_jar` with `java`, captures the exact `RegistryData` payloads
/// from its Configuration handshake, verifies that the captured registry index
/// matches the index of the staged content at `content_root`, publishes the
/// payloads to `content_root/reports/registry_network_nbt/**`, and stops the
/// server. The temporary server's scratch tree lives inside `scratch_dir` and is
/// removed on every path.
///
/// Returns the captured `registry id -> entry names` index.
pub async fn capture_registry_payloads_from_jar(
    server_jar: &Path,
    scratch_dir: &Path,
    content_root: &Path,
    java: &Path,
    startup_timeout: Duration,
) -> Result<BTreeMap<String, BTreeSet<String>>, RegistryCaptureError> {
    let jar = resolve_server_jar(server_jar)?;
    let java = resolve_java(java)?;
    let root = resolve_content_root(content_root)?;
    fs::create_dir_all(scratch_dir).map_err(|source| RegistryCaptureError::Io {
        path: scratch_dir.to_path_buf(),
        source,
    })?;

    let staging = tempfile::Builder::new()
        .prefix(".registry-network-nbt-")
        .tempdir_in(&root)
        .map_err(|source| RegistryCaptureError::Io {
            path: root.clone(),
            source,
        })?;
    // Read the staged index before starting a JVM, so a broken staging tree is
    // reported without paying for a server launch.
    let expected = expected_registry_index(&root)?;
    let work = tempfile::Builder::new()
        .prefix("vanilla-capture-")
        .tempdir_in(scratch_dir)
        .map_err(|source| RegistryCaptureError::Io {
            path: scratch_dir.to_path_buf(),
            source,
        })?;

    let server = TemporaryServer::launch(jar, java, work, startup_timeout).await?;
    let captured = capture_registry_payloads(server.addr(), staging.path()).await?;
    server.stop().await?;

    verify_index(&root, &expected, &captured)?;
    install_payloads(staging.path(), &root)?;

    let installed =
        mc_data::load(&root).map_err(|source| RegistryCaptureError::ContentNotIndexable {
            root: root.clone(),
            source,
        })?;
    if !installed.has_full_registry_payloads() {
        return Err(RegistryCaptureError::InstalledPayloadsIncomplete { root });
    }
    Ok(captured)
}

/// Capture every registry entry payload after declining Known Packs.
///
/// Drives the Configuration handshake against an already-started server at
/// `addr` and stages the payloads under
/// `staging_root/<mc_data::NETWORK_REGISTRY_PAYLOAD_DIR>`. The staging tree is
/// left in place for the caller to publish; the returned value is the captured
/// `registry id -> entry names` index.
pub async fn capture_registry_payloads(
    addr: SocketAddr,
    staging_root: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>, RegistryCaptureError> {
    let mut client = Client::connect(addr)
        .await
        .map_err(handshake_refused(addr))?;
    let _ = client
        .drive_login(addr, "RegistryCapture")
        .await
        .map_err(handshake_refused(addr))?;

    loop {
        let frame = client.read_frame().await.map_err(handshake_refused(addr))?;
        if frame.id == ClientboundKnownPacks::ID {
            let mut body = frame.body;
            let _ = ClientboundKnownPacks::decode(&mut body).map_err(handshake_refused(addr))?;
            require_drained(body, "Known Packs", addr)?;
            break;
        }
    }
    client
        .write_packet(&ServerboundKnownPacks { packs: Vec::new() })
        .await
        .map_err(handshake_refused(addr))?;

    let mut captured = BTreeMap::new();
    loop {
        let frame = client.read_frame().await.map_err(handshake_refused(addr))?;
        if frame.id == RegistryData::ID {
            let mut body = frame.body;
            let registry = RegistryData::decode(&mut body).map_err(handshake_refused(addr))?;
            require_drained(body, "RegistryData", addr)?;

            let mut entries = BTreeSet::new();
            for entry in registry.entries {
                let payload =
                    entry
                        .nbt_payload
                        .ok_or_else(|| RegistryCaptureError::PayloadMissing {
                            registry: registry.registry_id.to_string(),
                            entry: entry.name.to_string(),
                        })?;
                if !entries.insert(entry.name.to_string()) {
                    return Err(RegistryCaptureError::DuplicateData {
                        registry: registry.registry_id.to_string(),
                        detail: format!("entry {} appeared twice", entry.name),
                    });
                }
                let path = mc_data::network_registry_payload_path(
                    staging_root,
                    &registry.registry_id,
                    &entry.name,
                )
                .map_err(|error| RegistryCaptureError::PayloadPath {
                    registry: registry.registry_id.to_string(),
                    entry: entry.name.to_string(),
                    detail: error.to_string(),
                })?;
                let parent = path
                    .parent()
                    .ok_or_else(|| RegistryCaptureError::PayloadPath {
                        registry: registry.registry_id.to_string(),
                        entry: entry.name.to_string(),
                        detail: format!("captured payload path has no parent: {}", path.display()),
                    })?;
                fs::create_dir_all(parent).map_err(|source| RegistryCaptureError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
                fs::write(&path, payload.as_ref())
                    .map_err(|source| RegistryCaptureError::Io { path, source })?;
            }
            if captured
                .insert(registry.registry_id.to_string(), entries)
                .is_some()
            {
                return Err(RegistryCaptureError::DuplicateData {
                    registry: registry.registry_id.to_string(),
                    detail: "the registry packet appeared twice".into(),
                });
            }
            continue;
        }
        if frame.id == UpdateTags::ID {
            let mut body = frame.body;
            let _ = UpdateTags::decode(&mut body).map_err(handshake_refused(addr))?;
            require_drained(body, "Update Tags", addr)?;
            continue;
        }
        if frame.id == FinishConfiguration::ID {
            let mut body = frame.body;
            let _ = FinishConfiguration::decode(&mut body).map_err(handshake_refused(addr))?;
            require_drained(body, "Finish Configuration", addr)?;
            client
                .write_packet(&AcknowledgeFinishConfiguration)
                .await
                .map_err(handshake_refused(addr))?;
            return Ok(captured);
        }
        return Err(RegistryCaptureError::UnexpectedFrame {
            addr,
            packet_id: frame.id,
        });
    }
}

/// Map any wire-side failure of the Configuration exchange.
fn handshake_refused<E: std::fmt::Display>(
    addr: SocketAddr,
) -> impl FnOnce(E) -> RegistryCaptureError {
    move |error| RegistryCaptureError::HandshakeRefused {
        addr,
        detail: format!("{error:#}"),
    }
}

/// A decoded packet must consume its frame exactly.
fn require_drained(
    body: impl Buf,
    packet: &str,
    addr: SocketAddr,
) -> Result<(), RegistryCaptureError> {
    if body.has_remaining() {
        return Err(RegistryCaptureError::HandshakeRefused {
            addr,
            detail: format!("{packet} has trailing bytes"),
        });
    }
    Ok(())
}

/// Owns the temporary server started for one capture.
///
/// Launching, stopping and the scratch tree are all handled on worker threads so
/// an async caller never blocks on Java's startup or shutdown; dropping the
/// value still stops the server and removes the scratch tree.
struct TemporaryServer {
    process: Option<VanillaServerProcess>,
    work: Option<tempfile::TempDir>,
    addr: SocketAddr,
}

impl TemporaryServer {
    async fn launch(
        jar: PathBuf,
        java: PathBuf,
        work: tempfile::TempDir,
        startup_timeout: Duration,
    ) -> Result<Self, RegistryCaptureError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("vanilla-capture-launch".into())
            .spawn(move || {
                let launched = VanillaServerProcess::launch_with_java(
                    &jar,
                    work.path(),
                    &java,
                    startup_timeout,
                );
                let _ = tx.send((work, launched));
            })
            .map_err(|error| RegistryCaptureError::ServerProcess {
                detail: format!("spawn the server launch thread: {error}"),
            })?;
        let (work, launched) = rx.await.map_err(|_| RegistryCaptureError::ServerProcess {
            detail: "the server launch thread dropped without reporting".into(),
        })?;
        let process = launched.map_err(map_launch_error)?;
        let addr = process.addr();
        Ok(Self {
            process: Some(process),
            work: Some(work),
            addr,
        })
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop the server and remove its scratch tree, off the async executor.
    async fn stop(mut self) -> Result<(), RegistryCaptureError> {
        let cleanup = self.take_cleanup();
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("vanilla-capture-stop".into())
            .spawn(move || {
                let _ = tx.send(stop_server(cleanup));
            })
            .map_err(|error| RegistryCaptureError::ServerProcess {
                detail: format!("spawn the server stop thread: {error}"),
            })?;
        rx.await.map_err(|_| RegistryCaptureError::ServerProcess {
            detail: "the server stop thread dropped without reporting".into(),
        })?
    }

    fn take_cleanup(&mut self) -> (Option<VanillaServerProcess>, Option<tempfile::TempDir>) {
        (self.process.take(), self.work.take())
    }
}

impl Drop for TemporaryServer {
    fn drop(&mut self) {
        let cleanup = self.take_cleanup();
        if cleanup.0.is_none() && cleanup.1.is_none() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("vanilla-capture-drop".into())
            .spawn(move || {
                let _ = stop_server(cleanup);
            });
    }
}

/// Stop the process and drop the scratch tree; used from worker threads only.
fn stop_server(
    cleanup: (Option<VanillaServerProcess>, Option<tempfile::TempDir>),
) -> Result<(), RegistryCaptureError> {
    let (process, work) = cleanup;
    let stopped = match process {
        Some(process) => process
            .stop()
            .map_err(|error| RegistryCaptureError::ServerProcess {
                detail: format!("{error:#}"),
            }),
        None => Ok(()),
    };
    drop(work);
    stopped
}

fn map_launch_error(error: VanillaLaunchError) -> RegistryCaptureError {
    match error {
        VanillaLaunchError::JarMissing { path } => {
            RegistryCaptureError::ServerJarMissing { jar: path }
        }
        VanillaLaunchError::Spawn {
            program, source, ..
        } if source.kind() == io::ErrorKind::NotFound => {
            RegistryCaptureError::JavaUnavailable { java: program }
        }
        VanillaLaunchError::NotReady { timeout, log } => {
            RegistryCaptureError::ServerNotReady { timeout, log }
        }
        VanillaLaunchError::Exited { status, log } => {
            RegistryCaptureError::ServerExited { status, log }
        }
        VanillaLaunchError::WorkDir { path, detail } => RegistryCaptureError::Io {
            path,
            source: io::Error::other(detail),
        },
        other => RegistryCaptureError::ServerProcess {
            detail: other.to_string(),
        },
    }
}

/// Resolve the server jar to an absolute path so the child process, which runs
/// with the scratch directory as its working directory, opens the right file.
fn resolve_server_jar(server_jar: &Path) -> Result<PathBuf, RegistryCaptureError> {
    if !server_jar.is_file() {
        return Err(RegistryCaptureError::ServerJarMissing {
            jar: server_jar.to_path_buf(),
        });
    }
    fs::canonicalize(server_jar).map_err(|source| RegistryCaptureError::Io {
        path: server_jar.to_path_buf(),
        source,
    })
}

/// Resolve the Java executable: an explicit path, or a bare name looked up on
/// `PATH`.
fn resolve_java(java: &Path) -> Result<PathBuf, RegistryCaptureError> {
    let unavailable = || RegistryCaptureError::JavaUnavailable {
        java: java.to_path_buf(),
    };
    if java.components().count() > 1 {
        return if java.is_file() {
            Ok(java.to_path_buf())
        } else {
            Err(unavailable())
        };
    }
    let Some(path) = std::env::var_os("PATH") else {
        return Err(unavailable());
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(java))
        .find(|candidate| candidate.is_file())
        .ok_or_else(unavailable)
}

fn resolve_content_root(content_root: &Path) -> Result<PathBuf, RegistryCaptureError> {
    if !content_root.is_dir() {
        return Err(RegistryCaptureError::ContentRootMissing {
            root: content_root.to_path_buf(),
        });
    }
    fs::canonicalize(content_root).map_err(|source| RegistryCaptureError::Io {
        path: content_root.to_path_buf(),
        source,
    })
}

/// The registry index the staged content declares, in the same shape as the
/// captured index.
fn expected_registry_index(
    root: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>, RegistryCaptureError> {
    let data = mc_data::load(root).map_err(|source| RegistryCaptureError::ContentNotIndexable {
        root: root.to_path_buf(),
        source,
    })?;
    Ok(data
        .registries()
        .map(|registry| {
            (
                registry.id.to_string(),
                registry.entries.iter().map(ToString::to_string).collect(),
            )
        })
        .collect())
}

/// The captured bytes must describe exactly the staged registries; anything else
/// would make the payload set incomplete or unreachable for a real client.
fn verify_index(
    root: &Path,
    expected: &BTreeMap<String, BTreeSet<String>>,
    captured: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), RegistryCaptureError> {
    if expected == captured {
        return Ok(());
    }
    let mut detail = String::new();
    for (registry, expected_entries) in expected {
        let Some(captured_entries) = captured.get(registry) else {
            let _ = writeln!(detail, "registry {registry} was not captured");
            continue;
        };
        if captured_entries == expected_entries {
            continue;
        }
        let missing = expected_entries.difference(captured_entries).count();
        let unexpected = captured_entries.difference(expected_entries).count();
        let _ = writeln!(
            detail,
            "registry {registry}: {missing} staged entries missing from the capture, {unexpected} captured entries not in the staged content"
        );
    }
    for registry in captured.keys() {
        if !expected.contains_key(registry) {
            let _ = writeln!(
                detail,
                "registry {registry} is not part of the staged content"
            );
        }
    }
    let detail = detail.trim_end().to_string();
    let detail = if detail.is_empty() {
        format!(
            "the staged content at {} has no registry index",
            root.display()
        )
    } else {
        detail
    };
    Err(RegistryCaptureError::IndexMismatch { detail })
}

/// Publish the staged payload tree into the content root. The staged tree is a
/// sibling of the destination, so the swap is a rename that either fully lands
/// or leaves the previous payloads untouched.
fn install_payloads(staged_root: &Path, content_root: &Path) -> Result<(), RegistryCaptureError> {
    let staged = staged_root.join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR);
    let installed = content_root.join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR);
    if let Some(parent) = installed.parent() {
        fs::create_dir_all(parent).map_err(|source| RegistryCaptureError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    if installed.exists() {
        fs::remove_dir_all(&installed).map_err(|source| RegistryCaptureError::Io {
            path: installed.clone(),
            source,
        })?;
    }
    fs::rename(&staged, &installed).map_err(|source| RegistryCaptureError::Io {
        path: staged,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use mc_data::Identifier;
    use mc_nbt::Tag;
    use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
    use mc_protocol::packets::configuration::{KnownPackEntry, RegistryEntry};
    use mc_protocol::packets::handshake::Handshake;
    use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use uuid::Uuid;

    /// `(registry, entry, NBT int value)` the fixture server serves.
    const FIXTURE: &[(&str, &str, i32)] = &[
        ("minecraft:dimension_type", "minecraft:overworld", 7),
        ("minecraft:dimension_type", "minecraft:the_nether", 9),
        ("minecraft:damage_type", "minecraft:in_fire", 3),
    ];

    #[tokio::test]
    async fn writes_the_payload_set_a_configuration_server_sends() -> anyhow::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let registries = fixture_registries()?;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            serve_configuration(stream, registries).await
        });

        let staging = tempfile::tempdir()?;
        let captured = capture_registry_payloads(addr, staging.path()).await?;
        server.await??;

        let mut expected_index: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (registry, entry, value) in FIXTURE {
            expected_index
                .entry((*registry).to_string())
                .or_default()
                .insert((*entry).to_string());
            let registry = Identifier::parse(*registry)?;
            let entry = Identifier::parse(*entry)?;
            let path = mc_data::network_registry_payload_path(staging.path(), &registry, &entry)?;
            assert_eq!(
                fs::read(&path)?,
                fixture_payload_bytes(*value),
                "payload bytes at {}",
                path.display()
            );
        }
        assert_eq!(captured, expected_index);

        let payload_dir = staging.path().join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR);
        let mut files = Vec::new();
        collect_payload_files(&payload_dir, &mut files)?;
        assert_eq!(
            files.len(),
            FIXTURE.len(),
            "captured files: {files:#?} under {}",
            payload_dir.display()
        );
        Ok(())
    }

    #[test]
    fn publishes_payloads_under_a_content_root_without_a_reports_directory() -> anyhow::Result<()> {
        let staged = tempfile::tempdir()?;
        let staged_payloads = staged.path().join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR);
        let captured = staged_payloads.join("minecraft/dimension_type/minecraft/overworld.nbt");
        fs::create_dir_all(captured.parent().expect("captured payload has a parent"))?;
        fs::write(&captured, b"\x0a\x00")?;

        let content = tempfile::tempdir()?;
        let stale = content
            .path()
            .join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR)
            .join("minecraft/stale.nbt");
        fs::create_dir_all(stale.parent().expect("stale payload has a parent"))?;
        fs::write(&stale, b"stale")?;

        install_payloads(staged.path(), content.path()).map_err(anyhow::Error::from)?;

        let installed = content
            .path()
            .join(mc_data::NETWORK_REGISTRY_PAYLOAD_DIR)
            .join("minecraft/dimension_type/minecraft/overworld.nbt");
        assert_eq!(fs::read(&installed)?, b"\x0a\x00");
        assert!(
            !stale.exists(),
            "the previous payload tree must be replaced"
        );
        assert!(!staged_payloads.exists(), "the staged tree must be moved");
        Ok(())
    }

    #[tokio::test]
    async fn reports_a_refused_registry_handshake() -> anyhow::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let acceptor = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            drop(stream);
            anyhow::Ok(())
        });

        let staging = tempfile::tempdir()?;
        let error = capture_registry_payloads(addr, staging.path())
            .await
            .expect_err("a closed connection must refuse the handshake");
        assert!(
            matches!(error, RegistryCaptureError::HandshakeRefused { .. }),
            "unexpected error: {error}"
        );
        acceptor.await??;
        Ok(())
    }

    #[tokio::test]
    async fn reports_missing_java() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let jar = dir.path().join("server.jar");
        fs::write(&jar, b"not a jar")?;
        let content = tempfile::tempdir()?;
        let scratch = tempfile::tempdir()?;

        let error = capture_registry_payloads_from_jar(
            &jar,
            scratch.path(),
            content.path(),
            &dir.path().join("missing-java"),
            Duration::from_secs(30),
        )
        .await
        .expect_err("a missing Java executable must fail");
        assert!(
            matches!(error, RegistryCaptureError::JavaUnavailable { .. }),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn reports_staged_content_without_a_registry_index() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let jar = dir.path().join("server.jar");
        fs::write(&jar, b"not a jar")?;
        let content = tempfile::tempdir()?;
        let scratch = tempfile::tempdir()?;

        let error = capture_registry_payloads_from_jar(
            &jar,
            scratch.path(),
            content.path(),
            &dir.path().join("missing-java"),
            Duration::from_secs(30),
        )
        .await
        .expect_err("a content root with no registry index must fail");
        assert!(
            matches!(error, RegistryCaptureError::JavaUnavailable { .. }),
            "the Java check comes first: {error}"
        );

        let error = capture_registry_payloads_from_jar(
            &jar,
            scratch.path(),
            content.path(),
            &jar,
            Duration::from_secs(30),
        )
        .await
        .expect_err("a content root with no registry index must fail");
        assert!(
            matches!(error, RegistryCaptureError::ContentNotIndexable { .. }),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn reports_a_server_that_never_becomes_ready() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let jar = dir.path().join("server.jar");
        fs::write(&jar, b"not a jar")?;
        let java = script(
            dir.path(),
            "java-never-ready",
            "echo \"Starting fixture\"\nexec sleep 30\n",
        )?;
        let content = staged_content_fixture()?;
        let scratch = tempfile::tempdir()?;

        let error = capture_registry_payloads_from_jar(
            &jar,
            scratch.path(),
            content.path(),
            &java,
            Duration::from_millis(500),
        )
        .await
        .expect_err("a server that never announces readiness must fail");
        assert!(
            matches!(error, RegistryCaptureError::ServerNotReady { .. }),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn reports_a_server_that_exits_before_ready() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let jar = dir.path().join("server.jar");
        fs::write(&jar, b"not a jar")?;
        let java = script(
            dir.path(),
            "java-exits",
            "echo \"boot failure\" 1>&2\nexit 3\n",
        )?;
        let content = staged_content_fixture()?;
        let scratch = tempfile::tempdir()?;

        let error = capture_registry_payloads_from_jar(
            &jar,
            scratch.path(),
            content.path(),
            &java,
            Duration::from_secs(30),
        )
        .await
        .expect_err("a server that exits before ready must fail");
        match error {
            RegistryCaptureError::ServerExited { status, ref log } => {
                assert_eq!(status.code(), Some(3));
                assert!(log.contains("boot failure"), "log: {log}");
            }
            other => panic!("unexpected error: {other}"),
        }
        Ok(())
    }

    fn fixture_payload_bytes(value: i32) -> Vec<u8> {
        let tag = Tag::Compound(vec![("value".to_string(), Tag::Int(value))]);
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &tag).expect("fixture compound is a network NBT root");
        bytes
    }

    fn fixture_registries() -> anyhow::Result<Vec<RegistryData>> {
        let mut by_registry: BTreeMap<String, Vec<RegistryEntry>> = BTreeMap::new();
        for (registry, entry, value) in FIXTURE {
            by_registry
                .entry((*registry).to_string())
                .or_default()
                .push(RegistryEntry {
                    name: Identifier::parse(*entry)?,
                    nbt_payload: Some(fixture_payload_bytes(*value).into()),
                });
        }
        by_registry
            .into_iter()
            .map(|(registry, entries)| {
                Ok(RegistryData {
                    registry_id: Identifier::parse(registry)?,
                    entries,
                })
            })
            .collect()
    }

    /// The vanilla Configuration sequence for a client that declines the core
    /// pack: Login, Known Packs (declined), Registry Data, Update Tags, Finish.
    async fn serve_configuration(
        mut stream: TcpStream,
        registries: Vec<RegistryData>,
    ) -> anyhow::Result<()> {
        let mut rbuf = BytesMut::new();
        let _: Handshake = recv(&mut stream, &mut rbuf).await?;
        let _: LoginStart = recv(&mut stream, &mut rbuf).await?;
        send(
            &mut stream,
            &LoginSuccess {
                uuid: Uuid::nil(),
                name: "RegistryCapture".into(),
                properties: Vec::new(),
            },
        )
        .await?;
        let _: LoginAcknowledged = recv(&mut stream, &mut rbuf).await?;
        send(
            &mut stream,
            &ClientboundKnownPacks {
                packs: vec![KnownPackEntry {
                    namespace: "minecraft".into(),
                    id: "core".into(),
                    version: "26.1.2".into(),
                }],
            },
        )
        .await?;
        let declined: ServerboundKnownPacks = recv(&mut stream, &mut rbuf).await?;
        anyhow::ensure!(
            declined.packs.is_empty(),
            "the capture client must decline Known Packs, got {:?}",
            declined.packs
        );
        for registry in registries {
            send(&mut stream, &registry).await?;
        }
        send(&mut stream, &UpdateTags::default()).await?;
        send(&mut stream, &FinishConfiguration).await?;
        let _: AcknowledgeFinishConfiguration = recv(&mut stream, &mut rbuf).await?;
        Ok(())
    }

    async fn send<P: Packet>(stream: &mut TcpStream, packet: &P) -> anyhow::Result<()> {
        let mut body = BytesMut::new();
        packet.encode(&mut body)?;
        let framed = encode_frame(P::ID, &body, Compression::Disabled)?;
        stream.write_all(&framed).await?;
        Ok(())
    }

    async fn recv<P: Packet>(stream: &mut TcpStream, rbuf: &mut BytesMut) -> anyhow::Result<P> {
        loop {
            if let Some(mut frame) = try_decode_frame(rbuf, Compression::Disabled)? {
                anyhow::ensure!(
                    frame.id == P::ID,
                    "expected packet 0x{:02X}, got 0x{:02X}",
                    P::ID,
                    frame.id
                );
                return Ok(P::decode(&mut frame.body)?);
            }
            anyhow::ensure!(stream.read_buf(rbuf).await? > 0, "peer closed");
        }
    }

    fn collect_payload_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect_payload_files(&path, out)?;
            } else {
                out.push(path);
            }
        }
        Ok(())
    }

    /// A staged content tree with one raw entry per registry: what
    /// `mc_data::load` needs to build the expected index. The payload directory
    /// is what the capture itself adds.
    fn staged_content_fixture() -> anyhow::Result<tempfile::TempDir> {
        let root = tempfile::tempdir()?;
        for (_registry, subpath) in mc_data::KNOWN_REGISTRIES {
            let dir = root.path().join("data/minecraft").join(subpath);
            fs::create_dir_all(&dir)?;
            fs::write(dir.join("fixture.json"), b"{}\n")?;
        }
        Ok(root)
    }

    /// Write an executable stand-in for a Java executable.
    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> anyhow::Result<PathBuf> {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}"))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        Ok(path)
    }
}
