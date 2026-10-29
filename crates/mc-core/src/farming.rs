//! Farming, roughly vanilla: hoes till soil, seeds plant on farmland, bone
//! meal grows crops, and random ticks grow crops and saplings, hydrate or
//! dry farmland and spread grass.
//!
//! Random ticks follow vanilla: `randomTickSpeed` (default 3) random cells
//! per 16x16x16 section per tick. Cached eligibility skips inert sections;
//! activity/residency changes refresh the active chunk projection.
//! Every roll is a hash of (world seed, tick, section, sample): no RNG state
//! and the same tick always rolls the same.
//!
//! Crops use raw local light; saplings use day/weather-darkened light.
//! Hydration, soil cover and spreading dirt run only for sampled cells.
//! Plant/tree shapes use shared rules; leaf distance uses native block ticks.

use mc_data::ItemStack;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkSection, SECTION_DIM};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::boundary::{BlockPos, ChunkPos, EntityId, Hand};
use crate::config::CoreError;
use crate::intent::{Footprint, Intent, IntentKind, InvClaim};
use crate::placement::Face;
use crate::tick::TickState;

/// What a block state means to farming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inert,
    Dirt,
    DirtPath,
    /// Other plant soil (podzol, coarse dirt, rooted dirt, moss, mud).
    Soil,
    Grass,
    Mycelium,
    Farmland,
    Crop,
    Sapling,
    Plant,
    Leaves,
    Copper,
    /// Renewable minerals: pointed dripstone and budding amethyst.
    Mineral,
    Ice,
    Snow,
}

impl Kind {
    /// Whether random ticks do anything to it.
    fn ticks(self) -> bool {
        matches!(
            self,
            Self::Grass
                | Self::Mycelium
                | Self::Farmland
                | Self::Crop
                | Self::Sapling
                | Self::Plant
                | Self::Leaves
                | Self::Copper
                | Self::Mineral
                | Self::Ice
                | Self::Snow
        )
    }

    fn tillable(self) -> bool {
        matches!(self, Self::Dirt | Self::DirtPath | Self::Grass)
    }
}

fn kind_of(path: &str) -> Kind {
    match path {
        "dirt" => Kind::Dirt,
        "dirt_path" => Kind::DirtPath,
        "podzol" | "coarse_dirt" | "rooted_dirt" | "moss_block" | "mud" => Kind::Soil,
        "grass_block" => Kind::Grass,
        "mycelium" => Kind::Mycelium,
        "farmland" => Kind::Farmland,
        "ice" => Kind::Ice,
        "snow" => Kind::Snow,
        "wheat" | "carrots" | "potatoes" | "beetroots" | "torchflower_crop" => Kind::Crop,
        "bamboo_sapling" | "cactus" | "sugar_cane" | "bamboo" | "nether_wart" | "melon_stem"
        | "pumpkin_stem" | "sweet_berry_bush" | "cocoa" | "kelp" | "cave_vines"
        | "weeping_vines" | "twisting_vines" | "vine" | "brown_mushroom" | "red_mushroom"
        | "chorus_flower" | "pitcher_crop" => Kind::Plant,
        _ if crate::copper::family(path).is_some_and(|(_, age, waxed)| age < 3 && !waxed) => {
            Kind::Copper
        }
        "pointed_dripstone" | "budding_amethyst" => Kind::Mineral,
        "turtle_egg" | "sniffer_egg" | "frogspawn" => Kind::Mineral,
        _ if path.ends_with("_leaves") => Kind::Leaves,
        _ if path.ends_with("_sapling") && !path.starts_with("potted_") => Kind::Sapling,
        _ => Kind::Inert,
    }
}

/// Farming meaning of every block state, built once at startup.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub struct FarmStates {
    kinds: Vec<Kind>,
    /// Farmland by `moisture` 0..=7.
    farmland: [u32; 8],
}

impl FarmStates {
    pub(crate) fn resolve(blocks: &BlockRegistry) -> Result<Self, CoreError> {
        let farmland_id =
            mc_data::Identifier::parse("minecraft:farmland").expect("vanilla identifier parses");
        let mut farmland = [0; 8];
        for (moisture, slot) in farmland.iter_mut().enumerate() {
            *slot = blocks
                .by_name_and_props(
                    &farmland_id,
                    &[("moisture".to_owned(), moisture.to_string())],
                )
                .map(|state| state.0)
                .ok_or_else(|| CoreError::Registry(format!("missing farmland {moisture}")))?;
        }
        Ok(Self {
            kinds: blocks
                .states()
                .map(|state| kind_of(state.block.id.path()))
                .collect(),
            farmland,
        })
    }

    fn kind(&self, state: u32) -> Kind {
        self.kinds
            .get(state as usize)
            .copied()
            .unwrap_or(Kind::Inert)
    }

    /// Whether anything in the section can random tick.
    fn section_ticks(&self, section: &ChunkSection) -> bool {
        if section.is_empty() {
            return false;
        }
        match section.palette() {
            Some(palette) => palette.iter().any(|state| self.kind(state.0).ticks()),
            None => self.kind(section.get(0, 0, 0).0).ticks(),
        }
    }
}

