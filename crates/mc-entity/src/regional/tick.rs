use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mc_data::collision_shapes::CollisionClass;
use mc_physics::{
    BlockCollisionBox, BlockCollisionHeight, BlockMaterial, BlockMaterialIds, BlockSampler,
    EntityBody, PhysicsConfig,
};
use mc_world::{ChunkPos, WorldReadSnapshot, WorldReadView};

use crate::{
    EntityId, EntityPhysicsKind, EntityPhysicsQuery, EntityPhysicsStep, PathingProbe,
    PathingProbeResult, Vec3,
};

const SUPPORT_CONTACT_DEPTH: f64 = 1.0e-6;
const VOXEL_SHAPE_MERGE_TOLERANCE: f64 = 1.0e-7;

#[derive(Clone)]
pub struct RegionalTickWorld {
    active_chunks: Arc<HashSet<(i32, i32)>>,
    terrain_pathing_entities: Arc<HashSet<EntityId>>,
    pathing_snapshot: Arc<WorldReadSnapshot>,
    read: WorldReadView,
    materials: Arc<BlockMaterialIds>,
}

impl RegionalTickWorld {
    #[must_use]
    pub fn new(
        active_chunks: Arc<HashSet<(i32, i32)>>,
        terrain_pathing_entities: Arc<HashSet<EntityId>>,
        pathing_snapshot: Arc<WorldReadSnapshot>,
        read: WorldReadView,
        materials: Arc<BlockMaterialIds>,
    ) -> Self {
        Self {
            active_chunks,
            terrain_pathing_entities,
            pathing_snapshot,
            read,
            materials,
        }
    }

    pub(super) fn block_state_at(&self, position: Vec3) -> Option<u32> {
        let y = position.y.floor() as i32;
        if !(mc_world::MIN_Y..mc_world::MAX_Y).contains(&y) {
            return None;
        }
        self.pathing_snapshot
            .get_cached_block(mc_world::BlockPos {
                x: position.x.floor() as i32,
                y,
                z: position.z.floor() as i32,
            })
            .map(|state| state.0)
    }

    pub(super) fn pathing_probe<'a>(
        &'a self,
        entity_aabbs: &'a HashMap<EntityId, mc_physics::Aabb>,
    ) -> OwnerPathingProbe<'a> {
        OwnerPathingProbe {
            world: self,
            entity_aabbs,
            resolved_direct_paths: RefCell::new(HashSet::new()),
        }
    }

    pub(super) fn physics_world(
        &self,
        queries: impl Iterator<Item = EntityPhysicsQuery>,
    ) -> RegionalPhysicsWorld {
        let section = mc_world::SECTION_DIM as i32;
        let mut chunks = HashSet::new();
        for query in queries {
            let config = match query.kind {
                EntityPhysicsKind::Living | EntityPhysicsKind::PowderSnowWalkableLiving => {
                    PhysicsConfig::living_entity()
                }
                EntityPhysicsKind::AquaticLiving => PhysicsConfig::aquatic_entity(),
                _ => continue,
            };
            let bounds = physics_sample_bounds(query, config);
            if bounds.max_y < mc_world::MIN_Y || bounds.min_y >= mc_world::MAX_Y {
                continue;
            }
            for x in bounds.min_x.div_euclid(section)..=bounds.max_x.div_euclid(section) {
                for z in bounds.min_z.div_euclid(section)..=bounds.max_z.div_euclid(section) {
                    chunks.insert(ChunkPos { x, z });
                }
            }
        }
        let chunks = chunks.into_iter().collect::<Vec<_>>();
        RegionalPhysicsWorld {
            snapshot: self.read.snapshot_chunks(&chunks),
            sampled_chunks: chunks.into_boxed_slice(),
            read: self.read.clone(),
            materials: Arc::clone(&self.materials),
        }
    }
}

pub(super) struct RegionalPhysicsWorld {
    snapshot: WorldReadSnapshot,
    sampled_chunks: Box<[ChunkPos]>,
    read: WorldReadView,
    materials: Arc<BlockMaterialIds>,
}

impl RegionalPhysicsWorld {
    pub(super) fn is_current(&self) -> bool {
        let current = self.read.snapshot_chunks(&self.sampled_chunks);
        self.sampled_chunks.iter().all(|&position| {
            match (
                self.snapshot.chunk_ref(position),
                current.chunk_ref(position),
            ) {
                (Some(expected), Some(current)) => Arc::ptr_eq(expected, current),
                (None, None) => true,
                (Some(_), None) | (None, Some(_)) => false,
            }
        })
    }

