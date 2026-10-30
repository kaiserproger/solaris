//! Outgoing-only names never overwrite native CustomName or dirty saves.
use super::{Activity, VillagerProfession, conversion_tests::state};
use crate::boundary::{EntityAppearance, EntityId};
use crate::tick::TickState;
use std::sync::Arc;

fn villager(state: &mut TickState, kind: &str) -> (EntityId, crate::entities::EntitySlot) {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(kind).unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, [0.0, 80.0, 0.0], 20.0);
    (id, state.entities.slot_of(id).unwrap())
}

fn appearance(state: &mut TickState, id: EntityId) -> Arc<EntityAppearance> {
    Arc::clone(&state.entities.views().get(id).unwrap().appearance)
}

#[test]
fn child_nameplate_changes_when_growing_up() {
    let mut state = state();
    let (id, slot) = villager(&mut state, "minecraft:villager");
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = -1;
    let child = appearance(&mut state, id);
    assert!(child.custom_name.as_deref().unwrap().contains(" · ребёнок · "));
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = 0;
    let adult = appearance(&mut state, id);
    assert!(adult.custom_name.as_deref().unwrap().contains(" · безработный · "));
}

#[test]
fn uuid_name_profession_and_activity_are_visible_but_not_saved() {
    let mut state = state();
    let (id, slot) = villager(&mut state, "minecraft:villager");
    let uuid = state.entities.row(slot).uuid;
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = VillagerProfession::Farmer;
    v.activity = Activity::Work;
    super::sync_appearance(&mut state, slot);
    let projected = appearance(&mut state, id);
    assert_eq!(
        projected.custom_name.as_deref(),
        Some(
            format!(
                "{} · фермер · работа",
                crate::settlements::presentation::villager_name(uuid)
            )
            .as_str()
        )
    );
    assert!(projected.custom_name_visible);
    assert!(state.entities.row(slot).appearance.custom_name.is_none());
    let saved = crate::journal::EntityAppearanceImage::from(&*state.entities.row(slot).appearance);
    assert!(
        saved.custom_name.is_none(),
        "generated names must not enter native WAL/checkpoint images"
    );
    assert!(!saved.custom_name_visible);
}

#[test]
fn brain_activity_sleeping_and_profession_changes_refresh_outgoing_plate() {
    let mut state = state();
    let (id, slot) = villager(&mut state, "minecraft:villager");
    let idle = appearance(&mut state, id);
    for (activity, sleeping, expected) in [
        (Activity::Work, false, "работа"),
        (Activity::Rest, false, "отдых"),
        (Activity::Meet, false, "встреча"),
        (Activity::Idle, false, "прогулка"),
        (Activity::Play, false, "игра"),
        (Activity::Panic, false, "паника"),
        (Activity::Rest, true, "сон"),
    ] {
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        v.activity = activity;
        v.sleeping = sleeping;
        v.data.profession = VillagerProfession::Librarian;
        // Activity changes alone must project, without mutating saved appearance.
        let projected = appearance(&mut state, id);
        assert!(
            projected
                .custom_name
                .as_deref()
                .unwrap()
                .ends_with(&format!(" · библиотекарь · {expected}"))
        );
        assert!(!Arc::ptr_eq(&idle, &projected));
        assert!(state.entities.row(slot).appearance.custom_name.is_none());
    }
}

#[test]
fn unchanged_motion_reuses_nameplate_arc_and_does_not_dirty_native_state() {
    let mut state = state();
    let (id, slot) = villager(&mut state, "minecraft:villager");
    let initial = appearance(&mut state, id);
    state.entities.take_touched();
    state.appearance_changed.clear();
    super::sync_appearance(&mut state, slot);
    assert!(state.entities.touched_ids().is_empty());
    assert!(state.appearance_changed.is_empty());
    assert!(Arc::ptr_eq(&initial, &appearance(&mut state, id)));
    state.entities.set_pos(slot, [1.0, 80.0, 0.0]);
    let touched_before = state.entities.touched_ids().to_vec();
    let moved = appearance(&mut state, id);
    assert!(
        Arc::ptr_eq(&initial, &moved),
        "movement must not reformat or allocate a plate"
    );
    assert_eq!(state.entities.touched_ids(), touched_before);
}

#[test]
fn real_custom_name_including_generated_prefix_is_preserved() {
    let mut state = state();
    let (id, slot) = villager(&mut state, "minecraft:villager");
    let real_name = format!(
        "{} · мой житель",
        crate::settlements::presentation::villager_name(state.entities.row(slot).uuid)
    );
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name =
        Some(real_name.clone());
    let before = (*state.entities.row(slot).appearance).clone();
    assert_eq!(
        appearance(&mut state, id).custom_name.as_deref(),
        Some(real_name.as_str())
    );
    assert_eq!(*state.entities.row(slot).appearance, before);
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .activity = Activity::Panic;
    assert_eq!(
        appearance(&mut state, id).custom_name.as_deref(),
        Some(real_name.as_str())
    );
    let saved = crate::journal::EntityAppearanceImage::from(&*state.entities.row(slot).appearance);
    assert_eq!(saved.custom_name.as_deref(), Some(real_name.as_str()));
}

#[test]
fn wandering_trader_has_plate_without_resident_subtype_metadata() {
    let mut state = state();
    let (id, slot) = villager(&mut state, "minecraft:wandering_trader");
    let projected = appearance(&mut state, id);
    assert!(projected.custom_name.is_some());
    assert!(projected.custom_name_visible);
    assert!(
        !projected.extra.iter().any(|(_, value)| matches!(
            value,
            crate::boundary::EntityDataValue::VillagerData { .. }
        ))
    );
    assert!(state.entities.row(slot).appearance.custom_name.is_none());
}
