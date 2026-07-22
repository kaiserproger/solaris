#[tokio::test]
async fn survival_tnt_fuse_destroys_adjacent_dirt_over_wire() {
    let data = embedded_play_data();
    let air = embedded_block_state(&data, "minecraft:air");
    let tnt = embedded_block_state(&data, "minecraft:tnt");
    let dirt = embedded_block_state(&data, "minecraft:dirt");
    let oak_log = embedded_block_state(&data, "minecraft:oak_log");
    let stone = embedded_block_state(&data, "minecraft:stone");
    let flint_and_steel = embedded_item_id(&data, "minecraft:flint_and_steel");
    let dirt_item = embedded_item_id(&data, "minecraft:dirt");
    let oak_log_item = embedded_item_id(&data, "minecraft:oak_log");
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let tnt_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:tnt").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded TNT entity type");
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded item entity type");

    let mut world = embedded_world(&data);
    let surface_y = top_non_air_y(&mut world, 0, 0, air).expect("spawn terrain");
    let tnt_pos = mc_world::BlockPos {
        x: 0,
        y: surface_y + 1,
        z: 1,
    };
    let dirt_pos = mc_world::BlockPos { x: 1, ..tnt_pos };
    let oak_log_pos = mc_world::BlockPos {
        z: tnt_pos.z + 1,
        ..tnt_pos
    };
    let dirt_drop_position = (
        f64::from(dirt_pos.x) + 0.5,
        f64::from(dirt_pos.y) + 0.5,
        f64::from(dirt_pos.z) + 0.5,
    );
    let oak_log_drop_position = (
        f64::from(oak_log_pos.x) + 0.5,
        f64::from(oak_log_pos.y) + 0.5,
        f64::from(oak_log_pos.z) + 0.5,
    );
    world.set_block_at(tnt_pos, tnt).expect("seed TNT");
    world
        .set_block_at(dirt_pos, dirt)
        .expect("seed oracle dirt");
    world
        .set_block_at(oak_log_pos, oak_log)
        .expect("seed repo-owned oak log drop");
    world
        .set_block_at(
            mc_world::BlockPos {
                y: dirt_pos.y + 1,
                ..dirt_pos
            },
            stone,
        )
        .expect("cover dirt so random grass spreading cannot invalidate the fuse precondition");

    let mut explosion_resistance = vec![0.0; 29_873];
    explosion_resistance[dirt.0 as usize] = 0.5;
    explosion_resistance[oak_log.0 as usize] = 2.0;
    explosion_resistance[stone.0 as usize] = 6.0;
    let block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&data.report)
        .with_explosion_table(
            mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
                .expect("test explosion resistance table"),
        );
    let mut cfg = embedded_playable_config(&data, world, "TNT fuse wire");
    cfg.block_facts = Arc::new(block_facts);
    cfg.random_tick.seed = 0;
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "TntFuseWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:flint_and_steel 1 0".into(),
        })
        .await
        .expect("give flint and steel");
    wait_for_slot_stack(&mut client, flint_and_steel, 1).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(tnt_pos.x, tnt_pos.y, tnt_pos.z),
            direction: Direction::North,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.0,
            inside: false,
            world_border_hit: false,
            sequence: 901,
        })
        .await
        .expect("ignite TNT");

    let mut primed_id = None;
    let mut saw_tnt_air = false;
    let mut saw_dirt_air = false;
    let mut saw_oak_log_air = false;
    let mut saw_ack = false;
    let mut saw_durability = false;
    let mut saw_remove = false;
    let mut saw_explosion = false;
    let mut saw_damage = false;
    let mut item_entity_ids = HashSet::new();
    let mut oak_log_entity_ids = HashSet::new();
    let mut dirt_drop_count = 0;
    let mut oak_log_drop_count = 0;
    let mut observed_block_updates = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while !(saw_tnt_air
        && primed_id.is_some()
        && saw_remove
        && saw_dirt_air
        && saw_oak_log_air
        && saw_damage
        && saw_explosion)
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "TNT fuse outcome: {error}; primed={primed_id:?} tnt_air={saw_tnt_air} dirt_air={saw_dirt_air} oak_log_air={saw_oak_log_air} ack={saw_ack} durability={saw_durability} remove={saw_remove} damage={saw_damage} explosion={saw_explosion} dirt_drops={dirt_drop_count} oak_log_drops={oak_log_drop_count} updates={observed_block_updates:?}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode TNT AddEntity");
            if packet.entity_type_id == tnt_entity_type {
                assert!((packet.x - (f64::from(tnt_pos.x) + 0.5)).abs() < 0.001);
                assert!((packet.y - f64::from(tnt_pos.y)).abs() < 0.001);
                assert!((packet.z - (f64::from(tnt_pos.z) + 0.5)).abs() < 0.001);
                if let Some(entity_id) = primed_id {
                    assert_eq!(packet.entity_id, entity_id, "duplicate primed TNT entity");
                } else {
                    primed_id = Some(packet.entity_id);
                }
            } else if packet.entity_type_id == item_entity_type
                && (packet.x - dirt_drop_position.0).abs() < 0.001
                && (packet.y - dirt_drop_position.1).abs() < 0.001
                && (packet.z - dirt_drop_position.2).abs() < 0.001
            {
                assert!(
                    saw_dirt_air,
                    "explosion loot must spawn only after the conditional block batch commits"
                );
                item_entity_ids.insert(packet.entity_id);
            } else if packet.entity_type_id == item_entity_type
                && (packet.x - oak_log_drop_position.0).abs() < 0.001
                && (packet.y - oak_log_drop_position.1).abs() < 0.001
                && (packet.z - oak_log_drop_position.2).abs() < 0.001
            {
                oak_log_entity_ids.insert(packet.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode exploded block drop metadata");
            if item_entity_ids.contains(&packet.entity_id) {
                dirt_drop_count += packet
                    .values
                    .iter()
                    .filter(|value| {
                        matches!(
                            value,
                            EntityDataValue::ItemStack { index, stack }
                                if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                    && stack.item_id == dirt_item
                                    && stack.count == 1
                        )
                    })
                    .count();
            } else if oak_log_entity_ids.contains(&packet.entity_id) {
                oak_log_drop_count += packet
                    .values
                    .iter()
                    .filter(|value| {
                        matches!(
                            value,
                            EntityDataValue::ItemStack { index, stack }
                                if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                    && stack.item_id == oak_log_item
                                    && stack.count == 1
                        )
                    })
                    .count();
            }
        } else if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode explosion BlockUpdate");
            let position = unpack_block_pos(packet.position);
            observed_block_updates.push((position, packet.state_id));
            saw_tnt_air |=
                position == (tnt_pos.x, tnt_pos.y, tnt_pos.z) && packet.state_id == air.0 as i32;
            saw_dirt_air |=
                position == (dirt_pos.x, dirt_pos.y, dirt_pos.z) && packet.state_id == air.0 as i32;
            saw_oak_log_air |= position == (oak_log_pos.x, oak_log_pos.y, oak_log_pos.z)
                && packet.state_id == air.0 as i32;
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let packet = SectionBlocksUpdate::decode(&mut body)
                .expect("decode batched explosion block updates");
            let section_pos = pack_section_pos(
                dirt_pos.x.div_euclid(16),
                dirt_pos.y.div_euclid(16),
                dirt_pos.z.div_euclid(16),
            );
            if packet.section_pos == section_pos {
                for change in packet.changes {
                    let is_air = change.state_id == air.0 as i32;
                    observed_block_updates.push((
                        (
                            dirt_pos.x.div_euclid(16) * 16
                                + i32::from((change.relative_pos >> 8) & 0x0f),
                            dirt_pos.y.div_euclid(16) * 16 + i32::from(change.relative_pos & 0x0f),
                            dirt_pos.z.div_euclid(16) * 16
                                + i32::from((change.relative_pos >> 4) & 0x0f),
                        ),
                        change.state_id,
                    ));
                    saw_tnt_air |= is_air
                        && change.relative_pos
                            == pack_section_relative_pos(tnt_pos.x, tnt_pos.y, tnt_pos.z);
                    saw_dirt_air |= is_air
                        && change.relative_pos
                            == pack_section_relative_pos(dirt_pos.x, dirt_pos.y, dirt_pos.z);
                    saw_oak_log_air |= is_air
                        && change.relative_pos
                            == pack_section_relative_pos(
                                oak_log_pos.x,
                                oak_log_pos.y,
                                oak_log_pos.z,
                            );
                }
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode TNT ack");
            saw_ack |= packet.sequence == 901;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode flint and steel durability");
            saw_durability |= packet.slot == 36
                && packet.item_stack.item_id == flint_and_steel
                && packet.item_stack.damage == Some(1);
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode TNT removal");
            saw_remove |= primed_id.is_some_and(|id| packet.entity_ids.contains(&id));
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet = ClientboundSetHealth::decode(&mut body).expect("decode TNT damage health");
            saw_damage |= packet.health < 20.0;
        } else if frame.id == ClientboundExplode::ID {
            assert!(
                saw_remove,
                "explosion packet must follow TNT entity removal"
            );
            assert_eq!(
                dirt_drop_count, 1,
                "the committed dirt block must produce exactly one explosion drop before completion"
            );
            assert_eq!(
                oak_log_drop_count, 1,
                "the committed oak log must produce exactly one explosion drop before completion"
            );
            let mut body = frame.body;
            let packet = ClientboundExplode::decode(&mut body).expect("decode TNT explosion");
            assert!(
                body.is_empty(),
                "explosion decoder must consume the full body"
            );
            assert!((packet.center.x - (f64::from(tnt_pos.x) + 0.5)).abs() < 0.001);
            assert!((packet.center.y - (f64::from(tnt_pos.y) + 0.06125)).abs() < 0.001);
            assert!((packet.center.z - (f64::from(tnt_pos.z) + 0.5)).abs() < 0.001);
            assert_eq!(packet.radius, 4.0);
            assert!(
                packet.block_count >= 2,
                "the explosion candidate set must include the two committed blocks"
            );
            let knockback = packet.knockback.expect("nearby survival player knockback");
            assert!(knockback.x.is_finite());
            assert!(knockback.y.is_finite());
            assert!(knockback.z.is_finite());
            assert!(
                knockback.x * knockback.x + knockback.y * knockback.y + knockback.z * knockback.z
                    > 0.0
            );
            assert_eq!(packet.explosion_particle_id, 22);
            assert_eq!(packet.sound_reference_id, 697);
            assert_eq!(packet.block_particles.len(), 2);
            assert_eq!(packet.block_particles[0].particle_id, 59);
            assert_eq!(packet.block_particles[0].scaling, 0.5);
            assert_eq!(packet.block_particles[0].speed, 1.0);
            assert_eq!(packet.block_particles[0].weight, 1);
            assert_eq!(packet.block_particles[1].particle_id, 62);
            assert_eq!(packet.block_particles[1].scaling, 1.0);
            assert_eq!(packet.block_particles[1].speed, 1.0);
            assert_eq!(packet.block_particles[1].weight, 1);
            saw_explosion = true;
        }
    }

    assert!(primed_id.is_some(), "primed TNT must be observable");
    assert!(saw_tnt_air, "ignited TNT block must become air");
    assert!(saw_ack, "use-item-on sequence must be acknowledged");
    assert!(saw_durability, "flint and steel must take one durability");
    assert!(saw_remove, "primed TNT must be removed on fuse expiry");
    assert!(saw_explosion, "fuse expiry must emit ClientboundExplode");
    assert!(
        saw_damage,
        "nearby survival player must take explosion damage"
    );
    assert!(saw_dirt_air, "the live adjacent dirt must be destroyed");
    assert_eq!(dirt_drop_count, 1, "the dirt block must not duplicate loot");
}

