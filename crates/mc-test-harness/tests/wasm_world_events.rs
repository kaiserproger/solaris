//! P3 world-observation acceptance through one real component and one real client.
//!
//! The component is the repository's own example plugin, built to
//! `wasm32-unknown-unknown` and encoded into a component exactly the way a
//! published package is; the world is the harness's own flat arena over real
//! embedded data. A real client logs in over the wire and performs the actions the
//! observations are about - interacting with a summoned entity, placing a block
//! against the arena floor, breaking that block after survival ticks, picking its
//! drop up, crafting from given ingredients, killing a summoned entity with a
//! lethal blow, and dying of committed damage - and the audit fixture reports what
//! the host delivered to it: the contract's own records, with the actor, the
//! session, the dimension, the kind's detail, and the tick the batch was stamped
//! with.
//!
//! Nothing here manufactures a script event. Every observation asserted below is
//! published by the owner that committed the action, and the assertions are
//! written against values this test named itself (the block it placed against the
//! arena floor beside the column the server placed the player in, the item the
//! craft produced, the entity type it summoned, the pose it reported last), so a
//! component handed a rendered-but-empty record, a different session or a stale
//! session would fail rather than pass. Every pose this run acts from is the
//! arena's own stand pose - the first air cell above the fixture's floor, in the
//! column the server placed the player in - reported by this client itself, so no
//! action here depends on the pose the server hands out on entry as its own spawn
//! default.
//!
//! The same run deploys a second real component under its own id: the same guest,
//! subscribed to none of these observations, answering its own command root as a
//! liveness fence. Its silence about the run's observations - in the lines it
//! would have sent to the player and in the diagnostics it logs - is what proves
//! delivery reaches the packages that asked for it and no others.
//!
//! The tick an observation carries is the batch's own `EventContext`, which is the
//! server's pushed simulation tick. The contract has no `server.tick` event on
//! purpose, so the history's stamps are read back from the fixture's report and
//! asserted to be nonzero and non-decreasing, never guessed from a delivery count.

// Reuse the host tests' component builder.
#[path = "../../mc-plugin-host/tests/fixture/mod.rs"]
mod fixture;

// The harness's flat arena over real generated terrain.
#[path = "support/combat_world.rs"]
mod combat_world;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, HostServices, LogLevel, PlayerSessions,
    PluginHost, PluginLimits, discover, start_deployment_with,
};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientCommandAction, ClientboundCommands, ClientboundContainerSetSlot,
    ClientboundKeepAlive, ClientboundRespawn, ClientboundSystemChat, ConfirmTeleportation,
    Direction, EntityVec3, InteractionHand, LevelChunkWithLight, MovePlayerFlags, PlayerActionKind,
    ServerboundAttack, ServerboundChatCommand, ServerboundClientCommand, ServerboundInteract,
    ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundPlaceRecipe,
    ServerboundPlayerAction, ServerboundPlayerLoaded, ServerboundUseItemOn,
    SynchronizePlayerPosition, pack_block_pos,
};
use mc_test_harness::client::Client;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;

/// The package that subscribes to the observations, and the one that subscribes to
/// none of them.
const RECORDER: &str = "audit-recorder";
const STRANGER: &str = "audit-stranger";
/// The one player this run drives.
const PLAYER: &str = "AuditGuest";

/// The recorder's command root and the argument that asks for its history.
const AUDIT_ROOT: &str = "audit";
/// The stranger's own root: the fence that proves it is live while it stays silent.
const FENCE_ROOT: &str = "audit-fence";

/// The one line both guests log at startup.
const READY: &str = "P3_AUDIT ready";
/// What the recorder reports as each observation arrives, what its history holds,
/// and what the history's own summary starts with.
const OBSERVED: &str = "P3_AUDIT observe ";
const RECORDED: &str = "P3_AUDIT record ";
const REPORTED: &str = "P3_AUDIT report ";
/// What the stranger would publish if a world observation reached it.
const LEAK: &str = "P3_AUDIT LEAK";

/// The arena's own floor: the support world's stone slab, with everything above it
/// cleared, so the first air cell above the slab is where a player stands and every
/// position this run names is reachable air.
const FLOOR_Y: i32 = 199;
/// The top of the arena's cleared space: the pose the server hands out on entry
/// must land inside it, and it is what tells the fixture's arena from anywhere
/// else.
const ARENA_TOP_Y: i32 = 216;
/// The first air cell above the arena floor. A stand pose, reported by this client
/// itself, is built from this height and the column the server placed the player
/// in, so no action of this run depends on the pose the server hands out on entry.
const STAND_Y: f64 = (FLOOR_Y + 1) as f64;

const VIEW_DISTANCE: i32 = 2;

