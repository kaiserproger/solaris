//! Settlement management dialogs (26.1.2 server-driven UI): `/settlement`,
//! `/settlement menu [page]`, and every button click routed here as
//! `solaris:settlement/<action>` by [`crate::dialog::route_click`].
//!
//! Authority stays in the tick: a dialog is a view built from live state, and a
//! click is an untrusted request. Every click re-resolves the settlement from the
//! `s` payload entry, re-derives permission (record owner or operator, the same
//! rule as the chat commands; reads are open) and re-validates all other input
//! before calling the existing native paths ([`buildings::certify`],
//! [`buildings::upgrade`], production [`Action`]s, registry rename).
//!
//! Titles, buttons, labels and notes come from [`ui_text`]: the language is the
//! connection's client locale (`ClientInformation.language` carried by the edge),
//! Russian for `ru*` and English otherwise. Game data (item, blueprint,
//! profession and settlement names) stays as authored.

use std::collections::BTreeSet;

use mc_data::Identifier;
use mc_nbt::Tag;
use mc_script::{
    ScriptOperationFailure as Failure, ScriptSettlementBuildingRef,
    ScriptSettlementEconomyAction as Action, ScriptWorldBounds,
};
use uuid::Uuid;

use super::buildings::{self, BuildingId, BuildingKind, CertError};
use super::ui_text::{self, Key, Lang};
use super::{SettlementId, economy_customization, inventory};
use crate::boundary::{ConnectionId, PlayerChange};
use crate::dialog::{self, Button, ClickPayload, Dialog, Input};
use crate::tick::TickState;
use crate::villager::VillagerProfession as Profession;

pub(crate) const OWNER: &str = "settlement";

/// The two block positions a player marked for the certify form.
pub(crate) type Marks = [Option<[i32; 3]>; 2];

const MAX_LISTED: usize = 12;
const MAX_RESIDENTS: usize = 30;
const MAX_STATIONS: usize = 4;
const MAX_MARKS: usize = 64;
const MAX_ORDER: f32 = 64.0;
const MAX_EXTRA_HEIGHT: f32 = 24.0;

/// Actions that change state; everything else is a read-only screen.
const MUTATING: &[&str] = &[
    "certify",
    "mark1",
    "mark2",
    "certify_do",
    "upgrade",
    "project_order",
    "project_upgrade",
    "order_start",
    "order_pick",
    "order_create",
    "order_pause",
    "order_resume",
    "order_cancel",
    "rename",
    "army_recruit",
    "army_dismiss",
    "army_order",
];

struct Ctx {
    id: SettlementId,
    manage: bool,
    /// Dialog language for this connection's client locale.
    lang: Lang,
}

impl Ctx {
    fn t(&self, key: Key) -> &'static str {
        ui_text::text(self.lang, key)
    }

    /// Translated `key` with `{name}` placeholders filled in.
    fn f(&self, key: Key, args: &[(&str, &str)]) -> String {
        ui_text::fill(self.t(key), args)
    }
}

struct Resident {
    uuid: Uuid,
    profession: Profession,
    job: Option<BuildingId>,
    soldier: Option<super::army::Soldier>,
}

struct Station {
    building: BuildingId,
    kind: BuildingKind,
    at: [i32; 3],
    block: String,
}

#[derive(Default)]
struct Form {
    kind: String,
    from: String,
    to: String,
    extra: f32,
}

// ---------------------------------------------------------------- entry points

/// `/settlement` and `/settlement menu [page]`: the settlement around the player
/// (else one they own), on the requested page.
pub(crate) fn open_menu(state: &mut TickState, connection: ConnectionId, page: Option<&str>) {
    let lang = Lang::of(state.connections.locale_of(connection));
    let Some(id) = locate(state, connection) else {
        say(state, connection, ui_text::text(lang, Key::NoSettlement));
        return;
    };
    let Some(ctx) = ctx(state, connection, id) else {
        say(
            state,
            connection,
            ui_text::text(lang, Key::AdmissionUnavailable),
        );
        return;
    };
    let action = match page.unwrap_or("main") {
        "main" => "main",
        "buildings" => "buildings",
        "certify" => "certify",
        "residents" | "jobs" => "residents",
        "production" | "orders" => "orders",
        "army" => "army",
        "projects" => "projects",
        "settings" => "settings",
        _ => {
            say(state, connection, ui_text::text(lang, Key::Pages));
            return;
        }
    };
    dispatch(state, connection, &ctx, action, ClickPayload(None));
}

/// Opens the main settlement page for a known settlement anchor.
pub(crate) fn open_for_settlement(
    state: &mut TickState,
    connection: ConnectionId,
    id: SettlementId,
) {
    let Some(ctx) = ctx(state, connection, id) else {
        let lang = Lang::of(state.connections.locale_of(connection));
        say(
            state,
            connection,
            ui_text::text(lang, Key::AdmissionUnavailable),
        );
        return;
    };
    dispatch(state, connection, &ctx, "main", ClickPayload(None));
}

/// One `solaris:settlement/<action>` click.
pub(crate) fn click(
    state: &mut TickState,
    connection: ConnectionId,
    action: &str,
    payload: ClickPayload<'_>,
) {
    let Some(id) = payload
        .string("s")
        .and_then(|text| text.parse().ok())
        .map(SettlementId)
    else {
        return;
    };
    let Some(ctx) = ctx(state, connection, id) else {
        let lang = Lang::of(state.connections.locale_of(connection));
        say(state, connection, ui_text::text(lang, Key::SettlementGone));
        return;
    };
    dispatch(state, connection, &ctx, action, payload);
}

fn dispatch(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    action: &str,
    payload: ClickPayload<'_>,
) {
    if MUTATING.contains(&action) && !ctx.manage {
        main_screen(state, connection, ctx, ctx.t(Key::Denied));
        return;
    }
    match action {
        "main" => main_screen(state, connection, ctx, ""),
        "claim" => {
            let note = match super::commands::claim(state, connection, Some(ctx.id)) {
                Ok(message) => message,
                Err(message) => message.to_owned(),
            };
            if let Some(ctx) = self::ctx(state, connection, ctx.id) {
                main_screen(state, connection, &ctx, &note);
            }
        }
        "buildings" => buildings_screen(state, connection, ctx, ""),
        "residents" => residents_screen(state, connection, ctx),
        "orders" => orders_screen(state, connection, ctx, ""),
        "army" => army_screen(state, connection, ctx, ""),
        "projects" => projects_screen(state, connection, ctx, ""),
        "project_status" => project_status_screen(state, connection, ctx),
        "settings" => settings_screen(state, connection, ctx, ""),
        "certify" => {
            let form = form_of(state, connection, payload);
            certify_screen(state, connection, ctx, &form, "");
        }
        "mark1" | "mark2" => {
            let index = usize::from(action == "mark2");
            let mut form = form_of(state, connection, payload);
            match player_pos(state, connection) {
                Some(at) => {
                    set_mark(state, connection, index, at);
                    *(if index == 0 {
                        &mut form.from
                    } else {
                        &mut form.to
                    }) = corner_text(at);
                    let note = ctx.f(
                        Key::CornerSet,
                        &[("n", &(index + 1).to_string()), ("at", &corner_text(at))],
                    );
                    certify_screen(state, connection, ctx, &form, &note);
                }
                None => certify_screen(state, connection, ctx, &form, ctx.t(Key::PositionUnknown)),
            }
        }
        "certify_do" => certify(state, connection, ctx, payload),
        "upgrade" => upgrade(state, connection, ctx, payload),
        "project_order" => project_order(state, connection, ctx, payload),
        "project_upgrade" => project_upgrade(state, connection, ctx, payload),
        "order_start" => order_start(state, connection, ctx),
        "order_pick" => order_pick(state, connection, ctx, payload),
        "order_create" => order_create(state, connection, ctx, payload),
        "order_pause" | "order_resume" | "order_cancel" => {
            order_control(state, connection, ctx, action, payload)
        }
        "army_soldier" => army_soldier(state, connection, ctx, payload),
        "army_recruit" => army_recruit(state, connection, ctx),
        "army_dismiss" => army_dismiss(state, connection, ctx, payload),
        "army_order" => army_order(state, connection, ctx, payload),
        "rename" => rename(state, connection, ctx, payload),
        _ => {
            tracing::debug!(target: "dialog", connection = connection.0, action, "unknown settlement action");
        }
    }
}

