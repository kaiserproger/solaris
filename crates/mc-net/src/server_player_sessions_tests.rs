//! Behaviour of the live-session lookup a component host holds.
//!
//! Every case here goes through a bound server's own registry: the identity the
//! handle is asked with is the identity the registry advertises for a connected
//! player, and the session it must answer is the session that same snapshot
//! reports. Nothing is asserted about a setter echoing a getter.

use std::num::NonZeroUsize;

use super::*;

use crate::login::offline_uuid;

fn bound_test_config() -> ServerConfig {
    ServerConfig {
        tab_list: TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "player-sessions-lookup-test".into(),
        max_players: 8,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(LootTables::default()),
        block_light: None,
        items: Arc::new(ItemRegistry::default()),
        item_facts: Arc::new(ItemFactsTable::default()),
        block_facts: Arc::new(BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy::default(),
        random_tick: play::RandomTickPolicy::default(),
        command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
        loader_manifest: None,
        shutdown: ShutdownHandle::default(),
    }
}

/// The one advertised identity and the one session it holds, for a server whose
/// registry reports exactly one connected player.
fn only_online_player(server: &BoundServer) -> (String, u64) {
    let (players, _) = server
        .sessions
        .script_online_players(8)
        .expect("online player snapshot");
    assert_eq!(players.len(), 1, "exactly one player is connected");
    (
        players[0].context().uuid().to_owned(),
        players[0].player_id().value(),
    )
}

async fn bind_test_server() -> BoundServer {
    let (boundary, _endpoint) = mc_script::script_boundary_pair(
        NonZeroUsize::new(2).unwrap(),
        NonZeroUsize::new(2).unwrap(),
    );
    bind_with_scripts(bound_test_config(), boundary)
        .await
        .expect("bind test server")
}

#[tokio::test]
async fn player_sessions_handle_answers_the_published_servers_live_sessions() {
    let server = bind_test_server().await;
    let handle = PlayerSessionsHandle::new();
    let alice_identity = offline_uuid("Alice").to_string();
    let offline_identity = offline_uuid("Bob").to_string();

    // Nothing published yet, and nothing connected once published: an empty
    // handle and an empty server both answer nobody.
    assert_eq!(handle.session_of(&alice_identity), None);
    server.register_player_sessions(&handle);
    assert_eq!(handle.session_of(&alice_identity), None);

    let (_alice, connection) = server.sessions.register_joined_player_for_test("Alice");
    let (identity, session) = only_online_player(&server);
    assert_eq!(
        identity, alice_identity,
        "the registry advertises the join's identity"
    );

    // The identity the registry advertises answers the session the registry
    // reports, and nothing else answers anything.
    assert_eq!(handle.session_of(&identity), Some(session));
    assert_eq!(handle.session_of(&offline_identity), None);
    assert_eq!(handle.session_of("Alice"), None);
    assert_eq!(handle.session_of(""), None);

    // The connection ends: the same identity is answered as absent, not with
    // the session it used to hold.
    drop(connection);
    assert_eq!(handle.session_of(&identity), None);
    assert!(
        server
            .sessions
            .script_online_players(8)
            .expect("online player snapshot")
            .0
            .is_empty(),
        "the registry no longer reports the player"
    );
}

#[tokio::test]
async fn player_sessions_handle_follows_the_server_that_published_last() {
    let handle = PlayerSessionsHandle::new();

    let first = bind_test_server().await;
    first.register_player_sessions(&handle);
    let (_alice, _alice_connection) = first.sessions.register_joined_player_for_test("Alice");
    let (alice_identity, alice_session) = only_online_player(&first);
    assert_eq!(handle.session_of(&alice_identity), Some(alice_session));

    // A second server binds and publishes into the same handle.
    let second = bind_test_server().await;
    second.register_player_sessions(&handle);
    let (_bob, _bob_connection) = second.sessions.register_joined_player_for_test("Bob");
    let (bob_identity, bob_session) = only_online_player(&second);

    // The handle answers the new server's live sessions ...
    assert_eq!(handle.session_of(&bob_identity), Some(bob_session));
    // ... and no longer the old one's, even though that player is still
    // connected to the server it was published from.
    let (still_online, _) = first
        .sessions
        .script_online_players(8)
        .expect("online player snapshot");
    assert_eq!(still_online.len(), 1);
    assert_eq!(still_online[0].context().uuid(), alice_identity);
    assert_eq!(handle.session_of(&alice_identity), None);
}