/// How long any one wait may take before the run is called stalled. Nothing here
/// sleeps: every wait ends on a frame the server published or on the guest's own
/// line.
const WIRE_TIMEOUT: Duration = Duration::from_secs(15);
/// Survival ticks a placed block's break takes before the stop packet commits it,
/// and the recharge a melee attack needs before it lands again.
const BREAK_TICKS: u64 = 45;
const ATTACK_TICKS: u64 = 20;

/// The recorder's manifest: every observation of the matrix row set, and the audit
/// root its history request arrives on.
const RECORDER_MANIFEST: &str = r#"
id = "audit-recorder"
name = "Audit Recorder"
version = "0.1.0"
api = "0.7.0"
events = ["player.block_broken", "player.block_placed", "player.item_crafted", "player.item_picked_up", "player.entity_killed", "player.entity_interacted", "player.died"]
player_commands = ["audit"]
"#;

/// What tells the recorder's instance which role to bind.
const RECORDER_CONFIG: &str = "mode = \"audit-events\"\naudit_role = \"recorder\"\n";

/// The stranger's manifest: no subscription at all, and its own command root.
const STRANGER_MANIFEST: &str = r#"
id = "audit-stranger"
name = "Audit Stranger"
version = "0.1.0"
api = "0.7.0"
player_commands = ["audit-fence"]
"#;

/// What tells the stranger's instance which role to bind.
const STRANGER_CONFIG: &str = "mode = \"audit-events\"\naudit_role = \"stranger\"\n";

/// One package a run deploys: the directory it is written to, its manifest and the
/// configuration its instance reads.
struct Package {
    id: &'static str,
    manifest: &'static str,
    config: &'static str,
}

/// The two packages of this test.
const PACKAGES: [Package; 2] = [
    Package {
        id: RECORDER,
        manifest: RECORDER_MANIFEST,
        config: RECORDER_CONFIG,
    },
    Package {
        id: STRANGER,
        manifest: STRANGER_MANIFEST,
        config: STRANGER_CONFIG,
    },
];

/// One log line a component guest asked its host to record.
struct LogLine {
    /// The package that logged it.
    plugin: String,
    /// Whether the guest logged it at error level.
    error: bool,
    message: String,
}

/// The host-services implementation this test hands the component host: the
/// guests' diagnostics land in this test's own channel instead of a global
/// subscriber, so this binary never touches process-wide tracing state.
struct RecordingLog {
    id: String,
    lines: UnboundedSender<LogLine>,
}

impl HostServices for RecordingLog {
    fn log(&mut self, level: LogLevel, message: &str) {
        let _ = self.lines.send(LogLine {
            plugin: self.id.clone(),
            error: matches!(level, LogLevel::Error),
            message: message.to_owned(),
        });
    }

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// The component host's view of this server's live sessions: the production
/// wiring's own pass-through, so a line the guest addresses to a stable identity
/// reaches the connection that identity holds right now.
struct ServerSessions(mc_net::PlayerSessionsHandle);

impl PlayerSessions for ServerSessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        self.0.session_of(player)
    }
}

/// The immutable registries every run binds with, built from the embedded required
/// data so the test needs no `data/vanilla` sidecar.
struct Registries {
    data: Arc<mc_data::VanillaData>,
    blocks: Arc<mc_world::BlockRegistry>,
    items: Arc<mc_data::items::ItemRegistry>,
    tags: Arc<mc_data::tags::TagsData>,
    recipes: Arc<Vec<mc_data::recipes::Recipe>>,
    loot: Arc<mc_data::loot::LootTables>,
    item_facts: Arc<mc_data::item_components::ItemFactsTable>,
    block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
}

impl Registries {
    fn new() -> Self {
        let block_report = mc_data::blocks::solaris_required_blocks_report();
        let items = Arc::new(mc_data::items::solaris_required_items());
        let tags = Arc::new(mc_data::tags::solaris_required_item_tags(&items));
        let blocks = Arc::new(
            mc_world::BlockRegistry::from_report(&block_report).expect("required block registry"),
        );
        Self {
            data: Arc::new(mc_data::solaris_required_data()),
            blocks,
            items,
            tags,
            recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
            loot: Arc::new(mc_data::loot::builtin().clone()),
            item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
                &block_report,
            )),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        }
    }
}

