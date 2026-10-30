//! Sibling regression gates for the availability classifier.
//!
//! Deterministic source-only fixtures (`&[]` stock images): shepherd presence,
//! sheep color/radius/age gating, positive real-sheep wool, and
//! representative supported/unsupported raws, plus the recipe-expansion
//! diagnostic (bed reports wool, not planks, when wood is covered).

use super::{SourceReason, can_produce, missing_explanation, missing_source};
use crate::entities::EntitySlot;
use crate::settlements::SettlementId;
use crate::settlements::allocator::tests::{test_state, villager};
use crate::settlements::buildings::{Building, BuildingId, BuildingKind};
use crate::tick::TickState;
use crate::villager::{Activity, VillagerProfession};
use mc_script::ScriptWorldBounds;

fn state() -> TickState {
    test_state()
}

fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}

fn settle(state: &mut TickState) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "source".into(),
            "owner".into(),
            [8, 201, 8],
            48,
        )
        .unwrap()
}

fn worker(state: &mut TickState, id: SettlementId, profession: VillagerProfession) -> EntitySlot {
    let slot = villager(state, [8.5, 201.0, 8.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(state, id, entity).unwrap();
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = profession;
    v.activity = Activity::Work;
    slot
}

fn certify(state: &mut TickState, id: SettlementId, kind: BuildingKind) {
    let record = state.settlements.get_mut(id).unwrap();
    let bid = BuildingId((id.0 << 32) | (record.building_sequence + 1));
    record.building_sequence += 1;
    record.buildings.insert(
        bid,
        Building {
            id: bid,
            settlement: id,
            kind,
            level: 1,
            bounds: ScriptWorldBounds::new([1, 200, 1], [5, 204, 5]).unwrap(),
            work_points: vec![[3, 201, 3]],
            revision: 1,
            natural_zone: matches!(kind, BuildingKind::Mine | BuildingKind::LumberCamp),
        },
    );
}

fn sheep(state: &mut TickState, pos: [f64; 3], wool: u8) -> EntitySlot {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 8.0);
    let slot = state.entities.slot_of(id).unwrap();
    crate::tick::set_wool(
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
        wool,
    );
    slot
}

fn shepherd_town(state: &mut TickState) -> SettlementId {
    let id = settle(state);
    worker(state, id, VillagerProfession::Shepherd);
    id
}

#[test]
fn no_shepherd_means_no_wool() {
    let mut state = state();
    let id = settle(&mut state);
    sheep(&mut state, [8.5, 201.0, 8.5], 0);
    let wool = item(&state, "minecraft:white_wool");
    assert!(!can_produce(&state, id, wool, &[]));
    assert_eq!(missing_source(&state, id, wool, &[]), Some(wool));
}

#[test]
fn no_sheep_means_no_wool() {
    let mut state = state();
    let id = settle(&mut state);
    worker(&mut state, id, VillagerProfession::Shepherd);
    let wool = item(&state, "minecraft:white_wool");
    assert!(!can_produce(&state, id, wool, &[]));
    assert_eq!(missing_source(&state, id, wool, &[]), Some(wool));
}

#[test]
fn white_sheep_supplies_white_wool_but_not_red() {
    let mut state = state();
    let id = shepherd_town(&mut state);
    sheep(&mut state, [8.5, 201.0, 8.5], 0);
    let white = item(&state, "minecraft:white_wool");
    let red = item(&state, "minecraft:red_wool");
    assert!(can_produce(&state, id, white, &[]));
    assert_eq!(missing_source(&state, id, white, &[]), None);
    assert!(!can_produce(&state, id, red, &[]));
    assert_eq!(missing_source(&state, id, red, &[]), Some(red));
}

#[test]
fn colored_sheep_supplies_matching_wool() {
    let mut state = state();
    let id = shepherd_town(&mut state);
    sheep(&mut state, [8.5, 201.0, 8.5], 14);
    let red = item(&state, "minecraft:red_wool");
    let white = item(&state, "minecraft:white_wool");
    assert!(can_produce(&state, id, red, &[]));
    assert_eq!(missing_source(&state, id, red, &[]), None);
    assert_eq!(missing_source(&state, id, white, &[]), Some(white));
}

#[test]
fn sheep_outside_radius_is_no_source() {
    let mut state = state();
    let id = shepherd_town(&mut state);
    // Radius is 48 around [8, 201, 8]; +100 blocks out is well outside.
    sheep(&mut state, [108.5, 201.0, 8.5], 0);
    let wool = item(&state, "minecraft:white_wool");
    assert!(!can_produce(&state, id, wool, &[]));
    assert_eq!(missing_source(&state, id, wool, &[]), Some(wool));
}

#[test]
fn sheared_white_sheep_remains_a_source() {
    let mut state = state();
    let id = shepherd_town(&mut state);
    sheep(&mut state, [8.5, 201.0, 8.5], 0x10);
    let wool = item(&state, "minecraft:white_wool");
    assert!(can_produce(&state, id, wool, &[]));
    assert_eq!(missing_source(&state, id, wool, &[]), None);
}

#[test]
fn baby_sheep_is_no_source() {
    let mut state = state();
    let id = shepherd_town(&mut state);
    let lamb = sheep(&mut state, [8.5, 201.0, 8.5], 0);
    state.entities.row_mut(lamb).entity_life.child_ticks = 200;
    let wool = item(&state, "minecraft:white_wool");
    assert!(!can_produce(&state, id, wool, &[]));
    assert_eq!(missing_source(&state, id, wool, &[]), Some(wool));
}

#[test]
fn miner_plus_mine_supplies_cobble_but_lumber_needs_camp() {
    let mut state = state();
    let id = settle(&mut state);
    worker(&mut state, id, VillagerProfession::Miner);
    certify(&mut state, id, BuildingKind::Mine);
    let cobble = item(&state, "minecraft:cobblestone");
    let log = item(&state, "minecraft:oak_log");
    assert!(can_produce(&state, id, cobble, &[]));
    assert_eq!(missing_source(&state, id, cobble, &[]), None);
    assert!(!can_produce(&state, id, log, &[]));
    assert_eq!(missing_source(&state, id, log, &[]), Some(log));
}

#[test]
fn missing_explanation_reports_the_same_leaf_and_its_missing_capability() {
    let mut state = state();
    let id = settle(&mut state);
    let cobble = item(&state, "minecraft:cobblestone");
    let log = item(&state, "minecraft:oak_log");
    assert_eq!(missing_explanation(&state, id, cobble, &[]), Some((cobble, SourceReason::Mine)));
    assert_eq!(missing_explanation(&state, id, log, &[]), Some((log, SourceReason::LumberCamp)));
    certify(&mut state, id, BuildingKind::Mine);
    certify(&mut state, id, BuildingKind::LumberCamp);
    assert_eq!(missing_explanation(&state, id, cobble, &[]), Some((cobble, SourceReason::Miner)));
    assert_eq!(missing_explanation(&state, id, log, &[]), Some((log, SourceReason::Lumberjack)));
    worker(&mut state, id, VillagerProfession::Miner);
    worker(&mut state, id, VillagerProfession::Lumberjack);
    assert_eq!(missing_explanation(&state, id, cobble, &[]), None);
    assert_eq!(missing_explanation(&state, id, log, &[]), None);
}

#[test]
fn lumberjack_plus_camp_supplies_logs() {
    let mut state = state();
    let id = settle(&mut state);
    worker(&mut state, id, VillagerProfession::Lumberjack);
    certify(&mut state, id, BuildingKind::LumberCamp);
    let log = item(&state, "minecraft:oak_log");
    assert!(can_produce(&state, id, log, &[]));
    assert_eq!(missing_source(&state, id, log, &[]), None);
}

#[test]
fn farmer_plus_workshop_supplies_wheat() {
    let mut state = state();
    let id = settle(&mut state);
    worker(&mut state, id, VillagerProfession::Farmer);
    certify(
        &mut state,
        id,
        BuildingKind::Workshop(VillagerProfession::Farmer),
    );
    let wheat = item(&state, "minecraft:wheat");
    assert!(can_produce(&state, id, wheat, &[]));
    assert_eq!(missing_source(&state, id, wheat, &[]), None);
}

#[test]
fn unsupported_item_reports_itself() {
    let mut state = state();
    let id = settle(&mut state);
    worker(&mut state, id, VillagerProfession::Miner);
    certify(&mut state, id, BuildingKind::Mine);
    worker(&mut state, id, VillagerProfession::Lumberjack);
    certify(&mut state, id, BuildingKind::LumberCamp);
    worker(&mut state, id, VillagerProfession::Farmer);
    certify(
        &mut state,
        id,
        BuildingKind::Workshop(VillagerProfession::Farmer),
    );
    worker(&mut state, id, VillagerProfession::Shepherd);
    sheep(&mut state, [8.5, 201.0, 8.5], 0);
    // Netherite scrap has no settlement source and no grid recipe path.
    let scrap = item(&state, "minecraft:netherite_scrap");
    assert!(!can_produce(&state, id, scrap, &[]));
    assert_eq!(missing_source(&state, id, scrap, &[]), Some(scrap));
}

#[test]
fn bed_with_wood_covered_reports_wool_not_planks() {
    let mut state = state();
    let id = settle(&mut state);
    worker(&mut state, id, VillagerProfession::Lumberjack);
    certify(&mut state, id, BuildingKind::LumberCamp);
    // No shepherd and no sheep: the planks leg is covered, wool is not.
    let white_bed = item(&state, "minecraft:white_bed");
    let wool = item(&state, "minecraft:white_wool");
    assert!(!can_produce(&state, id, white_bed, &[]));
    assert_eq!(missing_source(&state, id, white_bed, &[]), Some(wool));
}

#[test]
fn bed_with_wool_and_wood_is_producible() {
    let mut state = state();
    let id = settle(&mut state);
    worker(&mut state, id, VillagerProfession::Lumberjack);
    certify(&mut state, id, BuildingKind::LumberCamp);
    worker(&mut state, id, VillagerProfession::Shepherd);
    sheep(&mut state, [8.5, 201.0, 8.5], 0);
    let white_bed = item(&state, "minecraft:white_bed");
    assert!(can_produce(&state, id, white_bed, &[]));
    assert_eq!(missing_source(&state, id, white_bed, &[]), None);
}

#[test]
fn stocked_leaf_satisfies_without_worker() {
    use crate::settlements::inventory;
    use mc_data::ItemStack;
    use mc_script::ScriptInventoryFence;

    let mut state = state();
    let id = settle(&mut state);
    // No workers at all: stocked white wool satisfies the leaf directly.
    let wool = item(&state, "minecraft:white_wool");
    let white_bed = item(&state, "minecraft:white_bed");
    let images = vec![inventory::Image {
        endpoint: inventory::Endpoint::Container {
            dimension: "minecraft:overworld".into(),
            position: [0, 0, 0],
        },
        slots: vec![ItemStack::new(wool, 1)],
        fence: ScriptInventoryFence::try_new(
            1,
            "0000000000000000000000000000000000000000000000000000000000000000".into(),
        )
        .unwrap(),
    }];
    assert!(can_produce(&state, id, wool, &images));
    assert_eq!(missing_source(&state, id, wool, &images), None);
    assert!(!can_produce(&state, id, white_bed, &images));
}
