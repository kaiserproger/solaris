//! Resident motion: geometry cache, route sampling, body motion.
//! Moved verbatim from `brain.rs`.

use super::*;

/// Test-only immutable A/B reference: disabling both caches leaves the same
/// single-standing algorithm and ordinary per-tick collision integration.
#[cfg(test)]
pub(super) const DISABLE_MOTION_GEOMETRY: bool = false;

/// Route-local sampled block states and the next standing point. Neither cache
/// participates in durable equality or journal-image cloning.
#[derive(Debug, Default)]
pub(crate) struct MotionGeometryCache {
    pub(super) blocks: Vec<u32>,
    pub(super) min: [i32; 3],
    pub(super) max: [i32; 3],
    pub(super) body: [f64; 2],
    pub(super) standing: Option<(BlockPos, Option<[f64; 3]>)>,
    pub(super) revision: u64,
    pub(super) pending: bool,
    #[cfg(test)]
    pub(super) disable: bool,
    #[cfg(test)]
    pub(super) geometry_hits: usize,
    #[cfg(test)]
    pub(super) geometry_misses: usize,
    #[cfg(test)]
    pub(super) standing_hits: usize,
}

impl Clone for MotionGeometryCache {
    fn clone(&self) -> Self {
        #[cfg(test)]
        {
            Self {
                disable: self.disable,
                ..Self::default()
            }
        }
        #[cfg(not(test))]
        {
            Self::default()
        }
    }
}