/// Palette eligibility is derived from committed chunks, not the tick overlay.
/// World-unique chunk revisions invalidate every section conservatively on
/// edits and reloads; unchanged chunks never rescan their palettes.
#[derive(Default)]
pub(crate) struct FarmSections {
    chunks: FxHashMap<ChunkPos, (u64, u32)>,
    active_revision: Option<u64>,
    active_chunks: Vec<ChunkPos>,
}

impl FarmSections {
    fn refresh_active(&mut self, world: &crate::world::ChunkWorld) {
        let revision = world.active_revision();
        if self.active_revision == Some(revision) {
            return;
        }
        // Membership changes also fence eviction/reload and chunk-map order.
        // Keep the original chunk traversal order for overlapping plant plans.
        self.chunks.retain(|pos, _| world.has_chunk(pos.x, pos.z));
        self.active_chunks.clear();
        self.active_chunks.extend(
            world
                .chunks()
                .filter(|chunk| world.is_block_active(chunk.pos.x, chunk.pos.z))
                .map(|chunk| chunk.pos),
        );
        self.active_revision = Some(revision);
    }

    fn mask(&mut self, farming: &FarmStates, chunk: &Chunk, revision: u64) -> u32 {
        let pos = ChunkPos {
            x: chunk.pos.x,
            z: chunk.pos.z,
        };
        let cached = self.chunks.entry(pos).or_insert((u64::MAX, 0));
        if cached.0 != revision {
            debug_assert_eq!(chunk.sections.len(), chunk.geometry().section_count());
            let mask = chunk
                .sections
                .iter()
                .enumerate()
                .fold(0, |mask, (index, section)| {
                    mask | (u32::from(farming.section_ticks(section)) << index)
                });
            *cached = (revision, mask);
        }
        cached.1
    }
}

/// Items that plant a block of another name (vanilla `Items`
/// `createBlockItemWithCustomItemName`): `(item, block)`.
const PLANTING: [(&str, &str); 10] = [
    ("wheat_seeds", "wheat"),
    ("beetroot_seeds", "beetroots"),
    ("carrot", "carrots"),
    ("potato", "potatoes"),
    ("melon_seeds", "melon_stem"),
    ("pumpkin_seeds", "pumpkin_stem"),
    ("sweet_berries", "sweet_berry_bush"),
    ("cocoa_beans", "cocoa"),
    ("torchflower_seeds", "torchflower_crop"),
    ("pitcher_pod", "pitcher_crop"),
];

/// The block an item places when it is not the block of the same name:
/// seeds, carrots and potatoes plant crops, and the harvested wheat item
/// places nothing.
#[must_use]
pub fn placed_block<'a>(
    blocks: &'a BlockRegistry,
    item: &mc_data::Identifier,
) -> Option<&'a mc_world::Block> {
    if item.path() == "wheat" {
        return None;
    }
    let Some((_, crop)) = PLANTING
        .iter()
        .find(|(planting, _)| *planting == item.path())
    else {
        return blocks.block(item);
    };
    blocks.block(&mc_data::Identifier::parse(format!("minecraft:{crop}")).ok()?)
}

/// The item that plants `block` (a crop path such as `wheat`), if any.
#[must_use]
pub fn planting_item(block: &str) -> Option<&'static str> {
    PLANTING
        .iter()
        .find(|(_, planted)| *planted == block)
        .map(|(item, _)| *item)
}

/// Vanilla placement policy beyond raw support: farmland crops need light
/// (Java refuses planting below 8), and flowers need soil, not bare stone.
#[must_use]
pub fn can_plant(state: &TickState, placed: u32, pos: BlockPos) -> bool {
    let Some(block) = state.registry.blocks.by_id(BlockStateId(placed)) else {
        return false;
    };
    let path = block.block.id.path();
    if matches!(
        path,
        "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "melon_stem"
            | "pumpkin_stem"
            | "torchflower_crop"
            | "pitcher_crop"
    ) {
        return state.raw_brightness(pos.x, pos.y, pos.z) >= 8;
    }
    let flower = state.registry.tags.contains_raw_id(
        "minecraft:block",
        "minecraft:small_flowers",
        block.block.raw_id,
    ) || matches!(
        path,
        "sunflower" | "lilac" | "rose_bush" | "peony" | "torchflower" | "pitcher_plant"
    );
    if !flower {
        return true;
    }
    let Some(support) = state
        .world
        .block_at(pos.x, pos.y - 1, pos.z)
        .and_then(|id| {
            state
                .registry
                .blocks
                .by_id(BlockStateId(id))
                .map(|block| block.block.id.path().to_owned())
        })
    else {
        return true;
    };
    matches!(
        support.as_str(),
        "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "mud"
            | "muddy_mangrove_roots"
            | "moss_block"
            | "pale_moss_block"
            | "grass_block"
            | "podzol"
            | "mycelium"
            | "farmland"
    )
}

/// Whether a freshly placed plant has its required native support.
#[must_use]
pub fn can_stand(state: &TickState, placed: u32, pos: BlockPos) -> bool {
    (!matches!(
        state.registry.farming.kind(placed),
        Kind::Crop | Kind::Sapling | Kind::Plant
    ) || state.world.block_at(pos.x, pos.y - 1, pos.z).is_some())
        && crate::plants::can_stand(state, placed, pos)
}

pub(crate) fn use_intent(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: Face,
    held: &ItemStack,
) -> Option<Intent> {
    let mut intent = plan_use_intent(state, entity, hand, clicked, face, held)?;
    intent.seq = state.next_seq(entity);
    Some(intent)
}

