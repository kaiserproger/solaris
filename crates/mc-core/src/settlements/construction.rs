//! Settlement planning and certification. Physical construction, material
//! debit, placement order and progress belong solely to the structure executor.
use serde::{Deserialize, Serialize};

use super::blueprints::{self, Rotation};
use super::buildings::{self, BuildingId, BuildingKind};
use super::{SettlementId, SettlementRecord};
use crate::plugin_structures::settlement_builds as executor;
use crate::tick::TickState;
use crate::villager::VillagerProfession as Profession;
use mc_script::{ScriptStructureState, ScriptWorldBounds};

#[path = "construction_availability.rs"]
mod availability;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub settlement: SettlementId,
    pub blueprint: String,
    pub level: u8,
    pub origin: [i32; 3],
    /// Clockwise quarter turns.
    pub rotation: u8,
    pub building: Option<BuildingId>,
    /// The canonical structure ledger owns all work/material progress.
    pub structure: String,
    /// Set by an owner/operator order. The planner never preempts it; an
    /// unordered (autonomous) project that placed nothing yields to an order.
    #[serde(default)]
    pub ordered: bool,
}

/// `Busy.` unless the active project is autonomous and has placed nothing.
/// Returns whether such a project must be cancelled to admit the order.
fn claim_slot(state: &TickState, id: SettlementId) -> Result<bool, String> {
    let record = state.settlements.get(id).ok_or("Settlement not found.")?;
    let Some(project) = &record.project else {
        return Ok(false);
    };
    let untouched = !project.ordered
        && executor::status(state, id, &project.structure)
            .is_ok_and(|snapshot| snapshot.completed_work_units == 0);
    if untouched {
        Ok(true)
    } else {
        Err("Busy.".into())
    }
}

/// Cancel the untouched autonomous project: block stock and builder carry
/// stay where they are, nothing is debited or placed.
fn preempt(state: &mut TickState, id: SettlementId) -> Result<(), String> {
    let Some(handle) = state
        .settlements
        .get(id)
        .and_then(|record| record.project.as_ref())
        .map(|project| project.structure.clone())
    else {
        return Ok(());
    };
    executor::cancel_structure(state, id, &handle)
        .map_err(|_| "Construction admission failed.".to_string())?;
    let record = state.settlements.get_mut(id).expect("preempted settlement");
    record.project = None;
    record.revision = record.revision.saturating_add(1);
    Ok(())
}

/// Choose the most available wood species that can fund the complete bill.
/// Oak is always considered, including when its materials are already crafted.
fn funded_species<'a>(
    state: &'a TickState,
    materials: &[(u32, u32)],
    images: &[super::inventory::Image],
) -> Option<&'a str> {
    let mut candidates = vec![("oak", 0_i64)];
    for stack in images
        .iter()
        .flat_map(|image| &image.slots)
        .filter(|s| !s.is_empty())
    {
        let Some(name) = state.registry.items.name_of(stack.item_id) else {
            continue;
        };
        if !name.as_str().starts_with("minecraft:") {
            continue;
        }
        let path = name.path();
        let Some((species, weight)) = [
            ("_pressure_plate", 1),
            ("_fence_gate", 1),
            ("_trapdoor", 1),
            ("_planks", 1),
            ("_stairs", 1),
            ("_slab", 1),
            ("_fence", 1),
            ("_door", 1),
            ("_log", 4),
        ]
        .into_iter()
        .find_map(|(suffix, weight)| path.strip_suffix(suffix).map(|s| (s, weight))) else {
            continue;
        };
        if species.starts_with("stripped_")
            || state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse(format!("minecraft:{species}_log")).ok()?)
                .is_none()
        {
            continue;
        }
        let amount = i64::from(stack.count) * weight;
        if let Some((_, total)) = candidates.iter_mut().find(|(known, _)| *known == species) {
            *total += amount;
        } else {
            candidates.push((species, amount));
        }
    }
    candidates.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    candidates.into_iter().find_map(|(species, _)| {
        let bill = species_bill(state, materials, species)?;
        super::construction_crafting::can_fund(state, &bill, images).then_some(species)
    })
}

fn species_item(state: &TickState, item: u32, species: &str) -> Option<u32> {
    let name = state.registry.items.name_of(item)?;
    if species == "oak" || !name.as_str().starts_with("minecraft:") {
        return Some(item);
    }
    let Some(suffix) = name.path().strip_prefix("oak_") else {
        return Some(item);
    };
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{species}_{suffix}")).ok()?)
}

fn species_bill(
    state: &TickState,
    materials: &[(u32, u32)],
    species: &str,
) -> Option<Vec<(u32, u32)>> {
    materials
        .iter()
        .map(|&(item, count)| Some((species_item(state, item, species)?, count)))
        .collect()
}

/// A real worker load can complete the first storage bill before a stack is full.
pub(crate) fn first_storage_funded_by_delivery(
    state: &TickState,
    id: SettlementId,
    carry: &[Option<mc_data::ItemStack>],
) -> bool {
    let Some(record) = state.settlements.get(id) else {
        return false;
    };
    if record.project.is_some()
        || record
            .buildings
            .values()
            .any(|b| b.kind == BuildingKind::Storage)
        || carry.iter().all(Option::is_none)
    {
        return false;
    }
    let Some(level) = blueprints::get(&state.registry, "storage").and_then(|b| b.level(1)) else {
        return false;
    };
    let mut images = executor::source_images(state, id);
    let Some(chest) = images.first_mut() else {
        return false;
    };
    // This is a read-only funding probe; only the regular inventory authority
    // can transfer the actual carry to the real container.
    chest.slots.extend(carry.iter().flatten().cloned());
    funded_species(state, &level.materials, &images).is_some()
}