    pub(super) fn step_local(&self, query: EntityPhysicsQuery) -> Option<EntityPhysicsStep> {
        let config = match query.kind {
            EntityPhysicsKind::Living | EntityPhysicsKind::PowderSnowWalkableLiving => {
                PhysicsConfig::living_entity()
            }
            EntityPhysicsKind::AquaticLiving => PhysicsConfig::aquatic_entity(),
            _ => return None,
        };
        if !self.samples_are_complete(query, config) {
            return Some(EntityPhysicsStep {
                id: query.id,
                position: query.position,
                velocity: Vec3::ZERO,
                on_ground: query.on_ground,
                horizontal_collision: false,
            });
        }
        let section = mc_world::SECTION_DIM as i32;
        let center = ChunkPos {
            x: (query.position.x.floor() as i32).div_euclid(section),
            z: (query.position.z.floor() as i32).div_euclid(section),
        };
        let sampler = SnapshotPhysicsSampler {
            world: self,
            query,
            center,
            center_chunk: self.snapshot.chunk_ref(center),
        };
        let stepped = mc_physics::step_entity(
            EntityBody {
                position: physics_vec(query.position),
                velocity: physics_vec(query.velocity),
                aabb: query.aabb,
                on_ground: query.on_ground,
            },
            &sampler,
            config,
        );
        Some(EntityPhysicsStep {
            id: query.id,
            position: entity_vec(stepped.body.position),
            velocity: entity_vec(stepped.body.velocity),
            on_ground: stepped.body.on_ground,
            horizontal_collision: stepped.horizontal_collision,
        })
    }

    fn samples_are_complete(&self, query: EntityPhysicsQuery, config: PhysicsConfig) -> bool {
        let bounds = physics_sample_bounds(query, config);
        if bounds.max_y < mc_world::MIN_Y || bounds.min_y >= mc_world::MAX_Y {
            return true;
        }
        let section = mc_world::SECTION_DIM as i32;
        for x in bounds.min_x.div_euclid(section)..=bounds.max_x.div_euclid(section) {
            for z in bounds.min_z.div_euclid(section)..=bounds.max_z.div_euclid(section) {
                if !self.snapshot.contains_chunk(ChunkPos { x, z }) {
                    return false;
                }
            }
        }
        true
    }
}

struct PhysicsSampleBounds {
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    min_z: i32,
    max_z: i32,
}

fn physics_sample_bounds(query: EntityPhysicsQuery, config: PhysicsConfig) -> PhysicsSampleBounds {
    let next_x = query.position.x + query.velocity.x * config.tick_seconds;
    let next_y = query.position.y + query.velocity.y * config.tick_seconds;
    let next_z = query.position.z + query.velocity.z * config.tick_seconds;
    let half = query.aabb.half_width;
    PhysicsSampleBounds {
        min_x: (query.position.x.min(next_x) - half - 1.0).floor() as i32,
        max_x: (query.position.x.max(next_x) + half + 1.0).floor() as i32,
        min_y: (query.position.y.min(next_y) - 2.0).floor() as i32,
        max_y: (query.position.y.max(next_y) + query.aabb.height + 2.0).floor() as i32,
        min_z: (query.position.z.min(next_z) - half - 1.0).floor() as i32,
        max_z: (query.position.z.max(next_z) + half + 1.0).floor() as i32,
    }
}

struct SnapshotPhysicsSampler<'a> {
    world: &'a RegionalPhysicsWorld,
    query: EntityPhysicsQuery,
    center: ChunkPos,
    center_chunk: Option<&'a mc_world::ChunkSnapshot>,
}

impl SnapshotPhysicsSampler<'_> {
    fn state_id_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !(mc_world::MIN_Y..mc_world::MAX_Y).contains(&y) {
            return None;
        }
        let section = mc_world::SECTION_DIM as i32;
        let chunk = ChunkPos {
            x: x.div_euclid(section),
            z: z.div_euclid(section),
        };
        if chunk == self.center {
            return self
                .center_chunk?
                .get_block(x.rem_euclid(section) as u8, y, z.rem_euclid(section) as u8)
                .map(|state| state.0);
        }
        self.world
            .snapshot
            .get_cached_block(mc_world::BlockPos { x, y, z })
            .map(|state| state.0)
    }
}