/// A hoe or bone meal use on `clicked`: the intent that tills the soil or
/// grows the crop, `None` when the held item does nothing there.
pub(crate) fn plan_use_intent(
    state: &TickState,
    entity: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: Face,
    held: &ItemStack,
) -> Option<Intent> {
    plan_use_intent_at(state, entity, hand, clicked, face, held, state.tick)
}

pub(crate) fn plan_use_intent_at(
    state: &TickState,
    entity: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: Face,
    held: &ItemStack,
    planning_tick: u64,
) -> Option<Intent> {
    let farming = &state.registry.farming;
    let item = state.registry.items.name_of(held.item_id)?.path();
    let player = state.players.get(&entity)?;
    if player.spectator() {
        return None;
    }
    let current = state.world.block_at(clicked.x, clicked.y, clicked.z)?;
    let kind = farming.kind(current);
    if item == "bone_meal" && kind != Kind::Crop {
        let plan = crate::plants::bonemeal(
            state,
            clicked,
            hash(&[
                planning_tick,
                entity.0 as u64,
                clicked.x as u64,
                clicked.y as u64,
                clicked.z as u64,
            ]),
        )?;
        let slot = crate::tick::held_slot_index(player, hand);
        let footprint = Footprint {
            block_expect: plan.expects,
            block_write: plan.writes,
            inv: vec![InvClaim {
                owner: entity,
                slot: u16::try_from(slot).unwrap_or(u16::MAX),
                expect_item: held.item_id,
                expect_count: held.count,
                take: i32::from(!player.creative()),
                put_item: None,
                put_count: 0,
            }],
            ..Default::default()
        };
        return Some(Intent {
            actor: entity,
            seq: 0,
            kind: IntentKind::Cultivate {
                pos: clicked,
                wear: 0,
            },
            footprint,
        });
    }
    let (write, expect_above, wear, take) = if item.ends_with("_hoe") && hand == Hand::Main {
        let above = BlockPos {
            y: clicked.y + 1,
            ..clicked
        };
        let air = state.registry.states.air;
        if face == Face::Down
            || !kind.tillable()
            || state.world.block_at(above.x, above.y, above.z) != Some(air)
        {
            return None;
        }
        (
            farming.farmland[0],
            Some((above, air)),
            i32::from(!player.creative()),
            0,
        )
    } else if item == "bone_meal" && kind == Kind::Crop {
        let grown = bone_meal_state(
            state,
            clicked,
            hash(&[
                planning_tick,
                entity.0 as u64,
                clicked.x as u64,
                clicked.y as u64,
                clicked.z as u64,
            ]),
        )?;
        (grown, None, 0, i32::from(!player.creative()))
    } else {
        return None;
    };
    let slot = crate::tick::held_slot_index(player, hand);
    let mut block_expect = vec![(clicked, current)];
    block_expect.extend(expect_above);
    let footprint = Footprint {
        block_expect,
        block_write: vec![(clicked, write)],
        inv: vec![InvClaim {
            owner: entity,
            slot: u16::try_from(slot).unwrap_or(u16::MAX),
            expect_item: held.item_id,
            expect_count: held.count,
            take,
            put_item: None,
            put_count: 0,
        }],
        ..Default::default()
    };
    Some(Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::Cultivate { pos: clicked, wear },
        footprint,
    })
}

/// Award only after the whole cultivation footprint and held debit are accepted.
pub(crate) fn accepted_use(state: &mut TickState, intent: &Intent, pos: BlockPos) {
    let held = intent
        .footprint
        .inv
        .iter()
        .find_map(|claim| {
            state
                .players
                .get(&claim.owner)?
                .inventory
                .slots
                .get(claim.slot as usize)
        })
        .cloned();
    let result = intent
        .footprint
        .block_write
        .iter()
        .find(|(at, _)| *at == pos)
        .map(|(_, id)| *id);
    crate::advancements::accepted_block(
        state,
        intent.actor,
        "minecraft:item_used_on_block",
        pos,
        result,
        held,
    );
    // Tall-flower duplication: the plan edits nothing; the duplicate drops
    // only when the bone meal is actually paid (the debit commits with the
    // intent, so a raced-out stack must not conjure a flower; creative pays
    // nothing and still duplicates, as in vanilla).
    let paid = intent.footprint.inv.iter().all(|claim| {
        state.players.get(&claim.owner).is_some_and(|player| {
            player.creative()
                || player
                    .inventory
                    .slots
                    .get(claim.slot as usize)
                    .is_some_and(|stack| stack.count >= claim.take)
        })
    });
    if !paid {
        return;
    }
    if let Some(current) = state.world.block_at(pos.x, pos.y, pos.z)
        && let Some(block) = state.registry.blocks.by_id(BlockStateId(current))
        && matches!(
            block.block.id.path(),
            "sunflower" | "lilac" | "rose_bush" | "peony"
        )
        && let Some(name) =
            mc_data::Identifier::parse(format!("minecraft:{}", block.block.id.path())).ok()
        && let Some(item) = state.registry.items.id_of(&name)
    {
        state.merge_life_drops.push((
            [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5],
            mc_data::ItemStack::new(item, 1),
        ));
    }
}

