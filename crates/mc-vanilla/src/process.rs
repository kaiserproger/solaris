//! Vanilla server process ownership for content import and parity tooling.
//!
//! Launches the developer-supplied Mojang server jar in a scratch directory,
//! waits for its own readiness log line (no polling sleep), and stops it on
//! drop. Mojang artifacts never enter the repository; only paths do.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

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

/// Pick a currently-free localhost port for a child server process.
pub fn reserve_local_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("reserve localhost port")?;
    Ok(listener.local_addr()?.port())
}

/// Typed failure while launching the vanilla oracle process.
///
/// The capture step maps these onto its own error so a caller can tell "Java
/// is not installed" from "the server never became ready" without parsing
/// message text.
#[derive(Debug, thiserror::Error)]
pub enum VanillaLaunchError {
    #[error("vanilla server jar missing: {}", path.display())]
    JarMissing { path: PathBuf },

    #[error("prepare the vanilla work directory at {}: {detail}", path.display())]
    WorkDir { path: PathBuf, detail: String },

    #[error("launch vanilla oracle {} with {}: {source}", jar.display(), program.display())]
    Spawn {
        jar: PathBuf,
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("vanilla oracle did not become ready within {timeout:?}: {log}")]
    NotReady { timeout: Duration, log: String },

    #[error("vanilla oracle exited before ready with {status}: {log}")]
    Exited { status: ExitStatus, log: String },

    #[error("reap the vanilla oracle process: {detail}")]
    Reap { detail: String },
}

/// Owns a running vanilla oracle process. Dropping it tries to stop the server
/// gracefully before killing the child as a fallback.
pub struct VanillaServerProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    log_rx: mpsc::Receiver<String>,
    addr: SocketAddr,
}

impl VanillaServerProcess {
    /// Launch with the `JAVA` environment override, or `java` from `PATH`.
    pub fn launch(jar: &Path, work_dir: &Path, timeout: Duration) -> Result<Self> {
        let java = std::env::var_os("JAVA").map_or_else(|| PathBuf::from("java"), PathBuf::from);
        Ok(Self::launch_with_java(jar, work_dir, &java, timeout)?)
    }

    /// Launch `jar` with an explicit Java executable.
    ///
    /// Readiness is the server's own "Done"/command-prompt log line, watched on
    /// the child's stdout/stderr; there is no polling sleep.
    pub fn launch_with_java(
        jar: &Path,
        work_dir: &Path,
        java: &Path,
        timeout: Duration,
    ) -> Result<Self, VanillaLaunchError> {
        Self::launch_with_world(jar, work_dir, java, timeout, false)
    }

    /// Launch a disposable world for the configuration-only registry capture.
    pub(crate) fn launch_for_registry_capture(
        jar: &Path,
        work_dir: &Path,
        java: &Path,
        timeout: Duration,
    ) -> Result<Self, VanillaLaunchError> {
        Self::launch_with_world(jar, work_dir, java, timeout, true)
    }

    fn launch_with_world(
        jar: &Path,
        work_dir: &Path,
        java: &Path,
        timeout: Duration,
        registry_capture: bool,
    ) -> Result<Self, VanillaLaunchError> {
        if !jar.is_file() {
            return Err(VanillaLaunchError::JarMissing {
                path: jar.to_path_buf(),
            });
        }
        let work_dir_error = |error: &dyn std::fmt::Display| VanillaLaunchError::WorkDir {
            path: work_dir.to_path_buf(),
            detail: error.to_string(),
        };
        std::fs::create_dir_all(work_dir).map_err(|error| work_dir_error(&error))?;
        std::fs::write(work_dir.join("eula.txt"), "eula=true\n")
            .map_err(|error| work_dir_error(&error))?;
        let port = reserve_local_port().map_err(|error| work_dir_error(&error))?;
        let capture_properties = if registry_capture {
            // The debug preset skips the normal global spawn search.
            "level-type=minecraft:debug_all_block_states\ngenerate-structures=false\n"
        } else {
            ""
        };
        std::fs::write(
            work_dir.join("server.properties"),
            format!(
                "online-mode=false\nwhite-list=false\nserver-ip=127.0.0.1\nserver-port={port}\nlevel-name=world\nview-distance=2\nsimulation-distance=2\nspawn-protection=0\nallow-flight=true\n{capture_properties}"
            ),
        )
        .map_err(|error| work_dir_error(&error))?;

        let mut child = Command::new(java)
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
            .map_err(|source| VanillaLaunchError::Spawn {
                jar: jar.to_path_buf(),
                program: java.to_path_buf(),
                source,
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| VanillaLaunchError::Reap {
                detail: "missing child stdout".into(),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| VanillaLaunchError::Reap {
                detail: "missing child stderr".into(),
            })?;
        let stdin = child.stdin.take();
        let (tx, rx) = mpsc::channel();
        spawn_log_watcher(stdout, tx.clone());
        spawn_log_watcher(stderr, tx);

        let deadline = Instant::now() + timeout;
        let mut recent = Vec::new();
        let exited = |child: &mut Child, recent: &Vec<String>| {
            let status = child.wait().map_err(|error| VanillaLaunchError::Reap {
                detail: error.to_string(),
            })?;
            Err(VanillaLaunchError::Exited {
                status,
                log: recent.join("\n"),
            })
        };
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let _ = child.kill();
                return Err(VanillaLaunchError::NotReady {
                    timeout,
                    log: recent.join("\n"),
                });
            }
            match rx.recv_timeout(remaining) {
                Ok(line) => {
                    recent.push(line.clone());
                    if recent.len() > 20 {
                        recent.remove(0);
                    }
                    if line.contains("Done") || line.contains("For help, type") {
                        return Ok(Self {
                            child,
                            stdin,
                            log_rx: rx,
                            addr: SocketAddr::from(([127, 0, 0, 1], port)),
                        });
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    return Err(VanillaLaunchError::NotReady {
                        timeout,
                        log: recent.join("\n"),
                    });
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return exited(&mut child, &recent),
            }
        }
    }

    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn send_command(&mut self, command: &str) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("vanilla oracle stdin is closed"))?;
        stdin.write_all(command.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    pub fn wait_for_log(
        &mut self,
        timeout: Duration,
        matches: impl Fn(&str) -> bool,
    ) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timed out waiting for vanilla oracle log event");
            }
            match self.log_rx.recv_timeout(remaining) {
                Ok(line) if matches(&line) => return Ok(line),
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let status = self.child.wait()?;
                    bail!("vanilla oracle exited while waiting for log event: {status}");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    bail!("timed out waiting for vanilla oracle log event");
                }
            }
        }
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<()> {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
            let _ = stdin.flush();
        }
        // A fresh 26.3 server spends ~10 s saving overworld chunks on `stop`
        // (11.1 s measured), so the old 10 s deadline killed a healthy stop.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let _ = self.child.kill();
                let _ = self.child.wait();
                bail!("timed out waiting for vanilla oracle to stop");
            }
            match self.log_rx.recv_timeout(remaining) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = self.child.wait()?;
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    bail!("timed out waiting for vanilla oracle to stop");
                }
            }
        }
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
}