fn funding_error(state: &TickState, id: SettlementId, materials: &[(u32, u32)]) -> Option<String> {
    let images = executor::source_images(state, id);
    if funded_species(state, materials, &images).is_some() {
        return None;
    }
    for &(item, count) in materials {
        if !super::construction_crafting::can_fund(state, &[(item, count)], &images) {
            return Some(match state.registry.items.name_of(item) {
                Some(name) => format!("not enough: {name} x{count}"),
                None => format!("not enough: item #{item} x{count}"),
            });
        }
    }
    // Requirements can each fit separately while sharing the same finite logs
    // or recipe surplus. Only the whole-project proof authorizes admission.
    Some("not enough: shared recipe ingredients".into())
}

/// Owner/operator entrypoints check `can_manage`; admission starts the sole
/// funded project immediately, without withdrawing or placing any material.
pub(crate) fn request_build(
    state: &mut TickState,
    id: SettlementId,
    blueprint: &str,
) -> Result<(), String> {
    let preempting = claim_slot(state, id)?;
    if preempting {
        // Materials the autonomous project's builder already hauled count as stock.
        executor::return_carry(state, id);
    }
    let registry = std::sync::Arc::clone(&state.registry);
    let blueprint = blueprints::get(&registry, blueprint).ok_or("Unknown blueprint.")?;
    let level = blueprint.level(1).ok_or("Blueprint has no first level.")?;
    if let Some(error) = funding_error(state, id, &level.materials) {
        return Err(error);
    }
    // The untouched autonomous site is released first so the order may take
    // its footprint; an unplaceable order leaves the planner to replan.
    if preempting {
        preempt(state, id)?;
    }
    let record = state.settlements.get(id).expect("claimed settlement");
    let images = executor::source_images(state, id);
    let (site, cursor) = site_project(state, record, blueprint, 0, u32::MAX, &images);
    let mut site =
        site.ok_or("No free loaded construction site is available within the material budget.")?;
    site.project.ordered = true;
    if !begin_cells(state, site.project, site.bounds, site.cells) {
        return Err("Construction admission failed.".into());
    }
    state
        .settlements
        .get_mut(id)
        .expect("admitted settlement")
        .construction_cursor = cursor;
    Ok(())
}

/// A rejected upgrade is never stored and cannot obstruct autonomous planning.
pub(crate) fn request_upgrade(
    state: &mut TickState,
    id: SettlementId,
    building: BuildingId,
) -> Result<(), String> {
    let preempting = claim_slot(state, id)?;
    if preempting {
        executor::return_carry(state, id);
    }
    let record = state.settlements.get(id).expect("claimed settlement");
    let building = record
        .buildings
        .get(&building)
        .ok_or("Building not found.")?;
    let blueprint = upgrade_blueprint(state, id, building)
        .ok_or("No blueprint upgrade available for this building.")?;
    let level = building.level + 1;
    if let Some(error) = funding_error(
        state,
        id,
        &blueprint.level(level).expect("upgrade level").materials,
    ) {
        return Err(error);
    }
    let images = executor::source_images(state, id);
    let species = funded_species(
        state,
        &blueprint.level(level).expect("upgrade level").materials,
        &images,
    )
    .ok_or("Construction materials changed during admission.")?
    .to_owned();
    let project = Project {
        settlement: id,
        blueprint: blueprint.id.clone(),
        level,
        origin: building.bounds.min,
        rotation: 0,
        building: Some(building.id),
        structure: String::new(),
        ordered: true,
    };
    if preempting {
        preempt(state, id)?;
    }
    if !begin(state, project, &species) {
        return Err("Construction admission failed.".into());
    }
    Ok(())
}

/// No escrow is discarded: real carried items stay with the same native body.
pub(crate) fn detach(v: &mut crate::villager::VillagerState) -> bool {
    if v.data.profession != Profession::Builder {
        return false;
    }
    v.data.profession = if super::allocator::traded(v) {
        Profession::Mason
    } else {
        Profession::None
    };
    if v.data.profession == Profession::None {
        v.offers.clear();
    }
    v.job_site = None;
    v.navigation_target = None;
    v.navigation_path.clear();
    v.navigation_chunks.clear();
    v.navigation_repath_at = 0;
    true
}

/// Release canonical site reservations, without deleting placed blocks or stock.
pub(crate) fn remove(state: &mut TickState, id: SettlementId) -> bool {
    if state.settlements.get(id).is_none() {
        return true;
    }
    executor::cancel(state, id).is_ok()
}