/// Plan only an actual landing. The caller supplies accepted position/distance,
/// never the client's claimed ground flag. Merge fences intervening block edits.
pub(crate) fn trample_intent(
    state: &TickState,
    entity: EntityId,
    position: [f64; 3],
    distance: f64,
) -> Option<Intent> {
    if distance <= 0.5 || state.players.get(&entity).is_some_and(|p| p.spectator()) {
        return None;
    }
    let at = BlockPos {
        x: position[0].floor() as i32,
        y: (position[1] - 0.01).floor() as i32,
        z: position[2].floor() as i32,
    };
    let id = state.world.block_at(at.x, at.y, at.z)?;
    if state.registry.farming.kind(id) != Kind::Farmland
        || crate::plugin_player::protects_cell(state, at)
    {
        return None;
    }
    let mut roll = Roll::new(hash(&[
        state.tick,
        entity.0 as u64,
        at.x as u64,
        at.z as u64,
    ]));
    if f64::from(roll.below(1_000_000)) / 1_000_000.0 >= distance - 0.5 {
        return None;
    }
    Some(Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::TrampleFarmland,
        footprint: Footprint {
            block_expect: vec![(at, id)],
            block_write: vec![(at, state.registry.states.dirt)],
            ..Default::default()
        },
    })
}

/// Shared crop bone-meal planner for player uses and native dispensers.
pub(crate) fn bone_meal_state(state: &TickState, pos: BlockPos, seed: u64) -> Option<u32> {
    let blocks = &state.registry.blocks;
    let current = state.world.block_at(pos.x, pos.y, pos.z)?;
    if state.registry.farming.kind(current) != Kind::Crop {
        return None;
    }
    let (age, max) = age_of(blocks, current)?;
    if age >= max && !is_block(blocks, current, "torchflower_crop") {
        return None;
    }
    let mut roll = Roll::new(seed);
    let mut grown = if is_block(blocks, current, "torchflower_crop") {
        1
    } else {
        2 + roll.below(4)
    };
    if is_block(blocks, current, "beetroots") {
        grown /= 3;
    }
    if is_block(blocks, current, "torchflower_crop") && age + grown > max {
        return blocks
            .by_name_and_props(
                &mc_data::Identifier::parse("minecraft:torchflower").ok()?,
                &[],
            )
            .map(|id| id.0);
    }
    with_prop(blocks, current, "age", &(age + grown).min(max).to_string())
}

/// What a random tick decided for one cell.
struct Change {
    at: BlockPos,
    /// The state the decision was made on; stale decisions are dropped.
    was: u32,
    outcome: Outcome,
}

enum Outcome {
    Set(Vec<(BlockPos, u32)>),
    Plants(crate::plants::Plan),
    LeafDecay,
    LeafRefresh,
    /// The block pops off and drops its loot.
    Pop,
    /// Egg blocks hatch: rewrite cells, then spawn live babies.
    Hatch {
        cells: Vec<(BlockPos, u32)>,
        spawn: Vec<crate::plants::EggSpawn>,
    },
}

/// Random-tick every loaded section once. New states go to the world and
/// `writes`; popped crops drop their loot into `spawned`.
pub fn run(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, spawned: &mut Vec<EntityId>) {
    let air = state.registry.states.air;
    let speed =
        crate::command_rules::integer(state, crate::command_rules::IntRule::RANDOM_TICK_SPEED)
            .max(0) as u32;
    for Change { at, was, outcome } in sample(state, speed, PARALLEL_MIN_CHUNKS) {
        if state.world.block_at(at.x, at.y, at.z) != Some(was) {
            continue;
        }
        let cells = match outcome {
            Outcome::Set(cells) => cells,
            Outcome::Plants(plan) => {
                if plan
                    .expects
                    .iter()
                    .any(|(pos, id)| state.world.block_at(pos.x, pos.y, pos.z) != Some(*id))
                {
                    continue;
                }
                plan.writes
            }
            Outcome::LeafRefresh => {
                crate::leaves::schedule(state, at, was);
                continue;
            }
            Outcome::LeafDecay => {
                if !crate::leaves::decays(state, at, was) {
                    continue;
                }
                if !state.pop_block_loot(at, was, spawned) {
                    continue;
                }
                vec![(
                    at,
                    crate::placement::break_state(
                        &state.registry.blocks,
                        was,
                        air,
                        state.registry.states.water_levels[0],
                    ),
                )]
            }
            Outcome::Pop => {
                if !state.pop_block_loot(at, was, spawned) {
                    continue;
                }
                vec![(at, air)]
            }
            Outcome::Hatch { cells, spawn } => {
                for birth in spawn {
                    let id = state.spawn_mob(birth.type_id, birth.pos, birth.health);
                    crate::entity_life::make_child(state, id);
                    spawned.push(id);
                }
                cells
            }
        };
        for (pos, new) in cells {
            state.world.set_block(pos.x, pos.y, pos.z, new);
            writes.retain(|(written, _)| *written != pos);
            writes.push((pos, new));
        }
    }
}

/// Below this many ticking chunks the rayon hand-off costs more than the loop.
/// Serial sampling costs ~0.5 us per ordinary chunk (~6 us when every section
/// ticks; 625 such chunks: 3.7 ms serial, 1.2 ms on the pool). A release
/// real-client run with 81 chunks spent ~42 ms waiting for woken pool workers
/// on ~10% of ticks while sampling itself took ~40 us per tick.
const PARALLEL_MIN_CHUNKS: usize = 256;

