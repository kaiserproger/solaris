//! P3 zone-market acceptance through one real component, one real zone owner and
//! one real world.
//!
//! The component is the repository's own example plugin, built to a component the
//! way a published package is; the zone owner is the server's own script zone
//! adapter, driven only by movement the server accepted; and the market is the
//! same server-owned menu the inventory/storage fixture serves. A real client logs
//! in over the wire, asks for the fixed `trade-zone` box, crosses the boundary, and
//! every observation this test makes is one a consumer of the running server can
//! make: the chat line a guest published, the screen the server published, the
//! close it published, and the authoritative inventory of a purchase and a refund.
//!
//! The crossings are real movement: the operator command places the client at the
//! outside position, and every boundary is then crossed by a movement packet the
//! server's own movement authority admits. Nothing here injects a transition, so a
//! run in which the owner publishes none publishes no marker either.
//!
//! A second package subscribes to `player.zone_entered`/`player.zone_exited` and
//! owns no zone. Its own fence command proves it is live while it never reports a
//! transition of somebody else's zone, which is what makes the zone owner's own
//! targeting of a transition a result rather than a missing subscription.
//!
//! The adapter's dimension scoping and its disconnect cleanup are pinned by the
//! server's own adapter tests (`mc-net` `script::zone_tests`); this test drives the
//! same adapter over a real connection and asserts what only a consumer sees -
//! including the reconnect, where the market must open on the connection the
//! transition named rather than on the one the fixture saw first.

// Reuse the host tests' component builder.
#[path = "../../mc-plugin-host/tests/fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_data::Identifier;
use mc_plugin_host::bindings::solaris::plugin::commands::{Command as WireCommand, UpsertZone};
use mc_plugin_host::bindings::solaris::plugin::types::Position;
use mc_plugin_host::{
    AdapterError, CommandBatch, DeploymentConfig, DiscoveryMode, HostQueues, HostServices,
    LogLevel, NoSessions, PlayerSessions, PluginHost, PluginLimits, discover,
    start_deployment_with, to_script_batch,
};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundContainerClose, ClientboundContainerSetContent,
    ClientboundKeepAlive, ClientboundOpenScreen, ClientboundSystemChat, ConfirmTeleportation,
    ContainerInput, HashedStack, HashedStackComponentHashes, ItemStack, LevelChunkWithLight,
    MovePlayerFlags, ServerboundChatCommand, ServerboundContainerClick, ServerboundKeepAlive,
    ServerboundMovePlayerPos, ServerboundPlayerLoaded, SynchronizePlayerPosition,
};
use mc_script::{CommandCapabilities, ScriptPluginManifest};
use mc_test_harness::client::Client;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// The one package that owns the zone and the market.
const OWNER: &str = "inventory-compat";
/// The second package: it subscribes to the transition events and owns no zone.
const OBSERVER: &str = "inventory-zone-observer";
/// The login name of the fixture's one player. Offline mode derives the uuid from
/// it, so every connection of this test is the same player.
const PLAYER: &str = "Trader";

/// The fixed box the contract names, as the fixture registers it.
const ZONE: &str = "trade-zone";
const ZONE_DIMENSION: &str = "minecraft:overworld";
const ZONE_MINIMUM: (f64, f64, f64) = (10.0, 80.0, 10.0);
const ZONE_MAXIMUM: (f64, f64, f64) = (16.0, 84.0, 16.0);
/// The two positions the box's boundaries are crossed at, exactly as the shared
/// contract states them: both in the box's own chunk, four blocks apart.
const OUTSIDE: (f64, f64, f64) = (8.5, 80.0, 12.5);
const INSIDE: (f64, f64, f64) = (12.5, 80.0, 12.5);
/// A second position inside the box, so a movement that changes nothing about the
/// membership is a real movement rather than a repeated one.
const INSIDE_ON: (f64, f64, f64) = (13.5, 80.0, 13.5);

/// The three lines the owner publishes. The readiness line waits for the owner's
/// own applied answer; the two transition lines are the fences over what the
/// fixture staged when the owner told it the player crossed.
const READY: &str = "P3_ZONE ready zone=trade-zone";
const ENTERED: &str = "P3_ZONE entered zone=trade-zone";
const EXITED: &str = "P3_ZONE exited zone=trade-zone";
/// The line the fixture logs when a connection leaves, which is what this test
/// waits for before it reconnects the same player.
const LEAVE_PREFIX: &str = "P3_ZONE_LEFT";
/// The observer's own command, the line it answers it with, and the prefix of the
/// line it would answer a transition with if one ever reached it.
const ZONE_FENCE_COMMAND: &str = "zone-fence";
const ZONE_FENCE: &str = "P3_ZONE_FENCE";
const ZONE_LEAK_PREFIX: &str = "P3_ZONE_LEAK";

