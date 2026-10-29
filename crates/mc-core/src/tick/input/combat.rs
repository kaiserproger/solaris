use crate::boundary::{ChangeSet, ConnectionId, EntityId};
use crate::entities::EntitySlot;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::{TickState, apply_player_effects, within_entity_reach};

pub(in crate::tick) fn attack(
    state: &mut TickState,
    (connection, target): (&ConnectionId, &EntityId),
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    let Some(entity) = state.connections.by_connection.get(connection).copied() else {
        return;
    };
    let Some(actor_slot) = state.entities.slot_of(entity) else {
        return;
    };
    if !crate::plugin_player::allows_damage(state, entity, *target) {
        return;
    }
    if let Some(target_slot) = state.entities.slot_of(*target)
        && crate::physical_attacks::try_attack(state, actor_slot, target_slot)
            != crate::physical_attacks::AttackAdmission::NotCustom
    {
        return;
    }
    if !within_entity_reach(state, entity, *target) {
        return;
    }
    if state
        .players
        .get(&entity)
        .is_some_and(|p| !p.spectator() && !p.vitals.is_dead())
        && state
            .entities
            .slot_of(*target)
            .is_some_and(|target| crate::leash::knot_interact(state, target))
    {
        return;
    }
    if crate::armor_stand::attack(state, entity, *target, changes)
        || crate::item_frame::attack(state, entity, *target, changes)
        || crate::painting::attack(state, entity, *target, changes)
    {
        return;
    }
    // Melee pipeline: item-data hit from the held stack; armor
    // reduction + hurt apply at merge against the target's armor.
    // MC-28289: attribute modifiers (attack_damage/attack_speed) resolve
    // from the held stack as of the player's last tick update — a swap in
    // the same tick as the attack still hits with the OLD item's
    // attributes. Enchantments and the weapon component read live from the
    // new item (vanilla reads those at attack time).
    let rules = state.registry.item_rules();
    let held = state
        .players
        .get(&entity)
        .map(|player| {
            player
                .inventory
                .slots
                .get(usize::from(player.inventory.selected.min(8)))
                .cloned()
                .unwrap_or(mc_data::ItemStack::EMPTY)
        })
        .unwrap_or(mc_data::ItemStack::EMPTY);
    let attribute_held = state
        .players
        .get(&entity)
        .and_then(|player| player.attribute_snapshot_held.clone())
        .unwrap_or_else(|| held.clone());
    let since_reset = (state
        .tick
        .saturating_sub(*state.last_swing.get(&entity).unwrap_or(&0))) as u32;
    let falling = state.players[&entity].fall_distance > 0.0;
    let pos = state.entities.pos(actor_slot);
    let wet_or_climbing = state
        .world
        .block_at(
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        )
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| {
            block.block.id.path() == "water"
                || block
                    .properties
                    .iter()
                    .any(|(name, value)| name == "waterlogged" && value == "true")
                || crate::statistics::climbable(state, block.block.raw_id)
        });
    let mace = crate::combat::mace_attack(
        &rules,
        &held,
        state.players[&entity].fall_distance,
        state.entities.row(actor_slot).appearance.pose == 1,
    );
    let mut hit = crate::combat::melee_hit_with_effective_attributes(
        &rules,
        &held,
        &attribute_held,
        crate::combat::Swing {
            ticks_since_reset: since_reset,
            falling,
            on_ground: state.entities.ground(actor_slot),
            sprinting: state.players.get(&entity).is_some_and(|p| p.sprinting),
            crit_blocked: [
                crate::effects::EffectId::Blindness,
                crate::effects::EffectId::Levitation,
                crate::effects::EffectId::SlowFalling,
            ]
            .into_iter()
            .any(|effect| state.players[&entity].active_effects.has(effect))
                || wet_or_climbing
                || crate::mounts::vehicle(state, entity).is_some(),
            melee_bonus: state.players[&entity].active_effects.melee_bonus(),
            attack_speed_multiplier: {
                let effects = &state.players[&entity].active_effects;
                f64::from(
                    (1.0 + effects.level(crate::effects::EffectId::Haste) * 0.1)
                        * (1.0 - effects.level(crate::effects::EffectId::MiningFatigue) * 0.1)
                            .max(0.0),
                )
            },
        },
        state.players[&entity]
            .effective_attribute("attack_damage")
            .zip(state.players[&entity].effective_attribute("attack_speed")),
    );
    if let Some(attack) = mace {
        hit.damage += crate::combat::smash_damage(&held, attack);
        hit.sweep_damage = None;
    }
    hit.damage += crate::enchantments::target_damage(state, *target, &held) * hit.strength;
    let bane_level = crate::enchantments::bane_level(state, *target, &held);
    let mut enchantment_seed = state.players[&entity].enchantment_seed;
    let bane_ticks = crate::enchantments::bane_ticks(bane_level, &mut enchantment_seed);
    let wear = state.players[&entity].plan_held_tool_wear_from_seed(
        &rules,
        hit.weapon_wear as i32,
        enchantment_seed,
    );
    if crate::plugin_inventories::below_player_debit_floors(state, entity, &[], wear.broken_slot())
    {
        return;
    }
    state.last_swing.insert(entity, state.tick);
    // Player.java:1097-1098: movement below 2.5 times the
    // player's base movement_speed (0.1 in this core).
    if state.players[&entity].known_horizontal_movement_squared >= 0.25_f64.powi(2) {
        hit.sweep_damage = None;
    }
    // Weapon wear + attack exhaustion apply with the swing.
    let player = state
        .players
        .get_mut(&entity)
        .expect("prepared melee player");
    if !player.creative() && hit.weapon_wear > 0 {
        player
            .vitals
            .add_exhaustion(crate::survival::ATTACK_EXHAUSTION);
    }
    let effects = player.apply_tool_wear(wear);
    apply_player_effects(state, *connection, entity, effects, changes);
    let seq = state.next_seq(entity);
    let source = if let Some(attack) = mace {
        crate::intent::DamageSource::Mace {
            attacker: entity,
            position: state.entities.pos(actor_slot),
            yaw: state.entities.rot(actor_slot)[0],
            bonus_knockback: hit.bonus_knockback,
            fire_ticks: crate::combat::fire_aspect_ticks(&held),
            bane_ticks,
            attack,
        }
    } else {
        crate::intent::DamageSource::Melee {
            attacker: entity,
            position: state.entities.pos(actor_slot),
            yaw: state.entities.rot(actor_slot)[0],
            bonus_knockback: hit.bonus_knockback,
            sweep: hit.sweep_damage.is_some(),
            fire_ticks: crate::combat::fire_aspect_ticks(&held),
            bane_ticks,
        }
    };
    intents.push(Intent {
        actor: entity,
        seq,
        kind: IntentKind::DamageEntity {
            target: *target,
            amount: hit.damage,
            kind: crate::survival::DamageKind::Melee,
            source: Some(source),
        },
        footprint: Footprint {
            entity_reads: vec![entity],
            entity_writes: vec![*target],
            ..Default::default()
        },
    });
    // Player.java:1203-1229: target box inflation and strict
    // feet-to-feet reach (without the packet prediction buffer).
    if let Some(sweep) = hit.sweep_damage
        && let Some(target_slot) = state.entities.slot_of(*target)
    {
        let target_pos = state.entities.pos(target_slot);
        let target_body = state.entities.body(target_slot);
        let minimum = [
            target_pos[0] - target_body.half_width - 1.0,
            target_pos[1] - 0.25,
            target_pos[2] - target_body.half_width - 1.0,
        ];
        let maximum = [
            target_pos[0] + target_body.half_width + 1.0,
            target_pos[1] + target_body.height + 0.25,
            target_pos[2] + target_body.half_width + 1.0,
        ];
        let position = state.entities.pos(actor_slot);
        let yaw = state.entities.rot(actor_slot)[0];
        let range: f64 = if state.players[&entity].creative() {
            5.0
        } else {
            3.0
        };
        for index in 0..state.entities.len() {
            let slot = EntitySlot(index as u32);
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            if !body.alive
                || body.id == entity
                || body.id == *target
                || crate::taming::excluded_sweep(row)
                || !matches!(
                    body.movement,
                    crate::blueprint::MovementClass::Walker
                        | crate::blueprint::MovementClass::Swimmer
                        | crate::blueprint::MovementClass::Player
                )
            {
                continue;
            }
            let nearby = state.entities.pos(slot);
            let distance: f64 = position
                .into_iter()
                .zip(nearby)
                .map(|(a, b)| (a - b).powi(2))
                .sum();
            if distance >= range * range
                || nearby[0] + body.half_width <= minimum[0]
                || nearby[0] - body.half_width >= maximum[0]
                || nearby[1] + body.height <= minimum[1]
                || nearby[1] >= maximum[1]
                || nearby[2] + body.half_width <= minimum[2]
                || nearby[2] - body.half_width >= maximum[2]
            {
                continue;
            }
            let nearby_id = body.id;
            let target_bonus =
                crate::enchantments::target_damage(state, nearby_id, &held) * hit.strength;
            let bane_level = crate::enchantments::bane_level(state, nearby_id, &held);
            let bane_ticks = crate::enchantments::bane_ticks(
                bane_level,
                &mut state.players.get_mut(&entity).unwrap().enchantment_seed,
            );
            let seq = state.next_seq(entity);
            intents.push(Intent {
                actor: entity,
                seq,
                kind: IntentKind::DamageEntity {
                    target: nearby_id,
                    amount: sweep + target_bonus,
                    kind: crate::survival::DamageKind::Melee,
                    source: Some(crate::intent::DamageSource::Sweep {
                        attacker: entity,
                        primary: *target,
                        position,
                        yaw,
                        fire_ticks: crate::combat::fire_aspect_ticks(&held),
                        bane_ticks,
                    }),
                },
                footprint: Footprint {
                    entity_reads: vec![entity, *target],
                    entity_writes: vec![nearby_id],
                    ..Default::default()
                },
            });
        }
    }
}
