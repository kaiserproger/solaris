use super::*;
use crate::boundary::PlayerInput;
use crate::settlements::allocator::tests::{block, test_state, villager};
use crate::tick::run_tick;
use mc_nbt::Tag;
use std::sync::Arc;

const CONN: ConnectionId = ConnectionId(7);
const PLAYER: &str = "00000000-0000-0000-0000-000000000000";

/// `test_state` plus the native work-recipe catalog, with one joined player.
fn joined(permission_level: u8) -> TickState {
    let mut state = test_state();
    let mut registry = (*state.registry).clone();
    crate::plugin_startup::apply(
        &mut registry,
        &mc_plugin_host::DeploymentContribution::default(),
        None,
        &[],
    )
    .unwrap();
    state.registry = Arc::new(registry);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: CONN,
            uuid: uuid::Uuid::nil(),
            name: "Mayor".into(),
            permission_level,
        }],
    );
    state
}

fn town(state: &mut TickState, owner: &str) -> SettlementId {
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "oak".into(),
            owner.into(),
            [8, 201, 8],
            48,
        )
        .unwrap()
}

fn field<'a>(tag: &'a Tag, key: &str) -> Option<&'a Tag> {
    match tag {
        Tag::Compound(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, v)| v),
        _ => None,
    }
}

fn list(tag: Option<&Tag>) -> &[Tag] {
    match tag {
        Some(Tag::List(list)) => &list.elements,
        _ => &[],
    }
}

fn text(tag: &Tag) -> String {
    match tag {
        Tag::String(value) => value.clone(),
        _ => String::new(),
    }
}

/// What a client sees of one dialog.
struct Shown {
    title: String,
    body: String,
    actions: Vec<(String, String)>,
    inputs: Vec<String>,
}

fn shown(tag: &Tag) -> Shown {
    let mut actions = Vec::new();
    for button in list(field(tag, "actions")) {
        let id = field(button, "action")
            .and_then(|action| field(action, "id"))
            .map(text)
            .unwrap_or_default();
        actions.push((text(field(button, "label").unwrap()), id));
    }
    Shown {
        title: text(field(tag, "title").unwrap()),
        body: list(field(tag, "body"))
            .iter()
            .map(|paragraph| text(field(paragraph, "contents").unwrap()))
            .collect::<Vec<_>>()
            .join("\n"),
        actions,
        inputs: list(field(tag, "inputs"))
            .iter()
            .map(|input| text(field(input, "key").unwrap()))
            .collect(),
    }
}

impl Shown {
    fn has(&self, action: &str) -> bool {
        self.actions
            .iter()
            .any(|(_, id)| id == &format!("solaris:settlement/{action}"))
    }
    fn label_of(&self, action: &str) -> &str {
        &self
            .actions
            .iter()
            .find(|(_, id)| id == &format!("solaris:settlement/{action}"))
            .unwrap()
            .0
    }
}

fn run(state: &mut TickState, inputs: &[PlayerInput]) -> Vec<PlayerChange> {
    let (_, outbox, _) = run_tick(state, inputs);
    outbox
        .into_iter()
        .filter(|(connection, _)| *connection == CONN)
        .flat_map(|(_, changes)| changes)
        .collect()
}

fn last_dialog(changes: &[PlayerChange]) -> Shown {
    changes
        .iter()
        .rev()
        .find_map(|change| match change {
            PlayerChange::ShowDialog { dialog } => Some(shown(dialog)),
            _ => None,
        })
        .expect("a dialog was shown")
}