/// The market the fixture opens from a boundary crossing: the id is the fixture's,
/// but what this test observes is the title the client is shown and the two fixed
/// buttons the server published with it.
const MARKET_TITLE: &str = "WASM Market";
const BUY_SLOT: i16 = 0;
const CLOSE_SLOT: i16 = 8;
const APPLE_ITEM: &str = "minecraft:apple";
const APPLE_LABEL: &str = "Apple: buy 2 emeralds / refund 1 apple";
const BARRIER_ITEM: &str = "minecraft:barrier";
const CLOSE_LABEL: &str = "Close";
/// The items one row of a script menu holds: the nine buttons, then the 27
/// main-inventory and nine hotbar slots the server appends.
const MENU_ITEMS: usize = 45;

/// The emeralds the operator gives the player before the trading phase, and the
/// totals the ledger's two committed transactions must leave: one purchase spends
/// two emeralds and grants an apple, and one refund puts both back.
const EMERALDS: i32 = 4;
const EMERALD_ITEM: &str = "minecraft:emerald";
const AFTER_BUY: (i32, i32) = (2, 1);
const AFTER_REFUND: (i32, i32) = (4, 0);

/// The lines the fixture's own transaction protocol reports. A committed action
/// names the ledger count the fixture's read-back confirmed.
const BUY_COMMITTED: &str = "P3_TRADE buy committed ledger=1";
const REFUND_COMMITTED: &str = "P3_TRADE refund committed ledger=0";

/// How long one step may spend on its real round trips before this test calls it
/// stalled. Nothing here sleeps: the wait ends on the fixture's own line, a frame
/// the server published, or a line the guests logged.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one wire read may wait for a join or a fence line.
const WIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// The component deployment's manifest: the `trade` root, the zone it registers,
/// the market and the storage its actions move. The owner declares the transition
/// subscriptions as the events it acts on; the owner's answers and its own zone's
/// transitions reach it by targeting, which the ownership of the box decides.
const OWNER_MANIFEST: &str = r#"
id = "inventory-compat"
name = "Inventory Compat"
version = "0.1.0"
api = "0.7.0"
events = ["player.joined", "player.left", "inventory.menu.clicked", "player.zone_entered", "player.zone_exited"]
player_commands = ["trade"]
capabilities = ["storage", "inventory_storage_transactions", "inventory_menus", "zones"]
"#;

/// What tells the owner's instance which mode to run.
const OWNER_CONFIG: &str = "mode = \"zone-market\"\n";

/// The observer package: the same component under its own id, subscribing to the
/// transition events and owning no zone. It declares no capability, because it asks
/// for nothing but its own chat lines, and the one command it answers is what makes
/// its silence about the owner's transitions a result.
const OBSERVER_MANIFEST: &str = r#"
id = "inventory-zone-observer"
name = "Inventory Zone Observer"
version = "0.1.0"
api = "0.7.0"
events = ["player.zone_entered", "player.zone_exited"]
player_commands = ["zone-fence"]
"#;

/// What tells the observer's instance which mode to run.
const OBSERVER_CONFIG: &str = "mode = \"zone-observer\"\n";

/// One package a run deploys: the directory it is written to, its manifest and the
/// configuration its instance reads.
struct Package {
    id: &'static str,
    manifest: &'static str,
    config: &'static str,
}

/// The two packages of this test: the owner with its zone and its market, and the
/// subscriber that owns neither.
const PACKAGES: [Package; 2] = [
    Package {
        id: OWNER,
        manifest: OWNER_MANIFEST,
        config: OWNER_CONFIG,
    },
    Package {
        id: OBSERVER,
        manifest: OBSERVER_MANIFEST,
        config: OBSERVER_CONFIG,
    },
];

/// One log line a component guest asked its host to record.
struct LogLine {
    /// The package that logged it, so a diagnostic names which instance answered.
    plugin: String,
    /// Whether the guest logged it at error level. The fixture reports what it
    /// could not conclude that way, so an error line means the run did not finish
    /// what it was asked for.
    error: bool,
    message: String,
}

/// The host-services implementation this test hands the component host: the
/// guest's diagnostics land in this test's own channel instead of a global
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

/// The component host's view of this server's live sessions.
///
/// The fixture addresses the player by stable identity - that is what a marker
/// line is sent to - and only the server knows which session that identity holds
/// right now, so this is the production wiring's own pass-through and nothing more.
struct ServerSessions(mc_net::PlayerSessionsHandle);

