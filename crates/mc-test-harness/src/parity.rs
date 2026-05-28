//! Autonomous parity-test primitives for comparing Solaris with a local vanilla
//! oracle.
//!
//! This module deliberately keeps Mojang-owned artifacts outside the repo. The
//! vanilla launcher only looks for a developer-supplied `.analysis/server.jar`
//! and creates all runtime files in a temporary directory.

use std::fmt;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundKeepAlive, ConfirmTeleportation, GameEvent, LoginPlay,
    MovePlayerFlags, ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundMovePlayerRot,
    ServerboundMovePlayerStatusOnly, ServerboundPlayerLoaded, SetCenterChunk,
    SynchronizePlayerPosition,
};

use crate::client::Client;

/// Default local path for the vanilla oracle jar. The jar itself is never
/// tracked by git.
pub const DEFAULT_VANILLA_JAR: &str = ".analysis/server.jar";

/// Result of probing for a local vanilla oracle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OracleAvailability {
    Available {
        jar: PathBuf,
    },
    Missing {
        expected: PathBuf,
    },
    JavaTooOld {
        jar: PathBuf,
        found_major: Option<u32>,
    },
}

impl OracleAvailability {
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    #[must_use]
    pub fn skip_message(&self) -> Option<String> {
        match self {
            Self::Available { .. } => None,
            Self::Missing { expected } => Some(format!(
                "skipping vanilla-backed parity test: {} missing; put a Mojang 26.1.x server jar there",
                expected.display()
            )),
            Self::JavaTooOld { jar, found_major } => Some(format!(
                "skipping vanilla-backed parity test: {} requires Java 25+; found {}",
                jar.display(),
                found_major.map_or_else(
                    || "unknown Java".to_string(),
                    |major| format!("Java {major}")
                ),
            )),
        }
    }
}

/// Locate the developer-supplied vanilla oracle jar under a repository root and
/// verify that `java` is new enough for 26.1.x class files.
#[must_use]
pub fn vanilla_oracle_availability(repo_root: impl AsRef<Path>) -> OracleAvailability {
    let expected = repo_root.as_ref().join(DEFAULT_VANILLA_JAR);
    if !expected.is_file() {
        return OracleAvailability::Missing { expected };
    }
    let found_major = java_major_version();
    if found_major.is_some_and(|major| major >= 25) {
        OracleAvailability::Available { jar: expected }
    } else {
        OracleAvailability::JavaTooOld {
            jar: expected,
            found_major,
        }
    }
}

#[must_use]
pub fn java_major_version() -> Option<u32> {
    let output = Command::new("java").arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    parse_java_major_version(&text)
}

#[must_use]
pub fn parse_java_major_version(version_output: &str) -> Option<u32> {
    let marker = "version \"";
    let start = version_output.find(marker)? + marker.len();
    let rest = &version_output[start..];
    let version = rest.split('"').next()?;
    let first = version.split('.').next()?;
    first.parse().ok()
}

/// A normalized fact captured from either Solaris or vanilla.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ObservationFact {
    PacketSeen {
        id: i32,
    },
    SpawnPosition {
        x: i64,
        y: i64,
        z: i64,
    },
    BlockState {
        x: i32,
        y: i32,
        z: i32,
        state_id: u32,
    },
    Health {
        half_hearts_milli: i32,
        food: i32,
    },
    Note {
        key: String,
        value: String,
    },
}

/// Observations for one scenario phase. Facts are sorted before diffing so packet
/// timing noise does not hide stable world-state mismatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationSet {
    pub subject: String,
    pub phase: String,
    facts: Vec<ObservationFact>,
}

impl ObservationSet {
    #[must_use]
    pub fn new(subject: impl Into<String>, phase: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            phase: phase.into(),
            facts: Vec::new(),
        }
    }

    pub fn push(&mut self, fact: ObservationFact) {
        self.facts.push(fact);
    }

    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.facts.sort();
        self.facts.dedup();
        self
    }

    #[must_use]
    pub fn facts(&self) -> &[ObservationFact] {
        &self.facts
    }
}

/// Human-readable diff between two normalized observation sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationDiff {
    pub phase: String,
    pub missing_from_actual: Vec<ObservationFact>,
    pub unexpected_in_actual: Vec<ObservationFact>,
}

impl ObservationDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.missing_from_actual.is_empty() && self.unexpected_in_actual.is_empty()
    }
}