impl PartialEq for MotionGeometryCache {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl MotionGeometryCache {
    pub(super) fn enabled(&self) -> bool {
        #[cfg(test)]
        {
            !DISABLE_MOTION_GEOMETRY && !self.disable
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    // Preserve allocation capacity and the test-only reference-mode flag.
    pub(crate) fn clear(&mut self) {
        self.blocks.clear();
        self.standing = None;
    }

    pub(super) fn begin(
        &mut self,
        state: &TickState,
        v: &VillagerState,
        width: f64,
        height: f64,
    ) -> bool {
        let unchanged = !route_changed(state, v);
        self.begin_route(state, v, width, height, unchanged);
        unchanged
    }

    pub(super) fn begin_route(
        &mut self,
        state: &TickState,
        v: &VillagerState,
        width: f64,
        height: f64,
        unchanged: bool,
    ) {
        // Door flips preserve a villager's route, not its physical shapes.
        let pending = !state.world.overlay_is_empty()
            && v.navigation_chunks
                .iter()
                .any(|&(chunk, _)| state.world.pending_block_chunks().contains(&chunk));
        let physical_changed = self.pending
            || pending
            || (state.world.revision_now() != self.revision
                && v.navigation_chunks.iter().any(|&(chunk, _)| {
                    let revision = state.world.chunk_revision(chunk.x, chunk.z);
                    revision == u64::MAX || revision > self.revision
                }));
        if !unchanged || physical_changed || self.body != [width, height] || !self.enabled() {
            self.clear();
            self.body = [width, height];
        }
        self.revision = state.world.revision_now();
        // A cache sampled from the overlay cannot survive its fold, even when
        // a second toggle restores the original state without a wire revision.
        self.pending = pending;
    }

    pub(super) fn standing(
        &mut self,
        state: &TickState,
        v: &VillagerState,
        cell: BlockPos,
        unchanged: bool,
    ) -> Option<[f64; 3]> {
        let [width, height] = self.body;
        let x = f64::from(cell.x) + 0.5;
        let z = f64::from(cell.z) + 0.5;
        let covered = unchanged
            && self.enabled()
            && route_covers_columns(
                v,
                [(x - width).floor() as i32, (z - width).floor() as i32],
                [(x + width).floor() as i32, (z + width).floor() as i32],
            );
        if covered
            && let Some((cached_cell, point)) = self.standing
            && cached_cell == cell
        {
            #[cfg(test)]
            {
                self.standing_hits += 1;
            }
            return point;
        }
        let point = allocator::standing(&(state).world, &(state).registry, cell, width, height);
        if covered {
            self.standing = Some((cell, point));
        }
        point
    }

    pub(super) fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if self.blocks.is_empty()
            || x < self.min[0]
            || x > self.max[0]
            || y < self.min[1]
            || y > self.max[1]
            || z < self.min[2]
            || z > self.max[2]
        {
            return None;
        }
        let index = ((x - self.min[0]) as usize * (self.max[1] - self.min[1] + 1) as usize
            + (y - self.min[1]) as usize)
            * (self.max[2] - self.min[2] + 1) as usize
            + (z - self.min[2]) as usize;
        self.blocks.get(index).copied()
    }

    pub(super) fn sample_motion(
        &mut self,
        state: &TickState,
        v: &VillagerState,
        body: mc_physics::EntityBody,
        desired: mc_physics::Vec3,
        unchanged: bool,
    ) -> bool {
        if !unchanged || !self.enabled() {
            return false;
        }
        let p = body.position;
        let w = body.aabb.half_width;
        let h = body.aabb.height;
        if ![p.x, p.y, p.z, w, h, desired.x, desired.y, desired.z]
            .into_iter()
            .all(f64::is_finite)
            || w <= 0.0
            || h <= 0.0
            || state.registry.max_collision_box_y == 0
        {
            return false;
        }
        // Match the ordinary resolver's swept-body/step scan, with no eager
        // halo prefill. Per-query misses still use TickSampler if that scan ever
        // grows additional sampling requirements.
        let step = mc_physics::STEP_HEIGHT;
        let local_y = f64::from(state.registry.max_collision_box_y) / 16.0;
        let min = [
            (p.x - w + desired.x.min(0.0)).floor() as i32,
            ((p.y + desired.y.min(-step) - local_y).floor() as i32).saturating_add(1),
            (p.z - w + desired.z.min(0.0)).floor() as i32,
        ];
        let max = [
            ((p.x + w + desired.x.max(0.0)).ceil() as i32).saturating_sub(1),
            ((p.y + h + desired.y.max(step)).ceil() as i32).saturating_sub(1),
            ((p.z + w + desired.z.max(0.0)).ceil() as i32).saturating_sub(1),
        ];
        let Some(cells) = (0..3).try_fold(1usize, |count, axis| {
            let len = usize::try_from(i64::from(max[axis]) - i64::from(min[axis]) + 1).ok()?;
            count.checked_mul(len)
        }) else {
            return false;
        };
        // Large impulses/bodies retain the ordinary bounded collision scan;
        // operational storage is reserved for a small local motion envelope.
        if !(1..=512).contains(&cells) {
            return false;
        }
        if !route_covers_columns(v, [min[0], min[2]], [max[0], max[2]]) {
            return false;
        }
        if !self.blocks.is_empty()
            && (0..3).all(|axis| min[axis] >= self.min[axis] && max[axis] <= self.max[axis])
        {
            #[cfg(test)]
            {
                self.geometry_hits += 1;
            }
            return true;
        }
        self.blocks.clear();
        #[cfg(test)]
        {
            self.geometry_misses += 1;
        }
        self.blocks.reserve(cells);
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    let Some(id) = state.world.physics_block_at(x, y, z) else {
                        self.blocks.clear();
                        return false;
                    };
                    if state.registry.collision_boxes.get(id as usize).is_none() {
                        self.blocks.clear();
                        return false;
                    }
                    self.blocks.push(id);
                }
            }
        }
        self.min = min;
        self.max = max;
        true
    }
}

/// Coverage is only a scope fence. Route validity uses the capability-aware
/// world proof; physical caches separately observe wire revisions and pending edits.
pub(super) fn route_covers_columns(v: &VillagerState, min: [i32; 2], max: [i32; 2]) -> bool {
    (min[0].div_euclid(16)..=max[0].div_euclid(16)).all(|x| {
        (min[1].div_euclid(16)..=max[1].div_euclid(16)).all(|z| {
            v.navigation_chunks
                .iter()
                .any(|&(chunk, _)| chunk == ChunkPos { x, z })
        })
    })
}

pub(super) struct MotionSampler<'a> {
    pub(super) live: TickSampler<'a>,
    pub(super) geometry: Option<&'a MotionGeometryCache>,
}

