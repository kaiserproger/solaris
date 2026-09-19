use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_script::precommit::{
    BuildContext, BuildEdit, HookActor, HookContext, HookDecision, HookFailure, HookFailurePolicy,
    HookKind, HookRegistration,
};
use mc_script::{ScriptHostInput, script_boundary_pair};
use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};

use super::*;
use crate::play::simulation::{
    BucketInventoryChange, SurvivalBreakHeldItem, SurvivalPlacementHeldItem,
};
use crate::play::{BlockEdit, BlockEditPrecondition, PlayerPose};

fn blocks() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[
            block_report("minecraft:air", 0),
            block_report("minecraft:stone", 1),
            block_report("minecraft:water", 2),
        ])
        .expect("test block registry"),
    )
}

fn block_report(id: &str, state_id: u32) -> BlockReport {
    BlockReport {
        id: Identifier::parse(id).expect("valid block identifier"),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: state_id,
            default: true,
            properties: BTreeMap::new(),
        }],
    }
}

fn resident_storage() -> (WorldStorage, BlockPos, BlockPos) {
    let mut storage = WorldStorage::in_memory(blocks());
    let chunk = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            chunk,
            Chunk::empty(
                chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").expect("valid biome"),
            ),
        )
        .expect("resident chunk");
    let support = BlockPos { x: 1, y: 64, z: 1 };
    let target = BlockPos { x: 2, ..support };
    storage
        .set_block_at(support, BlockStateId(1))
        .expect("resident support");
    (storage, support, target)
}

fn player_state(item: ItemStack) -> Arc<Mutex<PlayerPersistedState>> {
    let mut state = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = item;
    Arc::new(Mutex::new(state))
}

async fn build_approval(
    decision: HookDecision,
) -> (mc_script::precommit::Approval, mc_script::ScriptBoundary) {
    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(1).expect("non-zero event queue"),
        NonZeroUsize::new(1).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "judge",
            HookKind::Build,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("valid build hook");
    let pending = boundary
        .begin_precommit(HookContext::Build(
            BuildContext::try_new(
                HookActor::Environment,
                "minecraft:overworld",
                vec![BuildEdit::new(2, 64, 1, 0, 1)],
            )
            .expect("valid build context"),
        ))
        .expect("admit build approval");
    let Some(ScriptHostInput::Precommit(request)) = endpoint.recv_input_blocking() else {
        panic!("expected one build precommit request");
    };
    request.answer(decision).expect("answer build approval");
    (
        pending.resolve().await.expect("resolved build approval"),
        boundary,
    )
}

