use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::Arc;

use tokio::sync::mpsc;

use mc_script::precommit::{
    DamageTarget, HookActor, HookContext, HookDecision, HookFailurePolicy, HookKind,
    HookRegistration,
};
use mc_script::{ScriptHostInput, script_boundary_pair};

use mc_entity::projectile_26_1_2::{ARROW_DESPAWN_TICKS, BlockStateId};
use mc_entity::{Rotation, Vec3};

use crate::play::simulation::simulation_channel_with_capacity;
use crate::play::{ArrowPhysicsFact, EntityPhysicsStep, HurtingProjectilePhysicsFact};

use super::entity_lifecycle::spawn_command_entity_locked;
use super::outbound::dispatch_visibility_commands;
use super::projectiles::{
    HurtingProjectileMotionProfile, initial_hurting_projectile_state,
    initial_hurting_projectile_state_with_motion, initial_throwable_projectile_state,
    projectile_identity, spawn_arrow_locked, spawn_throwable_projectile_locked,
};
use super::{OutboundCommand, PlayerPose, SessionRegistry, SimulationAuthority};

#[test]
fn resident_arrow_is_visible_once_to_each_observer_and_hits_once() {
    let registry = SessionRegistry::new();
    registry.configure_arrow_kill_rewards(
        None,
        None,
        Some(77),
        Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
        Arc::new(mc_data::item_components::ItemFactsTable::default()),
        Arc::new(mc_data::loot::LootTables::default()),
    );
    let mut observers = Vec::new();
    for name in ["GuardObserverAlice", "GuardObserverBob"] {
        let (tx, rx) = mpsc::channel(64);
        let (id, initial) = registry.register(
            &crate::login::LoggedInProfile {
                uuid: crate::login::offline_uuid(name),
                name: name.to_owned(),
            },
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        dispatch_visibility_commands(initial);
        dispatch_visibility_commands(registry.mark_loaded(id, (0, 0)));
        observers.push(rx);
    }
    for (name, position) in [
        ("minecraft:villager", Vec3::new(4.5, 64.0, 4.5)),
        ("minecraft:cow", Vec3::new(6.5, 64.0, 4.5)),
    ] {
        dispatch_visibility_commands(registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            2,
            name.to_owned(),
            position,
        ));
    }
    let mut entities = registry
        .lock_entities("read resident archer and target")
        .snapshots()
        .collect::<Vec<_>>();
    entities.sort_unstable_by_key(|snapshot| snapshot.id);
    let (attacker, target) = (&entities[0], &entities[1]);
    let before = target.health;
    for receiver in &mut observers {
        while receiver.try_recv().is_ok() {}
    }
    assert!(registry.launch_resident_arrow(attacker, target));
    let arrows = registry.resident_arrows_for_test(attacker.position);
    let [arrow] = arrows.as_slice() else {
        panic!("one resident arrow is tracked");
    };
    for receiver in &mut observers {
        let commands = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        let spawned = commands
            .iter()
            .filter(|command| {
                matches!(command, OutboundCommand::SpawnEntity(entity) if entity.id == arrow.id)
            })
            .count();
        assert_eq!(
            spawned, 1,
            "each client receives one arrow spawn: {commands:?}"
        );
    }

    registry.apply_entity_physics_with_arrow_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow.id,
            position: Vec3::new(
                arrow.position.x + arrow.velocity.x,
                arrow.position.y + arrow.velocity.y,
                arrow.position.z + arrow.velocity.z,
            ),
            velocity: arrow.velocity,
            on_ground: false,
            horizontal_collision: false,
        }],
        &[ArrowPhysicsFact {
            arrow_id: arrow.id,
            block_hit: None,
            embedded_in_block: false,
            current_block_state: mc_world::BlockStateId(0),
            should_fall: false,
            fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
            in_water: false,
            in_water_or_rain: false,
        }],
    );
    assert!(
        registry
            .server_entity_snapshot(target.id)
            .expect("target remains alive")
            .health
            .is_some_and(|health| health < before)
    );
    for receiver in &mut observers {
        let mut hurt = 0;
        let mut health = 0;
        let mut despawn = 0;
        while let Ok(command) = receiver.try_recv() {
            match command {
                OutboundCommand::EntityHurt { entity_id } if entity_id == target.id.0 => hurt += 1,
                OutboundCommand::UpdateEntityHealth(snapshot) if snapshot.id == target.id => {
                    health += 1
                }
                OutboundCommand::DespawnEntity(snapshot) if snapshot.id == arrow.id => despawn += 1,
                _ => {}
            }
        }
        assert_eq!(
            (hurt, health, despawn),
            (1, 1, 1),
            "one shared projectile outcome per client"
        );
    }
    assert!(
        !registry.launch_resident_arrow(attacker, target),
        "a changed target snapshot cannot launch a second projectile"
    );
    assert!(
        registry
            .resident_arrows_for_test(attacker.position)
            .is_empty()
    );
}

