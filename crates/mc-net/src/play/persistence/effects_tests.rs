use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mc_data::items::ItemRegistry;
use mc_nbt::{ListTag, Tag, tag_type};
use mc_protocol::codec::ReadMc;
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::ClientboundUpdateEntityEffect;

use crate::login::{LoggedInProfile, offline_uuid};
use crate::play::session::SessionRegistry;
use crate::play::simulation::SimulationAuthority;
use crate::play::{OutboundCommand, PlayerPose, dispatch_visibility_commands};

use super::super::{PlayerPersistedState, load_player_state, save_player_state};

#[tokio::test]
async fn saved_unsorted_effects_restore_hidden_behaviour_and_notify_owner_and_tracker() {
    let effects = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:saturation".into())),
                ("duration".into(), Tag::Int(1)),
            ]),
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:regeneration".into())),
                ("amplifier".into(), Tag::Int(1)),
                ("duration".into(), Tag::Int(1)),
                (
                    "hidden_effect".into(),
                    Tag::Compound(vec![
                        ("duration".into(), Tag::Int(101)),
                        ("show_particles".into(), Tag::Byte(0)),
                        ("show_icon".into(), Tag::Byte(0)),
                    ]),
                ),
            ]),
        ],
    });
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let mut player = PlayerPersistedState::new_default(pose);
    player.survival.health = 10.0;
    player.survival.food = 1;
    player.survival.saturation = 0.0;
    player.effects = super::load(&effects).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let items = ItemRegistry::from_report(&[]);
    let uuid = offline_uuid("EffectOwner");
    save_player_state(directory.path(), uuid, &items, &player).unwrap();
    let loaded = load_player_state(
        directory.path(),
        uuid,
        &items,
        PlayerPersistedState::new_default(pose),
    )
    .unwrap()
    .unwrap();

    let registry = SessionRegistry::new();
    let mut peers = Vec::new();
    for name in ["EffectOwner", "EffectObserver"] {
        let profile = LoggedInProfile {
            uuid: offline_uuid(name),
            name: name.into(),
        };
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        let (id, initial) =
            registry.register(&profile, (0, 0), 0, HashSet::from([(0, 0)]), tx, pose);
        dispatch_visibility_commands(initial);
        peers.push((id, rx));
    }
    let shared = Arc::new(Mutex::new(loaded));
    registry.register_player_persistence(peers[0].0, Arc::clone(&shared));
    for (id, _) in &peers {
        dispatch_visibility_commands(registry.mark_loaded(*id, (0, 0)));
    }
    let owner_id = peers[0].0;
    dispatch_visibility_commands(registry.broadcast_player_effects(owner_id));
    for tick in 1..=2 {
        dispatch_visibility_commands(
            registry.tick_player_effects_owned(&SimulationAuthority::for_test(), tick),
        );
    }
    let survival = shared.lock().unwrap().survival;
    assert_eq!(survival.health, 11.0);
    assert_eq!(survival.food, 2);
    assert_eq!(survival.saturation, 2.0);

    for (_, receiver) in &mut peers {
        let packets = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            let mut packets = Vec::new();
            while packets.len() < 3 {
                let command = receiver
                    .recv()
                    .await
                    .expect("effect recipient remains connected");
                if !matches!(command, OutboundCommand::ApplyPlayerEffect { .. }) {
                    continue;
                }
                let mut wire = Vec::new();
                crate::play::send_player_effect_command(&mut wire, Compression::Disabled, command)
                    .await
                    .unwrap();
                packets.push(decode_effect_packet(&wire));
            }
            packets
        })
        .await
        .expect("loaded and restored effects reach their owner and tracker");
        assert_eq!(
            packets
                .iter()
                .map(|packet| packet.effect_id.raw())
                .collect::<Vec<_>>(),
            [22, 9, 9]
        );
        let restored = packets.last().unwrap();
        assert_eq!(restored.amplifier, 0);
        assert_eq!(restored.duration_ticks, 100);
        assert!(!restored.flags.visible);
        assert!(!restored.flags.show_icon);
    }

    let profile = LoggedInProfile {
        uuid: offline_uuid("LateObserver"),
        name: "LateObserver".into(),
    };
    let (tx, mut receiver) = tokio::sync::mpsc::channel(32);
    let (id, initial) = registry.register(&profile, (0, 0), 0, HashSet::from([(0, 0)]), tx, pose);
    dispatch_visibility_commands(initial);
    dispatch_visibility_commands(registry.mark_loaded(id, (0, 0)));
    let player = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match receiver
                .recv()
                .await
                .expect("late observer remains connected")
            {
                OutboundCommand::SpawnPlayer(player) if player.session_id == owner_id => {
                    break player;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("late observer sees the affected player");
    let mut wire = Vec::new();
    crate::play::wire_entities::send_player_spawn(
        &mut wire,
        Compression::Disabled,
        &player,
        &registry.player_effect_snapshot(owner_id),
    )
    .await
    .unwrap();
    let packet = decode_effect_packet(&wire);
    assert_eq!(packet.effect_id.raw(), 9);
    assert_eq!(packet.duration_ticks, 99);
    assert!(!packet.flags.visible);
    assert!(!packet.flags.show_icon);
}

fn decode_effect_packet(mut frames: &[u8]) -> ClientboundUpdateEntityEffect {
    while !frames.is_empty() {
        let length = usize::try_from(frames.read_varint().unwrap()).unwrap();
        let (mut packet, remaining) = frames.split_at(length);
        frames = remaining;
        if packet.read_varint().unwrap() == ClientboundUpdateEntityEffect::ID {
            return ClientboundUpdateEntityEffect::decode(&mut packet).unwrap();
        }
    }
    panic!("expected an effect update in player wire output");
}
