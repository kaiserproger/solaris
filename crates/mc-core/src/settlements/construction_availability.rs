//! Construction material source classifier.
//!
//! Answers whether a raw missing material is producible by the settlement's
//! *current* workers and buildings, so the planner can skip unfundable needs
//! (e.g. a house needing wool with no shepherd/sheep) instead of stalling on
//! them. This is a capability classifier, not a funding guarantee: exact stock
//! proof stays upstream with `executor::missing_stock` / `can_fund`.
//!
//! [`missing_source`] expands one requested item through bounded vanilla grid
//! recipes (shaped/shapeless only, no smelting) down to raw leaves and reports
//! the first unproducible raw ingredient, or `None` when the item is already
//! stocked or every leaf has a live source. A stocked leaf (existing
//! [`Image`] stock) always satisfies that leaf, even without a live worker.
//!
//! Raw sources reuse the actual worker output rules:
//!
//! * mine drops (the miner's natural-block drop images) need a bound adult
//!   Miner plus a certified Mine;
//! * `<stem>_log` needs a bound adult Lumberjack plus a certified LumberCamp;
//! * crops/seeds need a bound adult Farmer plus a certified Farmer workshop;
//! * `<color>_wool` needs a bound adult Shepherd plus a loaded adult living
//!   sheep of that color inside the settlement radius. A sheared sheep still
//!   counts: native grazing regrows the wool, so the sheared bit is ignored.
//!
//! Scans are bounded: one resident pass via the chunk-local
//! [`residents`](super::residents) index and one settlement-bounded chunk-index
//! walk for sheep. No full-world scans, no terrain scans, no new
//! material-generation semantics. No heap allocation on the query path: the
//! recipe cycle path is a fixed stack array.

use super::residents;
use crate::blueprint::DYE_COLORS;
use crate::boundary::ChunkPos;
use crate::entities::EntityKind;
use crate::settlements::buildings::BuildingKind;
use crate::settlements::inventory::Image;
use crate::settlements::{SettlementId, SettlementRecord};
use crate::tick::TickState;
use crate::villager::VillagerProfession;
use mc_data::recipes::{Ingredient, Recipe, RecipeKind};

/// Deepest allowed recipe chain, mirroring the crafting planner.
const MAX_DEPTH: usize = 8;
/// Fixed cycle-detection path: one slot per chain level, no heap allocation.
const VISIT_LEN: usize = MAX_DEPTH + 1;
/// Total recipe/ingredient operations across one [`missing_source`] call.
const MAX_WORK: usize = 512;
/// Sheep chunk-index entries examined per call; the shepherd uses 256.
const MAX_SHEEP_EXAMINED: usize = 1024;

/// Precomputed per-call source facts: bound adult workers, certified
/// buildings and loaded in-radius sheep colors.
struct Caps {
    miner: bool,
    lumberjack: bool,
    farmer: bool,
    shepherd: bool,
    mine: bool,
    camp: bool,
    farm: bool,
    /// Bit `i` set when an adult living sheep of [`DYE_COLORS`] color `i`
    /// stands inside the settlement radius.
    sheep_colors: u16,
}