/// One running component server: a real host over the packages this test writes to
/// disk, and a real server bound on the arena.
struct Running {
    /// The directory the packages were discovered from; it outlives the host.
    _deployment: tempfile::TempDir,
    address: SocketAddr,
    /// The server's own simulation clock, so a wait for "enough ticks for a break
    /// to commit" is a wait for ticks instead of a sleep.
    ticks: watch::Receiver<u64>,
    shutdown: mc_net::ShutdownHandle,
    host: PluginHost,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Running {
    /// Write the packages, start their host, and bind a real server on the arena.
    async fn start(
        registries: &Registries,
        block_report: &[mc_data::blocks::BlockReport],
        lines: UnboundedSender<LogLine>,
    ) -> Self {
        let deployment = tempfile::tempdir().expect("component deployment directory");
        for package in &PACKAGES {
            let directory = deployment.path().join(package.id);
            std::fs::create_dir_all(&directory).expect("component package directory");
            std::fs::write(directory.join("plugin.toml"), package.manifest)
                .expect("component manifest");
            std::fs::write(directory.join("plugin.wasm"), fixture::component_bytes())
                .expect("component artifact");
            std::fs::write(directory.join("config.toml"), package.config)
                .expect("component config");
        }

        let limits = PluginLimits::default();
        let config = DeploymentConfig {
            root: deployment.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: PACKAGES
                .iter()
                .map(|package| package.id.to_owned())
                .collect(),
            // Neither package asks for a capability: it sends its own lines and
            // reads the events its manifest declares.
            grants: BTreeMap::from([
                (RECORDER.to_owned(), Vec::new()),
                (STRANGER.to_owned(), Vec::new()),
            ]),
            require_grants: true,
            precommit_hooks: Vec::new(),
        };
        let discovered = discover(&config, &limits)
            .expect("the component packages are discovered")
            .into_packages();

        let sessions = mc_net::PlayerSessionsHandle::new();
        let host = start_deployment_with(
            discovered,
            limits,
            HostQueues::default(),
            Arc::new(ServerSessions(sessions.clone())),
            move |id: &str| RecordingLog {
                id: id.to_owned(),
                lines: lines.clone(),
            },
        )
        .expect("the component host starts");

        let shutdown = mc_net::ShutdownHandle::default();
        let world = combat_world::world(&registries.blocks, block_report, VIEW_DISTANCE);
        let bound = mc_net::bind_with_scripts(
            server_config(registries, world, &shutdown),
            host.boundary().clone(),
        )
        .await
        .expect("bind the component server");
        bound.register_player_sessions(&sessions);
        let ticks = bound
            .runtime_telemetry_handle()
            .subscribe_simulation_ticks();
        let address = bound.local_addr().expect("component server address");
        let server = tokio::spawn(async move { bound.serve().await });

        Self {
            _deployment: deployment,
            address,
            ticks,
            shutdown,
            host,
            server,
        }
    }

    /// Stop the server and its host, so their last diagnostics are readable.
    async fn stop(self) {
        self.shutdown.request();
        tokio::time::timeout(Duration::from_secs(30), self.server)
            .await
            .expect("the component server shutdown timed out")
            .expect("the component server task")
            .expect("the component server result");
        let host = self.host;
        let _counters = tokio::task::spawn_blocking(move || host.stop())
            .await
            .expect("component host stop task");
    }
}

/// The one server configuration this run binds with: the arena, and every player
/// an operator, because the actions this test takes are the server's own operator
/// commands.
fn server_config(
    registries: &Registries,
    world: mc_world::WorldStorage,
    shutdown: &mc_net::ShutdownHandle,
) -> mc_net::ServerConfig {
    mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "world events".to_owned(),
        max_players: 1,
        view_distance: VIEW_DISTANCE,
        data: Arc::clone(&registries.data),
        blocks: Arc::clone(&registries.blocks),
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::clone(&registries.tags),
        recipes: Arc::clone(&registries.recipes),
        loot: Arc::clone(&registries.loot),
        block_light: None,
        items: Arc::clone(&registries.items),
        item_facts: Arc::clone(&registries.item_facts),
        block_facts: Arc::clone(&registries.block_facts),
        entity_types: Arc::clone(&registries.entity_types),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: shutdown.clone(),
    }
}