fn chat(changes: &[PlayerChange]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|change| match change {
            PlayerChange::Chat { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn command(state: &mut TickState, line: &str) -> Vec<PlayerChange> {
    run(
        state,
        &[PlayerInput::Command {
            connection: CONN,
            command: line.into(),
            permission_level: 0,
        }],
    )
}

fn click(
    state: &mut TickState,
    id: SettlementId,
    action: &str,
    extra: &[(&str, Tag)],
) -> Vec<PlayerChange> {
    let mut entries = vec![("s".to_owned(), Tag::String(id.0.to_string()))];
    entries.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
    run(
        state,
        &[PlayerInput::DialogClick {
            connection: CONN,
            id: format!("solaris:settlement/{action}"),
            payload: Some(Tag::Compound(entries)),
        }],
    )
}

fn string(value: &str) -> Tag {
    Tag::String(value.into())
}

fn station(state: &mut TickState, at: [i32; 3], name: &str) {
    let id = block(state, name, &[]);
    state.world.set_block(at[0], at[1], at[2], id);
    state.world.fold_overlay();
}

fn certify_form(kind: &str) -> Vec<(&'static str, Tag)> {
    vec![
        ("kind", string(kind)),
        ("from", string("3 200 3")),
        ("to", string("7 200 7")),
        ("extra", Tag::Float(4.0)),
    ]
}

fn use_bell(state: &mut TickState, at: [i32; 3]) -> Vec<PlayerChange> {
    run(
        state,
        &[PlayerInput::UseItemOn {
            connection: CONN,
            hand: crate::boundary::Hand::Main,
            position: crate::boundary::BlockPos {
                x: at[0],
                y: at[1],
                z: at[2],
            },
            face: 2,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    )
}

fn assert_bell_rings(changes: &[PlayerChange], at: [i32; 3]) {
    assert!(
        changes.iter().any(|change| matches!(
            change,
            PlayerChange::BlockEvent { position, action: 1, parameter: 2, .. }
                if [position.x, position.y, position.z] == at
        )),
        "bell animation must still be queued"
    );
    assert!(
        changes.iter().any(|change| matches!(
            change,
            PlayerChange::BellRung { position }
                if [position.x, position.y, position.z] == at
        )),
        "bell sound must still be queued"
    );
}

#[test]
fn anchor_bell_opens_its_settlement_for_a_non_owner_and_rings() {
    let mut state = joined(0);
    let id = town(&mut state, "someone-else");
    // Another settlement is closer to the player: select by the clicked anchor,
    // not the command's nearest-settlement lookup.
    state
        .settlements
        .create(
            &mut state.settlement_sequence,
            "closer".into(),
            PLAYER.into(),
            [8, 202, 8],
            48,
        )
        .unwrap();
    station(&mut state, [8, 201, 8], "minecraft:bell");
    let changes = use_bell(&mut state, [8, 201, 8]);
    let dialog = last_dialog(&changes);
    assert_eq!(dialog.title, "Settlement oak");
    assert!(dialog.body.contains("View only"));
    assert_bell_rings(&changes, [8, 201, 8]);
    let denied = last_dialog(&click(
        &mut state,
        id,
        "rename",
        &[("name", string("stolen"))],
    ));
    assert!(denied.body.contains("Only the owner or an operator"));
    assert_eq!(state.settlements.get(id).unwrap().name, "oak");
}

#[test]
fn unrelated_bell_inside_settlement_only_rings() {
    let mut state = joined(0);
    town(&mut state, PLAYER);
    station(&mut state, [9, 201, 8], "minecraft:bell");
    let changes = use_bell(&mut state, [9, 201, 8]);
    assert_bell_rings(&changes, [9, 201, 8]);
    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, PlayerChange::ShowDialog { .. }))
    );
}

#[test]
fn settlement_anchor_without_a_bell_does_not_open_menu() {
    let mut state = joined(0);
    town(&mut state, PLAYER);
    station(&mut state, [8, 201, 8], "minecraft:stone");
    let changes = use_bell(&mut state, [8, 201, 8]);
    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, PlayerChange::ShowDialog { .. }))
    );
}

#[test]
fn settlement_command_opens_the_main_screen_with_live_numbers_for_a_player_inside() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let changes = command(&mut state, "settlement");
    let dialog = last_dialog(&changes);
    assert_eq!(dialog.title, "Settlement oak");
    assert!(dialog.body.contains("Radius: 48 blocks around 8 201 8"));
    assert!(dialog.body.contains("Residents: 0"));
    assert!(dialog.body.contains("Storage: none certified"));
    assert!(dialog.body.contains("Buildings: none yet"));
    assert!(!dialog.body.contains("need a certified workshop"));
    for action in ["buildings", "residents", "orders", "settings"] {
        assert!(dialog.has(action), "{action}");
    }
    // Pages go straight to a screen.
    let page = last_dialog(&command(&mut state, "settlement menu buildings"));
    assert!(page.title.starts_with("Buildings"));
    assert!(page.has("certify"));
    assert_eq!(id, SettlementId(1));
}

#[test]
fn command_without_any_settlement_explains_how_to_found_one() {
    let mut state = joined(0);
    let changes = command(&mut state, "settlement");
    assert!(chat(&changes)[0].contains("/settlement create"));
    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, PlayerChange::ShowDialog { .. }))
    );
}

#[test]
fn non_owner_sees_the_screens_read_only_and_every_mutating_click_is_refused() {
    let mut state = joined(0);
    let id = town(&mut state, "someone-else");
    let main = last_dialog(&command(&mut state, "settlement"));
    assert!(main.body.contains("View only"));
    let buildings = last_dialog(&click(&mut state, id, "buildings", &[]));
    assert!(!buildings.has("certify"));
    let settings = last_dialog(&click(&mut state, id, "settings", &[]));
    assert!(!settings.has("rename") && settings.inputs.is_empty());

    station(&mut state, [4, 201, 4], "minecraft:composter");
    for (action, extra) in [
        ("certify_do", certify_form("farmer")),
        ("certify", vec![]),
        ("order_start", vec![]),
        ("project_order", vec![("blueprint", string("house"))]),
        ("project_upgrade", vec![("b", string("1"))]),
        ("rename", vec![("name", string("hijacked"))]),
        ("upgrade", vec![("b", string("1"))]),
    ] {
        let refused = last_dialog(&click(&mut state, id, action, &extra));
        assert!(
            refused.body.contains("Only the owner or an operator"),
            "{action}"
        );
    }
    let record = state.settlements.get(id).unwrap();
    assert!(record.buildings.is_empty());
    assert!(record.project.is_none());
    assert_eq!(record.name, "oak");
}