impl fmt::Display for ObservationDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "{}: observations match", self.phase);
        }
        writeln!(f, "{}: observation mismatch", self.phase)?;
        for fact in &self.missing_from_actual {
            writeln!(f, "  missing from actual: {fact:?}")?;
        }
        for fact in &self.unexpected_in_actual {
            writeln!(f, "  unexpected in actual: {fact:?}")?;
        }
        Ok(())
    }
}

#[must_use]
pub fn diff_observations(expected: &ObservationSet, actual: &ObservationSet) -> ObservationDiff {
    let expected = expected.clone().normalized();
    let actual = actual.clone().normalized();
    let missing_from_actual = expected
        .facts
        .iter()
        .filter(|fact| !actual.facts.contains(fact))
        .cloned()
        .collect();
    let unexpected_in_actual = actual
        .facts
        .iter()
        .filter(|fact| !expected.facts.contains(fact))
        .cloned()
        .collect();
    ObservationDiff {
        phase: expected.phase,
        missing_from_actual,
        unexpected_in_actual,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKind {
    Solaris,
    Vanilla,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreAction {
    WaitTicks { ticks: u8 },
    MoveBy { dx_cm: i16, dz_cm: i16 },
    Look { yaw_deg: i16, pitch_deg: i16 },
    Reconnect,
}

impl CoreAction {
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::WaitTicks { ticks } => format!("wait:{ticks}"),
            Self::MoveBy { dx_cm, dz_cm } => format!("move:{dx_cm},{dz_cm}"),
            Self::Look { yaw_deg, pitch_deg } => format!("look:{yaw_deg},{pitch_deg}"),
            Self::Reconnect => "reconnect".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoreActionGenerator {
    state: u64,
}

impl CoreActionGenerator {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[must_use]
    pub fn generate(seed: u64, count: usize) -> Vec<CoreAction> {
        let mut generator = Self::new(seed);
        (0..count).map(|_| generator.next_action()).collect()
    }

    pub fn next_action(&mut self) -> CoreAction {
        match self.next_u32() % 4 {
            0 => CoreAction::WaitTicks {
                ticks: 1 + (self.next_u32() % 20) as u8,
            },
            1 => CoreAction::MoveBy {
                dx_cm: (self.next_u32() % 401) as i16 - 200,
                dz_cm: (self.next_u32() % 401) as i16 - 200,
            },
            2 => CoreAction::Look {
                yaw_deg: (self.next_u32() % 360) as i16 - 180,
                pitch_deg: (self.next_u32() % 181) as i16 - 90,
            },
            _ => CoreAction::Reconnect,
        }
    }

    fn next_u32(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        (self.state >> 32) as u32
    }
}

#[must_use]
pub fn shrink_action_prefix(actions: &[CoreAction], failing_len: usize) -> &[CoreAction] {
    &actions[..actions.len().min(failing_len)]
}

#[derive(Debug, Clone, Copy)]
pub struct ScenarioContext {
    pub kind: ServerKind,
    pub addr: SocketAddr,
}

pub type ScenarioFuture<'a> = Pin<Box<dyn Future<Output = Result<ObservationSet>> + Send + 'a>>;

/// Shared scenario surface. Implementors run the same logical flow against a
/// supplied server address and return normalized observations for diffing.
pub trait ParityScenario: Send + Sync {
    fn name(&self) -> &'static str;
    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a>;
}

/// Deterministic core-action scenario shared by Solaris-only smoke tests and
/// vanilla-backed parity diffs. It drives the login/play prelude, then executes
/// a small movement/look/wait sequence while recording normalized observations.
#[derive(Debug, Clone)]
pub struct CoreActionSequenceScenario {
    name: &'static str,
    actions: Vec<CoreAction>,
}

impl CoreActionSequenceScenario {
    #[must_use]
    pub fn new(name: &'static str, actions: Vec<CoreAction>) -> Self {
        Self { name, actions }
    }

    #[must_use]
    pub fn actions(&self) -> &[CoreAction] {
        &self.actions
    }
}

impl ParityScenario for CoreActionSequenceScenario {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run<'a>(&'a self, ctx: ScenarioContext) -> ScenarioFuture<'a> {
        Box::pin(async move { observe_core_action_sequence(ctx, self.name, &self.actions).await })
    }
}

async fn observe_core_action_sequence(
    ctx: ScenarioContext,
    phase: &'static str,
    actions: &[CoreAction],
) -> Result<ObservationSet> {
    let subject = match ctx.kind {
        ServerKind::Solaris => "solaris",
        ServerKind::Vanilla => "vanilla",
    };
    let mut client = Client::connect(ctx.addr).await?;
    let _login = client.drive_login(ctx.addr, subject).await?;
    client.drive_configuration().await?;

    let mut observations = ObservationSet::new(subject, phase);
    let _: LoginPlay = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen { id: LoginPlay::ID });
    let _: ClientboundCommands = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen {
        id: ClientboundCommands::ID,
    });
    let sync: SynchronizePlayerPosition = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen {
        id: SynchronizePlayerPosition::ID,
    });
    observations.push(ObservationFact::SpawnPosition {
        x: sync.x.floor() as i64,
        y: sync.y.floor() as i64,
        z: sync.z.floor() as i64,
    });
    let _: GameEvent = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen { id: GameEvent::ID });
    let _: SetCenterChunk = client.read_typed().await?;
    observations.push(ObservationFact::PacketSeen {
        id: SetCenterChunk::ID,
    });
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await?;
    client.write_packet(&ServerboundPlayerLoaded).await?;

    execute_core_actions(
        &mut client,
        &mut observations,
        actions,
        sync.x,
        sync.y,
        sync.z,
    )
    .await?;
    observe_post_action_liveness(&mut client, &mut observations).await?;
    Ok(observations.normalized())
}

