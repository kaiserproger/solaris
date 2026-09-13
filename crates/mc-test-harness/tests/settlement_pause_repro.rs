//! End-to-end regression for the settlement fund step of the M94-09 chain.
//!
//! Drives the shipped `solaris-settlements` package over a real headless server
//! exactly like the real-client scenario does (create -> site -> adopt ->
//! teleport -> survey -> project -> give -> fund) and asserts the funding step
//! answers the player. A silent fund is the defect: the command must produce
//! either `Reserved real materials for <building> (` or a typed refusal.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundSystemChat, ConfirmTeleportation, ServerboundChatCommand,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const NAME: &str = "fundrepro";
const BLUEPRINT: &str = "solaris:house_small";
const PLAYER: &str = "FundRepro";
const MATERIALS: &[(&str, i32)] = &[
    ("minecraft:oak_planks", 255),
    ("minecraft:oak_planks", 59),
    ("minecraft:oak_log", 16),
    ("minecraft:glass", 10),
    ("minecraft:red_bed", 10),
    ("minecraft:torch", 2),
    ("minecraft:oak_door", 2),
    ("minecraft:crafting_table", 1),
    ("minecraft:chest", 1),
];

#[tokio::test]
async fn settlement_fund_reserves_materials_and_answers_the_player() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    copy_settlement_package(plugins.path());
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 1);

    let world_dir = tempfile::tempdir().expect("world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("create world region");
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(81, Arc::clone(&blocks)));
    let world =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 49)
            .expect("open world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "settlement fund repro".into(),
        max_players: 1,
        view_distance: 10,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &block_report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new([PLAYER], false),
        loader_manifest: None,
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, PLAYER)
        .await
        .expect("login succeeds");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    command_and_wait(
        &mut client,
        &format!("settlement create {NAME} small"),
        &format!("Founded {NAME} (small hamlet)."),
    )
    .await;

    let site_line =
        command_and_wait_any(&mut client, &format!("settlement site {NAME}"), "origin ").await;
    let (site_id, origin_x, origin_z) = parse_site(&site_line);

    command_and_wait(
        &mut client,
        &format!("settlement adopt {NAME} {site_id}"),
        &format!("Adopted {site_id}"),
    )
    .await;

    command_and_wait(
        &mut client,
        &format!("tp {origin_x} 250 {origin_z}"),
        &format!("Teleported to {origin_x} 250 {origin_z}"),
    )
    .await;

    // The server only holds the survey footprint once it streams those chunks
    // to this client, so wait on the real chunk packets instead of a timer.
    wait_for_chunks(&mut client, origin_x, origin_z).await;

    let survey = command_and_wait_any(
        &mut client,
        &format!("settlement survey {NAME} plot"),
        "Survey settlement: ",
    )
    .await;
    assert!(
        survey.contains("chunks=loaded"),
        "plot survey did not run on loaded chunks: {survey:?}"
    );

    let projected = command_and_wait_any(
        &mut client,
        &format!("settlement project {NAME} {BLUEPRINT} here"),
        " projected (",
    )
    .await;
    let building = projected
        .split_whitespace()
        .next()
        .expect("projected building name")
        .to_owned();
    assert!(
        projected.contains(BLUEPRINT),
        "unexpected projection: {projected:?}"
    );

    for (item, count) in MATERIALS {
        command_and_wait(
            &mut client,
            &format!("give {item} {count}"),
            &format!("Gave {count} of {item}"),
        )
        .await;
    }

    let reserved = command_and_wait_any(
        &mut client,
        &format!("settlement fund {NAME} {building}"),
        &format!("Reserved real materials for {building} ("),
    )
    .await;

    let built = command_and_wait_any(
        &mut client,
        &format!("settlement build {NAME} {building}"),
        " committed (",
    )
    .await;
    assert!(
        built.contains(BLUEPRINT),
        "build did not commit {BLUEPRINT}: {built:?}"
    );
    drop(reserved);

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown timeout")
        .expect("server task")
        .expect("server result");
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