#[tokio::test]
async fn one_component_records_every_committed_world_observation_and_no_stranger_does() {
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let registries = Registries::new();
    let dirt = item_id(&registries.items, "minecraft:dirt");
    let oak_log = item_id(&registries.items, "minecraft:oak_log");
    let netherite_axe = item_id(&registries.items, "minecraft:netherite_axe");
    let oak_planks_recipe = registries
        .recipes
        .iter()
        .position(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
        .and_then(|index| i32::try_from(index).ok())
        .expect("embedded oak planks recipe display id");
    let villager_type = entity_type_id(&registries.entity_types, "minecraft:villager");
    let chicken_type = entity_type_id(&registries.entity_types, "minecraft:chicken");

    let (lines, mut log) = tokio::sync::mpsc::unbounded_channel();
    let mut run = Running::start(&registries, &block_report, lines).await;
    let mut logs = Vec::new();
    let mut chat = Chat::default();

    // Both packages are live, and each bound the role its configuration names.
    wait_for_log(
        &mut log,
        &mut logs,
        RECORDER,
        "P3_AUDIT ready role=recorder",
    )
    .await;
    wait_for_log(
        &mut log,
        &mut logs,
        STRANGER,
        "P3_AUDIT ready role=stranger",
    )
    .await;

    // One real client. The pose the server hands out on entry is its own spawn
    // default, so this run reads nothing from it but the arena column it placed the
    // player in: the fixture's arena is a stone slab whose first air cell is where a
    // player stands, and that stand pose is the one this client reports for itself
    // before every action. The only thing required of the handed-out pose is the
    // fixture's own - that it lies in the arena's cleared volume, above its floor.
    let mut client = Client::connect(run.address).await.expect("client connect");
    let login = client
        .drive_login(run.address, PLAYER)
        .await
        .expect("drive login");
    client.drive_configuration().await.expect("configuration");
    let play = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    assert!(
        sync.y > f64::from(FLOOR_Y) && sync.y <= f64::from(ARENA_TOP_Y),
        "the server handed out a pose outside the arena fixture's cleared volume: ({}, {}, {})",
        sync.x,
        sync.y,
        sync.z
    );
    // The arena column the server placed the player in, and every position this run
    // names: the stand pose it reports for the player, the floor face the placement
    // is aimed at and the position it fills, the column the drop of the broken block
    // rests in, and the two summoned entities.
    let column = (sync.x.floor() as i32, sync.z.floor() as i32);
    let stand = (
        f64::from(column.0) + 0.5,
        STAND_Y,
        f64::from(column.1) + 0.5,
    );
    let place_face = (column.0 + 1, FLOOR_Y, column.1);
    let placed_block = (place_face.0, FLOOR_Y + 1, place_face.2);
    let drop_stand = (
        f64::from(column.0) + 1.5,
        STAND_Y,
        f64::from(column.1) + 0.5,
    );
    let villager_pose = (
        f64::from(column.0) + 2.5,
        STAND_Y,
        f64::from(column.1) + 2.5,
    );
    let villager_near = (
        f64::from(column.0) + 2.5,
        STAND_Y,
        f64::from(column.1) + 1.5,
    );
    let chicken_pose = (
        f64::from(column.0) + 1.5,
        STAND_Y,
        f64::from(column.1) + 2.5,
    );
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    drain_until_chunk(
        &mut client,
        &mut chat,
        (column.0.div_euclid(16), column.1.div_euclid(16)),
    )
    .await;
    client
        .write_packet(&ServerboundPlayerLoaded)
        .await
        .expect("report the player loaded");
    // Movement is admitted only once the server has the load it just asked for, so
    // the arena's own stand pose is reported now, as a move of this client: from
    // here on the player stands on the arena floor and every coordinate below is the
    // fixture's own.
    move_to(&mut client, stand).await;

    let actor = login.uuid.to_string();
    let session = u64::try_from(play.entity_id).expect("a live session id");

    // One interaction with one summoned entity, from an empty hand and the off
    // hand: the server's own accepted-interaction path, which is what publishes the
    // observation.
    command(
        &mut client,
        &format!(
            "summon minecraft:villager {} {} {}",
            villager_pose.0, villager_pose.1, villager_pose.2
        ),
    )
    .await;
    let villager = wait_for_frame::<AddEntity>(
        &mut client,
        &mut chat,
        "the summoned villager",
        &|packet: &AddEntity| packet.entity_type_id == villager_type,
    )
    .await;
    assert!(
        (villager.x - villager_pose.0).abs() < 0.05
            && (villager.y - villager_pose.1).abs() < 0.05
            && (villager.z - villager_pose.2).abs() < 0.05,
        "the summoned villager stood at ({}, {}, {}), not where this run named it",
        villager.x,
        villager.y,
        villager.z
    );
    move_to(&mut client, villager_near).await;
    client
        .write_packet(&ServerboundInteract {
            entity_id: villager.entity_id,
            hand: InteractionHand::OffHand,
            location: EntityVec3::ZERO,
            using_secondary_action: true,
        })
        .await
        .expect("interact with the summoned villager");
    let interacted = observation(&mut chat, &mut client, 1).await;
    assert_eq!(interacted.kind, "interact");
    assert_eq!(interacted.detail, ["entity=minecraft:villager"]);
    assert_provenance(&interacted, &actor, session);

    // One placed block: `debug give` puts one dirt in the held slot and the real
    // use-on packet places it against the arena floor's top face.
    move_to(&mut client, stand).await;
    command(&mut client, "debug give minecraft:dirt 1 0").await;
    wait_for_frame::<ClientboundContainerSetSlot>(
        &mut client,
        &mut chat,
        "the given dirt",
        &|slot: &ClientboundContainerSetSlot| {
            slot.container_id == 0
                && slot.slot == 36
                && slot.item_stack.item_id == dirt
                && slot.item_stack.count == 1
        },
    )
    .await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(place_face.0, place_face.1, place_face.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 1,
        })
        .await
        .expect("place the given dirt");
    let placed = observation(&mut chat, &mut client, 2).await;
    assert_eq!(placed.kind, "place");
    assert_eq!(
        placed.detail,
        [
            "block=minecraft:dirt".to_owned(),
            format!(
                "at={},{},{}",
                placed_block.0, placed_block.1, placed_block.2
            ),
        ]
    );
    assert_provenance(&placed, &actor, session);

    // One broken block: the survival break of that same block takes its own ticks,
    // and the drop it leaves is the pickup the recorder sees next.
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(placed_block.0, placed_block.1, placed_block.2),
            direction: Direction::Up,
            sequence: 2,
        })
        .await
        .expect("start breaking the placed dirt");
    wait_ticks(&mut run.ticks, BREAK_TICKS).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: pack_block_pos(placed_block.0, placed_block.1, placed_block.2),
            direction: Direction::Up,
            sequence: 3,
        })
        .await
        .expect("finish breaking the placed dirt");
    let broken = observation(&mut chat, &mut client, 3).await;
    assert_eq!(broken.kind, "break");
    assert_eq!(
        broken.detail,
        [
            "block=minecraft:dirt".to_owned(),
            format!(
                "at={},{},{}",
                placed_block.0, placed_block.1, placed_block.2
            ),
        ]
    );
    assert_provenance(&broken, &actor, session);

    // The pickup of that drop, which the owner credits when the player is close
    // enough for the item's own reach.
    move_to(&mut client, drop_stand).await;
    let picked = observation(&mut chat, &mut client, 4).await;
    assert_eq!(picked.kind, "pickup");
    assert_eq!(picked.detail, ["item=minecraft:dirt", "count=1"]);
    assert_provenance(&picked, &actor, session);

    // One craft from ingredients the server itself handed the player: the recipe's
    // own inputs are committed to the output exactly once.
    command(&mut client, "debug give minecraft:oak_log 2 0").await;
    wait_for_frame::<ClientboundContainerSetSlot>(
        &mut client,
        &mut chat,
        "the given oak logs",
        &|slot: &ClientboundContainerSetSlot| {
            slot.container_id == 0
                && slot.slot == 36
                && slot.item_stack.item_id == oak_log
                && slot.item_stack.count == 2
        },
    )
    .await;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: oak_planks_recipe,
            use_max_items: true,
        })
        .await
        .expect("craft the recipe the server admits");
    let crafted = observation(&mut chat, &mut client, 5).await;
    assert_eq!(crafted.kind, "craft");
    assert_eq!(crafted.detail, ["item=minecraft:oak_planks", "count=8"]);
    assert_provenance(&crafted, &actor, session);

    // One killed entity: a summoned chicken, and the lethal blow of the melee
    // weapon the server hands the player for it.
    command(
        &mut client,
        &format!(
            "summon minecraft:chicken {} {} {}",
            chicken_pose.0, chicken_pose.1, chicken_pose.2
        ),
    )
    .await;
    let chicken = wait_for_frame::<AddEntity>(
        &mut client,
        &mut chat,
        "the summoned chicken",
        &|packet: &AddEntity| packet.entity_type_id == chicken_type,
    )
    .await;
    // The pose this run reports for the player is the chicken's own, so the blow
    // lands from where the entity stands; the same pose is the last one this run
    // reported, and so the one the death below is recorded at.
    let last_pose = (chicken.x, chicken.y, chicken.z);
    move_to(&mut client, last_pose).await;
    command(&mut client, "debug give minecraft:netherite_axe 1 0").await;
    wait_for_frame::<ClientboundContainerSetSlot>(
        &mut client,
        &mut chat,
        "the given netherite axe",
        &|slot: &ClientboundContainerSetSlot| {
            slot.container_id == 0
                && slot.slot == 36
                && slot.item_stack.item_id == netherite_axe
                && slot.item_stack.count == 1
        },
    )
    .await;
    wait_ticks(&mut run.ticks, ATTACK_TICKS).await;
    client
        .write_packet(&ServerboundAttack {
            entity_id: chicken.entity_id,
        })
        .await
        .expect("land the lethal blow");
    let killed = observation(&mut chat, &mut client, 6).await;
    assert_eq!(killed.kind, "kill");
    assert_eq!(killed.detail, ["entity=minecraft:chicken"]);
    assert_provenance(&killed, &actor, session);

    // One death, from committed damage, at the pose the player's own last move
    // reported - which is the pose the essentials consumer records for it.
    command(&mut client, "debug survival damage 100").await;
    let died = observation(&mut chat, &mut client, 7).await;
    assert_eq!(died.kind, "death");
    assert_eq!(died.detail.len(), 1);
    let death_pose = pose(&died.detail[0]);
    assert!(
        (death_pose.0 - last_pose.0).abs() < 0.05
            && (death_pose.1 - last_pose.1).abs() < 0.05
            && (death_pose.2 - last_pose.2).abs() < 0.05,
        "the death was recorded at {death_pose:?}, not at the pose the player last reported {last_pose:?}"
    );
    assert_provenance(&died, &actor, session);

    client
        .write_packet(&ServerboundClientCommand {
            action: ClientCommandAction::PerformRespawn,
        })
        .await
        .expect("respawn after the committed death");
    wait_for_frame::<ClientboundRespawn>(
        &mut client,
        &mut chat,
        "the respawn",
        &|_: &ClientboundRespawn| true,
    )
    .await;

    // The audit package's own history, read back from the player's command: the
    // records it retained are exactly the observations it reported, in order, with
    // the same stamps.
    let observed = [interacted, placed, broken, picked, crafted, killed, died];
    command(&mut client, &format!("{AUDIT_ROOT} report")).await;
    let summary = chat
        .find(&mut client, "the audit history", &|line: &str| {
            line.starts_with(REPORTED)
        })
        .await;
    let mut summary_fields = summary
        .strip_prefix(REPORTED)
        .expect("audit report prefix")
        .split_whitespace();
    assert_eq!(number(summary_fields.next(), "records", &summary), 7);
    assert_eq!(number(summary_fields.next(), "seen", &summary), 7);
    assert!(
        number(summary_fields.next(), "latest_tick", &summary) >= observed[6].tick,
        "the history report moved the audit clock backwards"
    );
    let history: Vec<Observation> = chat
        .lines
        .iter()
        .filter(|line| line.starts_with(RECORDED))
        .map(|line| parse(line))
        .collect();
    assert_eq!(history, observed, "the history differs from what arrived");
    let reported = chat
        .lines
        .iter()
        .filter(|line| line.starts_with(OBSERVED))
        .count();
    assert_eq!(reported, 7, "an observation was reported more than once");

    // The stamps are the server's own pushed tick, and they never move backwards:
    // the audit's record order is the order the batches were produced in.
    let mut previous = 0;
    for record in &observed {
        assert!(
            record.tick >= previous,
            "observation {} carried tick {} after {previous}",
            record.index,
            record.tick
        );
        previous = record.tick;
    }

    // The stranger answers its own root, and nothing else: a package that
    // subscribed to none of these observations was handed none of them.
    command(&mut client, &format!("{FENCE_ROOT} ready")).await;
    chat.find(&mut client, "the stranger's fence", &|line: &str| {
        line == "P3_AUDIT ready role=stranger"
    })
    .await;
    assert!(
        !chat.lines.iter().any(|line| line.starts_with(LEAK)),
        "a world observation reached a package that subscribed to none: {:?}",
        chat.lines
    );

    drop(client);
    run.stop().await;
    drain_logs(&mut log, &mut logs);
    let errors: Vec<&str> = logs
        .iter()
        .filter(|line| line.error)
        .map(|line| line.message.as_str())
        .collect();
    assert!(errors.is_empty(), "a guest reported an error: {errors:?}");
    assert!(
        logs.iter()
            .all(|line| line.plugin != STRANGER || !line.message.starts_with(LEAK)),
        "the stranger logged a world observation: {:?}",
        logs.iter()
            .filter(|line| line.plugin == STRANGER)
            .map(|line| line.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        logs.iter()
            .any(|line| line.plugin == STRANGER && line.message.starts_with(READY)),
        "the stranger never reported itself live: {:?}",
        logs.iter()
            .filter(|line| line.plugin == STRANGER)
            .map(|line| line.message.as_str())
            .collect::<Vec<_>>()
    );
}