impl mc_physics::BlockSampler for MotionSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> mc_physics::BlockMaterial {
        self.geometry
            .and_then(|geometry| geometry.block_at(x, y, z))
            .map_or_else(
                || self.live.material_at(x, y, z),
                |id| self.live.registry.materials.classify(id),
            )
    }

    fn max_collision_box_y(&self) -> u8 {
        self.live.max_collision_box_y()
    }

    fn collision_boxes_at(
        &self,
        x: i32,
        y: i32,
        z: i32,
        emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
    ) {
        if let Some(boxes) = self
            .geometry
            .and_then(|geometry| geometry.block_at(x, y, z))
            .and_then(|id| self.live.registry.collision_boxes.get(id as usize))
        {
            for &collision_box in boxes {
                emit(collision_box);
            }
        } else {
            self.live.collision_boxes_at(x, y, z, emit);
        }
    }
}
pub(super) fn move_to(
    state: &mut TickState,
    cache: &mut PoiCache,
    slot: EntitySlot,
    v: &mut VillagerState,
    target: Option<BlockPos>,
    exact: bool,
) {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height) = (body.half_width, body.height);
    let mut unchanged = false;
    if let Some(target) = target {
        let new_target = v.navigation_target != Some(target);
        let due = state.tick >= v.navigation_repath_at;
        let changed = route_changed(state, v);
        if new_target || changed {
            v.navigation_search_failed = false;
        }
        unchanged = !changed;
        // Progress is observed only at due checks, never every brain step: a
        // shorter path or a closer next cell means the villager is still
        // walking its route. A pushed or blocked villager stalls and must
        // replan from its actual position instead of keeping a stale route.
        let mut stalled = false;
        if due && !new_target && !changed && !v.navigation_path.is_empty() {
            let next = v.navigation_path.last().copied();
            let dist_sq = next.map(|cell| distance(pos, centre(cell))).unwrap_or(0.0);
            stalled = !(v.navigation_path.len() < v.navigation_progress_len
                || (next == v.navigation_progress_next && dist_sq < v.navigation_progress_dist_sq));
            v.navigation_progress_len = v.navigation_path.len();
            v.navigation_progress_next = next;
            v.navigation_progress_dist_sq = dist_sq;
        }
        // A due route is reused only while progressing; a stalled villager
        // replans from its current position even when the world is unchanged.
        if (new_target || changed || (due && v.navigation_path.is_empty()) || stalled)
            && cache.spend_path(state, Consumer::Brain)
        {
            let from = at(pos);
            let horizontal = from
                .x
                .abs_diff(target.x)
                .max(from.z.abs_diff(target.z))
                .saturating_add(8)
                .clamp(48, 512);
            let vertical = from.y.abs_diff(target.y).saturating_add(4).clamp(16, 512);
            let mut seek_path = std::mem::take(&mut cache.candidate_path);
            let mut ctx = allocator::RouteCtx {
                world: &state.world,
                registry: &state.registry,
                dimension: state.dimension,
                consumer: Consumer::Brain,
                budget: &mut state.tick_budget,
            };
            let found = allocator::find_path_ctx(
                &mut ctx,
                cache,
                pos,
                target,
                width,
                height,
                &mut seek_path,
                exact,
                horizontal,
                vertical,
                true,
            );
            let exhausted = ctx
                .budget
                .allowance(ctx.dimension, ctx.consumer, Kind::PathNode)
                == 0;
            // Preserve only a budget-denied route whose target and geometry
            // remain valid. Successful zero-step arrival clears the old path.
            if found || !exhausted || new_target || changed || v.navigation_path.is_empty() {
                std::mem::swap(&mut seek_path, &mut v.navigation_path);
                v.navigation_target = Some(target);
                v.navigation_search_failed = !found && !exhausted;
                capture_route(state, v, pos, true);
            }
            seek_path.clear();
            cache.candidate_path = seek_path;
            unchanged = !route_changed(state, v);
            // A world change invalidates the route, not the activity's target
            // deadline. Renewing it on every chunk admission freezes a villager
            // at an exhausted path because its next stroll never becomes due.
            if new_target || due {
                v.navigation_repath_at = state.tick + 40;
            }
        } else if changed || new_target {
            // Never follow a stale route while the bounded search is deferred.
            v.navigation_path.clear();
            v.navigation_chunks.clear();
            unchanged = false;
        } else if due && !v.navigation_path.is_empty() {
            // Keep the brain's next wake bounded without rerunning A*.
            v.navigation_repath_at = state.tick + 40;
        }
    } else {
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_chunks.clear();
        v.navigation_search_failed = false;
    }
    let scheduled = cache.activity_for(v.age, v.data.profession, state.day_time);
    move_committed_with_route(state, slot, v, scheduled, unchanged);
}

