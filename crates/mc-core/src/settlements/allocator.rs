//! Event-driven home/job-site/meeting-point allocator: the sole POI authority.
//! Owns the tick-owner [`PoiCache`]; the brain keeps only motion, schedules,
//! gossip, breeding and defense. Runs per dimension immediately before the
//! brain with its own telemetry phase. Shares the global tick budget with the
//! brain (`crate::tick_budget`): one [`Kind::Decision`] per allocator search
//! or full brain admission, one [`Kind::PathNode`] per A* expansion. Never loads
//! chunks; changed claims validate against live blocks with next-pass retry.
//! Sensor projection is resident-neighbor work; committed POI rescans follow
//! pushed chunk revisions after one bootstrap pass.
use super::SettlementId;
use crate::boundary::{BlockPos, ChangeSet, ChunkPos, EntityId};
use crate::entities::EntitySlot;
use crate::registry::CoreRegistry;
use crate::tick::TickState;
use crate::tick_budget::{Consumer, Kind};
use crate::villager::{VillagerProfession as Profession, VillagerState};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, VecDeque};

// `distance` returns squared blocks. Keep workstation use/profession reach
// wider than the pathfinder's adjacent-cell endpoint (including cell offsets).
pub(crate) const JOB_SITE_REACH_SQUARED: f64 = 3.0 * 3.0;
/// Rises above the departure floor at which a living-entity jump (0.42
/// blocks/tick decaying under gravity; ticks above one block: 1.157, 1.234,
/// 1.229, 1.144) can cross a ledge edge: the free-flight height first, then
/// the heights a low departure ceiling caps the rise to (just over one block).
const JUMP_CROSSING_RISES: [f64; 3] = [1.16, 1.10, 1.05];
pub(crate) const PICKUP_REACH: f64 = 1.425;
pub(crate) const POI_REFRESH_CHUNKS_PER_TICK: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PoiKind {
    Home,
    Job(Profession),
    Bell,
}
#[derive(Clone, Copy)]
pub(crate) struct Poi {
    pub(crate) pos: BlockPos,
    pub(crate) kind: PoiKind,
}
#[derive(Clone, Copy)]
pub(crate) struct Node {
    pos: BlockPos,
    parent: usize,
    cost: u32,
    expanded: bool,
}
pub(crate) struct ChunkPois {
    pub(crate) revision: u64,
    pub(crate) queued: bool,
    pub(crate) pois: Vec<Poi>,
}

/// Transient retry position, kept only while a resident has missing POIs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SearchProgress {
    kind: usize,
    candidates: [usize; 3],
}

/// Shared tick-owner POI, sensor and brain-admission scratch. Route/node
/// ownership stays here; brain scheduling methods live with the brain.
#[derive(Default)]
pub(crate) struct PoiCache {
    pub(crate) kinds: Vec<Option<PoiKind>>,
    pub(crate) presentation: super::presentation::Cache,
    pub(crate) food: crate::villager::brain::FoodTable,
    pub(crate) chunks: FxHashMap<ChunkPos, ChunkPois>,
    pub(crate) pending_chunks: VecDeque<ChunkPos>,
    pub(crate) poi_initialized: bool,
    pub(crate) near_sensor_chunks: FxHashSet<ChunkPos>,
    pub(crate) homes: FxHashMap<BlockPos, std::collections::BTreeSet<EntityId>>,
    pub(crate) jobs: FxHashMap<BlockPos, std::collections::BTreeSet<EntityId>>,
    pub(crate) occupied_beds: FxHashSet<BlockPos>,
    pub(crate) previous_occupied_beds: FxHashSet<BlockPos>,
    pub(crate) newborn_homes: Vec<(EntityId, BlockPos)>,
    pub(crate) resident_threats: std::collections::BTreeSet<EntityId>,
    pub(crate) search_at: FxHashMap<EntityId, u64>,
    pub(crate) search_due: BTreeSet<(u64, EntityId)>,
    pub(crate) search_needs: FxHashMap<EntityId, (u64, usize, SearchProgress)>,
    pub(crate) search_ready: BTreeSet<(u64, EntityId)>,
    pub(crate) search_ready_order: FxHashMap<EntityId, u64>,
    pub(crate) next_search_order: u64,
    pub(crate) ready_demand: usize,
    pub(crate) search_deferred: bool,
    pub(crate) threats: Vec<([f64; 3], f64)>,
    pub(crate) threat_chunks: FxHashMap<(i32, i32), Vec<usize>>,
    pub(crate) threat_max_range: f64,
    pub(crate) items: FxHashMap<(i32, i32), Vec<EntitySlot>>,
    pub(crate) nearby_items: Vec<EntitySlot>,
    /// Reused scratch for the exact resident chunks visible to defense sensors.
    pub(crate) defense_chunks: FxHashSet<ChunkPos>,
    /// Reused type-lane merge for village defense targeting.
    pub(crate) defense_slots: Vec<EntitySlot>,
    pub(crate) merchants: Vec<EntityId>,
    pub(crate) golems: Vec<EntitySlot>,
    pub(crate) candidates: Vec<(f64, Poi)>,
    pub(crate) candidate_path: Vec<BlockPos>,
    pub(crate) nodes: Vec<Node>,
    pub(crate) visited: FxHashMap<BlockPos, usize>,
    pub(crate) standing_cache: FxHashMap<BlockPos, Option<[f64; 3]>>,
    #[cfg(test)]
    pub(crate) disable_standing_memo: bool,
    pub(crate) open: BinaryHeap<Reverse<(u32, u32, u32, usize)>>,
    pub(crate) brain_bands: [Vec<u64>; 8],
    pub(crate) brain_due: BinaryHeap<Reverse<(u64, EntityId, u64)>>,
    pub(crate) profile_updates: Vec<EntityId>,
    pub(crate) rebuild_schedule: bool,
    pub(crate) near_residents: usize,
    pub(crate) execution_bits: Vec<u64>,
    pub(crate) execution_indices: Vec<usize>,
    pub(crate) next_player_chunks: Vec<(i32, i32)>,
    pub(crate) previous_merchants: Vec<EntityId>,
    pub(crate) previous_urgent_chunks: FxHashSet<(i32, i32)>,
    pub(crate) schedules: Option<[crate::villager::Activity; 3]>,
    pub(crate) admission_order: Vec<usize>,
    pub(crate) brain_cursors: [Option<EntityId>; 8],
    pub(crate) residents: Vec<EntitySlot>,
    pub(crate) resident_data: Vec<crate::villager::brain::ResidentBrain>,
    pub(crate) previous_data: Vec<crate::villager::brain::ResidentBrain>,
    pub(crate) resident_index: FxHashMap<EntityId, usize>,
    pub(crate) previous_index: FxHashMap<EntityId, usize>,
    pub(crate) resident_revision: Option<u64>,
    pub(crate) active_chunks: FxHashSet<ChunkPos>,
    pub(crate) revised_chunks: FxHashSet<ChunkPos>,
    pub(crate) claim_chunks: FxHashMap<ChunkPos, FxHashSet<EntityId>>,
    pub(crate) dirty_claims: FxHashSet<EntityId>,
    pub(crate) validation_queue: VecDeque<EntityId>,
    pub(crate) validation_cursor: usize,
    pub(crate) allocator_updates: Vec<EntityId>,
    pub(crate) loaded_chunks: FxHashSet<ChunkPos>,
    pub(crate) changed: FxHashSet<EntityId>,
    pub(crate) updates: Vec<EntityId>,
    pub(crate) motion_chunks: FxHashSet<ChunkPos>,
    pub(crate) selected: Vec<EntitySlot>,
    pub(crate) motion_revision: Option<u64>,
    pub(crate) full_brains: Vec<bool>,
    pub(crate) removed_items: Vec<EntitySlot>,
    pub(crate) nearby_residents: Vec<EntitySlot>,
    pub(crate) player_chunks: Vec<(i32, i32)>,
    pub(crate) player_distance: FxHashMap<(i32, i32), i32>,
    pub(crate) urgent_chunks: FxHashSet<(i32, i32)>,
    #[cfg(test)]
    pub(crate) brain_us: u128,
    #[cfg(test)]
    pub(crate) roster_us: u128,
    #[cfg(test)]
    pub(crate) roster_rebuilds: usize,
    #[cfg(test)]
    pub(crate) phase_us: [u128; 7],
    #[cfg(test)]
    pub(crate) measure_decisions: bool,
    #[cfg(test)]
    pub(crate) measure_actor: Option<EntityId>,
    #[cfg(test)]
    pub(crate) decision_current: [u128; 8],
    #[cfg(test)]
    pub(crate) decision_phases: [Vec<u128>; 8],
    #[cfg(test)]
    pub(crate) decision_calls: [usize; 8],
    #[cfg(test)]
    pub(crate) decision_ns: Vec<u128>,
    #[cfg(test)]
    pub(crate) path_denied: FxHashSet<EntityId>,
}

