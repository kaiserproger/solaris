use super::{
    Compression, Identifier, ItemRegistry, ItemReport, ItemStack, PlayerInventory, PlayerPose,
    correct_player_collision, set_collision_test_block, vanilla_collision_state_id,
    vanilla_collision_test_state,
};
use std::sync::Arc;

#[tokio::test]
async fn collision_correction_applies_powder_snow_movement_context() {
    let mut state = vanilla_collision_test_state();
    let powder_snow = vanilla_collision_state_id(&state, "minecraft:powder_snow", &[]);
    set_collision_test_block(&state, powder_snow).await;
    const LEATHER_BOOTS_ID: u32 = 1;
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:leather_boots").unwrap(),
        protocol_id: LEATHER_BOOTS_ID,
    }]));
    state.inventory.slots[PlayerInventory::FEET_ARMOR_SLOT] = ItemStack::new(LEATHER_BOOTS_ID, 1);

    let mut writer = Vec::new();
    let mut next_teleport_id = 1;
    let mut pending_teleport = None;
    assert!(
        correct_player_collision(
            Some(&state),
            &mut writer,
            Compression::Disabled,
            PlayerPose::new(0.5, 65.0, 0.5),
            PlayerPose::new(0.5, 64.99, 0.5),
            10,
            &mut next_teleport_id,
            &mut pending_teleport,
        )
        .await
        .unwrap(),
        "leather boots must correct entry through powder snow from above"
    );

    writer.clear();
    pending_teleport = None;
    let mut descending = PlayerPose::new(0.5, 65.0, 0.5);
    descending.shifting = true;
    assert!(
        !correct_player_collision(
            Some(&state),
            &mut writer,
            Compression::Disabled,
            descending,
            PlayerPose::new(0.5, 64.99, 0.5),
            11,
            &mut next_teleport_id,
            &mut pending_teleport,
        )
        .await
        .unwrap(),
        "Shift descent must pass through the correction path"
    );

    let mut falling = PlayerPose::new(0.5, 64.91, 0.5);
    falling.fall_start_y = 68.0;
    let mut landing = PlayerPose::new(0.5, 64.89, 0.5);
    landing.fall_start_y = 68.0;
    assert!(
        correct_player_collision(
            Some(&state),
            &mut writer,
            Compression::Disabled,
            falling,
            landing,
            12,
            &mut next_teleport_id,
            &mut pending_teleport,
        )
        .await
        .unwrap(),
        "a long fall must collide with the 0.9F landing shape"
    );
}