impl PlayerSessions for ServerSessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        self.0.session_of(player)
    }
}

/// The immutable registries every run of this test binds with, built once from the
/// embedded required data so the test needs no `data/vanilla` sidecar.
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

/// A generator that produces empty chunks.
///
/// The contract's fixed box sits at y 80..84 in the overworld, and this run needs
/// air around it: a terrain generator would put the ground wherever its noise says,
/// and a player embedded in stone is refused by the server's own movement authority
/// before any boundary could be crossed. Every chunk is still a full chunk of the
/// overworld's own geometry, so the world's residency, collision and lighting paths
/// all run exactly as they do over generated terrain.
struct EmptyTerrain;

impl mc_world::ChunkGenerator for EmptyTerrain {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(0),
            Identifier::parse("minecraft:plains").expect("checked biome identifier"),
        );
        chunk.status = "minecraft:full".to_owned();
        chunk.mark_dirty();
        chunk
    }
}

/// One running component server: a real host over the packages this test writes to
/// disk, and a real server bound on the world it opened.
struct Running {
    /// The directory the packages were discovered from. It stays alive for as long
    /// as the host runs, because that is what the host was started from.
    _deployment: tempfile::TempDir,
    address: SocketAddr,
    shutdown: mc_net::ShutdownHandle,
    log: UnboundedReceiver<LogLine>,
    host: PluginHost,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Running {
    /// Write the packages, start their host, and bind a real server on the world.
    ///
    /// The order is the live server's: the host exists before the network binds,
    /// and it resolves players through the same session handle the server publishes
    /// into, so a plugin's message reaches the connection that holds a player right
    /// now.
    async fn start(registries: &Registries, world_dir: &Path) -> Self {
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
            grants: BTreeMap::from([
                (
                    OWNER.to_owned(),
                    vec![
                        "storage".to_owned(),
                        "inventory_storage_transactions".to_owned(),
                        "inventory_menus".to_owned(),
                        "zones".to_owned(),
                    ],
                ),
                (OBSERVER.to_owned(), Vec::new()),
            ]),
            require_grants: true,
            precommit_hooks: Vec::new(),
        };
        let discovered = discover(&config, &limits)
            .expect("the component packages are discovered")
            .into_packages();

        let sessions = mc_net::PlayerSessionsHandle::new();
        let (lines, log) = tokio::sync::mpsc::unbounded_channel();
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
        let config = server_config(
            registries,
            open_world(world_dir, registries),
            &shutdown,
            "zone market",
        );
        let bound = mc_net::bind_with_scripts(config, host.boundary().clone())
            .await
            .expect("bind the component server");
        bound.register_player_sessions(&sessions);
        let address = bound.local_addr().expect("component server address");
        let server = tokio::spawn(async move { bound.serve().await });

        Self {
            _deployment: deployment,
            address,
            shutdown,
            log,
            host,
            server,
        }
    }

    /// Stop the server and its host, and answer the lines the guests logged.
    async fn stop(mut self) -> Vec<LogLine> {
        self.shutdown.request();
        let server = self.server;
        tokio::time::timeout(Duration::from_secs(30), server)
            .await
            .expect("the component server shutdown timed out")
            .expect("the component server task")
            .expect("the component server result");
        let host = self.host;
        let _counters = tokio::task::spawn_blocking(move || host.stop())
            .await
            .expect("component host stop task");
        let mut lines = Vec::new();
        while let Ok(line) = self.log.try_recv() {
            lines.push(line);
        }
        lines
    }
}

