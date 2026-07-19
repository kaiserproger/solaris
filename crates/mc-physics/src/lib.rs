//! # mc-physics
//!
//! Block physics, collisions, fluids.
//!
//! Part of the Solaris engine.

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const TICK_SECONDS: f64 = 0.05;
pub const GRAVITY_BLOCKS_PER_SECOND_SQUARED: f64 = 32.0;
pub const TERMINAL_VELOCITY_BLOCKS_PER_SECOND: f64 = -78.4;
pub const GROUND_FRICTION: f64 = 0.6;
pub const AIR_DRAG: f64 = 0.98;
pub const WATER_DRAG: f64 = 0.8;
pub const WATER_BUOYANCY_BLOCKS_PER_SECOND_SQUARED: f64 = 7.0;
pub const STEP_HEIGHT: f64 = 0.6;
pub const LIVING_JUMP_SPEED_BLOCKS_PER_SECOND: f64 = 0.419_999_986_886_978_15 / TICK_SECONDS;
pub const ARROW_GRAVITY_BLOCKS_PER_SECOND_SQUARED: f64 = 9.8;
pub const ARROW_DRAG: f64 = 0.99;
pub const ARROW_WATER_DRAG: f64 = 0.6;

const MAX_DISPLACEMENT_PER_STEP: f64 = 16.0;
const MAX_BODY_EXTENT: f64 = 16.0;
const MAX_COLLISION_SCAN_CELLS: usize = 8_192;
const MAX_COLLISION_CANDIDATES: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub half_width: f64,
    pub height: f64,
}