#[test]
fn snowball_uses_throwable_item_gravity_and_survives_a_miss_tick() {
    let registry = SessionRegistry::new();
    let snowball_id;
    {
        let mut inner = registry.lock_session_entities("seed snowball flight");
        snowball_id = spawn_throwable_projectile_locked(
            &mut inner,
            None,
            120,
            "minecraft:snowball",
            Vec3::new(0.5, 64.0, 0.5),
            Vec3::new(0.1, 0.0, 0.0),
            Rotation::ZERO,
        )
        .0;
    }

    registry.apply_entity_physics_with_throwable_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: snowball_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, -0.03, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: snowball_id,
            block_hit: None,
            in_water: false,
        }],
    );

    let snapshot = registry
        .server_entity_snapshot(snowball_id)
        .expect("snowball survives a miss tick");
    let gravity_scaled = 0.03 * 0.99;
    assert!(
        (snapshot.velocity.y + gravity_scaled).abs() < 1e-9,
        "snowballs must fall on the 0.03 throwable-item gravity, got {}",
        snapshot.velocity.y
    );
    assert!(
        (snapshot.velocity.y + 0.05 * 0.99).abs() > 0.01,
        "snowballs must not ride the witch potion 0.05 gravity"
    );
}

#[test]
fn snowball_discards_on_entity_hit_without_potion_effects() {
    let registry = SessionRegistry::new();
    let (snowball_id, cow_id, cow_health);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed snowball entity hit");
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        snowball_id = spawn_throwable_projectile_locked(
            &mut inner,
            None,
            120,
            "minecraft:snowball",
            Vec3::new(0.5, 64.0, 0.5),
            Vec3::new(0.1, 0.0, 0.0),
            Rotation::ZERO,
        )
        .0;
    }

    registry.apply_entity_physics_with_throwable_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: snowball_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.099, -0.0297, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: snowball_id,
            block_hit: None,
            in_water: false,
        }],
    );

    assert!(
        registry.server_entity_snapshot(snowball_id).is_none(),
        "snowball is discarded after an entity impact"
    );
    let cow = registry
        .lock_entities("inspect snowball entity hit")
        .snapshot(cow_id)
        .expect("cow exists");
    assert_eq!(cow.health, cow_health, "snowballs carry no potion payload");
}

#[test]
fn grounded_arrow_ages_in_a_dense_entity_chunk() {
    let registry = SessionRegistry::new();
    let arrow_id;
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed dense grounded arrow");
        arrow_id = spawn_arrow_locked(
            &mut inner,
            None,
            1,
            Vec3::new(0.5, 64.0, 0.5),
            Vec3::ZERO,
            Rotation::ZERO,
        )
        .0;
        for ordinal in 0..129 {
            spawn_command_entity_locked(
                &mut inner,
                4,
                "minecraft:cow".to_owned(),
                Vec3::new(1.0 + f64::from(ordinal) * 0.01, 64.0, 0.5),
                &mob_behaviors,
            );
        }

        let expected = inner.entities.snapshot(arrow_id).expect("arrow exists");
        let mut grounded = expected.clone();
        let state = grounded
            .retained
            .arrow_state
            .as_mut()
            .expect("arrow has projectile state");
        state.in_ground = true;
        state.despawn_age = ARROW_DESPAWN_TICKS - 1;
        state.last_block_state = Some(BlockStateId::new(1));
        assert!(
            inner
                .entities
                .replace_snapshot_if_current(expected, grounded)
        );
    }

    registry.apply_entity_physics_with_arrow_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_id,
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: true,
        }],
        &[ArrowPhysicsFact {
            arrow_id,
            block_hit: None,
            embedded_in_block: true,
            current_block_state: mc_world::BlockStateId(1),
            should_fall: false,
            fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
            in_water: false,
            in_water_or_rain: false,
        }],
    );

    assert!(registry.server_entity_snapshot(arrow_id).is_none());
}

