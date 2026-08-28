use mc_entity::projectile_26_1_2::{ARROW_DESPAWN_TICKS, BlockStateId};
use mc_entity::{Rotation, Vec3};

use crate::play::{ArrowPhysicsFact, EntityPhysicsStep, HurtingProjectilePhysicsFact};

use super::SessionRegistry;
use super::entity_lifecycle::spawn_command_entity_locked;
use super::projectiles::{
    HurtingProjectileMotionProfile, initial_hurting_projectile_state,
    initial_hurting_projectile_state_with_motion, initial_throwable_projectile_state,
    projectile_identity, spawn_arrow_locked,
};

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