#[tokio::test]
async fn one_component_drives_its_market_from_real_zone_boundaries_over_the_wire() {
    let world = tempfile::tempdir().expect("one temporary persistent world");
    std::fs::create_dir_all(world.path().join("region")).expect("world region directory");
    let registries = Registries::new();
    let mut run = Running::start(&registries, world.path()).await;
    let items = Arc::clone(&registries.items);
    let address = run.address;

    let mut journal = Journal::default();
    let mut client = login(address, PLAYER).await;
    // The observer is live before any crossing: its fence line is the positive
    // control that makes its silence about the owner's transitions a result.
    command(&mut client, ZONE_FENCE_COMMAND).await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "fence",
        ZONE_FENCE,
        Want::Marker,
    )
    .await;
    trade(&mut client, "zone-setup").await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "zone-setup",
        READY,
        Want::Marker,
    )
    .await;

    // The first crossing: the operator command places the client outside the box,
    // which is a real teleport the server commits on the connection, and the
    // movement packet then carries it across the boundary. The teleport's own
    // answer is what the wait ends on, so the position sync it published before it
    // is a frame this client has already read and confirmed: the movement is then
    // admitted instead of being held behind an unconfirmed teleport.
    teleport(&mut client, OUTSIDE).await;
    let teleported = teleported_to(OUTSIDE);
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "tp",
        &teleported,
        Want::Marker,
    )
    .await;
    // The client's own report of where the teleport left it. The operator command
    // commits a server-side pose no client authored, and the zone owner consumes
    // accepted client movement: this report is what acknowledges the authoritative
    // pose at the outside position, so the crossing that follows is driven from a
    // pose the client itself reported rather than from the teleport's own.
    move_to(&mut client, OUTSIDE).await;
    move_to(&mut client, INSIDE).await;
    let entered = wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "enter",
        ENTERED,
        Want::Screen,
    )
    .await;
    let market = entered.screen.expect("the entry opened the market");
    assert_market(&market, &items, &journal);

    // A movement that stays inside the box changes no membership: the owner
    // publishes no transition, so the fixture opens no second market. The fence
    // line is what makes "no second marker" an observation rather than a guess: the
    // server processed the movement before it answered the command.
    move_to(&mut client, INSIDE_ON).await;
    command(&mut client, ZONE_FENCE_COMMAND).await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "fence",
        ZONE_FENCE,
        Want::Marker,
    )
    .await;
    assert_eq!(
        journal
            .chats
            .iter()
            .filter(|line| line.as_str() == ENTERED)
            .count(),
        1,
        "a movement inside the box published another entry"
    );
    assert_eq!(
        journal.opened, 1,
        "a movement inside the box opened a screen"
    );

    // The crossing back out closes the market on the connection that opened it.
    move_to(&mut client, OUTSIDE).await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "exit",
        EXITED,
        Want::Close(market.container_id),
    )
    .await;

    // The trading phase on this same connection: the operator gives the player the
    // emeralds a purchase spends, and a purchase and a refund move exactly the
    // deltas the fixture's own protocol states.
    command(&mut client, &format!("give {EMERALD_ITEM} {EMERALDS}")).await;
    move_to(&mut client, INSIDE).await;
    let market = wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "enter",
        ENTERED,
        Want::Screen,
    )
    .await
    .screen
    .expect("the re-entry opened the market");
    click_buy(&mut client, &market, ContainerInput::Pickup, 0).await;
    let seen = wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "buy",
        BUY_COMMITTED,
        Want::Close(market.container_id),
    )
    .await;
    assert_totals(
        seen.inventory
            .as_ref()
            .expect("a committed purchase publishes the authoritative inventory"),
        &items,
        AFTER_BUY,
    );

    move_to(&mut client, OUTSIDE).await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "exit",
        EXITED,
        Want::Marker,
    )
    .await;
    move_to(&mut client, INSIDE).await;
    let market = wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "enter",
        ENTERED,
        Want::Screen,
    )
    .await
    .screen
    .expect("the re-entry opened the market");
    click_buy(&mut client, &market, ContainerInput::Pickup, 1).await;
    let seen = wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "refund",
        REFUND_COMMITTED,
        Want::Close(market.container_id),
    )
    .await;
    assert_totals(
        seen.inventory
            .as_ref()
            .expect("a committed refund publishes the authoritative inventory"),
        &items,
        AFTER_REFUND,
    );

    // The player leaves the box before the reconnect, so the connection that comes
    // back begins outside it whatever pose the server restored - which is what makes
    // the entry after the next crossing the one this phase is about.
    move_to(&mut client, OUTSIDE).await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "exit",
        EXITED,
        Want::Marker,
    )
    .await;

    // The reconnect: the same player joins on a new connection, and the market the
    // next crossing opens must appear on that connection - which is what the
    // transition's own session decides.
    drop(client);
    wait_for_leave(&mut run.log).await;
    let mut client = login(address, PLAYER).await;
    trade(&mut client, "zone-setup").await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "zone-setup",
        READY,
        Want::Marker,
    )
    .await;
    // One observed movement at the outside position first, whichever pose the
    // server restored: it leaves the player's membership empty, so the movement
    // that follows is a real crossing rather than a repeat of a position the
    // owner already saw.
    move_to(&mut client, OUTSIDE).await;
    move_to(&mut client, INSIDE).await;
    let reentered = wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "reconnect enter",
        ENTERED,
        Want::Screen,
    )
    .await;
    let market = reentered
        .screen
        .expect("the crossing of the reconnected player opened no market");
    assert_eq!(
        market.title, MARKET_TITLE,
        "the market of the reconnected player's crossing"
    );

    // One more fence after the last crossing, so every transition this run produced
    // has been through the guests before the run reports what it never saw.
    command(&mut client, ZONE_FENCE_COMMAND).await;
    wait_for(
        &mut client,
        &mut run.log,
        &mut journal,
        "fence",
        ZONE_FENCE,
        Want::Marker,
    )
    .await;

    // No transition of the owner's zone ever reached the subscriber that owns no
    // zone: the fence line above proves the package was live, so this is the
    // owner-only targeting of the transition and not a package that never ran.
    assert!(
        !journal
            .chats
            .iter()
            .any(|line| line.starts_with(ZONE_LEAK_PREFIX)),
        "a zone transition reached a subscriber that owns no zone: {:?}",
        journal.chats
    );

    for line in run.stop().await {
        assert!(
            !line.error,
            "{} reported an error instead of concluding: {}",
            line.plugin, line.message
        );
    }
}

