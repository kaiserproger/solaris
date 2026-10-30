//! plan.rs unit tests: TickOutput -> planned packets per viewer.

use super::*;
use mc_core::boundary::{
    BlockChange, ChangeSet, ConnectionId, EntityAppearance, EntityEvent as BoundaryEvent, EntityId,
    EntityView, Hand, PlayerChange, TickOutput, WorldSnapshot,
};
use mc_protocol::packets::play::{
    ClientboundOpenScreen, ClientboundSystemChat, SetEntityMotion, pack_block_pos,
};
use mc_world::chunk::ChunkPos;
use std::sync::Arc;

#[path = "plan/chunk_delta_tests.rs"]
mod chunk_delta_tests;

#[path = "plan/section_blocks_tests.rs"]
mod section_blocks_tests;

#[path = "plan_index_tests.rs"]
mod index_tests;

#[path = "plan_tracking_tests.rs"]
mod tracking_tests;

#[path = "plan_native_density_tests.rs"]
mod native_density_tests;

#[path = "plan/entity_candidates_tests.rs"]
mod entity_candidates_tests;

#[path = "plan/shared_moves_tests.rs"]
mod shared_moves_tests;

#[path = "plan/entity_type_flags_tests.rs"]
mod entity_type_flags_tests;

#[path = "plan/entity_interest_tests.rs"]
mod entity_interest_tests;

#[path = "plan/books_frames_tests.rs"]
mod books_frames_tests;

fn with_cache<T>(f: impl FnOnce(&ChunkPacketCache) -> T) -> T {
    f(&ChunkPacketCache::new())
}

/// Decoded body of one planned (framed) packet.
fn body_of(packet: &PlannedPacket) -> bytes::Bytes {
    let mut framed = bytes::BytesMut::from(&packet.data[..]);
    mc_protocol::frame::try_decode_frame(&mut framed, Compression::Threshold(256))
        .expect("planned frame decodes")
        .expect("planned frame is complete")
        .body
}

#[test]
fn authenticated_roster_replaces_a_provisional_player_object_once() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    let entity = EntityId(2);
    viewer
        .tracked_entities
        .insert(entity, TrackedEntity::default());
    let roster = PlayerChange::Roster {
        players: vec![mc_core::dimensions::RosterPlayer {
            connection: ConnectionId(2),
            entity,
            uuid: uuid::Uuid::from_u128(2),
            name: "Bob".into(),
            game_mode: 0,
        }]
        .into(),
        removed: Arc::from([]),
    };
    let packets = plan_player_changes(std::slice::from_ref(&roster), &mut viewer);
    assert!(packets.iter().any(|packet| packet.id == RemoveEntities::ID));
    assert!(!viewer.tracked_entities.contains_key(&entity));
    viewer
        .tracked_entities
        .insert(entity, TrackedEntity::default());
    let packets = plan_player_changes(&[roster], &mut viewer);
    assert!(!packets.iter().any(|packet| packet.id == RemoveEntities::ID));
    assert!(viewer.tracked_entities.contains_key(&entity));
}

#[test]
fn player_air_supply_is_reliable_own_metadata_without_world_credit() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.entity_id = EntityId(3174);
    viewer.world_credit = 0;
    for air in [299, 0, -19, 300] {
        let packets = plan_player_changes(&[PlayerChange::AirSupply { air }], &mut viewer);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].connection, viewer.connection);
        assert!(packets[0].reliable);
        assert_eq!(packets[0].id, ClientboundSetEntityData::ID);
        let decoded = ClientboundSetEntityData::decode(&mut &body_of(&packets[0])[..]).unwrap();
        assert_eq!(decoded.entity_id, viewer.entity_id.0);
        assert_eq!(
            decoded.values,
            vec![WireEntityDataValue::Int {
                index: mc_protocol::packets::play::ENTITY_DATA_AIR_SUPPLY_INDEX,
                value: air,
            }]
        );
    }
    assert_eq!(viewer.world_credit, 0);
}

#[test]
fn one_shot_explosions_precede_resumable_world_work_with_bounded_credit() {
    with_cache(|cache| {
        for forget_all in [false, true] {
            for credit in [0, 96] {
                let (pos, chunk) = chunk_at(0, 0);
                let changes = ChangeSet {
                    explosions: (0..96)
                        .map(|n| mc_core::boundary::ExplosionEvent {
                            center: [8.0 + f64::from(n) / 100.0, 64.0, 8.0],
                            radius: 4.0,
                            blocks: 1,
                            knockback: vec![(EntityId(1), [0.1, 0.2, 0.3])],
                        })
                        .collect(),
                    ..Default::default()
                };
                let out = output(vec![(pos, chunk)], vec![], changes);
                let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
                viewer.world_credit = credit;
                viewer.forget_all = forget_all;
                viewer
                    .loaded_chunks
                    .insert((0, 0), out.snapshot.chunks[0].revision);
                for x in 0..16 {
                    for z in 0..16 {
                        viewer
                            .pending_blocks
                            .insert(mc_world::chunk::BlockPos { x, y: 64, z });
                    }
                }
                if forget_all {
                    viewer.chunk_repairs.insert((0, 0));
                }
                let packets = plan_viewer(&out, &mut viewer, 0, cache);
                let blasts: Vec<_> = packets
                    .iter()
                    .filter(|packet| packet.id == ClientboundExplode::ID)
                    .collect();
                assert_eq!(blasts.len(), credit);
                assert!(blasts.iter().all(|packet| !packet.reliable));
                assert_eq!(
                    packets.iter().filter(|packet| !packet.reliable).count(),
                    credit
                );
                for (n, packet) in blasts.iter().enumerate() {
                    let blast = ClientboundExplode::decode(&mut &body_of(packet)[..]).unwrap();
                    assert_eq!(blast.center.x, 8.0 + n as f64 / 100.0);
                }
                assert_eq!(viewer.forget_all, forget_all);
                assert_eq!(viewer.chunk_repairs.contains(&(0, 0)), forget_all);
                if !forget_all {
                    viewer.world_credit = 512;
                    assert_eq!(viewer.pending_blocks.len(), 256);
                    let quiet = TickOutput {
                        changes: Arc::new(ChangeSet::default()),
                        ..out
                    };
                    let resumed = plan_viewer(&quiet, &mut viewer, 0, cache);
                    assert_eq!(
                        resumed
                            .iter()
                            .filter(|packet| {
                                packet.id == mc_protocol::packets::play::SectionBlocksUpdate::ID
                            })
                            .count(),
                        1
                    );
                    assert!(viewer.pending_blocks.is_empty());
                    assert!(!viewer.chunk_repairs.contains(&(0, 0)));
                    assert!(
                        !resumed
                            .iter()
                            .any(|packet| packet.id == ClientboundExplode::ID)
                    );
                }
            }
        }
    });
}

#[test]
fn statistics_snapshot_matches_local_javap_and_viewer_ownership() {
    // Local server-26.1.2.jar javap: GameProtocols CB registration 3;
    // AwardStats -> ByteBufCodecs.map(Stat.STREAM_CODEC, VAR_INT).
    // Stat -> STAT_TYPE registry dispatch; StatType -> value registry id.
    let changes = [PlayerChange::Statistics {
        stats: vec![(8, 1, 300), (0, 128, 2)],
    }];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].id, 0x03);
    assert_eq!(packets[0].connection, ConnectionId(7));
    assert!(packets[0].reliable);
    assert_eq!(
        &body_of(&packets[0])[..],
        &[2, 8, 1, 0xac, 2, 0, 0x80, 1, 2]
    );
    let empty = plan_player_changes(
        &[PlayerChange::Statistics { stats: Vec::new() }],
        &mut viewer_at(7, [0.0; 3]),
    );
    assert_eq!(&body_of(&empty[0])[..], &[0]);
}

#[test]
fn merchant_packets_match_local_vanilla_javap_layout_and_viewer_ownership() {
    use mc_core::villager::VillagerTradeOffer;
    use mc_protocol::packets::play::ClientboundMerchantOffers;
    let offer = VillagerTradeOffer {
        cost_a: mc_data::ItemStack::new(1, 2),
        cost_b: Some(mc_data::ItemStack::new(2, 3)),
        result: mc_data::ItemStack::new(3, 4),
        uses: 1,
        max_uses: 12,
        xp: 5,
        reward_exp: true,
        special_price: -1,
        price_multiplier: 0.05,
        demand: 7,
    };
    let changes = vec![
        PlayerChange::MenuOpened {
            window: 4,
            menu_type: 19,
            title: r#"{"translate":"entity.minecraft.villager.fletcher"}"#.into(),
        },
        PlayerChange::MerchantOffers {
            window: 4,
            offers: vec![offer],
            level: 2,
            xp: 10,
            can_restock: true,
        },
    ];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 2);
    // 26.3 packets.json: open_screen = 60, merchant_offers = 53.
    assert_eq!(packets[0].id, 0x3c);
    assert_eq!(packets[1].id, 0x35);
    let open = ClientboundOpenScreen::decode(&mut body_of(&packets[0])).unwrap();
    assert_eq!(open.container_id, 4);
    assert_eq!(open.menu_type, 19);
    assert_eq!(
        mc_nbt::read_network(&mut &open.title_nbt[..]).unwrap(),
        mc_nbt::Tag::Compound(vec![(
            "translate".into(),
            mc_nbt::Tag::String("entity.minecraft.villager.fletcher".into()),
        )]),
    );
    let mut expected = vec![4, 1, 1, 2, 0, 4, 3, 0, 0, 1, 2, 3, 0, 0];
    // MerchantOffer.writeToStream: A cost, result, optional B, out of stock,
    // BE i32 uses/max/xp/special, BE float multiplier, BE i32 demand.
    for counter in [1_i32, 12, 5, -1] {
        expected.extend_from_slice(&counter.to_be_bytes());
    }
    expected.extend_from_slice(&0.05_f32.to_be_bytes());
    expected.extend_from_slice(&7_i32.to_be_bytes());
    expected.extend_from_slice(&[2, 10, 1, 1]);
    assert_eq!(body_of(&packets[1]).as_ref(), expected.as_slice());
    let decoded = ClientboundMerchantOffers::decode(&mut body_of(&packets[1])).unwrap();
    assert_eq!(decoded.offers[0].uses, 1);
    assert_eq!(decoded.offers[0].cost_b.as_ref().unwrap().count, 3);
}

#[test]
fn effects_and_absorption_are_reliable_and_use_the_viewers_entity_id() {
    use mc_core::effects::{ActiveEffect, EffectId};
    use mc_protocol::packets::play::{
        ClientboundRemoveEntityEffect, ClientboundUpdateEntityEffect,
    };
    let mut effect = ActiveEffect::new(EffectId::Darkness, -1, 255);
    effect.ambient = true;
    effect.show_particles = false;
    effect.show_icon = false;
    let changes = [
        PlayerChange::MobEffect {
            effect,
            blend: false,
        },
        PlayerChange::RemoveMobEffect {
            id: EffectId::Poison,
        },
        PlayerChange::Absorption { amount: 6.0 },
    ];
    let mut viewer = ViewerState::new(ConnectionId(7), EntityId(42), [0.0; 3]);
    let packets = plan_player_changes(&changes, &mut viewer);
    assert_eq!(packets.len(), 3);
    assert!(
        packets
            .iter()
            .all(|packet| packet.reliable && packet.connection == ConnectionId(7))
    );
    assert_eq!(packets[0].id, ClientboundUpdateEntityEffect::ID);
    let update = ClientboundUpdateEntityEffect::decode(&mut body_of(&packets[0])).unwrap();
    assert_eq!(update.entity_id, 42);
    assert_eq!(update.effect_id.raw(), EffectId::Darkness as u32);
    assert_eq!(update.amplifier, 255);
    assert_eq!(update.duration_ticks, -1);
    assert!(update.flags.ambient);
    assert!(!(update.flags.visible || update.flags.show_icon || update.flags.blend));
    assert_eq!(packets[1].id, ClientboundRemoveEntityEffect::ID);
    let remove = ClientboundRemoveEntityEffect::decode(&mut body_of(&packets[1])).unwrap();
    assert_eq!(remove.entity_id, 42);
    assert_eq!(remove.effect_id.raw(), EffectId::Poison as u32);
    let absorption = ClientboundSetEntityData::decode(&mut body_of(&packets[2])).unwrap();
    assert_eq!(absorption.entity_id, 42);
    assert_eq!(
        absorption.values,
        vec![WireEntityDataValue::Float {
            index: PLAYER_DATA_ABSORPTION_INDEX,
            value: 6.0,
        }]
    );
}

#[test]
fn effect_appearance_reaches_other_viewers_and_clears_particles_and_flags() {
    use mc_core::boundary::EffectParticle;
    use mc_protocol::packets::play::EffectParticle as WireParticle;
    with_cache(|cache| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut target = entity(42, [8.0, 64.0, 8.0]);
        target.type_id = player_entity_type_id();
        target.appearance = Arc::new(EntityAppearance {
            shared_flags: 0x60,
            effect_particles: vec![
                EffectParticle::EntityEffect { argb: 0x26123456 },
                EffectParticle::TrialOmen,
            ],
            effects_ambient: true,
            absorption: 4.0,
            ..Default::default()
        });
        let mut viewer = viewer_at(7, [8.0, 64.0, 8.0]);
        let first = output(
            vec![(pos, Arc::clone(&chunk))],
            vec![target.clone()],
            ChangeSet::default(),
        );
        let packets = plan_viewer(&first, &mut viewer, 0, cache);
        let appearance = packets
            .iter()
            .filter(|packet| packet.id == ClientboundSetEntityData::ID)
            .map(|packet| ClientboundSetEntityData::decode(&mut body_of(packet)).unwrap())
            .find(|packet| packet.entity_id == 42)
            .expect("other viewer receives appearance");
        assert!(appearance.values.contains(&WireEntityDataValue::Byte {
            index: 0,
            value: 0x60
        }));
        assert!(
            appearance
                .values
                .contains(&WireEntityDataValue::EffectParticles {
                    index: 10,
                    particles: vec![
                        WireParticle::EntityEffect { argb: 0x26123456 },
                        WireParticle::TrialOmen
                    ]
                })
        );
        assert!(appearance.values.contains(&WireEntityDataValue::Boolean {
            index: 11,
            value: true
        }));
        assert!(appearance.values.contains(&WireEntityDataValue::Float {
            index: 17,
            value: 4.0
        }));
        target.appearance = Arc::new(EntityAppearance::default());
        let second = output(vec![(pos, chunk)], vec![target], ChangeSet::default());
        let packets = plan_viewer(&second, &mut viewer, 0, cache);
        let cleared = packets
            .iter()
            .filter(|packet| packet.id == ClientboundSetEntityData::ID)
            .map(|packet| ClientboundSetEntityData::decode(&mut body_of(packet)).unwrap())
            .find(|packet| packet.entity_id == 42)
            .expect("other viewer receives cleared appearance");
        assert!(
            cleared
                .values
                .contains(&WireEntityDataValue::Byte { index: 0, value: 0 })
        );
        assert!(
            cleared
                .values
                .contains(&WireEntityDataValue::EffectParticles {
                    index: 10,
                    particles: Vec::new()
                })
        );
        assert!(cleared.values.contains(&WireEntityDataValue::Boolean {
            index: 11,
            value: false
        }));
        assert!(cleared.values.contains(&WireEntityDataValue::Float {
            index: 17,
            value: 0.0
        }));
    });
}