/// A sampled cell whose block can random tick, with the roll stream after the
/// three coordinate draws.
struct Hit {
    at: BlockPos,
    id: u32,
    kind: Kind,
    roll: u64,
}

/// `speed` random cells of every section that can tick, and what each does.
///
/// Drawing coordinates and classifying the sampled block are pure reads of the
/// world, so they run per chunk on the pool; `tick_cell` then runs serially over
/// the hits in the original chunk/section/sample order.
fn sample(state: &mut TickState, speed: u32, min_parallel: usize) -> Vec<Change> {
    let mut changes = Vec::new();
    if speed == 0 {
        return changes;
    }
    state.farm_sections.refresh_active(&state.world);
    let farming = &state.registry.farming;
    let world = &state.world;
    let mut work: Vec<(&Chunk, u32)> = Vec::with_capacity(state.farm_sections.active_chunks.len());
    for active in 0..state.farm_sections.active_chunks.len() {
        let pos = state.farm_sections.active_chunks[active];
        let chunk = world.chunk(pos).expect("cached active resident chunk");
        let ticking = state.farm_sections.mask(
            farming,
            chunk,
            world.chunk_revision(chunk.pos.x, chunk.pos.z),
        );
        if ticking != 0 {
            work.push((chunk, ticking));
        }
    }
    // Only chunks with pending overlay edits need the overlay-aware read;
    // elsewhere it returns exactly the committed section cell.
    let overlay_chunks: FxHashSet<(i32, i32)> = world
        .overlay_entries()
        .map(|((x, _, z), _)| (x.div_euclid(16), z.div_euclid(16)))
        .collect();
    let (seed, tick) = (world.seed() as u64, state.tick);
    let hits_of = |&(chunk, ticking): &(&Chunk, u32)| {
        let mut hits = Vec::new();
        let overlay = overlay_chunks.contains(&(chunk.pos.x, chunk.pos.z));
        sample_chunk(
            world, farming, overlay, seed, tick, speed, chunk, ticking, &mut hits,
        );
        hits
    };
    let hits: Vec<Vec<Hit>> = if work.len() >= min_parallel {
        work.par_iter().with_min_len(16).map(hits_of).collect()
    } else {
        work.iter().map(hits_of).collect()
    };
    for Hit { at, id, kind, roll } in hits.into_iter().flatten() {
        tick_cell(state, at, id, kind, &mut Roll::new(roll), &mut changes);
    }
    changes
}

#[allow(clippy::too_many_arguments)]
fn sample_chunk(
    world: &crate::world::ChunkWorld,
    farming: &FarmStates,
    overlay: bool,
    seed: u64,
    tick: u64,
    speed: u32,
    chunk: &Chunk,
    mut ticking: u32,
    hits: &mut Vec<Hit>,
) {
    while ticking != 0 {
        let index = ticking.trailing_zeros() as usize;
        ticking &= ticking - 1;
        let section = &chunk.sections[index];
        // The sample hash folds seed/tick/chunk/section identically for
        // every draw in this section: fold that prefix once so per-sample
        // work is one `mix`, with bit-identical `Roll` seeds (same draws,
        // order and distribution as the full six-part fold).
        let prefix = hash(&[
            seed,
            tick,
            chunk.pos.x as u64,
            chunk.pos.z as u64,
            index as u64,
        ]);
        let base_y = chunk.geometry().min_y() + (index * SECTION_DIM) as i32;
        for sample in 0..u64::from(speed) {
            let mut roll = Roll::new(mix(prefix ^ sample));
            let local = [roll.below(16), roll.below(16), roll.below(16)];
            let at = BlockPos {
                x: chunk.pos.x * 16 + local[0] as i32,
                y: base_y + local[1] as i32,
                z: chunk.pos.z * 16 + local[2] as i32,
            };
            let mut id = section
                .get(local[0] as u8, local[1] as u8, local[2] as u8)
                .0;
            if overlay {
                id = world.block_at(at.x, at.y, at.z).unwrap_or(id);
            }
            let kind = farming.kind(id);
            if kind.ticks() {
                hits.push(Hit {
                    at,
                    id,
                    kind,
                    roll: roll.0,
                });
            }
        }
    }
}