impl BlockSampler for SnapshotPhysicsSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.state_id_at(x, y, z)
            .map_or(BlockMaterial::Air, |state| {
                self.world.materials.classify(state)
            })
    }

    fn collision_height_at(&self, x: i32, y: i32, z: i32) -> Option<BlockCollisionHeight> {
        self.state_id_at(x, y, z)
            .and_then(|state| self.world.materials.collision_height(state))
    }

    fn max_collision_box_y(&self) -> u8 {
        let max_y = mc_data::collision_shapes::vanilla_collision_shapes().max_box_y();
        u8::try_from((max_y + 255) / 256).expect("vanilla collision height fits u8")
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        let Some(state) = self.state_id_at(x, y, z) else {
            return;
        };
        let fact = canonical_pathing_state_facts()
            .get(state as usize)
            .and_then(Option::as_ref);
        if fact.is_some_and(|fact| fact.powder_snow) {
            if self.query.fall_distance > 2.5 {
                if let Some(collision_box) = BlockCollisionBox::from_fixed_4096([
                    0,
                    0,
                    0,
                    4096,
                    (0.9_f32 * 4096.0) as i16,
                    4096,
                ]) {
                    emit(collision_box);
                }
            } else if self.query.kind == EntityPhysicsKind::PowderSnowWalkableLiving
                && self.query.position.y > f64::from(y) + 1.0 - 1.0e-5_f32 as f64
            {
                emit(BlockCollisionBox::FULL_BLOCK);
            }
            return;
        }
        if fact.is_some() {
            match mc_data::collision_shapes::vanilla_collision_class(state) {
                CollisionClass::Empty => return,
                CollisionClass::FullCube => {
                    emit(BlockCollisionBox::FULL_BLOCK);
                    return;
                }
                CollisionClass::Complex | CollisionClass::Missing => {}
            }
        }
        let exact =
            fact.and_then(|_| mc_data::collision_shapes::vanilla_collision_shapes().get(state));
        if let Some(boxes) = exact {
            for collision_box in boxes.iter() {
                if let Some(collision_box) =
                    BlockCollisionBox::from_fixed_4096(collision_box.coordinates())
                {
                    emit(collision_box);
                }
            }
        } else if let Some(height) = self.world.materials.collision_height(state) {
            let max_y = (height.as_blocks() * 16.0) as u8;
            if let Some(collision_box) = BlockCollisionBox::from_sixteenths(0, 0, 0, 16, max_y, 16)
            {
                emit(collision_box);
            }
        }
    }
}

fn physics_vec(vec: Vec3) -> mc_physics::Vec3 {
    mc_physics::Vec3::new(vec.x, vec.y, vec.z)
}

fn entity_vec(vec: mc_physics::Vec3) -> Vec3 {
    Vec3::new(vec.x, vec.y, vec.z)
}

pub(super) struct OwnerPathingProbe<'a> {
    world: &'a RegionalTickWorld,
    entity_aabbs: &'a HashMap<EntityId, mc_physics::Aabb>,
    resolved_direct_paths: RefCell<HashSet<EntityId>>,
}

impl OwnerPathingProbe<'_> {
    pub(super) fn take_resolved_direct_paths(self) -> HashSet<EntityId> {
        self.resolved_direct_paths.into_inner()
    }

    fn state_at(&self, x: i32, y: i32, z: i32) -> Result<u32, PathingProbeResult> {
        let section = mc_world::SECTION_DIM as i32;
        let chunk = ChunkPos {
            x: x.div_euclid(section),
            z: z.div_euclid(section),
        };
        if !self.world.active_chunks.contains(&(chunk.x, chunk.z)) {
            return Err(PathingProbeResult::Unloaded);
        }
        if !self.world.pathing_snapshot.contains_chunk(chunk) {
            return Err(PathingProbeResult::Unloaded);
        }
        self.world
            .pathing_snapshot
            .get_cached_block(mc_world::BlockPos { x, y, z })
            .map(|state| state.0)
            .ok_or(PathingProbeResult::Blocked)
    }
}

#[derive(Clone, Copy)]
struct CanonicalPathingStateFact {
    powder_snow: bool,
}

enum PathingCollisionShape<'a> {
    Empty,
    FullCube,
    Voxel(mc_data::collision_shapes::CollisionShape<'a>),
}

