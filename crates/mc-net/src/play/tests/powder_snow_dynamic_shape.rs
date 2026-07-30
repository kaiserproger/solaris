use super::{
    BlockStateId, Identifier, ItemRegistry, ItemReport, ItemStack, PlayerInventory, PlayerPose,
    interaction_state_for_blocks, player_pose_collides_with_solid_using_context,
    set_collision_test_block,
};
use mc_data::blocks::solaris_required_blocks_report;
use std::sync::Arc;

#[tokio::test]
async fn powder_snow_dynamic_shape_requires_exact_vanilla_state_identity() {
    let mut reports = solaris_required_blocks_report();
    let powder_snow = reports
        .iter_mut()
        .find(|block| block.id.as_str() == "minecraft:powder_snow")
        .expect("embedded registry contains powder snow");
    let state_id = powder_snow.states[0].id;
    powder_snow
        .properties
        .insert("solaris_test".to_string(), vec!["mismatch".to_string()]);
    powder_snow.states[0]
        .properties
        .insert("solaris_test".to_string(), "mismatch".to_string());
    let blocks = mc_world::BlockRegistry::from_report(&reports)
        .expect("altered powder snow registry retains dense vanilla state ids");
    let mut state = interaction_state_for_blocks(Arc::new(blocks));
    set_collision_test_block(&state, BlockStateId(state_id)).await;

    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));
    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::new(LEATHER_BOOTS_ID, 1);
    assert!(
        player_pose_collides_with_solid_using_context(
            Some(&state),
            PlayerPose::new(0.5, 64.5, 0.5),
            PlayerPose::new(0.5, 64.5, 0.5),
        )
        .await,
        "a fingerprint mismatch must use conservative custom-block fallback"
    );
}