// ------------------------------------------------------------------- resolving

fn ctx(state: &TickState, connection: ConnectionId, id: SettlementId) -> Option<Ctx> {
    let record = state.settlements.get(id)?;
    let actor = state.connections.uuid_of(connection)?.to_string();
    Some(Ctx {
        id,
        manage: record.can_manage(&actor, state.plugin_players.is_operator(connection)),
        lang: Lang::of(state.connections.locale_of(connection)),
    })
}

fn player_pos(state: &TickState, connection: ConnectionId) -> Option<[i32; 3]> {
    let entity = state.connections.entity_of(connection)?;
    let slot = state.entities.slot_of(entity)?;
    Some(state.entities.pos(slot).map(|value| value.floor() as i32))
}

/// The nearest settlement whose radius contains the player, else the first the
/// player owns.
fn locate(state: &TickState, connection: ConnectionId) -> Option<SettlementId> {
    let actor = state.connections.uuid_of(connection)?.to_string();
    if let Some(at) = player_pos(state, connection) {
        let inside = state
            .settlements
            .records()
            .filter_map(|record| {
                let distance: i64 = at
                    .iter()
                    .zip(record.anchor)
                    .map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2))
                    .sum();
                (distance <= i64::from(record.radius).pow(2)).then_some((distance, record.id))
            })
            .min();
        if let Some((_, id)) = inside {
            return Some(id);
        }
    }
    state
        .settlements
        .records()
        .find(|record| record.owner.as_deref() == Some(actor.as_str()))
        .map(|record| record.id)
}

fn say(state: &mut TickState, connection: ConnectionId, message: &str) {
    state.push_player(
        connection,
        PlayerChange::Chat {
            message: message.to_owned(),
        },
    );
}

fn present(state: &mut TickState, connection: ConnectionId, dialog: Dialog) {
    if let Err(error) = dialog::open(state, connection, dialog) {
        tracing::warn!(target: "dialog", %error, "settlement dialog rejected");
    }
}

fn button(label: impl Into<String>, action: &str, id: SettlementId) -> Button {
    Button::custom(label.into(), &format!("solaris:{OWNER}/{action}"))
        .with("s", Tag::String(id.0.to_string()))
}

fn frame(ctx: &Ctx, title: String, note: &str, text: String, buttons: Vec<Button>) -> Dialog {
    let mut dialog = Dialog::multi_action(title, buttons);
    for paragraph in [note, text.as_str()] {
        if !paragraph.is_empty() {
            dialog = dialog.text(paragraph);
        }
    }
    dialog.exit(Button::plain(ctx.t(Key::Close)))
}

fn short(id: BuildingId) -> u64 {
    id.0 & u64::from(u32::MAX)
}

fn corner_text(at: [i32; 3]) -> String {
    format!("{} {} {}", at[0], at[1], at[2])
}

fn kind_label(ctx: &Ctx, kind: BuildingKind) -> String {
    match kind {
        BuildingKind::House => ctx.t(Key::KindHouse).into(),
        BuildingKind::Storage => ctx.t(Key::KindStorage).into(),
        BuildingKind::Mine => ctx.t(Key::KindMine).into(),
        BuildingKind::LumberCamp => ctx.t(Key::KindLumberCamp).into(),
        BuildingKind::Barracks => ctx.t(Key::KindBarracks).into(),
        BuildingKind::Wall => ctx.t(Key::KindWall).into(),
        BuildingKind::Tower => ctx.t(Key::KindTower).into(),
        BuildingKind::Gate => ctx.t(Key::KindGate).into(),
        BuildingKind::Workshop(profession) => ctx.f(
            Key::KindWorkshop,
            &[("profession", &format!("{profession:?}"))],
        ),
    }
}

fn category(ctx: &Ctx, kind: BuildingKind) -> &'static str {
    match kind {
        BuildingKind::House => ctx.t(Key::CategoryHousing),
        BuildingKind::Storage => ctx.t(Key::CategoryStorage),
        BuildingKind::Workshop(_) | BuildingKind::Mine | BuildingKind::LumberCamp => {
            ctx.t(Key::CategoryProduction)
        }
        BuildingKind::Barracks => ctx.t(Key::CategoryMilitary),
        BuildingKind::Wall | BuildingKind::Tower | BuildingKind::Gate => {
            ctx.t(Key::CategoryFortification)
        }
    }
}

fn all_kinds() -> Vec<BuildingKind> {
    let mut kinds = vec![BuildingKind::House, BuildingKind::Storage];
    kinds.extend(
        [
            Profession::Armorer,
            Profession::Butcher,
            Profession::Cartographer,
            Profession::Cleric,
            Profession::Farmer,
            Profession::Fisherman,
            Profession::Fletcher,
            Profession::Leatherworker,
            Profession::Librarian,
            Profession::Mason,
            Profession::Shepherd,
            Profession::Toolsmith,
            Profession::Weaponsmith,
        ]
        .map(BuildingKind::Workshop),
    );
    kinds.extend([
        BuildingKind::Mine,
        BuildingKind::LumberCamp,
        BuildingKind::Barracks,
        BuildingKind::Wall,
        BuildingKind::Tower,
        BuildingKind::Gate,
    ]);
    kinds
}