/// A zone command with no `zones` grant is refused by the capability it needs, and
/// the same record under that grant converts: the refusal is the grant's own, not a
/// record this test got wrong.
#[test]
fn a_zone_command_without_its_grant_is_refused_by_the_capability_it_needs() {
    let limits = PluginLimits::default();
    let mut batch = CommandBatch::new();
    batch
        .push(fixed_zone(), &limits)
        .expect("the fixed zone stages");
    assert_eq!(
        to_script_batch(
            batch,
            batch_limit(),
            &NoSessions,
            &CommandCapabilities::none()
        ),
        Err(AdapterError::PermissionDenied {
            capability: "zones"
        }),
    );

    let mut batch = CommandBatch::new();
    batch
        .push(fixed_zone(), &limits)
        .expect("the fixed zone stages");
    to_script_batch(
        batch,
        batch_limit(),
        &NoSessions,
        &granted_zone_capabilities(),
    )
    .expect("the fixture's own zone command converts under the grant it declares");
}

/// The fixed box, in the contract's own record, for the checks that never reach a
/// server.
fn fixed_zone() -> WireCommand {
    WireCommand::UpsertZone(UpsertZone {
        zone: ZONE.to_owned(),
        dimension: ZONE_DIMENSION.to_owned(),
        minimum: Position {
            x: ZONE_MINIMUM.0,
            y: ZONE_MINIMUM.1,
            z: ZONE_MINIMUM.2,
        },
        maximum: Position {
            x: ZONE_MAXIMUM.0,
            y: ZONE_MAXIMUM.1,
            z: ZONE_MAXIMUM.2,
        },
    })
}

/// The bound every staged batch of these checks is admitted against.
fn batch_limit() -> NonZeroUsize {
    NonZeroUsize::new(mc_script::MAX_SCRIPT_COMMAND_BATCH).expect("non-zero")
}

