use std::collections::HashSet;

use mc_entity::{EntityDragonBreathCloudState, Vec3};
use tokio::sync::mpsc;

use super::*;
use crate::login::LoggedInProfile;

fn register_test_session(registry: &SessionRegistry, name: &str) -> SessionId {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (outbound, _receiver) = mpsc::channel(8);
    registry
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            outbound,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0
}

#[test]
fn dragon_spawn_reserves_vanilla_multipart_entity_id_span() {
    let registry = SessionRegistry::new();
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        43,
        "minecraft:ender_dragon".to_owned(),
        Vec3::new(0.5, 64.0, 10.5),
    );
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        11,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 12.5),
    );
    let records = registry.persisted_entity_records();
    let dragon_id = records
        .iter()
        .find(|record| record.snapshot.type_name == "minecraft:ender_dragon")
        .expect("dragon")
        .snapshot
        .id;
    let cow_id = records
        .iter()
        .find(|record| record.snapshot.type_name == "minecraft:cow")
        .expect("cow")
        .snapshot
        .id;
    assert_eq!(cow_id.0, dragon_id.0 + 9);
}

#[test]
fn dragon_player_part_damage_enters_dying_and_pays_fightless_xp_schedule() {
    let registry = SessionRegistry::new();
    registry.configure_arrow_kill_rewards(
        None,
        Some(99),
        None,
        std::sync::Arc::new(mc_data::items::solaris_required_items()),
        std::sync::Arc::new(mc_data::item_components::solaris_required_item_facts()),
        std::sync::Arc::new(mc_data::loot::builtin().clone()),
    );
    let player = register_test_session(&registry, "DragonDamageTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        43,
        "minecraft:ender_dragon".to_owned(),
        Vec3::new(0.5, 64.0, 10.5),
    );
    let dragon_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:ender_dragon")
        .expect("test dragon")
        .snapshot
        .id;
    let _ = registry.tick_entities_and_collect_physics_queries(1);
    let initial = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == dragon_id)
        .expect("dragon snapshot")
        .snapshot;
    let state = initial.retained.dragon_air.unwrap_or_else(|| {
        mc_entity::dragon_26_1_2::DragonAirState::new(initial.position, initial.rotation.yaw)
    });
    let neck = mc_entity::dragon_26_1_2::part_center(
        &state,
        initial.position,
        initial.rotation.yaw,
        mc_entity::dragon_26_1_2::DragonPart::Neck,
    )
    .expect("neck position");
    let result = registry.player_attack_server_entity(
        &SimulationAuthority::for_test(),
        super::entity_combat::ServerEntityPlayerAttack {
            entity_id: EntityId(dragon_id.0 + 2),
            amount: 8.0,
            game_mode: GameMode::Survival,
            player_pose: PlayerPose::new(neck.x, neck.y, neck.z),
            attacker: None,
        },
    );
    assert!(matches!(result, PlayerAttackResult::Damaged(_)));
    let after_neck = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == dragon_id)
        .expect("dragon after neck hit")
        .snapshot;
    assert_eq!(after_neck.health, initial.health - 3.0);

    registry.apply_entity_physics_with_hurting_facts_and_dispatch(10, &[], &[]);
    let state = after_neck.retained.dragon_air.unwrap_or_else(|| {
        mc_entity::dragon_26_1_2::DragonAirState::new(after_neck.position, after_neck.rotation.yaw)
    });
    let head = mc_entity::dragon_26_1_2::part_center(
        &state,
        after_neck.position,
        after_neck.rotation.yaw,
        mc_entity::dragon_26_1_2::DragonPart::Head,
    )
    .expect("head position");
    let result = registry.player_attack_server_entity(
        &SimulationAuthority::for_test(),
        super::entity_combat::ServerEntityPlayerAttack {
            entity_id: EntityId(dragon_id.0 + 1),
            amount: 1_000.0,
            game_mode: GameMode::Survival,
            player_pose: PlayerPose::new(head.x, head.y, head.z),
            attacker: None,
        },
    );
    assert!(matches!(result, PlayerAttackResult::Damaged(_)));
    let dying = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == dragon_id)
        .expect("dying dragon")
        .snapshot;
    assert_eq!(dying.health, 1.0);
    assert_eq!(
        dying.retained.dragon_air.expect("dragon air state").phase,
        mc_entity::dragon_26_1_2::DragonAirPhase::Dying
    );

    let mut dispatches = Vec::new();
    for tick in 11..=210 {
        dispatches.extend(registry.tick_dragon_air_combat(&SimulationAuthority::for_test(), tick));
    }
    let terminal_death_time = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == dragon_id)
        .and_then(|record| {
            record
                .snapshot
                .retained
                .dragon_air
                .map(|state| state.death_time)
        });
    assert!(
        registry.server_entity_snapshot(dragon_id).is_none(),
        "dragon must be removed after 200 dying ticks: {terminal_death_time:?}"
    );
    let xp = registry
        .persisted_entity_records()
        .into_iter()
        .filter_map(|record| record.snapshot.experience_value)
        .sum::<i32>();
    assert_eq!(xp, 500);
    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::DespawnEntity(entity) if entity.id == dragon_id
        )
    }));
}

