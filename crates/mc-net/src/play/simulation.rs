#[cfg(test)]
use super::HerdSpawn;
use super::SettlementInhabitantSpawn;
use super::block_edit_commit::{
    apply_block_edit_batch_to_storage_conditionally,
    apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally,
    resident_block_edit_result_outcome, resident_block_edits, resident_block_preconditions,
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
use super::movement::{
    PlayerMovementAuthorityResources, PlayerMovementRejection, PlayerPoseCommitKind,
};
use super::owned_inventory::{
    WarehouseStructureMaterialDebit, WarehouseTransferOutcome, WarehouseTransferRequest,
    container_chest_image,
};
use super::persistence::PersistedEntityCheckpoint;
#[cfg(test)]
use super::session::EntityKillRewards;
use super::session::resident_orders::{ResidentAttack, ResidentHit};
use super::session::{
    BucketUseTransaction, CampfireUseTransaction, ChestTransaction, ChestTransactionRequest,
    ContainerCommitContext, ContainerStateCommitError, CreditedArrowPickup,
    CreditedExperiencePickup, CreditedItemPickup, ENTITY_DEATH_TICKS, EntityAttackOutcome,
    FurnaceTransaction, FurnaceTransactionRequest, OutboundCommand, PlayerAttackResult,
    PlayerEntityAttack, PlayerInventoryCommitError, ScriptPlayerTeleportCompletion,
    ServerEntityExplosionImpact, ServerOwnedChestCommit, SessionId, SessionRegistry,
    SurvivalBreakTransaction, SurvivalPlacementTransaction, VillagerPopulationSelection,
    VisibilityDispatch, dispatch_visibility_commands,
};
use super::{
    AppliedBlockEdit, BlockEdit, BlockEditBatchOutcome, BlockEditPrecondition,
    BlockMutationSnapshot, CAMPFIRE_BLOCK_ENTITY_TYPE_ID, CampfireCookingState, ChestCommitOutcome,
    ChestView, ContainerDropPlan, ContainerPlayerPlan, ContainerXpPlan, FurnaceCommitOutcome,
    GameMode, MAX_BLOCK_EDIT_COMMAND_EDITS, PendingCampfireOutput, PlayerInventoryCommitOutcome,
    PlayerPose, SharedContainerCommit, SurvivalState, WorldHandle, air_state_id,
    block_edit_changes_light, chest_menu_state_change_count, chest_slot_stacks,
    furnace_output_was_taken, furnace_slot_stacks, is_campfire_block,
    schedule_fluid_ticks_near_applied, schedule_leaf_ticks_near_applied,
};
use mc_data::block_facts::BlockFactsTable;
use mc_data::block_light::BlockLightTable;
use mc_data::{Identifier, ItemStack, tags::TagsData};
use mc_entity::runtime_26_1_2::TargetKind;
use mc_entity::villager_population_26_1_2::VillagerFoodItemIds;
use mc_entity::{
    EntityEffectOperation, EntityEffectRequest, EntityEffectResult, EntityId, EntityItemStack,
    EntitySnapshot, EntityTrackingMotion, REGION_SIZE_CHUNKS, RegionKey, RegionLease,
    RegionOwnership, RegionOwnershipError, RegionPhase, Rotation, Vec3,
};
use mc_physics::BlockMaterialIds;
use mc_script::ScriptPlayerTeleportFailure;
use mc_world::{
    BlockMutationToken, BlockPos, BlockRegistry, BlockStateId, ChestBlockEntity,
    FurnaceBlockEntity, ResidentBlockEditBatchResult, ScheduledBlockTick, WorldError,
    WorldMutationView, WorldReadView, WorldStorage,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
#[cfg(feature = "load-bench")]
use std::time::Instant;
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::sync::oneshot;
use tracing::{trace, warn};

#[cfg(test)]
mod block_drop_tests;
#[cfg(test)]
mod inventory_recovery_tests;
#[cfg(test)]
mod player_teleport_tests;
mod queue;
mod regional_mutation;
mod request_wait;
mod save_barrier;

/// The native owner's side of the pre-commit hooks: the questions a build asks
/// and the one ticket it spends at the commit.
pub(in crate::play) mod precommit;

#[allow(unused_imports)]
pub(crate) use queue::SIMULATION_COMMAND_QUEUE_CAPACITY;
pub(in crate::play) use queue::SimulationResponseSender;
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
const SIMULATION_QUEUE_ADMISSION_TIMEOUT: Duration = Duration::from_millis(250);
const SIMULATION_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

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
    consumed: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
tokio::task_local! {
    static BLOCK_DROP_AWAIT_PROBE: Arc<BlockDropAwaitProbe>;
}

#[cfg(test)]
fn block_drop_await_probe(
    stage: BlockDropAwaitStage,
    entered: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) -> Arc<BlockDropAwaitProbe> {
    Arc::new(BlockDropAwaitProbe {
        stage,
        entered,
        release: std::sync::Mutex::new(release),
        consumed: std::sync::atomic::AtomicBool::new(false),
    })
}

#[cfg(test)]
async fn with_block_drop_await_probe<F>(probe: Arc<BlockDropAwaitProbe>, future: F) -> F::Output
where
    F: std::future::Future,
{
    BLOCK_DROP_AWAIT_PROBE.scope(probe, future).await
}

#[cfg(test)]
async fn pause_block_drop_after(stage: BlockDropAwaitStage) {
    let probe = BLOCK_DROP_AWAIT_PROBE
        .try_with(Arc::clone)
        .ok()
        .filter(|probe| probe.stage == stage);
    let Some(probe) = probe else {
        return;
    };
    if probe
        .consumed
        .swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    probe.entered.send(()).expect("block-drop probe receiver");
    let waiter = Arc::clone(&probe);
    tokio::task::spawn_blocking(move || {
        waiter
            .release
            .lock()
            .expect("test lock poisoned")
            .recv()
            .expect("block-drop probe release");
    })
    .await
    .expect("block-drop probe worker");
}

fn elapsed_us(started: std::time::Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SimulationRequestError {
    Full,
    QueueAdmissionTimeout,
    Closed,
    OwnerStopped,
    ResponseTimeout,
    ResponseMismatch,
    ShuttingDown,
    #[cfg(test)]
    WorldBusy,
    WorldUnavailable,
    WorldMutationFailed,
    CrossRegion,
    InvalidCommand,
    /// A before-build or before-damage chain refused the frozen effect.  Preserve
    /// the native reason so callers can treat cancellation, expiry and stale
    /// tickets as terminal refusals rather than retrying a gameplay action.
    Precommit(mc_script::precommit::HookFailure),
    StaleSession,
    PlayerMovementRejected(PlayerMovementRejection),
}

#[derive(Debug)]
pub(super) struct SimulationAuthority(());

#[derive(Clone)]
pub(crate) struct EntitySimulationWorldContext<'a> {
    world_read: Option<&'a mc_world::WorldReadView>,
    pathing_materials: Option<Arc<mc_physics::BlockMaterialIds>>,
    blocks: Option<&'a mc_world::BlockRegistry>,
    items: Option<&'a mc_data::items::ItemRegistry>,
}

#[derive(Clone, Copy)]
pub(crate) struct EntitySimulationTickPolicy {
    pub(crate) pathing_candidates_per_entity: usize,
    pub(crate) simulation_distance: i32,
}

pub(crate) struct RegionallyCommittedEntityMovement {
    pub(crate) states: Vec<EntityTrackingMotion>,
    pub(crate) fence: mc_entity::VersionedEntitySnapshots,
}

impl<'a> EntitySimulationWorldContext<'a> {
    pub(crate) fn new(
        world_read: Option<&'a mc_world::WorldReadView>,
        pathing_materials: Option<&Arc<mc_physics::BlockMaterialIds>>,
        blocks: &'a mc_world::BlockRegistry,
        items: &'a mc_data::items::ItemRegistry,
    ) -> Self {
        Self {
            world_read,
            pathing_materials: pathing_materials.cloned(),
            blocks: Some(blocks),
            items: Some(items),
        }
    }

    #[cfg(test)]
    pub(in crate::play) const fn empty() -> Self {
        Self {
            world_read: None,
            pathing_materials: None,
            blocks: None,
            items: None,
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn with_pathing_for_test(
        world_read: &'a mc_world::WorldReadView,
        pathing_materials: Arc<mc_physics::BlockMaterialIds>,
    ) -> Self {
        Self {
            world_read: Some(world_read),
            pathing_materials: Some(pathing_materials),
            blocks: None,
            items: None,
        }
    }

    pub(in crate::play) fn pathing(
        &self,
    ) -> Option<(&'a mc_world::WorldReadView, &mc_physics::BlockMaterialIds)> {
        self.world_read.zip(self.pathing_materials.as_deref())
    }

    pub(in crate::play) fn regional_pathing_materials(
        &self,
    ) -> Option<Arc<mc_physics::BlockMaterialIds>> {
        self.pathing_materials.clone()
    }

    pub(in crate::play) fn profession_context(
        &self,
    ) -> Option<(
        &'a mc_world::WorldReadView,
        &'a mc_world::BlockRegistry,
        &'a mc_data::items::ItemRegistry,
    )> {
        Some((self.world_read?, self.blocks?, self.items?))
    }
}

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
    pub(crate) daylight_cycle_enabled: bool,
    pub(crate) weather: super::session::WeatherState,
    pub(crate) players_sleeping_percentage: u32,
    pub(crate) keep_inventory: bool,
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
            .field("daylight_cycle_enabled", &self.daylight_cycle_enabled)
            .field("weather", &self.weather)
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
    /// A detached before-damage completion.  The target owner validates and
    /// consumes its frozen ticket; this command never waits on guest code.
    ResumePlayerDamagePrecommit(Box<super::session::damage_precommit::PlayerDamagePrecommitResume>),
    ResumeEntityDamagePrecommit(Box<super::session::damage_precommit::EntityDamagePrecommitResume>),
    /// The owner derives the exact break batch under its current world fence,
    /// then resolves before-build off the owner turn and resumes that batch.
    BeginPrecommitSurvivalBreak(Box<HookedSurvivalBreakCommand>),
    BeginPrecommitSurvivalPlacement(Box<HookedSurvivalPlacementCommand>),
    BeginPrecommitBucketUse(Box<HookedBucketUseCommand>),
    PrecommitSurvivalPlacementFailure(mc_script::precommit::HookFailure),
    PrecommitBucketUseFailure(mc_script::precommit::HookFailure),
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
    DamageScriptEntity {
        entity_id: EntityId,
        damage: f32,
        plugin_id: String,
    },
    DamageResidentEntity {
        attack: Box<ResidentAttack>,
        plugin_id: String,
    },
    SetWorldTime {
        world_time: u64,
    },
    #[cfg(test)]
    EnsureChunkHerd {
        chunk: (i32, i32),
        spawns: Vec<HerdSpawn>,
    },
    EnsureSettlementInhabitants {
        chunk: (i32, i32),
        spawns: Vec<SettlementInhabitantSpawn>,
    },
    ApplyBlockEdits {
        /// A current protection-image fence for a server-owned programmatic
        /// build.  Player paths carry their own permission checks; a changed
        /// image refuses before the approval or world mutation is consumed.
        zone_fence: Option<crate::script::ZoneProtectionFence>,
        actor_session: Option<SessionId>,
        edits: Vec<BlockEdit>,
        preconditions: Vec<BlockEditPrecondition>,
        scheduled_block_ticks: Vec<ScheduledBlockTick>,
        /// Whether this batch schedules neighbouring leaf decay after an
        /// applied edit. Resident crop and ore breaks leave this false.
        leaf_trigger: bool,
        /// The chain's ticket for this batch, spent by the owner before it
        /// applies anything. `None` is every batch committed without a
        /// registered `before-build` handler.
        hook_approval: Option<mc_script::precommit::Approval>,
        /// The encoded plugin receipt a server-owned structure portion journals
        /// beside its block after-images. Ordinary block edits carry none.
        plugin_receipt: Option<Vec<u8>>,
        /// One physical warehouse after-image published with this receipt.
        material_debit: Option<PreparedStructureMaterialDebit>,
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
    CommitMerchantTrade(Box<MerchantTradeCommand>),
    CommitSheepShear(SheepShearCommand),
    CommitZombieVillagerCure(ZombieVillagerCureCommand),
    CommitPlayerSurvival(Box<PlayerSurvivalCommand>),
    CommitPlayerPose {
        actor_session: SessionId,
        kind: PlayerPoseCommitKind,
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
    CommitThrowableItemRelease(ThrowableItemReleaseCommand),
    CommitBoatPlacement(BoatPlacementCommand),
    CommitChest {
        primary_position: BlockPos,
        positions: Vec<BlockPos>,
        expected_tokens: Option<Vec<BlockMutationToken>>,
        expected_state_id: i32,
        /// The acting session, or `None` for a server-owned deposit that acts
        /// for no player.
        actor_session: Option<SessionId>,
        expected: Vec<ChestBlockEntity>,
        updated: Vec<ChestBlockEntity>,
        /// The player participant, or `None` for a server-owned deposit whose
        /// second participant rides the plugin receipt.
        player: Option<Box<ContainerPlayerPlan>>,
        /// The encoded plugin operation receipt a server-owned deposit journals
        /// beside the container's after-image, or `None` for the menu path. A
        /// receipt makes the command server-owned: it drops the open-menu fence
        /// and publishes the container's slots to every viewer including the
        /// actor.
        plugin_receipt: Option<Vec<u8>>,
        treatment: Option<super::owned_inventory::PreparedTreatmentParticipant>,
    },
    CommitFurnace {
        position: BlockPos,
        expected_state_id: i32,
        actor_session: SessionId,
        expected: FurnaceBlockEntity,
        updated: Box<FurnaceBlockEntity>,
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
            Self::ResumePlayerDamagePrecommit(_) => "resume_player_damage_precommit",
            Self::ResumeEntityDamagePrecommit(_) => "resume_entity_damage_precommit",
            Self::BeginPrecommitSurvivalBreak(_) => "begin_precommit_survival_break",
            Self::ApplyServerEntityEffect(_) => "apply_server_entity_effect",
            Self::BeginPrecommitSurvivalPlacement(_) => "begin_precommit_survival_placement",
            Self::BeginPrecommitBucketUse(_) => "begin_precommit_bucket_use",
            Self::PrecommitSurvivalPlacementFailure(_) => "precommit_survival_placement_failure",
            Self::PrecommitBucketUseFailure(_) => "precommit_bucket_use_failure",
            #[cfg(test)]
            Self::AttackServerEntity { .. } => "attack_server_entity",
            Self::SpawnCommandEntity { .. } => "spawn_command_entity",
            Self::DamageScriptEntity { .. } => "damage_script_entity",
            Self::DamageResidentEntity { .. } => "damage_resident_entity",
            Self::SetWorldTime { .. } => "set_world_time",
            #[cfg(test)]
            Self::EnsureChunkHerd { .. } => "ensure_chunk_herd",
            Self::EnsureSettlementInhabitants { .. } => "ensure_settlement_inhabitants",
            Self::ApplyBlockEdits { .. } => "apply_block_edits",
            Self::CommitBlockDrops { .. } => "commit_block_drops",
            Self::ScheduleFluidTicksNearApplied { .. } => "schedule_fluid_ticks_near_applied",
            Self::CommitSurvivalBreak(_) => "commit_survival_break",
            Self::CommitSurvivalPlacement(_) => "commit_survival_placement",
            Self::CommitBucketUse(_) => "commit_bucket_use",
            Self::CommitFoodUse(_) => "commit_food_use",
            Self::CommitAnimalFeed(_) => "commit_animal_feed",
            Self::CommitMerchantTrade(_) => "commit_merchant_trade",
            Self::CommitSheepShear(_) => "commit_sheep_shear",
            Self::CommitZombieVillagerCure(_) => "commit_zombie_villager_cure",
            Self::CommitPlayerSurvival(_) => "commit_player_survival",
            Self::CommitPlayerPose { .. } => "commit_player_pose",
            Self::CommitPlayerStateEvent { .. } => "commit_player_state_event",
            Self::CommitPlayerInventory { .. } => "commit_player_inventory",
            Self::CommitThrowableItemRelease(_) => "commit_throwable_item_release",
            Self::CommitBoatPlacement(_) => "commit_boat_placement",
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

/// A construction material debit fenced against the exact container block
/// image that the simulation owner captured before it admitted the build.
#[derive(Debug, Clone)]
pub(in crate::play) struct PreparedStructureMaterialDebit {
    pub(in crate::play) debit: WarehouseStructureMaterialDebit,
    pub(in crate::play) precondition: mc_world::ResidentBlockPrecondition,
}

/// One complete server-owned block-edit submission prepared by a plugin.
struct ServerOwnedBlockEditSubmission {
    edits: Vec<BlockEdit>,
    expected_preconditions: Option<Vec<BlockEditPrecondition>>,
    leaf_trigger: bool,
    zone_fence: Option<crate::script::ZoneProtectionFence>,
    plugin_receipt: Option<Vec<u8>>,
    material_debit: Option<WarehouseStructureMaterialDebit>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PlayerPoseCommitRequest {
    pub(super) actor_session: SessionId,
    pub(super) kind: PlayerPoseCommitKind,
    pub(super) pose: PlayerPose,
    pub(super) exhaustion: f32,
}

fn regular_player_pose_command(command: &SimulationCommand) -> Option<PlayerPoseCommitRequest> {
    match command {
        SimulationCommand::CommitPlayerPose {
            actor_session,
            kind: PlayerPoseCommitKind::Movement,
            pose,
            exhaustion,
            script_teleport_completion: None,
        } => Some(PlayerPoseCommitRequest {
            actor_session: *actor_session,
            kind: PlayerPoseCommitKind::Movement,
            pose: *pose,
            exhaustion: *exhaustion,
        }),
        _ => None,
    }
}

#[derive(Debug)]
pub(in crate::play) enum SimulationResponse {
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
    /// A detached precommit continuation applied its own publications.
    DamagePrecommit,
    #[cfg(test)]
    EntityAttack(Option<Box<EntityAttackOutcome>>),
    ResidentDamage(Option<ResidentHit>),
    EntitySpawn(Vec<VisibilityDispatch>),
    ScriptEntityDamage(Option<ScriptEntityDamageCommit>),
    WorldTimeSet,
    BlockEdits(Result<Box<Option<BlockEditBatchOutcome>>, SimulationRequestError>),
    /// A server-owned structure portion whose world after-images and plugin
    /// receipt were accepted in one world-journal decision.
    SettlementPortion(Result<Option<u64>, SimulationRequestError>),
    BlockDrops(Result<Box<Option<BlockEditBatchOutcome>>, SimulationRequestError>),
    FluidTicksScheduled,
    SurvivalBreak(Result<Option<Box<CommittedSurvivalBreak>>, SimulationRequestError>),
    SurvivalPlacement(Result<Option<Box<CommittedSurvivalPlacement>>, SimulationRequestError>),
    BucketUse(Result<Option<Box<CommittedBucketUse>>, SimulationRequestError>),
    FoodUse(Result<Option<Box<CommittedFoodUse>>, SimulationRequestError>),
    AnimalFeed(Result<Option<Box<CommittedAnimalFeed>>, SimulationRequestError>),
    MerchantTrade(Result<Option<Box<CommittedMerchantTrade>>, SimulationRequestError>),
    SheepShear(Result<Option<Box<CommittedSheepShear>>, SimulationRequestError>),
    ZombieVillagerCure(Result<Option<Box<CommittedZombieVillagerCure>>, SimulationRequestError>),
    PlayerSurvival(Result<Option<Box<PlayerSurvivalCommitOutcome>>, SimulationRequestError>),
    PlayerPose(Result<CommittedPlayerPose, SimulationRequestError>),
    PlayerStateEvent(Result<(), SimulationRequestError>),
    PlayerInventory(Box<Result<PlayerInventoryCommitOutcome, SimulationRequestError>>),
    ThrowableItemRelease(
        Result<Option<Box<CommittedThrowableItemRelease>>, SimulationRequestError>,
    ),
    BoatPlacement(Result<Option<Box<CommittedBoatPlacement>>, SimulationRequestError>),
    BowRelease(Result<Option<Box<CommittedBowRelease>>, SimulationRequestError>),
    SelectedItemDrop(Result<Option<Box<CommittedSelectedItemDrop>>, SimulationRequestError>),
    ChestCommit(Result<Box<ChestCommitOutcome>, SimulationRequestError>),
    WarehouseTransfer(Result<WarehouseTransferOutcome, SimulationRequestError>),
    FurnaceCommit(Result<Box<FurnaceCommitOutcome>, SimulationRequestError>),
    OpaqueBlockEntity(Result<bool, SimulationRequestError>),
    CampfireUse(Result<Option<Box<CommittedCampfireUse>>, SimulationRequestError>),
    TntIgnition(Result<Option<Box<CommittedTntIgnition>>, SimulationRequestError>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScriptEntityDamageCommit {
    pub(crate) health: f32,
    pub(crate) killed: bool,
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
    actor_session: Option<SessionId>,
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

fn command_relight_actor_session(command: &SimulationCommand) -> Option<Option<SessionId>> {
    match command {
        SimulationCommand::ApplyBlockEdits { actor_session, .. } => Some(*actor_session),
        SimulationCommand::CommitSurvivalBreak(command) => Some(Some(command.actor_session)),
        SimulationCommand::CommitSurvivalPlacement(command) => Some(Some(command.actor_session)),
        SimulationCommand::CommitBucketUse(command) => Some(Some(command.actor_session)),
        SimulationCommand::CommitTntIgnition { actor_session, .. } => Some(Some(*actor_session)),
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
                .loaded_recipients_for_chunks(&light_chunks, pending.actor_session)
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
            | SimulationCommand::BeginPrecommitSurvivalBreak(_)
            | SimulationCommand::CommitSurvivalPlacement(_)
            | SimulationCommand::BeginPrecommitSurvivalPlacement(_)
            | SimulationCommand::BeginPrecommitBucketUse(_)
            | SimulationCommand::CommitBucketUse(_)
            | SimulationCommand::CommitChest { .. }
            | SimulationCommand::CommitFurnace { .. }
            | SimulationCommand::CommitOpaqueBlockEntity { .. }
            | SimulationCommand::CommitCampfireUse(_)
            | SimulationCommand::CommitTntIgnition { .. }
    )
}

/// Whether one command's own journal decision must carry a plugin receipt.
///
/// A server-owned container composite has no menu participant to fall back on:
/// its receipt and the container's after-image are durable only together, so it
/// is admitted to a journaled run or refused before any mutation.
fn command_needs_world_journal(command: &SimulationCommand) -> bool {
    matches!(
        command,
        SimulationCommand::ApplyBlockEdits {
            plugin_receipt: Some(_),
            ..
        } | SimulationCommand::CommitChest {
            plugin_receipt: Some(_),
            ..
        }
    )
}

fn command_is_background(command: &SimulationCommand) -> bool {
    #[cfg(test)]
    if matches!(command, SimulationCommand::EnsureChunkHerd { .. }) {
        return true;
    }
    matches!(
        command,
        SimulationCommand::EnsureSettlementInhabitants { .. }
    )
}

fn command_orders_earlier_herds(command: &SimulationCommand) -> bool {
    matches!(command, SimulationCommand::SetWorldTime { .. })
}

/// A server-owned block-edit batch has no writer session, so it never runs the
/// writer's `finalize_visible_block_edit_outcome` pass; drop the cooking state of
/// a campfire it replaced here. The batch's own delta replaces the block
/// client-side, so no block-entity reset is sent for this path.
fn clear_server_owned_campfire_cooking(
    sessions: &SessionRegistry,
    storage: Option<&mc_world::WorldStorage>,
    actor_session: Option<SessionId>,
    outcome: Option<&BlockEditBatchOutcome>,
) {
    let (Some(storage), Some(outcome)) = (storage, outcome) else {
        return;
    };
    if actor_session.is_some() {
        return;
    }
    for edit in &outcome.applied {
        if is_campfire_block(storage.registry(), edit.previous)
            && !is_campfire_block(storage.registry(), edit.new_state)
        {
            sessions.clear_campfire_cooking(edit.pos);
        }
    }
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
            SurvivalBreakRequest::PrecommitFailure(_) => return None,
        };
        let first = positions.next()?;
        let owner = RegionKey::from_chunk(first.x.div_euclid(16), first.z.div_euclid(16));
        return positions
            .all(|pos| RegionKey::from_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16)) == owner)
            .then_some(owner);
    }
    if let SimulationCommand::BeginPrecommitSurvivalBreak(command) = command {
        return Some(RegionKey::from_chunk(
            command.plan.position.x.div_euclid(16),
            command.plan.position.z.div_euclid(16),
        ));
    }
    if let SimulationCommand::BeginPrecommitSurvivalPlacement(command) = command {
        let position = command.plan.edits.first()?.pos;
        return Some(RegionKey::from_chunk(
            position.x.div_euclid(16),
            position.z.div_euclid(16),
        ));
    }
    if let SimulationCommand::BeginPrecommitBucketUse(command) = command {
        return Some(RegionKey::from_chunk(
            command.plan.edit.pos.x.div_euclid(16),
            command.plan.edit.pos.z.div_euclid(16),
        ));
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
        #[cfg(test)]
        SimulationCommand::EnsureChunkHerd { chunk, .. } => {
            return Some(RegionKey::from_chunk(chunk.0, chunk.1));
        }
        SimulationCommand::EnsureSettlementInhabitants { chunk, .. } => {
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
        actor_session,
        edits,
        preconditions,
        scheduled_block_ticks,
        zone_fence,
        hook_approval,
        plugin_receipt,
        ..
    } = command
    else {
        return false;
    };
    let journaled_server_portion = actor_session.is_none() && plugin_receipt.is_some();
    // Ordinary server-owned batches stay on the canonical path because only it
    // owns their campfire eviction. A receipt-bearing construction portion is
    // different: its regional worker rechecks its zone fence and spends its
    // build approval immediately before appending the shared world decision.
    if !journaled_server_portion
        && (actor_session.is_none() || zone_fence.is_some() || hook_approval.is_some())
    {
        return false;
    }
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
        let at_regional_edge = matches!(edit.pos.x.rem_euclid(8 * 16), 0 | 127)
            || matches!(edit.pos.z.rem_euclid(8 * 16), 0 | 127);
        if !seen.insert(edit.pos) || (!journaled_server_portion && at_regional_edge) {
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
            SurvivalBreakRequest::PrecommitFailure(_) => false,
        };
        let Some(region) = command_single_owner_region(command) else {
            return false;
        };
        let root = match &break_command.request {
            SurvivalBreakRequest::Prepared(plan) => plan.edits.first().map(|edit| edit.pos),
            SurvivalBreakRequest::Block(plan) => Some(plan.position),
            SurvivalBreakRequest::PrecommitFailure(_) => None,
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
        actor_session,
        player,
        plugin_receipt,
        ..
    } = command
    {
        let mut unique = HashSet::with_capacity(positions.len());
        // The menu path is session-authored: it has one acting session and the
        // plan that session's inventory moves with. A session and a player plan
        // travel together in every shape, so a command carrying one without the
        // other is refused rather than committed without the player fence.
        if actor_session.is_some() != player.is_some() {
            return false;
        }
        if plugin_receipt.is_none() && actor_session.is_none() {
            return false;
        }
        return !positions.is_empty()
            && positions.len() <= 2
            && positions.first() == Some(primary_position)
            && positions.len() == expected.len()
            && positions.len() == updated.len()
            && positions.iter().all(|position| unique.insert(*position))
            && player.as_deref().is_none_or(valid_container_player_plan)
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

/// Reactivity scheduled near every applied edit: leaf distance ticks (the
/// existing owner) plus the redstone components that must re-evaluate their
/// neighbourhood on the next tick.
fn schedule_reactivity_near_applied(
    storage: &mut WorldStorage,
    world_tick: u64,
    applied: &[AppliedBlockEdit],
) {
    schedule_leaf_ticks_near_applied(storage, world_tick, applied);
    super::scheduled_blocks::redstone::schedule_redstone_ticks_near_applied(
        storage, world_tick, applied,
    );
}

pub(super) fn resident_block_edit_outcome(
    mutation: &WorldMutationView,
    block_light: Option<&BlockLightTable>,
    world_tick: u64,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> Option<BlockEditBatchOutcome> {
    let resident_edits = resident_block_edits(edits);
    let resident_preconditions = resident_block_preconditions(preconditions);
    resident_block_edit_result_outcome(mutation.apply_block_edits_conditionally(
        &resident_edits,
        &resident_preconditions,
        scheduled_block_ticks,
        block_light,
        Some(world_tick.saturating_add(1)),
    ))
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
    actor_session: Option<SessionId>,
    outcome: &BlockEditBatchOutcome,
) {
    sessions.invalidate_prepared_chunks(&outcome.edit_chunks);
    let mut dispatches = sessions
        .loaded_recipients_for_chunks(&outcome.edit_chunks, actor_session)
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
                .loaded_recipients_for_chunks(&light_chunks, actor_session)
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
    expected_tokens: Option<&'a [BlockMutationToken]>,
    expected_state_id: i32,
    actor_session: Option<SessionId>,
    expected: &'a [ChestBlockEntity],
    updated: &'a [ChestBlockEntity],
    /// The player participant, present for the menu path and for a deposit that
    /// moves a player's own inventory.
    player: Option<&'a ContainerPlayerPlan>,
    /// Present when the envelope is a server-owned composite; the menu path
    /// never reaches it, because that mode only travels through a journaled
    /// regional run.
    plugin_receipt: Option<&'a [u8]>,
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
    pub(super) loader_block_drop: Option<ItemStack>,
    pub(super) held: SurvivalBreakHeldItem,
    pub(super) drop_items: bool,
    /// The chain's ticket for this edit, spent at the commit. `None` is the
    /// ordinary path: this deployment registers no `before-build` handler.
    pub(super) hook_approval: Option<mc_script::precommit::Approval>,
    /// The player-zone definition image that admitted this break.
    pub(super) zone_fence: Option<crate::script::ZoneProtectionFence>,
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
            .field("loader_block_drop", &self.loader_block_drop)
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
    /// The player-zone definition image carried from the original break request.
    pub(super) zone_fence: Option<crate::script::ZoneProtectionFence>,
    /// The chain's ticket for this batch, spent at the commit.
    pub(super) hook_approval: Option<mc_script::precommit::Approval>,
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

pub(super) struct HookedSurvivalBreakCommand {
    actor_session: SessionId,
    plan: SurvivalBlockBreakPlan,
    boundary: mc_script::ScriptBoundary,
    resume_handle: SimulationHandle,
}

impl std::fmt::Debug for HookedSurvivalBreakCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HookedSurvivalBreakCommand")
            .field("actor_session", &self.actor_session)
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(super) struct HookedSurvivalPlacementCommand {
    actor_session: SessionId,
    plan: SurvivalPlacementPlan,
    boundary: mc_script::ScriptBoundary,
    resume_handle: SimulationHandle,
}

#[derive(Debug)]
pub(super) struct HookedBucketUseCommand {
    actor_session: SessionId,
    plan: BucketUsePlan,
    boundary: mc_script::ScriptBoundary,
    resume_handle: SimulationHandle,
}

#[derive(Debug, Clone)]
enum SurvivalBreakRequest {
    Prepared(SurvivalBreakPlan),
    Block(SurvivalBlockBreakPlan),
    PrecommitFailure(mc_script::precommit::HookFailure),
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

/// Support cascade for one explosion-destroyed cell: ground plants and
/// columns above it pop under the same transaction. Edits already present
/// (the blast destroys the plant itself) and unreadable cells are skipped
/// rather than failing the blast.
fn plan_explosion_support_cascade(
    blocks: &mc_world::BlockRegistry,
    storage: &impl super::BlockPlanningRead,
    existing: &[super::BlockEdit],
    base: mc_world::BlockPos,
    air: mc_world::BlockStateId,
) -> Vec<(super::BlockEdit, super::BlockEditPrecondition)> {
    let mut cascade = Vec::new();
    super::block_break::append_vertical_support_cascade(blocks, storage, &mut cascade, base, air);
    let mut out: Vec<(super::BlockEdit, super::BlockEditPrecondition)> =
        Vec::with_capacity(cascade.len());
    for edit in cascade {
        if existing.iter().any(|existing| existing.pos == edit.pos)
            || out.iter().any(|(emitted, _)| emitted.pos == edit.pos)
        {
            continue;
        }
        let Some(expected_state) = storage.get_cached_block(edit.pos) else {
            continue;
        };
        let Some(expected_token) = storage.block_mutation_token(edit.pos) else {
            continue;
        };
        out.push((
            edit,
            super::BlockEditPrecondition {
                pos: edit.pos,
                expected_state,
                expected_token,
            },
        ));
    }
    out
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
    /// The chain's ticket for this placement, spent at the commit. `None` is the
    /// ordinary path: this deployment registers no `before-build` handler.
    pub(super) hook_approval: Option<mc_script::precommit::Approval>,
    /// The player-zone definition image that admitted this placement.
    pub(super) zone_fence: Option<crate::script::ZoneProtectionFence>,
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
    /// The chain's ticket for this edit, spent at the commit.
    pub(super) hook_approval: Option<mc_script::precommit::Approval>,
    /// The player-zone definition image that admitted this bucket edit.
    pub(super) zone_fence: Option<crate::script::ZoneProtectionFence>,
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
    pub(super) can_always_eat: bool,
    pub(super) remainder: Option<FoodUseRemainder>,
}

#[derive(Debug, Clone)]
pub(super) struct FoodUseRemainder {
    pub(super) stack: ItemStack,
    pub(super) max_stack: i32,
    pub(super) entity_type_id: Option<i32>,
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
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AnimalFeedTargets {
    pub(crate) cow: bool,
    pub(crate) sheep: bool,
    pub(crate) chicken: bool,
}

impl AnimalFeedTargets {
    pub(crate) fn is_empty(self) -> bool {
        !self.cow && !self.sheep && !self.chicken
    }

    pub(crate) fn accepts(self, entity_type: &str) -> bool {
        match entity_type {
            "minecraft:cow" => self.cow,
            "minecraft:sheep" => self.sheep,
            "minecraft:chicken" => self.chicken,
            _ => false,
        }
    }

    /// Resolve the vanilla food tags shared by interaction, temptation and
    /// resident provisioning.
    pub(crate) fn from_tags(tags: &TagsData, item_id: u32) -> Self {
        let Ok(item_id) = i32::try_from(item_id) else {
            return Self::default();
        };
        let item_registry = Identifier::parse("minecraft:item").expect("static item registry id");
        let Some(item_tags) = tags.registries.get(&item_registry) else {
            return Self::default();
        };
        let contains = |tag_name: &str| {
            Identifier::parse(tag_name)
                .ok()
                .and_then(|tag| item_tags.get(&tag))
                .is_some_and(|entries| entries.contains(&item_id))
        };
        Self {
            cow: contains("minecraft:cow_food"),
            sheep: contains("minecraft:sheep_food"),
            chicken: contains("minecraft:chicken_food"),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MerchantTradeDestination {
    Cursor,
    Inventory,
}

#[derive(Debug, Clone)]
pub(super) struct MerchantTradePlan {
    pub(super) entity_id: EntityId,
    pub(super) expected_merchant: mc_entity::villager_merchant_26_1_2::VillagerMerchantState,
    pub(super) expected_gossip: mc_entity::villager_gossip_26_1_2::VillagerGossipState,
    pub(super) offer_index: usize,
    pub(super) expected_inventory: PlayerInventory,
    pub(super) expected_carried_item: ItemStack,
    pub(super) expected_merchant_input: Option<Box<[ItemStack; 2]>>,
    pub(super) destination: MerchantTradeDestination,
    pub(super) cost_a_max_stack: i32,
    pub(super) result_max_stack: i32,
}

#[derive(Debug)]
pub(super) struct MerchantTradeCommand {
    actor_session: SessionId,
    plan: MerchantTradePlan,
}

#[derive(Debug)]
pub(super) struct CommittedMerchantTrade {
    pub(super) inventory: PlayerInventory,
    pub(super) carried_item: ItemStack,
    pub(super) merchant_input: Option<Box<[ItemStack; 2]>>,
    pub(super) merchant: mc_entity::villager_merchant_26_1_2::VillagerMerchantState,
    pub(super) gossip: mc_entity::villager_gossip_26_1_2::VillagerGossipState,
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

#[derive(Debug, Clone)]
pub(super) struct ZombieVillagerCurePlan {
    pub(super) entity_id: EntityId,
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) golden_apple_item_id: u32,
}

#[derive(Debug)]
pub(super) struct ZombieVillagerCureCommand {
    actor_session: SessionId,
    plan: ZombieVillagerCurePlan,
}

#[derive(Debug)]
pub(super) struct CommittedZombieVillagerCure {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
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
    pub(super) keep_inventory: bool,
    pub(super) position: Vec3,
    /// The chain's ticket for the effect this plan commits. Damage owners set it
    /// when a `before-damage` handler is registered and every other plan uses
    /// `None`, which is the ordinary path.
    pub(super) hook_approval: Option<mc_script::precommit::Approval>,
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
    Committed(Box<CommittedPlayerSurvival>),
    Rejected(Box<AuthoritativePlayerStateSnapshot>),
}

#[derive(Debug)]
pub(super) struct PlayerSurvivalCommand {
    actor_session: SessionId,
    plan: Box<PlayerSurvivalPlan>,
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
#[derive(Debug, Clone)]
pub(super) struct ThrowableItemReleasePlan {
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) game_mode: GameMode,
    pub(super) entity_type_id: i32,
    pub(super) entity_type_name: String,
    pub(super) position: Vec3,
    pub(super) velocity: Vec3,
    pub(super) rotation: Rotation,
}

#[derive(Debug)]
pub(super) struct ThrowableItemReleaseCommand {
    actor_session: SessionId,
    plan: ThrowableItemReleasePlan,
}

#[derive(Debug)]
pub(super) struct CommittedThrowableItemRelease {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[derive(Debug, Clone)]
pub(super) struct BoatPlacementPlan {
    pub(super) held_slot: usize,
    pub(super) expected_held: ItemStack,
    pub(super) game_mode: GameMode,
    pub(super) entity_type_id: i32,
    pub(super) entity_type_name: String,
    pub(super) position: Vec3,
    pub(super) rotation: Rotation,
}

#[derive(Debug)]
pub(super) struct BoatPlacementCommand {
    actor_session: SessionId,
    plan: BoatPlacementPlan,
}

#[derive(Debug)]
pub(super) struct CommittedBoatPlacement {
    pub(super) inventory: PlayerInventory,
    pub(super) changed_slots: Vec<(usize, ItemStack)>,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}
type SimulationOutcome = Result<SimulationResponse, SimulationRequestError>;

#[derive(Debug, Clone)]
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
            SimulationRequestError::Full | SimulationRequestError::QueueAdmissionTimeout => {
                Self::Busy
            }
            SimulationRequestError::ShuttingDown => Self::ShuttingDown,
            SimulationRequestError::ResponseMismatch => Self::ResponseMismatch,
            SimulationRequestError::Closed
            | SimulationRequestError::OwnerStopped
            | SimulationRequestError::ResponseTimeout
            | SimulationRequestError::WorldUnavailable
            | SimulationRequestError::WorldMutationFailed
            | SimulationRequestError::CrossRegion
            | SimulationRequestError::InvalidCommand
            | SimulationRequestError::Precommit(_)
            | SimulationRequestError::StaleSession
            | SimulationRequestError::PlayerMovementRejected(_) => Self::Unavailable,
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
    /// The build chain this world asks before it commits, shared by every clone
    /// of one handle: installed once from the deployment's own boundary, so a
    /// programmatic or settlement build asks the same chain a player's build
    /// does instead of having a path of its own around it.
    precommit_boundary: Arc<std::sync::OnceLock<mc_script::ScriptBoundary>>,
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
        let receiver = self.enqueue_with_fence(
            self.session_fence,
            SimulationCommand::ReadBlockSnapshot { position },
        )?;
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

    pub(crate) async fn damage_script_entity(
        &self,
        entity_id: EntityId,
        damage: f32,
        plugin_id: &str,
    ) -> Result<Option<ScriptEntityDamageCommit>, SimulationRequestError> {
        if self.session_fence.is_some()
            || !damage.is_finite()
            || damage <= 0.0
            || plugin_id.is_empty()
        {
            return Err(SimulationRequestError::InvalidCommand);
        }
        let receiver = self.enqueue_with_fence(
            None,
            SimulationCommand::DamageScriptEntity {
                entity_id,
                damage,
                plugin_id: plugin_id.to_owned(),
            },
        )?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ScriptEntityDamage(result))) => Ok(result),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(crate) async fn damage_resident_entity(
        &self,
        plugin_id: &str,
        attack: ResidentAttack,
    ) -> Result<Option<ResidentHit>, SimulationRequestError> {
        if self.session_fence.is_some() || plugin_id.is_empty() {
            return Err(SimulationRequestError::InvalidCommand);
        }
        let receiver = self.enqueue_with_fence(
            None,
            SimulationCommand::DamageResidentEntity {
                attack: Box::new(attack),
                plugin_id: plugin_id.to_owned(),
            },
        )?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ResidentDamage(result))) => Ok(result),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
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
        let receiver = self.enqueue_with_fence(
            self.session_fence,
            SimulationCommand::ApplyBlockEdits {
                actor_session: self.session_fence,
                edits,
                preconditions,
                scheduled_block_ticks,
                material_debit: None,
                leaf_trigger: true,
                hook_approval: None,
                plugin_receipt: None,
                zone_fence: None,
            },
        )?;
        match receiver.await {
            Ok(Ok(SimulationResponse::BlockEdits(Ok(outcome)))) => Ok(*outcome),
            Ok(Ok(SimulationResponse::BlockEdits(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    /// Commit one server-owned block-edit batch (settlement structure
    /// placement) through the authoritative simulation pipeline.
    ///
    /// If a build chain is active, capture every authoritative precondition
    /// before asking it and conditionally commit that exact image afterwards.
    pub(crate) async fn apply_server_owned_block_edits(
        &self,
        plugin_id: &str,
        edits: Vec<BlockEdit>,
        zone_fence: Option<crate::script::ZoneProtectionFence>,
    ) -> Result<Option<BlockEditBatchOutcome>, SimulationRequestError> {
        match self
            .submit_server_owned_block_edits(
                plugin_id,
                ServerOwnedBlockEditSubmission {
                    edits,
                    expected_preconditions: None,
                    leaf_trigger: true,
                    zone_fence,
                    plugin_receipt: None,
                    material_debit: None,
                },
            )
            .await?
        {
            SimulationResponse::BlockEdits(Ok(outcome)) => Ok(*outcome),
            SimulationResponse::BlockEdits(Err(error)) => Err(error),
            _ => Err(SimulationRequestError::ResponseMismatch),
        }
    }

    /// Commit one server-owned block batch and plugin receipt under one
    /// world-journal decision. The owner captures the exact source image before
    /// admission.
    #[cfg(test)]
    pub(crate) async fn commit_server_owned_block_edits(
        &self,
        plugin_id: &str,
        edits: Vec<BlockEdit>,
        zone_fence: Option<crate::script::ZoneProtectionFence>,
        receipt: Vec<u8>,
    ) -> Result<Option<u64>, SimulationRequestError> {
        self.commit_server_owned_block_edits_inner(
            plugin_id,
            ServerOwnedBlockEditSubmission {
                edits,
                expected_preconditions: None,
                leaf_trigger: true,
                zone_fence,
                plugin_receipt: Some(receipt),
                material_debit: None,
            },
        )
        .await
    }

    /// Commit one structure portion and the exact physical warehouse material
    /// debit under the receipt's one world-journal decision.
    pub(crate) async fn commit_server_owned_structure_portion(
        &self,
        plugin_id: &str,
        edits: Vec<BlockEdit>,
        zone_fence: Option<crate::script::ZoneProtectionFence>,
        receipt: Vec<u8>,
        material_debit: WarehouseStructureMaterialDebit,
    ) -> Result<Option<u64>, SimulationRequestError> {
        self.commit_server_owned_block_edits_inner(
            plugin_id,
            ServerOwnedBlockEditSubmission {
                edits,
                expected_preconditions: None,
                leaf_trigger: true,
                zone_fence,
                plugin_receipt: Some(receipt),
                material_debit: Some(material_debit),
            },
        )
        .await
    }

    /// Commit a server-owned block batch against source images the caller
    /// previewed. This keeps the world mutation attached to that previewed loot,
    /// rather than recapturing a replacement block as a new harvest.
    pub(crate) async fn commit_server_owned_block_edits_with_preconditions(
        &self,
        plugin_id: &str,
        edits: Vec<BlockEdit>,
        preconditions: Option<Vec<BlockEditPrecondition>>,
        leaf_trigger: bool,
        zone_fence: Option<crate::script::ZoneProtectionFence>,
        receipt: Vec<u8>,
    ) -> Result<Option<u64>, SimulationRequestError> {
        self.commit_server_owned_block_edits_inner(
            plugin_id,
            ServerOwnedBlockEditSubmission {
                edits,
                expected_preconditions: preconditions,
                leaf_trigger,
                zone_fence,
                plugin_receipt: Some(receipt),
                material_debit: None,
            },
        )
        .await
    }

    async fn commit_server_owned_block_edits_inner(
        &self,
        plugin_id: &str,
        submission: ServerOwnedBlockEditSubmission,
    ) -> Result<Option<u64>, SimulationRequestError> {
        match self
            .submit_server_owned_block_edits(plugin_id, submission)
            .await?
        {
            SimulationResponse::SettlementPortion(outcome) => outcome,
            _ => Err(SimulationRequestError::ResponseMismatch),
        }
    }

    async fn submit_server_owned_block_edits(
        &self,
        plugin_id: &str,
        submission: ServerOwnedBlockEditSubmission,
    ) -> Result<SimulationResponse, SimulationRequestError> {
        let ServerOwnedBlockEditSubmission {
            edits,
            expected_preconditions,
            leaf_trigger,
            zone_fence,
            plugin_receipt,
            material_debit,
        } = submission;
        if self.session_fence.is_some() {
            return Err(SimulationRequestError::InvalidCommand);
        }
        let has_build_hooks = self.precommit_boundary().is_some_and(|boundary| {
            boundary.has_precommit_hooks(mc_script::precommit::HookKind::Build)
        });
        let (preconditions, hook_approval) = if plugin_receipt.is_some() || has_build_hooks {
            let preconditions = match expected_preconditions {
                Some(preconditions) => preconditions,
                None => self.server_owned_block_preconditions(&edits).await?,
            };
            let approval = if has_build_hooks {
                self.request_build_hook(
                    mc_script::precommit::HookActor::Plugin(plugin_id.to_owned()),
                    &edits,
                    &preconditions,
                )
                .await
                .map_err(SimulationRequestError::Precommit)?
            } else {
                None
            };
            (preconditions, approval)
        } else {
            (Vec::new(), None)
        };
        let material_debit = if let Some(debit) = material_debit {
            let Some(snapshot) = self.read_block_snapshot(debit.position).await? else {
                return Err(SimulationRequestError::Precommit(
                    mc_script::precommit::HookFailure::Stale,
                ));
            };
            Some(PreparedStructureMaterialDebit {
                precondition: mc_world::ResidentBlockPrecondition {
                    pos: debit.position,
                    expected_state: snapshot.state,
                    expected_token: snapshot.token,
                },
                debit,
            })
        } else {
            None
        };
        let receiver = self.enqueue_with_fence(
            None,
            SimulationCommand::ApplyBlockEdits {
                actor_session: None,
                edits,
                preconditions,
                scheduled_block_ticks: Vec::new(),
                leaf_trigger,
                hook_approval,
                material_debit,
                zone_fence,
                plugin_receipt,
            },
        )?;
        match receiver.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    async fn server_owned_block_preconditions(
        &self,
        edits: &[BlockEdit],
    ) -> Result<Vec<BlockEditPrecondition>, SimulationRequestError> {
        let mut preconditions = Vec::with_capacity(edits.len());
        for edit in edits {
            let Some(snapshot) = self.read_block_snapshot(edit.pos).await? else {
                return Err(SimulationRequestError::Precommit(
                    mc_script::precommit::HookFailure::Stale,
                ));
            };
            preconditions.push(BlockEditPrecondition {
                pos: edit.pos,
                expected_state: snapshot.state,
                expected_token: snapshot.token,
            });
        }
        Ok(preconditions)
    }

    pub(crate) async fn place_loader_block_server_owned(
        &self,
        plugin_id: &str,
        position: BlockPos,
        state: BlockStateId,
        zone_fence: Option<crate::script::ZoneProtectionFence>,
    ) -> Result<bool, SimulationRequestError> {
        self.apply_server_owned_block_edits(
            plugin_id,
            vec![BlockEdit {
                pos: position,
                new_state: state,
            }],
            zone_fence,
        )
        .await
        .map(|outcome| outcome.is_some())
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
        let command = if let Some(boundary) = self
            .precommit_boundary()
            .filter(|boundary| boundary.has_precommit_hooks(mc_script::precommit::HookKind::Build))
        {
            SimulationCommand::BeginPrecommitSurvivalBreak(Box::new(HookedSurvivalBreakCommand {
                actor_session,
                plan,
                boundary: boundary.clone(),
                resume_handle: self.clone(),
            }))
        } else {
            SimulationCommand::CommitSurvivalBreak(Box::new(SurvivalBreakCommand {
                actor_session,
                request: SurvivalBreakRequest::Block(plan),
            }))
        };
        let receiver = self.enqueue_player_command(command)?;
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
        let command = if let Some(boundary) = self
            .precommit_boundary()
            .filter(|boundary| boundary.has_precommit_hooks(mc_script::precommit::HookKind::Build))
        {
            SimulationCommand::BeginPrecommitSurvivalPlacement(Box::new(
                HookedSurvivalPlacementCommand {
                    actor_session,
                    plan,
                    boundary: boundary.clone(),
                    resume_handle: self.clone(),
                },
            ))
        } else {
            SimulationCommand::CommitSurvivalPlacement(Box::new(SurvivalPlacementCommand {
                actor_session,
                plan,
            }))
        };
        let receiver = self.enqueue_player_command(command)?;
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
        let command = if let Some(boundary) = self
            .precommit_boundary()
            .filter(|boundary| boundary.has_precommit_hooks(mc_script::precommit::HookKind::Build))
        {
            SimulationCommand::BeginPrecommitBucketUse(Box::new(HookedBucketUseCommand {
                actor_session,
                plan,
                boundary: boundary.clone(),
                resume_handle: self.clone(),
            }))
        } else {
            SimulationCommand::CommitBucketUse(Box::new(BucketUseCommand {
                actor_session,
                plan,
            }))
        };
        let receiver = self.enqueue_player_command(command)?;
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

    pub(super) async fn commit_merchant_trade(
        &self,
        plan: MerchantTradePlan,
    ) -> Result<Option<CommittedMerchantTrade>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitMerchantTrade(
            Box::new(MerchantTradeCommand {
                actor_session,
                plan,
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::MerchantTrade(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::MerchantTrade(Err(error)))) => Err(error),
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

    pub(super) async fn commit_zombie_villager_cure(
        &self,
        plan: ZombieVillagerCurePlan,
    ) -> Result<Option<CommittedZombieVillagerCure>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitZombieVillagerCure(
            ZombieVillagerCureCommand {
                actor_session,
                plan,
            },
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ZombieVillagerCure(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::ZombieVillagerCure(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_player_survival(
        &self,
        plan: Box<PlayerSurvivalPlan>,
    ) -> Result<Option<Box<PlayerSurvivalCommitOutcome>>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitPlayerSurvival(
            Box::new(PlayerSurvivalCommand {
                actor_session,
                plan,
            }),
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::PlayerSurvival(Ok(committed)))) => Ok(committed),
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
        self.commit_player_pose_with_kind(PlayerPoseCommitKind::Movement, pose, exhaustion)
            .await
    }

    pub(super) async fn commit_player_teleport(
        &self,
        pose: super::PlayerPose,
    ) -> Result<CommittedPlayerPose, SimulationRequestError> {
        self.commit_player_pose_with_kind(PlayerPoseCommitKind::Teleport, pose, 0.0)
            .await
    }

    async fn commit_player_pose_with_kind(
        &self,
        kind: PlayerPoseCommitKind,
        pose: super::PlayerPose,
        exhaustion: f32,
    ) -> Result<CommittedPlayerPose, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self
            .enqueue_player_command_wait(SimulationCommand::CommitPlayerPose {
                actor_session,
                kind,
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

    pub(super) async fn commit_throwable_item_release(
        &self,
        plan: ThrowableItemReleasePlan,
    ) -> Result<Option<CommittedThrowableItemRelease>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(
            SimulationCommand::CommitThrowableItemRelease(ThrowableItemReleaseCommand {
                actor_session,
                plan,
            }),
        )?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ThrowableItemRelease(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::ThrowableItemRelease(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    pub(super) async fn commit_boat_placement(
        &self,
        plan: BoatPlacementPlan,
    ) -> Result<Option<CommittedBoatPlacement>, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitBoatPlacement(
            BoatPlacementCommand {
                actor_session,
                plan,
            },
        ))?;
        match receiver.await {
            Ok(Ok(SimulationResponse::BoatPlacement(Ok(committed)))) => {
                Ok(committed.map(|committed| *committed))
            }
            Ok(Ok(SimulationResponse::BoatPlacement(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the chest snapshot and player plan must remain explicit at this test boundary"
    )]
    pub(super) async fn commit_chest(
        &self,
        primary_position: BlockPos,
        positions: Vec<BlockPos>,
        expected_tokens: Vec<BlockMutationToken>,
        expected_state_id: i32,
        expected: Vec<ChestBlockEntity>,
        updated: Vec<ChestBlockEntity>,
        player: ContainerPlayerPlan,
    ) -> Result<ChestCommitOutcome, SimulationRequestError> {
        let actor_session = self.session_id()?;
        let receiver = self.enqueue_player_command(SimulationCommand::CommitChest {
            primary_position,
            positions,
            expected_tokens: Some(expected_tokens),
            expected_state_id,
            actor_session: Some(actor_session),
            expected,
            updated,
            player: Some(Box::new(player)),
            plugin_receipt: None,
            treatment: None,
        })?;
        match receiver.await {
            Ok(Ok(SimulationResponse::ChestCommit(Ok(outcome)))) => Ok(*outcome),
            Ok(Ok(SimulationResponse::ChestCommit(Err(error)))) => Err(error),
            Ok(Ok(_)) => Err(SimulationRequestError::ResponseMismatch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(SimulationRequestError::OwnerStopped),
        }
    }

    /// Commit one server-owned warehouse deposit: the container's canonical
    /// slots and - when a player participates - that player's inventory move
    /// together, and the container's after-image and the caller's encoded
    /// plugin receipt ride the same world journal decision.
    ///
    /// A deposit with no player participant (a worker's cargo, whose other
    /// participant is a plugin-owned record inside the receipt) is enqueued
    /// with no session fence, exactly like every other server-owned command;
    /// one that moves a player's inventory is enqueued under that player's
    /// session fence. The owner turn owns the fences, the journal append and
    /// the publication in both cases.
    pub(crate) async fn commit_warehouse_transfer(
        &self,
        request: WarehouseTransferRequest,
    ) -> Result<WarehouseTransferOutcome, SimulationRequestError> {
        let WarehouseTransferRequest {
            position,
            expected_state_id,
            expected_container,
            updated_container,
            player,
            receipt,
            treatment,
        } = request;
        let Some(expected) = container_chest_image(&expected_container) else {
            return Err(SimulationRequestError::InvalidCommand);
        };
        let Some(updated) = container_chest_image(&updated_container) else {
            return Err(SimulationRequestError::InvalidCommand);
        };
        let receiver = match player {
            Some(participant) => {
                let (Ok(expected_inventory), Ok(updated_inventory)) = (
                    <[ItemStack; 46]>::try_from(participant.expected_inventory),
                    <[ItemStack; 46]>::try_from(participant.updated_inventory),
                ) else {
                    return Err(SimulationRequestError::InvalidCommand);
                };
                let plan = ContainerPlayerPlan {
                    expected_inventory: PlayerInventory {
                        slots: expected_inventory,
                    },
                    expected_carried_item: participant.expected_carried_item,
                    updated_inventory: PlayerInventory {
                        slots: updated_inventory,
                    },
                    updated_carried_item: participant.updated_carried_item,
                    crafting_table_input: None,
                    enchanting_table_input: None,
                    merchant_input: None,
                    drops: Vec::new(),
                    xp_orb: None,
                };
                self.for_session(participant.actor_id)
                    .enqueue_player_command(SimulationCommand::CommitChest {
                        primary_position: position,
                        positions: vec![position],
                        expected_tokens: None,
                        expected_state_id,
                        actor_session: Some(participant.actor_id),
                        expected: vec![expected],
                        updated: vec![updated],
                        player: Some(Box::new(plan)),
                        plugin_receipt: Some(receipt),
                        treatment: treatment.clone(),
                    })?
            }
            None => {
                if self.session_fence.is_some() {
                    return Err(SimulationRequestError::InvalidCommand);
                }
                self.enqueue_with_fence(
                    None,
                    SimulationCommand::CommitChest {
                        primary_position: position,
                        positions: vec![position],
                        expected_tokens: None,
                        expected_state_id,
                        actor_session: None,
                        expected: vec![expected],
                        updated: vec![updated],
                        player: None,
                        plugin_receipt: Some(receipt),
                        treatment,
                    },
                )?
            }
        };
        match receiver.await {
            Ok(Ok(SimulationResponse::WarehouseTransfer(Ok(outcome)))) => Ok(outcome),
            Ok(Ok(SimulationResponse::WarehouseTransfer(Err(error)))) => Err(error),
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
            updated: Box::new(updated),
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

#[derive(Clone, Copy)]
pub(crate) struct ExplosionRegistries<'a> {
    blocks: &'a BlockRegistry,
    items: &'a mc_data::items::ItemRegistry,
    entity_types: &'a mc_data::entity_types::EntityTypeRegistry,
}

impl<'a> ExplosionRegistries<'a> {
    pub(crate) fn from_config(config: &'a crate::server::ServerConfig) -> Self {
        Self {
            blocks: &config.blocks,
            items: &config.items,
            entity_types: &config.entity_types,
        }
    }

    #[cfg(test)]
    fn new(
        blocks: &'a BlockRegistry,
        items: &'a mc_data::items::ItemRegistry,
        entity_types: &'a mc_data::entity_types::EntityTypeRegistry,
    ) -> Self {
        Self {
            blocks,
            items,
            entity_types,
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
    player_movement_authority: Option<PlayerMovementAuthorityResources>,
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

/// A contiguous execution run with identical routing predicates.
///
/// Grouping is purely an execution optimization; the predicates preserve their
/// existing branch precedence and command order.
struct SimulationExecutionRun {
    requires_world: bool,
    regional_block_edit: bool,
    journaled_block_edit: bool,
    block_drop_command: bool,
    cross_region_plugin_edit: bool,
    envelopes: Vec<SimulationCommandEnvelope>,
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
    pub(crate) fn configure_player_movement_authority(
        &mut self,
        world_read: WorldReadView,
        blocks: Arc<BlockRegistry>,
        block_facts: Arc<BlockFactsTable>,
    ) {
        self.player_movement_authority = Some(PlayerMovementAuthorityResources::new(
            world_read,
            blocks,
            block_facts,
        ));
    }

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
        #[cfg(test)]
        self.release_retryable_herd_requests(pending.retryable_chunks());
        dispatch_visibility_commands(pending.into_dispatches());
        sessions.tick_weather(ticks);
        dispatch_visibility_commands(
            sessions
                .item_pickup_ready_dispatches_owned(&self.authority, sessions.simulation_tick()),
        );
        dispatch_visibility_commands(sessions.tick_sleep_owned(&self.authority));
        dispatch_visibility_commands(
            sessions.tick_player_effects_owned(&self.authority, sessions.simulation_tick()),
        );
        sessions.world_time()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn tick_primed_tnt<F>(
        &mut self,
        sessions: &SessionRegistry,
        world: Option<&WorldHandle>,
        block_light: Option<&BlockLightTable>,
        block_facts: &BlockFactsTable,
        registries: ExplosionRegistries<'_>,
        materials: Option<&BlockMaterialIds>,
        zone_protection: F,
    ) -> usize
    where
        F: FnOnce() -> Option<crate::script::ZoneProtectionSnapshot>,
    {
        let blocks = registries.blocks;
        let items = registries.items;
        let entity_types = registries.entity_types;
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
        let zone_protection = zone_protection();

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
        let explosion_item_entity_type_id = entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:item").expect("static item entity id"))
            .and_then(|id| i32::try_from(id).ok());
        let chained_tnt_entity_type_id = entity_types
            .id_of(&mc_data::Identifier::parse(TNT_ENTITY_TYPE_NAME).expect("static TNT entity id"))
            .and_then(|id| i32::try_from(id).ok());
        if let Some(world) = world {
            let mut storage = world.lock().await;
            for expired in &expired_tnt {
                let entity_id = expired.entity_id;
                let air = expired.air;
                let center = expired.center();
                let power = expired.power();
                let candidates = if expired.destroys_blocks() {
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
                                explodable: zone_protection.as_ref().is_none_or(|protection| {
                                    protection.ambient_block_mutation_allowed(
                                        "minecraft:overworld",
                                        position,
                                    )
                                }),
                            })
                        },
                    ) else {
                        continue;
                    };
                    candidates
                } else {
                    HashSet::new()
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
                                    .or_else(|| (!expired.destroys_blocks()).then(Vec::new))
                            })
                            .map(|mut impact| {
                                if !expired.damages_entities() {
                                    impact.damage = 0.0;
                                }
                                (target.session_id, impact)
                            })
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
                                        .or_else(|| (!expired.destroys_blocks()).then(Vec::new))
                                },
                            )
                            .map(|impact: EntityExplosionImpact| {
                                ServerEntityExplosionImpact {
                                    entity_id: target.entity_id,
                                    damage: if expired.damages_entities() {
                                        impact.damage
                                    } else {
                                        0.0
                                    },
                                    knockback: impact.knockback,
                                }
                            })
                        })
                        .collect();
                    entity_impacts.insert(entity_id, impacts);
                } else if !expired.destroys_blocks() {
                    let impacts = expired
                        .explosion_targets()
                        .iter()
                        .filter_map(|target| {
                            let feet = Vec3::new(target.pose.x, target.pose.y, target.pose.z);
                            plan_player_explosion_impact(center, power, feet, |_| Some(Vec::new()))
                                .map(|mut impact| {
                                    if !expired.damages_entities() {
                                        impact.damage = 0.0;
                                    }
                                    (target.session_id, impact)
                                })
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
                                |_| Some(Vec::new()),
                            )
                            .map(|impact: EntityExplosionImpact| {
                                ServerEntityExplosionImpact {
                                    entity_id: target.entity_id,
                                    damage: if expired.damages_entities() {
                                        impact.damage
                                    } else {
                                        0.0
                                    },
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
                    // Plants and columns above a destroyed support pop with
                    // it under the same transaction. Cells that cannot be
                    // read are left alone rather than failing the blast.
                    for (edit, precondition) in
                        plan_explosion_support_cascade(blocks, &*storage, &edits, position, air)
                    {
                        edits.push(edit);
                        preconditions.push(precondition);
                    }
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
                            let Some(item_id) = items.id_of(&drop.item) else {
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

            let drops = explosion_drops.remove(&entity_id).unwrap_or_default();
            let drop_dispatches = sessions.spawn_item_drop_batch_owned(
                &self.authority,
                drops
                    .into_iter()
                    .map(|drop| (drop.entity_type_id, drop.position, drop.stack)),
            );
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

    pub(crate) fn tick_animal_temptation(
        &self,
        sessions: &SessionRegistry,
        tags: &mc_data::tags::TagsData,
    ) -> usize {
        sessions.tick_animal_temptation(tags)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tick_villager_population(
        &self,
        sessions: &SessionRegistry,
        population: &VillagerPopulationSelection,
        current_tick: u64,
        food_items: VillagerFoodItemIds,
        villager_type_id: i32,
        item_type_id: i32,
        elapsed_ticks: u32,
    ) -> usize {
        let (births, dispatches) = sessions.tick_villager_population(
            &self.authority,
            population,
            current_tick,
            food_items,
            villager_type_id,
            item_type_id,
            elapsed_ticks,
        );
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

    #[cfg(test)]
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

    pub(crate) fn tick_hostile_attacks_with_world_sight(
        &self,
        sessions: &SessionRegistry,
        tick: u64,
        air: BlockStateId,
        world: Option<&WorldReadView>,
        blocks: &BlockRegistry,
    ) -> usize {
        let (attacks, dispatches) = sessions.tick_hostile_attacks_with_world_sight(
            &self.authority,
            tick,
            air,
            world,
            blocks,
        );
        dispatch_visibility_commands(dispatches);
        attacks
    }

    pub(crate) fn tick_dragon_authority(&self, sessions: &SessionRegistry, tick: u64) {
        dispatch_visibility_commands(sessions.tick_dragon_air_combat(&self.authority, tick));
        dispatch_visibility_commands(sessions.tick_dragon_breath_clouds(&self.authority, tick));
    }

    pub(crate) fn tick_village_defense(
        &self,
        sessions: &SessionRegistry,
        tick: u64,
        iron_golem_type_id: i32,
        world_read: Option<&mc_world::WorldReadView>,
        materials: Option<&BlockMaterialIds>,
    ) -> super::session::VillageDefenseReport {
        let (report, dispatches) = sessions.tick_village_defense(
            &self.authority,
            tick,
            iron_golem_type_id,
            world_read,
            materials,
        );
        dispatch_visibility_commands(dispatches);
        report
    }

    pub(crate) fn tick_dying_entities(&self, sessions: &SessionRegistry, tick: u64) {
        dispatch_visibility_commands(sessions.tick_dying_entities(&self.authority, tick));
    }

    pub(crate) async fn run_random_ticks_with_budget(
        &self,
        config: &crate::server::ServerConfig,
        sessions: &SessionRegistry,
        access: SimulationWorldAccess<'_>,
        protection: Option<&crate::script::ZoneProtectionSnapshot>,
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
            protection,
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
            None,
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
        let mut runs: Vec<SimulationExecutionRun> = Vec::new();
        for envelope in batch {
            let requires_world = command_requires_world(&envelope.command);
            let block_drop_command =
                matches!(envelope.command, SimulationCommand::CommitBlockDrops { .. });
            let cross_region_plugin_edit = requires_world
                && regional_block_edits_available
                && !envelope.response_is_closed()
                && envelope
                    .session_fence
                    .is_none_or(|session_id| sessions.is_active_session(session_id))
                && matches!(
                    envelope.command,
                    SimulationCommand::ApplyBlockEdits {
                        plugin_receipt: Some(_),
                        leaf_trigger: true,
                        ..
                    }
                )
                && world_chunk_journal.is_some();
            let regional_block_edit = requires_world
                && regional_block_edits_available
                && !cross_region_plugin_edit
                && !envelope.response_is_closed()
                && envelope
                    .session_fence
                    .is_none_or(|session_id| sessions.is_active_session(session_id))
                && command_can_use_regional_mutation(&envelope.command, access.read, block_light)
                // A server-owned composite carries a plugin receipt in the same
                // decision as its container after-image, so a run that cannot
                // journal it must not admit it: the non-regional fallback
                // refuses the command instead of committing a container whose
                // receipt has nowhere to go.
                && (!command_needs_world_journal(&envelope.command)
                    || world_chunk_journal.is_some());
            let journaled_block_edit = regional_block_edit
                && world_chunk_journal.is_some()
                && matches!(
                    envelope.command,
                    SimulationCommand::ApplyBlockEdits { .. }
                        | SimulationCommand::CommitChest {
                            plugin_receipt: Some(_),
                            ..
                        }
                );
            if let Some(last_run) = runs.last_mut()
                && last_run.requires_world == requires_world
                && last_run.regional_block_edit == regional_block_edit
                && last_run.journaled_block_edit == journaled_block_edit
                && last_run.block_drop_command == block_drop_command
                && last_run.cross_region_plugin_edit == cross_region_plugin_edit
            {
                last_run.envelopes.push(envelope);
            } else {
                runs.push(SimulationExecutionRun {
                    requires_world,
                    regional_block_edit,
                    journaled_block_edit,
                    block_drop_command,
                    cross_region_plugin_edit,
                    envelopes: vec![envelope],
                });
            }
        }

        let mut processed = 0;
        let mut lane_attribution = Vec::new();
        let mut runs = runs.into_iter();
        while let Some(SimulationExecutionRun {
            requires_world,
            regional_block_edit,
            journaled_block_edit,
            block_drop_command,
            cross_region_plugin_edit,
            envelopes: run,
        }) = runs.next()
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
                    journaled_block_edit.then(|| {
                        world_chunk_journal
                            .as_ref()
                            .expect("journaled run has a journal")
                    }),
                    run,
                )
                .await
            } else if cross_region_plugin_edit {
                let result = self
                    .process_cross_region_plugin_block_edit_run(sessions, access, block_light, run)
                    .await;
                owner_fail_stopped = result.fail_stopped;
                result.report
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
                        if matches!(
                            envelope.command,
                            SimulationCommand::SaveBarrier {
                                capture_world: true
                            }
                        ) {
                            self.process_world_save_barrier(sessions, storage, envelope)
                        } else {
                            self.process_batch(
                                sessions,
                                BatchWorldAccess::Storage(&mut storage),
                                block_light,
                                Some(&mut pending_relight),
                                vec![envelope],
                            )
                        }
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
                for remaining in runs {
                    for envelope in remaining.envelopes {
                        envelope.respond(Err(SimulationRequestError::OwnerStopped));
                    }
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

    async fn process_cross_region_plugin_block_edit_run(
        &mut self,
        sessions: &SessionRegistry,
        access: SimulationWorldAccess<'_>,
        block_light: Option<&BlockLightTable>,
        run: Vec<SimulationCommandEnvelope>,
    ) -> ResidentBlockDropRunResult {
        let (Some(mutation), Some(world_read)) = (access.mutation, access.read) else {
            return ResidentBlockDropRunResult {
                report: self.process_batch(
                    sessions,
                    BatchWorldAccess::Unavailable(SimulationRequestError::WorldUnavailable),
                    block_light,
                    None,
                    run,
                ),
                fail_stopped: false,
            };
        };
        let mut processed = 0;
        let world_tick = sessions.simulation_tick();
        let mut owner_fail_stopped = false;
        for envelope in run {
            if owner_fail_stopped {
                envelope.respond(Err(SimulationRequestError::OwnerStopped));
                continue;
            }
            if envelope.response_is_closed() {
                self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let SimulationCommand::ApplyBlockEdits {
                actor_session,
                edits,
                preconditions,
                scheduled_block_ticks,
                leaf_trigger,
                zone_fence,
                hook_approval,
                plugin_receipt: Some(plugin_receipt),
                material_debit,
            } = &envelope.command
            else {
                envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                continue;
            };
            if !*leaf_trigger {
                envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                continue;
            }
            if zone_fence.as_ref().is_some_and(|fence| !fence.is_current()) {
                envelope.respond(Err(SimulationRequestError::Precommit(
                    mc_script::precommit::HookFailure::PermissionDenied,
                )));
                continue;
            }
            // A non-keep decision can refuse before any world preparation; a
            // Keep remains unspent until the receipt-bearing transaction has
            // revalidated its source images at the common durable commit.
            if hook_approval.as_ref().is_some_and(|approval| {
                !matches!(
                    approval.decision(),
                    mc_script::precommit::HookDecision::Keep
                )
            }) {
                if let Err(error) = precommit::refuse_build_approval(hook_approval) {
                    envelope.respond(Err(SimulationRequestError::Precommit(error)));
                } else {
                    envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                }
                continue;
            }
            let _warehouse_admission = if let Some(material_debit) = material_debit {
                let Some(admission) = sessions
                    .try_lock_warehouse_reservation_admission(material_debit.debit.position)
                else {
                    envelope.respond(Ok(SimulationResponse::SettlementPortion(Ok(None))));
                    continue;
                };
                let Some(expected) =
                    container_chest_image(&material_debit.debit.expected_container)
                else {
                    envelope.respond(Ok(SimulationResponse::SettlementPortion(Ok(None))));
                    continue;
                };
                let Some(updated) = container_chest_image(&material_debit.debit.updated_container)
                else {
                    envelope.respond(Ok(SimulationResponse::SettlementPortion(Ok(None))));
                    continue;
                };
                if !sessions.warehouse_reservation_stock_survives_after_consuming(
                    material_debit.debit.position,
                    &expected,
                    &updated,
                ) {
                    envelope.respond(Ok(SimulationResponse::SettlementPortion(Ok(None))));
                    continue;
                }
                Some(admission)
            } else {
                None
            };
            let edits = super::block_edit_commit::resident_block_edits(edits);
            let preconditions =
                super::block_edit_commit::resident_block_preconditions(preconditions);
            match super::commit_cross_region_scheduled_block_tick(
                sessions,
                mutation,
                world_tick,
                super::ResidentBlockCommit {
                    edits: &edits,
                    preconditions: &preconditions,
                    consumed_block_ticks: scheduled_block_ticks,
                    consumed_fluid_ticks: &[],
                    scheduled_fluid_ticks: &[],
                    light_table: block_light,
                    leaf_trigger_tick: Some(world_tick.saturating_add(1)),
                },
                Some(plugin_receipt.clone()),
                super::CrossRegionScheduledBlockTickContext {
                    material_debit: material_debit.as_ref(),
                    zone_fence: zone_fence.clone(),
                    hook_approval: hook_approval.clone(),
                },
            )
            .await
            {
                Ok(Some((mut outcome, Some(decision_id)))) => {
                    let (light_sources, light_updates) =
                        regional_light_updates(world_read, block_light, Some(&outcome));
                    publish_regional_light_updates(
                        sessions,
                        mutation,
                        access.light,
                        light_sources.as_ref(),
                        light_updates,
                        &mut outcome,
                    );
                    dispatch_regional_block_outcome(sessions, *actor_session, &outcome);
                    if let Some(material_debit) = material_debit {
                        let (position, slots) = Self::material_debit_chest_slot_dispatch(
                            mutation.block_registry(),
                            world_read,
                            mutation,
                            material_debit.debit.position,
                            material_debit.debit.updated_container.clone(),
                        );
                        let (_, dispatches) =
                            sessions.server_chest_slot_dispatches(position, slots);
                        dispatch_visibility_commands(dispatches);
                    }
                    self.metrics
                        .block_edits_processed
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Ok(SimulationResponse::SettlementPortion(Ok(Some(
                        decision_id,
                    )))));
                }
                Ok(Some((_, None))) | Ok(None) => {
                    self.metrics
                        .block_edits_processed
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Ok(SimulationResponse::SettlementPortion(Ok(None))));
                }
                Err(()) => {
                    self.metrics
                        .rejected_world_mutation
                        .fetch_add(1, Ordering::Relaxed);
                    envelope.respond(Err(SimulationRequestError::WorldMutationFailed));
                    owner_fail_stopped = true;
                    self.shutdown();
                }
            }
            processed += 1;
        }
        ResidentBlockDropRunResult {
            report: SimulationTickReport {
                processed,
                remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
                ..SimulationTickReport::default()
            },
            fail_stopped: owner_fail_stopped,
        }
    }

    fn material_debit_chest_slot_dispatch(
        blocks: &BlockRegistry,
        world_read: &WorldReadView,
        mutation: &WorldMutationView,
        position: BlockPos,
        fallback_slots: Vec<ItemStack>,
    ) -> (BlockPos, Vec<ItemStack>) {
        let Some(state) = world_read.get_cached_block(position) else {
            return (position, fallback_slots);
        };
        let mut positions = vec![position];
        if let Some(partner) = super::block_placement::chest::paired_position(
            blocks,
            |candidate| world_read.get_cached_block(candidate),
            position,
            state,
        ) {
            positions.push(partner);
            positions.sort_by_key(|candidate| (candidate.x, candidate.y, candidate.z));
        }
        let mut chests = Vec::with_capacity(positions.len());
        for slot_position in &positions {
            let Some(mut image) = mutation.chest_block_entities(&[*slot_position]) else {
                return (position, fallback_slots);
            };
            let Some(image) = image.pop() else {
                return (position, fallback_slots);
            };
            chests.push(image);
        }
        (positions[0], chest_slot_stacks(&ChestView { chests }))
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

            let resident_edits = resident_block_edits(&edits);
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
            expected_tokens,
            expected_state_id,
            actor_session,
            expected,
            updated,
            player,
            plugin_receipt,
        } = request;
        if plugin_receipt.is_some() {
            // A server-owned composite journals its receipt and the container's
            // after-image in one decision, so it only ever runs in the
            // journaled regional lane; reaching the menu path means that lane
            // could not admit it, and nothing may mutate.
            return Err(SimulationRequestError::WorldMutationFailed);
        }
        let (Some(actor_session), Some(player)) = (actor_session, player) else {
            // The menu fallback is session-authored end to end: a command
            // without a player participant is a composite, and a composite
            // belongs to the journaled regional lane above.
            return Err(SimulationRequestError::InvalidCommand);
        };
        if positions.is_empty()
            || positions.len() > 2
            || positions.first() != Some(&primary_position)
            || expected.len() != positions.len()
            || updated.len() != positions.len()
            || expected_tokens.is_none_or(|tokens| tokens.len() != positions.len())
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
        if authoritative != expected
            || expected_tokens.is_some_and(|tokens| {
                positions.iter().zip(tokens).any(|(position, token)| {
                    storage.block_mutation_token(*position) != Some(*token)
                })
            })
        {
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
        let Some(_warehouse_reservation_admission) =
            sessions.try_lock_warehouse_reservation_admission(primary_position)
        else {
            let (inventory, carried_item) = sessions
                .player_container_state(actor_session)
                .ok_or(SimulationRequestError::StaleSession)?;
            return Ok(Box::new(SharedContainerCommit::Rejected {
                state_id: sessions.chest_state_id(primary_position),
                authoritative,
                inventory,
                carried_item,
            }));
        };
        if !sessions.warehouse_reservation_stock_survives(positions, updated) {
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
        let before_view = ChestView {
            chests: expected.to_vec(),
        };
        let after_view = ChestView {
            chests: updated.to_vec(),
        };
        let state_id_increment = chest_menu_state_change_count(
            &before_view,
            &after_view,
            &player.expected_inventory,
            &player.updated_inventory,
            &player.expected_carried_item,
            &player.updated_carried_item,
        );
        let slots = chest_slot_stacks(&after_view);
        let commit = sessions.commit_chest_slots(
            &self.authority,
            ContainerCommitContext {
                position: primary_position,
                expected_state_id,
                actor_session,
                player,
            },
            state_id_increment,
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
        match storage.commit_opaque_block_entity_conditionally(
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

    fn process_regular_player_pose_batch(
        &self,
        sessions: &SessionRegistry,
        first_envelope: SimulationCommandEnvelope,
        first_pose: PlayerPoseCommitRequest,
        batch: &mut VecDeque<SimulationCommandEnvelope>,
    ) -> usize {
        let mut pose_envelopes = vec![first_envelope];
        let mut pose_requests = vec![first_pose];
        while let Some(next) = batch.front() {
            let Some(request) = regular_player_pose_command(&next.command) else {
                break;
            };
            if next.response_is_closed()
                || next
                    .session_fence
                    .is_some_and(|session_id| !sessions.is_active_session(session_id))
            {
                break;
            }
            pose_requests.push(request);
            pose_envelopes.push(batch.pop_front().expect("matching pose command"));
        }
        let command_count = pose_envelopes.len();
        #[cfg(feature = "load-bench")]
        let command_started = Instant::now();
        let outcomes = sessions.commit_player_pose_batch(
            &self.authority,
            pose_requests,
            self.player_movement_authority.as_ref(),
        );
        #[cfg(feature = "load-bench")]
        {
            let command_elapsed_us =
                u64::try_from(command_started.elapsed().as_micros()).unwrap_or(u64::MAX);
            self.metrics.record_command_kind_batch(
                "commit_player_pose",
                command_count,
                command_elapsed_us,
            );
        }
        let mut dispatches = Vec::new();
        let mut responses = Vec::with_capacity(command_count);
        for outcome in outcomes {
            match outcome {
                Ok((mut pose_dispatches, committed)) => {
                    dispatches.append(&mut pose_dispatches);
                    responses.push(SimulationResponse::PlayerPose(Ok(committed)));
                }
                Err(error) => {
                    responses.push(SimulationResponse::PlayerPose(Err(error)));
                }
            }
        }
        dispatch_visibility_commands(dispatches);
        for (envelope, response) in pose_envelopes.into_iter().zip(responses) {
            envelope.respond(Ok(response));
        }
        self.metrics
            .processed
            .fetch_add(command_count as u64, Ordering::Relaxed);
        command_count
    }

    fn read_block_snapshot_response(
        &self,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        resident_block_snapshot: Option<(BlockPos, BlockMutationSnapshot)>,
        position: BlockPos,
    ) -> SimulationResponse {
        let result = if let Some((snapshot_position, snapshot)) = resident_block_snapshot
            && snapshot_position == position
        {
            Ok(Some(snapshot))
        } else if let Some(storage) = storage {
            match storage.get_block(position) {
                Ok(Some(state)) => Ok(storage
                    .block_mutation_token(position)
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

    fn read_chest_snapshot_response(
        &self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        positions: &[BlockPos],
    ) -> SimulationResponse {
        let result = if positions.is_empty()
            || positions.len() > 2
            || positions.windows(2).any(|pair| pair[0] == pair[1])
        {
            Err(SimulationRequestError::InvalidCommand)
        } else if let Some(storage) = storage {
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

    fn read_furnace_snapshot_response(
        &self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        position: BlockPos,
    ) -> SimulationResponse {
        let result = if let Some(storage) = storage {
            match storage.furnace_block_entity(position) {
                Ok(Some(furnace)) => Ok(Box::new(FurnaceReadSnapshot {
                    furnace,
                    state_id: sessions.furnace_state_id(position),
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

    #[allow(clippy::too_many_arguments)]
    fn pickup_item_response(
        &self,
        sessions: &SessionRegistry,
        entity_id: EntityId,
        collector_session: SessionId,
        expected_item_id: u32,
        expected_damage: Option<i32>,
        expected_enchantments: &[mc_data::ItemEnchantment],
        max_stack: i32,
    ) -> SimulationResponse {
        let mut credited = sessions
            .pickup_item_into_inventory(
                &self.authority,
                entity_id,
                collector_session,
                expected_item_id,
                expected_damage,
                expected_enchantments,
                max_stack,
            )
            .map(Box::new);
        if let Some(credited) = credited.as_mut() {
            dispatch_visibility_commands(std::mem::take(&mut credited.dispatches));
        }
        SimulationResponse::ItemPickupCredit(credited)
    }

    fn pickup_experience_response(
        &self,
        sessions: &SessionRegistry,
        entity_id: EntityId,
        collector_session: SessionId,
    ) -> SimulationResponse {
        let mut credited = sessions
            .pickup_experience_into_player(&self.authority, entity_id, collector_session)
            .map(Box::new);
        if let Some(credited) = credited.as_mut() {
            dispatch_visibility_commands(std::mem::take(&mut credited.dispatches));
        }
        SimulationResponse::ExperiencePickupCredit(credited)
    }

    fn pickup_arrow_response(
        &self,
        sessions: &SessionRegistry,
        entity_id: EntityId,
        collector_session: SessionId,
        arrow_item_id: u32,
        max_stack: i32,
    ) -> SimulationResponse {
        let mut credited = sessions
            .pickup_arrow_into_inventory(
                &self.authority,
                entity_id,
                collector_session,
                arrow_item_id,
                max_stack,
            )
            .map(Box::new);
        if let Some(credited) = credited.as_mut() {
            dispatch_visibility_commands(std::mem::take(&mut credited.dispatches));
        }
        SimulationResponse::ArrowPickupCredit(credited)
    }

    fn player_attack_response(
        &self,
        sessions: &SessionRegistry,
        attacker_session: SessionId,
        entity_id: EntityId,
        damage: f32,
        attacker_costs: Option<&PlayerSurvivalPlan>,
        cooldown_tick: u64,
    ) -> SimulationResponse {
        let authority_tick = sessions.simulation_tick();
        let mut result = sessions.player_attack_entity(
            &self.authority,
            PlayerEntityAttack {
                attacker_session,
                entity_id,
                amount: damage,
                attacker_costs,
                authority_tick,
                hook_approval: None,
            },
        );
        if let PlayerAttackResult::Damaged(outcome) = &mut result
            && let EntityAttackOutcome::PlayerDamaged { dispatches, .. } = &mut **outcome
        {
            dispatch_visibility_commands(std::mem::take(dispatches));
        }
        if !matches!(result, PlayerAttackResult::ValidationRejected) {
            sessions.publish_player_attack(
                attacker_session,
                entity_id.0,
                cooldown_tick,
                authority_tick,
            );
        }
        SimulationResponse::PlayerAttack(result)
    }

    fn entity_effect_response(
        &self,
        sessions: &SessionRegistry,
        command: &ServerEntityEffectCommand,
        response: &mut Option<queue::SimulationResponseSender>,
    ) -> SimulationResponse {
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
            response,
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

    fn set_world_time_response(
        &self,
        sessions: &SessionRegistry,
        world_time: u64,
    ) -> SimulationResponse {
        let outcome = sessions.set_world_time_owned(&self.authority, world_time);
        #[cfg(test)]
        self.release_retryable_herd_requests(outcome.retryable_chunks());
        dispatch_visibility_commands(outcome.into_dispatches());
        SimulationResponse::WorldTimeSet
    }

    fn spawn_command_entity_response(
        &self,
        sessions: &SessionRegistry,
        entity_type_id: i32,
        entity_type_name: &str,
        position: Vec3,
    ) -> SimulationResponse {
        SimulationResponse::EntitySpawn(sessions.spawn_command_entity(
            &self.authority,
            entity_type_id,
            entity_type_name.to_owned(),
            position,
        ))
    }

    fn script_entity_damage_response(
        &self,
        sessions: &SessionRegistry,
        entity_id: EntityId,
        damage: f32,
        plugin_id: &str,
        response: &mut Option<queue::SimulationResponseSender>,
    ) -> SimulationResponse {
        let result = sessions
            .damage_script_entity(&self.authority, entity_id, damage, plugin_id, response)
            .map(|mut outcome| {
                let (health, killed) = match &outcome {
                    EntityAttackOutcome::Damaged { damage, .. } => (damage.snapshot.health, false),
                    EntityAttackOutcome::Killed { damage, .. } => (damage.snapshot.health, true),
                    EntityAttackOutcome::PlayerDamaged { .. } => {
                        unreachable!("script entity damage never targets players")
                    }
                };
                dispatch_visibility_commands(std::mem::take(outcome.dispatches_mut()));
                ScriptEntityDamageCommit { health, killed }
            });
        SimulationResponse::ScriptEntityDamage(result)
    }

    fn food_use_response(
        &self,
        sessions: &SessionRegistry,
        command: &FoodUseCommand,
    ) -> SimulationResponse {
        let result = if valid_food_use_plan(&command.plan) {
            Ok(sessions
                .commit_food_use(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::FoodUse(result)
    }

    fn animal_feed_response(
        &self,
        sessions: &SessionRegistry,
        command: &AnimalFeedCommand,
    ) -> SimulationResponse {
        let result = if valid_animal_feed_plan(&command.plan) {
            Ok(sessions
                .commit_animal_feed(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::AnimalFeed(result)
    }

    fn merchant_trade_response(
        &self,
        sessions: &SessionRegistry,
        command: &MerchantTradeCommand,
    ) -> SimulationResponse {
        let result = if valid_merchant_trade_plan(&command.plan) {
            Ok(sessions
                .commit_merchant_trade(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::MerchantTrade(result)
    }

    fn sheep_shear_response(
        &self,
        sessions: &SessionRegistry,
        command: &SheepShearCommand,
    ) -> SimulationResponse {
        let result = if valid_sheep_shear_plan(&command.plan) {
            Ok(sessions
                .commit_sheep_shear(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::SheepShear(result)
    }

    fn zombie_villager_cure_response(
        &self,
        sessions: &SessionRegistry,
        command: &ZombieVillagerCureCommand,
    ) -> SimulationResponse {
        let result = if valid_zombie_villager_cure_plan(&command.plan) {
            Ok(sessions
                .commit_zombie_villager_cure(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::ZombieVillagerCure(result)
    }

    fn player_survival_response(
        &self,
        sessions: &SessionRegistry,
        command: &PlayerSurvivalCommand,
    ) -> SimulationResponse {
        let result = if valid_player_survival_plan(&command.plan) {
            Ok(sessions
                .commit_player_survival(&self.authority, command.actor_session, &command.plan)
                .map(|outcome| {
                    Box::new(match outcome {
                        PlayerSurvivalCommitOutcome::Committed(mut committed) => {
                            dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
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

    fn player_pose_response(
        &self,
        sessions: &SessionRegistry,
        actor_session: SessionId,
        kind: PlayerPoseCommitKind,
        pose: PlayerPose,
        exhaustion: f32,
    ) -> SimulationResponse {
        let result = sessions
            .commit_player_pose_request(
                &self.authority,
                PlayerPoseCommitRequest {
                    actor_session,
                    kind,
                    pose,
                    exhaustion,
                },
                self.player_movement_authority.as_ref(),
            )
            .map(|(dispatches, committed)| {
                dispatch_visibility_commands(dispatches);
                committed
            });
        SimulationResponse::PlayerPose(result)
    }

    fn player_state_event_response(
        &self,
        sessions: &SessionRegistry,
        actor_session: SessionId,
        event: PlayerStateEvent,
    ) -> SimulationResponse {
        let result = sessions
            .commit_player_state_event(&self.authority, actor_session, event)
            .map(dispatch_visibility_commands);
        SimulationResponse::PlayerStateEvent(result)
    }

    fn player_inventory_response(
        &self,
        sessions: &SessionRegistry,
        actor_session: SessionId,
        player: &ContainerPlayerPlan,
    ) -> SimulationResponse {
        let mut result = if valid_container_player_plan(player) {
            sessions
                .commit_player_inventory(&self.authority, actor_session, player)
                .map_err(|error| match error {
                    PlayerInventoryCommitError::MissingPlayer => {
                        SimulationRequestError::StaleSession
                    }
                })
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        if let Ok(PlayerInventoryCommitOutcome::Committed { dispatches, .. }) = &mut result {
            dispatch_visibility_commands(std::mem::take(dispatches));
        }
        SimulationResponse::PlayerInventory(Box::new(result))
    }

    fn bow_release_response(
        &self,
        sessions: &SessionRegistry,
        command: &BowReleaseCommand,
    ) -> SimulationResponse {
        let result = if valid_bow_release_plan(&command.plan) {
            Ok(sessions
                .commit_bow_release(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::BowRelease(result)
    }

    fn selected_item_drop_response(
        &self,
        sessions: &SessionRegistry,
        command: &SelectedItemDropCommand,
    ) -> SimulationResponse {
        let result = if valid_selected_item_drop_plan(&command.plan) {
            Ok(sessions
                .commit_selected_item_drop(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::SelectedItemDrop(result)
    }

    fn throwable_item_release_response(
        &self,
        sessions: &SessionRegistry,
        command: &ThrowableItemReleaseCommand,
    ) -> SimulationResponse {
        let result = if valid_throwable_item_release_plan(&command.plan) {
            Ok(sessions
                .commit_throwable_item_release(
                    &self.authority,
                    command.actor_session,
                    &command.plan,
                )
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::ThrowableItemRelease(result)
    }

    fn boat_placement_response(
        &self,
        sessions: &SessionRegistry,
        command: &BoatPlacementCommand,
    ) -> SimulationResponse {
        let result = if valid_boat_placement_plan(&command.plan) {
            Ok(sessions
                .commit_boat_placement(&self.authority, command.actor_session, &command.plan)
                .map(|mut committed| {
                    dispatch_visibility_commands(std::mem::take(&mut committed.dispatches));
                    Box::new(committed)
                }))
        } else {
            Err(SimulationRequestError::InvalidCommand)
        };
        SimulationResponse::BoatPlacement(result)
    }

    fn chest_commit_response(
        &self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        request: ChestCommitRequest<'_>,
    ) -> SimulationResponse {
        let mut result = self.commit_chest_command(sessions, storage, world_error, request);
        if let Ok(outcome) = &mut result
            && let SharedContainerCommit::Committed { dispatches, .. } = outcome.as_mut()
        {
            dispatch_visibility_commands(std::mem::take(dispatches));
        }
        SimulationResponse::ChestCommit(result)
    }

    fn furnace_commit_response(
        &self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        request: FurnaceCommitRequest<'_>,
    ) -> SimulationResponse {
        let mut result = self.commit_furnace_command(sessions, storage, world_error, request);
        if let Ok(outcome) = &mut result
            && let SharedContainerCommit::Committed { dispatches, .. } = outcome.as_mut()
        {
            dispatch_visibility_commands(std::mem::take(dispatches));
        }
        SimulationResponse::FurnaceCommit(result)
    }

    fn opaque_block_entity_response(
        &self,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: &[u8],
    ) -> SimulationResponse {
        SimulationResponse::OpaqueBlockEntity(self.commit_opaque_block_entity_command(
            storage,
            world_error,
            position,
            expected_state,
            expected_token,
            bytes.to_vec(),
        ))
    }

    fn campfire_use_response(
        &self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        world_error: SimulationRequestError,
        command: &CampfireUseCommand,
    ) -> SimulationResponse {
        let result = if !valid_campfire_use_plan(&command.plan) {
            Err(SimulationRequestError::InvalidCommand)
        } else if let Some(storage) = storage {
            sessions
                .commit_campfire_use(
                    &self.authority,
                    storage,
                    command.actor_session,
                    &command.plan,
                )
                .map(|committed| {
                    committed.map(|committed| {
                        dispatch_visibility_commands(sessions.block_entity_data_dispatches(
                            command.plan.position,
                            Some(command.actor_session),
                            CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
                            command.plan.client_nbt.clone(),
                        ));
                        Box::new(committed)
                    })
                })
        } else {
            self.record_world_access_error(world_error);
            Err(world_error)
        };
        SimulationResponse::CampfireUse(result)
    }

    fn active_session_envelope(
        &self,
        sessions: &SessionRegistry,
        envelope: SimulationCommandEnvelope,
    ) -> Option<SimulationCommandEnvelope> {
        if envelope.response_is_closed() {
            self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        if envelope
            .session_fence
            .is_some_and(|session_id| !sessions.is_active_session(session_id))
        {
            self.metrics
                .rejected_stale_session
                .fetch_add(1, Ordering::Relaxed);
            envelope.respond(Err(SimulationRequestError::StaleSession));
            return None;
        }
        Some(envelope)
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
            let Some(mut envelope) = self.active_session_envelope(sessions, envelope) else {
                continue;
            };
            if let SimulationCommand::BeginPrecommitSurvivalBreak(command) = &envelope.command {
                let actor_session = command.actor_session;
                let prepared = if let Some(storage) = storage.as_deref_mut() {
                    prepare_survival_block_break_plan(storage, &command.plan)
                } else {
                    self.record_world_access_error(world_error);
                    envelope.respond(Ok(SimulationResponse::SurvivalBreak(Err(world_error))));
                    processed += 1;
                    self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let Some(plan) = prepared else {
                    envelope.respond(Ok(SimulationResponse::SurvivalBreak(Ok(None))));
                    processed += 1;
                    self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let pending = precommit::player_actor(sessions, actor_session)
                    .and_then(|actor| {
                        precommit::build_context(actor, &plan.edits, &plan.preconditions)
                    })
                    .and_then(|context| command.boundary.begin_precommit(context));
                let pending = match pending {
                    Ok(pending) => pending,
                    Err(error) => {
                        envelope.respond(Ok(SimulationResponse::SurvivalBreak(Err(
                            SimulationRequestError::Precommit(error),
                        ))));
                        processed += 1;
                        self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let handle = command.resume_handle.clone();
                let response = envelope.take_response();
                handle.spawn_precommit_resume(
                    pending,
                    Some(actor_session),
                    response,
                    move |decision| {
                        let request = match decision {
                            Ok(approval) => {
                                let mut plan = plan;
                                plan.hook_approval = Some(approval);
                                SurvivalBreakRequest::Prepared(plan)
                            }
                            Err(error) => SurvivalBreakRequest::PrecommitFailure(error),
                        };
                        SimulationCommand::CommitSurvivalBreak(Box::new(SurvivalBreakCommand {
                            actor_session,
                            request,
                        }))
                    },
                );
                processed += 1;
                self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if let SimulationCommand::BeginPrecommitSurvivalPlacement(command) = &envelope.command {
                let actor_session = command.actor_session;
                let pending = precommit::player_actor(sessions, actor_session)
                    .and_then(|actor| {
                        precommit::build_context(
                            actor,
                            &command.plan.edits,
                            &command.plan.preconditions,
                        )
                    })
                    .and_then(|context| command.boundary.begin_precommit(context));
                let pending = match pending {
                    Ok(pending) => pending,
                    Err(error) => {
                        envelope.respond(Ok(SimulationResponse::SurvivalPlacement(Err(
                            SimulationRequestError::Precommit(error),
                        ))));
                        processed += 1;
                        self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let handle = command.resume_handle.clone();
                let plan = command.plan.clone();
                let response = envelope.take_response();
                handle.spawn_precommit_resume(
                    pending,
                    Some(actor_session),
                    response,
                    move |decision| match decision {
                        Ok(approval) => {
                            let mut plan = plan;
                            plan.hook_approval = Some(approval);
                            SimulationCommand::CommitSurvivalPlacement(Box::new(
                                SurvivalPlacementCommand {
                                    actor_session,
                                    plan,
                                },
                            ))
                        }
                        Err(error) => SimulationCommand::PrecommitSurvivalPlacementFailure(error),
                    },
                );
                processed += 1;
                self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if let SimulationCommand::BeginPrecommitBucketUse(command) = &envelope.command {
                let actor_session = command.actor_session;
                let pending = precommit::player_actor(sessions, actor_session)
                    .and_then(|actor| {
                        precommit::build_context(
                            actor,
                            std::slice::from_ref(&command.plan.edit),
                            std::slice::from_ref(&command.plan.precondition),
                        )
                    })
                    .and_then(|context| command.boundary.begin_precommit(context));
                let pending = match pending {
                    Ok(pending) => pending,
                    Err(error) => {
                        envelope.respond(Ok(SimulationResponse::BucketUse(Err(
                            SimulationRequestError::Precommit(error),
                        ))));
                        processed += 1;
                        self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                };
                let handle = command.resume_handle.clone();
                let plan = command.plan.clone();
                let response = envelope.take_response();
                handle.spawn_precommit_resume(
                    pending,
                    Some(actor_session),
                    response,
                    move |decision| match decision {
                        Ok(approval) => {
                            let mut plan = plan;
                            plan.hook_approval = Some(approval);
                            SimulationCommand::CommitBucketUse(Box::new(BucketUseCommand {
                                actor_session,
                                plan,
                            }))
                        }
                        Err(error) => SimulationCommand::PrecommitBucketUseFailure(error),
                    },
                );
                processed += 1;
                self.metrics.processed.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if let Some(first_pose) = regular_player_pose_command(&envelope.command) {
                processed += self
                    .process_regular_player_pose_batch(sessions, envelope, first_pose, &mut batch);
                continue;
            }
            let detached = envelope.is_detached();
            #[cfg(test)]
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
            #[cfg(feature = "load-bench")]
            let command_kind = envelope.command.kind();
            #[cfg(feature = "load-bench")]
            let command_started = Instant::now();
            let mut response = match &envelope.command {
                SimulationCommand::SaveBarrier { capture_world } => self.save_barrier_response(
                    sessions,
                    storage.as_deref_mut(),
                    world_error,
                    *capture_world,
                ),
                SimulationCommand::ReadBlockSnapshot { position } => self
                    .read_block_snapshot_response(
                        storage.as_deref_mut(),
                        world_error,
                        resident_block_snapshot,
                        *position,
                    ),
                SimulationCommand::ReadChestSnapshot { positions } => self
                    .read_chest_snapshot_response(
                        sessions,
                        storage.as_deref_mut(),
                        world_error,
                        positions,
                    ),
                SimulationCommand::ReadFurnaceSnapshot { position } => self
                    .read_furnace_snapshot_response(
                        sessions,
                        storage.as_deref_mut(),
                        world_error,
                        *position,
                    ),
                SimulationCommand::PickupItemIntoInventory {
                    entity_id,
                    collector_session,
                    expected_item_id,
                    expected_damage,
                    expected_enchantments,
                    max_stack,
                } => self.pickup_item_response(
                    sessions,
                    *entity_id,
                    *collector_session,
                    *expected_item_id,
                    *expected_damage,
                    expected_enchantments,
                    *max_stack,
                ),
                SimulationCommand::PickupExperienceIntoPlayer {
                    entity_id,
                    collector_session,
                } => self.pickup_experience_response(sessions, *entity_id, *collector_session),
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
                } => self.pickup_arrow_response(
                    sessions,
                    *entity_id,
                    *collector_session,
                    *arrow_item_id,
                    *max_stack,
                ),
                SimulationCommand::PlayerAttackServerEntity {
                    attacker_session,
                    entity_id,
                    damage,
                    attacker_costs,
                    cooldown_tick,
                } => self.player_attack_response(
                    sessions,
                    *attacker_session,
                    *entity_id,
                    *damage,
                    attacker_costs.as_deref(),
                    *cooldown_tick,
                ),
                SimulationCommand::ResumePlayerDamagePrecommit(resume) => {
                    let dispatches = sessions
                        .resume_player_damage_precommit(&self.authority, (**resume).clone());
                    dispatch_visibility_commands(dispatches);
                    SimulationResponse::DamagePrecommit
                }
                SimulationCommand::ResumeEntityDamagePrecommit(resume) => {
                    let (result, dispatches) = sessions
                        .resume_entity_damage_precommit(&self.authority, (**resume).clone());
                    dispatch_visibility_commands(dispatches);
                    match result {
                        super::session::damage_precommit::EntityDamagePrecommitResult::Direct
                        | super::session::damage_precommit::EntityDamagePrecommitResult::Projectile => {
                            SimulationResponse::DamagePrecommit
                        }
                        super::session::damage_precommit::EntityDamagePrecommitResult::Resident(
                            result,
                        ) => SimulationResponse::ResidentDamage(result),
                        super::session::damage_precommit::EntityDamagePrecommitResult::Script(
                            result,
                        ) => SimulationResponse::ScriptEntityDamage(result.map(
                            |(health, killed)| ScriptEntityDamageCommit { health, killed },
                        )),
                        super::session::damage_precommit::EntityDamagePrecommitResult::Effect(
                            result,
                        ) => SimulationResponse::EntityEffect(result),
                    }
                }
                SimulationCommand::ApplyServerEntityEffect(command) => {
                    let command = (**command).clone();
                    self.entity_effect_response(sessions, &command, &mut envelope.response)
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
                } => self.spawn_command_entity_response(
                    sessions,
                    *entity_type_id,
                    entity_type_name,
                    *position,
                ),
                SimulationCommand::DamageScriptEntity {
                    entity_id,
                    damage,
                    plugin_id,
                } => self.script_entity_damage_response(
                    sessions,
                    *entity_id,
                    *damage,
                    plugin_id,
                    &mut envelope.response,
                ),
                SimulationCommand::DamageResidentEntity { attack, plugin_id } => {
                    let response = &mut envelope.response;
                    SimulationResponse::ResidentDamage(sessions.damage_resident_entity(
                        &self.authority,
                        (**attack).clone(),
                        plugin_id,
                        response,
                    ))
                }
                SimulationCommand::SetWorldTime { world_time } => {
                    self.set_world_time_response(sessions, *world_time)
                }
                #[cfg(test)]
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
                SimulationCommand::EnsureSettlementInhabitants { spawns, .. } => {
                    let dispatches =
                        sessions.ensure_settlement_inhabitants(&self.authority, spawns);
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
                    hook_approval,
                    zone_fence,
                    plugin_receipt,
                    ..
                } => {
                    let settlement_portion = plugin_receipt.is_some();
                    let result = if !valid_block_edit_command(
                        edits,
                        preconditions,
                        scheduled_block_ticks,
                    ) {
                        Err(SimulationRequestError::InvalidCommand)
                    } else if zone_fence.as_ref().is_some_and(|fence| !fence.is_current()) {
                        Err(SimulationRequestError::Precommit(
                            mc_script::precommit::HookFailure::PermissionDenied,
                        ))
                    } else if let Err(error) = precommit::refuse_build_approval(hook_approval) {
                        Err(SimulationRequestError::Precommit(error))
                    } else if settlement_portion {
                        Err(SimulationRequestError::WorldMutationFailed)
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
                        clear_server_owned_campfire_cooking(
                            sessions,
                            storage.as_deref(),
                            *actor_session,
                            outcome.as_ref(),
                        );
                        if let Some(outcome) = outcome.as_mut() {
                            if !regional {
                                let storage = storage
                                    .as_deref_mut()
                                    .expect("coordinator block edit storage");
                                schedule_reactivity_near_applied(
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
                                .loaded_recipients_for_chunks(&outcome.edit_chunks, *actor_session)
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
                                        .loaded_recipients_for_chunks(&light_chunks, *actor_session)
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
                    if settlement_portion {
                        SimulationResponse::SettlementPortion(result.map(|_| None))
                    } else {
                        SimulationResponse::BlockEdits(result)
                    }
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
                SimulationCommand::CommitSurvivalBreak(command) => self
                    .commit_survival_break_response(
                        sessions,
                        storage.as_deref_mut(),
                        block_light,
                        pending_relight.as_deref_mut(),
                        world_error,
                        command,
                    ),
                SimulationCommand::BeginPrecommitSurvivalBreak(_) => {
                    unreachable!("precommit break admission is handled before owner dispatch")
                }
                SimulationCommand::PrecommitSurvivalPlacementFailure(error) => {
                    SimulationResponse::SurvivalPlacement(Err(SimulationRequestError::Precommit(
                        *error,
                    )))
                }
                SimulationCommand::BeginPrecommitSurvivalPlacement(_) => {
                    unreachable!("precommit placement admission is handled before owner dispatch")
                }
                SimulationCommand::CommitSurvivalPlacement(command) => self
                    .commit_survival_placement_response(
                        sessions,
                        storage.as_deref_mut(),
                        block_light,
                        pending_relight.as_deref_mut(),
                        world_error,
                        command,
                    ),
                SimulationCommand::PrecommitBucketUseFailure(error) => {
                    SimulationResponse::BucketUse(Err(SimulationRequestError::Precommit(*error)))
                }
                SimulationCommand::BeginPrecommitBucketUse(_) => {
                    unreachable!("precommit bucket admission is handled before owner dispatch")
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
                                    schedule_reactivity_near_applied(
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
                    self.food_use_response(sessions, command)
                }
                SimulationCommand::CommitAnimalFeed(command) => {
                    self.animal_feed_response(sessions, command)
                }
                SimulationCommand::CommitMerchantTrade(command) => {
                    self.merchant_trade_response(sessions, command)
                }
                SimulationCommand::CommitSheepShear(command) => {
                    self.sheep_shear_response(sessions, command)
                }
                SimulationCommand::CommitZombieVillagerCure(command) => {
                    self.zombie_villager_cure_response(sessions, command)
                }
                SimulationCommand::CommitPlayerSurvival(command) => {
                    self.player_survival_response(sessions, command)
                }
                SimulationCommand::CommitPlayerPose {
                    actor_session,
                    kind,
                    pose,
                    exhaustion,
                    ..
                } => self.player_pose_response(sessions, *actor_session, *kind, *pose, *exhaustion),
                SimulationCommand::CommitPlayerStateEvent {
                    actor_session,
                    event,
                } => self.player_state_event_response(sessions, *actor_session, *event),
                SimulationCommand::CommitPlayerInventory {
                    actor_session,
                    player,
                } => self.player_inventory_response(sessions, *actor_session, player),
                SimulationCommand::CommitBowRelease(command) => {
                    self.bow_release_response(sessions, command)
                }
                SimulationCommand::CommitSelectedItemDrop(command) => {
                    self.selected_item_drop_response(sessions, command)
                }
                SimulationCommand::CommitThrowableItemRelease(command) => {
                    self.throwable_item_release_response(sessions, command)
                }
                SimulationCommand::CommitBoatPlacement(command) => {
                    self.boat_placement_response(sessions, command)
                }
                SimulationCommand::CommitChest {
                    primary_position,
                    positions,
                    expected_tokens,
                    expected_state_id,
                    actor_session,
                    expected,
                    updated,
                    player,
                    plugin_receipt,
                    treatment,
                } => {
                    if treatment.is_some() {
                        SimulationResponse::WarehouseTransfer(Err(
                            SimulationRequestError::InvalidCommand,
                        ))
                    } else {
                        self.chest_commit_response(
                            sessions,
                            storage.as_deref_mut(),
                            world_error,
                            ChestCommitRequest {
                                primary_position: *primary_position,
                                expected_tokens: expected_tokens.as_deref(),
                                positions,
                                expected_state_id: *expected_state_id,
                                actor_session: *actor_session,
                                expected,
                                updated,
                                player: player.as_deref(),
                                plugin_receipt: plugin_receipt.as_deref(),
                            },
                        )
                    }
                }
                SimulationCommand::CommitFurnace {
                    position,
                    expected_state_id,
                    actor_session,
                    expected,
                    updated,
                    player,
                } => self.furnace_commit_response(
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
                ),
                SimulationCommand::CommitOpaqueBlockEntity {
                    position,
                    expected_state,
                    expected_token,
                    bytes,
                } => self.opaque_block_entity_response(
                    storage.as_deref_mut(),
                    world_error,
                    *position,
                    *expected_state,
                    *expected_token,
                    bytes,
                ),
                SimulationCommand::CommitCampfireUse(command) => self.campfire_use_response(
                    sessions,
                    storage.as_deref_mut(),
                    world_error,
                    command,
                ),
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
            #[cfg(feature = "load-bench")]
            {
                let command_elapsed_us =
                    u64::try_from(command_started.elapsed().as_micros()).unwrap_or(u64::MAX);
                self.metrics
                    .record_command_kind(command_kind, command_elapsed_us);
            }
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
                    .expect("pending relight command supports owner relight");
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

    fn commit_survival_break_response(
        &mut self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        block_light: Option<&BlockLightTable>,
        pending_relight: Option<&mut Option<PendingOwnerRelight>>,
        world_error: SimulationRequestError,
        command: &SurvivalBreakCommand,
    ) -> SimulationResponse {
        {
            let result = if let SurvivalBreakRequest::PrecommitFailure(error) = &command.request {
                Err(SimulationRequestError::Precommit(*error))
            } else if let Some(storage) = storage {
                let request_is_valid = match &command.request {
                    SurvivalBreakRequest::Prepared(_) => true,
                    SurvivalBreakRequest::Block(plan) => valid_survival_block_break_plan(plan),
                    SurvivalBreakRequest::PrecommitFailure(_) => {
                        unreachable!("precommit failure returns before native break validation")
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
                        SurvivalBreakRequest::PrecommitFailure(_) => {
                            unreachable!("precommit failure returns before break preparation")
                        }
                    };
                    let plan = match &command.request {
                        SurvivalBreakRequest::Prepared(plan) => Some(plan),
                        SurvivalBreakRequest::Block(_) => prepared_plan.as_ref(),
                        SurvivalBreakRequest::PrecommitFailure(_) => {
                            unreachable!("precommit failure returns before break commit")
                        }
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
                                    if let Some(entity_type_id) = plan.falling_block_entity_type_id
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
                                            && !is_campfire_block(&plan.blocks, edit.new_state)
                                            && sessions.clear_campfire_cooking(edit.pos)
                                        {
                                            committed.block.cleared_campfires.push(edit.pos);
                                        }
                                    }
                                    schedule_reactivity_near_applied(
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
    }

    fn commit_survival_placement_response(
        &mut self,
        sessions: &SessionRegistry,
        storage: Option<&mut WorldStorage>,
        block_light: Option<&BlockLightTable>,
        pending_relight: Option<&mut Option<PendingOwnerRelight>>,
        world_error: SimulationRequestError,
        command: &SurvivalPlacementCommand,
    ) -> SimulationResponse {
        {
            let result = if !valid_survival_placement_plan(&command.plan) {
                Err(SimulationRequestError::InvalidCommand)
            } else if let Some(storage) = storage {
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
                            schedule_reactivity_near_applied(
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
                                committed.block.precomputed_light_updates = Some(light_updates);
                            }
                            sessions.invalidate_prepared_chunks(&committed.block.edit_chunks);
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
    let token = storage.block_mutation_token(request.position)?;
    let expected = request.expected_target;
    let same_block_state_change = previous != expected.state
        && token.chunk_instance_id == expected.token.chunk_instance_id
        && token.last_replacement_version == expected.token.last_replacement_version
        && request.blocks.by_id(previous)?.block.id
            == request.blocks.by_id(expected.state)?.block.id;
    if (!same_block_state_change && (previous != expected.state || token != expected.token))
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
        BlockMutationSnapshot {
            state: previous,
            token,
        },
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
        hook_approval: request.hook_approval.clone(),
        zone_fence: request.zone_fence.clone(),
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
        && plan
            .remainder
            .as_ref()
            .is_none_or(|remainder| remainder.stack.count == 1 && remainder.max_stack > 0)
}

fn valid_animal_feed_plan(plan: &AnimalFeedPlan) -> bool {
    plan.held_slot < 46
        && !plan.expected_held.is_empty()
        && plan.expected_held.item_id == plan.food_item_id
        && !plan.targets.is_empty()
}

fn valid_merchant_trade_plan(plan: &MerchantTradePlan) -> bool {
    plan.offer_index < plan.expected_merchant.offers.len()
        && plan.expected_merchant.validate().is_ok()
        && (1..=64).contains(&plan.cost_a_max_stack)
        && (1..=64).contains(&plan.result_max_stack)
        && plan.expected_merchant_input.as_ref().is_some_and(|inputs| {
            inputs
                .iter()
                .all(|stack| stack.is_empty() || stack.count > 0)
        })
}

fn valid_sheep_shear_plan(plan: &SheepShearPlan) -> bool {
    plan.held_slot < 46
        && plan.expected_held.item_id == plan.shears_item_id
        && plan.expected_held.count == 1
        && plan.shears_max_damage > 0
        && plan.item_entity_type_id >= 0
        && plan.wool_item_ids.iter().all(|item_id| *item_id > 0)
}

fn valid_zombie_villager_cure_plan(plan: &ZombieVillagerCurePlan) -> bool {
    plan.held_slot < 46
        && !plan.expected_held.is_empty()
        && plan.expected_held.item_id == plan.golden_apple_item_id
}

fn valid_player_survival_plan(plan: &PlayerSurvivalPlan) -> bool {
    let survival_is_valid = |state: SurvivalState| {
        state.health.is_finite()
            && state.saturation.is_finite()
            && state.exhaustion.is_finite()
            && (0.0..=mc_entity::player_survival_26_1_2::MAX_HEALTH).contains(&state.health)
            && (0..=mc_entity::player_survival_26_1_2::MAX_FOOD).contains(&state.food)
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
            || plan.keep_inventory
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

fn valid_throwable_item_release_plan(plan: &ThrowableItemReleasePlan) -> bool {
    let held_is_in_hand = plan.held_slot == PlayerInventory::OFFHAND_SLOT
        || (PlayerInventory::HOTBAR_BASE..PlayerInventory::OFFHAND_SLOT).contains(&plan.held_slot);
    held_is_in_hand
        && !plan.expected_held.is_empty()
        && plan.entity_type_id >= 0
        && !plan.entity_type_name.is_empty()
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

fn valid_boat_placement_plan(plan: &BoatPlacementPlan) -> bool {
    let held_is_in_hand = plan.held_slot == PlayerInventory::OFFHAND_SLOT
        || (PlayerInventory::HOTBAR_BASE..PlayerInventory::OFFHAND_SLOT).contains(&plan.held_slot);
    held_is_in_hand
        && !plan.expected_held.is_empty()
        && plan.entity_type_id >= 0
        && !plan.entity_type_name.is_empty()
        && plan.position.x.is_finite()
        && plan.position.y.is_finite()
        && plan.position.z.is_finite()
        && plan.rotation.yaw.is_finite()
        && plan.rotation.pitch.is_finite()
        && plan.rotation.head_yaw.is_finite()
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
#[path = "simulation/tests.rs"]
mod tests;
