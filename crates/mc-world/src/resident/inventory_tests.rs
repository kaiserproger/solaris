use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::blocks::{BlockReport, BlockStateReport};

use super::*;
use crate::storage::WorldStorage;

struct Fixture {
    world: WorldStorage,
    changes: Vec<ResidentBlockEntityChange<ChestBlockEntity>>,
    preconditions: Vec<ResidentBlockPrecondition>,
}

fn fixture() -> Fixture {
    let registry = Arc::new(
        BlockRegistry::from_report(
            &["minecraft:air", "minecraft:chest"]
                .into_iter()
                .enumerate()
                .map(|(id, name)| BlockReport {
                    id: Identifier::parse(name).unwrap(),
                    properties: BTreeMap::new(),
                    states: vec![BlockStateReport {
                        id: id as u32,
                        default: true,
                        properties: BTreeMap::new(),
                    }],
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    );
    let mut world = WorldStorage::in_memory(registry);
    let mut changes = Vec::new();
    let mut preconditions = Vec::new();
    for (index, x) in [127, 128].into_iter().enumerate() {
        let position = BlockPos { x, y: 2, z: 3 };
        let chunk = chunk_pos_of(position);
        world
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        world.set_block_at(position, BlockStateId(1)).unwrap();
        let mut expected = ChestBlockEntity::default();
        expected.slots[0].item_id = 7;
        expected.slots[0].count = if index == 0 { 3 } else { 0 };
        let mut updated = expected.clone();
        updated.slots[0].count = if index == 0 { 0 } else { 3 };
        world
            .set_chest_block_entity(position, expected.clone())
            .unwrap();
        preconditions.push(ResidentBlockPrecondition {
            pos: position,
            expected_state: BlockStateId(1),
            expected_token: world.block_mutation_token(position).unwrap(),
        });
        changes.push(ResidentBlockEntityChange {
            position,
            expected,
            updated,
        });
    }
    Fixture {
        world,
        changes,
        preconditions,
    }
}

#[test]
fn chest_inventory_durable_commit_publishes_all_regions_only_after_success() {
    let mut fixture = fixture();
    let mutation = fixture.world.mutation_view();
    let rejected = mutation
        .prepare_chest_inventory_transaction(&fixture.changes, &fixture.preconditions, 7)
        .unwrap_or_else(|_| panic!("valid chest transaction"));
    assert!(matches!(
        rejected.commit_durably_classified(|_| Err("not written"), |_| false,),
        ResidentCrossRegionScheduledBlockTickCommitResult::DurabilityFailed(_)
    ));
    for change in &fixture.changes {
        assert_eq!(
            fixture.world.chest_block_entity(change.position).unwrap(),
            Some(change.expected.clone())
        );
    }
    let transaction = mutation
        .prepare_chest_inventory_transaction(&fixture.changes, &fixture.preconditions, 7)
        .unwrap_or_else(|_| panic!("known failure preserves reusable sources"));
    assert!(matches!(
        transaction.commit_durably_classified(|_| Ok::<_, ()>(()), |_| false,),
        ResidentCrossRegionScheduledBlockTickCommitResult::Applied(_)
    ));
    for change in &fixture.changes {
        assert_eq!(
            fixture.world.chest_block_entity(change.position).unwrap(),
            Some(change.updated.clone())
        );
    }
}

#[test]
fn chest_inventory_stale_source_rejects_before_persistence() {
    let mut fixture = fixture();
    let mutation = fixture.world.mutation_view();
    let transaction = mutation
        .prepare_chest_inventory_transaction(&fixture.changes, &fixture.preconditions, 7)
        .unwrap_or_else(|_| panic!("valid chest transaction"));
    let mut changed = fixture.changes[1].expected.clone();
    changed.slots[0].count = 4;
    fixture
        .world
        .set_chest_block_entity(fixture.changes[1].position, changed.clone())
        .unwrap();
    assert!(matches!(
        transaction.commit_durably_classified(
            |_| -> Result<(), ()> { panic!("stale transaction cannot persist") },
            |_| false,
        ),
        ResidentCrossRegionScheduledBlockTickCommitResult::Stale
    ));
    assert_eq!(
        fixture
            .world
            .chest_block_entity(fixture.changes[0].position)
            .unwrap(),
        Some(fixture.changes[0].expected.clone())
    );
    assert_eq!(
        fixture
            .world
            .chest_block_entity(fixture.changes[1].position)
            .unwrap(),
        Some(changed)
    );
}

#[test]
fn chest_inventory_unknown_commit_fail_stops_native_world_mutations() {
    let fixture = fixture();
    let mutation = fixture.world.mutation_view();
    let transaction = mutation
        .prepare_chest_inventory_transaction(&fixture.changes, &fixture.preconditions, 7)
        .unwrap_or_else(|_| panic!("valid chest transaction"));
    assert!(matches!(
        transaction.commit_durably_classified(|_| Err("sync outcome unknown"), |_| true,),
        ResidentCrossRegionScheduledBlockTickCommitResult::DurabilityFailed(_)
    ));
    let position = fixture.changes[0].position;
    let native = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mutation.commit_chests_conditionally(
            &[position],
            &[fixture.changes[0].expected.clone()],
            &[ChestBlockEntity::default()],
        )
    }));
    assert!(
        native.is_err(),
        "native writes cannot follow an uncertain inventory commit"
    );
}

#[test]
fn chest_inventory_accepts_lazy_empty_chests_but_not_removed_containers() {
    let mut fixture = fixture();
    let destination = fixture.changes[1].position;
    fixture
        .world
        .set_block_at(destination, BlockStateId(0))
        .unwrap();
    fixture
        .world
        .set_block_at(destination, BlockStateId(1))
        .unwrap();
    fixture.changes[1].expected = ChestBlockEntity::default();
    fixture.preconditions[1].expected_token =
        fixture.world.block_mutation_token(destination).unwrap();
    let mutation = fixture.world.mutation_view();
    let transaction = mutation
        .prepare_chest_inventory_transaction(&fixture.changes, &fixture.preconditions, 7)
        .unwrap_or_else(|_| panic!("an empty physical chest need not have materialized contents"));
    assert!(matches!(
        transaction.commit_durably_classified(|_| Ok::<_, ()>(()), |_| false,),
        ResidentCrossRegionScheduledBlockTickCommitResult::Applied(_)
    ));
    assert_eq!(
        fixture.world.chest_block_entity(destination).unwrap(),
        Some(fixture.changes[1].updated.clone())
    );

    fixture
        .world
        .set_block_at(destination, BlockStateId(0))
        .unwrap();
    fixture.preconditions[1].expected_state = BlockStateId(0);
    fixture.preconditions[1].expected_token =
        fixture.world.block_mutation_token(destination).unwrap();
    assert!(matches!(
        mutation.prepare_chest_inventory_transaction(
            &fixture.changes[1..],
            &fixture.preconditions[1..],
            8,
        ),
        Err(ResidentBlockEditBatchResult::Stale)
    ));
}
