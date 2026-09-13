use super::*;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{ClientboundTabList, PlayerInfoActions, PlayerInfoUpdate};
use tokio::sync::mpsc;

fn literal_text(nbt: &[u8]) -> String {
    let mut cursor: &[u8] = nbt;
    let tag = mc_nbt::read_network(&mut cursor).expect("tab list component is network NBT");
    assert!(
        cursor.is_empty(),
        "tab list component consumes its whole NBT blob"
    );
    let mc_nbt::Tag::Compound(fields) = tag else {
        panic!("tab list component is a compound");
    };
    assert_eq!(fields.len(), 1, "tab list component is a literal");
    let (name, value) = &fields[0];
    assert_eq!(name, "text");
    let mc_nbt::Tag::String(text) = value else {
        panic!("tab list component holds literal text");
    };
    text.clone()
}

async fn login_frames(
    config: &ServerConfig,
    sessions: &Arc<SessionRegistry>,
    name: &str,
) -> Vec<(i32, Vec<u8>)> {
    let (simulation, _owner) = simulation_channel();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let mut reader = tokio::io::empty();
    let mut writer = Vec::new();
    let mut buf = BytesMut::new();
    let result = handle(
        &mut reader,
        &mut writer,
        &mut buf,
        Compression::Disabled,
        &profile,
        &[],
        CommandPermissions { op: false },
        config,
        crate::server::ConnectionWorld::default(),
        Arc::clone(sessions),
        ChunkPipelineResources::with_limits(1, 1),
        None,
        None,
        simulation,
        Vec::new(),
        None,
        None,
        None,
    )
    .await;
    assert!(matches!(result, Err(ConnectionError::Eof)));
    let mut frames = bytes::BytesMut::from(writer.as_slice());
    let mut out = Vec::new();
    while let Some(frame) =
        mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled).unwrap()
    {
        out.push((frame.id, frame.body.to_vec()));
    }
    out
}

fn decode_roster(body: &[u8]) -> PlayerInfoUpdate {
    let mut cursor: &[u8] = body;
    let update = PlayerInfoUpdate::decode(&mut cursor).expect("player info update decodes");
    assert!(cursor.is_empty(), "roster consumes its whole frame");
    update
}

#[tokio::test]
async fn login_sends_tab_list_header_footer_and_full_roster() {
    let mut config = play_loop_slow_client_test_config();
    config.max_players = 8;
    config.tab_list.header = "Welcome\nSolaris".to_owned();
    config.tab_list.footer = "play.example.com".to_owned();
    let sessions = Arc::new(SessionRegistry::new());

    let (peer_tx, mut peer_rx) = mpsc::channel(64);
    let peer_profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("TabPeer"),
        name: "TabPeer".to_owned(),
    };
    sessions
        .try_register(SessionRegistration {
            profile: &peer_profile,
            properties: &[],
            center: (0, 0),
            view_distance: 0,
            desired: HashSet::from([(0, 0)]),
            tx: peer_tx,
            pose: PlayerPose::new(0.5, 64.0, 0.5),
            game_mode: GameMode::Creative,
            max_sessions: 8,
            script_operator: false,
            dimension: "minecraft:overworld",
            loader_session: None,
        })
        .expect("peer registers");

    let frames = login_frames(&config, &sessions, "TabNewcomer").await;

    let roster_pos = frames
        .iter()
        .position(|(id, _)| *id == PlayerInfoUpdate::ID)
        .expect("login burst carries the online roster");
    let roster = decode_roster(&frames[roster_pos].1);
    assert_eq!(roster.actions, PlayerInfoActions::minimal_add_player());
    assert_eq!(roster.entries.len(), 2);
    let peer_entry = roster
        .entries
        .iter()
        .find(|entry| entry.name == "TabPeer")
        .expect("roster carries the already-online peer");
    assert_eq!(peer_entry.game_mode, GameMode::Creative.id());
    assert!(peer_entry.listed);
    let self_entry = roster
        .entries
        .iter()
        .find(|entry| entry.name == "TabNewcomer")
        .expect("roster carries the newcomer");
    assert_eq!(self_entry.game_mode, GameMode::Survival.id());
    assert_eq!(
        self_entry.profile_id,
        crate::login::offline_uuid("TabNewcomer")
    );

    let tab_list_pos = frames
        .iter()
        .position(|(id, _)| *id == ClientboundTabList::ID)
        .expect("login burst carries the configured header/footer");
    assert!(
        roster_pos < tab_list_pos,
        "roster precedes the header/footer packet"
    );
    let mut cursor: &[u8] = &frames[tab_list_pos].1;
    let tab_list = ClientboundTabList::decode(&mut cursor).expect("tab list packet decodes");
    assert!(cursor.is_empty(), "tab list consumes its whole frame");
    assert_eq!(literal_text(&tab_list.header_nbt), "Welcome\nSolaris");
    assert_eq!(literal_text(&tab_list.footer_nbt), "play.example.com");

    let join_notice = peer_rx.try_recv().expect("peer is told about the joiner");
    let OutboundCommand::PlayerInfo(join) = join_notice else {
        panic!("peer's notice is the joiner's player info");
    };
    assert_eq!(join.entries.len(), 1);
    assert_eq!(join.entries[0].name, "TabNewcomer");

    let OutboundCommand::PlayerInfoRemove(remove) =
        peer_rx.try_recv().expect("peer is told about the leaver")
    else {
        panic!("peer's next notice is the removal");
    };
    assert_eq!(
        remove.profile_ids,
        vec![crate::login::offline_uuid("TabNewcomer")]
    );

    let mut default_config = play_loop_slow_client_test_config();
    default_config.max_players = 8;
    let default_frames = login_frames(
        &default_config,
        &Arc::new(SessionRegistry::new()),
        "TabSolo",
    )
    .await;
    assert!(
        default_frames
            .iter()
            .all(|(id, _)| *id != ClientboundTabList::ID),
        "empty header/footer sends no tab list packet (vanilla default)"
    );
    let solo_roster = default_frames
        .iter()
        .find(|(id, _)| *id == PlayerInfoUpdate::ID)
        .expect("even the default burst carries the roster");
    assert_eq!(decode_roster(&solo_roster.1).entries.len(), 1);
}
