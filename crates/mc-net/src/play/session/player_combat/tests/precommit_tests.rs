use super::*;
use mc_entity::EntityLifecycle;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::sync::Arc;

use mc_data::item_components::{ItemArmorFacts, ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_entity::Vec3;
use mc_script::precommit::{
    DamageTarget, HookActor, HookContext, HookDecision, HookFailurePolicy, HookKind,
    HookRegistration,
};
use mc_script::{ScriptHostInput, script_boundary_pair};

use crate::play::session::resident_orders::ResidentAttack;
use crate::play::simulation::simulation_channel_with_capacity;

fn damage_boundary() -> (mc_script::ScriptBoundary, mc_script::ScriptHostEndpoint) {
    let (boundary, endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "judge",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-damage hook");
    let manifest = mc_script::ScriptPluginManifest::new(
        "judge",
        "Judge",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .validate()
    .expect("valid source plugin");
    endpoint
        .register_plugin_routes(&manifest)
        .expect("live programmatic damage source");
    (boundary, endpoint)
}

fn answer_next(endpoint: &mut mc_script::ScriptHostEndpoint, decision: HookDecision) {
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request
            .answer(decision)
            .expect("live native before-damage request"),
        _ => panic!("expected one before-damage request"),
    }
}

fn spawn_cow(registry: &SessionRegistry) -> EntityId {
    let observer = register_player(
        registry,
        "CowObserver",
        PlayerPose::new(0.5, 64.0, 0.5),
        PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
    );
    registry.mark_loaded(observer, (0, 0));
    match &registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        2,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    )[0]
    .command
    {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected cow spawn dispatch, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_pvp_preserves_target_resources_attacker_costs_and_death_outbox() {
    let shield_name = Identifier::parse("minecraft:shield").expect("shield identifier");
    let chest_name = Identifier::parse("minecraft:iron_chestplate").expect("chest identifier");
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: shield_name.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: chest_name.clone(),
            protocol_id: 2,
        },
    ]));
    let facts = Arc::new(ItemFactsTable::from_entries([
        (
            shield_name,
            ItemFacts {
                max_damage: Some(336),
                ..ItemFacts::default()
            },
        ),
        (
            chest_name,
            ItemFacts {
                max_damage: Some(240),
                armor: Some(ItemArmorFacts {
                    slot: "chest".to_owned(),
                    armor: 6.0,
                    toughness: 0.0,
                }),
                ..ItemFacts::default()
            },
        ),
    ]));
    let registry = SessionRegistry::new();
    registry.configure_player_combat(None, None, items, facts);
    let mut deaths = registry.install_script_commit_event_outbox();
    let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let target_pose = PlayerPose::new(0.5, 64.0, 1.5);
    let attacker = register_player(
        &registry,
        "CancelledPvpAttacker",
        attacker_pose,
        PlayerPersistedState::new_default(attacker_pose),
    );
    let mut target_state = PlayerPersistedState::new_default(target_pose);
    target_state.inventory.slots[6] = ItemStack::new(2, 1).with_damage(3);
    target_state.inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(1, 1);
    let target = register_player(
        &registry,
        "CancelledPvpTarget",
        target_pose,
        target_state.clone(),
    );
    let shield = ActiveShield {
        started_tick: 0,
        slot: PlayerInventory::OFFHAND_SLOT,
        expected_stack: ItemStack::new(1, 1),
    };
    registry.set_active_shield(target, Some(shield.clone()));
    registry.advance_world_time(crate::play::combat::SHIELD_ACTIVATION_DELAY_TICKS);
    let (target_entity, attacker_state, target_state) = {
        let inner = registry.lock_inner("capture cancelled PvP authority state");
        (
            EntityId(inner.sessions[&target].entity_id),
            Arc::clone(&inner.player_persistence[&attacker]),
            Arc::clone(&inner.player_persistence[&target]),
        )
    };
    let attacker_before = attacker_state.lock().expect("attacker state lock").clone();
    let target_before = target_state.lock().expect("target state lock").clone();
    let mut attacker_costs = survival_plan(
        &attacker_before,
        attacker_before.inventory.clone(),
        attacker_pose,
    );
    attacker_costs.updated_survival.add_exhaustion(2.0);

    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);
    assert!(matches!(
        registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target_entity,
                amount: mc_entity::player_survival_26_1_2::MAX_HEALTH,
                attacker_costs: Some(&attacker_costs),
                authority_tick: registry.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::Damaged(_)
    ));

    answer_next(&mut endpoint, HookDecision::Cancel);
    assert!(
        owner.wait_for_command().await,
        "answered hook resumes the owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);

    let attacker_after = attacker_state.lock().expect("attacker state lock").clone();
    assert_eq!(
        attacker_after.survival, attacker_before.survival,
        "cancel must not charge the attacker"
    );
    assert_eq!(
        attacker_after.inventory.slots, attacker_before.inventory.slots,
        "cancel must not spend attacker equipment"
    );
    let target_after = target_state.lock().expect("target state lock").clone();
    assert_eq!(
        target_after.survival, target_before.survival,
        "cancel must not change target health"
    );
    assert_eq!(
        target_after.inventory.slots, target_before.inventory.slots,
        "cancel must not damage target armor"
    );
    assert_eq!(
        registry
            .lock_inner("verify cancelled shield")
            .active_shields
            .get(&target),
        Some(&shield),
        "cancel must not consume or alter the active shield"
    );
    assert!(
        deaths.try_recv_required().is_none(),
        "cancel must not publish a death event"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn replacement_reenters_pvp_with_the_raw_amount_before_armor() {
    let chest_name = Identifier::parse("minecraft:iron_chestplate").expect("chest identifier");
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: chest_name.clone(),
        protocol_id: 1,
    }]));
    let facts = Arc::new(ItemFactsTable::from_entries([(
        chest_name,
        ItemFacts {
            max_damage: Some(240),
            armor: Some(ItemArmorFacts {
                slot: "chest".to_owned(),
                armor: 6.0,
                toughness: 0.0,
            }),
            ..ItemFacts::default()
        },
    )]));
    let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let target_pose = PlayerPose::new(0.5, 64.0, 1.5);

    let direct = SessionRegistry::new();
    direct.configure_player_combat(None, None, Arc::clone(&items), Arc::clone(&facts));
    let direct_attacker = register_player(
        &direct,
        "RawReplaceDirectAttacker",
        attacker_pose,
        PlayerPersistedState::new_default(attacker_pose),
    );
    let mut direct_target_state = PlayerPersistedState::new_default(target_pose);
    direct_target_state.inventory.slots[6] = ItemStack::new(1, 1);
    let direct_target = register_player(
        &direct,
        "RawReplaceDirectTarget",
        target_pose,
        direct_target_state,
    );
    let (direct_target_entity, direct_target_state) = {
        let inner = direct.lock_inner("capture direct raw reduction target");
        (
            EntityId(inner.sessions[&direct_target].entity_id),
            Arc::clone(&inner.player_persistence[&direct_target]),
        )
    };
    assert!(matches!(
        direct.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: direct_attacker,
                entity_id: direct_target_entity,
                amount: 6.0,
                attacker_costs: None,
                authority_tick: direct.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::Damaged(_)
    ));
    let direct_after = direct_target_state
        .lock()
        .expect("direct target state lock")
        .clone();

    let hooked = SessionRegistry::new();
    hooked.configure_player_combat(None, None, items, facts);
    let hooked_attacker = register_player(
        &hooked,
        "RawReplaceHookedAttacker",
        attacker_pose,
        PlayerPersistedState::new_default(attacker_pose),
    );
    let mut hooked_target_state = PlayerPersistedState::new_default(target_pose);
    hooked_target_state.inventory.slots[6] = ItemStack::new(1, 1);
    let hooked_target = register_player(
        &hooked,
        "RawReplaceHookedTarget",
        target_pose,
        hooked_target_state,
    );
    let (hooked_target_entity, hooked_target_state) = {
        let inner = hooked.lock_inner("capture hooked raw reduction target");
        (
            EntityId(inner.sessions[&hooked_target].entity_id),
            Arc::clone(&inner.player_persistence[&hooked_target]),
        )
    };
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    hooked.install_precommit_boundary(boundary);
    hooked.install_damage_precommit_handle(handle);
    assert!(matches!(
        hooked.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: hooked_attacker,
                entity_id: hooked_target_entity,
                amount: 14.0,
                attacker_costs: None,
                authority_tick: hooked.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::Damaged(_)
    ));

    answer_next(&mut endpoint, HookDecision::Replace(6.0));
    assert!(
        owner.wait_for_command().await,
        "answered replacement resumes the owner"
    );
    assert_eq!(owner.process_tick(&hooked, 1).processed, 1);
    let hooked_after = hooked_target_state
        .lock()
        .expect("hooked target state lock")
        .clone();
    assert_eq!(
        hooked_after.survival, direct_after.survival,
        "replacement amount must enter the normal armor reduction pipeline as raw damage"
    );
    assert_eq!(
        hooked_after.inventory.slots, direct_after.inventory.slots,
        "replacement must apply the same armor durability as a direct raw hit"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn changed_target_revision_refuses_the_answered_pvp_ticket() {
    let registry = SessionRegistry::new();
    let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let target_pose = PlayerPose::new(0.5, 64.0, 1.5);
    let attacker = register_player(
        &registry,
        "StaleRevisionAttacker",
        attacker_pose,
        PlayerPersistedState::new_default(attacker_pose),
    );
    let target = register_player(
        &registry,
        "StaleRevisionTarget",
        target_pose,
        PlayerPersistedState::new_default(target_pose),
    );
    let (target_entity, target_state) = {
        let inner = registry.lock_inner("capture stale revision target");
        (
            EntityId(inner.sessions[&target].entity_id),
            Arc::clone(&inner.player_persistence[&target]),
        )
    };
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);
    assert!(matches!(
        registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target_entity,
                amount: 4.0,
                attacker_costs: None,
                authority_tick: registry.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::Damaged(_)
    ));

    {
        let mut state = target_state.lock().expect("target state lock");
        state.survival.add_exhaustion(1.0);
    }
    let revised = target_state.lock().expect("target state lock").clone();
    answer_next(&mut endpoint, HookDecision::Keep);
    assert!(
        owner.wait_for_command().await,
        "answered hook resumes the owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let after = target_state.lock().expect("target state lock").clone();
    assert_eq!(
        after.survival, revised.survival,
        "a target revision change must refuse the deferred damage"
    );
    assert_eq!(
        after.inventory.slots, revised.inventory.slots,
        "stale deferred damage must not mutate target equipment"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn disconnected_target_replaced_by_the_same_identity_never_receives_stale_pvp_damage() {
    let registry = SessionRegistry::new();
    let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let target_pose = PlayerPose::new(0.5, 64.0, 1.5);
    let attacker = register_player(
        &registry,
        "DisconnectedPvpAttacker",
        attacker_pose,
        PlayerPersistedState::new_default(attacker_pose),
    );
    let target = register_player(
        &registry,
        "DisconnectedPvpTarget",
        target_pose,
        PlayerPersistedState::new_default(target_pose),
    );
    let (target_entity, departed_state) = {
        let inner = registry.lock_inner("capture disconnect target");
        (
            EntityId(inner.sessions[&target].entity_id),
            Arc::clone(&inner.player_persistence[&target]),
        )
    };
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);
    assert!(matches!(
        registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target_entity,
                amount: 4.0,
                attacker_costs: None,
                authority_tick: registry.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::Damaged(_)
    ));

    registry.unregister(target);
    let replacement = register_player(
        &registry,
        "DisconnectedPvpTarget",
        target_pose,
        PlayerPersistedState::new_default(target_pose),
    );
    let replacement_state = {
        let inner = registry.lock_inner("capture replacement target");
        Arc::clone(&inner.player_persistence[&replacement])
    };
    assert_ne!(
        replacement, target,
        "replacement must be a new session generation"
    );
    answer_next(&mut endpoint, HookDecision::Keep);
    assert!(
        owner.wait_for_command().await,
        "answered hook resumes the owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(
        departed_state
            .lock()
            .expect("departed state lock")
            .survival
            .health,
        mc_entity::player_survival_26_1_2::MAX_HEALTH,
        "the departed target state remains untouched"
    );
    assert_eq!(
        replacement_state
            .lock()
            .expect("replacement state lock")
            .survival
            .health,
        mc_entity::player_survival_26_1_2::MAX_HEALTH,
        "the replacement session must not receive the old ticket's damage"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_player_entity_damage_preserves_target_and_frozen_attacker_costs() {
    let registry = SessionRegistry::new();
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let attacker = register_player(
        &registry,
        "CancelledEntityAttacker",
        pose,
        PlayerPersistedState::new_default(pose),
    );
    let attacker_state = {
        let inner = registry.lock_inner("capture cancelled entity attacker");
        Arc::clone(&inner.player_persistence[&attacker])
    };
    let attacker_before = attacker_state.lock().expect("attacker state lock").clone();
    let mut costs = survival_plan(&attacker_before, attacker_before.inventory.clone(), pose);
    costs.updated_survival.add_exhaustion(2.0);
    let target = spawn_cow(&registry);
    let target_before = registry
        .lock_entities("capture cancelled entity target")
        .snapshot(target)
        .expect("spawned cow");
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);

    assert!(matches!(
        registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target,
                amount: 100.0,
                attacker_costs: Some(&costs),
                authority_tick: registry.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::AcceptedNoDamage
    ));
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => {
            let HookContext::Damage(context) = request.context() else {
                panic!("entity attack must ask a damage hook");
            };
            assert!(matches!(
                context.source(),
                HookActor::Player(player) if player.session() == attacker
            ));
            assert_eq!(
                context.target(),
                &DamageTarget::Entity(u64::try_from(target.0).expect("positive entity id"))
            );
            assert_eq!(context.kind(), "player-attack");
            assert_eq!(context.amount(), 100.0);
            request
                .answer(HookDecision::Cancel)
                .expect("live entity damage request");
        }
        _ => panic!("expected one entity damage request"),
    }
    assert!(
        owner.wait_for_command().await,
        "cancelled hook resumes the owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(
        registry
            .lock_entities("verify cancelled entity target")
            .snapshot(target),
        Some(target_before),
        "cancel must not mutate the entity"
    );
    let after = attacker_state.lock().expect("attacker state lock");
    assert_eq!(after.survival, attacker_before.survival);
    assert_eq!(after.inventory.slots, attacker_before.inventory.slots);
    assert_eq!(after.carried_item, attacker_before.carried_item);
    assert_eq!(after.xp, attacker_before.xp);
}

#[tokio::test(flavor = "current_thread")]
async fn replacement_reenters_script_entity_damage_with_the_raw_amount() {
    let direct = SessionRegistry::new();
    let direct_target = spawn_cow(&direct);
    let mut response = None;
    assert!(
        direct
            .damage_script_entity(
                &SimulationAuthority::for_test(),
                direct_target,
                6.0,
                "judge",
                &mut response,
            )
            .is_some()
    );
    let direct_after = direct
        .lock_entities("capture direct script damage")
        .snapshot(direct_target)
        .expect("damaged cow");

    let hooked = SessionRegistry::new();
    let target = spawn_cow(&hooked);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    hooked.install_precommit_boundary(boundary);
    hooked.install_damage_precommit_handle(handle.clone());
    let script_request = tokio::spawn({
        let handle = handle.clone();
        async move { handle.damage_script_entity(target, 14.0, "judge").await }
    });
    tokio::task::yield_now().await;
    assert!(
        owner.wait_for_command().await,
        "script damage request reaches the simulation owner"
    );
    assert_eq!(owner.process_tick(&hooked, 1).processed, 1);
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => {
            let HookContext::Damage(context) = request.context() else {
                panic!("script entity damage must ask a damage hook");
            };
            assert!(matches!(
                context.source(),
                HookActor::Plugin(plugin) if plugin == "judge"
            ));
            assert_eq!(context.kind(), "script");
            assert_eq!(context.amount(), 14.0);
            request
                .answer(HookDecision::Replace(6.0))
                .expect("live script entity damage request");
        }
        _ => panic!("expected one script entity damage request"),
    }
    assert!(
        owner.wait_for_command().await,
        "replacement resumes the owner"
    );
    assert_eq!(owner.process_tick(&hooked, 1).processed, 1);
    assert_eq!(
        script_request.await.expect("script damage request task"),
        Ok(Some(crate::play::simulation::ScriptEntityDamageCommit {
            health: direct_after.health,
            killed: false,
        })),
        "the script caller receives the native committed result after replacement"
    );
    assert_eq!(
        hooked
            .lock_entities("capture replaced script damage")
            .snapshot(target)
            .expect("damaged cow")
            .health,
        direct_after.health,
        "replacement must enter native entity damage as the raw amount"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_resident_damage_stays_hooked_and_leaves_the_entity_unchanged() {
    let registry = SessionRegistry::new();
    let target = spawn_cow(&registry);
    let expected = registry
        .lock_entities("capture resident damage target")
        .snapshot(target)
        .expect("spawned cow");
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle.clone());
    let resident_request = tokio::spawn({
        let handle = handle.clone();
        let expected = expected.clone();
        async move {
            handle
                .damage_resident_entity(
                    "judge",
                    ResidentAttack {
                        uuid: uuid::Uuid::nil(),
                        amount: 4.0,
                        expected,
                    },
                )
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(
        owner.wait_for_command().await,
        "resident request reaches the simulation owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => {
            let HookContext::Damage(context) = request.context() else {
                panic!("resident damage must ask a damage hook");
            };
            assert!(matches!(
                context.source(),
                HookActor::Plugin(plugin) if plugin == "judge"
            ));
            assert_eq!(context.kind(), "resident");
            request
                .answer(HookDecision::Cancel)
                .expect("live resident damage request");
        }
        _ => panic!("expected one resident damage request"),
    }
    assert!(
        owner.wait_for_command().await,
        "cancelled resident hook resumes the owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(
        matches!(
            resident_request.await.expect("resident request task"),
            Ok(None)
        ),
        "cancelled resident damage must resolve only after the native refusal"
    );
    assert_eq!(
        registry
            .lock_entities("verify cancelled resident damage")
            .snapshot(target),
        Some(expected),
        "resident damage must not bypass a cancelled hook"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn stale_entity_or_attacker_state_refuses_deferred_entity_damage() {
    let registry = SessionRegistry::new();
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let attacker = register_player(
        &registry,
        "StaleEntityAttacker",
        pose,
        PlayerPersistedState::new_default(pose),
    );
    let attacker_state = {
        let inner = registry.lock_inner("capture stale entity attacker");
        Arc::clone(&inner.player_persistence[&attacker])
    };
    let attacker_before = attacker_state.lock().expect("attacker state lock").clone();
    let mut costs = survival_plan(&attacker_before, attacker_before.inventory.clone(), pose);
    costs.updated_survival.add_exhaustion(2.0);
    let target = spawn_cow(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);
    assert!(matches!(
        registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target,
                amount: 4.0,
                attacker_costs: Some(&costs),
                authority_tick: registry.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::AcceptedNoDamage
    ));
    {
        let mut state = attacker_state.lock().expect("attacker state lock");
        state.survival.add_exhaustion(1.0);
    }
    let attacker_revised = attacker_state.lock().expect("attacker state lock").clone();
    let target_before = registry
        .lock_entities("capture stale attacker target")
        .snapshot(target)
        .expect("spawned cow");
    answer_next(&mut endpoint, HookDecision::Keep);
    assert!(
        owner.wait_for_command().await,
        "stale attacker resumes the owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(
        registry
            .lock_entities("verify stale attacker target")
            .snapshot(target),
        Some(target_before),
        "a changed frozen attacker cost plan must refuse damage"
    );
    {
        let after = attacker_state.lock().expect("attacker state lock");
        assert_eq!(after.survival, attacker_revised.survival);
        assert_eq!(after.inventory.slots, attacker_revised.inventory.slots);
        assert_eq!(after.carried_item, attacker_revised.carried_item);
        assert_eq!(after.xp, attacker_revised.xp);
    }
    let stale_target_registry = SessionRegistry::new();
    let stale_target = spawn_cow(&stale_target_registry);
    let (stale_handle, mut stale_owner) = simulation_channel_with_capacity(2);
    let (stale_boundary, mut stale_endpoint) = damage_boundary();
    stale_target_registry.install_precommit_boundary(stale_boundary);
    stale_target_registry.install_damage_precommit_handle(stale_handle);
    let mut response = None;
    assert!(
        stale_target_registry
            .damage_script_entity(
                &SimulationAuthority::for_test(),
                stale_target,
                4.0,
                "judge",
                &mut response,
            )
            .is_none()
    );
    let revised = stale_target_registry
        .damage_server_entity_for_test(stale_target, 1.0)
        .expect("direct native revision");
    answer_next(&mut stale_endpoint, HookDecision::Keep);
    assert!(
        stale_owner.wait_for_command().await,
        "stale target hook resumes the owner"
    );
    assert_eq!(
        stale_owner
            .process_tick(&stale_target_registry, 1)
            .processed,
        1
    );
    assert_eq!(
        stale_target_registry
            .lock_entities("verify stale entity target")
            .snapshot(stale_target),
        Some(revised.snapshot),
        "a changed target snapshot must refuse the old approval"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn accepted_lethal_script_damage_publishes_one_native_death_drop_and_xp() {
    let registry = SessionRegistry::new();
    let cow = Identifier::parse("minecraft:cow").expect("cow identifier");
    let leather = Identifier::parse("minecraft:leather").expect("leather identifier");
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: leather.clone(),
        protocol_id: 41,
    }]));
    let loot = Arc::new(mc_data::loot::LootTables::from_drop_lists(
        BTreeMap::from([(cow, vec![mc_data::loot::LootDrop::single(leather)])]),
        BTreeMap::new(),
    ));
    registry.configure_arrow_kill_rewards(
        Some(98),
        Some(99),
        None,
        items,
        Arc::new(ItemFactsTable::default()),
        loot,
    );
    let target = spawn_cow(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);
    let mut response = None;
    assert!(
        registry
            .damage_script_entity(
                &SimulationAuthority::for_test(),
                target,
                100.0,
                "judge",
                &mut response,
            )
            .is_none()
    );
    answer_next(&mut endpoint, HookDecision::Keep);
    assert!(
        owner.wait_for_command().await,
        "accepted lethal hook resumes the owner"
    );
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);

    let records = registry.persisted_entity_records();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:item")
            .count(),
        1,
        "accepted lethal damage must create one native item drop"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:experience_orb")
            .count(),
        1,
        "accepted lethal damage must create one native XP drop"
    );
    assert_eq!(
        registry
            .lock_entities("verify accepted lethal entity state")
            .snapshot(target)
            .expect("dying cow remains authoritative")
            .lifecycle,
        mc_entity::EntityLifecycle::Despawning,
        "accepted lethal damage must enter the native death lifecycle once"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn kept_player_entity_damage_runs_the_native_melee_kill_kernel() {
    let registry = SessionRegistry::new();
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let attacker = register_player(
        &registry,
        "KeptAttacker",
        pose,
        PlayerPersistedState::new_default(pose),
    );
    let attacker_state = {
        let inner = registry.lock_inner("capture kept entity attacker");
        Arc::clone(&inner.player_persistence[&attacker])
    };
    let attacker_before = attacker_state.lock().expect("attacker state lock").clone();
    let costs = survival_plan(&attacker_before, attacker_before.inventory.clone(), pose);
    let target = spawn_cow(&registry);
    let mut events = registry.install_script_commit_event_outbox();
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);

    assert!(matches!(
        registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target,
                amount: 100.0,
                attacker_costs: Some(&costs),
                authority_tick: registry.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::AcceptedNoDamage
    ));
    answer_next(&mut endpoint, HookDecision::Keep);
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(
        events.try_recv_required().is_some(),
        "keeping a lethal player melee hit must retain its native kill event"
    );
    assert!(
        registry
            .persisted_entity_records()
            .iter()
            .any(|record| record.snapshot.id == target
                && record.snapshot.lifecycle == EntityLifecycle::Despawning)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn spectator_after_deferred_player_entity_damage_refuses_before_damage_or_costs() {
    let registry = SessionRegistry::new();
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let attacker = register_player(
        &registry,
        "SpectatorEntityAttacker",
        pose,
        PlayerPersistedState::new_default(pose),
    );
    let attacker_state = {
        let inner = registry.lock_inner("capture spectator entity attacker");
        Arc::clone(&inner.player_persistence[&attacker])
    };
    let attacker_before = attacker_state.lock().expect("attacker state lock").clone();
    let costs = survival_plan(&attacker_before, attacker_before.inventory.clone(), pose);
    let target = spawn_cow(&registry);
    let target_before = registry
        .lock_entities("capture spectator target")
        .snapshot(target)
        .expect("spawned cow");
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);

    assert!(matches!(
        registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target,
                amount: 4.0,
                attacker_costs: Some(&costs),
                authority_tick: registry.simulation_tick(),
                hook_approval: None,
            },
        ),
        PlayerAttackResult::AcceptedNoDamage
    ));
    attacker_state
        .lock()
        .expect("attacker state lock")
        .game_mode = GameMode::Spectator;
    answer_next(&mut endpoint, HookDecision::Keep);
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(
        registry
            .lock_entities("verify spectator target")
            .snapshot(target),
        Some(target_before)
    );
    let after = attacker_state.lock().expect("attacker state lock");
    assert_eq!(after.survival, attacker_before.survival);
    assert_eq!(after.inventory.slots, attacker_before.inventory.slots);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_dragon_part_damage_never_bypasses_the_damage_hook() {
    let registry = SessionRegistry::new();
    let observer = register_player(
        &registry,
        "DragonHookObserver",
        PlayerPose::new(0.5, 64.0, 0.5),
        PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
    );
    registry.mark_loaded(observer, (0, 0));
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        43,
        "minecraft:ender_dragon".to_owned(),
        Vec3::new(0.5, 64.0, 10.5),
    );
    let dragon = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:ender_dragon")
        .expect("spawned dragon")
        .snapshot;
    let state = dragon.retained.dragon_air.unwrap_or_else(|| {
        mc_entity::dragon_26_1_2::DragonAirState::new(dragon.position, dragon.rotation.yaw)
    });
    let neck = mc_entity::dragon_26_1_2::part_center(
        &state,
        dragon.position,
        dragon.rotation.yaw,
        mc_entity::dragon_26_1_2::DragonPart::Neck,
    )
    .expect("dragon neck position");
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let (boundary, mut endpoint) = damage_boundary();
    registry.install_precommit_boundary(boundary);
    registry.install_damage_precommit_handle(handle);

    assert!(matches!(
        registry.player_attack_server_entity(
            &SimulationAuthority::for_test(),
            crate::play::session::entity_combat::ServerEntityPlayerAttack {
                entity_id: EntityId(dragon.id.0 + 2),
                amount: 8.0,
                game_mode: GameMode::Survival,
                player_pose: PlayerPose::new(neck.x, neck.y, neck.z),
                attacker: None,
            },
        ),
        PlayerAttackResult::AcceptedNoDamage
    ));
    answer_next(&mut endpoint, HookDecision::Cancel);
    assert!(owner.wait_for_command().await);
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(
        registry
            .lock_entities("verify cancelled dragon damage")
            .snapshot(dragon.id)
            .expect("dragon remains after cancelled hit"),
        dragon
    );
}
