use std::sync::Arc;

use super::{BlockEdit, in_memory_button_world, plan_toggle_block_interaction, real_door_state};

#[test]
#[ignore = "explicit local 26.1.2 blocks sidecar parity gate"]
fn real_door_states_plan_hand_toggle_when_sidecar_is_present() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let blocks_json = manifest.join("../../data/vanilla/reports/blocks.json");
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("registry builds"));
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let lower = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let upper = mc_world::BlockPos { y: 65, ..lower };
    let oak_lower = real_door_state(&blocks, "minecraft:oak_door", "lower", false);
    let oak_upper = real_door_state(&blocks, "minecraft:oak_door", "upper", false);
    let oak_open = real_door_state(&blocks, "minecraft:oak_door", "lower", true);
    let oak_upper_open = real_door_state(&blocks, "minecraft:oak_door", "upper", true);
    world
        .set_block_at(lower, oak_lower)
        .expect("set lower")
        .expect("chunk exists");
    world
        .set_block_at(upper, oak_upper)
        .expect("set upper")
        .expect("chunk exists");

    let plan = plan_toggle_block_interaction(&blocks, &world, lower, oak_lower, 0)
        .expect("real oak door should hand-toggle");

    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: lower,
                new_state: oak_open,
            },
            BlockEdit {
                pos: upper,
                new_state: oak_upper_open,
            },
        ]
    );
}