/// Needed authored kinds in priority order. A natural work zone is not an
/// authored level-one building, and housing remains needed until beds suffice.
pub(super) fn needed_kinds(
    record: &SettlementRecord,
    residents: usize,
) -> impl Iterator<Item = BuildingKind> + '_ {
    let has = |kind| {
        record
            .buildings
            .values()
            .any(|building| building.kind == kind && !building.natural_zone)
    };
    let beds: usize = record
        .buildings
        .values()
        .filter(|building| building.kind == BuildingKind::House)
        .map(|building| usize::from(building.level))
        .sum();
    [
        (BuildingKind::Storage, !has(BuildingKind::Storage)),
        (BuildingKind::House, beds < residents),
        (
            BuildingKind::Workshop(Profession::Farmer),
            !has(BuildingKind::Workshop(Profession::Farmer)),
        ),
        (BuildingKind::LumberCamp, !has(BuildingKind::LumberCamp)),
        (BuildingKind::Mine, !has(BuildingKind::Mine)),
        (
            BuildingKind::Workshop(Profession::Builder),
            !has(BuildingKind::Workshop(Profession::Builder)),
        ),
        (
            BuildingKind::Barracks,
            residents >= 6 && !has(BuildingKind::Barracks),
        ),
        (
            BuildingKind::Tower,
            residents >= 8 && !has(BuildingKind::Tower),
        ),
        (
            BuildingKind::Wall,
            residents >= 10 && !has(BuildingKind::Wall),
        ),
    ]
    .into_iter()
    .filter_map(|(kind, needed)| needed.then_some(kind))
}

pub(crate) fn sources(state: &TickState, id: SettlementId) -> Vec<[i32; 3]> {
    buildings::storage_points(state, id)
}

fn clearable(state: &TickState, block: u32) -> bool {
    // Clearing is a native-flora veneer only: air plus the vanilla placement-
    // replaceable surface plants below. Fluids, paths, buildings, torches,
    // flowers and paid farm crops stay blocking so a wider clear can never
    // wash or overwrite foreign/unsafe content, and an upgrade diff that
    // omits unchanged L1 fields never loses paid seeds.
    if block == state.registry.states.air {
        return true;
    }
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(block))
        .is_some_and(|definition| {
            !definition.has_fluid()
                && matches!(
                    definition.block.id.path(),
                    "short_grass"
                        | "tall_grass"
                        | "short_dry_grass"
                        | "tall_dry_grass"
                        | "fern"
                        | "large_fern"
                        | "dead_bush"
                        | "bush"
                        | "vine"
                        | "glow_lichen"
                        | "resin_clump"
                        | "leaf_litter"
                        | "hanging_roots"
                        | "warped_roots"
                        | "nether_sprouts"
                        | "crimson_roots"
                        | "fire"
                        | "soul_fire"
                        | "snow"
                        | "void_air"
                        | "cave_air"
                )
        })
}

/// Only natural terrain can be excavated; buildings, paths and fluids are not
/// site preparation materials.
fn excavatable(state: &TickState, block: u32) -> bool {
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(block))
        .is_some_and(|definition| {
            !definition.has_fluid()
                && matches!(
                    definition.block.id.path(),
                    "dirt"
                        | "grass_block"
                        | "coarse_dirt"
                        | "rooted_dirt"
                        | "podzol"
                        | "mycelium"
                        | "stone"
                        | "granite"
                        | "diorite"
                        | "andesite"
                        | "deepslate"
                        | "sand"
                        | "red_sand"
                        | "gravel"
                        | "clay"
                )
        })
}

const SITE_VARIANCE: i32 = 3;

/// Floor replacement permits solid ground or clearable fill space, but never
/// fluids, paths, paid crops or authored non-ground blocks under an overhang.
fn floor_replaceable(state: &TickState, block: u32) -> bool {
    clearable(state, block)
        || (state
            .registry
            .collision_boxes
            .get(block as usize)
            .is_some_and(|boxes| !boxes.is_empty())
            && state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(block))
                .is_some_and(|definition| {
                    !definition.has_fluid()
                        && !matches!(definition.block.id.path(), "dirt_path" | "farmland")
                }))
}

/// Check the whole future footprint, not merely this level's placed cells.
/// One candidate per planning admission; never load chunks or clear paths.
pub(super) fn site_free(
    state: &TickState,
    record: &SettlementRecord,
    origin: [i32; 3],
    size: [u8; 3],
) -> bool {
    let max: [i32; 3] = std::array::from_fn(|axis| origin[axis] + i32::from(size[axis]) - 1);
    let mut minimum = origin[1];
    let mut maximum = origin[1];
    if record.buildings.values().any(|building| {
        [0, 2].into_iter().all(|axis| {
            origin[axis] <= building.bounds.max[axis] + 1
                && building.bounds.min[axis] <= max[axis] + 1
        })
    }) {
        return false;
    }
    for x in origin[0]..=max[0] {
        for z in origin[2]..=max[2] {
            if [x, max[1], z]
                .into_iter()
                .zip(record.anchor)
                .map(|(value, anchor)| (i64::from(value) - i64::from(anchor)).pow(2))
                .sum::<i64>()
                > i64::from(record.radius).pow(2)
            {
                return false;
            }
            let Some(height) = surface_y(state, record.anchor[1], x, z) else {
                return false;
            };
            minimum = minimum.min(height);
            maximum = maximum.max(height);
            if maximum - minimum > SITE_VARIANCE {
                return false;
            }
            let ground = state
                .world
                .physics_block_at(x, height, z)
                .expect("loaded surface");
            if !floor_replaceable(state, ground) {
                return false;
            }
            for y in height.min(origin[1])..=max[1].max(height) {
                let Some(block) = state.world.physics_block_at(x, y, z) else {
                    return false;
                };
                if state.block_entities.contains_key(&(x, y, z))
                    || (y == origin[1] && !floor_replaceable(state, block))
                    || (y > origin[1] && !clearable(state, block) && !excavatable(state, block))
                    || (y > height && y <= origin[1] && !clearable(state, block))
                {
                    return false;
                }
            }
        }
    }
    !executor::area_reserved(
        state,
        ScriptWorldBounds::new([origin[0], minimum, origin[2]], max).expect("blueprint bounds"),
    )
}

