use super::super::{
    ContainerClickAction, ContainerInput, ServerboundContainerClick, classify_container_click,
};

#[test]
fn only_vanilla_outside_slot_sentinel_can_drop_the_cursor() {
    let click = |slot_num| ServerboundContainerClick {
        container_id: 0,
        state_id: 1,
        slot_num,
        button_num: 0,
        container_input: ContainerInput::Pickup,
        changed_slots: Vec::new(),
        carried_item: mc_protocol::packets::play::HashedStack::empty(),
    };

    assert!(matches!(
        classify_container_click(&click(-999)),
        ContainerClickAction::OutsidePickup { button: 0 }
    ));
    for malformed in [-1, -2, i16::MIN] {
        assert!(matches!(
            classify_container_click(&click(malformed)),
            ContainerClickAction::Unsupported
        ));
    }
}
