//! Dashboard data provider: bridges live mc-net handles into the dashboard
//! payload, plus the bounded WARN/ERROR ring the dashboard exposes as recent
//! warnings.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::ServerConfig;
use crate::dashboard::{
    AutoscaleView, ChunksView, DashboardStats, EntitiesView, LatencyUs, MemoryView, NetworkView,
    PlayersView, SaveReportView, SpawnCategoryReport, SpawnView, StatsPayload, TickView, WorldView,
};

const WARNING_RING_CAPACITY: usize = 128;
const ONLINE_PLAYER_NAME_LIMIT: usize = 64;

/// Bounded retained log ring for WARN/ERROR lines.
pub struct WarningRing {
    capacity: usize,
    lines: Mutex<VecDeque<String>>,
}

impl WarningRing {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            lines: Mutex::new(VecDeque::new()),
        }
    }

    fn push_line(&self, line: &str) {
        let mut lines = self.lines.lock().expect("warning ring lock poisoned");
        if lines.len() >= self.capacity {
            lines.pop_front();
        }
        lines.push_back(line.to_owned());
    }

    /// Current ring contents, oldest first.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("warning ring lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

/// Cloneable writer that appends formatted events into a [`WarningRing`].
#[derive(Clone)]
pub struct WarningRingSink(pub Arc<WarningRing>);