#[test]
fn grounded_arrows_share_one_owner_commit() {
    let registry = SessionRegistry::new();
    let mut arrow_ids = Vec::new();
    {
        let mut inner = registry.lock_session_entities("seed grounded arrow batch");
        for ordinal in 0..5 {
            let arrow_id = spawn_arrow_locked(
                &mut inner,
                None,
                1,
                Vec3::new(0.5 + f64::from(ordinal), 64.0, 0.5),
                Vec3::ZERO,
                Rotation::ZERO,
            )
            .0;
            let expected = inner.entities.snapshot(arrow_id).expect("arrow exists");
            let mut grounded = expected.clone();
            let state = grounded
                .retained
                .arrow_state
                .as_mut()
                .expect("arrow has projectile state");
            state.in_ground = true;
            state.last_block_state = Some(BlockStateId::new(1));
            assert!(
                inner
                    .entities
                    .replace_snapshot_if_current(expected, grounded)
            );
            arrow_ids.push(arrow_id);
        }
    }

    let steps = arrow_ids
        .iter()
        .enumerate()
        .map(|(ordinal, &id)| EntityPhysicsStep {
            id,
            position: Vec3::new(0.5 + ordinal as f64, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: true,
        })
        .collect::<Vec<_>>();
    let facts = arrow_ids
        .iter()
        .map(|&arrow_id| ArrowPhysicsFact {
            arrow_id,
            block_hit: None,
            embedded_in_block: true,
            current_block_state: mc_world::BlockStateId(1),
            should_fall: false,
            fall_velocity_scale: Vec3::new(0.1, 0.1, 0.1),
            in_water: false,
            in_water_or_rain: false,
        })
        .collect::<Vec<_>>();

    registry.reset_entity_owner_requests_for_test();
    registry.apply_entity_physics_with_arrow_facts_and_dispatch(1, &steps, &facts);

    assert_eq!(
        registry.entity_owner_requests_for_test(),
        4,
        "owner traffic must stay constant for the whole grounded-arrow batch"
    );
    for arrow_id in arrow_ids {
        let snapshot = registry
            .lock_entities("inspect grounded arrow batch")
            .snapshot(arrow_id)
            .expect("grounded arrow remains");
        assert_eq!(
            snapshot
                .retained
                .arrow_state
                .expect("arrow state")
                .despawn_age,
            1
        );
    }
}

#[test]
fn breeze_wind_charge_hit_deals_one_damage_and_arms_trigger_explosion() {
    let registry = SessionRegistry::new();
    let (projectile_id, cow_id, cow_health);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed breeze wind charge entity hit");
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        projectile_id = spawn_command_entity_locked(
            &mut inner,
            115,
            "minecraft:breeze_wind_charge".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner
            .entities
            .snapshot(projectile_id)
            .expect("breeze wind charge exists");
        let state = initial_hurting_projectile_state_with_motion(
            None,
            "minecraft:breeze_wind_charge",
            expected.position,
            Vec3::ZERO,
            Rotation::ZERO,
            HurtingProjectileMotionProfile {
                acceleration_power: 0.0,
                air_inertia: 1.0,
                water_inertia: 1.0,
            },
        )
        .expect("valid breeze wind charge state")
        .retarget_velocity(mc_entity::projectile_26_1_2::Vec3::new(0.7, 0.0, 0.0))
        .expect("valid breeze wind charge velocity");
        let mut next = expected.clone();
        next.velocity = Vec3::new(0.7, 0.0, 0.0);
        next.retained.hurting_projectile_state = Some(state);
        next.retained.pending_explosion = mc_entity::EntityPendingExplosionState::new(
            u64::MAX,
            3.0,
            mc_entity::EntityExplosionInteraction::Trigger,
            false,
            0,
        );
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: projectile_id,
            position: Vec3::new(1.2, 64.0, 0.5),
            velocity: Vec3::new(0.7, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id,
            block_hit: None,
            in_water: false,
        }],
    );

    let cow = registry
        .lock_entities("inspect breeze wind charge entity hit")
        .snapshot(cow_id)
        .expect("cow survives breeze wind charge direct hit");
    assert_eq!(cow.health, cow_health - 1.0);
    let armed = registry
        .lock_entities("inspect armed breeze wind charge")
        .snapshot(projectile_id)
        .expect("wind charge remains until explosion owner claims it");
    assert!(
        armed
            .retained
            .pending_explosion
            .is_some_and(|explosion| explosion.expires_tick != u64::MAX)
    );

    let mut expired =
        registry.claim_due_primed_tnt(&crate::play::simulation::SimulationAuthority::for_test(), 1);
    assert_eq!(expired.len(), 1);
    let explosion = expired.pop().expect("armed wind charge explosion");
    assert_eq!(explosion.entity_id, projectile_id);
    assert_eq!(explosion.power(), 3.0);
    assert!(!explosion.destroys_blocks());
    assert!(!explosion.damages_entities());
    assert!(registry.server_entity_snapshot(projectile_id).is_none());
}