fn missing_text(error: &CertError) -> String {
    error
        .missing
        .iter()
        .map(|requirement| {
            if requirement.required <= 1 && requirement.found == 0 {
                requirement.description.clone()
            } else {
                format!(
                    "{} {}/{}",
                    requirement.description, requirement.found, requirement.required
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn block_name(state: &TickState, at: [i32; 3]) -> Option<&str> {
    let native = state.world.block_at(at[0], at[1], at[2])?;
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(native))
        .map(|block| block.block.id.as_str())
}

/// Live bound residents, deterministic order (uuid).
fn residents(state: &TickState, id: SettlementId) -> Vec<Resident> {
    let mut people = Vec::new();
    for slots in state.entities.villager_chunks().values() {
        for &entity in slots {
            let Some(slot) = state.entities.slot_of(entity) else {
                continue;
            };
            if !state.entities.body(slot).alive {
                continue;
            }
            let row = state.entities.row(slot);
            let Some(villager) = row.villager.as_ref() else {
                continue;
            };
            if villager.settlement != Some(id) || villager.wandering {
                continue;
            }
            people.push(Resident {
                uuid: row.uuid,
                profession: villager.data.profession,
                job: buildings::building_for_job(state, villager).map(|building| building.id),
                soldier: villager.soldier.clone(),
            });
        }
    }
    people.sort_by_key(|resident| resident.uuid);
    people
}

fn busy_workers(state: &TickState, id: SettlementId) -> BTreeSet<Uuid> {
    state
        .settlements
        .get(id)
        .into_iter()
        .flat_map(|record| record.economy.production.jobs.values())
        .filter(|job| !job.cancelled && job.done < job.count)
        .map(|job| job.worker)
        .collect()
}

// ------------------------------------------------------------------------ main

fn storage_summary(ctx: &Ctx, state: &TickState, id: SettlementId) -> String {
    let points = buildings::storage_points(state, id);
    if points.is_empty() {
        return ctx.t(Key::StorageNone).to_owned();
    }
    let dimension = state.dimension.name();
    let mut totals = std::collections::BTreeMap::<&str, i64>::new();
    for at in points.iter().take(64) {
        let endpoint = inventory::Endpoint::Container {
            dimension: dimension.to_owned(),
            position: *at,
        };
        let Ok(image) = inventory::load(state, &endpoint) else {
            continue;
        };
        for stack in image.slots.iter().filter(|stack| !stack.is_empty()) {
            if let Some(name) = state.registry.items.name_of(stack.item_id) {
                *totals.entry(name.path()).or_default() += i64::from(stack.count);
            }
        }
    }
    let mut top: Vec<_> = totals.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let items = if top.is_empty() {
        ctx.t(Key::Empty).to_owned()
    } else {
        top.iter()
            .take(6)
            .map(|(name, count)| {
                ctx.f(
                    Key::ItemCount,
                    &[("name", name), ("count", &count.to_string())],
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    ctx.f(
        Key::StorageCount,
        &[("containers", &points.len().to_string()), ("items", &items)],
    )
}

fn main_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let people = residents(state, ctx.id);
    if record.owner.is_none() {
        let text = ctx.f(
            Key::UnclaimedBody,
            &[
                ("residents", &people.len().to_string()),
                ("buildings", &record.buildings.len().to_string()),
            ],
        );
        let buttons = vec![
            button(ctx.t(Key::Claim), "claim", ctx.id),
            button(ctx.t(Key::Buildings), "buildings", ctx.id),
            button(ctx.t(Key::ResidentsJobs), "residents", ctx.id),
            button(ctx.t(Key::Projects), "projects", ctx.id),
        ];
        let dialog = frame(
            ctx,
            ctx.f(Key::TitleSettlement, &[("name", &record.name)]),
            note,
            text,
            buttons,
        );
        return present(state, connection, dialog);
    }
    let working = people
        .iter()
        .filter(|resident| resident.job.is_some())
        .count();
    let owner = record
        .owner_name
        .as_deref()
        .or(record.owner.as_deref())
        .unwrap_or(ctx.t(Key::Autonomous));
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for building in record.buildings.values() {
        *counts.entry(category(ctx, building.kind)).or_default() += 1;
    }
    let certified = if counts.is_empty() {
        ctx.t(Key::NoneYet).to_owned()
    } else {
        counts
            .iter()
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = ctx.f(
        Key::MainBody,
        &[
            ("owner", owner),
            ("radius", &record.radius.to_string()),
            ("x", &record.anchor[0].to_string()),
            ("y", &record.anchor[1].to_string()),
            ("z", &record.anchor[2].to_string()),
            ("residents", &people.len().to_string()),
            ("working", &working.to_string()),
            ("idle", &(people.len() - working).to_string()),
            ("certified", &certified),
            ("storage", &storage_summary(ctx, state, ctx.id)),
        ],
    );
    // Workers whose profession has no certified job: only certified workshops give jobs.
    // Soldiers serve in barracks, not workshops, so they never count as stranded.
    let stranded = people
        .iter()
        .filter(|resident| {
            resident.job.is_none()
                && !matches!(
                    resident.profession,
                    Profession::None | Profession::Nitwit | Profession::Soldier
                )
        })
        .count();
    if stranded > 0 {
        text.push('\n');
        text.push_str(&ctx.f(Key::Stranded, &[("stranded", &stranded.to_string())]));
    }
    if !ctx.manage {
        text.push('\n');
        text.push_str(ctx.t(Key::ViewOnlyOwner));
    }
    let title = ctx.f(Key::TitleSettlement, &[("name", &record.name)]);
    let buttons = vec![
        button(ctx.t(Key::Buildings), "buildings", ctx.id),
        button(ctx.t(Key::ResidentsJobs), "residents", ctx.id),
        button(ctx.t(Key::ProductionOrders), "orders", ctx.id),
        button(ctx.t(Key::Army), "army", ctx.id),
        button(ctx.t(Key::Projects), "projects", ctx.id),
        button(ctx.t(Key::Settings), "settings", ctx.id),
    ];
    present(state, connection, frame(ctx, title, note, text, buttons));
}

// ------------------------------------------------------------------- buildings

fn buildings_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let people = residents(state, ctx.id);
    let mut lines = Vec::new();
    let mut buttons = Vec::new();
    for building in record.buildings.values().take(MAX_LISTED) {
        let seq = short(building.id);
        let mut line = format!(
            "#{seq} {} L{} [{} .. {}]",
            kind_label(ctx, building.kind),
            building.level,
            corner_text(building.bounds.min),
            corner_text(building.bounds.max),
        );
        let slots = buildings::job_slots(building.kind, building.level);
        if slots > 0 {
            let filled = people
                .iter()
                .filter(|resident| resident.job == Some(building.id))
                .count();
            line.push_str(&ctx.f(
                Key::JobsRow,
                &[
                    ("filled", &filled.to_string()),
                    ("slots", &slots.to_string()),
                ],
            ));
        }
        if building.level >= 3 {
            line.push_str(ctx.t(Key::MaxLevel));
        } else {
            let next = building.level + 1;
            match buildings::inspect(state, building.kind, next, building.bounds) {
                Ok(_) => line.push_str(&ctx.f(Key::ReadyFor, &[("next", &next.to_string())])),
                Err(error) => {
                    line.push_str(&ctx.f(
                        Key::NeedsNext,
                        &[
                            ("next", &next.to_string()),
                            ("missing", &missing_text(&error)),
                        ],
                    ));
                }
            }
            if ctx.manage {
                buttons.push(
                    button(
                        ctx.f(
                            Key::UpgradeTo,
                            &[("seq", &seq.to_string()), ("next", &next.to_string())],
                        ),
                        "upgrade",
                        ctx.id,
                    )
                    .with("b", Tag::String(building.id.0.to_string())),
                );
            }
        }
        lines.push(line);
    }
    if record.buildings.len() > MAX_LISTED {
        lines.push(ctx.f(
            Key::More,
            &[("n", &(record.buildings.len() - MAX_LISTED).to_string())],
        ));
    }
    let mut text = if lines.is_empty() {
        ctx.t(Key::NoBuildings).to_owned()
    } else {
        lines.join("\n")
    };
    if !ctx.manage {
        text.push('\n');
        text.push_str(ctx.t(Key::ViewOnly));
    }
    if ctx.manage {
        buttons.push(button(ctx.t(Key::CertifyNew), "certify", ctx.id));
    }
    buttons.push(button(ctx.t(Key::Back), "main", ctx.id));
    let title = ctx.f(Key::TitleBuildings, &[("name", &record.name)]);
    present(state, connection, frame(ctx, title, note, text, buttons));
}

fn form_of(state: &TickState, connection: ConnectionId, payload: ClickPayload<'_>) -> Form {
    let marks = state
        .settlement_marks
        .get(&connection)
        .copied()
        .unwrap_or_default();
    let here = player_pos(state, connection);
    let default_corner =
        |mark: Option<[i32; 3]>| mark.or(here).map(corner_text).unwrap_or_default();
    Form {
        kind: payload.string("kind").unwrap_or("house").to_owned(),
        from: payload
            .string("from")
            .map_or_else(|| default_corner(marks[0]), str::to_owned),
        to: payload
            .string("to")
            .map_or_else(|| default_corner(marks[1]), str::to_owned),
        extra: payload.float("extra").unwrap_or(4.0),
    }
}

fn set_mark(state: &mut TickState, connection: ConnectionId, index: usize, at: [i32; 3]) {
    if state.settlement_marks.len() >= MAX_MARKS
        && !state.settlement_marks.contains_key(&connection)
    {
        let connections = &state.connections;
        state
            .settlement_marks
            .retain(|other, _| connections.entity_of(*other).is_some());
    }
    state.settlement_marks.entry(connection).or_default()[index] = Some(at);
}

fn certify_screen(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    form: &Form,
    note: &str,
) {
    let kinds = all_kinds();
    let selected = BuildingKind::parse(&form.kind).unwrap_or(BuildingKind::House);
    let options = kinds
        .iter()
        .map(|kind| {
            (
                kind.name().to_owned(),
                Tag::String(format!(
                    "{}: {}",
                    category(ctx, *kind),
                    kind_label(ctx, *kind)
                )),
            )
        })
        .collect();
    // Requirements are reported on failure (and per building under "needs"), not
    // as a paragraph: that keeps the form inside a small window.
    // Short labels, one button row and no Close button (Escape closes): the
    // form must fit a small window without scrolling.
    let buttons = vec![
        button(ctx.t(Key::Certify), "certify_do", ctx.id)
            .width(100)
            .tooltip(ctx.t(Key::TipCorners)),
        button(ctx.t(Key::Corner1), "mark1", ctx.id)
            .width(100)
            .tooltip(ctx.t(Key::TipCorner1)),
        button(ctx.t(Key::Corner2), "mark2", ctx.id)
            .width(100)
            .tooltip(ctx.t(Key::TipCorner2)),
        button(ctx.t(Key::Back), "buildings", ctx.id).width(100),
    ];
    let mut dialog = Dialog::multi_action(ctx.t(Key::TitleCertify), buttons).columns(4);
    if !note.is_empty() {
        dialog = dialog.text(note);
    }
    let dialog = dialog
        .input(
            Input::single_option("kind", ctx.t(Key::LabelKind), options)
                .initial_option(selected.name()),
        )
        .input(
            Input::text("from", ctx.t(Key::LabelFrom))
                .initial_text(&form.from)
                .max_length(40),
        )
        .input(
            Input::text("to", ctx.t(Key::LabelTo))
                .initial_text(&form.to)
                .max_length(40),
        )
        .input(
            Input::number_range("extra", ctx.t(Key::LabelExtra), 0.0, MAX_EXTRA_HEIGHT)
                .initial_number(form.extra.clamp(0.0, MAX_EXTRA_HEIGHT).round())
                .step(1.0),
        );
    present(state, connection, dialog);
}

fn corner(text: &str) -> Option<[i32; 3]> {
    let mut parts = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|part| !part.is_empty());
    let at = [
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ];
    parts.next().is_none().then_some(at)
}

fn certify(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, payload: ClickPayload<'_>) {
    let form = form_of(state, connection, payload);
    let Some(kind) = BuildingKind::parse(&form.kind) else {
        return certify_screen(state, connection, ctx, &form, ctx.t(Key::ChooseKind));
    };
    let (Some(a), Some(b)) = (corner(&form.from), corner(&form.to)) else {
        return certify_screen(state, connection, ctx, &form, ctx.t(Key::BadCorners));
    };
    let extra = form.extra.clamp(0.0, MAX_EXTRA_HEIGHT) as i32;
    let min = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
    let mut max = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
    max[1] = max[1].saturating_add(extra);
    let Ok(bounds) = ScriptWorldBounds::new(min, max) else {
        return certify_screen(state, connection, ctx, &form, ctx.t(Key::InvalidCorners));
    };
    match buildings::certify(state, ctx.id, kind, bounds) {
        Ok(id) => {
            state.settlement_marks.remove(&connection);
            let note = ctx.f(
                Key::Certified,
                &[
                    ("kind", &kind_label(ctx, kind)),
                    ("id", &short(id).to_string()),
                ],
            );
            buildings_screen(state, connection, ctx, &note);
        }
        Err(error) => {
            let note = ctx.f(
                Key::CannotCertify,
                &[
                    ("kind", &kind_label(ctx, kind)),
                    ("missing", &missing_text(&error)),
                ],
            );
            certify_screen(state, connection, ctx, &form, &note);
        }
    }
}

fn building_of(state: &TickState, ctx: &Ctx, payload: ClickPayload<'_>) -> Option<BuildingId> {
    let id = BuildingId(payload.string("b")?.parse().ok()?);
    state
        .settlements
        .get(ctx.id)?
        .buildings
        .contains_key(&id)
        .then_some(id)
}

fn upgrade(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, payload: ClickPayload<'_>) {
    let Some(id) = building_of(state, ctx, payload) else {
        return buildings_screen(state, connection, ctx, ctx.t(Key::BuildingGone));
    };
    let note = match buildings::upgrade(state, id) {
        Ok(()) => {
            let level = state
                .settlements
                .get(ctx.id)
                .and_then(|record| record.buildings.get(&id))
                .map_or(0, |building| building.level);
            ctx.f(
                Key::Upgraded,
                &[
                    ("id", &short(id).to_string()),
                    ("level", &level.to_string()),
                ],
            )
        }
        Err(error) => ctx.f(
            Key::CannotUpgrade,
            &[
                ("id", &short(id).to_string()),
                ("missing", &missing_text(&error)),
            ],
        ),
    };
    buildings_screen(state, connection, ctx, &note);
}

// ------------------------------------------------------------------- residents

fn residents_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let people = residents(state, ctx.id);
    let mut lines = Vec::new();
    let mut free = 0;
    for building in record.buildings.values() {
        let slots = buildings::job_slots(building.kind, building.level);
        if building.kind == BuildingKind::Barracks {
            let filled = people
                .iter()
                .filter(|resident| {
                    resident
                        .soldier
                        .as_ref()
                        .is_some_and(|soldier| soldier.barracks == building.id)
                })
                .count();
            let beds = slots;
            lines.push(ctx.f(
                Key::SoldiersRow,
                &[
                    ("id", &short(building.id).to_string()),
                    ("kind", &kind_label(ctx, building.kind)),
                    ("level", &building.level.to_string()),
                    ("filled", &filled.to_string()),
                    ("beds", &beds.to_string()),
                ],
            ));
            continue;
        }
        if slots == 0 {
            continue;
        }
        let filled = people
            .iter()
            .filter(|resident| resident.job == Some(building.id))
            .count();
        free += slots.saturating_sub(filled);
        lines.push(ctx.f(
            Key::JobsFilledRow,
            &[
                ("id", &short(building.id).to_string()),
                ("kind", &kind_label(ctx, building.kind)),
                ("level", &building.level.to_string()),
                ("filled", &filled.to_string()),
                ("slots", &slots.to_string()),
            ],
        ));
    }
    lines.push(
        ctx.f(
            Key::FreeSlots,
            &[
                ("free", &free.to_string()),
                (
                    "people",
                    &people
                        .iter()
                        .filter(|resident| resident.job.is_none() && resident.soldier.is_none())
                        .count()
                        .to_string(),
                ),
            ],
        ),
    );
    lines.push(String::new());
    for resident in people.iter().take(MAX_RESIDENTS) {
        let name = super::presentation::villager_name(resident.uuid);
        let place = resident
            .job
            .and_then(|id| record.buildings.get(&id))
            .map_or_else(
                || ctx.t(Key::NoJob).to_owned(),
                |building| {
                    ctx.f(
                        Key::WorksAt,
                        &[
                            ("id", &short(building.id).to_string()),
                            ("kind", &kind_label(ctx, building.kind)),
                        ],
                    )
                },
            );
        let duty = resident
            .soldier
            .as_ref()
            .map(|soldier| {
                ctx.f(
                    Key::SoldierDuty,
                    &[("order", &format!("{:?}", soldier.order))],
                )
            })
            .unwrap_or_default();
        lines.push(format!("{name} ({:?}): {place}{duty}", resident.profession));
    }
    if people.len() > MAX_RESIDENTS {
        lines.push(ctx.f(
            Key::More,
            &[("n", &(people.len() - MAX_RESIDENTS).to_string())],
        ));
    }
    let title = ctx.f(Key::TitleResidents, &[("name", &record.name)]);
    let buttons = vec![button(ctx.t(Key::Back), "main", ctx.id)];
    present(
        state,
        connection,
        frame(ctx, title, "", lines.join("\n"), buttons),
    );
}

// ---------------------------------------------------------------------- orders

fn job_status(ctx: &Ctx, job: &super::production::Job) -> String {
    if job.cancelled {
        ctx.t(Key::JobCancelled).to_owned()
    } else if job.done >= job.count {
        ctx.t(Key::JobDone).to_owned()
    } else {
        match job.paused.as_deref() {
            Some("paused") => ctx.t(Key::JobPaused).to_owned(),
            Some(reason) => ctx.f(Key::JobWaiting, &[("reason", reason)]),
            None => ctx.t(Key::JobWorking).to_owned(),
        }
    }
}

fn orders_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let mut jobs: Vec<_> = record.economy.production.jobs.values().collect();
    jobs.sort_by_key(|job| (job.cancelled || job.done >= job.count, job.accepted_tick));
    let mut lines = Vec::new();
    let mut buttons = Vec::new();
    for job in jobs.iter().take(MAX_LISTED) {
        let recipe = job.recipe.rsplit(':').next().unwrap_or(&job.recipe);
        lines.push(ctx.f(
            Key::OrderRow,
            &[
                ("id", &job.id),
                ("recipe", recipe),
                ("done", &job.done.to_string()),
                ("count", &job.count.to_string()),
                ("worker", super::presentation::villager_name(job.worker)),
                ("status", &job_status(ctx, job)),
            ],
        ));
        if ctx.manage && !job.cancelled && job.done < job.count {
            let (label, action) = if job.paused.as_deref() == Some("paused") {
                (ctx.f(Key::Resume, &[("id", &job.id)]), "order_resume")
            } else {
                (ctx.f(Key::Pause, &[("id", &job.id)]), "order_pause")
            };
            buttons.push(button(label, action, ctx.id).with("j", Tag::String(job.id.clone())));
            buttons.push(
                button(
                    ctx.f(Key::Cancel, &[("id", &job.id)]),
                    "order_cancel",
                    ctx.id,
                )
                .with("j", Tag::String(job.id.clone())),
            );
        }
    }
    if jobs.len() > MAX_LISTED {
        lines.push(ctx.f(Key::More, &[("n", &(jobs.len() - MAX_LISTED).to_string())]));
    }
    let mut text = if lines.is_empty() {
        ctx.t(Key::NoOrders).to_owned()
    } else {
        lines.join("\n")
    };
    if ctx.manage {
        text.push('\n');
        text.push_str(ctx.t(Key::NewOrderHelp));
        buttons.insert(0, button(ctx.t(Key::NewOrder), "order_start", ctx.id));
    } else {
        text.push('\n');
        text.push_str(ctx.t(Key::ViewOnly));
    }
    buttons.push(button(ctx.t(Key::Back), "main", ctx.id));
    let title = ctx.f(Key::TitleOrders, &[("name", &record.name)]);
    present(state, connection, frame(ctx, title, note, text, buttons));
}

/// Crafting stations inside certified workshops, a bounded scan on click only.
fn stations(state: &TickState, id: SettlementId) -> Vec<Station> {
    let wanted: BTreeSet<&str> = state
        .registry
        .item_facts
        .work_recipes()
        .map(|recipe| recipe.station.as_str())
        .collect();
    let mut found = Vec::new();
    let Some(record) = state.settlements.get(id) else {
        return found;
    };
    for building in record
        .buildings
        .values()
        .filter(|building| matches!(building.kind, BuildingKind::Workshop(_)))
        .take(MAX_LISTED)
    {
        let mut here = 0;
        'scan: for y in building.bounds.min[1]..=building.bounds.max[1] {
            for z in building.bounds.min[2]..=building.bounds.max[2] {
                for x in building.bounds.min[0]..=building.bounds.max[0] {
                    let Some(name) = block_name(state, [x, y, z]) else {
                        continue;
                    };
                    if wanted.contains(name) {
                        found.push(Station {
                            building: building.id,
                            kind: building.kind,
                            at: [x, y, z],
                            block: name.to_owned(),
                        });
                        here += 1;
                        if here == MAX_STATIONS {
                            break 'scan;
                        }
                    }
                }
            }
        }
    }
    found
}

fn order_start(state: &mut TickState, connection: ConnectionId, ctx: &Ctx) {
    let found = stations(state, ctx.id);
    match found.as_slice() {
        [] => orders_screen(state, connection, ctx, ctx.t(Key::NoStation)),
        [only] => recipes_screen(state, connection, ctx, only.building, only.at),
        _ => {
            let buttons = found
                .iter()
                .map(|station| {
                    button(
                        ctx.f(
                            Key::StationRow,
                            &[
                                ("id", &short(station.building).to_string()),
                                ("kind", &kind_label(ctx, station.kind)),
                                (
                                    "block",
                                    station.block.rsplit(':').next().unwrap_or(&station.block),
                                ),
                                ("at", &corner_text(station.at)),
                            ],
                        ),
                        "order_pick",
                        ctx.id,
                    )
                    .with("b", Tag::String(station.building.0.to_string()))
                    .with("p", Tag::String(corner_text(station.at)))
                    .width(300)
                })
                .chain([button(ctx.t(Key::Back), "orders", ctx.id)])
                .collect();
            let dialog = frame(
                ctx,
                ctx.t(Key::ChooseStation).to_owned(),
                "",
                ctx.t(Key::WhereWork).to_owned(),
                buttons,
            )
            .columns(1);
            present(state, connection, dialog);
        }
    }
}

fn order_pick(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let (Some(building), Some(at)) = (
        building_of(state, ctx, payload),
        payload.string("p").and_then(corner),
    ) else {
        return orders_screen(state, connection, ctx, ctx.t(Key::StationGone));
    };
    recipes_screen(state, connection, ctx, building, at);
}

fn recipes_screen(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    building: BuildingId,
    at: [i32; 3],
) {
    let Some(block) = block_name(state, at).map(str::to_owned) else {
        return orders_screen(state, connection, ctx, ctx.t(Key::StationUnloaded));
    };
    let busy = busy_workers(state, ctx.id);
    let free: Vec<Resident> = residents(state, ctx.id)
        .into_iter()
        .filter(|resident| !busy.contains(&resident.uuid))
        .take(50)
        .collect();
    if free.is_empty() {
        return orders_screen(state, connection, ctx, ctx.t(Key::NoFreeResident));
    }
    let workers = free
        .iter()
        .map(|resident| {
            (
                resident.uuid.to_string(),
                Tag::String(format!(
                    "{} ({:?})",
                    super::presentation::villager_name(resident.uuid),
                    resident.profession
                )),
            )
        })
        .collect();
    let mut buttons = Vec::new();
    for recipe in state
        .registry
        .item_facts
        .work_recipes()
        .filter(|recipe| recipe.station.as_str() == block)
        .take(60)
    {
        let outputs = recipe
            .outputs
            .iter()
            .map(|stack| format!("{} x{}", stack.item.path(), stack.count))
            .collect::<Vec<_>>()
            .join(", ");
        buttons.push(
            button(
                format!("{} -> {outputs}", recipe.id.path()),
                "order_create",
                ctx.id,
            )
            .with("b", Tag::String(building.0.to_string()))
            .with("p", Tag::String(corner_text(at)))
            .with("r", Tag::String(recipe.id.as_str().to_owned()))
            .width(300),
        );
    }
    buttons.push(button(ctx.t(Key::Back), "orders", ctx.id));
    let dialog = frame(
        ctx,
        ctx.t(Key::TitleNewOrder).to_owned(),
        "",
        ctx.f(Key::PickProduct, &[("at", &corner_text(at))]),
        buttons,
    )
    .columns(1)
    .input(
        Input::number_range("count", ctx.t(Key::Quantity), 1.0, MAX_ORDER)
            .initial_number(8.0)
            .step(1.0),
    )
    .input(Input::single_option(
        "worker",
        ctx.t(Key::Resident),
        workers,
    ));
    present(state, connection, dialog);
}

/// A free cell beside `station` to stand on: air at feet and head, support below.
/// Cells inside the building win; order is fixed for determinism.
fn stand_at(state: &TickState, station: [i32; 3], bounds: ScriptWorldBounds) -> Option<[i32; 3]> {
    let air = state.registry.states.air;
    let is_air = |at: [i32; 3]| state.world.block_at(at[0], at[1], at[2]) == Some(air);
    let solid_below = |at: [i32; 3]| {
        state
            .world
            .block_at(at[0], at[1] - 1, at[2])
            .is_some_and(|block| block != air)
    };
    let inside = |at: [i32; 3]| (0..3).all(|i| bounds.min[i] <= at[i] && at[i] <= bounds.max[i]);
    let mut candidates = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            if (dx, dz) != (0, 0) {
                candidates.push([station[0] + dx, station[1], station[2] + dz]);
            }
        }
    }
    let usable =
        |at: &[i32; 3]| is_air(*at) && is_air([at[0], at[1] + 1, at[2]]) && solid_below(*at);
    candidates
        .iter()
        .copied()
        .find(|at| inside(*at) && usable(at))
        .or_else(|| candidates.iter().copied().find(usable))
}

fn failure_text(ctx: &Ctx, failure: Failure) -> String {
    match failure {
        Failure::Busy => ctx.t(Key::FailureBusy).to_owned(),
        Failure::NotFound => ctx.t(Key::FailureNotFound).to_owned(),
        Failure::Forbidden => ctx.t(Key::FailureForbidden).to_owned(),
        Failure::Blocked => ctx.t(Key::FailureBlocked).to_owned(),
        Failure::Unloaded => ctx.t(Key::FailureUnloaded).to_owned(),
        other => ctx.f(Key::FailureOther, &[("reason", &format!("{other:?}"))]),
    }
}

fn order_create(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let (Some(building), Some(at), Some(recipe), Some(worker)) = (
        building_of(state, ctx, payload),
        payload.string("p").and_then(corner),
        payload
            .string("r")
            .and_then(|name| Identifier::parse(name).ok()),
        payload
            .string("worker")
            .and_then(|text| text.parse::<Uuid>().ok()),
    ) else {
        return orders_screen(state, connection, ctx, ctx.t(Key::OrderIncomplete));
    };
    let count = payload
        .float("count")
        .map_or(1, |value| value.clamp(1.0, MAX_ORDER) as u32);
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let Some(found) = record.buildings.get(&building).cloned() else {
        return orders_screen(state, connection, ctx, ctx.t(Key::WorkshopGone));
    };
    let valid = matches!(found.kind, BuildingKind::Workshop(_))
        && (0..3).all(|i| found.bounds.min[i] <= at[i] && at[i] <= found.bounds.max[i])
        && state
            .registry
            .item_facts
            .work_recipe(&recipe)
            .is_some_and(|work| block_name(state, at) == Some(work.station.as_str()));
    if !valid {
        return orders_screen(state, connection, ctx, ctx.t(Key::WrongStation));
    }
    let Some(stand) = stand_at(state, at, found.bounds) else {
        return orders_screen(state, connection, ctx, ctx.t(Key::NoStand));
    };
    let production = &record.economy.production;
    let mut sequence = production.sequence.saturating_add(1);
    let job = loop {
        let candidate = format!("ui-{sequence}");
        if !production.jobs.contains_key(&candidate) {
            break candidate;
        }
        sequence += 1;
    };
    let action = Action::ProductionStart {
        job: job.clone(),
        worker: worker.to_string(),
        recipe: recipe.as_str().to_owned(),
        count,
        building: ScriptSettlementBuildingRef::new(
            "building".to_owned(),
            building.0.to_string(),
            format!("solaris:{}", found.kind.name()),
        ),
        station: at,
        stand_at: stand,
    };
    let note = match economy_customization::execute(state, ctx.id, &action) {
        Ok(()) => ctx.f(
            Key::OrderStarted,
            &[
                ("job", &job),
                ("item", recipe.path()),
                ("count", &count.to_string()),
            ],
        ),
        Err(failure) => failure_text(ctx, failure),
    };
    orders_screen(state, connection, ctx, &note);
}

fn order_control(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    action: &str,
    payload: ClickPayload<'_>,
) {
    let Some(job) = payload.string("j").map(str::to_owned).filter(|job| {
        state
            .settlements
            .get(ctx.id)
            .is_some_and(|record| record.economy.production.jobs.contains_key(job))
    }) else {
        return orders_screen(state, connection, ctx, ctx.t(Key::OrderGone));
    };
    let request = match action {
        "order_pause" => Action::ProductionPause { job: job.clone() },
        "order_resume" => Action::ProductionResume { job: job.clone() },
        _ => Action::ProductionCancel { job: job.clone() },
    };
    let note = match economy_customization::execute(state, ctx.id, &request) {
        Ok(()) => ctx.f(Key::OrderUpdated, &[("job", &job)]),
        Err(failure) => failure_text(ctx, failure),
    };
    orders_screen(state, connection, ctx, &note);
}

// ------------------------------------------------------------------------ army

/// First eight hex digits of a soldier UUID: the roster label. Clicks carry the full UUID.
fn soldier_id(uuid: Uuid) -> String {
    uuid.to_string()[..8].to_owned()
}

fn order_word(order: &super::army::Order) -> String {
    use super::army::Order;
    match order {
        Order::Follow(_) => "Follow".to_owned(),
        Order::Guard(at) => format!("Guard {}", corner_text(*at)),
        Order::Patrol => "Patrol".to_owned(),
        Order::Return => "Return".to_owned(),
    }
}

/// Native soldiers. The roster is one short line and one `Soldier` button per
/// soldier, so its button grid stays small however many soldiers there are; the
/// per-soldier orders and dismissal live on [`soldier_screen`]. The all-soldier
/// orders stay here. Follow targets the clicking player; guard posts at their
/// feet. Reads are open; mutations require `manage` (checked before dispatch,
/// re-read here for the live executor pose).
fn army_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let rows = super::army::rows(state, ctx.id);
    let mut text = if rows.is_empty() {
        ctx.t(Key::NoSoldiers).to_owned()
    } else {
        rows.iter()
            .take(MAX_RESIDENTS)
            .map(|row| {
                format!(
                    "{} — {:.0} HP — {}",
                    soldier_id(row.uuid),
                    row.health,
                    order_word(&row.order)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    if !ctx.manage {
        text.push('\n');
        text.push_str(ctx.t(Key::ViewOnly));
    }
    let mut buttons = Vec::new();
    if ctx.manage {
        buttons.push(button(ctx.t(Key::Recruit), "army_recruit", ctx.id));
    }
    for row in rows.iter().take(MAX_RESIDENTS) {
        buttons.push(
            button(
                ctx.f(Key::SoldierButton, &[("id", &soldier_id(row.uuid))]),
                "army_soldier",
                ctx.id,
            )
            .with("u", Tag::String(row.uuid.to_string())),
        );
    }
    if ctx.manage && !rows.is_empty() {
        for (label, order) in [
            (Key::FollowMeAll, "follow"),
            (Key::GuardHereAll, "guard"),
            (Key::PatrolAll, "patrol"),
            (Key::ReturnAll, "return"),
        ] {
            buttons.push(
                button(ctx.t(label), "army_order", ctx.id)
                    .with("u", Tag::String("all".to_owned()))
                    .with("o", Tag::String(order.to_owned())),
            );
        }
    }
    buttons.push(button(ctx.t(Key::Back), "main", ctx.id));
    let title = ctx.f(Key::TitleArmy, &[("name", &record.name)]);
    present(state, connection, frame(ctx, title, note, text, buttons));
}

/// One soldier: details plus its Follow/Guard/Patrol/Return and Dismiss buttons.
/// A soldier that no longer exists falls back to the roster.
fn soldier_screen(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    uuid: Uuid,
    note: &str,
) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let Some(row) = super::army::rows(state, ctx.id)
        .into_iter()
        .find(|row| row.uuid == uuid)
    else {
        return army_screen(state, connection, ctx, ctx.t(Key::SoldierGone));
    };
    let text = format!(
        "{uuid}\n{:.0} HP — {}\n{}",
        row.health,
        order_word(&row.order),
        row.equipment.join(", ")
    );
    let mut buttons = Vec::new();
    if ctx.manage {
        for (label, order) in [
            (Key::FollowMe, "follow"),
            (Key::GuardHere, "guard"),
            (Key::Patrol, "patrol"),
            (Key::Return, "return"),
        ] {
            buttons.push(
                button(ctx.t(label), "army_order", ctx.id)
                    .with("u", Tag::String(uuid.to_string()))
                    .with("o", Tag::String(order.to_owned())),
            );
        }
        buttons.push(
            button(ctx.t(Key::Dismiss), "army_dismiss", ctx.id)
                .with("u", Tag::String(uuid.to_string())),
        );
    }
    buttons.push(button(ctx.t(Key::Back), "army", ctx.id));
    let title = ctx.f(
        Key::TitleSoldier,
        &[("id", &soldier_id(uuid)), ("name", &record.name)],
    );
    present(state, connection, frame(ctx, title, note, text, buttons));
}

/// Back to the soldier a click targeted, or to the roster for an all-soldier click.
fn army_target(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    target: Option<Uuid>,
    note: &str,
) {
    match target {
        Some(uuid) => soldier_screen(state, connection, ctx, uuid, note),
        None => army_screen(state, connection, ctx, note),
    }
}

fn army_soldier(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    match payload.string("u").and_then(|text| text.parse().ok()) {
        Some(uuid) => soldier_screen(state, connection, ctx, uuid, ""),
        None => army_screen(state, connection, ctx, ctx.t(Key::SoldierGone)),
    }
}

/// A successful recruit lands on the new soldier's page: it has no Recruit
/// button, so a repeated click on the same spot cannot recruit a second soldier.
fn army_recruit(state: &mut TickState, connection: ConnectionId, ctx: &Ctx) {
    match super::army::recruit(state, ctx.id) {
        Ok(uuid) => {
            let note = ctx.f(Key::Recruited, &[("uuid", &soldier_id(uuid))]);
            soldier_screen(state, connection, ctx, uuid, &note);
        }
        Err(message) => army_screen(state, connection, ctx, &message),
    }
}

fn army_dismiss(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let Some(uuid) = payload.string("u").and_then(|text| text.parse().ok()) else {
        return army_screen(state, connection, ctx, ctx.t(Key::SoldierGone));
    };
    let note = match super::army::dismiss(state, ctx.id, uuid) {
        Ok(()) => ctx.f(Key::Dismissed, &[("uuid", &soldier_id(uuid))]),
        Err(message) => message,
    };
    army_screen(state, connection, ctx, &note);
}

fn army_order(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let target = payload.string("u").unwrap_or("all");
    let uuid = if target == "all" {
        None
    } else if let Ok(uuid) = target.parse() {
        Some(uuid)
    } else {
        return army_screen(state, connection, ctx, ctx.t(Key::SoldierGone));
    };
    let order = match payload.string("o") {
        Some("follow") => {
            let Some(executor) = state.connections.entity_of(connection) else {
                return army_target(state, connection, ctx, uuid, ctx.t(Key::PositionUnknown));
            };
            let Some(slot) = state.entities.slot_of(executor) else {
                return army_target(state, connection, ctx, uuid, ctx.t(Key::PositionUnknown));
            };
            super::army::Order::Follow(state.entities.row(slot).uuid)
        }
        Some("guard") => match player_pos(state, connection) {
            Some(at) => super::army::Order::Guard(at),
            None => {
                return army_target(state, connection, ctx, uuid, ctx.t(Key::PositionUnknown));
            }
        },
        Some("patrol") => super::army::Order::Patrol,
        Some("return") => super::army::Order::Return,
        _ => return army_target(state, connection, ctx, uuid, ctx.t(Key::ChooseOrder)),
    };
    let note = match super::army::order(state, ctx.id, uuid, order) {
        Ok(count) => ctx.f(Key::Ordered, &[("count", &count.to_string())]),
        Err(message) => message,
    };
    army_target(state, connection, ctx, uuid, &note);
}

// -------------------------------------------------------------------- projects

/// Owner order buttons for every authored blueprint, `Upgrade` buttons for
/// blueprint-built buildings with a next level, and the `Status` page.
///
/// A dialog renders its body above the action grid, in one scroll column, so a
/// status block here would push every button below the fold. The body therefore
/// holds only the click note: the grid stays the first thing in the column, and
/// the live project status sits behind `Status` ([`project_status_screen`]).
/// Clicks route through the `MUTATING` allowlist; forged payloads revalidate.
fn projects_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let mut buttons = Vec::new();
    if ctx.manage {
        for blueprint in super::blueprints::all(&state.registry) {
            buttons.push(
                button(
                    ctx.f(Key::OrderBlueprint, &[("blueprint", &blueprint.id)]),
                    "project_order",
                    ctx.id,
                )
                .with("blueprint", Tag::String(blueprint.id.clone())),
            );
        }
        for building in record
            .buildings
            .values()
            .filter(|building| super::construction::upgrade_available(state, ctx.id, building.id))
            .take(MAX_LISTED)
        {
            buttons.push(
                button(
                    ctx.f(
                        Key::UpgradeRow,
                        &[
                            ("id", &short(building.id).to_string()),
                            ("kind", &kind_label(ctx, building.kind)),
                            ("level", &building.level.to_string()),
                            ("next", &(building.level + 1).to_string()),
                        ],
                    ),
                    "project_upgrade",
                    ctx.id,
                )
                .with("b", Tag::String(building.id.0.to_string())),
            );
        }
    }
    buttons.push(button(ctx.t(Key::Status), "project_status", ctx.id));
    buttons.push(button(ctx.t(Key::Back), "main", ctx.id));
    let text = if ctx.manage {
        String::new()
    } else {
        ctx.t(Key::ViewOnly).to_owned()
    };
    let title = ctx.f(Key::TitleProjects, &[("name", &record.name)]);
    present(state, connection, frame(ctx, title, note, text, buttons));
}

/// Live project progress and all unmet bill entries.
fn project_status_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let waiting = super::construction::waiting(state, ctx.id);
    let title = ctx.f(Key::TitleProjectStatus, &[("name", &record.name)]);
    let buttons = vec![button(ctx.t(Key::Back), "projects", ctx.id)];
    let mut dialog = Dialog::multi_action(title, buttons);
    if let Some(project) = &record.project {
        match crate::plugin_structures::settlement_builds::status(state, ctx.id, &project.structure) {
            Ok(snapshot) => {
                let total: u64 = snapshot.stages.iter().map(|stage| stage.work_units).sum();
                let done: u64 = snapshot.stages.iter()
                    .take(snapshot.current_stage_index as usize)
                    .map(|stage| stage.work_units).sum::<u64>() + snapshot.completed_work_units;
                let name = blueprint_label(ctx, &project.blueprint);
                let text = match ctx.lang {
                    Lang::En => format!("{name} · level {} · {done}/{total}\nSite: {}, {}, {}", project.level, project.origin[0], project.origin[1], project.origin[2]),
                    Lang::Ru => format!("{name} · уровень {} · {done}/{total}\nМесто: {}, {}, {}", project.level, project.origin[0], project.origin[1], project.origin[2]),
                };
                dialog = dialog.text(text);
                match crate::plugin_structures::settlement_builds::missing(state, ctx.id, &project.structure) {
                    Ok(missing) => {
                        if !missing.is_empty() {
                            let items = missing.into_iter().map(|(item, count)| {
                                let suffix = match ctx.lang {
                                    Lang::En => format!(" — missing {count}\n"),
                                    Lang::Ru => format!(" — не хватает {count}\n"),
                                };
                                material_text(state, &item, suffix)
                            }).collect();
                            dialog = dialog.text(material_group("", items));
                        }
                    }
                    Err(_) => dialog = dialog.text(match ctx.lang {
                        Lang::En => "Storage unavailable.",
                        Lang::Ru => "Склад недоступен.",
                    }),
                }
            }
            Err(_) => dialog = dialog.text(match ctx.lang {
                Lang::En => "Project records unavailable; construction paused.",
                Lang::Ru => "Данные стройки недоступны; строительство приостановлено.",
            }),
        }
    } else {
        let text = if state.settlements.member_count(ctx.id) == 0 {
            match ctx.lang { Lang::En => "No active project: no residents.", Lang::Ru => "Нет стройки: нет жителей." }
        } else if super::construction::has_funded_candidate(state, ctx.id) {
            match ctx.lang { Lang::En => "No active project: waiting for the planner.", Lang::Ru => "Нет стройки: ожидание планирования." }
        } else if !waiting.is_empty() {
            match ctx.lang { Lang::En => "No active project: waiting for materials.", Lang::Ru => "Нет стройки: не хватает материалов." }
        } else {
            match ctx.lang { Lang::En => "No active project: no building currently needed.", Lang::Ru => "Нет стройки: сейчас новые здания не нужны." }
        };
        dialog = dialog.text(text);
    }
    for kind in &waiting {
        let items = kind.needs.iter().map(|need| {
            let reason = source_reason(ctx.lang, need.no_source);
            material_text(state, &need.item, format!(" {}/{} — {reason}\n", need.have, need.total))
        }).collect();
        dialog = dialog.text(material_group(&format!("{}:\n", blueprint_label(ctx, &kind.blueprint)), items));
    }
    present(state, connection, dialog.exit(Button::plain(ctx.t(Key::Close))));
}

fn material_group(title: &str, items: Vec<Tag>) -> Tag {
    Tag::Compound(vec![
        ("text".into(), Tag::String(title.into())),
        ("extra".into(), Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::COMPOUND,
            elements: items,
        })),
    ])
}