/// The capabilities a manifest that declared the zone capability resolves to,
/// which is what the host pre-checks a callback's answer against.
fn granted_zone_capabilities() -> CommandCapabilities {
    ScriptPluginManifest::new(
        OWNER,
        "Inventory Compat",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_zones()
    .validate_for(mc_script::COMPONENT_PLUGIN_API_VERSION)
    .expect("the fixture's own declaration validates")
    .to_command_capabilities()
}

/// One menu, as the server published it: the window and the revision a click has to
/// name, and what the client was shown in it.
struct MenuScreen {
    container_id: i32,
    state_id: i32,
    title: String,
    items: Vec<ItemStack>,
}

/// Everything a run observed while it drove its client: the chat lines the guests
/// published, how many screens the server opened, and the windows it closed.
///
/// A wait appends here rather than answering with each line separately, because the
/// commands of one admitted batch reach a client through more than one publication
/// lane and the order those lanes delivered them in is not something this test may
/// assume. Reading the whole journal once a step has concluded is what makes an
/// assertion like "no second marker arrived" or "that transition never leaked"
/// independent of that order.
#[derive(Default)]
struct Journal {
    /// Every system chat line the client read, in the order it read them.
    chats: Vec<String>,
    /// How many screens the server opened for this client.
    opened: usize,
    /// Every container the server closed, in the order it closed them.
    closed: Vec<i32>,
}

/// What one wait must have observed before it may return.
#[derive(Clone, Copy)]
enum Want {
    /// The guest's own line, and nothing else.
    Marker,
    /// The guest's own line and the screen the crossing opened.
    Screen,
    /// The guest's own line and the close of one window.
    Close(i32),
}

/// What one wait saw of the step it was watching.
#[derive(Default)]
struct Observed {
    /// The last authoritative inventory the step published, if it published one.
    inventory: Option<ClientboundContainerSetContent>,
    /// The menu the step opened, with the content frame that matched its screen.
    screen: Option<MenuScreen>,
}

/// Log the fixture's player in over a fresh connection and return the client, once
/// the server has published the play entry, the command tree and the position the
/// client acknowledges.
///
/// The player also reports itself loaded, which is what a real client does and what
/// opens the server's movement gate: without it the first movement packets are
/// dropped until the server's own load deadline passes.
async fn login(address: SocketAddr, name: &str) -> Client {
    let mut client = Client::connect(address).await.expect("client connect");
    let _ = client
        .drive_login(address, name)
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition =
        client.read_typed().await.expect("initial player position");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("confirm initial position");
    client
        .write_packet(&ServerboundPlayerLoaded)
        .await
        .expect("report the player loaded");
    // The player's own chunk, before any movement: the server's movement authority
    // admits a pose only when the chunks the body occupies are ones it has already
    // streamed to this session (`PlaySession::loaded`), so a driver that moves
    // before this packet is read is corrected back to where it was and the
    // crossing it asked for never happens. Every world-backed fixture in this
    // harness waits for the same packet, for the same reason.
    wait_for_chunk(&mut client, box_chunk()).await;
    client
}

/// The chunk the contract's box and both of its crossing positions live in.
fn box_chunk() -> (i32, i32) {
    chunk_of(OUTSIDE)
}

/// The chunk one authoritative position occupies.
fn chunk_of((x, _y, z): (f64, f64, f64)) -> (i32, i32) {
    (
        (x.floor() as i32).div_euclid(16),
        (z.floor() as i32).div_euclid(16),
    )
}

/// Read frames until the server has streamed one chunk, exactly as a joining
/// client waits for its terrain.
///
/// Everything else a client answers while it loads is answered: a keepalive is
/// echoed and a position sync is confirmed, because the server holds the movement
/// of an unconfirmed teleport.
async fn wait_for_chunk(client: &mut Client, wanted: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let mut frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|error| panic!("the chunk {wanted:?} never streamed: {error}"));
        if frame.id == ClientboundKeepAlive::ID {
            let keepalive = ClientboundKeepAlive::decode(&mut frame.body)
                .expect("decode keepalive while loading terrain");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo keepalive while loading terrain");
        } else if frame.id == SynchronizePlayerPosition::ID {
            let sync = SynchronizePlayerPosition::decode(&mut frame.body)
                .expect("decode position sync while loading terrain");
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: sync.teleport_id,
                })
                .await
                .expect("confirm position sync while loading terrain");
        } else if frame.id == LevelChunkWithLight::ID {
            let chunk = LevelChunkWithLight::decode(&mut frame.body).expect("decode level chunk");
            if (chunk.chunk_x, chunk.chunk_z) == wanted {
                return;
            }
        }
    }
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

/// Send one `trade` action and nothing else.
async fn trade(client: &mut Client, action: &str) {
    command(client, &format!("trade {action}")).await;
}

/// Send one authoritative movement to a position, exactly as a client does.
///
/// Every position this test names is inside the box's own chunk and within the
/// server's per-packet movement bound of the one before it, so the movement
/// authority admits it and the zone observer sees the pose the client reported.
///
/// The client reports the ground state the world actually has: this run's terrain
/// is air everywhere, so there is nothing at the box's feet to stand on and the
/// honest client state is airborne. Claiming a ground the empty world does not
/// have is what the server's own fall authority punishes - the pose commit is
/// followed by the fall the claim implies, and a dead player's market is never
/// opened (`OpenScriptMenu` is dropped while the survival state is dead), so the
/// crossing would be observed while the screen it opened could not be.
async fn move_to(client: &mut Client, (x, y, z): (f64, f64, f64)) {
    client
        .write_packet(&ServerboundMovePlayerPos {
            x,
            y,
            z,
            flags: MovePlayerFlags::new(false, false),
        })
        .await
        .unwrap_or_else(|error| panic!("send movement to {x}/{y}/{z}: {error}"));
}

/// Place the client with the server's own operator command, which is how a driver
/// puts a player where a boundary is without walking it there.
async fn teleport(client: &mut Client, (x, y, z): (f64, f64, f64)) {
    command(client, &format!("tp {x} {y} {z}")).await;
}

/// The line the server answers that command with, in the server's own wording. A
/// wait over it is what makes the teleport's position sync - published just before
/// the line - a frame this client has read and confirmed.
fn teleported_to((x, y, z): (f64, f64, f64)) -> String {
    format!("Teleported to {x} {y} {z}")
}