#[tokio::test]
async fn survival_tnt_explosion_primes_adjacent_tnt_over_wire() {
    let data = embedded_play_data();
    let air = embedded_block_state(&data, "minecraft:air");
    let tnt = embedded_block_state(&data, "minecraft:tnt");
    let dirt = embedded_block_state(&data, "minecraft:dirt");
    let stone = embedded_block_state(&data, "minecraft:stone");
    let flint_and_steel = embedded_item_id(&data, "minecraft:flint_and_steel");
    let dirt_item = embedded_item_id(&data, "minecraft:dirt");
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let tnt_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:tnt").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded TNT entity type");
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded item entity type");

    let mut world = embedded_world(&data);
    let surface_y = top_non_air_y(&mut world, 0, 0, air).expect("spawn terrain");
    let first_tnt = mc_world::BlockPos {
        x: 0,
        y: surface_y + 1,
        z: 1,
    };
    let chained_tnt = mc_world::BlockPos { x: 1, ..first_tnt };
    let dirt_pos = mc_world::BlockPos { x: 2, ..first_tnt };
    let dirt_drop_position = (
        f64::from(dirt_pos.x) + 0.5,
        f64::from(dirt_pos.y) + 0.5,
        f64::from(dirt_pos.z) + 0.5,
    );
    world.set_block_at(first_tnt, tnt).expect("seed first TNT");
    world
        .set_block_at(chained_tnt, tnt)
        .expect("seed chained TNT");
    world.set_block_at(dirt_pos, dirt).expect("seed chain dirt");

    let mut explosion_resistance = vec![0.0; 29_873];
    explosion_resistance[dirt.0 as usize] = 0.5;
    explosion_resistance[stone.0 as usize] = 6.0;
    let block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&data.report)
        .with_explosion_table(
            mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
                .expect("test explosion resistance table"),
        );
    let mut cfg = embedded_playable_config(&data, world, "TNT chain wire");
    cfg.block_facts = Arc::new(block_facts);
    cfg.random_tick.seed = 3;
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "TntChainWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:flint_and_steel 1 0".into(),
        })
        .await
        .expect("give flint and steel");
    wait_for_slot_stack(&mut client, flint_and_steel, 1).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(first_tnt.x, first_tnt.y, first_tnt.z),
            direction: Direction::North,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.0,
            inside: false,
            world_border_hit: false,
            sequence: 902,
        })
        .await
        .expect("ignite first TNT");

    let mut primed_ids = Vec::new();
    let mut removed_ids = HashSet::new();
    let mut chained_block_air = false;
    let mut dirt_block_air = false;
    let mut dirt_item_entities = HashSet::new();
    let mut dirt_drop_count = 0;
    let mut explosion_count = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while primed_ids.len() < 2
        || explosion_count < 2
        || !chained_block_air
        || !primed_ids.get(1).is_some_and(|id| removed_ids.contains(id))
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "TNT chain outcome: {error}; ids={primed_ids:?} removed={removed_ids:?} explosions={explosion_count} chained_air={chained_block_air}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode chained TNT AddEntity");
            if packet.entity_type_id == tnt_entity_type && !primed_ids.contains(&packet.entity_id) {
                if primed_ids.len() == 1 {
                    assert_eq!(
                        explosion_count, 1,
                        "chained TNT must spawn after first explosion"
                    );
                    assert!((packet.x - (f64::from(chained_tnt.x) + 0.5)).abs() < 0.001);
                    assert!((packet.y - f64::from(chained_tnt.y)).abs() < 0.001);
                    assert!((packet.z - (f64::from(chained_tnt.z) + 0.5)).abs() < 0.001);
                }
                primed_ids.push(packet.entity_id);
            } else if packet.entity_type_id == item_entity_type
                && (packet.x - dirt_drop_position.0).abs() < 0.001
                && (packet.y - dirt_drop_position.1).abs() < 0.001
                && (packet.z - dirt_drop_position.2).abs() < 0.001
            {
                assert!(
                    dirt_block_air,
                    "chain loot must follow the successful dirt block commit"
                );
                dirt_item_entities.insert(packet.entity_id);
            }
        } else if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode TNT chain BlockUpdate");
            chained_block_air |= unpack_block_pos(packet.position)
                == (chained_tnt.x, chained_tnt.y, chained_tnt.z)
                && packet.state_id == air.0 as i32;
            dirt_block_air |= unpack_block_pos(packet.position)
                == (dirt_pos.x, dirt_pos.y, dirt_pos.z)
                && packet.state_id == air.0 as i32;
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let packet =
                SectionBlocksUpdate::decode(&mut body).expect("decode TNT chain section update");
            let section_pos = pack_section_pos(
                chained_tnt.x.div_euclid(16),
                chained_tnt.y.div_euclid(16),
                chained_tnt.z.div_euclid(16),
            );
            if packet.section_pos == section_pos {
                chained_block_air |= packet.changes.iter().any(|change| {
                    change.state_id == air.0 as i32
                        && change.relative_pos
                            == pack_section_relative_pos(
                                chained_tnt.x,
                                chained_tnt.y,
                                chained_tnt.z,
                            )
                });
                dirt_block_air |= packet.changes.iter().any(|change| {
                    change.state_id == air.0 as i32
                        && change.relative_pos
                            == pack_section_relative_pos(dirt_pos.x, dirt_pos.y, dirt_pos.z)
                });
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode chained explosion dirt metadata");
            if dirt_item_entities.contains(&packet.entity_id) {
                dirt_drop_count += packet
                    .values
                    .iter()
                    .filter(|value| {
                        matches!(
                            value,
                            EntityDataValue::ItemStack { index, stack }
                                if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                    && stack.item_id == dirt_item
                                    && stack.count == 1
                        )
                    })
                    .count();
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode TNT chain removal");
            removed_ids.extend(packet.entity_ids);
        } else if frame.id == ClientboundExplode::ID {
            let expected_id = primed_ids
                .get(explosion_count)
                .copied()
                .expect("explosion must correspond to an observed primed TNT");
            assert!(
                removed_ids.contains(&expected_id),
                "each explosion packet must follow its entity removal"
            );
            let mut body = frame.body;
            let packet = ClientboundExplode::decode(&mut body).expect("decode chained explosion");
            assert!(body.is_empty());
            assert_eq!(packet.radius, 4.0);
            explosion_count += 1;
            if explosion_count == 2 {
                assert_eq!(
                    dirt_drop_count, 1,
                    "two chained explosions must not duplicate one committed dirt drop"
                );
            }
        }
    }

    assert_eq!(primed_ids.len(), 2);
    assert!(chained_block_air);
    assert_eq!(explosion_count, 2);
    assert!(dirt_block_air);
    assert_eq!(dirt_drop_count, 1);
}