fn planned_cells(
    state: &TickState,
    project: &Project,
    species: &str,
) -> Option<(ScriptWorldBounds, Vec<executor::PlannedCell>)> {
    let blueprint = blueprints::get(&state.registry, &project.blueprint)?;
    let level = blueprint.level(project.level)?;
    let rotated = level.rotated(Rotation::from_quarter_turns(project.rotation));
    let mut quota = level.materials.clone();
    let mut cells: Vec<_> = rotated
        .removals
        .iter()
        .map(|offset| executor::PlannedCell {
            at: rotated.world(project.origin, *offset),
            state: state.registry.states.air,
            resource: None,
            quantity: 0,
        })
        .collect();
    let size = blueprint.footprint(Rotation::from_quarter_turns(project.rotation));
    let mut bottom = project.origin[1];
    if project.building.is_none() {
        let dirt = state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse("minecraft:dirt").ok()?)?
            .default
            .0;
        for x in project.origin[0]..project.origin[0] + i32::from(size[0]) {
            for z in project.origin[2]..project.origin[2] + i32::from(size[2]) {
                let ground = surface_y(state, project.origin[1], x, z)?;
                for y in ground + 1..=project.origin[1] {
                    // The authored floor already pays for its own cells.
                    if y == project.origin[1]
                        && rotated
                            .blocks
                            .iter()
                            .any(|(offset, _)| rotated.world(project.origin, *offset) == [x, y, z])
                    {
                        continue;
                    }
                    bottom = bottom.min(y);
                    cells.push(executor::PlannedCell {
                        at: [x, y, z],
                        state: dirt,
                        resource: Some("minecraft:dirt".into()),
                        quantity: 1,
                    });
                }
            }
        }
    }
    for x in project.origin[0]..project.origin[0] + i32::from(size[0]) {
        for z in project.origin[2]..project.origin[2] + i32::from(size[2]) {
            for y in project.origin[1] + 1..project.origin[1] + i32::from(size[1]) {
                let block = state.world.physics_block_at(x, y, z)?;
                if block != state.registry.states.air
                    && (clearable(state, block)
                        || (project.building.is_none() && excavatable(state, block)))
                    && !cells.iter().any(|cell| cell.at == [x, y, z])
                {
                    cells.push(executor::PlannedCell {
                        at: [x, y, z],
                        state: state.registry.states.air,
                        resource: None,
                        quantity: 0,
                    });
                }
            }
        }
    }
    for (offset, block) in &rotated.blocks {
        let placed = *block;
        let (resource, quantity) =
            if let Some((item, quantity)) = blueprints::material_for(&state.registry, *block) {
                let (_, remaining) = quota.iter_mut().find(|(id, _)| *id == item)?;
                *remaining = remaining.checked_sub(quantity)?;
                (
                    Some(state.registry.items.name_of(item)?.to_string()),
                    quantity,
                )
            } else {
                (None, 0)
            };
        cells.push(executor::PlannedCell {
            at: rotated.world(project.origin, *offset),
            state: placed.0,
            resource,
            quantity,
        });
    }
    // Catalog materials must map exactly to physical cells; never build free
    // blocks because a blueprint cannot be funded by its declared stock.
    if quota.iter().any(|(_, count)| *count != 0) {
        return None;
    }
    apply_species(state, &mut cells, species)?;
    // Include the whole placement extent (roof crowns/yards) in executor
    // interference fencing; certification uses the narrower authored bounds.
    let bounds = ScriptWorldBounds::new(
        [project.origin[0], bottom, project.origin[2]],
        std::array::from_fn(|axis| project.origin[axis] + i32::from(size[axis]) - 1),
    )
    .ok()?;
    Some((bounds, cells))
}

/// Rewrite the already-funded authored cells, not the stored project later.
fn apply_species(
    state: &TickState,
    cells: &mut [executor::PlannedCell],
    species: &str,
) -> Option<()> {
    if species == "oak" {
        return Some(());
    }
    for cell in cells {
        let original = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(cell.state))?;
        if original.block.id.as_str().starts_with("minecraft:")
            && let Some(suffix) = original.block.id.path().strip_prefix("oak_")
        {
            let name = mc_data::Identifier::parse(format!("minecraft:{species}_{suffix}")).ok()?;
            cell.state = state
                .registry
                .blocks
                .by_name_and_props(&name, &original.properties)?
                .0;
        }
        if let Some(resource) = &mut cell.resource {
            let item = state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse(resource.as_str()).ok()?)?;
            let mapped = species_item(state, item, species)?;
            if mapped != item {
                *resource = state.registry.items.name_of(mapped)?.to_string();
            }
        }
    }
    Some(())
}

fn cell_bill(state: &TickState, cells: &[executor::PlannedCell]) -> Option<Vec<(u32, u32)>> {
    let mut bill = std::collections::BTreeMap::new();
    for cell in cells {
        if let Some(resource) = &cell.resource {
            let item = state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse(resource).ok()?)?;
            *bill.entry(item).or_insert(0) += cell.quantity;
        }
    }
    Some(bill.into_iter().collect())
}

fn begin(state: &mut TickState, project: Project, species: &str) -> bool {
    let Some((bounds, cells)) = planned_cells(state, &project, species) else {
        return false;
    };
    begin_cells(state, project, bounds, cells)
}