impl io::Write for WarningRingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let line = String::from_utf8_lossy(buf);
        self.0.push_line(line.trim_end());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for WarningRingSink {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Build the stdout + warning-ring subscriber pair and return the ring.
#[must_use]
pub fn warning_ring() -> Arc<WarningRing> {
    Arc::new(WarningRing::new(WARNING_RING_CAPACITY))
}

/// Live provider assembled from one bound server's read-only handles.
pub struct ServerDashboardStats {
    started_at: Instant,
    world: WorldView,
    autoscale_enabled: bool,
    autoscale_profile: String,
    telemetry: mc_net::RuntimeTelemetryHandle,
    control: Option<mc_net::RuntimeControlHandle>,
    pressure: mc_net::OutboundPressureHandle,
    facts: mc_net::OperatorFactsHandle,
    tick_watch: tokio::sync::watch::Receiver<u64>,
    plugins: crate::dashboard::PluginsView,
    warnings: Arc<WarningRing>,
    tps_sample: Mutex<Option<(u64, Instant)>>,
}

impl ServerDashboardStats {
    /// Assemble the provider from a bound server and its configuration.
    pub fn new(
        bound: &mc_net::BoundServer,
        config: &ServerConfig,
        started_at: Instant,
        warnings: Arc<WarningRing>,
        plugin_ids: Vec<String>,
    ) -> Self {
        let telemetry = bound.runtime_telemetry_handle();
        let tick_watch = telemetry.subscribe_simulation_ticks();
        Self {
            started_at,
            world: WorldView {
                name: config.server.name.clone(),
                motd: config.server.motd.clone(),
                seed: config.data.seed,
                mode: format!("{:?}", config.data.worldgen_mode),
                online_mode: config.auth.online_mode,
                view_distance: config.server.view_distance.max(0) as u32,
                simulation_distance: config.server.simulation_distance.max(0) as u32,
                max_players: config.server.max_players,
            },
            autoscale_enabled: config.autoscale.enabled,
            autoscale_profile: format!("{:?}", config.autoscale.profile),
            control: bound.runtime_control_handle(),
            pressure: bound.outbound_pressure_handle(),
            facts: bound.operator_facts_handle(),
            telemetry,
            tick_watch,
            plugins: crate::dashboard::PluginsView {
                loaded: plugin_ids,
                disabled: Vec::new(),
            },
            warnings,
            tps_sample: Mutex::new(None),
        }
    }

    fn tps(&self, current_tick: u64, now: Instant) -> f64 {
        let mut sample = self.tps_sample.lock().expect("tps sample lock poisoned");
        let tps = if let Some((tick, at)) = *sample {
            let ticks = current_tick.saturating_sub(tick);
            let secs = now.duration_since(at).as_secs_f64();
            if ticks > 0 && secs > 0.25 {
                ticks as f64 / secs
            } else {
                0.0
            }
        } else {
            0.0
        };
        *sample = Some((current_tick, now));
        tps
    }
}

fn latency(value: mc_net::RuntimeLatencyPercentiles) -> LatencyUs {
    LatencyUs {
        samples: value.samples as u64,
        p50_us: value.p50_us,
        p95_us: value.p95_us,
        p99_us: value.p99_us,
        max_us: value.max_us,
    }
}

impl DashboardStats for ServerDashboardStats {
    fn stats(&self) -> StatsPayload {
        let now = Instant::now();
        let telemetry = self.telemetry.snapshot();
        let counters = mc_net::operator_counter_snapshot();
        let pressure = self.pressure.snapshot();
        let (names, _) = self.facts.online_player_names(ONLINE_PLAYER_NAME_LIMIT);
        let current_tick = *self.tick_watch.borrow();

        let tick = telemetry
            .tick_percentiles
            .map(|percentiles| TickView {
                total: latency(percentiles.tick),
                stages: [
                    ("world_time", percentiles.world_time),
                    ("sheep_grazing", percentiles.sheep_grazing),
                    ("animal_breeding", percentiles.animal_breeding),
                    ("hostile_attacks", percentiles.hostile_attacks),
                    ("entity_goals", percentiles.entity_goals),
                    ("entity_physics", percentiles.entity_physics),
                    ("entity_dispatch", percentiles.entity_dispatch),
                    ("campfire_tick", percentiles.campfire_tick),
                    ("inhabited_time", percentiles.inhabited_time),
                    ("entity_save", percentiles.entity_save),
                    ("random_tick", percentiles.random_tick),
                    ("block_tick", percentiles.block_tick),
                    ("fluid_tick", percentiles.fluid_tick),
                ]
                .into_iter()
                .map(|(name, value)| (name.to_owned(), latency(value)))
                .collect(),
            })
            .unwrap_or_default();

        let autoscale = self
            .control
            .as_ref()
            .map(|control| {
                let snapshot = control.snapshot();
                AutoscaleView {
                    enabled: self.autoscale_enabled,
                    profile: self.autoscale_profile.clone(),
                    view_distance: snapshot.limits.view_distance.max(0) as u32,
                    chunk_send_rate: snapshot.limits.chunk_send_rate,
                    chunk_load_rate: snapshot.limits.chunk_load_rate,
                    chunk_generate_rate: snapshot.limits.chunk_generate_rate,
                    scale_up_decisions: snapshot.scale_up_decisions,
                    scale_down_decisions: snapshot.scale_down_decisions,
                    draining: snapshot.draining,
                }
            })
            .unwrap_or_else(|| AutoscaleView {
                enabled: false,
                profile: self.autoscale_profile.clone(),
                view_distance: self.world.view_distance,
                chunk_send_rate: 0,
                chunk_load_rate: 0,
                chunk_generate_rate: 0,
                scale_up_decisions: 0,
                scale_down_decisions: 0,
                draining: false,
            });

        let save = self
            .facts
            .last_save_report()
            .map(|retained| SaveReportView {
                age_secs: retained.at.elapsed().as_secs(),
                players_saved: retained.report.players_saved as u64,
                entities_saved: retained.report.entities_saved as u64,
                chunks_flushed: retained.report.chunks_flushed as u64,
                world_metadata_saved: retained.report.world_metadata_saved,
                elapsed_ms: retained.report.timings.total_us / 1_000,
                errors: retained.report.errors.clone(),
            });

        let spawn = self
            .facts
            .natural_spawn_report()
            .map(|retained| SpawnView {
                friendly: spawn_category(retained.report.friendly),
                hostile: spawn_category(retained.report.hostile),
            })
            .unwrap_or_default();

        StatsPayload {
            version: format!(
                "Solaris {} (MC {})",
                crate::VERSION,
                mc_protocol::TARGET_RELEASE
            ),
            uptime_secs: self.started_at.elapsed().as_secs(),
            world: self.world.clone(),
            players: PlayersView {
                count: telemetry.active_sessions as u32,
                max: self.world.max_players,
                names,
            },
            tps: self.tps(current_tick, now),
            tick,
            memory: MemoryView {
                used_mb: telemetry.memory_used_mb,
                limit_mb: telemetry.memory_limit_mb,
                available_mb: telemetry
                    .memory_limit_mb
                    .saturating_sub(telemetry.memory_used_mb),
            },
            autoscale,
            chunks: ChunksView {
                ticketed: telemetry.ticketed_chunks as u64,
                prepared: telemetry.prepared_chunks as u64,
                loaded_total: counters.chunks_loaded,
                generated_total: counters.chunks_generated,
                streamed_total: counters.chunks_streamed,
            },
            entities: EntitiesView {
                total: telemetry.server_entities as u64,
                categories: self.facts.entity_category_counts(),
            },
            spawn,
            save,
            network: NetworkView {
                bytes_written: counters.outbound_bytes_written,
                reliable_drops: pressure.reliable_command_drops,
                reliable_retries: pressure.reliable_command_retries,
                slow_client_sheds: pressure.slow_client_pressure_sheds,
                best_effort_animation_drops: pressure.best_effort_animation_drops,
            },
            plugins: self.plugins.clone(),
            warnings: self.warnings.lines(),
        }
    }
}

fn spawn_category(value: mc_net::NaturalSpawnCategoryReport) -> SpawnCategoryReport {
    SpawnCategoryReport {
        attempts: value.attempts,
        chunks_sampled: value.chunks_sampled,
        templates_considered: value.templates_considered,
        committed: value.committed,
        rejected_unloaded: value.rejected_unloaded,
        rejected_time: value.rejected_time,
        rejected_player_distance: value.rejected_player_distance,
        rejected_block_or_fluid: value.rejected_block_or_fluid,
        rejected_darkness: value.rejected_darkness,
        rejected_collision: value.rejected_collision,
        rejected_cap: value.rejected_cap,
        rejected_duplicate: value.rejected_duplicate_or_stale,
    }
}