/// One observation, as a report line spells it out.
#[derive(Debug, PartialEq, Eq)]
struct Observation {
    /// The fixture's own index, counted from one.
    index: u64,
    /// The audit kind: `interact`, `place`, `break`, `pickup`, `craft`, `kill` or
    /// `death`.
    kind: String,
    /// The `EventContext.tick` of the batch that delivered it.
    tick: u64,
    /// The stable identity the record named.
    actor: String,
    /// The connection the record named.
    session: u64,
    /// The dimension the record named.
    dimension: String,
    /// The kind's own detail, one or two `name=value` tokens.
    detail: Vec<String>,
}

/// Read one report line:
/// `P3_AUDIT <verb> <index> kind=<kind> tick=<tick> actor=<uuid> session=<id> dimension=<dim> <detail...>`.
fn parse(line: &str) -> Observation {
    let mut tokens = line.split(' ');
    assert_eq!(tokens.next(), Some("P3_AUDIT"), "not an audit line: {line}");
    let verb = tokens.next().expect("a report verb");
    assert!(
        verb == "observe" || verb == "record",
        "unknown report verb in {line}"
    );
    let index = tokens
        .next()
        .and_then(|token| token.parse().ok())
        .unwrap_or_else(|| panic!("no index in {line}"));
    let kind = field(tokens.next(), "kind", line);
    let tick = number(tokens.next(), "tick", line);
    let actor = field(tokens.next(), "actor", line);
    let session = number(tokens.next(), "session", line);
    let dimension = field(tokens.next(), "dimension", line);
    let detail: Vec<String> = tokens.map(str::to_owned).collect();
    assert!(!detail.is_empty(), "no detail in {line}");
    Observation {
        index,
        kind,
        tick,
        actor,
        session,
        dimension,
        detail,
    }
}

