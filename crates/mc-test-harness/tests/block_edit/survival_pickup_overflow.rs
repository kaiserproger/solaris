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

    let mut inventory_snapshot = None;
    let mut expected_main_dirt = 0_i32;
    for count in [255_i32, 200] {
        client
            .write_packet(&ServerboundChatCommand {
                command: format!("debug give minecraft:dirt {count} 1"),
            })
            .await
            .expect("give bounded main-inventory filler");
        let filled_hotbar = wait_for_container_slot(&mut client, 0, 37, |stack| {
            stack.item_id == dirt_id && stack.count == count
        })
        .await;
        client
            .write_packet(&ServerboundContainerClick {
                container_id: 0,
                state_id: filled_hotbar.state_id,
                slot_num: 37,
                button_num: 0,
                container_input: ContainerInput::QuickMove,
                changed_slots: Vec::new(),
                carried_item: HashedStack::empty(),
            })
            .await
            .expect("quick-move bounded filler into main inventory");
        expected_main_dirt += count;
        inventory_snapshot = Some(
            wait_for_inventory_content(&mut client, |packet| {
                packet.container_id == 0
                    && packet.items.get(37).is_some_and(|stack| stack.is_empty())
                    && packet.items[9..=35]
                        .iter()
                        .filter(|stack| stack.item_id == dirt_id)
                        .map(|stack| stack.count)
                        .sum::<i32>()
                        == expected_main_dirt
            })
            .await,
        );
    }
    let mut target_inventory = inventory_snapshot
        .expect("bounded filler must publish inventory content")
        .items;

    let (mut dropper, _) = connect_to_play(addr, "PartialDropper").await;
    drain_until_chunk(&mut dropper, (0, 0)).await;
    for _ in 0..7 {
        dropper
            .write_packet(&ServerboundChatCommand {
                command: "debug give minecraft:dirt 255 0".into(),
            })
            .await
            .expect("seed bounded dirt filler entity");
        let _ = wait_for_container_slot(&mut dropper, 0, 36, |stack| {
            stack.item_id == dirt_id && stack.count == 255
        })
        .await;
        dropper
            .write_packet(&ServerboundPlayerAction {
                action: PlayerActionKind::DropAllItems,
                position: 0,
                direction: Direction::Down,
                sequence: 400,
            })
            .await
            .expect("drop bounded dirt filler entity");
        let _ = wait_for_container_slot(&mut dropper, 0, 36, |stack| stack.is_empty()).await;
        collect_full_item_drop_into_inventory(
            &mut client,
            item_entity_type,
            dirt_id,
            &mut target_inventory,
            255,
        )
        .await;
    }

    assert_eq!(target_inventory[36].item_id, cobblestone_id);
    assert_eq!(target_inventory[36].count, 63);
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
    let _ = wait_for_container_slot(&mut dropper, 0, 36, |stack| stack.is_empty()).await;

    let mut item_entity_id = None;
    let mut item_position = None;
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
                item_position = Some((packet.x, packet.y, packet.z));
                move_into_item_touch_box(&mut client, packet.x, packet.y, packet.z).await;
            }
        } else if frame.id == MoveEntityPos::ID {
            let mut body = frame.body;
            let packet = MoveEntityPos::decode(&mut body).expect("decode partial pickup relative move");
            if Some(packet.entity_id) == item_entity_id
                && let Some((x, y, z)) = item_position.as_mut()
            {
                *x += f64::from(packet.delta_x) / 4096.0;
                *y += f64::from(packet.delta_y) / 4096.0;
                *z += f64::from(packet.delta_z) / 4096.0;
                move_into_item_touch_box(&mut client, *x, *y, *z).await;
            }
        } else if frame.id == MoveEntityPosRot::ID {
            let mut body = frame.body;
            let packet =
                MoveEntityPosRot::decode(&mut body).expect("decode partial pickup relative move+rot");
            if Some(packet.entity_id) == item_entity_id
                && let Some((x, y, z)) = item_position.as_mut()
            {
                *x += f64::from(packet.delta_x) / 4096.0;
                *y += f64::from(packet.delta_y) / 4096.0;
                *z += f64::from(packet.delta_z) / 4096.0;
                move_into_item_touch_box(&mut client, *x, *y, *z).await;
            }
        } else if frame.id == EntityPositionSync::ID {
            let mut body = frame.body;
            let packet =
                EntityPositionSync::decode(&mut body).expect("decode partial pickup position sync");
            if Some(packet.entity_id) == item_entity_id {
                let position = packet.values.position;
                item_position = Some((position.x, position.y, position.z));
                move_into_item_touch_box(&mut client, position.x, position.y, position.z).await;
            }
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let packet = SynchronizePlayerPosition::decode(&mut body)
                .expect("decode unexpected partial pickup movement correction");
            panic!("walking into partial pickup range was corrected: {packet:?}");
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
        if frame.id == MoveEntityPos::ID {
            let mut body = frame.body;
            let packet = MoveEntityPos::decode(&mut body).expect("decode partial remainder move");
            if packet.entity_id == item_entity_id
                && let Some((x, y, z)) = item_position.as_mut()
            {
                *x += f64::from(packet.delta_x) / 4096.0;
                *y += f64::from(packet.delta_y) / 4096.0;
                *z += f64::from(packet.delta_z) / 4096.0;
                move_into_item_touch_box(&mut client, *x, *y, *z).await;
            }
        } else if frame.id == MoveEntityPosRot::ID {
            let mut body = frame.body;
            let packet =
                MoveEntityPosRot::decode(&mut body).expect("decode partial remainder move+rot");
            if packet.entity_id == item_entity_id
                && let Some((x, y, z)) = item_position.as_mut()
            {
                *x += f64::from(packet.delta_x) / 4096.0;
                *y += f64::from(packet.delta_y) / 4096.0;
                *z += f64::from(packet.delta_z) / 4096.0;
                move_into_item_touch_box(&mut client, *x, *y, *z).await;
            }
        } else if frame.id == EntityPositionSync::ID {
            let mut body = frame.body;
            let packet =
                EntityPositionSync::decode(&mut body).expect("decode partial remainder position sync");
            if packet.entity_id == item_entity_id {
                let position = packet.values.position;
                item_position = Some((position.x, position.y, position.z));
                move_into_item_touch_box(&mut client, position.x, position.y, position.z).await;
            }
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let packet = SynchronizePlayerPosition::decode(&mut body)
                .expect("decode unexpected partial remainder movement correction");
            panic!("walking after partial item was corrected: {packet:?}");
        } else if frame.id == ClientboundSetTime::ID {
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

fn inventory_item_count(items: &[ItemStack], item_id: u32) -> i32 {
    items[9..=44]
        .iter()
        .filter(|stack| stack.item_id == item_id)
        .map(|stack| stack.count)
        .sum()
}

async fn collect_full_item_drop_into_inventory(
    client: &mut Client,
    item_entity_type: i32,
    item_id: u32,
    target_inventory: &mut Vec<ItemStack>,
    expected_added: i32,
) {
    let before = inventory_item_count(target_inventory, item_id);
    let expected_total = before + expected_added;
    let mut item_entity_id = None;
    let mut item_position = None;
    let mut saw_matching_stack = false;
    let mut saw_take_or_remove = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if saw_matching_stack
            && saw_take_or_remove
            && inventory_item_count(target_inventory, item_id) == expected_total
        {
            return;
        }
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("full filler item pickup completion");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode filler AddEntity");
            if packet.entity_type_id == item_entity_type && item_entity_id.is_none() {
                item_entity_id = Some(packet.entity_id);
                item_position = Some((packet.x, packet.y, packet.z));
                move_into_item_touch_box(client, packet.x, packet.y, packet.z).await;
            }
        } else if frame.id == MoveEntityPos::ID {
            let mut body = frame.body;
            let packet = MoveEntityPos::decode(&mut body).expect("decode filler relative move");
            if Some(packet.entity_id) == item_entity_id
                && let Some((x, y, z)) = item_position.as_mut()
            {
                *x += f64::from(packet.delta_x) / 4096.0;
                *y += f64::from(packet.delta_y) / 4096.0;
                *z += f64::from(packet.delta_z) / 4096.0;
                move_into_item_touch_box(client, *x, *y, *z).await;
            }
        } else if frame.id == MoveEntityPosRot::ID {
            let mut body = frame.body;
            let packet = MoveEntityPosRot::decode(&mut body).expect("decode filler relative move+rot");
            if Some(packet.entity_id) == item_entity_id
                && let Some((x, y, z)) = item_position.as_mut()
            {
                *x += f64::from(packet.delta_x) / 4096.0;
                *y += f64::from(packet.delta_y) / 4096.0;
                *z += f64::from(packet.delta_z) / 4096.0;
                move_into_item_touch_box(client, *x, *y, *z).await;
            }
        } else if frame.id == EntityPositionSync::ID {
            let mut body = frame.body;
            let packet = EntityPositionSync::decode(&mut body).expect("decode filler position sync");
            if Some(packet.entity_id) == item_entity_id {
                let position = packet.values.position;
                item_position = Some((position.x, position.y, position.z));
                move_into_item_touch_box(client, position.x, position.y, position.z).await;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode filler item metadata");
            if Some(packet.entity_id) == item_entity_id {
                saw_matching_stack |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == item_id
                                && stack.count == expected_added
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode filler inventory slot");
            if packet.container_id == 0
                && let Ok(slot) = usize::try_from(packet.slot)
                && let Some(target) = target_inventory.get_mut(slot)
            {
                *target = packet.item_stack;
            }
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode filler inventory content");
            if packet.container_id == 0 {
                *target_inventory = packet.items;
            }
        } else if frame.id == ClientboundTakeItemEntity::ID {
            let mut body = frame.body;
            let packet = ClientboundTakeItemEntity::decode(&mut body)
                .expect("decode filler TakeItem");
            saw_take_or_remove |= Some(packet.item_entity_id) == item_entity_id;
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode filler removal");
            saw_take_or_remove |= item_entity_id
                .is_some_and(|entity_id| packet.entity_ids.contains(&entity_id));
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let packet = SynchronizePlayerPosition::decode(&mut body)
                .expect("decode unexpected filler movement correction");
            panic!("walking into filler pickup range was corrected: {packet:?}");
        }
    }
}