#[tokio::test]
async fn survival_tnt_explosion_damages_mob_over_wire() {
    let data = embedded_play_data();
    let air = embedded_block_state(&data, "minecraft:air");
    let tnt = embedded_block_state(&data, "minecraft:tnt");
    let flint_and_steel = embedded_item_id(&data, "minecraft:flint_and_steel");
    let raw_chicken = embedded_item_id(&data, "minecraft:chicken");
    let entity_types = mc_data::entity_types::solaris_required_entity_types();
    let entity_type_id = |name: &str| {
        entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .and_then(|id| i32::try_from(id).ok())
            .unwrap_or_else(|| panic!("embedded {name} entity type"))
    };
    let tnt_entity_type = entity_type_id("minecraft:tnt");
    let chicken_entity_type = entity_type_id("minecraft:chicken");
    let item_entity_type = entity_type_id("minecraft:item");
    let experience_orb_entity_type = entity_type_id("minecraft:experience_orb");

    let mut world = embedded_world(&data);
    let surface_y = top_non_air_y(&mut world, 0, 0, air).expect("spawn terrain");
    let tnt_pos = mc_world::BlockPos {
        x: 0,
        y: surface_y + 1,
        z: 1,
    };
    world.set_block_at(tnt_pos, tnt).expect("seed TNT");

    let explosion_resistance = vec![100.0; 29_873];
    let block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&data.report)
        .with_explosion_table(
            mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
                .expect("test explosion resistance table"),
        );
    let mut cfg = embedded_playable_config(&data, world, "TNT mob damage wire");
    cfg.block_facts = Arc::new(block_facts);
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "TntMobWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:flint_and_steel 1 0".into(),
        })
        .await
        .expect("give flint and steel");
    wait_for_slot_stack(&mut client, flint_and_steel, 1).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(tnt_pos.x, tnt_pos.y, tnt_pos.z),
            direction: Direction::North,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.0,
            inside: false,
            world_border_hit: false,
            sequence: 903,
        })
        .await
        .expect("ignite TNT");

    let explosion_center = (
        f64::from(tnt_pos.x) + 0.5,
        f64::from(tnt_pos.y) + 0.06125,
        f64::from(tnt_pos.z) + 0.5,
    );
    let chicken_position = (explosion_center.0, f64::from(tnt_pos.y), explosion_center.2);
    let mut summon_sent = false;
    let mut primed_id = None;
    let mut chicken_id = None;
    let mut item_entity_ids = HashSet::new();
    let mut saw_tnt_remove = false;
    let mut saw_chicken_remove = false;
    let mut saw_explosion = false;
    let mut saw_raw_chicken_drop = false;
    let mut saw_experience_orb = false;
    let mut explosion_game_time = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while !(saw_tnt_remove
        && saw_chicken_remove
        && saw_explosion
        && saw_raw_chicken_drop
        && saw_experience_orb)
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "TNT mob lifecycle: {error}; summon={summon_sent} primed={primed_id:?} chicken={chicken_id:?} tnt_remove={saw_tnt_remove} chicken_remove={saw_chicken_remove} explosion={saw_explosion} raw_chicken={saw_raw_chicken_drop} xp={saw_experience_orb}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }

        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode TNT mob AddEntity");
            if packet.entity_type_id == tnt_entity_type {
                assert_eq!(
                    primed_id.replace(packet.entity_id),
                    None,
                    "duplicate primed TNT"
                );
                client
                    .write_packet(&ServerboundChatCommand {
                        command: format!(
                            "summon minecraft:chicken {} {} {}",
                            chicken_position.0, chicken_position.1, chicken_position.2
                        ),
                    })
                    .await
                    .expect("summon chicken in primed TNT");
                summon_sent = true;
            } else if packet.entity_type_id == chicken_entity_type
                && (packet.x - chicken_position.0).abs() < 0.01
                && (packet.y - chicken_position.1).abs() < 0.01
                && (packet.z - chicken_position.2).abs() < 0.01
            {
                assert!(summon_sent, "chicken must follow the summon command");
                assert_eq!(
                    chicken_id.replace(packet.entity_id),
                    None,
                    "duplicate chicken"
                );
            } else if packet.entity_type_id == item_entity_type {
                item_entity_ids.insert(packet.entity_id);
            } else if packet.entity_type_id == experience_orb_entity_type {
                saw_experience_orb |= packet.data == 1;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode exploded chicken drop metadata");
            if item_entity_ids.contains(&packet.entity_id) {
                saw_raw_chicken_drop |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == raw_chicken
                                && stack.count == 1
                    )
                });
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode TNT mob removal");
            saw_tnt_remove |= primed_id.is_some_and(|id| packet.entity_ids.contains(&id));
            saw_chicken_remove |= chicken_id.is_some_and(|id| packet.entity_ids.contains(&id));
        } else if frame.id == ClientboundExplode::ID {
            assert!(
                saw_tnt_remove,
                "TNT removal must precede its explosion packet"
            );
            let mut body = frame.body;
            let packet = ClientboundExplode::decode(&mut body).expect("decode mob explosion");
            assert!(body.is_empty());
            assert!((packet.center.x - explosion_center.0).abs() < 0.001);
            assert!((packet.center.y - explosion_center.1).abs() < 0.001);
            assert!((packet.center.z - explosion_center.2).abs() < 0.001);
            assert_eq!(packet.radius, 4.0);
            saw_explosion = true;
        } else if frame.id == ClientboundSetTime::ID && saw_explosion {
            let mut body = frame.body;
            let packet = ClientboundSetTime::decode(&mut body).expect("decode TNT lifecycle time");
            let baseline = *explosion_game_time.get_or_insert(packet.game_time);
            assert!(
                packet.game_time.saturating_sub(baseline) <= 60 || saw_chicken_remove,
                "exploded chicken remained visible for more than 60 simulation ticks"
            );
        }
    }

    assert!(primed_id.is_some(), "primed TNT must be observable");
    assert!(chicken_id.is_some(), "summoned chicken must be observable");
    assert!(
        saw_chicken_remove,
        "lethal explosion must remove the chicken"
    );
    assert!(
        saw_raw_chicken_drop,
        "dead chicken must spawn its item drop"
    );
    assert!(saw_experience_orb, "dead chicken must spawn one XP orb");
}