/// Replays only cereal-crop random samples over a bounded debt interval.
/// Intermediate stages stay local until the merge owner validates block_expect.
/// Light, moisture and random-tick speed come from the current resident
/// snapshot; this does not reconstruct historical weather or farmland drying.
pub(crate) fn catch_up_crops(
    state: &TickState,
    pos: ChunkPos,
    elapsed: u64,
) -> Vec<(BlockPos, u32, u32)> {
    let Some(chunk) = state.world.chunk(pos) else {
        return Vec::new();
    };
    let speed =
        crate::command_rules::integer(state, crate::command_rules::IntRule::RANDOM_TICK_SPEED)
            .max(0) as u64;
    let start = state.tick.wrapping_sub(state.world.simulation_debt(pos));
    let mut images: FxHashMap<BlockPos, (u32, u32)> = FxHashMap::default();
    for (index, section) in chunk.sections.iter().enumerate() {
        let eligible = match section.palette() {
            Some(palette) => palette
                .iter()
                .any(|id| state.registry.farming.kind(id.0) == Kind::Crop),
            None => state.registry.farming.kind(section.get(0, 0, 0).0) == Kind::Crop,
        };
        if !eligible {
            continue;
        }
        for offset in 0..elapsed {
            for sample in 0..speed {
                let mut roll = Roll::new(hash(&[
                    state.world.seed() as u64,
                    start.wrapping_add(offset),
                    pos.x as u64,
                    pos.z as u64,
                    index as u64,
                    sample,
                ]));
                let local = [roll.below(16), roll.below(16), roll.below(16)];
                let at = BlockPos {
                    x: pos.x * 16 + local[0] as i32,
                    y: chunk.geometry().min_y() + (index * SECTION_DIM) as i32 + local[1] as i32,
                    z: pos.z * 16 + local[2] as i32,
                };
                let Some(initial) = state.world.block_at(at.x, at.y, at.z) else {
                    continue;
                };
                let current = images.get(&at).map_or(initial, |(_, next)| *next);
                if state.registry.farming.kind(current) != Kind::Crop
                    || state.world.light_at(at.x, at.y, at.z).is_none()
                    || (-1..=1).any(|dx| {
                        (-1..=1).any(|dz| {
                            [at.y - 1, at.y]
                                .into_iter()
                                .any(|y| state.world.block_at(at.x + dx, y, at.z + dz).is_none())
                        })
                    })
                {
                    continue;
                }
                if let Some(Outcome::Set(cells)) = crop(state, at, current, &mut roll)
                    && let Some((_, next)) = cells.into_iter().next()
                {
                    images.insert(at, (initial, next));
                }
            }
        }
    }
    let mut images: Vec<_> = images
        .into_iter()
        .map(|(at, (initial, next))| (at, initial, next))
        .collect();
    images.sort_unstable_by_key(|(at, _, _)| (at.x, at.y, at.z));
    images
}

fn tick_cell(
    state: &TickState,
    at: BlockPos,
    id: u32,
    kind: Kind,
    roll: &mut Roll,
    out: &mut Vec<Change>,
) {
    let outcome = match kind {
        Kind::Crop => crop(state, at, id, roll),
        Kind::Farmland => farmland(state, at, id),
        Kind::Grass | Kind::Mycelium => grass(state, at, roll),
        Kind::Ice | Kind::Snow => {
            let light = state
                .world
                .light_at(at.x, at.y, at.z)
                .map(|(_, block)| block);
            let dampening = if kind == Kind::Ice {
                state.registry.light.opacity(id).unwrap_or(0)
            } else {
                0
            };
            if light.is_some_and(|light| light > 11_u8.saturating_sub(dampening)) {
                if kind == Kind::Snow {
                    Some(Outcome::Pop)
                } else {
                    let next = if state.dimension.water_evaporates() {
                        state.registry.states.air
                    } else {
                        state.registry.states.water
                    };
                    Some(Outcome::Set(vec![(at, next)]))
                }
            } else {
                None
            }
        }
        Kind::Sapling => sapling(state, at, id, roll),
        Kind::Copper => {
            let gate = roll.below(u32::MAX);
            let scan = roll.below(u32::MAX);
            crate::copper::random_tick(state, at, id, gate, scan).map(Outcome::Set)
        }
        Kind::Mineral => {
            let seed = hash(&[roll.below(u32::MAX) as u64, state.tick]);
            if let Some(hatch) = crate::plants::egg_hatch(state, at, id, seed) {
                Some(Outcome::Hatch {
                    cells: hatch.cells,
                    spawn: hatch.spawn,
                })
            } else {
                crate::plants::random_growth(state, at, id, seed).map(Outcome::Plants)
            }
        }
        Kind::Plant => {
            if is_block(&state.registry.blocks, id, "pitcher_crop") {
                if prop(&state.registry.blocks, id, "half") == Some("lower") {
                    crop(state, at, id, roll)
                } else {
                    None
                }
            } else if !can_stand(state, id, at) {
                Some(Outcome::Pop)
            } else {
                crate::plants::random_growth(
                    state,
                    at,
                    id,
                    hash(&[roll.below(u32::MAX) as u64, state.tick]),
                )
                .map(Outcome::Plants)
            }
        }
        Kind::Leaves => {
            let stored = prop(&state.registry.blocks, id, "distance")
                .and_then(|value| value.parse::<u8>().ok());
            match crate::leaves::distance(state, at) {
                Some(distance) if stored == Some(distance) => (distance == 7
                    && prop(&state.registry.blocks, id, "persistent") == Some("false"))
                .then_some(Outcome::LeafDecay),
                _ => Some(Outcome::LeafRefresh),
            }
        }
        _ => None,
    };
    if let Some(outcome) = outcome {
        out.push(Change {
            at,
            was: id,
            outcome,
        });
    }
}

fn kind_at(state: &TickState, x: i32, y: i32, z: i32) -> Kind {
    state
        .world
        .block_at(x, y, z)
        .map_or(Kind::Inert, |id| state.registry.farming.kind(id))
}