/// The value of one `name=value` token.
fn field(token: Option<&str>, name: &str, line: &str) -> String {
    token
        .and_then(|token| token.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no {name} field in {line}"))
        .to_owned()
}

/// The number of one `name=<number>` token.
fn number(token: Option<&str>, name: &str, line: &str) -> u64 {
    field(token, name, line)
        .parse()
        .unwrap_or_else(|_| panic!("the {name} field of {line} is not a number"))
}

/// The pose inside one `position=x,y,z` detail token.
fn pose(token: &str) -> (f64, f64, f64) {
    let Some(pose) = token.strip_prefix("position=") else {
        panic!("not a death detail: {token}");
    };
    let mut values = pose.split(',');
    let mut next = || {
        values
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("{token} is not a pose"))
    };
    let pose = (next(), next(), next());
    assert!(values.next().is_none(), "{token} has too many coordinates");
    pose
}

/// The actor, session and dimension every observation of this run must name: the
/// identity and connection that produced it, and the one dimension of the arena.
fn assert_provenance(observation: &Observation, actor: &str, session: u64) {
    assert_eq!(
        observation.actor, actor,
        "the observation named another actor"
    );
    assert_eq!(
        observation.session, session,
        "the observation named another session"
    );
    assert_eq!(observation.dimension, "minecraft:overworld");
    assert!(
        observation.tick > 0,
        "the observation carried no tick stamp: {observation:?}"
    );
}