/// Test-only probe: `true` when `item` is stocked or every raw leaf behind it
/// has a live source. Production uses [`missing_source`] directly.
#[cfg(test)]
pub(super) fn can_produce(
    state: &TickState,
    id: SettlementId,
    item: u32,
    images: &[Image],
) -> bool {
    missing_source(state, id, item, images).is_none()
}
/// The unproducible raw ingredient behind `item`, or `None` when the item is
/// stocked or fully source-backed. Unknown items report `Some(item)`; a
/// missing settlement reports `Some(item)` unless the item itself is stocked.
pub(super) fn missing_source(
    state: &TickState,
    id: SettlementId,
    item: u32,
    images: &[Image],
) -> Option<u32> {
    if stocked(images, item) {
        return None;
    }
    let Some(record) = state.settlements.get(id) else {
        return Some(item);
    };
    let caps = caps(state, id, record);
    let mut work = 0usize;
    let mut visiting = [0u32; VISIT_LEN];
    resolve(state, &caps, item, images, 0, &mut work, &mut visiting)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceReason {
    Mine,
    Miner,
    LumberCamp,
    Lumberjack,
    Farm,
    Farmer,
    Shepherd,
    Sheep,
    Unknown,
}

/// Explain the same missing leaf as `missing_source`, without changing admission.
pub(super) fn missing_explanation(
    state: &TickState,
    id: SettlementId,
    item: u32,
    images: &[Image],
) -> Option<(u32, SourceReason)> {
    let leaf = missing_source(state, id, item, images)?;
    let Some(record) = state.settlements.get(id) else {
        return Some((leaf, SourceReason::Unknown));
    };
    let caps = caps(state, id, record);
    let reason = state.registry.items.name_of(leaf).map(|name| {
        let path = name.path();
        if name.namespace() != "minecraft" {
            SourceReason::Unknown
        } else if mine_raw(path) && !caps.mine {
            SourceReason::Mine
        } else if mine_raw(path) && !caps.miner {
            SourceReason::Miner
        } else if log_raw(path) && !caps.camp {
            SourceReason::LumberCamp
        } else if log_raw(path) && !caps.lumberjack {
            SourceReason::Lumberjack
        } else if farmer_raw(path) && !caps.farm {
            SourceReason::Farm
        } else if farmer_raw(path) && !caps.farmer {
            SourceReason::Farmer
        } else if path.ends_with("_wool") && !caps.shepherd {
            SourceReason::Shepherd
        } else if path.ends_with("_wool") {
            SourceReason::Sheep
        } else {
            SourceReason::Unknown
        }
    }).unwrap_or(SourceReason::Unknown);
    Some((leaf, reason))
}

fn caps(state: &TickState, id: SettlementId, record: &SettlementRecord) -> Caps {
    let mut caps = Caps {
        miner: false,
        lumberjack: false,
        farmer: false,
        shepherd: false,
        mine: false,
        camp: false,
        farm: false,
        sheep_colors: 0,
    };
    for slot in residents(state, id) {
        let body = state.entities.body(slot);
        let Some(v) = state.entities.row(slot).villager.as_ref() else {
            continue;
        };
        if !body.alive || v.settlement != Some(id) || v.age < 0 || v.conversion_time.is_some() {
            continue;
        }
        match v.data.profession {
            VillagerProfession::Miner => caps.miner = true,
            VillagerProfession::Lumberjack => caps.lumberjack = true,
            VillagerProfession::Farmer => caps.farmer = true,
            VillagerProfession::Shepherd => caps.shepherd = true,
            _ => {}
        }
    }
    for building in record.buildings.values() {
        match building.kind {
            BuildingKind::Mine => caps.mine = true,
            BuildingKind::LumberCamp => caps.camp = true,
            BuildingKind::Workshop(VillagerProfession::Farmer) => caps.farm = true,
            _ => {}
        }
    }
    caps.sheep_colors = sheep_colors(state, record.anchor, record.radius.min(256));
    caps
}

/// Colors of adult living sheep inside the settlement sphere, read through
/// the entity chunk index over the settlement-bounded chunk range only. The
/// sheared bit is ignored: grazed regrowth keeps a sheared sheep a source.
fn sheep_colors(state: &TickState, anchor: [i32; 3], radius: u32) -> u16 {
    let Ok(sheep_name) = mc_data::Identifier::parse("minecraft:sheep") else {
        return 0;
    };
    let Some(sheep) = state.registry.entity_types.id_of(&sheep_name) else {
        return 0;
    };
    let span = radius as i32;
    let limit = f64::from(radius) * f64::from(radius);
    let (min_cx, max_cx) = (
        anchor[0].saturating_sub(span).div_euclid(16),
        anchor[0].saturating_add(span).div_euclid(16),
    );
    let (min_cz, max_cz) = (
        anchor[2].saturating_sub(span).div_euclid(16),
        anchor[2].saturating_add(span).div_euclid(16),
    );
    let mut colors = 0u16;
    let mut examined = 0usize;
    'outer: for cx in min_cx..=max_cx {
        for cz in min_cz..=max_cz {
            let Some(ids) = state
                .entities
                .brain_sensor_chunks()
                .get(&ChunkPos { x: cx, z: cz })
            else {
                continue;
            };
            for id in ids {
                if examined == MAX_SHEEP_EXAMINED {
                    break 'outer;
                }
                examined += 1;
                let Some(slot) = state.entities.slot_of(*id) else {
                    continue;
                };
                if !matches!(state.entities.body(slot).kind, EntityKind::Mob { type_id } if type_id == sheep)
                {
                    continue;
                }
                let row = state.entities.row(slot);
                if !state.entities.body(slot).alive || !crate::entity_life::adult(row) {
                    continue;
                }
                let p = state.entities.pos(slot);
                let inside = (p[0] - f64::from(anchor[0])).powi(2)
                    + (p[1] - f64::from(anchor[1])).powi(2)
                    + (p[2] - f64::from(anchor[2])).powi(2);
                if inside > limit {
                    continue;
                }
                colors |= 1u16 << (crate::tick::wool_of(&row.appearance) & 15);
            }
        }
    }
    colors
}