#[test]
fn dragon_fireball_hit_spawns_breath_cloud_without_direct_damage_and_discards() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_area_effect_cloud_entity_type(Some(3));
    let (fireball_id, cow_id, cow_health, dragon_id);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed dragon fireball entity hit");
        dragon_id = spawn_command_entity_locked(
            &mut inner,
            43,
            "minecraft:ender_dragon".to_owned(),
            Vec3::new(-10.0, 70.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        fireball_id = spawn_command_entity_locked(
            &mut inner,
            37,
            "minecraft:dragon_fireball".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner
            .entities
            .snapshot(fireball_id)
            .expect("dragon fireball exists");
        let state = initial_hurting_projectile_state(
            Some(projectile_identity(dragon_id)),
            "minecraft:dragon_fireball",
            expected.position,
            Vec3::new(1.0, 0.0, 0.0),
            Rotation::ZERO,
        )
        .expect("valid dragon fireball state");
        let mut next = expected.clone();
        next.velocity = Vec3::new(
            state.projectile.velocity.x,
            state.projectile.velocity.y,
            state.projectile.velocity.z,
        );
        next.retained.hurting_projectile_state = Some(state);
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: fireball_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: fireball_id,
            block_hit: None,
            in_water: false,
        }],
    );

    let cow = registry
        .lock_entities("inspect dragon fireball entity hit")
        .snapshot(cow_id)
        .expect("cow survives dragon fireball impact");
    assert_eq!(
        cow.health, cow_health,
        "dragon fireball impact has no direct damage"
    );
    assert!(registry.server_entity_snapshot(fireball_id).is_none());
    let cloud = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:area_effect_cloud")
        .expect("dragon fireball spawns breath cloud")
        .snapshot;
    let state = cloud
        .retained
        .dragon_breath_cloud
        .expect("dragon breath cloud retained state");
    assert_eq!(state.owner_entity_id, dragon_id.0);
    assert_eq!(state.duration_ticks, 600);
    assert_eq!(state.radius, 3.0);
    assert_eq!(state.amplifier, 1);
    assert_eq!(state.reapplication_delay_ticks, 20);
}

#[test]
fn large_fireball_hit_deals_six_damage_and_arms_mob_explosion() {
    let registry = SessionRegistry::new();
    let (fireball_id, cow_id, cow_health);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed large fireball entity hit");
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        fireball_id = spawn_command_entity_locked(
            &mut inner,
            112,
            "minecraft:fireball".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner
            .entities
            .snapshot(fireball_id)
            .expect("large fireball exists");
        let state = initial_hurting_projectile_state(
            None,
            "minecraft:fireball",
            expected.position,
            Vec3::new(1.0, 0.0, 0.0),
            Rotation::ZERO,
        )
        .expect("valid large fireball state");
        let mut next = expected.clone();
        next.velocity = Vec3::new(
            state.projectile.velocity.x,
            state.projectile.velocity.y,
            state.projectile.velocity.z,
        );
        next.retained.hurting_projectile_state = Some(state);
        next.retained.pending_explosion = mc_entity::EntityPendingExplosionState::new(
            u64::MAX,
            1.0,
            mc_entity::EntityExplosionInteraction::Mob,
            true,
            0,
        );
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: fireball_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: fireball_id,
            block_hit: None,
            in_water: false,
        }],
    );

    let cow = registry
        .lock_entities("inspect large fireball entity hit")
        .snapshot(cow_id)
        .expect("cow survives large fireball direct hit");
    assert_eq!(cow.health, cow_health - 6.0);
    let armed = registry
        .lock_entities("inspect armed large fireball")
        .snapshot(fireball_id)
        .expect("large fireball remains until explosion owner claims it");
    assert!(
        armed
            .retained
            .pending_explosion
            .is_some_and(|explosion| explosion.expires_tick != u64::MAX)
    );

    let mut expired =
        registry.claim_due_primed_tnt(&crate::play::simulation::SimulationAuthority::for_test(), 1);
    assert_eq!(expired.len(), 1);
    let explosion = expired.pop().expect("armed large fireball explosion");
    assert_eq!(explosion.entity_id, fireball_id);
    assert_eq!(explosion.power(), 1.0);
    assert!(explosion.destroys_blocks());
    assert!(explosion.damages_entities());
    assert!(registry.server_entity_snapshot(fireball_id).is_none());
}