/// Click the market's purchase button, naming the window and the revision the
/// client was shown and the item the server published at that slot.
async fn click_buy(client: &mut Client, market: &MenuScreen, input: ContainerInput, button: i8) {
    let slot = BUY_SLOT;
    let held = &market.items[usize::try_from(slot).expect("the slot is a wire index")];
    client
        .write_packet(&ServerboundContainerClick {
            container_id: market.container_id,
            state_id: market.state_id,
            slot_num: slot,
            button_num: button,
            container_input: input,
            changed_slots: vec![(slot, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: held.item_id,
                count: held.count,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("send the purchase click");
}

/// Read one step's frames and the guests' lines until the step published everything
/// it is expected to, failing on the fixture's error line first.
///
/// The wait ends on a line or a frame a guest produced, never on a sleep: a marker
/// that never arrives is the step stalling, and the error line - which the fixture
/// logs instead of a marker - is the most specific failure this test can report.
/// The client answers what a real client answers while it waits: a keepalive is
/// echoed and a position sync is confirmed, because the server holds the movement
/// of an unconfirmed teleport.
async fn wait_for(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
    step: &str,
    marker: &str,
    want: Want,
) -> Observed {
    let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
    let first_line = journal.chats.len();
    let first_close = journal.closed.len();
    let mut observed = Observed::default();
    let mut pending: Option<(i32, ClientboundOpenScreen)> = None;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "the {step} step never reported {marker}; saw {:?}",
                journal.chats
            );
        }
        let reported = journal.chats[first_line..]
            .iter()
            .any(|text| text == marker);
        let done = reported
            && match want {
                Want::Marker => true,
                Want::Screen => observed.screen.is_some(),
                Want::Close(id) => journal.closed[first_close..].contains(&id),
            };
        if done {
            return observed;
        }
        tokio::select! {
            line = log.recv() => {
                let Some(line) = line else {
                    panic!("the component host left before the {step} step reported {marker}");
                };
                assert!(
                    !line.error,
                    "{} reported a plugin error instead of the {step} step reporting {marker}: {}",
                    line.plugin,
                    line.message
                );
            }
            frame = tokio::time::timeout(remaining, client.read_frame()) => {
                let mut frame = match frame {
                    Ok(Ok(frame)) => frame,
                    Ok(Err(error)) => panic!("the {step} step lost its connection: {error}"),
                    Err(_) => panic!(
                        "the {step} step never reported {marker}; saw {:?}",
                        journal.chats
                    ),
                };
                if frame.id == ClientboundContainerSetContent::ID {
                    let content = ClientboundContainerSetContent::decode(&mut frame.body)
                        .expect("decode container content");
                    match pending.take() {
                        // The content of the screen this wait just read: an open is
                        // one screen and one content frame, and they name the same
                        // window.
                        Some((container_id, screen))
                            if container_id == content.container_id =>
                        {
                            observed.screen = Some(MenuScreen {
                                container_id,
                                state_id: content.state_id,
                                title: literal_text_component_text(&screen.title_nbt),
                                items: content.items,
                            });
                        }
                        Some(other) => pending = Some(other),
                        // Any other content frame is the authoritative inventory or a
                        // resync of a menu this step is not opening.
                        None => {
                            if content.container_id == 0 {
                                observed.inventory = Some(content);
                            }
                        }
                    }
                } else if frame.id == ClientboundOpenScreen::ID {
                    let screen = ClientboundOpenScreen::decode(&mut frame.body)
                        .expect("decode script menu screen");
                    journal.opened += 1;
                    pending = Some((screen.container_id, screen));
                } else if frame.id == ClientboundContainerClose::ID {
                    let close = ClientboundContainerClose::decode(&mut frame.body)
                        .expect("decode container close");
                    journal.closed.push(close.container_id);
                } else if frame.id == ClientboundSystemChat::ID {
                    let chat = ClientboundSystemChat::decode(&mut frame.body)
                        .expect("decode system chat");
                    journal.chats.push(literal_text_component_text(&chat.content_nbt));
                } else if frame.id == ClientboundKeepAlive::ID {
                    let keepalive = ClientboundKeepAlive::decode(&mut frame.body)
                        .expect("decode keepalive");
                    client
                        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                        .await
                        .expect("echo keepalive");
                } else if frame.id == SynchronizePlayerPosition::ID {
                    let sync = SynchronizePlayerPosition::decode(&mut frame.body)
                        .expect("decode position sync");
                    client
                        .write_packet(&ConfirmTeleportation {
                            teleport_id: sync.teleport_id,
                        })
                        .await
                        .expect("confirm position sync");
                }
            }
        }
    }
}

/// Wait for the fixture's own report that a connection has gone.
///
/// The server deregisters a session before it announces the leave, so the line this
/// waits for is the point after which the same player can log in again.
async fn wait_for_leave(log: &mut UnboundedReceiver<LogLine>) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let line = tokio::time::timeout(remaining, log.recv())
            .await
            .expect("the fixture never reported the connection leaving");
        let Some(line) = line else {
            panic!("the component host left before the fixture reported a connection leaving");
        };
        assert!(
            !line.error,
            "{} reported an error while the player left: {}",
            line.plugin, line.message
        );
        if line.message.starts_with(LEAVE_PREFIX) {
            return;
        }
    }
}

