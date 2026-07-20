use super::*;

#[cfg(test)]
#[path = "pathing_tests.rs"]
mod pathing_tests;

// Entity.checkSupportingBlock probes this exact distance below the feet in 26.1.2.
const SUPPORT_CONTACT_DEPTH: f64 = 1.0e-6;
// Shapes.joinIsNotEmpty merges voxel coordinates this close before applying AND.
const VOXEL_SHAPE_MERGE_TOLERANCE: f64 = 1.0e-7;

#[derive(Clone)]
struct CanonicalPathingStateFact {
    block: Identifier,
    properties: Box<[(String, String)]>,
}

enum PathingCollisionShape<'a> {
    FullCube,
    Voxel(&'a [mc_data::collision_shapes::CollisionBox]),
}

fn canonical_pathing_state_fact(state: u32) -> Option<&'static CanonicalPathingStateFact> {
    static FACTS: std::sync::OnceLock<Box<[Option<CanonicalPathingStateFact>]>> =
        std::sync::OnceLock::new();
    FACTS
        .get_or_init(|| {
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
                            block: block.id.clone(),
                            properties,
                        });
                    }
                }
            }
            facts.into_boxed_slice()
        })
        .get(state as usize)
        .and_then(Option::as_ref)
}

pub(super) struct LoadedChunkPathingProbe<'a> {
    active_chunks: &'a HashSet<(i32, i32)>,
    terrain_pathing_entities: &'a HashSet<EntityId>,
    entity_aabbs: &'a HashMap<EntityId, mc_physics::Aabb>,
    terrain: Option<LoadedTerrainPathingProbe<'a>>,
    pub(super) resolved_direct_paths: Mutex<HashSet<EntityId>>,
}

pub(super) struct LoadedTerrainPathingProbe<'a> {
    snapshot: &'a mc_world::WorldReadSnapshot,
    materials: &'a mc_physics::BlockMaterialIds,
}

#[cfg(test)]
pub(super) fn terrain_snapshot_chunks_for_probe_positions(
    positions: impl IntoIterator<Item = (EntityId, Vec3)>,
    terrain_pathing_entities: &HashSet<EntityId>,
    entity_aabbs: &HashMap<EntityId, mc_physics::Aabb>,
    active_chunks: &HashSet<(i32, i32)>,
) -> Vec<ChunkPos> {
    let mut chunks = HashSet::new();
    for (entity_id, position) in positions {
        insert_terrain_snapshot_chunks_for_probe_position(
            &mut chunks,
            entity_id,
            position,
            terrain_pathing_entities,
            entity_aabbs,
            active_chunks,
        );
    }
    sorted_chunk_positions(chunks)
}

pub(super) fn insert_terrain_snapshot_chunks_for_probe_position(
    chunks: &mut HashSet<ChunkPos>,
    entity_id: EntityId,
    position: Vec3,
    terrain_pathing_entities: &HashSet<EntityId>,
    entity_aabbs: &HashMap<EntityId, mc_physics::Aabb>,
    active_chunks: &HashSet<(i32, i32)>,
) {
    const EPSILON: f64 = 1.0e-6;

    if !terrain_pathing_entities.contains(&entity_id) || !position.is_finite() {
        return;
    }
    let aabb = entity_aabbs
        .get(&entity_id)
        .copied()
        .unwrap_or(mc_physics::Aabb::COW);
    let min_x = (position.x - aabb.half_width + EPSILON).floor() as i32;
    let max_x = (position.x + aabb.half_width - EPSILON).floor() as i32;
    let min_z = (position.z - aabb.half_width + EPSILON).floor() as i32;
    let max_z = (position.z + aabb.half_width - EPSILON).floor() as i32;
    let min_chunk_x = min_x.div_euclid(SECTION_DIM as i32);
    let max_chunk_x = max_x.div_euclid(SECTION_DIM as i32);
    let min_chunk_z = min_z.div_euclid(SECTION_DIM as i32);
    let max_chunk_z = max_z.div_euclid(SECTION_DIM as i32);
    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            if active_chunks.contains(&(chunk_x, chunk_z)) {
                chunks.insert(ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                });
            }
        }
    }
}

pub(super) fn sorted_chunk_positions(chunks: HashSet<ChunkPos>) -> Vec<ChunkPos> {
    let mut chunks = chunks.into_iter().collect::<Vec<_>>();
    chunks.sort_unstable_by_key(|chunk| (chunk.x, chunk.z));
    chunks
}

pub(super) fn acquire_regional_worker_permits(
    resources: &crate::chunk_pipeline::ChunkPipelineResources,
    parallel_batch_count: usize,
) -> Vec<crate::chunk_pipeline::ChunkPipelinePermit> {
    // The ticker owns the runtime CPU reserved outside this background permit pool.
    // It must never wait for a worker whose result the ticker itself has to accept.
    if parallel_batch_count < 2 || resources.cpu_limit() < 2 {
        return Vec::new();
    }
    let target = (parallel_batch_count - 1).min(resources.cpu_limit() - 1);
    let mut permits = Vec::with_capacity(target);
    for _ in 0..target {
        let Some(permit) = resources.try_acquire_cpu() else {
            break;
        };
        permits.push(permit);
    }
    permits
}