fn material_text(state: &TickState, item: &str, suffix: String) -> Tag {
    let Ok(id) = Identifier::parse(item) else {
        return Tag::String("Unknown material".into());
    };
    let prefix = if state.registry.blocks.block(&id).is_some() { "block" } else { "item" };
    Tag::Compound(vec![
        ("translate".into(), Tag::String(format!("{prefix}.{}.{}", id.namespace(), id.path()))),
        ("extra".into(), Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::COMPOUND,
            elements: vec![Tag::Compound(vec![("text".into(), Tag::String(suffix))])],
        })),
    ])
}

fn source_reason(lang: Lang, reason: Option<super::construction::SourceReason>) -> &'static str {
    use super::construction::SourceReason;
    match (lang, reason) {
        (Lang::En, None) => "awaiting production",
        (Lang::Ru, None) => "ожидается производство",
        (Lang::En, Some(SourceReason::Mine)) => "no source: needs a mine",
        (Lang::Ru, Some(SourceReason::Mine)) => "нет источника: нужна шахта",
        (Lang::En, Some(SourceReason::Miner)) => "no miner",
        (Lang::Ru, Some(SourceReason::Miner)) => "нет шахтёра",
        (Lang::En, Some(SourceReason::LumberCamp)) => "no source: needs a lumber camp",
        (Lang::Ru, Some(SourceReason::LumberCamp)) => "нет источника: нужна лесопилка",
        (Lang::En, Some(SourceReason::Lumberjack)) => "no lumberjack",
        (Lang::Ru, Some(SourceReason::Lumberjack)) => "нет лесоруба",
        (Lang::En, Some(SourceReason::Farm)) => "no source: needs a farm",
        (Lang::Ru, Some(SourceReason::Farm)) => "нет источника: нужна ферма",
        (Lang::En, Some(SourceReason::Farmer)) => "no farmer",
        (Lang::Ru, Some(SourceReason::Farmer)) => "нет фермера",
        (Lang::En, Some(SourceReason::Shepherd)) => "no shepherd",
        (Lang::Ru, Some(SourceReason::Shepherd)) => "нет пастуха",
        (Lang::En, Some(SourceReason::Sheep)) => "no sheep of the required color",
        (Lang::Ru, Some(SourceReason::Sheep)) => "нет овцы нужного цвета",
        (Lang::En, Some(SourceReason::Unknown)) => "no source",
        (Lang::Ru, Some(SourceReason::Unknown)) => "нет источника",
    }
}