#[test]
fn wither_skull_hit_deals_eight_damage_and_arms_mob_explosion() {
    let registry = SessionRegistry::new();
    let (skull_id, cow_id, cow_health);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed wither skull entity hit");
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        skull_id = spawn_command_entity_locked(
            &mut inner,
            151,
            "minecraft:wither_skull".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner
            .entities
            .snapshot(skull_id)
            .expect("wither skull exists");
        let state = initial_hurting_projectile_state(
            None,
            "minecraft:wither_skull",
            expected.position,
            Vec3::new(1.0, 0.0, 0.0),
            Rotation::ZERO,
        )
        .expect("valid wither skull state");
        let mut next = expected.clone();
        next.velocity = Vec3::new(
            state.projectile.velocity.x,
            state.projectile.velocity.y,
            state.projectile.velocity.z,
        );
        next.retained.hurting_projectile_state = Some(state);
        next.retained.pending_explosion = mc_entity::EntityPendingExplosionState::new(
            u64::MAX,
            1.0,
            mc_entity::EntityExplosionInteraction::Mob,
            true,
            0,
        );
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: skull_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: skull_id,
            block_hit: None,
            in_water: false,
        }],
    );

    let cow = registry
        .lock_entities("inspect wither skull entity hit")
        .snapshot(cow_id)
        .expect("cow survives wither skull direct hit");
    assert_eq!(cow.health, cow_health - 8.0);
    let armed = registry
        .lock_entities("inspect armed wither skull")
        .snapshot(skull_id)
        .expect("wither skull remains until explosion owner claims it");
    assert!(
        armed
            .retained
            .pending_explosion
            .is_some_and(|explosion| explosion.expires_tick != u64::MAX)
    );

    let mut expired =
        registry.claim_due_primed_tnt(&crate::play::simulation::SimulationAuthority::for_test(), 1);
    assert_eq!(expired.len(), 1);
    let explosion = expired.pop().expect("armed wither skull explosion");
    assert_eq!(explosion.entity_id, skull_id);
    assert_eq!(explosion.power(), 1.0);
    assert!(explosion.destroys_blocks());
    assert!(explosion.damages_entities());
    assert!(registry.server_entity_snapshot(skull_id).is_none());
}

#[test]
fn witch_harming_potion_uses_throwable_kernel_for_six_damage_and_discards() {
    let registry = SessionRegistry::new();
    let (potion_id, cow_id, cow_health);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed witch throwable entity hit");
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        potion_id = spawn_command_entity_locked(
            &mut inner,
            116,
            "minecraft:splash_potion".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner
            .entities
            .snapshot(potion_id)
            .expect("witch potion exists");
        let state = initial_throwable_projectile_state(
            None,
            "minecraft:splash_potion",
            expected.position,
            Vec3::new(0.1, 0.0, 0.0),
            Rotation::ZERO,
        )
        .expect("valid witch potion state");
        let mut next = expected.clone();
        next.velocity = Vec3::new(0.1, 0.0, 0.0);
        next.retained.throwable_projectile_state = Some(state);
        next.retained.witch_potion = Some(mc_entity::EntityWitchPotionKind::Harming);
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_throwable_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: potion_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.099, -0.0495, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: potion_id,
            block_hit: None,
            in_water: false,
        }],
    );

    let cow = registry
        .lock_entities("inspect witch throwable entity hit")
        .snapshot(cow_id)
        .expect("cow survives harming potion direct hit");
    assert_eq!(cow.health, cow_health - 6.0);
    assert!(registry.server_entity_snapshot(potion_id).is_none());
}

#[test]
fn small_fireball_kernel_hits_entity_for_five_damage_and_discards() {
    let registry = SessionRegistry::new();
    let (fireball_id, cow_id, cow_health);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed small fireball entity hit");
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        fireball_id = spawn_command_entity_locked(
            &mut inner,
            93,
            "minecraft:small_fireball".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner
            .entities
            .snapshot(fireball_id)
            .expect("fireball exists");
        let state = initial_hurting_projectile_state(
            None,
            "minecraft:small_fireball",
            expected.position,
            Vec3::new(1.0, 0.0, 0.0),
            Rotation::ZERO,
        )
        .expect("valid small fireball state");
        let mut next = expected.clone();
        next.velocity = Vec3::new(
            state.projectile.velocity.x,
            state.projectile.velocity.y,
            state.projectile.velocity.z,
        );
        next.retained.hurting_projectile_state = Some(state);
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: fireball_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: fireball_id,
            block_hit: None,
            in_water: false,
        }],
    );

    assert!(
        registry.server_entity_snapshot(fireball_id).is_none(),
        "small fireball is discarded after an entity impact"
    );
    let cow = registry
        .lock_entities("inspect small fireball entity hit")
        .snapshot(cow_id)
        .expect("cow survives the five-damage hit");
    assert_eq!(cow.health, cow_health - 5.0);
    assert_eq!(
        cow.retained.remaining_fire_ticks,
        5 * mc_entity::fire_26_1_2::TICKS_PER_SECOND
    );
}