#[test]
fn operator_manages_a_settlement_it_does_not_own() {
    let mut state = joined(2);
    let id = town(&mut state, "someone-else");
    let buildings = last_dialog(&click(&mut state, id, "buildings", &[]));
    assert!(buildings.has("certify"));
    let renamed = last_dialog(&click(&mut state, id, "rename", &[("name", string("elm"))]));
    assert!(renamed.body.contains("Renamed to elm."));
    assert_eq!(state.settlements.get(id).unwrap().name, "elm");
}

#[test]
fn stale_or_foreign_ids_never_panic_or_leak() {
    let mut state = joined(0);
    town(&mut state, PLAYER);
    let changes = click(&mut state, SettlementId(99), "buildings", &[]);
    assert_eq!(chat(&changes), ["That settlement no longer exists."]);
    // Missing/garbled settlement entry and unknown actions are ignored.
    let changes = run(
        &mut state,
        &[
            PlayerInput::DialogClick {
                connection: CONN,
                id: "solaris:settlement/buildings".into(),
                payload: Some(Tag::Compound(vec![("s".into(), string("nope"))])),
            },
            PlayerInput::DialogClick {
                connection: CONN,
                id: "solaris:settlement/ghost".into(),
                payload: None,
            },
        ],
    );
    assert!(chat(&changes).is_empty());
    let changes = click(&mut state, SettlementId(1), "ghost", &[]);
    assert!(changes.is_empty());
}

#[test]
fn certify_reports_missing_requirements_then_certifies_and_lists_with_upgrade_needs() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    // Nothing built: the form comes back with the reasons and the typed values.
    let failed = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &certify_form("farmer"),
    ));
    assert_eq!(failed.title, "Certify building");
    assert!(
        failed
            .body
            .contains("Cannot certify Farmer workshop. Missing:")
    );
    assert_eq!(failed.inputs, ["kind", "from", "to", "extra"]);
    assert!(state.settlements.get(id).unwrap().buildings.is_empty());

    station(&mut state, [4, 201, 4], "minecraft:composter");
    let done = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &certify_form("farmer"),
    ));
    assert!(done.title.starts_with("Buildings"));
    assert!(
        done.body
            .contains("Certified Farmer workshop #1 (level 1).")
    );
    assert!(done.body.contains("jobs 0/1"));
    assert!(done.body.contains("L2 needs: workstations 1/2"));
    assert!(done.has("upgrade"));
    assert_eq!(state.settlements.get(id).unwrap().buildings.len(), 1);

    // Upgrade refused while the second station is missing, then granted.
    let refused = last_dialog(&click(
        &mut state,
        id,
        "upgrade",
        &[("b", string(&(id.0 << 32 | 1).to_string()))],
    ));
    assert!(
        refused
            .body
            .contains("Cannot upgrade #1. Missing: workstations 1/2.")
    );
    station(&mut state, [6, 201, 4], "minecraft:composter");
    let upgraded = last_dialog(&click(
        &mut state,
        id,
        "upgrade",
        &[("b", string(&(id.0 << 32 | 1).to_string()))],
    ));
    assert!(upgraded.body.contains("Building #1 upgraded to level 2."));
    assert!(upgraded.body.contains("Farmer workshop L2"));
    // A building of another settlement id is rejected, not trusted.
    let forged = last_dialog(&click(
        &mut state,
        id,
        "upgrade",
        &[("b", string(&(5u64 << 32 | 1).to_string()))],
    ));
    assert!(forged.body.contains("no longer exists"));
}

#[test]
fn marking_corners_prefills_the_certify_form_and_bad_input_is_explained() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let first = last_dialog(&click(&mut state, id, "mark1", &certify_form("house")));
    let at = player_pos(&state, CONN).unwrap();
    assert!(
        first
            .body
            .contains(&format!("Corner 1 = {}", corner_text(at)))
    );
    assert_eq!(state.settlement_marks[&CONN][0], Some(at));
    // Reopening from the buildings screen uses the stored mark as corner 1.
    click(&mut state, id, "buildings", &[]);
    let reopened = last_dialog(&click(&mut state, id, "certify", &[]));
    assert_eq!(reopened.inputs, ["kind", "from", "to", "extra"]);
    let bad = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &[
            ("kind", string("house")),
            ("from", string("a b c")),
            ("to", string("1 2 3")),
        ],
    ));
    assert!(bad.body.contains("three whole numbers"));
    let unknown = last_dialog(&click(
        &mut state,
        id,
        "certify_do",
        &[
            ("kind", string("castle")),
            ("from", string("1 2 3")),
            ("to", string("1 2 3")),
        ],
    ));
    assert!(unknown.body.contains("Choose a building type"));
}

