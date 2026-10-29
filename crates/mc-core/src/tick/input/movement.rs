use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
use crate::effects::EffectId;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::{SNEAKING_FLAG, TickSampler, TickState, update_player_crouching};
use mc_physics::mth;

const FLY_KICK_REASON: &str = "Flying is not enabled on this server";

/// Scaffolding collision depends on the player's feet and sneak input, not only
/// the block state. The context-free registry shape is the top platform.
struct MovementSampler<'a> {
    base: TickSampler<'a>,
    feet_y: f64,
    descending: bool,
}

impl mc_physics::BlockSampler for MovementSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> mc_physics::BlockMaterial {
        self.base.material_at(x, y, z)
    }

    fn max_collision_box_y(&self) -> u8 {
        self.base.max_collision_box_y()
    }

    fn collision_boxes_at(
        &self,
        x: i32,
        y: i32,
        z: i32,
        emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
    ) {
        let id = self.base.world.physics_block_at(x, y, z);
        if let Some(block) =
            id.and_then(|id| self.base.registry.blocks.by_id(mc_world::BlockStateId(id)))
            && block.block.id.path() == "scaffolding"
        {
            let above = |height| self.feet_y > f64::from(y) + height - f64::from(1e-5_f32);
            if !above(1.0) || self.descending {
                let prop = |name: &str, value: &str| {
                    block
                        .properties
                        .iter()
                        .any(|(key, val)| key == name && val == value)
                };
                if !prop("distance", "0") && prop("bottom", "true") && above(0.0) {
                    emit(
                        mc_physics::BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 2, 16).unwrap(),
                    );
                }
                return;
            }
        }
        crate::tick::emit_collision_boxes(self.base.registry, id, emit);
    }
}

fn movement_sampler(
    state: &TickState,
    slot: crate::entities::EntitySlot,
    feet_y: f64,
) -> MovementSampler<'_> {
    MovementSampler {
        base: TickSampler {
            world: &state.world,
            registry: &state.registry,
        },
        feet_y,
        descending: state.entities.row(slot).appearance.shared_flags & SNEAKING_FLAG != 0,
    }
}

fn supported(state: &TickState, slot: crate::entities::EntitySlot, position: [f64; 3]) -> bool {
    let body = state.entities.body(slot);
    mc_physics::resolve_collision_displacement(
        &movement_sampler(state, slot, position[1]),
        mc_physics::Vec3::new(position[0], position[1], position[2]),
        mc_physics::Aabb {
            half_width: body.half_width,
            height: body.height,
        },
        mc_physics::Vec3::new(0.0, -0.001, 0.0),
        false,
        0.0,
    )
    .is_some_and(|resolved| resolved.grounded)
}

fn air_time_exception(
    state: &TickState,
    entity: crate::boundary::EntityId,
    slot: crate::entities::EntitySlot,
    position: [f64; 3],
) -> bool {
    let Some(player) = state.players.get(&entity) else {
        return true;
    };
    if player.creative()
        || player.spectator()
        || player.fall_flying
        || player.active_effects.has(EffectId::Levitation)
        || player.active_effects.has(EffectId::SlowFalling)
        || state.entities.vel(slot)[1] > 0.0
    {
        return true;
    }
    let at = |y: f64| {
        state
            .world
            .block_at(
                position[0].floor() as i32,
                y.floor() as i32,
                position[2].floor() as i32,
            )
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
    };
    let water = |y| {
        at(y).is_some_and(|block| {
            crate::fluids::contained(state, block.id.0)
                .is_some_and(|(fluid, _)| fluid == crate::fluids::Fluid::Water)
        })
    };
    water(position[1])
        || water(
            position[1] + crate::tick::player_eye_height(state.entities.row(slot).appearance.pose),
        )
        || at(position[1])
            .is_some_and(|block| crate::statistics::climbable(state, block.block.raw_id))
        || supported(state, slot, position)
}