fn begin_cells(
    state: &mut TickState,
    mut project: Project,
    bounds: ScriptWorldBounds,
    cells: Vec<executor::PlannedCell>,
) -> bool {
    let Ok(handle) = executor::prepare(
        state,
        project.settlement,
        project.blueprint.clone(),
        bounds,
        cells,
    ) else {
        return false;
    };
    project.structure = handle;
    let record = state
        .settlements
        .get_mut(project.settlement)
        .expect("planned settlement");
    record.project = Some(project);
    record.revision = record.revision.saturating_add(1);
    true
}

fn upgrade_blueprint<'a>(
    state: &'a TickState,
    id: SettlementId,
    building: &buildings::Building,
) -> Option<&'a blueprints::Blueprint> {
    if building.natural_zone {
        return None;
    }
    let blueprint = blueprints::all(&state.registry)
        .iter()
        .find(|blueprint| blueprint.kind == building.kind)?;
    blueprint.level(building.level.checked_add(1)?)?;
    let previous = blueprint
        .level(building.level)?
        .rotated(Rotation::from_quarter_turns(0));
    // Natural zones and player-authored certificates are not blueprint L1.
    (previous.bounds(building.bounds.min) == building.bounds
        && executor::built_at(state, id, &blueprint.id, building.bounds.min))
    .then_some(blueprint)
}

pub(crate) fn upgrade_available(state: &TickState, id: SettlementId, building: BuildingId) -> bool {
    state
        .settlements
        .get(id)
        .and_then(|record| record.buildings.get(&building))
        .is_some_and(|building| upgrade_blueprint(state, id, building).is_some())
}

fn upgrade_project(state: &TickState, record: &SettlementRecord) -> Option<(Project, String)> {
    record.buildings.values().find_map(|building| {
        let blueprint = upgrade_blueprint(state, record.id, building)?;
        let level = building.level + 1;
        let images = executor::source_images(state, record.id);
        let species =
            funded_species(state, &blueprint.level(level)?.materials, &images)?.to_owned();
        Some((
            Project {
                settlement: record.id,
                blueprint: blueprint.id.clone(),
                level,
                origin: building.bounds.min,
                rotation: 0,
                building: Some(building.id),
                structure: String::new(),
                ordered: false,
            },
            species,
        ))
    })
}

/// Candidate columns examined per autonomous planning pass; an owner order
/// searches the whole settlement sphere.
const SITE_ATTEMPTS: u32 = 64;
/// Ground may differ this much from the bell's elevation and still be a site.
const SITE_BAND: i32 = 8;

/// Highest standing surface in the band around the bell for one column: a solid
/// block with clearable space directly above it. Hillside villages have no
/// flat ground at the bell's own elevation, so sites follow the terrain.
fn surface_y(state: &TickState, anchor_y: i32, x: i32, z: i32) -> Option<i32> {
    (anchor_y - SITE_BAND..=anchor_y + SITE_BAND)
        .rev()
        .find(|&y| {
            state.world.physics_block_at(x, y, z).is_some_and(|ground| {
                state
                    .registry
                    .collision_boxes
                    .get(ground as usize)
                    .is_some_and(|boxes| !boxes.is_empty())
            }) && state
                .world
                .physics_block_at(x, y + 1, z)
                .is_some_and(|above| clearable(state, above))
        })
}

/// Column offset of the `n`-th cell in ring order: ring `d` holds the `8d`
/// cells at Chebyshev distance `d` from the anchor, so a scan in index order
/// visits columns nearest the anchor first.
fn ring_offset(n: u32) -> (i32, i32) {
    if n == 0 {
        return (0, 0);
    }
    let d = (n.isqrt() as i32 + 1) / 2;
    let k = (n - (2 * d - 1).pow(2) as u32) as i32;
    let (side, t) = (k / (2 * d), k % (2 * d));
    match side {
        0 => (d, -d + t),
        1 => (d - t, d),
        2 => (-d, d - t),
        _ => (-d + t, -d),
    }
}

struct FundedSite {
    project: Project,
    bounds: ScriptWorldBounds,
    cells: Vec<executor::PlannedCell>,
}

/// The same bounded site search serves autonomous planning and immediate
/// owner admission: the nearest free, fully funded site to the settlement anchor, searched
/// outward in rings from ring index `start`. `attempts` bounds the columns
/// examined; the owner order starts at the anchor with `u32::MAX` and so finds
/// a funded loaded site on the first try when any exists. A rejected owner order does
/// not save a search cursor; success returns cursor 0 so the next search
/// starts at the anchor again.
fn site_project(
    state: &TickState,
    record: &SettlementRecord,
    blueprint: &blueprints::Blueprint,
    start: u32,
    attempts: u32,
    images: &[super::inventory::Image],
) -> (Option<FundedSite>, u32) {
    let size = blueprint.footprint(Rotation::from_quarter_turns(0));
    let radius = record.radius.min(256) as i32;
    let cells = ((radius * 2 + 1) as u32).pow(2);
    let mut index = start % cells;
    let (mut examined, mut visited) = (0, 0);
    while examined < attempts && visited < cells {
        let (dx, dz) = ring_offset(index);
        index = (index + 1) % cells;
        visited += 1;
        if dx * dx + dz * dz > radius * radius {
            continue;
        }
        examined += 1;
        // Centre the footprint on the ring cell so the building, not its
        // corner, is as near the anchor as the cell is.
        let (x, z) = (
            record.anchor[0] + dx - i32::from(size[0] / 2),
            record.anchor[2] + dz - i32::from(size[2] / 2),
        );
        let Some(ground) = surface_y(state, record.anchor[1], x, z) else {
            continue;
        };
        let origin = [x, ground, z];
        if site_free(state, record, origin, size) {
            let project = Project {
                settlement: record.id,
                blueprint: blueprint.id.clone(),
                level: 1,
                origin,
                rotation: 0,
                building: None,
                structure: String::new(),
                ordered: false,
            };
            if let Some((bounds, mut cells)) = planned_cells(state, &project, "oak")
                && let Some(bill) = cell_bill(state, &cells)
                && let Some(species) = funded_species(state, &bill, images)
                && apply_species(state, &mut cells, species).is_some()
            {
                return (
                    Some(FundedSite {
                        project,
                        bounds,
                        cells,
                    }),
                    0,
                );
            }
        }
    }
    (None, index)
}

