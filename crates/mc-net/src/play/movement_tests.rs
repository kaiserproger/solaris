use std::sync::Arc;

use mc_data::{
    Identifier,
    blocks::solaris_required_blocks_report,
    items::{ItemRegistry, ItemReport},
};
use mc_protocol::packets::play::ItemStack;
use mc_world::{BlockPos, BlockStateId};

use super::inventory::PlayerInventory;
use super::tests::{
    fluid_test_facts, fluid_test_registry, insert_fluid_test_chunk, interaction_state_for_blocks,
};
use super::{
    PlayerPose, player_pose_collides_with_solid, player_pose_collides_with_solid_using_context,
    player_water_overlap,
};
use crate::play::chunk_stream::passable_block_name;

#[test]
fn water_plants_do_not_become_full_cube_collision_fallbacks() {
    for name in [
        "minecraft:kelp",
        "minecraft:kelp_plant",
        "minecraft:seagrass",
        "minecraft:tall_seagrass",
        "minecraft:bubble_column",
    ] {
        assert!(passable_block_name(name), "{name} must be passable");
    }
}

#[test]
fn torches_do_not_become_full_cube_collision_fallbacks() {
    for name in [
        "minecraft:torch",
        "minecraft:wall_torch",
        "minecraft:soul_torch",
        "minecraft:soul_wall_torch",
        "minecraft:redstone_torch",
        "minecraft:redstone_wall_torch",
    ] {
        assert!(passable_block_name(name), "{name} must be passable");
    }
}

#[tokio::test]
async fn swimming_pose_submerges_eyes_in_one_block_of_water() {
    let mut state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    state.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&state).await;
    state
        .world
        .lock()
        .await
        .set_block_at(BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(2))
        .unwrap();

    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.swimming = true;

    assert_eq!(player_water_overlap(&state, pose).await, (true, true));
}

#[tokio::test]
async fn representative_player_geometry_boundary_matrix() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let mut state = interaction_state_for_blocks(blocks);
    insert_fluid_test_chunk(&state).await;
    let block_state = |name: &str| {
        state
            .blocks
            .block(&Identifier::parse(name).expect("valid movement matrix block"))
            .unwrap_or_else(|| panic!("missing movement matrix block {name}"))
            .default
    };

    state
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 0, y: 64, z: 0 },
            block_state("minecraft:stone"),
        )
        .expect("seed movement matrix ceiling");

    struct PoseCase {
        name: &'static str,
        shifting: bool,
        swimming: bool,
        body_height: f64,
        eye_height: f64,
        collides_at_ceiling_edge: bool,
    }
    for case in [
        PoseCase {
            name: "standing",
            shifting: false,
            swimming: false,
            body_height: 1.8,
            eye_height: 1.62,
            collides_at_ceiling_edge: true,
        },
        PoseCase {
            name: "crouching",
            shifting: true,
            swimming: false,
            body_height: 1.5,
            eye_height: 1.27,
            collides_at_ceiling_edge: false,
        },
        PoseCase {
            name: "swimming",
            shifting: false,
            swimming: true,
            body_height: 0.6,
            eye_height: 0.4,
            collides_at_ceiling_edge: false,
        },
    ] {
        let mut pose = PlayerPose::new(0.5, 62.21, 0.5);
        pose.shifting = case.shifting;
        pose.swimming = case.swimming;
        assert_eq!(pose.body_height(), case.body_height, "{} body", case.name);
        assert_eq!(pose.eye_height(), case.eye_height, "{} eyes", case.name);
        assert_eq!(
            player_pose_collides_with_solid(Some(&state), pose).await,
            case.collides_at_ceiling_edge,
            "{} ceiling collision boundary",
            case.name
        );
    }

    state
        .world
        .lock()
        .await
        .set_block_at(
            BlockPos { x: 0, y: 64, z: 0 },
            block_state("minecraft:powder_snow"),
        )
        .expect("seed movement matrix powder snow");
    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));

    struct PowderCase {
        name: &'static str,
        leather_boots: bool,
        shifting: bool,
        expected_collision: bool,
    }
    for case in [
        PowderCase {
            name: "no boots sinks",
            leather_boots: false,
            shifting: false,
            expected_collision: false,
        },
        PowderCase {
            name: "leather boots stand from above",
            leather_boots: true,
            shifting: false,
            expected_collision: true,
        },
        PowderCase {
            name: "Shift descends through boots support",
            leather_boots: true,
            shifting: true,
            expected_collision: false,
        },
    ] {
        state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = if case.leather_boots {
            ItemStack::new(LEATHER_BOOTS_ID, 1)
        } else {
            ItemStack::EMPTY
        };
        let mut previous = PlayerPose::new(0.5, 65.0, 0.5);
        previous.shifting = case.shifting;
        assert_eq!(
            player_pose_collides_with_solid_using_context(
                Some(&state),
                PlayerPose::new(0.5, 64.99, 0.5),
                previous,
            )
            .await,
            case.expected_collision,
            "{}",
            case.name
        );
    }

    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::EMPTY;
    for (name, y, expected_collision) in [
        ("exact falling-shape top", 64.9, false),
        ("inside falling shape", 64.89, true),
    ] {
        let mut pose = PlayerPose::new(0.5, y, 0.5);
        pose.fall_start_y = 68.0;
        assert_eq!(
            player_pose_collides_with_solid(Some(&state), pose).await,
            expected_collision,
            "{name}"
        );
    }
}