/// Every system chat line this run read, in the order it read them.
///
/// One journal for the whole test, because the commands of one admitted batch
/// reach a client through more than one publication lane and the order those lanes
/// delivered them in is nothing a test may assume. A wait that finds what it wants
/// here first cannot consume the line another wait is about to need.
#[derive(Default)]
struct Chat {
    lines: Vec<String>,
}

impl Chat {
    /// The `n`-th line that starts with `prefix`, waiting for it to arrive.
    async fn nth(&mut self, client: &mut Client, what: &str, prefix: &str, n: usize) -> String {
        let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
        loop {
            let matching: Vec<&String> = self
                .lines
                .iter()
                .filter(|line| line.starts_with(prefix))
                .collect();
            if let Some(line) = matching.get(n.saturating_sub(1)) {
                return (*line).clone();
            }
            self.read(client, what, deadline).await;
        }
    }

    /// One line that satisfies `wanted`, from the lines read so far or from the
    /// wire.
    async fn find(
        &mut self,
        client: &mut Client,
        what: &str,
        wanted: &dyn Fn(&str) -> bool,
    ) -> String {
        let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
        loop {
            if let Some(line) = self.lines.iter().find(|line| wanted(line)) {
                return line.clone();
            }
            self.read(client, what, deadline).await;
        }
    }

    /// Read one frame, recording a chat line or answering a keepalive.
    async fn read(&mut self, client: &mut Client, what: &str, deadline: tokio::time::Instant) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let mut frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|error| panic!("{what}: {error}; lines read so far: {:?}", self.lines));
        absorb(self, client, &mut frame).await;
    }
}

/// The `n`-th observation the recorder reported, waiting for it to arrive.
async fn observation(chat: &mut Chat, client: &mut Client, n: usize) -> Observation {
    let line = chat
        .nth(client, &format!("observation {n}"), OBSERVED, n)
        .await;
    parse(&line)
}