#[test]
fn shulker_bullet_entity_hit_deals_four_damage_and_adds_levitation() {
    let registry = SessionRegistry::new();
    let (bullet_id, cow_id, cow_health);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed shulker bullet entity hit");
        cow_id = spawn_command_entity_locked(
            &mut inner,
            11,
            "minecraft:cow".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        cow_health = inner.entities.snapshot(cow_id).expect("cow exists").health;
        bullet_id = spawn_command_entity_locked(
            &mut inner,
            113,
            "minecraft:shulker_bullet".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner.entities.snapshot(bullet_id).expect("bullet exists");
        let state = initial_hurting_projectile_state_with_motion(
            None,
            "minecraft:shulker_bullet",
            expected.position,
            Vec3::new(0.1, 0.0, 0.0),
            Rotation::ZERO,
            HurtingProjectileMotionProfile {
                acceleration_power: 0.0,
                air_inertia: 1.0,
                water_inertia: 1.0,
            },
        )
        .expect("valid shulker bullet state");
        let mut next = expected.clone();
        next.velocity = Vec3::new(0.1, 0.0, 0.0);
        next.retained.hurting_projectile_state = Some(state);
        next.retained.shulker_bullet = Some(mc_entity::EntityShulkerBulletState::new(cow_id.0));
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: bullet_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: bullet_id,
            block_hit: None,
            in_water: false,
        }],
    );

    assert!(registry.server_entity_snapshot(bullet_id).is_none());
    let cow = registry
        .lock_entities("inspect shulker bullet entity hit")
        .snapshot(cow_id)
        .expect("cow survives shulker bullet");
    assert_eq!(cow.health, cow_health - 4.0);
    let effects = cow.retained.active_effects.expect("levitation retained");
    let levitation = effects
        .effects
        .chains
        .iter()
        .find(|chain| chain.current.id.raw() == 24)
        .expect("levitation effect id 24");
    assert_eq!(levitation.current.duration, 200);
    assert_eq!(levitation.current.amplifier, 0);
}

#[test]
fn lethal_small_fireball_entity_hit_uses_projectile_kill_rewards() {
    let registry = SessionRegistry::new();
    registry.configure_arrow_kill_rewards(
        None,
        Some(99),
        None,
        std::sync::Arc::new(mc_data::items::solaris_required_items()),
        std::sync::Arc::new(mc_data::item_components::solaris_required_item_facts()),
        std::sync::Arc::new(mc_data::loot::builtin().clone()),
    );
    let (fireball_id, chicken_id);
    {
        let mob_behaviors = registry.mob_behavior_table();
        let mut inner = registry.lock_session_entities("seed lethal small fireball hit");
        chicken_id = spawn_command_entity_locked(
            &mut inner,
            10,
            "minecraft:chicken".to_owned(),
            Vec3::new(1.0, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        assert!(
            inner
                .entities
                .snapshot(chicken_id)
                .expect("chicken exists")
                .health
                <= 5.0
        );
        fireball_id = spawn_command_entity_locked(
            &mut inner,
            93,
            "minecraft:small_fireball".to_owned(),
            Vec3::new(0.5, 64.0, 0.5),
            &mob_behaviors,
        )
        .0;
        let expected = inner
            .entities
            .snapshot(fireball_id)
            .expect("fireball exists");
        let state = initial_hurting_projectile_state(
            None,
            "minecraft:small_fireball",
            expected.position,
            Vec3::new(1.0, 0.0, 0.0),
            Rotation::ZERO,
        )
        .expect("valid small fireball state");
        let mut next = expected.clone();
        next.velocity = Vec3::new(
            state.projectile.velocity.x,
            state.projectile.velocity.y,
            state.projectile.velocity.z,
        );
        next.retained.hurting_projectile_state = Some(state);
        assert!(inner.entities.replace_snapshot_if_current(expected, next));
    }

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: fireball_id,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: fireball_id,
            block_hit: None,
            in_water: false,
        }],
    );

    let records = registry.persisted_entity_records();
    let chicken = records
        .iter()
        .find(|record| record.snapshot.id == chicken_id)
        .expect("dying chicken remains authoritative");
    assert_eq!(
        chicken.snapshot.lifecycle,
        mc_entity::EntityLifecycle::Despawning
    );
    assert!(records.iter().any(|record| {
        record.snapshot.type_name == "minecraft:experience_orb"
            && record.snapshot.type_id == 99
            && record.snapshot.experience_value == Some(1)
    }));
}