#[test]
fn scoreboard_objective_sidebar_is_reliable_and_viewer_scoped() {
    use mc_core::scoreboard::{Objective, ScoreboardEvent, component};
    use mc_protocol::packets::play::{ClientboundSetDisplayObjective, ClientboundSetObjective};
    let changes = [
        PlayerChange::Scoreboard {
            event: ScoreboardEvent::Objective {
                name: "health".into(),
                method: 0,
                objective: Some(Objective {
                    criteria: "health".into(),
                    display_name: "Health".into(),
                }),
            },
        },
        PlayerChange::Scoreboard {
            event: ScoreboardEvent::Display {
                slot: 1,
                objective: "health".into(),
            },
        },
    ];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 2);
    assert!(
        packets
            .iter()
            .all(|p| p.reliable && p.connection == ConnectionId(7))
    );
    assert_eq!(packets[0].id, ClientboundSetObjective::ID);
    let mut body = body_of(&packets[0]);
    assert_eq!(
        ClientboundSetObjective::decode(&mut body).unwrap(),
        ClientboundSetObjective {
            name: "health".into(),
            method: 0,
            display_name: Some(component("Health")),
            render_type: 1,
        }
    );
    assert!(body.is_empty());
    assert_eq!(packets[1].id, ClientboundSetDisplayObjective::ID);
    let mut body = body_of(&packets[1]);
    assert_eq!(
        ClientboundSetDisplayObjective::decode(&mut body).unwrap(),
        ClientboundSetDisplayObjective {
            slot: 1,
            objective: "health".into(),
        }
    );
    assert!(body.is_empty());
}

#[test]
fn styled_chat_preserves_component_and_is_viewer_scoped() {
    let content = mc_nbt::Tag::Compound(vec![
        ("text".into(), mc_nbt::Tag::String("Styled".into())),
        ("color".into(), mc_nbt::Tag::String("red".into())),
    ]);
    let changes = [PlayerChange::StyledChat {
        content: content.clone(),
    }];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    let mut body = body_of(&packets[0]);
    let packet = ClientboundSystemChat::decode(&mut body).unwrap();
    assert!(!packet.overlay);
    assert_eq!(
        mc_nbt::read_network(&mut &packet.content_nbt[..]).unwrap(),
        content
    );
    assert!(body.is_empty());
}

#[test]
fn wooden_sword_and_empty_hand_sync_own_attack_speed() {
    use mc_core::boundary::PlayerAttribute;
    use mc_data::item_components::ItemAttributeModifier;
    use mc_protocol::packets::play::ClientboundUpdateEntityAttributes;
    let mut viewer = viewer_at(7, [0.0; 3]);
    for (modifiers, expected) in [
        (
            vec![ItemAttributeModifier {
                kind: "minecraft:attack_speed".into(),
                id: "minecraft:base_attack_speed".into(),
                amount: -2.4,
                operation: "add_value".into(),
                slot: Some("mainhand".into()),
            }],
            1.6,
        ),
        (Vec::new(), 4.0),
    ] {
        let change = PlayerChange::Attributes {
            attributes: vec![PlayerAttribute {
                kind: "attack_speed",
                base: 4.0,
                modifiers,
            }],
        };
        let packets = plan_player_changes(&[change], &mut viewer);
        assert_eq!(packets.len(), 1);
        assert!(packets[0].reliable);
        let mut body = body_of(&packets[0]);
        let packet = ClientboundUpdateEntityAttributes::decode(&mut body).unwrap();
        assert_eq!(packet.entity_id, 7);
        let attribute = &packet.attributes[0];
        assert_eq!(attribute.attribute_id.raw(), 5);
        let value = attribute.base + attribute.modifiers.iter().map(|m| m.amount).sum::<f64>();
        assert!((value - expected).abs() < 1.0e-12);
        assert!(body.is_empty());
    }
}

#[test]
fn speed_effect_uses_existing_movement_attribute_packet_and_reset() {
    use mc_core::boundary::PlayerAttribute;
    use mc_data::item_components::ItemAttributeModifier;
    use mc_protocol::packets::play::{
        AttributeModifierOperation, ClientboundUpdateEntityAttributes,
    };
    let mut viewer = viewer_at(7, [0.0; 3]);
    for (modifiers, expected) in [
        (
            vec![ItemAttributeModifier {
                kind: "minecraft:movement_speed".into(),
                id: "minecraft:effect.speed".into(),
                amount: 0.4,
                operation: "add_multiplied_total".into(),
                slot: None,
            }],
            0.14,
        ),
        (Vec::new(), 0.1),
    ] {
        let change = PlayerChange::Attributes {
            attributes: vec![PlayerAttribute {
                kind: "movement_speed",
                base: 0.1,
                modifiers,
            }],
        };
        let packets = plan_player_changes(&[change], &mut viewer);
        assert_eq!(packets.len(), 1);
        assert!(packets[0].reliable);
        let mut body = body_of(&packets[0]);
        let packet = ClientboundUpdateEntityAttributes::decode(&mut body).unwrap();
        assert_eq!(packet.entity_id, 7);
        let attribute = &packet.attributes[0];
        assert_eq!(attribute.attribute_id.raw(), 26);
        let value = attribute
            .modifiers
            .iter()
            .fold(attribute.base, |base, modifier| {
                assert_eq!(
                    modifier.operation,
                    AttributeModifierOperation::AddMultipliedTotal
                );
                base * (1.0 + modifier.amount)
            });
        assert!((value - expected).abs() < 1e-12);
        assert!(body.is_empty());
    }
}

#[test]
fn armor_and_toughness_are_presented_to_player() {
    use mc_core::boundary::PlayerAttribute;
    use mc_data::item_components::ItemAttributeModifier;
    use mc_protocol::packets::play::ClientboundUpdateEntityAttributes;
    let mut viewer = viewer_at(7, [0.0; 3]);
    let attributes = [("armor", 3.0), ("armor_toughness", 2.0)]
        .into_iter()
        .map(|(kind, amount)| PlayerAttribute {
            kind,
            base: 0.0,
            modifiers: vec![ItemAttributeModifier {
                kind: format!("minecraft:{kind}"),
                id: "minecraft:armor.helmet".into(),
                amount,
                operation: "add_value".into(),
                slot: None,
            }],
        })
        .collect();
    let packets = plan_player_changes(&[PlayerChange::Attributes { attributes }], &mut viewer);
    let mut body = body_of(&packets[0]);
    let packet = ClientboundUpdateEntityAttributes::decode(&mut body).unwrap();
    assert_eq!(
        packet
            .attributes
            .iter()
            .map(|attribute| (attribute.attribute_id.raw(), attribute.modifiers[0].amount))
            .collect::<Vec<_>>(),
        vec![(1, 3.0), (2, 2.0)]
    );
}

fn chunk_at(x: i32, z: i32) -> (ChunkPos, Arc<mc_world::chunk::Chunk>) {
    let mut chunk = mc_world::chunk::Chunk::empty(
        mc_world::chunk::ChunkPos { x, z },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").expect("static id"),
    );
    chunk.section_lights.fill(mc_world::chunk::SectionLight {
        block: Some(mc_world::chunk::LightSection::uniform(0)),
        sky: Some(mc_world::chunk::LightSection::uniform(0xFF)),
    });
    (mc_world::chunk::ChunkPos { x, z }, Arc::new(chunk))
}

fn viewer_at(conn: u64, pos: [f64; 3]) -> ViewerState {
    ViewerState::new(ConnectionId(conn), EntityId(conn as i32), pos)
}

fn entity(id: i32, pos: [f64; 3]) -> EntityView {
    EntityView {
        id: EntityId(id),
        uuid: uuid::Uuid::nil(),
        type_id: 15, // block_display: interval 1 for generic delivery/baseline tests.
        position: pos,
        velocity: [0.0, 0.0, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: true,
        appearance: Arc::new(EntityAppearance::default()),
    }
}

fn output(
    chunks: Vec<(ChunkPos, Arc<mc_world::chunk::Chunk>)>,
    entities: Vec<EntityView>,
    changes: ChangeSet,
) -> TickOutput {
    // Every output stands for fresh chunk content: unique revisions.
    static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut chunks: Vec<mc_core::boundary::SnapshotChunk> = chunks
        .into_iter()
        .map(|(pos, chunk)| mc_core::boundary::SnapshotChunk {
            pos,
            revision: REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            chunk,
        })
        .collect();
    chunks.sort_by_key(|entry| (entry.pos.x, entry.pos.z));
    TickOutput {
        tick: 1,
        snapshot: Arc::new(WorldSnapshot {
            tick: 1,
            day_time: 1000,
            chunks: Arc::new(chunks),
            entities: entities.into(),
            day_time_rate: 1.0,
            ..Default::default()
        }),
        changes: Arc::new(changes),
        times: Default::default(),
    }
}

#[test]
fn villager_stair_seat_spawns_links_and_removes_in_order() {
    use mc_protocol::packets::play::ClientboundSetPassengers;
    with_cache(|cache| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut resident = entity(42, [8.5, 64.5, 8.5]);
        resident.type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:villager").unwrap().protocol_id;
        resident.appearance = Arc::new(EntityAppearance {
            villager_seat: Some(mc_core::boundary::BlockPos { x: 8, y: 64, z: 8 }),
            ..Default::default()
        });
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        let first = output(vec![(pos, Arc::clone(&chunk))], vec![resident.clone()], ChangeSet::default());
        let packets = plan_viewer(&first, &mut viewer, 1, cache);
        let spawn = packets.iter().position(|p| p.id == AddEntity::ID
            && AddEntity::decode(&mut body_of(p)).unwrap().entity_id < 0).expect("seat spawn");
        let seat = AddEntity::decode(&mut body_of(&packets[spawn])).unwrap().entity_id;
        let link = packets.iter().position(|p| p.id == ClientboundSetPassengers::ID
            && ClientboundSetPassengers::decode(&mut body_of(p)).unwrap().passenger_ids == [42]).expect("mount");
        assert!(spawn < link);
        let attached = ClientboundSetPassengers::decode(&mut body_of(&packets[link])).unwrap();
        assert_eq!(attached.vehicle_id, seat);
        resident.appearance = Arc::new(EntityAppearance::default());
        let second = output(vec![(pos, chunk)], vec![resident], ChangeSet::default());
        viewer.world_credit = 1;
        let deferred = plan_viewer(&second, &mut viewer, 2, cache);
        assert!(!deferred.iter().any(|p| p.id == RemoveEntities::ID
            && RemoveEntities::decode(&mut body_of(p)).unwrap().entity_ids.contains(&seat)));
        viewer.world_credit = usize::MAX;
        let packets = plan_viewer(&second, &mut viewer, 2, cache);
        let dismount = packets.iter().position(|p| p.id == ClientboundSetPassengers::ID
            && ClientboundSetPassengers::decode(&mut body_of(p)).unwrap().passenger_ids.is_empty()).expect("dismount");
        let removal = packets.iter().position(|p| p.id == RemoveEntities::ID
            && RemoveEntities::decode(&mut body_of(p)).unwrap().entity_ids.contains(&seat)).expect("seat removal");
        assert!(dismount < removal);
    });
}

#[test]
fn position_sync_preserves_core_ids_across_batches_and_viewer_recreation() {
    use mc_protocol::packets::play::SynchronizePlayerPosition;
    let correction = |teleport_id| PlayerChange::PositionCorrected {
        teleport_id,
        position: [3.7, 64.0, 6.0],
        yaw: 90.0,
        pitch: 0.0,
    };
    let mut viewer = viewer_at(1, [0.5, 64.0, 0.5]);
    for ids in [&[2, 3][..], &[4][..]] {
        let changes: Vec<_> = ids.iter().copied().map(correction).collect();
        let packets = plan_player_changes(&changes, &mut viewer);
        let actual: Vec<_> = packets
            .iter()
            .map(|packet| {
                assert_eq!(packet.id, SynchronizePlayerPosition::ID);
                SynchronizePlayerPosition::decode(&mut &*body_of(packet))
                    .unwrap()
                    .teleport_id
            })
            .collect();
        assert_eq!(actual, ids);
    }
    let mut replay_viewer = viewer_at(1, [0.5, 64.0, 0.5]);
    let packets = plan_player_changes(&[correction(5)], &mut replay_viewer);
    assert_eq!(
        SynchronizePlayerPosition::decode(&mut &*body_of(&packets[0]))
            .unwrap()
            .teleport_id,
        5
    );
}

#[test]
fn chunk_send_and_unload() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let out = output(vec![(pos, chunk)], vec![], ChangeSet::default());
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        let packets = plan_viewer(&out, &mut v, 0, c);
        assert!(
            packets.iter().any(
                |p| p.id == mc_protocol::packets::play::LevelChunkWithLight::ID && !p.reliable
            )
        );
        assert!(v.loaded_chunks.contains_key(&(0, 0)));
        // Move far away with an empty snapshot: unload fires.
        v.position = [800.0, 64.0, 800.0];
        let out2 = output(vec![], vec![], ChangeSet::default());
        let packets = plan_viewer(&out2, &mut v, 0, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::ForgetLevelChunk::ID)
        );
        assert!(!v.loaded_chunks.contains_key(&(0, 0)));
    });
}

/// A spawn or chunk planned without world credit is planned once credit
/// returns; once planned it is not repeated.
#[test]
fn spawn_and_chunk_wait_for_credit() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let out = output(
            vec![(pos, chunk)],
            vec![entity(10, [8.0, 64.0, 8.0])],
            ChangeSet::default(),
        );
        let ids = |packets: &[crate::plan::PlannedPacket]| {
            packets.iter().map(|p| p.id).collect::<Vec<_>>()
        };
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        v.world_credit = 0;
        let starved = plan_viewer(&out, &mut v, 0, c);
        assert!(
            starved.iter().all(|p| p.reliable),
            "no credit, no world packet"
        );
        v.world_credit = 512;
        let sent = plan_viewer(&out, &mut v, 0, c);
        assert!(ids(&sent).contains(&mc_protocol::packets::play::AddEntity::ID));
        assert!(ids(&sent).contains(&mc_protocol::packets::play::LevelChunkWithLight::ID));
        let settled = plan_viewer(&out, &mut v, 0, c);
        assert!(!ids(&settled).contains(&mc_protocol::packets::play::AddEntity::ID));
        assert!(!ids(&settled).contains(&mc_protocol::packets::play::LevelChunkWithLight::ID));
    });
}

/// A removal without credit leaves the entity spawned on the client: once
/// it is visible again its moves still reach the client (no frozen ghost).
#[test]
fn deferred_removal_keeps_moving_the_entity() {
    with_cache(|c| {
        let ids = |packets: &[crate::plan::PlannedPacket]| {
            packets.iter().map(|p| p.id).collect::<Vec<_>>()
        };
        let (pos, chunk) = chunk_at(0, 0);
        let out = output(
            vec![(pos, chunk)],
            vec![entity(10, [8.0, 64.0, 8.0])],
            ChangeSet::default(),
        );
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        let _ = plan_viewer(&out, &mut v, 1, c);
        let (pos, chunk) = chunk_at(0, 0);
        v.world_credit = 0;
        let gone = plan_viewer(
            &output(vec![(pos, chunk)], vec![], ChangeSet::default()),
            &mut v,
            1,
            c,
        );
        assert!(!ids(&gone).contains(&mc_protocol::packets::play::RemoveEntities::ID));
        v.world_credit = 512;
        let (pos, chunk) = chunk_at(0, 0);
        let back = plan_viewer(
            &output(
                vec![(pos, chunk)],
                vec![entity(10, [8.5, 64.0, 8.0])],
                ChangeSet::default(),
            ),
            &mut v,
            1,
            c,
        );
        assert!(!ids(&back).contains(&mc_protocol::packets::play::AddEntity::ID));
        assert!(ids(&back).contains(&mc_protocol::packets::play::MoveEntityPos::ID));
    });
}

/// With credit for one move per publish and every entity moving on every
/// tick, each entity still gets a move within as many publishes as there
/// are movers (no starvation of the later ones).
#[test]
fn limited_credit_rotates_entity_moves() {
    use mc_protocol::packets::play::MoveEntityPos;
    with_cache(|c| {
        // No resident chunks: only entity traffic competes for the credit.
        let start: Vec<EntityView> = (10..13)
            .map(|id| entity(id, [2.0 + f64::from(id), 64.0, 8.0]))
            .collect();
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        v.loaded_chunks.insert((0, 0), 0);
        plan_viewer(
            &output(vec![], start.clone(), ChangeSet::default()),
            &mut v,
            1,
            c,
        );
        let mut movers = HashSet::default();
        for round in 1..=3 {
            // Same appearance `Arc`s: only the positions changed.
            let moved: Vec<EntityView> = start
                .iter()
                .cloned()
                .map(|mut e| {
                    e.position[0] += 0.5 * f64::from(round);
                    e
                })
                .collect();
            v.world_credit = 1;
            let packets = plan_viewer(&output(vec![], moved, ChangeSet::default()), &mut v, 1, c);
            for packet in packets.iter().filter(|p| p.id == MoveEntityPos::ID) {
                let moved = MoveEntityPos::decode(&mut &body_of(packet)[..]).expect("decodes");
                movers.insert(moved.entity_id);
            }
        }
        assert_eq!(movers, [10, 11, 12].into_iter().collect::<HashSet<_>>());
    });
}