async fn observe_post_action_liveness(
    client: &mut Client,
    observations: &mut ObservationSet,
) -> Result<()> {
    let frame = client
        .read_frame_with_timeout(Duration::from_secs(5))
        .await
        .context("wait for post-action server liveness frame")?;
    if frame.id == ClientboundKeepAlive::ID {
        let mut body = frame.body.clone();
        let keepalive = ClientboundKeepAlive::decode(&mut body)?;
        client
            .write_packet(&ServerboundKeepAlive { id: keepalive.id })
            .await?;
    }
    observations.push(ObservationFact::Note {
        key: "post_action_liveness".into(),
        value: "clientbound_frame".into(),
    });
    Ok(())
}

async fn execute_core_actions(
    client: &mut Client,
    observations: &mut ObservationSet,
    actions: &[CoreAction],
    mut x: f64,
    y: f64,
    mut z: f64,
) -> Result<()> {
    let mut yaw = 0.0_f32;
    let mut pitch = 0.0_f32;
    let flags = MovePlayerFlags::new(false, false);

    for (index, action) in actions.iter().enumerate() {
        observations.push(ObservationFact::Note {
            key: format!("action.{index}"),
            value: action.summary(),
        });
        match *action {
            CoreAction::WaitTicks { ticks } => {
                client
                    .write_packet(&ServerboundMovePlayerStatusOnly { flags })
                    .await?;
                observations.push(ObservationFact::PacketSeen {
                    id: ServerboundMovePlayerStatusOnly::ID,
                });
                tokio::time::sleep(Duration::from_millis(u64::from(ticks) * 50)).await;
            }
            CoreAction::MoveBy { dx_cm, dz_cm } => {
                x += f64::from(dx_cm) / 100.0;
                z += f64::from(dz_cm) / 100.0;
                client
                    .write_packet(&ServerboundMovePlayerPos { x, y, z, flags })
                    .await?;
                observations.push(ObservationFact::PacketSeen {
                    id: ServerboundMovePlayerPos::ID,
                });
            }
            CoreAction::Look { yaw_deg, pitch_deg } => {
                yaw = f32::from(yaw_deg);
                pitch = f32::from(pitch_deg);
                client
                    .write_packet(&ServerboundMovePlayerRot { yaw, pitch, flags })
                    .await?;
                observations.push(ObservationFact::PacketSeen {
                    id: ServerboundMovePlayerRot::ID,
                });
            }
            CoreAction::Reconnect => {
                observations.push(ObservationFact::Note {
                    key: format!("action.{index}.skipped"),
                    value: "reconnect".into(),
                });
            }
        }
    }

    client
        .write_packet(&ServerboundMovePlayerRot { yaw, pitch, flags })
        .await?;
    observations.push(ObservationFact::Note {
        key: "actions_executed".into(),
        value: actions.len().to_string(),
    });
    Ok(())
}

/// Pick a currently-free localhost port for a child server process.
pub fn reserve_local_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("reserve localhost port")?;
    Ok(listener.local_addr()?.port())
}

/// Owns a running vanilla oracle process. Dropping it tries to stop the server
/// gracefully before killing the child as a fallback.
pub struct VanillaServerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    addr: SocketAddr,
}