/// Miner drop images of the natural blocks miners excavate. No silk touch:
/// stone itself is never a raw yield.
fn mine_raw(path: &str) -> bool {
    matches!(
        path,
        "cobblestone"
            | "cobbled_deepslate"
            | "dirt"
            | "gravel"
            | "sand"
            | "granite"
            | "diorite"
            | "andesite"
            | "tuff"
            | "calcite"
            | "coal"
            | "raw_iron"
            | "raw_copper"
            | "raw_gold"
            | "diamond"
            | "emerald"
            | "redstone"
            | "lapis_lazuli"
    )
}

/// Lumberjack trunk rule: unstripped logs only; nether stems never match.
fn log_raw(path: &str) -> bool {
    path.ends_with("_log") && !path.starts_with("stripped_")
}

/// Farmer crop table, mirroring the harvest worker's mature-crop yields.
fn farmer_raw(path: &str) -> bool {
    matches!(
        path,
        "wheat" | "carrot" | "potato" | "beetroot" | "wheat_seeds" | "beetroot_seeds"
    )
}

fn raw_producible(state: &TickState, caps: &Caps, item: u32) -> Option<bool> {
    let name = state.registry.items.name_of(item)?;
    if !name.as_str().starts_with("minecraft:") {
        return None;
    }
    let path = name.path();
    if mine_raw(path) {
        return Some(caps.miner && caps.mine);
    }
    if log_raw(path) {
        return Some(caps.lumberjack && caps.camp);
    }
    if farmer_raw(path) {
        return Some(caps.farmer && caps.farm);
    }
    let color = path.strip_suffix("_wool")?;
    let index = DYE_COLORS.iter().position(|dyed| *dyed == color)?;
    Some(caps.shepherd && caps.sheep_colors & (1u16 << index) != 0)
}

fn stocked(images: &[Image], item: u32) -> bool {
    images
        .iter()
        .flat_map(|image| &image.slots)
        .any(|stack| !stack.is_empty() && stack.item_id == item && stack.count > 0)
}

fn stocked_ingredient(state: &TickState, images: &[Image], ingredient: &Ingredient) -> bool {
    images.iter().flat_map(|image| &image.slots).any(|stack| {
        !stack.is_empty()
            && mc_data::recipes::ingredient_accepts_stack(
                &state.registry.items,
                &state.registry.item_facts,
                &state.registry.tags,
                stack,
                ingredient,
            )
    })
}

fn is_grid(recipe: &Recipe) -> bool {
    recipe.id.as_str().starts_with("minecraft:")
        && matches!(
            recipe.kind,
            RecipeKind::Shaped(_) | RecipeKind::Shapeless(_)
        )
}

fn result_identity(state: &TickState, recipe: &Recipe) -> Option<(u32, i32)> {
    let count = i32::try_from(recipe.result.count).ok().filter(|c| *c > 0)?;
    if let Some(id) = state.registry.items.id_of(&recipe.result.item) {
        return Some((id, count));
    }
    let stack = recipe
        .result
        .to_stack(&state.registry.items, &state.registry.item_facts)?;
    if stack.count <= 0 {
        return None;
    }
    Some((stack.item_id, count))
}

/// Borrowed one-unit-per-entry ingredient list; `None` for malformed grids.
/// Fixed stack array: shaped borrows `key` refs, shapeless borrows `iter`
/// refs; never clones. The classifier only inspects capability, never
/// consumes ingredients.
fn expanded(recipe: &Recipe) -> Option<[Option<&Ingredient>; 9]> {
    let mut out: [Option<&Ingredient>; 9] = [None; 9];
    let mut len = 0usize;
    match &recipe.kind {
        RecipeKind::Shaped(shaped) => {
            for row in &shaped.pattern {
                for ch in row.chars() {
                    if ch == ' ' {
                        continue;
                    }
                    if len >= 9 {
                        return None;
                    }
                    out[len] = Some(shaped.key.get(&ch)?);
                    len += 1;
                }
            }
        }
        RecipeKind::Shapeless(shapeless) => {
            if shapeless.ingredients.is_empty() || shapeless.ingredients.len() > 9 {
                return None;
            }
            for ingredient in &shapeless.ingredients {
                out[len] = Some(ingredient);
                len += 1;
            }
        }
        _ => return None,
    }
    if len == 0 {
        return None;
    }
    Some(out)
}

