//! Session phases and frame-level translation over synthetic packet tables.

use bytes::{Bytes, BytesMut};
use mc_protocol::{Compression, RawFrame};

use super::tables::{Packets, Reports};
use super::*;

const CANONICAL: &str = r#"{
    "login": {
        "clientbound": {"minecraft:login_disconnect": {"protocol_id": 0}, "minecraft:login_finished": {"protocol_id": 2}},
        "serverbound": {"minecraft:hello": {"protocol_id": 0}, "minecraft:login_acknowledged": {"protocol_id": 3}}
    },
    "configuration": {
        "clientbound": {"minecraft:finish_configuration": {"protocol_id": 3}, "minecraft:post_effects": {"protocol_id": 10}, "minecraft:store_cookie": {"protocol_id": 11}},
        "serverbound": {"minecraft:finish_configuration": {"protocol_id": 3}}
    },
    "play": {
        "clientbound": {"minecraft:block_update": {"protocol_id": 9}, "minecraft:add_transient_block": {"protocol_id": 37}, "minecraft:keep_alive": {"protocol_id": 45}, "minecraft:set_player_inventory": {"protocol_id": 100}},
        "serverbound": {"minecraft:keep_alive": {"protocol_id": 28}, "minecraft:punch": {"protocol_id": 46}, "minecraft:set_creative_mode_slot": {"protocol_id": 57}}
    }
}"#;

const LEGACY: &str = r#"{
    "login": {
        "clientbound": {"minecraft:login_disconnect": {"protocol_id": 0}, "minecraft:login_finished": {"protocol_id": 2}},
        "serverbound": {"minecraft:hello": {"protocol_id": 0}, "minecraft:login_acknowledged": {"protocol_id": 3}}
    },
    "configuration": {
        "clientbound": {"minecraft:finish_configuration": {"protocol_id": 3}, "minecraft:store_cookie": {"protocol_id": 10}},
        "serverbound": {"minecraft:finish_configuration": {"protocol_id": 3}}
    },
    "play": {
        "clientbound": {"minecraft:block_update": {"protocol_id": 9}, "minecraft:keep_alive": {"protocol_id": 44}, "minecraft:set_player_inventory": {"protocol_id": 99}},
        "serverbound": {"minecraft:keep_alive": {"protocol_id": 27}, "minecraft:swing": {"protocol_id": 63}, "minecraft:set_creative_mode_slot": {"protocol_id": 98}}
    }
}"#;

fn session() -> Session {
    let canonical = Reports::parse(CANONICAL, "{}", "{}").unwrap();
    let legacy = Reports::parse(LEGACY, "{}", "{}").unwrap();
    test_session(Tables {
        packets: Packets::new(&canonical, &legacy),
        ..Tables::default()
    })
}

fn frame(id: i32, body: &[u8]) -> RawFrame {
    RawFrame {
        id,
        body: Bytes::copy_from_slice(body),
    }
}

#[test]
fn legacy_creative_echo_keeps_unknown_item_but_real_changes_pass() {
    use super::tables::IdMap;
    use mc_protocol::codec::WriteMc;

    let mut writer = session();
    Arc::get_mut(&mut writer.profile).unwrap().tables.items =
        IdMap::by_name([("stone", 10), ("new_item", 11)], [("stone", 1)], |name| {
            (name == "new_item").then(|| "stone".to_owned())
        });
    writer.phase = Phase::Play;
    let mut reader = writer.split();
    let mut sent = Vec::new();
    sent.write_varint(0);
    for value in [1, 11, 0, 0] {
        sent.write_varint(value);
    }
    let (_, legacy) = writer.clientbound(100, &sent).unwrap().unwrap();
    let mut echo = Vec::new();
    echo.write_i16(36);
    echo.extend_from_slice(&legacy[1..]);
    assert!(
        reader.serverbound(frame(98, &echo)).unwrap().is_none(),
        "stand-in echo must not replace the real item"
    );
    let mut changed = Vec::new();
    changed.write_i16(37);
    for value in [1, 1, 0, 0] {
        changed.write_varint(value);
    }
    let translated = reader.serverbound(frame(98, &changed)).unwrap().unwrap();
    assert_eq!(&translated.body[..], &[0, 37, 1, 10, 0, 0]);
}

