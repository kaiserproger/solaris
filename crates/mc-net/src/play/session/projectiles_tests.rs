use mc_entity::projectile_26_1_2::{ARROW_DESPAWN_TICKS, BlockStateId};
use mc_entity::{Rotation, Vec3};

use crate::play::{ArrowPhysicsFact, EntityPhysicsStep};

use super::SessionRegistry;
use super::entity_lifecycle::spawn_command_entity_locked;
use super::projectiles::spawn_arrow_locked;

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