fn projectile_damage_boundary() -> (mc_script::ScriptBoundary, mc_script::ScriptHostEndpoint) {
    let (boundary, endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "projectile-judge",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-damage hook");
    (boundary, endpoint)
}

fn seed_small_fireball_entity_impact(
    registry: &SessionRegistry,
) -> (mc_entity::EntityId, mc_entity::EntityId, f32) {
    let mob_behaviors = registry.mob_behavior_table();
    let mut inner = registry.lock_session_entities("seed hooked small fireball entity hit");
    let target = spawn_command_entity_locked(
        &mut inner,
        11,
        "minecraft:cow".to_owned(),
        Vec3::new(1.0, 64.0, 0.5),
        &mob_behaviors,
    )
    .0;
    let health = inner.entities.snapshot(target).expect("cow exists").health;
    let projectile = spawn_command_entity_locked(
        &mut inner,
        93,
        "minecraft:small_fireball".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
        &mob_behaviors,
    )
    .0;
    let expected = inner
        .entities
        .snapshot(projectile)
        .expect("small fireball exists");
    let state = initial_hurting_projectile_state(
        None,
        "minecraft:small_fireball",
        expected.position,
        Vec3::new(1.0, 0.0, 0.0),
        Rotation::ZERO,
    )
    .expect("valid small fireball state");
    let mut next = expected.clone();
    next.velocity = Vec3::new(
        state.projectile.velocity.x,
        state.projectile.velocity.y,
        state.projectile.velocity.z,
    );
    next.retained.hurting_projectile_state = Some(state);
    assert!(inner.entities.replace_snapshot_if_current(expected, next));
    (projectile, target, health)
}
fn seed_small_fireball_player_impact(registry: &SessionRegistry) -> mc_entity::EntityId {
    let mob_behaviors = registry.mob_behavior_table();
    let mut inner = registry.lock_session_entities("seed hooked small fireball player hit");
    let projectile = spawn_command_entity_locked(
        &mut inner,
        93,
        "minecraft:small_fireball".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
        &mob_behaviors,
    )
    .0;
    let expected = inner
        .entities
        .snapshot(projectile)
        .expect("small fireball exists");
    let state = initial_hurting_projectile_state(
        None,
        "minecraft:small_fireball",
        expected.position,
        Vec3::new(1.0, 0.0, 0.0),
        Rotation::ZERO,
    )
    .expect("valid small fireball state");
    let mut next = expected.clone();
    next.velocity = Vec3::new(
        state.projectile.velocity.x,
        state.projectile.velocity.y,
        state.projectile.velocity.z,
    );
    next.retained.hurting_projectile_state = Some(state);
    assert!(inner.entities.replace_snapshot_if_current(expected, next));
    projectile
}

fn fire_small_fireball(registry: &SessionRegistry, projectile: mc_entity::EntityId) {
    registry.apply_entity_physics_with_hurting_facts_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: projectile,
            position: Vec3::new(0.6, 64.0, 0.5),
            velocity: Vec3::new(0.1, 0.0, 0.0),
            on_ground: false,
            horizontal_collision: false,
        }],
        &[HurtingProjectilePhysicsFact {
            projectile_id: projectile,
            block_hit: None,
            in_water: false,
        }],
    );
}