/// Vanilla `CropBlock.randomTick`: a crop off farmland pops; otherwise it
/// ages with chance `1 / (25 / growth speed + 1)` (beetroots a third of that).
fn crop(state: &TickState, at: BlockPos, id: u32, roll: &mut Roll) -> Option<Outcome> {
    if kind_at(state, at.x, at.y - 1, at.z) != Kind::Farmland {
        return Some(Outcome::Pop);
    }
    let blocks = &state.registry.blocks;
    let (age, max) = age_of(blocks, id)?;
    let torchflower = is_block(blocks, id, "torchflower_crop");
    let pitcher = is_block(blocks, id, "pitcher_crop");
    if state.raw_brightness(at.x, at.y, at.z) < 9
        || (!torchflower && age >= max)
        || roll.below((25.0 / growth_speed(state, at, id)) as u32 + 1) != 0
        || (is_block(blocks, id, "beetroots") && roll.below(3) != 0)
        || (torchflower && roll.below(3) == 0)
    {
        return None;
    }
    let grown = if torchflower && age == max {
        blocks
            .by_name_and_props(
                &mc_data::Identifier::parse("minecraft:torchflower").ok()?,
                &[],
            )?
            .0
    } else {
        with_prop(blocks, id, "age", &(age + 1).to_string())?
    };
    let mut writes = vec![(at, grown)];
    if pitcher && age + 1 >= 3 {
        let above = BlockPos { y: at.y + 1, ..at };
        let previous = state.world.block_at(above.x, above.y, above.z)?;
        if previous != state.registry.states.air && !is_block(blocks, previous, "pitcher_crop") {
            return None;
        }
        writes.push((above, with_prop(blocks, grown, "half", "upper")?));
    }
    Some(Outcome::Set(writes))
}

/// Vanilla `CropBlock.getGrowthSpeed`: farmland under and around the crop
/// speeds it (more when wet), a same-crop neighbour pattern halves it.
fn growth_speed(state: &TickState, at: BlockPos, crop: u32) -> f32 {
    let farming = &state.registry.farming;
    let mut speed = 1.0;
    for dx in -1..=1 {
        for dz in -1..=1 {
            let mut gain = match state.world.block_at(at.x + dx, at.y - 1, at.z + dz) {
                Some(id) if farming.kind(id) == Kind::Farmland => {
                    if id == farming.farmland[0] {
                        1.0
                    } else {
                        3.0
                    }
                }
                _ => 0.0,
            };
            if dx != 0 || dz != 0 {
                gain /= 4.0;
            }
            speed += gain;
        }
    }
    let same = |dx: i32, dz: i32| {
        state
            .world
            .block_at(at.x + dx, at.y, at.z + dz)
            .is_some_and(|id| same_block(&state.registry.blocks, id, crop))
    };
    let along_x = same(-1, 0) || same(1, 0);
    let along_z = same(0, -1) || same(0, 1);
    let diagonal = same(-1, -1) || same(1, -1) || same(-1, 1) || same(1, 1);
    if (along_x && along_z) || diagonal {
        speed /= 2.0;
    }
    speed
}

/// Vanilla `FarmBlock.randomTick`: water within four blocks (same level or
/// one above) soaks it to 7; otherwise it dries one step at a time, and
/// bone-dry farmland without a crop turns to dirt.
fn farmland(state: &TickState, at: BlockPos, id: u32) -> Option<Outcome> {
    let states = &state.registry.states;
    let farming = &state.registry.farming;
    let above = state.world.block_at(at.x, at.y + 1, at.z)?;
    if covered_farmland(state, at, id) {
        return Some(Outcome::Set(vec![(at, states.dirt)]));
    }
    let wet = (-4..=4).any(|dx| {
        (-4..=4).any(|dz| {
            (0..=1).any(|dy| {
                state
                    .world
                    .block_at(at.x + dx, at.y + dy, at.z + dz)
                    .is_some_and(|near| {
                        states.water_levels.contains(&near)
                            || prop(&state.registry.blocks, near, "waterlogged") == Some("true")
                            || is_block(&state.registry.blocks, near, "kelp")
                            || is_block(&state.registry.blocks, near, "kelp_plant")
                            || is_block(&state.registry.blocks, near, "seagrass")
                            || is_block(&state.registry.blocks, near, "tall_seagrass")
                    })
            })
        })
    }) || crate::conduit::rain_exposed(
        state,
        [f64::from(at.x), f64::from(at.y), f64::from(at.z)],
    );
    let moisture = farming.farmland.iter().position(|state| *state == id)?;
    let new = if wet {
        (moisture < 7).then_some(farming.farmland[7])?
    } else if moisture > 0 {
        farming.farmland[moisture - 1]
    } else if farming.kind(above) == Kind::Crop
        || is_block(&state.registry.blocks, above, "melon_stem")
        || is_block(&state.registry.blocks, above, "pumpkin_stem")
        || is_block(&state.registry.blocks, above, "attached_melon_stem")
        || is_block(&state.registry.blocks, above, "pitcher_crop")
        || is_block(&state.registry.blocks, above, "attached_pumpkin_stem")
    {
        return None;
    } else {
        states.dirt
    };
    Some(Outcome::Set(vec![(at, new)]))
}