fn bound_resident(state: &mut TickState, id: SettlementId) -> uuid::Uuid {
    let slot = villager(state, [5.5, 201.0, 7.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(state, id, entity).unwrap();
    state.entities.row(slot).uuid
}

#[test]
fn main_hint_counts_working_profession_residents_without_a_certified_job() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let farmer = bound_resident(&mut state, id);
    let idle = bound_resident(&mut state, id);
    let nitwit = bound_resident(&mut state, id);
    for (uuid, profession) in [
        (farmer, Profession::Farmer),
        (idle, Profession::None),
        (nitwit, Profession::Nitwit),
    ] {
        let slot = state.entities.slot_of_uuid(uuid).unwrap();
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .data
            .profession = profession;
    }
    let main = last_dialog(&click(&mut state, id, "main", &[]));
    assert!(
        main.body
            .contains("1 workers need a certified workshop — open Buildings → Certify"),
        "{}",
        main.body
    );
    // Certifying a matching workshop and claiming its work point recovers the
    // worker; the profession is kept throughout.
    station(&mut state, [4, 201, 4], "minecraft:composter");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let dimension = state.dimension.name().to_owned();
    let slot = state.entities.slot_of_uuid(farmer).unwrap();
    let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
    villager.job_site = Some(crate::boundary::BlockPos { x: 4, y: 201, z: 4 });
    villager.job_site_dimension = dimension;
    let main = last_dialog(&click(&mut state, id, "main", &[]));
    assert!(
        !main.body.contains("need a certified workshop"),
        "{}",
        main.body
    );
    let slot = state.entities.slot_of_uuid(farmer).unwrap();
    let villager = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(villager.data.profession, Profession::Farmer);
}

#[test]
fn residents_screen_lists_jobs_and_free_slots() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let uuid = bound_resident(&mut state, id);
    let screen = last_dialog(&click(&mut state, id, "residents", &[]));
    let name = super::super::presentation::villager_name(uuid);
    assert!(
        screen
            .body
            .contains("#1 Farmer workshop L1: 0/1 jobs filled")
    );
    assert!(
        screen
            .body
            .contains("Free job slots: 1; unemployed residents: 1")
    );
    assert!(screen.body.contains(&format!("{name} (None): no job")));
}

#[test]
fn production_order_flow_creates_pauses_and_cancels_a_real_job() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    station(&mut state, [5, 201, 5], "minecraft:crafting_table");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let building = BuildingId(id.0 << 32 | 1);

    // No resident yet: the flow explains it instead of opening an empty form.
    let none = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert!(none.body.contains("No free resident"));

    let worker = bound_resident(&mut state, id);
    let form = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert_eq!(form.title, "New production order");
    assert_eq!(form.inputs, ["count", "worker"]);
    assert!(form.has("order_create"));

    let recipe = state
        .registry
        .item_facts
        .work_recipes()
        .find(|recipe| recipe.station.as_str() == "minecraft:crafting_table")
        .unwrap()
        .id
        .as_str()
        .to_owned();
    let create = |recipe: &str, at: &str| {
        vec![
            ("b", string(&building.0.to_string())),
            ("p", string(at)),
            ("r", string(recipe)),
            ("worker", string(&worker.to_string())),
            ("count", Tag::Float(3.0)),
        ]
    };
    // A station position that is not a crafting table is refused.
    let wrong = last_dialog(&click(
        &mut state,
        id,
        "order_create",
        &create(&recipe, "4 201 4"),
    ));
    assert!(wrong.body.contains("needs a different workstation"));
    assert!(
        state
            .settlements
            .get(id)
            .unwrap()
            .economy
            .production
            .jobs
            .is_empty()
    );

    let ok = last_dialog(&click(
        &mut state,
        id,
        "order_create",
        &create(&recipe, "5 201 5"),
    ));
    assert!(ok.body.contains("Order ui-1 started"), "{}", ok.body);
    let job = state.settlements.get(id).unwrap().economy.production.jobs["ui-1"].clone();
    assert_eq!(
        (job.worker, job.count, job.station),
        (worker, 3, [5, 201, 5])
    );
    assert_eq!(job.building.kind, "building");
    assert_eq!(job.building.id, building.0.to_string());
    assert!((0..3).all(|i| (job.stand_at[i] - job.station[i]).abs() <= 1));
    assert_ne!(job.stand_at, job.station);

    // The same worker cannot take a second order.
    let busy = last_dialog(&click(
        &mut state,
        id,
        "order_create",
        &create(&recipe, "5 201 5"),
    ));
    assert!(busy.body.contains("already has an active order"));
    let none = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert!(none.body.contains("No free resident"));

    let listing = last_dialog(&click(&mut state, id, "orders", &[]));
    assert!(listing.body.contains("ui-1:"));
    assert!(listing.has("order_pause") && listing.has("order_cancel"));
    assert_eq!(listing.label_of("order_pause"), "Pause ui-1");

    let paused = last_dialog(&click(
        &mut state,
        id,
        "order_pause",
        &[("j", string("ui-1"))],
    ));
    assert!(paused.body.contains("paused"));
    assert!(paused.has("order_resume"));
    let resumed = last_dialog(&click(
        &mut state,
        id,
        "order_resume",
        &[("j", string("ui-1"))],
    ));
    assert!(resumed.has("order_pause"));
    let cancelled = last_dialog(&click(
        &mut state,
        id,
        "order_cancel",
        &[("j", string("ui-1"))],
    ));
    assert!(cancelled.body.contains("cancelled"));
    assert!(state.settlements.get(id).unwrap().economy.production.jobs["ui-1"].cancelled);
    let gone = last_dialog(&click(
        &mut state,
        id,
        "order_cancel",
        &[("j", string("nope"))],
    ));
    assert!(gone.body.contains("no longer exists"));
}

