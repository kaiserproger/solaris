//! Every bound one plugin instance runs under.
//!
//! The numbers are Wasmtime's units: fuel counts guest instructions, the epoch
//! deadline is a wall-clock watchdog owned by a thread other than the one
//! running the guest, and memory is enforced per linear memory by the store
//! limiter. They are set before instantiation because a component can run guest
//! code while it is being initialized.
//!
//! The defaults below are deliberately conservative starting points, not
//! measured budgets: the plan requires calibrating them against the baseline and
//! two real Rust plugins before this host serves gameplay, and every field here
//! is meant to be replaced by a measurement.

/// What the host appends to a diagnostic line it had to cut. A line that carries
/// this marker was longer than [`PluginLimits::log_line_bytes`].
pub const TRUNCATION_MARKER: &str = "...[truncated]";

/// Bounds applied to one plugin instance and to the batches it answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginLimits {
    /// Largest accepted component artifact, in bytes. Checked before compiling,
    /// because compilation is host work the guest's fuel does not pay for.
    pub artifact_bytes: usize,
    /// Guest instructions allowed per callback.
    pub fuel_per_call: u64,
    /// Epoch ticks allowed per callback. One tick is the ticker interval, so this
    /// is the wall-clock watchdog: a guest that blocks in a host call still ends.
    pub epoch_ticks_per_call: u64,
    /// Bytes one callback may hand the host, counted the way Wasmtime charges
    /// them: every list element at its canonical stride and every string at its
    /// length. Wasmtime spends this budget *before* it copies anything, so an
    /// answer past it is refused rather than half-copied, and the cost does not
    /// depend on whether the guest's strings share one region of its memory.
    ///
    /// It is a bound on *host* memory, which every other limit here is not: the
    /// store limiter bounds the guest's memories, tables and instances, and a
    /// guest is charged nothing for the host copies its answer implies. Leaving
    /// this at Wasmtime's own default (2 GiB) is what a hostile guest exploits -
    /// a 64 MiB guest asked the host for gigabytes of `String`s in one call
    /// before this bound existed.
    pub hostcall_bytes: usize,
    /// Maximum guest linear-memory capacity held by the live and candidate
    /// deployment together during a reload. The host calculates this from every
    /// store's `memories * guest_memory_bytes` bound before it compiles or starts
    /// the candidate, so an oversized replacement cannot pressure out the live
    /// generation.
    pub reload_candidate_memory_bytes: usize,
    /// Bytes one linear memory may reach. Applied to each memory separately, so
    /// a component that declares several memories gets this bound each.
    pub guest_memory_bytes: usize,
    /// Elements one table may reach.
    pub table_elements: usize,
    /// Core instances one store may hold.
    pub instances: usize,
    /// Tables one store may hold.
    pub tables: usize,
    /// Linear memories one store may hold.
    pub memories: usize,
    /// Guest stack, in bytes.
    pub wasm_stack_bytes: u64,
    /// Commands one callback may return. The server clamps every batch to
    /// [`mc_script::MAX_SCRIPT_COMMAND_BATCH`], so this default *is* that bound:
    /// a host bound above it would let the staging area accept a batch the
    /// admission then refuses, and the plugin would be told nothing.
    pub commands_per_call: usize,
    /// Bytes one returned string may hold, after lifting.
    pub text_bytes: usize,
    /// Events one delivered batch may hold.
    pub events_per_batch: usize,
    /// Bytes one diagnostic line may hold. The host truncates a longer message on
    /// a character boundary and appends [`TRUNCATION_MARKER`], so a guest cannot
    /// choose how much of the operator's log one line costs.
    pub log_line_bytes: usize,
    /// Diagnostic lines one callback may emit. Further lines are dropped and
    /// counted, so a guest that logs in a loop cannot drive the sink's volume.
    pub log_lines_per_call: u32,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            artifact_bytes: 16 * 1024 * 1024,
            fuel_per_call: 50_000_000,
            epoch_ticks_per_call: 4,
            // Derived from what the contract itself admits rather than measured
            // from a workload: one callback may answer a batch of
            // `commands_per_call` commands, each at its text bound and each able
            // to name a storage key, a correlation id and a value at their own
            // bounds, which is 32 x (8192 + 128 + 64 + 4096) = 400 KiB; and the
            // `configure` plan admits 64 tree declarations of 64 biome names plus
            // 64 biomes of four groups of 32 entries, which is on the order of a
            // megabyte of names. 8 MiB is an order of magnitude above the largest
            // truthful answer and 256x below Wasmtime's default, so a plugin
            // doing its job never sees it while a hostile one does. A measurement
            // against the ported packages (P7) may lower it; nothing may raise it
            // back toward the default.
            hostcall_bytes: 8 * 1024 * 1024,
            // A strict standard-pack reload was refused at the previous 2 GiB
            // limit because its declared maximum was 2 × 5 × 4 × 64 MiB = 2.5
            // GiB. This admission capacity is reserved, not allocated; a sixth
            // package still fails closed. It remains provisional until P7
            // measures the real package workloads and calibrates the budget.
            reload_candidate_memory_bytes: 2 * 5 * 4 * 64 * 1024 * 1024,
            guest_memory_bytes: 64 * 1024 * 1024,
            table_elements: 100_000,
            instances: 4,
            tables: 8,
            memories: 4,
            wasm_stack_bytes: 1024 * 1024,
            commands_per_call: mc_script::MAX_SCRIPT_COMMAND_BATCH,
            text_bytes: 8192,
            events_per_batch: 512,
            log_line_bytes: 512,
            log_lines_per_call: 64,
        }
    }
}