fn exceeds_air_time(
    state: &mut TickState,
    entity: crate::boundary::EntityId,
    slot: crate::entities::EntitySlot,
    old: [f64; 3],
    new: [f64; 3],
    input: &PlayerInput,
) -> Option<[f64; 3]> {
    if state.max_airtime_ticks == 0 || !matches!(input, PlayerInput::Move { .. }) {
        return None;
    }
    let reset = state.entities.ground(slot)
        || new[1] < old[1]
        || air_time_exception(state, entity, slot, new);
    let player = state.players.get_mut(&entity)?;
    if reset {
        player.airtime_ticks = 0;
        player.airtime_sample_tick = Some(state.tick);
        player.airtime_safe_position = Some(new);
        return None;
    }
    if player.airtime_safe_position.is_none() {
        player.airtime_safe_position = Some(old);
    }
    if player.airtime_sample_tick != Some(state.tick) {
        player.airtime_sample_tick = Some(state.tick);
        player.airtime_ticks = player.airtime_ticks.saturating_add(1);
    }
    (player.airtime_ticks >= state.max_airtime_ticks).then_some(
        player
            .airtime_safe_position
            .expect("airtime sample initialized safe position"),
    )
}

pub(in crate::tick) fn move_input(
    state: &mut TickState,
    connection: &ConnectionId,
    input: &PlayerInput,
    intents: &mut Vec<Intent>,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    if state
        .players
        .get(&entity)
        .is_some_and(|player| player.awaiting_teleport.is_some())
    {
        return;
    }
    let current_position = state.entities.pos(slot);
    let [current_yaw, current_pitch, _] = state.entities.rot(slot);
    let (position, yaw, pitch, on_ground) = match input {
        PlayerInput::Move {
            position,
            yaw,
            pitch,
            on_ground,
            ..
        } => (position, yaw, pitch, on_ground),
        PlayerInput::MoveRotation {
            yaw,
            pitch,
            on_ground,
            ..
        } => (&current_position, yaw, pitch, on_ground),
        PlayerInput::MoveStatus { on_ground, .. } => {
            (&current_position, &current_yaw, &current_pitch, on_ground)
        }
        _ => unreachable!(),
    };
    if !position.iter().all(|value| value.is_finite()) || !yaw.is_finite() || !pitch.is_finite() {
        return;
    }
    if crate::mounts::vehicle(state, entity).is_some() {
        state.entities.set_rot(slot, *yaw, *pitch);
        return;
    }
    crate::tick::elytra::refresh(state, entity);
    let fall_flying = state
        .players
        .get(&entity)
        .is_some_and(|player| player.fall_flying);
    let old = state.entities.pos(slot);
    let was_grounded = state.entities.ground(slot);
    let displacement = [
        position[0] - old[0],
        position[1] - old[1],
        position[2] - old[2],
    ];
    let distance_sq = displacement[0] * displacement[0]
        + displacement[1] * displacement[1]
        + displacement[2] * displacement[2];
    // Local 26.1.2 ServerGamePacketListenerImpl.handleMovePlayer uses a
    // 300 squared-distance allowance for gliding, versus 100 for walking.
    // Rocket acceleration is client-simulated; collision checks still apply.
    let movement_limit = if fall_flying { 300.0 } else { 100.0 };
    if distance_sq > movement_limit {
        state.correct_position(*connection, old, *yaw, *pitch);
        return;
    }
    let sampler = movement_sampler(state, slot, old[1]);
    let body = state.entities.body(slot);
    let half_width = body.half_width;
    let height = body.height;
    let spectator = state
        .players
        .get(&entity)
        .is_some_and(|player| player.spectator());
    let resolved = if spectator {
        None
    } else {
        mc_physics::resolve_collision_displacement(
            &sampler,
            mc_physics::Vec3::new(old[0], old[1], old[2]),
            mc_physics::Aabb { half_width, height },
            mc_physics::Vec3::new(displacement[0], displacement[1], displacement[2]),
            *on_ground,
            mc_physics::STEP_HEIGHT,
        )
    };
    let new_pos = if let Some(fixed) = resolved {
        let fixed_pos = [
            old[0] + fixed.delta.x,
            old[1] + fixed.delta.y,
            old[2] + fixed.delta.z,
        ];
        // A landing that ends exactly on the floor is not clipped by the
        // collision solver (the client already stopped there), and
        // rotation/status/level moves have nothing to clip at all. Validate
        // the claimed support at the final position rather than marking the
        // player airborne, which would carry fall distance across jumps and
        // mine five times slower.
        let grounded = fixed.grounded
            || (*on_ground && displacement[1] <= 0.0 && supported(state, slot, fixed_pos));
        state.entities.set_pos(slot, fixed_pos);
        state.entities.set_rot(slot, *yaw, *pitch);
        state.entities.set_ground(slot, grounded);
        fixed_pos
    } else {
        state.entities.set_pos(slot, *position);
        state.entities.set_rot(slot, *yaw, *pitch);
        state.entities.set_ground(slot, *on_ground);
        *position
    };
    if let Some(safe_position) = exceeds_air_time(state, entity, slot, old, new_pos, input) {
        state.entities.set_pos(slot, safe_position);
        state.entities.set_ground(slot, false);
        state.correct_position(*connection, safe_position, *yaw, *pitch);
        state.push_player(
            *connection,
            PlayerChange::Disconnect {
                reason: FLY_KICK_REASON.to_owned(),
            },
        );
        return;
    }
    crate::physical_defense::accepted_movement(
        state,
        slot,
        mth::length2(new_pos[0] - old[0], new_pos[2] - old[2]),
    );
    if state.entities.ground(slot) && mth::length2(new_pos[0] - old[0], new_pos[2] - old[2]) >= 0.1
    {
        crate::vibration::emit(
            state,
            new_pos,
            crate::vibration::GameEvent::Step,
            Some(entity),
        );
    }
    if let Some(player) = state.players.get_mut(&entity) {
        player.known_horizontal_movement_squared =
            (new_pos[0] - old[0]).powi(2) + (new_pos[2] - old[2]).powi(2);
    }
    let sneaking = state.entities.row(slot).appearance.shared_flags & SNEAKING_FLAG != 0;
    update_player_crouching(state, entity, slot, sneaking);
    crate::statistics::movement(state, entity, slot, old, new_pos, *on_ground);
    if new_pos
        .iter()
        .zip(position)
        .any(|(actual, reported)| (actual - reported).abs() > 0.001)
    {
        state.correct_position(*connection, new_pos, *yaw, *pitch);
    }
    // Charge only accepted movement, not a clipped client prediction.
    let in_water =
        crate::fluids::water_at_point(state, [new_pos[0], new_pos[1] + 0.01, new_pos[2]]);
    if let Some(player) = state.players.get_mut(&entity)
        && !player.creative()
        && !player.spectator()
    {
        let dx = new_pos[0] - old[0];
        let dz = new_pos[2] - old[2];
        let horizontal = (dx * dx + dz * dz).sqrt() as f32;
        let rate = if in_water {
            crate::survival::SWIM_EXHAUSTION_PER_METER
        } else if player.sprinting {
            crate::survival::SPRINT_EXHAUSTION_PER_METER
        } else {
            0.0
        };
        let before = (player.vitals.food, player.vitals.saturation);
        let distance = if in_water {
            let dy = (new_pos[1] - old[1]) as f32;
            (horizontal * horizontal + dy * dy).sqrt()
        } else {
            horizontal
        };
        player.vitals.add_exhaustion(distance * rate);
        if !in_water && was_grounded && new_pos[1] > old[1] && !on_ground {
            player.vitals.add_exhaustion(if player.sprinting {
                crate::survival::SPRINT_JUMP_EXHAUSTION
            } else {
                crate::survival::JUMP_EXHAUSTION
            });
        }
        if before != (player.vitals.food, player.vitals.saturation) {
            let health = player.health_change();
            state.push_player(*connection, health);
        }
    }
    // Landing uses authoritative support; flowing/contained water cancels falls.
    let feet = [
        new_pos[0].floor() as i32,
        new_pos[1].floor() as i32,
        new_pos[2].floor() as i32,
    ];
    let cushioned = state
        .world
        .block_at(feet[0], feet[1], feet[2])
        .is_some_and(|id| {
            in_water
                || state
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(id))
                    .is_some_and(|block| crate::statistics::climbable(state, block.block.raw_id))
        });
    if state.entities.ground(slot) && !cushioned {
        let distance = state.players.get(&entity).map_or(0.0, |player| {
            player.fall_distance + (old[1] - new_pos[1]).max(0.0)
        });
        if let Some(mut intent) = crate::farming::trample_intent(state, entity, new_pos, distance) {
            intent.seq = state.next_seq(entity);
            intents.push(intent);
        }
    }
    let grounded = state.entities.ground(slot);
    let landing_factor = if grounded {
        state
            .world
            .block_at(feet[0], (new_pos[1] - 0.01).floor() as i32, feet[2])
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
            .map_or(1.0, |block| match block.block.id.path() {
                "hay_block" | "honey_block" => 0.2,
                "slime_block" if !sneaking => 0.0,
                name if name.ends_with("_bed") => 0.5,
                _ => 1.0,
            })
    } else {
        1.0
    };
    let damage = state.players.get_mut(&entity).map_or(0.0, |player| {
        let dy = new_pos[1] - old[1];
        if fall_flying && matches!(input, PlayerInput::Move { .. }) && dy > -0.5 {
            player.fall_distance = 0.0;
        }
        (player.fall(dy, grounded, cushioned) * landing_factor).ceil()
    });
    if damage > 0.0 {
        let seq = state.next_seq(entity);
        intents.push(Intent {
            actor: entity,
            seq,
            kind: IntentKind::DamageEntity {
                target: entity,
                amount: damage,
                kind: crate::survival::DamageKind::Fall,
                source: None,
            },
            footprint: Footprint {
                entity_reads: vec![entity],
                entity_writes: vec![entity],
                ..Default::default()
            },
        });
    }
    if grounded {
        crate::tick::elytra::land(state, entity);
    } else {
        crate::tick::elytra::refresh(state, entity);
    }
}