#[test]
fn village_motion_frames_reach_tracked_viewer_while_streaming_chunks() {
    use mc_protocol::packets::play::{LevelChunkWithLight, MoveEntityPos};
    let kind = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let mut villager = entity(10, [2.5, 64.0, 2.5]);
    villager.type_id = kind;
    villager.uuid = uuid::Uuid::from_u128(0x1c3fa733dbfd541eb5fb2e39b6f22f79);
    let mut viewer = viewer_at(3, [8.0, 64.0, 8.0]);
    let mut chunks = vec![chunk_at(0, 0)];
    let initial = output(chunks.clone(), vec![villager.clone()], ChangeSet::default());
    let packets = crate::fanout::publish(&initial, [&mut viewer].into_iter(), 4);
    assert!(viewer.tracked_entities.contains_key(&villager.id));
    assert!(
        packets
            .iter()
            .all(|packet| packet.connection == ConnectionId(3))
    );
    let mut received_position = villager.position;
    for (round, (x, z)) in spiral_square((0, 0), 4)
        .into_iter()
        .skip(1)
        .take(40)
        .enumerate()
    {
        chunks.push(chunk_at(x, z));
        villager.position[0] += 0.125;
        let mut next = output(chunks.clone(), vec![villager.clone()], ChangeSet::default());
        // Existing chunk content is unchanged; only the newly admitted chunk
        // and the villager's position differ from the preceding snapshot.
        let snapshot = Arc::get_mut(&mut next.snapshot).unwrap();
        snapshot.tick = round as u64 + 2;
        let tick = snapshot.tick;
        for chunk in Arc::make_mut(&mut snapshot.chunks) {
            chunk.revision = 1;
        }
        viewer.world_credit = 32;
        let packets = crate::fanout::publish(&next, [&mut viewer].into_iter(), 4);
        assert!(
            packets
                .iter()
                .filter(|packet| packet.priority == crate::fanout::Priority::BestEffort)
                .count()
                <= viewer.world_credit
        );
        let mut received_move = false;
        let mut received_chunk = false;
        for packet in packets {
            assert_eq!(packet.connection, ConnectionId(3));
            let mut framed = bytes::BytesMut::from(&packet.data[..]);
            let mut frame =
                mc_protocol::frame::try_decode_frame(&mut framed, Compression::Threshold(256))
                    .unwrap()
                    .unwrap();
            if frame.id == LevelChunkWithLight::ID {
                received_chunk = true;
            } else if frame.id == MoveEntityPos::ID {
                let movement = MoveEntityPos::decode(&mut frame.body).unwrap();
                assert_eq!(movement.entity_id, villager.id.0);
                received_position[0] += f64::from(movement.delta_x) / 4096.0;
                received_position[1] += f64::from(movement.delta_y) / 4096.0;
                received_position[2] += f64::from(movement.delta_z) / 4096.0;
                received_move = true;
            }
        }
        assert!(received_chunk, "viewer must still be streaming new chunks");
        let interval = u64::from(
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(kind)
                .unwrap()
                .update_interval,
        );
        assert_eq!(
            received_move,
            (tick + villager.id.0 as u64).is_multiple_of(interval),
            "tracked villager uses vanilla movement cadence while chunks stream"
        );
        if received_move {
            assert_eq!(received_position, villager.position);
        } else {
            assert!((villager.position[0] - received_position[0]).abs() <= 0.25);
        }
    }
}

/// A viewer that sees more entities than its world credit carries gets the
/// nearest ones first and the rest on later publishes, never all at once.
#[test]
fn entity_spawns_follow_credit_nearest_first() {
    with_cache(|c| {
        let entities: Vec<EntityView> = (0..600)
            .map(|i| entity(100 + i, [0.5 + f64::from(i) * 0.02, 64.0, 8.0]))
            .collect();
        let (pos, chunk) = chunk_at(0, 0);
        let out = output(vec![(pos, chunk)], entities, ChangeSet::default());
        let mut v = viewer_at(1, [0.5, 64.0, 8.0]);
        let spawned = |packets: &[crate::plan::PlannedPacket]| {
            packets
                .iter()
                .filter(|p| p.id == mc_protocol::packets::play::AddEntity::ID)
                .count()
        };
        v.world_credit = 200;
        let first = plan_viewer(&out, &mut v, 1, c);
        let n = spawned(&first);
        assert!(first.iter().filter(|p| !p.reliable).count() <= 200);
        assert!(n > 0 && n < 600);
        assert_eq!(v.tracked_entities.len(), n);
        assert!(
            (100..100 + n as i32).all(|id| v.tracked_entities.contains_key(&EntityId(id))),
            "nearest {n} spawned first"
        );
        for _ in 0..600 / n {
            v.world_credit = 200;
            plan_viewer(&out, &mut v, 1, c);
        }
        assert_eq!(v.tracked_entities.len(), 600);
    });
}

#[test]
fn spawn_move_remove_via_index() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let e = entity(10, [8.0, 64.0, 8.0]);
        let mut changes = ChangeSet::default();
        changes.entities_spawned.push(EntityId(10));
        let out = output(vec![(pos, chunk)], vec![e], changes);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        let packets = plan_viewer(&out, &mut v, 1, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::AddEntity::ID)
        );
        assert!(v.tracked_entities.contains_key(&EntityId(10)));
        // Small move -> relative packet.
        let (pos2, chunk2) = chunk_at(0, 0);
        let mut e2 = entity(10, [8.5, 64.0, 8.0]);
        e2.yaw = 0.0;
        let moved = ChangeSet::default();
        let out2 = output(vec![(pos2, chunk2)], vec![e2], moved);
        let packets = plan_viewer(&out2, &mut v, 1, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::MoveEntityPos::ID)
        );
        // Large move -> absolute sync.
        let (pos3, chunk3) = chunk_at(50, 50);
        let e3 = entity(10, [800.0, 64.0, 800.0]);
        let moved2 = ChangeSet::default();
        // Keep viewer near the entity so it stays visible.
        v.position = [800.0, 64.0, 800.0];
        v.center_sent = None;
        v.loaded_chunks.clear();
        let out3 = output(vec![(pos3, chunk3)], vec![e3], moved2);
        let packets = plan_viewer(&out3, &mut v, 60, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::EntityPositionSync::ID)
        );
        // Removal.
        let mut removed = ChangeSet::default();
        removed.entities_removed.push(EntityId(10));
        let (pos4, chunk4) = chunk_at(0, 0);
        let out4 = output(vec![(pos4, chunk4)], vec![], removed);
        let packets = plan_viewer(&out4, &mut v, 60, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::RemoveEntities::ID)
        );
        assert!(!v.tracked_entities.contains_key(&EntityId(10)));
    });
}

#[test]
fn appearance_change_emits_data() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut e = entity(11, [8.0, 64.0, 8.0]);
        e.appearance = Arc::new(EntityAppearance {
            health: 10.0,
            ..Default::default()
        });
        let mut changes = ChangeSet::default();
        changes.entities_spawned.push(EntityId(11));
        changes.entities_appearance_changed.push(EntityId(11));
        let out = output(vec![(pos, chunk)], vec![e], changes);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        let packets = plan_viewer(&out, &mut v, 1, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::ClientboundSetEntityData::ID)
        );
    });
}

#[test]
fn self_entity_data_sent_without_add_entity() {
    // Main's Java-client verdict: after the burst the edge goes silent and
    // vanilla times out. The excluded self id must still get its
    // SetEntityData (pose/flags/health) — no AddEntity, no moves, no
    // RemoveEntities for self — on every tick output until it lands.
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        // Viewer entity id 1 (core-allocated) present in the snapshot.
        let mut v = ViewerState::new(ConnectionId(2), EntityId(1), [8.0, 80.0, 8.0]);
        let e = entity(1, [8.0, 80.0, 8.0]);
        let out = output(
            vec![(pos, Arc::clone(&chunk))],
            vec![e],
            ChangeSet::default(),
        );
        let packets = plan_viewer(&out, &mut v, 0, c);
        // Self data present exactly once...
        let data: Vec<_> = packets
            .iter()
            .filter(|p| {
                p.id == mc_protocol::packets::play::ClientboundSetEntityData::ID
                    && p.connection == ConnectionId(2)
            })
            .collect();
        assert_eq!(data.len(), 1, "self SetEntityData exactly once");
        assert!(
            packets
                .iter()
                .all(|p| p.id != mc_protocol::packets::play::AddEntity::ID)
        );
        assert!(
            packets
                .iter()
                .all(|p| p.id != mc_protocol::packets::play::RemoveEntities::ID)
        );
        // Second tick, same appearance: no repeat (already sent)...
        let packets2 = plan_viewer(&out, &mut v, 0, c);
        assert!(
            packets2
                .iter()
                .all(|p| p.id != mc_protocol::packets::play::ClientboundSetEntityData::ID)
        );
        // ...until a respawn: the client makes a new local player.
        let _ = plan_player_changes(
            &[PlayerChange::Respawned {
                teleport_id: 2,
                dimension: Default::default(),
                position: [8.0, 80.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            }],
            &mut v,
        );
        let out3 = output(
            vec![(pos, Arc::clone(&chunk))],
            vec![entity(1, [8.0, 80.0, 8.0])],
            ChangeSet::default(),
        );
        let packets3 = plan_viewer(&out3, &mut v, 0, c);
        assert!(
            packets3
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::ClientboundSetEntityData::ID)
        );
    });
}

#[test]
fn item_appearance_omits_living_health() {
    // Vanilla hard-fails (`undefined/mismatched entity accessor
    // minecraft:item index 9`) on a Living health float for an item
    // entity: items carry DATA_ITEM (index 8) only.
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut e = entity(2, [8.0, 78.0, 8.0]);
        e.appearance = Arc::new(EntityAppearance {
            health: 1.0,
            item: Some(mc_data::item_stack::ItemStack::EMPTY),
            ..Default::default()
        });
        // Force the item type id (test entities default to type 1).
        let item_type = mc_data::entity_types::solaris_required_entity_types()
            .id_of(&mc_data::Identifier::parse("minecraft:item").expect("static id parses"))
            .expect("item type present");
        e.type_id = item_type;
        let mut changes = ChangeSet::default();
        changes.entities_spawned.push(EntityId(2));
        let out = output(vec![(pos, chunk)], vec![e], changes);
        let mut v = viewer_at(1, [8.0, 78.0, 8.0]);
        let packets = plan_viewer(&out, &mut v, 1, c);
        let data: Vec<_> = packets
            .iter()
            .filter(|p| p.id == mc_protocol::packets::play::ClientboundSetEntityData::ID)
            .collect();
        // Exactly two: shared-flags/pose (+name none/baby none) and the
        // DATA_ITEM stack — no Living health float (which hard-fails
        // vanilla; the count pins the omission).
        assert_eq!(data.len(), 2);
        for packet in data {
            let metadata = ClientboundSetEntityData::decode(&mut body_of(packet)).unwrap();
            assert!(
                metadata
                    .values
                    .iter()
                    .all(|value| !matches!(value.index(), 9 | 10 | 11 | 17))
            );
        }
    });
}

#[test]
fn experience_orb_spawn_uses_add_entity_and_value_metadata_without_living_accessors() {
    with_cache(|cache| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut orb = entity(2, [8.0, 78.0, 8.0]);
        orb.type_id = mc_data::entity_types::solaris_required_entity_types()
            .id_of(&mc_data::Identifier::parse("minecraft:experience_orb").unwrap())
            .unwrap();
        orb.appearance = Arc::new(EntityAppearance {
            experience_orb: Some((7, 3)),
            health: 5.0,
            ..Default::default()
        });
        let mut changes = ChangeSet::default();
        changes.entities_spawned.push(EntityId(2));
        let output = output(vec![(pos, chunk)], vec![orb], changes);
        let mut viewer = viewer_at(1, [8.0, 78.0, 8.0]);
        let packets = plan_viewer(&output, &mut viewer, 1, cache);
        assert!(packets.iter().any(|packet| packet.id == AddEntity::ID));
        let metadata = packets
            .iter()
            .filter(|packet| packet.id == ClientboundSetEntityData::ID)
            .map(|packet| ClientboundSetEntityData::decode(&mut body_of(packet)).unwrap())
            .flat_map(|packet| packet.values)
            .collect::<Vec<_>>();
        assert!(metadata.iter().any(|value| matches!(
            value,
            mc_protocol::packets::play::EntityDataValue::Int { index: 8, value: 7 }
        )));
        assert!(
            metadata
                .iter()
                .all(|value| !matches!(value.index(), 9 | 10 | 11 | 17))
        );
    });
}

#[test]
fn block_update_and_ack() {
    with_cache(|c| {
        // The client holds the chunk; then a change in it is a BlockUpdate.
        let (pos, chunk) = chunk_at(0, 0);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        plan_viewer(
            &output(vec![(pos, chunk.clone())], vec![], ChangeSet::default()),
            &mut v,
            1,
            c,
        );
        let mut changes = ChangeSet::default();
        changes.blocks.push(BlockChange {
            position: mc_world::chunk::BlockPos { x: 8, y: 64, z: 8 },
            state: 5,
        });
        let out = output(vec![(pos, chunk)], vec![], changes);
        let packets = plan_viewer(&out, &mut v, 1, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::BlockUpdate::ID && !p.reliable)
        );
        let acks = plan_player_changes(&[PlayerChange::BlockActionsAcked { sequence: 42 }], &mut v);
        let ack = acks
            .iter()
            .find(|p| p.id == mc_protocol::packets::play::BlockChangedAck::ID)
            .expect("ack planned");
        assert!(ack.reliable);
    });
}

#[test]
fn refused_build_repairs_are_reliable_viewer_scoped_and_precede_ack_without_world_credit() {
    use mc_protocol::packets::play::{BlockChangedAck, BlockUpdate, pack_block_pos};
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.world_credit = 0;
    let position = mc_core::boundary::BlockPos { x: 9, y: 64, z: 8 };
    let changes = [
        PlayerChange::BlockResynced { position, block: 5 },
        PlayerChange::BlockActionsAcked { sequence: 42 },
    ];
    let packets = plan_player_changes(&changes, &mut viewer);
    assert_eq!(
        packets.iter().map(|packet| packet.id).collect::<Vec<_>>(),
        [BlockUpdate::ID, BlockChangedAck::ID]
    );
    assert!(packets.iter().all(|packet| packet.reliable));
    let update = BlockUpdate::decode(&mut &body_of(&packets[0])[..]).unwrap();
    assert_eq!(update.position, pack_block_pos(9, 64, 8));
    assert_eq!(update.state_id, 5);
    let ack = BlockChangedAck::decode(&mut &body_of(&packets[1])[..]).unwrap();
    assert_eq!(ack.sequence, 42);
    assert_eq!(viewer.world_credit, 0);
}

