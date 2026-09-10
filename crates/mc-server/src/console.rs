use std::collections::VecDeque;
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use mc_server::{
    dashboard::DashboardStats,
    dashboard_stats::{WarningRing, WarningRingSink},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub(crate) mod commands;
pub(crate) mod server_commands;

use commands::{CommandHandler, ConsoleCommand, ConsoleReply};

#[derive(Clone)]
pub(super) struct ConsoleOutput {
    lines: Arc<WarningRing>,
}

impl ConsoleOutput {
    pub fn new() -> Self {
        Self {
            lines: Arc::new(WarningRing::new(128)),
        }
    }

    pub fn supported() -> bool {
        io::stdin().is_terminal()
            && io::stdout().is_terminal()
            && std::env::var("TERM").is_ok_and(|term| term != "dumb")
    }

    fn message(&self, text: &str) {
        let _ = WarningRingSink(Arc::clone(&self.lines)).write_all(text.as_bytes());
    }
}

struct ScreenGuard;

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

pub(super) struct Console<H: CommandHandler> {
    pub stats: Arc<dyn DashboardStats>,
    pub ticks: tokio::sync::watch::Receiver<u64>,
    pub handler: H,
    pub output: ConsoleOutput,
    pub interactive: bool,
}

impl<H: CommandHandler> Console<H> {
    pub async fn run(mut self) -> Result<()> {
        if !self.interactive {
            return self.run_lines().await;
        }
        enable_raw_mode().context("enabling console input; use --no-console for plain stdin")?;
        let _guard = ScreenGuard;
        execute!(io::stdout(), EnterAlternateScreen)?;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        terminal.clear()?;
        let mut events = EventStream::new();
        let mut input = String::new();
        let mut history = VecDeque::<String>::new();
        let mut history_position = 0;
        let mut last_draw_tick = *self.ticks.borrow();
        self.output
            .message("help: commands | Up/Down: history | Ctrl-C or stop: save and shut down");
        loop {
            let stats = self.stats.stats();
            let lines = self.output.lines.lines();
            terminal.draw(|frame| {
                let areas = Layout::vertical([Constraint::Length(5), Constraint::Min(3), Constraint::Length(3)]).split(frame.area());
                let header = format!(
                    "{} | {} | uptime {}s\nPlayers {}/{} | TPS {:.1} | tick p95 {:.2} ms | memory {} MiB\nChunks {} prepared / {} ticketed | entities {} | plugins {}",
                    stats.version, stats.world.name, stats.uptime_secs, stats.players.count, stats.players.max,
                    stats.tps, stats.tick.total.p95_us as f64 / 1000.0, stats.memory.used_mb,
                    stats.chunks.prepared, stats.chunks.ticketed, stats.entities.total, stats.plugins.loaded.len()
                );
                frame.render_widget(Paragraph::new(clean_text(&header)).block(Block::default().borders(Borders::ALL).title("Server")), areas[0]);
                let visible = usize::from(areas[1].height.saturating_sub(2));
                let mut visible_lines = lines.iter().rev().flat_map(|line| line.lines().rev()).take(visible).collect::<Vec<_>>();
                visible_lines.reverse();
                let log = clean_text(&visible_lines.join("\n"));
                let log = Paragraph::new(log).wrap(Wrap { trim: false });
                let scroll = log.line_count(areas[1].width.saturating_sub(2))
                    .saturating_sub(visible).min(u16::MAX as usize) as u16;
                frame.render_widget(log.scroll((scroll, 0)).block(Block::default().borders(Borders::ALL).title("Console")), areas[1]);
                let width = usize::from(areas[2].width.saturating_sub(3));
                let offset = input.chars().count().saturating_sub(width);
                let shown: String = input.chars().skip(offset).collect();
                frame.render_widget(Paragraph::new(shown).block(Block::default().borders(Borders::ALL).title("Command")), areas[2]);
                if areas[2].width > 2 && areas[2].height > 2 {
                    frame.set_cursor_position((areas[2].x + 1 + input.chars().count().min(width) as u16, areas[2].y + 1));
                }
            })?;
            // The simulation producer wakes this view. No repaint timer or polling loop.
            loop {
                tokio::select! {
                    changed = self.ticks.changed() => {
                        if changed.is_err() { return Ok(()); }
                        let tick = *self.ticks.borrow_and_update();
                        if tick.saturating_sub(last_draw_tick) >= 20 {
                            last_draw_tick = tick;
                            break;
                        }
                    }
                    event = events.next() => {
                        match event.transpose()? {
                            None => return Ok(()),
                            Some(Event::Resize(_, _)) => break,
                            Some(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                                if key.modifiers.contains(KeyModifiers::CONTROL) {
                                    match key.code {
                                        KeyCode::Char('c' | 'd') => return Ok(()),
                                        KeyCode::Char('u') => input.clear(),
                                        _ => {}
                                    }
                                } else {
                                    match key.code {
                                        KeyCode::Char(c) if !c.is_control() && input.len() < 1024 => input.push(c),
                                        KeyCode::Backspace => { input.pop(); }
                                        KeyCode::Up if history_position > 0 => {
                                            history_position -= 1;
                                            input.clone_from(&history[history_position]);
                                        }
                                        KeyCode::Down => {
                                            history_position = (history_position + 1).min(history.len());
                                            input = history.get(history_position).cloned().unwrap_or_default();
                                        }
                                        KeyCode::Enter => {
                                            let command = std::mem::take(&mut input);
                                            let command = command.trim();
                                            if !command.is_empty() {
                                                self.output.message(&format!("> {command}"));
                                                if history.len() == 50 { history.pop_front(); }
                                                history.push_back(command.to_owned());
                                                history_position = history.len();
                                                let result = match ConsoleCommand::parse(command) {
                                                    Ok(command) => self.handler.execute(command).await,
                                                    Err(error) => Err(error.into()),
                                                };
                                                match result {
                                                    Ok(ConsoleReply::Shutdown) => return Ok(()),
                                                    Ok(ConsoleReply::Output(message)) => self.output.message(&message),
                                                    Err(error) => self.output.message(&format!("Error: {error:#}")),
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    async fn run_lines(self) -> Result<()> {
        let (sender, mut lines) = tokio::sync::mpsc::channel(16);
        // One process-owned reader; mc-net never consumes the embedding application's stdin.
        std::thread::Builder::new()
            .name("solaris-console-input".into())
            .spawn(move || {
                let stdin = io::stdin();
                for line in stdin.lock().lines() {
                    let failed = line.is_err();
                    if sender.blocking_send(line).is_err() || failed {
                        break;
                    }
                }
            })?;
        while let Some(line) = lines.recv().await {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let result = match ConsoleCommand::parse(&line) {
                Ok(command) => self.handler.execute(command).await,
                Err(error) => Err(error.into()),
            };
            match result {
                Ok(ConsoleReply::Shutdown) => return Ok(()),
                Ok(ConsoleReply::Output(message)) => println!("{}", clean_text(&message)),
                Err(error) => eprintln!("console: {error:#}"),
            }
        }
        // EOF is normal under systemd; it disables input, not the server.
        std::future::pending().await
    }
}

fn clean_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .collect()
}