/// Wait until the 64x64 plot footprint centred on the player is resident on the
/// client: the same chunk stream the server surveys is what it streams here.
async fn wait_for_chunks(client: &mut Client, origin_x: i32, origin_z: i32) {
    let first = ((origin_x - 32) >> 4, (origin_z - 32) >> 4);
    let last = ((origin_x + 31) >> 4, (origin_z + 31) >> 4);
    let mut wanted = Vec::new();
    for x in first.0..=last.0 {
        for z in first.1..=last.1 {
            wanted.push((x, z));
        }
    }
    let total = wanted.len();
    tokio::time::timeout(Duration::from_secs(60), async {
        while !wanted.is_empty() {
            let mut frame = client.read_frame().await.expect("chunk stream frame");
            if frame.id == SynchronizePlayerPosition::ID {
                let sync = SynchronizePlayerPosition::decode(&mut frame.body).expect("sync decode");
                let _ = client
                    .write_packet(&ConfirmTeleportation {
                        teleport_id: sync.teleport_id,
                    })
                    .await;
            } else if frame.id == mc_protocol::packets::play::LevelChunkWithLight::ID {
                let chunk =
                    mc_protocol::packets::play::LevelChunkWithLight::decode(&mut frame.body)
                        .expect("level chunk decode");
                wanted.retain(|cell| *cell != (chunk.chunk_x, chunk.chunk_z));
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "only {} of {total} plot chunks streamed",
            total - wanted.len()
        )
    });
}

fn parse_site(line: &str) -> (String, i32, i32) {
    let mut parts = line.split_whitespace();
    let site_id = parts.next().expect("site id").to_owned();
    let _variant = parts.next().expect("site variant");
    assert_eq!(parts.next(), Some("origin"), "site line carries an origin");
    let origin = parts.next().expect("site origin");
    let mut axes = origin.split(',');
    let x = axes.next().unwrap().parse().expect("origin x");
    let _y = axes.next().unwrap().parse::<i32>().expect("origin y");
    let z = axes.next().unwrap().parse().expect("origin z");
    (site_id, x, z)
}

async fn command_and_wait(client: &mut Client, command: &str, expected: &str) {
    command_and_wait_any(client, command, expected).await;
}

/// Issue one player command and return the chat line containing `needle`.
async fn command_and_wait_any(client: &mut Client, command: &str, needle: &str) -> String {
    let before = drain_chat(client).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: command.to_owned(),
        })
        .await
        .unwrap_or_else(|error| panic!("send /{command}: {error}"));
    let mut seen = Vec::new();
    let mut ids: Vec<i32> = Vec::new();
    let line = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let mut frame = client.read_frame().await.expect("frame during command");
            if ids.len() < 40 {
                ids.push(frame.id);
            }
            if frame.id == SynchronizePlayerPosition::ID {
                let sync = SynchronizePlayerPosition::decode(&mut frame.body).expect("sync decode");
                let _ = client
                    .write_packet(&ConfirmTeleportation {
                        teleport_id: sync.teleport_id,
                    })
                    .await;
            } else if frame.id == ClientboundSystemChat::ID {
                let chat = ClientboundSystemChat::decode(&mut frame.body).expect("chat decode");
                let message = literal_text(&chat.content_nbt);
                if seen.len() < 24 {
                    seen.push(message.clone());
                }
                if message.contains(needle) && !before.contains(&message) {
                    return message;
                }
            }
        }
    })
    .await;
    line.unwrap_or_else(|_| {
        panic!(
            "/{command} did not produce a line containing {needle:?}; new chat: {seen:?}; frame ids: {ids:x?}"
        )
    })
}

/// Read whatever chat is already queued without blocking (best-effort baseline).
async fn drain_chat(client: &mut Client) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let Ok(frame) = client
            .read_frame_with_timeout(Duration::from_millis(1))
            .await
        else {
            break;
        };
        if frame.id == ClientboundSystemChat::ID {
            let mut frame = frame;
            if let Ok(chat) = ClientboundSystemChat::decode(&mut frame.body) {
                lines.push(literal_text(&chat.content_nbt));
            }
        }
    }
    lines
}

fn literal_text(component: &[u8]) -> String {
    let mut bytes = Bytes::copy_from_slice(component);
    let tag = mc_nbt::read_network(&mut bytes).expect("read text component nbt");
    let mc_nbt::Tag::Compound(fields) = tag else {
        panic!("text component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("literal text component")
}

fn copy_settlement_package(root: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("../solaris-default-plugins/solaris-settlements");
    let source = source
        .canonicalize()
        .unwrap_or_else(|error| panic!("settlement package at {}: {error}", source.display()));
    let target = root.join("solaris-settlements");
    copy_dir(&source, &target);
}

fn copy_dir(source: &Path, target: &Path) {
    std::fs::create_dir_all(target).expect("create plugin directory");
    for entry in std::fs::read_dir(source).expect("read plugin directory") {
        let entry = entry.expect("plugin directory entry");
        let file_type = entry.file_type().expect("plugin entry type");
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&entry.path(), &destination);
        } else {
            std::fs::copy(entry.path(), destination).expect("copy plugin file");
        }
    }
}