#[test]
fn menus_health_chat_gamemode_cursor() {
    let changes = vec![
        PlayerChange::MenuOpened {
            window: 1,
            menu_type: 2,
            title: "Chest".to_owned(),
        },
        PlayerChange::MenuContents {
            window: 1,
            state_id: 3,
            slots: vec![None, None],
            cursor: None,
        },
        PlayerChange::MenuSlot {
            window: 1,
            state_id: 3,
            slot: 0,
            item: None,
        },
        PlayerChange::Cursor { item: None },
        PlayerChange::MenuProperty {
            window: 1,
            property: 0,
            value: 5,
        },
        PlayerChange::Health {
            health: 20.0,
            food: 20,
            saturation: 5.0,
        },
        PlayerChange::Experience {
            progress: 0.5,
            level: 3,
            total: 40,
        },
        PlayerChange::Chat {
            message: "hi".to_owned(),
        },
        PlayerChange::GameModeChanged { game_mode: 1 },
        PlayerChange::PositionCorrected {
            teleport_id: 2,
            position: [1.0, 2.0, 3.0],
            yaw: 0.0,
            pitch: 0.0,
        },
    ];
    let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
    let packets = plan_player_changes(&changes, &mut v);
    for id in [
        mc_protocol::packets::play::ClientboundOpenScreen::ID,
        mc_protocol::packets::play::ClientboundContainerSetContent::ID,
        mc_protocol::packets::play::ClientboundContainerSetSlot::ID,
        mc_protocol::packets::play::ClientboundSetCursorItem::ID,
        mc_protocol::packets::play::ClientboundContainerSetData::ID,
        mc_protocol::packets::play::ClientboundSetHealth::ID,
        mc_protocol::packets::play::ClientboundSetExperience::ID,
        mc_protocol::packets::play::ClientboundSystemChat::ID,
        mc_protocol::packets::play::GameEvent::ID,
        mc_protocol::packets::play::ClientboundPlayerAbilities::ID,
        mc_protocol::packets::play::SynchronizePlayerPosition::ID,
    ] {
        assert!(
            packets.iter().any(|p| p.id == id && p.reliable),
            "missing reliable {id:#04x}"
        );
    }
}

#[test]
fn nonlethal_health_post_image_is_reliable_and_self_directed() {
    use mc_protocol::packets::play::ClientboundSetHealth;

    let change = PlayerChange::Health {
        health: 17.975,
        food: 18,
        saturation: 3.25,
    };
    let mut victim = viewer_at(1, [8.0, 64.0, 8.0]);
    let packets = plan_player_changes(std::slice::from_ref(&change), &mut victim);
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    assert_eq!(packets[0].id, ClientboundSetHealth::ID);
    let mut body = body_of(&packets[0]);
    let health = ClientboundSetHealth::decode(&mut body).unwrap();
    assert_eq!(health.health, 17.975);
    assert_eq!(health.food, 18);
    assert_eq!(health.saturation, 3.25);
    assert!(body.is_empty());
}

#[test]
fn furnace_progress_and_charcoal_result_decode_for_only_the_viewer() {
    use mc_protocol::packets::play::{ClientboundContainerSetContent, ClientboundContainerSetData};
    let charcoal = mc_data::ItemStack::new(123, 1);
    let mut changes: Vec<_> = [101, 300, 0, 200]
        .into_iter()
        .enumerate()
        .map(|(property, value)| PlayerChange::MenuProperty {
            window: 4,
            property: property as i16,
            value,
        })
        .collect();
    let mut slots = vec![None; 39];
    slots[2] = Some(charcoal.clone());
    changes.push(PlayerChange::MenuContents {
        window: 4,
        state_id: 9,
        slots,
        cursor: None,
    });
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    let packets = plan_player_changes(&changes, &mut viewer);
    assert_eq!(packets.len(), 5);
    for (property, value) in [101, 300, 0, 200].into_iter().enumerate() {
        let packet = &packets[property];
        assert!(packet.reliable);
        assert_eq!(packet.id, ClientboundContainerSetData::ID);
        let data = ClientboundContainerSetData::decode(&mut &body_of(packet)[..]).unwrap();
        assert_eq!(
            (data.container_id, data.id, data.value),
            (4, property as i16, value)
        );
    }
    let packet = &packets[4];
    assert!(packet.reliable);
    assert_eq!(packet.id, ClientboundContainerSetContent::ID);
    let content = ClientboundContainerSetContent::decode(&mut &body_of(packet)[..]).unwrap();
    assert_eq!(
        (content.container_id, content.state_id, content.items.len()),
        (4, 9, 39)
    );
    assert_eq!(content.items[2], charcoal);
    assert!(content.carried_item.is_empty());
}

#[test]
fn entity_events_map() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        // The client has entities 5 and 6 (visuals are only for those).
        plan_viewer(
            &output(
                vec![(pos, chunk.clone())],
                vec![entity(5, [8.0, 64.0, 8.0]), entity(6, [9.0, 64.0, 8.0])],
                ChangeSet::default(),
            ),
            &mut v,
            1,
            c,
        );
        let mut changes = ChangeSet::default();
        changes.entity_events.push(BoundaryEvent::Swing {
            entity: EntityId(5),
            hand: Hand::Main,
        });
        changes.entity_events.push(BoundaryEvent::Hurt {
            entity: EntityId(5),
        });
        changes.entity_events.push(BoundaryEvent::Death {
            entity: EntityId(5),
        });
        changes.entity_events.push(BoundaryEvent::Pickup {
            item: EntityId(6),
            collector: EntityId(7),
            count: 3,
        });
        let out = output(
            vec![(pos, chunk)],
            vec![entity(5, [8.0, 64.0, 8.0]), entity(6, [9.0, 64.0, 8.0])],
            changes,
        );
        let packets = plan_viewer(&out, &mut v, 1, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::ClientboundSwingAnimation::ID)
        );
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::EntityEvent::ID)
        );
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::ClientboundTakeItemEntity::ID)
        );
    });
}

#[test]
fn causal_damage_maps_self_and_visible_entities_without_unknown_victims() {
    use mc_protocol::packets::play::ClientboundDamageEvent;
    with_cache(|cache| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        plan_viewer(
            &output(
                vec![(pos, chunk.clone())],
                vec![entity(5, [8.0, 64.0, 8.0])],
                ChangeSet::default(),
            ),
            &mut viewer,
            0,
            cache,
        );
        let mut changes = ChangeSet::default();
        for victim in [1, 5, 99] {
            changes.entity_events.push(BoundaryEvent::Damage {
                entity: EntityId(victim),
                source_type_id: 7,
                cause: (victim != 1).then_some(EntityId(6)),
                direct: (victim != 1).then_some(EntityId(8)),
                source_position: None,
            });
        }
        let packets = plan_viewer(
            &output(
                vec![(pos, chunk)],
                vec![entity(5, [8.0, 64.0, 8.0])],
                changes,
            ),
            &mut viewer,
            0,
            cache,
        );
        let damage: Vec<_> = packets
            .iter()
            .filter(|packet| packet.id == ClientboundDamageEvent::ID)
            .collect();
        assert_eq!(damage.len(), 2);
        assert_eq!(damage[0].id, 0x19, "local 26.1.2 GameProtocols");
        assert_eq!(body_of(damage[0]).as_ref(), &[1, 7, 0, 0, 0]);
        assert_eq!(body_of(damage[1]).as_ref(), &[5, 7, 7, 9, 0]);
        let decoded = ClientboundDamageEvent::decode(&mut &body_of(damage[1])[..]).unwrap();
        assert_eq!(decoded.source_cause_id, 6);
        assert_eq!(decoded.source_direct_id, 8);
    });
}

/// A full pickup removes the item in the same tick: the client still gets
/// `TakeItemEntity` (its collect animation and the scenario's pickup proof),
/// and before the removal, as vanilla orders them.
#[test]
fn full_pickup_sends_take_before_removal() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        plan_viewer(
            &output(
                vec![(pos, chunk.clone())],
                vec![entity(6, [9.0, 64.0, 8.0])],
                ChangeSet::default(),
            ),
            &mut v,
            1,
            c,
        );
        let mut changes = ChangeSet::default();
        changes.entities_removed.push(EntityId(6));
        changes.entity_events.push(BoundaryEvent::Pickup {
            item: EntityId(6),
            collector: EntityId(1),
            count: 1,
        });
        let packets = plan_viewer(&output(vec![(pos, chunk)], vec![], changes), &mut v, 1, c);
        let position = |id: i32| packets.iter().position(|p| p.id == id);
        let take =
            position(mc_protocol::packets::play::ClientboundTakeItemEntity::ID).expect("take sent");
        let removal = position(mc_protocol::packets::play::RemoveEntities::ID).expect("removed");
        assert!(take < removal, "take precedes removal");
    });
}

#[test]
fn disconnect_plans_one_reliable_kick() {
    with_cache(|_| {
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        // Own kick: exactly one reliable PlayDisconnect packet.
        let own = plan_player_changes(
            &[PlayerChange::Disconnect {
                reason: "queue overflow".to_owned(),
            }],
            &mut v,
        );
        assert_eq!(own.len(), 1);
        assert_eq!(own[0].id, mc_protocol::packets::play::PlayDisconnect::ID);
        assert!(own[0].reliable);
        assert_eq!(own[0].connection, ConnectionId(1));
    });
}

#[test]
fn joined_adopts_core_entity_id() {
    with_cache(|_| {
        // The pump invents entity ids from ConnectionId; the core's Joined
        // entity id (diverged here) must win so the plan anchors correctly.
        let mut v = ViewerState::new(ConnectionId(3), EntityId(3), [0.5, 64.0, 0.5]);
        let own = plan_player_changes(
            &[PlayerChange::Joined {
                entity: EntityId(41),
                dimension: Default::default(),
                position: [8.0, 64.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            }],
            &mut v,
        );
        assert_eq!(v.entity_id, EntityId(41));
        assert!(
            own.iter()
                .any(|p| p.id == mc_protocol::packets::play::SynchronizePlayerPosition::ID)
        );
    });
}

#[test]
fn pre_join_burst_tail_maps_to_reliable() {
    // Guards the P1 join race: when the whole ordered burst is stashed
    // pre-registration, replaying it through `plan_player_changes` must
    // yield the authoritative tail (not just the Joined sync packet).
    with_cache(|_| {
        let id = ConnectionId(3);
        let changes = vec![
            PlayerChange::Joined {
                entity: EntityId(41),
                dimension: Default::default(),
                position: [8.0, 64.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            },
            PlayerChange::MenuContents {
                window: 0,
                state_id: 1,
                slots: Vec::new(),
                cursor: None,
            },
            PlayerChange::Health {
                health: 20.0,
                food: 20,
                saturation: 5.0,
            },
            PlayerChange::Experience {
                progress: 0.0,
                level: 0,
                total: 0,
            },
        ];
        let mut v = ViewerState::new(id, EntityId(3), [0.5, 64.0, 0.5]);
        let planned = plan_player_changes(&changes, &mut v);
        assert_eq!(v.entity_id, EntityId(41));
        for want in [
            mc_protocol::packets::play::SynchronizePlayerPosition::ID,
            mc_protocol::packets::play::ClientboundContainerSetContent::ID,
            mc_protocol::packets::play::ClientboundSetHealth::ID,
            mc_protocol::packets::play::ClientboundSetExperience::ID,
        ] {
            assert!(
                planned.iter().any(|p| p.id == want && p.reliable),
                "missing reliable burst packet {want:#04x}"
            );
        }
    });
}

#[test]
fn respawned_plans_respawn_then_position_sync() {
    // Contract: core emits Respawned BEFORE Health/Experience on an
    // accepted respawn; the edge must send vanilla Respawn (same
    // dimension, death-respawn flags) followed by the position sync,
    // all reliable and self-directed.
    with_cache(|_| {
        let id = ConnectionId(3);
        let mut v = ViewerState::new(id, EntityId(41), [8.0, 64.0, 8.0]);
        v.loaded_chunks.insert((0, 5), 1);
        v.tracked_entities
            .insert(EntityId(7), TrackedEntity::default());
        let changes = vec![
            PlayerChange::Respawned {
                teleport_id: 2,
                dimension: Default::default(),
                position: [8.0, 80.0, 8.0],
                yaw: 1.5,
                pitch: -0.25,
                game_mode: 0,
            },
            PlayerChange::Health {
                health: 20.0,
                food: 20,
                saturation: 5.0,
            },
            PlayerChange::Experience {
                progress: 0.0,
                level: 0,
                total: 0,
            },
        ];
        let planned = plan_player_changes(&changes, &mut v);
        let ids: Vec<i32> = planned.iter().map(|p| p.id).collect();
        let respawn_at = ids
            .iter()
            .position(|p| *p == mc_protocol::packets::play::ClientboundRespawn::ID)
            .expect("Respawn packet planned");
        let sync_at = ids
            .iter()
            .position(|p| *p == mc_protocol::packets::play::SynchronizePlayerPosition::ID)
            .expect("position sync planned");
        assert!(
            respawn_at < sync_at,
            "Respawn must precede the position sync: {ids:02x?}"
        );
        assert!(
            planned.iter().all(|p| p.reliable),
            "respawn burst must be fully reliable"
        );
        // Respawn body carries the death-respawn flags (data_to_keep = 0).
        let body = body_of(&planned[respawn_at]);
        let mut cursor: &[u8] = &body;
        let respawn = mc_protocol::packets::play::ClientboundRespawn::decode(&mut cursor)
            .expect("Respawn body decodes");
        assert_eq!(respawn.data_to_keep, 0);
        assert_eq!(respawn.game_mode, 0);
        assert_eq!(respawn.dimension_name.as_str(), "minecraft:overworld");
        // Position sync lands on the respawn position with the new yaw/pitch.
        let sync_body = body_of(&planned[sync_at]);
        let mut sync_cursor: &[u8] = &sync_body;
        let sync = mc_protocol::packets::play::SynchronizePlayerPosition::decode(&mut sync_cursor)
            .expect("sync body decodes");
        assert_eq!((sync.x, sync.y, sync.z), (8.0, 80.0, 8.0));
        // The client leaves its loading screen only on the chunk-wait event.
        let wait_at = planned
            .iter()
            .position(|p| {
                p.id == mc_protocol::packets::play::GameEvent::ID
                    && body_of(p).first()
                        == Some(
                            &mc_protocol::packets::play::GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
                        )
            })
            .expect("chunk-wait game event planned");
        assert!(respawn_at < wait_at);
        // The world view is forgotten by the next world plan, anchored on
        // the respawn position.
        assert!(v.forget_all);
        assert_eq!(v.position, [8.0, 80.0, 8.0]);
    });
}

#[test]
fn respawn_forgets_everything_held_before_resending() {
    // The vanilla client keeps its level across a same-dimension respawn,
    // so every entity and chunk it holds is removed/forgotten on the world
    // queue (after frames already queued there), then the view around the
    // respawn position is sent again: no ghost from the death site.
    with_cache(|c| {
        // Respawn position [8,80,8] sits in chunk (0,0); the viewer dies
        // far away at (1008,1008) holding chunk (63,63), plus chunk (0,0)
        // and entity 5 from earlier.
        let (pos, chunk) = chunk_at(0, 0);
        let out = output(vec![(pos, chunk)], vec![], ChangeSet::default());
        let mut v = viewer_at(1, [1008.0, 80.0, 1008.0]);
        v.loaded_chunks.insert((63, 63), 1);
        v.loaded_chunks.insert((0, 0), 1);
        v.tracked_entities
            .insert(EntityId(5), TrackedEntity::default());
        v.center_sent = Some((63, 63));
        let reliable = plan_player_changes(
            &[PlayerChange::Respawned {
                teleport_id: 2,
                dimension: Default::default(),
                position: [8.0, 80.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            }],
            &mut v,
        );
        assert!(
            reliable
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::ClientboundRespawn::ID && p.reliable),
            "outbox plan must carry the Respawn packet"
        );
        let packets = plan_viewer(&out, &mut v, 0, c);
        let at = |id: i32| packets.iter().position(|p| p.id == id && !p.reliable);
        let removed =
            at(mc_protocol::packets::play::RemoveEntities::ID).expect("held entity removed");
        let forgets = packets
            .iter()
            .filter(|p| p.id == mc_protocol::packets::play::ForgetLevelChunk::ID)
            .count();
        assert_eq!(forgets, 2, "both held chunks forgotten, the kept one too");
        let resent = at(mc_protocol::packets::play::LevelChunkWithLight::ID).expect("view resent");
        assert!(removed < resent);
        assert!(v.loaded_chunks.contains_key(&(0, 0)));
        assert!(!v.loaded_chunks.contains_key(&(63, 63)));
        assert!(!v.tracked_entities.contains_key(&EntityId(5)));
    });
}

/// World-queue forgets wait for credit, but an authoritative clock sync does
/// not. The forget pass resumes when credit returns.
#[test]
fn respawn_forget_pass_waits_for_credit() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let out = output(vec![(pos, chunk)], vec![], ChangeSet::default());
        let mut v = viewer_at(1, [1008.0, 80.0, 1008.0]);
        v.loaded_chunks.insert((63, 63), 1);
        v.loaded_chunks.insert((62, 63), 1);
        let _ = plan_player_changes(
            &[PlayerChange::Respawned {
                teleport_id: 2,
                dimension: Default::default(),
                position: [8.0, 80.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            }],
            &mut v,
        );
        v.world_credit = 1;
        let packets = plan_viewer(&out, &mut v, 0, c);
        assert!(
            packets
                .iter()
                .any(|p| p.id == ClientboundSetTime::ID && p.reliable)
        );
        assert!(
            packets
                .iter()
                .filter(|p| !p.reliable)
                .all(|p| p.id == mc_protocol::packets::play::ForgetLevelChunk::ID)
        );
        assert_eq!(packets.iter().filter(|p| !p.reliable).count(), 1);
        assert!(v.forget_all);
        v.world_credit = usize::MAX;
        let packets = plan_viewer(&out, &mut v, 0, c);
        assert!(!v.forget_all);
        assert!(
            packets
                .iter()
                .any(|p| p.id == mc_protocol::packets::play::LevelChunkWithLight::ID)
        );
        assert_eq!(
            v.loaded_chunks.keys().copied().collect::<Vec<_>>(),
            vec![(0, 0)]
        );
    });
}

