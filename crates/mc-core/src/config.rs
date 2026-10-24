//! [`Core`] configuration and error type.

use std::path::PathBuf;

use thiserror::Error;

/// Configuration for [`crate::Core`].
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// World root: `dimensions/minecraft/*`, `players/data/`, global `data/` and private `journal/`.
    pub world_dir: PathBuf,
    /// World seed for chunk generation.
    pub seed: i64,
    /// Chunk view distance (radius) sent to each player.
    pub view_distance: u8,
    /// Chunk gameplay simulation radius (independent of client residency).
    pub simulation_distance: u8,
    /// Bounded submit queue depth, including reserved session Leave credits.
    /// At least four slots are needed for atomic view/locale/Join/Leave admission.
    pub input_queue_cap: usize,
    /// Broadcast channel depth for [`TickOutput`](crate::boundary::TickOutput).
    pub broadcast_cap: usize,
    /// Per-connection reliable [`PlayerChange`](crate::boundary::PlayerChange)
    /// outbox depth. Overflow disconnects the connection.
    pub player_outbox_cap: usize,
    /// Journal fsync cadence: fsync after this many appended batches.
    pub journal_fsync_every_batches: u32,
    /// Checkpoint capture cadence; successful commits prune the covered journal.
    pub checkpoint_every_ticks: u64,
    /// Vanilla content-cache root (`data/minecraft/...`, `reports/...`).
    /// Required outside tests; missing content is a startup error. Only
    /// `#[cfg(test)]` builds fall back to the checkout's `data/vanilla`.
    pub vanilla_data_dir: Option<PathBuf>,
    /// Strictly discovered production component package directory supplied by edge.
    pub plugin_directory: Option<PathBuf>,
    /// `false` (default): native sessions play immediately.
    /// `true`: new native sessions start locked and fail closed until the
    /// auth plugin unlocks them. Owned by the AuthGate native gate.
    pub require_plugin_login: bool,
    /// Consecutive non-descending airborne movement packets before the native
    /// fly check corrects and disconnects a survival player. Zero disables it.
    pub max_airtime_ticks: u32,
    /// Vanilla `pause-when-empty-seconds`: stop advancing world simulation
    /// after this many seconds without an online player. Network, console and
    /// saves keep running, and the first join resumes ticking. Zero disables.
    /// The minimal config keeps it disabled for embedded/test cores; the edge
    /// server sets the vanilla default (60) from `[server]`.
    pub pause_when_empty_seconds: u32,
    /// Vanilla command-block execution gate; disabled by default.
    pub enable_command_block: bool,
    /// Per-mob switches layered under the `mobGriefing` gamerule.
    pub mob_griefing: MobGriefingRules,
    /// `[block_log]`: attributed block/container history for `/co`.
    pub block_log: crate::block_log::Settings,
    /// Opt-in per-tick input log under `<world>/input-log/` (replay support;
    /// see `input_log.rs`). Off by default.
    pub input_log: bool,
}

/// Finer block-damage switches under vanilla `mobGriefing` (which stays the
/// master gate). Vanilla 26.1.2 has no per-mob gamerules, so these are server
/// configuration; every value defaults to true, matching vanilla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobGriefingRules {
    /// Creeper blast removes blocks.
    pub creeper_block_damage: bool,
    /// Ghast fireball blast removes blocks.
    pub ghast_block_damage: bool,
    /// Wither spawn blast, skull blasts and the hurt-triggered block break.
    pub wither_block_damage: bool,
    /// Villager crop harvesting and replanting.
    pub villager_farming: bool,
    /// Sheared sheep eating grass into dirt while their wool regrows.
    pub sheep_grazing: bool,
}

impl Default for MobGriefingRules {
    fn default() -> Self {
        Self {
            creeper_block_damage: true,
            ghast_block_damage: true,
            wither_block_damage: true,
            villager_farming: true,
            sheep_grazing: true,
        }
    }
}

impl CoreConfig {
    /// Minimal config; queue depths and cadences take tested defaults.
    #[must_use]
    pub fn new(world_dir: PathBuf, seed: i64) -> Self {
        Self {
            world_dir,
            seed,
            view_distance: 8,
            simulation_distance: 6,
            input_queue_cap: 4096,
            broadcast_cap: 32,
            player_outbox_cap: 512,
            journal_fsync_every_batches: 10,
            checkpoint_every_ticks: 1200,
            vanilla_data_dir: None,
            plugin_directory: None,
            require_plugin_login: false,
            max_airtime_ticks: 80,
            pause_when_empty_seconds: 0,
            enable_command_block: false,
            mob_griefing: MobGriefingRules::default(),
            block_log: crate::block_log::Settings::default(),
            input_log: false,
        }
    }
}

/// Every failure mode of [`crate::Core`] startup and operation.
#[derive(Debug, Error)]
#[cfg_attr(test, derive(Clone))]
pub enum CoreError {
    /// Component discovery, configuration or runtime startup failed closed.
    #[error("plugin: {0}")]
    Plugin(String),
    /// World directory or journal setup failed.
    #[error("storage: {0}")]
    Storage(String),
    /// Block/entity/item registries did not build.
    #[error("registry: {0}")]
    Registry(String),
    /// Worldgen router or density data failed to load.
    #[error("worldgen: {0}")]
    Worldgen(String),
    /// Journal recovery found no complete batch and no manifest.
    #[error("recovery: {0}")]
    Recovery(String),
    /// The tick thread has stopped.
    #[error("tick thread stopped")]
    TickStopped,
    /// Input capacity cannot admit the complete initial native session.
    #[error("native input queue full")]
    InputFull,
    /// Remote ticket exceeds its budget.
    #[error("ticket over budget: {0}")]
    TicketOverBudget(String),
}
