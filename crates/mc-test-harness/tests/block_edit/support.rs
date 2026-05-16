use super::*;

pub(super) async fn connect_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    (client, sync)
}

pub(super) async fn drain_until_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drain chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            if (pkt.chunk_x, pkt.chunk_z) == target {
                return;
            }
        }
    }
}

pub(super) async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let mut body = body.clone();
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo KeepAlive");
    true
}

pub(super) async fn read_ack_without_target_update(
    client: &mut Client,
    sequence: i32,
    target: (i32, i32, i32),
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("ack before target update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate before ack");
            let pos = unpack_block_pos(pkt.position);
            assert_ne!(
                pos, target,
                "survival break mutated before timed completion"
            );
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_food_level(client: &mut Client, food: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("food level update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if pkt.food == food {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_health_level(client: &mut Client, health: f32) {
    wait_for_health_near(client, health, f32::EPSILON).await;
}

pub(super) async fn wait_for_health_near(client: &mut Client, health: f32, tolerance: f32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("health level update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if (pkt.health - health).abs() <= tolerance {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_death_inventory_drop(
    client: &mut Client,
    item_entity_type: i32,
    item_id: u32,
    count: i32,
) {
    let mut item_entities = HashSet::new();
    let mut saw_drop_stack = false;
    let mut saw_inventory_clear = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_drop_stack && saw_inventory_clear) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("death inventory drop");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode death drop AddEntity");
            if pkt.entity_type_id == item_entity_type {
                item_entities.insert(pkt.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body).expect("decode death drop data");
            if item_entities.contains(&pkt.entity_id) {
                saw_drop_stack |= pkt.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == item_id
                                && stack.count == count
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode death inventory clear");
            saw_inventory_clear |= pkt.container_id == 0
                && pkt.items.get(36).is_some_and(|stack| stack.is_empty())
                && pkt.carried_item.is_empty();
        }
    }
}

pub(super) async fn read_ack_without_food_or_slot_change(
    client: &mut Client,
    sequence: i32,
    item_id: u32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("dead use item ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                pkt.item_stack.item_id != item_id || pkt.item_stack.count != 1,
                "dead use item must not consume the held food stack"
            );
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            assert_ne!(pkt.food, 20, "dead use item must not restore food");
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                return;
            }
        }
    }
}

pub(super) async fn assert_no_food_or_slot_change(
    client: &mut Client,
    item_id: u32,
    duration: Duration,
) {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(frame) => frame,
            Err(_) => return,
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                pkt.item_stack.item_id != item_id || pkt.item_stack.count != 1,
                "canceled use item must not consume the held food stack"
            );
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            assert_ne!(pkt.food, 20, "canceled use item must not restore food");
        }
    }
}

pub(super) async fn wait_for_slot_stack(client: &mut Client, item_id: u32, count: i32) {
    let _ = wait_for_slot_stack_update(client, item_id, count).await;
}

pub(super) async fn wait_for_slot_stack_update(
    client: &mut Client,
    item_id: u32,
    count: i32,
) -> ClientboundContainerSetSlot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("slot stack update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.item_stack.item_id == item_id && pkt.item_stack.count == count {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_slot_damage(
    client: &mut Client,
    slot: i16,
    item_id: u32,
    damage: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("slot damage update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.slot == slot
                && pkt.item_stack.item_id == item_id
                && pkt.item_stack.damage == Some(damage)
            {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_container_slot(
    client: &mut Client,
    container_id: i32,
    slot: i16,
    predicate: impl Fn(&mc_protocol::packets::play::ItemStack) -> bool,
) -> ClientboundContainerSetSlot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("container slot update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.container_id == container_id && pkt.slot == slot && predicate(&pkt.item_stack) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_inventory_content(
    client: &mut Client,
    predicate: impl Fn(&ClientboundContainerSetContent) -> bool,
) -> ClientboundContainerSetContent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("inventory content update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body).expect("decode SetContent");
            if predicate(&pkt) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_open_screen(
    client: &mut Client,
    menu_type: i32,
) -> ClientboundOpenScreen {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("open screen");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenScreen::ID {
            let mut body = frame.body;
            let pkt = ClientboundOpenScreen::decode(&mut body).expect("decode OpenScreen");
            if pkt.menu_type == menu_type {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_furnace_content(
    client: &mut Client,
    container_id: i32,
    predicate: impl Fn(&ClientboundContainerSetContent) -> bool,
) -> ClientboundContainerSetContent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("furnace content update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode furnace SetContent");
            if pkt.container_id == container_id && predicate(&pkt) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_furnace_data(
    client: &mut Client,
    container_id: i32,
    data_id: i16,
    predicate: impl Fn(i16) -> bool,
) -> ClientboundContainerSetData {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("furnace data update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetData::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetData::decode(&mut body).expect("decode SetData");
            if pkt.container_id == container_id && pkt.id == data_id && predicate(pkt.value) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_block_update(
    client: &mut Client,
    pos: (i32, i32, i32),
    state_id: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("block update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            if unpack_block_pos(pkt.position) == pos && pkt.state_id == state_id {
                return;
            }
        }
    }
}

pub(super) fn mask_to_u64(longs: &[i64]) -> u64 {
    longs.first().copied().unwrap_or(0) as u64
}