/// Check the market one crossing opened: the title the client was shown, the two
/// fixed buttons with the labels the fixture chose, and the player inventory the
/// server appends after them.
fn assert_market(market: &MenuScreen, items: &mc_data::items::ItemRegistry, journal: &Journal) {
    assert_eq!(
        journal.opened, 1,
        "the crossing opened {} screens",
        journal.opened
    );
    assert_eq!(
        market.title, MARKET_TITLE,
        "the title of the market the crossing opened"
    );
    assert_eq!(
        market.items.len(),
        MENU_ITEMS,
        "the items of the market the crossing opened"
    );
    let buy = &market.items[usize::try_from(BUY_SLOT).expect("the slot is a wire index")];
    assert_eq!(
        buy.item_id,
        item_id(items, APPLE_ITEM),
        "the market's buy slot"
    );
    assert_eq!(buy.count, 1, "the count at the market's buy slot");
    assert_eq!(
        buy.custom_name.as_deref(),
        Some(APPLE_LABEL),
        "the label of the market's buy slot"
    );
    let close = &market.items[usize::try_from(CLOSE_SLOT).expect("the slot is a wire index")];
    assert_eq!(
        close.item_id,
        item_id(items, BARRIER_ITEM),
        "the market's close slot"
    );
    assert_eq!(close.count, 1, "the count at the market's close slot");
    assert_eq!(
        close.custom_name.as_deref(),
        Some(CLOSE_LABEL),
        "the label of the market's close slot"
    );
}

/// Check the item totals of one authoritative inventory. A trade that conserved
/// nothing would move them, which is what makes a committed purchase and refund a
/// statement about both sides of the transaction.
fn assert_totals(
    content: &ClientboundContainerSetContent,
    items: &mc_data::items::ItemRegistry,
    totals: (i32, i32),
) {
    assert_eq!(
        total_of(content, item_id(items, EMERALD_ITEM)),
        totals.0,
        "the emeralds the player holds"
    );
    assert_eq!(
        total_of(content, item_id(items, APPLE_ITEM)),
        totals.1,
        "the apples the player holds"
    );
}

/// The total count of one item across every slot of one authoritative inventory.
fn total_of(content: &ClientboundContainerSetContent, item: u32) -> i32 {
    content
        .items
        .iter()
        .filter(|held| held.item_id == item)
        .map(|held| held.count)
        .sum()
}

/// The embedded id of one item resource.
fn item_id(items: &mc_data::items::ItemRegistry, resource: &str) -> u32 {
    items
        .id_of(&Identifier::parse(resource).expect("checked item identifier"))
        .unwrap_or_else(|| panic!("missing embedded item {resource}"))
}

/// One persistent world handle, reopened from disk.
fn open_world(
    world_dir: &Path,
    registries: &Registries,
) -> Arc<tokio::sync::Mutex<mc_world::WorldStorage>> {
    let storage =
        mc_world::WorldStorage::open_with_capacity(world_dir, Arc::clone(&registries.blocks), 49)
            .expect("reopen the persistent world")
            .with_item_registry(Arc::clone(&registries.items))
            .with_generator(Arc::new(EmptyTerrain));
    Arc::new(tokio::sync::Mutex::new(storage))
}

/// The server configuration every run binds with: one persistent world, so the real
/// movement, zone and menu owners are started and their decisions are the ones this
/// test observes.
fn server_config(
    registries: &Registries,
    world: Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    shutdown: &mc_net::ShutdownHandle,
    motd: &str,
) -> mc_net::ServerConfig {
    mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: motd.to_owned(),
        max_players: 1,
        view_distance: 2,
        data: Arc::clone(&registries.data),
        blocks: Arc::clone(&registries.blocks),
        world: Some(world),
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

/// The literal text of one system-chat frame, as the server's own component encodes
/// it.
fn literal_text_component_text(component: &[u8]) -> String {
    let mut bytes = Bytes::copy_from_slice(component);
    let mc_nbt::Tag::Compound(fields) =
        mc_nbt::read_network(&mut bytes).expect("read text component nbt")
    else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("literal system chat text")
}