fn clock_output(tick: u64, day_time: i64, day_time_rate: f32) -> TickOutput {
    TickOutput {
        tick,
        snapshot: Arc::new(WorldSnapshot {
            tick,
            day_time,
            day_time_rate,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}

fn assert_reliable_clock(packets: &[PlannedPacket], expected: ClientboundSetTime) {
    let mut clocks = packets.iter().filter(|p| p.id == ClientboundSetTime::ID);
    let clock = clocks.next().expect("authoritative clock update");
    assert!(clock.reliable);
    assert_eq!(
        ClientboundSetTime::decode(&mut &body_of(clock)[..]).unwrap(),
        expected
    );
    assert!(clocks.next().is_none(), "one clock update per projection");
}

#[test]
fn idle_viewer_gets_world_clock_every_20_ticks() {
    // Vanilla resyncs the clock every 20 ticks; it is the only steady
    // traffic an idle player receives. The pump may coalesce outputs, so a
    // skipped multiple-of-20 tick must not suppress the next resync.
    with_cache(|c| {
        let at = |tick: u64| clock_output(tick, 6000 + tick as i64, 1.0);
        let clocks = |packets: Vec<PlannedPacket>| -> Vec<ClientboundSetTime> {
            packets
                .iter()
                .filter(|p| p.id == ClientboundSetTime::ID)
                .map(|p| ClientboundSetTime::decode(&mut &body_of(p)[..]).expect("SetTime decodes"))
                .collect()
        };
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        assert_eq!(
            clocks(plan_viewer(&at(5), &mut v, 0, c)),
            vec![ClientboundSetTime::overworld(5, 6005, 1.0)]
        );
        for tick in 6..25 {
            assert!(
                clocks(plan_viewer(&at(tick), &mut v, 0, c)).is_empty(),
                "no resync at tick {tick}"
            );
        }
        // Tick 25 never planned (coalesced); tick 27 is past the interval.
        assert_eq!(
            clocks(plan_viewer(&at(27), &mut v, 0, c)),
            vec![ClientboundSetTime::overworld(27, 6027, 1.0)]
        );
    });
}

#[test]
fn frozen_clock_is_projected_with_zero_client_rate() {
    with_cache(|cache| {
        let output = clock_output(30, 6000, 0.0);
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        let packets = plan_viewer(&output, &mut viewer, 0, cache);
        let time = packets
            .iter()
            .find(|packet| packet.id == ClientboundSetTime::ID)
            .unwrap();
        assert!(time.reliable);
        assert_eq!(
            ClientboundSetTime::decode(&mut &body_of(time)[..]).unwrap(),
            ClientboundSetTime::overworld(30, 6000, 0.0)
        );
    });
}
#[test]
fn time_set_jump_bypasses_interval() {
    with_cache(|cache| {
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        let _ = plan_viewer(&clock_output(10, 10, 1.0), &mut viewer, 0, cache);
        // Forward/backward commands and a sleep skip all beat the heartbeat.
        for (tick, day_time) in [(11, 6000), (12, 1000), (13, 24_000)] {
            assert_reliable_clock(
                &plan_viewer(&clock_output(tick, day_time, 1.0), &mut viewer, 0, cache),
                ClientboundSetTime::overworld(tick as i64, day_time, 1.0),
            );
        }
    });
}

#[test]
fn unchanged_clock_stays_quiet() {
    with_cache(|cache| {
        for rate in [0.0, 1.0] {
            let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
            let _ = plan_viewer(&clock_output(10, 6000, rate), &mut viewer, 0, cache);
            for tick in 10..30 {
                let day_time = 6000 + ((tick - 10) as f32 * rate) as i64;
                let output = clock_output(tick, day_time, rate);
                for _ in 0..2 {
                    assert!(
                        plan_viewer(&output, &mut viewer, 0, cache)
                            .iter()
                            .all(|p| p.id != ClientboundSetTime::ID),
                        "no clock update at tick {tick}, rate {rate}"
                    );
                }
            }
        }
    });
}

#[test]
fn dirty_clock_survives_zero_chunk_credit() {
    with_cache(|cache| {
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        let _ = plan_viewer(&clock_output(10, 10, 1.0), &mut viewer, 0, cache);
        viewer.loaded_chunks.insert((63, 63), 1);
        viewer.world_credit = 0;
        let output = clock_output(11, 6000, 1.0);
        let packets = plan_viewer(&output, &mut viewer, 0, cache);
        assert_reliable_clock(&packets, ClientboundSetTime::overworld(11, 6000, 1.0));
        assert!(packets.iter().all(|p| p.reliable));
        assert!(
            viewer.loaded_chunks.contains_key(&(63, 63)),
            "chunk debt remains"
        );
        assert!(
            plan_viewer(&output, &mut viewer, 0, cache)
                .iter()
                .all(|p| p.id != ClientboundSetTime::ID)
        );
    });
}

#[test]
fn gamerule_flip_sends_rate() {
    with_cache(|cache| {
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        let _ = plan_viewer(&clock_output(10, 6000, 1.0), &mut viewer, 0, cache);
        viewer.world_credit = 0;
        // Day time agrees with the old interpolation: rate alone is dirty.
        for (tick, rate) in [(11, 0.0), (12, 1.0)] {
            assert_reliable_clock(
                &plan_viewer(&clock_output(tick, 6001, rate), &mut viewer, 0, cache),
                ClientboundSetTime::overworld(tick as i64, 6001, rate),
            );
        }
        assert!(
            plan_viewer(&clock_output(13, 6002, 1.0), &mut viewer, 0, cache)
                .iter()
                .all(|p| p.id != ClientboundSetTime::ID)
        );
    });
}

#[test]
fn same_dimension_respawn_resends_clock_without_chunk_credit() {
    with_cache(|cache| {
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        let output = clock_output(10, 6000, 1.0);
        let _ = plan_viewer(&output, &mut viewer, 0, cache);
        viewer.loaded_chunks.insert((63, 63), 1);
        viewer.world_credit = 0;
        let _ = plan_player_changes(
            &[PlayerChange::Respawned {
                teleport_id: 2,
                dimension: Default::default(),
                position: [8.0, 80.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            }],
            &mut viewer,
        );
        let packets = plan_viewer(&output, &mut viewer, 0, cache);
        assert_reliable_clock(&packets, ClientboundSetTime::overworld(10, 6000, 1.0));
        assert!(viewer.forget_all, "forget pass still awaits world credit");
    });
}

#[test]
fn weather_changes_project_rain_and_thunder() {
    use mc_protocol::packets::play::GameEvent;
    for raining in [false, true] {
        let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
        let change = PlayerChange::WeatherChanged {
            raining,
            rain_level: if raining { 1.0 } else { 0.0 },
            thunder_level: 0.5,
        };
        let packets = plan_player_changes(&[change], &mut viewer);
        assert_eq!(packets.len(), 3);
        let events: Vec<_> = packets
            .iter()
            .map(|packet| {
                assert!(packet.reliable);
                GameEvent::decode(&mut &body_of(packet)[..]).unwrap()
            })
            .collect();
        assert_eq!(
            events[0].event,
            if raining {
                GameEvent::EVENT_START_RAINING
            } else {
                GameEvent::EVENT_STOP_RAINING
            }
        );
        assert_eq!(events[1].event, GameEvent::EVENT_RAIN_LEVEL_CHANGE);
        assert_eq!(events[1].value, if raining { 1.0 } else { 0.0 });
        assert_eq!(events[2].event, GameEvent::EVENT_THUNDER_LEVEL_CHANGE);
        assert_eq!(events[2].value, 0.5);
    }
}

/// A move and head turn deferred for lack of credit are sent on a later
/// publish although the entity has stopped (same snapshot, empty change
/// set); once sent they are not repeated.
#[test]
fn deferred_entity_move_is_sent_after_the_entity_stops() {
    use mc_protocol::packets::play::{MoveEntityPos, RotateHead};
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        let start = output(
            vec![(pos, chunk.clone())],
            vec![entity(10, [8.0, 64.0, 8.0])],
            ChangeSet::default(),
        );
        plan_viewer(&start, &mut v, 1, c);
        assert!(v.tracked_entities.contains_key(&EntityId(10)));
        let mut turned = entity(10, [8.5, 64.0, 8.0]);
        turned.head_yaw = 90.0;
        let stopped = output(vec![(pos, chunk)], vec![turned], ChangeSet::default());
        let deltas = [MoveEntityPos::ID, RotateHead::ID];
        v.world_credit = 0;
        let starved = plan_viewer(&stopped, &mut v, 1, c);
        assert!(!starved.iter().any(|p| deltas.contains(&p.id)));
        v.world_credit = 512;
        let again = plan_viewer(&stopped, &mut v, 1, c);
        let ids: Vec<i32> = again.iter().map(|p| p.id).collect();
        assert!(
            deltas.iter().all(|id| ids.contains(id)),
            "deferred deltas sent: {ids:02x?}"
        );
        let quiet = plan_viewer(&stopped, &mut v, 1, c);
        assert!(!quiet.iter().any(|p| deltas.contains(&p.id)));
    });
}

/// A block update deferred for lack of credit is sent with the snapshot's
/// newest state on a later publish, which carries no block change; then
/// never again.
#[test]
fn deferred_block_update_is_sent_from_the_snapshot() {
    use mc_protocol::packets::play::{BlockUpdate, pack_block_pos};
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        plan_viewer(
            &output(vec![(pos, chunk.clone())], vec![], ChangeSet::default()),
            &mut v,
            0,
            c,
        );
        assert!(v.loaded_chunks.contains_key(&(0, 0)));
        let mut edited = (*chunk).clone();
        edited.set_block(3, 64, 5, mc_world::BlockStateId(1));
        let edited = Arc::new(edited);
        let mut changes = ChangeSet::default();
        changes.blocks.push(BlockChange {
            position: mc_world::chunk::BlockPos { x: 3, y: 64, z: 5 },
            state: 1,
        });
        v.world_credit = 0;
        let changed_out = output(vec![(pos, edited)], vec![], changes);
        let changed = plan_viewer(&changed_out, &mut v, 0, c);
        assert!(!changed.iter().any(|p| p.id == BlockUpdate::ID));
        v.world_credit = 512;
        // Same snapshot (same revision), no change set.
        let quiet = TickOutput {
            changes: Arc::new(ChangeSet::default()),
            ..changed_out
        };
        let updates = |packets: Vec<PlannedPacket>| -> Vec<BlockUpdate> {
            packets
                .iter()
                .filter(|p| p.id == BlockUpdate::ID)
                .map(|p| BlockUpdate::decode(&mut &body_of(p)[..]).expect("BlockUpdate decodes"))
                .collect()
        };
        assert_eq!(
            updates(plan_viewer(&quiet, &mut v, 0, c)),
            vec![BlockUpdate {
                position: pack_block_pos(3, 64, 5),
                state_id: 1,
            }]
        );
        assert!(updates(plan_viewer(&quiet, &mut v, 0, c)).is_empty());
    });
}

/// After the pump skipped outputs, a held chunk whose revision moved on is
/// sent again whole and a changed appearance is resent, with no change set
/// describing either; without a skip, this tick's block updates cover a
/// touched chunk and nothing is resent.
#[test]
fn skipped_outputs_are_reconciled_from_the_snapshot() {
    use mc_protocol::packets::play::{BlockUpdate, ClientboundSetEntityData, LevelChunkWithLight};
    with_cache(|c| {
        let ids = |packets: &[PlannedPacket]| packets.iter().map(|p| p.id).collect::<Vec<_>>();
        let (pos, chunk) = chunk_at(0, 0);
        let mut v = viewer_at(1, [8.0, 64.0, 8.0]);
        plan_viewer(
            &output(
                vec![(pos, chunk.clone())],
                vec![entity(10, [8.0, 64.0, 8.0])],
                ChangeSet::default(),
            ),
            &mut v,
            0,
            c,
        );
        // Not skipped: the block change explains the new revision.
        let mut changes = ChangeSet::default();
        changes.blocks.push(BlockChange {
            position: mc_world::chunk::BlockPos { x: 3, y: 64, z: 5 },
            state: 1,
        });
        let mut same = entity(10, [8.0, 64.0, 8.0]);
        same.appearance = Arc::clone(&v.tracked_entities[&EntityId(10)].appearance);
        let edited = plan_viewer(
            &output(vec![(pos, chunk.clone())], vec![same.clone()], changes),
            &mut v,
            0,
            c,
        );
        assert!(ids(&edited).contains(&BlockUpdate::ID));
        assert!(!ids(&edited).contains(&LevelChunkWithLight::ID));
        assert!(!ids(&edited).contains(&ClientboundSetEntityData::ID));
        // Skipped outputs changed the chunk and the entity's appearance.
        v.skipped = true;
        let changed = entity(10, [8.0, 64.0, 8.0]);
        let after_skip = plan_viewer(
            &output(vec![(pos, chunk)], vec![changed], ChangeSet::default()),
            &mut v,
            0,
            c,
        );
        assert!(ids(&after_skip).contains(&LevelChunkWithLight::ID));
        assert!(ids(&after_skip).contains(&ClientboundSetEntityData::ID));
        assert!(!v.skipped);
    });
}

#[test]
fn unseen_chunk_window_completes_before_continuously_stale_repairs() {
    with_cache(|cache| {
        let chunks = (-6..=6)
            .flat_map(|x| (-6..=6).map(move |z| chunk_at(x, z)))
            .collect();
        let mut out = output(chunks, Vec::new(), ChangeSet::default());
        let mut viewer = viewer_at(1, [0.5, 64.0, 0.5]);
        let mut delivered = HashSet::default();
        for _ in 0..11 {
            for chunk in Arc::make_mut(&mut Arc::get_mut(&mut out.snapshot).unwrap().chunks) {
                if (-4..0).contains(&chunk.pos.x) && (-4..0).contains(&chunk.pos.z) {
                    chunk.revision += 1;
                }
            }
            let packets = plan_viewer(&out, &mut viewer, 6, cache);
            let sent: Vec<_> = packets.iter().filter_map(|packet| packet.chunk).collect();
            assert!(sent.len() <= CHUNKS_PER_TICK);
            delivered.extend(sent);
        }
        assert_eq!(
            delivered.len(),
            169,
            "repairs must not starve the initial complete window"
        );
        assert_eq!(viewer.loaded_chunks.len(), 169);
        // A skipped/credit-starved publish must retain repair debt even when
        // later snapshots no longer carry the original change set.
        for chunk in Arc::make_mut(&mut Arc::get_mut(&mut out.snapshot).unwrap().chunks) {
            if (-4..0).contains(&chunk.pos.x) && (-4..0).contains(&chunk.pos.z) {
                chunk.revision += 1;
            }
        }
        viewer.skipped = true;
        viewer.world_credit = 0;
        assert!(
            plan_viewer(&out, &mut viewer, 6, cache)
                .iter()
                .all(|packet| packet.chunk.is_none())
        );
        viewer.world_credit = usize::MAX;
        let repair = plan_viewer(&out, &mut viewer, 6, cache);
        assert_eq!(
            repair
                .iter()
                .filter(|packet| packet.chunk.is_some())
                .count(),
            16
        );
        for chunk in out.snapshot.chunks.iter() {
            assert_eq!(
                viewer.loaded_chunks[&(chunk.pos.x, chunk.pos.z)],
                chunk.revision
            );
        }
    });
}