pub(crate) fn poi_kind(registry: &CoreRegistry, id: u32) -> Option<PoiKind> {
    let block = registry.blocks.by_id(mc_world::BlockStateId(id))?;
    let name = block.block.id.path();
    if name.ends_with("_bed") {
        return block
            .properties
            .iter()
            .any(|(k, v)| k == "part" && v == "head")
            .then_some(PoiKind::Home);
    }
    Some(match name {
        "bell" => PoiKind::Bell,
        "blast_furnace" => PoiKind::Job(Profession::Armorer),
        "smoker" => PoiKind::Job(Profession::Butcher),
        "cartography_table" => PoiKind::Job(Profession::Cartographer),
        "brewing_stand" => PoiKind::Job(Profession::Cleric),
        "composter" => PoiKind::Job(Profession::Farmer),
        "barrel" => PoiKind::Job(Profession::Fisherman),
        "fletching_table" => PoiKind::Job(Profession::Fletcher),
        "cauldron" | "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => {
            PoiKind::Job(Profession::Leatherworker)
        }
        "lectern" => PoiKind::Job(Profession::Librarian),
        "stonecutter" => PoiKind::Job(Profession::Mason),
        "loom" => PoiKind::Job(Profession::Shepherd),
        "smithing_table" => PoiKind::Job(Profession::Toolsmith),
        "grindstone" => PoiKind::Job(Profession::Weaponsmith),
        _ => return None,
    })
}

impl PoiCache {
    pub(crate) fn refresh_sensor_chunks(&mut self, state: &TickState) {
        self.near_sensor_chunks.clear();
        // Threats reach at most 15 blocks, pickup reaches 1.425 and golem
        // detection reaches 16: neighboring chunks cover every query.
        for chunk in state.entities.villager_chunks().keys() {
            for x in chunk.x - 1..=chunk.x + 1 {
                for z in chunk.z - 1..=chunk.z + 1 {
                    self.near_sensor_chunks.insert(ChunkPos { x, z });
                }
            }
        }
    }

    fn queue_poi_revision(&mut self, pos: ChunkPos, revision: u64) {
        let entry = self.chunks.entry(pos).or_insert_with(|| ChunkPois {
            revision: u64::MAX,
            queued: false,
            pois: Vec::new(),
        });
        if entry.revision != revision && !entry.queued {
            // Never offer a stale workstation while its bounded rescan is
            // pending. Existing claims still validate against live blocks.
            entry.pois.clear();
            entry.queued = true;
            self.pending_chunks.push_back(pos);
        }
    }

    pub(crate) fn refresh_poi_revisions(&mut self, state: &TickState) {
        if !self.poi_initialized {
            for chunk in state.world.chunks() {
                self.queue_poi_revision(
                    chunk.pos,
                    state.world.chunk_revision(chunk.pos.x, chunk.pos.z),
                );
            }
            self.poi_initialized = true;
        }
        let mut unloaded = false;
        let revised = std::mem::take(&mut self.revised_chunks);
        for &pos in &revised {
            if state.world.has_chunk(pos.x, pos.z) {
                let revision = state.world.chunk_revision(pos.x, pos.z);
                self.queue_poi_revision(pos, revision);
            } else {
                unloaded |= self.chunks.remove(&pos).is_some();
            }
        }
        self.revised_chunks = revised;
        if unloaded {
            self.pending_chunks
                .retain(|pos| self.chunks.contains_key(pos));
        }
    }

    pub(crate) fn dirty_claim(&mut self, id: EntityId) {
        if self.dirty_claims.insert(id) {
            self.validation_queue.push_back(id);
        }
    }

    fn forget_search(&mut self, id: EntityId) {
        if let Some((due, missing, _)) = self.search_needs.remove(&id) {
            self.search_due.remove(&(due, id));
            if let Some(order) = self.search_ready_order.remove(&id) {
                self.search_ready.remove(&(order, id));
                self.ready_demand -= missing;
            }
        }
    }

    fn sync_search(&mut self, state: &TickState, slot: EntitySlot) {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let id = body.id;
        let missing = if active(state, slot) {
            let v = row.villager.as_ref().unwrap();
            usize::from(v.home.is_none())
                + usize::from(v.meeting_point.is_none())
                + usize::from(
                    v.age >= 0
                        && !matches!(v.data.profession, Profession::Nitwit | Profession::Soldier)
                        && v.job_site.is_none(),
                )
        } else {
            0
        };
        let due = self.search_at.get(&id).copied().unwrap_or(state.tick);
        if missing == 0 {
            self.forget_search(id);
        } else if self.search_needs.get(&id).map(|entry| (entry.0, entry.1)) != Some((due, missing))
        {
            let progress = self
                .search_needs
                .get(&id)
                .map(|entry| entry.2)
                .unwrap_or_default();
            self.forget_search(id);
            self.search_needs.insert(id, (due, missing, progress));
            self.search_due.insert((due, id));
        }
    }

    fn promote_searches(&mut self, tick: u64) {
        while let Some(&(due, id)) = self.search_due.first() {
            if due > tick {
                break;
            }
            self.search_due.pop_first();
            self.ready_search(id);
        }
    }

    fn ready_search(&mut self, id: EntityId) {
        if self.search_ready_order.contains_key(&id) {
            return;
        }
        let order = self.next_search_order;
        self.next_search_order += 1;
        self.search_ready_order.insert(id, order);
        self.search_ready.insert((order, id));
        self.ready_demand += self.search_needs[&id].1;
    }

    pub(crate) fn update_claim_chunks(
        &mut self,
        owner: EntityId,
        old: [Option<BlockPos>; 3],
        new: [Option<BlockPos>; 3],
    ) {
        let before = claim_chunks(old);
        let after = claim_chunks(new);
        for (index, chunk) in before.into_iter().enumerate() {
            if let Some(chunk) = chunk
                && !before[..index].contains(&Some(chunk))
                && !after.contains(&Some(chunk))
                && let Some(owners) = self.claim_chunks.get_mut(&chunk)
            {
                owners.remove(&owner);
                if owners.is_empty() {
                    self.claim_chunks.remove(&chunk);
                }
            }
        }
        for (index, chunk) in after.into_iter().enumerate() {
            if let Some(chunk) = chunk
                && !after[..index].contains(&Some(chunk))
                && !before.contains(&Some(chunk))
            {
                self.claim_chunks.entry(chunk).or_default().insert(owner);
            }
        }
    }

    /// Shared-budget path admission: one decision per search, one node per
    /// A* expansion. A denied villager retries on its original deadline.
    pub(crate) fn spend_path(&mut self, state: &mut TickState, consumer: Consumer) -> bool {
        let admitted = RouteCtx::tick(state, consumer).admit();
        #[cfg(test)]
        if !admitted {
            self.note_path_denial();
        }
        admitted
    }
}