#[cfg(test)]
pub(super) fn move_committed(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    scheduled: Activity,
) {
    let unchanged = !route_changed(state, v);
    move_committed_with_route(state, slot, v, scheduled, unchanged);
}

/// Physics and the already committed path only: no sensors or route searches.
/// The proof is reused only within this synchronous movement call.
pub(super) fn move_committed_with_route(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    scheduled: Activity,
    unchanged: bool,
) {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height) = (body.half_width, body.height);
    let mut geometry = std::mem::take(&mut v.motion_geometry);
    geometry.begin_route(state, v, width, height, unchanged);
    let next = loop {
        let Some(&cell) = v.navigation_path.last() else {
            break None;
        };
        let standing = geometry.standing(state, v, cell, unchanged);
        let reached = standing.unwrap_or_else(|| centre(cell));
        if (pos[0] - reached[0]).powi(2) + (pos[2] - reached[2]).powi(2) < 0.09
            && (pos[1] - reached[1]).abs() < 0.6
        {
            v.navigation_path.pop();
        } else {
            break standing;
        }
    };
    let motion = resolve_body_motion(
        state,
        slot,
        next,
        walking_speed(v, scheduled),
        v,
        &mut geometry,
        unchanged,
    );
    v.motion_geometry = geometry;
    apply_body_motion(state, slot, motion);
}

pub(super) fn walking_speed(v: &VillagerState, scheduled: Activity) -> f64 {
    match v.activity {
        Activity::Panic => match scheduled {
            Activity::Idle => 10.0,
            Activity::Play => 7.2,
            _ => 6.0,
        },
        Activity::Play => 3.6,
        Activity::Idle => 5.0,
        _ => 3.0,
    }
}

pub(super) fn move_skipped(
    state: &mut TickState,
    slot: EntitySlot,
    terrain_changed: bool,
    merchant_busy: bool,
    scheduled: Activity,
) {
    if state.entities.row(slot).villager.as_ref().unwrap().sleeping {
        return;
    }
    let mut geometry = std::mem::take(state.entities.villager_motion_geometry_mut(slot).unwrap());
    let pos = state.entities.pos(slot);
    let row = state.entities.row(slot);
    let v = row.villager.as_ref().unwrap();
    let effective = if state.tick < v.panic_until {
        Activity::Panic
    } else {
        scheduled
    };
    let unchanged = geometry.begin(
        state,
        v,
        state.entities.body(slot).half_width,
        state.entities.body(slot).height,
    );
    let allowed =
        !merchant_busy && effective == v.activity && (v.navigation_path.is_empty() || unchanged);
    let mut reached = 0;
    let mut next = None;
    let mut has_next = false;
    if allowed {
        for &cell in v.navigation_path.iter().rev() {
            let standing = geometry.standing(state, v, cell, unchanged);
            let point = standing.unwrap_or_else(|| centre(cell));
            if (pos[0] - point[0]).powi(2) + (pos[2] - point[2]).powi(2) < 0.09
                && (pos[1] - point[1]).abs() < 0.6
            {
                reached += 1;
            } else {
                has_next = true;
                next = standing;
                break;
            }
        }
    }
    let speed = walking_speed(v, scheduled);
    let still = !terrain_changed
        && !has_next
        && state.entities.ground(slot)
        && state.entities.vel(slot) == [0.0; 3]
        && state.entities.horizontal_impulse(slot) == [0.0; 2];
    let motion = if still {
        None
    } else {
        resolve_body_motion(state, slot, next, speed, v, &mut geometry, unchanged)
    };
    *state.entities.villager_motion_geometry_mut(slot).unwrap() = geometry;
    if reached != 0 {
        state.entities.advance_villager_path(slot, reached);
    }
    apply_body_motion(state, slot, motion);
}

#[derive(Debug, PartialEq)]
pub(super) struct BodyMotion {
    pub(super) position: [f64; 3],
    pub(super) velocity: [f64; 3],
    pub(super) grounded: bool,
    pub(super) impulse: [f64; 2],
    pub(super) yaw: Option<f32>,
}