#[test]
fn reliable_public_block_entity_update_covers_chunk_revision_without_full_resend() {
    with_cache(|cache| {
        let (pos, chunk) = chunk_at(0, 0);
        let mut viewer = viewer_at(1, [0.5, 64.0, 0.5]);
        plan_viewer(
            &output(
                vec![(pos, Arc::clone(&chunk))],
                Vec::new(),
                ChangeSet::default(),
            ),
            &mut viewer,
            0,
            cache,
        );
        let packets = plan_player_changes(
            &[PlayerChange::BlockEntityData {
                position: mc_world::chunk::BlockPos { x: 1, y: 64, z: 1 },
                block_entity_type: 7,
                nbt: mc_nbt::Tag::Compound(Vec::new()),
            }],
            &mut viewer,
        );
        assert_eq!(packets.len(), 1);
        assert!(packets[0].reliable);
        let after = output(vec![(pos, chunk)], Vec::new(), ChangeSet::default());
        assert!(
            plan_viewer(&after, &mut viewer, 0, cache)
                .iter()
                .all(|packet| packet.chunk.is_none())
        );
        assert_eq!(
            viewer.loaded_chunks[&(0, 0)],
            after.snapshot.chunks[0].revision
        );
    });
}

/// Explode packets reach viewers within 64 blocks only, carrying that
/// viewer's own knockback.
#[test]
fn explosion_reaches_nearby_viewers_with_their_knockback() {
    with_cache(|c| {
        let (pos, chunk) = chunk_at(0, 0);
        let changes = || ChangeSet {
            explosions: vec![mc_core::boundary::ExplosionEvent {
                center: [8.0, 64.0, 8.0],
                radius: 3.0,
                blocks: 5,
                knockback: vec![(EntityId(1), [0.5, 0.25, 0.0])],
            }],
            ..Default::default()
        };
        let mut near = viewer_at(1, [8.0, 64.0, 12.0]);
        let out = output(vec![(pos, Arc::clone(&chunk))], vec![], changes());
        let packets = plan_viewer(&out, &mut near, 0, c);
        let blast = packets
            .iter()
            .find(|p| p.id == ClientboundExplode::ID)
            .expect("explode packet");
        let body = body_of(blast);
        let decoded = ClientboundExplode::decode(&mut &body[..]).expect("decodes");
        assert_eq!(decoded.block_count, 5);
        assert_eq!(decoded.knockback.map(|push| push.y), Some(0.25));
        // Another viewer is pushed by nothing.
        let mut other = viewer_at(2, [8.0, 64.0, 12.0]);
        let packets = plan_viewer(&out, &mut other, 0, c);
        let body = body_of(
            packets
                .iter()
                .find(|p| p.id == ClientboundExplode::ID)
                .expect("explode packet"),
        );
        let decoded = ClientboundExplode::decode(&mut &body[..]).expect("decodes");
        assert!(decoded.knockback.is_none());
        let mut far = viewer_at(3, [400.0, 64.0, 8.0]);
        let out = output(vec![(pos, chunk)], vec![], changes());
        let packets = plan_viewer(&out, &mut far, 0, c);
        assert!(packets.iter().all(|p| p.id != ClientboundExplode::ID));
    });
}

/// Arrows are no living entities: a health float at index 9 would hit the
/// client's `PIERCE_LEVEL` byte accessor and kill the session.
#[test]
fn arrow_entity_data_carries_no_health() {
    let types = mc_data::entity_types::solaris_required_entity_types();
    let id = |name: &str| {
        types
            .id_of(&mc_data::Identifier::parse(name).expect("id"))
            .expect("type")
    };
    let planned = |type_id: u32| {
        let mut out = Vec::new();
        push_appearance_for(
            &mut out,
            ConnectionId(1),
            EntityId(5),
            &EntityAppearance::default(),
            type_id,
            None,
        );
        ClientboundSetEntityData::decode(&mut body_of(&out[0])).expect("appearance metadata")
    };
    let arrow = planned(id("minecraft:arrow"));
    assert!(
        arrow
            .values
            .iter()
            .all(|value| !matches!(value.index(), 9 | 10 | 11 | 17))
    );
    let skeleton = planned(id("minecraft:skeleton"));
    assert!(skeleton.values.contains(&WireEntityDataValue::Float {
        index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
        value: 0.0,
    }));
    assert!(
        skeleton
            .values
            .contains(&WireEntityDataValue::EffectParticles {
                index: LIVING_ENTITY_DATA_EFFECT_PARTICLES_INDEX,
                particles: Vec::new(),
            })
    );
    assert!(skeleton.values.contains(&WireEntityDataValue::Boolean {
        index: LIVING_ENTITY_DATA_EFFECT_AMBIENCE_INDEX,
        value: false,
    }));
}

#[test]
fn sign_editor_installs_block_before_opening_and_text_uses_vanilla_entity_type() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    let position = mc_core::boundary::BlockPos { x: 9, y: 64, z: 8 };
    let packets = plan_player_changes(
        &[
            PlayerChange::SignEditorOpened {
                position,
                front: true,
                block: 123,
            },
            PlayerChange::BlockEntityData {
                position,
                block_entity_type: 7,
                nbt: mc_nbt::Tag::Compound(vec![("is_waxed".into(), mc_nbt::Tag::Byte(0))]),
            },
        ],
        &mut viewer,
    );
    assert_eq!(
        packets.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![
            BlockUpdate::ID,
            mc_protocol::packets::play::ClientboundOpenSignEditor::ID,
            mc_protocol::packets::play::ClientboundBlockEntityData::ID,
        ]
    );
    assert!(packets.iter().all(|p| p.reliable));
    let bytes = body_of(&packets[2]);
    let text =
        mc_protocol::packets::play::ClientboundBlockEntityData::decode(&mut &bytes[..]).unwrap();
    assert_eq!(text.block_entity_type, 7);
    assert_eq!(
        text.position,
        pack_block_pos(position.x, position.y, position.z)
    );
}

#[test]
fn accepted_combat_motion_is_reliable_self_only_and_uses_wire_tick_units() {
    let mut viewer = ViewerState::new(ConnectionId(1), EntityId(41), [8.0, 64.0, 8.0]);
    viewer.world_credit = 0;
    let change = PlayerChange::Motion {
        entity: EntityId(41),
        velocity: [-8.0, 8.0, 4.0],
    };
    let packets = plan_player_changes(std::slice::from_ref(&change), &mut viewer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].connection, ConnectionId(1));
    assert_eq!(packets[0].id, SetEntityMotion::ID);
    assert!(
        packets[0].reliable,
        "motion survives exhausted world credit and skipped snapshots"
    );
    let decoded = SetEntityMotion::decode(&mut body_of(&packets[0])).unwrap();
    assert_eq!(decoded.entity_id, 41, "core entity id, not connection id");
    for (actual, expected) in [decoded.movement.x, decoded.movement.y, decoded.movement.z]
        .into_iter()
        .zip([-0.4, 0.4, 0.2])
    {
        assert!((actual - expected).abs() < 1.0e-3);
    }
}

#[test]
fn campfire_items_sync_is_reliable_and_targets_only_its_viewer() {
    let position = mc_core::boundary::BlockPos { x: 9, y: 64, z: 8 };
    let nbt = mc_nbt::Tag::Compound(vec![(
        "Items".into(),
        mc_nbt::Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::COMPOUND,
            elements: vec![mc_nbt::Tag::Compound(vec![
                ("Slot".into(), mc_nbt::Tag::Byte(0)),
                ("id".into(), mc_nbt::Tag::String("minecraft:chicken".into())),
                ("count".into(), mc_nbt::Tag::Int(1)),
            ])],
        }),
    )]);
    let change = PlayerChange::BlockEntityData {
        position,
        block_entity_type: 33,
        nbt: nbt.clone(),
    };
    let packets = plan_player_changes(
        std::slice::from_ref(&change),
        &mut viewer_at(1, [8.0, 64.0, 8.0]),
    );
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    assert_eq!(
        packets[0].id,
        mc_protocol::packets::play::ClientboundBlockEntityData::ID
    );
    let body = body_of(&packets[0]);
    let decoded =
        mc_protocol::packets::play::ClientboundBlockEntityData::decode(&mut &body[..]).unwrap();
    assert_eq!(decoded.block_entity_type, 33);
    assert_eq!(
        decoded.position,
        pack_block_pos(position.x, position.y, position.z)
    );
    assert_eq!(decoded.nbt, nbt);
}

#[test]
fn shelf_items_sync_uses_native_type_thirty_nine_and_slot_byte() {
    use mc_data::block_entity_kinds::BlockEntityKind;
    use mc_nbt::{ListTag, Tag, tag_type};
    use mc_protocol::packets::play::ClientboundBlockEntityData;
    let position = mc_core::boundary::BlockPos {
        x: 192,
        y: 78,
        z: -446,
    };
    let nbt = Tag::Compound(vec![
        (
            "Items".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("Slot".into(), Tag::Byte(1)),
                    ("id".into(), Tag::String("minecraft:diamond".into())),
                    ("count".into(), Tag::Int(1)),
                ])],
            }),
        ),
        ("align_items_to_bottom".into(), Tag::Byte(0)),
    ]);
    let change = PlayerChange::BlockEntityData {
        position,
        block_entity_type: BlockEntityKind::Shelf.protocol_id(),
        nbt: nbt.clone(),
    };
    let packets = plan_player_changes(
        std::slice::from_ref(&change),
        &mut viewer_at(1, [192.0, 78.0, -446.0]),
    );
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    assert_eq!(packets[0].id, ClientboundBlockEntityData::ID);
    let body = body_of(&packets[0]);
    let mut cursor = &body[..];
    let decoded = ClientboundBlockEntityData::decode(&mut cursor).unwrap();
    assert!(cursor.is_empty(), "native packet has no trailing fields");
    assert_eq!(decoded.block_entity_type, 39);
    assert_eq!(decoded.position, pack_block_pos(192, 78, -446));
    assert_eq!(decoded.nbt, nbt);
}

#[test]
fn sweep_event_encodes_vanilla_sound_and_single_directional_particle() {
    use mc_protocol::packets::play::{ClientboundSoundEntity, ClientboundSweepParticle};
    let mut packets = Vec::new();
    push_entity_event(
        &mut packets,
        ConnectionId(1),
        BoundaryEvent::SweepAttack {
            entity: EntityId(41),
            position: [8.0, 64.9, 9.0],
            yaw: 0.0,
        },
    );
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].id, 0x76);
    let sound = ClientboundSoundEntity::decode(&mut &body_of(&packets[0])[..]).unwrap();
    assert_eq!(sound.sound.as_str(), "minecraft:entity.player.attack.sweep");
    assert_eq!((sound.entity_id, sound.source), (41, 7));
    assert_eq!((sound.volume, sound.pitch), (1.0, 1.0));
    assert_eq!(packets[1].id, 0x30);
    let body = body_of(&packets[1]);
    assert_eq!(body.len(), 53);
    assert_eq!(&body[..3], &[77, 0, 0], "26.3 particle id precedes flags");
    assert_eq!(
        &body[39..],
        &[0; 14],
        "zero speeds, count and randomization"
    );
    let particle = ClientboundSweepParticle::decode(&mut &body[..]).unwrap();
    assert_eq!(particle.position, [8.0, 64.9, 9.0]);
    assert_eq!(particle.direction, [0.0, 0.0, 1.0]);
}

#[test]
fn command_sound_and_particle_changes_encode_packets() {
    use mc_protocol::packets::play::{ClientboundCommandParticle, ClientboundSound};
    let changes = [
        PlayerChange::CommandSound {
            sound: mc_data::Identifier::parse("minecraft:block.bell.use").unwrap(),
            source: 4,
            position: [8, 480, 16],
            volume: 1.0,
            pitch: 1.0,
        },
        PlayerChange::CommandParticle {
            particle_id: 77,
            position: [1.0, 60.0, 2.0],
            delta: [0.0; 3],
            speed: 0.0,
            count: 1,
            force: false,
        },
    ];
    let packets = plan_player_changes(&changes, &mut viewer_at(1, [1.0, 60.0, 2.0]));
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].id, ClientboundSound::ID);
    let sound = ClientboundSound::decode(&mut &body_of(&packets[0])[..]).unwrap();
    assert_eq!(sound.sound.as_str(), "minecraft:block.bell.use");
    assert_eq!(sound.position, [8, 480, 16]);
    assert_eq!(packets[1].id, ClientboundCommandParticle::ID);
    let particle = ClientboundCommandParticle::decode(&mut &body_of(&packets[1])[..]).unwrap();
    assert_eq!(particle.particle_id, 77);
    assert_eq!(particle.count, 1);
}

#[test]
fn shield_events_send_vanilla_entity_sound_not_removed_status_bytes() {
    use mc_protocol::packets::play::ClientboundSoundEntity;
    for disabled in [false, true] {
        let mut packets = Vec::new();
        push_entity_event(
            &mut packets,
            ConnectionId(1),
            BoundaryEvent::ShieldBlock {
                entity: EntityId(41),
                disabled,
            },
        );
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].id, 0x76);
        let sound = ClientboundSoundEntity::decode(&mut &body_of(&packets[0])[..]).unwrap();
        assert_eq!(sound.entity_id, 41);
        assert_eq!(sound.source, 7);
        assert_eq!(
            sound.sound.as_str(),
            if disabled {
                "minecraft:item.shield.break"
            } else {
                "minecraft:item.shield.block"
            }
        );
        assert_eq!(sound.volume, if disabled { 0.8 } else { 1.0 });
        assert_eq!(sound.pitch, 1.0);
        assert_eq!(sound.fixed_range, None);
    }
}

#[test]
fn shield_cooldown_is_reliable_and_only_reaches_the_owner() {
    use mc_protocol::packets::play::ClientboundCooldown;
    let change = PlayerChange::ShieldCooldown { ticks: 100 };
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.world_credit = 0;
    let packets = plan_player_changes(std::slice::from_ref(&change), &mut viewer);
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    let cooldown = ClientboundCooldown::decode(&mut &body_of(&packets[0])[..]).unwrap();
    assert_eq!(cooldown.duration, 100);
    assert_eq!(cooldown.cooldown_group.as_str(), "minecraft:shield");
}

#[test]
fn villager_growth_resets_baby_metadata_and_preserves_villager_data() {
    let types = mc_data::entity_types::solaris_required_entity_types();
    let type_id = types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    for baby in [true, false] {
        let mut out = Vec::new();
        push_appearance_for(
            &mut out,
            ConnectionId(1),
            EntityId(2),
            &EntityAppearance {
                baby,
                extra: vec![(
                    19,
                    mc_core::boundary::EntityDataValue::VillagerData {
                        kind: 2,
                        profession: 5,
                        level: 3,
                    },
                )],
                ..Default::default()
            },
            type_id,
            None,
        );
        let packet = ClientboundSetEntityData::decode(&mut body_of(&out[0])).unwrap();
        assert!(packet.values.contains(&WireEntityDataValue::Boolean {
            index: 16,
            value: baby
        }));
        assert!(packet.values.contains(&WireEntityDataValue::VillagerData {
            index: 19,
            villager_type: 2,
            profession: 5,
            level: 3
        }));
    }
}