fn plan(state: &mut TickState, id: SettlementId) {
    let Some(record) = state.settlements.get(id) else {
        return;
    };
    if record.project.is_some() {
        return;
    }
    let residents = state.settlements.member_count(id);
    if residents == 0 {
        return;
    }
    // Probe one canonical storage image set; an unfundable early need cannot
    // monopolize the single project slot, including after a refused order.
    let images = executor::source_images(state, id);
    let catalog = blueprints::all(&state.registry);
    let blueprint = needed_kinds(record, residents).find_map(|kind| {
        let blueprint = catalog.iter().find(|blueprint| blueprint.kind == kind)?;
        let level = blueprint.level(1)?;
        funded_species(state, &level.materials, &images).map(|_| blueprint)
    });
    let Some(blueprint) = blueprint else {
        if let Some((project, species)) = upgrade_project(state, record) {
            begin(state, project, &species);
        }
        return;
    };
    let (site, cursor) = site_project(
        state,
        record,
        blueprint,
        record.construction_cursor,
        SITE_ATTEMPTS,
        &images,
    );
    let record = state.settlements.get_mut(id).expect("planned settlement");
    record.construction_cursor = cursor;
    record.revision = record.revision.saturating_add(1);
    if let Some(site) = site {
        begin_cells(state, site.project, site.bounds, site.cells);
    }
}

/// Chunk-local resident index, with a hard admission cap. Other mob populations
/// never contribute to planner or builder search cost.
pub(crate) fn residents(
    state: &TickState,
    id: SettlementId,
) -> impl Iterator<Item = crate::entities::EntitySlot> + '_ {
    let (anchor, radius) = state.settlements.get(id).map_or(([0; 3], 0), |record| {
        (record.anchor, record.radius.min(256) as i32)
    });
    let min_x = (anchor[0] - radius).div_euclid(16);
    let max_x = (anchor[0] + radius).div_euclid(16);
    let min_z = (anchor[2] - radius).div_euclid(16);
    let max_z = (anchor[2] + radius).div_euclid(16);
    (min_x..=max_x)
        .flat_map(move |x| {
            (min_z..=max_z).flat_map(move |z| {
                state
                    .entities
                    .villager_chunks()
                    .get(&crate::boundary::ChunkPos { x, z })
                    .into_iter()
                    .flatten()
                    .filter_map(|id| state.entities.slot_of(*id))
            })
        })
        .take(1024)
}

/// Builders fetch every block's material from storage. A resident that cannot
/// walk there (shut in by terrain, e.g. a house door blocked by a tree crown)
/// would hold the single builder slot without ever building.
fn reaches_storage(
    state: &TickState,
    id: SettlementId,
    slot: crate::entities::EntitySlot,
    cache: &mut super::allocator::PoiCache,
    path: &mut Vec<crate::boundary::BlockPos>,
) -> bool {
    let Some(chest) = buildings::storage_points(state, id).first().copied() else {
        return true;
    };
    let target = crate::boundary::BlockPos {
        x: chest[0],
        y: chest[1],
        z: chest[2],
    };
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height) = (body.half_width, body.height);
    let from = super::allocator::at(pos);
    let horizontal = from
        .x
        .abs_diff(target.x)
        .max(from.z.abs_diff(target.z))
        .saturating_add(8)
        .clamp(48, 512);
    let vertical = from.y.abs_diff(target.y).saturating_add(4).clamp(16, 512);
    // A one-shot admission probe, never a per-tick allocation: villagers and the
    // allocator routinely spend the whole shared route allowance before the
    // planning pass runs, which refused every candidate and left the project
    // without a builder. A fresh allowance bounds each probe at its own search.
    let mut budget = crate::tick_budget::Budget::default();
    let mut ctx = super::allocator::RouteCtx {
        world: &state.world,
        registry: &state.registry,
        dimension: state.dimension,
        consumer: crate::tick_budget::Consumer::Allocator,
        budget: &mut budget,
    };
    ctx.admit()
        && super::allocator::find_path_ctx(
            &mut ctx, cache, pos, target, width, height, path, false, horizontal, vertical, true,
        )
}

/// Residents tried per pass; the rest wait for the next planning pass.
const BUILDER_CANDIDATES: usize = 8;