impl Aabb {
    pub const COW: Self = Self {
        half_width: 0.45,
        height: 1.4,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMaterial {
    Air,
    Solid,
    Water,
    Lava,
}

impl BlockMaterial {
    #[must_use]
    pub const fn is_solid(self) -> bool {
        matches!(self, Self::Solid)
    }

    #[must_use]
    pub const fn is_fluid(self) -> bool {
        matches!(self, Self::Water | Self::Lava)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockCollisionHeight(u8);

impl BlockCollisionHeight {
    pub const FULL_BLOCK: Self = Self::from_sixteenths(16);

    #[must_use]
    pub const fn from_sixteenths(height: u8) -> Self {
        assert!(height > 0 && height <= 16);
        Self(height)
    }

    #[must_use]
    pub const fn as_blocks(self) -> f64 {
        self.0 as f64 / 16.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockCollisionBox {
    coordinates: [u8; 6],
}

impl BlockCollisionBox {
    pub const FULL_BLOCK: Self = Self::from_sixteenths(0, 0, 0, 16, 16, 16);

    #[must_use]
    pub const fn from_sixteenths(
        min_x: u8,
        min_y: u8,
        min_z: u8,
        max_x: u8,
        max_y: u8,
        max_z: u8,
    ) -> Self {
        assert!(min_x < max_x && min_y < max_y && min_z < max_z);
        Self {
            coordinates: [min_x, min_y, min_z, max_x, max_y, max_z],
        }
    }

    #[must_use]
    pub const fn coordinates(self) -> [u8; 6] {
        self.coordinates
    }

    fn as_blocks(self) -> [f64; 6] {
        self.coordinates.map(|value| f64::from(value) / 16.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockMaterialIds {
    pub air: u32,
    pub water: Vec<u32>,
    pub lava: Vec<u32>,
    pub passable: Vec<u32>,
    collision_heights: Vec<(u32, BlockCollisionHeight)>,
}

impl BlockMaterialIds {
    #[must_use]
    pub fn new(air: u32, water: Option<u32>, lava: Option<u32>) -> Self {
        let water = match water {
            Some(state) => Vec::from([state]),
            None => Vec::new(),
        };
        let lava = match lava {
            Some(state) => Vec::from([state]),
            None => Vec::new(),
        };
        Self {
            air,
            water,
            lava,
            passable: Vec::new(),
            collision_heights: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_passable(mut self, passable: Vec<u32>) -> Self {
        self.passable = passable;
        self
    }

    #[must_use]
    pub fn with_water_states(mut self, water: Vec<u32>) -> Self {
        if !water.is_empty() {
            self.water = water;
        }
        self
    }

    #[must_use]
    pub fn with_lava_states(mut self, lava: Vec<u32>) -> Self {
        if !lava.is_empty() {
            self.lava = lava;
        }
        self
    }

    #[must_use]
    pub fn with_collision_height(mut self, states: Vec<u32>, height: BlockCollisionHeight) -> Self {
        self.collision_heights
            .extend(states.into_iter().map(|state| (state, height)));
        self
    }

    #[must_use]
    pub fn classify(&self, state_id: u32) -> BlockMaterial {
        if state_id == self.air {
            BlockMaterial::Air
        } else if self.water.contains(&state_id) {
            BlockMaterial::Water
        } else if self.lava.contains(&state_id) {
            BlockMaterial::Lava
        } else if self.passable.contains(&state_id) {
            BlockMaterial::Air
        } else {
            BlockMaterial::Solid
        }
    }

    #[must_use]
    pub fn collision_height(&self, state_id: u32) -> Option<BlockCollisionHeight> {
        if !self.classify(state_id).is_solid() {
            return None;
        }
        self.collision_heights
            .iter()
            .find_map(|&(state, height)| (state == state_id).then_some(height))
            .or(Some(BlockCollisionHeight::FULL_BLOCK))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityBody {
    pub position: Vec3,
    pub velocity: Vec3,
    pub aabb: Aabb,
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsConfig {
    pub tick_seconds: f64,
    pub gravity: f64,
    pub terminal_velocity: f64,
    pub ground_friction: f64,
    pub air_drag: f64,
    pub vertical_air_drag: f64,
    pub water_drag: f64,
    pub water_buoyancy: f64,
    pub step_height: f64,
    pub jump_speed: f64,
    pub stop_on_solid: bool,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            tick_seconds: TICK_SECONDS,
            gravity: GRAVITY_BLOCKS_PER_SECOND_SQUARED,
            terminal_velocity: TERMINAL_VELOCITY_BLOCKS_PER_SECOND,
            ground_friction: GROUND_FRICTION,
            air_drag: AIR_DRAG,
            vertical_air_drag: 1.0,
            water_drag: WATER_DRAG,
            water_buoyancy: WATER_BUOYANCY_BLOCKS_PER_SECOND_SQUARED,
            step_height: STEP_HEIGHT,
            jump_speed: 0.0,
            stop_on_solid: false,
        }
    }
}

impl PhysicsConfig {
    #[must_use]
    pub fn living_entity() -> Self {
        Self {
            jump_speed: LIVING_JUMP_SPEED_BLOCKS_PER_SECOND,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn arrow_projectile() -> Self {
        Self {
            gravity: ARROW_GRAVITY_BLOCKS_PER_SECOND_SQUARED,
            air_drag: ARROW_DRAG,
            vertical_air_drag: ARROW_DRAG,
            water_drag: ARROW_WATER_DRAG,
            water_buoyancy: 0.0,
            step_height: 0.0,
            stop_on_solid: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    pub body: EntityBody,
    pub in_fluid: bool,
    pub horizontal_collision: bool,
}

pub trait BlockSampler {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial;

    /// Largest local `max_y` emitted by [`BlockSampler::collision_boxes_at`],
    /// in sixteenths of a block. Implementations that emit over-height boxes
    /// must override this so collision scans include boxes rooted below a body.
    fn max_collision_box_y(&self) -> u8 {
        16
    }

    fn collision_height_at(&self, x: i32, y: i32, z: i32) -> Option<BlockCollisionHeight> {
        self.material_at(x, y, z)
            .is_solid()
            .then_some(BlockCollisionHeight::FULL_BLOCK)
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        if let Some(height) = self.collision_height_at(x, y, z) {
            emit(BlockCollisionBox::from_sixteenths(
                0, 0, 0, 16, height.0, 16,
            ));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GridPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl GridPos {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn below(self) -> Self {
        Self::new(self.x, self.y - 1, self.z)
    }

    #[must_use]
    pub const fn horizontal_neighbours(self) -> [Self; 4] {
        [
            Self::new(self.x + 1, self.y, self.z),
            Self::new(self.x - 1, self.y, self.z),
            Self::new(self.x, self.y, self.z + 1),
            Self::new(self.x, self.y, self.z - 1),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockUpdateIntent {
    Move { from: GridPos, to: GridPos },
    SpreadFluid { from: GridPos, to: GridPos },
}

pub fn falling_block_intent<S: BlockSampler>(
    pos: GridPos,
    sampler: &S,
) -> Option<BlockUpdateIntent> {
    let below = pos.below();
    matches!(
        sampler.material_at(below.x, below.y, below.z),
        BlockMaterial::Air | BlockMaterial::Water | BlockMaterial::Lava
    )
    .then_some(BlockUpdateIntent::Move {
        from: pos,
        to: below,
    })
}

pub fn fluid_spread_intents<S: BlockSampler>(
    pos: GridPos,
    sampler: &S,
    max_outputs: usize,
) -> Vec<BlockUpdateIntent> {
    if !sampler.material_at(pos.x, pos.y, pos.z).is_fluid() {
        return Vec::new();
    }
    let mut intents = Vec::new();
    let below = pos.below();
    if sampler.material_at(below.x, below.y, below.z) == BlockMaterial::Air {
        intents.push(BlockUpdateIntent::SpreadFluid {
            from: pos,
            to: below,
        });
        return intents;
    }
    for target in pos.horizontal_neighbours() {
        if intents.len() >= max_outputs {
            break;
        }
        if sampler.material_at(target.x, target.y, target.z) == BlockMaterial::Air {
            intents.push(BlockUpdateIntent::SpreadFluid {
                from: pos,
                to: target,
            });
        }
    }
    intents
}

pub fn step_entity<S: BlockSampler>(
    mut body: EntityBody,
    sampler: &S,
    config: PhysicsConfig,
) -> StepResult {
    let was_on_ground = body.on_ground;
    let raw_displacement = Vec3::new(
        body.velocity.x * config.tick_seconds,
        body.velocity.y * config.tick_seconds,
        body.velocity.z * config.tick_seconds,
    );
    let jump_displacement = config.jump_speed * config.tick_seconds;
    if !valid_step_input(body, config)
        || !bounded_displacement(raw_displacement)
        || !jump_displacement.is_finite()
        || jump_displacement.abs() > MAX_DISPLACEMENT_PER_STEP
    {
        return rejected_step(body, false, raw_displacement);
    }

    let in_fluid = body_overlaps_fluid(body, sampler);
    if config.stop_on_solid && body.on_ground && body.velocity == Vec3::ZERO {
        return StepResult {
            body,
            in_fluid,
            horizontal_collision: false,
        };
    }
    if in_fluid {
        body.velocity.y += config.water_buoyancy * config.tick_seconds;
        body.velocity.x *= config.water_drag;
        body.velocity.y *= config.water_drag;
        body.velocity.z *= config.water_drag;
    } else {
        body.velocity.y -= config.gravity * config.tick_seconds;
        body.velocity.x *= config.air_drag;
        body.velocity.y *= config.vertical_air_drag;
        body.velocity.z *= config.air_drag;
    }
    body.velocity.y = body.velocity.y.max(config.terminal_velocity);

    let desired = Vec3::new(
        body.velocity.x * config.tick_seconds,
        body.velocity.y * config.tick_seconds,
        body.velocity.z * config.tick_seconds,
    );
    let start_box = WorldAabb::for_body(body);
    if !bounded_displacement(desired) {
        return rejected_step(body, in_fluid, desired);
    }
    let Some(collision_boxes) =
        collision_boxes_for_motion(sampler, start_box, desired, config.step_height.max(0.0))
    else {
        return rejected_step(body, in_fluid, desired);
    };
    let movement = if config.stop_on_solid {
        resolve_until_first_impact(start_box, desired, &collision_boxes)
    } else {
        resolve_movement(
            start_box,
            desired,
            &collision_boxes,
            was_on_ground,
            config.step_height,
        )
    };
    body.position.x += movement.delta.x;
    body.position.y += movement.delta.y;
    body.position.z += movement.delta.z;

    let clipped_x = axis_was_clipped(desired.x, movement.delta.x);
    let clipped_z = axis_was_clipped(desired.z, movement.delta.z);
    let horizontal_collision = clipped_x || clipped_z;
    if clipped_x {
        body.velocity.x = 0.0;
    }
    if clipped_z {
        body.velocity.z = 0.0;
    }
    if axis_was_clipped(desired.y, movement.delta.y) {
        body.velocity.y = 0.0;
    }

    if movement.collided && config.stop_on_solid {
        body.velocity = Vec3::ZERO;
        body.on_ground = true;
        return StepResult {
            body,
            in_fluid,
            horizontal_collision,
        };
    }

    body.on_ground = movement.grounded;
    let can_start_jump =
        horizontal_collision && !movement.stepped && (was_on_ground || movement.grounded);
    if can_start_jump {
        body.on_ground = true;
        if try_start_jump(
            &mut body,
            desired,
            &collision_boxes,
            false,
            config.jump_speed,
        ) {
            let jump_box = WorldAabb::for_body(body);
            let jump_dy = body.velocity.y * config.tick_seconds;
            let jump_collision_boxes =
                collision_boxes_for_motion(sampler, jump_box, Vec3::new(0.0, jump_dy, 0.0), 0.0);
            let clipped_jump_dy = jump_collision_boxes
                .as_deref()
                .map_or(0.0, |boxes| clip_y(jump_box, jump_dy, boxes));
            body.position.y += clipped_jump_dy;
            if jump_collision_boxes.is_none() || axis_was_clipped(jump_dy, clipped_jump_dy) {
                body.velocity.y = 0.0;
            }
            body.on_ground = false;
        } else {
            body.on_ground = movement.grounded;
        }
    }
    if body.on_ground {
        body.velocity.y = 0.0;
        body.velocity.x *= config.ground_friction;
        body.velocity.z *= config.ground_friction;
    }

    StepResult {
        body,
        in_fluid,
        horizontal_collision,
    }
}

fn valid_step_input(body: EntityBody, config: PhysicsConfig) -> bool {
    let finite_values = [
        body.position.x,
        body.position.y,
        body.position.z,
        body.velocity.x,
        body.velocity.y,
        body.velocity.z,
        body.aabb.half_width,
        body.aabb.height,
        config.tick_seconds,
        config.gravity,
        config.terminal_velocity,
        config.ground_friction,
        config.air_drag,
        config.vertical_air_drag,
        config.water_drag,
        config.water_buoyancy,
        config.step_height,
        config.jump_speed,
    ];
    finite_values.into_iter().all(f64::is_finite)
        && config.tick_seconds > 0.0
        && body.aabb.half_width > 0.0
        && body.aabb.half_width <= MAX_BODY_EXTENT
        && body.aabb.height > 0.0
        && body.aabb.height <= MAX_BODY_EXTENT
}

fn bounded_displacement(displacement: Vec3) -> bool {
    [displacement.x, displacement.y, displacement.z]
        .into_iter()
        .all(|axis| axis.is_finite() && axis.abs() <= MAX_DISPLACEMENT_PER_STEP)
}

fn rejected_step(mut body: EntityBody, in_fluid: bool, displacement: Vec3) -> StepResult {
    body.velocity = Vec3::ZERO;
    StepResult {
        body,
        in_fluid,
        horizontal_collision: !displacement.x.is_finite()
            || !displacement.z.is_finite()
            || displacement.x != 0.0
            || displacement.z != 0.0,
    }
}

#[derive(Debug, Clone, Copy)]
struct WorldAabb {
    min_x: f64,
    min_y: f64,
    min_z: f64,
    max_x: f64,
    max_y: f64,
    max_z: f64,
}

impl WorldAabb {
    fn for_body(body: EntityBody) -> Self {
        Self {
            min_x: body.position.x - body.aabb.half_width,
            min_y: body.position.y,
            min_z: body.position.z - body.aabb.half_width,
            max_x: body.position.x + body.aabb.half_width,
            max_y: body.position.y + body.aabb.height,
            max_z: body.position.z + body.aabb.half_width,
        }
    }

    fn moved(self, delta: Vec3) -> Self {
        Self {
            min_x: self.min_x + delta.x,
            min_y: self.min_y + delta.y,
            min_z: self.min_z + delta.z,
            max_x: self.max_x + delta.x,
            max_y: self.max_y + delta.y,
            max_z: self.max_z + delta.z,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedMovement {
    delta: Vec3,
    grounded: bool,
    stepped: bool,
    collided: bool,
}

#[derive(Debug, Clone, Copy)]
struct SweepHit {
    time: f64,
    blocked_x: bool,
    blocked_y: bool,
    blocked_z: bool,
}

fn collision_boxes_for_motion<S: BlockSampler>(
    sampler: &S,
    body: WorldAabb,
    desired: Vec3,
    step_height: f64,
) -> Option<Vec<WorldAabb>> {
    let min_x = (body.min_x + desired.x.min(0.0)).floor() as i32;
    let max_x = (body.max_x + desired.x.max(0.0)).ceil() as i32 - 1;
    let min_z = (body.min_z + desired.z.min(0.0)).floor() as i32;
    let max_z = (body.max_z + desired.z.max(0.0)).ceil() as i32 - 1;
    let scan_min_y = body.min_y + desired.y.min(-step_height);
    let scan_max_y = body.max_y + desired.y.max(step_height);
    let max_local_y = f64::from(sampler.max_collision_box_y()) / 16.0;
    assert!(max_local_y > 0.0, "collision box max_y must be positive");
    let min_y = (scan_min_y - max_local_y).floor() as i32 + 1;
    let max_y = scan_max_y.ceil() as i32 - 1;
    let scan_cells = inclusive_range_len(min_x, max_x)
        .checked_mul(inclusive_range_len(min_y, max_y))?
        .checked_mul(inclusive_range_len(min_z, max_z))?;
    if scan_cells > MAX_COLLISION_SCAN_CELLS {
        return None;
    }
    let mut boxes = Vec::with_capacity(scan_cells.min(MAX_COLLISION_CANDIDATES));

    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let mut overflowed = false;
                sampler.collision_boxes_at(x, y, z, &mut |collision_box| {
                    if boxes.len() >= MAX_COLLISION_CANDIDATES {
                        overflowed = true;
                        return;
                    }
                    let [
                        box_min_x,
                        box_min_y,
                        box_min_z,
                        box_max_x,
                        box_max_y,
                        box_max_z,
                    ] = collision_box.as_blocks();
                    boxes.push(WorldAabb {
                        min_x: f64::from(x) + box_min_x,
                        min_y: f64::from(y) + box_min_y,
                        min_z: f64::from(z) + box_min_z,
                        max_x: f64::from(x) + box_max_x,
                        max_y: f64::from(y) + box_max_y,
                        max_z: f64::from(z) + box_max_z,
                    });
                });
                if overflowed {
                    return None;
                }
            }
        }
    }

    Some(boxes)
}

fn inclusive_range_len(min: i32, max: i32) -> usize {
    if min > max {
        return 0;
    }
    (i64::from(max) - i64::from(min) + 1) as usize
}

fn resolve_movement(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
    was_on_ground: bool,
    step_height: f64,
) -> ResolvedMovement {
    let base = resolve_axes(body, desired, collision_boxes);
    let base_grounded = desired.y < 0.0 && axis_was_clipped(desired.y, base.y);
    let horizontal_clipped =
        axis_was_clipped(desired.x, base.x) || axis_was_clipped(desired.z, base.z);
    if step_height <= 0.0 || !horizontal_clipped || !(was_on_ground || base_grounded) {
        return ResolvedMovement {
            delta: base,
            grounded: base_grounded,
            stepped: false,
            collided: axis_was_clipped(desired.x, base.x)
                || axis_was_clipped(desired.y, base.y)
                || axis_was_clipped(desired.z, base.z),
        };
    }

    let raised = clip_y(body, step_height, collision_boxes);
    let raised_box = body.moved(Vec3::new(0.0, raised, 0.0));
    let stepped_horizontal = resolve_horizontal_sweep(
        raised_box,
        Vec3::new(desired.x, 0.0, desired.z),
        collision_boxes,
    );
    let after_horizontal = raised_box.moved(stepped_horizontal);
    let requested_down = desired.y - raised;
    let stepped_down = clip_y(after_horizontal, requested_down, collision_boxes);
    let stepped = Vec3::new(
        stepped_horizontal.x,
        raised + stepped_down,
        stepped_horizontal.z,
    );

    if horizontal_distance_squared(stepped) <= horizontal_distance_squared(base) {
        return ResolvedMovement {
            delta: base,
            grounded: base_grounded,
            stepped: false,
            collided: true,
        };
    }

    ResolvedMovement {
        delta: stepped,
        grounded: requested_down < 0.0 && axis_was_clipped(requested_down, stepped_down),
        stepped: true,
        collided: axis_was_clipped(desired.x, stepped.x)
            || axis_was_clipped(desired.y, stepped.y)
            || axis_was_clipped(desired.z, stepped.z),
    }
}

fn resolve_axes(body: WorldAabb, desired: Vec3, collision_boxes: &[WorldAabb]) -> Vec3 {
    let dy = clip_y(body, desired.y, collision_boxes);
    let after_y = body.moved(Vec3::new(0.0, dy, 0.0));
    let horizontal = resolve_horizontal_sweep(
        after_y,
        Vec3::new(desired.x, 0.0, desired.z),
        collision_boxes,
    );
    Vec3::new(horizontal.x, dy, horizontal.z)
}

fn resolve_until_first_impact(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
) -> ResolvedMovement {
    let Some(hit) = first_sweep_hit(body, desired, collision_boxes) else {
        return ResolvedMovement {
            delta: desired,
            grounded: false,
            stepped: false,
            collided: false,
        };
    };

    ResolvedMovement {
        delta: Vec3::new(
            desired.x * hit.time,
            desired.y * hit.time,
            desired.z * hit.time,
        ),
        grounded: desired.y < 0.0 && hit.blocked_y,
        stepped: false,
        collided: true,
    }
}

fn resolve_horizontal_sweep(
    mut body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
) -> Vec3 {
    let mut resolved = Vec3::ZERO;
    let mut remaining = desired;

    // At most two horizontal normals can be removed. A third pass only applies
    // the remaining tangent movement after the second contact.
    for _ in 0..3 {
        let Some(hit) = first_sweep_hit(body, remaining, collision_boxes) else {
            resolved.x += remaining.x;
            resolved.z += remaining.z;
            break;
        };

        let travelled = Vec3::new(remaining.x * hit.time, 0.0, remaining.z * hit.time);
        resolved.x += travelled.x;
        resolved.z += travelled.z;
        body = body.moved(travelled);

        let remainder = 1.0 - hit.time;
        remaining.x *= remainder;
        remaining.z *= remainder;
        if hit.blocked_x {
            remaining.x = 0.0;
        }
        if hit.blocked_z {
            remaining.z = 0.0;
        }
        if remaining.x == 0.0 && remaining.z == 0.0 {
            break;
        }
    }

    Vec3::new(
        if axis_was_clipped(desired.x, resolved.x) {
            0.0
        } else {
            desired.x
        },
        0.0,
        if axis_was_clipped(desired.z, resolved.z) {
            0.0
        } else {
            desired.z
        },
    )
}

fn first_sweep_hit(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
) -> Option<SweepHit> {
    const TIME_EPSILON: f64 = 1.0e-12;

    let mut nearest: Option<SweepHit> = None;
    for obstacle in collision_boxes {
        let Some((entry_x, exit_x)) = sweep_axis_interval(
            body.min_x,
            body.max_x,
            obstacle.min_x,
            obstacle.max_x,
            desired.x,
        ) else {
            continue;
        };
        let Some((entry_y, exit_y)) = sweep_axis_interval(
            body.min_y,
            body.max_y,
            obstacle.min_y,
            obstacle.max_y,
            desired.y,
        ) else {
            continue;
        };
        let Some((entry_z, exit_z)) = sweep_axis_interval(
            body.min_z,
            body.max_z,
            obstacle.min_z,
            obstacle.max_z,
            desired.z,
        ) else {
            continue;
        };

        let entry = entry_x.max(entry_y).max(entry_z);
        let exit = exit_x.min(exit_y).min(exit_z);
        if entry > exit + TIME_EPSILON || exit < 0.0 || !(-TIME_EPSILON..=1.0).contains(&entry) {
            continue;
        }

        let time = entry.max(0.0);
        let hit = SweepHit {
            time,
            blocked_x: desired.x != 0.0 && (entry_x - entry).abs() <= TIME_EPSILON,
            blocked_y: desired.y != 0.0 && (entry_y - entry).abs() <= TIME_EPSILON,
            blocked_z: desired.z != 0.0 && (entry_z - entry).abs() <= TIME_EPSILON,
        };
        if !hit.blocked_x && !hit.blocked_y && !hit.blocked_z {
            continue;
        }

        match &mut nearest {
            Some(current) if (current.time - time).abs() <= TIME_EPSILON => {
                current.blocked_x |= hit.blocked_x;
                current.blocked_y |= hit.blocked_y;
                current.blocked_z |= hit.blocked_z;
            }
            Some(current) if current.time < time => {}
            _ => nearest = Some(hit),
        }
    }
    nearest
}

fn sweep_axis_interval(
    body_min: f64,
    body_max: f64,
    obstacle_min: f64,
    obstacle_max: f64,
    movement: f64,
) -> Option<(f64, f64)> {
    if movement > 0.0 {
        Some((
            (obstacle_min - body_max) / movement,
            (obstacle_max - body_min) / movement,
        ))
    } else if movement < 0.0 {
        Some((
            (obstacle_max - body_min) / movement,
            (obstacle_min - body_max) / movement,
        ))
    } else if body_min < obstacle_max && body_max > obstacle_min {
        Some((f64::NEG_INFINITY, f64::INFINITY))
    } else {
        None
    }
}

fn clip_y(body: WorldAabb, mut dy: f64, collision_boxes: &[WorldAabb]) -> f64 {
    for obstacle in collision_boxes {
        if body.min_x < obstacle.max_x
            && body.max_x > obstacle.min_x
            && body.min_z < obstacle.max_z
            && body.max_z > obstacle.min_z
        {
            if dy > 0.0 && body.max_y <= obstacle.min_y {
                dy = dy.min(obstacle.min_y - body.max_y);
            } else if dy < 0.0 && body.min_y >= obstacle.max_y {
                dy = dy.max(obstacle.max_y - body.min_y);
            }
        }
    }
    dy
}

fn axis_was_clipped(requested: f64, resolved: f64) -> bool {
    (requested - resolved).abs() > 1.0e-12
}

fn horizontal_distance_squared(delta: Vec3) -> f64 {
    delta.x * delta.x + delta.z * delta.z
}

pub fn ground_y_for_body<S: BlockSampler>(body: EntityBody, sampler: &S) -> Option<f64> {
    let feet_y = body.position.y.floor() as i32;
    let (min_x, max_x, min_z, max_z) = body_block_bounds(body);
    let mut highest_top: Option<f64> = None;

    for x in min_x..=max_x {
        for y in [feet_y, feet_y - 1] {
            for z in min_z..=max_z {
                sampler.collision_boxes_at(x, y, z, &mut |collision_box| {
                    if body_overlaps_box_horizontally(body, x, z, collision_box) {
                        let top = f64::from(y) + collision_box.as_blocks()[4];
                        highest_top = Some(highest_top.map_or(top, |current| current.max(top)));
                    }
                });
            }
        }
    }

    highest_top
}

fn body_overlaps_fluid<S: BlockSampler>(body: EntityBody, sampler: &S) -> bool {
    let min_y = body.position.y.floor() as i32;
    let max_y = (body.position.y + body.aabb.height).floor() as i32;
    bbox_columns(body)
        .into_iter()
        .any(|(x, z)| (min_y..=max_y).any(|y| sampler.material_at(x, y, z).is_fluid()))
}

fn body_block_bounds(body: EntityBody) -> (i32, i32, i32, i32) {
    let half = body.aabb.half_width;
    (
        (body.position.x - half).floor() as i32,
        (body.position.x + half - 1.0e-7).floor() as i32,
        (body.position.z - half).floor() as i32,
        (body.position.z + half - 1.0e-7).floor() as i32,
    )
}

fn body_overlaps_box_horizontally(
    body: EntityBody,
    block_x: i32,
    block_z: i32,
    collision_box: BlockCollisionBox,
) -> bool {
    let [min_x, _, min_z, max_x, _, max_z] = collision_box.as_blocks();
    let body_min_x = body.position.x - body.aabb.half_width;
    let body_max_x = body.position.x + body.aabb.half_width;
    let body_min_z = body.position.z - body.aabb.half_width;
    let body_max_z = body.position.z + body.aabb.half_width;
    body_min_x < f64::from(block_x) + max_x
        && body_max_x > f64::from(block_x) + min_x
        && body_min_z < f64::from(block_z) + max_z
        && body_max_z > f64::from(block_z) + min_z
}

fn try_start_jump(
    body: &mut EntityBody,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
    stepped_up: bool,
    jump_speed: f64,
) -> bool {
    if !body.on_ground || stepped_up || jump_speed <= 0.0 {
        return false;
    }
    let body_box = WorldAabb::for_body(*body);
    let raised = clip_y(body_box, 1.0, collision_boxes);
    if axis_was_clipped(1.0, raised) {
        return false;
    }
    let raised_box = body_box.moved(Vec3::new(0.0, raised, 0.0));
    let raised_horizontal = resolve_horizontal_sweep(
        raised_box,
        Vec3::new(desired.x, 0.0, desired.z),
        collision_boxes,
    );
    if axis_was_clipped(desired.x, raised_horizontal.x)
        || axis_was_clipped(desired.z, raised_horizontal.z)
    {
        return false;
    }
    body.velocity.y = body.velocity.y.max(jump_speed);
    true
}

fn bbox_columns(body: EntityBody) -> [(i32, i32); 4] {
    let x = body.position.x;
    let z = body.position.z;
    let half = body.aabb.half_width;
    [
        ((x - half).floor() as i32, (z - half).floor() as i32),
        ((x - half).floor() as i32, (z + half).floor() as i32),
        ((x + half).floor() as i32, (z - half).floor() as i32),
        ((x + half).floor() as i32, (z + half).floor() as i32),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn cow_fallback_aabb_matches_vanilla_adult_dimensions() {
        assert_eq!(
            Aabb::COW,
            Aabb {
                half_width: 0.45,
                height: 1.4,
            }
        );
    }

    struct FlatWorld {
        ground_y: i32,
        water_y: Option<i32>,
    }

    impl BlockSampler for FlatWorld {
        fn material_at(&self, _x: i32, y: i32, _z: i32) -> BlockMaterial {
            if self.water_y == Some(y) {
                BlockMaterial::Water
            } else if y <= self.ground_y {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    struct OneBlockStepWorld;

    impl BlockSampler for OneBlockStepWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if y <= 63 || (x == 1 && y == 64 && z == 0) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    struct FarmlandToFullBlockStepWorld;

    impl BlockSampler for FarmlandToFullBlockStepWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if y == 63 && z == 0 && matches!(x, 0 | 1) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }

        fn collision_height_at(&self, x: i32, y: i32, z: i32) -> Option<BlockCollisionHeight> {
            self.material_at(x, y, z).is_solid().then_some(if x == 0 {
                BlockCollisionHeight::from_sixteenths(15)
            } else {
                BlockCollisionHeight::FULL_BLOCK
            })
        }
    }

    struct BottomSlabEdgeWorld;

    impl BlockSampler for BottomSlabEdgeWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if y <= 62 || (x == 1 && y == 63 && z == 0) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }

        fn collision_boxes_at(
            &self,
            x: i32,
            y: i32,
            z: i32,
            emit: &mut dyn FnMut(BlockCollisionBox),
        ) {
            if y <= 62 {
                emit(BlockCollisionBox::FULL_BLOCK);
            } else if x == 1 && y == 63 && z == 0 {
                emit(BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 8, 16));
            }
        }
    }

    struct IsolatedFenceWorld;

    impl BlockSampler for IsolatedFenceWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if y <= 62 || (x == 1 && y == 63 && z == 0) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }

        fn collision_boxes_at(
            &self,
            x: i32,
            y: i32,
            z: i32,
            emit: &mut dyn FnMut(BlockCollisionBox),
        ) {
            if y <= 62 {
                emit(BlockCollisionBox::FULL_BLOCK);
            } else if x == 1 && y == 63 && z == 0 {
                emit(BlockCollisionBox::from_sixteenths(6, 0, 6, 10, 24, 10));
            }
        }

        fn max_collision_box_y(&self) -> u8 {
            24
        }
    }

    struct StraightBottomStairWorld;

    impl BlockSampler for StraightBottomStairWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if y <= 62 || (x == 1 && y == 63 && z == 0) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }

        fn collision_boxes_at(
            &self,
            x: i32,
            y: i32,
            z: i32,
            emit: &mut dyn FnMut(BlockCollisionBox),
        ) {
            if y <= 62 {
                emit(BlockCollisionBox::FULL_BLOCK);
            } else if x == 1 && y == 63 && z == 0 {
                emit(BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 8, 16));
                emit(BlockCollisionBox::from_sixteenths(0, 8, 0, 16, 16, 8));
            }
        }
    }

    struct SlabUnderLowCeilingWorld;

    impl BlockSampler for SlabUnderLowCeilingWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if y <= 62 || (x == 1 && y == 63 && z == 0) || (x == 1 && y == 64 && z == 0) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }

        fn collision_boxes_at(
            &self,
            x: i32,
            y: i32,
            z: i32,
            emit: &mut dyn FnMut(BlockCollisionBox),
        ) {
            if y <= 62 {
                emit(BlockCollisionBox::FULL_BLOCK);
            } else if x == 1 && y == 63 && z == 0 {
                emit(BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 8, 16));
            } else if x == 1 && y == 64 && z == 0 {
                emit(BlockCollisionBox::from_sixteenths(0, 12, 0, 16, 16, 16));
            }
        }
    }

    struct SweptWallWorld;

    impl BlockSampler for SweptWallWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if y <= 63 || (x == 2 && matches!(y, 64 | 65) && z == 0) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    struct IsolatedCornerWorld {
        x: i32,
        z: i32,
    }

    impl BlockSampler for IsolatedCornerWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if x == self.x && y == 64 && z == self.z {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    struct JumpCeilingWorld;

    impl BlockSampler for JumpCeilingWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if z == 0 && ((x == 0 && y == 63) || (x == 1 && y == 64) || (x == 0 && y == 67)) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    struct ProjectileWallWorld;

    impl BlockSampler for ProjectileWallWorld {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            if x == 2 && y == 64 && matches!(z, 0..=2) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    struct PanicSampler;

    impl BlockSampler for PanicSampler {
        fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
            panic!("invalid movement must be rejected before sampling")
        }
    }

    struct CountingAirWorld {
        material_samples: Cell<usize>,
    }

    impl BlockSampler for CountingAirWorld {
        fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
            self.material_samples
                .set(self.material_samples.get().saturating_add(1));
            BlockMaterial::Air
        }
    }

    struct DenseCollisionWorld {
        emitted: Cell<usize>,
    }

    impl BlockSampler for DenseCollisionWorld {
        fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
            BlockMaterial::Air
        }

        fn collision_boxes_at(
            &self,
            _x: i32,
            _y: i32,
            _z: i32,
            emit: &mut dyn FnMut(BlockCollisionBox),
        ) {
            for _ in 0..1_024 {
                self.emitted.set(self.emitted.get().saturating_add(1));
                emit(BlockCollisionBox::FULL_BLOCK);
            }
        }
    }

    fn unit_tick_config() -> PhysicsConfig {
        PhysicsConfig {
            tick_seconds: 1.0,
            gravity: 0.0,
            air_drag: 1.0,
            vertical_air_drag: 1.0,
            step_height: 0.0,
            ..PhysicsConfig::default()
        }
    }

    #[test]
    fn gravity_lands_body_on_solid_ground() {
        let world = FlatWorld {
            ground_y: 63,
            water_y: None,
        };
        let mut body = EntityBody {
            position: Vec3::new(0.5, 66.0, 0.5),
            velocity: Vec3::ZERO,
            aabb: Aabb::COW,
            on_ground: false,
        };

        for _ in 0..40 {
            body = step_entity(body, &world, PhysicsConfig::default()).body;
        }

        assert_eq!(body.position.y, 64.0);
        assert_eq!(body.velocity.y, 0.0);
        assert!(body.on_ground);
    }

    #[test]
    fn air_drag_damps_horizontal_motion() {
        let world = FlatWorld {
            ground_y: -64,
            water_y: None,
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 70.0, 0.5),
            velocity: Vec3::new(1.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: false,
        };

        let stepped = step_entity(body, &world, PhysicsConfig::default()).body;

        assert!(stepped.velocity.x < body.velocity.x);
        assert!(stepped.velocity.y < 0.0);
    }

    #[test]
    fn horizontal_collision_stops_body_at_wall() {
        let world = LocalBlocks {
            solids: vec![
                GridPos::new(0, 63, 0),
                GridPos::new(1, 64, 0),
                GridPos::new(1, 65, 0),
            ],
            fluids: Vec::new(),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &world, PhysicsConfig::default());
        let stepped = result.body;

        assert!(result.horizontal_collision);
        assert_eq!(stepped.position.x, body.position.x);
        assert_eq!(stepped.velocity.x, 0.0);
        assert_eq!(stepped.velocity.y, 0.0);
        assert!(stepped.on_ground);
    }

    #[test]
    fn swept_motion_stops_before_a_wall_crossed_in_one_tick() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(80.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &SweptWallWorld, PhysicsConfig::default());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert_eq!(result.body.velocity.x, 0.0);
    }

    #[test]
    fn diagonal_swept_motion_slides_along_a_wall() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(80.0, 0.0, 10.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &SweptWallWorld, PhysicsConfig::default());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert!(result.body.position.z > body.position.z);
        assert_eq!(result.body.velocity.x, 0.0);
        assert!(result.body.velocity.z > 0.0);
    }

    #[test]
    fn diagonal_sweep_stops_at_an_isolated_positive_corner() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(2.0, 0.0, 2.0),
            aabb: Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: false,
        };

        let result = step_entity(
            body,
            &IsolatedCornerWorld { x: 1, z: 1 },
            unit_tick_config(),
        );

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert_eq!(result.body.position.z, body.position.z);
        assert_eq!(result.body.velocity.x, 0.0);
        assert_eq!(result.body.velocity.z, 0.0);
    }

    #[test]
    fn diagonal_sweep_stops_at_an_isolated_negative_corner() {
        let body = EntityBody {
            position: Vec3::new(2.5, 64.0, 2.5),
            velocity: Vec3::new(-2.0, 0.0, -2.0),
            aabb: Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: false,
        };

        let result = step_entity(
            body,
            &IsolatedCornerWorld { x: 1, z: 1 },
            unit_tick_config(),
        );

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert_eq!(result.body.position.z, body.position.z);
        assert_eq!(result.body.velocity.x, 0.0);
        assert_eq!(result.body.velocity.z, 0.0);
    }

    #[test]
    fn auto_jump_scans_the_full_vertical_displacement() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(1.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.25,
                height: 1.0,
            },
            on_ground: true,
        };
        let config = PhysicsConfig {
            jump_speed: 4.0,
            ..unit_tick_config()
        };

        let result = step_entity(body, &JumpCeilingWorld, config);

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.y, 66.0);
        assert_eq!(result.body.velocity.y, 0.0);
        assert!(result.body.position.y + result.body.aabb.height <= 67.0);
    }

    #[test]
    fn stop_on_solid_stops_at_the_first_impact_point() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(2.0, 0.0, 1.0),
            aabb: Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: false,
        };
        let config = PhysicsConfig {
            stop_on_solid: true,
            ..unit_tick_config()
        };

        let result = step_entity(body, &ProjectileWallWorld, config);

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, 1.75);
        assert_eq!(result.body.position.z, 1.125);
        assert_eq!(result.body.velocity, Vec3::ZERO);
    }

    #[test]
    fn living_entity_does_not_jump_when_the_raised_path_is_blocked() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(80.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &SweptWallWorld, PhysicsConfig::living_entity());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position, body.position);
        assert_eq!(result.body.velocity, Vec3::ZERO);
        assert!(result.body.on_ground);
    }