#[test]
fn villager_conversion_replaces_known_entity_type_before_respawning_same_identity() {
    with_cache(|cache| {
        let types = mc_data::entity_types::solaris_required_entity_types();
        let id = |name: &str| {
            types
                .id_of(&mc_data::Identifier::parse(name).unwrap())
                .unwrap()
        };
        let (pos, chunk) = chunk_at(0, 0);
        let mut resident = entity(2, [8., 80., 8.]);
        resident.type_id = id("minecraft:villager");
        let uuid = resident.uuid;
        let mut viewer = viewer_at(1, [8., 80., 8.]);
        plan_viewer(
            &output(
                vec![(pos, Arc::clone(&chunk))],
                vec![resident.clone()],
                ChangeSet::default(),
            ),
            &mut viewer,
            1,
            cache,
        );
        for name in ["minecraft:zombie_villager", "minecraft:villager"] {
            resident.type_id = id(name);
            let packets = plan_viewer(
                &output(
                    vec![(pos, Arc::clone(&chunk))],
                    vec![resident.clone()],
                    ChangeSet::default(),
                ),
                &mut viewer,
                1,
                cache,
            );
            let removal = packets
                .iter()
                .position(|p| p.id == RemoveEntities::ID)
                .unwrap();
            let spawn = packets.iter().position(|p| p.id == AddEntity::ID).unwrap();
            assert!(removal < spawn);
            let added = AddEntity::decode(&mut body_of(&packets[spawn])).unwrap();
            assert_eq!(added.entity_id, 2);
            assert_eq!(added.uuid, uuid);
            assert_eq!(added.entity_type_id, resident.type_id as i32);
            assert_eq!(
                viewer
                    .tracked_entities
                    .get(&EntityId(2))
                    .map(|tracked| tracked.type_id),
                Some(resident.type_id)
            );
        }
    });
}

#[test]
fn animal_growth_clears_child_metadata_for_every_breeder_schema() {
    let types = mc_data::entity_types::solaris_required_entity_types();
    for name in [
        "cow",
        "pig",
        "sheep",
        "chicken",
        "rabbit",
        "mooshroom",
        "fox",
        "ocelot",
        "panda",
        "goat",
        "armadillo",
        "sniffer",
        "horse",
        "donkey",
        "llama",
        "camel",
        "strider",
        "wolf",
        "cat",
        "bee",
        "happy_ghast",
        "axolotl",
        "frog",
        "turtle",
        "nautilus",
        "hoglin",
    ] {
        let type_id = types
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap();
        for baby in [true, false] {
            let mut out = Vec::new();
            push_appearance_for(
                &mut out,
                ConnectionId(1),
                EntityId(5),
                &EntityAppearance {
                    baby,
                    ..Default::default()
                },
                type_id,
                None,
            );
            let packet = ClientboundSetEntityData::decode(&mut body_of(&out[0])).unwrap();
            assert!(
                packet.values.contains(&WireEntityDataValue::Boolean {
                    index: 16,
                    value: baby
                }),
                "{name} baby={baby}"
            );
        }
    }
    let piglin = types
        .id_of(&mc_data::Identifier::parse("minecraft:piglin").unwrap())
        .unwrap();
    let mut out = Vec::new();
    push_appearance_for(
        &mut out,
        ConnectionId(1),
        EntityId(6),
        &EntityAppearance {
            baby: true,
            ..Default::default()
        },
        piglin,
        None,
    );
    let packet = ClientboundSetEntityData::decode(&mut body_of(&out[0])).unwrap();
    assert!(packet.values.contains(&WireEntityDataValue::Boolean {
        index: 17,
        value: true
    }));
    assert!(!packet.values.contains(&WireEntityDataValue::Boolean {
        index: 16,
        value: true
    }));
}

#[test]
fn body_and_saddle_equipment_use_exact_slots_and_clear_after_removal() {
    let type_id = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:horse").unwrap())
        .unwrap();
    let equipped = EntityAppearance {
        body_armor: Some(mc_data::ItemStack::new(1, 1)),
        saddle: Some(mc_data::ItemStack::new(2, 1)),
        ..Default::default()
    };
    let cleared = EntityAppearance::default();
    for (appearance, previous) in [(&equipped, None), (&cleared, Some(&equipped))] {
        let mut out = Vec::new();
        push_appearance_for(
            &mut out,
            ConnectionId(1),
            EntityId(5),
            appearance,
            type_id,
            previous,
        );
        assert_eq!(out.len(), 2);
        let packet = ClientboundSetEntityEquipment::decode(&mut body_of(&out[1])).unwrap();
        assert_eq!(packet.entity_id, 5);
        assert_eq!(packet.equipment.len(), 8);
        assert_eq!(packet.equipment[6].slot, EquipmentSlot::Body);
        assert_eq!(packet.equipment[7].slot, EquipmentSlot::Saddle);
        assert_eq!(
            packet.equipment[6].item,
            appearance.body_armor.clone().unwrap_or_default()
        );
        assert_eq!(
            packet.equipment[7].item,
            appearance.saddle.clone().unwrap_or_default()
        );
        assert!(
            packet.equipment[..6]
                .iter()
                .all(|entry| entry.item.is_empty())
        );
    }
    let mut out = Vec::new();
    push_appearance_for(
        &mut out,
        ConnectionId(1),
        EntityId(5),
        &equipped,
        type_id,
        Some(&equipped),
    );
    assert_eq!(
        out.len(),
        1,
        "unchanged equipment is not resent with health"
    );
}

#[test]
fn armor_stand_equipment_and_stored_pose_project_to_vanilla_packets_and_clear_slots() {
    let type_id = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:armor_stand").unwrap())
        .unwrap();
    let mut equipped = EntityAppearance::default();
    equipped.equipment[0] = Some(mc_data::ItemStack::new(1, 1));
    equipped.equipment[5] = Some(mc_data::ItemStack::new(2, 1));
    equipped.extra = vec![
        (15, mc_core::boundary::EntityDataValue::Byte(4)),
        (
            16,
            mc_core::boundary::EntityDataValue::Rotations([10.0, 20.0, 30.0]),
        ),
    ];
    let cleared = EntityAppearance {
        extra: equipped.extra.clone(),
        ..Default::default()
    };
    for (appearance, previous) in [(&equipped, None), (&cleared, Some(&equipped))] {
        let mut out = Vec::new();
        push_appearance_for(
            &mut out,
            ConnectionId(1),
            EntityId(5),
            appearance,
            type_id,
            previous,
        );
        assert_eq!(out.len(), 2);
        let metadata = ClientboundSetEntityData::decode(&mut body_of(&out[0])).unwrap();
        assert!(metadata.values.contains(&WireEntityDataValue::Byte {
            index: 15,
            value: 4
        }));
        assert!(metadata.values.contains(&WireEntityDataValue::Rotations {
            index: 16,
            value: EntityRotations {
                x: 10.0,
                y: 20.0,
                z: 30.0
            },
        }));
        let equipment = ClientboundSetEntityEquipment::decode(&mut body_of(&out[1])).unwrap();
        assert_eq!(equipment.entity_id, 5);
        assert_eq!(equipment.equipment[0].slot, EquipmentSlot::MainHand);
        assert_eq!(equipment.equipment[5].slot, EquipmentSlot::Head);
        assert_eq!(
            equipment.equipment[0].item,
            appearance.equipment[0].clone().unwrap_or_default()
        );
        assert_eq!(
            equipment.equipment[5].item,
            appearance.equipment[5].clone().unwrap_or_default()
        );
    }
}

#[path = "cooldown_tests.rs"]
mod cooldown_tests;
#[path = "dialog_tests.rs"]
mod dialog_tests;

#[test]
fn native_note_event_installs_state_before_protocol_07_and_is_viewer_owned() {
    use mc_protocol::packets::play::ClientboundBlockEvent;
    let position = mc_core::boundary::BlockPos {
        x: -1,
        y: 200,
        z: 2,
    };
    let changes = [PlayerChange::BlockEvent {
        position,
        block: 103,
        state: 9876,
        action: 0,
        parameter: 0,
    }];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 2);
    assert!(
        packets
            .iter()
            .all(|packet| packet.reliable && packet.connection == ConnectionId(7))
    );
    assert_eq!(packets[0].id, BlockUpdate::ID);
    let update = BlockUpdate::decode(&mut body_of(&packets[0])).unwrap();
    assert_eq!(update.position, pack_block_pos(-1, 200, 2));
    assert_eq!(update.state_id, 9876);
    assert_eq!(packets[1].id, 0x07);
    let event = ClientboundBlockEvent::decode(&mut body_of(&packets[1])).unwrap();
    assert_eq!(event.position, update.position);
    assert_eq!(event.block_type, 103);
    assert_eq!((event.action, event.parameter), (0, 0));
}

#[test]
fn bell_ring_maps_animation_and_vanilla_positional_sound() {
    use mc_protocol::packets::play::{ClientboundBlockEvent, ClientboundSound};
    let position = mc_core::boundary::BlockPos {
        x: -1,
        y: 200,
        z: 2,
    };
    let changes = [
        PlayerChange::BlockEvent {
            position,
            block: 103,
            state: 9876,
            action: 1,
            parameter: 2,
        },
        PlayerChange::BellRung { position },
    ];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 3);
    assert!(
        packets
            .iter()
            .all(|packet| packet.reliable && packet.connection == ConnectionId(7))
    );
    assert_eq!(packets[0].id, BlockUpdate::ID);
    assert_eq!(packets[1].id, ClientboundBlockEvent::ID);
    let event = ClientboundBlockEvent::decode(&mut body_of(&packets[1])).unwrap();
    assert_eq!(
        (event.action, event.parameter, event.block_type),
        (1, 2, 103)
    );
    assert_eq!(packets[2].id, ClientboundSound::ID);
    let sound = ClientboundSound::decode(&mut body_of(&packets[2])).unwrap();
    assert_eq!(sound.sound.as_str(), "minecraft:block.bell.use");
    assert_eq!(sound.fixed_range, None);
    assert_eq!(sound.source, 4);
    assert_eq!(sound.position, [-4, 1604, 20]);
    assert_eq!((sound.volume, sound.pitch, sound.seed), (2.0, 1.0, 0));
}

#[test]
fn hanging_sign_editor_and_type_eight_nbt_use_native_packets_in_order() {
    use mc_protocol::packets::play::{ClientboundBlockEntityData, ClientboundOpenSignEditor};
    let position = mc_core::boundary::BlockPos { x: 1, y: 200, z: 2 };
    let nbt = mc_nbt::Tag::Compound(vec![("is_waxed".into(), mc_nbt::Tag::Byte(0))]);
    let changes = [
        PlayerChange::SignEditorOpened {
            position,
            front: true,
            block: 9876,
        },
        PlayerChange::BlockEntityData {
            position,
            block_entity_type: 8,
            nbt: nbt.clone(),
        },
    ];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 3);
    assert_eq!(packets[0].id, BlockUpdate::ID);
    let editor = ClientboundOpenSignEditor::decode(&mut body_of(&packets[1])).unwrap();
    assert!(editor.is_front_text);
    assert_eq!(editor.position, pack_block_pos(1, 200, 2));
    let text = ClientboundBlockEntityData::decode(&mut body_of(&packets[2])).unwrap();
    assert_eq!(text.position, editor.position);
    assert_eq!(text.block_entity_type, 8);
    assert_eq!(text.nbt, nbt);
}

#[test]
fn world_border_init_changes_and_credit_deferral_use_vanilla_packet() {
    use mc_protocol::packets::play::ClientboundInitializeBorder;
    with_cache(|cache| {
        let mut viewer = viewer_at(1, [0.0; 3]);
        let mut output = output(Vec::new(), Vec::new(), ChangeSet::default());
        let borders = |packets: Vec<PlannedPacket>| {
            packets
                .into_iter()
                .filter(|packet| packet.id == ClientboundInitializeBorder::ID)
                .map(|packet| ClientboundInitializeBorder::decode(&mut body_of(&packet)).unwrap())
                .collect::<Vec<_>>()
        };
        let initial = borders(plan_viewer(&output, &mut viewer, 0, cache));
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].new_size, mc_core::command_border::MAX_SIZE);
        assert!(borders(plan_viewer(&output, &mut viewer, 0, cache)).is_empty());
        Arc::get_mut(&mut output.snapshot).unwrap().border = mc_core::command_border::Border {
            center_x: 2.5,
            center_z: -3.5,
            size: 100.0,
            lerp_target: 60.0,
            lerp_remaining: 200,
            warning_blocks: 7,
            warning_time: 3,
            ..Default::default()
        };
        viewer.world_credit = 0;
        assert!(borders(plan_viewer(&output, &mut viewer, 0, cache)).is_empty());
        viewer.world_credit = 10;
        let changed = borders(plan_viewer(&output, &mut viewer, 0, cache));
        assert_eq!(changed.len(), 1);
        assert_eq!(
            (
                changed[0].center_x,
                changed[0].center_z,
                changed[0].new_size
            ),
            (2.5, -3.5, 60.0)
        );
        assert_eq!(changed[0].old_size, 100.0);
        assert_eq!(changed[0].lerp_time, 200);
        assert_eq!(changed[0].warning_blocks, 7);
        assert_eq!(changed[0].warning_time, 3);
        assert!(borders(plan_viewer(&output, &mut viewer, 0, cache)).is_empty());
    });
}

#[test]
fn warden_emerging_spawn_data_and_all_warden_poses_use_verified_ordinals() {
    with_cache(|cache| {
        for (pose, expected) in [
            (11, EntityPose::Roaring),
            (12, EntityPose::Sniffing),
            (13, EntityPose::Emerging),
            (14, EntityPose::Digging),
        ] {
            let (pos, chunk) = chunk_at(0, 0);
            let mut warden = entity(2, [8.0, 78.0, 8.0]);
            warden.type_id = mc_data::entity_types::solaris_required_entity_types()
                .id_of(&mc_data::Identifier::parse("minecraft:warden").unwrap())
                .unwrap();
            warden.appearance = Arc::new(EntityAppearance {
                pose,
                spawn_data: i32::from(pose == 13),
                health: 500.0,
                extra: vec![(16, mc_core::boundary::EntityDataValue::VarInt(105))],
                ..Default::default()
            });
            let mut changes = ChangeSet::default();
            changes.entities_spawned.push(warden.id);
            let tick = output(vec![(pos, chunk)], vec![warden], changes);
            let mut viewer = viewer_at(1, [8.0, 78.0, 8.0]);
            let packets = plan_viewer(&tick, &mut viewer, 1, cache);
            let spawn = packets
                .iter()
                .find(|packet| packet.id == AddEntity::ID)
                .unwrap();
            let spawn = AddEntity::decode(&mut body_of(spawn)).unwrap();
            assert_eq!(spawn.data, i32::from(pose == 13));
            let data: Vec<_> = packets
                .iter()
                .filter(|packet| packet.id == ClientboundSetEntityData::ID)
                .flat_map(|packet| {
                    ClientboundSetEntityData::decode(&mut body_of(packet))
                        .unwrap()
                        .values
                })
                .collect();
            assert!(data.iter().any(
                |value| matches!(value,WireEntityDataValue::Pose{index:6,pose} if *pose==expected)
            ));
            assert!(data.iter().any(|value| matches!(
                value,
                WireEntityDataValue::Int {
                    index: 16,
                    value: 105
                }
            )));
        }
    });
}

#[path = "bottles_tests.rs"]
mod bottles;

/// Vanilla sends SetEquipment only to trackers: the owner's client writes
/// MAINHAND into its currently selected slot, so a self packet ghosts items.
#[test]
fn player_equipment_reaches_trackers_but_never_the_owner() {
    with_cache(|cache| {
        let player_type = mc_data::entity_types::solaris_required_entity_types()
            .id_of(&mc_data::Identifier::parse("minecraft:player").unwrap())
            .unwrap();
        let (pos, chunk) = chunk_at(0, 0);
        let mut player = entity(1, [8., 80., 8.]);
        player.type_id = player_type;
        let mut equipment: [Option<mc_data::ItemStack>; 6] = Default::default();
        equipment[0] = Some(mc_data::ItemStack::new(1, 1));
        player.appearance = Arc::new(EntityAppearance {
            equipment,
            ..Default::default()
        });
        let equipment_for = |viewer: &mut ViewerState| {
            plan_viewer(
                &output(
                    vec![(pos, Arc::clone(&chunk))],
                    vec![player.clone()],
                    ChangeSet::default(),
                ),
                viewer,
                1,
                cache,
            )
            .iter()
            .filter(|p| p.id == ClientboundSetEntityEquipment::ID)
            .map(|p| {
                ClientboundSetEntityEquipment::decode(&mut body_of(p))
                    .unwrap()
                    .entity_id
            })
            .collect::<Vec<_>>()
        };
        assert!(equipment_for(&mut viewer_at(1, [8., 80., 8.])).is_empty());
        assert_eq!(equipment_for(&mut viewer_at(2, [8., 80., 8.])), vec![1]);
    });
}