/// A builder that is on shift but cannot walk to storage is shut in (a house
/// door under a tree crown) and holds the single builder slot without ever
/// building. It is released so a resident who can reach storage takes over.
fn release_trapped_builder(state: &mut TickState, id: SettlementId) -> bool {
    let incumbent = residents(state, id).find(|&slot| {
        state.entities.body(slot).alive
            && state.entities.row(slot).villager.as_ref().is_some_and(|v| {
                v.settlement == Some(id)
                    && v.age >= 0
                    && v.conversion_time.is_none()
                    && v.data.profession == Profession::Builder
            })
    });
    let Some(slot) = incumbent else {
        return false;
    };
    let on_shift = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .is_some_and(|v| crate::villager::building::working(state, v));
    let mut cache = super::allocator::PoiCache::default();
    let mut path = Vec::new();
    if !on_shift || reaches_storage(state, id, slot, &mut cache, &mut path) {
        return true;
    }
    let released = detach(
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .expect("builder incumbent"),
    );
    crate::villager::sync_appearance(state, slot);
    !released
}

fn assign_builder(state: &mut TickState, id: SettlementId) {
    if release_trapped_builder(state, id) {
        return;
    }
    let mut candidates = Vec::new();
    for slot in residents(state, id) {
        let body = state.entities.body(slot);
        let Some(v) = state.entities.row(slot).villager.as_ref() else {
            continue;
        };
        if body.alive && v.settlement == Some(id) && v.age >= 0 && v.conversion_time.is_none() {
            if v.data.profession == Profession::Builder {
                return;
            }
            let priority = match v.data.profession {
                Profession::None => Some(0),
                Profession::Mason if !super::allocator::traded(v) => Some(1),
                profession
                    if !super::allocator::traded(v)
                        && !matches!(
                            profession,
                            Profession::Miner
                                | Profession::Lumberjack
                                | Profession::Farmer
                                | Profession::Shepherd
                                | Profession::Soldier
                                | Profession::Nitwit
                        ) =>
                {
                    Some(2)
                }
                _ => None,
            };
            if let Some(priority) = priority {
                candidates.push((priority, slot));
            }
        }
    }
    // Stable: residents of equal priority keep their index order.
    candidates.sort_by_key(|&(priority, _)| priority);
    let mut cache = super::allocator::PoiCache::default();
    let mut path = Vec::new();
    let Some(slot) = candidates
        .into_iter()
        .take(BUILDER_CANDIDATES)
        .map(|(_, slot)| slot)
        .find(|&slot| reaches_storage(state, id, slot, &mut cache, &mut path))
    else {
        return;
    };
    let v = state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .expect("builder candidate");
    v.data.profession = Profession::Builder;
    v.offers.clear();
    v.job_site = None;
    v.navigation_target = None;
    v.navigation_path.clear();
    v.navigation_chunks.clear();
    v.navigation_repath_at = 0;
    crate::villager::sync_appearance(state, slot);
}

/// Planning keeps its one-settlement/second budget. Construction has a separate
/// four-tick admission clock, retaining the bounded round-robin settlement budget.
pub(crate) fn tick(state: &mut TickState) {
    if !state.tick.is_multiple_of(4) {
        return;
    }
    let count = state.settlements.records().count();
    if count == 0 {
        return;
    }
    let selected = |state: &TickState, turn: u64| {
        state
            .settlements
            .records()
            .nth(turn as usize % count)
            .filter(|record| {
                state.world.is_active(
                    record.anchor[0].div_euclid(16),
                    record.anchor[2].div_euclid(16),
                )
            })
            .map(|record| record.id)
    };
    if state.tick.is_multiple_of(20)
        && let Some(id) = selected(state, state.tick / 20)
    {
        plan(state, id);
        if state
            .settlements
            .get(id)
            .is_some_and(|record| record.project.is_some())
        {
            assign_builder(state, id);
        }
    }
    if let Some(id) = selected(state, state.tick / 4)
        && state
            .settlements
            .get(id)
            .is_some_and(|record| record.project.is_some())
    {
        crate::villager::building::tick(state, id);
    }
}

pub(crate) fn finish_project(state: &mut TickState, id: SettlementId) -> bool {
    let Some(project) = state
        .settlements
        .get(id)
        .and_then(|record| record.project.clone())
    else {
        return false;
    };
    if !executor::status(state, id, &project.structure)
        .is_ok_and(|snapshot| snapshot.state == ScriptStructureState::Committed)
    {
        return false;
    }
    let Some(blueprint) = blueprints::get(&state.registry, &project.blueprint) else {
        return false;
    };
    let Some(level) = blueprint.level(project.level) else {
        return false;
    };
    let bounds = level
        .rotated(Rotation::from_quarter_turns(project.rotation))
        .bounds(project.origin);
    let Ok(building) = buildings::certify(state, id, blueprint.kind, bounds) else {
        return false;
    };
    if project
        .building
        .is_some_and(|expected| expected != building)
    {
        return false;
    }
    if project.level > 1 && buildings::upgrade(state, building).is_err() {
        return false;
    }
    let record = state.settlements.get_mut(id).expect("certified settlement");
    record.project = None;
    record.revision = record.revision.saturating_add(1);
    true
}

/// One needed blueprint kind whose level-one bill the settlement cannot fund.
pub(crate) struct WaitingKind {
    pub(crate) blueprint: String,
    /// Unfunded bill entries, in bill order.
    pub(crate) needs: Vec<WaitingNeed>,
}

/// One unfunded bill entry of [`WaitingKind`].
pub(crate) enum WaitingNeed {
    /// A supported source can produce it, but it is not in settlement storage.
    Awaiting { item: String, count: u64 },
    /// Nothing can source this bill entry; `item` is the first missing link.
    NoSource { item: String },
}

