//! Line-oriented operator console. A detached stdin reader cannot hold Tokio's
//! blocking pool open after stop/SIGTERM while waiting for another input line.
use std::io::{BufRead, Write};
use std::sync::Arc;

use mc_core::boundary::{PlayerChange, PlayerInput};
use mc_core::command_console::{CONNECTION, ConsoleCommand};

use crate::server::EdgeServer;
/// Serializes stdin and RCON on the existing console source. Only an active
/// RCON command retains output, bounded independently of command volume.
#[derive(Default)]
pub(crate) struct ConsoleState {
    serial: tokio::sync::Mutex<()>,
    response: parking_lot::Mutex<Option<Result<String, &'static str>>>,
    drained: std::sync::atomic::AtomicU64,
    ready: tokio::sync::Notify,
    pub(crate) running: std::sync::atomic::AtomicBool,
    language: std::collections::HashMap<String, String>,
}

impl ConsoleState {
    pub(crate) fn load(root: &std::path::Path) -> anyhow::Result<Self> {
        let path = root.join("assets/minecraft/lang/en_us.json");
        let bytes = std::fs::read(&path).map_err(|error| {
            anyhow::anyhow!(
                "cannot read {}: {error}; reimport vanilla content",
                path.display()
            )
        })?;
        Ok(Self {
            language: serde_json::from_slice(&bytes)?,
            ..Self::default()
        })
    }

    pub(crate) fn feedback(&self, message: &str) {
        if let Some(response) = self.response.lock().as_mut()
            && let Ok(text) = response
        {
            if text.len() + message.len() < 1024 * 1024 {
                text.push_str(message);
                text.push('\n');
            } else {
                *response = Err("RCON output exceeded 1 MiB; full feedback is on stdout");
            }
        }
        print_line(message);
    }

    pub(crate) fn drained(&self, tick: u64) {
        self.drained
            .store(tick, std::sync::atomic::Ordering::Release);
        self.ready.notify_waiters();
    }
    async fn wait_drained(&self, tick: u64) {
        loop {
            let ready = self.ready.notified();
            if self.drained.load(std::sync::atomic::Ordering::Acquire) >= tick {
                break;
            }
            ready.await;
        }
    }
}

struct Capture<'a>(&'a ConsoleState);
impl Drop for Capture<'_> {
    fn drop(&mut self) {
        self.0.response.lock().take();
    }
}

pub(crate) async fn remote(server: &EdgeServer, line: &str) -> anyhow::Result<String> {
    let _serial = server.console_state.serial.lock().await;
    let capture = Capture(&server.console_state);
    *capture.0.response.lock() = Some(Ok(String::new()));
    if let Some(tick) = execute_inner(server, line).await? {
        capture.0.wait_drained(tick).await;
    }
    capture
        .0
        .response
        .lock()
        .take()
        .unwrap_or_else(|| Ok(String::new()))
        .map_err(anyhow::Error::msg)
}

fn read_console(reader: impl BufRead, sender: tokio::sync::mpsc::Sender<std::io::Result<String>>) {
    for line in reader.lines() {
        let failed = line
            .as_ref()
            .is_err_and(|error| error.kind() != std::io::ErrorKind::InvalidData);
        if sender.blocking_send(line).is_err() || failed {
            break;
        }
    }
}

pub(crate) fn spawn(server: Arc<EdgeServer>) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let (sender, mut lines) = tokio::sync::mpsc::channel(16);
    std::thread::Builder::new()
        .name("server-console".into())
        .spawn(move || read_console(std::io::stdin().lock(), sender))?;
    Ok(tokio::spawn(async move {
        while let Some(line) = lines.recv().await {
            match line {
                Ok(line) => {
                    if let Err(error) = execute(&server, &line).await {
                        print_line(&format!("Command failed: {error:#}"));
                    }
                }
                Err(error) => {
                    print_line(&format!("Console input failed: {error}"));
                }
            }
        }
    }))
}

/// Console-only `backup`: copies the running world into `[data] backup_dir`
/// without pausing ticks. It runs detached, so the console stays usable and
/// the outcome prints when the copy finishes.
fn backup(server: &EdgeServer, command: &str) {
    if command != "backup" {
        server
            .console_state
            .feedback("Incorrect argument for command");
        return;
    }
    let Some(directory) = server.cfg.data.backup_dir.clone() else {
        server
            .console_state
            .feedback("Backups are disabled: set backup_dir under [data]");
        return;
    };
    server
        .console_state
        .feedback("Backing up the world (the server keeps running)");
    let core = Arc::clone(&server.hub);
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let result =
            tokio::task::spawn_blocking(move || crate::backup::run_hot_backup(&core, &directory))
                .await;
        match result {
            Ok(Ok((path, backup))) => print_line(&format!(
                "Backup {} complete: checkpoint tick {}, recoverable to tick {}, {} files, {} ms",
                path.display(),
                backup.checkpoint_tick,
                backup.wal_tick,
                backup.files.len(),
                started.elapsed().as_millis()
            )),
            Ok(Err(error)) => print_line(&format!("Backup failed: {error:#}")),
            Err(error) => print_line(&format!("Backup failed: {error}")),
        }
    });
}