/// Unproducible raw ingredient behind `item`, or `None` when stocked,
/// raw-producible, or craftable through any supported recipe path whose
/// every ingredient is in turn stocked or producible.
fn resolve(
    state: &TickState,
    caps: &Caps,
    item: u32,
    images: &[Image],
    depth: usize,
    work: &mut usize,
    visiting: &mut [u32; VISIT_LEN],
) -> Option<u32> {
    if depth > MAX_DEPTH || *work > MAX_WORK {
        return Some(item);
    }
    if stocked(images, item) {
        return None;
    }
    let raw_source = raw_producible(state, caps, item);
    if raw_source == Some(true) {
        return None;
    }
    if visiting[..depth].contains(&item) {
        return Some(item);
    }
    visiting[depth] = item;
    let mut failure = None;
    // Prefer the item's canonical recipe for a useful shortage explanation;
    // recoloring/repacking alternatives may need unrelated extra resources.
    // Every alternative is still eligible to prove that a source exists.
    'recipes: for canonical in [true, false] {
        for recipe in state.registry.recipes.iter() {
            if !is_grid(recipe) || (recipe.id == recipe.result.item) != canonical {
                continue;
            }
            let Some((id, _)) = result_identity(state, recipe) else {
                continue;
            };
            if id != item {
                continue;
            }
            let Some(ingredients) = expanded(recipe) else {
                continue;
            };
            *work = work.saturating_add(1);
            match resolve_ingredients(
                state,
                caps,
                item,
                &ingredients,
                images,
                depth,
                work,
                visiting,
            ) {
                Ok(()) => return None,
                Err(leaf) => {
                    failure.get_or_insert(leaf);
                    if *work > MAX_WORK {
                        break 'recipes;
                    }
                }
            }
        }
    }
    // A supported raw resource remains the useful waiting reason even when
    // its alternate crafting route also lacks a source (wool from string).
    if raw_source.is_some() {
        Some(item)
    } else {
        Some(failure.unwrap_or(item))
    }
}

/// `Ok(())` when every ingredient is stocked or producible through some
/// supported path; otherwise the first unproducible raw leaf.
#[allow(clippy::too_many_arguments)]
fn resolve_ingredients(
    state: &TickState,
    caps: &Caps,
    parent: u32,
    ingredients: &[Option<&Ingredient>; 9],
    images: &[Image],
    depth: usize,
    work: &mut usize,
    visiting: &mut [u32; VISIT_LEN],
) -> Result<(), u32> {
    for ingredient in ingredients.iter().flatten() {
        *work = work.saturating_add(1);
        if *work > MAX_WORK {
            return Err(parent);
        }
        if stocked_ingredient(state, images, ingredient) {
            continue;
        }
        let mut satisfied = false;
        let mut failure = parent;
        let mut supported_failure = None;
        let mut any_candidate = false;
        for (_, candidate) in state.registry.items.iter() {
            if !mc_data::recipes::ingredient_accepts_item(
                &state.registry.items,
                &state.registry.tags,
                candidate,
                ingredient,
            ) {
                continue;
            }
            if !any_candidate {
                // Deterministic fallback: first registry-order candidate.
                any_candidate = true;
                failure = candidate;
            }
            *work = work.saturating_add(1);
            match resolve(state, caps, candidate, images, depth + 1, work, visiting) {
                None => {
                    satisfied = true;
                    break;
                }
                Some(leaf) => {
                    if raw_producible(state, caps, leaf).is_some() {
                        supported_failure.get_or_insert(leaf);
                    }
                    failure = leaf;
                    if *work > MAX_WORK {
                        break;
                    }
                }
            }
        }
        if !satisfied {
            return Err(if any_candidate { supported_failure.unwrap_or(failure) } else { parent });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "construction_availability_tests.rs"]
mod tests;
