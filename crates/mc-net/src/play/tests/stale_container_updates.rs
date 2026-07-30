use super::{
    ActiveContainer, ChestWindow, FurnaceKind, FurnaceWindow, active_chest_window_at,
    active_furnace_window_at,
};

#[test]
fn stale_container_updates_do_not_discard_another_open_container() {
    let furnace_pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let chest_pos = mc_world::BlockPos { x: 2, y: 64, z: 2 };
    let mut active = Some(ActiveContainer::Chest(ChestWindow::new(vec![chest_pos], 7)));

    assert!(active_furnace_window_at(&mut active, furnace_pos).is_none());
    assert!(matches!(
        active,
        Some(ActiveContainer::Chest(ChestWindow {
            container_id: 7,
            ..
        }))
    ));

    active = Some(ActiveContainer::Furnace(FurnaceWindow::new(
        furnace_pos,
        8,
        FurnaceKind::Furnace,
    )));
    assert!(active_chest_window_at(&mut active, chest_pos).is_none());
    assert!(matches!(
        active,
        Some(ActiveContainer::Furnace(FurnaceWindow {
            container_id: 8,
            ..
        }))
    ));
}