pub(crate) async fn execute(server: &EdgeServer, line: &str) -> anyhow::Result<()> {
    let _serial = server.console_state.serial.lock().await;
    let tick = execute_inner(server, line).await?;
    if let Some(tick) = tick
        && server
            .console_state
            .running
            .load(std::sync::atomic::Ordering::Relaxed)
    {
        server.console_state.wait_drained(tick).await;
    }
    Ok(())
}

async fn execute_inner(server: &EdgeServer, line: &str) -> anyhow::Result<Option<u64>> {
    let command = line.trim().trim_start_matches('/');
    if command.is_empty() {
        return Ok(None);
    }
    if command.split_whitespace().next() == Some("backup")
        && !server
            .hub
            .plugin_commands()
            .iter()
            .any(|(name, _)| name == "backup")
    {
        backup(server, command);
        return Ok(None);
    }
    if server.handle_admin_command(CONNECTION, command).await? {
        return Ok(None);
    }
    let mut published = server.hub.subscribe();
    let (command, completed) = ConsoleCommand::new(command.into());
    let input = PlayerInput::ConsoleCommand(command);
    // Console input uses the same bounded admission as authenticated players.
    anyhow::ensure!(
        server.hub.submit(input),
        "authoritative command queue is full"
    );
    let completed_tick = completed
        .await
        .map_err(|_| anyhow::anyhow!("core stopped before command execution"))?;
    loop {
        match published.recv().await {
            Ok(output) if output.tick >= completed_tick => return Ok(Some(completed_tick)),
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                anyhow::bail!("core stopped before command publication");
            }
        }
    }
}

pub(crate) fn print_line(message: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

fn component_text(
    component: &mc_nbt::Tag,
    language: &std::collections::HashMap<String, String>,
) -> String {
    use mc_nbt::Tag;
    match component {
        Tag::String(text) => text.clone(),
        Tag::List(list) => list
            .elements
            .iter()
            .map(|value| component_text(value, language))
            .collect(),
        Tag::Compound(fields) => {
            let field = |key: &str| {
                fields
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value)
            };
            let mut text = if let Some(text) = field("text") {
                component_text(text, language)
            } else if let Some(Tag::String(key)) = field("translate") {
                let fallback = match field("fallback") {
                    Some(Tag::String(value)) => value.as_str(),
                    _ => key,
                };
                let template = language.get(key).map_or(fallback, String::as_str);
                let args = match field("with") {
                    Some(Tag::List(args)) => args
                        .elements
                        .iter()
                        .map(|arg| component_text(arg, language))
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                };
                let mut rendered = String::new();
                let mut rest = template;
                let mut next = 0usize;
                while let Some(index) = rest.find('%') {
                    rendered.push_str(&rest[..index]);
                    rest = &rest[index + 1..];
                    if let Some(suffix) = rest.strip_prefix('%') {
                        rendered.push('%');
                        rest = suffix;
                    } else if let Some(suffix) = rest.strip_prefix('s') {
                        if let Some(arg) = args.get(next) {
                            rendered.push_str(arg);
                        }
                        next += 1;
                        rest = suffix;
                    } else {
                        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                        if digits > 0 && rest[digits..].starts_with("$s") {
                            if let Ok(index) = rest[..digits].parse::<usize>()
                                && let Some(arg) =
                                    index.checked_sub(1).and_then(|index| args.get(index))
                            {
                                rendered.push_str(arg);
                            }
                            rest = &rest[digits + 2..];
                        } else {
                            rendered.push('%');
                        }
                    }
                }
                rendered.push_str(rest);
                rendered
            } else {
                String::new()
            };
            if let Some(extra) = field("extra") {
                text.push_str(&component_text(extra, language));
            }
            text
        }
        _ => String::new(),
    }
}

pub(crate) fn output(state: &ConsoleState, changes: &[PlayerChange]) {
    for change in changes {
        match change {
            PlayerChange::Chat { message, .. } => state.feedback(message),
            PlayerChange::StyledChat { content, .. } => {
                state.feedback(&component_text(content, &state.language))
            }
            PlayerChange::Disconnect { reason, .. } => state.feedback(reason),
            PlayerChange::Title(
                mc_core::boundary::TitleChange::Title(content)
                | mc_core::boundary::TitleChange::Subtitle(content)
                | mc_core::boundary::TitleChange::ActionBar(content),
            ) => state.feedback(&component_text(content, &state.language)),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
pub(crate) mod tests;