#[test]
fn dragon_air_owner_moves_and_fires_owned_fireball_without_generic_goal_motion() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_dragon_fireball_entity_type(Some(37));
    let player = register_test_session(&registry, "DragonAirTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        43,
        "minecraft:ender_dragon".to_owned(),
        Vec3::new(0.5, 64.0, 10.5),
    );
    let dragon_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:ender_dragon")
        .expect("test dragon")
        .snapshot
        .id;
    let _ = registry.tick_entities_and_collect_physics_queries(1);
    let initial = registry
        .server_entity_snapshot(dragon_id)
        .expect("dragon snapshot after activation");

    let mut dispatches = Vec::new();
    for tick in 1..=10 {
        dispatches.extend(registry.tick_dragon_air_combat(&SimulationAuthority::for_test(), tick));
        if registry
            .persisted_entity_records()
            .into_iter()
            .any(|record| record.snapshot.type_name == "minecraft:dragon_fireball")
        {
            break;
        }
    }

    let dragon = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == dragon_id)
        .expect("dragon remains authoritative")
        .snapshot;
    assert_eq!(dragon.goal, mc_entity::GoalState::Idle);
    assert_ne!(dragon.position, initial.position);
    assert!(dragon.retained.dragon_air.is_some());
    let fireball = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:dragon_fireball")
        .expect("aligned dragon strafe fires");
    let owner = fireball
        .snapshot
        .retained
        .hurting_projectile_state
        .expect("dragon fireball projectile kernel")
        .projectile
        .owner
        .expect("dragon fireball owner");
    assert_eq!(owner.raw(), u128::from(dragon_id.0 as u32));
    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::SpawnEntity(entity) if entity.type_name == "minecraft:dragon_fireball"
        )
    }));
}

#[test]
fn dragon_breath_cloud_pulses_every_five_ticks_reapplies_after_twenty_and_discards() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "DragonCloudTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        3,
        "minecraft:area_effect_cloud".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let cloud_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:area_effect_cloud")
        .expect("test dragon cloud")
        .snapshot
        .id;
    {
        let mut entities = registry.lock_entities("seed dragon cloud retained state");
        let expected = entities.snapshot(cloud_id).expect("cloud exists");
        let mut next = expected.clone();
        next.retained.dragon_breath_cloud = Some(EntityDragonBreathCloudState::dragon_fireball(-1));
        assert!(entities.replace_snapshot_if_current(expected, next));
    }

    let _ = registry.tick_entities_and_collect_physics_queries(1);
    let mut first_pulse = Vec::new();
    for tick in 1..=5 {
        first_pulse
            .extend(registry.tick_dragon_breath_clouds(&SimulationAuthority::for_test(), tick));
    }
    assert!(first_pulse.iter().any(|dispatch| {
        dispatch.recipient.id == player
            && matches!(
                dispatch.command,
                OutboundCommand::DamagePlayer {
                    damage: PlayerDamageRequest {
                        kind: PlayerDamageKind::IndirectMagic,
                        amount,
                        ..
                    }
                } if (amount - 6.0).abs() < f32::EPSILON
            )
    }));

    let mut blocked_pulses = Vec::new();
    for tick in 6..=24 {
        blocked_pulses
            .extend(registry.tick_dragon_breath_clouds(&SimulationAuthority::for_test(), tick));
    }
    assert!(!blocked_pulses.iter().any(|dispatch| {
        dispatch.recipient.id == player
            && matches!(dispatch.command, OutboundCommand::DamagePlayer { .. })
    }));

    let reapplied = registry.tick_dragon_breath_clouds(&SimulationAuthority::for_test(), 25);
    assert!(reapplied.iter().any(|dispatch| {
        dispatch.recipient.id == player
            && matches!(dispatch.command, OutboundCommand::DamagePlayer { .. })
    }));

    {
        let mut entities = registry.lock_entities("age dragon cloud to terminal tick");
        let expected = entities
            .snapshot(cloud_id)
            .expect("cloud remains before expiry");
        let mut next = expected.clone();
        next.retained
            .dragon_breath_cloud
            .as_mut()
            .expect("dragon cloud state")
            .age_ticks = 599;
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    let removed = registry.tick_dragon_breath_clouds(&SimulationAuthority::for_test(), 600);
    assert!(registry.server_entity_snapshot(cloud_id).is_none());
    assert!(removed.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::DespawnEntity(entity) if entity.id == cloud_id
        )
    }));
}