fn placement_plan(
    storage: &WorldStorage,
    support: BlockPos,
    target: BlockPos,
    approval: Option<mc_script::precommit::Approval>,
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
                expected_token: storage.block_mutation_token(target).expect("target token"),
            },
            BlockEditPrecondition {
                pos: support,
                expected_state: BlockStateId(1),
                expected_token: storage
                    .block_mutation_token(support)
                    .expect("support token"),
            },
        ],
        scheduled_block_ticks: Vec::new(),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        held: SurvivalPlacementHeldItem {
            inventory_slot: PlayerInventory::HOTBAR_BASE,
            expected: ItemStack::new(42, 2),
        },
        expected_game_mode: crate::play::GameMode::Survival,
        hook_approval: approval,
        zone_fence: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_regional_placement_preserves_block_and_inventory() {
    let (storage, support, target) = resident_storage();
    let mutation = storage.mutation_view();
    let player = player_state(ItemStack::new(42, 2));
    let (approval, _) = build_approval(HookDecision::Cancel).await;
    let plan = placement_plan(&storage, support, target, Some(approval));

    let result = SurvivalPlacementTransaction {
        player_state: Arc::clone(&player),
    }
    .commit(&mutation, None, 1, &plan);

    assert!(matches!(
        result,
        Err(SimulationRequestError::Precommit(HookFailure::Cancelled))
    ));
    assert_eq!(storage.get_cached_block(target), Some(BlockStateId(0)));
    assert_eq!(
        player.lock().expect("player state").inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stale_regional_break_approval_preserves_block_and_inventory() {
    let (mut storage, _support, target) = resident_storage();
    storage
        .set_block_at(target, BlockStateId(1))
        .expect("resident target");
    let mutation = storage.mutation_view();
    let player = player_state(ItemStack::new(42, 1));
    let (approval, boundary) = build_approval(HookDecision::Keep).await;
    boundary
        .set_precommit_hooks(Vec::new())
        .expect("retire approval generation");
    let plan = SurvivalBreakPlan {
        edits: vec![BlockEdit {
            pos: target,
            new_state: BlockStateId(0),
        }],
        preconditions: vec![BlockEditPrecondition {
            pos: target,
            expected_state: BlockStateId(1),
            expected_token: storage.block_mutation_token(target).expect("target token"),
        }],
        blocks: blocks(),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        falling_block_entity_type_id: None,
        held: SurvivalBreakHeldItem {
            hotbar_slot: 0,
            expected: ItemStack::new(42, 1),
            max_damage: Some(10),
        },
        drops: Vec::new(),
        zone_fence: None,
        hook_approval: Some(approval),
    };

    let result = SurvivalBreakTransaction {
        player_state: Arc::clone(&player),
    }
    .commit(&mutation, None, 1, &plan);

    assert!(matches!(
        result,
        Err(SimulationRequestError::Precommit(HookFailure::Stale))
    ));
    assert_eq!(storage.get_cached_block(target), Some(BlockStateId(1)));
    assert_eq!(
        player.lock().expect("player state").inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn consumed_regional_bucket_approval_preserves_block_and_inventory() {
    let (storage, _support, target) = resident_storage();
    let mutation = storage.mutation_view();
    let player = player_state(ItemStack::new(61, 1));
    let (approval, _) = build_approval(HookDecision::Keep).await;
    let mut prior_consumer = approval.clone();
    assert_eq!(
        prior_consumer.consume().expect("first approval consumer"),
        HookDecision::Keep
    );
    let plan = BucketUsePlan {
        edit: BlockEdit {
            pos: target,
            new_state: BlockStateId(2),
        },
        precondition: BlockEditPrecondition {
            pos: target,
            expected_state: BlockStateId(0),
            expected_token: storage.block_mutation_token(target).expect("target token"),
        },
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        inventory: Some(BucketInventoryChange {
            held_slot: PlayerInventory::HOTBAR_BASE,
            expected_held: ItemStack::new(61, 1),
            replacement_item: 60,
            replacement_max_stack: 16,
        }),
        schedule_fluid_ticks: false,
        zone_fence: None,
        hook_approval: Some(approval),
    };

    let result = BucketUseTransaction {
        player_state: Arc::clone(&player),
    }
    .commit(&mutation, None, 1, &plan);

    assert!(matches!(
        result,
        Err(SimulationRequestError::Precommit(HookFailure::Stale))
    ));
    assert_eq!(storage.get_cached_block(target), Some(BlockStateId(0)));
    assert_eq!(
        player.lock().expect("player state").inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(61, 1)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn kept_regional_placement_updates_block_and_inventory() {
    let (storage, support, target) = resident_storage();
    let mutation = storage.mutation_view();
    let player = player_state(ItemStack::new(42, 2));
    let (approval, _) = build_approval(HookDecision::Keep).await;
    let plan = placement_plan(&storage, support, target, Some(approval));

    let result = SurvivalPlacementTransaction {
        player_state: Arc::clone(&player),
    }
    .commit(&mutation, None, 1, &plan);

    assert!(matches!(result, Ok(Some(_))));
    assert_eq!(storage.get_cached_block(target), Some(BlockStateId(1)));
    assert_eq!(
        player.lock().expect("player state").inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
}