/// Beds can straddle a boundary. Subscribe to the possible adjacent foot
/// columns even while the head is unloaded (and its facing is unavailable).
fn claim_chunks(claims: [Option<BlockPos>; 3]) -> [Option<ChunkPos>; 5] {
    let mut chunks = [None; 5];
    for (chunk, pos) in chunks.iter_mut().zip(claims) {
        *chunk = pos.map(|pos| ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        });
    }
    if let Some(home) = claims[0] {
        let chunk = chunks[0].unwrap();
        chunks[3] = match home.x.rem_euclid(16) {
            0 => Some(ChunkPos {
                x: chunk.x - 1,
                ..chunk
            }),
            15 => Some(ChunkPos {
                x: chunk.x + 1,
                ..chunk
            }),
            _ => None,
        };
        chunks[4] = match home.z.rem_euclid(16) {
            0 => Some(ChunkPos {
                z: chunk.z - 1,
                ..chunk
            }),
            15 => Some(ChunkPos {
                z: chunk.z + 1,
                ..chunk
            }),
            _ => None,
        };
    }
    chunks
}

pub(crate) fn at(pos: [f64; 3]) -> BlockPos {
    BlockPos {
        x: pos[0].floor() as i32,
        y: pos[1].ceil() as i32,
        z: pos[2].floor() as i32,
    }
}
pub(crate) fn centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}
pub(crate) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}
pub(crate) fn active(state: &TickState, slot: EntitySlot) -> bool {
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    body.alive
        && !row.controls.no_ai
        && row
            .villager
            .as_ref()
            .is_some_and(|v| !v.wandering && v.conversion_time.is_none())
        && {
            let pos = state.entities.pos(slot);
            state.world.is_active(
                (pos[0].floor() as i32).div_euclid(16),
                (pos[2].floor() as i32).div_euclid(16),
            )
        }
}
pub(crate) fn bed_pair(
    world: &crate::world::ChunkWorld,
    registry: &std::sync::Arc<CoreRegistry>,
    pos: BlockPos,
) -> Option<BlockPos> {
    let id = world.block_at(pos.x, pos.y, pos.z)?;
    let head = registry.blocks.by_id(mc_world::BlockStateId(id))?;
    if poi_kind(registry, id) != Some(PoiKind::Home) {
        return None;
    }
    let (foot, _) = crate::placement::other_half(&registry.blocks, pos, id)?;
    let other = world.block_at(foot.x, foot.y, foot.z)?;
    let other = registry.blocks.by_id(mc_world::BlockStateId(other))?;
    let facing = head.properties.iter().find(|(key, _)| key == "facing");
    (other.block.id == head.block.id
        && other
            .properties
            .iter()
            .any(|(key, value)| key == "part" && value == "foot")
        && other.properties.iter().find(|(key, _)| key == "facing") == facing)
        .then_some(foot)
}
pub(crate) fn bed_has_baby_space(state: &TickState, pos: BlockPos) -> bool {
    bed_pair(&state.world, &state.registry, pos).is_some_and(|foot| {
        [pos, foot].into_iter().all(|p| {
            clear_body(
                &state.world,
                &state.registry,
                [
                    f64::from(p.x) + 0.5,
                    f64::from(p.y) + 1.0,
                    f64::from(p.z) + 0.5,
                ],
                0.3,
                2.0,
            )
        })
    })
}

/// Lifecycle invalidation: conversion clears every POI memory (home, job,
/// meeting) plus courtship state through the single POI authority.
/// Profession-takeover gate at an owned job site: the allocator confirms the
/// live block still offers the workstation and no other villager claims it.
/// Keeps the single-authority rule: the brain reads, the allocator confirms.
pub(crate) fn confirm_profession(
    state: &TickState,
    cache: &PoiCache,
    owner: EntityId,
    settlement: Option<super::SettlementId>,
    job: BlockPos,
    profession: Profession,
) -> bool {
    let natural = natural_job(&state.settlements, settlement, job) == Some(profession);
    (natural
        || state
            .world
            .block_at(job.x, job.y, job.z)
            .is_some_and(|block| job_matches(&state.registry, profession, block)))
        && !cache.claimed(
            owner,
            Poi {
                pos: job,
                kind: PoiKind::Job(profession),
            },
        )
}

fn job_matches(registry: &CoreRegistry, profession: Profession, block: u32) -> bool {
    let kind = match profession {
        Profession::Miner => super::buildings::BuildingKind::Mine,
        Profession::Lumberjack => super::buildings::BuildingKind::LumberCamp,
        _ => super::buildings::BuildingKind::Workshop(profession),
    };
    super::buildings::workstation_matches(registry, kind, block)
}

fn natural_job(
    registry: &super::Registry,
    settlement: Option<super::SettlementId>,
    pos: BlockPos,
) -> Option<Profession> {
    registry
        .get(settlement?)?
        .buildings
        .values()
        .find_map(|building| {
            (building.natural_zone
                && matches!(
                    building.kind,
                    super::buildings::BuildingKind::Mine
                        | super::buildings::BuildingKind::LumberCamp
                )
                && building.work_points.contains(&[pos.x, pos.y, pos.z]))
            .then(|| building.kind.profession())
            .flatten()
        })
}

pub(crate) fn clear_pois(v: &mut VillagerState) {
    v.home = None;
    v.job_site = None;
    v.meeting_point = None;
}

/// Household relocation releases every old claim through the same authority
/// that admits destination POIs. The changed columns wake normal acquisition.
pub(crate) fn release_members(state: &mut TickState, members: &[EntityId]) {
    for &id in members {
        let slot = state
            .entities
            .slot_of(id)
            .expect("validated household member");
        let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
        let old = [v.home, v.job_site, v.meeting_point];
        clear_pois(v);
        v.sleeping = false;
        v.partner = None;
        v.courtship_until = None;
        v.courtship_started_tick = None;
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_chunks.clear();
        v.navigation_repath_at = 0;
        v.poi_validation = PoiValidation::default();
        state.villager_pois.update_claims(id, old, [None; 3]);
        state.villager_pois.search_at.insert(id, state.tick);
    }
}

/// Disjoint tick borrows for route search: world/registry reads plus the one
/// global budget, with the dimension and consumer whose allowance is charged.
pub(crate) struct RouteCtx<'a> {
    pub(crate) world: &'a crate::world::ChunkWorld,
    pub(crate) registry: &'a std::sync::Arc<CoreRegistry>,
    pub(crate) dimension: crate::dimensions::Dimension,
    pub(crate) consumer: Consumer,
    pub(crate) budget: &'a mut crate::tick_budget::Budget,
}

impl<'a> RouteCtx<'a> {
    pub(crate) fn tick(state: &'a mut TickState, consumer: Consumer) -> Self {
        Self {
            world: &state.world,
            registry: &state.registry,
            dimension: state.dimension,
            consumer,
            budget: &mut state.tick_budget,
        }
    }

    pub(crate) fn admit(&mut self) -> bool {
        self.budget
            .note_demand(self.dimension, self.consumer, Kind::PathNode, 1);
        self.budget
            .allowance(self.dimension, self.consumer, Kind::PathNode)
            > 0
            // Full brain admission already paid its decision before processing.
            && (self.consumer == Consumer::Brain
                || self
                    .budget
                    .try_charge(self.dimension, self.consumer, Kind::Decision, 1))
    }
}

/// Exact shape overlap, not the material classification: stairs/slabs/fences
/// retain their real collision geometry and unloaded blocks remain obstacles.
/// Spawn/placement clearance: any fluid in the body or its support refuses.
pub(crate) fn clear_body(
    world: &crate::world::ChunkWorld,
    registry: &std::sync::Arc<CoreRegistry>,
    pos: [f64; 3],
    width: f64,
    height: f64,
) -> bool {
    body_clear(world, registry, pos, width, height, false, false)
}

/// Walking clearance for routes and motion. Like vanilla ground navigation a
/// body may wade through water that covers only its feet (a shallow flow or a
/// one-block ford, e.g. spring water spilling over village streets and
/// bridges); water reaching the head cell and all lava still refuse.
pub(crate) fn clear_body_for_route(
    world: &crate::world::ChunkWorld,
    registry: &std::sync::Arc<CoreRegistry>,
    pos: [f64; 3],
    width: f64,
    height: f64,
    can_open_doors: bool,
) -> bool {
    body_clear(world, registry, pos, width, height, can_open_doors, true)
}