#[test]
fn production_without_a_crafting_table_says_what_to_place() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let note = last_dialog(&click(&mut state, id, "order_start", &[]));
    assert!(
        note.body
            .contains("No crafting table inside a certified workshop")
    );
}

#[test]
fn rename_validates_and_updates_the_record() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let bad = last_dialog(&click(
        &mut state,
        id,
        "rename",
        &[("name", string("bad name!"))],
    ));
    assert!(bad.body.contains("invalid settlement name"));
    assert_eq!(state.settlements.get(id).unwrap().name, "oak");
    let good = last_dialog(&click(
        &mut state,
        id,
        "rename",
        &[("name", string("birch"))],
    ));
    assert!(good.body.contains("Renamed to birch."));
    assert_eq!(state.settlements.get(id).unwrap().name, "birch");
    assert_eq!(good.inputs, ["name"]);
}

#[test]
fn autonomous_main_page_keeps_claim_first_and_fits_a_small_window() {
    let mut state = joined(0);
    let at = player_pos(&state, CONN).unwrap();
    let id = state
        .settlements
        .adopt_village(
            &mut state.settlement_sequence,
            "minecraft:overworld:compact-village",
            at,
        )
        .unwrap();
    for index in 0..8 {
        let slot = villager(
            &mut state,
            [at[0] as f64 + index as f64, at[1] as f64, at[2] as f64],
        );
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(&mut state, id, entity).unwrap();
    }
    let page = last_dialog(&command(&mut state, "settlement"));
    assert_eq!(
        page.actions.first().map(|(_, action)| action.as_str()),
        Some("solaris:settlement/claim"),
        "the primary action must not be placed below the other navigation buttons"
    );
    assert!(
        page.actions.len() <= 4,
        "unclaimed navigation must stay compact"
    );
    assert!(
        page.body.lines().count() <= 3,
        "move detailed statistics off the claim page"
    );
    assert!(
        page.body.len() <= 240,
        "the claim page must not need body scrolling"
    );
    assert!(page.has("buildings") && page.has("residents") && page.has("projects"));
}

#[test]
fn autonomous_dialog_claim_requires_live_presence_and_refreshes_management() {
    let mut state = joined(2);
    let at = player_pos(&state, CONN).unwrap();
    let id = state
        .settlements
        .adopt_village(
            &mut state.settlement_sequence,
            "minecraft:overworld:ui-village",
            at,
        )
        .unwrap();
    let changes = command(&mut state, "settlement");
    assert!(last_dialog(&changes).has("claim"));
    let original = state.settlements.get(id).unwrap().name.clone();
    click(&mut state, id, "rename", &[("name", string("stolen"))]);
    assert_eq!(state.settlements.get(id).unwrap().name, original);
    let projects = last_dialog(&click(&mut state, id, "projects", &[]));
    assert!(!projects.has("project_order"));
    for (action, extra) in [
        ("project_order", vec![("blueprint", string("house"))]),
        ("project_upgrade", vec![("b", string("1"))]),
    ] {
        click(&mut state, id, action, &extra);
    }
    assert!(
        state.settlements.get(id).unwrap().project.is_none(),
        "even an operator must claim before ordering construction"
    );
    let entity = state.connections.entity_of(CONN).unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [1000.0, 201.0, 1000.0]);
    click(&mut state, id, "claim", &[]);
    assert_eq!(state.settlements.get(id).unwrap().owner, None);
    state.entities.set_pos(slot, at.map(f64::from));
    let changes = click(&mut state, id, "claim", &[]);
    assert_eq!(
        state.settlements.get(id).unwrap().owner.as_deref(),
        Some(PLAYER)
    );
    assert!(!last_dialog(&changes).has("claim"));
}

