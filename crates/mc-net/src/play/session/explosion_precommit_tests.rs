use super::*;
use mc_script::precommit::{
    HookContext, HookDecision, HookFailurePolicy, HookKind, HookRegistration,
};
use mc_script::{ScriptHostInput, script_boundary_pair};
use std::num::NonZeroUsize;

#[tokio::test(flavor = "current_thread")]
async fn before_damage_keep_preserves_explosion_knockback_and_cancel_commits_nothing() {
    let impact = Vec3::new(0.4, 0.2, -0.1);
    let direct = SessionRegistry::new();
    let direct_session = register_test_session(&direct, "DirectExplosionObserver");
    assert!(direct.mark_loaded(direct_session, (0, 0)).is_empty());
    let direct_target = match &direct.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_owned(),
        Vec3::new(1.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };
    direct.apply_explosion_entity_impacts(
        &SimulationAuthority::for_test(),
        &[ServerEntityExplosionImpact {
            entity_id: direct_target,
            damage: 2.0,
            knockback: impact,
        }],
    );
    let direct_snapshot = direct
        .lock_entities("inspect direct explosion impact")
        .snapshot(direct_target)
        .expect("direct target remains");

    let kept = SessionRegistry::new();
    let kept_session = register_test_session(&kept, "KeptExplosionObserver");
    assert!(kept.mark_loaded(kept_session, (0, 0)).is_empty());
    let kept_target = match &kept.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_owned(),
        Vec3::new(1.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };
    let (handle, mut owner) = crate::play::simulation::simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "explosion-judge",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-damage hook");
    kept.install_precommit_boundary(boundary);
    kept.install_damage_precommit_handle(handle);
    assert!(
        kept.apply_explosion_entity_impacts(
            &SimulationAuthority::for_test(),
            &[ServerEntityExplosionImpact {
                entity_id: kept_target,
                damage: 2.0,
                knockback: impact,
            }],
        )
        .is_empty()
    );
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => {
            let HookContext::Damage(context) = request.context() else {
                panic!("explosion impact must ask a damage hook");
            };
            assert_eq!(context.kind(), "explosion");
            request
                .answer(HookDecision::Keep)
                .expect("live explosion request");
        }
        _ => panic!("expected one explosion damage request"),
    }
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&kept, 1).processed, 1);
    let kept_snapshot = kept
        .lock_entities("inspect kept explosion impact")
        .snapshot(kept_target)
        .expect("kept target remains");
    assert_eq!(kept_snapshot.health, direct_snapshot.health);
    assert_eq!(kept_snapshot.velocity, direct_snapshot.velocity);

    let cancelled = SessionRegistry::new();
    let cancelled_session = register_test_session(&cancelled, "CancelledExplosionObserver");
    assert!(cancelled.mark_loaded(cancelled_session, (0, 0)).is_empty());
    let cancelled_target = match &cancelled.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_owned(),
        Vec3::new(1.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };
    let before_cancel = cancelled
        .lock_entities("capture cancelled explosion target")
        .snapshot(cancelled_target)
        .expect("cancelled target exists");
    let (handle, mut owner) = crate::play::simulation::simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "explosion-judge",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-damage hook");
    cancelled.install_precommit_boundary(boundary);
    cancelled.install_damage_precommit_handle(handle);
    cancelled.apply_explosion_entity_impacts(
        &SimulationAuthority::for_test(),
        &[ServerEntityExplosionImpact {
            entity_id: cancelled_target,
            damage: 2.0,
            knockback: impact,
        }],
    );
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request
            .answer(HookDecision::Cancel)
            .expect("live explosion request"),
        _ => panic!("expected one explosion damage request"),
    }
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&cancelled, 1).processed, 1);
    assert_eq!(
        cancelled
            .lock_entities("inspect cancelled explosion impact")
            .snapshot(cancelled_target)
            .expect("cancelled target remains"),
        before_cancel
    );
}

#[test]
fn zero_damage_explosion_knockback_stays_on_the_native_path_with_damage_hooks() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "KnockbackOnlyExplosionObserver");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let target = match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:chicken".to_owned(),
        Vec3::new(1.5, 64.0, 0.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected chicken spawn dispatch, got {other:?}"),
    };
    let (boundary, _endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "explosion-judge",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-damage hook");
    registry.install_precommit_boundary(boundary);
    registry.apply_explosion_entity_impacts(
        &SimulationAuthority::for_test(),
        &[ServerEntityExplosionImpact {
            entity_id: target,
            damage: 0.0,
            knockback: Vec3::new(0.4, 0.2, -0.1),
        }],
    );
    let target = registry
        .lock_entities("inspect knockback-only explosion impact")
        .snapshot(target)
        .expect("target remains");
    assert_eq!(target.health, 4.0);
    assert_eq!(target.velocity, Vec3::new(0.4, 0.2, -0.1));
}
