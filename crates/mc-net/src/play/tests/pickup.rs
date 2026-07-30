use std::sync::Arc;

use mc_data::items::ItemReport;

use super::{
    EntityId, EntityItemStack, EntityPhysicsStep, ITEM_PICKUP_DELAY_TICKS, Identifier,
    InteractionState, ItemRegistry, ItemStack, PlayerInventory, PlayerPose, Rotation, Vec3,
    XpState, decode_container_set_slot_packets, interaction_state_for_items, pickup_nearby_arrows,
    pickup_nearby_items, pickup_nearby_xp, register_interaction_player, simulation,
    spawn_test_simulation_owner,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_pickup_tasks_conserve_item_and_xp_entities() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let mut alice = interaction_state_for_items(Arc::clone(&items));
    let mut bob = interaction_state_for_items(items);
    bob.sessions = Arc::clone(&alice.sessions);
    let (simulation, simulation_stop_tx, simulation_task) =
        spawn_test_simulation_owner(Arc::clone(&alice.sessions));
    let simulation_probe = simulation.clone();
    let alice_id = register_interaction_player(&mut alice, "PickupTaskAlice");
    let bob_id = register_interaction_player(&mut bob, "PickupTaskBob");
    alice.simulation = simulation.for_session(alice_id);
    bob.simulation = simulation.for_session(bob_id);
    alice.sessions.spawn_item_drop(
        1,
        Vec3::new(0.5, 64.0, 0.5),
        EntityItemStack::new(dirt_id, 3),
    );
    alice.sessions.advance_world_time(ITEM_PICKUP_DELAY_TICKS);

    let item_gate = Arc::new(tokio::sync::Barrier::new(3));
    let alice_item_task = {
        let gate = Arc::clone(&item_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            gate.wait().await;
            pickup_nearby_items(&mut alice, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
                .await
                .expect("Alice item pickup task succeeds");
            alice
        })
    };
    let bob_item_task = {
        let gate = Arc::clone(&item_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            gate.wait().await;
            pickup_nearby_items(&mut bob, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
                .await
                .expect("Bob item pickup task succeeds");
            bob
        })
    };
    item_gate.wait().await;
    let mut alice = alice_item_task.await.expect("Alice item task joins");
    let mut bob = bob_item_task.await.expect("Bob item task joins");
    let inventory_item_count = |state: &InteractionState| {
        state
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == dirt_id)
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    assert_eq!(inventory_item_count(&alice) + inventory_item_count(&bob), 3);
    assert!(
        alice
            .sessions
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );

    alice.sessions.spawn_xp_orb(2, Vec3::new(0.5, 64.0, 0.5), 5);
    let xp_gate = Arc::new(tokio::sync::Barrier::new(3));
    let alice_xp_task = {
        let gate = Arc::clone(&xp_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut xp = XpState::default();
            gate.wait().await;
            pickup_nearby_xp(
                &mut alice,
                &mut writer,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
            )
            .await
            .expect("Alice XP pickup task succeeds");
            (alice, xp)
        })
    };
    let bob_xp_task = {
        let gate = Arc::clone(&xp_gate);
        tokio::spawn(async move {
            let mut writer = Vec::new();
            let mut xp = XpState::default();
            gate.wait().await;
            pickup_nearby_xp(
                &mut bob,
                &mut writer,
                &mut xp,
                PlayerPose::new(0.5, 64.0, 0.5),
            )
            .await
            .expect("Bob XP pickup task succeeds");
            (bob, xp)
        })
    };
    xp_gate.wait().await;
    let (alice, alice_xp) = alice_xp_task.await.expect("Alice XP task joins");
    let (_bob, bob_xp) = bob_xp_task.await.expect("Bob XP task joins");
    let _ = simulation_stop_tx.send(());
    simulation_task.await.expect("simulation owner joins");
    assert_eq!(alice_xp.total + bob_xp.total, 5);
    assert!(
        alice
            .sessions
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
    let simulation_snapshot = simulation_probe.snapshot();
    assert!(simulation_snapshot.enqueued >= 2);
    assert_eq!(simulation_snapshot.enqueued, simulation_snapshot.processed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grounded_arrow_pickup_credits_owner_inventory_and_writes_slot() {
    let arrow = Identifier::parse("minecraft:arrow").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: arrow.clone(),
        protocol_id: 10,
    }]));
    let arrow_item_id = items.id_of(&arrow).unwrap();
    let mut state = interaction_state_for_items(items);
    state.sessions.spawn_arrow_for_test(
        None,
        3,
        Vec3::new(1.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    );
    let arrow_entity_id = state
        .sessions
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:arrow")
        .unwrap()
        .snapshot
        .id;
    state.sessions.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: arrow_entity_id,
            position: Vec3::new(1.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    let session_id = register_interaction_player(&mut state, "ArrowPickupConnection");
    let (simulation, stop_tx, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    pickup_nearby_arrows(&mut state, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
        .await
        .unwrap();
    let _ = stop_tx.send(());
    task.await.unwrap();

    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(arrow_item_id, 1)
    );
    assert!(
        state
            .sessions
            .server_entity_snapshot(arrow_entity_id)
            .is_none()
    );
    let packets = decode_container_set_slot_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].slot, PlayerInventory::HOTBAR_BASE as i16);
    assert_eq!(packets[0].item_stack, ItemStack::new(arrow_item_id, 1));
}

#[tokio::test]
async fn full_simulation_queue_leaves_item_pickup_state_unchanged() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    register_interaction_player(&mut state, "FullQueuePickup");
    let (simulation, simulation_owner) = simulation::simulation_channel_with_capacity(1);
    let _blocked = simulation
        .enqueue(simulation::SimulationCommand::ClaimExperiencePickup {
            entity_id: EntityId(999),
            collector_session: 1,
        })
        .unwrap();
    state.simulation = simulation.for_session(state.session_id);
    state.sessions.spawn_item_drop(
        1,
        Vec3::new(0.5, 64.0, 0.5),
        EntityItemStack::new(dirt_id, 3),
    );
    state.sessions.advance_world_time(ITEM_PICKUP_DELAY_TICKS);

    let mut writer = Vec::new();
    pickup_nearby_items(&mut state, &mut writer, PlayerPose::new(0.5, 64.0, 0.5))
        .await
        .expect("queue pressure is a fail-closed no-pickup");

    assert_eq!(
        state
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count)
            .sum::<i32>(),
        0
    );
    assert_eq!(
        state
            .sessions
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)[0]
            .item_stack
            .as_ref()
            .unwrap()
            .count,
        3
    );
    assert_eq!(simulation.snapshot().rejected_full, 1);
    drop(simulation_owner);
}