fn blueprint_build(state: &mut TickState, id: SettlementId, blueprint: &str) -> BuildingId {
    use crate::settlements::construction::tests::{complete, fund, start};
    for x in 0..16 {
        for z in 0..16 {
            for y in 201..224 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let slot = villager(state, [1.5, 201.0, 1.5]);
    let entity = state.entities.body(slot).id;
    crate::settlements::bind(state, id, entity).unwrap();
    let chest = block(state, "minecraft:chest", &[]);
    state.world.set_block(1, 201, 1, chest);
    state.block_entities.insert(
        (1, 201, 1),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    state.settlements.get_mut(id).unwrap().bootstrap_storage = Some([1, 201, 1]);
    {
        let villager = state.entities.row_mut(slot).villager.as_mut().unwrap();
        villager.data.profession = crate::villager::VillagerProfession::Builder;
        villager.activity = crate::villager::Activity::Work;
    }
    state.day_time = 3000;
    state.world.fold_overlay();
    start(state, id, blueprint, 1, None);
    fund(state, blueprint, 1);
    complete(state, id, slot);
    assert!(state.settlements.get(id).unwrap().project.is_none());
    *state
        .settlements
        .get(id)
        .unwrap()
        .buildings
        .keys()
        .next()
        .unwrap()
}
/// Cleared site plus an empty bootstrap chest and a bound resident, so an
/// owner click runs the same admission shape as the planner RED fixture.
/// Funding stays explicit at the call site: only the click may start the
/// single project, and the click itself must debit nothing.
fn prep_projects_site(state: &mut TickState, id: SettlementId) {
    for x in 0..16 {
        for z in 0..16 {
            for y in 201..224 {
                if (x, y, z) == (1, 201, 1) {
                    continue;
                }
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    if let std::collections::btree_map::Entry::Vacant(entry) =
        state.block_entities.entry((1, 201, 1))
    {
        entry.insert(crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        });
        let chest = block(state, "minecraft:chest", &[]);
        state.world.set_block(1, 201, 1, chest);
        state.settlements.get_mut(id).unwrap().bootstrap_storage = Some([1, 201, 1]);
    }
    if crate::settlements::member_count(state, id) == 0 {
        let slot = villager(state, [1.5, 201.0, 1.5]);
        let entity = state.entities.body(slot).id;
        crate::settlements::bind(state, id, entity).unwrap();
    }
    state.day_time = 3000;
    state.world.fold_overlay();
}

#[test]
fn projects_screen_offers_every_blueprint_order_to_the_owner() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let page = last_dialog(&click(&mut state, id, "projects", &[]));
    assert!(page.title.starts_with("Projects"));
    let expected: Vec<String> = crate::settlements::blueprints::all(&state.registry)
        .iter()
        .map(|blueprint| format!("Order: {}", blueprint.id))
        .collect();
    assert_eq!(expected.len(), 9);
    for label in expected {
        assert!(
            page.actions.iter().any(
                |(text, action)| text == &label && action == "solaris:settlement/project_order"
            ),
            "{label}"
        );
    }
    assert!(!page.body.contains("View only"));

    // Without funded stock the order starts nothing and saves nothing.
    prep_projects_site(&mut state, id);
    let revision = state.settlements.get(id).unwrap().revision;
    let before_block = state.world.block_at(8, 201, 8);
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    let poor = last_dialog(&click(
        &mut state,
        id,
        "project_order",
        &[("blueprint", string("watch_tower"))],
    ));
    assert!(poor.body.contains("not enough:"), "{}", poor.body);
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(state.settlements.get(id).unwrap().revision, revision);
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);
    assert_eq!(state.world.block_at(8, 201, 8), before_block);
    assert_eq!(poor.title, page.title);

    // Funded stock starts the single project immediately, with no debit or edit.
    crate::settlements::construction::tests::fund(&mut state, "watch_tower", 1);
    let before = state.block_entities[&(1, 201, 1)].slots.clone();
    let ordered = last_dialog(&click(
        &mut state,
        id,
        "project_order",
        &[("blueprint", string("watch_tower"))],
    ));
    assert!(
        ordered.body.contains("Ordered watch_tower"),
        "{}",
        ordered.body
    );
    let project = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .clone()
        .expect("funded order starts immediately");
    assert_eq!(project.blueprint, "watch_tower");
    assert_eq!(project.level, 1);
    assert!(!project.structure.is_empty());
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, before);
    assert_eq!(state.world.block_at(8, 201, 8), before_block);
    assert_eq!(ordered.title, page.title);

    // A second order while the project is active is refused without mutation.
    let busy = last_dialog(&click(
        &mut state,
        id,
        "project_order",
        &[("blueprint", string("house"))],
    ));
    assert!(busy.body.contains("Busy."), "{}", busy.body);
    assert_eq!(
        state
            .settlements
            .get(id)
            .unwrap()
            .project
            .as_ref()
            .expect("active project kept")
            .blueprint,
        "watch_tower"
    );
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, before);
    assert_eq!(busy.title, page.title);
}

#[test]
fn projects_screen_shows_upgrade_only_for_blueprint_built_buildings() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let building = blueprint_build(&mut state, id, "house");
    assert!(crate::settlements::construction::upgrade_available(
        &state, id, building
    ));
    let page = last_dialog(&click(&mut state, id, "projects", &[]));
    let seq = building.0 & u64::from(u32::MAX);
    assert!(
        page.actions
            .iter()
            .any(|(text, action)| text.contains(&format!("#{seq}"))
                && text.contains("House L1")
                && text.starts_with("Upgrade")
                && action == "solaris:settlement/project_upgrade"),
        "{:?}",
        page.actions
    );

    // Without L2 stock the upgrade starts nothing and saves nothing.
    for slot in state
        .block_entities
        .get_mut(&(1, 201, 1))
        .unwrap()
        .slots
        .iter_mut()
    {
        *slot = mc_data::ItemStack::EMPTY;
    }
    let stock = state.block_entities[&(1, 201, 1)].slots.clone();
    let poor = last_dialog(&click(
        &mut state,
        id,
        "project_upgrade",
        &[("b", string(&building.0.to_string()))],
    ));
    assert!(poor.body.contains("not enough:"), "{}", poor.body);
    assert!(state.settlements.get(id).unwrap().project.is_none());
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, stock);

    // Funded L2 stock starts the upgrade immediately, with no debit.
    crate::settlements::construction::tests::fund(&mut state, "house", 2);
    let before = state.block_entities[&(1, 201, 1)].slots.clone();
    let started = last_dialog(&click(
        &mut state,
        id,
        "project_upgrade",
        &[("b", string(&building.0.to_string()))],
    ));
    assert!(
        started
            .body
            .contains(&format!("Upgrade for #{seq} started")),
        "{}",
        started.body
    );
    let project = state
        .settlements
        .get(id)
        .unwrap()
        .project
        .clone()
        .expect("funded upgrade starts immediately");
    assert_eq!(project.building, Some(building));
    assert_eq!(project.level, 2);
    assert!(!project.structure.is_empty());
    assert_eq!(state.block_entities[&(1, 201, 1)].slots, before);

    // A player-certified building of the same kind is never offered an upgrade.
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    station(&mut state, [4, 201, 4], "minecraft:composter");
    click(&mut state, id, "certify_do", &certify_form("farmer"));
    let page = last_dialog(&click(&mut state, id, "projects", &[]));
    assert!(
        !page
            .actions
            .iter()
            .any(|(_, action)| action == "solaris:settlement/project_upgrade"),
        "{:?}",
        page.actions
    );
}