/// Needed authored kinds the settlement cannot fund yet, one entry per kind.
/// Formatting (translation, per-line cap) belongs to the dialog.
pub(crate) fn waiting(state: &TickState, id: SettlementId) -> Vec<WaitingKind> {
    let Some(record) = state.settlements.get(id) else {
        return Vec::new();
    };
    let images = executor::source_images(state, id);
    let catalog = blueprints::all(&state.registry);
    let active = record
        .project
        .as_ref()
        .map(|project| project.blueprint.as_str());
    let mut kinds = Vec::new();
    for kind in needed_kinds(record, state.settlements.member_count(id)) {
        let Some(blueprint) = catalog.iter().find(|blueprint| blueprint.kind == kind) else {
            continue;
        };
        if active == Some(blueprint.id.as_str()) {
            continue;
        }
        let Some(level) = blueprint.level(1) else {
            continue;
        };
        if funded_species(state, &level.materials, &images).is_some() {
            continue;
        }
        let mut needs = Vec::new();
        for (item, count) in
            super::construction_crafting::missing_base(state, &level.materials, &images)
        {
            if let Some(leaf) = availability::missing_source(state, id, item, &images) {
                if let Some(name) = state.registry.items.name_of(leaf) {
                    needs.push(WaitingNeed::NoSource {
                        item: name.to_string(),
                    });
                }
            } else if let Some(name) = state.registry.items.name_of(item) {
                needs.push(WaitingNeed::Awaiting {
                    item: name.to_string(),
                    count: u64::from(count),
                });
            }
        }
        if !needs.is_empty() {
            kinds.push(WaitingKind {
                blueprint: blueprint.id.clone(),
                needs,
            });
        }
    }
    kinds
}

/// Plain-text waiting lines for [`project_status`] diagnostics only.
#[cfg(test)]
fn waiting_materials(state: &TickState, id: SettlementId, lines: &mut Vec<String>) {
    for kind in waiting(state, id) {
        for need in kind.needs {
            match need {
                WaitingNeed::NoSource { item } => lines.push(format!(
                    "Waiting {}: needs {item} — no source",
                    kind.blueprint
                )),
                WaitingNeed::Awaiting { item, count } => lines.push(format!(
                    "Waiting {}: needs {item} x{count} — awaiting production",
                    kind.blueprint
                )),
            }
        }
    }
}

/// Plain-text project status (summary plus one English waiting line per unmet
/// item) for failure diagnostics; the dialog formats [`waiting`] itself.
#[cfg(test)]
pub(crate) fn project_status(state: &TickState, id: SettlementId) -> Vec<String> {
    let mut lines = project_summary(state, id);
    waiting_materials(state, id, &mut lines);
    lines
}

/// Project progress and material state without the waiting block: what the
/// Projects dialog shows before its capped, translated waiting lines.
pub(crate) fn project_summary(state: &TickState, id: SettlementId) -> Vec<String> {
    let Some(record) = state.settlements.get(id) else {
        return vec!["Settlement not found.".into()];
    };
    let Some(project) = record.project.as_ref() else {
        return vec!["No active project. Waiting for residents, materials or a free site.".into()];
    };
    let Ok(snapshot) = executor::status(state, id, &project.structure) else {
        return vec!["Project ledger unavailable; paused.".into()];
    };
    let total: u64 = snapshot.stages.iter().map(|stage| stage.work_units).sum();
    let done: u64 = snapshot
        .stages
        .iter()
        .take(snapshot.current_stage_index as usize)
        .map(|stage| stage.work_units)
        .sum::<u64>()
        + snapshot.completed_work_units;
    let mut lines = vec![
        format!(
            "{} · level {} · {}%",
            project.blueprint,
            project.level,
            done * 100 / total.max(1)
        ),
        format!("Blocks: {done}/{total} · {:?}", snapshot.state),
        format!(
            "Site: {}, {}, {}",
            project.origin[0], project.origin[1], project.origin[2]
        ),
    ];
    lines.push("Remaining materials (including site preparation):".into());
    lines.extend(
        snapshot
            .remaining
            .iter()
            .map(|material| format!("{}: {}", material.resource, material.quantity)),
    );
    match executor::missing(state, id, &project.structure) {
        Ok(missing) if missing.is_empty() => {
            lines.push("Materials ready; builder collecting and placing.".into())
        }
        Ok(missing) => {
            lines.push("Missing materials:".into());
            lines.extend(
                missing
                    .into_iter()
                    .map(|(item, count)| format!("{item}: {count}")),
            );
        }
        Err(_) => lines.push("Storage unavailable.".into()),
    }
    lines
}

pub(super) fn validate_record(record: &SettlementRecord) -> Result<(), &'static str> {
    if let Some(project) = &record.project
        && (project.settlement != record.id
            || project.blueprint.is_empty()
            || project.blueprint.len() > 128
            || project.structure.is_empty()
            || project.structure.len() > 128
            || !(1..=3).contains(&project.level)
            || project.rotation > 3
            || project
                .building
                .is_some_and(|id| !record.buildings.contains_key(&id))
            || (project.level > 1) != project.building.is_some())
    {
        return Err("invalid settlement construction project");
    }
    Ok(())
}

#[cfg(test)]
#[path = "construction_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "construction_fencing_tests.rs"]
mod fencing_tests;

#[cfg(test)]
#[path = "construction_durability_tests.rs"]
mod durability_tests;

#[cfg(test)]
#[path = "construction_planner_tests.rs"]
mod planner_tests;

#[cfg(test)]
#[path = "construction_site_tests.rs"]
mod site_tests;