fn blueprint_label(ctx: &Ctx, blueprint: &str) -> &'static str {
    match (ctx.lang, blueprint) {
        (Lang::En, "house") => "House", (Lang::Ru, "house") => "Дом",
        (Lang::En, "storage") => "Storage", (Lang::Ru, "storage") => "Склад",
        (Lang::En, "farm") => "Farm", (Lang::Ru, "farm") => "Ферма",
        (Lang::En, "mason") => "Mason's workshop", (Lang::Ru, "mason") => "Мастерская каменщика",
        (Lang::En, "lumber_camp") => "Lumber camp", (Lang::Ru, "lumber_camp") => "Лесопилка",
        (Lang::En, "mine") => "Mine", (Lang::Ru, "mine") => "Шахта",
        (Lang::En, "barracks") => "Barracks", (Lang::Ru, "barracks") => "Казарма",
        (Lang::En, "wall") => "Wall", (Lang::Ru, "wall") => "Стена",
        (Lang::En, "watch_tower") => "Watchtower", (Lang::Ru, "watch_tower") => "Сторожевая башня",
        (Lang::En, _) => "Building", (Lang::Ru, _) => "Здание",
    }
}

fn project_order(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let Some(blueprint) = payload.string("blueprint") else {
        return projects_screen(state, connection, ctx, ctx.t(Key::ChooseBlueprint));
    };
    let note = match super::construction::request_build(state, ctx.id, blueprint) {
        Ok(()) => ctx.f(Key::BlueprintOrdered, &[("blueprint", blueprint)]),
        Err(error) => error,
    };
    projects_screen(state, connection, ctx, &note);
}