impl<'a> LoadedChunkPathingProbe<'a> {
    pub(super) fn new(
        active_chunks: &'a HashSet<(i32, i32)>,
        terrain_pathing_entities: &'a HashSet<EntityId>,
        entity_aabbs: &'a HashMap<EntityId, mc_physics::Aabb>,
        terrain: Option<LoadedTerrainPathingProbe<'a>>,
    ) -> Self {
        Self {
            active_chunks,
            terrain_pathing_entities,
            entity_aabbs,
            terrain,
            resolved_direct_paths: Mutex::new(HashSet::new()),
        }
    }
}

impl<'a> LoadedTerrainPathingProbe<'a> {
    pub(super) fn new(
        snapshot: &'a mc_world::WorldReadSnapshot,
        materials: &'a mc_physics::BlockMaterialIds,
    ) -> Self {
        Self {
            snapshot,
            materials,
        }
    }
}

impl PathingProbe for LoadedChunkPathingProbe<'_> {
    fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
        self.can_entity_stand_at(EntityId(i32::MIN), position)
    }

    fn can_entity_stand_at(&self, entity_id: EntityId, position: Vec3) -> PathingProbeResult {
        const EPSILON: f64 = 1.0e-6;

        let aabb = self
            .entity_aabbs
            .get(&entity_id)
            .copied()
            .unwrap_or(mc_physics::Aabb::COW);
        if !position.is_finite()
            || position.y.floor() < f64::from(mc_world::chunk::MIN_Y)
            || (position.y + aabb.height).ceil() > f64::from(mc_world::chunk::MAX_Y)
        {
            return PathingProbeResult::Blocked;
        }
        if !self
            .active_chunks
            .contains(&chunk_pos_from_coords(position.x, position.z))
        {
            return PathingProbeResult::Unloaded;
        }
        if entity_id != EntityId(i32::MIN) && !self.terrain_pathing_entities.contains(&entity_id) {
            return PathingProbeResult::Walkable;
        }
        let Some(terrain) = self.terrain.as_ref() else {
            return PathingProbeResult::Walkable;
        };

        let min_x = (position.x - aabb.half_width + EPSILON).floor() as i32;
        let max_x = (position.x + aabb.half_width - EPSILON).floor() as i32;
        let min_z = (position.z - aabb.half_width + EPSILON).floor() as i32;
        let max_z = (position.z + aabb.half_width - EPSILON).floor() as i32;
        let collision_shapes = mc_data::collision_shapes::vanilla_collision_shapes();
        let max_collision_box_y = f64::from(collision_shapes.max_box_y()) / 16.0;
        let body_root_min_y =
            ((position.y - max_collision_box_y).floor() as i32).max(mc_world::chunk::MIN_Y);
        let max_y = (position.y + aabb.height - EPSILON).floor() as i32;
        let body_min_y = (position.y + EPSILON).floor() as i32;
        let mut touches_fluid = false;
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                for y in body_root_min_y..=max_y {
                    let state = match self.state_at(terrain, x, y, z) {
                        Ok(state) => state,
                        Err(result) => return result,
                    };
                    let material = terrain.materials.classify(state);
                    if material.is_solid()
                        && Self::body_intersects_state_collision(
                            position,
                            aabb,
                            x,
                            y,
                            z,
                            state,
                            collision_shapes,
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

        let (support_min_y, support_max_y) =
            Self::support_candidate_y_bounds(position.y, max_collision_box_y);
        let support_min_y = support_min_y.max(mc_world::chunk::MIN_Y);
        let support_max_y = support_max_y.min(mc_world::chunk::MAX_Y - 1);
        for x in min_x..=max_x {
            for z in min_z..=max_z {
                for y in support_min_y..=support_max_y {
                    let state = match self.state_at(terrain, x, y, z) {
                        Ok(state) => state,
                        Err(result) => return result,
                    };
                    if terrain.materials.classify(state).is_solid()
                        && Self::state_supports_feet(
                            position,
                            aabb,
                            x,
                            y,
                            z,
                            state,
                            collision_shapes,
                        )
                    {
                        return PathingProbeResult::Walkable;
                    }
                }
            }
        }
        PathingProbeResult::Blocked
    }

    fn direct_path_resolved(&self, entity_id: EntityId) {
        if self.terrain_pathing_entities.contains(&entity_id) {
            self.resolved_direct_paths
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(entity_id);
        }
    }
}

impl LoadedChunkPathingProbe<'_> {
    fn body_intersects_state_collision(
        position: Vec3,
        aabb: mc_physics::Aabb,
        x: i32,
        y: i32,
        z: i32,
        state: u32,
        collision_shapes: &mc_data::collision_shapes::CollisionShapeTable,
    ) -> bool {
        match Self::collision_shape_with_facts(
            state,
            canonical_pathing_state_fact(state),
            collision_shapes,
        ) {
            PathingCollisionShape::FullCube => {
                Self::body_intersects_box(position, aabb, x, y, z, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0])
            }
            PathingCollisionShape::Voxel(boxes) => boxes.iter().copied().any(|collision_box| {
                Self::body_intersects_voxel_box(position, aabb, x, y, z, collision_box.as_blocks())
            }),
        }
    }

    fn collision_shape_with_facts<'a>(
        state: u32,
        facts: Option<&CanonicalPathingStateFact>,
        collision_shapes: &'a mc_data::collision_shapes::CollisionShapeTable,
    ) -> PathingCollisionShape<'a> {
        let Some(facts) = facts else {
            return PathingCollisionShape::FullCube;
        };
        let Some(boxes) =
            collision_shapes.get_for_state(state, &facts.block, facts.properties.as_ref())
        else {
            return PathingCollisionShape::FullCube;
        };
        if boxes.len() == 1 && boxes[0].coordinates() == [0, 0, 0, 16, 16, 16] {
            PathingCollisionShape::FullCube
        } else {
            PathingCollisionShape::Voxel(boxes)
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

    fn body_intersects_voxel_box(
        position: Vec3,
        aabb: mc_physics::Aabb,
        x: i32,
        y: i32,
        z: i32,
        [min_x, min_y, min_z, max_x, max_y, max_z]: [f64; 6],
    ) -> bool {
        Self::voxel_shape_axis_intersects(
            position.x - aabb.half_width,
            position.x + aabb.half_width,
            f64::from(x) + min_x,
            f64::from(x) + max_x,
        ) && Self::voxel_shape_axis_intersects(
            position.y,
            position.y + aabb.height,
            f64::from(y) + min_y,
            f64::from(y) + max_y,
        ) && Self::voxel_shape_axis_intersects(
            position.z - aabb.half_width,
            position.z + aabb.half_width,
            f64::from(z) + min_z,
            f64::from(z) + max_z,
        )
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

    fn support_candidate_y_bounds(feet_y: f64, max_collision_box_y: f64) -> (i32, i32) {
        let support_strip_min = feet_y - SUPPORT_CONTACT_DEPTH;
        // A candidate with top exactly at the rounded boundary may still overlap before
        // f64 cancellation. Widen by one representable value; the shape test rejects it.
        let lower_boundary = (support_strip_min - max_collision_box_y).next_down();
        let min_y = lower_boundary.floor() as i32 + 1;
        let max_y = feet_y.ceil() as i32 - 1;
        (min_y, max_y)
    }

    fn support_strip_intersects_y(feet_y: f64, collision_min_y: f64, collision_max_y: f64) -> bool {
        feet_y - SUPPORT_CONTACT_DEPTH < collision_max_y && feet_y > collision_min_y
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
        match Self::collision_shape_with_facts(
            state,
            canonical_pathing_state_fact(state),
            collision_shapes,
        ) {
            PathingCollisionShape::FullCube => {
                Self::support_strip_intersects_y(position.y, f64::from(y), f64::from(y) + 1.0)
                    && position.x - aabb.half_width < f64::from(x) + 1.0
                    && position.x + aabb.half_width > f64::from(x)
                    && position.z - aabb.half_width < f64::from(z) + 1.0
                    && position.z + aabb.half_width > f64::from(z)
            }
            PathingCollisionShape::Voxel(boxes) => boxes.iter().copied().any(|collision_box| {
                let [min_x, min_y, min_z, max_x, max_y, max_z] = collision_box.as_blocks();
                Self::voxel_shape_axis_intersects(
                    position.y - SUPPORT_CONTACT_DEPTH,
                    position.y,
                    f64::from(y) + min_y,
                    f64::from(y) + max_y,
                ) && Self::voxel_shape_axis_intersects(
                    position.x - aabb.half_width,
                    position.x + aabb.half_width,
                    f64::from(x) + min_x,
                    f64::from(x) + max_x,
                ) && Self::voxel_shape_axis_intersects(
                    position.z - aabb.half_width,
                    position.z + aabb.half_width,
                    f64::from(z) + min_z,
                    f64::from(z) + max_z,
                )
            }),
        }
    }

    fn state_at(
        &self,
        terrain: &LoadedTerrainPathingProbe<'_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> Result<u32, PathingProbeResult> {
        let chunk_pos = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        if !self.active_chunks.contains(&(chunk_pos.x, chunk_pos.z)) {
            return Err(PathingProbeResult::Unloaded);
        }
        let chunk = terrain
            .snapshot
            .chunk(chunk_pos)
            .ok_or(PathingProbeResult::Unloaded)?;
        let local_x = x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = z.rem_euclid(SECTION_DIM as i32) as u8;
        let state = chunk
            .get_block(local_x, y, local_z)
            .ok_or(PathingProbeResult::Blocked)?;
        Ok(state.0)
    }
}