fn body_clear(
    world: &crate::world::ChunkWorld,
    registry: &std::sync::Arc<CoreRegistry>,
    pos: [f64; 3],
    width: f64,
    height: f64,
    can_open_doors: bool,
    wade: bool,
) -> bool {
    let min = [pos[0] - width, pos[1] + 1e-6, pos[2] - width];
    let max = [pos[0] + width, pos[1] + height - 1e-6, pos[2] + width];
    let foot = min[1].floor() as i32;
    for x in min[0].floor() as i32..=max[0].floor() as i32 {
        for y in (foot - 1)..=max[1].floor() as i32 {
            for z in min[2].floor() as i32..=max[2].floor() as i32 {
                let Some(id) = world.block_at(x, y, z) else {
                    return false;
                };
                // Fractional-height floors put water in the sampled row below
                // the feet; it cannot obstruct a walking body above the floor.
                match registry.materials.classify(id) {
                    mc_physics::BlockMaterial::Lava => return false,
                    mc_physics::BlockMaterial::Water if !wade || y > foot => return false,
                    _ => {}
                }
                let Some(boxes) = registry.collision_boxes.get(id as usize) else {
                    return false;
                };
                for shape in boxes.iter() {
                    let b = shape.as_blocks();
                    if min[0] < f64::from(x) + b[3] - 1e-6
                        && max[0] > f64::from(x) + b[0] + 1e-6
                        && min[1] < f64::from(y) + b[4] - 1e-6
                        && max[1] > f64::from(y) + b[1] + 1e-6
                        && min[2] < f64::from(z) + b[5] - 1e-6
                        && max[2] > f64::from(z) + b[2] + 1e-6
                        && !(can_open_doors && crate::villager::doors::wooden(registry, id))
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}
pub(crate) fn standing(
    world: &crate::world::ChunkWorld,
    registry: &std::sync::Arc<CoreRegistry>,
    cell: BlockPos,
    width: f64,
    height: f64,
) -> Option<[f64; 3]> {
    standing_for_route(world, registry, cell, width, height, false)
}

pub(crate) fn standing_for_route(
    world: &crate::world::ChunkWorld,
    registry: &std::sync::Arc<CoreRegistry>,
    cell: BlockPos,
    width: f64,
    height: f64,
    can_open_doors: bool,
) -> Option<[f64; 3]> {
    let id = world.block_at(cell.x, cell.y - 1, cell.z)?;
    let shapes = registry.collision_boxes.get(id as usize)?;
    let top = shapes
        .iter()
        .filter_map(|b| {
            let b = b.as_blocks();
            (b[0] <= 0.5 && b[3] >= 0.5 && b[2] <= 0.5 && b[5] >= 0.5 && b[4] <= 1.0)
                .then_some(b[4])
        })
        .max_by(f64::total_cmp)?;
    let pos = [
        f64::from(cell.x) + 0.5,
        f64::from(cell.y - 1) + top,
        f64::from(cell.z) + 0.5,
    ];
    clear_body_for_route(world, registry, pos, width, height, can_open_doors).then_some(pos)
}

/// The body falls straight down the landing column from `from_y` to the
/// landing position `end`; every unit-spaced body position on the way (a body
/// is taller than one block) must be clear.
fn drop_shaft_clear(
    ctx: &RouteCtx<'_>,
    end: [f64; 3],
    from_y: f64,
    width: f64,
    height: f64,
    can_open_doors: bool,
) -> bool {
    let mut y = from_y;
    while y > end[1] + 1e-6 {
        if !clear_body_for_route(
            ctx.world,
            ctx.registry,
            [end[0], y, end[2]],
            width,
            height,
            can_open_doors,
        ) {
            return false;
        }
        y -= 1.0;
    }
    true
}
/// Highest a body may hover above the floor it just left, measured from that
/// floor: a jump rises 1.25 blocks.
const JUMP_APEX: f64 = 1.3;

/// Whether a collision top lies under the body's feet.
fn resting(ctx: &RouteCtx<'_>, body: [f64; 3], width: f64) -> bool {
    let row = (body[1] - 1e-4).floor() as i32;
    for x in (body[0] - width).floor() as i32..=(body[0] + width).floor() as i32 {
        for z in (body[2] - width).floor() as i32..=(body[2] + width).floor() as i32 {
            let boxes = ctx
                .world
                .block_at(x, row, z)
                .and_then(|id| ctx.registry.collision_boxes.get(id as usize));
            for shape in boxes.into_iter().flat_map(|boxes| boxes.iter()) {
                let b = shape.as_blocks();
                if (f64::from(row) + b[4] - body[1]).abs() < 1e-3
                    && body[0] - width < f64::from(x) + b[3] - 1e-6
                    && body[0] + width > f64::from(x) + b[0] + 1e-6
                    && body[2] - width < f64::from(z) + b[5] - 1e-6
                    && body[2] + width > f64::from(z) + b[2] + 1e-6
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Cell a route starts from. A body mid-jump hovers above its floor, so its
/// cell is one or two rows too high: a step the floor cannot make looks like a
/// one-block rise, and a builder jumping at a two-block wall replanned the same
/// impossible route forever. Such a body starts from the floor it left.
fn grounded_start(
    ctx: &RouteCtx<'_>,
    cache: &mut PoiCache,
    body: [f64; 3],
    width: f64,
    height: f64,
    can_open_doors: bool,
) -> BlockPos {
    let cell = at(body);
    if resting(ctx, body, width) {
        return cell;
    }
    (0..=2)
        .map(|drop| BlockPos {
            y: cell.y - drop,
            ..cell
        })
        .find(|&below| {
            standing_memo(ctx, cache, below, width, height, can_open_doors)
                .is_some_and(|floor| floor[1] <= body[1] + 1e-6 && body[1] - floor[1] <= JUMP_APEX)
        })
        .unwrap_or(cell)
}

fn heuristic(a: BlockPos, b: BlockPos) -> u32 {
    (a.x.abs_diff(b.x) + a.z.abs_diff(b.z) + a.y.abs_diff(b.y)).saturating_mul(10)
}

/// Weighted A* keeps a POI route bounded when a nearby target is behind a
/// wall: favour closing the remaining distance over flooding every equally
/// short street on the wrong side. Routes need only be valid, not shortest.
fn search_priority(cost: u32, pos: BlockPos, target: BlockPos) -> (u32, u32) {
    let remaining = heuristic(pos, target);
    (cost.saturating_add(remaining.saturating_mul(8)), remaining)
}
#[cfg(test)]
pub(crate) fn find_path(
    state: &mut TickState,
    cache: &mut PoiCache,
    start: [f64; 3],
    target: BlockPos,
    width: f64,
    height: f64,
    path: &mut Vec<BlockPos>,
) -> bool {
    let mut ctx = RouteCtx::tick(state, Consumer::Allocator);
    find_path_ctx(
        &mut ctx, cache, start, target, width, height, path, false, 48, 16, true,
    )
}

pub(crate) fn find_path_exact(
    state: &mut TickState,
    cache: &mut PoiCache,
    start: [f64; 3],
    target: BlockPos,
    width: f64,
    height: f64,
    path: &mut Vec<BlockPos>,
) -> bool {
    let mut ctx = RouteCtx::tick(state, Consumer::Allocator);
    find_path_exact_ctx(&mut ctx, cache, start, target, width, height, path)
}

pub(crate) fn find_path_exact_ctx(
    ctx: &mut RouteCtx<'_>,
    cache: &mut PoiCache,
    start: [f64; 3],
    target: BlockPos,
    width: f64,
    height: f64,
    path: &mut Vec<BlockPos>,
) -> bool {
    if !ctx.admit() {
        return false;
    }
    let cell = at(start);
    let horizontal = cell
        .x
        .abs_diff(target.x)
        .max(cell.z.abs_diff(target.z))
        .saturating_add(8)
        .clamp(48, 128);
    let vertical = cell.y.abs_diff(target.y).saturating_add(4).clamp(16, 128);
    find_path_ctx(
        ctx, cache, start, target, width, height, path, true, horizontal, vertical, false,
    )
}

/// Tick-owned route search over disjoint world/registry/budget borrows.
#[allow(clippy::too_many_arguments)]
pub(crate) fn find_path_ctx(
    ctx: &mut RouteCtx<'_>,
    cache: &mut PoiCache,
    start: [f64; 3],
    target: BlockPos,
    width: f64,
    height: f64,
    path: &mut Vec<BlockPos>,
    exact: bool,
    horizontal: u32,
    vertical: u32,
    can_open_doors: bool,
) -> bool {
    path.clear();
    cache.nodes.clear();
    cache.visited.clear();
    cache.open.clear();
    cache.standing_cache.clear();
    let body = start;
    let start = grounded_start(ctx, cache, body, width, height, can_open_doors);
    cache.nodes.push(Node {
        pos: start,
        parent: 0,
        cost: 0,
        expanded: false,
    });
    cache.visited.insert(start, 0);
    let (score, remaining) = search_priority(0, start, target);
    cache.open.push(Reverse((score, remaining, 0, 0)));
    let mut closest = 0;
    while let Some(Reverse((_, _, queued_cost, index))) = cache.open.pop() {
        let node = cache.nodes[index];
        // A cheaper predecessor may have superseded this queued entry. Do not
        // spend one of the shared tick nodes expanding it a second time.
        if node.cost != queued_cost {
            continue;
        }
        // An A* node needs expansion at most once. Retaining the first route
        // is sufficient for navigation and prevents terrain alternatives from
        // repeatedly consuming the shared per-tick allowance.
        if cache.nodes[index].expanded {
            continue;
        }
        cache.nodes[index].expanded = true;
        if !ctx
            .budget
            .try_charge(ctx.dimension, ctx.consumer, Kind::PathNode, 1)
        {
            break;
        }
        if heuristic(node.pos, target) < heuristic(cache.nodes[closest].pos, target) {
            closest = index;
        }
        let reached = if exact {
            node.pos == target
        } else {
            node.pos.x.abs_diff(target.x) <= 1
                && node.pos.z.abs_diff(target.z) <= 1
                && node.pos.y.abs_diff(target.y) <= 1
        };
        if reached {
            let mut i = index;
            while i != 0 {
                path.push(cache.nodes[i].pos);
                i = cache.nodes[i].parent;
            }
            return true;
        }
        if cache.nodes.len() >= 8_192 {
            break;
        }
        // The first node is where the body really is: a rim over water or a
        // ledge edge has no standable support below its cell centre.
        let begin = standing_memo(ctx, cache, node.pos, width, height, can_open_doors)
            .unwrap_or_else(|| if index == 0 { body } else { centre(node.pos) });
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            for dy in [0, 1, -1, -2, -3] {
                let next = BlockPos {
                    x: node.pos.x + dx,
                    y: node.pos.y + dy,
                    z: node.pos.z + dz,
                };
                if start.x.abs_diff(next.x) > horizontal
                    || start.z.abs_diff(next.z) > horizontal
                    || start.y.abs_diff(next.y) > vertical
                {
                    continue;
                }
                if cache
                    .visited
                    .get(&next)
                    .is_some_and(|&index| cache.nodes[index].expanded)
                {
                    continue;
                }
                let Some(end) = standing_memo(ctx, cache, next, width, height, can_open_doors)
                else {
                    continue;
                };
                // A jump requires headroom over the starting column; a drop
                // traverses the edge horizontally before falling. A real jump
                // (above step height) clears the ledge only while the body is
                // high, so the crossing needs headroom at the highest height
                // the departure column lets the jump reach.
                let travel_y = begin[1].max(end[1]);
                let crossing_y = if end[1] > begin[1] + mc_physics::STEP_HEIGHT {
                    JUMP_CROSSING_RISES
                        .iter()
                        .map(|rise| begin[1] + rise)
                        .find(|&y| {
                            clear_body_for_route(
                                ctx.world,
                                ctx.registry,
                                [begin[0], y, begin[2]],
                                width,
                                height,
                                can_open_doors,
                            )
                        })
                        .map_or(travel_y, |y| y.max(travel_y))
                } else {
                    travel_y
                };
                if !clear_body_for_route(
                    ctx.world,
                    ctx.registry,
                    [begin[0], travel_y, begin[2]],
                    width,
                    height,
                    can_open_doors,
                ) || !clear_body_for_route(
                    ctx.world,
                    ctx.registry,
                    [
                        (begin[0] + end[0]) * 0.5,
                        crossing_y,
                        (begin[2] + end[2]) * 0.5,
                    ],
                    width,
                    height,
                    can_open_doors,
                ) || (end[1] < begin[1]
                    && !drop_shaft_clear(ctx, end, travel_y, width, height, can_open_doors))
                {
                    continue;
                }
                let cost = node.cost + 10 + dy.unsigned_abs() * 4;
                if let Some(&old) = cache.visited.get(&next) {
                    if cache.nodes[old].cost <= cost {
                        break;
                    }
                    cache.nodes[old].parent = index;
                    cache.nodes[old].cost = cost;
                    let (score, remaining) = search_priority(cost, next, target);
                    cache.open.push(Reverse((score, remaining, cost, old)));
                } else {
                    let i = cache.nodes.len();
                    cache.nodes.push(Node {
                        pos: next,
                        parent: index,
                        cost,
                        expanded: false,
                    });
                    cache.visited.insert(next, i);
                    let (score, remaining) = search_priority(cost, next, target);
                    cache.open.push(Reverse((score, remaining, cost, i)));
                }
                break;
            }
        }
    }
    // Navigation may still flee/stroll toward the closest reachable cell;
    // POI acquisition requires the successful return and never claims it.
    while closest != 0 {
        path.push(cache.nodes[closest].pos);
        closest = cache.nodes[closest].parent;
    }
    false
}
/// Settlement housing scope for POI admission. Bound villagers claim only
/// POIs inside their settlement anchor radius; a bound villager whose record
/// is absent from the local registry (foreign dimension) acquires nothing;
/// unbound villagers keep vanilla 48-block reach. Membership columns are
/// never touched here: a body outside its radius keeps its membership.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HousingScope {
    Unbound,
    Foreign,
    Bound { anchor: [i32; 3], radius: u32 },
}

fn housing_scope(state: &TickState, settlement: Option<SettlementId>) -> HousingScope {
    match settlement {
        None => HousingScope::Unbound,
        Some(id) => match state.settlements.get(id) {
            Some(record) => HousingScope::Bound {
                anchor: record.anchor,
                radius: record.radius,
            },
            None => HousingScope::Foreign,
        },
    }
}

fn scope_allows(scope: HousingScope, pos: BlockPos) -> bool {
    match scope {
        HousingScope::Unbound => true,
        HousingScope::Foreign => false,
        HousingScope::Bound { anchor, radius } => {
            let squared: f64 = [pos.x, pos.y, pos.z]
                .iter()
                .zip(anchor)
                .map(|(&coordinate, anchor)| (f64::from(coordinate) - f64::from(anchor)).powi(2))
                .sum();
            squared <= f64::from(radius).powi(2)
        }
    }
}

fn acquire(
    state: &mut TickState,
    cache: &mut PoiCache,
    slot: EntitySlot,
    v: &mut VillagerState,
    kind: PoiKind,
) -> Option<BlockPos> {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (owner, half_width, height) = (body.id, body.half_width, body.height);
    let scope = housing_scope(state, v.settlement);
    if matches!(scope, HousingScope::Bound { .. }) && matches!(kind, PoiKind::Job(_)) {
        // Only certified slots are settlement jobs. Work-point lists are
        // bounded at certification; never rescan the settlement radius here.
        cache.candidates.clear();
        let mut needs_resources = false;
        if let Some(record) = v.settlement.and_then(|id| state.settlements.get(id)) {
            needs_resources = v.data.profession == Profession::None
                && !record
                    .buildings
                    .values()
                    .any(|building| building.kind == super::buildings::BuildingKind::Storage);
            for building in record.buildings.values() {
                let Some(profession) = building.kind.profession() else {
                    continue;
                };
                if (traded(v)
                    || matches!(
                        v.data.profession,
                        Profession::Farmer
                            | Profession::Shepherd
                            | Profession::Miner
                            | Profession::Lumberjack
                            | Profession::Builder
                            | Profession::Soldier
                    ))
                    && profession != v.data.profession
                {
                    continue;
                }
                for &[x, y, z] in building
                    .work_points
                    .iter()
                    .take(super::buildings::job_slots(building.kind, building.level))
                {
                    let poi = Poi {
                        pos: BlockPos { x, y, z },
                        kind: PoiKind::Job(profession),
                    };
                    if state.world.block_at(x, y, z).is_some_and(|id| {
                        building.natural_zone
                            || super::buildings::workstation_matches(
                                &state.registry,
                                building.kind,
                                id,
                            )
                    }) && scope_allows(scope, poi.pos)
                        && !cache.claimed(owner, poi)
                    {
                        cache.candidates.push((distance(pos, centre(poi.pos)), poi));
                    }
                }
            }
        }
        cache.candidates.sort_unstable_by(|a, b| {
            let resource = |poi: Poi| {
                needs_resources
                    && matches!(
                        poi.kind,
                        PoiKind::Job(Profession::Miner | Profession::Lumberjack)
                    )
            };
            resource(a.1)
                .cmp(&resource(b.1))
                .then_with(|| b.0.total_cmp(&a.0))
                .then_with(|| {
                    (b.1.pos.x, b.1.pos.y, b.1.pos.z).cmp(&(a.1.pos.x, a.1.pos.y, a.1.pos.z))
                })
        });
    } else {
        cache.nearby(pos, owner, |candidate| match (kind, candidate) {
            (PoiKind::Job(_), PoiKind::Job(p)) => !traded(v) || p == v.data.profession,
            _ => candidate == kind,
        });
        cache
            .candidates
            .retain(|(_, poi)| scope_allows(scope, poi.pos));
    }
    let kind_index = match kind {
        PoiKind::Home => 0,
        PoiKind::Job(_) => 1,
        PoiKind::Bell => 2,
    };
    let cursor = cache.search_needs[&owner].2.candidates[kind_index];
    // A blocked workstation must not trigger unbounded repeated A* searches.
    for attempt in 0..cache.candidates.len().min(5) {
        let offset = cursor.wrapping_add(attempt) % cache.candidates.len();
        let (_, poi) = cache.candidates[cache.candidates.len() - 1 - offset];
        if !cache.spend_path(state, Consumer::Allocator) {
            cache.search_deferred = true;
            break;
        }
        cache.search_needs.get_mut(&owner).unwrap().2.candidates[kind_index] =
            cursor.wrapping_add(attempt + 1);
        let mut path = std::mem::take(&mut cache.candidate_path);
        // Disjoint field borrows: route search reads world/registry while
        // charging the shared budget pool. The search window follows the
        // settlement claim, not the flat 48-block stroll window.
        let tick_now = state.tick;
        let from = at(pos);
        let horizontal = from
            .x
            .abs_diff(poi.pos.x)
            .max(from.z.abs_diff(poi.pos.z))
            .saturating_add(8)
            .clamp(48, 512);
        let vertical = from.y.abs_diff(poi.pos.y).saturating_add(4).clamp(16, 512);
        let mut ctx = RouteCtx {
            world: &state.world,
            registry: &state.registry,
            dimension: state.dimension,
            consumer: Consumer::Allocator,
            budget: &mut state.tick_budget,
        };
        let reached = find_path_ctx(
            &mut ctx, cache, pos, poi.pos, half_width, height, &mut path, false, horizontal,
            vertical, true,
        );
        let nodes_empty = ctx
            .budget
            .allowance(ctx.dimension, ctx.consumer, Kind::PathNode)
            == 0;
        if reached {
            std::mem::swap(&mut path, &mut v.navigation_path);
        } else if nodes_empty {
            cache.search_deferred = true;
        }
        cache.candidate_path = path;
        if reached {
            cache.search_needs.get_mut(&owner).unwrap().2.candidates[kind_index] = 0;
            v.navigation_target = Some(poi.pos);
            capture_route(state, v, pos, true);
            v.navigation_repath_at = tick_now + 40;
            return Some(poi.pos);
        }
    }
    None
}

/// Rotating acquisition admission over the shared global pool: movement and
/// birth routes charge the same allowance, and a budget-denied villager keeps
/// its original retry deadline.
fn acquire_pois(state: &mut TickState, cache: &mut PoiCache) {
    cache.promote_searches(state.tick);
    let mut searched = 0;
    while let Some((_, id)) = cache.search_ready.pop_first() {
        cache.search_ready_order.remove(&id);
        cache.ready_demand -= cache.search_needs[&id].1;
        let Some(slot) = state.entities.slot_of(id) else {
            cache.forget_search(id);
            continue;
        };
        cache.sync_search(state, slot);
        if !cache.search_needs.contains_key(&id) || !active(state, slot) {
            continue;
        }
        let mut v = state
            .entities
            .row_mut_unmarked(slot)
            .villager
            .take()
            .unwrap();
        let before = [v.home, v.job_site, v.meeting_point];
        let old = before;
        cache.search_deferred = false;
        let progress = &mut cache.search_needs.get_mut(&id).unwrap().2;
        let first = progress.kind;
        progress.kind = (first + 1) % 3;
        // A blocked home must not consume the next tick's entire budget again
        // before an otherwise reachable job or meeting point gets a turn.
        for offset in 0..3 {
            match (first + offset) % 3 {
                0 if v.home.is_none() => {
                    v.home = acquire(state, cache, slot, &mut v, PoiKind::Home);
                    if v.home.is_some() {
                        v.home_dimension = state.dimension.name().to_owned();
                        if let (Some(settlement), Some(household), Some(home)) =
                            (v.settlement, v.household.as_deref(), v.home)
                            && state
                                .settlements
                                .get(settlement)
                                .and_then(|record| record.households.get(household))
                                .is_some_and(|family| family.home.is_none())
                        {
                            let record = state.settlements.get_mut(settlement).unwrap();
                            record.revision = record.revision.saturating_add(1);
                            let family = record.households.get_mut(household).unwrap();
                            family.home = Some([home.x, home.y, home.z]);
                            family.revision = family.revision.saturating_add(1);
                        }
                    }
                }
                1 if v.age >= 0
                    && (2_000..9_000).contains(&state.day_time.rem_euclid(24_000))
                    && !matches!(v.data.profession, Profession::Nitwit | Profession::Soldier)
                    && v.job_site.is_none() =>
                {
                    let profession = v.data.profession;
                    v.job_site = acquire(state, cache, slot, &mut v, PoiKind::Job(profession));
                    if v.job_site.is_some() {
                        v.job_site_dimension = state.dimension.name().to_owned();
                    }
                }
                2 if v.meeting_point.is_none() => {
                    v.meeting_point = acquire(state, cache, slot, &mut v, PoiKind::Bell);
                    if v.meeting_point.is_some() {
                        v.meeting_point_dimension = state.dimension.name().to_owned();
                    }
                }
                _ => {}
            }
            if cache.search_deferred {
                break;
            }
        }
        cache.update_claims(id, old, [v.home, v.job_site, v.meeting_point]);
        if before != [v.home, v.job_site, v.meeting_point] {
            state.entities.row_mut(slot).villager = Some(v);
            cache.allocator_updates.push(id);
        } else {
            state.entities.row_mut_unmarked(slot).villager = Some(v);
        }
        if !cache.search_deferred {
            cache.search_at.insert(
                id,
                state
                    .tick
                    .saturating_add(200 + id.0.unsigned_abs() as u64 % 40),
            );
            cache.allocator_updates.push(id);
        }
        cache.sync_search(state, slot);
        if cache.search_deferred && cache.search_needs.contains_key(&id) {
            cache.promote_searches(state.tick);
            cache.ready_search(id);
        }
        searched += 1;
        if searched == 64 || cache.search_deferred {
            break;
        }
    }
}

pub(crate) fn threat_range(name: &str) -> Option<f64> {
    Some(match name {
        "zombie" | "zombie_villager" | "husk" | "drowned" | "vex" => 8.0,
        "vindicator" | "zoglin" => 10.0,
        "evoker" | "illusioner" | "ravager" => 12.0,
        "pillager" => 15.0,
        _ => return None,
    })
}

/// Whether the shared node allowance is exhausted (route-refusal retry later).
pub(crate) fn nodes_exhausted(state: &TickState) -> bool {
    state
        .tick_budget
        .allowance(state.dimension, Consumer::Brain, Kind::PathNode)
        == 0
}

pub(crate) fn free_bed(
    state: &mut TickState,
    cache: &mut PoiCache,
    slot: EntitySlot,
    path: &mut Vec<BlockPos>,
) -> Option<BlockPos> {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let (owner, half_width, height, claimed_home, scope) = (
        body.id,
        body.half_width,
        body.height,
        row.villager.as_ref().and_then(|v| v.home),
        housing_scope(state, row.villager.as_ref().and_then(|v| v.settlement)),
    );
    cache.nearby(pos, owner, |kind| kind == PoiKind::Home);
    cache
        .candidates
        .retain(|(_, poi)| scope_allows(scope, poi.pos));
    for _ in 0..5 {
        let (_, poi) = cache.candidates.pop()?;
        if !bed_has_baby_space(state, poi.pos) || claimed_home == Some(poi.pos) {
            continue;
        }
        if !cache.spend_path(state, Consumer::Brain) {
            cache.search_deferred = true;
            break;
        }
        let mut ctx = RouteCtx {
            world: &state.world,
            registry: &state.registry,
            dimension: state.dimension,
            consumer: Consumer::Brain,
            budget: &mut state.tick_budget,
        };
        if find_path_ctx(
            &mut ctx, cache, pos, poi.pos, half_width, height, path, false, 48, 16, true,
        ) {
            return Some(poi.pos);
        }
        if ctx
            .budget
            .allowance(ctx.dimension, ctx.consumer, Kind::PathNode)
            == 0
        {
            cache.search_deferred = true;
            break;
        }
    }
    None
}

/// One refresh feeds allocation and the following brain phase. Runtime indexes
/// hydrate from all residents once, then follow row/chunk/owner changes.
pub(crate) fn tick(state: &mut TickState, _changes: &mut ChangeSet) -> u64 {
    let begin = std::time::Instant::now();
    let mut cache = std::mem::take(&mut state.villager_pois);
    cache.changed.clear();
    state.entities.swap_brain_changes(&mut cache.changed);
    state.world.swap_poi_revisions(&mut cache.revised_chunks);
    cache.refresh(state);
    super::refresh_counts(state, cache.updates.iter().copied());
    let demand = validate_changed_claims(state, &mut cache);
    state
        .tick_budget
        .note_demand(state.dimension, Consumer::Allocator, Kind::Decision, demand);
    acquire_pois(state, &mut cache);
    // Acquisition can change the winning owner; settle releases before brains
    // consume this same tick's claim index.
    validate_changed_claims(state, &mut cache);
    cache.refresh_allocator_updates(state);
    state.villager_pois = cache;
    state
        .tick_budget
        .finish(state.dimension, Consumer::Allocator);
    super::economy::tick(state);
    begin.elapsed().as_micros() as u64
}

/// Validate the event worklist to a fixed point: releases can dirty other
/// owners of a position. Only actual durable differences take WAL custody.
fn validate_changed_claims(state: &mut TickState, cache: &mut PoiCache) -> usize {
    while let Some(id) = cache.validation_queue.pop_front() {
        cache.dirty_claims.remove(&id);
        let Some(slot) = state.entities.slot_of(id) else {
            cache.forget_search(id);
            continue;
        };
        if !active(state, slot) {
            cache.forget_search(id);
            continue;
        }
        let (old, new, before, after) = {
            let ctx = PoiCtx {
                world: &state.world,
                registry: &state.registry,
                dimension: state.dimension,
                settlements: &state.settlements,
            };
            let v = state
                .entities
                .row_mut_unmarked(slot)
                .villager
                .as_mut()
                .unwrap();
            let before = (
                v.home,
                v.job_site,
                v.meeting_point,
                v.data.profession,
                v.offers.len(),
                v.restocks_today,
            );
            let old = [v.home, v.job_site, v.meeting_point];
            ctx.validate(&*cache, id, v);
            let new = [v.home, v.job_site, v.meeting_point];
            let after = (
                v.home,
                v.job_site,
                v.meeting_point,
                v.data.profession,
                v.offers.len(),
                v.restocks_today,
            );
            (old, new, before, after)
        };
        // Claims are the allocator's sole authority: publish same-tick
        // releases back into the claim index the brain reads.
        if old != new {
            cache.update_claims(id, old, new);
        }
        if before != after {
            state.entities.row_mut(slot);
            cache.allocator_updates.push(id);
        }
        cache.sync_search(state, slot);
    }
    cache.promote_searches(state.tick);
    cache.ready_demand
}

#[cfg(test)]
#[path = "allocator_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "allocator_event_tests.rs"]
mod event_tests;
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ValidatedPoi {
    pos: BlockPos,
    dimension: crate::dimensions::Dimension,
    revision: u64,
    foot: Option<(ChunkPos, u64)>,
    block: Option<u32>,
    bed_valid: bool,
}

/// Geometry only: dimensions, profession and authoritative claim custody are
/// still checked on every full decision. No runtime cache enters a WAL image.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PoiValidation {
    pub(crate) sites: [Option<ValidatedPoi>; 3],
    #[cfg(test)]
    pub(crate) geometry_checks: usize,
}

/// Disjoint read borrows for POI claim validation: world/registry/dimension
/// without the entity rows being validated, so the loop mutates each row in
/// place instead of taking/restoring the whole `VillagerState`.
struct PoiCtx<'a> {
    world: &'a crate::world::ChunkWorld,
    registry: &'a std::sync::Arc<CoreRegistry>,
    dimension: crate::dimensions::Dimension,
    settlements: &'a super::Registry,
}

impl PoiCtx<'_> {
    fn revision_current(&self, chunk: ChunkPos, revision: u64) -> bool {
        // Option<ValidatedPoi> distinguishes an uninitialized cache from revision
        // zero. Unknown chunks never reuse; admission/re-admission changes revision.
        revision != u64::MAX
            && self.world.chunk_revision(chunk.x, chunk.z) == revision
            && !self.world.pending_block_chunks().contains(&chunk)
    }

    fn geometry(
        &self,
        cache: &PoiCache,
        memory: &mut PoiValidation,
        index: usize,
        pos: BlockPos,
    ) -> ValidatedPoi {
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if let Some(known) = memory.sites[index]
            && known.pos == pos
            && known.dimension == self.dimension
            && self.revision_current(chunk, known.revision)
            && known
                .foot
                .is_none_or(|(chunk, revision)| self.revision_current(chunk, revision))
        {
            return known;
        }
        #[cfg(test)]
        {
            memory.geometry_checks += 1;
        }
        let block = self.world.block_at(pos.x, pos.y, pos.z);
        let home = index == 0 && block.is_some_and(|id| cache.kind(id) == Some(PoiKind::Home));
        // Record the actual other half even when its geometry is invalid, so a
        // repaired foot in the neighboring chunk cannot reuse an invalid verdict.
        let foot = home
            .then(|| {
                crate::placement::other_half(&self.registry.blocks, pos, block.unwrap()).map(
                    |(foot, _)| ChunkPos {
                        x: foot.x.div_euclid(16),
                        z: foot.z.div_euclid(16),
                    },
                )
            })
            .flatten()
            .filter(|foot| *foot != chunk)
            .map(|foot| (foot, self.world.chunk_revision(foot.x, foot.z)));
        let known = ValidatedPoi {
            pos,
            dimension: self.dimension,
            revision: self.world.chunk_revision(chunk.x, chunk.z),
            foot,
            block,
            // Partial unload is unknown geometry, just like an absent head.
            // The foot subscription forces a fresh verdict on re-admission.
            bed_valid: home
                && (foot.is_some_and(|(chunk, _)| !self.world.has_chunk(chunk.x, chunk.z))
                    || bed_pair(self.world, self.registry, pos).is_some()),
        };
        memory.sites[index] = Some(known);
        known
    }

    fn validate(&self, cache: &PoiCache, owner: EntityId, v: &mut VillagerState) {
        let dimension = self.dimension.name();
        let foreign_home = v.home.is_some() && v.home_dimension != dimension;
        let foreign_meeting = v.meeting_point.is_some() && v.meeting_point_dimension != dimension;
        let foreign_job = v.job_site.is_some() && v.job_site_dimension != dimension;
        if foreign_home {
            v.home = None;
        }
        if foreign_meeting {
            v.meeting_point = None;
        }
        if foreign_job {
            v.job_site = None;
        }
        // Coordinates from another namespace cannot validate against local blocks,
        // nor leave a path to the old memory in the local navigation cache.
        if foreign_home || foreign_meeting || foreign_job {
            v.navigation_path.clear();
            v.navigation_target = None;
            v.navigation_chunks.clear();
            v.navigation_repath_at = 0;
        }
        if let Some(pos) = v.home {
            let poi = Poi {
                pos,
                kind: PoiKind::Home,
            };
            let geometry = self.geometry(cache, &mut v.poi_validation, 0, pos);
            if !cached_valid_poi(geometry, cache, poi.kind) || cache.claimed(owner, poi) {
                v.home = None;
            }
        }
        if let Some(pos) = v.meeting_point {
            let geometry = self.geometry(cache, &mut v.poi_validation, 1, pos);
            if !cached_valid_poi(geometry, cache, PoiKind::Bell) {
                v.meeting_point = None;
            }
        }
        if let Some(pos) = v.job_site {
            let geometry = self.geometry(cache, &mut v.poi_validation, 2, pos);
            let mut uncertified = false;
            let mut kind = geometry.block.and_then(|id| cache.kind(id));
            if v.settlement.is_some() {
                let certified =
                    super::buildings::job_profession(self.settlements, v.settlement, pos);
                uncertified = certified.is_none();
                kind = match (geometry.block, certified) {
                    (_, Some(expected))
                        if natural_job(self.settlements, v.settlement, pos) == Some(expected) =>
                    {
                        Some(PoiKind::Job(expected))
                    }
                    (Some(block), Some(expected))
                        if job_matches(self.registry, expected, block) =>
                    {
                        Some(PoiKind::Job(expected))
                    }
                    (None, Some(expected)) => Some(PoiKind::Job(expected)),
                    _ => None,
                };
                // An absent certificate is not an unloaded workstation.
                if uncertified {
                    v.job_site = None;
                }
            }
            let valid = match kind {
                Some(PoiKind::Job(p)) => {
                    (v.data.profession != Profession::Builder || p == Profession::Builder)
                        && (!traded(v) || p == v.data.profession)
                        && !cache.claimed(
                            owner,
                            Poi {
                                pos,
                                kind: PoiKind::Job(p),
                            },
                        )
                }
                None => geometry.block.is_none(),
                _ => false,
            };
            if !valid || v.job_site.is_none() {
                v.job_site = None;
                if !uncertified {
                    lose_profession(v);
                }
            }
        } else if v.settlement.is_none() {
            // Only unbound jobless villagers reset here. Bound residents
            // keep professions/offers while waiting for certification.
            lose_profession(v);
        }
        // A cleared/replaced memory must not retain a verdict for later reuse.
        for (known, pos) in
            v.poi_validation
                .sites
                .iter_mut()
                .zip([v.home, v.meeting_point, v.job_site])
        {
            if pos.is_none() {
                *known = None;
            }
        }
    }
}

fn cached_valid_poi(geometry: ValidatedPoi, cache: &PoiCache, expected: PoiKind) -> bool {
    // An evicted chunk is not evidence that a persisted POI was destroyed.
    geometry.block.is_none_or(|id| {
        cache.kind(id) == Some(expected) && (expected != PoiKind::Home || geometry.bed_valid)
    })
}
pub(crate) fn traded(v: &VillagerState) -> bool {
    v.xp > 0 || v.data.level > 1 || v.offers.iter().any(|o| o.uses > 0)
}

#[cfg(test)]
pub(crate) fn validate_pois(
    state: &TickState,
    cache: &PoiCache,
    owner: EntityId,
    v: &mut VillagerState,
) {
    PoiCtx {
        world: &state.world,
        registry: &state.registry,
        dimension: state.dimension,
        settlements: &state.settlements,
    }
    .validate(cache, owner, v);
}

fn lose_profession(v: &mut VillagerState) {
    if !traded(v) && v.data.profession != Profession::Nitwit {
        v.data.profession = Profession::None;
        v.offers.clear();
        v.restocks_today = 0;
    }
}

/// Capture the sole route proof, including support and swept-body chunk halos.
pub(crate) fn capture_route(
    state: &TickState,
    v: &mut VillagerState,
    start: [f64; 3],
    can_open_doors: bool,
) {
    v.navigation_can_open_doors = can_open_doors;
    v.navigation_chunks.clear();
    v.motion_geometry.clear();
    v.navigation_progress_len = v.navigation_path.len();
    v.navigation_progress_next = v.navigation_path.last().copied();
    v.navigation_progress_dist_sq = v
        .navigation_progress_next
        .map_or(0.0, |cell| distance(start, centre(cell)));
    let origin = BlockPos {
        x: start[0].floor() as i32,
        y: start[1].floor() as i32,
        z: start[2].floor() as i32,
    };
    for cell in std::iter::once(origin)
        .chain(v.navigation_path.iter().copied())
        .chain(v.navigation_target)
    {
        for x in cell.x.saturating_sub(1).div_euclid(16)..=cell.x.saturating_add(1).div_euclid(16) {
            for z in
                cell.z.saturating_sub(1).div_euclid(16)..=cell.z.saturating_add(1).div_euclid(16)
            {
                let chunk = ChunkPos { x, z };
                if !v.navigation_chunks.iter().any(|&(pos, _)| pos == chunk) {
                    let revision = if can_open_doors {
                        state.world.villager_route_revision(x, z)
                    } else {
                        state.world.chunk_revision(x, z)
                    };
                    v.navigation_chunks.push((chunk, revision));
                }
            }
        }
    }
}

pub(crate) fn route_changed(state: &TickState, v: &VillagerState) -> bool {
    let pending = if v.navigation_can_open_doors {
        state.world.pending_villager_route_chunks()
    } else {
        state.world.pending_block_chunks()
    };
    v.navigation_chunks.is_empty()
        || v.navigation_chunks.iter().any(|&(chunk, revision)| {
            let current = if v.navigation_can_open_doors {
                state.world.villager_route_revision(chunk.x, chunk.z)
            } else {
                state.world.chunk_revision(chunk.x, chunk.z)
            };
            revision == u64::MAX
                || !state.world.has_chunk(chunk.x, chunk.z)
                || current != revision
                || pending.contains(&chunk)
        })
}

fn standing_memo(
    ctx: &RouteCtx<'_>,
    cache: &mut PoiCache,
    cell: BlockPos,
    width: f64,
    height: f64,
    can_open_doors: bool,
) -> Option<[f64; 3]> {
    #[cfg(test)]
    if cache.disable_standing_memo {
        return standing_for_route(ctx.world, ctx.registry, cell, width, height, can_open_doors);
    }
    *cache.standing_cache.entry(cell).or_insert_with(|| {
        standing_for_route(ctx.world, ctx.registry, cell, width, height, can_open_doors)
    })
}