fn project_upgrade(
    state: &mut TickState,
    connection: ConnectionId,
    ctx: &Ctx,
    payload: ClickPayload<'_>,
) {
    let Some(id) = building_of(state, ctx, payload) else {
        return projects_screen(state, connection, ctx, ctx.t(Key::BuildingGone));
    };
    let note = match super::construction::request_upgrade(state, ctx.id, id) {
        Ok(()) => ctx.f(Key::UpgradeStarted, &[("id", &short(id).to_string())]),
        Err(error) => error,
    };
    projects_screen(state, connection, ctx, &note);
}

fn settings_screen(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, note: &str) {
    let Some(record) = state.settlements.get(ctx.id) else {
        return;
    };
    let text = ctx.f(
        Key::SettingsBody,
        &[
            ("name", &record.name),
            ("radius", &record.radius.to_string()),
            ("revision", &record.revision.to_string()),
        ],
    );
    let mut buttons = Vec::new();
    if ctx.manage {
        buttons.push(button(ctx.t(Key::Rename), "rename", ctx.id));
    }
    buttons.push(button(ctx.t(Key::Back), "main", ctx.id));
    let title = ctx.f(Key::TitleSettings, &[("name", &record.name)]);
    let mut dialog = frame(ctx, title, note, text, buttons);
    if ctx.manage {
        dialog = dialog.input(
            Input::text("name", ctx.t(Key::NewName))
                .initial_text(&record.name)
                .max_length(64),
        );
    }
    present(state, connection, dialog);
}

fn rename(state: &mut TickState, connection: ConnectionId, ctx: &Ctx, payload: ClickPayload<'_>) {
    let Some(name) = payload.string("name") else {
        return settings_screen(state, connection, ctx, ctx.t(Key::EnterName));
    };
    let note = match state.settlements.rename(ctx.id, name.to_owned()) {
        Ok(()) => ctx.f(Key::Renamed, &[("name", name)]),
        Err(message) => message.to_owned(),
    };
    settings_screen(state, connection, ctx, &note);
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "army_ui_tests.rs"]
mod army_ui_tests;