#[test]
fn projects_screen_keeps_every_order_button_above_the_status_lines() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    blueprint_build(&mut state, id, "house");
    let page = last_dialog(&click(&mut state, id, "projects", &[]));
    // A dialog renders its body above the action grid in one scroll column, so
    // an empty body is what keeps Order/Upgrade reachable without scrolling
    // past status lines: with a note-less page the grid comes first.
    assert_eq!(page.body, "", "no status line may precede the grid");
    assert_eq!(
        page.actions.first().map(|(_, action)| action.as_str()),
        Some("solaris:settlement/project_order"),
        "{:?}",
        page.actions
    );
    assert!(page.has("project_upgrade") && page.has("project_status"));

    let status = last_dialog(&click(&mut state, id, "project_status", &[]));
    assert_eq!(status.title, "Project status - oak");
    assert!(status.has("projects"));
    assert!(
        status.body.contains("No active project:"),
        "{}",
        status.body
    );
    assert!(!status.has("project_order") && !status.has("project_upgrade"));
}

#[test]
fn project_status_waiting_uses_translatable_material_names_without_truncation() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    prep_projects_site(&mut state, id);
    let changes = click(&mut state, id, "project_status", &[]);
    let dialog = changes
        .iter()
        .rev()
        .find_map(|change| match change {
            PlayerChange::ShowDialog { dialog } => Some(dialog),
            _ => None,
        })
        .expect("project status dialog");
    let body = format!("{:?}", field(dialog, "body").unwrap());
    assert!(list(field(dialog, "body")).len() <= 64, "{body}");
    assert!(body.contains("needs a mine"), "{body}");
    assert!(body.contains("translate"), "{body}");
    assert!(!body.contains("minecraft:cobblestone"), "{body}");
    assert!(!body.contains("more)"), "{body}");
    let planks = list(field(dialog, "body"))
        .iter()
        .flat_map(|paragraph| list(field(field(paragraph, "contents").unwrap(), "extra")))
        .find(|item| {
            field(item, "translate").is_some_and(|key| text(key) == "block.minecraft.oak_planks")
        })
        .expect("planks requirement");
    assert!(
        format!("{planks:?}").contains("block.minecraft.oak_log"),
        "{planks:?}"
    );
    assert!(format!("{planks:?}").contains("needs, e.g."), "{planks:?}");
}

#[test]
fn project_status_does_not_claim_all_candidates_lack_materials_when_one_is_funded() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    prep_projects_site(&mut state, id);
    assert!(!super::super::construction::has_funded_candidate(
        &state, id
    ));
    crate::settlements::construction::tests::fund(&mut state, "storage", 1);
    assert!(super::super::construction::has_funded_candidate(&state, id));
}

#[test]
fn project_status_active_shows_readable_progress() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    prep_projects_site(&mut state, id);
    crate::settlements::construction::tests::fund(&mut state, "watch_tower", 1);
    click(
        &mut state,
        id,
        "project_order",
        &[("blueprint", string("watch_tower"))],
    );
    let status = last_dialog(&click(&mut state, id, "project_status", &[]));
    assert!(
        status.body.contains("Watchtower · level 1 · "),
        "{}",
        status.body
    );
    assert!(status.body.contains("Site: "), "{}", status.body);
    assert!(!status.body.contains("minecraft:"), "{}", status.body);
}

