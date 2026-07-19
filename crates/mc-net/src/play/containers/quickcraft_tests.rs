use super::{
    QUICKCRAFT_HEADER_CONTINUE, QUICKCRAFT_HEADER_END, QUICKCRAFT_HEADER_START,
    QUICKCRAFT_TYPE_CHARITABLE, QUICKCRAFT_TYPE_GREEDY, QuickCraftClick, QuickCraftState,
    QuickCraftStep, quickcraft_distribution_count,
};

#[test]
fn quickcraft_tracks_pending_slots_until_end() {
    let mut quickcraft = QuickCraftState::default();

    assert_eq!(
        quickcraft.advance(
            false,
            QuickCraftClick {
                header: QUICKCRAFT_HEADER_START,
                kind: QUICKCRAFT_TYPE_GREEDY,
                slot: None,
            },
        ),
        QuickCraftStep::Started
    );
    assert_eq!(
        quickcraft.advance(
            false,
            QuickCraftClick {
                header: QUICKCRAFT_HEADER_CONTINUE,
                kind: QUICKCRAFT_TYPE_GREEDY,
                slot: Some(4),
            },
        ),
        QuickCraftStep::Continued { slot: Some(4) }
    );
    quickcraft.add_slot(4);
    quickcraft.add_slot(4);
    assert_eq!(quickcraft.selected_slot_count(), 1);
    assert_eq!(
        quickcraft.advance(
            false,
            QuickCraftClick {
                header: QUICKCRAFT_HEADER_END,
                kind: QUICKCRAFT_TYPE_GREEDY,
                slot: None,
            },
        ),
        QuickCraftStep::Finished
    );

    let selection = quickcraft.finish();
    assert_eq!(selection.kind, QUICKCRAFT_TYPE_GREEDY);
    assert_eq!(selection.slots, vec![4]);
    assert_eq!(quickcraft.selected_slot_count(), 0);
}

#[test]
fn quickcraft_rejects_an_invalid_start_kind() {
    let mut quickcraft = QuickCraftState::default();

    assert_eq!(
        quickcraft.advance(
            false,
            QuickCraftClick {
                header: QUICKCRAFT_HEADER_START,
                kind: 2,
                slot: None,
            },
        ),
        QuickCraftStep::Rejected
    );
}

#[test]
fn quickcraft_resets_when_the_cursor_empties_mid_drag() {
    let mut quickcraft = QuickCraftState::default();

    assert_eq!(
        quickcraft.advance(
            false,
            QuickCraftClick {
                header: QUICKCRAFT_HEADER_START,
                kind: QUICKCRAFT_TYPE_CHARITABLE,
                slot: None,
            },
        ),
        QuickCraftStep::Started
    );
    assert_eq!(
        quickcraft.advance(
            true,
            QuickCraftClick {
                header: QUICKCRAFT_HEADER_CONTINUE,
                kind: QUICKCRAFT_TYPE_CHARITABLE,
                slot: Some(4),
            },
        ),
        QuickCraftStep::Rejected
    );
    assert_eq!(
        quickcraft.advance(
            false,
            QuickCraftClick {
                header: QUICKCRAFT_HEADER_CONTINUE,
                kind: QUICKCRAFT_TYPE_CHARITABLE,
                slot: Some(4),
            },
        ),
        QuickCraftStep::Rejected
    );
}

#[test]
fn quickcraft_distribution_matches_drag_modes() {
    assert_eq!(
        quickcraft_distribution_count(10, 3, QUICKCRAFT_TYPE_CHARITABLE),
        3
    );
    assert_eq!(
        quickcraft_distribution_count(10, 3, QUICKCRAFT_TYPE_GREEDY),
        1
    );
    assert_eq!(
        quickcraft_distribution_count(10, 0, QUICKCRAFT_TYPE_CHARITABLE),
        0
    );
}