#[tokio::test(flavor = "current_thread")]
async fn before_damage_controls_projectile_entity_impact_without_duplicate_commit() {
    let direct = SessionRegistry::new();
    let (direct_projectile, direct_target, direct_health) =
        seed_small_fireball_entity_impact(&direct);
    fire_small_fireball(&direct, direct_projectile);
    assert_eq!(
        direct
            .lock_entities("inspect direct projectile control")
            .snapshot(direct_target)
            .expect("direct target remains")
            .health,
        direct_health - 5.0,
        "the fixture's ordinary projectile path must damage the target"
    );
    assert!(
        direct.server_entity_snapshot(direct_projectile).is_none(),
        "the ordinary impact consumes the projectile"
    );

    let cancelled = SessionRegistry::new();
    let (cancelled_projectile, cancelled_target, cancelled_health) =
        seed_small_fireball_entity_impact(&cancelled);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = projectile_damage_boundary();
    cancelled.install_precommit_boundary(boundary);
    cancelled.install_damage_precommit_handle(handle);
    fire_small_fireball(&cancelled, cancelled_projectile);
    fire_small_fireball(&cancelled, cancelled_projectile);
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => {
            let HookContext::Damage(context) = request.context() else {
                panic!("projectile impact must ask a damage hook");
            };
            assert!(matches!(
                context.source(),
                HookActor::Entity(id) if *id == u64::try_from(cancelled_projectile.0).expect("positive projectile id")
            ));
            assert_eq!(
                context.target(),
                &DamageTarget::Entity(
                    u64::try_from(cancelled_target.0).expect("positive target id")
                )
            );
            assert_eq!(context.kind(), "projectile");
            assert_eq!(context.amount(), 5.0);
            request
                .answer(HookDecision::Cancel)
                .expect("live projectile damage request");
        }
        _ => panic!("expected one projectile entity damage request"),
    }
    assert!(
        owner.wait_for_command().await,
        "cancelled hook resumes the owner"
    );
    assert_eq!(owner.process_tick(&cancelled, 1).processed, 1);
    let cancelled_target_snapshot = cancelled
        .lock_entities("inspect cancelled projectile target")
        .snapshot(cancelled_target)
        .expect("cancelled target remains");
    assert_eq!(cancelled_target_snapshot.health, cancelled_health);
    assert_eq!(
        cancelled_target_snapshot.lifecycle,
        mc_entity::EntityLifecycle::Alive,
        "cancellation must not begin a death"
    );
    assert!(
        cancelled
            .server_entity_snapshot(cancelled_projectile)
            .is_none(),
        "cancellation must consume the frozen projectile impact without damaging its target"
    );
    assert!(!cancelled.persisted_entity_records().iter().any(|record| {
        record.snapshot.type_name == "minecraft:item"
            || record.snapshot.type_name == "minecraft:experience_orb"
    }));

    let replaced = SessionRegistry::new();
    let (replaced_projectile, replaced_target, replaced_health) =
        seed_small_fireball_entity_impact(&replaced);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = projectile_damage_boundary();
    replaced.install_precommit_boundary(boundary);
    replaced.install_damage_precommit_handle(handle);
    fire_small_fireball(&replaced, replaced_projectile);
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request
            .answer(HookDecision::Replace(2.0))
            .expect("live replaced projectile damage request"),
        _ => panic!("expected one projectile entity damage request"),
    }
    assert!(
        owner.wait_for_command().await,
        "replacement resumes the owner"
    );
    assert_eq!(owner.process_tick(&replaced, 1).processed, 1);
    assert_eq!(
        replaced
            .lock_entities("inspect replaced projectile target")
            .snapshot(replaced_target)
            .expect("replaced target remains")
            .health,
        replaced_health - 2.0,
        "replacement must re-enter native projectile damage with the raw amount"
    );
    assert!(
        replaced
            .server_entity_snapshot(replaced_projectile)
            .is_none()
    );
    assert_eq!(
        owner.process_tick(&replaced, 1).processed,
        0,
        "the one-shot approval must not commit the frozen impact twice"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn player_projectile_precommit_uses_held_session_state_without_relocking() {
    let registry = SessionRegistry::new();
    let target_session = registry.register_script_router_test_session("projectile-target");
    let target_state = std::sync::Arc::new(std::sync::Mutex::new(
        crate::play::PlayerPersistedState::new_default(crate::play::PlayerPose::new(
            0.5, 64.0, 0.5,
        )),
    ));
    let target_health = target_state
        .lock()
        .expect("player state lock")
        .survival
        .health;
    registry.register_player_persistence(target_session, std::sync::Arc::clone(&target_state));
    let projectile = seed_small_fireball_player_impact(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = projectile_damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);

    fire_small_fireball(&registry, projectile);
    fire_small_fireball(&registry, projectile);

    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => {
            let HookContext::Damage(context) = request.context() else {
                panic!("projectile impact must ask a damage hook");
            };
            assert_eq!(
                context.target(),
                &DamageTarget::Player(
                    mc_script::precommit::HookPlayer::try_new(
                        registry
                            .player_uuid(target_session)
                            .expect("registered player UUID"),
                        target_session,
                    )
                    .expect("registered hook player"),
                )
            );
            request
                .answer(HookDecision::Cancel)
                .expect("live player projectile damage request");
        }
        _ => panic!("expected one player projectile damage request"),
    }
    assert!(
        owner.wait_for_command().await,
        "player projectile hook must resume without self-deadlocking the session registry"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(
        target_state
            .lock()
            .expect("player state lock")
            .survival
            .health,
        target_health,
        "cancellation must not damage the player",
    );
    assert!(
        registry.server_entity_snapshot(projectile).is_none(),
        "cancellation must finalize the frozen projectile collision",
    );
    assert_eq!(
        owner.process_tick(&registry, 1).processed,
        0,
        "a pending projectile collision must submit and resume only once",
    );
}

#[test]
fn expired_projectile_precommit_finishes_without_late_damage() {
    let registry = SessionRegistry::new();
    let (projectile, target, health) = seed_small_fireball_entity_impact(&registry);
    let (boundary, _endpoint) = projectile_damage_boundary();
    registry.install_precommit_boundary(boundary);
    // The bounded resume queue may refuse a detached continuation. Its
    // expired collision still has to terminate on the next native step.
    registry
        .lock_inner("seed expired projectile decision")
        .pending_projectile_damage
        .insert(projectile, std::time::Instant::now());
    fire_small_fireball(&registry, projectile);
    assert!(registry.server_entity_snapshot(projectile).is_none());
    assert_eq!(
        registry
            .lock_entities("inspect expired projectile target")
            .snapshot(target)
            .expect("target remains")
            .health,
        health,
    );
}