fn canonical_pathing_state_facts() -> &'static [Option<CanonicalPathingStateFact>] {
    static FACTS: std::sync::OnceLock<Box<[Option<CanonicalPathingStateFact>]>> =
        std::sync::OnceLock::new();
    FACTS.get_or_init(|| {
        let reports = mc_data::blocks::solaris_required_blocks_report();
        let max_state = reports
            .iter()
            .flat_map(|block| block.states.iter().map(|state| state.id))
            .max()
            .unwrap_or(0);
        let mut facts = (0..=max_state).map(|_| None).collect::<Vec<_>>();
        let collision_shapes = mc_data::collision_shapes::vanilla_collision_shapes();
        for block in reports {
            for state in block.states {
                let properties = block
                    .properties
                    .keys()
                    .map(|name| {
                        (
                            name.clone(),
                            state.properties.get(name).cloned().unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                if collision_shapes
                    .get_for_state(state.id, &block.id, properties.as_ref())
                    .is_some()
                {
                    facts[state.id as usize] = Some(CanonicalPathingStateFact {
                        powder_snow: block.id.as_str() == "minecraft:powder_snow",
                    });
                }
            }
        }
        facts.into_boxed_slice()
    })
}
/// Eagerly initializes canonical pathing facts and the shared collision-class
/// table used by physics and pathing. First use also parses vanilla shape data;
/// call once during startup so initialization cannot stall a simulation tick.
pub fn warm_physics_caches() -> std::num::NonZeroUsize {
    let facts = canonical_pathing_state_facts();
    let _ = mc_data::collision_shapes::vanilla_collision_class(0);
    std::num::NonZeroUsize::new(facts.len()).expect("physics pathing facts must contain states")
}

fn collision_shape_with_facts(
    state: u32,
    collision_shapes: &mc_data::collision_shapes::CollisionShapeTable,
) -> PathingCollisionShape<'_> {
    if canonical_pathing_state_facts()
        .get(state as usize)
        .and_then(Option::as_ref)
        .is_none()
    {
        return PathingCollisionShape::FullCube;
    }
    match mc_data::collision_shapes::vanilla_collision_class(state) {
        CollisionClass::Empty => PathingCollisionShape::Empty,
        CollisionClass::FullCube | CollisionClass::Missing => PathingCollisionShape::FullCube,
        CollisionClass::Complex => collision_shapes.get(state).map_or(
            PathingCollisionShape::FullCube,
            PathingCollisionShape::Voxel,
        ),
    }
}

fn voxel_shape_axis_intersects(
    first_min: f64,
    first_max: f64,
    second_min: f64,
    second_max: f64,
) -> bool {
    first_min < second_max - VOXEL_SHAPE_MERGE_TOLERANCE
        && second_min < first_max - VOXEL_SHAPE_MERGE_TOLERANCE
}

fn support_strip_intersects_y(feet_y: f64, collision_min_y: f64, collision_max_y: f64) -> bool {
    feet_y - SUPPORT_CONTACT_DEPTH < collision_max_y && feet_y > collision_min_y
}

fn physics_aabb(
    entity_aabbs: &HashMap<EntityId, mc_physics::Aabb>,
    id: EntityId,
) -> mc_physics::Aabb {
    entity_aabbs
        .get(&id)
        .copied()
        .unwrap_or(mc_physics::Aabb::COW)
}

impl OwnerPathingProbe<'_> {
    fn body_intersects_state_collision(
        position: Vec3,
        aabb: mc_physics::Aabb,
        x: i32,
        y: i32,
        z: i32,
        state: u32,
        collision_shapes: &mc_data::collision_shapes::CollisionShapeTable,
    ) -> bool {
        match collision_shape_with_facts(state, collision_shapes) {
            PathingCollisionShape::Empty => false,
            PathingCollisionShape::FullCube => {
                Self::body_intersects_box(position, aabb, x, y, z, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
            }
            PathingCollisionShape::Voxel(boxes) => boxes.iter().any(|collision_box| {
                Self::body_intersects_box(position, aabb, x, y, z, collision_box.as_blocks())
            }),
        }
    }

    fn body_intersects_box(
        position: Vec3,
        aabb: mc_physics::Aabb,
        x: i32,
        y: i32,
        z: i32,
        [min_x, min_y, min_z, max_x, max_y, max_z]: [f64; 6],
    ) -> bool {
        position.x - aabb.half_width < f64::from(x) + max_x
            && position.x + aabb.half_width > f64::from(x) + min_x
            && position.y < f64::from(y) + max_y
            && position.y + aabb.height > f64::from(y) + min_y
            && position.z - aabb.half_width < f64::from(z) + max_z
            && position.z + aabb.half_width > f64::from(z) + min_z
    }

    fn state_supports_feet(
        position: Vec3,
        aabb: mc_physics::Aabb,
        x: i32,
        y: i32,
        z: i32,
        state: u32,
        collision_shapes: &mc_data::collision_shapes::CollisionShapeTable,
    ) -> bool {
        match collision_shape_with_facts(state, collision_shapes) {
            PathingCollisionShape::Empty => false,
            PathingCollisionShape::FullCube => {
                support_strip_intersects_y(position.y, f64::from(y), f64::from(y) + 1.0)
                    && position.x - aabb.half_width < f64::from(x) + 1.0
                    && position.x + aabb.half_width > f64::from(x)
                    && position.z - aabb.half_width < f64::from(z) + 1.0
                    && position.z + aabb.half_width > f64::from(z)
            }
            PathingCollisionShape::Voxel(boxes) => boxes.iter().any(|collision_box| {
                let [min_x, min_y, min_z, max_x, max_y, max_z] = collision_box.as_blocks();
                voxel_shape_axis_intersects(
                    position.y - SUPPORT_CONTACT_DEPTH,
                    position.y,
                    f64::from(y) + min_y,
                    f64::from(y) + max_y,
                ) && voxel_shape_axis_intersects(
                    position.x - aabb.half_width,
                    position.x + aabb.half_width,
                    f64::from(x) + min_x,
                    f64::from(x) + max_x,
                ) && voxel_shape_axis_intersects(
                    position.z - aabb.half_width,
                    position.z + aabb.half_width,
                    f64::from(z) + min_z,
                    f64::from(z) + max_z,
                )
            }),
        }
    }
}

impl PathingProbe for OwnerPathingProbe<'_> {
    fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
        self.can_entity_stand_at(EntityId(i32::MIN), position)
    }

    fn can_entity_stand_at(&self, entity_id: EntityId, position: Vec3) -> PathingProbeResult {
        const EPSILON: f64 = 1.0e-6;

        let aabb = physics_aabb(self.entity_aabbs, entity_id);
        if !position.is_finite()
            || position.y.floor() < f64::from(mc_world::MIN_Y)
            || (position.y + aabb.height).ceil() > f64::from(mc_world::MAX_Y)
        {
            return PathingProbeResult::Blocked;
        }
        let section = mc_world::SECTION_DIM as i32;
        let chunk = (
            (position.x.floor() as i32).div_euclid(section),
            (position.z.floor() as i32).div_euclid(section),
        );
        if !self.world.active_chunks.contains(&chunk) {
            return PathingProbeResult::Unloaded;
        }
        if entity_id != EntityId(i32::MIN)
            && !self.world.terrain_pathing_entities.contains(&entity_id)
        {
            return PathingProbeResult::Walkable;
        }

        let min_x = (position.x - aabb.half_width + EPSILON).floor() as i32;
        let max_x = (position.x + aabb.half_width - EPSILON).floor() as i32;
        let min_z = (position.z - aabb.half_width + EPSILON).floor() as i32;
        let max_z = (position.z + aabb.half_width - EPSILON).floor() as i32;
        let shapes = mc_data::collision_shapes::vanilla_collision_shapes();
        let max_collision_box_y = shapes.max_box_y_blocks();
        let body_root_min_y =
            ((position.y - max_collision_box_y).floor() as i32).max(mc_world::MIN_Y);
        let max_y = (position.y + aabb.height - EPSILON).floor() as i32;
        let body_min_y = (position.y + EPSILON).floor() as i32;
        let mut touches_fluid = false;
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                for y in body_root_min_y..=max_y {
                    let state = match self.state_at(x, y, z) {
                        Ok(state) => state,
                        Err(result) => return result,
                    };
                    let material = self.world.materials.classify(state);
                    if material.is_solid()
                        && Self::body_intersects_state_collision(
                            position, aabb, x, y, z, state, shapes,
                        )
                    {
                        return PathingProbeResult::Blocked;
                    }
                    if material.is_fluid() && y >= body_min_y {
                        touches_fluid = true;
                    }
                }
            }
        }
        if touches_fluid {
            return PathingProbeResult::Walkable;
        }

        let support_min = (position.y - SUPPORT_CONTACT_DEPTH - max_collision_box_y)
            .next_down()
            .floor() as i32
            + 1;
        let support_max = position.y.ceil() as i32 - 1;
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                for y in support_min.max(mc_world::MIN_Y)..=support_max.min(mc_world::MAX_Y - 1) {
                    let state = match self.state_at(x, y, z) {
                        Ok(state) => state,
                        Err(result) => return result,
                    };
                    if self.world.materials.classify(state).is_solid()
                        && Self::state_supports_feet(position, aabb, x, y, z, state, shapes)
                    {
                        return PathingProbeResult::Walkable;
                    }
                }
            }
        }
        PathingProbeResult::Blocked
    }

    fn direct_path_resolved(&self, entity_id: EntityId) {
        if self.world.terrain_pathing_entities.contains(&entity_id) {
            self.resolved_direct_paths.borrow_mut().insert(entity_id);
        }
    }
}