/// Cover checks are also used by the event-driven neighbor settlement.
pub(crate) fn covered_farmland(state: &TickState, at: BlockPos, id: u32) -> bool {
    if state.registry.farming.kind(id) != Kind::Farmland {
        return false;
    }
    state
        .world
        .block_at(at.x, at.y + 1, at.z)
        .is_some_and(|above| {
            state.registry.materials.classify(above).is_solid()
                && !is_block(&state.registry.blocks, above, "pitcher_crop")
                && !prop(&state.registry.blocks, above, "facing").is_some_and(|_| {
                    state
                        .registry
                        .blocks
                        .by_id(BlockStateId(above))
                        .is_some_and(|block| block.block.id.path().ends_with("_fence_gate"))
                })
        })
}

fn grass_survives(state: &TickState, at: BlockPos) -> bool {
    let Some(above) = state.world.block_at(at.x, at.y + 1, at.z) else {
        return true;
    };
    if is_block(&state.registry.blocks, above, "snow")
        && prop(&state.registry.blocks, above, "layers") == Some("1")
    {
        return true;
    }
    !state.registry.states.water_levels.contains(&above)
        && prop(&state.registry.blocks, above, "waterlogged") != Some("true")
        && state
            .registry
            .light
            .opacity(above)
            .is_none_or(|opacity| opacity < 15)
}

/// Grass and mycelium share survival/light rules, preserving their own family.
fn grass(state: &TickState, at: BlockPos, roll: &mut Roll) -> Option<Outcome> {
    if !grass_survives(state, at) {
        return Some(Outcome::Set(vec![(at, state.registry.states.dirt)]));
    }
    if state.raw_brightness(at.x, at.y + 1, at.z) < 9 {
        return None;
    }
    let source = state.world.block_at(at.x, at.y, at.z)?;
    let mut cells = Vec::new();
    for _ in 0..4 {
        let target = BlockPos {
            x: at.x + roll.below(3) as i32 - 1,
            y: at.y + roll.below(5) as i32 - 3,
            z: at.z + roll.below(3) as i32 - 1,
        };
        if kind_at(state, target.x, target.y, target.z) == Kind::Dirt
            && grass_survives(state, target)
        {
            let above = state.world.block_at(target.x, target.y + 1, target.z)?;
            let snowy = if is_block(&state.registry.blocks, above, "snow") {
                "true"
            } else {
                "false"
            };
            let grown = with_prop(&state.registry.blocks, source, "snowy", snowy)?;
            cells.push((target, grown));
        }
    }
    (!cells.is_empty()).then_some(Outcome::Set(cells))
}

/// Vanilla `SaplingBlock.randomTick`: a one in seven chance to advance the
/// stage, and a sapling already at stage 1 grows its tree.
fn sapling(state: &TickState, at: BlockPos, id: u32, roll: &mut Roll) -> Option<Outcome> {
    if !can_stand(state, id, at) {
        return Some(Outcome::Pop);
    }
    if state.brightness(at.x, at.y + 1, at.z) < 9 || roll.below(7) != 0 {
        return None;
    }
    crate::plants::random_growth(
        state,
        at,
        id,
        hash(&[roll.below(u32::MAX) as u64, state.tick]),
    )
    .map(Outcome::Plants)
}

pub(crate) fn prop<'a>(blocks: &'a BlockRegistry, state: u32, name: &str) -> Option<&'a str> {
    blocks
        .by_id(BlockStateId(state))?
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// The state with one property changed.
pub(crate) fn with_prop(
    blocks: &BlockRegistry,
    state: u32,
    name: &str,
    value: &str,
) -> Option<u32> {
    let current = blocks.by_id(BlockStateId(state))?;
    let mut props = current.properties.clone();
    props.iter_mut().find(|(key, _)| key == name)?.1 = value.to_owned();
    blocks
        .by_name_and_props(&current.block.id, &props)
        .map(|id| id.0)
}

/// `(age, max age)` of a crop state.
pub(crate) fn age_of(blocks: &BlockRegistry, state: u32) -> Option<(u32, u32)> {
    let age = prop(blocks, state, "age")?.parse().ok()?;
    let values = &blocks
        .by_id(BlockStateId(state))?
        .block
        .properties
        .iter()
        .find(|(key, _)| key == "age")?
        .1;
    Some((age, u32::try_from(values.len()).ok()?.checked_sub(1)?))
}

fn is_block(blocks: &BlockRegistry, state: u32, path: &str) -> bool {
    blocks
        .by_id(BlockStateId(state))
        .is_some_and(|state| state.block.id.path() == path)
}

fn same_block(blocks: &BlockRegistry, a: u32, b: u32) -> bool {
    let raw = |id| {
        blocks
            .by_id(BlockStateId(id))
            .map(|state| state.block.raw_id)
    };
    raw(a).is_some() && raw(a) == raw(b)
}

/// SplitMix64 finaliser.
fn mix(z: u64) -> u64 {
    let z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub(crate) fn hash(parts: &[u64]) -> u64 {
    parts.iter().fold(0, |acc, part| mix(acc ^ part))
}

/// A deterministic roll stream.
pub(crate) struct Roll(u64);

impl Roll {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Uniform in `0..bound`.
    pub(crate) fn below(&mut self, bound: u32) -> u32 {
        self.0 = mix(self.0);
        (((self.0 >> 32) * u64::from(bound)) >> 32) as u32
    }
}

#[cfg(test)]
#[path = "farming_cache_tests.rs"]
mod cache_tests;

#[cfg(test)]
#[path = "farming_environment_tests.rs"]
mod environment_tests;
