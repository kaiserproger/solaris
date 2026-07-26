#[tokio::test]
async fn near_full_inventory_partially_picks_up_and_preserves_remainder_identity() {
    let data = embedded_play_data();
    let dirt_id = embedded_item_id(&data, "minecraft:dirt");
    let cobblestone_id = embedded_item_id(&data, "minecraft:cobblestone");
    let item_entity_type = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded item entity type");
    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(
        &data,
        embedded_world(&data),
        "partial pickup overflow conservation",
    );
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind partial pickup server");
    let addr = bound.local_addr().expect("partial pickup local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "PartialPickup").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    for expected_full_main_slots in 1..=27 {
        client
            .write_packet(&ServerboundChatCommand {
                command: "debug give minecraft:dirt 64 0".into(),
            })
            .await
            .expect("give main-inventory filler");
        let filled_hotbar = wait_for_container_slot(&mut client, 0, 36, |stack| {
            stack.item_id == dirt_id && stack.count == 64
        })
        .await;
        client
            .write_packet(&ServerboundContainerClick {
                container_id: 0,
                state_id: filled_hotbar.state_id,
                slot_num: 36,
                button_num: 0,
                container_input: ContainerInput::QuickMove,
                changed_slots: Vec::new(),
                carried_item: HashedStack::empty(),
            })
            .await
            .expect("quick-move filler into main inventory");
        let _ = wait_for_inventory_content(&mut client, |packet| {
            packet.container_id == 0
                && packet.items.get(36).is_some_and(|stack| stack.is_empty())
                && packet.items[9..=35]
                    .iter()
                    .filter(|stack| stack.item_id == dirt_id && stack.count == 64)
                    .count()
                    == expected_full_main_slots
        })
        .await;
    }

    for hotbar_slot in 1_u8..=8 {
        client
            .write_packet(&ServerboundChatCommand {
                command: format!("debug give minecraft:dirt 64 {hotbar_slot}"),
            })
            .await
            .expect("fill hotbar filler slot");
        let _ = wait_for_container_slot(
            &mut client,
            0,
            i16::from(36 + hotbar_slot),
            |stack| stack.item_id == dirt_id && stack.count == 64,
        )
        .await;
    }
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:cobblestone 63 0".into(),
        })
        .await
        .expect("seed one admissible cobblestone capacity");
    let _ = wait_for_container_slot(&mut client, 0, 36, |stack| {
        stack.item_id == cobblestone_id && stack.count == 63
    })
    .await;

    let (mut dropper, _) = connect_to_play(addr, "PartialDropper").await;
    drain_until_chunk(&mut dropper, (0, 0)).await;
    dropper
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:cobblestone 3 0".into(),
        })
        .await
        .expect("seed partial pickup entity stack");
    let _ = wait_for_container_slot(&mut dropper, 0, 36, |stack| {
        stack.item_id == cobblestone_id && stack.count == 3
    })
    .await;
    dropper
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::DropAllItems,
            position: 0,
            direction: Direction::Down,
            sequence: 401,
        })
        .await
        .expect("drop complete partial pickup fixture");

    let mut item_entity_id = None;
    let mut saw_stack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !saw_stack {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("partial pickup fixture visibility");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode partial pickup AddEntity");
            if packet.entity_type_id == item_entity_type {
                item_entity_id = Some(packet.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode partial pickup fixture metadata");
            if Some(packet.entity_id) == item_entity_id {
                saw_stack |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == cobblestone_id
                                && stack.count == 3
                    )
                });
            }
        }
    }
    let item_entity_id = item_entity_id.expect("partial pickup entity identity");

    let mut time_baseline = None;
    let mut saw_ready_window = false;
    let mut saw_one_item_credit = false;
    let mut saw_same_entity_remainder = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_ready_window && saw_one_item_credit && saw_same_entity_remainder) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("partial pickup credit and remainder");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let packet = ClientboundSetTime::decode(&mut body)
                .expect("decode partial pickup tick fence");
            let baseline = *time_baseline.get_or_insert(packet.game_time);
            saw_ready_window |= packet.game_time.saturating_sub(baseline) >= 6;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode partial pickup inventory credit");
            if packet.slot == 36 {
                assert_eq!(packet.item_stack.item_id, cobblestone_id);
                assert_eq!(packet.item_stack.count, 64);
                saw_one_item_credit = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode partial pickup remainder metadata");
            if packet.entity_id == item_entity_id {
                let remainder = packet.values.iter().find_map(|value| match value {
                    EntityDataValue::ItemStack { index, stack }
                        if *index == ITEM_ENTITY_DATA_ITEM_INDEX => Some(stack),
                    _ => None,
                });
                let remainder = remainder.expect("remainder item metadata");
                assert_eq!(remainder.item_id, cobblestone_id);
                assert_eq!(remainder.count, 2);
                saw_same_entity_remainder = true;
            }
        } else if frame.id == ClientboundTakeItemEntity::ID {
            let mut body = frame.body;
            let packet = ClientboundTakeItemEntity::decode(&mut body)
                .expect("decode unexpected partial pickup take");
            assert_ne!(
                packet.item_entity_id, item_entity_id,
                "partial pickup must not publish full take animation"
            );
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body)
                .expect("decode unexpected partial pickup removal");
            assert!(
                !packet.entity_ids.contains(&item_entity_id),
                "partial pickup must preserve remainder entity identity"
            );
        }
    }

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("partial pickup server shutdown")
        .expect("partial pickup server join")
        .expect("partial pickup server serve");
}