    #[test]
    fn non_finite_displacement_is_rejected_before_world_sampling() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(f64::NAN, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &PanicSampler, PhysicsConfig::default());

        assert_eq!(result.body.position, body.position);
        assert_eq!(result.body.velocity, Vec3::ZERO);
    }

    #[test]
    fn oversized_displacement_is_rejected_before_world_sampling() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(17.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &PanicSampler, unit_tick_config());

        assert_eq!(result.body.position, body.position);
        assert_eq!(result.body.velocity, Vec3::ZERO);
        assert!(result.horizontal_collision);
    }

    #[test]
    fn oversized_scan_volume_is_rejected_before_candidate_collection() {
        let world = CountingAirWorld {
            material_samples: Cell::new(0),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(16.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 10.0,
                height: 16.0,
            },
            on_ground: false,
        };

        let result = step_entity(body, &world, unit_tick_config());

        assert_eq!(result.body.position, body.position);
        assert_eq!(result.body.velocity, Vec3::ZERO);
        assert!(world.material_samples.get() <= 100);
    }

    #[test]
    fn collision_candidate_collection_stops_at_its_cap() {
        let world = DenseCollisionWorld {
            emitted: Cell::new(0),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(16.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: false,
        };

        let result = step_entity(body, &world, unit_tick_config());

        assert_eq!(result.body.position, body.position);
        assert_eq!(result.body.velocity, Vec3::ZERO);
        assert!(world.emitted.get() <= 8_192 + 1_024);
    }

    #[test]
    fn arrow_projectile_applies_gravity_and_vertical_drag() {
        let world = FlatWorld {
            ground_y: -64,
            water_y: None,
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 70.0, 0.5),
            velocity: Vec3::new(0.0, 1.0, 0.0),
            aabb: Aabb {
                half_width: 0.25,
                height: 0.25,
            },
            on_ground: false,
        };

        let stepped = step_entity(body, &world, PhysicsConfig::arrow_projectile()).body;

        assert!(stepped.velocity.y < body.velocity.y);
        assert!(stepped.position.y > body.position.y);
    }

    #[test]
    fn arrow_projectile_stops_on_solid_without_step_up() {
        let world = LocalBlocks {
            solids: vec![GridPos::new(1, 64, 0)],
            fluids: Vec::new(),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.25,
                height: 0.25,
            },
            on_ground: true,
        };

        let stepped = step_entity(body, &world, PhysicsConfig::arrow_projectile()).body;

        assert!((stepped.position.x - 0.75).abs() < 1.0e-9);
        let desired_x = body.velocity.x * ARROW_DRAG * TICK_SECONDS;
        let impact_time = (1.0 - (body.position.x + body.aabb.half_width)) / desired_x;
        let desired_y =
            -ARROW_GRAVITY_BLOCKS_PER_SECOND_SQUARED * TICK_SECONDS * ARROW_DRAG * TICK_SECONDS;
        let expected_y = body.position.y + desired_y * impact_time;
        assert!((stepped.position.y - expected_y).abs() < 1.0e-9);
        assert!((stepped.position.z - body.position.z).abs() < 1.0e-9);
        assert_eq!(stepped.velocity, Vec3::ZERO);
        assert!(stepped.on_ground);
    }

    #[test]
    fn stopped_arrow_projectile_remains_stopped() {
        let world = FlatWorld {
            ground_y: 63,
            water_y: None,
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            aabb: Aabb {
                half_width: 0.25,
                height: 0.25,
            },
            on_ground: true,
        };

        let stepped = step_entity(body, &world, PhysicsConfig::arrow_projectile()).body;

        assert_eq!(stepped, body);
    }

    #[test]
    fn living_body_starts_full_block_climb_with_jump() {
        let world = LocalBlocks {
            solids: vec![GridPos::new(0, 63, 0), GridPos::new(1, 64, 0)],
            fluids: Vec::new(),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &world, PhysicsConfig::living_entity());
        let stepped = result.body;

        assert!(result.horizontal_collision);
        assert_eq!(stepped.position.x, body.position.x);
        assert!(
            stepped.position.y > 64.0 && stepped.position.y < 65.0,
            "{stepped:?}"
        );
        assert!((stepped.velocity.y - LIVING_JUMP_SPEED_BLOCKS_PER_SECOND).abs() < 1.0e-9);
        assert!(!stepped.on_ground);
    }

    #[test]
    fn living_body_steps_from_farmland_to_full_block_without_starting_jump() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0 + 15.0 / 16.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 4.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(
            body,
            &FarmlandToFullBlockStepWorld,
            PhysicsConfig::living_entity(),
        );

        assert!(!result.horizontal_collision);
        assert_eq!(result.body.position.y, 64.0);
        assert_eq!(result.body.velocity.y, 0.0);
        assert!(result.body.on_ground);
        assert!(result.body.position.x > body.position.x);
        let expected_z = body.position.z + body.velocity.z * AIR_DRAG * TICK_SECONDS;
        let expected_velocity_z = body.velocity.z * AIR_DRAG * GROUND_FRICTION;
        assert!((result.body.position.z - expected_z).abs() < 1.0e-9);
        assert!((result.body.velocity.z - expected_velocity_z).abs() < 1.0e-9);
    }

    #[test]
    fn living_body_walks_over_vanilla_bottom_slab_height_difference() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &BottomSlabEdgeWorld, PhysicsConfig::living_entity());

        assert!(!result.horizontal_collision);
        assert_eq!(result.body.position.y, 63.5);
        assert!(result.body.position.x > body.position.x);
        assert_eq!(result.body.velocity.y, 0.0);
        assert!(result.body.on_ground);
    }

    #[test]
    fn diagonal_step_path_over_slab_lands_on_support() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 20.0),
            aabb: Aabb {
                half_width: 0.2,
                height: 0.7,
            },
            on_ground: true,
        };

        let result = step_entity(body, &BottomSlabEdgeWorld, PhysicsConfig::living_entity());

        assert!(!result.horizontal_collision);
        assert!((result.body.position.x - 1.48).abs() < 1.0e-9);
        assert!((result.body.position.z - 1.48).abs() < 1.0e-9);
        assert!((result.body.position.y - 63.0).abs() < 1.0e-9);
        assert!(result.body.on_ground);
    }

    #[test]
    fn small_body_can_move_beside_an_isolated_fence_post() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0, 0.1),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.2,
                height: 0.7,
            },
            on_ground: true,
        };

        let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

        assert!(!result.horizontal_collision);
        assert!(result.body.position.x > body.position.x);
        assert_eq!(result.body.position.y, 63.0);
    }

    #[test]
    fn isolated_fence_post_blocks_its_occupied_center() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.2,
                height: 0.7,
            },
            on_ground: true,
        };

        let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert_eq!(result.body.velocity.x, 0.0);
    }

    #[test]
    fn overheight_fence_rooted_below_body_still_blocks_motion() {
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.2,
                height: 0.7,
            },
            on_ground: false,
        };

        let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert_eq!(result.body.velocity.x, 0.0);
    }

    #[test]
    fn swept_motion_cannot_tunnel_through_an_overheight_fence() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0, 0.5),
            velocity: Vec3::new(60.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.2,
                height: 0.7,
            },
            on_ground: true,
        };

        let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert_eq!(result.body.velocity.x, 0.0);
    }

    #[test]
    fn step_path_is_rejected_when_a_low_ceiling_blocks_the_rise() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0, 0.5),
            velocity: Vec3::new(60.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &SlabUnderLowCeilingWorld, PhysicsConfig::default());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert!((result.body.position.y - 63.0).abs() < 1.0e-9);
    }

    #[test]
    fn body_steps_on_lower_tread_of_composed_stair_shape() {
        let body = EntityBody {
            position: Vec3::new(0.5, 63.0, 0.75),
            velocity: Vec3::new(20.0, 0.0, 0.0),
            aabb: Aabb {
                half_width: 0.2,
                height: 0.7,
            },
            on_ground: true,
        };

        let result = step_entity(
            body,
            &StraightBottomStairWorld,
            PhysicsConfig::living_entity(),
        );

        assert!(!result.horizontal_collision);
        assert!(result.body.position.x > body.position.x);
        assert_eq!(result.body.position.y, 63.5);
        assert!(result.body.on_ground);
    }

    #[test]
    fn living_jump_clears_blocked_axis_and_keeps_tangent_motion() {
        let world = LocalBlocks {
            solids: vec![GridPos::new(0, 63, 0), GridPos::new(1, 64, 0)],
            fluids: Vec::new(),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(20.0, 0.0, 4.0),
            aabb: Aabb::COW,
            on_ground: true,
        };

        let result = step_entity(body, &world, PhysicsConfig::living_entity());

        assert!(result.horizontal_collision);
        assert_eq!(result.body.position.x, body.position.x);
        assert!(result.body.position.z > body.position.z);
        assert_eq!(result.body.velocity.x, 0.0);
        assert!(result.body.velocity.z > 0.0);
        assert!(result.body.velocity.y > 0.0);
    }

    #[test]
    fn passive_livestock_finish_climbing_one_block() {
        let livestock = [
            (
                "cow",
                Aabb {
                    half_width: 0.45,
                    height: 1.4,
                },
            ),
            (
                "sheep",
                Aabb {
                    half_width: 0.45,
                    height: 0.9,
                },
            ),
            (
                "chicken",
                Aabb {
                    half_width: 0.2,
                    height: 0.7,
                },
            ),
        ];

        for (name, aabb) in livestock {
            let mut body = EntityBody {
                position: Vec3::new(0.5, 64.0, 0.5),
                velocity: Vec3::ZERO,
                aabb,
                on_ground: true,
            };
            let mut reached_step = false;

            for _ in 0..40 {
                body.velocity.x = 2.0;
                body = step_entity(body, &OneBlockStepWorld, PhysicsConfig::living_entity()).body;
                if body.position.x > 1.0 && body.position.y >= 65.0 {
                    reached_step = true;
                    break;
                }
            }

            assert!(reached_step, "{name} must jump onto a full block");
        }
    }

    #[test]
    fn living_jump_stops_below_solid_ceiling() {
        let world = LocalBlocks {
            solids: vec![GridPos::new(0, 63, 0), GridPos::new(0, 66, 0)],
            fluids: Vec::new(),
        };
        let mut body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(0.0, LIVING_JUMP_SPEED_BLOCKS_PER_SECOND, 0.0),
            aabb: Aabb::COW,
            on_ground: false,
        };

        for _ in 0..4 {
            body = step_entity(body, &world, PhysicsConfig::living_entity()).body;
        }

        assert!(body.position.y + body.aabb.height <= 66.0);
        assert!(body.velocity.y <= 0.0);
    }

    #[test]
    fn water_applies_buoyancy_and_stronger_drag() {
        let world = FlatWorld {
            ground_y: 60,
            water_y: Some(64),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(1.0, -1.0, 0.0),
            aabb: Aabb::COW,
            on_ground: false,
        };

        let result = step_entity(body, &world, PhysicsConfig::default());

        assert!(result.in_fluid);
        assert!(result.body.velocity.x < 1.0);
        assert!(result.body.velocity.y > -1.0);
    }

    #[test]
    fn material_ids_classify_fluids_apart_from_solids() {
        let ids = BlockMaterialIds::new(0, Some(5), Some(6));

        assert_eq!(ids.classify(0), BlockMaterial::Air);
        assert_eq!(ids.classify(5), BlockMaterial::Water);
        assert_eq!(ids.classify(6), BlockMaterial::Lava);
        assert_eq!(ids.classify(7), BlockMaterial::Solid);
    }

    #[test]
    fn material_ids_classify_passable_apart_from_fluids() {
        let ids = BlockMaterialIds::new(0, Some(5), Some(6)).with_passable(vec![6, 7]);

        assert_eq!(ids.classify(5), BlockMaterial::Water);
        assert_eq!(ids.classify(6), BlockMaterial::Lava);
        assert_eq!(ids.classify(7), BlockMaterial::Air);
    }

    #[derive(Debug)]
    struct LocalBlocks {
        solids: Vec<GridPos>,
        fluids: Vec<GridPos>,
    }

    impl BlockSampler for LocalBlocks {
        fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
            let pos = GridPos::new(x, y, z);
            if self.fluids.contains(&pos) {
                BlockMaterial::Water
            } else if self.solids.contains(&pos) {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    #[test]
    fn falling_block_intent_moves_into_replaceable_cell() {
        let world = LocalBlocks {
            solids: vec![GridPos::new(0, 10, 0)],
            fluids: Vec::new(),
        };

        let intent = falling_block_intent(GridPos::new(0, 10, 0), &world);

        assert_eq!(
            intent,
            Some(BlockUpdateIntent::Move {
                from: GridPos::new(0, 10, 0),
                to: GridPos::new(0, 9, 0)
            })
        );
    }

    #[test]
    fn falling_block_intent_distinguishes_support_from_replaceable_targets() {
        let source = GridPos::new(0, 10, 0);
        let supported = LocalBlocks {
            solids: vec![source, source.below()],
            fluids: Vec::new(),
        };
        assert_eq!(falling_block_intent(source, &supported), None);

        let through_water = LocalBlocks {
            solids: vec![source],
            fluids: vec![source.below()],
        };
        assert_eq!(
            falling_block_intent(source, &through_water),
            Some(BlockUpdateIntent::Move {
                from: source,
                to: source.below()
            })
        );
    }

    #[test]
    fn long_fall_clamps_velocity_before_ground_impact() {
        let world = LocalBlocks {
            solids: Vec::new(),
            fluids: Vec::new(),
        };
        let mut body = EntityBody {
            position: Vec3::new(0.5, 120.0, 0.5),
            velocity: Vec3::ZERO,
            aabb: Aabb::COW,
            on_ground: false,
        };

        for _ in 0..200 {
            body = step_entity(body, &world, PhysicsConfig::default()).body;
        }

        assert!(body.velocity.y >= TERMINAL_VELOCITY_BLOCKS_PER_SECOND);
        assert!(body.velocity.y < -10.0);
        assert!(!body.on_ground);
    }

    #[test]
    fn non_fluid_blocks_do_not_emit_spread_intents() {
        let world = LocalBlocks {
            solids: vec![GridPos::new(0, 10, 0)],
            fluids: Vec::new(),
        };

        assert!(fluid_spread_intents(GridPos::new(0, 10, 0), &world, 4).is_empty());
    }

    #[test]
    fn fluid_prefers_downward_spread_before_horizontal() {
        let world = LocalBlocks {
            solids: Vec::new(),
            fluids: vec![GridPos::new(0, 10, 0)],
        };

        let intents = fluid_spread_intents(GridPos::new(0, 10, 0), &world, 4);

        assert_eq!(
            intents,
            vec![BlockUpdateIntent::SpreadFluid {
                from: GridPos::new(0, 10, 0),
                to: GridPos::new(0, 9, 0)
            }]
        );
    }

    #[test]
    fn fluid_horizontal_spread_is_bounded_and_deterministic() {
        let world = LocalBlocks {
            solids: vec![GridPos::new(0, 9, 0)],
            fluids: vec![GridPos::new(0, 10, 0)],
        };

        let intents = fluid_spread_intents(GridPos::new(0, 10, 0), &world, 2);

        assert_eq!(intents.len(), 2);
        assert_eq!(
            intents[0],
            BlockUpdateIntent::SpreadFluid {
                from: GridPos::new(0, 10, 0),
                to: GridPos::new(1, 10, 0)
            }
        );
        assert_eq!(
            intents[1],
            BlockUpdateIntent::SpreadFluid {
                from: GridPos::new(0, 10, 0),
                to: GridPos::new(-1, 10, 0)
            }
        );
    }
}
