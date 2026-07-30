use super::{
    BlockEdit, apply_block_edit_batch_to_storage_conditionally, button_and_door_test_registry,
    in_memory_button_world, piston_test_registry, plan_toggle_block_interaction,
    plan_toggle_block_interaction_with_protection,
};
use std::sync::Arc;

#[test]
fn lever_toggle_powers_adjacent_iron_door() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let door_lower = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let door_upper = mc_world::BlockPos {
        y: 65,
        ..door_lower
    };
    for (pos, state_id) in [(lever, 7), (door_lower, 3), (door_upper, 4)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place lever propagation test block");
    }

    let plan = plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(7), 0)
        .expect("lever should power adjacent iron door");

    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(8),
            },
            BlockEdit {
                pos: door_lower,
                new_state: mc_world::BlockStateId(5),
            },
            BlockEdit {
                pos: door_upper,
                new_state: mc_world::BlockStateId(6),
            },
        ]
    );
    assert!(plan.scheduled_block_ticks.is_empty());
}

#[test]
fn lever_extends_one_block_piston_and_retracts_the_head() {
    let blocks = Arc::new(piston_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    for (pos, state_id) in [(lever, 3), (piston, 5), (arm, 8)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place piston test block");
    }

    let extend =
        plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(3), 0)
            .expect("lever should extend adjacent piston");
    assert_eq!(
        extend.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(4),
            },
            BlockEdit {
                pos: piston,
                new_state: mc_world::BlockStateId(6),
            },
            BlockEdit {
                pos: destination,
                new_state: mc_world::BlockStateId(8),
            },
            BlockEdit {
                pos: arm,
                new_state: mc_world::BlockStateId(7),
            },
        ]
    );
    apply_block_edit_batch_to_storage_conditionally(
        &mut world,
        None,
        &extend.edits,
        &extend.preconditions,
    )
    .expect("extension plan remains current");

    let other_lever = mc_world::BlockPos { x: 2, y: 65, z: 1 };
    world
        .set_block_at(other_lever, mc_world::BlockStateId(3))
        .expect("place alternate piston control");
    let stale_retract =
        plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(4), 1)
            .expect("lever should retract adjacent piston");
    assert_eq!(
        stale_retract.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(3),
            },
            BlockEdit {
                pos: piston,
                new_state: mc_world::BlockStateId(5),
            },
            BlockEdit {
                pos: arm,
                new_state: mc_world::BlockStateId(0),
            },
        ]
    );
    world
        .set_block_at(other_lever, mc_world::BlockStateId(4))
        .expect("power alternate piston control");
    assert!(
        apply_block_edit_batch_to_storage_conditionally(
            &mut world,
            None,
            &stale_retract.edits,
            &stale_retract.preconditions,
        )
        .is_none(),
        "alternate power change must stale the retraction"
    );
    assert_eq!(
        world.get_cached_block(piston),
        Some(mc_world::BlockStateId(6))
    );
    world
        .set_block_at(other_lever, mc_world::BlockStateId(3))
        .expect("release alternate piston control");
    let retract =
        plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(4), 2)
            .expect("released alternate control permits retraction");
    apply_block_edit_batch_to_storage_conditionally(
        &mut world,
        None,
        &retract.edits,
        &retract.preconditions,
    )
    .expect("retraction plan remains current");
    assert_eq!(
        world.get_cached_block(destination),
        Some(mc_world::BlockStateId(8))
    );
}

#[test]
fn empty_piston_extends_with_an_occupied_block_two_spaces_ahead() {
    let blocks = Arc::new(piston_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    for (pos, state_id) in [(lever, 3), (piston, 5), (destination, 8)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place empty piston test block");
    }

    let plan = plan_toggle_block_interaction(&blocks, &world, lever, mc_world::BlockStateId(3), 0)
        .expect("empty piston should extend");

    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: lever,
                new_state: mc_world::BlockStateId(4),
            },
            BlockEdit {
                pos: piston,
                new_state: mc_world::BlockStateId(6),
            },
            BlockEdit {
                pos: arm,
                new_state: mc_world::BlockStateId(7),
            },
        ]
    );
}

#[test]
fn protected_piston_destination_rejects_the_atomic_piston_group() {
    let blocks = Arc::new(piston_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lever = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let piston = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let arm = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    let destination = mc_world::BlockPos { x: 4, y: 64, z: 1 };
    for (pos, state_id) in [(lever, 3), (piston, 5), (arm, 8)] {
        world
            .set_block_at(pos, mc_world::BlockStateId(state_id))
            .expect("place protected piston test block");
    }
    let zone = mc_script::ScriptAxisAlignedZone::try_new_with_protection(
        "piston-destination",
        "minecraft:overworld",
        mc_script::ScriptPosition::try_new(4.0, 64.0, 1.0).unwrap(),
        mc_script::ScriptPosition::try_new(4.0, 64.0, 1.0).unwrap(),
        Some(
            mc_script::ScriptZoneProtection::try_actor_or_operator(
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let protection = crate::script::ZoneProtectionSnapshot::from_zones(vec![zone]);

    let plan = plan_toggle_block_interaction_with_protection(
        &blocks,
        &world,
        lever,
        mc_world::BlockStateId(3),
        0,
        Some(&protection),
    )
    .expect("direct lever edit remains valid");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos: lever,
            new_state: mc_world::BlockStateId(4),
        }]
    );
    assert_eq!(
        world.get_cached_block(piston),
        Some(mc_world::BlockStateId(5))
    );
    assert_eq!(world.get_cached_block(arm), Some(mc_world::BlockStateId(8)));
    assert_eq!(
        world.get_cached_block(destination),
        Some(mc_world::BlockStateId(0))
    );
}