#[test]
fn custom_name_visible_uses_entity_boolean_index_three_on_spawn_and_change() {
    let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:cow")
        .unwrap()
        .protocol_id;
    for visible in [false, true] {
        let appearance = EntityAppearance {
            custom_name: Some("Named Cow".into()),
            custom_name_visible: visible,
            ..Default::default()
        };
        for previous in [
            None,
            Some(EntityAppearance {
                custom_name_visible: !visible,
                ..appearance.clone()
            }),
        ] {
            let mut packets = Vec::new();
            push_appearance_for(
                &mut packets,
                ConnectionId(1),
                EntityId(2),
                &appearance,
                type_id,
                previous.as_ref(),
            );
            let data = packets
                .iter()
                .find(|packet| packet.id == ClientboundSetEntityData::ID)
                .unwrap();
            let decoded = ClientboundSetEntityData::decode(&mut body_of(data)).unwrap();
            assert!(decoded.values.contains(&WireEntityDataValue::Boolean {
                index: 3,
                value: visible
            }));
        }
        let mut packets = Vec::new();
        push_appearance_for(
            &mut packets,
            ConnectionId(1),
            EntityId(2),
            &appearance,
            type_id,
            Some(&appearance),
        );
        let data = packets
            .iter()
            .find(|packet| packet.id == ClientboundSetEntityData::ID)
            .unwrap();
        let decoded = ClientboundSetEntityData::decode(&mut body_of(data)).unwrap();
        assert!(
            !decoded
                .values
                .iter()
                .any(|value| matches!(value, WireEntityDataValue::Boolean { index: 3, .. }))
        );
    }
}

/// Short, in-process fanout benchmark. Run pinned in release, with --ignored
/// --nocapture --test-threads=1; setup and snapshot construction are not timed.
#[test]
#[ignore = "release performance receipt; not a correctness gate"]
fn edge_plan_scale_bench() {
    use std::time::Instant;
    for (scene, populations) in [("sparse", &[100, 500, 1000][..]), ("dense", &[100][..])] {
        for &players in populations {
            let mut entities = Vec::new();
            for cell in 0..1000_i32 {
                let x = if scene == "dense" { 0 } else { cell * 16 };
                for n in 0..100 {
                    let mut mob = entity(
                        10_000 + cell * 100 + n,
                        [f64::from(x * 16) + 8.0, 64.0, 8.0],
                    );
                    mob.type_id = 30; // Exact vanilla cow protocol id.
                    entities.push(mob);
                }
                if scene == "sparse" {
                    let mut own = entity(cell + 1, [f64::from(x * 16) + 8.0, 64.0, 8.0]);
                    own.type_id = player_entity_type_id();
                    entities.push(own);
                }
            }
            if scene == "dense" {
                for id in 1..=players {
                    let mut own = entity(id, [8.0, 64.0, 8.0]);
                    own.type_id = player_entity_type_id();
                    entities.push(own);
                }
            }
            let mut output = output(vec![], entities, ChangeSet::default());
            let mut viewers: Vec<_> = (0..players)
                .map(|n| {
                    let x = if scene == "dense" { 0 } else { n * 16 };
                    let mut viewer = viewer_at(n as u64 + 1, [f64::from(x * 16) + 8.0, 64.0, 8.0]);
                    viewer.loaded_chunks.insert((x, 0), 0);
                    viewer.world_credit = 512;
                    viewer
                })
                .collect();
            // Keep the dense fixture bounded like the live 512-slot pump:
            // measure admission + motion, not a fictitious unlimited queue.
            for _ in 0..4 {
                std::hint::black_box(crate::fanout::publish(&output, viewers.iter_mut(), 4));
            }
            let mut samples = Vec::new();
            let mut bytes = 0;
            for tick in 2..=41 {
                let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
                snapshot.tick = tick;
                snapshot.day_time += 1;
                let mut rows: Vec<EntityView> = snapshot.entities.iter().cloned().collect();
                for entity in &mut rows {
                    if entity.type_id == 30 {
                        entity.position[0] += 0.01;
                    }
                }
                snapshot.entities = rows.into();
                let start = Instant::now();
                let packets = crate::fanout::publish(&output, viewers.iter_mut(), 4);
                samples.push(start.elapsed().as_secs_f64() * 1e6);
                bytes += packets
                    .iter()
                    .map(|packet| packet.data.len())
                    .sum::<usize>();
                std::hint::black_box(packets);
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "EDGE_PLAN scene={scene} viewers={players} entities=100000 samples={} p50_us={:.3} p99_us={:.3} us_per_viewer={:.3} bytes_per_second_per_viewer={:.1}",
                samples.len(),
                samples[20],
                samples[39],
                samples[20] / f64::from(players),
                bytes as f64 * 20.0 / (40.0 * f64::from(players)),
            );
        }
    }
}

#[test]
fn indexed_entity_wire_trace_keeps_spawn_move_event_and_remove_bytes() {
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let mut mob = entity(10, [8.0, 64.0, 8.0]);
    mob.type_id = 1; // Preserve the S0/S1 wire fixture; move below is on its due tick.
    let initial = output(vec![], vec![mob.clone()], ChangeSet::default());
    let packets = plan_viewer(&initial, &mut viewer, 4, &cache);
    let mut expected = Vec::new();
    push_best_effort(
        &mut expected,
        viewer.connection,
        &AddEntity {
            entity_id: 10,
            uuid: mob.uuid,
            entity_type_id: mob.type_id as i32,
            x: 8.0,
            y: 64.0,
            z: 8.0,
            movement: per_tick(mob.velocity),
            pitch: mob.pitch,
            yaw: mob.yaw,
            head_yaw: mob.head_yaw,
            data: mob.appearance.spawn_data,
        },
    );
    push_appearance(&mut expected, viewer.connection, &mob, None);
    let wire = |packets: &[PlannedPacket]| {
        packets
            .iter()
            .filter(|packet| {
                [
                    AddEntity::ID,
                    ClientboundSetEntityData::ID,
                    MoveEntityPos::ID,
                    WireEntityEvent::ID,
                    RemoveEntities::ID,
                ]
                .contains(&packet.id)
            })
            .map(|packet| packet.data.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(wire(&packets), wire(&expected));
    mob.position[0] += 0.5;
    let mut moved = output(vec![], vec![mob.clone()], ChangeSet::default());
    Arc::get_mut(&mut moved.snapshot).unwrap().tick = 2;
    let packets = plan_viewer(&moved, &mut viewer, 4, &cache);
    expected.clear();
    push_best_effort(
        &mut expected,
        viewer.connection,
        &MoveEntityPos {
            entity_id: 10,
            delta_x: 2048,
            delta_y: 0,
            delta_z: 0,
            on_ground: true,
        },
    );
    assert_eq!(wire(&packets), wire(&expected));
    // Death of an entity no longer in the snapshot must precede removal.
    let gone = output(
        vec![],
        vec![],
        ChangeSet {
            entity_events: vec![BoundaryEvent::Death {
                entity: EntityId(10),
            }],
            ..Default::default()
        },
    );
    let packets = plan_viewer(&gone, &mut viewer, 4, &cache);
    expected.clear();
    push_entity_event(
        &mut expected,
        viewer.connection,
        BoundaryEvent::Death {
            entity: EntityId(10),
        },
    );
    push_best_effort(
        &mut expected,
        viewer.connection,
        &RemoveEntities {
            entity_ids: vec![10],
        },
    );
    assert_eq!(wire(&packets), wire(&expected));
}

#[test]
fn title_changes_plan_matching_title_packets() {
    use mc_core::boundary::TitleChange;
    use mc_protocol::packets::play::{
        ClientboundClearTitles, ClientboundSetActionBarText, ClientboundSetSubtitleText,
        ClientboundSetTitleText, ClientboundSetTitlesAnimation,
    };
    type TitleCase<'a> = (PlayerChange, i32, Box<dyn Fn(bytes::Bytes) + 'a>);
    let text = || mc_nbt::Tag::Compound(vec![("text".into(), mc_nbt::Tag::String("Hi".into()))]);
    let cases: Vec<TitleCase<'_>> = vec![
        (
            PlayerChange::Title(TitleChange::Title(text())),
            ClientboundSetTitleText::ID,
            Box::new(|body| {
                assert_eq!(
                    ClientboundSetTitleText::decode(&mut &body[..]).unwrap(),
                    ClientboundSetTitleText { content: text() }
                );
            }),
        ),
        (
            PlayerChange::Title(TitleChange::Subtitle(text())),
            ClientboundSetSubtitleText::ID,
            Box::new(|body| {
                assert_eq!(
                    ClientboundSetSubtitleText::decode(&mut &body[..]).unwrap(),
                    ClientboundSetSubtitleText { content: text() }
                );
            }),
        ),
        (
            PlayerChange::Title(TitleChange::ActionBar(text())),
            ClientboundSetActionBarText::ID,
            Box::new(|body| {
                assert_eq!(
                    ClientboundSetActionBarText::decode(&mut &body[..]).unwrap(),
                    ClientboundSetActionBarText { content: text() }
                );
            }),
        ),
        (
            PlayerChange::Title(TitleChange::Times {
                fade_in: 10,
                stay: 70,
                fade_out: 20,
            }),
            ClientboundSetTitlesAnimation::ID,
            Box::new(|body| {
                assert_eq!(
                    ClientboundSetTitlesAnimation::decode(&mut &body[..]).unwrap(),
                    ClientboundSetTitlesAnimation {
                        fade_in: 10,
                        stay: 70,
                        fade_out: 20,
                    }
                );
            }),
        ),
        (
            PlayerChange::Title(TitleChange::Clear { reset_times: true }),
            ClientboundClearTitles::ID,
            Box::new(|body| {
                assert_eq!(
                    ClientboundClearTitles::decode(&mut &body[..]).unwrap(),
                    ClientboundClearTitles { reset_times: true }
                );
            }),
        ),
    ];
    for (change, id, check) in cases {
        let mut viewer = viewer_at(7, [0.0; 3]);
        let packets = plan_player_changes(&[change], &mut viewer);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].connection, viewer.connection);
        assert!(packets[0].reliable);
        assert_eq!(packets[0].id, id);
        check(body_of(&packets[0]));
    }
}

#[test]
fn ticking_state_and_step_plan_matching_packets() {
    use mc_protocol::packets::play::{ClientboundTickingState, ClientboundTickingStep};
    let mut viewer = viewer_at(7, [0.0; 3]);
    let packets = plan_player_changes(
        &[PlayerChange::TickingState {
            rate: 4.0,
            frozen: true,
        }],
        &mut viewer,
    );
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    assert_eq!(packets[0].id, ClientboundTickingState::ID);
    assert_eq!(
        ClientboundTickingState::decode(&mut &body_of(&packets[0])[..]).unwrap(),
        ClientboundTickingState {
            tick_rate: 4.0,
            frozen: true,
        }
    );

    let mut viewer = viewer_at(7, [0.0; 3]);
    let packets = plan_player_changes(&[PlayerChange::TickingStep { steps: 3 }], &mut viewer);
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    assert_eq!(packets[0].id, ClientboundTickingStep::ID);
    assert_eq!(
        ClientboundTickingStep::decode(&mut &body_of(&packets[0])[..]).unwrap(),
        ClientboundTickingStep { tick_steps: 3 }
    );
}

#[test]
fn boss_bar_changes_plan_matching_boss_operations() {
    use mc_core::boundary::BossBarChange;
    use mc_protocol::packets::play::{BossOperation, ClientboundBossEvent};
    let id = uuid::Uuid::from_u128(0x0102030405060708090a0b0c0d0e0f10);
    let name = || mc_nbt::Tag::String("Boss".into());
    let cases: Vec<(BossBarChange, BossOperation)> = vec![
        (
            BossBarChange::Add {
                name: name(),
                progress: 0.5,
                color: 5,
                overlay: 0,
                flags: 1,
            },
            BossOperation::Add {
                name: name(),
                progress: 0.5,
                color: 5,
                overlay: 0,
                flags: 1,
            },
        ),
        (BossBarChange::Remove, BossOperation::Remove),
        (BossBarChange::Progress(0.25), BossOperation::Progress(0.25)),
        (BossBarChange::Name(name()), BossOperation::Name(name())),
        (
            BossBarChange::Style {
                color: 5,
                overlay: 0,
            },
            BossOperation::Style {
                color: 5,
                overlay: 0,
            },
        ),
        (BossBarChange::Properties(7), BossOperation::Properties(7)),
    ];
    for (change, operation) in cases {
        let mut viewer = viewer_at(7, [0.0; 3]);
        let packets = plan_player_changes(&[PlayerChange::BossBar { id, change }], &mut viewer);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].connection, viewer.connection);
        assert!(packets[0].reliable);
        assert_eq!(packets[0].id, ClientboundBossEvent::ID);
        assert_eq!(
            ClientboundBossEvent::decode(&mut &body_of(&packets[0])[..]).unwrap(),
            ClientboundBossEvent {
                uuid: id,
                operation,
            }
        );
    }
}

#[test]
fn dimension_change_removes_boss_bars_the_client_still_shows() {
    with_cache(|_| {
        let id = ConnectionId(3);
        let mut v = ViewerState::new(id, EntityId(41), [8.0, 64.0, 8.0]);
        let dragon = EntityView {
            id: EntityId(20),
            uuid: uuid::Uuid::from_u128(12),
            type_id: mc_data::entity_types::solaris_required_entity_types()
                .id_of(&mc_data::Identifier::parse("minecraft:ender_dragon").unwrap())
                .unwrap(),
            position: [0.0; 3],
            velocity: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: false,
            appearance: Arc::new(mc_core::boundary::EntityAppearance {
                health: 200.0,
                ..Default::default()
            }),
        };
        let mut shown = Vec::new();
        v.boss_baseline
            .sync(id, std::iter::once(&dragon), &mut shown);
        assert_eq!(shown.len(), 1, "dragon bar is shown");
        let planned = plan_player_changes(
            &[PlayerChange::Respawned {
                teleport_id: 2,
                dimension: mc_core::dimensions::Dimension::End,
                position: [8.0, 80.0, 8.0],
                yaw: 0.0,
                pitch: 0.0,
                game_mode: 0,
            }],
            &mut v,
        );
        let ids: Vec<i32> = planned.iter().map(|p| p.id).collect();
        let remove = ids
            .iter()
            .position(|p| *p == mc_protocol::packets::play::ClientboundBossEvent::ID)
            .expect("boss bar removed");
        let respawn = ids
            .iter()
            .position(|p| *p == mc_protocol::packets::play::ClientboundRespawn::ID)
            .expect("Respawn planned");
        assert!(remove < respawn);
        // The baseline is empty again: nothing further to remove.
        let mut again = Vec::new();
        v.boss_baseline.clear(id, &mut again);
        assert!(again.is_empty());
    });
}

#[test]
fn filled_map_slot_update_is_planned_for_the_client() {
    // s22-04: the map_id component had no wire codec, so the whole slot packet was dropped.
    with_cache(|_| {
        let id = ConnectionId(3);
        let mut v = ViewerState::new(id, EntityId(41), [8.0, 64.0, 8.0]);
        let filled = mc_data::items::solaris_required_items()
            .id_of(&mc_data::Identifier::parse("minecraft:filled_map").unwrap())
            .unwrap();
        let mut stack = mc_data::ItemStack::new(filled, 1);
        let mut value = Vec::new();
        mc_nbt::write_network_value(&mut value, &mc_nbt::Tag::Int(7)).unwrap();
        stack.extended_mut().preserved_components.push(
            mc_data::item_stack::PreservedItemComponent {
                name: "minecraft:map_id".into(),
                value,
                baseline: None,
            },
        );
        let planned = plan_player_changes(
            &[PlayerChange::MenuSlot {
                window: 0,
                state_id: 2,
                slot: 36,
                item: Some(stack),
            }],
            &mut v,
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(
            planned[0].id,
            mc_protocol::packets::play::ClientboundContainerSetSlot::ID
        );
    });
}
