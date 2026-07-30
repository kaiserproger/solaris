use super::{
    Identifier, ItemRegistry, ItemReport, ItemStack, PlayerInventory, PlayerPose,
    player_pose_collides_with_solid_using_context, set_collision_test_block,
    vanilla_collision_state_id, vanilla_collision_test_state,
};
use std::sync::Arc;

#[tokio::test]
async fn powder_snow_collision_uses_player_equipment_and_movement_context() {
    let mut state = vanilla_collision_test_state();
    let powder_snow = vanilla_collision_state_id(&state, "minecraft:powder_snow", &[]);
    set_collision_test_block(&state, powder_snow).await;

    let above = PlayerPose::new(0.5, 65.0, 0.5);
    let entering = PlayerPose::new(0.5, 64.99, 0.5);
    assert!(
        !player_pose_collides_with_solid_using_context(Some(&state), entering, above).await,
        "a player without leather boots sinks into powder snow"
    );

    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));
    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::new(LEATHER_BOOTS_ID, 1);
    assert!(
        player_pose_collides_with_solid_using_context(Some(&state), entering, above).await,
        "leather boots support a player entering powder snow from above"
    );

    let mut descending = above;
    descending.shifting = true;
    assert!(
        !player_pose_collides_with_solid_using_context(Some(&state), entering, descending).await,
        "holding Shift lets a leather-booted player descend through powder snow"
    );
    assert!(
        !player_pose_collides_with_solid_using_context(
            Some(&state),
            PlayerPose::new(0.5, 64.4, 0.5),
            PlayerPose::new(0.5, 64.5, 0.5),
        )
        .await,
        "boots do not turn powder snow solid after the player is already inside it"
    );
}