impl VanillaServerProcess {
    pub fn launch(jar: &Path, work_dir: &Path, timeout: Duration) -> Result<Self> {
        if !jar.is_file() {
            bail!("vanilla server jar missing: {}", jar.display());
        }
        std::fs::create_dir_all(work_dir)
            .with_context(|| format!("create {}", work_dir.display()))?;
        std::fs::write(work_dir.join("eula.txt"), "eula=true\n")?;
        let port = reserve_local_port()?;
        std::fs::write(
            work_dir.join("server.properties"),
            format!(
                "online-mode=false\nserver-ip=127.0.0.1\nserver-port={port}\nlevel-name=world\nview-distance=2\nsimulation-distance=2\nspawn-protection=0\nallow-flight=true\n"
            ),
        )?;

        let mut child = Command::new("java")
            .arg("-Xms256M")
            .arg("-Xmx1G")
            .arg("-jar")
            .arg(jar)
            .arg("nogui")
            .current_dir(work_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("launch vanilla oracle {}", jar.display()))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing child stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("missing child stderr"))?;
        let stdin = child.stdin.take();
        let (tx, rx) = mpsc::channel();
        spawn_log_watcher(stdout, tx.clone());
        spawn_log_watcher(stderr, tx);

        let started = Instant::now();
        let mut recent = Vec::new();
        while started.elapsed() < timeout {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line) => {
                    recent.push(line.clone());
                    if recent.len() > 20 {
                        recent.remove(0);
                    }
                    if line.contains("Done") || line.contains("For help, type") {
                        return Ok(Self {
                            child,
                            stdin,
                            addr: SocketAddr::from(([127, 0, 0, 1], port)),
                        });
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(status) = child.try_wait()? {
                        bail!(
                            "vanilla oracle exited before ready with {status}: {}",
                            recent.join("\n")
                        );
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = child.kill();
        bail!(
            "timed out waiting for vanilla oracle: {}",
            recent.join("\n")
        );
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<()> {
        if let Some(stdin) = &mut self.stdin {
            let _ = stdin.write_all(b"stop\n");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

impl Drop for VanillaServerProcess {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

fn spawn_log_watcher<R>(reader: R, tx: mpsc::Sender<String>)
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_java_major_versions() {
        assert_eq!(
            parse_java_major_version("openjdk version \"25.0.1\" 2026-01-01"),
            Some(25)
        );
        assert_eq!(
            parse_java_major_version("openjdk version \"21.0.9\" 2025-10-21 LTS"),
            Some(21)
        );
    }

    #[test]
    fn missing_oracle_reports_clear_skip_message() {
        let temp = tempfile::tempdir().expect("tempdir");
        let availability = vanilla_oracle_availability(temp.path());
        assert!(!availability.is_available());
        let message = availability.skip_message().expect("skip message");
        assert!(message.contains(".analysis/server.jar"));
        assert!(message.contains("skipping vanilla-backed parity test"));
    }

    #[test]
    fn core_action_generator_is_seeded_and_shrinkable() {
        let first = CoreActionGenerator::generate(0x51, 8);
        let second = CoreActionGenerator::generate(0x51, 8);
        let different = CoreActionGenerator::generate(0x52, 8);

        assert_eq!(first, second);
        assert_ne!(first, different);
        assert_eq!(shrink_action_prefix(&first, 3), &first[..3]);
        assert_eq!(shrink_action_prefix(&first, 99), &first[..]);
        assert!(first.iter().all(|action| !action.summary().is_empty()));
    }

    #[test]
    fn observation_diff_is_order_insensitive_and_points_at_phase() {
        let mut expected = ObservationSet::new("vanilla", "break-dirt");
        expected.push(ObservationFact::PacketSeen { id: 0x23 });
        expected.push(ObservationFact::BlockState {
            x: 0,
            y: 64,
            z: 0,
            state_id: 0,
        });

        let mut actual = ObservationSet::new("solaris", "break-dirt");
        actual.push(ObservationFact::BlockState {
            x: 0,
            y: 64,
            z: 0,
            state_id: 1,
        });
        actual.push(ObservationFact::PacketSeen { id: 0x23 });

        let diff = diff_observations(&expected, &actual);
        assert_eq!(diff.phase, "break-dirt");
        assert_eq!(diff.missing_from_actual.len(), 1);
        assert_eq!(diff.unexpected_in_actual.len(), 1);
        assert!(diff.to_string().contains("missing from actual"));
    }

    #[test]
    fn identical_observations_match_after_normalization() {
        let mut left = ObservationSet::new("vanilla", "spawn");
        left.push(ObservationFact::PacketSeen { id: 1 });
        left.push(ObservationFact::PacketSeen { id: 1 });
        let mut right = ObservationSet::new("solaris", "spawn");
        right.push(ObservationFact::PacketSeen { id: 1 });

        assert!(diff_observations(&left, &right).is_empty());
    }
}
