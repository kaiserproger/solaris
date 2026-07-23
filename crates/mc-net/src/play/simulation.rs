use super::block_edit_commit::{
    apply_block_edit_batch_to_storage_conditionally,
    apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally,
    apply_opaque_block_entity_to_storage_conditionally,
};
use super::explosions::{
    CommittedTntIgnition, EntityExplosionImpact, ExplosionBlockSample, JavaLegacyRandom,
    PlayerExplosionImpact, TNT_ENTITY_TYPE_NAME, TntIgnitionPlan, plan_entity_explosion_impact,
    plan_explosion_candidates, plan_player_explosion_impact,
};
use super::falling_blocks::{
    LandedFallingBlock, falling_block_start_chunks, is_falling_block_state,
    plan_falling_block_starts,
};
use super::inventory::PlayerInventory;
use super::lighting::{
    IncrementalLightSources, capture_incremental_light_sources,
    capture_incremental_light_sources_from_read_view, collect_full_light_updates_for_current_world,
    collect_incremental_light_updates_for_applied_edits, compute_incremental_light_updates,
    incremental_light_sources_are_current, persist_baked_light_updates,
};
use super::persistence::PersistedEntityCheckpoint;
#[cfg(test)]
use super::session::EntityKillRewards;
use super::session::{
    BucketUseTransaction, CampfireUseTransaction, ChestTransaction, ChestTransactionRequest,
    ContainerCommitContext, ContainerStateCommitError, CreditedArrowPickup,
    CreditedExperiencePickup, CreditedItemPickup, ENTITY_DEATH_TICKS, EntityAttackOutcome,
    FurnaceTransaction, FurnaceTransactionRequest, OutboundCommand, PlayerAttackResult,
    PlayerEntityAttack, PlayerInventoryCommitError, ScriptPlayerTeleportCompletion,
    ServerEntityExplosionImpact, SessionId, SessionRegistry, SurvivalBreakTransaction,
    SurvivalPlacementTransaction, VisibilityDispatch, dispatch_visibility_commands,
};
use super::{
    AppliedBlockEdit, ArrowPhysicsFact, BlockDelta, BlockEdit, BlockEditBatchOutcome,
    BlockEditPrecondition, BlockMutationSnapshot, CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
    CampfireCookingState, ChestCommitOutcome, ChestView, ContainerDropPlan, ContainerPlayerPlan,
    ContainerXpPlan, EntityPhysicsQuery, EntityPhysicsStep, FurnaceCommitOutcome, GameMode,
    HerdSpawn, PendingCampfireOutput, PlayerInventoryCommitOutcome, PlayerPose,
    SharedContainerCommit, SurvivalState, WorldHandle, air_state_id, block_edit_changes_light,
    chest_slot_stacks, furnace_output_was_taken, furnace_slot_stacks, is_campfire_block,
    schedule_fluid_ticks_near_applied, schedule_leaf_ticks_near_applied,
};
use mc_data::block_facts::BlockFactsTable;
use mc_data::block_light::BlockLightTable;
use mc_entity::runtime_26_1_2::TargetKind;
use mc_entity::{
    EntityEffectOperation, EntityEffectRequest, EntityEffectResult, EntityId, EntityItemStack,
    EntitySnapshot, REGION_SIZE_CHUNKS, RegionKey, RegionLease, RegionOwnership,
    RegionOwnershipError, RegionPhase, Rotation, Vec3,
};
use mc_physics::BlockMaterialIds;
use mc_protocol::packets::play::ItemStack;
use mc_script::ScriptPlayerTeleportFailure;
use mc_world::{
    BlockMutationToken, BlockPos, BlockRegistry, BlockStateId, ChestBlockEntity,
    FurnaceBlockEntity, ResidentBlockEdit, ResidentBlockEditBatchResult, ResidentBlockPrecondition,
    ScheduledBlockTick, WorldError, WorldMutationView, WorldStorage,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::sync::oneshot;
use tracing::{trace, warn};

#[cfg(test)]
mod block_drop_tests;
#[cfg(test)]
mod player_teleport_tests;
mod queue;
mod regional_mutation;

#[allow(unused_imports)]
pub(crate) use queue::SIMULATION_COMMAND_QUEUE_CAPACITY;
#[cfg(test)]
pub(crate) use queue::simulation_channel;
#[cfg(test)]
pub(super) use queue::simulation_channel_with_capacity;
pub(crate) use queue::{SIMULATION_COMMAND_BATCH_LIMIT, simulation_channel_with_explosion_seed};
use queue::{SimulationCommandEnvelope, SimulationQueueMetrics};
#[cfg(test)]
pub(in crate::play) use regional_mutation::RegionalBlockEditProbe;

pub(crate) type SimulationQueueSnapshot = queue::SimulationQueueSnapshot;

const MAX_SURVIVAL_BREAK_EDITS: usize = 512;
const MAX_SURVIVAL_BREAK_DROPS: usize = 512;
const MAX_BLOCK_EDIT_COMMAND_EDITS: usize = 512;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockDropAwaitStage {
    AfterReservation,
    AfterAppend,
}

#[cfg(test)]
struct BlockDropAwaitProbe {
    stage: BlockDropAwaitStage,
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(test)]
static BLOCK_DROP_AWAIT_PROBE: std::sync::Mutex<Option<Arc<BlockDropAwaitProbe>>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
static BLOCK_DROP_AWAIT_TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
fn install_block_drop_await_probe(
    stage: BlockDropAwaitStage,
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    let mut slot = BLOCK_DROP_AWAIT_PROBE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert!(slot.is_none(), "block-drop await probe already installed");
    *slot = Some(Arc::new(BlockDropAwaitProbe {
        stage,
        entered,
        release: std::sync::Mutex::new(release),
    }));
}

#[cfg(test)]
async fn pause_block_drop_after(stage: BlockDropAwaitStage) {
    let probe = BLOCK_DROP_AWAIT_PROBE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .filter(|probe| probe.stage == stage)
        .cloned();
    let Some(probe) = probe else {
        return;
    };
    probe.entered.send(()).expect("block-drop probe receiver");
    let waiter = Arc::clone(&probe);
    tokio::task::spawn_blocking(move || {
        waiter
            .release
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv()
            .expect("block-drop probe release");
    })
    .await
    .expect("block-drop probe worker");
    let mut slot = BLOCK_DROP_AWAIT_PROBE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if slot
        .as_ref()
        .is_some_and(|installed| Arc::ptr_eq(installed, &probe))
    {
        slot.take();
    }
}

fn elapsed_us(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SimulationRequestError {
    Full,
    Closed,
    OwnerStopped,
    ResponseMismatch,
    ShuttingDown,
    #[cfg(test)]
    WorldBusy,
    WorldUnavailable,
    WorldMutationFailed,
    CrossRegion,
    InvalidCommand,
    StaleSession,
}

#[derive(Debug)]
pub(super) struct SimulationAuthority(());

#[cfg(test)]
impl SimulationAuthority {
    pub(super) fn for_test() -> Self {
        Self(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PlayerStateEvent {
    SelectedHotbarSlot(u8),
    RespawnPose(PlayerPose),
    GameMode(GameMode),
}

pub(crate) struct SimulationSaveSnapshot {
    pub(crate) players: Vec<(
        uuid::Uuid,
        super::persistence::PlayerPersistedState,
        Option<u64>,
    )>,
    pub(crate) entities: PersistedEntityCheckpoint,
    pub(crate) entity_journal_phases: Vec<mc_entity::RegionPhase>,
    pub(crate) world_chunk_journal_watermark: Option<u64>,
    pub(crate) world_time: u64,
    pub(crate) players_sleeping_percentage: u32,
    pub(crate) simulation_tick: u64,
    pub(crate) world_flush_plan: Option<mc_world::DirtyFlushPlan>,
}

impl std::fmt::Debug for SimulationSaveSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SimulationSaveSnapshot")
            .field("players", &self.players.len())
            .field("entities", &self.entities.records.len())
            .field("world_time", &self.world_time)
            .field(
                "players_sleeping_percentage",
                &self.players_sleeping_percentage,
            )
            .field("simulation_tick", &self.simulation_tick)
            .field(
                "world_flush_chunks",
                &self
                    .world_flush_plan
                    .as_ref()
                    .map(mc_world::DirtyFlushPlan::chunk_count),
            )
            .finish()
    }
}

#[derive(Debug)]
pub(super) enum SimulationCommand {
    SaveBarrier {
        capture_world: bool,
    },
    ReadBlockSnapshot {
        position: BlockPos,
    },
    ReadChestSnapshot {
        positions: Vec<BlockPos>,
    },
    ReadFurnaceSnapshot {
        position: BlockPos,
    },
    PickupItemIntoInventory {
        entity_id: EntityId,
        collector_session: SessionId,
        expected_item_id: u32,
        expected_damage: Option<i32>,
        expected_enchantments: Vec<mc_data::ItemEnchantment>,
        max_stack: i32,
    },
    PickupExperienceIntoPlayer {
        entity_id: EntityId,
        collector_session: SessionId,
    },
    #[cfg(test)]
    ClaimExperiencePickup {
        entity_id: EntityId,
        collector_session: SessionId,
    },
    PickupArrowIntoInventory {
        entity_id: EntityId,
        collector_session: SessionId,
        arrow_item_id: u32,
        max_stack: i32,
    },
    PlayerAttackServerEntity {
        attacker_session: SessionId,
        entity_id: EntityId,
        damage: f32,
        attacker_costs: Option<Box<PlayerSurvivalPlan>>,
        cooldown_tick: u64,
    },
    ApplyServerEntityEffect(Box<ServerEntityEffectCommand>),
    #[cfg(test)]
    AttackServerEntity {
        entity_id: EntityId,
        damage: f32,
        knockback_origin: Option<Vec3>,
        rewards: EntityKillRewards,
    },
    SpawnCommandEntity {
        entity_type_id: i32,
        entity_type_name: String,
        position: Vec3,
    },
    SetWorldTime {
        world_time: u64,
    },
    EnsureChunkHerd {
        chunk: (i32, i32),
        spawns: Vec<HerdSpawn>,
    },
    ApplyBlockEdits {
        actor_session: SessionId,
        edits: Vec<BlockEdit>,
        preconditions: Vec<BlockEditPrecondition>,
        scheduled_block_ticks: Vec<ScheduledBlockTick>,
    },
    CommitBlockDrops {
        actor_session: SessionId,
        edits: Vec<BlockEdit>,
        preconditions: Vec<BlockEditPrecondition>,
        drops: Vec<SurvivalBreakDrop>,
    },
    ScheduleFluidTicksNearApplied {
        applied: Vec<AppliedBlockEdit>,
        block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
        world_tick: u64,
    },
    CommitSurvivalBreak(Box<SurvivalBreakCommand>),
    CommitSurvivalPlacement(Box<SurvivalPlacementCommand>),
    CommitBucketUse(Box<BucketUseCommand>),
    CommitFoodUse(FoodUseCommand),
    CommitAnimalFeed(AnimalFeedCommand),
    CommitSheepShear(SheepShearCommand),
    CommitPlayerSurvival(Box<PlayerSurvivalCommand>),
    CommitPlayerPose {
        actor_session: SessionId,
        pose: super::PlayerPose,
        exhaustion: f32,
        script_teleport_completion: Option<ScriptPlayerTeleportCompletion>,
    },
    CommitPlayerStateEvent {
        actor_session: SessionId,
        event: PlayerStateEvent,
    },
    CommitPlayerInventory {
        actor_session: SessionId,
        player: Box<ContainerPlayerPlan>,
    },
    CommitBowRelease(BowReleaseCommand),
    CommitSelectedItemDrop(SelectedItemDropCommand),
    CommitChest {
        primary_position: BlockPos,
        positions: Vec<BlockPos>,
        expected_state_id: i32,
        actor_session: SessionId,
        expected: Vec<ChestBlockEntity>,
        updated: Vec<ChestBlockEntity>,
        player: Box<ContainerPlayerPlan>,
    },
    CommitFurnace {
        position: BlockPos,
        expected_state_id: i32,
        actor_session: SessionId,
        expected: FurnaceBlockEntity,
        updated: FurnaceBlockEntity,
        player: Box<ContainerPlayerPlan>,
    },
    CommitOpaqueBlockEntity {
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    },
    CommitCampfireUse(Box<CampfireUseCommand>),
    CommitTntIgnition {
        actor_session: SessionId,
        plan: TntIgnitionPlan,
    },
}

impl SimulationCommand {
    fn kind(&self) -> &'static str {
        match self {
            Self::SaveBarrier { .. } => "save_barrier",
            Self::ReadBlockSnapshot { .. } => "read_block_snapshot",
            Self::ReadChestSnapshot { .. } => "read_chest_snapshot",
            Self::ReadFurnaceSnapshot { .. } => "read_furnace_snapshot",
            Self::PickupItemIntoInventory { .. } => "pickup_item_into_inventory",
            Self::PickupExperienceIntoPlayer { .. } => "pickup_experience_into_player",
            #[cfg(test)]
            Self::ClaimExperiencePickup { .. } => "claim_experience_pickup",
            Self::PickupArrowIntoInventory { .. } => "pickup_arrow_into_inventory",
            Self::PlayerAttackServerEntity { .. } => "player_attack_server_entity",
            Self::ApplyServerEntityEffect(_) => "apply_server_entity_effect",
            #[cfg(test)]
            Self::AttackServerEntity { .. } => "attack_server_entity",
            Self::SpawnCommandEntity { .. } => "spawn_command_entity",
            Self::SetWorldTime { .. } => "set_world_time",
            Self::EnsureChunkHerd { .. } => "ensure_chunk_herd",
            Self::ApplyBlockEdits { .. } => "apply_block_edits",
            Self::CommitBlockDrops { .. } => "commit_block_drops",
            Self::ScheduleFluidTicksNearApplied { .. } => "schedule_fluid_ticks_near_applied",
            Self::CommitSurvivalBreak(_) => "commit_survival_break",
            Self::CommitSurvivalPlacement(_) => "commit_survival_placement",
            Self::CommitBucketUse(_) => "commit_bucket_use",
            Self::CommitFoodUse(_) => "commit_food_use",
            Self::CommitAnimalFeed(_) => "commit_animal_feed",
            Self::CommitSheepShear(_) => "commit_sheep_shear",
            Self::CommitPlayerSurvival(_) => "commit_player_survival",
            Self::CommitPlayerPose { .. } => "commit_player_pose",
            Self::CommitPlayerStateEvent { .. } => "commit_player_state_event",
            Self::CommitPlayerInventory { .. } => "commit_player_inventory",
            Self::CommitBowRelease(_) => "commit_bow_release",
            Self::CommitSelectedItemDrop(_) => "commit_selected_item_drop",
            Self::CommitChest { .. } => "commit_chest",
            Self::CommitFurnace { .. } => "commit_furnace",
            Self::CommitOpaqueBlockEntity { .. } => "commit_opaque_block_entity",
            Self::CommitCampfireUse(_) => "commit_campfire_use",
            Self::CommitTntIgnition { .. } => "commit_tnt_ignition",
        }
    }

    pub(in crate::play) fn complete_script_player_teleport(&mut self, outcome: &SimulationOutcome) {
        let Self::CommitPlayerPose {
            script_teleport_completion,
            ..
        } = self
        else {
            return;
        };
        let Some(completion) = script_teleport_completion.take() else {
            return;
        };
        let result = match outcome {
            Ok(SimulationResponse::PlayerPose(Ok(_))) => Ok(()),
            Ok(SimulationResponse::PlayerPose(Err(SimulationRequestError::StaleSession)))
            | Err(SimulationRequestError::StaleSession) => {
                Err(ScriptPlayerTeleportFailure::PlayerUnavailable)
            }
            _ => Err(ScriptPlayerTeleportFailure::RuntimeUnavailable),
        };
        completion.complete(result);
    }
}

#[derive(Debug)]
pub(super) enum SimulationResponse {
    SaveSnapshot(Result<Box<SimulationSaveSnapshot>, SimulationRequestError>),
    BlockSnapshot(Result<Option<BlockMutationSnapshot>, SimulationRequestError>),
    ChestSnapshot(Result<Box<ChestReadSnapshot>, SimulationRequestError>),
    FurnaceSnapshot(Result<Box<FurnaceReadSnapshot>, SimulationRequestError>),
    ItemPickupCredit(Option<Box<CreditedItemPickup>>),
    ExperiencePickupCredit(Option<Box<CreditedExperiencePickup>>),
    #[cfg(test)]
    ExperiencePickup,
    ArrowPickupCredit(Option<Box<CreditedArrowPickup>>),
    PlayerAttack(PlayerAttackResult),
    EntityEffect(EntityEffectResult),
    #[cfg(test)]
    EntityAttack(Option<Box<EntityAttackOutcome>>),
    EntitySpawn(Vec<VisibilityDispatch>),
    WorldTimeSet,
    BlockEdits(Result<Box<Option<BlockEditBatchOutcome>>, SimulationRequestError>),
    BlockDrops(Result<Box<Option<BlockEditBatchOutcome>>, SimulationRequestError>),
    FluidTicksScheduled,
    SurvivalBreak(Result<Option<Box<CommittedSurvivalBreak>>, SimulationRequestError>),
    SurvivalPlacement(Result<Option<Box<CommittedSurvivalPlacement>>, SimulationRequestError>),
    BucketUse(Result<Option<Box<CommittedBucketUse>>, SimulationRequestError>),
    FoodUse(Result<Option<Box<CommittedFoodUse>>, SimulationRequestError>),
    AnimalFeed(Result<Option<Box<CommittedAnimalFeed>>, SimulationRequestError>),
    SheepShear(Result<Option<Box<CommittedSheepShear>>, SimulationRequestError>),
    PlayerSurvival(Result<Option<Box<PlayerSurvivalCommitOutcome>>, SimulationRequestError>),
    PlayerPose(Result<CommittedPlayerPose, SimulationRequestError>),
    PlayerStateEvent(Result<(), SimulationRequestError>),
    PlayerInventory(Box<Result<PlayerInventoryCommitOutcome, SimulationRequestError>>),
    BowRelease(Result<Option<Box<CommittedBowRelease>>, SimulationRequestError>),
    SelectedItemDrop(Result<Option<Box<CommittedSelectedItemDrop>>, SimulationRequestError>),
    ChestCommit(Result<Box<ChestCommitOutcome>, SimulationRequestError>),
    FurnaceCommit(Result<Box<FurnaceCommitOutcome>, SimulationRequestError>),
    OpaqueBlockEntity(Result<bool, SimulationRequestError>),
    CampfireUse(Result<Option<Box<CommittedCampfireUse>>, SimulationRequestError>),
    TntIgnition(Result<Option<Box<CommittedTntIgnition>>, SimulationRequestError>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct CommittedPlayerPose {
    pub(super) food: i32,
    pub(super) saturation: f32,
    pub(super) exhaustion: f32,
    pub(super) resources_changed: bool,
}

impl CommittedPlayerPose {
    pub(super) fn apply_resources_to(self, survival: &mut SurvivalState) {
        survival.food = self.food;
        survival.saturation = self.saturation;
        survival.exhaustion = self.exhaustion;
    }
}

#[derive(Debug)]
struct PendingOwnerRelight {
    envelope: SimulationCommandEnvelope,
    response: SimulationResponse,
    actor_session: SessionId,
    sources: IncrementalLightSources,
}

fn response_block_edit_outcome_mut(
    response: &mut SimulationResponse,
) -> Option<&mut BlockEditBatchOutcome> {
    match response {
        SimulationResponse::BlockEdits(Ok(outcome)) => outcome.as_mut().as_mut(),
        SimulationResponse::SurvivalBreak(Ok(Some(committed))) => Some(&mut committed.block),
        SimulationResponse::SurvivalPlacement(Ok(Some(committed))) => Some(&mut committed.block),
        SimulationResponse::BucketUse(Ok(Some(committed))) => Some(&mut committed.block),
        SimulationResponse::TntIgnition(Ok(Some(committed))) => Some(&mut committed.block),
        _ => None,
    }
}

fn command_relight_actor_session(command: &SimulationCommand) -> Option<SessionId> {
    match command {
        SimulationCommand::ApplyBlockEdits { actor_session, .. } => Some(*actor_session),
        SimulationCommand::CommitSurvivalBreak(command) => Some(command.actor_session),
        SimulationCommand::CommitSurvivalPlacement(command) => Some(command.actor_session),
        SimulationCommand::CommitBucketUse(command) => Some(command.actor_session),
        SimulationCommand::CommitTntIgnition { actor_session, .. } => Some(*actor_session),
        _ => None,
    }
}

fn prepare_owner_relight(
    storage: &mut WorldStorage,
    table: &BlockLightTable,
    outcome: &mut BlockEditBatchOutcome,
    defer_compute: bool,
) -> Option<Vec<super::session::OutboundLightUpdate>> {
    if defer_compute {
        outcome.pending_light_sources =
            Some(capture_incremental_light_sources(storage, table, outcome));
        None
    } else {
        Some(collect_incremental_light_updates_for_applied_edits(
            storage, table, outcome,
        ))
    }
}

async fn finish_pending_owner_relight(
    sessions: &SessionRegistry,
    world: &WorldHandle,
    mutation: Option<&WorldMutationView>,
    table: &BlockLightTable,
    pending_relight: Option<PendingOwnerRelight>,
) {
    let Some(mut pending) = pending_relight else {
        return;
    };
    let updates = {
        let Some(outcome) = response_block_edit_outcome_mut(&mut pending.response) else {
            debug_assert!(false, "pending relight response lost its block outcome");
            pending.envelope.respond(Ok(pending.response));
            return;
        };
        #[cfg(test)]
        sessions.pause_before_server_relight_compute_for_test();
        compute_incremental_light_updates(&pending.sources, table, outcome)
    };

    let updates = if let Some(mutation) = mutation {
        let outcome = response_block_edit_outcome_mut(&mut pending.response)
            .expect("pending relight response keeps its block outcome");
        publish_computed_light_updates(mutation, table, outcome, &pending.sources, updates)
    } else {
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "publish simulation relight result",
            std::time::Instant::now(),
            world.lock().await,
        );
        let outcome = response_block_edit_outcome_mut(&mut pending.response)
            .expect("pending relight response keeps its block outcome");
        if incremental_light_sources_are_current(&storage, &pending.sources) {
            persist_baked_light_updates(&mut storage, &updates);
            updates
        } else {
            collect_full_light_updates_for_current_world(&mut storage, table, outcome)
        }
    };

    let outcome = response_block_edit_outcome_mut(&mut pending.response)
        .expect("pending relight response keeps its block outcome");
    let light_chunks = updates
        .iter()
        .map(|update| (update.pos.x, update.pos.z))
        .collect::<HashSet<_>>();
    sessions.invalidate_prepared_chunks(&light_chunks);
    if !updates.is_empty() {
        dispatch_visibility_commands(
            sessions
                .loaded_recipients_for_chunks(&light_chunks, Some(pending.actor_session))
                .into_iter()
                .map(|recipient| VisibilityDispatch {
                    recipient,
                    command: OutboundCommand::LightUpdates(updates.clone()),
                })
                .collect(),
        );
    }
    outcome.precomputed_light_updates = Some(updates);
    pending.envelope.respond(Ok(pending.response));
}

fn schedule_resident_fluid_ticks_near_applied(
    world_read: &mc_world::WorldReadView,
    mutation: &WorldMutationView,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    world_tick: u64,
    applied: &[AppliedBlockEdit],
) {
    let ticks = super::plan_fluid_ticks_near_applied(world_read, block_facts, world_tick, applied);
    mutation.schedule_fluid_ticks(&ticks);
}

fn command_requires_world(command: &SimulationCommand) -> bool {
    matches!(
        command,
        SimulationCommand::SaveBarrier {
            capture_world: true
        } | SimulationCommand::ReadBlockSnapshot { .. }
            | SimulationCommand::ReadChestSnapshot { .. }
            | SimulationCommand::ReadFurnaceSnapshot { .. }
            | SimulationCommand::ApplyBlockEdits { .. }
            | SimulationCommand::CommitBlockDrops { .. }
            | SimulationCommand::ScheduleFluidTicksNearApplied { .. }
            | SimulationCommand::CommitSurvivalBreak(_)
            | SimulationCommand::CommitSurvivalPlacement(_)
            | SimulationCommand::CommitBucketUse(_)
            | SimulationCommand::CommitChest { .. }
            | SimulationCommand::CommitFurnace { .. }
            | SimulationCommand::CommitOpaqueBlockEntity { .. }
            | SimulationCommand::CommitCampfireUse(_)
            | SimulationCommand::CommitTntIgnition { .. }
    )
}

fn command_is_background(command: &SimulationCommand) -> bool {
    matches!(command, SimulationCommand::EnsureChunkHerd { .. })
}

fn command_orders_earlier_herds(command: &SimulationCommand) -> bool {
    matches!(command, SimulationCommand::SetWorldTime { .. })
}

fn command_single_owner_region(command: &SimulationCommand) -> Option<RegionKey> {
    if let SimulationCommand::ApplyBlockEdits {
        edits,
        preconditions,
        scheduled_block_ticks,
        ..
    } = command
    {
        let mut positions = edits
            .iter()
            .map(|edit| edit.pos)
            .chain(preconditions.iter().map(|precondition| precondition.pos))
            .chain(scheduled_block_ticks.iter().map(|tick| tick.pos));
        let first = positions.next()?;
        let owner = RegionKey::from_chunk(first.x.div_euclid(16), first.z.div_euclid(16));
        return positions
            .all(|pos| RegionKey::from_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16)) == owner)
            .then_some(owner);
    }
    if let SimulationCommand::CommitSurvivalPlacement(command) = command {
        let mut positions = command
            .plan
            .edits
            .iter()
            .map(|edit| edit.pos)
            .chain(
                command
                    .plan
                    .preconditions
                    .iter()
                    .map(|precondition| precondition.pos),
            )
            .chain(
                command
                    .plan
                    .scheduled_block_ticks
                    .iter()
                    .map(|tick| tick.pos),
            );
        let first = positions.next()?;
        let owner = RegionKey::from_chunk(first.x.div_euclid(16), first.z.div_euclid(16));
        return positions
            .all(|pos| RegionKey::from_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16)) == owner)
            .then_some(owner);
    }
    if let SimulationCommand::CommitSurvivalBreak(command) = command {
        let mut positions: Box<dyn Iterator<Item = BlockPos> + '_> = match &command.request {
            SurvivalBreakRequest::Prepared(plan) => Box::new(
                plan.edits.iter().map(|edit| edit.pos).chain(
                    plan.preconditions
                        .iter()
                        .map(|precondition| precondition.pos),
                ),
            ),
            SurvivalBreakRequest::Block(plan) => Box::new(std::iter::once(plan.position)),
        };
        let first = positions.next()?;
        let owner = RegionKey::from_chunk(first.x.div_euclid(16), first.z.div_euclid(16));
        return positions
            .all(|pos| RegionKey::from_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16)) == owner)
            .then_some(owner);
    }
    if let SimulationCommand::CommitBucketUse(command) = command {
        return Some(RegionKey::from_chunk(
            command.plan.edit.pos.x.div_euclid(16),
            command.plan.edit.pos.z.div_euclid(16),
        ));
    }
    if let SimulationCommand::CommitBlockDrops {
        edits,
        preconditions,
        drops,
        ..
    } = command
    {
        let mut positions = edits
            .iter()
            .map(|edit| edit.pos)
            .chain(preconditions.iter().map(|precondition| precondition.pos));
        let first = positions.next()?;
        let owner = RegionKey::from_chunk(first.x.div_euclid(16), first.z.div_euclid(16));
        return (positions.all(|pos| {
            RegionKey::from_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16)) == owner
        }) && drops
            .iter()
            .all(|drop| RegionKey::from_position(drop.position) == Some(owner)))
        .then_some(owner);
    }
    if let SimulationCommand::CommitChest { positions, .. } = command {
        let mut positions = positions.iter();
        let first = positions.next()?;
        let owner = RegionKey::from_chunk(first.x.div_euclid(16), first.z.div_euclid(16));
        return positions
            .all(|pos| RegionKey::from_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16)) == owner)
            .then_some(owner);
    }
    if let SimulationCommand::CommitFurnace { position, .. } = command {
        return Some(RegionKey::from_chunk(
            position.x.div_euclid(16),
            position.z.div_euclid(16),
        ));
    }
    if let SimulationCommand::CommitOpaqueBlockEntity { position, .. } = command {
        return Some(RegionKey::from_chunk(
            position.x.div_euclid(16),
            position.z.div_euclid(16),
        ));
    }
    if let SimulationCommand::CommitCampfireUse(command) = command {
        return Some(RegionKey::from_chunk(
            command.plan.position.x.div_euclid(16),
            command.plan.position.z.div_euclid(16),
        ));
    }

    let position = match command {
        SimulationCommand::SpawnCommandEntity { position, .. } => *position,
        SimulationCommand::EnsureChunkHerd { chunk, .. } => {
            return Some(RegionKey::from_chunk(chunk.0, chunk.1));
        }
        _ => return None,
    };
    RegionKey::from_position(position)
}

fn command_can_use_resident_mutation(
    command: &SimulationCommand,
    world_read: Option<&mc_world::WorldReadView>,
    block_light: Option<&BlockLightTable>,
    light_inert_only: bool,
) -> bool {
    if let SimulationCommand::ScheduleFluidTicksNearApplied { applied, .. } = command {
        return world_read.is_some()
            && !applied.is_empty()
            && applied.len() <= MAX_BLOCK_EDIT_COMMAND_EDITS;
    }
    let SimulationCommand::ApplyBlockEdits {
        edits,
        preconditions,
        scheduled_block_ticks,
        ..
    } = command
    else {
        return false;
    };
    let Some(world_read) = world_read else {
        return false;
    };
    if edits.is_empty()
        || command_single_owner_region(command).is_none()
        || !valid_block_edit_command(edits, preconditions, scheduled_block_ticks)
    {
        return false;
    }
    if edits
        .iter()
        .map(|edit| edit.pos)
        .chain(preconditions.iter().map(|precondition| precondition.pos))
        .chain(scheduled_block_ticks.iter().map(|tick| tick.pos))
        .any(|position| world_read.get_cached_block(position).is_none())
    {
        return false;
    }
    let mut seen = HashSet::with_capacity(edits.len());
    for edit in edits {
        if !seen.insert(edit.pos)
            || matches!(edit.pos.x.rem_euclid(8 * 16), 0 | 127)
            || matches!(edit.pos.z.rem_euclid(8 * 16), 0 | 127)
        {
            return false;
        }
        if light_inert_only && let Some(table) = block_light {
            let Some(precondition) = preconditions
                .iter()
                .find(|precondition| precondition.pos == edit.pos)
            else {
                return false;
            };
            if block_edit_changes_light(table, precondition.expected_state, edit.new_state) {
                return false;
            }
        }
    }
    true
}

fn command_can_use_resident_block_drop(
    command: &SimulationCommand,
    world_read: Option<&mc_world::WorldReadView>,
) -> bool {
    let SimulationCommand::CommitBlockDrops {
        edits,
        preconditions,
        drops,
        ..
    } = command
    else {
        return false;
    };
    let Some(world_read) = world_read else {
        return false;
    };
    valid_block_drop_command(edits, preconditions, drops)
        && command_single_owner_region(command).is_some()
        && edits
            .iter()
            .map(|edit| edit.pos)
            .chain(preconditions.iter().map(|precondition| precondition.pos))
            .all(|position| world_read.get_cached_block(position).is_some())
}

fn command_can_use_regional_mutation(
    command: &SimulationCommand,
    world_read: Option<&mc_world::WorldReadView>,
    block_light: Option<&BlockLightTable>,
) -> bool {
    if matches!(command, SimulationCommand::ApplyBlockEdits { .. }) {
        return command_can_use_resident_mutation(command, world_read, block_light, false);
    }
    let Some(world_read) = world_read else {
        return false;
    };
    if let SimulationCommand::CommitSurvivalBreak(break_command) = command {
        let valid = match &break_command.request {
            SurvivalBreakRequest::Prepared(plan) => valid_survival_break_plan(plan),
            SurvivalBreakRequest::Block(plan) => valid_survival_block_break_plan(plan),
        };
        let Some(region) = command_single_owner_region(command) else {
            return false;
        };
        let root = match &break_command.request {
            SurvivalBreakRequest::Prepared(plan) => plan.edits.first().map(|edit| edit.pos),
            SurvivalBreakRequest::Block(plan) => Some(plan.position),
        };
        return valid
            && root.is_some_and(|position| {
                !matches!(position.x.rem_euclid(REGION_SIZE_CHUNKS * 16), 0 | 127)
                    && !matches!(position.z.rem_euclid(REGION_SIZE_CHUNKS * 16), 0 | 127)
                    && world_read.get_cached_block(position).is_some()
                    && RegionKey::from_chunk(position.x.div_euclid(16), position.z.div_euclid(16))
                        == region
            });
    }
    if let SimulationCommand::CommitBucketUse(bucket) = command {
        let position = bucket.plan.edit.pos;
        return valid_bucket_use_plan(&bucket.plan)
            && world_read.get_cached_block(position).is_some()
            && !matches!(position.x.rem_euclid(REGION_SIZE_CHUNKS * 16), 0 | 127)
            && !matches!(position.z.rem_euclid(REGION_SIZE_CHUNKS * 16), 0 | 127);
    }
    if let SimulationCommand::CommitChest {
        primary_position,
        positions,
        expected,
        updated,
        player,
        ..
    } = command
    {
        let mut unique = HashSet::with_capacity(positions.len());
        return !positions.is_empty()
            && positions.len() <= 2
            && positions.first() == Some(primary_position)
            && positions.len() == expected.len()
            && positions.len() == updated.len()
            && positions.iter().all(|position| unique.insert(*position))
            && valid_container_player_plan(player)
            && command_single_owner_region(command).is_some()
            && positions
                .iter()
                .all(|position| world_read.get_cached_block(*position).is_some());
    }
    if let SimulationCommand::CommitFurnace {
        position,
        expected,
        updated,
        player,
        ..
    } = command
    {
        return valid_furnace_commit_command(expected, updated, player)
            && world_read.get_cached_block(*position).is_some();
    }
    if let SimulationCommand::CommitOpaqueBlockEntity { position, .. } = command {
        return world_read.get_cached_block(*position).is_some();
    }
    if let SimulationCommand::CommitCampfireUse(command) = command {
        return valid_campfire_use_plan(&command.plan)
            && world_read.get_cached_block(command.plan.position).is_some();
    }
    let SimulationCommand::CommitSurvivalPlacement(placement) = command else {
        return false;
    };
    if !valid_survival_placement_plan(&placement.plan)
        || command_single_owner_region(command).is_none()
    {
        return false;
    }
    placement
        .plan
        .edits
        .iter()
        .map(|edit| edit.pos)
        .chain(
            placement
                .plan
                .preconditions
                .iter()
                .map(|precondition| precondition.pos),
        )
        .chain(
            placement
                .plan
                .scheduled_block_ticks
                .iter()
                .map(|tick| tick.pos),
        )
        .all(|position| world_read.get_cached_block(position).is_some())
        && placement.plan.edits.iter().all(|edit| {
            !matches!(edit.pos.x.rem_euclid(8 * 16), 0 | 127)
                && !matches!(edit.pos.z.rem_euclid(8 * 16), 0 | 127)
        })
}

fn snapshot_region(
    world_read: &mc_world::WorldReadView,
    region: RegionKey,
) -> mc_world::WorldReadSnapshot {
    let start_x = region.x * REGION_SIZE_CHUNKS;
    let start_z = region.z * REGION_SIZE_CHUNKS;
    let chunks = (0..REGION_SIZE_CHUNKS)
        .flat_map(|offset_x| {
            (0..REGION_SIZE_CHUNKS).map(move |offset_z| mc_world::ChunkPos {
                x: start_x + offset_x,
                z: start_z + offset_z,
            })
        })
        .collect::<Vec<_>>();
    world_read.snapshot_chunks(&chunks)
}

pub(super) fn resident_block_edit_outcome(
    mutation: &WorldMutationView,
    block_light: Option<&BlockLightTable>,
    world_tick: u64,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> Option<BlockEditBatchOutcome> {
    let resident_edits = resident_block_edits(edits, preconditions, block_light);
    let resident_preconditions = resident_block_preconditions(preconditions);
    resident_block_edit_result_outcome(mutation.apply_block_edits_conditionally(
        &resident_edits,
        &resident_preconditions,
        scheduled_block_ticks,
        block_light,
        Some(world_tick.saturating_add(1)),
    ))
}

fn resident_block_edits(
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    block_light: Option<&BlockLightTable>,
) -> Vec<ResidentBlockEdit> {
    edits
        .iter()
        .map(|edit| ResidentBlockEdit {
            pos: edit.pos,
            new_state: edit.new_state,
            preserve_light: block_light.is_some_and(|table| {
                preconditions
                    .iter()
                    .find(|precondition| precondition.pos == edit.pos)
                    .is_some_and(|precondition| {
                        !block_edit_changes_light(
                            table,
                            precondition.expected_state,
                            edit.new_state,
                        )
                    })
            }),
        })
        .collect()
}

fn resident_block_preconditions(
    preconditions: &[BlockEditPrecondition],
) -> Vec<ResidentBlockPrecondition> {
    preconditions
        .iter()
        .map(|precondition| ResidentBlockPrecondition {
            pos: precondition.pos,
            expected_state: precondition.expected_state,
            expected_token: precondition.expected_token,
        })
        .collect()
}

pub(super) fn resident_block_edit_result_outcome(
    result: ResidentBlockEditBatchResult,
) -> Option<BlockEditBatchOutcome> {
    let ResidentBlockEditBatchResult::Applied(applied) = result else {
        return None;
    };
    let mut outcome = BlockEditBatchOutcome::default();
    for edit in applied {
        let chunk = (edit.pos.x.div_euclid(16), edit.pos.z.div_euclid(16));
        let changes_light = edit.changes_light;
        if let Some(previous_light) = edit.previous_light {
            outcome
                .previous_light_chunks
                .entry(chunk)
                .or_insert(previous_light);
        }
        outcome.applied.push(AppliedBlockEdit {
            pos: edit.pos,
            previous: edit.previous,
            new_state: edit.new_state,
        });
        outcome
            .resulting_tokens
            .insert(edit.pos, edit.resulting_token);
        outcome.deltas.push(BlockDelta {
            x: edit.pos.x,
            y: edit.pos.y,
            z: edit.pos.z,
            state_id: edit.new_state,
        });
        outcome.edit_chunks.insert(chunk);
        if changes_light {
            outcome.light_edit_chunks.insert(chunk);
        }
    }
    Some(outcome)
}

fn regional_light_updates(
    world_read: &mc_world::WorldReadView,
    block_light: Option<&BlockLightTable>,
    outcome: Option<&BlockEditBatchOutcome>,
) -> (
    Option<IncrementalLightSources>,
    Vec<super::session::OutboundLightUpdate>,
) {
    let (Some(table), Some(outcome)) = (
        block_light,
        outcome.filter(|outcome| !outcome.light_edit_chunks.is_empty()),
    ) else {
        return (None, Vec::new());
    };
    let sources = capture_incremental_light_sources_from_read_view(world_read, table, outcome);
    let updates = compute_incremental_light_updates(&sources, table, outcome);
    (Some(sources), updates)
}

fn applied_edits_need_fluid_ticks(
    world_read: &mc_world::WorldReadView,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    applied: &[AppliedBlockEdit],
) -> bool {
    applied.iter().any(|edit| {
        std::iter::once(edit.pos)
            .chain(super::fluid_neighbour_positions(edit.pos))
            .any(|position| {
                world_read
                    .get_cached_block(position)
                    .is_some_and(|state| block_facts.fluid(state.0).is_some())
            })
    })
}

fn publish_regional_light_updates(
    sessions: &SessionRegistry,
    mutation: &WorldMutationView,
    block_light: Option<&Arc<BlockLightTable>>,
    sources: Option<&IncrementalLightSources>,
    light_updates: Vec<super::session::OutboundLightUpdate>,
    outcome: &mut BlockEditBatchOutcome,
) {
    let (Some(sources), Some(table)) = (sources, block_light) else {
        return;
    };
    let light_updates =
        publish_computed_light_updates(mutation, table, outcome, sources, light_updates);
    let light_chunks = light_updates
        .iter()
        .map(|update| (update.pos.x, update.pos.z))
        .collect::<HashSet<_>>();
    sessions.invalidate_prepared_chunks(&light_chunks);
    outcome.precomputed_light_updates = Some(light_updates);
}

fn publish_computed_light_updates(
    mutation: &WorldMutationView,
    table: &BlockLightTable,
    outcome: &BlockEditBatchOutcome,
    sources: &IncrementalLightSources,
    light_updates: Vec<super::session::OutboundLightUpdate>,
) -> Vec<super::session::OutboundLightUpdate> {
    if mutation.publish_baked_light_conditionally(
        &sources.chunks,
        light_updates
            .iter()
            .map(|update| (update.pos, &update.light)),
    ) {
        return light_updates;
    }

    mutation.recompute_and_publish_baked_light(
        sources.chunks.keys().copied(),
        |chunks| {
            let current = IncrementalLightSources {
                chunks: chunks.clone(),
            };
            compute_incremental_light_updates(&current, table, outcome)
        },
        |update| (update.pos, &update.light),
    )
}

fn dispatch_regional_block_outcome(
    sessions: &SessionRegistry,
    actor_session: SessionId,
    outcome: &BlockEditBatchOutcome,
) {
    sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
    let mut dispatches = sessions
        .loaded_recipients_for_chunks(&outcome.edit_chunks, Some(actor_session))
        .into_iter()
        .map(|recipient| VisibilityDispatch {
            recipient,
            command: OutboundCommand::BlockDeltas(outcome.deltas.clone()),
        })
        .collect::<Vec<_>>();
    if let Some(updates) = outcome.precomputed_light_updates.as_ref()
        && !updates.is_empty()
    {
        let light_chunks = updates
            .iter()
            .map(|update| (update.pos.x, update.pos.z))
            .collect::<HashSet<_>>();
        dispatches.extend(
            sessions
                .loaded_recipients_for_chunks(&light_chunks, Some(actor_session))
                .into_iter()
                .map(|recipient| VisibilityDispatch {
                    recipient,
                    command: OutboundCommand::LightUpdates(updates.clone()),
                }),
        );
    }
    dispatch_visibility_commands(dispatches);
}

#[derive(Debug)]
enum WorldContainerCommitError {
    MissingChunk(BlockPos),
    Storage(WorldError),
}

struct ChestCommitRequest<'a> {
    primary_position: BlockPos,
    positions: &'a [BlockPos],
    expected_state_id: i32,
    actor_session: SessionId,
    expected: &'a [ChestBlockEntity],
    updated: &'a [ChestBlockEntity],
    player: &'a ContainerPlayerPlan,
}

struct FurnaceCommitRequest<'a> {
    position: BlockPos,
    expected_state_id: i32,
    actor_session: SessionId,
    expected: &'a FurnaceBlockEntity,
    updated: &'a FurnaceBlockEntity,
    player: &'a ContainerPlayerPlan,
}

#[derive(Debug, Clone)]
pub(super) struct CampfireUsePlan {
    pub(super) position: BlockPos,
    pub(super) expected_state: BlockStateId,
    pub(super) expected_token: BlockMutationToken,
    pub(super) expected_cooking: CampfireCookingState,
    pub(super) updated_cooking: CampfireCookingState,
    pub(super) persistent_bytes: Vec<u8>,
    pub(super) client_nbt: mc_nbt::Tag,
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
}

#[derive(Debug, Clone)]
pub(super) struct CampfireUseCommand {
    actor_session: SessionId,
    plan: CampfireUsePlan,
}

#[derive(Debug)]
pub(super) struct CommittedCampfireUse {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SurvivalBreakHeldItem {
    pub(super) hotbar_slot: u8,
    pub(super) expected: ItemStack,
    pub(super) max_damage: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SurvivalBreakDrop {
    pub(super) entity_type_id: i32,
    pub(super) position: Vec3,
    pub(super) stack: EntityItemStack,
}

#[derive(Clone)]
pub(super) struct SurvivalBlockBreakPlan {
    pub(super) position: BlockPos,
    pub(super) expected_target: BlockMutationSnapshot,
    pub(super) blocks: Arc<mc_world::BlockRegistry>,
    pub(super) block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    pub(super) water: Option<BlockStateId>,
    pub(super) items: Arc<mc_data::items::ItemRegistry>,
    pub(super) item_facts: Arc<mc_data::item_components::ItemFactsTable>,
    pub(super) loot: Arc<mc_data::loot::LootTables>,
    pub(super) item_entity_type_id: Option<i32>,
    pub(super) falling_block_entity_type_id: Option<i32>,
    pub(super) held: SurvivalBreakHeldItem,
    pub(super) drop_items: bool,
}

impl std::fmt::Debug for SurvivalBlockBreakPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SurvivalBlockBreakPlan")
            .field("position", &self.position)
            .field("expected_target", &self.expected_target)
            .field("water", &self.water)
            .field("item_entity_type_id", &self.item_entity_type_id)
            .field(
                "falling_block_entity_type_id",
                &self.falling_block_entity_type_id,
            )
            .field("held", &self.held)
            .field("drop_items", &self.drop_items)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub(super) struct SurvivalBreakPlan {
    pub(super) edits: Vec<BlockEdit>,
    pub(super) preconditions: Vec<BlockEditPrecondition>,
    pub(super) blocks: Arc<mc_world::BlockRegistry>,
    pub(super) block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    pub(super) falling_block_entity_type_id: Option<i32>,
    pub(super) held: SurvivalBreakHeldItem,
    pub(super) drops: Vec<SurvivalBreakDrop>,
}

impl std::fmt::Debug for SurvivalBreakPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SurvivalBreakPlan")
            .field("edits", &self.edits)
            .field("preconditions", &self.preconditions)
            .field(
                "falling_block_entity_type_id",
                &self.falling_block_entity_type_id,
            )
            .field("held", &self.held)
            .field("drops", &self.drops)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SurvivalBreakCommand {
    actor_session: SessionId,
    request: SurvivalBreakRequest,
}

#[derive(Debug, Clone)]
enum SurvivalBreakRequest {
    Prepared(SurvivalBreakPlan),
    Block(SurvivalBlockBreakPlan),
}

#[derive(Debug)]
pub(super) struct CommittedSurvivalBreak {
    pub(super) block: BlockEditBatchOutcome,
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

fn append_block_edit_outcome(
    target: &mut BlockEditBatchOutcome,
    mut additional: BlockEditBatchOutcome,
) {
    target.applied.append(&mut additional.applied);
    target.deltas.append(&mut additional.deltas);
    target.edit_chunks.extend(additional.edit_chunks);
    target
        .light_edit_chunks
        .extend(additional.light_edit_chunks);
    for (chunk, light) in additional.previous_light_chunks {
        target.previous_light_chunks.entry(chunk).or_insert(light);
    }
    target
        .cleared_campfires
        .append(&mut additional.cleared_campfires);
    if let Some(mut updates) = additional.precomputed_light_updates.take() {
        target
            .precomputed_light_updates
            .get_or_insert_default()
            .append(&mut updates);
    }
}

fn explosion_collision_boxes(
    storage: &mut WorldStorage,
    materials: &BlockMaterialIds,
    position: BlockPos,
) -> Option<Vec<[f64; 6]>> {
    let state = storage.get_block(position).ok().flatten()?;
    if !materials.classify(state.0).is_solid() {
        return Some(Vec::new());
    }
    if let Some(boxes) = mc_data::collision_shapes::vanilla_collision_shapes().get(state.0) {
        return Some(
            boxes
                .iter()
                .map(|collision_box| collision_box.as_blocks())
                .collect(),
        );
    }
    let height = materials.collision_height(state.0)?.as_blocks();
    Some(vec![[0.0, 0.0, 0.0, 1.0, height, 1.0]])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SurvivalPlacementHeldItem {
    pub(super) inventory_slot: usize,
    pub(super) expected: ItemStack,
}

#[derive(Clone)]
pub(super) struct SurvivalPlacementPlan {
    pub(super) edits: Vec<BlockEdit>,
    pub(super) preconditions: Vec<BlockEditPrecondition>,
    pub(super) scheduled_block_ticks: Vec<ScheduledBlockTick>,
    pub(super) block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    pub(super) held: SurvivalPlacementHeldItem,
    pub(super) expected_game_mode: GameMode,
}

pub(super) fn placement_inventory_debit(
    authoritative: GameMode,
    expected: GameMode,
) -> Option<bool> {
    if authoritative != expected {
        return None;
    }
    match authoritative {
        GameMode::Survival => Some(true),
        GameMode::Creative => Some(false),
        GameMode::Adventure | GameMode::Spectator => None,
    }
}

impl std::fmt::Debug for SurvivalPlacementPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SurvivalPlacementPlan")
            .field("edits", &self.edits)
            .field("preconditions", &self.preconditions)
            .field("scheduled_block_ticks", &self.scheduled_block_ticks)
            .field("held", &self.held)
            .field("expected_game_mode", &self.expected_game_mode)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SurvivalPlacementCommand {
    actor_session: SessionId,
    plan: SurvivalPlacementPlan,
}

#[derive(Debug)]
pub(super) struct CommittedSurvivalPlacement {
    pub(super) block: BlockEditBatchOutcome,
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BucketInventoryChange {
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) replacement_item: u32,
    pub(super) replacement_max_stack: i32,
}

#[derive(Clone)]
pub(super) struct BucketUsePlan {
    pub(super) edit: BlockEdit,
    pub(super) precondition: BlockEditPrecondition,
    pub(super) block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    pub(super) inventory: Option<BucketInventoryChange>,
    pub(super) schedule_fluid_ticks: bool,
}

impl std::fmt::Debug for BucketUsePlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BucketUsePlan")
            .field("edit", &self.edit)
            .field("precondition", &self.precondition)
            .field("inventory", &self.inventory)
            .field("schedule_fluid_ticks", &self.schedule_fluid_ticks)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub(super) struct BucketUseCommand {
    actor_session: SessionId,
    plan: BucketUsePlan,
}

#[derive(Debug)]
pub(super) struct CommittedBucketUse {
    pub(super) block: BlockEditBatchOutcome,
    pub(super) inventory: Option<PlayerInventory>,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
}

#[derive(Debug, Clone)]
pub(super) struct FoodUsePlan {
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) expected_survival: SurvivalState,
    pub(super) food: i32,
    pub(super) saturation: f32,
}

#[derive(Debug)]
pub(super) struct FoodUseCommand {
    actor_session: SessionId,
    plan: FoodUsePlan,
}

#[derive(Debug)]
pub(super) struct CommittedFoodUse {
    pub(super) inventory: PlayerInventory,
    pub(super) survival: SurvivalState,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct AnimalFeedTargets {
    pub(super) cow: bool,
    pub(super) sheep: bool,
    pub(super) chicken: bool,
}

impl AnimalFeedTargets {
    pub(super) fn is_empty(self) -> bool {
        !self.cow && !self.sheep && !self.chicken
    }

    pub(super) fn accepts(self, entity_type: &str) -> bool {
        match entity_type {
            "minecraft:cow" => self.cow,
            "minecraft:sheep" => self.sheep,
            "minecraft:chicken" => self.chicken,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AnimalFeedPlan {
    pub(super) entity_id: EntityId,
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) food_item_id: u32,
    pub(super) targets: AnimalFeedTargets,
}

#[derive(Debug)]
pub(super) struct AnimalFeedCommand {
    actor_session: SessionId,
    plan: AnimalFeedPlan,
}

#[derive(Debug)]
pub(super) struct CommittedAnimalFeed {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug, Clone)]
pub(super) struct SheepShearPlan {
    pub(super) entity_id: EntityId,
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) shears_item_id: u32,
    pub(super) shears_max_damage: i32,
    pub(super) item_entity_type_id: i32,
    pub(super) wool_item_ids: [u32; 16],
}

#[derive(Debug)]
pub(super) struct SheepShearCommand {
    actor_session: SessionId,
    plan: SheepShearPlan,
}

#[derive(Debug)]
pub(super) struct CommittedSheepShear {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    #[cfg(test)]
    pub(super) drop_count: usize,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug)]
pub(super) struct ChestReadSnapshot {
    pub(super) view: ChestView,
    pub(super) state_id: i32,
}

#[derive(Debug)]
pub(super) struct FurnaceReadSnapshot {
    pub(super) furnace: FurnaceBlockEntity,
    pub(super) state_id: i32,
}

#[derive(Debug, Clone)]
pub(super) struct PlayerSurvivalPlan {
    pub(super) expected_survival: SurvivalState,
    pub(super) updated_survival: SurvivalState,
    pub(super) expected_inventory: PlayerInventory,
    pub(super) updated_inventory: PlayerInventory,
    pub(super) expected_carried_item: ItemStack,
    pub(super) expected_xp: super::persistence::XpState,
    pub(super) updated_xp: super::persistence::XpState,
    pub(super) active_shield: Option<ActiveShieldTransition>,
    pub(super) enchanting_table_input: Option<super::EnchantingTableInputPlan>,
    pub(super) item_entity_type_id: Option<i32>,
    pub(super) xp_orb_entity_type_id: Option<i32>,
    pub(super) position: Vec3,
}

#[derive(Debug, Clone)]
pub(super) struct ActiveShieldTransition {
    pub(super) expected: Option<super::combat::ActiveShield>,
    pub(super) updated: Option<super::combat::ActiveShield>,
}

#[derive(Debug, Clone)]
pub(super) struct AuthoritativePlayerStateSnapshot {
    pub(super) inventory: PlayerInventory,
    pub(super) carried_item: ItemStack,
    pub(super) active_shield: Option<super::combat::ActiveShield>,
}

#[derive(Debug)]
pub(super) enum PlayerSurvivalCommitOutcome {
    Committed(CommittedPlayerSurvival),
    Rejected(AuthoritativePlayerStateSnapshot),
}

#[derive(Debug)]
pub(super) struct PlayerSurvivalCommand {
    actor_session: SessionId,
    plan: PlayerSurvivalPlan,
}

#[derive(Debug)]
pub(super) struct CommittedPlayerSurvival {
    pub(super) survival: SurvivalState,
    pub(super) inventory: PlayerInventory,
    pub(super) carried_item: ItemStack,
    pub(super) xp: super::persistence::XpState,
    pub(super) died: bool,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug, Clone)]
pub(super) struct BowReleasePlan {
    pub(super) bow_slot: usize,
    pub(super) expected_bow: ItemStack,
    pub(super) arrow_slot: usize,
    pub(super) expected_arrow: ItemStack,
    pub(super) bow_max_damage: i32,
    pub(super) entity_type_id: i32,
    pub(super) position: Vec3,
    pub(super) velocity: Vec3,
    pub(super) rotation: Rotation,
}

#[derive(Debug)]
pub(super) struct BowReleaseCommand {
    actor_session: SessionId,
    plan: BowReleasePlan,
}

#[derive(Debug)]
pub(super) struct CommittedBowRelease {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug, Clone)]
pub(super) struct SelectedItemDropPlan {
    pub(super) held_hotbar_slot: u8,
    pub(super) expected_held: ItemStack,
    pub(super) drop_count: i32,
    pub(super) entity_type_id: i32,
    pub(super) position: Vec3,
}

#[derive(Debug)]
pub(super) struct SelectedItemDropCommand {
    actor_session: SessionId,
    plan: SelectedItemDropPlan,
}

#[derive(Debug)]
pub(super) struct CommittedSelectedItemDrop {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

type SimulationOutcome = Result<SimulationResponse, SimulationRequestError>;

#[derive(Debug)]
pub(super) struct ServerEntityEffectCommand {
    entity_id: EntityId,
    expected: Option<Box<EntitySnapshot>>,
    operation: EntityEffectOperation,
    target_kind: TargetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityEffectRequestError {
    Busy,
    Unavailable,
    ShuttingDown,
    ResponseMismatch,
}

#[derive(Clone)]
pub struct EntityEffectHandle {
    simulation: SimulationHandle,
}

impl EntityEffectHandle {
    pub async fn apply(
        &self,
        entity_id: EntityId,
        operation: EntityEffectOperation,
        target_kind: TargetKind,
    ) -> Result<EntityEffectResult, EntityEffectRequestError> {
        self.simulation
            .apply_entity_effect(entity_id, None, operation, target_kind)
            .await
            .map_err(EntityEffectRequestError::from)
    }
}

impl From<SimulationRequestError> for EntityEffectRequestError {
    fn from(error: SimulationRequestError) -> Self {
        match error {
            SimulationRequestError::Full => Self::Busy,
            SimulationRequestError::ShuttingDown => Self::ShuttingDown,
            SimulationRequestError::ResponseMismatch => Self::ResponseMismatch,
            SimulationRequestError::Closed
            | SimulationRequestError::OwnerStopped
            | SimulationRequestError::WorldUnavailable
            | SimulationRequestError::WorldMutationFailed
            | SimulationRequestError::CrossRegion
            | SimulationRequestError::InvalidCommand
            | SimulationRequestError::StaleSession => Self::Unavailable,
            #[cfg(test)]
            SimulationRequestError::WorldBusy => Self::Busy,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SimulationHandle {
    sender: mpsc::Sender<SimulationCommandEnvelope>,
    metrics: Arc<SimulationQueueMetrics>,
    session_fence: Option<SessionId>,
}

impl SimulationHandle {
    pub(super) async fn apply_entity_effect(
        &self,
        entity_id: EntityId,
        expected: Option<EntitySnapshot>,
        operation: EntityEffectOperation,
        target_kind: TargetKind,
    ) -> Result<EntityEffectResult, SimulationRequestError> {
        let receiver = self.enqueue_with_fence(
            None,
            SimulationCommand::ApplyServerEntityEffect(Box::new(ServerEntityEffectCommand {
                entity_id,
                expected: expected.map(Box::new),
                operation,
                target_kind,
            })),
        )?;
        match receiver.await {
            Ok(Ok(SimulationResponse::EntityEffect(result))) => Ok(result),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(crate) fn entity_effect_handle(&self) -> EntityEffectHandle {
        EntityEffectHandle {
            simulation: self.clone(),
        }
    }

    pub(crate) async fn save_barrier(
        &self,
        capture_world: bool,
    ) -> Result<SimulationSaveSnapshot, SimulationRequestError> {
        let receiver =
            self.enqueue_with_fence(None, SimulationCommand::SaveBarrier { capture_world })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::SaveSnapshot(Ok(snapshot)))) => Ok(*snapshot),
            Ok(Ok(SimulationResponse::SaveSnapshot(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn read_block_snapshot(
        &self,
        position: BlockPos,
    ) -> Result<Option<BlockMutationSnapshot>, SimulationRequestError> {
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::ReadBlockSnapshot { position })
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::BlockSnapshot(result))) => result,
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    #[cfg(test)]
    pub(super) async fn player_attack_server_entity(
        &self,
        entity_id: EntityId,
        damage: f32,
    ) -> Result<PlayerAttackResult, SimulationRequestError> {
        self.player_attack_server_entity_inner(entity_id, damage, None, 0)
            .await
    }

    pub(super) async fn player_attack_server_entity_with_costs(
        &self,
        entity_id: EntityId,
        damage: f32,
        attacker_costs: PlayerSurvivalPlan,
        cooldown_tick: u64,
    ) -> Result<PlayerAttackResult, SimulationRequestError> {
        self.player_attack_server_entity_inner(
            entity_id,
            damage,
            Some(Box::new(attacker_costs)),
            cooldown_tick,
        )
        .await
    }

    async fn player_attack_server_entity_inner(
        &self,
        entity_id: EntityId,
        damage: f32,
        attacker_costs: Option<Box<PlayerSurvivalPlan>>,
        cooldown_tick: u64,
    ) -> Result<PlayerAttackResult, SimulationRequestError> {
        let attacker_session = self.session_id()?;
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::PlayerAttackServerEntity {
                attacker_session,
                entity_id,
                damage,
                attacker_costs,
                cooldown_tick,
            })
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::PlayerAttack(result))) => Ok(result),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn read_chest_snapshot(
        &self,
        positions: Vec<BlockPos>,
    ) -> Result<ChestReadSnapshot, SimulationRequestError> {
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::ReadChestSnapshot { positions })
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ChestSnapshot(result))) => result.map(|snapshot| *snapshot),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn read_furnace_snapshot(
        &self,
        position: BlockPos,
    ) -> Result<FurnaceReadSnapshot, SimulationRequestError> {
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::ReadFurnaceSnapshot { position })
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::FurnaceSnapshot(result))) => result.map(|snapshot| *snapshot),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn pickup_item_into_inventory(
        &self,
        entity_id: EntityId,
        expected_item_id: u32,
        expected_damage: Option<i32>,
        expected_enchantments: Vec<mc_data::ItemEnchantment>,
        max_stack: i32,
    ) -> Result<Option<CreditedItemPickup>, SimulationRequestError> {
        let collector_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::PickupItemIntoInventory {
            entity_id,
            collector_session,
            expected_item_id,
            expected_damage,
            expected_enchantments,
            max_stack,
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ItemPickupCredit(credited))) => {
                Ok(credited.map(|credited| *credited))
            }
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn pickup_experience_into_player(
        &self,
        entity_id: EntityId,
    ) -> Result<Option<CreditedExperiencePickup>, SimulationRequestError> {
        let collector_session = self.session_id()?;
        let receiver =
            self.enqueue_player_command(SimulationCommand::PickupExperienceIntoPlayer {
                entity_id,
                collector_session,
            })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ExperiencePickupCredit(credited))) => {
                Ok(credited.map(|credited| *credited))
            }
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn pickup_arrow_into_inventory(
        &self,
        entity_id: EntityId,
        arrow_item_id: u32,
        max_stack: i32,
    ) -> Result<Option<CreditedArrowPickup>, SimulationRequestError> {
        let collector_session = self.session_id()?;
        let receiver =
            self.enqueue_player_command(SimulationCommand::PickupArrowIntoInventory {
                entity_id,
                collector_session,
                arrow_item_id,
                max_stack,
            })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ArrowPickupCredit(credited))) => {
                Ok(credited.map(|credited| *credited))
            }
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn spawn_command_entity(
        &self,
        entity_type_id: i32,
        entity_type_name: String,
        position: Vec3,
    ) -> Result<Vec<VisibilityDispatch>, SimulationRequestError> {
        let receiver = self.enqueue_player_command(SimulationCommand::SpawnCommandEntity {
            entity_type_id,
            entity_type_name,
            position,
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::EntitySpawn(dispatches))) => Ok(dispatches),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn set_world_time(
        &self,
        world_time: u64,
    ) -> Result<(), SimulationRequestError> {
        let receiver =
            self.enqueue_player_command(SimulationCommand::SetWorldTime { world_time })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::WorldTimeSet)) => Ok(()),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(crate) async fn set_world_time_server_owned(
        &self,
        world_time: u64,
    ) -> Result<(), SimulationRequestError> {
        if self.session_fence.is_some() {
            return Err(SimulationRequestError::InvalidCommand);
        }
        let receiver =
            self.enqueue_with_fence(None, SimulationCommand::SetWorldTime { world_time })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::WorldTimeSet)) => Ok(()),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(crate) async fn spawn_script_entity(
        &self,
        actor_session: u64,
        entity_type_id: i32,
        entity_type_name: String,
        position: Vec3,
    ) -> Result<(), SimulationRequestError> {
        let dispatches = self
            .for_session(actor_session)
            .spawn_command_entity(entity_type_id, entity_type_name, position)
            .await?;
        dispatch_visibility_commands(dispatches);
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn apply_block_edits(
        &self,
        edits: Vec<BlockEdit>,
        preconditions: Vec<BlockEditPrecondition>,
    ) -> Result<Option<BlockEditBatchOutcome>, SimulationRequestError> {
        self.apply_block_edits_with_scheduled_ticks(edits, preconditions, Vec::new())
            .await
    }

    pub(super) async fn apply_block_edits_with_scheduled_ticks(
        &self,
        edits: Vec<BlockEdit>,
        preconditions: Vec<BlockEditPrecondition>,
        scheduled_block_ticks: Vec<ScheduledBlockTick>,
    ) -> Result<Option<BlockEditBatchOutcome>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::ApplyBlockEdits {
            actor_session,
            edits,
            preconditions,
            scheduled_block_ticks,
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::BlockEdits(Ok(outcome)))) => Ok(*outcome),
            Ok(Ok(SimulationResponse::BlockEdits(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_block_drops(
        &self,
        edits: Vec<BlockEdit>,
        preconditions: Vec<BlockEditPrecondition>,
        drops: Vec<SurvivalBreakDrop>,
    ) -> Result<Option<BlockEditBatchOutcome>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitBlockDrops {
            actor_session,
            edits,
            preconditions,
            drops,
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::BlockDrops(Ok(outcome)))) => Ok(*outcome),
            Ok(Ok(SimulationResponse::BlockDrops(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn schedule_fluid_ticks_near_applied(
        &self,
        applied: Vec<AppliedBlockEdit>,
        block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
        world_tick: u64,
    ) -> Result<(), SimulationRequestError> {
        if applied.is_empty() {
            return Ok(());
        }
        if applied.len() > MAX_BLOCK_EDIT_COMMAND_EDITS {
            return Err(SimulationRequestError::InvalidCommand);
        }
        self.enqueue_detached_wait(SimulationCommand::ScheduleFluidTicksNearApplied {
            applied,
            block_facts,
            world_tick,
        })
        .await
    }

    pub(super) async fn commit_survival_break(
        &self,
        plan: SurvivalBreakPlan,
    ) -> Result<Option<CommittedSurvivalBreak>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitSurvivalBreak(
            Box::new(SurvivalBreakCommand {
                actor_session,
                request: SurvivalBreakRequest::Prepared(plan),
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::SurvivalBreak(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::SurvivalBreak(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_tnt_ignition(
        &self,
        plan: TntIgnitionPlan,
    ) -> Result<Option<CommittedTntIgnition>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitTntIgnition {
            actor_session,
            plan,
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::TntIgnition(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::TntIgnition(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_survival_block_break(
        &self,
        plan: SurvivalBlockBreakPlan,
    ) -> Result<Option<CommittedSurvivalBreak>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitSurvivalBreak(
            Box::new(SurvivalBreakCommand {
                actor_session,
                request: SurvivalBreakRequest::Block(plan),
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::SurvivalBreak(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::SurvivalBreak(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_survival_placement(
        &self,
        plan: SurvivalPlacementPlan,
    ) -> Result<Option<CommittedSurvivalPlacement>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(
            Box::new(SurvivalPlacementCommand {
                actor_session,
                plan,
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::SurvivalPlacement(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::SurvivalPlacement(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_bucket_use(
        &self,
        plan: BucketUsePlan,
    ) -> Result<Option<CommittedBucketUse>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitBucketUse(
            Box::new(BucketUseCommand {
                actor_session,
                plan,
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::BucketUse(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::BucketUse(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_food_use(
        &self,
        plan: FoodUsePlan,
    ) -> Result<Option<CommittedFoodUse>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver =
            self.enqueue_player_command(SimulationCommand::CommitFoodUse(FoodUseCommand {
                actor_session,
                plan,
            }))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::FoodUse(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::FoodUse(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_animal_feed(
        &self,
        plan: AnimalFeedPlan,
    ) -> Result<Option<CommittedAnimalFeed>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver =
            self.enqueue_player_command(SimulationCommand::CommitAnimalFeed(AnimalFeedCommand {
                actor_session,
                plan,
            }))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::AnimalFeed(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::AnimalFeed(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_sheep_shear(
        &self,
        plan: SheepShearPlan,
    ) -> Result<Option<CommittedSheepShear>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver =
            self.enqueue_player_command(SimulationCommand::CommitSheepShear(SheepShearCommand {
                actor_session,
                plan,
            }))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::SheepShear(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::SheepShear(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_player_survival(
        &self,
        plan: PlayerSurvivalPlan,
    ) -> Result<Option<PlayerSurvivalCommitOutcome>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitPlayerSurvival(
            Box::new(PlayerSurvivalCommand {
                actor_session,
                plan,
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::PlayerSurvival(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::PlayerSurvival(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_player_pose(
        &self,
        pose: super::PlayerPose,
        exhaustion: f32,
    ) -> Result<CommittedPlayerPose, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::CommitPlayerPose {
                actor_session,
                pose,
                exhaustion,
                script_teleport_completion: None,
            })
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::PlayerPose(result))) => result,
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_script_player_teleport(
        &self,
        pose: super::PlayerPose,
        completion: ScriptPlayerTeleportCompletion,
    ) -> Result<CommittedPlayerPose, SimulationRequestError> {
        let receiver = self
            .enqueue_script_player_teleport_wait(pose, completion)
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::PlayerPose(result))) => result,
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_selected_hotbar_slot(
        &self,
        slot: u8,
    ) -> Result<(), SimulationRequestError> {
        self.commit_player_state_event(PlayerStateEvent::SelectedHotbarSlot(slot))
            .await
    }

    pub(super) async fn commit_respawn_pose(
        &self,
        pose: PlayerPose,
    ) -> Result<(), SimulationRequestError> {
        self.commit_player_state_event(PlayerStateEvent::RespawnPose(pose))
            .await
    }

    pub(super) async fn commit_game_mode(
        &self,
        game_mode: GameMode,
    ) -> Result<(), SimulationRequestError> {
        self.commit_player_state_event(PlayerStateEvent::GameMode(game_mode))
            .await
    }

    async fn commit_player_state_event(
        &self,
        event: PlayerStateEvent,
    ) -> Result<(), SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::CommitPlayerStateEvent {
                actor_session,
                event,
            })
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::PlayerStateEvent(result))) => result,
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_player_inventory(
        &self,
        player: ContainerPlayerPlan,
    ) -> Result<PlayerInventoryCommitOutcome, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::CommitPlayerInventory {
                actor_session,
                player: Box::new(player),
            })
            .await?;
        match receiver.await {
            Ok(Ok(SimulationResponse::PlayerInventory(result))) => *result,
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_bow_release(
        &self,
        plan: BowReleasePlan,
    ) -> Result<Option<CommittedBowRelease>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver =
            self.enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
                actor_session,
                plan,
            }))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::BowRelease(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::BowRelease(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_selected_item_drop(
        &self,
        plan: SelectedItemDropPlan,
    ) -> Result<Option<CommittedSelectedItemDrop>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
            SelectedItemDropCommand {
                actor_session,
                plan,
            },
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::SelectedItemDrop(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::SelectedItemDrop(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_chest(
        &self,
        primary_position: BlockPos,
        positions: Vec<BlockPos>,
        expected_state_id: i32,
        expected: Vec<ChestBlockEntity>,
        updated: Vec<ChestBlockEntity>,
        player: ContainerPlayerPlan,
    ) -> Result<ChestCommitOutcome, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitChest {
            primary_position,
            positions,
            expected_state_id,
            actor_session,
            expected,
            updated,
            player: Box::new(player),
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ChestCommit(Ok(outcome)))) => Ok(*outcome),
            Ok(Ok(SimulationResponse::ChestCommit(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_furnace(
        &self,
        position: BlockPos,
        expected_state_id: i32,
        expected: FurnaceBlockEntity,
        updated: FurnaceBlockEntity,
        player: ContainerPlayerPlan,
    ) -> Result<FurnaceCommitOutcome, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitFurnace {
            position,
            expected_state_id,
            actor_session,
            expected,
            updated,
            player: Box::new(player),
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::FurnaceCommit(Ok(outcome)))) => Ok(*outcome),
            Ok(Ok(SimulationResponse::FurnaceCommit(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_opaque_block_entity(
        &self,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    ) -> Result<bool, SimulationRequestError> {
        let receiver = self.enqueue_player_command(SimulationCommand::CommitOpaqueBlockEntity {
            position,
            expected_state,
            expected_token,
            bytes,
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::OpaqueBlockEntity(Ok(committed)))) => Ok(committed),
            Ok(Ok(SimulationResponse::OpaqueBlockEntity(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_campfire_use(
        &self,
        plan: CampfireUsePlan,
    ) -> Result<Option<CommittedCampfireUse>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitCampfireUse(
            Box::new(CampfireUseCommand {
                actor_session,
                plan,
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::CampfireUse(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::CampfireUse(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }
}

#[derive(Debug)]
pub(crate) struct SimulationOwner {
    receiver: mpsc::Receiver<SimulationCommandEnvelope>,
    prefetched: Option<SimulationCommandEnvelope>,
    deferred_background: VecDeque<SimulationCommandEnvelope>,
    metrics: Arc<SimulationQueueMetrics>,
    authority: SimulationAuthority,
    region_ownership: RegionOwnership,
    explosion_random: JavaLegacyRandom,
    #[cfg(test)]
    last_region_routes: Vec<RegionCommandRoute>,
    #[cfg(test)]
    regional_block_edit_probe: Option<RegionalBlockEditProbe>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
struct RegionCommandRoute {
    sequence: u64,
    lease: RegionLease,
}

struct PreparedRegionBatch {
    phase: RegionPhase,
    routes: HashMap<u64, RegionLease>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SimulationWorldAccess<'a> {
    pub(crate) read: Option<&'a mc_world::WorldReadView>,
    pub(crate) mutation: Option<&'a WorldMutationView>,
    pub(crate) cpu: Option<&'a crate::chunk_pipeline::ChunkPipelineResources>,
    pub(crate) light: Option<&'a Arc<BlockLightTable>>,
}

enum BatchWorldAccess<'a> {
    Unavailable(SimulationRequestError),
    Storage(&'a mut WorldStorage),
    ResidentBlock(BlockPos, BlockMutationSnapshot),
    ResidentMutation(&'a WorldMutationView, Option<&'a mc_world::WorldReadView>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SimulationCommandAttribution {
    pub(crate) kind: &'static str,
    pub(crate) post_admission_command_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SimulationLaneAttribution {
    pub(crate) cpu_admission_wait_us: u64,
    pub(crate) commands: Vec<SimulationCommandAttribution>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SimulationTickReport {
    pub(crate) processed: usize,
    pub(crate) remaining_depth: usize,
    pub(crate) lane_attribution: Vec<SimulationLaneAttribution>,
}

struct ResidentBlockDropRunResult {
    report: SimulationTickReport,
    fail_stopped: bool,
}

enum BlockDropJournalAppendError {
    Journal(crate::play::world_journal::WorldChunkJournalError),
    Worker(tokio::task::JoinError),
}

impl BlockDropJournalAppendError {
    fn outcome_unknown(&self) -> bool {
        match self {
            Self::Journal(error) => error.outcome_unknown(),
            Self::Worker(_) => true,
        }
    }
}

impl SimulationOwner {
    fn prepare_single_lane_region_routes(
        &mut self,
        batch: &[SimulationCommandEnvelope],
    ) -> Result<Option<PreparedRegionBatch>, RegionOwnershipError> {
        let route_keys = batch
            .iter()
            .filter_map(|envelope| {
                let key = command_single_owner_region(&envelope.command)?;
                Some((envelope.sequence, key))
            })
            .collect::<Vec<_>>();

        #[cfg(test)]
        self.last_region_routes.clear();
        if route_keys.is_empty() {
            return Ok(None);
        }

        let mut new_keys = route_keys.iter().map(|(_, key)| *key).collect::<Vec<_>>();
        new_keys.sort_unstable();
        new_keys.dedup();
        for key in new_keys {
            if self.region_ownership.lease(key).is_none() {
                self.region_ownership.assign(key, 0)?;
            }
        }

        let routes = route_keys
            .iter()
            .map(|(sequence, key)| {
                let lease = self
                    .region_ownership
                    .lease(*key)
                    .ok_or(RegionOwnershipError::UnknownRegion)?;
                Ok((*sequence, lease))
            })
            .collect::<Result<HashMap<_, _>, RegionOwnershipError>>()?;
        #[cfg(test)]
        {
            self.last_region_routes = route_keys
                .iter()
                .map(|(sequence, _)| RegionCommandRoute {
                    sequence: *sequence,
                    lease: routes[sequence],
                })
                .collect();
        }
        let phase = self.region_ownership.begin_phase()?;
        Ok(Some(PreparedRegionBatch { phase, routes }))
    }

    #[cfg(test)]
    fn last_region_routes(&self) -> &[RegionCommandRoute] {
        &self.last_region_routes
    }

    pub(crate) fn advance_world_time(&self, sessions: &SessionRegistry, ticks: u64) -> u64 {
        let (_, pending) = sessions.advance_world_time_owned(&self.authority, ticks);
        self.release_retryable_herd_requests(pending.retryable_chunks());
        dispatch_visibility_commands(pending.into_dispatches());
        dispatch_visibility_commands(
            sessions
                .item_pickup_ready_dispatches_owned(&self.authority, sessions.simulation_tick()),
        );
        dispatch_visibility_commands(sessions.tick_sleep_owned(&self.authority));
        sessions.world_time()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn tick_primed_tnt<F>(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        block_light: Option<&BlockLightTable>,
        block_facts: &BlockFactsTable,
        blocks: &BlockRegistry,
        materials: Option<&BlockMaterialIds>,
        claim_protection: F,
    ) -> usize
    where
        F: FnOnce() -> Option<crate::script::ClaimProtectionSnapshot>,
    {
        let current_tick = sessions.simulation_tick();
        let expired_tnt = sessions.claim_due_primed_tnt(&self.authority, current_tick);
        if expired_tnt.is_empty() {
            return 0;
        }
        let expired = expired_tnt.len();
        if !block_facts.has_explosion_table() {
            let dispatches = expired_tnt
                .into_iter()
                .flat_map(|tnt| sessions.plan_expired_tnt_dispatches(tnt, 0, &HashMap::new()))
                .collect();
            dispatch_visibility_commands(dispatches);
            return expired;
        }
        let claim_protection = claim_protection();

        let entity_targets = expired_tnt
            .iter()
            .map(|tnt| {
                let center = tnt.center();
                (
                    tnt.entity_id,
                    sessions.explosion_entity_targets(
                        &self.authority,
                        center,
                        f64::from(tnt.power()) * 2.0,
                    ),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut outcomes = HashMap::new();
        let mut candidate_counts = HashMap::new();
        let mut player_impacts: HashMap<EntityId, HashMap<SessionId, PlayerExplosionImpact>> =
            HashMap::new();
        let mut entity_impacts: HashMap<EntityId, Vec<ServerEntityExplosionImpact>> =
            HashMap::new();
        let mut chained_tnt = HashMap::<EntityId, Vec<_>>::new();
        let mut explosion_drops = HashMap::<EntityId, Vec<_>>::new();
        let explosion_items = mc_data::items::solaris_required_items();
        let explosion_item_entity_type_id = mc_data::entity_types::solaris_required_entity_types()
            .id_of(&mc_data::Identifier::parse("minecraft:item").expect("static item entity id"))
            .and_then(|id| i32::try_from(id).ok());
        let chained_tnt_entity_type_id = mc_data::entity_types::solaris_required_entity_types()
            .id_of(&mc_data::Identifier::parse(TNT_ENTITY_TYPE_NAME).expect("static TNT entity id"))
            .and_then(|id| i32::try_from(id).ok());
        if let Some(world) = world {
            let mut storage = world.lock().await;
            for expired in &expired_tnt {
                let entity_id = expired.entity_id;
                let air = expired.air;
                let center = expired.center();
                let power = expired.power();
                let Ok(candidates) = plan_explosion_candidates(
                    center,
                    power,
                    &mut self.explosion_random,
                    |position| {
                        let state = storage.get_block(position).ok().flatten()?;
                        let resistance = if state == air {
                            None
                        } else {
                            Some(block_facts.explosion_resistance(state.0)?)
                        };
                        Some(ExplosionBlockSample {
                            resistance,
                            explodable: claim_protection.as_ref().is_none_or(|protection| {
                                protection
                                    .ambient_block_mutation_allowed("minecraft:overworld", position)
                            }),
                        })
                    },
                ) else {
                    continue;
                };

                let block_count = i32::try_from(candidates.len()).unwrap_or(i32::MAX);
                candidate_counts.insert(entity_id, block_count);
                if let Some(materials) = materials {
                    let impacts = expired
                        .explosion_targets()
                        .iter()
                        .filter_map(|target| {
                            let feet = Vec3::new(target.pose.x, target.pose.y, target.pose.z);
                            plan_player_explosion_impact(center, power, feet, |position| {
                                explosion_collision_boxes(&mut storage, materials, position)
                            })
                            .map(|impact| (target.session_id, impact))
                        })
                        .collect();
                    player_impacts.insert(entity_id, impacts);
                    let impacts = entity_targets
                        .get(&entity_id)
                        .into_iter()
                        .flatten()
                        .filter_map(|target| {
                            plan_entity_explosion_impact(
                                center,
                                power,
                                target.position,
                                target.eye_position,
                                target.aabb_min,
                                target.aabb_max,
                                |position| {
                                    explosion_collision_boxes(&mut storage, materials, position)
                                },
                            )
                            .map(|impact: EntityExplosionImpact| {
                                ServerEntityExplosionImpact {
                                    entity_id: target.entity_id,
                                    damage: impact.damage,
                                    knockback: impact.knockback,
                                }
                            })
                        })
                        .collect();
                    entity_impacts.insert(entity_id, impacts);
                }
                let mut positions = candidates.into_iter().collect::<Vec<_>>();
                positions.sort_unstable_by_key(|position| (position.x, position.y, position.z));
                self.explosion_random.shuffle(&mut positions);
                let mut edits = Vec::new();
                let mut preconditions = Vec::new();
                for position in positions {
                    let Ok(Some(state)) = storage.get_block(position) else {
                        continue;
                    };
                    if state == air {
                        continue;
                    }
                    let Some(token) = storage.block_mutation_token(position) else {
                        continue;
                    };
                    edits.push(BlockEdit {
                        pos: position,
                        new_state: air,
                    });
                    preconditions.push(BlockEditPrecondition {
                        pos: position,
                        expected_state: state,
                        expected_token: token,
                    });
                }
                if edits.is_empty() {
                    continue;
                }
                if let Some(additional) = apply_block_edit_batch_to_storage_conditionally(
                    &mut storage,
                    block_light,
                    &edits,
                    &preconditions,
                ) {
                    for edit in &additional.applied {
                        let Some(state) = blocks.by_id(edit.previous) else {
                            continue;
                        };
                        if state.block.id.as_str() == TNT_ENTITY_TYPE_NAME {
                            let Some(entity_type_id) = chained_tnt_entity_type_id else {
                                continue;
                            };
                            let angle = self.explosion_random.next_double()
                                * f64::from(std::f32::consts::TAU);
                            let velocity = Vec3::new(
                                -angle.sin() * 0.02,
                                f64::from(0.2_f32),
                                -angle.cos() * 0.02,
                            );
                            let fuse_ticks = u64::from(self.explosion_random.next_int(20) + 10);
                            chained_tnt.entry(entity_id).or_default().push((
                                entity_type_id,
                                Vec3::new(
                                    f64::from(edit.pos.x) + 0.5,
                                    f64::from(edit.pos.y),
                                    f64::from(edit.pos.z) + 0.5,
                                ),
                                velocity,
                                fuse_ticks,
                                air,
                            ));
                            continue;
                        }

                        let Some(entity_type_id) = explosion_item_entity_type_id else {
                            continue;
                        };
                        let Some(drops) = mc_data::loot::builtin().block_explosion_drops(
                            &state.block.id,
                            &state.properties,
                            None,
                            || self.explosion_random.next_float(),
                        ) else {
                            continue;
                        };
                        for drop in drops {
                            let Some(item_id) = explosion_items.id_of(&drop.item) else {
                                continue;
                            };
                            let Ok(count) = drop.count.try_sample(0) else {
                                continue;
                            };
                            let Ok(count) = i32::try_from(count) else {
                                continue;
                            };
                            explosion_drops
                                .entry(entity_id)
                                .or_default()
                                .push(SurvivalBreakDrop {
                                    entity_type_id,
                                    position: Vec3::new(
                                        f64::from(edit.pos.x) + 0.5,
                                        f64::from(edit.pos.y) + 0.5,
                                        f64::from(edit.pos.z) + 0.5,
                                    ),
                                    stack: EntityItemStack::new(item_id, count),
                                });
                        }
                    }
                    outcomes.insert(entity_id, additional);
                }
            }
        }

        for tnt in expired_tnt {
            let entity_id = tnt.entity_id;
            if let Some(outcome) = outcomes.remove(&entity_id) {
                sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
                let block_dispatches = sessions
                    .ordered_loaded_recipients_for_chunks(&outcome.edit_chunks, None)
                    .into_iter()
                    .map(|recipient| VisibilityDispatch {
                        recipient,
                        command: OutboundCommand::BlockDeltas(outcome.deltas.clone()),
                    })
                    .collect();
                dispatch_visibility_commands(block_dispatches);
                if let (Some(world), Some(table)) = (world, block_light) {
                    let light_updates = super::collect_server_origin_light_updates(
                        world, sessions, table, &outcome,
                    )
                    .await;
                    if !light_updates.is_empty() {
                        let light_chunks = light_updates
                            .iter()
                            .map(|update| (update.pos.x, update.pos.z))
                            .collect();
                        let light_dispatches = sessions
                            .ordered_loaded_recipients_for_chunks(&light_chunks, None)
                            .into_iter()
                            .map(|recipient| VisibilityDispatch {
                                recipient,
                                command: OutboundCommand::LightUpdates(light_updates.clone()),
                            })
                            .collect();
                        dispatch_visibility_commands(light_dispatches);
                    }
                }
            }

            let mut drop_dispatches = Vec::new();
            for drop in explosion_drops.remove(&entity_id).unwrap_or_default() {
                drop_dispatches.extend(sessions.spawn_item_drop_owned(
                    &self.authority,
                    drop.entity_type_id,
                    drop.position,
                    drop.stack,
                ));
            }
            dispatch_visibility_commands(drop_dispatches);

            let block_count = candidate_counts.get(&entity_id).copied().unwrap_or(0);
            let impacts = player_impacts.remove(&entity_id).unwrap_or_default();
            let mut dispatches = sessions.plan_expired_tnt_dispatches(tnt, block_count, &impacts);
            let impacts = entity_impacts.remove(&entity_id).unwrap_or_default();
            dispatches.extend(sessions.apply_explosion_entity_impacts(&self.authority, &impacts));
            dispatch_visibility_commands(dispatches);

            for (entity_type_id, position, velocity, fuse_ticks, air) in
                chained_tnt.remove(&entity_id).unwrap_or_default()
            {
                dispatch_visibility_commands(sessions.spawn_chained_primed_tnt(
                    &self.authority,
                    entity_type_id,
                    position,
                    velocity,
                    fuse_ticks,
                    air,
                ));
            }
        }
        expired
    }

    pub(crate) fn tick_animal_breeding(
        &self,
        sessions: &SessionRegistry,
        elapsed_ticks: u16,
    ) -> usize {
        let (births, dispatches) = sessions.tick_animal_breeding(&self.authority, elapsed_ticks);
        dispatch_visibility_commands(dispatches);
        births
    }

    pub(crate) async fn run_sheep_grazing(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        world_read: Option<&mc_world::WorldReadView>,
        world_mutation: Option<&mc_world::WorldMutationView>,
        tick: u64,
    ) -> super::SheepGrazingReport {
        super::run_sheep_grazing_owned(
            &self.authority,
            config,
            sessions,
            world_read,
            world_mutation,
            tick,
        )
        .await
    }

    pub(crate) fn tick_hostile_attacks(
        &self,
        sessions: &SessionRegistry,
        tick: u64,
        air: BlockStateId,
    ) -> usize {
        let (attacks, dispatches) = sessions.tick_hostile_attacks(&self.authority, tick, air);
        dispatch_visibility_commands(dispatches);
        attacks
    }

    pub(crate) fn tick_dying_entities(&self, sessions: &SessionRegistry, tick: u64) {
        dispatch_visibility_commands(sessions.tick_dying_entities(&self.authority, tick));
    }

    pub(crate) async fn run_random_ticks_with_budget(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        access: SimulationWorldAccess<'_>,
        world_tick: u64,
        chunk_budget: usize,
    ) -> super::RandomTickReport {
        super::run_random_ticks_owned(
            &self.authority,
            config,
            sessions,
            access,
            #[cfg(test)]
            self.regional_block_edit_probe.clone(),
            world_tick,
            chunk_budget,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn run_scheduled_block_ticks_with_budget(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        access: SimulationWorldAccess<'_>,
        world_tick: u64,
        budget: usize,
    ) -> super::ScheduledBlockTickReport {
        super::run_scheduled_block_ticks_owned(
            config,
            sessions,
            access,
            #[cfg(test)]
            self.regional_block_edit_probe.clone(),
            world_tick,
            budget,
        )
        .await
    }

    pub(crate) async fn run_scheduled_fluid_ticks_with_budget(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        world_read: Option<&mc_world::WorldReadView>,
        world_mutation: Option<&mc_world::WorldMutationView>,
        world_tick: u64,
        budget: usize,
    ) -> super::ScheduledFluidTickReport {
        super::run_scheduled_fluid_ticks_owned(
            &self.authority,
            config,
            sessions,
            world_read,
            world_mutation,
            world_tick,
            budget,
        )
        .await
    }

    pub(crate) async fn run_campfire_cooking_ticks(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        world_read: Option<&mc_world::WorldReadView>,
        world_mutation: Option<&mc_world::WorldMutationView>,
    ) -> super::CampfireCookingTickReport {
        super::run_campfire_cooking_ticks_owned(self, config, sessions, world_read, world_mutation)
            .await
    }

    pub(crate) async fn run_furnace_ticks(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        world_read: Option<&mc_world::WorldReadView>,
        world_mutation: Option<&mc_world::WorldMutationView>,
    ) -> usize {
        super::run_furnace_ticks_owned(
            &self.authority,
            config,
            sessions,
            world_read,
            world_mutation,
        )
        .await
    }

    pub(crate) async fn land_falling_blocks(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        world_read: Option<&mc_world::WorldReadView>,
        candidates: &[LandedFallingBlock],
    ) -> usize {
        super::land_falling_blocks_owned(&self.authority, config, sessions, world_read, candidates)
            .await
    }

    pub(crate) fn collect_entity_physics_queries(
        &self,
        sessions: &SessionRegistry,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        pathing_candidates_per_entity: usize,
        simulation_distance: i32,
        pathing: Option<(&mc_world::WorldReadView, &mc_physics::BlockMaterialIds)>,
    ) -> Vec<EntityPhysicsQuery> {
        sessions.tick_entities_and_collect_physics_queries_owned(
            &self.authority,
            cpu_resources,
            tick,
            pathing_candidates_per_entity,
            simulation_distance,
            pathing,
        )
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics(
        &self,
        sessions: &SessionRegistry,
        tick: u64,
        steps: &[EntityPhysicsStep],
    ) {
        sessions.apply_entity_physics_and_dispatch_owned(&self.authority, tick, steps);
    }

    pub(crate) fn apply_entity_physics_if_current(
        &self,
        sessions: &SessionRegistry,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
        arrow_physics_facts: &[ArrowPhysicsFact],
    ) -> Vec<EntityPhysicsStep> {
        sessions.apply_entity_physics_if_current_and_dispatch_owned(
            &self.authority,
            cpu_resources,
            tick,
            expected,
            steps,
            arrow_physics_facts,
        )
    }

    pub(crate) fn restore_persisted_entities(
        &self,
        sessions: &SessionRegistry,
        checkpoint: PersistedEntityCheckpoint,
    ) -> usize {
        sessions.restore_persisted_entities_owned(&self.authority, checkpoint)
    }

    pub(crate) fn restore_world_time(&self, sessions: &SessionRegistry, world_time: u64) {
        sessions.restore_world_time_owned(&self.authority, world_time);
    }

    pub(super) fn materialize_pending_campfire_outputs(
        &self,
        sessions: &SessionRegistry,
        entity_type_id: i32,
        position: mc_world::BlockPos,
        outputs: &[PendingCampfireOutput],
    ) -> Vec<EntitySnapshot> {
        sessions.materialize_pending_campfire_outputs_owned(
            &self.authority,
            entity_type_id,
            position,
            outputs,
        )
    }

    pub(super) fn publish_materialized_campfire_outputs(
        &self,
        sessions: &SessionRegistry,
        snapshots: &[EntitySnapshot],
    ) -> Vec<VisibilityDispatch> {
        sessions.publish_materialized_campfire_outputs_owned(&self.authority, snapshots)
    }

    #[cfg(test)]
    pub(crate) fn process_tick(
        &mut self,
        sessions: &SessionRegistry,
        budget: usize,
    ) -> SimulationTickReport {
        let batch = self.drain_batch(budget);
        self.process_batch(
            sessions,
            BatchWorldAccess::Unavailable(SimulationRequestError::WorldUnavailable),
            None,
            None,
            batch,
        )
    }

    #[cfg(test)]
    pub(crate) fn process_tick_with_world(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        block_light: Option<&BlockLightTable>,
        budget: usize,
    ) -> SimulationTickReport {
        let batch = self.drain_batch(budget);
        if !batch
            .iter()
            .any(|envelope| command_requires_world(&envelope.command))
        {
            return self.process_batch(
                sessions,
                BatchWorldAccess::Unavailable(SimulationRequestError::WorldUnavailable),
                block_light,
                None,
                batch,
            );
        }
        let Some(world) = world else {
            return self.process_batch(
                sessions,
                BatchWorldAccess::Unavailable(SimulationRequestError::WorldUnavailable),
                block_light,
                None,
                batch,
            );
        };
        match world.try_lock() {
            Ok(mut storage) => self.process_batch(
                sessions,
                BatchWorldAccess::Storage(&mut storage),
                block_light,
                None,
                batch,
            ),
            Err(_) => self.process_batch(
                sessions,
                BatchWorldAccess::Unavailable(SimulationRequestError::WorldBusy),
                block_light,
                None,
                batch,
            ),
        }
    }

    #[cfg(test)]
    pub(crate) async fn process_commands_with_world(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        block_light: Option<&BlockLightTable>,
        budget: usize,
    ) -> SimulationTickReport {
        let batch = self.drain_batch(budget);
        self.process_envelopes_with_world(
            sessions,
            world,
            SimulationWorldAccess::default(),
            block_light,
            batch,
        )
        .await
    }

    pub(crate) async fn process_commands_with_world_views(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        access: SimulationWorldAccess<'_>,
        block_light: Option<&BlockLightTable>,
        budget: usize,
    ) -> SimulationTickReport {
        let batch = self.drain_batch(budget);
        self.process_envelopes_with_world(sessions, world, access, block_light, batch)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn process_ready_commands_with_world(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        block_light: Option<&BlockLightTable>,
        budget: usize,
    ) -> SimulationTickReport {
        let batch = self.drain_ready_batch(budget);
        self.process_envelopes_with_world(
            sessions,
            world,
            SimulationWorldAccess::default(),
            block_light,
            batch,
        )
        .await
    }

    pub(crate) async fn process_ready_commands_with_world_views(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        access: SimulationWorldAccess<'_>,
        block_light: Option<&BlockLightTable>,
        budget: usize,
    ) -> SimulationTickReport {
        let batch = self.drain_ready_batch(budget);
        self.process_envelopes_with_world(sessions, world, access, block_light, batch)
            .await
    }

    async fn process_envelopes_with_world(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        access: SimulationWorldAccess<'_>,
        block_light: Option<&BlockLightTable>,
        batch: Vec<SimulationCommandEnvelope>,
    ) -> SimulationTickReport {
        let regional_block_edits_available = world.is_some()
            && access.mutation.is_some()
            && access.cpu.is_some()
            && access.read.is_some()
            && (block_light.is_none() || access.light.is_some());
        let world_chunk_journal = sessions.world_chunk_journal();
        let mut runs: Vec<(bool, bool, bool, bool, Vec<SimulationCommandEnvelope>)> = Vec::new();
        for envelope in batch {
            let requires_world = command_requires_world(&envelope.command);
            let block_drop_command =
                matches!(envelope.command, SimulationCommand::CommitBlockDrops { .. });
            let regional_block_edit = requires_world
                && regional_block_edits_available
                && !envelope.response_is_closed()
                && envelope
                    .session_fence
                    .is_none_or(|session_id| sessions.is_active_session(session_id))
                && command_can_use_regional_mutation(&envelope.command, access.read, block_light);
            let journaled_block_edit = regional_block_edit
                && world_chunk_journal.is_some()
                && matches!(envelope.command, SimulationCommand::ApplyBlockEdits { .. });
            if let Some((
                last_requires_world,
                last_regional_block_edit,
                last_journaled,
                last_block_drop_command,
                run,
            )) = runs.last_mut()
                && *last_requires_world == requires_world
                && *last_regional_block_edit == regional_block_edit
                && *last_journaled == journaled_block_edit
                && *last_block_drop_command == block_drop_command
            {
                run.push(envelope);
            } else {
                runs.push((
                    requires_world,
                    regional_block_edit,
                    journaled_block_edit,
                    block_drop_command,
                    vec![envelope],
                ));
            }
        }

        let mut processed = 0;
        let mut lane_attribution = Vec::new();
        let mut runs = runs.into_iter();
        while let Some((
            requires_world,
            regional_block_edit,
            journaled_block_edit,
            block_drop_command,
            run,
        )) = runs.next()
        {
            let mut owner_fail_stopped = false;
            let report = if !requires_world {
                self.process_batch(
                    sessions,
                    BatchWorldAccess::Unavailable(SimulationRequestError::WorldUnavailable),
                    block_light,
                    None,
                    run,
                )
            } else if regional_block_edit {
                self.process_regional_block_edit_run(
                    sessions,
                    access,
                    block_light,
                    journaled_block_edit.then(|| {
                        world_chunk_journal
                            .as_ref()
                            .expect("journaled run has a journal")
                    }),
                    run,
                )
                .await
            } else if block_drop_command {
                let result = self
                    .process_resident_block_drop_run(
                        sessions,
                        access,
                        block_light,
                        world_chunk_journal.as_ref(),
                        run,
                    )
                    .await;
                owner_fail_stopped = result.fail_stopped;
                result.report
            } else if let Some(world) = world {
                let mut processed = 0;
                for envelope in run {
                    let mut pending_relight = None;
                    let resident_block_snapshot = match &envelope.command {
                        SimulationCommand::ReadBlockSnapshot { position } => access
                            .read
                            .and_then(|view| view.block_mutation_snapshot(*position))
                            .map(|(state, token)| {
                                (*position, BlockMutationSnapshot { state, token })
                            }),
                        _ => None,
                    };
                    let resident_mutation = access.mutation.filter(|_| {
                        command_can_use_resident_mutation(
                            &envelope.command,
                            access.read,
                            block_light,
                            true,
                        )
                    });
                    let report = if let Some((position, snapshot)) = resident_block_snapshot {
                        self.process_batch(
                            sessions,
                            BatchWorldAccess::ResidentBlock(position, snapshot),
                            block_light,
                            None,
                            vec![envelope],
                        )
                    } else if let Some(mutation) = resident_mutation {
                        self.process_batch(
                            sessions,
                            BatchWorldAccess::ResidentMutation(mutation, access.read),
                            block_light,
                            None,
                            vec![envelope],
                        )
                    } else {
                        let mut storage = crate::lock_metrics::timed_guard(
                            crate::lock_metrics::LockMetricKind::WorldStorage,
                            "simulation world command",
                            std::time::Instant::now(),
                            world.lock().await,
                        );
                        self.process_batch(
                            sessions,
                            BatchWorldAccess::Storage(&mut storage),
                            block_light,
                            Some(&mut pending_relight),
                            vec![envelope],
                        )
                    };
                    processed += report.processed;
                    if let Some(table) = block_light {
                        finish_pending_owner_relight(
                            sessions,
                            world,
                            access.mutation,
                            table,
                            pending_relight,
                        )
                        .await;
                    } else {
                        debug_assert!(pending_relight.is_none());
                    }
                }
                SimulationTickReport {
                    processed,
                    remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                    ..SimulationTickReport::default()
                }
            } else {
                self.process_batch(
                    sessions,
                    BatchWorldAccess::Unavailable(SimulationRequestError::WorldUnavailable),
                    block_light,
                    None,
                    run,
                )
            };
            processed += report.processed;
            lane_attribution.extend(report.lane_attribution);
            if owner_fail_stopped {
                for envelope in runs.flat_map(|(_, _, _, _, run)| run) {
                    self.reject_drained_envelope(envelope, SimulationRequestError::OwnerStopped);
                }
                self.shutdown();
                break;
            }
        }

        SimulationTickReport {
            processed,
            remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
            lane_attribution,
        }
    }

    fn reject_drained_envelope(
        &self,
        envelope: SimulationCommandEnvelope,
        error: SimulationRequestError,
    ) {
        if envelope.response_is_closed() {
            self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics
                .rejected_shutdown
                .fetch_add(1, Ordering::Relaxed);
            envelope.respond(Err(error));
        }
    }

    async fn append_block_drop_decision(
        journal: &crate::play::world_journal::WorldChunkJournal,
        world_tick: u64,
        decision_id: u64,
        snapshots: Vec<mc_world::ChunkSnapshot>,
    ) -> Result<(), BlockDropJournalAppendError> {
        let journal = journal.clone();
        tokio::task::spawn_blocking(move || {
            journal.record_reserved_snapshot_groups(world_tick, vec![(decision_id, snapshots)])
        })
        .await
        .map_err(BlockDropJournalAppendError::Worker)?
        .map_err(BlockDropJournalAppendError::Journal)
    }

    async fn close_empty_block_drop_decision(
        journal: &crate::play::world_journal::WorldChunkJournal,
        world_tick: u64,
        decision_id: u64,
    ) -> Result<(), BlockDropJournalAppendError> {
        journal
            .wait_for_append_turn(decision_id)
            .await
            .map_err(BlockDropJournalAppendError::Journal)?;
        Self::append_block_drop_decision(journal, world_tick, decision_id, Vec::new()).await
    }

    fn request_is_stale(sessions: &SessionRegistry, envelope: &SimulationCommandEnvelope) -> bool {
        envelope
            .session_fence
            .is_some_and(|session_id| !sessions.is_active_session(session_id))
    }

    async fn process_resident_block_drop_run(
        &mut self,
        sessions: &SessionRegistry,
        access: SimulationWorldAccess<'_>,
        block_light: Option<&BlockLightTable>,
        journal: Option<&crate::play::world_journal::WorldChunkJournal>,
        run: Vec<SimulationCommandEnvelope>,
    ) -> ResidentBlockDropRunResult {
        let mut processed = 0;
        let mut fail_stopped = false;
        let world_tick = sessions.simulation_tick();
        let mut envelopes = run.into_iter();
        for envelope in envelopes.by_ref() {
            if envelope.response_is_closed() {
                self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if Self::request_is_stale(sessions, &envelope) {
                self.metrics
                    .rejected_stale_session
                    .fetch_add(1, Ordering::Relaxed);
                envelope.respond(Err(SimulationRequestError::StaleSession));
                continue;
            }

            processed += 1;
            self.metrics.processed.fetch_add(1, Ordering::Relaxed);
            self.metrics
                .block_edits_processed
                .fetch_add(1, Ordering::Relaxed);

            let error = match &envelope.command {
                SimulationCommand::CommitBlockDrops {
                    edits,
                    preconditions,
                    drops,
                    ..
                } if !valid_block_drop_command(edits, preconditions, drops) => {
                    Some(SimulationRequestError::InvalidCommand)
                }
                SimulationCommand::CommitBlockDrops { .. }
                    if command_single_owner_region(&envelope.command).is_none() =>
                {
                    Some(SimulationRequestError::CrossRegion)
                }
                SimulationCommand::CommitBlockDrops { .. }
                    if access.mutation.is_none()
                        || !command_can_use_resident_block_drop(&envelope.command, access.read) =>
                {
                    Some(SimulationRequestError::WorldUnavailable)
                }
                SimulationCommand::CommitBlockDrops { .. } => None,
                _ => Some(SimulationRequestError::InvalidCommand),
            };

            if let Some(error) = error {
                if matches!(error, SimulationRequestError::WorldUnavailable) {
                    self.record_world_access_error(error);
                }
                envelope.respond(Err(error));
                continue;
            }

            let SimulationCommand::CommitBlockDrops {
                actor_session,
                edits,
                preconditions,
                drops,
            } = &envelope.command
            else {
                unreachable!("block drop route received a different command");
            };
            let actor_session = *actor_session;
            let edits = edits.clone();
            let preconditions = preconditions.clone();
            let drops = drops.clone();
            let mutation = access.mutation.expect("resident block drop mutation view");
            let decision_id = if let Some(journal) = journal {
                let journal = journal.clone();
                match tokio::task::spawn_blocking(move || journal.reserve_decision_ids(1)).await {
                    Ok(Ok(ids)) => {
                        let Some(decision_id) = ids.into_iter().next() else {
                            warn!("block-drop journal reserved no decision id");
                            sessions.report_world_chunk_journal_failure();
                            self.metrics
                                .rejected_world_mutation
                                .fetch_add(1, Ordering::Relaxed);
                            envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                            fail_stopped = true;
                            break;
                        };
                        Some(decision_id)
                    }
                    Ok(Err(error)) => {
                        warn!(%error, "block-drop journal decision reservation failed");
                        sessions.report_world_chunk_journal_failure();
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        fail_stopped = true;
                        break;
                    }
                    Err(error) => {
                        warn!(?error, "block-drop journal reservation worker failed");
                        sessions.report_world_chunk_journal_failure();
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        fail_stopped = true;
                        break;
                    }
                }
            } else {
                None
            };

            #[cfg(test)]
            if decision_id.is_some() {
                pause_block_drop_after(BlockDropAwaitStage::AfterReservation).await;
            }

            if envelope.response_is_closed() || Self::request_is_stale(sessions, &envelope) {
                if let (Some(journal), Some(decision_id)) = (journal, decision_id)
                    && let Err(error) =
                        Self::close_empty_block_drop_decision(journal, world_tick, decision_id)
                            .await
                {
                    match error {
                        BlockDropJournalAppendError::Journal(error) => {
                            warn!(
                                outcome_unknown = error.outcome_unknown(),
                                %error,
                                "block-drop cancelled decision closure failed"
                            );
                        }
                        BlockDropJournalAppendError::Worker(error) => {
                            warn!(
                                ?error,
                                "block-drop cancelled decision closure worker failed"
                            );
                        }
                    }
                    sessions.report_world_chunk_journal_failure();
                    self.metrics
                        .rejected_world_mutation
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                    fail_stopped = true;
                    break;
                }
                if envelope.response_is_closed() {
                    self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.metrics
                        .rejected_stale_session
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Err(SimulationRequestError::StaleSession));
                }
                continue;
            }

            if let (Some(journal), Some(decision_id)) = (journal, decision_id) {
                if let Err(error) = journal.wait_for_append_turn(decision_id).await {
                    warn!(%error, "block-drop journal append ordering failed");
                    sessions.report_world_chunk_journal_failure();
                    self.metrics
                        .rejected_world_mutation
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                    fail_stopped = true;
                    break;
                }
                if envelope.response_is_closed() || Self::request_is_stale(sessions, &envelope) {
                    if let Err(error) =
                        Self::close_empty_block_drop_decision(journal, world_tick, decision_id)
                            .await
                    {
                        match error {
                            BlockDropJournalAppendError::Journal(error) => warn!(
                                outcome_unknown = error.outcome_unknown(),
                                %error,
                                "block-drop ordered cancellation closure failed"
                            ),
                            BlockDropJournalAppendError::Worker(error) => warn!(
                                ?error,
                                "block-drop ordered cancellation closure worker failed"
                            ),
                        }
                        sessions.report_world_chunk_journal_failure();
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        fail_stopped = true;
                        break;
                    }
                    if envelope.response_is_closed() {
                        self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
                    } else {
                        self.metrics
                            .rejected_stale_session
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::StaleSession));
                    }
                    continue;
                }
            }

            let resident_edits = resident_block_edits(&edits, &preconditions, block_light);
            let resident_preconditions = resident_block_preconditions(&preconditions);
            let (raw_outcome, touched_chunks) = if let Some(decision_id) = decision_id {
                mutation.apply_block_edits_conditionally_journaled(
                    decision_id,
                    &resident_edits,
                    &resident_preconditions,
                    &[],
                    block_light,
                    Some(world_tick.saturating_add(1)),
                )
            } else {
                (
                    mutation.apply_block_edits_conditionally(
                        &resident_edits,
                        &resident_preconditions,
                        &[],
                        block_light,
                        Some(world_tick.saturating_add(1)),
                    ),
                    Vec::new(),
                )
            };

            #[cfg(test)]
            if journal.is_some()
                && let Some(probe) = self.regional_block_edit_probe.as_ref()
            {
                probe.enter(
                    command_single_owner_region(&envelope.command)
                        .expect("validated block-drop command has a resident owner"),
                );
            }

            if let (Some(journal), Some(decision_id)) = (journal, decision_id) {
                let world_read = access.read.expect("resident block drop read view");
                let snapshot = world_read.snapshot_chunks(&touched_chunks);
                let snapshots = touched_chunks
                    .iter()
                    .filter_map(|position| snapshot.chunk(*position))
                    .collect::<Vec<_>>();
                if snapshots.len() != touched_chunks.len() {
                    warn!("block-drop journal snapshot was incomplete");
                    let closure =
                        Self::close_empty_block_drop_decision(journal, world_tick, decision_id)
                            .await;
                    if let Err(error) = closure {
                        match error {
                            BlockDropJournalAppendError::Journal(error) => warn!(
                                outcome_unknown = error.outcome_unknown(),
                                %error,
                                "block-drop incomplete snapshot closure failed"
                            ),
                            BlockDropJournalAppendError::Worker(error) => warn!(
                                ?error,
                                "block-drop incomplete snapshot closure worker failed"
                            ),
                        }
                    }
                    sessions.report_world_chunk_journal_failure();
                    self.metrics
                        .rejected_world_mutation
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                    fail_stopped = true;
                    break;
                }
                match Self::append_block_drop_decision(journal, world_tick, decision_id, snapshots)
                    .await
                {
                    Ok(()) => {}
                    Err(error) => {
                        let outcome_unknown = error.outcome_unknown();
                        match &error {
                            BlockDropJournalAppendError::Journal(error) => warn!(
                                outcome_unknown,
                                %error,
                                "block-drop journal append failed"
                            ),
                            BlockDropJournalAppendError::Worker(error) => warn!(
                                outcome_unknown,
                                ?error,
                                "block-drop journal append worker failed"
                            ),
                        }
                        if !outcome_unknown
                            && let Err(closure_error) = Self::close_empty_block_drop_decision(
                                journal,
                                world_tick,
                                decision_id,
                            )
                            .await
                        {
                            match closure_error {
                                BlockDropJournalAppendError::Journal(error) => warn!(
                                    outcome_unknown = error.outcome_unknown(),
                                    %error,
                                    "block-drop known append failure closure failed"
                                ),
                                BlockDropJournalAppendError::Worker(error) => warn!(
                                    ?error,
                                    "block-drop known append failure closure worker failed"
                                ),
                            }
                        }
                        sessions.report_world_chunk_journal_failure();
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        fail_stopped = true;
                        break;
                    }
                }

                #[cfg(test)]
                pause_block_drop_after(BlockDropAwaitStage::AfterAppend).await;

                let cancelled_before_clear = envelope.response_is_closed();
                let stale_before_clear = Self::request_is_stale(sessions, &envelope);
                let cleared =
                    mutation.clear_journal_pending_conditionally(decision_id, &touched_chunks);
                if cleared != touched_chunks.len() {
                    warn!(
                        decision_id,
                        expected = touched_chunks.len(),
                        cleared,
                        "block-drop journal fence clear did not retire the exact decision"
                    );
                    sessions.report_world_chunk_journal_failure();
                    self.metrics
                        .rejected_world_mutation
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                    fail_stopped = true;
                    break;
                }
                let cancelled = cancelled_before_clear || envelope.response_is_closed();
                let stale = stale_before_clear || Self::request_is_stale(sessions, &envelope);
                if cancelled || stale {
                    if cancelled {
                        self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
                    } else {
                        self.metrics
                            .rejected_stale_session
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::StaleSession));
                    }
                    continue;
                }
            }

            match raw_outcome {
                ResidentBlockEditBatchResult::Applied(applied) => {
                    let Some(outcome) = resident_block_edit_result_outcome(
                        ResidentBlockEditBatchResult::Applied(applied),
                    ) else {
                        unreachable!("applied resident block-drop result lost its outcome");
                    };
                    if outcome.applied.len() != edits.len() {
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                        fail_stopped = true;
                        break;
                    }
                    let drop_dispatches = match sessions.try_spawn_item_drop_batch_owned(
                        &self.authority,
                        drops
                            .iter()
                            .map(|drop| (drop.entity_type_id, drop.position, drop.stack.clone())),
                    ) {
                        Ok(dispatches) => dispatches,
                        Err(error) => {
                            warn!(?error, "block-drop entity batch commit failed");
                            self.metrics
                                .rejected_world_mutation
                                .fetch_add(1, Ordering::Relaxed);
                            envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                            fail_stopped = true;
                            break;
                        }
                    };
                    self.publish_resident_block_drop(
                        sessions,
                        actor_session,
                        drop_dispatches,
                        &outcome,
                    );
                    envelope.respond(Ok(SimulationResponse::BlockDrops(Ok(Box::new(Some(
                        outcome,
                    ))))));
                }
                ResidentBlockEditBatchResult::Stale => {
                    envelope.respond(Ok(SimulationResponse::BlockDrops(Ok(Box::new(None)))));
                }
                ResidentBlockEditBatchResult::Missing => {
                    self.record_world_access_error(SimulationRequestError::WorldUnavailable);
                    envelope.respond(Err(SimulationRequestError::WorldUnavailable));
                }
                ResidentBlockEditBatchResult::CrossRegion => {
                    envelope.respond(Err(SimulationRequestError::CrossRegion));
                }
            }
        }

        if fail_stopped {
            for envelope in envelopes {
                self.reject_drained_envelope(envelope, SimulationRequestError::OwnerStopped);
            }
        }

        ResidentBlockDropRunResult {
            report: SimulationTickReport {
                processed,
                remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                ..SimulationTickReport::default()
            },
            fail_stopped,
        }
    }

    fn publish_resident_block_drop(
        &self,
        sessions: &SessionRegistry,
        actor_session: SessionId,
        drop_dispatches: Vec<VisibilityDispatch>,
        outcome: &BlockEditBatchOutcome,
    ) {
        sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
        let mut dispatches = sessions
            .loaded_recipients_for_chunks(&outcome.edit_chunks, Some(actor_session))
            .into_iter()
            .map(|recipient| VisibilityDispatch {
                recipient,
                command: OutboundCommand::BlockDeltas(outcome.deltas.clone()),
            })
            .collect::<Vec<_>>();
        dispatches.extend(drop_dispatches);
        dispatch_visibility_commands(dispatches);
    }

    fn commit_chest_command(
        &self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        request: ChestCommitRequest<'_>,
    ) -> Result<Box<ChestCommitOutcome>, SimulationRequestError> {
        let ChestCommitRequest {
            primary_position,
            positions,
            expected_state_id,
            actor_session,
            expected,
            updated,
            player,
        } = request;
        if positions.is_empty()
            || positions.len() > 2
            || positions.first() != Some(&primary_position)
            || expected.len() != positions.len()
            || updated.len() != positions.len()
            || !valid_container_player_plan(player)
        {
            return Err(SimulationRequestError::InvalidCommand);
        }
        let Some(storage) = storage else {
            self.record_world_access_error(world_error);
            return Err(world_error);
        };
        if positions
            .iter()
            .any(|position| storage.block_mutation_token(*position).is_none())
        {
            self.metrics
                .rejected_world_unavailable
                .fetch_add(1, Ordering::Relaxed);
            return Err(SimulationRequestError::WorldUnavailable);
        }
        let mut authoritative = Vec::with_capacity(positions.len());
        for position in positions {
            match storage.chest_block_entity(*position) {
                Ok(Some(chest)) => authoritative.push(chest),
                Ok(None) => {
                    self.metrics
                        .rejected_world_unavailable
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(SimulationRequestError::WorldUnavailable);
                }
                Err(error) => {
                    self.record_world_mutation_failure(
                        "read chest block entity",
                        WorldContainerCommitError::Storage(error),
                    );
                    return Err(SimulationRequestError::WorldMutationFailed);
                }
            }
        }
        if authoritative != expected {
            let (inventory, carried_item) = sessions
                .player_container_state(actor_session)
                .ok_or(SimulationRequestError::StaleSession)?;
            return Ok(Box::new(SharedContainerCommit::Rejected {
                state_id: sessions.chest_state_id(primary_position),
                authoritative,
                inventory,
                carried_item,
            }));
        }
        let slots = chest_slot_stacks(&ChestView {
            chests: updated.to_vec(),
        });
        let commit = sessions.commit_chest_slots(
            &self.authority,
            ContainerCommitContext {
                position: primary_position,
                expected_state_id,
                actor_session,
                player,
            },
            slots,
            || {
                for (&position, chest) in positions.iter().zip(updated) {
                    match storage.set_chest_block_entity(position, chest.clone()) {
                        Ok(true) => {}
                        Ok(false) => {
                            return Err(WorldContainerCommitError::MissingChunk(position));
                        }
                        Err(error) => return Err(WorldContainerCommitError::Storage(error)),
                    }
                }
                Ok(())
            },
        );
        match commit {
            Ok((state_id, inventory, carried_item, dispatches)) => {
                Ok(Box::new(SharedContainerCommit::Committed {
                    state_id,
                    inventory,
                    carried_item,
                    dispatches,
                }))
            }
            Err(ContainerStateCommitError::Rejected {
                state_id,
                inventory,
                carried_item,
            }) => Ok(Box::new(SharedContainerCommit::Rejected {
                state_id,
                authoritative,
                inventory: *inventory,
                carried_item,
            })),
            Err(ContainerStateCommitError::MissingPlayer) => {
                Err(SimulationRequestError::StaleSession)
            }
            Err(ContainerStateCommitError::Commit(error)) => {
                self.record_world_mutation_failure("write chest block entity", error);
                Err(SimulationRequestError::WorldMutationFailed)
            }
        }
    }

    fn commit_furnace_command(
        &self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        request: FurnaceCommitRequest<'_>,
    ) -> Result<Box<FurnaceCommitOutcome>, SimulationRequestError> {
        let FurnaceCommitRequest {
            position,
            expected_state_id,
            actor_session,
            expected,
            updated,
            player,
        } = request;
        if !valid_furnace_commit_command(expected, updated, player) {
            return Err(SimulationRequestError::InvalidCommand);
        }
        let Some(storage) = storage else {
            self.record_world_access_error(world_error);
            return Err(world_error);
        };
        if storage.block_mutation_token(position).is_none() {
            self.metrics
                .rejected_world_unavailable
                .fetch_add(1, Ordering::Relaxed);
            return Err(SimulationRequestError::WorldUnavailable);
        }
        let authoritative = match storage.furnace_block_entity(position) {
            Ok(Some(furnace)) => furnace,
            Ok(None) => {
                self.metrics
                    .rejected_world_unavailable
                    .fetch_add(1, Ordering::Relaxed);
                return Err(SimulationRequestError::WorldUnavailable);
            }
            Err(error) => {
                self.record_world_mutation_failure(
                    "read furnace block entity",
                    WorldContainerCommitError::Storage(error),
                );
                return Err(SimulationRequestError::WorldMutationFailed);
            }
        };
        if authoritative.slots != expected.slots
            || authoritative.recipes_used != expected.recipes_used
        {
            let (inventory, carried_item) = sessions
                .player_container_state(actor_session)
                .ok_or(SimulationRequestError::StaleSession)?;
            return Ok(Box::new(SharedContainerCommit::Rejected {
                state_id: sessions.furnace_state_id(position),
                authoritative,
                inventory,
                carried_item,
            }));
        }
        let mut merged = authoritative.clone();
        merged.slots = updated.slots.clone();
        merged.recipes_used = updated.recipes_used.clone();
        let commit = sessions.commit_furnace_slots(
            &self.authority,
            ContainerCommitContext {
                position,
                expected_state_id,
                actor_session,
                player,
            },
            furnace_slot_stacks(&merged),
            || match storage.set_furnace_block_entity(position, merged.clone()) {
                Ok(true) => Ok(()),
                Ok(false) => Err(WorldContainerCommitError::MissingChunk(position)),
                Err(error) => Err(WorldContainerCommitError::Storage(error)),
            },
        );
        match commit {
            Ok((state_id, inventory, carried_item, dispatches)) => {
                Ok(Box::new(SharedContainerCommit::Committed {
                    state_id,
                    inventory,
                    carried_item,
                    dispatches,
                }))
            }
            Err(ContainerStateCommitError::Rejected {
                state_id,
                inventory,
                carried_item,
            }) => Ok(Box::new(SharedContainerCommit::Rejected {
                state_id,
                authoritative,
                inventory: *inventory,
                carried_item,
            })),
            Err(ContainerStateCommitError::MissingPlayer) => {
                Err(SimulationRequestError::StaleSession)
            }
            Err(ContainerStateCommitError::Commit(error)) => {
                self.record_world_mutation_failure("write furnace block entity", error);
                Err(SimulationRequestError::WorldMutationFailed)
            }
        }
    }

    fn record_world_access_error(&self, error: SimulationRequestError) {
        match error {
            #[cfg(test)]
            SimulationRequestError::WorldBusy => {
                self.metrics
                    .rejected_world_busy
                    .fetch_add(1, Ordering::Relaxed);
            }
            SimulationRequestError::WorldUnavailable => {
                self.metrics
                    .rejected_world_unavailable
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => unreachable!("invalid world access error: {error:?}"),
        }
    }

    fn record_world_mutation_failure(
        &self,
        operation: &'static str,
        error: WorldContainerCommitError,
    ) {
        self.metrics
            .rejected_world_mutation
            .fetch_add(1, Ordering::Relaxed);
        match error {
            WorldContainerCommitError::MissingChunk(position) => {
                warn!(
                    ?position,
                    operation, "simulation container commit lost cached chunk"
                );
            }
            WorldContainerCommitError::Storage(error) => {
                warn!(%error, operation, "simulation container world mutation failed");
            }
        }
    }

    fn commit_opaque_block_entity_command(
        &self,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    ) -> Result<bool, SimulationRequestError> {
        let Some(storage) = storage else {
            self.record_world_access_error(world_error);
            return Err(world_error);
        };
        match apply_opaque_block_entity_to_storage_conditionally(
            storage,
            position,
            expected_state,
            expected_token,
            bytes,
        ) {
            Ok(committed) => Ok(committed),
            Err(error) => {
                self.record_world_mutation_failure(
                    "write opaque block entity",
                    WorldContainerCommitError::Storage(error),
                );
                Err(SimulationRequestError::WorldMutationFailed)
            }
        }
    }

    fn process_batch(
        &mut self,
        sessions: &SessionRegistry,
        world_access: BatchWorldAccess<'_>,
        block_light: Option<&BlockLightTable>,
        mut pending_relight: Option<&mut Option<PendingOwnerRelight>>,
        batch: Vec<SimulationCommandEnvelope>,
    ) -> SimulationTickReport {
        let (mut storage, world_error, resident_block_snapshot, resident_mutation, resident_read) =
            match world_access {
                BatchWorldAccess::Unavailable(error) => (None, error, None, None, None),
                BatchWorldAccess::Storage(storage) => (
                    Some(storage),
                    SimulationRequestError::WorldUnavailable,
                    None,
                    None,
                    None,
                ),
                BatchWorldAccess::ResidentBlock(position, snapshot) => (
                    None,
                    SimulationRequestError::WorldUnavailable,
                    Some((position, snapshot)),
                    None,
                    None,
                ),
                BatchWorldAccess::ResidentMutation(mutation, read) => (
                    None,
                    SimulationRequestError::WorldUnavailable,
                    None,
                    Some(mutation),
                    read,
                ),
            };
        let regional_batch = match self.prepare_single_lane_region_routes(&batch) {
            Ok(phase) => phase,
            Err(error) => {
                warn!(?error, "simulation regional route preparation failed");
                let processed = batch.len();
                for envelope in batch {
                    envelope.respond(Err(SimulationRequestError::InvalidCommand));
                }
                return SimulationTickReport {
                    processed,
                    remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                    ..SimulationTickReport::default()
                };
            }
        };
        let mut processed = 0usize;
        let mut batch = VecDeque::from(batch);
        while let Some(envelope) = batch.pop_front() {
            if let Some(lease) = regional_batch
                .as_ref()
                .and_then(|regional| regional.routes.get(&envelope.sequence))
                && !self.region_ownership.validate(*lease)
            {
                warn!(?lease, "simulation command has a stale regional lease");
                envelope.respond(Err(SimulationRequestError::InvalidCommand));
                processed += 1;
                continue;
            }
            if envelope.response_is_closed() {
                self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if envelope
                .session_fence
                .is_some_and(|session_id| !sessions.is_active_session(session_id))
            {
                self.metrics
                    .rejected_stale_session
                    .fetch_add(1, Ordering::Relaxed);
                envelope.respond(Err(SimulationRequestError::StaleSession));
                continue;
            }
            let detached = envelope.is_detached();
            if detached
                && let SimulationCommand::EnsureChunkHerd { chunk, spawns } = &envelope.command
            {
                let mut herds = vec![(*chunk, spawns.clone())];
                while batch.front().is_some_and(|next| {
                    next.is_detached()
                        && next.session_fence.is_none()
                        && matches!(next.command, SimulationCommand::EnsureChunkHerd { .. })
                }) {
                    let next = batch.pop_front().expect("matching detached herd command");
                    let SimulationCommand::EnsureChunkHerd { chunk, spawns } = next.command else {
                        unreachable!("detached herd predicate matches command")
                    };
                    herds.push((chunk, spawns));
                }
                let command_count = herds.len();
                let outcome = sessions.ensure_chunk_herds(&self.authority, &herds);
                self.release_retryable_herd_requests(outcome.retryable_chunks());
                dispatch_visibility_commands(outcome.into_dispatches());
                processed += command_count;
                self.metrics
                    .processed
                    .fetch_add(command_count as u64, Ordering::Relaxed);
                continue;
            }
            let item_pickup = matches!(
                &envelope.command,
                SimulationCommand::PickupItemIntoInventory { .. }
            );
            let block_edit = matches!(
                &envelope.command,
                SimulationCommand::ApplyBlockEdits { .. }
                    | SimulationCommand::CommitBlockDrops { .. }
                    | SimulationCommand::CommitSurvivalBreak(_)
                    | SimulationCommand::CommitSurvivalPlacement(_)
                    | SimulationCommand::CommitBucketUse(_)
                    | SimulationCommand::CommitTntIgnition { .. }
            );
            let container_commit = matches!(
                &envelope.command,
                SimulationCommand::CommitPlayerInventory { .. }
                    | SimulationCommand::CommitChest { .. }
                    | SimulationCommand::CommitFurnace { .. }
            );
            let block_entity_commit = matches!(
                &envelope.command,
                SimulationCommand::CommitOpaqueBlockEntity { .. }
                    | SimulationCommand::CommitCampfireUse(_)
            );
            let mut response = match &envelope.command {
                SimulationCommand::SaveBarrier { capture_world } => {
                    let simulation_tick = sessions.simulation_tick();
                    let world_flush_plan = if *capture_world {
                        match storage.as_deref_mut() {
                            Some(storage) => {
                                match storage.plan_dirty_flush_at_tick(simulation_tick) {
                                    Ok(plan) => Ok(Some(plan)),
                                    Err(error) => {
                                        self.metrics
                                            .rejected_world_mutation
                                            .fetch_add(1, Ordering::Relaxed);
                                        warn!(%error, "simulation save barrier world plan failed");
                                        Err(SimulationRequestError::WorldMutationFailed)
                                    }
                                }
                            }
                            None => {
                                self.record_world_access_error(world_error);
                                Err(world_error)
                            }
                        }
                    } else {
                        Ok(None)
                    };
                    SimulationResponse::SaveSnapshot(world_flush_plan.map(|world_flush_plan| {
                        let (entities, entity_journal_phases) =
                            sessions.persisted_entity_save_snapshot();
                        Box::new(SimulationSaveSnapshot {
                            players: sessions.persisted_player_states(),
                            entities,
                            entity_journal_phases,
                            world_chunk_journal_watermark: sessions.world_chunk_journal_watermark(),
                            world_time: sessions.world_time(),
                            players_sleeping_percentage: sessions.players_sleeping_percentage(),
                            simulation_tick,
                            world_flush_plan,
                        })
                    }))
                }
                SimulationCommand::ReadBlockSnapshot { position } => {
                    let result = if let Some((snapshot_position, snapshot)) =
                        resident_block_snapshot
                        && snapshot_position == *position
                    {
                        Ok(Some(snapshot))
                    } else if let Some(storage) = storage.as_deref_mut() {
                        match storage.get_block(*position) {
                            Ok(Some(state)) => Ok(storage
                                .block_mutation_token(*position)
                                .map(|token| BlockMutationSnapshot { state, token })),
                            Ok(None) => Ok(None),
                            Err(error) => {
                                self.metrics
                                    .rejected_world_mutation
                                    .fetch_add(1, Ordering::Relaxed);
                                warn!(%error, ?position, "simulation block snapshot read failed");
                                Err(SimulationRequestError::WorldMutationFailed)
                            }
                        }
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::BlockSnapshot(result)
                }
                SimulationCommand::ReadChestSnapshot { positions } => {
                    let result = if positions.is_empty()
                        || positions.len() > 2
                        || positions.windows(2).any(|pair| pair[0] == pair[1])
                    {
                        Err(SimulationRequestError::InvalidCommand)
                    } else if let Some(storage) = storage.as_deref_mut() {
                        let mut chests = Vec::with_capacity(positions.len());
                        let mut error = None;
                        for position in positions {
                            match storage.chest_block_entity(*position) {
                                Ok(Some(chest)) => chests.push(chest),
                                Ok(None) => {
                                    error = Some(SimulationRequestError::WorldUnavailable);
                                    break;
                                }
                                Err(storage_error) => {
                                    self.metrics
                                        .rejected_world_mutation
                                        .fetch_add(1, Ordering::Relaxed);
                                    warn!(%storage_error, ?position, "simulation chest snapshot read failed");
                                    error = Some(SimulationRequestError::WorldMutationFailed);
                                    break;
                                }
                            }
                        }
                        if let Some(error) = error {
                            Err(error)
                        } else {
                            Ok(Box::new(ChestReadSnapshot {
                                state_id: sessions.chest_state_id(positions[0]),
                                view: ChestView { chests },
                            }))
                        }
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::ChestSnapshot(result)
                }
                SimulationCommand::ReadFurnaceSnapshot { position } => {
                    let result = if let Some(storage) = storage.as_deref_mut() {
                        match storage.furnace_block_entity(*position) {
                            Ok(Some(furnace)) => Ok(Box::new(FurnaceReadSnapshot {
                                furnace,
                                state_id: sessions.furnace_state_id(*position),
                            })),
                            Ok(None) => Err(SimulationRequestError::WorldUnavailable),
                            Err(storage_error) => {
                                self.metrics
                                    .rejected_world_mutation
                                    .fetch_add(1, Ordering::Relaxed);
                                warn!(%storage_error, ?position, "simulation furnace snapshot read failed");
                                Err(SimulationRequestError::WorldMutationFailed)
                            }
                        }
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::FurnaceSnapshot(result)
                }
                SimulationCommand::PickupItemIntoInventory {
                    entity_id,
                    collector_session,
                    expected_item_id,
                    expected_damage,
                    expected_enchantments,
                    max_stack,
                } => {
                    let mut credited = sessions
                        .pickup_item_into_inventory(
                            &self.authority,
                            *entity_id,
                            *collector_session,
                            *expected_item_id,
                            *expected_damage,
                            expected_enchantments,
                            *max_stack,
                        )
                        .map(Box::new);
                    if let Some(credited) = credited.as_mut() {
                        dispatch_visibility_commands(std::mem::take(&mut credited.dispatches));
                    }
                    SimulationResponse::ItemPickupCredit(credited)
                }
                SimulationCommand::PickupExperienceIntoPlayer {
                    entity_id,
                    collector_session,
                } => {
                    let mut credited = sessions
                        .pickup_experience_into_player(
                            &self.authority,
                            *entity_id,
                            *collector_session,
                        )
                        .map(Box::new);
                    if let Some(credited) = credited.as_mut() {
                        dispatch_visibility_commands(std::mem::take(&mut credited.dispatches));
                    }
                    SimulationResponse::ExperiencePickupCredit(credited)
                }
                #[cfg(test)]
                SimulationCommand::ClaimExperiencePickup {
                    entity_id,
                    collector_session,
                } => {
                    if let Some(mut claimed) = sessions.claim_experience_pickup(
                        &self.authority,
                        *entity_id,
                        *collector_session,
                    ) {
                        dispatch_visibility_commands(std::mem::take(&mut claimed.dispatches));
                    }
                    SimulationResponse::ExperiencePickup
                }
                SimulationCommand::PickupArrowIntoInventory {
                    entity_id,
                    collector_session,
                    arrow_item_id,
                    max_stack,
                } => {
                    let mut credited = sessions
                        .pickup_arrow_into_inventory(
                            &self.authority,
                            *entity_id,
                            *collector_session,
                            *arrow_item_id,
                            *max_stack,
                        )
                        .map(Box::new);
                    if let Some(credited) = credited.as_mut() {
                        dispatch_visibility_commands(std::mem::take(&mut credited.dispatches));
                    }
                    SimulationResponse::ArrowPickupCredit(credited)
                }
                SimulationCommand::PlayerAttackServerEntity {
                    attacker_session,
                    entity_id,
                    damage,
                    attacker_costs,
                    cooldown_tick,
                } => {
                    let authority_tick = sessions.simulation_tick();
                    let mut result = sessions.player_attack_entity(
                        &self.authority,
                        PlayerEntityAttack {
                            attacker_session: *attacker_session,
                            entity_id: *entity_id,
                            amount: *damage,
                            attacker_costs: attacker_costs.as_deref(),
                            authority_tick,
                        },
                    );
                    if let PlayerAttackResult::Damaged(outcome) = &mut result
                        && let EntityAttackOutcome::PlayerDamaged { dispatches, .. } =
                            &mut **outcome
                    {
                        dispatch_visibility_commands(std::mem::take(dispatches));
                    }
                    if !matches!(result, PlayerAttackResult::ValidationRejected) {
                        sessions.publish_player_attack(
                            *attacker_session,
                            entity_id.0,
                            *cooldown_tick,
                            authority_tick,
                        );
                    }
                    SimulationResponse::PlayerAttack(result)
                }
                SimulationCommand::ApplyServerEntityEffect(command) => {
                    let request = EntityEffectRequest {
                        operation: command.operation.clone(),
                        target_kind: command.target_kind,
                        death_remove_tick: sessions
                            .simulation_tick()
                            .saturating_add(ENTITY_DEATH_TICKS),
                    };
                    let (result, dispatches) = sessions.apply_server_entity_effect_request(
                        &self.authority,
                        command.expected.as_deref().cloned(),
                        command.entity_id,
                        request,
                    );
                    match &result {
                        EntityEffectResult::Applied(applied) => {
                            trace!(
                                entity_id = applied.snapshot.id.0,
                                health = applied.snapshot.health,
                                "server entity effect transaction accepted"
                            );
                        }
                        EntityEffectResult::Rejected(rejection) => {
                            trace!(
                                entity_id = command.entity_id.0,
                                ?rejection,
                                "server entity effect transaction rejected"
                            );
                        }
                    }
                    dispatch_visibility_commands(dispatches);
                    SimulationResponse::EntityEffect(result)
                }
                #[cfg(test)]
                SimulationCommand::AttackServerEntity {
                    entity_id,
                    damage,
                    knockback_origin,
                    rewards,
                } => SimulationResponse::EntityAttack(
                    sessions
                        .attack_server_entity(
                            &self.authority,
                            *entity_id,
                            *damage,
                            *knockback_origin,
                            rewards,
                        )
                        .map(Box::new),
                ),
                SimulationCommand::SpawnCommandEntity {
                    entity_type_id,
                    entity_type_name,
                    position,
                } => SimulationResponse::EntitySpawn(sessions.spawn_command_entity(
                    &self.authority,
                    *entity_type_id,
                    entity_type_name.clone(),
                    *position,
                )),
                SimulationCommand::SetWorldTime { world_time } => {
                    let outcome = sessions.set_world_time_owned(&self.authority, *world_time);
                    self.release_retryable_herd_requests(outcome.retryable_chunks());
                    dispatch_visibility_commands(outcome.into_dispatches());
                    SimulationResponse::WorldTimeSet
                }
                SimulationCommand::EnsureChunkHerd { chunk, spawns } => {
                    let outcome = sessions.ensure_chunk_herd(&self.authority, *chunk, spawns);
                    self.release_retryable_herd_requests(outcome.retryable_chunks());
                    let dispatches = outcome.into_dispatches();
                    if detached {
                        dispatch_visibility_commands(dispatches);
                        SimulationResponse::EntitySpawn(Vec::new())
                    } else {
                        SimulationResponse::EntitySpawn(dispatches)
                    }
                }
                SimulationCommand::ApplyBlockEdits {
                    actor_session,
                    edits,
                    preconditions,
                    scheduled_block_ticks,
                } => {
                    let result = if !valid_block_edit_command(
                        edits,
                        preconditions,
                        scheduled_block_ticks,
                    ) {
                        Err(SimulationRequestError::InvalidCommand)
                    } else if storage.is_some() || resident_mutation.is_some() {
                        let regional = storage.is_none();
                        let mut outcome = if let Some(storage) = storage.as_deref_mut() {
                            apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
                                storage,
                                block_light,
                                edits,
                                preconditions,
                                scheduled_block_ticks,
                            )
                        } else {
                            resident_block_edit_outcome(
                                resident_mutation.expect("resident mutation access"),
                                block_light,
                                sessions.simulation_tick(),
                                edits,
                                preconditions,
                                scheduled_block_ticks,
                            )
                        };
                        if let Some(outcome) = outcome.as_mut() {
                            if !regional {
                                let storage = storage
                                    .as_deref_mut()
                                    .expect("coordinator block edit storage");
                                schedule_leaf_ticks_near_applied(
                                    storage,
                                    sessions.simulation_tick(),
                                    &outcome.applied,
                                );
                                if let Some(table) = block_light
                                    && let Some(light_updates) = prepare_owner_relight(
                                        storage,
                                        table,
                                        outcome,
                                        pending_relight.is_some(),
                                    )
                                {
                                    let light_chunks = light_updates
                                        .iter()
                                        .map(|update| (update.pos.x, update.pos.z))
                                        .collect::<HashSet<_>>();
                                    sessions.invalidate_prepared_chunks(&light_chunks);
                                    outcome.precomputed_light_updates = Some(light_updates);
                                }
                            }
                            sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
                            let mut dispatches = sessions
                                .loaded_recipients_for_chunks(
                                    &outcome.edit_chunks,
                                    Some(*actor_session),
                                )
                                .into_iter()
                                .map(|recipient| VisibilityDispatch {
                                    recipient,
                                    command: OutboundCommand::BlockDeltas(outcome.deltas.clone()),
                                })
                                .collect::<Vec<_>>();
                            if let Some(light_updates) = outcome.precomputed_light_updates.as_ref()
                                && !light_updates.is_empty()
                            {
                                let light_chunks = light_updates
                                    .iter()
                                    .map(|update| (update.pos.x, update.pos.z))
                                    .collect::<HashSet<_>>();
                                dispatches.extend(
                                    sessions
                                        .loaded_recipients_for_chunks(
                                            &light_chunks,
                                            Some(*actor_session),
                                        )
                                        .into_iter()
                                        .map(|recipient| VisibilityDispatch {
                                            recipient,
                                            command: OutboundCommand::LightUpdates(
                                                light_updates.clone(),
                                            ),
                                        }),
                                );
                            }
                            dispatch_visibility_commands(dispatches);
                        }
                        Ok(Box::new(outcome))
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::BlockEdits(result)
                }
                SimulationCommand::CommitBlockDrops { .. } => {
                    self.record_world_access_error(world_error);
                    SimulationResponse::BlockDrops(Err(world_error))
                }
                SimulationCommand::ScheduleFluidTicksNearApplied {
                    applied,
                    block_facts,
                    world_tick,
                } => {
                    if applied.len() <= MAX_BLOCK_EDIT_COMMAND_EDITS {
                        if let Some(storage) = storage.as_deref_mut() {
                            schedule_fluid_ticks_near_applied(
                                storage,
                                block_facts,
                                *world_tick,
                                applied,
                            );
                        } else if let (Some(mutation), Some(read)) =
                            (resident_mutation, resident_read)
                        {
                            let ticks = super::plan_fluid_ticks_near_applied(
                                read,
                                block_facts,
                                *world_tick,
                                applied,
                            );
                            mutation.schedule_fluid_ticks(&ticks);
                        } else {
                            self.record_world_access_error(world_error);
                        }
                    }
                    SimulationResponse::FluidTicksScheduled
                }
                SimulationCommand::CommitSurvivalBreak(command) => {
                    let result = if let Some(storage) = storage.as_deref_mut() {
                        let request_is_valid = match &command.request {
                            SurvivalBreakRequest::Prepared(_) => true,
                            SurvivalBreakRequest::Block(plan) => {
                                valid_survival_block_break_plan(plan)
                            }
                        };
                        if !request_is_valid {
                            Err(SimulationRequestError::InvalidCommand)
                        } else {
                            let prepared_plan = match &command.request {
                                SurvivalBreakRequest::Prepared(_) => None,
                                SurvivalBreakRequest::Block(plan) => {
                                    prepare_survival_block_break_plan(storage, plan)
                                }
                            };
                            let plan = match &command.request {
                                SurvivalBreakRequest::Prepared(plan) => Some(plan),
                                SurvivalBreakRequest::Block(_) => prepared_plan.as_ref(),
                            };
                            match plan {
                                None => Ok(None),
                                Some(plan) if !valid_survival_break_plan(plan) => {
                                    Err(SimulationRequestError::InvalidCommand)
                                }
                                Some(plan) => sessions
                                    .commit_survival_break(
                                        &self.authority,
                                        storage,
                                        block_light,
                                        command.actor_session,
                                        plan,
                                    )
                                    .map(|committed| {
                                        committed.map(|mut committed| {
                                            if let Some(entity_type_id) =
                                                plan.falling_block_entity_type_id
                                            {
                                                let air = air_state_id(&plan.blocks);
                                                let start_plan = plan_falling_block_starts(
                                                    &plan.blocks,
                                                    &plan.block_facts,
                                                    storage,
                                                    &committed.block.applied,
                                                    air,
                                                );
                                                let removal_edits = start_plan
                                                    .starts
                                                    .into_iter()
                                                    .map(|start| BlockEdit {
                                                        pos: start.pos,
                                                        new_state: air,
                                                    })
                                                    .collect::<Vec<_>>();
                                                if let Some(falling) =
                                                    apply_block_edit_batch_to_storage_conditionally(
                                                        storage,
                                                        block_light,
                                                        &removal_edits,
                                                        &start_plan.preconditions,
                                                    )
                                                {
                                                    for edit in &falling.applied {
                                                        if !is_falling_block_state(
                                                            &plan.blocks,
                                                            edit.previous,
                                                        ) {
                                                            continue;
                                                        }
                                                        committed.dispatches.extend(
                                                            sessions.spawn_falling_block_owned(
                                                                &self.authority,
                                                                entity_type_id,
                                                                Vec3::new(
                                                                    f64::from(edit.pos.x) + 0.5,
                                                                    f64::from(edit.pos.y),
                                                                    f64::from(edit.pos.z) + 0.5,
                                                                ),
                                                                edit.previous,
                                                            ),
                                                        );
                                                    }
                                                    append_block_edit_outcome(
                                                        &mut committed.block,
                                                        falling,
                                                    );
                                                }
                                            }
                                            for edit in &committed.block.applied {
                                                if is_campfire_block(&plan.blocks, edit.previous)
                                                    && !is_campfire_block(
                                                        &plan.blocks,
                                                        edit.new_state,
                                                    )
                                                    && sessions.clear_campfire_cooking(edit.pos)
                                                {
                                                    committed
                                                        .block
                                                        .cleared_campfires
                                                        .push(edit.pos);
                                                }
                                            }
                                            schedule_leaf_ticks_near_applied(
                                                storage,
                                                sessions.simulation_tick(),
                                                &committed.block.applied,
                                            );
                                            schedule_fluid_ticks_near_applied(
                                                storage,
                                                &plan.block_facts,
                                                sessions.simulation_tick(),
                                                &committed.block.applied,
                                            );
                                            if let Some(table) = block_light
                                                && let Some(light_updates) = prepare_owner_relight(
                                                    storage,
                                                    table,
                                                    &mut committed.block,
                                                    pending_relight.is_some(),
                                                )
                                            {
                                                let light_chunks = light_updates
                                                    .iter()
                                                    .map(|update| (update.pos.x, update.pos.z))
                                                    .collect::<HashSet<_>>();
                                                sessions.invalidate_prepared_chunks(&light_chunks);
                                                committed.block.precomputed_light_updates =
                                                    Some(light_updates);
                                            }
                                            sessions.invalidate_prepared_chunks(
                                                &committed.block.edit_chunks,
                                            );
                                            let recipients = sessions.loaded_recipients_for_chunks(
                                                &committed.block.edit_chunks,
                                                Some(command.actor_session),
                                            );
                                            let mut dispatches = recipients
                                                .iter()
                                                .cloned()
                                                .map(|recipient| VisibilityDispatch {
                                                    recipient,
                                                    command: OutboundCommand::BlockDeltas(
                                                        committed.block.deltas.clone(),
                                                    ),
                                                })
                                                .collect::<Vec<_>>();
                                            if let Some(light_updates) =
                                                committed.block.precomputed_light_updates.as_ref()
                                                && !light_updates.is_empty()
                                            {
                                                let light_chunks = light_updates
                                                    .iter()
                                                    .map(|update| (update.pos.x, update.pos.z))
                                                    .collect::<HashSet<_>>();
                                                dispatches.extend(
                                                    sessions
                                                        .loaded_recipients_for_chunks(
                                                            &light_chunks,
                                                            Some(command.actor_session),
                                                        )
                                                        .into_iter()
                                                        .map(|recipient| VisibilityDispatch {
                                                            recipient,
                                                            command: OutboundCommand::LightUpdates(
                                                                light_updates.clone(),
                                                            ),
                                                        }),
                                                );
                                            }
                                            dispatches.append(&mut committed.dispatches);
                                            dispatch_visibility_commands(dispatches);
                                            Box::new(committed)
                                        })
                                    }),
                            }
                        }
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::SurvivalBreak(result)
                }
                SimulationCommand::CommitSurvivalPlacement(command) => {
                    let result = if !valid_survival_placement_plan(&command.plan) {
                        Err(SimulationRequestError::InvalidCommand)
                    } else if let Some(storage) = storage.as_deref_mut() {
                        sessions
                            .commit_survival_placement(
                                &self.authority,
                                storage,
                                block_light,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|committed| {
                                committed.map(|mut committed| {
                                    schedule_leaf_ticks_near_applied(
                                        storage,
                                        sessions.simulation_tick(),
                                        &committed.block.applied,
                                    );
                                    schedule_fluid_ticks_near_applied(
                                        storage,
                                        &command.plan.block_facts,
                                        sessions.simulation_tick(),
                                        &committed.block.applied,
                                    );
                                    if let Some(table) = block_light
                                        && let Some(light_updates) = prepare_owner_relight(
                                            storage,
                                            table,
                                            &mut committed.block,
                                            pending_relight.is_some(),
                                        )
                                    {
                                        let light_chunks = light_updates
                                            .iter()
                                            .map(|update| (update.pos.x, update.pos.z))
                                            .collect::<HashSet<_>>();
                                        sessions.invalidate_prepared_chunks(&light_chunks);
                                        committed.block.precomputed_light_updates =
                                            Some(light_updates);
                                    }
                                    sessions
                                        .invalidate_prepared_chunks(&committed.block.edit_chunks);
                                    let recipients = sessions.loaded_recipients_for_chunks(
                                        &committed.block.edit_chunks,
                                        Some(command.actor_session),
                                    );
                                    let mut dispatches = recipients
                                        .iter()
                                        .cloned()
                                        .map(|recipient| VisibilityDispatch {
                                            recipient,
                                            command: OutboundCommand::BlockDeltas(
                                                committed.block.deltas.clone(),
                                            ),
                                        })
                                        .collect::<Vec<_>>();
                                    if let Some(light_updates) =
                                        committed.block.precomputed_light_updates.as_ref()
                                        && !light_updates.is_empty()
                                    {
                                        let light_chunks = light_updates
                                            .iter()
                                            .map(|update| (update.pos.x, update.pos.z))
                                            .collect::<HashSet<_>>();
                                        dispatches.extend(
                                            sessions
                                                .loaded_recipients_for_chunks(
                                                    &light_chunks,
                                                    Some(command.actor_session),
                                                )
                                                .into_iter()
                                                .map(|recipient| VisibilityDispatch {
                                                    recipient,
                                                    command: OutboundCommand::LightUpdates(
                                                        light_updates.clone(),
                                                    ),
                                                }),
                                        );
                                    }
                                    dispatch_visibility_commands(dispatches);
                                    Box::new(committed)
                                })
                            })
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::SurvivalPlacement(result)
                }
                SimulationCommand::CommitBucketUse(command) => {
                    let result = if !valid_bucket_use_plan(&command.plan) {
                        Err(SimulationRequestError::InvalidCommand)
                    } else if let Some(storage) = storage.as_deref_mut() {
                        sessions
                            .commit_bucket_use(
                                &self.authority,
                                storage,
                                block_light,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|committed| {
                                committed.map(|mut committed| {
                                    schedule_leaf_ticks_near_applied(
                                        storage,
                                        sessions.simulation_tick(),
                                        &committed.block.applied,
                                    );
                                    if command.plan.schedule_fluid_ticks {
                                        schedule_fluid_ticks_near_applied(
                                            storage,
                                            &command.plan.block_facts,
                                            sessions.simulation_tick(),
                                            &committed.block.applied,
                                        );
                                    }
                                    if let Some(table) = block_light
                                        && let Some(light_updates) = prepare_owner_relight(
                                            storage,
                                            table,
                                            &mut committed.block,
                                            pending_relight.is_some(),
                                        )
                                    {
                                        let light_chunks = light_updates
                                            .iter()
                                            .map(|update| (update.pos.x, update.pos.z))
                                            .collect::<HashSet<_>>();
                                        sessions.invalidate_prepared_chunks(&light_chunks);
                                        committed.block.precomputed_light_updates =
                                            Some(light_updates);
                                    }
                                    sessions
                                        .invalidate_prepared_chunks(&committed.block.edit_chunks);
                                    let mut dispatches = sessions
                                        .loaded_recipients_for_chunks(
                                            &committed.block.edit_chunks,
                                            Some(command.actor_session),
                                        )
                                        .into_iter()
                                        .map(|recipient| VisibilityDispatch {
                                            recipient,
                                            command: OutboundCommand::BlockDeltas(
                                                committed.block.deltas.clone(),
                                            ),
                                        })
                                        .collect::<Vec<_>>();
                                    if let Some(light_updates) =
                                        committed.block.precomputed_light_updates.as_ref()
                                        && !light_updates.is_empty()
                                    {
                                        let light_chunks = light_updates
                                            .iter()
                                            .map(|update| (update.pos.x, update.pos.z))
                                            .collect::<HashSet<_>>();
                                        dispatches.extend(
                                            sessions
                                                .loaded_recipients_for_chunks(
                                                    &light_chunks,
                                                    Some(command.actor_session),
                                                )
                                                .into_iter()
                                                .map(|recipient| VisibilityDispatch {
                                                    recipient,
                                                    command: OutboundCommand::LightUpdates(
                                                        light_updates.clone(),
                                                    ),
                                                }),
                                        );
                                    }
                                    dispatch_visibility_commands(dispatches);
                                    Box::new(committed)
                                })
                            })
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::BucketUse(result)
                }
                SimulationCommand::CommitFoodUse(command) => {
                    let result = if valid_food_use_plan(&command.plan) {
                        Ok(sessions
                            .commit_food_use(&self.authority, command.actor_session, &command.plan)
                            .map(Box::new))
                    } else {
                        Err(SimulationRequestError::InvalidCommand)
                    };
                    SimulationResponse::FoodUse(result)
                }
                SimulationCommand::CommitAnimalFeed(command) => {
                    let result = if valid_animal_feed_plan(&command.plan) {
                        Ok(sessions
                            .commit_animal_feed(
                                &self.authority,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|mut committed| {
                                dispatch_visibility_commands(std::mem::take(
                                    &mut committed.dispatches,
                                ));
                                Box::new(committed)
                            }))
                    } else {
                        Err(SimulationRequestError::InvalidCommand)
                    };
                    SimulationResponse::AnimalFeed(result)
                }
                SimulationCommand::CommitSheepShear(command) => {
                    let result = if valid_sheep_shear_plan(&command.plan) {
                        Ok(sessions
                            .commit_sheep_shear(
                                &self.authority,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|mut committed| {
                                dispatch_visibility_commands(std::mem::take(
                                    &mut committed.dispatches,
                                ));
                                Box::new(committed)
                            }))
                    } else {
                        Err(SimulationRequestError::InvalidCommand)
                    };
                    SimulationResponse::SheepShear(result)
                }
                SimulationCommand::CommitPlayerSurvival(command) => {
                    let result = if valid_player_survival_plan(&command.plan) {
                        Ok(sessions
                            .commit_player_survival(
                                &self.authority,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|outcome| {
                                Box::new(match outcome {
                                    PlayerSurvivalCommitOutcome::Committed(mut committed) => {
                                        dispatch_visibility_commands(std::mem::take(
                                            &mut committed.dispatches,
                                        ));
                                        PlayerSurvivalCommitOutcome::Committed(committed)
                                    }
                                    rejected => rejected,
                                })
                            }))
                    } else {
                        Err(SimulationRequestError::InvalidCommand)
                    };
                    SimulationResponse::PlayerSurvival(result)
                }
                SimulationCommand::CommitPlayerPose {
                    actor_session,
                    pose,
                    exhaustion,
                    ..
                } => {
                    let result = sessions
                        .commit_player_pose(&self.authority, *actor_session, *pose, *exhaustion)
                        .map(|(dispatches, committed)| {
                            dispatch_visibility_commands(dispatches);
                            committed
                        });
                    SimulationResponse::PlayerPose(result)
                }
                SimulationCommand::CommitPlayerStateEvent {
                    actor_session,
                    event,
                } => {
                    let result = sessions
                        .commit_player_state_event(&self.authority, *actor_session, *event)
                        .map(dispatch_visibility_commands);
                    SimulationResponse::PlayerStateEvent(result)
                }
                SimulationCommand::CommitPlayerInventory {
                    actor_session,
                    player,
                } => {
                    let mut result = if valid_container_player_plan(player) {
                        sessions
                            .commit_player_inventory(&self.authority, *actor_session, player)
                            .map_err(|error| match error {
                                PlayerInventoryCommitError::MissingPlayer => {
                                    SimulationRequestError::StaleSession
                                }
                            })
                    } else {
                        Err(SimulationRequestError::InvalidCommand)
                    };
                    if let Ok(PlayerInventoryCommitOutcome::Committed { dispatches, .. }) =
                        &mut result
                    {
                        dispatch_visibility_commands(std::mem::take(dispatches));
                    }
                    SimulationResponse::PlayerInventory(Box::new(result))
                }
                SimulationCommand::CommitBowRelease(command) => {
                    let result = if valid_bow_release_plan(&command.plan) {
                        Ok(sessions
                            .commit_bow_release(
                                &self.authority,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|mut committed| {
                                dispatch_visibility_commands(std::mem::take(
                                    &mut committed.dispatches,
                                ));
                                Box::new(committed)
                            }))
                    } else {
                        Err(SimulationRequestError::InvalidCommand)
                    };
                    SimulationResponse::BowRelease(result)
                }
                SimulationCommand::CommitSelectedItemDrop(command) => {
                    let result = if valid_selected_item_drop_plan(&command.plan) {
                        Ok(sessions
                            .commit_selected_item_drop(
                                &self.authority,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|mut committed| {
                                dispatch_visibility_commands(std::mem::take(
                                    &mut committed.dispatches,
                                ));
                                Box::new(committed)
                            }))
                    } else {
                        Err(SimulationRequestError::InvalidCommand)
                    };
                    SimulationResponse::SelectedItemDrop(result)
                }
                SimulationCommand::CommitChest {
                    primary_position,
                    positions,
                    expected_state_id,
                    actor_session,
                    expected,
                    updated,
                    player,
                } => {
                    let mut result = self.commit_chest_command(
                        sessions,
                        storage.as_deref_mut(),
                        world_error,
                        ChestCommitRequest {
                            primary_position: *primary_position,
                            positions,
                            expected_state_id: *expected_state_id,
                            actor_session: *actor_session,
                            expected,
                            updated,
                            player,
                        },
                    );
                    if let Ok(outcome) = &mut result
                        && let SharedContainerCommit::Committed { dispatches, .. } =
                            outcome.as_mut()
                    {
                        dispatch_visibility_commands(std::mem::take(dispatches));
                    }
                    SimulationResponse::ChestCommit(result)
                }
                SimulationCommand::CommitFurnace {
                    position,
                    expected_state_id,
                    actor_session,
                    expected,
                    updated,
                    player,
                } => {
                    let mut result = self.commit_furnace_command(
                        sessions,
                        storage.as_deref_mut(),
                        world_error,
                        FurnaceCommitRequest {
                            position: *position,
                            expected_state_id: *expected_state_id,
                            actor_session: *actor_session,
                            expected,
                            updated,
                            player,
                        },
                    );
                    if let Ok(outcome) = &mut result
                        && let SharedContainerCommit::Committed { dispatches, .. } =
                            outcome.as_mut()
                    {
                        dispatch_visibility_commands(std::mem::take(dispatches));
                    }
                    SimulationResponse::FurnaceCommit(result)
                }
                SimulationCommand::CommitOpaqueBlockEntity {
                    position,
                    expected_state,
                    expected_token,
                    bytes,
                } => {
                    SimulationResponse::OpaqueBlockEntity(self.commit_opaque_block_entity_command(
                        storage.as_deref_mut(),
                        world_error,
                        *position,
                        *expected_state,
                        *expected_token,
                        bytes.clone(),
                    ))
                }
                SimulationCommand::CommitCampfireUse(command) => {
                    let result = if !valid_campfire_use_plan(&command.plan) {
                        Err(SimulationRequestError::InvalidCommand)
                    } else if let Some(storage) = storage.as_deref_mut() {
                        sessions
                            .commit_campfire_use(
                                &self.authority,
                                storage,
                                command.actor_session,
                                &command.plan,
                            )
                            .map(|committed| {
                                committed.map(|committed| {
                                    dispatch_visibility_commands(
                                        sessions.block_entity_data_dispatches(
                                            command.plan.position,
                                            Some(command.actor_session),
                                            CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
                                            command.plan.client_nbt.clone(),
                                        ),
                                    );
                                    Box::new(committed)
                                })
                            })
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::CampfireUse(result)
                }
                SimulationCommand::CommitTntIgnition {
                    actor_session,
                    plan,
                } => {
                    let result = if let Some(storage) = storage.as_deref_mut() {
                        sessions
                            .commit_tnt_ignition(
                                &self.authority,
                                storage,
                                block_light,
                                *actor_session,
                                plan,
                            )
                            .map(|committed| {
                                committed.map(|mut committed| {
                                    if let Some(table) = block_light
                                        && let Some(light_updates) = prepare_owner_relight(
                                            storage,
                                            table,
                                            &mut committed.block,
                                            pending_relight.is_some(),
                                        )
                                    {
                                        committed.block.precomputed_light_updates =
                                            Some(light_updates);
                                    }
                                    sessions
                                        .invalidate_prepared_chunks(&committed.block.edit_chunks);
                                    let mut dispatches = sessions
                                        .loaded_recipients_for_chunks(
                                            &committed.block.edit_chunks,
                                            Some(*actor_session),
                                        )
                                        .into_iter()
                                        .map(|recipient| VisibilityDispatch {
                                            recipient,
                                            command: OutboundCommand::BlockDeltas(
                                                committed.block.deltas.clone(),
                                            ),
                                        })
                                        .collect::<Vec<_>>();
                                    dispatches.append(&mut committed.dispatches);
                                    dispatch_visibility_commands(dispatches);
                                    Box::new(committed)
                                })
                            })
                    } else {
                        self.record_world_access_error(world_error);
                        Err(world_error)
                    };
                    SimulationResponse::TntIgnition(result)
                }
            };
            processed += 1;
            self.metrics.processed.fetch_add(1, Ordering::Relaxed);
            if item_pickup {
                self.metrics
                    .item_pickups_processed
                    .fetch_add(1, Ordering::Relaxed);
            }
            if block_edit {
                self.metrics
                    .block_edits_processed
                    .fetch_add(1, Ordering::Relaxed);
            }
            if container_commit {
                self.metrics
                    .container_commits_processed
                    .fetch_add(1, Ordering::Relaxed);
            }
            if block_entity_commit {
                self.metrics
                    .block_entity_commits_processed
                    .fetch_add(1, Ordering::Relaxed);
            }
            let pending_sources = response_block_edit_outcome_mut(&mut response)
                .and_then(|outcome| outcome.pending_light_sources.take());
            if let Some(sources) = pending_sources {
                let actor_session = command_relight_actor_session(&envelope.command)
                    .expect("pending relight command has an actor session");
                let slot = pending_relight
                    .as_deref_mut()
                    .expect("pending relight slot exists when compute is deferred");
                debug_assert!(slot.is_none());
                *slot = Some(PendingOwnerRelight {
                    envelope,
                    response,
                    actor_session,
                    sources,
                });
            } else {
                envelope.respond(Ok(response));
            }
        }
        if let Some(regional) = regional_batch {
            let mut lanes = self
                .region_ownership
                .leases()
                .map(|lease| lease.lane)
                .collect::<Vec<_>>();
            lanes.sort_unstable();
            lanes.dedup();
            let completion = lanes
                .into_iter()
                .try_for_each(|lane| self.region_ownership.acknowledge_lane(regional.phase, lane));
            if let Err(error) =
                completion.and_then(|()| self.region_ownership.finish_phase(regional.phase))
            {
                warn!(?error, "simulation regional phase completion failed");
            }
        }
        SimulationTickReport {
            processed,
            remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
            ..SimulationTickReport::default()
        }
    }
}

fn valid_survival_block_break_plan(plan: &SurvivalBlockBreakPlan) -> bool {
    plan.held.hotbar_slot <= 8
        && plan.held.max_damage.is_none_or(|max_damage| max_damage > 0)
        && plan
            .item_entity_type_id
            .is_none_or(|entity_type_id| entity_type_id >= 0)
        && plan
            .falling_block_entity_type_id
            .is_none_or(|entity_type_id| entity_type_id >= 0)
        && plan.blocks.by_id(plan.expected_target.state).is_some()
}

fn prepare_survival_block_break_plan(
    storage: &impl super::BlockPlanningRead,
    request: &SurvivalBlockBreakPlan,
) -> Option<SurvivalBreakPlan> {
    let previous = storage.get_cached_block(request.position)?;
    if previous != request.expected_target.state
        || storage.block_mutation_token(request.position) != Some(request.expected_target.token)
        || super::block_break_is_denied(&request.blocks, previous)
    {
        return None;
    }

    let air = air_state_id(&request.blocks);
    let replacement = super::break_replacement_state_in_storage(
        &request.blocks,
        &request.block_facts,
        request.water,
        storage,
        request.position,
        air,
    );
    let edits = super::plan_break_block_edits(
        &request.blocks,
        storage,
        request.position,
        previous,
        replacement,
        air,
    );
    let preconditions = super::plan_break_edit_preconditions(
        &request.blocks,
        storage,
        &edits,
        request.position,
        request.expected_target,
    )?;
    let drops = if request.drop_items {
        super::plan_survival_break_drops(request, &edits, &preconditions, air)
    } else {
        Vec::new()
    };

    Some(SurvivalBreakPlan {
        edits,
        preconditions,
        blocks: Arc::clone(&request.blocks),
        block_facts: Arc::clone(&request.block_facts),
        falling_block_entity_type_id: request.falling_block_entity_type_id,
        held: request.held.clone(),
        drops,
    })
}

fn valid_survival_break_plan(plan: &SurvivalBreakPlan) -> bool {
    if plan.edits.is_empty()
        || plan.edits.len() > MAX_SURVIVAL_BREAK_EDITS
        || plan.preconditions.is_empty()
        || plan.preconditions.len() > MAX_SURVIVAL_BREAK_EDITS
        || plan.drops.len() > MAX_SURVIVAL_BREAK_DROPS
        || plan
            .falling_block_entity_type_id
            .is_some_and(|entity_type_id| entity_type_id < 0)
        || plan.held.hotbar_slot > 8
        || plan
            .held
            .max_damage
            .is_some_and(|max_damage| max_damage <= 0)
        || plan.drops.iter().any(|drop| !valid_block_drop(drop))
    {
        return false;
    }
    let mut edit_positions = HashSet::with_capacity(plan.edits.len());
    if !plan
        .edits
        .iter()
        .all(|edit| edit_positions.insert(edit.pos))
    {
        return false;
    }
    let mut precondition_positions = HashSet::with_capacity(plan.preconditions.len());
    if !plan
        .preconditions
        .iter()
        .all(|precondition| precondition_positions.insert(precondition.pos))
    {
        return false;
    }
    plan.edits.iter().all(|edit| {
        plan.preconditions.iter().any(|precondition| {
            precondition.pos == edit.pos && precondition.expected_state != edit.new_state
        })
    })
}

fn valid_block_drop_command(
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    drops: &[SurvivalBreakDrop],
) -> bool {
    if edits.is_empty()
        || edits.len() > MAX_SURVIVAL_BREAK_EDITS
        || preconditions.len() != edits.len()
        || drops.is_empty()
        || drops.len() > MAX_SURVIVAL_BREAK_DROPS
        || drops.iter().any(|drop| !valid_block_drop(drop))
    {
        return false;
    }
    let mut positions = HashSet::with_capacity(edits.len());
    edits.iter().zip(preconditions).all(|(edit, precondition)| {
        edit.pos == precondition.pos
            && edit.new_state != precondition.expected_state
            && positions.insert(edit.pos)
    })
}

fn valid_block_drop(drop: &SurvivalBreakDrop) -> bool {
    drop.entity_type_id >= 0
        && !drop.stack.is_empty()
        && drop.position.x.is_finite()
        && drop.position.y.is_finite()
        && drop.position.z.is_finite()
}

fn valid_survival_placement_plan(plan: &SurvivalPlacementPlan) -> bool {
    if plan.edits.is_empty()
        || plan.edits.len() > MAX_SURVIVAL_BREAK_EDITS
        || plan.preconditions.is_empty()
        || plan.preconditions.len() > MAX_SURVIVAL_BREAK_EDITS
        || plan.scheduled_block_ticks.len() > MAX_SURVIVAL_BREAK_EDITS
        || !matches!(
            plan.held.inventory_slot,
            PlayerInventory::HOTBAR_BASE..=PlayerInventory::OFFHAND_SLOT
        )
        || plan.held.expected.is_empty()
    {
        return false;
    }
    let mut edit_positions = HashSet::with_capacity(plan.edits.len());
    if !plan
        .edits
        .iter()
        .all(|edit| edit_positions.insert(edit.pos))
    {
        return false;
    }
    let mut precondition_positions = HashSet::with_capacity(plan.preconditions.len());
    if !plan
        .preconditions
        .iter()
        .all(|precondition| precondition_positions.insert(precondition.pos))
    {
        return false;
    }
    plan.edits.iter().all(|edit| {
        plan.preconditions.iter().any(|precondition| {
            precondition.pos == edit.pos && precondition.expected_state != edit.new_state
        })
    }) && plan
        .scheduled_block_ticks
        .iter()
        .all(|tick| edit_positions.contains(&tick.pos))
}

fn valid_bucket_use_plan(plan: &BucketUsePlan) -> bool {
    plan.edit.pos == plan.precondition.pos
        && plan.edit.new_state != plan.precondition.expected_state
        && plan.inventory.as_ref().is_none_or(|inventory| {
            inventory.held_slot < 46
                && !inventory.expected_held.is_empty()
                && inventory.replacement_max_stack > 0
        })
}

fn valid_campfire_use_plan(plan: &CampfireUsePlan) -> bool {
    plan.held_slot < 46
        && !plan.expected_held.is_empty()
        && plan.expected_cooking != plan.updated_cooking
        && !plan.persistent_bytes.is_empty()
}

fn valid_food_use_plan(plan: &FoodUsePlan) -> bool {
    (PlayerInventory::HOTBAR_BASE..=PlayerInventory::OFFHAND_SLOT).contains(&plan.held_slot)
        && !plan.expected_held.is_empty()
        && plan.food > 0
        && plan.saturation.is_finite()
        && plan.saturation >= 0.0
}

fn valid_animal_feed_plan(plan: &AnimalFeedPlan) -> bool {
    plan.held_slot < 46
        && !plan.expected_held.is_empty()
        && plan.expected_held.item_id == plan.food_item_id
        && !plan.targets.is_empty()
}

fn valid_sheep_shear_plan(plan: &SheepShearPlan) -> bool {
    plan.held_slot < 46
        && plan.expected_held.item_id == plan.shears_item_id
        && plan.expected_held.count == 1
        && plan.shears_max_damage > 0
        && plan.item_entity_type_id >= 0
        && plan.wool_item_ids.iter().all(|item_id| *item_id > 0)
}

fn valid_player_survival_plan(plan: &PlayerSurvivalPlan) -> bool {
    let survival_is_valid = |state: SurvivalState| {
        state.health.is_finite()
            && state.saturation.is_finite()
            && state.exhaustion.is_finite()
            && (0.0..=SurvivalState::MAX_HEALTH).contains(&state.health)
            && (0..=SurvivalState::MAX_FOOD).contains(&state.food)
            && state.saturation >= 0.0
            && state.exhaustion >= 0.0
    };
    let position_is_valid =
        plan.position.x.is_finite() && plan.position.y.is_finite() && plan.position.z.is_finite();
    let dies = !plan.expected_survival.is_dead() && plan.updated_survival.is_dead();
    let has_drops = plan.updated_inventory.slots[1..]
        .iter()
        .any(|stack| !stack.is_empty())
        || !plan.expected_carried_item.is_empty();
    let dropped_xp = plan.updated_xp.level.saturating_mul(7).clamp(0, 100);
    let xp_is_valid = |xp: &super::persistence::XpState| {
        xp.level >= 0
            && xp.total >= 0
            && xp.progress.is_finite()
            && (0.0..=1.0).contains(&xp.progress)
    };

    survival_is_valid(plan.expected_survival)
        && survival_is_valid(plan.updated_survival)
        && xp_is_valid(&plan.expected_xp)
        && xp_is_valid(&plan.updated_xp)
        && position_is_valid
        && (!dies
            || ((!has_drops || plan.item_entity_type_id.is_some_and(|id| id >= 0))
                && (dropped_xp == 0 || plan.xp_orb_entity_type_id.is_some_and(|id| id >= 0))))
}

fn valid_bow_release_plan(plan: &BowReleasePlan) -> bool {
    let bow_is_in_hand = plan.bow_slot == PlayerInventory::OFFHAND_SLOT
        || (PlayerInventory::HOTBAR_BASE..PlayerInventory::OFFHAND_SLOT).contains(&plan.bow_slot);
    bow_is_in_hand
        && (9..=PlayerInventory::OFFHAND_SLOT).contains(&plan.arrow_slot)
        && plan.arrow_slot != plan.bow_slot
        && !plan.expected_bow.is_empty()
        && plan.expected_bow.count == 1
        && !plan.expected_arrow.is_empty()
        && plan.expected_arrow.count > 0
        && plan.bow_max_damage > 0
        && plan.entity_type_id >= 0
        && plan.position.x.is_finite()
        && plan.position.y.is_finite()
        && plan.position.z.is_finite()
        && plan.velocity.x.is_finite()
        && plan.velocity.y.is_finite()
        && plan.velocity.z.is_finite()
        && (plan.velocity.x * plan.velocity.x
            + plan.velocity.y * plan.velocity.y
            + plan.velocity.z * plan.velocity.z)
            > 0.0
        && plan.rotation.yaw.is_finite()
        && plan.rotation.pitch.is_finite()
        && plan.rotation.head_yaw.is_finite()
}

fn valid_selected_item_drop_plan(plan: &SelectedItemDropPlan) -> bool {
    plan.held_hotbar_slot <= 8
        && !plan.expected_held.is_empty()
        && plan.drop_count > 0
        && plan.drop_count <= plan.expected_held.count
        && plan.entity_type_id >= 0
        && plan.position.x.is_finite()
        && plan.position.y.is_finite()
        && plan.position.z.is_finite()
}

fn valid_container_player_plan(plan: &ContainerPlayerPlan) -> bool {
    plan.drops.len() <= super::MAX_CONTAINER_PLAYER_DROPS
        && plan.drops.iter().all(|drop| {
            drop.entity_type_id >= 0
                && !drop.stack.is_empty()
                && drop.position.x.is_finite()
                && drop.position.y.is_finite()
                && drop.position.z.is_finite()
        })
        && plan.xp_orb.as_ref().is_none_or(|xp_orb| {
            xp_orb.entity_type_id >= 0
                && xp_orb.value > 0
                && xp_orb.position.x.is_finite()
                && xp_orb.position.y.is_finite()
                && xp_orb.position.z.is_finite()
        })
}

fn valid_furnace_commit_command(
    expected: &FurnaceBlockEntity,
    updated: &FurnaceBlockEntity,
    player: &ContainerPlayerPlan,
) -> bool {
    let output_was_taken = furnace_output_was_taken(expected, updated);
    valid_container_player_plan(player)
        && expected.burn_remaining == updated.burn_remaining
        && expected.burn_total == updated.burn_total
        && expected.cook_progress == updated.cook_progress
        && expected.cook_total == updated.cook_total
        && (output_was_taken || expected.recipes_used == updated.recipes_used)
        && (output_was_taken || player.xp_orb.is_none())
        && (!output_was_taken || updated.recipes_used.is_empty())
        && (player.xp_orb.is_none() || !expected.recipes_used.is_empty())
}

fn valid_block_edit_command(
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> bool {
    if edits.len() > MAX_BLOCK_EDIT_COMMAND_EDITS
        || preconditions.len() > MAX_BLOCK_EDIT_COMMAND_EDITS
        || scheduled_block_ticks.len() > MAX_BLOCK_EDIT_COMMAND_EDITS
    {
        return false;
    }
    let edit_positions = edits.iter().map(|edit| edit.pos).collect::<HashSet<_>>();
    scheduled_block_ticks
        .iter()
        .all(|tick| edit_positions.contains(&tick.pos))
}

#[cfg(test)]
#[path = "simulation/explosion_load_tests.rs"]
mod explosion_load_tests;

#[cfg(test)]
mod tests {
    use super::super::{
        EntityPhysicsStep, GameMode, HOSTILE_MELEE_PERIOD_TICKS, ITEM_PICKUP_DELAY_TICKS,
        PlayerPose, SKELETON_SHOT_PERIOD_TICKS, SessionRegistry, SurvivalState,
    };
    use super::*;
    use crate::login::LoggedInProfile;
    use crate::play::inventory::PlayerInventory;
    use crate::play::persistence::{PlayerPersistedState, XpState};
    use crate::play::session::OutboundCommand;
    use mc_data::Identifier;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_data::items::ItemRegistry;
    use mc_entity::{EntityItemStack, Rotation, Vec3};
    use mc_protocol::packets::play::ItemStack;
    use mc_script::{ScriptAxisAlignedZone, ScriptPosition};
    use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Mutex;

    struct FailOnceEntityCommitJournal {
        failure: Option<mc_entity::RegionalDecisionJournalError>,
        commits: Arc<AtomicUsize>,
    }

    impl mc_entity::RegionalDecisionJournal for FailOnceEntityCommitJournal {
        fn record_commit(
            &mut self,
            _decision: &mc_entity::RegionalCommitDecision,
        ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
            self.commits.fetch_add(1, Ordering::Relaxed);
            match self.failure.take() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }

        fn clear_commit(
            &mut self,
            _phase: mc_entity::RegionPhase,
        ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
            Ok(())
        }
    }

    async fn assert_request_enqueued<F>(
        mut request: std::pin::Pin<&mut F>,
        handle: &SimulationHandle,
    ) where
        F: std::future::Future,
    {
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(request.as_mut(), cx).is_pending(),
                "request must wait for the simulation owner response"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(handle.snapshot().depth, 1, "request must be enqueued");
    }

    fn seed_claim_entities(registry: &SessionRegistry) -> (EntityId, EntityId) {
        let position = Vec3::new(0.5, 64.0, 0.5);
        registry.spawn_item_drop(1, position, EntityItemStack::new(42, 3));
        registry.spawn_xp_orb(2, position, 5);
        registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
        let item = registry.nearby_item_entities(position, 2.25)[0].id;
        let experience = registry.nearby_experience_entities(position, 2.25)[0].id;
        (item, experience)
    }

    fn publish_entity_spawns(
        dispatches: Vec<VisibilityDispatch>,
        outbound: &mut mpsc::Receiver<OutboundCommand>,
    ) -> Vec<EntityId> {
        let entity_ids = dispatches
            .iter()
            .filter_map(|dispatch| match &dispatch.command {
                OutboundCommand::SpawnEntity(entity) => Some(entity.id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(entity_ids.len(), dispatches.len());
        dispatch_visibility_commands(dispatches);
        for expected in &entity_ids {
            assert!(matches!(
                outbound.try_recv(),
                Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == *expected
            ));
        }
        entity_ids
    }

    fn seed_claim_entities_published(
        registry: &SessionRegistry,
        outbound: &mut mpsc::Receiver<OutboundCommand>,
    ) -> (EntityId, EntityId) {
        let position = Vec3::new(0.5, 64.0, 0.5);
        dispatch_visibility_commands(registry.spawn_item_drop(
            1,
            position,
            EntityItemStack::new(42, 3),
        ));
        dispatch_visibility_commands(registry.spawn_xp_orb(2, position, 5));
        registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
        let item = registry.nearby_item_entities(position, 2.25)[0].id;
        let experience = registry.nearby_experience_entities(position, 2.25)[0].id;
        let mut published_spawns = 0;
        while let Ok(command) = outbound.try_recv() {
            if matches!(command, OutboundCommand::SpawnEntity(_)) {
                published_spawns += 1;
            }
        }
        assert_eq!(published_spawns, 2);
        (item, experience)
    }

    fn register_test_session(registry: &SessionRegistry, name: &str) -> SessionId {
        register_test_session_with_outbound(registry, name).0
    }

    fn register_test_session_with_outbound(
        registry: &SessionRegistry,
        name: &str,
    ) -> (SessionId, mpsc::Receiver<OutboundCommand>) {
        register_test_session_at_with_outbound(registry, name, PlayerPose::new(0.5, 64.0, 0.5))
    }

    fn register_test_session_at_with_outbound(
        registry: &SessionRegistry,
        name: &str,
        pose: PlayerPose,
    ) -> (SessionId, mpsc::Receiver<OutboundCommand>) {
        let profile = crate::login::LoggedInProfile {
            uuid: crate::login::offline_uuid(name),
            name: name.to_owned(),
        };
        let (tx, rx) = mpsc::channel(8);
        let session_id = registry
            .register(&profile, (0, 0), 2, HashSet::new(), tx, pose)
            .0;
        (session_id, rx)
    }

    fn register_test_player_state(
        registry: &SessionRegistry,
        session_id: SessionId,
        inventory: PlayerInventory,
    ) -> Arc<Mutex<PlayerPersistedState>> {
        let mut state = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
        state.inventory = inventory;
        let state = Arc::new(Mutex::new(state));
        registry.register_player_persistence(session_id, Arc::clone(&state));
        state
    }

    fn empty_container_player_plan() -> ContainerPlayerPlan {
        let inventory = PlayerInventory::empty();
        ContainerPlayerPlan {
            expected_inventory: inventory.clone(),
            expected_carried_item: ItemStack::EMPTY,
            updated_inventory: inventory,
            updated_carried_item: ItemStack::EMPTY,
            crafting_table_input: None,
            enchanting_table_input: None,
            drops: Vec::new(),
            xp_orb: None,
        }
    }

    fn test_survival_break_plan(
        pos: BlockPos,
        token: BlockMutationToken,
        tool_item_id: u32,
        drop_item_id: u32,
    ) -> SurvivalBreakPlan {
        SurvivalBreakPlan {
            edits: vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            blocks: Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
                &test_block_reports(),
            )),
            falling_block_entity_type_id: Some(99),
            held: SurvivalBreakHeldItem {
                hotbar_slot: 0,
                expected: ItemStack::new(tool_item_id, 1),
                max_damage: Some(10),
            },
            drops: vec![SurvivalBreakDrop {
                entity_type_id: 1,
                position: Vec3::new(0.5, 64.5, 0.5),
                stack: EntityItemStack::new(drop_item_id, 1),
            }],
        }
    }

    fn test_survival_block_break_plan(
        pos: BlockPos,
        token: BlockMutationToken,
    ) -> SurvivalBlockBreakPlan {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let items = Arc::new(ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:wooden_pickaxe").unwrap(),
                protocol_id: 42,
            },
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:cobblestone").unwrap(),
                protocol_id: 7,
            },
        ]));
        SurvivalBlockBreakPlan {
            position: pos,
            expected_target: BlockMutationSnapshot {
                state: BlockStateId(1),
                token,
            },
            blocks,
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
                &test_block_reports(),
            )),
            water: Some(BlockStateId(2)),
            items,
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            item_entity_type_id: Some(1),
            falling_block_entity_type_id: Some(99),
            held: SurvivalBreakHeldItem {
                hotbar_slot: 0,
                expected: ItemStack::new(42, 1),
                max_damage: Some(10),
            },
            drop_items: true,
        }
    }

    fn test_survival_placement_plan(
        target: BlockPos,
        target_token: BlockMutationToken,
        support: BlockPos,
        support_token: BlockMutationToken,
        item_id: u32,
        count: i32,
    ) -> SurvivalPlacementPlan {
        SurvivalPlacementPlan {
            edits: vec![BlockEdit {
                pos: target,
                new_state: BlockStateId(1),
            }],
            preconditions: vec![
                BlockEditPrecondition {
                    pos: target,
                    expected_state: BlockStateId(0),
                    expected_token: target_token,
                },
                BlockEditPrecondition {
                    pos: support,
                    expected_state: BlockStateId(1),
                    expected_token: support_token,
                },
            ],
            scheduled_block_ticks: Vec::new(),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
                &test_block_reports(),
            )),
            held: SurvivalPlacementHeldItem {
                inventory_slot: PlayerInventory::HOTBAR_BASE,
                expected: ItemStack::new(item_id, count),
            },
            expected_game_mode: GameMode::Survival,
        }
    }

    fn test_bucket_use_plan(target: BlockPos, target_token: BlockMutationToken) -> BucketUsePlan {
        BucketUsePlan {
            edit: BlockEdit {
                pos: target,
                new_state: BlockStateId(2),
            },
            precondition: BlockEditPrecondition {
                pos: target,
                expected_state: BlockStateId(0),
                expected_token: target_token,
            },
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
                &test_block_reports(),
            )),
            inventory: Some(BucketInventoryChange {
                held_slot: PlayerInventory::HOTBAR_BASE,
                expected_held: ItemStack::new(61, 1),
                replacement_item: 60,
                replacement_max_stack: 16,
            }),
            schedule_fluid_ticks: true,
        }
    }

    fn persisted_item_drop_count(registry: &SessionRegistry) -> usize {
        registry
            .persisted_entity_records()
            .into_iter()
            .filter(|record| record.snapshot.item_stack.is_some())
            .count()
    }

    fn claim_xp(entity_id: i32, collector_session: SessionId) -> SimulationCommand {
        SimulationCommand::ClaimExperiencePickup {
            entity_id: EntityId(entity_id),
            collector_session,
        }
    }

    fn seed_grounded_arrow(registry: &SessionRegistry) -> EntityId {
        registry.spawn_arrow_for_test(
            None,
            3,
            Vec3::new(0.5, 64.0, 0.5),
            Vec3::new(0.0, 0.0, 1.0),
            Rotation::ZERO,
        );
        let entity_id = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.type_name == "minecraft:arrow")
            .expect("spawned arrow record")
            .snapshot
            .id;
        registry.apply_entity_physics_and_dispatch(
            1,
            &[EntityPhysicsStep {
                id: entity_id,
                position: Vec3::new(0.5, 64.0, 0.5),
                velocity: Vec3::ZERO,
                on_ground: true,
                horizontal_collision: false,
            }],
        );
        entity_id
    }

    fn seed_attack_target(registry: &SessionRegistry) -> EntityId {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:zombie".to_owned(),
            Vec3::new(1.5, 64.0, 0.5),
        );
        registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.type_name == "minecraft:zombie")
            .expect("spawned attack target")
            .snapshot
            .id
    }

    fn block_report(id: &str, state_id: u32) -> BlockReport {
        BlockReport {
            id: Identifier::parse(id).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state_id,
                default: true,
                properties: BTreeMap::new(),
            }],
        }
    }

    fn fluid_block_report(id: &str, state_id: u32) -> BlockReport {
        let mut properties = BTreeMap::new();
        properties.insert("level".to_owned(), vec!["0".to_owned()]);
        let mut state_properties = BTreeMap::new();
        state_properties.insert("level".to_owned(), "0".to_owned());
        BlockReport {
            id: Identifier::parse(id).unwrap(),
            properties,
            states: vec![BlockStateReport {
                id: state_id,
                default: true,
                properties: state_properties,
            }],
        }
    }

    fn test_block_reports() -> Vec<BlockReport> {
        vec![
            block_report("minecraft:air", 0),
            block_report("minecraft:stone", 1),
            fluid_block_report("minecraft:water", 2),
            block_report("minecraft:sand", 3),
            block_report("minecraft:campfire", 4),
            block_report("minecraft:tnt", 5),
            block_report("minecraft:dirt", 6),
        ]
    }

    fn test_block_storage() -> (WorldStorage, BlockPos, mc_world::BlockMutationToken) {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        seed_test_block_storage(WorldStorage::in_memory(blocks))
    }

    fn test_block_storage_with_capacity(
        capacity: usize,
    ) -> (WorldStorage, BlockPos, mc_world::BlockMutationToken) {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        seed_test_block_storage(WorldStorage::in_memory_with_capacity(blocks, capacity))
    }

    fn seed_test_block_storage(
        mut storage: WorldStorage,
    ) -> (WorldStorage, BlockPos, mc_world::BlockMutationToken) {
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let pos = BlockPos { x: 1, y: 64, z: 1 };
        assert_eq!(
            storage.set_block_at(pos, BlockStateId(1)).unwrap(),
            Some(BlockStateId(0))
        );
        let token = storage
            .block_mutation_token(pos)
            .expect("resident block mutation token");
        (storage, pos, token)
    }

    fn test_container_storage() -> (WorldStorage, BlockPos) {
        let (storage, pos, _) = test_block_storage();
        (storage, pos)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn block_snapshot_query_runs_through_simulation_owner() {
        let (storage, position, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "BlockSnapshotReader");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.read_block_snapshot(position));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(
            owner
                .process_commands_with_world(&registry, Some(&world), None, 1)
                .await
                .processed,
            1
        );
        assert_eq!(
            request.await.unwrap(),
            Some(BlockMutationSnapshot {
                state: BlockStateId(1),
                token,
            })
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resident_block_snapshot_does_not_wait_for_world_writer() {
        let (storage, position, token) = test_block_storage();
        let read_view = storage.read_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let session = register_test_session(&registry, "ResidentBlockSnapshotReader");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.read_block_snapshot(position));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_registry = Arc::clone(&registry);
        let owner_read_view = read_view.clone();
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_registry,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&owner_read_view),
                        ..SimulationWorldAccess::default()
                    },
                    None,
                    1,
                )
                .await
        });
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request).await;
        drop(writer);

        assert_eq!(
            outcome.expect("resident read completes while storage writer is held"),
            Ok(Some(BlockMutationSnapshot {
                state: BlockStateId(1),
                token,
            }))
        );
        assert_eq!(owner_task.await.expect("owner task").processed, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resident_block_edit_does_not_wait_for_world_writer() {
        let (storage, position, token) = test_block_storage();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let light = BlockLightTable::from_arrays(
            "resident inert edit",
            vec![0, 0, 0, 0, 0],
            vec![0, 0, 0, 0, 0],
            vec![true, true, true, true, true],
        );
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let session = register_test_session(&registry, "ResidentBlockEditor");
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_registry = Arc::clone(&registry);
        let owner_read_view = read_view.clone();
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_registry,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&owner_read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    Some(&light),
                    1,
                )
                .await
        });
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .expect("resident mutation completion event");
        drop(writer);

        let outcome = outcome.expect("resident mutation response");
        assert_eq!(outcome.unwrap().applied.len(), 1);
        assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
        assert_eq!(owner_task.await.expect("owner task").processed, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn regional_block_edit_is_durable_before_response_publication() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let items = Arc::new(ItemRegistry::from_report(&[]));
        let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        let chunk_position = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk_position,
                Chunk::empty(
                    chunk_position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let position = BlockPos { x: 1, y: 64, z: 1 };
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        let token = storage.block_mutation_token(position).unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let (journal, pending) = super::super::world_journal::WorldChunkJournal::open(
            temp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        assert!(pending.is_empty());
        sessions.install_world_chunk_journal(journal);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .enqueue(SimulationCommand::ApplyBlockEdits {
                actor_session: 0,
                edits: vec![BlockEdit {
                    pos: position,
                    new_state: BlockStateId(0),
                }],
                preconditions: vec![BlockEditPrecondition {
                    pos: position,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                }],
                scheduled_block_ticks: Vec::new(),
            })
            .unwrap();

        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &sessions,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
                .processed,
            1
        );
        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert!(!world.lock().await.plan_dirty_flush().unwrap().is_empty());

        let (reopened, pending) =
            super::super::world_journal::WorldChunkJournal::open(temp.path(), blocks, items)
                .unwrap();
        let restored = reopened.decode_pending(&pending).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(
            restored[0].get_block(1, 64, 1),
            Some(BlockStateId(0)),
            "response publication requires the post-mutation chunk image to be durable"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn journaled_block_edit_does_not_capture_following_non_journaled_mutation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let items = Arc::new(ItemRegistry::from_report(&[]));
        let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        let chunk_position = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk_position,
                Chunk::empty(
                    chunk_position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let block_position = BlockPos { x: 1, y: 64, z: 1 };
        let entity_position = BlockPos { x: 2, y: 64, z: 1 };
        storage
            .set_block_at(block_position, BlockStateId(1))
            .unwrap();
        storage
            .set_block_at(entity_position, BlockStateId(1))
            .unwrap();
        let block_token = storage.block_mutation_token(block_position).unwrap();
        let entity_token = storage.block_mutation_token(entity_position).unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let (journal, pending) = super::super::world_journal::WorldChunkJournal::open(
            temp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        assert!(pending.is_empty());
        sessions.install_world_chunk_journal(journal);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let block_response = handle
            .enqueue(SimulationCommand::ApplyBlockEdits {
                actor_session: 0,
                edits: vec![BlockEdit {
                    pos: block_position,
                    new_state: BlockStateId(0),
                }],
                preconditions: vec![BlockEditPrecondition {
                    pos: block_position,
                    expected_state: BlockStateId(1),
                    expected_token: block_token,
                }],
                scheduled_block_ticks: Vec::new(),
            })
            .unwrap();
        let bytes = vec![10, 0, 0, 0];
        let entity_response = handle
            .enqueue(SimulationCommand::CommitOpaqueBlockEntity {
                position: entity_position,
                expected_state: BlockStateId(1),
                expected_token: entity_token,
                bytes: bytes.clone(),
            })
            .unwrap();

        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &sessions,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    2,
                )
                .await
                .processed,
            2
        );
        assert!(matches!(
            block_response.await.unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert!(matches!(
            entity_response.await.unwrap().unwrap(),
            SimulationResponse::OpaqueBlockEntity(Ok(true))
        ));

        let live = read_view.snapshot_chunks(&[chunk_position]);
        assert_eq!(
            live.chunk(chunk_position)
                .unwrap()
                .block_entities
                .get(&entity_position),
            Some(&bytes)
        );
        let (reopened, pending) =
            super::super::world_journal::WorldChunkJournal::open(temp.path(), blocks, items)
                .unwrap();
        let restored = reopened.decode_pending(&pending).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].get_block(1, 64, 1), Some(BlockStateId(0)));
        assert!(!restored[0].block_entities.contains_key(&entity_position));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn regional_block_edit_append_failure_rejects_success_and_poisons_journal() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let items = Arc::new(ItemRegistry::from_report(&[]));
        let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        let chunk_position = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk_position,
                Chunk::empty(
                    chunk_position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let position = BlockPos { x: 1, y: 64, z: 1 };
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        let token = storage.block_mutation_token(position).unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let mut journal_failure = sessions.subscribe_world_chunk_journal_failure();
        let (journal, pending) = super::super::world_journal::WorldChunkJournal::open(
            temp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        assert!(pending.is_empty());
        sessions.install_world_chunk_journal(journal);
        let (session, mut outbound) = register_test_session_with_outbound(&sessions, "WalFailure");
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .enqueue(SimulationCommand::ApplyBlockEdits {
                actor_session: session,
                edits: vec![BlockEdit {
                    pos: position,
                    new_state: BlockStateId(0),
                }],
                preconditions: vec![BlockEditPrecondition {
                    pos: position,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                }],
                scheduled_block_ticks: Vec::new(),
            })
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        owner.install_regional_block_edit_probe(entered_tx, release_rx);
        let owner_sessions = Arc::clone(&sessions);
        let owner_world = Arc::clone(&world);
        let owner_read = read_view.clone();
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&owner_read),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });

        tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
            .await
            .unwrap();
        let solaris_directory = temp.path().join("solaris");
        std::fs::remove_file(solaris_directory.join("world-chunk-journal.bin")).unwrap();
        std::fs::remove_dir(&solaris_directory).unwrap();
        std::fs::write(&solaris_directory, b"blocks journal directory").unwrap();
        release_tx.send(()).unwrap();

        assert_eq!(owner_task.await.unwrap().processed, 1);
        assert!(matches!(
            response.await.unwrap(),
            Err(SimulationRequestError::WorldMutationFailed)
        ));
        journal_failure.changed().await.unwrap();
        assert!(*journal_failure.borrow_and_update());
        assert!(outbound.try_recv().is_err());
        assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
        assert!(world.lock().await.plan_dirty_flush().unwrap().is_empty());

        let snapshot = read_view.snapshot_chunks(&[chunk_position]);
        let error = sessions
            .world_chunk_journal()
            .unwrap()
            .record_snapshots(2, vec![snapshot.chunk(chunk_position).unwrap()])
            .unwrap_err();
        assert!(matches!(
            error,
            super::super::world_journal::WorldChunkJournalError::PoisonedOutcomeUnknown
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resident_fluid_tick_schedule_notifies_flush_without_world_writer() {
        let (mut storage, position, _) = test_block_storage_with_capacity(1);
        storage.set_block_at(position, BlockStateId(2)).unwrap();
        let flush_calls = Arc::new(AtomicUsize::new(0));
        let (flush_started, flush_started_rx) = oneshot::channel();
        let mut flush_started = Some(flush_started);
        let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn({
            let flush_calls = Arc::clone(&flush_calls);
            move || {
                let flush_calls = Arc::clone(&flush_calls);
                let flush_started = flush_started.take();
                async move {
                    flush_calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(flush_started) = flush_started {
                        let _ = flush_started.send(());
                    }
                }
            }
        });
        let dirty_flush = coordinator.notifier();
        storage.set_dirty_high_water_notifier(Arc::new(move || dirty_flush.request()));
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &test_block_reports(),
        ));
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .enqueue(SimulationCommand::ScheduleFluidTicksNearApplied {
                applied: vec![AppliedBlockEdit {
                    pos: position,
                    previous: BlockStateId(0),
                    new_state: BlockStateId(2),
                }],
                block_facts: facts,
                world_tick: 40,
            })
            .unwrap();

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let owner_read = read_view.clone();
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&owner_read),
                        mutation: Some(&mutation_view),
                        cpu: None,
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });

        let response = tokio::time::timeout(std::time::Duration::from_secs(1), response)
            .await
            .expect("resident fluid schedule completion event")
            .unwrap()
            .unwrap();
        assert!(matches!(response, SimulationResponse::FluidTicksScheduled));
        assert_eq!(flush_started_rx.await, Ok(()));
        drop(writer);

        assert_eq!(owner_task.await.unwrap().processed, 1);
        let mut storage = world.lock().await;
        let ticks = storage
            .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .unwrap();
        assert!(ticks.iter().any(|tick| tick.pos == position));
        drop(storage);
        coordinator.drain().await;
        assert!(flush_calls.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_block_edit_does_not_notify_dirty_flush() {
        let (storage, position, token) = test_block_storage();
        let notifications = Arc::new(AtomicUsize::new(0));
        storage.set_dirty_high_water_notifier({
            let notifications = Arc::clone(&notifications);
            Arc::new(move || {
                notifications.fetch_add(1, Ordering::SeqCst);
            })
        });
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .enqueue(SimulationCommand::ApplyBlockEdits {
                actor_session: 0,
                edits: vec![BlockEdit {
                    pos: position,
                    new_state: BlockStateId(0),
                }],
                preconditions: vec![BlockEditPrecondition {
                    pos: position,
                    expected_state: BlockStateId(0),
                    expected_token: token,
                }],
                scheduled_block_ticks: Vec::new(),
            })
            .unwrap();

        let report = owner
            .process_commands_with_world_views(
                &sessions,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await;

        assert_eq!(report.processed, 1);
        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_none()
        ));
        assert_eq!(notifications.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn regional_block_edits_in_distinct_lanes_overlap() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(blocks);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
        for chunk in chunks {
            storage
                .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
                .unwrap();
        }
        let positions = [
            BlockPos { x: 1, y: 64, z: 1 },
            BlockPos {
                x: 8 * 16 + 1,
                y: 64,
                z: 1,
            },
        ];
        for position in positions {
            storage.set_block_at(position, BlockStateId(1)).unwrap();
        }
        let tokens = positions.map(|position| storage.block_mutation_token(position).unwrap());
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let light = Arc::new(BlockLightTable::from_arrays(
            "regional light-changing edit",
            vec![0, 0, 0, 0, 0],
            vec![0, 15, 0, 0, 0],
            vec![true, false, true, true, true],
        ));
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = positions
            .into_iter()
            .zip(tokens)
            .map(|(position, token)| {
                handle
                    .enqueue(SimulationCommand::ApplyBlockEdits {
                        actor_session: 0,
                        edits: vec![BlockEdit {
                            pos: position,
                            new_state: BlockStateId(0),
                        }],
                        preconditions: vec![BlockEditPrecondition {
                            pos: position,
                            expected_state: BlockStateId(1),
                            expected_token: token,
                        }],
                        scheduled_block_ticks: Vec::new(),
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        owner.install_regional_block_edit_probe(entered_tx, release_rx);

        let owner_world = Arc::clone(&world);
        let owner_registry = Arc::clone(&registry);
        let worker = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(owner.process_commands_with_world_views(
                    &owner_registry,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: Some(&light),
                    },
                    Some(light.as_ref()),
                    2,
                ))
        });

        let first = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first regional worker entry");
        let second = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second regional worker enters before release");
        assert_ne!(first, second);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();

        let report = worker.join().unwrap();
        assert_eq!(report.processed, 2);
        assert_eq!(report.lane_attribution.len(), 2);
        assert!(
            report
                .lane_attribution
                .iter()
                .flat_map(|lane| &lane.commands)
                .all(|attribution| attribution.kind == "apply_block_edits")
        );
        for response in responses {
            let SimulationResponse::BlockEdits(Ok(outcome)) =
                response.blocking_recv().unwrap().unwrap()
            else {
                panic!("regional light-changing response mismatch");
            };
            let outcome = outcome.expect("regional light-changing commit");
            assert!(outcome.precomputed_light_updates.is_some());
        }
        let world = world.blocking_lock();
        for position in positions {
            assert_eq!(world.get_cached_block(position), Some(BlockStateId(0)));
            assert!(
                mc_world::light::ChunkLight::from_section_lights(
                    &world
                        .cached_chunk_snapshot(ChunkPos {
                            x: position.x.div_euclid(16),
                            z: position.z.div_euclid(16),
                        })
                        .unwrap()
                        .section_lights,
                )
                .is_some()
            );
        }
    }

    #[test]
    fn lane_attribution_counts_wait_once_for_two_commands() {
        let report = SimulationTickReport {
            processed: 2,
            remaining_depth: 0,
            lane_attribution: vec![SimulationLaneAttribution {
                cpu_admission_wait_us: 65_467,
                commands: vec![
                    SimulationCommandAttribution {
                        kind: "apply_block_edits",
                        post_admission_command_us: 321,
                    },
                    SimulationCommandAttribution {
                        kind: "apply_block_edits",
                        post_admission_command_us: 654,
                    },
                ],
            }],
        };

        assert_eq!(report.lane_attribution.len(), 1);
        assert_eq!(report.lane_attribution[0].cpu_admission_wait_us, 65_467);
        assert_eq!(report.lane_attribution[0].commands.len(), 2);
        assert_eq!(
            report.lane_attribution[0].commands[1].post_admission_command_us,
            654
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn regional_block_edits_in_one_lane_preserve_sequence() {
        let (storage, position, token) = test_block_storage();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let command = || SimulationCommand::ApplyBlockEdits {
            actor_session: 0,
            edits: vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            scheduled_block_ticks: Vec::new(),
        };
        let first = handle.enqueue(command()).unwrap();
        let stale = handle.enqueue(command()).unwrap();

        let report = owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                2,
            )
            .await;
        assert_eq!(report.processed, 2);
        assert_eq!(report.lane_attribution.len(), 1);
        assert_eq!(report.lane_attribution[0].commands.len(), 2);
        assert!(matches!(
            first.await.unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert!(matches!(
            stale.await.unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_none()
        ));
        assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mixed_world_run_preserves_regional_waves_around_coordinator_barrier() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(blocks);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
        for chunk in chunks {
            storage
                .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
                .unwrap();
        }
        let positions = [
            BlockPos { x: 1, y: 64, z: 1 },
            BlockPos {
                x: 8 * 16 + 1,
                y: 64,
                z: 1,
            },
        ];
        for position in positions {
            storage.set_block_at(position, BlockStateId(1)).unwrap();
        }
        let tokens = positions.map(|position| storage.block_mutation_token(position).unwrap());
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(3);
        let command = |position, token| SimulationCommand::ApplyBlockEdits {
            actor_session: 0,
            edits: vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            scheduled_block_ticks: Vec::new(),
        };
        let first = handle.enqueue(command(positions[0], tokens[0])).unwrap();
        let second = handle.enqueue(command(positions[1], tokens[1])).unwrap();
        let barrier = handle
            .enqueue(SimulationCommand::ReadChestSnapshot {
                positions: vec![positions[0]],
            })
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        owner.install_regional_block_edit_probe(entered_tx, release_rx);

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let owner_read = read_view.clone();
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&owner_read),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    3,
                )
                .await
        });

        let first_region = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first regional worker entry");
        let second_region = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second regional worker enters before release");
        assert_ne!(first_region, second_region);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();

        let first = first.await.unwrap().unwrap();
        assert!(matches!(
            first,
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert!(matches!(
            second.await.unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert_eq!(
            read_view.get_cached_block(positions[0]),
            Some(BlockStateId(0))
        );
        assert_eq!(
            read_view.get_cached_block(positions[1]),
            Some(BlockStateId(0))
        );

        drop(writer);
        assert!(matches!(
            barrier.await.unwrap().unwrap(),
            SimulationResponse::ChestSnapshot(_)
        ));
        assert_eq!(owner_task.await.unwrap().processed, 3);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chest_snapshot_query_runs_through_simulation_owner() {
        let (mut storage, position) = test_container_storage();
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 3,
            damage: None,
            enchantments: Vec::new(),
        };
        storage
            .set_chest_block_entity(position, chest.clone())
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "ChestSnapshotReader");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.read_chest_snapshot(vec![position]));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(
            owner
                .process_commands_with_world(&registry, Some(&world), None, 1)
                .await
                .processed,
            1
        );
        let snapshot = request.await.expect("chest snapshot owner response");
        assert_eq!(snapshot.state_id, 1);
        assert_eq!(snapshot.view.chests, vec![chest]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn furnace_snapshot_query_runs_through_simulation_owner() {
        let (mut storage, position) = test_container_storage();
        let mut furnace = FurnaceBlockEntity::default();
        furnace.slots[1] = mc_world::FurnaceSlot {
            item_id: 17,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        storage
            .set_furnace_block_entity(position, furnace.clone())
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "FurnaceSnapshotReader");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.read_furnace_snapshot(position));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(
            owner
                .process_commands_with_world(&registry, Some(&world), None, 1)
                .await
                .processed,
            1
        );
        let snapshot = request.await.expect("furnace snapshot owner response");
        assert_eq!(snapshot.state_id, 1);
        assert_eq!(snapshot.furnace, furnace);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_attack_validation_and_damage_run_in_one_owner_command() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "AtomicAttackAlice");
        register_test_player_state(&registry, session, PlayerInventory::empty());
        let target = seed_attack_target(&registry);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.player_attack_server_entity(target, 5.0));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let PlayerAttackResult::Damaged(outcome) =
            request.await.expect("player attack owner response")
        else {
            panic!("nearby living target must take damage");
        };
        assert_eq!(outcome.damage().snapshot.health, 15.0);

        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            5,
            "minecraft:cow".to_owned(),
            Vec3::new(20.5, 64.0, 0.5),
        );
        let far_target = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.type_name == "minecraft:cow")
            .expect("far attack target")
            .snapshot
            .id;
        let far_health_before = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == far_target)
            .expect("far target before attack")
            .snapshot
            .health;
        let mut rejected = Box::pin(session_handle.player_attack_server_entity(far_target, 5.0));

        assert_request_enqueued(rejected.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            rejected.await.expect("far attack owner response"),
            PlayerAttackResult::ValidationRejected
        ));
        assert_eq!(
            registry
                .persisted_entity_records()
                .into_iter()
                .find(|record| record.snapshot.id == far_target)
                .expect("far target remains")
                .snapshot
                .health,
            far_health_before
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_attack_rechecks_authoritative_attacker_mode_and_liveness() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "FencedAttackAlice");
        let attacker_state =
            register_test_player_state(&registry, session, PlayerInventory::empty());
        let target = seed_attack_target(&registry);
        let target_health = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == target)
            .expect("attack target before fenced requests")
            .snapshot
            .health;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);

        attacker_state.lock().unwrap().game_mode = GameMode::Spectator;
        let mut spectator_attack =
            Box::pin(session_handle.player_attack_server_entity(target, 5.0));
        assert_request_enqueued(spectator_attack.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            spectator_attack
                .await
                .expect("spectator attack owner response"),
            PlayerAttackResult::ValidationRejected
        ));

        {
            let mut state = attacker_state.lock().unwrap();
            state.game_mode = GameMode::Survival;
            state.survival.health = 0.0;
        }
        let mut dead_attack = Box::pin(session_handle.player_attack_server_entity(target, 5.0));
        assert_request_enqueued(dead_attack.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            dead_attack.await.expect("dead attack owner response"),
            PlayerAttackResult::ValidationRejected
        ));
        assert_eq!(
            registry
                .persisted_entity_records()
                .into_iter()
                .find(|record| record.snapshot.id == target)
                .expect("attack target after fenced requests")
                .snapshot
                .health,
            target_health
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_attack_rejects_out_of_reach_adventure_and_creative_targets() {
        for game_mode in [GameMode::Adventure, GameMode::Creative] {
            let registry = SessionRegistry::new();
            let session = register_test_session(&registry, "RemoteAttackAlice");
            register_test_player_state(&registry, session, PlayerInventory::empty());
            registry.spawn_command_entity(
                &SimulationAuthority::for_test(),
                5,
                "minecraft:cow".to_owned(),
                Vec3::new(20.5, 64.0, 0.5),
            );
            let target = registry
                .persisted_entity_records()
                .into_iter()
                .find(|record| record.snapshot.type_name == "minecraft:cow")
                .expect("far attack target")
                .snapshot;
            let (handle, mut owner) = simulation_channel_with_capacity(1);
            let session_handle = handle.for_session(session);
            registry
                .commit_player_state_event(
                    &SimulationAuthority::for_test(),
                    session,
                    PlayerStateEvent::GameMode(game_mode),
                )
                .expect("set authoritative attacker mode");
            let mut request = Box::pin(session_handle.player_attack_server_entity(target.id, 5.0));

            assert_request_enqueued(request.as_mut(), &handle).await;
            assert_eq!(owner.process_tick(&registry, 1).processed, 1);
            assert!(matches!(
                request.await.expect("far attack owner response"),
                PlayerAttackResult::ValidationRejected
            ));
            assert_eq!(
                registry
                    .persisted_entity_records()
                    .into_iter()
                    .find(|record| record.snapshot.id == target.id)
                    .expect("far target remains")
                    .snapshot
                    .health,
                target.health
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_shield_commit_refreshes_identity_before_queued_pvp_in_same_batch() {
        let registry = SessionRegistry::new();
        let attacker_pose = PlayerPose::new(0.5, 64.0, 2.5);
        let target_pose = PlayerPose::new(0.5, 64.0, 0.5);
        let (attacker, _attacker_rx) =
            register_test_session_at_with_outbound(&registry, "ShieldBatchAttacker", attacker_pose);
        let (target, _target_rx) =
            register_test_session_at_with_outbound(&registry, "ShieldBatchTarget", target_pose);
        register_test_player_state(&registry, attacker, PlayerInventory::empty());

        let items = Arc::new(mc_data::items::solaris_required_items());
        let item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
        let shield_item = items
            .id_of(&Identifier::parse("minecraft:shield").unwrap())
            .unwrap();
        registry.configure_player_combat(None, None, Arc::clone(&items), item_facts);
        registry.set_world_time(10);

        let shield_slot = PlayerInventory::OFFHAND_SLOT;
        let initial_stack = ItemStack::new(shield_item, 1);
        let locally_damaged_stack = initial_stack.clone().with_damage(5);
        let mut initial_inventory = PlayerInventory::empty();
        initial_inventory.slots[shield_slot] = initial_stack.clone();
        let target_state = register_test_player_state(&registry, target, initial_inventory.clone());
        let initial_shield = crate::play::combat::ActiveShield {
            started_tick: 0,
            slot: shield_slot,
            expected_stack: initial_stack,
        };
        let locally_refreshed_shield = crate::play::combat::ActiveShield {
            expected_stack: locally_damaged_stack.clone(),
            ..initial_shield.clone()
        };
        registry.set_active_shield(target, Some(initial_shield.clone()));

        let mut locally_updated_inventory = initial_inventory.clone();
        locally_updated_inventory.slots[shield_slot] = locally_damaged_stack;
        let local_plan = PlayerSurvivalPlan {
            expected_survival: SurvivalState::FULL,
            updated_survival: SurvivalState::FULL,
            expected_inventory: initial_inventory,
            updated_inventory: locally_updated_inventory,
            expected_carried_item: ItemStack::EMPTY,
            expected_xp: XpState::default(),
            updated_xp: XpState::default(),
            active_shield: Some(ActiveShieldTransition {
                expected: Some(initial_shield),
                updated: Some(locally_refreshed_shield),
            }),
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            position: Vec3::new(target_pose.x, target_pose.y, target_pose.z),
        };

        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let target_handle = handle.for_session(target);
        let attacker_handle = handle.for_session(attacker);
        let mut local_commit = Box::pin(target_handle.commit_player_survival(local_plan));
        assert_request_enqueued(local_commit.as_mut(), &handle).await;
        let mut pvp = Box::pin(
            attacker_handle
                .player_attack_server_entity(EntityId(i32::try_from(target).unwrap()), 4.0),
        );
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(pvp.as_mut(), cx).is_pending(),
                "queued PvP must wait for the owner response"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(handle.snapshot().depth, 2);

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        assert!(matches!(
            local_commit.await.unwrap(),
            Some(PlayerSurvivalCommitOutcome::Committed(_))
        ));
        assert!(matches!(
            pvp.await.unwrap(),
            PlayerAttackResult::Damaged(outcome)
                if matches!(
                    *outcome,
                    EntityAttackOutcome::PlayerDamaged {
                        damage_applied: false,
                        ..
                    }
                )
        ));
        let target_state = target_state.lock().unwrap();
        assert_eq!(target_state.survival, SurvivalState::FULL);
        assert_eq!(
            target_state.inventory.slots[shield_slot],
            ItemStack::new(shield_item, 1).with_damage(10)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_melee_routes_damage_only_to_target_session() {
        let registry = SessionRegistry::new();
        let (alice, mut alice_rx) = register_test_session_at_with_outbound(
            &registry,
            "PvpAlice",
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let (bob, mut bob_rx) = register_test_session_at_with_outbound(
            &registry,
            "PvpBob",
            PlayerPose::new(0.5, 64.0, 2.5),
        );
        let (carol, mut carol_rx) = register_test_session_at_with_outbound(
            &registry,
            "PvpCarol",
            PlayerPose::new(0.5, 64.0, 3.5),
        );
        for session in [alice, bob, carol] {
            register_test_player_state(&registry, session, PlayerInventory::empty());
        }
        while alice_rx.try_recv().is_ok() {}
        while bob_rx.try_recv().is_ok() {}
        while carol_rx.try_recv().is_ok() {}

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let alice_handle = handle.for_session(alice);
        let mut request = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let PlayerAttackResult::Damaged(outcome) =
            request.await.expect("player melee owner response")
        else {
            panic!("reachable player target must accept melee damage");
        };
        assert!(matches!(
            &*outcome,
            EntityAttackOutcome::PlayerDamaged { target_session, .. } if *target_session == bob
        ));
        dispatch_visibility_commands(outcome.into_dispatches());
        assert!(matches!(
            bob_rx.try_recv(),
            Ok(OutboundCommand::PlayerDamageCommitted { .. })
        ));
        assert!(
            bob_rx.try_recv().is_err(),
            "the authoritative commit must publish exactly once to the victim"
        );
        while let Ok(command) = alice_rx.try_recv() {
            assert!(!matches!(
                command,
                OutboundCommand::DamagePlayer { .. }
                    | OutboundCommand::PlayerDamageCommitted { .. }
            ));
            assert!(!matches!(command, OutboundCommand::EntityEvent { .. }));
        }
        while let Ok(command) = carol_rx.try_recv() {
            assert!(!matches!(
                command,
                OutboundCommand::DamagePlayer { .. }
                    | OutboundCommand::PlayerDamageCommitted { .. }
            ));
            assert!(!matches!(command, OutboundCommand::EntityEvent { .. }));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reciprocal_player_attacks_commit_without_connection_loop_progress() {
        let registry = SessionRegistry::new();
        let mut attacks = registry.subscribe_player_attacks();
        let (alice, _alice_rx) = register_test_session_at_with_outbound(
            &registry,
            "ReciprocalAlice",
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let (bob, _bob_rx) = register_test_session_at_with_outbound(
            &registry,
            "ReciprocalBob",
            PlayerPose::new(0.5, 64.0, 2.5),
        );
        let alice_state = register_test_player_state(&registry, alice, PlayerInventory::empty());
        let bob_state = register_test_player_state(&registry, bob, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let alice_handle = handle.for_session(alice);
        let bob_handle = handle.for_session(bob);
        let attack_costs = |position: Vec3| {
            let mut updated_survival = SurvivalState::FULL;
            updated_survival.add_exhaustion(SurvivalState::ENTITY_ATTACK_EXHAUSTION);
            PlayerSurvivalPlan {
                expected_survival: SurvivalState::FULL,
                updated_survival,
                expected_inventory: PlayerInventory::empty(),
                updated_inventory: PlayerInventory::empty(),
                expected_carried_item: ItemStack::EMPTY,
                expected_xp: XpState::default(),
                updated_xp: XpState::default(),
                active_shield: None,
                enchanting_table_input: None,
                item_entity_type_id: None,
                xp_orb_entity_type_id: None,
                position,
            }
        };
        let mut alice_attack = Box::pin(alice_handle.player_attack_server_entity_with_costs(
            EntityId(i32::try_from(bob).unwrap()),
            4.0,
            attack_costs(Vec3::new(0.5, 64.0, 0.5)),
            7,
        ));
        let mut bob_attack = Box::pin(bob_handle.player_attack_server_entity_with_costs(
            EntityId(i32::try_from(alice).unwrap()),
            4.0,
            attack_costs(Vec3::new(0.5, 64.0, 2.5)),
            8,
        ));

        std::future::poll_fn(|cx| {
            assert!(alice_attack.as_mut().poll(cx).is_pending());
            assert!(bob_attack.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(handle.snapshot().depth, 2);
        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        let first = attacks.try_recv().expect("first authority observation");
        let second = attacks.try_recv().expect("second authority observation");
        assert_eq!(
            (first.attacker_session_id, first.target_entity_id),
            (alice, i32::try_from(bob).unwrap())
        );
        assert_eq!(
            (second.attacker_session_id, second.target_entity_id),
            (bob, i32::try_from(alice).unwrap())
        );
        assert_eq!((first.cooldown_tick, second.cooldown_tick), (7, 8));
        assert_eq!((first.authority_tick, second.authority_tick), (0, 0));
        assert_eq!(
            (first.authority_sequence, second.authority_sequence),
            (1, 2)
        );
        assert!(attacks.try_recv().is_err());
        assert!(matches!(
            alice_attack.await.expect("Alice attack owner response"),
            PlayerAttackResult::Damaged(_)
        ));
        assert!(matches!(
            bob_attack.await.expect("Bob attack owner response"),
            PlayerAttackResult::Damaged(_)
        ));
        let alice_state = alice_state.lock().unwrap();
        let bob_state = bob_state.lock().unwrap();
        assert_eq!(alice_state.survival.health, 16.0);
        assert_eq!(bob_state.survival.health, 16.0);
        assert_eq!(alice_state.survival.exhaustion, 0.1);
        assert_eq!(bob_state.survival.exhaustion, 0.1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_melee_rejects_out_of_range_target() {
        let registry = SessionRegistry::new();
        let (alice, _alice_rx) = register_test_session_at_with_outbound(
            &registry,
            "FarPvpAlice",
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let (bob, mut bob_rx) = register_test_session_at_with_outbound(
            &registry,
            "FarPvpBob",
            PlayerPose::new(0.5, 64.0, 2.5),
        );
        for session in [alice, bob] {
            register_test_player_state(&registry, session, PlayerInventory::empty());
        }
        while bob_rx.try_recv().is_ok() {}

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let alice_handle = handle.for_session(alice);
        let mut nearby = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );
        assert_request_enqueued(nearby.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            nearby.await.expect("near player melee owner response"),
            PlayerAttackResult::Damaged(outcome)
                if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
        ));
        while bob_rx.try_recv().is_ok() {}

        registry
            .commit_player_pose(
                &SimulationAuthority::for_test(),
                bob,
                PlayerPose::new(20.5, 64.0, 0.5),
                0.0,
            )
            .expect("move target out of melee range");
        let mut request = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            request.await.expect("far player melee owner response"),
            PlayerAttackResult::ValidationRejected
        ));
        assert!(bob_rx.try_recv().is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_melee_accepts_adventure_and_rejects_invulnerable_modes() {
        let registry = SessionRegistry::new();
        let (alice, _alice_rx) = register_test_session_at_with_outbound(
            &registry,
            "ModePvpAlice",
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let (bob, mut bob_rx) = register_test_session_at_with_outbound(
            &registry,
            "ModePvpBob",
            PlayerPose::new(0.5, 64.0, 2.5),
        );
        for session in [alice, bob] {
            register_test_player_state(&registry, session, PlayerInventory::empty());
        }
        while bob_rx.try_recv().is_ok() {}

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let alice_handle = handle.for_session(alice);
        let mut survival_target = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );
        assert_request_enqueued(survival_target.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            survival_target.await.expect("survival PvP owner response"),
            PlayerAttackResult::Damaged(outcome)
                if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
        ));
        while bob_rx.try_recv().is_ok() {}

        registry
            .commit_player_state_event(
                &SimulationAuthority::for_test(),
                bob,
                PlayerStateEvent::GameMode(GameMode::Adventure),
            )
            .expect("switch target to adventure");
        let mut adventure_target = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );
        assert_request_enqueued(adventure_target.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            adventure_target
                .await
                .expect("adventure PvP owner response"),
            PlayerAttackResult::Damaged(outcome)
                if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
        ));

        registry
            .commit_player_state_event(
                &SimulationAuthority::for_test(),
                bob,
                PlayerStateEvent::GameMode(GameMode::Creative),
            )
            .expect("switch target to creative");
        let mut creative_target = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );
        assert_request_enqueued(creative_target.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            creative_target.await.expect("creative PvP owner response"),
            PlayerAttackResult::ValidationRejected
        ));
        assert!(bob_rx.try_recv().is_err());

        registry
            .commit_player_state_event(
                &SimulationAuthority::for_test(),
                bob,
                PlayerStateEvent::GameMode(GameMode::Spectator),
            )
            .expect("switch target to spectator");
        let mut spectator_target = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );
        assert_request_enqueued(spectator_target.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            spectator_target
                .await
                .expect("spectator PvP owner response"),
            PlayerAttackResult::ValidationRejected
        ));
        assert!(bob_rx.try_recv().is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_melee_uses_fenced_attacker_identity_and_rejects_self() {
        let registry = SessionRegistry::new();
        let (alice, _alice_rx) = register_test_session_at_with_outbound(
            &registry,
            "IdentityPvpAlice",
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        let (bob, mut bob_rx) = register_test_session_at_with_outbound(
            &registry,
            "IdentityPvpBob",
            PlayerPose::new(0.5, 64.0, 1.5),
        );
        for session in [alice, bob] {
            register_test_player_state(&registry, session, PlayerInventory::empty());
        }
        while bob_rx.try_recv().is_ok() {}

        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let alice_handle = handle.for_session(alice);
        let mut valid = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );
        assert_request_enqueued(valid.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            valid.await.expect("valid player melee owner response"),
            PlayerAttackResult::Damaged(outcome)
                if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
        ));
        while bob_rx.try_recv().is_ok() {}

        registry
            .commit_player_pose(
                &SimulationAuthority::for_test(),
                alice,
                PlayerPose::new(20.5, 64.0, 0.5),
                0.0,
            )
            .expect("move attacker away from target");
        let mut authoritative_pose = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
        );
        assert_request_enqueued(authoritative_pose.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            authoritative_pose
                .await
                .expect("authoritative-pose player melee owner response"),
            PlayerAttackResult::ValidationRejected
        ));
        assert!(bob_rx.try_recv().is_err());

        let mut self_attack = Box::pin(
            alice_handle.player_attack_server_entity(EntityId(i32::try_from(alice).unwrap()), 4.0),
        );
        assert_request_enqueued(self_attack.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            self_attack.await.expect("self melee owner response"),
            PlayerAttackResult::ValidationRejected
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn world_lock_is_released_before_following_non_world_command() {
        let (storage, position, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let session = register_test_session(&registry, "WorldBatchIsolation");
        let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let session_handle = handle.for_session(session);
        let mut world_request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
        ));
        let mut inventory_request =
            Box::pin(session_handle.commit_player_inventory(empty_container_player_plan()));

        assert_request_enqueued(world_request.as_mut(), &handle).await;
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(inventory_request.as_mut(), cx).is_pending(),
                "inventory request must wait for the simulation owner response"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(handle.snapshot().depth, 2);

        let blocked_player = Arc::clone(&persisted);
        let (player_locked_tx, player_locked_rx) = tokio::sync::oneshot::channel();
        let (release_player_tx, release_player_rx) = std::sync::mpsc::channel();
        let player_blocker = std::thread::spawn(move || {
            let _guard = blocked_player.lock().unwrap();
            player_locked_tx.send(()).unwrap();
            release_player_rx.recv().unwrap();
        });
        player_locked_rx.await.unwrap();
        let owner_registry = Arc::clone(&registry);
        let owner_world = Arc::clone(&world);
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world(&owner_registry, Some(&owner_world), None, 2)
                .await
        });

        assert!(world_request.await.unwrap().is_some());
        let world_is_available =
            match tokio::time::timeout(std::time::Duration::from_secs(1), world.lock()).await {
                Ok(storage) => {
                    drop(storage);
                    true
                }
                Err(_) => false,
            };

        release_player_tx.send(()).unwrap();
        player_blocker.join().unwrap();
        assert!(
            world_is_available,
            "non-world command must not retain the world lock"
        );
        assert!(matches!(
            inventory_request.await.unwrap(),
            PlayerInventoryCommitOutcome::Committed { .. }
        ));
        assert_eq!(owner_task.await.unwrap().processed, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn packet_owner_relight_compute_and_publish_do_not_hold_world_writer() {
        let (storage, position, token) = test_block_storage();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let session = register_test_session(&registry, "OwnerRelightWriterRelease");
        let table = Arc::new(BlockLightTable::from_arrays(
            "test",
            vec![0, 0, 0, 0, 15],
            vec![0, 15, 0, 15, 0],
            vec![true, false, true, false, true],
        ));
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let (reached_tx, reached_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        registry.install_server_relight_compute_probe(reached_tx, resume_rx);
        let owner_registry = Arc::clone(&registry);
        let owner_world = Arc::clone(&world);
        let owner_table = Arc::clone(&table);
        let owner_thread = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(owner.process_commands_with_world_views(
                    &owner_registry,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: None,
                        light: Some(&owner_table),
                    },
                    Some(&owner_table),
                    1,
                ))
        });

        reached_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("packet owner relight reaches the compute boundary");
        let mut writer = world
            .try_lock()
            .expect("packet relight compute releases the world writer");
        writer
            .set_block_at(BlockPos { x: 2, y: 64, z: 2 }, BlockStateId(1))
            .unwrap();
        resume_tx.send(()).expect("release packet owner relight");
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .expect("packet relight publishes while the world writer remains held")
            .expect("packet owner relight response")
            .expect("block edit committed");
        drop(writer);
        let report = owner_thread.join().expect("packet owner relight joins");

        assert_eq!(report.processed, 1);
        assert_eq!(outcome.applied.len(), 1);
        let updates = outcome
            .precomputed_light_updates
            .expect("owner response includes published light");
        assert_eq!(updates.len(), 1);
        let storage = world
            .try_lock()
            .expect("owner relight released world writer");
        let current = storage
            .cached_chunk_snapshot(mc_world::ChunkPos { x: 0, z: 0 })
            .expect("edited chunk remains resident");
        let mut refs = [[None; 3]; 3];
        refs[1][1] = Some(current.as_ref());
        let expected = mc_world::light::compute_chunk_light_in(
            &mut mc_world::light::LightWorkspace::new(),
            refs,
            &table,
        );
        assert_eq!(updates[0].light, expected);
        assert_eq!(
            mc_world::light::ChunkLight::from_section_lights(&current.section_lights),
            Some(expected)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_pose_commit_updates_session_and_persistence_in_one_owner_turn() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "PoseOwner");
        let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut pose = PlayerPose::new(7.5, 65.0, -3.5);
        pose.yaw = 91.0;
        pose.pitch = -12.0;
        pose.flags = mc_protocol::packets::play::MovePlayerFlags::new(true, false);
        pose.sprinting = true;
        let mut request = Box::pin(session_handle.commit_player_pose(pose, 0.25));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(
            request.await,
            Ok(CommittedPlayerPose {
                food: 20,
                saturation: 5.0,
                exhaustion: 0.25,
                resources_changed: false,
            })
        );

        let session_pose = registry.player_pose(session).expect("active player pose");
        assert_eq!(session_pose.x, pose.x);
        assert_eq!(session_pose.y, pose.y);
        assert_eq!(session_pose.z, pose.z);
        assert_eq!(session_pose.yaw, pose.yaw);
        assert_eq!(session_pose.pitch, pose.pitch);
        assert_eq!(session_pose.flags, pose.flags);
        assert_eq!(session_pose.sprinting, pose.sprinting);

        let persisted_pose = persisted.lock().unwrap().pose;
        assert_eq!(persisted_pose.x, pose.x);
        assert_eq!(persisted_pose.y, pose.y);
        assert_eq!(persisted_pose.z, pose.z);
        assert_eq!(persisted_pose.yaw, pose.yaw);
        assert_eq!(persisted_pose.pitch, pose.pitch);
        assert_eq!(persisted_pose.flags, pose.flags);
        assert_eq!(persisted_pose.sprinting, pose.sprinting);
        assert_eq!(persisted.lock().unwrap().survival.exhaustion, 0.25);

        let mut threshold = Box::pin(session_handle.commit_player_pose(pose, 3.75));
        assert_request_enqueued(threshold.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 2).processed, 1);
        assert_eq!(
            threshold.await,
            Ok(CommittedPlayerPose {
                food: 20,
                saturation: 4.0,
                exhaustion: 0.0,
                resources_changed: true,
            })
        );
        assert_eq!(persisted.lock().unwrap().survival.saturation, 4.0);

        let mut local = SurvivalState {
            health: 7.0,
            ..SurvivalState::FULL
        };
        CommittedPlayerPose {
            food: 19,
            saturation: 0.0,
            exhaustion: 1.5,
            resources_changed: true,
        }
        .apply_resources_to(&mut local);
        assert_eq!(local.health, 7.0);
        assert_eq!(local.food, 19);
        assert_eq!(local.saturation, 0.0);
        assert_eq!(local.exhaustion, 1.5);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_session_pose_commit_is_rejected_without_mutating_persistence() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StalePoseOwner");
        let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
        let original_pose = persisted.lock().unwrap().pose;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request =
            Box::pin(session_handle.commit_player_pose(PlayerPose::new(40.5, 70.0, 40.5), 0.25));

        assert_request_enqueued(request.as_mut(), &handle).await;
        let _ = registry.unregister(session);
        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            request.await,
            Err(SimulationRequestError::StaleSession)
        ));

        let persisted_pose = persisted.lock().unwrap().pose;
        assert_eq!(persisted_pose.x, original_pose.x);
        assert_eq!(persisted_pose.y, original_pose.y);
        assert_eq!(persisted_pose.z, original_pose.z);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_metadata_events_commit_through_the_simulation_owner() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "MetadataOwner");
        let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut respawn = PlayerPose::new(12.5, 70.0, -8.5);
        respawn.yaw = 135.0;

        let mut hotbar_request = Box::pin(session_handle.commit_selected_hotbar_slot(4));
        assert_request_enqueued(hotbar_request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(hotbar_request.await, Ok(()));

        let mut respawn_request = Box::pin(session_handle.commit_respawn_pose(respawn));
        assert_request_enqueued(respawn_request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(respawn_request.await, Ok(()));

        let mut game_mode_request = Box::pin(session_handle.commit_game_mode(GameMode::Creative));
        assert_request_enqueued(game_mode_request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(game_mode_request.await, Ok(()));

        let persisted = persisted.lock().unwrap();
        assert_eq!(persisted.selected_hotbar_slot, 4);
        assert_eq!(persisted.spawn.pose().x, respawn.x);
        assert_eq!(persisted.spawn.pose().y, respawn.y);
        assert_eq!(persisted.spawn.pose().z, respawn.z);
        assert_eq!(persisted.spawn.pose().yaw, respawn.yaw);
        assert_eq!(persisted.game_mode, GameMode::Creative);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_session_metadata_event_does_not_mutate_persistence() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleMetadataOwner");
        let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_selected_hotbar_slot(7));

        assert_request_enqueued(request.as_mut(), &handle).await;
        let _ = registry.unregister(session);
        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert_eq!(request.await, Err(SimulationRequestError::StaleSession));
        assert_eq!(persisted.lock().unwrap().selected_hotbar_slot, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn player_inventory_commit_updates_cursor_persistence_and_drop_in_one_owner_turn() {
        let registry = SessionRegistry::new();
        let (session, _outbound) = register_test_session_with_outbound(&registry, "InventoryOwner");
        let before_inventory = PlayerInventory::empty();
        let persisted = register_test_player_state(&registry, session, before_inventory.clone());
        let before_cursor = ItemStack::new(99, 2);
        persisted.lock().unwrap().carried_item = before_cursor.clone();
        let mut updated_inventory = before_inventory.clone();
        updated_inventory.slots[9] = ItemStack::new(42, 1);
        let updated_cursor = ItemStack::new(99, 1);
        let plan = ContainerPlayerPlan {
            expected_inventory: before_inventory,
            expected_carried_item: before_cursor,
            updated_inventory: updated_inventory.clone(),
            updated_carried_item: updated_cursor.clone(),
            crafting_table_input: None,
            enchanting_table_input: None,
            drops: vec![ContainerDropPlan {
                entity_type_id: 1,
                position: Vec3::new(0.5, 65.0, 0.5),
                stack: EntityItemStack::new(99, 1),
            }],
            xp_orb: None,
        };
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_player_inventory(plan));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let outcome = request.await.unwrap();

        assert!(matches!(
            outcome,
            PlayerInventoryCommitOutcome::Committed { ref inventory, ref carried_item, .. }
                if inventory.slots == updated_inventory.slots && carried_item == &updated_cursor
        ));
        let persisted = persisted.lock().unwrap();
        assert_eq!(persisted.inventory.slots, updated_inventory.slots);
        assert_eq!(persisted.carried_item, updated_cursor);
        drop(persisted);
        assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn duplicate_player_inventory_commit_has_one_winner_and_one_drop() {
        let registry = SessionRegistry::new();
        let (session, _outbound) =
            register_test_session_with_outbound(&registry, "DuplicateInventoryOwner");
        let before_inventory = PlayerInventory::empty();
        let persisted = register_test_player_state(&registry, session, before_inventory.clone());
        let before_cursor = ItemStack::new(77, 2);
        persisted.lock().unwrap().carried_item = before_cursor.clone();
        let mut updated_inventory = before_inventory.clone();
        updated_inventory.slots[9] = ItemStack::new(42, 1);
        let updated_cursor = ItemStack::new(77, 1);
        let plan = ContainerPlayerPlan {
            expected_inventory: before_inventory,
            expected_carried_item: before_cursor,
            updated_inventory: updated_inventory.clone(),
            updated_carried_item: updated_cursor.clone(),
            crafting_table_input: None,
            enchanting_table_input: None,
            drops: vec![ContainerDropPlan {
                entity_type_id: 1,
                position: Vec3::new(0.5, 65.0, 0.5),
                stack: EntityItemStack::new(77, 1),
            }],
            xp_orb: None,
        };
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let session_handle = handle.for_session(session);
        let mut first = Box::pin(session_handle.commit_player_inventory(plan.clone()));
        let mut duplicate = Box::pin(session_handle.commit_player_inventory(plan));

        assert_request_enqueued(first.as_mut(), &handle).await;
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(duplicate.as_mut(), cx).is_pending(),
                "duplicate request must wait for the simulation owner response"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(handle.snapshot().depth, 2);
        assert_eq!(owner.process_tick(&registry, 2).processed, 2);

        assert!(matches!(
            first.await.unwrap(),
            PlayerInventoryCommitOutcome::Committed { .. }
        ));
        assert!(matches!(
            duplicate.await.unwrap(),
            PlayerInventoryCommitOutcome::Rejected { ref inventory, ref carried_item, .. }
                if inventory.slots == updated_inventory.slots && carried_item == &updated_cursor
        ));
        assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_session_player_inventory_commit_is_rejected_without_mutation() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleInventoryOwner");
        let before_inventory = PlayerInventory::empty();
        let persisted = register_test_player_state(&registry, session, before_inventory.clone());
        let before_cursor = ItemStack::new(55, 1);
        persisted.lock().unwrap().carried_item = before_cursor.clone();
        let mut updated_inventory = before_inventory.clone();
        updated_inventory.slots[9] = ItemStack::new(42, 1);
        let plan = ContainerPlayerPlan {
            expected_inventory: before_inventory.clone(),
            expected_carried_item: before_cursor.clone(),
            updated_inventory,
            updated_carried_item: ItemStack::EMPTY,
            crafting_table_input: None,
            enchanting_table_input: None,
            drops: Vec::new(),
            xp_orb: None,
        };
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_player_inventory(plan));

        assert_request_enqueued(request.as_mut(), &handle).await;
        let _ = registry.unregister(session);
        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            request.await,
            Err(SimulationRequestError::StaleSession)
        ));

        let persisted = persisted.lock().unwrap();
        assert_eq!(persisted.inventory.slots, before_inventory.slots);
        assert_eq!(persisted.carried_item, before_cursor);
    }

    #[test]
    fn default_simulation_channel_is_bounded() {
        let (handle, _owner) = simulation_channel();
        assert_eq!(handle.snapshot().capacity, 1024);
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[test]
    fn simulation_channel_rejects_zero_capacity() {
        assert!(std::panic::catch_unwind(|| simulation_channel_with_capacity(0)).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn command_arrival_wakes_owner_and_preserves_the_envelope() {
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let mut wake = Box::pin(owner.wait_for_command());
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(wake.as_mut(), cx).is_pending(),
                "empty command queue must keep the owner parked"
            );
            std::task::Poll::Ready(())
        })
        .await;

        let _response = handle.enqueue(claim_xp(1, 10)).expect("command fits");
        assert!(wake.as_mut().await, "command arrival must wake the owner");
        drop(wake);

        let batch = owner.drain_batch(1);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].sequence, 0);
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pushed_processing_defers_herd_burst_and_serves_later_gameplay_command() {
        const HERD_COMMANDS: usize = 40;
        const HERDS_PER_TICK: usize = 2;

        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(HERD_COMMANDS + 1);
        for index in 0..HERD_COMMANDS {
            handle
                .ensure_chunk_herd((index as i32, 0), Vec::new())
                .expect("herd command fits");
        }
        let gameplay = handle
            .enqueue(SimulationCommand::SpawnCommandEntity {
                entity_type_id: 4,
                entity_type_name: "minecraft:cow".to_owned(),
                position: Vec3::new(0.5, 64.0, 0.5),
            })
            .expect("gameplay command fits");

        assert!(owner.wait_for_command().await);
        let pushed = owner
            .process_ready_commands_with_world(&registry, None, None, 256)
            .await;
        assert_eq!(pushed.processed, 1);
        assert_eq!(pushed.remaining_depth, HERD_COMMANDS);
        assert!(matches!(
            gameplay.await.unwrap().unwrap(),
            SimulationResponse::EntitySpawn(_)
        ));

        let first_tick = owner
            .process_commands_with_world(&registry, None, None, 256)
            .await;
        assert_eq!(first_tick.processed, HERDS_PER_TICK);
        assert_eq!(first_tick.remaining_depth, HERD_COMMANDS - HERDS_PER_TICK);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pushed_time_set_orders_earlier_and_later_herds_across_the_barrier() {
        let registry = SessionRegistry::new();
        let observer = register_test_session(&registry, "PushedTimeBarrierObserver");
        let earlier_chunk = (5, 0);
        let later_chunk = (6, 0);
        registry.mark_loaded(observer, earlier_chunk);
        registry.mark_loaded(observer, later_chunk);
        let (handle, mut owner) = simulation_channel_with_capacity(3);
        owner.restore_world_time(&registry, super::super::NIGHT_START_TICK);
        let hostile_herd = |chunk, x| {
            vec![super::super::HerdSpawn {
                chunk,
                slot: 0,
                entity_type_id: 5,
                entity_type_name: "minecraft:zombie".to_owned(),
                position: Vec3::new(x, 64.0, 8.5),
                hostile: true,
                sheep_color: None,
            }]
        };

        handle
            .ensure_chunk_herd(earlier_chunk, hostile_herd(earlier_chunk, 88.5))
            .expect("queue earlier night herd");
        let time_set = handle
            .for_session(observer)
            .enqueue(SimulationCommand::SetWorldTime { world_time: 0 })
            .expect("queue daytime barrier");
        handle
            .ensure_chunk_herd(later_chunk, hostile_herd(later_chunk, 104.5))
            .expect("queue later herd");

        assert!(owner.wait_for_command().await);
        let report = owner
            .process_ready_commands_with_world_views(
                &registry,
                None,
                SimulationWorldAccess::default(),
                None,
                256,
            )
            .await;

        assert_eq!(report.processed, 2);
        assert_eq!(report.remaining_depth, 1);
        assert!(matches!(
            time_set.await.expect("time set response"),
            Ok(SimulationResponse::WorldTimeSet)
        ));
        assert_eq!(registry.world_time(), 0);
        let records = registry.persisted_entity_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].snapshot.position.x, 88.5);
        let pushed_metrics = handle.snapshot();
        assert_eq!(pushed_metrics.enqueued, 3);
        assert_eq!(pushed_metrics.depth, 1);
        assert_eq!(pushed_metrics.processed, 2);
        assert_eq!(pushed_metrics.max_batch, 2);

        let later = owner
            .process_commands_with_world_views(
                &registry,
                None,
                SimulationWorldAccess::default(),
                None,
                256,
            )
            .await;
        assert_eq!(later.processed, 1);
        assert_eq!(later.remaining_depth, 0);
        assert_eq!(registry.persisted_entity_records().len(), 1);
        let drained_metrics = handle.snapshot();
        assert_eq!(drained_metrics.depth, 0);
        assert_eq!(drained_metrics.processed, 3);
        assert_eq!(drained_metrics.max_batch, 2);

        owner.advance_world_time(&registry, super::super::NIGHT_START_TICK);
        let mut positions = registry
            .persisted_entity_records()
            .into_iter()
            .map(|record| record.snapshot.position.x)
            .collect::<Vec<_>>();
        positions.sort_by(f64::total_cmp);
        assert_eq!(positions, [88.5, 104.5]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unbound_handle_rejects_player_command_before_enqueue() {
        let (handle, _owner) = simulation_channel();

        assert!(matches!(
            handle
                .pickup_item_into_inventory(EntityId(1), 42, None, Vec::new(), 64)
                .await,
            Err(SimulationRequestError::InvalidCommand)
        ));
        assert_eq!(handle.snapshot().enqueued, 0);
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[test]
    fn full_simulation_channel_rejects_without_growing_depth() {
        let (handle, _owner) = simulation_channel_with_capacity(1);
        let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
        let error = handle
            .enqueue(claim_xp(2, 11))
            .expect_err("second command must fail closed");

        assert_eq!(error, SimulationRequestError::Full);
        assert_eq!(handle.snapshot().depth, 1);
        assert_eq!(handle.snapshot().rejected_full, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_sender_is_closed_when_owner_drops() {
        let (handle, owner) = simulation_channel_with_capacity(1);
        let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
        let session_handle = handle.for_session(7);
        let mut waiting = Box::pin(
            session_handle
                .enqueue_player_command_wait(SimulationCommand::SetWorldTime { world_time: 1 }),
        );

        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
                "full queue must hold the sender until capacity or closure"
            );
            std::task::Poll::Ready(())
        })
        .await;

        drop(owner);

        assert!(matches!(waiting.await, Err(SimulationRequestError::Closed)));
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_detached_sender_is_closed_when_owner_shuts_down() {
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
        let mut waiting = Box::pin(
            handle.enqueue_detached_wait(SimulationCommand::SetWorldTime { world_time: 1 }),
        );

        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
                "full queue must hold the detached sender until capacity or closure"
            );
            std::task::Poll::Ready(())
        })
        .await;

        owner.shutdown();

        assert!(matches!(waiting.await, Err(SimulationRequestError::Closed)));
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[test]
    fn owner_drop_rejects_pending_response_and_closes_queue() {
        let (handle, owner) = simulation_channel_with_capacity(1);
        let response = handle.enqueue(claim_xp(1, 10)).expect("command fits");

        drop(owner);

        assert!(matches!(
            response.blocking_recv().expect("owner drop response"),
            Err(SimulationRequestError::OwnerStopped)
        ));
        assert!(matches!(
            handle.enqueue(claim_xp(2, 11)),
            Err(SimulationRequestError::Closed)
        ));
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[test]
    fn full_simulation_channel_rejects_attack_without_damage() {
        let registry = SessionRegistry::new();
        let target = seed_attack_target(&registry);
        let (handle, _owner) = simulation_channel_with_capacity(1);
        let _occupant = handle.enqueue(claim_xp(1, 10)).expect("first command fits");

        let error = handle
            .enqueue(SimulationCommand::AttackServerEntity {
                entity_id: target,
                damage: 20.0,
                knockback_origin: Some(Vec3::new(0.5, 64.0, 0.5)),
                rewards: EntityKillRewards::default(),
            })
            .expect_err("attack must fail closed while queue is full");
        let health = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == target)
            .expect("target remains")
            .snapshot
            .health;

        assert_eq!(error, SimulationRequestError::Full);
        assert_eq!(health, 20.0);
        assert_eq!(handle.snapshot().rejected_full, 1);
    }

    #[test]
    fn owner_drains_commands_in_monotonic_sequence_order() {
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let _first = handle.enqueue(claim_xp(1, 10)).unwrap();
        let _second = handle.enqueue(claim_xp(2, 11)).unwrap();

        let batch = owner.drain_batch(2);

        assert_eq!(
            batch
                .iter()
                .map(|command| command.sequence)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(handle.snapshot().depth, 0);
        assert_eq!(handle.snapshot().dequeued, 2);
        assert_eq!(handle.snapshot().max_batch, 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_sender_is_admitted_after_earlier_command_drains() {
        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
        let session_handle = handle.for_session(7);
        let mut waiting = Box::pin(
            session_handle
                .enqueue_player_command_wait(SimulationCommand::SetWorldTime { world_time: 1 }),
        );

        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
                "full queue must hold the later sender"
            );
            std::task::Poll::Ready(())
        })
        .await;

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let _response = waiting.await.expect("capacity release admits sender");
        let batch = owner.drain_batch(1);

        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].sequence, 1);
    }

    #[test]
    fn zero_budget_leaves_queued_command_unprocessed() {
        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let mut response = handle.enqueue(claim_xp(1, 10)).expect("command fits");

        let report = owner.process_tick(&registry, 0);

        assert_eq!(report.processed, 0);
        assert_eq!(report.remaining_depth, 1);
        assert!(matches!(
            response.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(handle.snapshot().max_batch, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_drains_prefetched_deferred_and_receiver_commands() {
        let (handle, mut owner) = simulation_channel_with_capacity(3);
        handle
            .ensure_chunk_herd((1, 1), Vec::new())
            .expect("background herd command fits");
        assert!(owner.drain_ready_batch(1).is_empty());

        let prefetched = handle
            .enqueue(claim_xp(1, 10))
            .expect("prefetched command fits");
        assert!(owner.wait_for_command().await);
        let queued = handle
            .enqueue(claim_xp(2, 11))
            .expect("receiver command fits");

        owner.shutdown();

        assert!(matches!(
            prefetched.await.expect("prefetched shutdown response"),
            Err(SimulationRequestError::ShuttingDown)
        ));
        assert!(matches!(
            queued.await.expect("receiver shutdown response"),
            Err(SimulationRequestError::ShuttingDown)
        ));
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.depth, 0);
        assert_eq!(snapshot.rejected_shutdown, 3);
        assert_eq!(snapshot.dequeued, 3);
    }

    #[test]
    fn cancelled_request_is_removed_before_owner_application() {
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle.enqueue(claim_xp(1, 10)).unwrap();
        drop(response);

        assert!(owner.drain_batch(1).is_empty());
        assert_eq!(handle.snapshot().cancelled, 1);
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_session_claim_is_rejected_and_reconnect_can_claim() {
        let registry = SessionRegistry::new();
        let old_session = register_test_session(&registry, "FenceAlice");
        let old_player_state =
            register_test_player_state(&registry, old_session, PlayerInventory::empty());
        let (item, _) = seed_claim_entities(&registry);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let old_handle = handle.for_session(old_session);
        let mut stale_request =
            Box::pin(old_handle.pickup_item_into_inventory(item, 42, None, Vec::new(), 64));
        assert_request_enqueued(stale_request.as_mut(), &handle).await;

        registry.unregister(old_session);
        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            stale_request.await,
            Err(SimulationRequestError::StaleSession)
        ));
        assert_eq!(
            registry
                .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .len(),
            1
        );
        assert!(
            old_player_state.lock().unwrap().inventory.slots[9..=44]
                .iter()
                .all(ItemStack::is_empty)
        );
        assert_eq!(handle.snapshot().rejected_stale_session, 1);

        let new_session = register_test_session(&registry, "FenceAlice");
        assert_ne!(new_session, old_session);
        let new_player_state =
            register_test_player_state(&registry, new_session, PlayerInventory::empty());
        let new_handle = handle.for_session(new_session);
        let mut fresh_request =
            Box::pin(new_handle.pickup_item_into_inventory(item, 42, None, Vec::new(), 64));
        assert_request_enqueued(fresh_request.as_mut(), &handle).await;

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(fresh_request.await.unwrap().unwrap().credited.count, 3);
        assert_eq!(
            new_player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 3)
        );
        assert!(
            registry
                .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .is_empty()
        );
    }

    #[test]
    fn item_pickup_credit_survives_requester_loss_after_owner_apply() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "PickupCreditAlice");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupItemIntoInventory {
                entity_id: item,
                collector_session: session,
                expected_item_id: 42,
                expected_damage: None,
                expected_enchantments: Vec::new(),
                max_stack: 64,
            })
            .expect("pickup command fits");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        drop(response);
        let outbound = [outbound.try_recv().unwrap(), outbound.try_recv().unwrap()];
        registry.unregister(session);

        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 3)
        );
        assert!(
            registry
                .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .is_empty()
        );
        assert!(
            outbound.iter().any(|command| matches!(
                command,
                OutboundCommand::TakeItemEntity { amount: 3, .. }
            ))
        );
        assert!(outbound.iter().any(
            |command| matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == item)
        ));
    }

    #[test]
    fn item_pickup_credit_is_conservative_under_partial_capacity() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "PickupCreditBob");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        let mut inventory = PlayerInventory::empty();
        for slot in 9..=44 {
            inventory.slots[slot] = ItemStack::new(42, 64);
        }
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupItemIntoInventory {
                entity_id: item,
                collector_session: session,
                expected_item_id: 42,
                expected_damage: None,
                expected_enchantments: Vec::new(),
                max_stack: 64,
            })
            .expect("pickup command fits");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let SimulationResponse::ItemPickupCredit(Some(outcome)) =
            response.blocking_recv().unwrap().unwrap()
        else {
            panic!("pickup credit response kind changed");
        };

        assert_eq!(outcome.credited.count, 1);
        assert_eq!(
            outcome.changed_slots,
            vec![(PlayerInventory::HOTBAR_BASE, ItemStack::new(42, 64))]
        );
        assert_eq!(
            outcome.inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 64)
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 64)
        );
        let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
        assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 2);
        assert!(matches!(
            outbound.try_recv(),
            Ok(OutboundCommand::UpdateEntityData(snapshot))
                if snapshot.id == item
                    && snapshot.item_stack.as_ref().is_some_and(|stack| stack.count == 2)
        ));
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn full_inventory_rejects_pickup_without_removing_entity() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "PickupCreditCarol");
        let mut inventory = PlayerInventory::empty();
        for slot in 9..=44 {
            inventory.slots[slot] = ItemStack::new(42, 64);
        }
        let player_state = register_test_player_state(&registry, session, inventory);
        let (item, _) = seed_claim_entities(&registry);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupItemIntoInventory {
                entity_id: item,
                collector_session: session,
                expected_item_id: 42,
                expected_damage: None,
                expected_enchantments: Vec::new(),
                max_stack: 64,
            })
            .expect("pickup command fits");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let SimulationResponse::ItemPickupCredit(outcome) =
            response.blocking_recv().unwrap().unwrap()
        else {
            panic!("pickup credit response kind changed");
        };

        assert!(outcome.is_none());
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 64)
        );
        let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
        assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 3);
    }

    #[test]
    fn invalid_hotbar_rejects_pickup_without_inventory_or_entity_publication() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "InvalidHotbarPickup");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        let inventory = PlayerInventory::empty();
        let player_state = register_test_player_state(&registry, session, inventory.clone());
        player_state.lock().unwrap().selected_hotbar_slot = 9;
        let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupItemIntoInventory {
                entity_id: item,
                collector_session: session,
                expected_item_id: 42,
                expected_damage: None,
                expected_enchantments: Vec::new(),
                max_stack: 64,
            })
            .expect("pickup command fits");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::ItemPickupCredit(None)
        ));

        assert_eq!(
            player_state.lock().unwrap().inventory.slots,
            inventory.slots
        );
        let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, item);
        assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 3);
        assert!(outbound.try_recv().is_err());
    }

    #[test]
    fn stale_item_stack_after_pickup_plan_preserves_inventory_entity_and_publication() {
        let registry = Arc::new(SessionRegistry::new());
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "StalePickupItemStack");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        let mut inventory = PlayerInventory::empty();
        for slot in 9..=44 {
            inventory.slots[slot] = ItemStack::new(42, 64);
        }
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
        let player_state = register_test_player_state(&registry, session, inventory.clone());
        let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
        let (plan_reached_tx, plan_reached_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        registry.install_item_pickup_plan_probe_for_test(plan_reached_tx, resume_rx);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupItemIntoInventory {
                entity_id: item,
                collector_session: session,
                expected_item_id: 42,
                expected_damage: None,
                expected_enchantments: Vec::new(),
                max_stack: 64,
            })
            .expect("pickup command fits");
        let owner_registry = Arc::clone(&registry);
        let owner_thread = std::thread::spawn(move || owner.process_tick(&owner_registry, 1));

        plan_reached_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("pickup reaches the post-plan CAS fence");
        let replacement = EntityItemStack {
            item_id: 42,
            count: 3,
            damage: Some(7),
            enchantments: Vec::new(),
        };
        assert!(registry.replace_item_stack_after_pickup_plan_for_test(item, replacement.clone()));
        resume_tx.send(()).expect("release pickup CAS");
        assert_eq!(owner_thread.join().unwrap().processed, 1);
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::ItemPickupCredit(None)
        ));

        assert_eq!(
            player_state.lock().unwrap().inventory.slots,
            inventory.slots
        );
        let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, item);
        assert_eq!(remaining[0].item_stack.as_ref(), Some(&replacement));
        assert!(outbound.try_recv().is_err());
    }

    fn assert_player_state_cannot_pick_up(
        player_name: &str,
        game_mode: GameMode,
        survival: SurvivalState,
    ) {
        let registry = SessionRegistry::new();
        let (item, experience) = seed_claim_entities(&registry);
        let arrow = seed_grounded_arrow(&registry);
        let session = register_test_session(&registry, player_name);
        let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
        {
            let mut state = player_state.lock().unwrap();
            state.game_mode = game_mode;
            state.survival = survival;
        }
        let (handle, mut owner) = simulation_channel_with_capacity(3);
        let item_response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupItemIntoInventory {
                entity_id: item,
                collector_session: session,
                expected_item_id: 42,
                expected_damage: None,
                expected_enchantments: Vec::new(),
                max_stack: 64,
            })
            .unwrap();
        let experience_response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
                entity_id: experience,
                collector_session: session,
            })
            .unwrap();
        let arrow_response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupArrowIntoInventory {
                entity_id: arrow,
                collector_session: session,
                arrow_item_id: 42,
                max_stack: 64,
            })
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 3).processed, 3);
        assert!(matches!(
            item_response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::ItemPickupCredit(None)
        ));
        assert!(matches!(
            experience_response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::ExperiencePickupCredit(None)
        ));
        assert!(matches!(
            arrow_response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::ArrowPickupCredit(None)
        ));

        let state = player_state.lock().unwrap();
        assert!(state.inventory.slots.iter().all(ItemStack::is_empty));
        assert_eq!(state.xp.total, 0);
        drop(state);
        assert_eq!(
            registry
                .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .len(),
            1
        );
        assert_eq!(
            registry
                .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .len(),
            1
        );
        assert!(registry.server_entity_snapshot(arrow).is_some());
    }

    #[test]
    fn spectator_cannot_receive_item_arrow_or_experience_pickup_credit() {
        assert_player_state_cannot_pick_up(
            "SpectatorPickup",
            GameMode::Spectator,
            SurvivalState::FULL,
        );
    }

    #[test]
    fn dead_player_cannot_receive_item_arrow_or_experience_pickup_credit() {
        assert_player_state_cannot_pick_up(
            "DeadPickup",
            GameMode::Survival,
            SurvivalState {
                health: 0.0,
                ..SurvivalState::FULL
            },
        );
    }

    #[test]
    fn experience_credit_survives_requester_loss_after_owner_apply() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "XpCreditAlice");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
        player_state.lock().unwrap().xp = XpState {
            level: 1,
            progress: 3.0 / 9.0,
            total: 10,
            seed: 17,
        };
        let (_, experience) = seed_claim_entities_published(&registry, &mut outbound);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
                entity_id: experience,
                collector_session: session,
            })
            .expect("experience command fits");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        drop(response);
        let outbound = [outbound.try_recv().unwrap(), outbound.try_recv().unwrap()];
        registry.unregister(session);

        let saved = player_state.lock().unwrap();
        assert_eq!(saved.xp.total, 15);
        assert_eq!(saved.xp.level, 1);
        assert!((saved.xp.progress - (8.0 / 9.0)).abs() < f32::EPSILON);
        assert_eq!(saved.xp.seed, 17);
        assert!(
            registry
                .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .is_empty()
        );
        assert!(
            outbound.iter().any(|command| matches!(
                command,
                OutboundCommand::TakeItemEntity { amount: 5, .. }
            ))
        );
        assert!(outbound.iter().any(
            |command| matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == experience)
        ));
    }

    #[test]
    fn concurrent_experience_credit_has_one_exact_winner() {
        let registry = SessionRegistry::new();
        let alice = register_test_session(&registry, "XpCreditBob");
        let bob = register_test_session(&registry, "XpCreditCarol");
        let alice_state = register_test_player_state(&registry, alice, PlayerInventory::empty());
        let bob_state = register_test_player_state(&registry, bob, PlayerInventory::empty());
        let (_, experience) = seed_claim_entities(&registry);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let alice_response = handle
            .for_session(alice)
            .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
                entity_id: experience,
                collector_session: alice,
            })
            .unwrap();
        let bob_response = handle
            .for_session(bob)
            .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
                entity_id: experience,
                collector_session: bob,
            })
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        let outcomes = [alice_response, bob_response].map(|response| {
            let SimulationResponse::ExperiencePickupCredit(outcome) =
                response.blocking_recv().unwrap().unwrap()
            else {
                panic!("experience credit response kind changed");
            };
            outcome
        });

        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_some()).count(),
            1
        );
        assert_eq!(
            alice_state.lock().unwrap().xp.total + bob_state.lock().unwrap().xp.total,
            5
        );
        assert!(
            registry
                .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .is_empty()
        );
    }

    #[test]
    fn stale_session_experience_credit_cannot_remove_orb_or_change_xp() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "XpCreditStale");
        let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (_, experience) = seed_claim_entities(&registry);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
                entity_id: experience,
                collector_session: session,
            })
            .unwrap();
        registry.unregister(session);

        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            response.blocking_recv().unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
        assert_eq!(player_state.lock().unwrap().xp.total, 0);
        assert_eq!(
            registry
                .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .len(),
            1
        );
    }

    #[test]
    fn arrow_credit_survives_requester_loss_after_owner_apply() {
        let registry = SessionRegistry::new();
        let arrow = seed_grounded_arrow(&registry);
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "ArrowCreditAlice");
        let spawn = registry.mark_loaded(session, (0, 0));
        assert!(spawn.iter().any(|dispatch| matches!(
            &dispatch.command,
            OutboundCommand::SpawnEntity(entity) if entity.id == arrow
        )));
        dispatch_visibility_commands(spawn);
        assert!(matches!(
            outbound.try_recv(),
            Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == arrow
        ));
        let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupArrowIntoInventory {
                entity_id: arrow,
                collector_session: session,
                arrow_item_id: 42,
                max_stack: 64,
            })
            .expect("arrow command fits");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        drop(response);
        let outbound = [outbound.try_recv().unwrap(), outbound.try_recv().unwrap()];
        registry.unregister(session);

        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
        assert!(registry.server_entity_snapshot(arrow).is_none());
        assert!(
            outbound.iter().any(|command| matches!(
                command,
                OutboundCommand::TakeItemEntity { amount: 1, .. }
            ))
        );
        assert!(outbound.iter().any(
            |command| matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == arrow)
        ));
    }

    #[test]
    fn full_inventory_rejects_arrow_pickup_without_removal() {
        let registry = SessionRegistry::new();
        let arrow = seed_grounded_arrow(&registry);
        let session = register_test_session(&registry, "ArrowCreditFull");
        let mut inventory = PlayerInventory::empty();
        for slot in 9..=44 {
            inventory.slots[slot] = ItemStack::new(42, 64);
        }
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupArrowIntoInventory {
                entity_id: arrow,
                collector_session: session,
                arrow_item_id: 42,
                max_stack: 64,
            })
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let SimulationResponse::ArrowPickupCredit(outcome) =
            response.blocking_recv().unwrap().unwrap()
        else {
            panic!("arrow credit response kind changed");
        };

        assert!(outcome.is_none());
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 64)
        );
        assert!(registry.server_entity_snapshot(arrow).is_some());
    }

    #[test]
    fn concurrent_arrow_credit_has_one_exact_winner() {
        let registry = SessionRegistry::new();
        let arrow = seed_grounded_arrow(&registry);
        let alice = register_test_session(&registry, "ArrowCreditBob");
        let bob = register_test_session(&registry, "ArrowCreditCarol");
        let alice_state = register_test_player_state(&registry, alice, PlayerInventory::empty());
        let bob_state = register_test_player_state(&registry, bob, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let alice_response = handle
            .for_session(alice)
            .enqueue(SimulationCommand::PickupArrowIntoInventory {
                entity_id: arrow,
                collector_session: alice,
                arrow_item_id: 42,
                max_stack: 64,
            })
            .unwrap();
        let bob_response = handle
            .for_session(bob)
            .enqueue(SimulationCommand::PickupArrowIntoInventory {
                entity_id: arrow,
                collector_session: bob,
                arrow_item_id: 42,
                max_stack: 64,
            })
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        let outcomes = [alice_response, bob_response].map(|response| {
            let SimulationResponse::ArrowPickupCredit(outcome) =
                response.blocking_recv().unwrap().unwrap()
            else {
                panic!("arrow credit response kind changed");
            };
            outcome
        });

        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_some()).count(),
            1
        );
        let credited = |state: &Arc<Mutex<PlayerPersistedState>>| {
            state.lock().unwrap().inventory.slots[9..=44]
                .iter()
                .filter(|stack| stack.item_id == 42)
                .map(|stack| stack.count)
                .sum::<i32>()
        };
        assert_eq!(credited(&alice_state) + credited(&bob_state), 1);
        assert!(registry.server_entity_snapshot(arrow).is_none());
    }

    #[test]
    fn stale_session_arrow_credit_cannot_remove_or_credit() {
        let registry = SessionRegistry::new();
        let arrow = seed_grounded_arrow(&registry);
        let session = register_test_session(&registry, "ArrowCreditStale");
        let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::PickupArrowIntoInventory {
                entity_id: arrow,
                collector_session: session,
                arrow_item_id: 42,
                max_stack: 64,
            })
            .unwrap();
        registry.unregister(session);

        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            response.blocking_recv().unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
        assert!(
            player_state.lock().unwrap().inventory.slots[9..=44]
                .iter()
                .all(ItemStack::is_empty)
        );
        assert!(registry.server_entity_snapshot(arrow).is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stale_session_block_edit_cannot_mutate_world() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "FenceBuilder");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;
        registry.unregister(session);

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            0
        );
        assert!(matches!(
            request.await,
            Err(SimulationRequestError::StaleSession)
        ));
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(1))
        );
        assert_eq!(handle.snapshot().rejected_stale_session, 1);
    }

    #[test]
    fn queued_duplicate_lethal_attack_does_not_duplicate_rewards() {
        let registry = SessionRegistry::new();
        let target = seed_attack_target(&registry);
        let rewards = EntityKillRewards {
            items: vec![(5, EntityItemStack::new(42, 1))],
            experience: Some((6, 5)),
        };
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let lethal_response = handle
            .enqueue(SimulationCommand::AttackServerEntity {
                entity_id: target,
                damage: 20.0,
                knockback_origin: Some(Vec3::new(0.5, 64.0, 0.5)),
                rewards: rewards.clone(),
            })
            .unwrap();
        let duplicate_response = handle
            .enqueue(SimulationCommand::AttackServerEntity {
                entity_id: target,
                damage: 20.0,
                knockback_origin: Some(Vec3::new(0.5, 64.0, 0.5)),
                rewards,
            })
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        let killed = match lethal_response.blocking_recv().unwrap().unwrap() {
            SimulationResponse::EntityAttack(Some(outcome)) => match *outcome {
                EntityAttackOutcome::Killed {
                    damage,
                    entity,
                    dispatches: _,
                    ..
                } => (damage, entity),
                other => panic!("expected lethal entity attack outcome, got {other:?}"),
            },
            other => panic!("expected lethal entity attack response, got {other:?}"),
        };
        assert!(matches!(
            duplicate_response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::EntityAttack(None)
        ));

        assert_eq!(killed.0.snapshot.health, 0.0);
        assert_eq!(killed.1.type_name, "minecraft:zombie");
        assert!(registry.server_entity_snapshot(target).is_some());
        assert_eq!(
            registry
                .persisted_entity_records()
                .into_iter()
                .filter(|record| record.snapshot.item_stack.is_some())
                .count(),
            1
        );
        assert_eq!(
            registry
                .nearby_experience_entities(killed.1.position, 2.25)
                .len(),
            1
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn script_entity_spawn_is_session_fenced_visible_and_saved() {
        let registry = SessionRegistry::new();
        let (actor, mut actor_rx) = register_test_session_with_outbound(&registry, "ScriptSpawner");
        registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.mark_loaded(actor, (0, 0));
        let (handle, mut owner) = simulation_channel_with_capacity(2);

        let mut spawn = Box::pin(handle.spawn_script_entity(
            actor,
            90,
            "minecraft:pig".to_owned(),
            Vec3::new(2.5, 64.0, 1.5),
        ));
        assert_request_enqueued(spawn.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(spawn.await, Ok(()));
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), actor_rx.recv())
                .await
                .expect("entity spawn dispatch was not delivered"),
            Some(OutboundCommand::SpawnEntity(_))
        ));

        let mut barrier = Box::pin(handle.save_barrier(false));
        assert_request_enqueued(barrier.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let snapshot = barrier.await.unwrap();
        assert!(snapshot.entities.records.iter().any(|entity| {
            entity.snapshot.type_name == "minecraft:pig"
                && entity.snapshot.position == Vec3::new(2.5, 64.0, 1.5)
        }));

        let stale = register_test_session(&registry, "StaleScriptSpawner");
        let mut stale_spawn = Box::pin(handle.spawn_script_entity(
            stale,
            90,
            "minecraft:pig".to_owned(),
            Vec3::new(3.5, 64.0, 1.5),
        ));
        assert_request_enqueued(stale_spawn.as_mut(), &handle).await;
        registry.unregister(stale);
        let stale_report = owner.process_tick(&registry, 1);
        assert_eq!(stale_report.processed, 0);
        assert_eq!(stale_report.remaining_depth, 0);
        assert_eq!(stale_spawn.await, Err(SimulationRequestError::StaleSession));
        assert_eq!(
            registry
                .persisted_entity_records()
                .into_iter()
                .filter(|entity| entity.snapshot.type_name == "minecraft:pig")
                .count(),
            1
        );
    }

    #[test]
    fn single_lane_region_routes_preserve_sequence_and_spawn_outcome() {
        let positions = [Vec3::new(-0.5, 64.0, 0.5), Vec3::new(128.5, 64.0, 0.5)];
        let routed = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = positions.map(|position| {
            handle
                .enqueue(SimulationCommand::SpawnCommandEntity {
                    entity_type_id: 4,
                    entity_type_name: "minecraft:zombie".to_owned(),
                    position,
                })
                .expect("regional spawn command fits")
        });

        assert_eq!(owner.process_tick(&routed, 2).processed, 2);
        let routes = owner.last_region_routes();
        assert_eq!(routes.len(), 2);
        assert!(routes[0].sequence < routes[1].sequence);
        assert_eq!(routes[0].lease.key, mc_entity::RegionKey::new(-1, 0));
        assert_eq!(routes[1].lease.key, mc_entity::RegionKey::new(1, 0));
        assert!(routes.iter().all(|route| route.lease.lane == 0));
        assert!(
            routes
                .iter()
                .all(|route| route.lease.epoch == mc_entity::RegionEpoch::INITIAL)
        );

        let routed_dispatch_counts =
            responses.map(
                |response| match response.blocking_recv().unwrap().unwrap() {
                    SimulationResponse::EntitySpawn(dispatches) => dispatches.len(),
                    other => panic!("expected regional entity spawn response, got {other:?}"),
                },
            );
        assert_eq!(routed_dispatch_counts, [0, 0]);

        let snapshots = routed.persisted_entity_records();
        assert_eq!(snapshots.len(), 2);
        assert!(
            snapshots
                .iter()
                .all(|record| record.snapshot.type_name == "minecraft:zombie")
        );
        assert!(positions.iter().all(|position| {
            snapshots
                .iter()
                .any(|record| record.snapshot.position == *position)
        }));

        let next_phase = owner
            .region_ownership
            .begin_phase()
            .expect("routed batch closed its exact phase");
        owner
            .region_ownership
            .acknowledge_lane(next_phase, 0)
            .expect("lane 0 completes test phase");
        owner
            .region_ownership
            .finish_phase(next_phase)
            .expect("test phase closes");
    }

    #[test]
    fn block_edit_routes_only_when_every_world_position_has_one_region_owner() {
        let inside = BlockPos { x: 1, y: 64, z: 1 };
        let same_region = BlockPos {
            x: 7 * 16 + 15,
            y: 64,
            z: 1,
        };
        let other_region = BlockPos {
            x: 8 * 16,
            y: 64,
            z: 1,
        };
        let token = BlockMutationToken {
            chunk_instance_id: 1,
            version: 2,
        };
        let command = |precondition_pos, tick_pos| SimulationCommand::ApplyBlockEdits {
            actor_session: 1,
            edits: vec![
                BlockEdit {
                    pos: inside,
                    new_state: BlockStateId(1),
                },
                BlockEdit {
                    pos: same_region,
                    new_state: BlockStateId(1),
                },
            ],
            preconditions: vec![BlockEditPrecondition {
                pos: precondition_pos,
                expected_state: BlockStateId(0),
                expected_token: token,
            }],
            scheduled_block_ticks: vec![ScheduledBlockTick::new(
                tick_pos,
                Identifier::parse("minecraft:stone").unwrap(),
                5,
                0,
            )],
        };

        assert_eq!(
            command_single_owner_region(&command(inside, same_region)),
            Some(RegionKey::new(0, 0))
        );
        assert_eq!(
            command_single_owner_region(&command(other_region, same_region)),
            None
        );
        assert_eq!(
            command_single_owner_region(&command(inside, other_region)),
            None
        );
        let mut cross_region_edit = command(inside, same_region);
        let SimulationCommand::ApplyBlockEdits { edits, .. } = &mut cross_region_edit else {
            unreachable!();
        };
        edits.push(BlockEdit {
            pos: other_region,
            new_state: BlockStateId(1),
        });
        assert_eq!(command_single_owner_region(&cross_region_edit), None);
    }

    #[test]
    fn active_region_phase_rejects_routed_command_without_mutation() {
        let registry = SessionRegistry::new();
        let position = Vec3::new(0.5, 64.0, 0.5);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let active_phase = owner
            .region_ownership
            .begin_phase()
            .expect("occupy regional phase");
        let response = handle
            .enqueue(SimulationCommand::SpawnCommandEntity {
                entity_type_id: 4,
                entity_type_name: "minecraft:zombie".to_owned(),
                position,
            })
            .expect("spawn command fits");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            response.blocking_recv().expect("owner response"),
            Err(SimulationRequestError::InvalidCommand)
        ));
        assert!(registry.nearby_hostile_entities(position, 2.25).is_empty());
        owner
            .region_ownership
            .finish_phase(active_phase)
            .expect("release occupied phase");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn barrier_completes_after_every_earlier_command() {
        let registry = SessionRegistry::new();
        let position = Vec3::new(1.5, 64.0, 0.5);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let spawn = handle
            .enqueue(SimulationCommand::SpawnCommandEntity {
                entity_type_id: 4,
                entity_type_name: "minecraft:zombie".to_owned(),
                position,
            })
            .unwrap();
        let mut barrier = Box::pin(handle.save_barrier(false));
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(barrier.as_mut(), cx).is_pending(),
                "barrier must wait for its owner response"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(handle.snapshot().depth, 2);

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            spawn.await.unwrap().unwrap(),
            SimulationResponse::EntitySpawn(_)
        ));
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(barrier.as_mut(), cx).is_pending(),
                "barrier must remain pending until its own ordered command runs"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(registry.nearby_hostile_entities(position, 2.25).len(), 1);

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let snapshot = barrier.await.expect("save barrier snapshot");
        assert_eq!(snapshot.entities.records.len(), 1);
        assert_eq!(snapshot.entities.records[0].type_name, "minecraft:zombie");
    }

    #[test]
    fn owner_tick_phase_controls_entity_lifecycle_expiry() {
        let registry = SessionRegistry::new();
        let position = Vec3::new(0.5, 64.0, 0.5);
        registry.spawn_item_drop(1, position, EntityItemStack::new(42, 1));
        let (_handle, owner) = simulation_channel_with_capacity(1);

        assert_eq!(
            owner.advance_world_time(&registry, super::super::ITEM_DESPAWN_AGE_TICKS),
            super::super::ITEM_DESPAWN_AGE_TICKS
        );
        owner.apply_entity_physics(&registry, super::super::ITEM_DESPAWN_AGE_TICKS, &[]);

        assert!(registry.nearby_item_entities(position, 2.25).is_empty());
    }

    #[test]
    fn queued_chunk_herd_spawn_deduplicates_chunk() {
        let chunk = (1, 1);
        let position = Vec3::new(24.5, 64.0, 24.5);
        let spawns = vec![super::super::HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position,
            hostile: false,
            sheep_color: None,
        }];
        let queued = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let first = handle
            .enqueue(SimulationCommand::EnsureChunkHerd {
                chunk,
                spawns: spawns.clone(),
            })
            .unwrap();
        let duplicate = handle
            .enqueue(SimulationCommand::EnsureChunkHerd { chunk, spawns })
            .unwrap();

        assert_eq!(owner.process_tick(&queued, 2).processed, 2);
        let first_dispatches = match first.blocking_recv().unwrap().unwrap() {
            SimulationResponse::EntitySpawn(dispatches) => dispatches,
            other => panic!("expected herd spawn response, got {other:?}"),
        };
        let duplicate_dispatches = match duplicate.blocking_recv().unwrap().unwrap() {
            SimulationResponse::EntitySpawn(dispatches) => dispatches,
            other => panic!("expected herd dedupe response, got {other:?}"),
        };

        assert!(first_dispatches.is_empty());
        assert!(duplicate_dispatches.is_empty());
        let persisted = queued.persisted_entity_records();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].snapshot.type_name, "minecraft:cow");
        assert_eq!(persisted[0].snapshot.position, position);
    }

    #[test]
    fn detached_chunk_herds_share_one_owner_batch() {
        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        for (chunk, position) in [
            ((1, 1), Vec3::new(24.5, 64.0, 24.5)),
            ((2, 1), Vec3::new(40.5, 64.0, 24.5)),
        ] {
            handle
                .ensure_chunk_herd(
                    chunk,
                    vec![super::super::HerdSpawn {
                        chunk,
                        slot: 0,
                        entity_type_id: 4,
                        entity_type_name: "minecraft:cow".to_owned(),
                        position,
                        hostile: false,
                        sheep_color: None,
                    }],
                )
                .expect("queue detached herd");
        }
        registry.reset_entity_owner_requests_for_test();

        let report = owner.process_tick(&registry, 2);

        assert_eq!(report.processed, 2);
        assert_eq!(registry.entity_owner_requests_for_test(), 1);
        assert_eq!(registry.persisted_entity_records().len(), 2);
    }

    #[test]
    fn detached_safe_herd_failure_releases_enqueue_claim_for_one_retry() {
        let chunk = (3, 2);
        let commits = Arc::new(AtomicUsize::new(0));
        let registry = SessionRegistry::new_with_entity_owner_journal(
            1,
            Box::new(FailOnceEntityCommitJournal {
                failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
                commits: Arc::clone(&commits),
            }),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let spawns = vec![super::super::HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(56.5, 64.0, 40.5),
            hostile: false,
            sheep_color: None,
        }];

        handle
            .ensure_chunk_herd(chunk, spawns.clone())
            .expect("queue first detached herd");
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(registry.persisted_entity_records().is_empty());

        handle
            .ensure_chunk_herd(chunk, spawns.clone())
            .expect("safe failure releases detached herd claim");
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(registry.persisted_entity_records().len(), 1);
        assert_eq!(commits.load(Ordering::Relaxed), 2);

        handle
            .ensure_chunk_herd(chunk, spawns)
            .expect("committed herd remains coalesced");
        assert_eq!(handle.snapshot().enqueued, 2);
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn world_time_handles_enforce_player_and_server_fences_and_owner_ordering() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "TimeFenceOwner");
        let (handle, mut owner) = simulation_channel_with_capacity(1);

        assert_eq!(
            handle.set_world_time(1).await,
            Err(SimulationRequestError::InvalidCommand)
        );
        assert_eq!(
            handle
                .for_session(session)
                .set_world_time_server_owned(1)
                .await,
            Err(SimulationRequestError::InvalidCommand)
        );

        let mut request =
            Box::pin(handle.set_world_time_server_owned(super::super::NIGHT_START_TICK));
        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(registry.world_time(), 0);

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        request.await.expect("server-owned time set response");
        assert_eq!(registry.world_time(), super::super::NIGHT_START_TICK);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_time_set_safe_failure_releases_pending_herd_for_exact_retry() {
        let chunk = (6, 2);
        let commits = Arc::new(AtomicUsize::new(0));
        let registry = SessionRegistry::new_with_entity_owner_journal(
            1,
            Box::new(FailOnceEntityCommitJournal {
                failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
                commits: Arc::clone(&commits),
            }),
        );
        let observer = register_test_session(&registry, "TimeSetSafeRetryObserver");
        registry.mark_loaded(observer, chunk);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let spawns = vec![super::super::HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 5,
            entity_type_name: "minecraft:zombie".to_owned(),
            position: Vec3::new(104.5, 64.0, 40.5),
            hostile: true,
            sheep_color: None,
        }];

        handle
            .ensure_chunk_herd(chunk, spawns.clone())
            .expect("queue daytime hostile herd");
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(registry.persisted_entity_records().is_empty());

        let session_handle = handle.for_session(observer);
        let mut time_set = Box::pin(session_handle.set_world_time(super::super::NIGHT_START_TICK));
        assert_request_enqueued(time_set.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        time_set.await.expect("session-fenced time set response");
        assert!(registry.persisted_entity_records().is_empty());

        handle
            .ensure_chunk_herd(chunk, spawns)
            .expect("SAFE time-set failure releases exact herd request");
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(registry.persisted_entity_records().len(), 1);
        assert_eq!(commits.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn natural_night_safe_failure_releases_pending_herd_for_exact_retry() {
        let chunk = (7, 2);
        let commits = Arc::new(AtomicUsize::new(0));
        let registry = SessionRegistry::new_with_entity_owner_journal(
            1,
            Box::new(FailOnceEntityCommitJournal {
                failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
                commits: Arc::clone(&commits),
            }),
        );
        let observer = register_test_session(&registry, "NaturalNightSafeRetryObserver");
        registry.mark_loaded(observer, chunk);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let spawns = vec![super::super::HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 5,
            entity_type_name: "minecraft:zombie".to_owned(),
            position: Vec3::new(120.5, 64.0, 40.5),
            hostile: true,
            sheep_color: None,
        }];

        handle
            .ensure_chunk_herd(chunk, spawns.clone())
            .expect("queue daytime hostile herd");
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(registry.persisted_entity_records().is_empty());

        assert_eq!(
            owner.advance_world_time(&registry, super::super::NIGHT_START_TICK),
            super::super::NIGHT_START_TICK
        );
        assert!(registry.persisted_entity_records().is_empty());

        handle
            .ensure_chunk_herd(chunk, spawns)
            .expect("SAFE natural-night failure releases exact herd request");
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(registry.persisted_entity_records().len(), 1);
        assert_eq!(commits.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn owner_night_transition_activates_pending_hostiles_once() {
        let chunk = (1, 1);
        let registry = SessionRegistry::new();
        let (tx, _rx) = mpsc::channel(4);
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "night-observer".to_owned(),
        };
        let (session_id, _) = registry.register(
            &profile,
            (0, 0),
            0,
            HashSet::from([chunk]),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        registry.mark_loaded(session_id, chunk);
        let spawns = [
            super::super::HerdSpawn {
                chunk,
                slot: 0,
                entity_type_id: 4,
                entity_type_name: "minecraft:cow".to_owned(),
                position: Vec3::new(24.5, 64.0, 24.5),
                hostile: false,
                sheep_color: None,
            },
            super::super::HerdSpawn {
                chunk,
                slot: 1,
                entity_type_id: 5,
                entity_type_name: "minecraft:zombie".to_owned(),
                position: Vec3::new(25.5, 64.0, 24.5),
                hostile: true,
                sheep_color: None,
            },
        ];

        registry.ensure_chunk_herd_legacy_for_test(chunk, &spawns);
        let entity_types = || {
            registry
                .persisted_entity_records()
                .into_iter()
                .map(|record| record.snapshot.type_name)
                .collect::<Vec<_>>()
        };
        assert_eq!(entity_types(), ["minecraft:cow"]);

        let (_handle, owner) = simulation_channel_with_capacity(1);
        assert_eq!(
            owner.advance_world_time(&registry, super::super::NIGHT_START_TICK - 1),
            super::super::NIGHT_START_TICK - 1
        );
        assert_eq!(entity_types(), ["minecraft:cow"]);

        assert_eq!(
            owner.advance_world_time(&registry, 1),
            super::super::NIGHT_START_TICK
        );
        let mut nighttime_types = entity_types();
        nighttime_types.sort();
        assert_eq!(nighttime_types, ["minecraft:cow", "minecraft:zombie"]);

        owner.advance_world_time(&registry, 1);
        assert_eq!(entity_types().len(), 2);
    }

    #[test]
    fn queued_time_set_cannot_overtake_herd_admission() {
        let chunk = (5, 5);
        let registry = Arc::new(SessionRegistry::new());
        let observer = register_test_session(&registry, "OrderedNightObserver");
        registry.mark_loaded(observer, chunk);
        let (claim_open_tx, claim_open_rx) = std::sync::mpsc::sync_channel(0);
        let (release_claim_tx, release_claim_rx) = std::sync::mpsc::sync_channel(0);
        registry.install_chunk_herd_claim_probe_for_test(claim_open_tx, release_claim_rx);

        let (handle, mut owner) = simulation_channel_with_capacity(2);
        handle
            .ensure_chunk_herd(
                chunk,
                vec![super::super::HerdSpawn {
                    chunk,
                    slot: 0,
                    entity_type_id: 5,
                    entity_type_name: "minecraft:zombie".to_owned(),
                    position: Vec3::new(88.5, 64.0, 88.5),
                    hostile: true,
                    sheep_color: None,
                }],
            )
            .expect("queue detached hostile herd");

        let owner_registry = Arc::clone(&registry);
        let owner_thread = std::thread::spawn(move || {
            let first = owner.process_tick(&owner_registry, 2);
            let second = owner.process_tick(&owner_registry, 2);
            (first.processed, second.processed)
        });
        claim_open_rx
            .recv()
            .expect("herd reached its session claim boundary");
        let time_response = handle
            .for_session(observer)
            .enqueue(SimulationCommand::SetWorldTime {
                world_time: super::super::NIGHT_START_TICK,
            })
            .expect("queue session-fenced time set behind herd insertion");
        release_claim_tx.send(()).expect("release herd admission");

        assert_eq!(owner_thread.join().expect("simulation owner"), (1, 1));
        assert!(matches!(
            time_response.blocking_recv().expect("time set response"),
            Ok(SimulationResponse::WorldTimeSet)
        ));
        let records = registry.persisted_entity_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].snapshot.type_name, "minecraft:zombie");
        assert_eq!(registry.world_time(), super::super::NIGHT_START_TICK);
    }

    #[test]
    fn stale_session_time_set_is_rejected_without_changing_time() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleTimeSetter");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue(SimulationCommand::SetWorldTime {
                world_time: super::super::NIGHT_START_TICK,
            })
            .expect("queue fenced time set");
        registry.unregister(session);

        owner.process_tick(&registry, 1);

        assert!(matches!(
            response.blocking_recv().expect("time set response"),
            Err(SimulationRequestError::StaleSession)
        ));
        assert_eq!(registry.world_time(), 0);
        assert_eq!(handle.snapshot().rejected_stale_session, 1);
    }

    #[test]
    fn detached_chunk_herd_applies_without_being_counted_as_cancelled() {
        let chunk = (2, 2);
        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(1);

        handle
            .ensure_chunk_herd(
                chunk,
                vec![super::super::HerdSpawn {
                    chunk,
                    slot: 0,
                    entity_type_id: 4,
                    entity_type_name: "minecraft:cow".to_owned(),
                    position: Vec3::new(40.5, 64.0, 40.5),
                    hostile: false,
                    sheep_color: None,
                }],
            )
            .expect("detached herd command enqueues");

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert_eq!(registry.persisted_entity_records().len(), 1);
        assert_eq!(handle.snapshot().cancelled, 0);
        assert_eq!(handle.snapshot().processed, 1);
    }

    #[test]
    fn detached_chunk_herd_enqueues_each_chunk_once() {
        let chunk = (3, 3);
        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(2);

        handle
            .ensure_chunk_herd(chunk, Vec::new())
            .expect("first herd command enqueues");
        handle
            .ensure_chunk_herd(chunk, Vec::new())
            .expect("duplicate herd command coalesces");

        assert_eq!(handle.snapshot().enqueued, 1);
        assert_eq!(handle.snapshot().depth, 1);
        assert_eq!(owner.process_tick(&registry, 2).processed, 1);
        assert_eq!(handle.snapshot().depth, 0);
    }

    #[test]
    fn concurrent_chunk_herd_waiter_observes_winning_enqueue_failure() {
        let chunk = (4, 4);
        let (handle, _owner) = simulation_channel_with_capacity(1);
        let (winner_claimed_tx, winner_claimed_rx) = std::sync::mpsc::sync_channel(0);
        let (release_winner_tx, release_winner_rx) = std::sync::mpsc::sync_channel(0);
        let (waiter_blocked_tx, waiter_blocked_rx) = std::sync::mpsc::sync_channel(0);
        handle.install_herd_enqueue_probe_for_test(
            winner_claimed_tx,
            release_winner_rx,
            waiter_blocked_tx,
        );

        let winner_handle = handle.clone();
        let winner = std::thread::spawn(move || winner_handle.ensure_chunk_herd(chunk, Vec::new()));
        winner_claimed_rx
            .recv()
            .expect("winning producer claimed herd chunk");

        handle
            .enqueue(SimulationCommand::EnsureChunkHerd {
                chunk: (9, 9),
                spawns: Vec::new(),
            })
            .expect("fill simulation queue after winner claims");

        let waiter_handle = handle.clone();
        let waiter = std::thread::spawn(move || waiter_handle.ensure_chunk_herd(chunk, Vec::new()));
        waiter_blocked_rx
            .recv()
            .expect("losing producer waits for winning enqueue result");
        release_winner_tx
            .send(())
            .expect("release winning herd producer");

        assert_eq!(
            winner.join().expect("winning producer"),
            Err(SimulationRequestError::Full)
        );
        assert_eq!(
            waiter.join().expect("waiting producer"),
            Err(SimulationRequestError::Full)
        );
        assert_eq!(handle.snapshot().enqueued, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_break_transaction_commits_block_tool_and_drop_together() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "AtomicBreakMiner");
        let mut inventory = PlayerInventory::empty();
        let tool_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[tool_slot] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let plan = test_survival_break_plan(pos, token, 42, 7);
        let mut request = Box::pin(session_handle.commit_survival_break(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        let committed = request
            .await
            .expect("break response")
            .expect("matching break commits");

        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(0))
        );
        let player_state = player_state.lock().unwrap();
        assert_eq!(player_state.inventory.slots[tool_slot].damage, Some(1));
        assert_eq!(
            committed.inventory.slots[tool_slot],
            player_state.inventory.slots[tool_slot]
        );
        drop(player_state);
        let drops = registry
            .persisted_entity_records()
            .into_iter()
            .filter(|record| record.snapshot.item_stack.is_some())
            .collect::<Vec<_>>();
        assert_eq!(drops.len(), 1);
        assert_eq!(
            drops[0].snapshot.item_stack,
            Some(EntityItemStack::new(7, 1))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_block_break_is_planned_and_committed_by_the_owner() {
        let (mut storage, pos, _) = test_block_storage();
        let water_neighbor = BlockPos {
            x: pos.x + 1,
            ..pos
        };
        storage
            .set_block_at(water_neighbor, BlockStateId(2))
            .unwrap();
        let token = storage
            .block_mutation_token(pos)
            .expect("root token after neighbouring fluid edit");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "OwnerPlannedMiner");
        let mut inventory = PlayerInventory::empty();
        let tool_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[tool_slot] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut plan = test_survival_block_break_plan(pos, token);
        plan.loot = Arc::new(mc_data::loot::LootTables::from_drop_maps(
            BTreeMap::new(),
            BTreeMap::from([(
                Identifier::parse("minecraft:stone").unwrap(),
                mc_data::loot::LootDrop::uniform(
                    Identifier::parse("minecraft:cobblestone").unwrap(),
                    4,
                    9,
                ),
            )]),
        ));
        let expected_count = mc_data::loot::LootCount::UniformInclusive { min: 4, max: 9 }
            .try_sample(super::super::block_break_loot_seed(
                pos,
                BlockStateId(1),
                token,
            ))
            .unwrap();
        let mut request = Box::pin(session_handle.commit_survival_block_break(plan));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        let committed = request
            .await
            .expect("break response")
            .expect("matching break commits");

        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(2))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[tool_slot].damage,
            Some(1)
        );
        assert_eq!(committed.block.applied.len(), 1);
        let drops = registry
            .persisted_entity_records()
            .into_iter()
            .filter_map(|record| record.snapshot.item_stack)
            .collect::<Vec<_>>();
        assert_eq!(
            drops,
            vec![EntityItemStack::new(
                7,
                i32::try_from(expected_count).unwrap()
            )]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resident_survival_break_and_relight_do_not_wait_for_world_writer() {
        let (mut storage, pos, _) = test_block_storage();
        let water_neighbor = BlockPos {
            x: pos.x + 1,
            ..pos
        };
        storage
            .set_block_at(water_neighbor, BlockStateId(2))
            .unwrap();
        let token = storage.block_mutation_token(pos).unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let session = register_test_session(&registry, "RegionalBreakMiner");
        let mut inventory = PlayerInventory::empty();
        let tool_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[tool_slot] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let light = Arc::new(BlockLightTable::from_arrays(
            "regional break publication",
            vec![0, 0, 0, 0, 0],
            vec![0, 15, 0, 0, 0],
            vec![true, false, true, true, true],
        ));
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let plan = test_survival_block_break_plan(pos, token);
        let mut request = Box::pin(session_handle.commit_survival_block_break(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_registry = Arc::clone(&registry);
        let owner_read = read_view.clone();
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_registry,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&owner_read),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: Some(&light),
                    },
                    Some(light.as_ref()),
                    1,
                )
                .await
        });

        let committed = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .expect("resident survival break completion event")
            .expect("resident survival break response")
            .expect("matching resident survival break commits");
        drop(writer);

        assert_eq!(owner_task.await.unwrap().processed, 1);
        assert_eq!(read_view.get_cached_block(pos), Some(BlockStateId(2)));
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[tool_slot].damage,
            Some(1)
        );
        assert_eq!(committed.block.applied.len(), 1);
        assert!(committed.block.precomputed_light_updates.is_some());
        let chunk = read_view
            .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap();
        assert!(
            chunk
                .scheduled_fluid_ticks()
                .iter()
                .any(|tick| tick.pos == pos)
        );
        assert!(mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some());
        assert_eq!(persisted_item_drop_count(&registry), 1);
    }

    #[test]
    fn survival_block_breaks_in_distinct_regions_overlap() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(blocks);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
        for chunk in chunks {
            storage
                .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
                .unwrap();
        }
        let positions = [
            BlockPos { x: 1, y: 64, z: 1 },
            BlockPos {
                x: 8 * 16 + 1,
                y: 64,
                z: 1,
            },
        ];
        for position in positions {
            storage.set_block_at(position, BlockStateId(1)).unwrap();
        }
        let tokens = positions.map(|position| storage.block_mutation_token(position).unwrap());
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let light = Arc::new(BlockLightTable::from_arrays(
            "regional survival break",
            vec![0, 0, 0, 0, 0],
            vec![0, 15, 0, 0, 0],
            vec![true, false, true, true, true],
        ));
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actors = [
            register_test_session(&sessions, "RegionalMinerA"),
            register_test_session(&sessions, "RegionalMinerB"),
        ];
        let player_states = actors.map(|actor| {
            let mut inventory = PlayerInventory::empty();
            inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
            register_test_player_state(&sessions, actor, inventory)
        });
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = (0..2)
            .map(|index| {
                handle
                    .for_session(actors[index])
                    .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                        SurvivalBreakCommand {
                            actor_session: actors[index],
                            request: SurvivalBreakRequest::Block(test_survival_block_break_plan(
                                positions[index],
                                tokens[index],
                            )),
                        },
                    )))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        owner.install_regional_block_edit_probe(entered_tx, release_rx);

        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let worker = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(owner.process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: Some(&light),
                    },
                    Some(light.as_ref()),
                    2,
                ))
        });

        let first = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first regional survival break worker entry");
        let second = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second regional survival break worker enters before release");
        assert_ne!(first, second);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();

        assert_eq!(worker.join().unwrap().processed, 2);
        for response in responses {
            let SimulationResponse::SurvivalBreak(Ok(Some(committed))) =
                response.blocking_recv().unwrap().unwrap()
            else {
                panic!("regional survival break response mismatch");
            };
            assert!(committed.block.precomputed_light_updates.is_some());
        }
        for (position, player_state) in positions.into_iter().zip(player_states) {
            assert_eq!(
                world.blocking_lock().get_cached_block(position),
                Some(BlockStateId(0))
            );
            assert_eq!(
                player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
                Some(1)
            );
        }
        assert_eq!(persisted_item_drop_count(&sessions), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn owner_planned_survival_break_rejects_a_stale_root_without_side_effects() {
        let (storage, pos, token) = test_block_storage();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleOwnerPlannedMiner");
        let mut inventory = PlayerInventory::empty();
        let tool_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[tool_slot] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(
            session_handle.commit_survival_block_break(test_survival_block_break_plan(pos, token)),
        );
        assert_request_enqueued(request.as_mut(), &handle).await;

        world
            .lock()
            .await
            .set_block_at(pos, BlockStateId(3))
            .unwrap();
        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &registry,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
                .processed,
            1
        );

        assert!(request.await.expect("break response").is_none());
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(3))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[tool_slot].damage,
            None
        );
        assert_eq!(persisted_item_drop_count(&registry), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_tool_edit_honours_extra_world_preconditions_atomically() {
        let (storage, pos, token) = test_block_storage();
        let above = BlockPos {
            y: pos.y + 1,
            ..pos
        };
        let above_token = storage
            .block_mutation_token(above)
            .expect("resident block above token");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "AtomicHoeUser");
        let mut inventory = PlayerInventory::empty();
        let tool_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[tool_slot] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut plan = test_survival_break_plan(pos, token, 42, 7);
        plan.preconditions.push(BlockEditPrecondition {
            pos: above,
            expected_state: BlockStateId(0),
            expected_token: above_token,
        });
        plan.falling_block_entity_type_id = None;
        plan.drops.clear();
        let mut request = Box::pin(session_handle.commit_survival_break(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        request
            .await
            .expect("tool edit response")
            .expect("matching world guards commit");
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[tool_slot].damage,
            Some(1)
        );
    }

    #[test]
    fn survival_tool_edit_rejects_stale_extra_guard_without_tool_damage() {
        let (storage, pos, token) = test_block_storage();
        let above = BlockPos {
            y: pos.y + 1,
            ..pos
        };
        let above_token = storage
            .block_mutation_token(above)
            .expect("resident block above token");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleHoeUser");
        let mut inventory = PlayerInventory::empty();
        let tool_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[tool_slot] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let mut plan = test_survival_break_plan(pos, token, 42, 7);
        plan.preconditions.push(BlockEditPrecondition {
            pos: above,
            expected_state: BlockStateId(0),
            expected_token: above_token,
        });
        plan.falling_block_entity_type_id = None;
        plan.drops.clear();
        world
            .blocking_lock()
            .set_block_at(above, BlockStateId(2))
            .expect("replace block above after planning");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: session,
                    request: SurvivalBreakRequest::Prepared(plan),
                },
            )))
            .unwrap();

        owner.process_tick_with_world(&registry, Some(&world), None, 1);
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalBreak(Ok(None))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(pos),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[tool_slot].damage,
            None
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_placement_transaction_commits_block_and_inventory_debit_together() {
        let (mut storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        assert_eq!(storage.get_block(target).unwrap(), Some(BlockStateId(0)));
        let target_token = storage
            .block_mutation_token(target)
            .expect("resident placement target token");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "AtomicPlacementBuilder");
        let mut inventory = PlayerInventory::empty();
        let held_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[held_slot] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        plan.scheduled_block_ticks.push(ScheduledBlockTick::new(
            target,
            Identifier::parse("minecraft:stone").unwrap(),
            8,
            0,
        ));
        let mut request = Box::pin(session_handle.commit_survival_placement(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        let committed = request
            .await
            .expect("placement response")
            .expect("matching placement commits");

        let mut world = world.lock().await;
        assert_eq!(world.get_cached_block(target), Some(BlockStateId(1)));
        let scheduled = world
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("placement schedules owner tick");
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, target);
        assert_eq!(scheduled[0].trigger_tick, 8);
        drop(world);
        let player_state = player_state.lock().unwrap();
        assert_eq!(
            player_state.inventory.slots[held_slot],
            ItemStack::new(42, 1)
        );
        assert_eq!(
            committed.inventory.slots[held_slot],
            player_state.inventory.slots[held_slot]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creative_placement_transaction_commits_without_inventory_debit() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage
            .block_mutation_token(target)
            .expect("resident placement target token");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "CreativePlacementBuilder");
        let mut inventory = PlayerInventory::empty();
        let held_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[held_slot] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        player_state.lock().unwrap().game_mode = GameMode::Creative;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        plan.expected_game_mode = GameMode::Creative;
        let mut request = Box::pin(session_handle.commit_survival_placement(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        let committed = request
            .await
            .expect("placement response")
            .expect("matching creative placement commits");

        assert_eq!(
            world.lock().await.get_cached_block(target),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[held_slot],
            ItemStack::new(42, 2)
        );
        assert_eq!(committed.inventory.slots[held_slot], ItemStack::new(42, 2));
        assert!(committed.changed_slots.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resident_survival_placement_does_not_wait_for_world_writer() {
        let (mut storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let water_neighbor = BlockPos {
            x: target.x + 1,
            ..target
        };
        storage
            .set_block_at(water_neighbor, BlockStateId(2))
            .unwrap();
        let target_token = storage
            .block_mutation_token(target)
            .expect("resident placement target token");
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let session = register_test_session(&registry, "RegionalPlacementBuilder");
        let mut inventory = PlayerInventory::empty();
        let held_slot = PlayerInventory::HOTBAR_BASE;
        inventory.slots[held_slot] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        let mut request = Box::pin(session_handle.commit_survival_placement(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_registry = Arc::clone(&registry);
        let owner_read = read_view.clone();
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_registry,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&owner_read),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });

        let committed = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .expect("resident placement completion event")
            .expect("resident placement response")
            .expect("matching resident placement commits");
        drop(writer);

        assert_eq!(owner_task.await.unwrap().processed, 1);
        assert_eq!(read_view.get_cached_block(target), Some(BlockStateId(1)));
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[held_slot],
            ItemStack::new(42, 1)
        );
        assert_eq!(committed.block.applied.len(), 1);
        let chunk = read_view
            .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap();
        assert!(
            chunk
                .scheduled_fluid_ticks()
                .iter()
                .any(|tick| tick.pos == water_neighbor)
        );
    }

    #[test]
    fn survival_placements_in_distinct_regions_overlap() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(blocks);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
        for chunk in chunks {
            storage
                .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
                .unwrap();
        }
        let supports = [
            BlockPos { x: 1, y: 64, z: 1 },
            BlockPos {
                x: 8 * 16 + 1,
                y: 64,
                z: 1,
            },
        ];
        for support in supports {
            storage.set_block_at(support, BlockStateId(1)).unwrap();
        }
        let targets = supports.map(|support| BlockPos {
            x: support.x + 1,
            ..support
        });
        let support_tokens = supports.map(|pos| storage.block_mutation_token(pos).unwrap());
        let target_tokens = targets.map(|pos| storage.block_mutation_token(pos).unwrap());
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let light = Arc::new(BlockLightTable::from_arrays(
            "regional survival placement",
            vec![0, 0, 0, 0, 0],
            vec![0, 15, 0, 0, 0],
            vec![true, false, true, true, true],
        ));
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actors = [
            register_test_session(&sessions, "RegionalBuilderA"),
            register_test_session(&sessions, "RegionalBuilderB"),
        ];
        let player_states = actors.map(|actor| {
            let mut inventory = PlayerInventory::empty();
            inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
            register_test_player_state(&sessions, actor, inventory)
        });
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = (0..2)
            .map(|index| {
                handle
                    .for_session(actors[index])
                    .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                        SurvivalPlacementCommand {
                            actor_session: actors[index],
                            plan: test_survival_placement_plan(
                                targets[index],
                                target_tokens[index],
                                supports[index],
                                support_tokens[index],
                                42,
                                2,
                            ),
                        },
                    )))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        owner.install_regional_block_edit_probe(entered_tx, release_rx);

        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let worker = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(owner.process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: Some(&light),
                    },
                    Some(light.as_ref()),
                    2,
                ))
        });

        let first = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first regional placement worker entry");
        let second = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second regional placement worker enters before release");
        assert_ne!(first, second);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();

        assert_eq!(worker.join().unwrap().processed, 2);
        for response in responses {
            let SimulationResponse::SurvivalPlacement(Ok(Some(committed))) =
                response.blocking_recv().unwrap().unwrap()
            else {
                panic!("regional survival placement response mismatch");
            };
            assert!(committed.block.precomputed_light_updates.is_some());
        }
        for (target, player_state) in targets.into_iter().zip(player_states) {
            assert_eq!(
                world.blocking_lock().get_cached_block(target),
                Some(BlockStateId(1))
            );
            assert_eq!(
                player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
                ItemStack::new(42, 1)
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn offhand_food_use_transaction_commits_inventory_and_hunger_together() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "AtomicFoodPlayer");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let expected_survival = SurvivalState {
            food: 16,
            saturation: 1.0,
            ..SurvivalState::FULL
        };
        player_state.lock().unwrap().survival = expected_survival;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_food_use(FoodUsePlan {
            held_slot: PlayerInventory::OFFHAND_SLOT,
            expected_held: ItemStack::new(42, 2),
            expected_survival,
            food: 4,
            saturation: 2.4,
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let committed = request
            .await
            .expect("food response")
            .expect("matching food use commits");

        let player_state = player_state.lock().unwrap();
        assert_eq!(
            player_state.inventory.slots[PlayerInventory::OFFHAND_SLOT],
            ItemStack::new(42, 1)
        );
        assert_eq!(player_state.survival.food, 20);
        assert!(player_state.survival.saturation > expected_survival.saturation);
        assert_eq!(committed.inventory.slots, player_state.inventory.slots);
        assert_eq!(committed.survival, player_state.survival);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn animal_feed_transaction_debits_wheat_and_enters_love_together() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "AtomicAnimalFeeder");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());

        let wheat_item_id = 42;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        let mut inventory = PlayerInventory::empty();
        inventory.slots[held_slot] = ItemStack::new(wheat_item_id, 2);
        let player_state = register_test_player_state(&registry, session, inventory);

        let position = Vec3::new(1.5, 64.0, 0.5);
        let spawns = vec![super::super::HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position,
            hostile: false,
            sheep_color: None,
        }];
        let entity_id = publish_entity_spawns(
            registry.ensure_chunk_herd_legacy_for_test((0, 0), &spawns),
            &mut outbound,
        )[0];

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_animal_feed(AnimalFeedPlan {
            entity_id,
            held_slot,
            expected_held: ItemStack::new(wheat_item_id, 2),
            food_item_id: wheat_item_id,
            targets: AnimalFeedTargets {
                cow: true,
                sheep: true,
                chicken: false,
            },
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let committed = request
            .await
            .expect("animal feed response")
            .expect("matching animal feed commits");

        {
            let player_state = player_state.lock().unwrap();
            assert_eq!(
                player_state.inventory.slots[held_slot],
                ItemStack::new(wheat_item_id, 1)
            );
            assert_eq!(committed.inventory.slots, player_state.inventory.slots);
        }
        assert_eq!(
            registry
                .server_entity_snapshot(entity_id)
                .and_then(|entity| entity.animal)
                .expect("cow breeding state")
                .love_ticks,
            mc_entity::ANIMAL_LOVE_DURATION_TICKS
        );
        assert!(matches!(
            outbound.recv().await,
            Some(OutboundCommand::EntityEvent {
                entity_id: wire_id,
                event_id: 18,
            }) if wire_id == entity_id.0
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sheep_shear_transaction_damages_tool_marks_entity_and_spawns_wool_once() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "AtomicShearer");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());

        let shears_item_id = 42;
        let wool_item_id = 43;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        let mut inventory = PlayerInventory::empty();
        inventory.slots[held_slot] = ItemStack::new(shears_item_id, 1);
        let player_state = register_test_player_state(&registry, session, inventory);

        let position = Vec3::new(1.5, 64.0, 0.5);
        let entity_id = publish_entity_spawns(
            registry.ensure_chunk_herd_legacy_for_test(
                (0, 0),
                &[super::super::HerdSpawn {
                    chunk: (0, 0),
                    slot: 0,
                    entity_type_id: 4,
                    entity_type_name: "minecraft:sheep".to_owned(),
                    position,
                    hostile: false,
                    sheep_color: None,
                }],
            ),
            &mut outbound,
        )[0];

        let plan = SheepShearPlan {
            entity_id,
            held_slot,
            expected_held: ItemStack::new(shears_item_id, 1),
            shears_item_id,
            shears_max_damage: 238,
            item_entity_type_id: 1,
            wool_item_ids: [wool_item_id; 16],
        };
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_sheep_shear(plan.clone()));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let committed = request
            .await
            .expect("sheep shear response")
            .expect("matching sheep shear commits");
        assert!((1..=3).contains(&committed.drop_count));
        assert_eq!(
            committed.inventory.slots[held_slot],
            ItemStack::new(shears_item_id, 1).with_damage(1)
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[held_slot],
            ItemStack::new(shears_item_id, 1).with_damage(1)
        );
        assert!(
            registry
                .server_entity_snapshot(entity_id)
                .and_then(|entity| entity.animal)
                .and_then(|animal| animal.sheep_wool)
                .is_some_and(|wool| wool.sheared)
        );

        let mut metadata_updates = 0;
        let mut wool_spawns = 0;
        while let Ok(command) = outbound.try_recv() {
            match command {
                OutboundCommand::UpdateEntityData(entity) if entity.id == entity_id => {
                    metadata_updates += 1;
                }
                OutboundCommand::SpawnEntity(entity)
                    if entity
                        .item_stack
                        .as_ref()
                        .is_some_and(|stack| stack.item_id == wool_item_id && stack.count == 1) =>
                {
                    wool_spawns += 1;
                }
                _ => {}
            }
        }
        assert_eq!(metadata_updates, 1);
        assert_eq!(wool_spawns, committed.drop_count);

        let second_plan = SheepShearPlan {
            expected_held: ItemStack::new(shears_item_id, 1).with_damage(1),
            ..plan
        };
        let session_handle = handle.for_session(session);
        let mut second = Box::pin(session_handle.commit_sheep_shear(second_plan));
        assert_request_enqueued(second.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 2).processed, 1);
        assert!(second.await.expect("second sheep shear response").is_none());
        assert!(outbound.try_recv().is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn animal_pair_breeds_on_the_sixtieth_simulation_tick() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "AnimalBreeder");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());

        let wheat_item_id = 42;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        let mut inventory = PlayerInventory::empty();
        inventory.slots[held_slot] = ItemStack::new(wheat_item_id, 2);
        register_test_player_state(&registry, session, inventory);

        let spawns = vec![
            super::super::HerdSpawn {
                chunk: (0, 0),
                slot: 0,
                entity_type_id: 4,
                entity_type_name: "minecraft:cow".to_owned(),
                position: Vec3::new(1.5, 64.0, 0.5),
                hostile: false,
                sheep_color: None,
            },
            super::super::HerdSpawn {
                chunk: (0, 0),
                slot: 1,
                entity_type_id: 4,
                entity_type_name: "minecraft:cow".to_owned(),
                position: Vec3::new(2.5, 64.0, 0.5),
                hostile: false,
                sheep_color: None,
            },
        ];
        let parent_ids = publish_entity_spawns(
            registry.ensure_chunk_herd_legacy_for_test((0, 0), &spawns),
            &mut outbound,
        );
        assert_eq!(parent_ids.len(), 2);
        registry.publish_active_simulation_entities_for_test(parent_ids.iter().copied());

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        for (index, entity_id) in parent_ids.iter().copied().enumerate() {
            let expected_count = 2 - index as i32;
            let session_handle = handle.for_session(session);
            let mut request = Box::pin(session_handle.commit_animal_feed(AnimalFeedPlan {
                entity_id,
                held_slot,
                expected_held: ItemStack::new(wheat_item_id, expected_count),
                food_item_id: wheat_item_id,
                targets: AnimalFeedTargets {
                    cow: true,
                    sheep: true,
                    chicken: false,
                },
            }));
            assert_request_enqueued(request.as_mut(), &handle).await;
            assert_eq!(owner.process_tick(&registry, 1).processed, 1);
            assert!(request.await.unwrap().is_some());
            assert!(matches!(
                outbound.recv().await,
                Some(OutboundCommand::EntityEvent { event_id: 18, .. })
            ));
        }

        for _ in 0..(mc_entity::ANIMAL_BREEDING_COURTSHIP_TICKS - 1) {
            assert_eq!(owner.tick_animal_breeding(&registry, 1), 0);
        }
        assert_eq!(
            registry
                .persisted_entity_records()
                .into_iter()
                .filter(|record| record.snapshot.type_name == "minecraft:cow")
                .count(),
            2
        );

        assert_eq!(owner.tick_animal_breeding(&registry, 1), 1);
        let cows = registry
            .persisted_entity_records()
            .into_iter()
            .filter(|record| record.snapshot.type_name == "minecraft:cow")
            .collect::<Vec<_>>();
        assert_eq!(cows.len(), 3);
        assert_eq!(
            cows.iter()
                .filter_map(|record| record.snapshot.animal)
                .filter(|animal| animal.age_ticks == mc_entity::PARENT_BREEDING_COOLDOWN_TICKS)
                .count(),
            2
        );
        assert_eq!(
            cows.iter()
                .filter_map(|record| record.snapshot.animal)
                .filter(|animal| animal.age_ticks == mc_entity::BABY_START_AGE_TICKS)
                .count(),
            1
        );
        assert!(matches!(
            outbound.recv().await,
            Some(OutboundCommand::SpawnEntity(entity))
                if entity.animal.is_some_and(|animal| animal.is_baby())
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn red_and_yellow_sheep_breed_an_orange_ecs_child() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "SheepColorBreeder");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());

        let wheat_item_id = 42;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        let mut inventory = PlayerInventory::empty();
        inventory.slots[held_slot] = ItemStack::new(wheat_item_id, 2);
        register_test_player_state(&registry, session, inventory);

        let spawns = vec![
            super::super::HerdSpawn {
                chunk: (0, 0),
                slot: 0,
                entity_type_id: 4,
                entity_type_name: "minecraft:sheep".to_owned(),
                position: Vec3::new(1.5, 64.0, 0.5),
                hostile: false,
                sheep_color: Some(mc_entity::SheepColor::Red),
            },
            super::super::HerdSpawn {
                chunk: (0, 0),
                slot: 1,
                entity_type_id: 4,
                entity_type_name: "minecraft:sheep".to_owned(),
                position: Vec3::new(2.5, 64.0, 0.5),
                hostile: false,
                sheep_color: Some(mc_entity::SheepColor::Yellow),
            },
        ];
        let parent_ids = publish_entity_spawns(
            registry.ensure_chunk_herd_legacy_for_test((0, 0), &spawns),
            &mut outbound,
        );
        assert_eq!(parent_ids.len(), 2);
        registry.publish_active_simulation_entities_for_test(parent_ids.iter().copied());

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        for (index, entity_id) in parent_ids.iter().copied().enumerate() {
            let expected_count = 2 - index as i32;
            let session_handle = handle.for_session(session);
            let mut request = Box::pin(session_handle.commit_animal_feed(AnimalFeedPlan {
                entity_id,
                held_slot,
                expected_held: ItemStack::new(wheat_item_id, expected_count),
                food_item_id: wheat_item_id,
                targets: AnimalFeedTargets {
                    cow: true,
                    sheep: true,
                    chicken: false,
                },
            }));
            assert_request_enqueued(request.as_mut(), &handle).await;
            assert_eq!(owner.process_tick(&registry, 1).processed, 1);
            assert!(request.await.unwrap().is_some());
            assert!(matches!(
                outbound.recv().await,
                Some(OutboundCommand::EntityEvent { event_id: 18, .. })
            ));
        }

        for _ in 0..mc_entity::ANIMAL_BREEDING_COURTSHIP_TICKS {
            owner.tick_animal_breeding(&registry, 1);
        }
        let child = registry
            .persisted_entity_records()
            .into_iter()
            .map(|record| record.snapshot)
            .find(|entity| {
                entity
                    .animal
                    .is_some_and(|animal| animal.age_ticks == mc_entity::BABY_START_AGE_TICKS)
            })
            .expect("bred sheep child");
        let child_wool = child
            .animal
            .and_then(|animal| animal.sheep_wool)
            .expect("bred sheep child wool state");

        assert_eq!(child_wool.color, mc_entity::SheepColor::Orange);
        assert!(matches!(
            outbound.recv().await,
            Some(OutboundCommand::SpawnEntity(entity))
                if entity.id == child.id
                    && entity
                        .animal
                        .and_then(|animal| animal.sheep_wool)
                        .is_some_and(|wool| wool.color == mc_entity::SheepColor::Orange)
        ));
    }

    #[test]
    fn hostile_melee_is_pushed_on_its_simulation_tick() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "HostileTarget");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        let entity_id = publish_entity_spawns(
            registry.spawn_command_entity(
                &SimulationAuthority::for_test(),
                1,
                "minecraft:zombie".to_owned(),
                Vec3::new(0.5, 64.0, 0.0),
            ),
            &mut outbound,
        )[0];
        let (_, owner) = simulation_channel();
        let phase = u64::from(entity_id.0.unsigned_abs()) % HOSTILE_MELEE_PERIOD_TICKS;
        let due_tick = if phase == 0 {
            HOSTILE_MELEE_PERIOD_TICKS
        } else {
            HOSTILE_MELEE_PERIOD_TICKS - phase
        };

        assert_eq!(
            owner.tick_hostile_attacks(&registry, due_tick - 1, BlockStateId(0)),
            0
        );
        assert!(outbound.try_recv().is_err());
        assert_eq!(
            owner.tick_hostile_attacks(&registry, due_tick, BlockStateId(0)),
            1
        );

        let commands = std::iter::from_fn(|| outbound.try_recv().ok()).collect::<Vec<_>>();
        assert!(commands.iter().any(|command| matches!(
            command,
            OutboundCommand::DamagePlayer {
                damage: super::super::PlayerDamageRequest {
                    kind: super::super::PlayerDamageKind::MobAttack,
                    amount,
                    source_origin: Some(origin),
                },
                ..
            } if (*amount - 3.0).abs() < f32::EPSILON
                && *origin == Vec3::new(0.5, 64.0, 0.0)
        )));
    }

    #[test]
    fn skeleton_shoots_a_real_arrow_on_its_simulation_tick() {
        let registry = SessionRegistry::new();
        registry.configure_arrow_kill_rewards(
            Some(2),
            Some(3),
            Some(77),
            Arc::new(ItemRegistry::from_report(&[])),
            Arc::new(mc_data::item_components::ItemFactsTable::default()),
            Arc::new(mc_data::loot::LootTables::default()),
        );
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "SkeletonTarget");
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        let entity_id = publish_entity_spawns(
            registry.spawn_command_entity(
                &SimulationAuthority::for_test(),
                4,
                "minecraft:skeleton".to_owned(),
                Vec3::new(0.5, 64.0, 6.5),
            ),
            &mut outbound,
        )[0];
        let (_, owner) = simulation_channel();
        let phase = u64::from(entity_id.0.unsigned_abs()) % SKELETON_SHOT_PERIOD_TICKS;
        let due_tick = if phase == 0 {
            SKELETON_SHOT_PERIOD_TICKS
        } else {
            SKELETON_SHOT_PERIOD_TICKS - phase
        };

        assert_eq!(
            owner.tick_hostile_attacks(&registry, due_tick, BlockStateId(0)),
            1
        );

        let commands = std::iter::from_fn(|| outbound.try_recv().ok()).collect::<Vec<_>>();
        assert!(commands.iter().any(|command| matches!(
            command,
            OutboundCommand::SpawnEntity(entity)
                if entity.type_id == 77
                    && entity.type_name == "minecraft:arrow"
                    && entity.velocity.z < 0.0
                    && !entity.on_ground
        )));
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, OutboundCommand::DamagePlayer { .. }))
        );
    }

    #[test]
    fn food_use_transaction_rejects_stale_stack_and_survival_state() {
        for stale_survival in [false, true] {
            let registry = SessionRegistry::new();
            let session = register_test_session(&registry, "StaleFoodPlayer");
            let mut inventory = PlayerInventory::empty();
            inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
            let player_state = register_test_player_state(&registry, session, inventory);
            let expected_survival = SurvivalState {
                food: 16,
                saturation: 1.0,
                ..SurvivalState::FULL
            };
            {
                let mut state = player_state.lock().unwrap();
                state.survival = expected_survival;
                if stale_survival {
                    state.survival.food = 15;
                } else {
                    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
                }
            }
            let (handle, mut owner) = simulation_channel_with_capacity(1);
            let response = handle
                .for_session(session)
                .enqueue_player_command(SimulationCommand::CommitFoodUse(FoodUseCommand {
                    actor_session: session,
                    plan: FoodUsePlan {
                        held_slot: PlayerInventory::HOTBAR_BASE,
                        expected_held: ItemStack::new(42, 2),
                        expected_survival,
                        food: 4,
                        saturation: 2.4,
                    },
                }))
                .unwrap();

            assert_eq!(owner.process_tick(&registry, 1).processed, 1);
            assert!(matches!(
                response.blocking_recv().unwrap().unwrap(),
                SimulationResponse::FoodUse(Ok(None))
            ));
            let state = player_state.lock().unwrap();
            assert_ne!(state.survival.food, 20);
        }
    }

    #[test]
    fn food_use_owner_apply_survives_requester_loss() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "LostFoodRequester");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let expected_survival = SurvivalState {
            food: 16,
            saturation: 1.0,
            ..SurvivalState::FULL
        };
        player_state.lock().unwrap().survival = expected_survival;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitFoodUse(FoodUseCommand {
                actor_session: session,
                plan: FoodUsePlan {
                    held_slot: PlayerInventory::HOTBAR_BASE,
                    expected_held: ItemStack::new(42, 2),
                    expected_survival,
                    food: 4,
                    saturation: 2.4,
                },
            }))
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        drop(response);

        let state = player_state.lock().unwrap();
        assert_eq!(state.survival.food, 20);
        assert_eq!(
            state.inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
    }

    #[test]
    fn food_use_transaction_rejects_stale_session_before_mutation() {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleFoodSession");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let expected_survival = SurvivalState {
            food: 16,
            saturation: 1.0,
            ..SurvivalState::FULL
        };
        player_state.lock().unwrap().survival = expected_survival;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitFoodUse(FoodUseCommand {
                actor_session: session,
                plan: FoodUsePlan {
                    held_slot: PlayerInventory::HOTBAR_BASE,
                    expected_held: ItemStack::new(42, 2),
                    expected_survival,
                    food: 4,
                    saturation: 2.4,
                },
            }))
            .unwrap();
        registry.unregister(session);

        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            response.blocking_recv().unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
        let state = player_state.lock().unwrap();
        assert_eq!(state.survival, expected_survival);
        assert_eq!(
            state.inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }

    const BOW_TEST_ARROW_SLOT: usize = 10;

    fn test_bow_release_plan(expected_bow: ItemStack, expected_arrow: ItemStack) -> BowReleasePlan {
        BowReleasePlan {
            bow_slot: PlayerInventory::HOTBAR_BASE,
            expected_bow,
            arrow_slot: BOW_TEST_ARROW_SLOT,
            expected_arrow,
            bow_max_damage: 384,
            entity_type_id: 3,
            position: Vec3::new(0.5, 65.5, 0.5),
            velocity: Vec3::new(0.0, 0.1, 2.0),
            rotation: Rotation::ZERO,
        }
    }

    fn bow_release_inventory(bow: ItemStack, arrow_count: i32) -> PlayerInventory {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = bow;
        inventory.slots[BOW_TEST_ARROW_SLOT] = ItemStack::new(43, arrow_count);
        inventory
    }

    fn arrow_entity_count(registry: &SessionRegistry) -> usize {
        registry
            .persisted_entity_records()
            .into_iter()
            .filter(|record| record.snapshot.type_name == "minecraft:arrow")
            .count()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bow_release_transaction_commits_arrow_bow_and_projectile_together() {
        let registry = SessionRegistry::new();
        let (session, _outbound) = register_test_session_with_outbound(&registry, "AtomicBow");
        let bow = ItemStack::new(42, 1);
        let arrows = ItemStack::new(43, 3);
        let player_state = register_test_player_state(
            &registry,
            session,
            bow_release_inventory(bow.clone(), arrows.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request =
            Box::pin(session_handle.commit_bow_release(test_bow_release_plan(bow, arrows)));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let committed = request
            .await
            .expect("bow response")
            .expect("matching bow release commits");

        let state = player_state.lock().unwrap();
        assert_eq!(
            state.inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1).with_damage(1)
        );
        assert_eq!(
            state.inventory.slots[BOW_TEST_ARROW_SLOT],
            ItemStack::new(43, 2)
        );
        assert_eq!(committed.inventory.slots, state.inventory.slots);
        assert_eq!(arrow_entity_count(&registry), 1);
    }

    #[test]
    fn bow_release_transaction_rejects_stale_bow_or_arrow_without_projectile() {
        for stale_bow in [false, true] {
            let registry = SessionRegistry::new();
            let (session, _outbound) = register_test_session_with_outbound(&registry, "StaleBow");
            let bow = ItemStack::new(42, 1);
            let arrows = ItemStack::new(43, 3);
            let player_state = register_test_player_state(
                &registry,
                session,
                bow_release_inventory(bow.clone(), arrows.count),
            );
            {
                let mut state = player_state.lock().unwrap();
                let slot = if stale_bow {
                    PlayerInventory::HOTBAR_BASE
                } else {
                    BOW_TEST_ARROW_SLOT
                };
                state.inventory.slots[slot].count -= 1;
            }
            let (handle, mut owner) = simulation_channel_with_capacity(1);
            let response = handle
                .for_session(session)
                .enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
                    actor_session: session,
                    plan: test_bow_release_plan(bow, arrows),
                }))
                .unwrap();

            assert_eq!(owner.process_tick(&registry, 1).processed, 1);
            assert!(matches!(
                response.blocking_recv().unwrap().unwrap(),
                SimulationResponse::BowRelease(Ok(None))
            ));
            assert_eq!(arrow_entity_count(&registry), 0);
        }
    }

    #[test]
    fn duplicate_bow_release_commits_exactly_one_projectile() {
        let registry = SessionRegistry::new();
        let (session, _outbound) = register_test_session_with_outbound(&registry, "DoubleBow");
        let bow = ItemStack::new(42, 1);
        let arrows = ItemStack::new(43, 2);
        let player_state = register_test_player_state(
            &registry,
            session,
            bow_release_inventory(bow.clone(), arrows.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let mut responses = Vec::new();
        for _ in 0..2 {
            responses.push(
                handle
                    .for_session(session)
                    .enqueue_player_command(SimulationCommand::CommitBowRelease(
                        BowReleaseCommand {
                            actor_session: session,
                            plan: test_bow_release_plan(bow.clone(), arrows.clone()),
                        },
                    ))
                    .unwrap(),
            );
        }

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        let committed = responses
            .into_iter()
            .map(|response| {
                matches!(
                    response.blocking_recv().unwrap().unwrap(),
                    SimulationResponse::BowRelease(Ok(Some(_)))
                )
            })
            .filter(|committed| *committed)
            .count();

        assert_eq!(committed, 1);
        assert_eq!(arrow_entity_count(&registry), 1);
        let state = player_state.lock().unwrap();
        assert_eq!(
            state.inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1).with_damage(1)
        );
        assert_eq!(
            state.inventory.slots[BOW_TEST_ARROW_SLOT],
            ItemStack::new(43, 1)
        );
    }

    #[test]
    fn bow_release_owner_apply_survives_requester_loss_and_breaks_spent_bow() {
        let registry = SessionRegistry::new();
        let (session, _outbound) = register_test_session_with_outbound(&registry, "LostBow");
        let bow = ItemStack::new(42, 1).with_damage(383);
        let arrows = ItemStack::new(43, 1);
        let player_state = register_test_player_state(
            &registry,
            session,
            bow_release_inventory(bow.clone(), arrows.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
                actor_session: session,
                plan: test_bow_release_plan(bow, arrows),
            }))
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        drop(response);

        let state = player_state.lock().unwrap();
        assert!(state.inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());
        assert!(state.inventory.slots[BOW_TEST_ARROW_SLOT].is_empty());
        assert_eq!(arrow_entity_count(&registry), 1);
    }

    #[test]
    fn bow_release_transaction_rejects_stale_session_before_mutation() {
        let registry = SessionRegistry::new();
        let (session, _outbound) = register_test_session_with_outbound(&registry, "GoneBow");
        let bow = ItemStack::new(42, 1);
        let arrows = ItemStack::new(43, 1);
        let player_state = register_test_player_state(
            &registry,
            session,
            bow_release_inventory(bow.clone(), arrows.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
                actor_session: session,
                plan: test_bow_release_plan(bow, arrows),
            }))
            .unwrap();
        registry.unregister(session);

        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            response.blocking_recv().unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
        assert_eq!(arrow_entity_count(&registry), 0);
        let state = player_state.lock().unwrap();
        assert_eq!(
            state.inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
        assert_eq!(
            state.inventory.slots[BOW_TEST_ARROW_SLOT],
            ItemStack::new(43, 1)
        );
    }

    fn test_selected_item_drop_plan(
        expected_held: ItemStack,
        drop_count: i32,
    ) -> SelectedItemDropPlan {
        SelectedItemDropPlan {
            held_hotbar_slot: 0,
            expected_held,
            drop_count,
            entity_type_id: 2,
            position: Vec3::new(0.5, 65.0, 2.6),
        }
    }

    fn selected_item_drop_inventory(count: i32) -> PlayerInventory {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, count);
        inventory
    }

    fn persisted_item_drop_stacks(registry: &SessionRegistry) -> Vec<EntityItemStack> {
        registry
            .persisted_entity_records()
            .into_iter()
            .filter_map(|record| record.snapshot.item_stack)
            .collect()
    }

    #[test]
    fn lethal_player_survival_transition_commits_state_and_drops_once() {
        let registry = SessionRegistry::new();
        let (session, mut outbound) =
            register_test_session_with_outbound(&registry, "AtomicPlayerDeath");
        registry.mark_loaded(session, (0, 0));
        let mut inventory = PlayerInventory::empty();
        inventory.slots[5] = ItemStack::new(42, 1).with_damage(3);
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 4);
        let player_state = register_test_player_state(&registry, session, inventory.clone());
        {
            let mut state = player_state.lock().unwrap();
            state.carried_item = ItemStack::new(44, 2);
            state.xp = XpState {
                level: 12,
                progress: 0.5,
                total: 87,
                seed: 7,
            };
        }

        let expected_survival = SurvivalState::FULL;
        let mut updated_survival = expected_survival;
        updated_survival.apply_damage(SurvivalState::MAX_HEALTH);
        let plan = PlayerSurvivalPlan {
            expected_survival,
            updated_survival,
            expected_inventory: inventory.clone(),
            updated_inventory: inventory,
            expected_carried_item: ItemStack::new(44, 2),
            expected_xp: XpState {
                level: 12,
                progress: 0.5,
                total: 87,
                seed: 7,
            },
            updated_xp: XpState {
                level: 12,
                progress: 0.5,
                total: 87,
                seed: 7,
            },
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: Some(1),
            xp_orb_entity_type_id: Some(2),
            position: Vec3::new(0.5, 64.0, 0.5),
        };
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = (0..2)
            .map(|_| {
                handle
                    .for_session(session)
                    .enqueue_player_command(SimulationCommand::CommitPlayerSurvival(Box::new(
                        PlayerSurvivalCommand {
                            actor_session: session,
                            plan: plan.clone(),
                        },
                    )))
                    .unwrap()
            })
            .collect::<Vec<_>>();

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        let committed = responses
            .into_iter()
            .filter_map(
                |response| match response.blocking_recv().unwrap().unwrap() {
                    SimulationResponse::PlayerSurvival(Ok(Some(outcome))) => match *outcome {
                        PlayerSurvivalCommitOutcome::Committed(committed) => Some(committed),
                        PlayerSurvivalCommitOutcome::Rejected(_) => None,
                    },
                    SimulationResponse::PlayerSurvival(Ok(None)) => None,
                    other => panic!("expected player survival response, got {other:?}"),
                },
            )
            .collect::<Vec<_>>();
        assert_eq!(
            committed.len(),
            1,
            "duplicate lethal transition must not duplicate drops"
        );
        assert!(
            std::iter::from_fn(|| outbound.try_recv().ok()).any(|command| {
                matches!(
                    command,
                    OutboundCommand::PickupCandidates(candidates)
                        if candidates
                            .iter()
                            .any(|entity| entity.experience_value == Some(84))
                )
            })
        );

        let state = player_state.lock().unwrap();
        assert!(state.survival.is_dead());
        assert!(state.inventory.slots[1..].iter().all(ItemStack::is_empty));
        assert!(state.carried_item.is_empty());
        assert_eq!(state.xp.level, 0);
        assert_eq!(state.xp.total, 0);
        drop(state);

        let mut drops = persisted_item_drop_stacks(&registry);
        drops.sort_by_key(|stack| stack.item_id);
        assert_eq!(
            drops,
            vec![
                EntityItemStack {
                    item_id: 42,
                    count: 1,
                    damage: Some(3),
                    enchantments: Vec::new(),
                },
                EntityItemStack::new(43, 4),
                EntityItemStack::new(44, 2),
            ]
        );
        let experience = registry.nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
        assert_eq!(experience.len(), 1);
        assert_eq!(experience[0].experience_value, Some(84));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn selected_item_drop_transaction_commits_debit_and_entity_together() {
        let registry = SessionRegistry::new();
        let (session, _outbound) =
            register_test_session_with_outbound(&registry, "AtomicSelectedDrop");
        let expected = ItemStack::new(42, 3);
        let player_state = register_test_player_state(
            &registry,
            session,
            selected_item_drop_inventory(expected.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(
            session_handle.commit_selected_item_drop(test_selected_item_drop_plan(expected, 1)),
        );
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        let committed = request
            .await
            .expect("selected item drop response")
            .expect("matching selected item drop commits");

        let state = player_state.lock().unwrap();
        assert_eq!(
            state.inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
        assert_eq!(committed.inventory.slots, state.inventory.slots);
        assert_eq!(
            persisted_item_drop_stacks(&registry),
            vec![EntityItemStack::new(42, 1)]
        );
        drop(state);

        let item_entity_id = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.item_stack.is_some())
            .expect("selected item drop entity")
            .snapshot
            .id;
        let collector = register_test_session(&registry, "SelectedDropCollector");
        let pickup_pose = PlayerPose::new(0.5, 65.0, 2.6);
        let _ = registry.update_pose(session, pickup_pose);
        let _ = registry.update_pose(collector, pickup_pose);
        registry.advance_world_time(super::super::ITEM_PICKUP_DELAY_TICKS);
        assert!(
            registry
                .claim_item_pickup_for_test(item_entity_id, session, 1)
                .is_none(),
            "drop owner must remain blocked after the generic pickup delay"
        );
        assert!(
            registry
                .claim_item_pickup_for_test(item_entity_id, collector, 1)
                .is_some(),
            "another session may collect the dropped item"
        );
    }

    #[test]
    fn selected_item_drop_all_commits_empty_slot_and_complete_stack() {
        let registry = SessionRegistry::new();
        let (session, _outbound) =
            register_test_session_with_outbound(&registry, "AtomicSelectedDropAll");
        let expected = ItemStack::new(42, 3).with_damage(7);
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = expected.clone();
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
                SelectedItemDropCommand {
                    actor_session: session,
                    plan: test_selected_item_drop_plan(expected, 3),
                },
            ))
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SelectedItemDrop(Ok(Some(_)))
        ));
        assert!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty()
        );
        assert_eq!(
            persisted_item_drop_stacks(&registry),
            vec![EntityItemStack {
                item_id: 42,
                count: 3,
                damage: Some(7),
                enchantments: Vec::new(),
            }]
        );
    }

    #[test]
    fn selected_item_drop_rejects_stale_slot_or_stack_without_entity() {
        for stale_selection in [false, true] {
            let registry = SessionRegistry::new();
            let (session, _outbound) =
                register_test_session_with_outbound(&registry, "StaleSelectedDrop");
            let expected = ItemStack::new(42, 2);
            let player_state = register_test_player_state(
                &registry,
                session,
                selected_item_drop_inventory(expected.count),
            );
            if stale_selection {
                player_state.lock().unwrap().selected_hotbar_slot = 1;
            } else {
                player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].count =
                    1;
            }
            let (handle, mut owner) = simulation_channel_with_capacity(1);
            let response = handle
                .for_session(session)
                .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
                    SelectedItemDropCommand {
                        actor_session: session,
                        plan: test_selected_item_drop_plan(expected, 1),
                    },
                ))
                .unwrap();

            assert_eq!(owner.process_tick(&registry, 1).processed, 1);
            assert!(matches!(
                response.blocking_recv().unwrap().unwrap(),
                SimulationResponse::SelectedItemDrop(Ok(None))
            ));
            assert!(persisted_item_drop_stacks(&registry).is_empty());
        }
    }

    #[test]
    fn duplicate_selected_item_drop_commits_exactly_one_entity() {
        let registry = SessionRegistry::new();
        let (session, _outbound) =
            register_test_session_with_outbound(&registry, "DuplicateSelectedDrop");
        let expected = ItemStack::new(42, 2);
        let player_state = register_test_player_state(
            &registry,
            session,
            selected_item_drop_inventory(expected.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = (0..2)
            .map(|_| {
                handle
                    .for_session(session)
                    .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
                        SelectedItemDropCommand {
                            actor_session: session,
                            plan: test_selected_item_drop_plan(expected.clone(), 1),
                        },
                    ))
                    .unwrap()
            })
            .collect::<Vec<_>>();

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        let committed = responses
            .into_iter()
            .map(|response| {
                matches!(
                    response.blocking_recv().unwrap().unwrap(),
                    SimulationResponse::SelectedItemDrop(Ok(Some(_)))
                )
            })
            .filter(|committed| *committed)
            .count();

        assert_eq!(committed, 1);
        assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
    }

    #[test]
    fn selected_item_drop_owner_apply_survives_requester_loss() {
        let registry = SessionRegistry::new();
        let (session, _outbound) =
            register_test_session_with_outbound(&registry, "LostSelectedDropRequester");
        let expected = ItemStack::new(42, 2);
        let player_state = register_test_player_state(
            &registry,
            session,
            selected_item_drop_inventory(expected.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
                SelectedItemDropCommand {
                    actor_session: session,
                    plan: test_selected_item_drop_plan(expected, 1),
                },
            ))
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        drop(response);

        assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
    }

    #[test]
    fn selected_item_drop_rejects_stale_session_before_mutation() {
        let registry = SessionRegistry::new();
        let (session, _outbound) =
            register_test_session_with_outbound(&registry, "StaleSelectedDropSession");
        let expected = ItemStack::new(42, 2);
        let player_state = register_test_player_state(
            &registry,
            session,
            selected_item_drop_inventory(expected.count),
        );
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
                SelectedItemDropCommand {
                    actor_session: session,
                    plan: test_selected_item_drop_plan(expected, 1),
                },
            ))
            .unwrap();
        registry.unregister(session);

        assert_eq!(owner.process_tick(&registry, 1).processed, 0);
        assert!(matches!(
            response.blocking_recv().unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
        assert!(persisted_item_drop_stacks(&registry).is_empty());
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }

    #[test]
    fn survival_placement_transaction_debits_the_offhand_slot() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "OffhandPlacement");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let mut plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        plan.held.inventory_slot = PlayerInventory::OFFHAND_SLOT;
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan,
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(Some(_)))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::OFFHAND_SLOT],
            ItemStack::new(42, 1)
        );
    }

    #[test]
    fn survival_placement_transaction_rejects_stale_support_without_inventory_debit() {
        let (mut storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        storage
            .set_block_at(support, BlockStateId(0))
            .expect("replace support before stale placement");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StalePlacementSupport");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan: test_survival_placement_plan(
                        target,
                        target_token,
                        support,
                        support_token,
                        42,
                        2,
                    ),
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(None))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }

    #[test]
    fn creative_placement_transaction_rejects_stale_support_without_mutation() {
        let (mut storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        storage
            .set_block_at(support, BlockStateId(0))
            .expect("replace support before stale creative placement");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleCreativePlacementSupport");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        player_state.lock().unwrap().game_mode = GameMode::Creative;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let mut plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        plan.expected_game_mode = GameMode::Creative;
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan,
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(None))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }

    #[test]
    fn placement_transaction_rejects_game_mode_change_before_commit() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "PlacementModeChanged");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        player_state.lock().unwrap().game_mode = GameMode::Creative;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let mut plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        plan.expected_game_mode = GameMode::Creative;
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan,
                },
            )))
            .unwrap();
        player_state.lock().unwrap().game_mode = GameMode::Survival;

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(None))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }

    #[test]
    fn placement_transaction_rejects_non_building_game_modes() {
        for game_mode in [GameMode::Adventure, GameMode::Spectator] {
            let (storage, support, support_token) = test_block_storage();
            let target = BlockPos {
                x: support.x + 1,
                ..support
            };
            let target_token = storage.block_mutation_token(target).unwrap();
            let world = Arc::new(tokio::sync::Mutex::new(storage));
            let registry = SessionRegistry::new();
            let session = register_test_session(&registry, "PlacementNonBuildingMode");
            let mut inventory = PlayerInventory::empty();
            inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
            let player_state = register_test_player_state(&registry, session, inventory);
            player_state.lock().unwrap().game_mode = game_mode;
            let (handle, mut owner) = simulation_channel_with_capacity(1);
            let mut plan =
                test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
            plan.expected_game_mode = game_mode;
            let response = handle
                .for_session(session)
                .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                    SurvivalPlacementCommand {
                        actor_session: session,
                        plan,
                    },
                )))
                .unwrap();

            owner.process_tick_with_world(&registry, Some(&world), None, 1);
            assert!(matches!(
                response.blocking_recv().unwrap().unwrap(),
                SimulationResponse::SurvivalPlacement(Ok(None))
            ));
            assert_eq!(
                world.blocking_lock().get_cached_block(target),
                Some(BlockStateId(0))
            );
            assert_eq!(
                player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
                ItemStack::new(42, 2)
            );
        }
    }

    #[test]
    fn survival_placement_transaction_rejects_held_stack_mismatch_without_mutation() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "MismatchedPlacementStack");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan: test_survival_placement_plan(
                        target,
                        target_token,
                        support,
                        support_token,
                        42,
                        2,
                    ),
                },
            )))
            .unwrap();

        owner.process_tick_with_world(&registry, Some(&world), None, 1);
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(None))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(43, 2)
        );
    }

    #[test]
    fn survival_placement_transaction_survives_requester_loss_after_apply() {
        let (mut storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let water = BlockPos {
            x: target.x + 1,
            ..target
        };
        storage
            .set_block_at(water, BlockStateId(2))
            .expect("place adjacent water");
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "LostPlacementRequester");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let block_light = BlockLightTable::from_arrays(
            "test",
            vec![0; 5],
            vec![0, 15, 1, 15, 15],
            vec![true, false, false, false, false],
        );
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan: test_survival_placement_plan(
                        target,
                        target_token,
                        support,
                        support_token,
                        42,
                        2,
                    ),
                },
            )))
            .unwrap();

        owner.process_tick_with_world(&registry, Some(&world), Some(&block_light), 1);
        drop(response);
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
        let mut storage = world.blocking_lock();
        let chunk = storage
            .cached_chunk_snapshot(ChunkPos { x: 0, z: 0 })
            .unwrap();
        assert!(mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some());
        let ticks = storage
            .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .unwrap();
        assert!(ticks.iter().any(|tick| tick.pos == water));
    }

    #[test]
    fn survival_placement_transaction_rejects_stale_session_without_mutation() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "DisconnectedPlacementBuilder");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan: test_survival_placement_plan(
                        target,
                        target_token,
                        support,
                        support_token,
                        42,
                        2,
                    ),
                },
            )))
            .unwrap();
        registry.unregister(session);

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            0
        );
        assert!(matches!(
            response.blocking_recv().unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }

    #[test]
    fn concurrent_survival_placement_transactions_have_one_exact_winner() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let first_session = register_test_session(&registry, "FirstPlacementBuilder");
        let second_session = register_test_session(&registry, "SecondPlacementBuilder");
        let mut first_inventory = PlayerInventory::empty();
        first_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let first_state = register_test_player_state(&registry, first_session, first_inventory);
        let mut second_inventory = PlayerInventory::empty();
        second_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let second_state = register_test_player_state(&registry, second_session, second_inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let command = |actor_session| {
            SimulationCommand::CommitSurvivalPlacement(Box::new(SurvivalPlacementCommand {
                actor_session,
                plan: test_survival_placement_plan(
                    target,
                    target_token,
                    support,
                    support_token,
                    42,
                    2,
                ),
            }))
        };
        let first = handle
            .for_session(first_session)
            .enqueue_player_command(command(first_session))
            .unwrap();
        let second = handle
            .for_session(second_session)
            .enqueue_player_command(command(second_session))
            .unwrap();

        owner.process_tick_with_world(&registry, Some(&world), None, 2);
        assert!(matches!(
            first.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(Some(_)))
        ));
        assert!(matches!(
            second.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(None))
        ));
        assert_eq!(
            first_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
        assert_eq!(
            second_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bucket_use_transaction_survives_requester_loss_after_owner_apply() {
        let (storage, support, _) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = Arc::new(SessionRegistry::new());
        let (session, _actor_rx) =
            register_test_session_with_outbound(&registry, "LostBucketRequester");
        let (observer, mut observer_rx) =
            register_test_session_with_outbound(&registry, "BucketObserver");
        registry.replace_view(session, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.mark_loaded(session, (0, 0));
        registry.mark_loaded(observer, (0, 0));
        while observer_rx.try_recv().is_ok() {}
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(61, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitBucketUse(Box::new(
                BucketUseCommand {
                    actor_session: session,
                    plan: test_bucket_use_plan(target, target_token),
                },
            )))
            .unwrap();

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_registry = Arc::clone(&registry);
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_registry,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });
        let report = tokio::time::timeout(std::time::Duration::from_secs(1), owner_task)
            .await
            .expect("regional bucket owner completion event")
            .unwrap();
        assert_eq!(report.processed, 1);
        drop(response);
        drop(writer);

        assert_eq!(
            world.lock().await.get_cached_block(target),
            Some(BlockStateId(2))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(60, 1)
        );
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::BlockDeltas(_))
        ));
        let mut storage = world.lock().await;
        let ticks = storage
            .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .unwrap();
        assert!(ticks.iter().any(|tick| tick.pos == target));
    }

    #[test]
    fn bucket_uses_in_distinct_regions_overlap() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(blocks);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
        for chunk in chunks {
            storage
                .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
                .unwrap();
        }
        let targets = [
            BlockPos { x: 2, y: 64, z: 2 },
            BlockPos {
                x: 8 * 16 + 2,
                y: 64,
                z: 2,
            },
        ];
        let tokens = targets.map(|target| storage.block_mutation_token(target).unwrap());
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let light = Arc::new(BlockLightTable::from_arrays(
            "regional bucket use",
            vec![0, 0, 1, 0, 0],
            vec![0, 15, 1, 0, 0],
            vec![true, false, false, true, true],
        ));
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actors = [
            register_test_session(&sessions, "RegionalBucketA"),
            register_test_session(&sessions, "RegionalBucketB"),
        ];
        let player_states = actors.map(|actor| {
            let mut inventory = PlayerInventory::empty();
            inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(61, 1);
            register_test_player_state(&sessions, actor, inventory)
        });
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = (0..2)
            .map(|index| {
                handle
                    .for_session(actors[index])
                    .enqueue_player_command(SimulationCommand::CommitBucketUse(Box::new(
                        BucketUseCommand {
                            actor_session: actors[index],
                            plan: test_bucket_use_plan(targets[index], tokens[index]),
                        },
                    )))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        owner.install_regional_block_edit_probe(entered_tx, release_rx);

        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let worker = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(owner.process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: Some(&light),
                    },
                    Some(light.as_ref()),
                    2,
                ))
        });

        let first = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first regional bucket worker entry");
        let second = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second regional bucket worker enters before release");
        assert_ne!(first, second);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();

        assert_eq!(worker.join().unwrap().processed, 2);
        for response in responses {
            let SimulationResponse::BucketUse(Ok(Some(committed))) =
                response.blocking_recv().unwrap().unwrap()
            else {
                panic!("regional bucket response mismatch");
            };
            assert!(committed.block.precomputed_light_updates.is_some());
        }
        let mut storage = world.blocking_lock();
        for (target, player_state) in targets.into_iter().zip(player_states) {
            assert_eq!(storage.get_cached_block(target), Some(BlockStateId(2)));
            assert_eq!(
                player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
                ItemStack::new(60, 1)
            );
            let ticks = storage
                .scheduled_fluid_ticks(ChunkPos {
                    x: target.x.div_euclid(16),
                    z: target.z.div_euclid(16),
                })
                .unwrap()
                .unwrap();
            assert!(ticks.iter().any(|tick| tick.pos == target));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bucket_use_transaction_rejects_stale_block_without_inventory_change() {
        let (mut storage, support, _) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        storage
            .set_block_at(target, BlockStateId(1))
            .expect("replace bucket target before stale commit");
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleBucketRequester");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(61, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitBucketUse(Box::new(
                BucketUseCommand {
                    actor_session: session,
                    plan: test_bucket_use_plan(target, target_token),
                },
            )))
            .unwrap();

        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await;

        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::BucketUse(Ok(None))
        ));
        assert_eq!(
            world.lock().await.get_cached_block(target),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(61, 1)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_placement_world_busy_returns_without_retry_or_mutation() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "RetryingPlacementBuilder");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        let mut request = Box::pin(session_handle.commit_survival_placement(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;
        let guard = world.try_lock().unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            request.await,
            Err(SimulationRequestError::WorldBusy)
        ));
        assert_eq!(guard.get_cached_block(target), Some(BlockStateId(0)));
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.enqueued, 1);
        assert_eq!(snapshot.processed, 1);
        assert_eq!(snapshot.rejected_world_busy, 1);
        assert_eq!(snapshot.depth, 0);
    }

    #[test]
    fn survival_placement_owner_dispatches_peer_block_after_requester_loss() {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let (actor, _actor_rx) =
            register_test_session_with_outbound(&registry, "PlacementEventActor");
        let (observer, mut observer_rx) =
            register_test_session_with_outbound(&registry, "PlacementEventObserver");
        registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.mark_loaded(actor, (0, 0));
        registry.mark_loaded(observer, (0, 0));
        while observer_rx.try_recv().is_ok() {}
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        register_test_player_state(&registry, actor, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let block_light = BlockLightTable::from_arrays(
            "test",
            vec![0; 5],
            vec![0, 15, 1, 15, 15],
            vec![true, false, false, false, false],
        );
        let response = handle
            .for_session(actor)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: actor,
                    plan: test_survival_placement_plan(
                        target,
                        target_token,
                        support,
                        support_token,
                        42,
                        2,
                    ),
                },
            )))
            .unwrap();

        owner.process_tick_with_world(&registry, Some(&world), Some(&block_light), 1);
        drop(response);
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::BlockDeltas(_))
        ));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::LightUpdates(_))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(target),
            Some(BlockStateId(1))
        );
    }

    #[test]
    fn survival_break_transaction_rejects_stale_block_without_tool_or_drop_mutation() {
        let (mut storage, pos, token) = test_block_storage();
        storage
            .set_block_at(pos, BlockStateId(0))
            .expect("replace target before stale request");
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleBreakMiner");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: session,
                    request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                        pos, token, 42, 7,
                    )),
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalBreak(Ok(None))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(pos),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            None
        );
        assert_eq!(persisted_item_drop_count(&registry), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_break_transaction_survives_requester_loss_after_apply() {
        let (mut storage, pos, token) = test_block_storage();
        let water = BlockPos {
            x: pos.x + 1,
            ..pos
        };
        let sand = BlockPos {
            y: pos.y + 1,
            ..pos
        };
        storage
            .set_block_at(water, BlockStateId(2))
            .expect("place adjacent water");
        storage
            .set_block_at(sand, BlockStateId(3))
            .expect("place unsupported sand");
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "LostBreakRequester");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let block_light = Arc::new(BlockLightTable::from_arrays(
            "test",
            vec![0; 5],
            vec![0, 15, 1, 15, 15],
            vec![true, false, false, false, false],
        ));
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: session,
                    request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                        pos, token, 42, 7,
                    )),
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &registry,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: Some(&block_light),
                    },
                    Some(block_light.as_ref()),
                    1,
                )
                .await
                .processed,
            1
        );
        drop(response);

        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            Some(1)
        );
        assert_eq!(persisted_item_drop_count(&registry), 1);
        let mut storage = world.lock().await;
        assert_eq!(storage.get_cached_block(sand), Some(BlockStateId(0)));
        let chunk = storage
            .cached_chunk_snapshot(ChunkPos { x: 0, z: 0 })
            .unwrap();
        assert!(mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some());
        let ticks = storage
            .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .unwrap();
        assert!(ticks.iter().any(|tick| tick.pos == water));
        drop(storage);
        assert!(registry.persisted_entity_records().iter().any(|record| {
            record.snapshot.type_name == "minecraft:falling_block"
                && record.snapshot.block_state == Some(3)
        }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_break_clears_campfire_state_before_requester_response() {
        let (mut storage, pos, _) = test_block_storage();
        storage
            .set_block_at(pos, BlockStateId(4))
            .expect("replace target with campfire");
        let token = storage.block_mutation_token(pos).unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        assert!(
            registry
                .insert_campfire_cooking(pos, ItemStack::new(10, 1), ItemStack::new(11, 1), 20,)
                .is_some()
        );
        let session = register_test_session(&registry, "LostCampfireBreakRequester");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        register_test_player_state(&registry, session, inventory);
        let mut plan = test_survival_break_plan(pos, token, 42, 7);
        plan.preconditions[0].expected_state = BlockStateId(4);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: session,
                    request: SurvivalBreakRequest::Prepared(plan),
                },
            )))
            .unwrap();

        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await;

        assert!(registry.campfire_cooking_state(pos).is_empty());
        drop(response);
    }

    #[test]
    fn survival_break_transaction_rejects_stale_session_without_mutation() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "DisconnectedBreakMiner");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: session,
                    request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                        pos, token, 42, 7,
                    )),
                },
            )))
            .unwrap();
        registry.unregister(session);

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            0
        );
        assert!(matches!(
            response.blocking_recv().unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(pos),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            None
        );
        assert_eq!(persisted_item_drop_count(&registry), 0);
    }

    #[test]
    fn concurrent_survival_break_transactions_have_one_exact_winner() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let first_session = register_test_session(&registry, "FirstBreakMiner");
        let second_session = register_test_session(&registry, "SecondBreakMiner");
        let mut first_inventory = PlayerInventory::empty();
        first_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let first_state = register_test_player_state(&registry, first_session, first_inventory);
        let mut second_inventory = PlayerInventory::empty();
        second_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 1);
        let second_state = register_test_player_state(&registry, second_session, second_inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let first = handle
            .for_session(first_session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: first_session,
                    request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                        pos, token, 42, 7,
                    )),
                },
            )))
            .unwrap();
        let second = handle
            .for_session(second_session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: second_session,
                    request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                        pos, token, 43, 7,
                    )),
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 2)
                .processed,
            2
        );
        assert!(matches!(
            first.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalBreak(Ok(Some(_)))
        ));
        assert!(matches!(
            second.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalBreak(Ok(None))
        ));
        assert_eq!(
            first_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            Some(1)
        );
        assert_eq!(
            second_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            None
        );
        assert_eq!(persisted_item_drop_count(&registry), 1);
    }

    #[test]
    fn survival_break_transaction_rejects_held_stack_mismatch_without_mutation() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "MismatchedBreakTool");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: session,
                    request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                        pos, token, 42, 7,
                    )),
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalBreak(Ok(None))
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(pos),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(43, 1)
        );
        assert_eq!(persisted_item_drop_count(&registry), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_break_world_busy_returns_without_retry_or_mutation() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "RetryingBreakMiner");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&registry, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let plan = test_survival_break_plan(pos, token, 42, 7);
        let mut request = Box::pin(session_handle.commit_survival_break(plan));
        assert_request_enqueued(request.as_mut(), &handle).await;
        let guard = world.try_lock().expect("test owns world lock");

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            request.await,
            Err(SimulationRequestError::WorldBusy)
        ));
        assert_eq!(guard.get_cached_block(pos), Some(BlockStateId(1)));
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            None
        );
        assert_eq!(persisted_item_drop_count(&registry), 0);
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.enqueued, 1);
        assert_eq!(snapshot.processed, 1);
        assert_eq!(snapshot.rejected_world_busy, 1);
        assert_eq!(snapshot.depth, 0);
    }

    #[test]
    fn survival_break_owner_dispatches_peer_block_before_drop_spawn() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let (actor, mut actor_rx) =
            register_test_session_with_outbound(&registry, "BreakEventActor");
        let (observer, mut observer_rx) =
            register_test_session_with_outbound(&registry, "BreakEventObserver");
        registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
        dispatch_visibility_commands(registry.mark_loaded(actor, (0, 0)));
        assert!(matches!(
            actor_rx.try_recv(),
            Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == observer
        ));
        dispatch_visibility_commands(registry.mark_loaded(observer, (0, 0)));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == actor
        ));
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        register_test_player_state(&registry, actor, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(actor)
            .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                SurvivalBreakCommand {
                    actor_session: actor,
                    request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                        pos, token, 42, 7,
                    )),
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::SurvivalBreak(Ok(Some(_)))
        ));
        let first = observer_rx.try_recv();
        assert!(
            matches!(first, Ok(OutboundCommand::BlockDeltas(_))),
            "first break event was {first:?}"
        );
        let second = observer_rx.try_recv();
        assert!(
            matches!(second, Ok(OutboundCommand::SpawnEntity(_))),
            "second break event was {second:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_conditional_block_edit_matches_direct_storage_commit() {
        let (mut direct_storage, pos, direct_token) = test_block_storage();
        let direct = apply_block_edit_batch_to_storage_conditionally(
            &mut direct_storage,
            None,
            &[BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            &[BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: direct_token,
            }],
        )
        .expect("direct conditional edit");

        let (queued_storage, queued_pos, queued_token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "QueuedBlockEditor");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos: queued_pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos: queued_pos,
                expected_state: BlockStateId(1),
                expected_token: queued_token,
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        let queued = request
            .await
            .expect("block edit response")
            .expect("queued conditional edit");

        assert_eq!(queued.applied, direct.applied);
        assert_eq!(direct_storage.get_cached_block(pos), Some(BlockStateId(0)));
        assert_eq!(
            world.lock().await.get_cached_block(queued_pos),
            Some(BlockStateId(0))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn block_drop_transaction_commits_edit_and_drop_in_owner_order() {
        let (storage, pos, token) = test_block_storage();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let (actor, mut actor_rx) =
            register_test_session_with_outbound(&registry, "BlockDropActor");
        let (observer, mut observer_rx) =
            register_test_session_with_outbound(&registry, "BlockDropObserver");
        registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
        dispatch_visibility_commands(registry.mark_loaded(actor, (0, 0)));
        assert!(matches!(
            actor_rx.try_recv(),
            Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == observer
        ));
        dispatch_visibility_commands(registry.mark_loaded(observer, (0, 0)));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == actor
        ));

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(actor);
        let mut request = Box::pin(session_handle.commit_block_drops(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            vec![SurvivalBreakDrop {
                entity_type_id: 7,
                position: Vec3::new(0.5, 64.5, 0.5),
                stack: EntityItemStack::new(42, 2),
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &registry,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        ..SimulationWorldAccess::default()
                    },
                    None,
                    1,
                )
                .await
                .processed,
            1
        );
        let outcome = request.await.unwrap().expect("matching block transaction");
        assert_eq!(outcome.applied.len(), 1);
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(0))
        );
        let drops = registry.persisted_entity_records();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item_stack, Some(EntityItemStack::new(42, 2)));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::BlockDeltas(_))
        ));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::SpawnEntity(_))
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn block_drop_transaction_rejects_stale_token_without_drop() {
        let (mut storage, pos, stale_token) = test_block_storage();
        storage.set_block_at(pos, BlockStateId(0)).unwrap();
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let actor = register_test_session(&registry, "StaleBlockDropActor");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(actor);
        let mut request = Box::pin(session_handle.commit_block_drops(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: stale_token,
            }],
            vec![SurvivalBreakDrop {
                entity_type_id: 7,
                position: Vec3::new(0.5, 64.5, 0.5),
                stack: EntityItemStack::new(42, 2),
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await;
        assert!(request.await.unwrap().is_none());
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(1))
        );
        assert_eq!(persisted_item_drop_count(&registry), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_block_edit_schedules_tick_only_after_matching_commit() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "ScheduledBlockEditor");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let scheduled_tick = mc_world::ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:air").unwrap(),
            20,
            0,
        );

        let mut committed = Box::pin(session_handle.apply_block_edits_with_scheduled_ticks(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            vec![scheduled_tick.clone()],
        ));
        assert_request_enqueued(committed.as_mut(), &handle).await;
        owner.process_tick_with_world(&registry, Some(&world), None, 1);
        assert!(committed.await.unwrap().is_some());

        let mut stale = Box::pin(session_handle.apply_block_edits_with_scheduled_ticks(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(1),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            vec![mc_world::ScheduledBlockTick::new(
                pos,
                Identifier::parse("minecraft:stone").unwrap(),
                21,
                0,
            )],
        ));
        assert_request_enqueued(stale.as_mut(), &handle).await;
        owner.process_tick_with_world(&registry, Some(&world), None, 1);
        assert!(stale.await.unwrap().is_none());

        let mut storage = world.lock().await;
        let ticks = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .unwrap();
        assert_eq!(ticks, &[scheduled_tick]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generic_block_edit_owner_dispatches_peer_after_requester_loss() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let (actor, _actor_rx) =
            register_test_session_with_outbound(&registry, "BlockEditEventActor");
        let (observer, mut observer_rx) =
            register_test_session_with_outbound(&registry, "BlockEditEventObserver");
        registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.mark_loaded(actor, (0, 0));
        registry.mark_loaded(observer, (0, 0));
        while observer_rx.try_recv().is_ok() {}
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let block_light = BlockLightTable::from_arrays(
            "test",
            vec![0; 5],
            vec![0, 15, 1, 15, 15],
            vec![true, false, false, false, false],
        );
        let session_handle = handle.for_session(actor);
        let mut request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        owner
            .process_commands_with_world(&registry, Some(&world), Some(&block_light), 1)
            .await;
        drop(request);

        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::BlockDeltas(_))
        ));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::LightUpdates(_))
        ));
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(0))
        );
    }

    #[test]
    fn queued_conditional_block_edits_commit_only_first_matching_token() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let command = || SimulationCommand::ApplyBlockEdits {
            actor_session: 0,
            edits: vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            scheduled_block_ticks: Vec::new(),
        };
        let first = handle.enqueue(command()).unwrap();
        let stale = handle.enqueue(command()).unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 2)
                .processed,
            2
        );
        assert!(matches!(
            first.blocking_recv().unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert!(matches!(
            stale.blocking_recv().unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_none()
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(pos),
            Some(BlockStateId(0))
        );
        assert_ne!(world.blocking_lock().block_mutation_token(pos), Some(token));
    }

    #[test]
    fn queued_conditional_block_edit_rejects_busy_world_without_mutation() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .enqueue(SimulationCommand::ApplyBlockEdits {
                actor_session: 0,
                edits: vec![BlockEdit {
                    pos,
                    new_state: BlockStateId(0),
                }],
                preconditions: vec![BlockEditPrecondition {
                    pos,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                }],
                scheduled_block_ticks: Vec::new(),
            })
            .unwrap();
        let guard = world.try_lock().expect("test owns world lock");

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::BlockEdits(Err(SimulationRequestError::WorldBusy))
        ));
        assert_eq!(guard.get_cached_block(pos), Some(BlockStateId(1)));
        drop(guard);
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.processed, 1);
        assert_eq!(snapshot.rejected_world_busy, 1);
        assert_eq!(snapshot.rejected_world_unavailable, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn world_busy_response_is_not_blindly_retried() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "SingleAttemptBlockEditor");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;
        let guard = world.try_lock().expect("test owns world lock");

        assert_eq!(
            owner
                .process_tick_with_world(&registry, Some(&world), None, 1)
                .processed,
            1
        );
        let outcome =
            std::future::poll_fn(|cx| match std::future::Future::poll(request.as_mut(), cx) {
                std::task::Poll::Ready(outcome) => std::task::Poll::Ready(outcome),
                std::task::Poll::Pending => {
                    panic!("WorldBusy must be returned instead of scheduling a blind retry")
                }
            })
            .await;

        assert!(matches!(outcome, Err(SimulationRequestError::WorldBusy)));
        assert_eq!(guard.get_cached_block(pos), Some(BlockStateId(1)));
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.enqueued, 1);
        assert_eq!(snapshot.processed, 1);
        assert_eq!(snapshot.depth, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_owner_wakes_on_world_unlock_and_commits_once() {
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "ReactiveBlockEditor");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.apply_block_edits(
            vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;
        let guard = world.lock().await;
        let mut processing =
            Box::pin(owner.process_commands_with_world(&registry, Some(&world), None, 1));
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(processing.as_mut(), cx).is_pending(),
                "owner must wait for the world mutex release event"
            );
            std::task::Poll::Ready(())
        })
        .await;

        drop(guard);
        assert_eq!(processing.as_mut().await.processed, 1);
        drop(processing);
        assert!(request.await.unwrap().is_some());
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(BlockStateId(0))
        );
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.enqueued, 1);
        assert_eq!(snapshot.processed, 1);
        assert_eq!(snapshot.rejected_world_busy, 0);
    }

    #[test]
    fn queued_unconditional_block_edits_apply_in_owner_sequence_order() {
        let (storage, pos, _) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let remove = handle
            .enqueue(SimulationCommand::ApplyBlockEdits {
                actor_session: 0,
                edits: vec![BlockEdit {
                    pos,
                    new_state: BlockStateId(0),
                }],
                preconditions: Vec::new(),
                scheduled_block_ticks: Vec::new(),
            })
            .unwrap();
        let restore = handle
            .enqueue(SimulationCommand::ApplyBlockEdits {
                actor_session: 0,
                edits: vec![BlockEdit {
                    pos,
                    new_state: BlockStateId(1),
                }],
                preconditions: Vec::new(),
                scheduled_block_ticks: Vec::new(),
            })
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 2)
                .processed,
            2
        );
        assert!(matches!(
            remove.blocking_recv().unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert!(matches!(
            restore.blocking_recv().unwrap().unwrap(),
            SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
        ));
        assert_eq!(
            world.blocking_lock().get_cached_block(pos),
            Some(BlockStateId(1))
        );
        assert_eq!(handle.snapshot().block_edits_processed, 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chest_commit_rejects_actor_without_open_view() {
        let mut initial = mc_world::ChestBlockEntity::default();
        initial.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut updated = initial.clone();
        updated.slots[0].count = 1;
        let (mut storage, pos) = test_container_storage();
        storage
            .set_chest_block_entity(pos, initial.clone())
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actor = register_test_session(&sessions, "RemoteChestActor");
        let player = empty_container_player_plan();
        let persisted =
            register_test_player_state(&sessions, actor, player.expected_inventory.clone());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(actor);
        let mut request = Box::pin(session_handle.commit_chest(
            pos,
            vec![pos],
            1,
            vec![initial.clone()],
            vec![updated],
            player,
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            request.await.unwrap(),
            SharedContainerCommit::Rejected { .. }
        ));
        assert_eq!(
            world.lock().await.chest_block_entity(pos).unwrap(),
            Some(initial)
        );
        let persisted = persisted.lock().unwrap();
        assert!(persisted.inventory.slots.iter().all(ItemStack::is_empty));
        assert!(persisted.carried_item.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_chest_commit_matches_direct_state_and_viewer_version() {
        let mut initial = mc_world::ChestBlockEntity::default();
        initial.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut updated = initial.clone();
        updated.slots[0].count = 1;

        let (mut direct_storage, pos) = test_container_storage();
        direct_storage
            .set_chest_block_entity(pos, initial.clone())
            .unwrap();
        let direct_sessions = SessionRegistry::new();
        let (direct_state_id, _) = direct_sessions
            .try_chest_slot_dispatches(
                pos,
                1,
                7,
                super::super::chest_slot_stacks(&super::super::ChestView {
                    chests: vec![updated.clone()],
                }),
            )
            .unwrap();
        direct_storage
            .set_chest_block_entity(pos, updated.clone())
            .unwrap();

        let (mut queued_storage, queued_pos) = test_container_storage();
        queued_storage
            .set_chest_block_entity(queued_pos, initial.clone())
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
        let queued_sessions = Arc::new(SessionRegistry::new());
        let session = register_test_session(&queued_sessions, "QueuedChestActor");
        assert_eq!(
            queued_sessions.register_chest_viewer(session, queued_pos),
            1
        );
        let player = empty_container_player_plan();
        register_test_player_state(&queued_sessions, session, player.expected_inventory.clone());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_chest(
            queued_pos,
            vec![queued_pos],
            1,
            vec![initial.clone()],
            vec![updated.clone()],
            player,
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&queued_sessions, Some(&world), None, 1)
                .processed,
            1
        );
        let outcome = request.await.unwrap();
        let queued_state_id = match outcome {
            SharedContainerCommit::Committed {
                state_id,
                dispatches,
                ..
            } => {
                assert!(dispatches.is_empty());
                state_id
            }
            other => panic!("expected committed chest, got {other:?}"),
        };

        assert_eq!(queued_state_id, direct_state_id);
        assert_eq!(queued_sessions.chest_state_id(queued_pos), direct_state_id);
        assert_eq!(
            world.lock().await.chest_block_entity(queued_pos).unwrap(),
            direct_storage.chest_block_entity(pos).unwrap()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resident_chest_commit_completes_while_global_world_writer_is_held() {
        let mut initial = mc_world::ChestBlockEntity::default();
        initial.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut updated = initial.clone();
        updated.slots[0].count = 1;
        let (mut storage, position) = test_container_storage();
        storage
            .set_chest_block_entity(position, initial.clone())
            .unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actor = register_test_session(&sessions, "RegionalChestActor");
        assert_eq!(sessions.register_chest_viewer(actor, position), 1);
        let player = empty_container_player_plan();
        register_test_player_state(&sessions, actor, player.expected_inventory.clone());
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(actor);
        let mut request = Box::pin(session_handle.commit_chest(
            position,
            vec![position],
            1,
            vec![initial],
            vec![updated.clone()],
            player,
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .expect("resident chest completion event")
            .expect("resident chest response");
        drop(writer);

        assert_eq!(owner_task.await.unwrap().processed, 1);
        assert!(matches!(
            outcome,
            SharedContainerCommit::Committed { state_id: 2, .. }
        ));
        assert_eq!(
            world.lock().await.chest_block_entity(position).unwrap(),
            Some(updated)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resident_stale_chest_commit_returns_current_authoritative_state() {
        let mut initial = mc_world::ChestBlockEntity::default();
        initial.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut first_update = initial.clone();
        first_update.slots[0].count = 1;
        let mut stale_update = initial.clone();
        stale_update.slots[0] = mc_world::FurnaceSlot::EMPTY;
        let (mut storage, position) = test_container_storage();
        storage
            .set_chest_block_entity(position, initial.clone())
            .unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let actor = register_test_session(&sessions, "RegionalStaleChestActor");
        assert_eq!(sessions.register_chest_viewer(actor, position), 1);
        let player = empty_container_player_plan();
        register_test_player_state(&sessions, actor, player.expected_inventory.clone());
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let first = handle
            .enqueue(SimulationCommand::CommitChest {
                primary_position: position,
                positions: vec![position],
                expected_state_id: 1,
                actor_session: actor,
                expected: vec![initial.clone()],
                updated: vec![first_update.clone()],
                player: Box::new(player.clone()),
            })
            .unwrap();
        let stale = handle
            .enqueue(SimulationCommand::CommitChest {
                primary_position: position,
                positions: vec![position],
                expected_state_id: 1,
                actor_session: actor,
                expected: vec![initial],
                updated: vec![stale_update],
                player: Box::new(player),
            })
            .unwrap();

        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &sessions,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    2,
                )
                .await
                .processed,
            2
        );
        assert!(matches!(
            first.await.unwrap().unwrap(),
            SimulationResponse::ChestCommit(Ok(outcome))
                if matches!(*outcome, SharedContainerCommit::Committed { state_id: 2, .. })
        ));
        let SimulationResponse::ChestCommit(Ok(outcome)) = stale.await.unwrap().unwrap() else {
            panic!("regional stale chest response mismatch");
        };
        let SharedContainerCommit::Rejected {
            state_id,
            authoritative,
            ..
        } = *outcome
        else {
            panic!("regional stale chest commit unexpectedly applied");
        };
        assert_eq!(state_id, 2);
        assert_eq!(authoritative, vec![first_update]);
    }

    #[test]
    fn chest_commits_in_distinct_regions_overlap() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(blocks);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
        for chunk in chunks {
            storage
                .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
                .unwrap();
        }
        let positions = [
            BlockPos { x: 1, y: 64, z: 1 },
            BlockPos {
                x: 8 * 16 + 1,
                y: 64,
                z: 1,
            },
        ];
        let mut initial = mc_world::ChestBlockEntity::default();
        initial.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut updated = initial.clone();
        updated.slots[0].count = 1;
        for position in positions {
            storage.set_block_at(position, BlockStateId(1)).unwrap();
            storage
                .set_chest_block_entity(position, initial.clone())
                .unwrap();
        }
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actors = [
            register_test_session(&sessions, "RegionalChestActorA"),
            register_test_session(&sessions, "RegionalChestActorB"),
        ];
        for (actor, position) in actors.into_iter().zip(positions) {
            assert_eq!(sessions.register_chest_viewer(actor, position), 1);
            register_test_player_state(&sessions, actor, PlayerInventory::empty());
        }
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let responses = actors
            .into_iter()
            .zip(positions)
            .map(|(actor, position)| {
                handle
                    .enqueue(SimulationCommand::CommitChest {
                        primary_position: position,
                        positions: vec![position],
                        expected_state_id: 1,
                        actor_session: actor,
                        expected: vec![initial.clone()],
                        updated: vec![updated.clone()],
                        player: Box::new(empty_container_player_plan()),
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        sessions.install_container_commit_probe(entered_tx, release_rx);

        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let worker = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(owner.process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    2,
                ))
        });

        let first_region = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("first regional chest metadata lock");
        let second_region = entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("second regional chest metadata lock before release");
        assert_ne!(first_region, second_region);
        release_tx.send(()).unwrap();
        release_tx.send(()).unwrap();

        assert_eq!(worker.join().unwrap().processed, 2);
        for response in responses {
            assert!(matches!(
                response.blocking_recv().unwrap().unwrap(),
                SimulationResponse::ChestCommit(Ok(outcome))
                    if matches!(*outcome, SharedContainerCommit::Committed { state_id: 2, .. })
            ));
        }
        for position in positions {
            assert_eq!(
                world.blocking_lock().chest_block_entity(position).unwrap(),
                Some(updated.clone())
            );
        }
    }

    #[test]
    fn queued_chest_commit_moves_container_player_cursor_and_drop_once() {
        let mut initial = mc_world::ChestBlockEntity::default();
        initial.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut first_update = initial.clone();
        first_update.slots[0].count = 1;
        let mut stale_update = initial.clone();
        stale_update.slots[0] = mc_world::FurnaceSlot::EMPTY;
        let (mut storage, pos) = test_container_storage();
        storage
            .set_chest_block_entity(pos, initial.clone())
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let actor = register_test_session(&sessions, "AtomicChestActor");
        let (observer, mut observer_rx) =
            register_test_session_with_outbound(&sessions, "AtomicChestObserver");
        dispatch_visibility_commands(sessions.mark_loaded(observer, (0, 0)));
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == actor
        ));
        assert_eq!(sessions.register_chest_viewer(actor, pos), 1);
        assert_eq!(sessions.register_chest_viewer(observer, pos), 1);
        let before_inventory = PlayerInventory::empty();
        let player_state = register_test_player_state(&sessions, actor, before_inventory.clone());
        let before_carried_item = ItemStack::new(99, 2);
        player_state.lock().unwrap().carried_item = before_carried_item.clone();
        let mut updated_inventory = before_inventory.clone();
        updated_inventory.slots[9] = ItemStack::new(42, 1);
        let updated_carried_item = ItemStack::new(99, 1);
        let drop_position = Vec3::new(0.5, 65.0, 0.5);
        let player = ContainerPlayerPlan {
            expected_inventory: before_inventory,
            expected_carried_item: before_carried_item,
            updated_inventory: updated_inventory.clone(),
            updated_carried_item: updated_carried_item.clone(),
            crafting_table_input: None,
            enchanting_table_input: None,
            drops: vec![ContainerDropPlan {
                entity_type_id: 1,
                position: drop_position,
                stack: EntityItemStack::new(99, 1),
            }],
            xp_orb: None,
        };
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let first = handle
            .enqueue(SimulationCommand::CommitChest {
                primary_position: pos,
                positions: vec![pos],
                expected_state_id: 1,
                actor_session: actor,
                expected: vec![initial.clone()],
                updated: vec![first_update.clone()],
                player: Box::new(player.clone()),
            })
            .unwrap();
        let stale = handle
            .enqueue(SimulationCommand::CommitChest {
                primary_position: pos,
                positions: vec![pos],
                expected_state_id: 1,
                actor_session: actor,
                expected: vec![initial],
                updated: vec![stale_update],
                player: Box::new(player),
            })
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 2)
                .processed,
            2
        );
        assert!(matches!(
            observer_rx.blocking_recv(),
            Some(OutboundCommand::ChestSlots { state_id: 2, .. })
        ));
        assert!(matches!(
            observer_rx.blocking_recv(),
            Some(OutboundCommand::SpawnEntity(entity))
                if entity.item_stack == Some(EntityItemStack::new(99, 1))
        ));
        assert!(matches!(
            first.blocking_recv().unwrap().unwrap(),
            SimulationResponse::ChestCommit(Ok(outcome))
                if matches!(
                    *outcome,
                    SharedContainerCommit::Committed {
                        state_id: 2,
                        ref inventory,
                        ref carried_item,
                        ..
                    } if inventory.slots == updated_inventory.slots
                        && carried_item == &updated_carried_item
                )
        ));
        let (authoritative, rejected_inventory, rejected_carried_item) =
            match stale.blocking_recv().unwrap().unwrap() {
                SimulationResponse::ChestCommit(Ok(outcome)) => match *outcome {
                    SharedContainerCommit::Rejected {
                        state_id,
                        authoritative,
                        inventory,
                        carried_item,
                    } => {
                        assert_eq!(state_id, 2);
                        (authoritative, inventory, carried_item)
                    }
                    other => panic!("expected stale chest rejection, got {other:?}"),
                },
                other => panic!("expected chest response, got {other:?}"),
            };

        assert_eq!(authoritative, vec![first_update.clone()]);
        assert_eq!(rejected_inventory.slots, updated_inventory.slots);
        assert_eq!(rejected_carried_item, updated_carried_item);
        assert_eq!(
            world.blocking_lock().chest_block_entity(pos).unwrap(),
            Some(first_update)
        );
        let persisted = player_state.lock().unwrap();
        assert_eq!(persisted.inventory.slots, updated_inventory.slots);
        assert_eq!(persisted.carried_item, updated_carried_item);
        drop(persisted);
        let dropped = sessions.persisted_entity_records();
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].position, drop_position);
        assert_eq!(dropped[0].item_stack, Some(EntityItemStack::new(99, 1)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_furnace_commit_matches_direct_state_and_viewer_version() {
        let mut initial = mc_world::FurnaceBlockEntity::default();
        initial.slots[1] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut updated = initial.clone();
        updated.slots[1].count = 1;

        let (mut direct_storage, pos) = test_container_storage();
        direct_storage
            .set_furnace_block_entity(pos, initial.clone())
            .unwrap();
        let direct_sessions = SessionRegistry::new();
        let (direct_state_id, _) = direct_sessions
            .try_furnace_slot_dispatches(pos, 1, 7, super::super::furnace_slot_stacks(&updated))
            .unwrap();
        direct_storage
            .set_furnace_block_entity(pos, updated.clone())
            .unwrap();

        let (mut queued_storage, queued_pos) = test_container_storage();
        queued_storage
            .set_furnace_block_entity(queued_pos, initial.clone())
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
        let queued_sessions = Arc::new(SessionRegistry::new());
        let session = register_test_session(&queued_sessions, "QueuedFurnaceActor");
        queued_sessions.register_furnace_viewer(session, queued_pos);
        let player = empty_container_player_plan();
        register_test_player_state(&queued_sessions, session, player.expected_inventory.clone());
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request =
            Box::pin(session_handle.commit_furnace(queued_pos, 1, initial, updated, player));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&queued_sessions, Some(&world), None, 1)
                .processed,
            1
        );
        let outcome = request.await.unwrap();
        let queued_state_id = match outcome {
            SharedContainerCommit::Committed {
                state_id,
                dispatches,
                ..
            } => {
                assert!(dispatches.is_empty());
                state_id
            }
            other => panic!("expected committed furnace, got {other:?}"),
        };

        assert_eq!(queued_state_id, direct_state_id);
        assert_eq!(
            world.lock().await.furnace_block_entity(queued_pos).unwrap(),
            direct_storage.furnace_block_entity(pos).unwrap()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resident_furnace_commit_completes_while_global_world_writer_is_held() {
        let mut initial = mc_world::FurnaceBlockEntity::default();
        initial.slots[1] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut updated = initial.clone();
        updated.slots[1].count = 1;
        let (mut storage, position) = test_container_storage();
        storage
            .set_furnace_block_entity(position, initial.clone())
            .unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actor = register_test_session(&sessions, "RegionalFurnaceActor");
        assert_eq!(sessions.register_furnace_viewer(actor, position), 1);
        let player = empty_container_player_plan();
        register_test_player_state(&sessions, actor, player.expected_inventory.clone());
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(actor);
        let mut request =
            Box::pin(session_handle.commit_furnace(position, 1, initial, updated.clone(), player));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .expect("resident furnace completion event")
            .expect("resident furnace response");
        drop(writer);

        assert_eq!(owner_task.await.unwrap().processed, 1);
        assert!(matches!(
            outcome,
            SharedContainerCommit::Committed { state_id: 2, .. }
        ));
        assert_eq!(
            world.lock().await.furnace_block_entity(position).unwrap(),
            Some(updated)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn furnace_output_take_clears_used_recipes_and_spawns_xp_in_one_owner_commit() {
        let mut initial = mc_world::FurnaceBlockEntity::default();
        initial.slots[2] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 1,
            damage: None,
            enchantments: Vec::new(),
        };
        initial
            .recipes_used
            .insert("minecraft:test_smelting".to_string(), 1);
        let mut updated = initial.clone();
        updated.slots[2] = mc_world::FurnaceSlot::default();
        updated.recipes_used.clear();

        let (mut storage, position) = test_container_storage();
        storage
            .set_furnace_block_entity(position, initial.clone())
            .unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actor = register_test_session(&sessions, "FurnaceXpActor");
        sessions.register_furnace_viewer(actor, position);
        let mut player = empty_container_player_plan();
        player.updated_inventory.slots[9] = ItemStack::new(42, 1);
        let xp_position = Vec3::new(0.5, 64.0, 0.5);
        player.xp_orb = Some(ContainerXpPlan {
            entity_type_id: 49,
            position: xp_position,
            value: 1,
        });
        register_test_player_state(&sessions, actor, player.expected_inventory.clone());
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(actor);
        let mut request =
            Box::pin(session_handle.commit_furnace(position, 1, initial, updated, player));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &sessions,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
                .processed,
            1
        );
        assert!(matches!(
            request.await.unwrap(),
            SharedContainerCommit::Committed { ref inventory, .. }
                if inventory.slots[9] == ItemStack::new(42, 1)
        ));

        let furnace = world
            .lock()
            .await
            .furnace_block_entity(position)
            .unwrap()
            .unwrap();
        assert!(furnace.slots[2].is_empty());
        assert!(furnace.recipes_used.is_empty());
        let entities = sessions.persisted_entity_records();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].type_name, "minecraft:experience_orb");
        assert_eq!(entities[0].type_id, 49);
        assert_eq!(entities[0].position, xp_position);
        assert_eq!(entities[0].experience_value, Some(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn furnace_click_merges_slots_with_newer_owner_tick_data() {
        let mut expected = mc_world::FurnaceBlockEntity {
            burn_remaining: 10,
            burn_total: 10,
            ..mc_world::FurnaceBlockEntity::default()
        };
        expected.slots[1] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 2,
            damage: None,
            enchantments: Vec::new(),
        };
        let mut current = expected.clone();
        current.burn_remaining = 9;
        current.cook_progress = 1;
        let mut updated = expected.clone();
        updated.slots[1].count = 1;

        let (mut storage, position) = test_container_storage();
        storage
            .set_furnace_block_entity(position, current.clone())
            .unwrap();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let actor = register_test_session(&sessions, "FurnaceTickMerge");
        sessions.register_furnace_viewer(actor, position);
        let player = empty_container_player_plan();
        register_test_player_state(&sessions, actor, player.expected_inventory.clone());
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(actor);
        let request = session_handle.commit_furnace(position, 1, expected, updated.clone(), player);
        tokio::pin!(request);
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_commands_with_world_views(
                    &sessions,
                    Some(&world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
                .processed,
            1
        );
        assert!(matches!(
            request.await.unwrap(),
            SharedContainerCommit::Committed { .. }
        ));

        let persisted = world
            .lock()
            .await
            .furnace_block_entity(position)
            .unwrap()
            .unwrap();
        assert_eq!(persisted.slots, updated.slots);
        assert_eq!(persisted.burn_remaining, current.burn_remaining);
        assert_eq!(persisted.cook_progress, current.cook_progress);
    }

    #[test]
    fn queued_container_commit_rejects_busy_world_without_mutation() {
        let initial = mc_world::FurnaceBlockEntity::default();
        let mut updated = initial.clone();
        updated.slots[0] = mc_world::FurnaceSlot {
            item_id: 42,
            count: 1,
            damage: None,
            enchantments: Vec::new(),
        };
        let (mut storage, pos) = test_container_storage();
        storage
            .set_furnace_block_entity(pos, initial.clone())
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .enqueue(SimulationCommand::CommitFurnace {
                position: pos,
                expected_state_id: 1,
                actor_session: 7,
                expected: initial.clone(),
                updated,
                player: Box::new(empty_container_player_plan()),
            })
            .unwrap();
        let mut guard = world.try_lock().unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::FurnaceCommit(Err(SimulationRequestError::WorldBusy))
        ));
        assert_eq!(guard.furnace_block_entity(pos).unwrap(), Some(initial));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn queued_opaque_block_entity_commit_matches_direct_storage_write() {
        let bytes = vec![10, 0, 0, 0];
        let (mut direct_storage, pos, direct_token) = test_block_storage();
        assert!(
            apply_opaque_block_entity_to_storage_conditionally(
                &mut direct_storage,
                pos,
                BlockStateId(1),
                direct_token,
                bytes.clone(),
            )
            .unwrap()
        );

        let (queued_storage, queued_pos, queued_token) = test_block_storage();
        let read_view = queued_storage.read_view();
        let mutation_view = queued_storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
        let sessions = Arc::new(SessionRegistry::new());
        let session = register_test_session(&sessions, "QueuedSignActor");
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_opaque_block_entity(
            queued_pos,
            BlockStateId(1),
            queued_token,
            bytes.clone(),
        ));
        assert_request_enqueued(request.as_mut(), &handle).await;
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), request)
                .await
                .expect("resident opaque block-entity completion event")
                .expect("resident opaque block-entity response")
        );
        drop(writer);

        assert_eq!(owner_task.await.unwrap().processed, 1);
        assert_eq!(handle.snapshot().block_entity_commits_processed, 1);
        let queued = world.lock().await;
        let direct = direct_storage
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .get(&pos)
            .cloned();
        let queued = queued
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .get(&queued_pos)
            .cloned();
        assert_eq!(queued, direct);
        assert_eq!(queued, Some(bytes));
    }

    #[test]
    fn queued_opaque_block_entity_rejects_stale_token_without_write() {
        let (mut storage, pos, stale_token) = test_block_storage();
        storage.set_block_at(pos, BlockStateId(0)).unwrap();
        storage.set_block_at(pos, BlockStateId(1)).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .enqueue(SimulationCommand::CommitOpaqueBlockEntity {
                position: pos,
                expected_state: BlockStateId(1),
                expected_token: stale_token,
                bytes: vec![10, 0, 0, 0],
            })
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(matches!(
            response.blocking_recv().unwrap().unwrap(),
            SimulationResponse::OpaqueBlockEntity(Ok(false))
        ));
        assert!(
            !world
                .blocking_lock()
                .cached_chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .block_entities
                .contains_key(&pos)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn campfire_use_transaction_survives_requester_loss_after_owner_apply() {
        let input = ItemStack::new(42, 1);
        let result = ItemStack::new(43, 1);
        let expected = super::super::CampfireCookingState::default();
        let mut updated = expected.clone();
        assert!(updated.insert(input, result, 20));
        let bytes = vec![10, 0, 0, 0];
        let client_nbt = mc_nbt::Tag::Compound(Vec::new());
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let (session, _actor_rx) =
            register_test_session_with_outbound(&sessions, "LostCampfireUseRequester");
        let (observer, mut observer_rx) =
            register_test_session_with_outbound(&sessions, "CampfireUseObserver");
        sessions.replace_view(session, (0, 0), 2, HashSet::from([(0, 0)]));
        sessions.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
        sessions.mark_loaded(session, (0, 0));
        sessions.mark_loaded(observer, (0, 0));
        while observer_rx.try_recv().is_ok() {}
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&sessions, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_campfire_use(CampfireUsePlan {
            position: pos,
            expected_state: BlockStateId(1),
            expected_token: token,
            expected_cooking: expected,
            updated_cooking: updated.clone(),
            persistent_bytes: bytes.clone(),
            client_nbt: client_nbt.clone(),
            held_slot: PlayerInventory::HOTBAR_BASE,
            expected_held: ItemStack::new(42, 2),
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );
        drop(request);

        assert_eq!(sessions.campfire_cooking_state(pos), updated);
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
        assert_eq!(
            world
                .lock()
                .await
                .cached_chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .block_entities
                .get(&pos),
            Some(&bytes)
        );
        assert!(matches!(
            observer_rx.try_recv(),
            Ok(OutboundCommand::BlockEntityData {
                position: observed_position,
                nbt,
                ..
            }) if observed_position == pos && nbt == client_nbt
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn campfire_use_transaction_rejects_stale_held_stack_without_world_change() {
        let expected = super::super::CampfireCookingState::default();
        let mut updated = expected.clone();
        assert!(updated.insert(ItemStack::new(42, 1), ItemStack::new(43, 1), 20));
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let session = register_test_session(&sessions, "StaleCampfireUseRequester");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&sessions, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_campfire_use(CampfireUsePlan {
            position: pos,
            expected_state: BlockStateId(1),
            expected_token: token,
            expected_cooking: expected,
            updated_cooking: updated,
            persistent_bytes: vec![10, 0, 0, 0],
            client_nbt: mc_nbt::Tag::Compound(Vec::new()),
            held_slot: PlayerInventory::HOTBAR_BASE,
            expected_held: ItemStack::new(42, 2),
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );

        assert!(request.await.unwrap().is_none());
        assert!(sessions.campfire_cooking_state(pos).is_empty());
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
        assert!(
            !world
                .lock()
                .await
                .cached_chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .block_entities
                .contains_key(&pos)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resident_campfire_use_does_not_wait_for_world_writer() {
        let expected = super::super::CampfireCookingState::default();
        let mut updated = expected.clone();
        assert!(updated.insert(ItemStack::new(42, 1), ItemStack::new(43, 1), 20));
        let bytes = vec![10, 0, 0, 0];
        let (storage, pos, token) = test_block_storage();
        let read_view = storage.read_view();
        let mutation_view = storage.mutation_view();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = Arc::new(SessionRegistry::new());
        let session = register_test_session(&sessions, "RegionalCampfireActor");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&sessions, session, inventory);
        let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_campfire_use(CampfireUsePlan {
            position: pos,
            expected_state: BlockStateId(1),
            expected_token: token,
            expected_cooking: expected,
            updated_cooking: updated.clone(),
            persistent_bytes: bytes.clone(),
            client_nbt: mc_nbt::Tag::Compound(Vec::new()),
            held_slot: PlayerInventory::HOTBAR_BASE,
            expected_held: ItemStack::new(42, 1),
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;

        let writer = world.lock().await;
        let owner_world = Arc::clone(&world);
        let owner_sessions = Arc::clone(&sessions);
        let owner_task = tokio::spawn(async move {
            owner
                .process_commands_with_world_views(
                    &owner_sessions,
                    Some(&owner_world),
                    SimulationWorldAccess {
                        read: Some(&read_view),
                        mutation: Some(&mutation_view),
                        cpu: Some(&resources),
                        light: None,
                    },
                    None,
                    1,
                )
                .await
        });

        let completion = tokio::time::timeout(std::time::Duration::from_millis(300), request).await;
        drop(writer);
        assert_eq!(owner_task.await.unwrap().processed, 1);
        let committed = completion
            .expect("resident campfire completion event")
            .expect("campfire response")
            .expect("matching resident campfire use commits");

        assert_eq!(committed.changed_slots.len(), 1);
        assert_eq!(sessions.campfire_cooking_state(pos), updated);
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::EMPTY
        );
        let chunk = world
            .lock()
            .await
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap();
        assert_eq!(chunk.block_entities.get(&pos), Some(&bytes));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survival_tnt_ignition_expires_after_exactly_eighty_ticks() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let tnt = BlockPos { x: 1, y: 64, z: 1 };
        let chained_tnt = BlockPos { x: 2, y: 64, z: 1 };
        let protected = BlockPos { x: 1, y: 64, z: 2 };
        storage.set_block_at(tnt, BlockStateId(5)).unwrap();
        storage.set_block_at(protected, BlockStateId(6)).unwrap();
        let tnt_token = storage.block_mutation_token(tnt).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));

        let sessions = SessionRegistry::new();
        let session = register_test_session(&sessions, "TntIgnitionActor");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let player_state = register_test_player_state(&sessions, session, inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_tnt_ignition(TntIgnitionPlan {
            tnt: BlockEditPrecondition {
                pos: tnt,
                expected_state: BlockStateId(5),
                expected_token: tnt_token,
            },
            air: BlockStateId(0),
            game_mode: GameMode::Survival,
            held_slot: PlayerInventory::HOTBAR_BASE,
            expected_held: ItemStack::new(42, 1),
            flint_and_steel_max_damage: 64,
            tnt_entity_type_id: 132,
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );
        let committed = request.await.unwrap().expect("matching ignition commits");
        assert_eq!(committed.block.applied.len(), 1);
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            Some(1)
        );
        let saved = sessions.persisted_entity_save_snapshot().0;
        assert_eq!(saved.records.len(), 1, "primed TNT is retained by ECS");
        assert!(saved.records[0].snapshot.retained.primed_tnt.is_some());
        let restored = SessionRegistry::new();
        assert_eq!(restored.restore_persisted_entities(saved), 1);
        assert_eq!(restored.persisted_entity_records().len(), 1);
        assert!(
            restored
                .claim_due_primed_tnt(&SimulationAuthority::for_test(), 79)
                .is_empty()
        );
        assert_eq!(
            restored
                .claim_due_primed_tnt(&SimulationAuthority::for_test(), 80)
                .len(),
            1,
            "restored TNT must be scheduled in the deadline index"
        );

        world
            .lock()
            .await
            .set_block_at(chained_tnt, BlockStateId(5))
            .unwrap();

        let mut explosion_resistance = vec![0.0; 29_873];
        explosion_resistance[6] = 0.5;
        let block_facts = BlockFactsTable::default().with_explosion_table(
            mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
                .unwrap(),
        );
        let materials = mc_physics::BlockMaterialIds::new(0, None, None);

        owner.advance_world_time(&sessions, 79);
        assert_eq!(
            owner
                .tick_primed_tnt(
                    &sessions,
                    Some(&world),
                    None,
                    &block_facts,
                    &blocks,
                    Some(&materials),
                    || panic!("claim snapshot must stay lazy without a due explosion"),
                )
                .await,
            0
        );
        assert_eq!(
            world.lock().await.get_block(chained_tnt).unwrap(),
            Some(BlockStateId(5))
        );
        owner.advance_world_time(&sessions, 1);
        let claim_protection = crate::script::ClaimProtectionSnapshot::from_zones(vec![
            ScriptAxisAlignedZone::try_new(
                "protected-test",
                "minecraft:overworld",
                ScriptPosition::try_new(
                    f64::from(protected.x),
                    f64::from(protected.y),
                    f64::from(protected.z),
                )
                .unwrap(),
                ScriptPosition::try_new(
                    f64::from(protected.x),
                    f64::from(protected.y),
                    f64::from(protected.z),
                )
                .unwrap(),
            )
            .unwrap(),
        ]);
        assert_eq!(
            owner
                .tick_primed_tnt(
                    &sessions,
                    Some(&world),
                    None,
                    &block_facts,
                    &blocks,
                    Some(&materials),
                    || Some(claim_protection),
                )
                .await,
            1
        );
        assert_eq!(
            world.lock().await.get_block(chained_tnt).unwrap(),
            Some(BlockStateId(0))
        );
        assert_eq!(
            world.lock().await.get_block(protected).unwrap(),
            Some(BlockStateId(6)),
            "claim snapshot must remove protected blocks from explosion candidates"
        );
        let chained_fuses = sessions.primed_tnt_fuses_for_test();
        assert_eq!(chained_fuses.len(), 1);
        assert!((90..=109).contains(&chained_fuses[0].1));
        assert_eq!(sessions.persisted_entity_records().len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn simultaneous_tnt_explosions_publish_each_drop_before_its_packet() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let first_dirt = BlockPos { x: 2, y: 64, z: 1 };
        let second_dirt = BlockPos { x: 13, y: 64, z: 1 };
        storage.set_block_at(first_dirt, BlockStateId(6)).unwrap();
        storage.set_block_at(second_dirt, BlockStateId(6)).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));

        let sessions = SessionRegistry::new();
        let profile = LoggedInProfile {
            uuid: crate::login::offline_uuid("TwoTntObserver"),
            name: "TwoTntObserver".to_owned(),
        };
        let (tx, mut outbound) = mpsc::channel(64);
        let session = sessions
            .register(
                &profile,
                (0, 0),
                2,
                HashSet::new(),
                tx,
                PlayerPose::new(7.5, 64.0, 8.5),
            )
            .0;
        sessions.replace_view(session, (0, 0), 2, HashSet::from([(0, 0)]));
        assert!(sessions.mark_loaded(session, (0, 0)).is_empty());

        let (_, mut owner) = simulation_channel();
        let mut tnt_ids = Vec::new();
        for position in [Vec3::new(1.5, 64.0, 1.5), Vec3::new(14.5, 64.0, 1.5)] {
            let spawn = sessions.spawn_chained_primed_tnt(
                &owner.authority,
                132,
                position,
                Vec3::ZERO,
                80,
                BlockStateId(0),
            );
            tnt_ids.push(match &spawn[0].command {
                OutboundCommand::SpawnEntity(entity) => entity.id,
                other => panic!("expected TNT spawn, got {other:?}"),
            });
            super::super::dispatch_visibility_commands(spawn);
        }
        while outbound.try_recv().is_ok() {}
        let delayed_spawn = sessions.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(7.5, 64.0, 7.5),
        );
        assert!(matches!(
            delayed_spawn.as_slice(),
            [VisibilityDispatch {
                command: OutboundCommand::SpawnEntity(_),
                ..
            }]
        ));

        let mut explosion_resistance = vec![0.0; 29_873];
        explosion_resistance[6] = 0.5;
        let block_facts = BlockFactsTable::default().with_explosion_table(
            mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
                .unwrap(),
        );
        let materials = mc_physics::BlockMaterialIds::new(0, None, None);
        owner.advance_world_time(&sessions, 80);
        for index in 0..2 {
            if index != 0 {
                owner.advance_world_time(&sessions, 1);
            }
            assert_eq!(
                owner
                    .tick_primed_tnt(
                        &sessions,
                        Some(&world),
                        None,
                        &block_facts,
                        &blocks,
                        Some(&materials),
                        || None,
                    )
                    .await,
                1
            );
        }
        assert!(
            outbound.try_recv().is_err(),
            "all TNT world and entity publication must wait for older ordered work"
        );
        dispatch_visibility_commands(delayed_spawn);

        let commands = std::iter::from_fn(|| outbound.try_recv().ok()).collect::<Vec<_>>();
        let explosion_indexes = commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| {
                matches!(command, OutboundCommand::Explosion(_)).then_some(index)
            })
            .collect::<Vec<_>>();
        let block_delta_indexes = [first_dirt, second_dirt].map(|position| {
            commands
                .iter()
                .position(|command| match command {
                    OutboundCommand::BlockDeltas(deltas) => deltas.iter().any(|delta| {
                        (delta.x, delta.y, delta.z) == (position.x, position.y, position.z)
                    }),
                    _ => false,
                })
                .expect("matching TNT block delta")
        });
        let drop_indexes = commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| match command {
                OutboundCommand::SpawnEntity(entity) if entity.type_name == "minecraft:item" => {
                    Some(index)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let despawn_indexes = tnt_ids
            .iter()
            .map(|tnt_id| {
                commands
                    .iter()
                    .position(|command| {
                        matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == *tnt_id)
                    })
                    .expect("expired TNT despawn")
            })
            .collect::<Vec<_>>();
        assert_eq!(explosion_indexes.len(), 2, "one packet per expired TNT");
        assert_eq!(drop_indexes.len(), 2, "one dirt drop per explosion");
        assert!(block_delta_indexes[0] < drop_indexes[0]);
        assert!(drop_indexes[0] < despawn_indexes[0]);
        assert!(despawn_indexes[0] < explosion_indexes[0]);
        assert!(
            explosion_indexes[0] < block_delta_indexes[1],
            "the second TNT transaction must not begin before the first explosion packet"
        );
        assert!(block_delta_indexes[1] < drop_indexes[1]);
        assert!(drop_indexes[1] < despawn_indexes[1]);
        assert!(despawn_indexes[1] < explosion_indexes[1]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expired_tnt_waits_for_its_delayed_spawn_publication() {
        let blocks = BlockRegistry::from_report(&test_block_reports()).unwrap();
        let sessions = SessionRegistry::new();
        let profile = LoggedInProfile {
            uuid: crate::login::offline_uuid("DelayedTntObserver"),
            name: "DelayedTntObserver".to_owned(),
        };
        let (tx, mut outbound) = mpsc::channel(16);
        let session = sessions
            .register(
                &profile,
                (0, 0),
                2,
                HashSet::new(),
                tx,
                PlayerPose::new(0.5, 64.0, 0.5),
            )
            .0;
        assert!(sessions.mark_loaded(session, (0, 0)).is_empty());

        let (_, mut owner) = simulation_channel();
        let spawn = sessions.spawn_chained_primed_tnt(
            &owner.authority,
            132,
            Vec3::new(1.5, 64.0, 1.5),
            Vec3::ZERO,
            1,
            BlockStateId(0),
        );
        let tnt_id = match &spawn[0].command {
            OutboundCommand::SpawnEntity(entity) => entity.id,
            other => panic!("expected TNT spawn, got {other:?}"),
        };
        owner.advance_world_time(&sessions, 1);

        assert_eq!(
            owner
                .tick_primed_tnt(
                    &sessions,
                    None,
                    None,
                    &BlockFactsTable::default(),
                    &blocks,
                    None,
                    || None,
                )
                .await,
            1
        );
        assert!(
            outbound.try_recv().is_err(),
            "TNT terminal packets must wait for its required spawn publication"
        );

        dispatch_visibility_commands(spawn);
        assert!(matches!(
            outbound.try_recv(),
            Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == tnt_id
        ));
        assert!(matches!(
            outbound.try_recv(),
            Ok(OutboundCommand::DespawnEntity(entity)) if entity.id == tnt_id
        ));
        assert!(matches!(
            outbound.try_recv(),
            Ok(OutboundCommand::Explosion(_))
        ));
        assert!(outbound.try_recv().is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creative_offhand_tnt_ignition_does_not_mutate_inventory() {
        let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
        let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let tnt = BlockPos { x: 1, y: 64, z: 1 };
        storage.set_block_at(tnt, BlockStateId(5)).unwrap();
        let tnt_token = storage.block_mutation_token(tnt).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));

        let sessions = SessionRegistry::new();
        let session = register_test_session(&sessions, "CreativeOffhandTntActor");
        let mut inventory = PlayerInventory::empty();
        let mut flint_and_steel = ItemStack::new(42, 1);
        flint_and_steel.damage = Some(7);
        inventory.slots[PlayerInventory::OFFHAND_SLOT] = flint_and_steel.clone();
        let player_state = register_test_player_state(&sessions, session, inventory.clone());
        player_state.lock().unwrap().game_mode = GameMode::Creative;

        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_tnt_ignition(TntIgnitionPlan {
            tnt: BlockEditPrecondition {
                pos: tnt,
                expected_state: BlockStateId(5),
                expected_token: tnt_token,
            },
            air: BlockStateId(0),
            game_mode: GameMode::Creative,
            held_slot: PlayerInventory::OFFHAND_SLOT,
            expected_held: flint_and_steel,
            flint_and_steel_max_damage: 64,
            tnt_entity_type_id: 132,
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );
        let committed = request.await.unwrap().expect("matching ignition commits");

        assert!(committed.changed_slots.is_empty());
        assert_eq!(committed.inventory.slots, inventory.slots);
        assert_eq!(
            player_state.lock().unwrap().inventory.slots,
            inventory.slots
        );
        assert_eq!(
            world.lock().await.get_block(tnt).unwrap(),
            Some(BlockStateId(0))
        );
    }

    #[test]
    fn queued_campfire_commits_accept_only_first_matching_snapshot() {
        let expected = super::super::CampfireCookingState::default();
        let mut first_update = expected.clone();
        assert!(first_update.insert(
            super::super::ItemStack::new(42, 1),
            super::super::ItemStack::new(43, 1),
            20,
        ));
        let mut stale_update = expected.clone();
        assert!(stale_update.insert(
            super::super::ItemStack::new(44, 1),
            super::super::ItemStack::new(45, 1),
            20,
        ));
        let (storage, pos, token) = test_block_storage();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = SessionRegistry::new();
        let first_actor = register_test_session(&sessions, "FirstCampfireActor");
        let stale_actor = register_test_session(&sessions, "StaleCampfireActor");
        let mut first_inventory = PlayerInventory::empty();
        first_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        let first_player = register_test_player_state(&sessions, first_actor, first_inventory);
        let mut stale_inventory = PlayerInventory::empty();
        stale_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(44, 1);
        let stale_player = register_test_player_state(&sessions, stale_actor, stale_inventory);
        let (handle, mut owner) = simulation_channel_with_capacity(2);
        let first = handle
            .for_session(first_actor)
            .enqueue_player_command(SimulationCommand::CommitCampfireUse(Box::new(
                CampfireUseCommand {
                    actor_session: first_actor,
                    plan: CampfireUsePlan {
                        position: pos,
                        expected_state: BlockStateId(1),
                        expected_token: token,
                        expected_cooking: expected.clone(),
                        updated_cooking: first_update.clone(),
                        persistent_bytes: vec![1],
                        client_nbt: mc_nbt::Tag::Compound(Vec::new()),
                        held_slot: PlayerInventory::HOTBAR_BASE,
                        expected_held: ItemStack::new(42, 1),
                    },
                },
            )))
            .unwrap();
        let stale = handle
            .for_session(stale_actor)
            .enqueue_player_command(SimulationCommand::CommitCampfireUse(Box::new(
                CampfireUseCommand {
                    actor_session: stale_actor,
                    plan: CampfireUsePlan {
                        position: pos,
                        expected_state: BlockStateId(1),
                        expected_token: token,
                        expected_cooking: expected,
                        updated_cooking: stale_update,
                        persistent_bytes: vec![2],
                        client_nbt: mc_nbt::Tag::Compound(Vec::new()),
                        held_slot: PlayerInventory::HOTBAR_BASE,
                        expected_held: ItemStack::new(44, 1),
                    },
                },
            )))
            .unwrap();

        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 2)
                .processed,
            2
        );
        assert!(matches!(
            first.blocking_recv().unwrap().unwrap(),
            SimulationResponse::CampfireUse(Ok(Some(_)))
        ));
        assert!(matches!(
            stale.blocking_recv().unwrap().unwrap(),
            SimulationResponse::CampfireUse(Ok(None))
        ));
        assert_eq!(sessions.campfire_cooking_state(pos), first_update);
        assert_eq!(
            first_player.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::EMPTY
        );
        assert_eq!(
            stale_player.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(44, 1)
        );
        assert_eq!(
            world
                .blocking_lock()
                .cached_chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .block_entities
                .get(&pos)
                .cloned(),
            Some(vec![1])
        );
    }

    #[test]
    fn owner_budget_carries_remaining_commands_to_next_tick() {
        let registry = SessionRegistry::new();
        let position = Vec3::new(0.5, 64.0, 0.5);
        for value in 1..=3 {
            registry.spawn_xp_orb(2, position, value);
        }
        let ids = registry
            .nearby_experience_entities(position, 2.25)
            .into_iter()
            .map(|entity| entity.id)
            .collect::<Vec<_>>();
        let (handle, mut owner) = simulation_channel_with_capacity(3);
        let mut responses = ids
            .into_iter()
            .map(|entity_id| {
                handle
                    .enqueue(SimulationCommand::ClaimExperiencePickup {
                        entity_id,
                        collector_session: 7,
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();

        assert_eq!(owner.process_tick(&registry, 2).processed, 2);
        assert_eq!(handle.snapshot().depth, 1);
        assert!(matches!(
            responses[2].try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(owner.process_tick(&registry, 2).processed, 1);
        assert_eq!(handle.snapshot().depth, 0);
        assert_eq!(handle.snapshot().processed, 3);
    }

    #[test]
    fn cancelled_and_shutdown_commands_do_not_mutate_entities() {
        let cancelled_registry = SessionRegistry::new();
        let (_, cancelled_xp) = seed_claim_entities(&cancelled_registry);
        let (cancelled_handle, mut cancelled_owner) = simulation_channel_with_capacity(1);
        let cancelled_response = cancelled_handle
            .enqueue(SimulationCommand::ClaimExperiencePickup {
                entity_id: cancelled_xp,
                collector_session: 7,
            })
            .unwrap();
        drop(cancelled_response);
        assert_eq!(
            cancelled_owner
                .process_tick(&cancelled_registry, 1)
                .processed,
            0
        );
        assert_eq!(
            cancelled_registry
                .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .len(),
            1
        );

        let shutdown_registry = SessionRegistry::new();
        let (_, shutdown_xp) = seed_claim_entities(&shutdown_registry);
        let (shutdown_handle, mut shutdown_owner) = simulation_channel_with_capacity(1);
        let shutdown_response = shutdown_handle
            .enqueue(SimulationCommand::ClaimExperiencePickup {
                entity_id: shutdown_xp,
                collector_session: 7,
            })
            .unwrap();
        shutdown_owner.shutdown();
        assert_eq!(
            shutdown_response.blocking_recv().unwrap().unwrap_err(),
            SimulationRequestError::ShuttingDown
        );
        assert_eq!(
            shutdown_registry
                .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
                .len(),
            1
        );
    }
}