/// Answer a keepalive, confirm a position the server itself sent, and record a
/// chat line, so every wait in this file sees the same journal of what the server
/// published.
async fn absorb(chat: &mut Chat, client: &mut Client, frame: &mut mc_protocol::RawFrame) {
    if frame.id == ClientboundKeepAlive::ID {
        let packet = ClientboundKeepAlive::decode(&mut frame.body).expect("decode KeepAlive");
        client
            .write_packet(&ServerboundKeepAlive { id: packet.id })
            .await
            .expect("answer KeepAlive");
    } else if frame.id == SynchronizePlayerPosition::ID {
        // A correction is the server's own authoritative pose: a client confirms it
        // and carries on, and a move this run made that the authority refused is
        // visible in the pose a later observation reports rather than hidden here.
        let packet =
            SynchronizePlayerPosition::decode(&mut frame.body).expect("decode SyncPlayerPos");
        client
            .write_packet(&ConfirmTeleportation {
                teleport_id: packet.teleport_id,
            })
            .await
            .expect("confirm the server's own position");
    } else if frame.id == ClientboundSystemChat::ID {
        let packet = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
        chat.lines.push(text_of(&packet));
    }
}

/// Wait for one frame of `T` the run needs, recording the chat lines it reads
/// meanwhile. `wanted` decides which of that packet's frames is the one asked for.
async fn wait_for_frame<T: Packet>(
    client: &mut Client,
    chat: &mut Chat,
    what: &str,
    wanted: &dyn Fn(&T) -> bool,
) -> T {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let mut frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        if frame.id != T::ID {
            absorb(chat, client, &mut frame).await;
            continue;
        }
        let packet = T::decode(&mut frame.body).unwrap_or_else(|error| panic!("{what}: {error}"));
        if wanted(&packet) {
            return packet;
        }
    }
}

/// Wait for the arena's own chunk, so every later action happens in a loaded,
/// resident part of the world.
async fn drain_until_chunk(client: &mut Client, chat: &mut Chat, target: (i32, i32)) {
    wait_for_frame::<LevelChunkWithLight>(
        client,
        chat,
        "the arena chunk",
        &|packet: &LevelChunkWithLight| (packet.chunk_x, packet.chunk_z) == target,
    )
    .await;
}

/// Wait for `additional` more of the server's own simulation ticks.
async fn wait_ticks(ticks: &mut watch::Receiver<u64>, additional: u64) {
    let target = (*ticks.borrow()).saturating_add(additional);
    tokio::time::timeout(WIRE_TIMEOUT, async {
        loop {
            if *ticks.borrow_and_update() >= target {
                return;
            }
            ticks
                .changed()
                .await
                .expect("simulation tick publisher remains active");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the server did not push {additional} more simulation ticks"));
}

/// Send one chat command, exactly as a client does.
async fn command(client: &mut Client, command: &str) {
    client
        .write_packet(&ServerboundChatCommand {
            command: command.to_owned(),
        })
        .await
        .unwrap_or_else(|error| panic!("send /{command}: {error}"));
}

/// Report one authoritative pose, exactly as a client does.
async fn move_to(client: &mut Client, (x, y, z): (f64, f64, f64)) {
    client
        .write_packet(&ServerboundMovePlayerPos {
            x,
            y,
            z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .unwrap_or_else(|error| panic!("move to ({x}, {y}, {z}): {error}"));
}

/// Wait for one line a guest logged, answering the lines read so far to the run's
/// own journal.
async fn wait_for_log(
    log: &mut UnboundedReceiver<LogLine>,
    logs: &mut Vec<LogLine>,
    plugin: &str,
    message: &str,
) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        if logs
            .iter()
            .any(|line| line.plugin == plugin && line.message.starts_with(message))
        {
            return;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let line = tokio::time::timeout(remaining, log.recv())
            .await
            .unwrap_or_else(|_| panic!("{plugin} never logged {message:?}"))
            .expect("the component host left before every package reported itself");
        logs.push(line);
    }
}

/// Take every diagnostic the guests logged so far.
fn drain_logs(log: &mut UnboundedReceiver<LogLine>, logs: &mut Vec<LogLine>) {
    while let Ok(line) = log.try_recv() {
        logs.push(line);
    }
}

/// The plain text of one system chat frame.
fn text_of(packet: &ClientboundSystemChat) -> String {
    let mut bytes = Bytes::copy_from_slice(&packet.content_nbt);
    let tag = mc_nbt::read_network(&mut bytes).expect("read text component nbt");
    let mc_nbt::Tag::Compound(fields) = tag else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("system chat component must contain text")
}

/// One item's registry id, from the embedded required data.
fn item_id(items: &mc_data::items::ItemRegistry, name: &str) -> u32 {
    items
        .id_of(&mc_data::Identifier::parse(name).expect("checked item identifier"))
        .unwrap_or_else(|| panic!("missing item {name}"))
}

/// One entity type's registry id, from the embedded required data.
fn entity_type_id(registry: &mc_data::entity_types::EntityTypeRegistry, name: &str) -> i32 {
    registry
        .id_of(&mc_data::Identifier::parse(name).expect("checked entity identifier"))
        .and_then(|id| i32::try_from(id).ok())
        .unwrap_or_else(|| panic!("missing entity type {name}"))
}