#[test]
fn legacy_creative_echo_and_replacement_reach_player_inventory() {
    use super::tables::IdMap;
    use mc_core::boundary::{ConnectionId, PlayerInput};
    use mc_core::commands::GameMode;
    use mc_core::inventory::ItemRules;
    use mc_core::player::PlayerState;
    use mc_data::{Identifier, ItemStack};
    use mc_protocol::codec::WriteMc;

    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = mc_data::tags::TagsData::default();
    let rules = ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let stone = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let unknown = items
        .id_of(&Identifier::parse("minecraft:wooden_sword").unwrap())
        .unwrap();
    let mut writer = session();
    Arc::get_mut(&mut writer.profile).unwrap().tables.items = IdMap::by_name(
        [("stone", stone), ("new_item", unknown)],
        [("stone", 1)],
        |name| (name == "new_item").then(|| "stone".to_owned()),
    );
    writer.phase = Phase::Play;
    let mut reader = writer.split();
    let mut player = PlayerState::new(ConnectionId(1), GameMode::Creative);
    player.inventory.slots[0] = ItemStack::new(unknown, 1);
    let mut sent = Vec::new();
    sent.write_varint(0);
    for value in [1, unknown as i32, 0, 0] {
        sent.write_varint(value);
    }
    let (_, legacy) = writer.clientbound(100, &sent).unwrap().unwrap();
    let mut echo = Vec::new();
    echo.write_i16(36);
    echo.extend_from_slice(&legacy[1..]);
    let mut decode = crate::decode::DecodeState::default();
    let mut inputs = Vec::new();
    if let Some(frame) = reader.serverbound(frame(98, &echo)).unwrap() {
        crate::decode::decode_play_frame(
            ConnectionId(1),
            frame.id,
            &frame.body,
            &mut decode,
            &mut inputs,
        )
        .unwrap();
    }
    for input in inputs.drain(..) {
        if let PlayerInput::CreativeSlot { slot, stack, .. } = input {
            player.creative_slot(&rules, slot, stack);
        }
    }
    assert_eq!(player.inventory.slots[0], ItemStack::new(unknown, 1));
    let mut replacement = Vec::new();
    replacement.write_i16(36);
    for value in [2, 1, 0, 0] {
        replacement.write_varint(value);
    }
    let frame = reader
        .serverbound(frame(98, &replacement))
        .unwrap()
        .unwrap();
    crate::decode::decode_play_frame(
        ConnectionId(1),
        frame.id,
        &frame.body,
        &mut decode,
        &mut inputs,
    )
    .unwrap();
    for input in inputs {
        if let PlayerInput::CreativeSlot { slot, stack, .. } = input {
            player.creative_slot(&rules, slot, stack);
        }
    }
    assert_eq!(player.inventory.slots[0], ItemStack::new(stone, 2));
}

#[test]
fn phases_follow_the_client_acknowledgements() {
    let mut s = session();
    // Login: the session UUID 26.3 appends to the profile is cut.
    let profile = [1u8; 20];
    let (id, body) = s.clientbound(2, &profile).unwrap().unwrap();
    assert_eq!((id, body.len()), (2, 4));
    assert!(s.serverbound(frame(3, &[])).unwrap().is_some());
    // Configuration: post_effects is dropped, later ids shift back by one.
    assert!(s.clientbound(10, &[0]).unwrap().is_none());
    assert_eq!(s.clientbound(11, &[7]).unwrap().unwrap().0, 10);
    assert_eq!(s.serverbound(frame(3, &[])).unwrap().unwrap().id, 3);
    // Play.
    assert_eq!(s.clientbound(45, &[0; 8]).unwrap().unwrap().0, 44);
    assert!(s.clientbound(37, &[0; 9]).unwrap().is_none());
    assert_eq!(s.serverbound(frame(27, &[0; 8])).unwrap().unwrap().id, 28);
    assert_eq!(
        s.serverbound(frame(63, &[0])).unwrap().unwrap().id,
        46,
        "legacy swings become the canonical empty punch"
    );
    assert!(
        s.serverbound(frame(99, &[])).is_err(),
        "unknown ids fail closed"
    );
}

#[test]
fn pre_framed_batches_are_reframed_with_compression() {
    let mut s = session();
    s.phase = Phase::Play;
    let compression = Compression::Threshold(16);
    let mut batch = BytesMut::new();
    mc_protocol::frame::append_frame(&mut batch, 45, &[1; 8], compression).unwrap();
    mc_protocol::frame::append_frame(&mut batch, 37, &[2; 40], compression).unwrap();
    mc_protocol::frame::append_frame(&mut batch, 45, &[3; 64], compression).unwrap();
    let mut out = s.clientbound_frames(&batch, compression).unwrap();
    let first = mc_protocol::frame::try_decode_frame(&mut out, compression)
        .unwrap()
        .unwrap();
    assert_eq!((first.id, &first.body[..]), (44, &[1u8; 8][..]));
    let second = mc_protocol::frame::try_decode_frame(&mut out, compression)
        .unwrap()
        .unwrap();
    assert_eq!((second.id, &second.body[..]), (44, &[3u8; 64][..]));
    assert!(out.is_empty(), "the 26.3-only frame is gone");
}

#[test]
fn malformed_world_updates_fail_instead_of_being_silently_lost() {
    let mut s = session();
    s.phase = Phase::Play;
    assert!(s.clientbound(9, &[]).is_err());
}

#[test]
fn legacy_cache_sits_beside_the_canonical_one() {
    if std::env::var_os(CACHE_ENV).is_some() {
        return;
    }
    let release = Path::new("/srv/content").join(mc_protocol::TARGET_RELEASE);
    assert_eq!(cache_root(&release), Path::new("/srv/content/26.1.2"));
    assert_eq!(
        cache_root(Path::new("data/vanilla")),
        Path::new("data/vanilla-26.1.2")
    );
}