pub(super) fn resolve_body_motion(
    state: &TickState,
    slot: EntitySlot,
    next: Option<[f64; 3]>,
    speed: f64,
    v: &VillagerState,
    geometry: &mut MotionGeometryCache,
    unchanged: bool,
) -> Option<BodyMotion> {
    let pos = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let (width, height, impulse) = (
        body.half_width,
        body.height,
        state.entities.horizontal_impulse(slot),
    );
    let mut velocity = state.entities.vel(slot);
    let dt = mc_physics::TICK_SECONDS;
    velocity[1] = (velocity[1] - mc_physics::GRAVITY_BLOCKS_PER_SECOND_SQUARED * dt)
        .max(mc_physics::TERMINAL_VELOCITY_BLOCKS_PER_SECOND);
    let mut horizontal = [0.0; 2];
    if let Some(next) = next {
        let dx = next[0] - pos[0];
        let dz = next[2] - pos[2];
        let length = mth::length2(dx, dz);
        if length > 1e-6 {
            horizontal = [dx / length * speed, dz / length * speed];
        }
        if next[1] > pos[1] + mc_physics::STEP_HEIGHT && state.entities.ground(slot) {
            // JumpControl replaces vertical velocity after gravity: subtracting
            // gravity from the launch impulse prevents clearing a full block.
            velocity[1] = mc_physics::LIVING_JUMP_SPEED_BLOCKS_PER_SECOND;
        }
    }
    velocity = [
        horizontal[0] + impulse[0],
        velocity[1],
        horizontal[1] + impulse[1],
    ];
    let requested = mc_physics::Vec3::new(velocity[0] * dt, velocity[1] * dt, velocity[2] * dt);
    let body = mc_physics::EntityBody {
        position: mc_physics::Vec3::new(pos[0], pos[1], pos[2]),
        velocity: mc_physics::Vec3::ZERO,
        aabb: mc_physics::Aabb {
            half_width: width,
            height,
        },
        on_ground: state.entities.ground(slot),
    };
    let sampled = geometry.sample_motion(state, v, body, requested, unchanged);
    let sampler = MotionSampler {
        live: TickSampler {
            world: &state.world,
            registry: &state.registry,
        },
        geometry: sampled.then_some(&*geometry),
    };
    let fixed = mc_physics::resolve_collision_displacement(
        &sampler,
        body.position,
        body.aabb,
        requested,
        body.on_ground,
        mc_physics::STEP_HEIGHT,
    )?;
    let new = [
        pos[0] + fixed.delta.x,
        pos[1] + fixed.delta.y,
        pos[2] + fixed.delta.z,
    ];
    let drag = if fixed.grounded { 0.91 * 0.6 } else { 0.91 };
    let mut next_impulse = impulse.map(|v| {
        let n = v * drag;
        if n.abs() < 0.06 { 0.0 } else { n }
    });
    if (fixed.delta.x - requested.x).abs() > 1e-7 {
        velocity[0] = 0.0;
        next_impulse[0] = 0.0;
    }
    if (fixed.delta.z - requested.z).abs() > 1e-7 {
        velocity[2] = 0.0;
        next_impulse[1] = 0.0;
    }
    velocity[1] = if fixed.grounded || (fixed.delta.y - requested.y).abs() > 1e-7 {
        0.0
    } else {
        velocity[1] * 0.98
    };
    Some(BodyMotion {
        position: new,
        velocity,
        grounded: fixed.grounded,
        impulse: next_impulse,
        yaw: (horizontal[0] != 0.0 || horizontal[1] != 0.0).then(|| {
            -(mc_physics::mth::atan2(horizontal[0], horizontal[1]) as f32)
                * mc_physics::mth::RAD_TO_DEG
        }),
    })
}

pub(super) fn apply_body_motion(
    state: &mut TickState,
    slot: EntitySlot,
    motion: Option<BodyMotion>,
) {
    if let Some(motion) = motion {
        state.entities.set_pos(slot, motion.position);
        state.entities.set_vel(slot, motion.velocity);
        state.entities.set_ground(slot, motion.grounded);
        state.entities.set_brain_impulse(slot, motion.impulse);
        if let Some(yaw) = motion.yaw {
            state.entities.set_rot(slot, yaw, 0.0);
        }
    }
}