pub(in crate::tick) fn sneak(
    state: &mut TickState,
    (connection, sneaking): (&ConnectionId, &bool),
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    if *sneaking {
        crate::mounts::dismount(state, entity);
    }
    update_player_crouching(state, entity, slot, *sneaking);
}

pub(in crate::tick) fn vehicle_input(
    state: &mut TickState,
    (connection, flags): (&ConnectionId, &u8),
) {
    if let Some(entity) = state.connections.entity_of(*connection)
        && let Some(slot) = state.entities.slot_of(entity)
    {
        let input = if crate::mounts::vehicle(state, entity).is_some() {
            *flags
        } else {
            0
        };
        state.entities.row_mut(slot).entity_life.mount.input = input;
    }
}

pub(in crate::tick) fn move_vehicle(
    state: &mut TickState,
    (connection, position, yaw, pitch, on_ground): (&ConnectionId, &[f64; 3], &f32, &f32, &bool),
) {
    crate::mounts::move_vehicle(state, *connection, *position, *yaw, *pitch, *on_ground);
}

pub(in crate::tick) fn sprint(
    state: &mut TickState,
    (connection, sprinting): (&ConnectionId, &bool),
) {
    if let Some(entity) = state.connections.entity_of(*connection)
        && state.entities.slot_of(entity).is_some()
        && let Some(player) = state.players.get_mut(&entity)
        && player.sprinting != *sprinting
    {
        player.sprinting = *sprinting;
        player.attribute_equipment = None;
    }
}

pub(in crate::tick) fn leave_bed(state: &mut TickState, connection: &ConnectionId) {
    if let Some(entity) = state.connections.by_connection.get(connection).copied() {
        crate::sleep::wake(state, entity);
    }
}
