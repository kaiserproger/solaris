#[tokio::test]
async fn embedded_cows_breed_from_two_wheat_interactions_over_wire() {
    embedded_animals_breed_from_two_food_interactions_over_wire(
        "minecraft:cow",
        "minecraft:wheat",
    )
    .await;
}

#[tokio::test]
async fn embedded_chickens_breed_from_two_seed_interactions_over_wire() {
    embedded_animals_breed_from_two_food_interactions_over_wire(
        "minecraft:chicken",
        "minecraft:wheat_seeds",
    )
    .await;
}

#[tokio::test]
async fn embedded_adult_sheep_shearing_updates_tool_metadata_and_wool_over_wire() {
    let data = embedded_play_data();
    let shears_item_id = embedded_item_id(&data, "minecraft:shears");
    let wool_item_id = embedded_item_id(&data, "minecraft:white_wool");
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let sheep_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded sheep entity type");
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded item entity type");
    let cfg = embedded_playable_config(&data, embedded_world(&data), "sheep shearing wire");
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "SheepShearWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:shears 1 0".to_owned(),
        })
        .await
        .expect("give shears");
    wait_for_slot_stack(&mut client, shears_item_id, 1).await;

    let sheep_id = summon_wire_animal(
        &mut client,
        "minecraft:sheep",
        sheep_entity_type,
        (sync.x, sync.y, sync.z + 1.0),
    )
    .await;
    client
        .write_packet(&ServerboundInteract {
            entity_id: sheep_id,
            hand: InteractionHand::MainHand,
            location: EntityVec3::ZERO,
            using_secondary_action: false,
        })
        .await
        .expect("shear sheep");

    let mut item_entities = HashSet::new();
    let mut saw_sheared = false;
    let mut saw_damaged_shears = false;
    let mut saw_wool = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !saw_sheared || !saw_damaged_shears || !saw_wool {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("sheep shear result");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode shearing item entity");
            if packet.entity_type_id == item_entity_type {
                item_entities.insert(packet.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode sheep shearing metadata");
            if packet.entity_id == sheep_id {
                saw_sheared |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::Byte { index, value: 0x10 }
                            if *index == SHEEP_ENTITY_DATA_WOOL_INDEX
                    )
                });
            } else if item_entities.contains(&packet.entity_id) {
                saw_wool |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == wool_item_id
                                && stack.count == 1
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode damaged shears slot");
            saw_damaged_shears |= packet.container_id == 0
                && packet.slot == 36
                && packet.item_stack.item_id == shears_item_id
                && packet.item_stack.count == 1
                && packet.item_stack.damage == Some(1);
        }
    }
}

async fn embedded_animals_breed_from_two_food_interactions_over_wire(
    entity_name: &str,
    food_name: &str,
) {
    let data = embedded_play_data();
    let food_item_id = embedded_item_id(&data, food_name);
    let animal_entity_type = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse(entity_name).unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded animal entity type");
    let cfg = embedded_playable_config(&data, embedded_world(&data), "animal breeding wire");
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "AnimalBreedWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: format!("debug give {food_name} 2 0"),
        })
        .await
        .expect("give breeding food");
    wait_for_slot_stack(&mut client, food_item_id, 2).await;

    let first_position = (sync.x, sync.y, sync.z + 1.0);
    let second_position = (sync.x, sync.y, sync.z + 1.5);
    let first = summon_wire_animal(
        &mut client,
        entity_name,
        animal_entity_type,
        first_position,
    )
    .await;
    let second = summon_wire_animal(
        &mut client,
        entity_name,
        animal_entity_type,
        second_position,
    )
    .await;

    client
        .write_packet(&ServerboundInteract {
            entity_id: first,
            hand: InteractionHand::MainHand,
            location: EntityVec3::ZERO,
            using_secondary_action: false,
        })
        .await
        .expect("feed first cow");
    wait_for_wire_feed(&mut client, first, food_item_id, 1).await;

    client
        .write_packet(&ServerboundInteract {
            entity_id: second,
            hand: InteractionHand::MainHand,
            location: EntityVec3::ZERO,
            using_secondary_action: false,
        })
        .await
        .expect("feed second cow");
    wait_for_wire_feed(&mut client, second, food_item_id, 0).await;

    let mut child_id = None;
    let mut child_is_baby = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while child_id.is_none() || !child_is_baby {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("animal child spawn");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode animal child AddEntity");
            if packet.entity_type_id == animal_entity_type
                && packet.entity_id != first
                && packet.entity_id != second
            {
                child_id = Some(packet.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode animal child metadata");
            if Some(packet.entity_id) == child_id {
                child_is_baby = packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::Boolean { index, value: true }
                            if *index == AGEABLE_ENTITY_DATA_BABY_INDEX
                    )
                });
            }
        }
    }
}

async fn summon_wire_animal(
    client: &mut Client,
    entity_name: &str,
    animal_entity_type: i32,
    position: (f64, f64, f64),
) -> i32 {
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon {entity_name} {} {} {}",
                position.0, position.1, position.2,
            ),
        })
        .await
        .expect("summon animal");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("animal summon response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != AddEntity::ID {
            continue;
        }
        let mut body = frame.body;
        let packet = AddEntity::decode(&mut body).expect("decode animal AddEntity");
        if packet.entity_type_id == animal_entity_type
            && (packet.x - position.0).abs() < 0.01
            && (packet.y - position.1).abs() < 0.01
            && (packet.z - position.2).abs() < 0.01
        {
            return packet.entity_id;
        }
    }
}

async fn wait_for_wire_feed(
    client: &mut Client,
    entity_id: i32,
    food_item_id: u32,
    expected_count: i32,
) {
    let mut saw_event = false;
    let mut saw_slot = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !saw_event || !saw_slot {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("animal feed result");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == EntityEvent::ID {
            let mut body = frame.body;
            let packet = EntityEvent::decode(&mut body).expect("decode animal feed event");
            saw_event |= packet.entity_id == entity_id && packet.event_id == 18;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode animal feed slot update");
            saw_slot |= packet.container_id == 0
                && packet.slot == 36
                && if expected_count == 0 {
                    packet.item_stack.is_empty()
                } else {
                    packet.item_stack.item_id == food_item_id
                        && packet.item_stack.count == expected_count
                };
        }
    }
}