#[test]
fn projects_clicks_are_read_only_for_visitors_and_forged_payloads_start_nothing() {
    let mut state = joined(0);
    let id = town(&mut state, "someone-else");
    let page = last_dialog(&click(&mut state, id, "projects", &[]));
    assert!(page.body.contains("View only"));
    assert!(
        !page
            .actions
            .iter()
            .any(|(_, action)| action == "solaris:settlement/project_order"
                || action == "solaris:settlement/project_upgrade"),
        "{:?}",
        page.actions
    );

    for (action, extra) in [
        ("project_order", vec![("blueprint", string("house"))]),
        ("project_upgrade", vec![("b", string("1"))]),
    ] {
        let refused = last_dialog(&click(&mut state, id, action, &extra));
        assert!(
            refused.body.contains("Only the owner or an operator"),
            "{action}"
        );
    }
    assert!(state.settlements.get(id).unwrap().project.is_none());

    // Owner clicks with forged or stale payloads start nothing and stay on page.
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    let forged_blueprint = last_dialog(&click(
        &mut state,
        id,
        "project_order",
        &[("blueprint", string("castle"))],
    ));
    assert!(forged_blueprint.body.contains("Unknown blueprint"));
    let stale_building = last_dialog(&click(
        &mut state,
        id,
        "project_upgrade",
        &[("b", string("999"))],
    ));
    assert!(stale_building.body.contains("no longer exists"));
    let foreign = last_dialog(&click(
        &mut state,
        id,
        "project_upgrade",
        &[("b", string(&(5u64 << 32 | 1).to_string()))],
    ));
    assert!(foreign.body.contains("no longer exists"));
    assert!(state.settlements.get(id).unwrap().project.is_none());
}

// ------------------------------------------------------------------ localisation

/// Play `ClientInformation` locale updates arrive as `PlayerInput::SetLocale`.
fn set_locale(state: &mut TickState, locale: &str) {
    run_tick(
        state,
        &[PlayerInput::SetLocale {
            connection: CONN,
            locale: locale.into(),
        }],
    );
}

#[test]
fn russian_locale_localizes_the_main_menu_and_army_page() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    set_locale(&mut state, "ru_ru");

    let main = last_dialog(&command(&mut state, "settlement"));
    assert_eq!(main.title, "Поселение oak");
    assert!(main.has("army"));
    assert_eq!(main.label_of("buildings"), "Постройки");
    assert_eq!(main.label_of("residents"), "Жители и работа");
    assert_eq!(main.label_of("orders"), "Заказы производства");
    assert_eq!(main.label_of("army"), "Армия");
    assert_eq!(main.label_of("projects"), "Стройка");
    assert_eq!(main.label_of("settings"), "Настройки");
    assert!(main.body.contains("Владелец: "), "{}", main.body);
    assert!(
        main.body.contains("Радиус: 48 блоков вокруг 8 201 8"),
        "{}",
        main.body
    );
    assert!(main.body.contains("Постройки: пока нет"), "{}", main.body);
    assert!(
        main.body.contains("Склад: нет сертифицированного"),
        "{}",
        main.body
    );

    let army = last_dialog(&click(&mut state, id, "army", &[]));
    assert_eq!(army.title, "Армия — oak");
    assert_eq!(army.label_of("army_recruit"), "Нанять солдата");
    assert_eq!(army.label_of("main"), "Назад");
    assert!(army.body.contains("Солдат нет"), "{}", army.body);
}

#[test]
fn en_us_locale_keeps_the_settlement_dialogs_english() {
    let mut state = joined(0);
    let id = town(&mut state, PLAYER);
    set_locale(&mut state, "en_us");

    let main = last_dialog(&command(&mut state, "settlement"));
    assert_eq!(main.title, "Settlement oak");
    assert_eq!(main.label_of("army"), "Army");
    assert!(main.body.contains("Owner: "), "{}", main.body);
    assert!(main.body.contains("Buildings: none yet"), "{}", main.body);

    let army = last_dialog(&click(&mut state, id, "army", &[]));
    assert_eq!(army.title, "Army - oak");
    assert_eq!(army.label_of("army_recruit"), "Recruit soldier");
    assert_eq!(army.label_of("main"), "Back");
}

#[test]
fn play_locale_update_applies_to_the_next_open() {
    let mut state = joined(0);
    town(&mut state, PLAYER);
    let first = last_dialog(&command(&mut state, "settlement"));
    assert_eq!(first.title, "Settlement oak", "no locale yet reads English");

    set_locale(&mut state, "ru_ru");
    let second = last_dialog(&command(&mut state, "settlement"));
    assert_eq!(second.title, "Поселение oak");

    set_locale(&mut state, "en_gb");
    let third = last_dialog(&command(&mut state, "settlement"));
    assert_eq!(third.title, "Settlement oak");
}

#[test]
fn locale_is_recorded_before_join_and_cleared_on_leave() {
    let mut state = test_state();
    run_tick(
        &mut state,
        &[PlayerInput::SetLocale {
            connection: CONN,
            locale: "ru_ru".into(),
        }],
    );
    assert_eq!(state.connections.locale_of(CONN), Some("ru_ru"));

    run_tick(&mut state, &[PlayerInput::Leave { connection: CONN }]);
    assert_eq!(state.connections.locale_of(CONN), None);
}
