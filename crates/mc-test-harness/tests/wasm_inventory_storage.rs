//! P3 inventory/storage acceptance through one real component and one real world.
//!
//! The component is the repository's own example plugin, built to
//! `wasm32-unknown-unknown` and encoded as a component exactly the way a published
//! package is; the world is a real directory the server opens, saves and reads
//! back. A real client logs in over the wire and runs the fixture's `trade` root
//! one action at a time, and every observation this test makes is one a consumer
//! of the running server can make: the authoritative inventory the server
//! publishes, the line the fixture reports to the player, and the player state the
//! world's own file holds after the save path ran.
//!
//! The player starts with four emeralds and one unrelated component-bearing tool,
//! seeded through the world's own durable player format before the server opens
//! it - the same file the server writes and validates on login, so the tool's
//! components are real state rather than a value this test hands the guest. Two
//! purchases, an insufficient-funds refusal, a refund, a stale-original
//! compare-and-swap refusal, and a stale-session refusal after a reconnect then
//! prove the transaction commits both sides or neither, and the tool stays exactly
//! the stack it was.
//!
//! One action outside the acceptance sequence is driven here: `wide`, a purchase
//! whose storage side also writes three values at the contract's largest single
//! length, so the record's text is far past one string's bound while every string
//! is inside its own.
//!
//! A second test in this file drives the same component's server-owned market
//! menu over the wire: the `trade` root's menu actions open two menus and close
//! one, and a real client reads the screen and content frames the server publishes,
//! clicks the fixed slots with the window and revision it was shown, and reads the
//! fixture's own lines for what each click did. The purchases and refunds a click
//! causes are the same transaction the first test drives, so what the menu adds is
//! routing: a click on a stale window or a stale revision never reaches the plugin,
//! a close naming a menu the server is not holding for this plugin does not close
//! the one it is, and a menu request for a connection that has gone changes
//! nothing. The same run deploys a second real component that subscribes to
//! `inventory.menu.clicked` and owns no menu, which proves the click reaches its
//! owner and no other subscriber.

// Reuse the host tests' component builder.
#[path = "../../mc-plugin-host/tests/fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_data::Identifier;
use mc_nbt::Tag;
use mc_plugin_host::bindings::solaris::plugin::commands::{
    CloseInventoryMenu, Command as WireCommand, InventoryMenu, InventoryMenuSlot,
    InventoryResourceDelta, InventoryStorageTransaction, MessageTarget, OpenInventoryMenu,
    SendMessage,
};
use mc_plugin_host::bindings::solaris::plugin::storage::{StorageCasMutation, StorageMutation};
use mc_plugin_host::{
    AdapterError, CommandBatch, DeploymentConfig, DiscoveryMode, HostQueues, HostServices,
    LogLevel, NoSessions, PlayerSessions, PluginHost, PluginLimits, discover,
    start_deployment_with, to_script_batch,
};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundContainerClose, ClientboundContainerSetContent,
    ClientboundOpenScreen, ClientboundSystemChat, ConfirmTeleportation, ContainerInput,
    HashedStack, HashedStackComponentHashes, ItemStack, ServerboundChatCommand,
    ServerboundContainerClick, SynchronizePlayerPosition,
};
use mc_script::{CommandCapabilities, ScriptPluginManifest};
use mc_test_harness::client::Client;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// The second package this file deploys: one that subscribes to the click event
/// and owns no menu, so a click delivered to it would be a click delivered
/// somewhere it does not belong.
const OBSERVER: &str = "inventory-observer";

/// The one package this test deploys: the id the fixture's storage is keyed by.
const OWNER: &str = "inventory-compat";
/// The login name of the fixture's one player. Offline mode derives the uuid from
/// it, which is also the name of that player's durable file.
const PLAYER: &str = "Trader";
/// The line the fixture sends the joining player, which is the readiness a driver
/// waits for before it sends the first action.
const GREETING: &str = "Welcome!";

/// The resource names the fixture's protocol moves, as the seed and the wire
/// carry them.
const EMERALD_ITEM: &str = "minecraft:emerald";
const APPLE_ITEM: &str = "minecraft:apple";
/// The one unrelated item the player brings: a tool that carries components and
/// that no action of the sequence may touch.
const TOOL_ITEM: &str = "minecraft:diamond_pickaxe";
/// The name, damage and enchantment that tool carries, and that it must still
/// carry after every action.
const TOOL_NAME: &str = "Trader's Pick";
const TOOL_DAMAGE: i32 = 7;
const TOOL_ENCHANTMENT: &str = "minecraft:efficiency";
const TOOL_ENCHANTMENT_LEVEL: i32 = 3;

/// The inventory slots the world's player file fills: the first main-inventory
/// slot, which a purchase drains from, and the hotbar slot the tool sits in.
const EMERALD_SLOT: u8 = 9;
const TOOL_SLOT: u8 = 36;
/// How many emeralds the world's player file gives the player: two purchases.
const SEED_EMERALDS: i32 = 4;

/// The session a malformed record names in the conversion checks. Those checks
/// never reach a server, so the number is only a value of the record.
const UNKNOWN_SESSION: u64 = 7;

/// The fixture's ledger key.
const LEDGER_KEY: &str = "trade-ledger";

/// The line the fixture logs when the connection it traded on has left, which is
/// what this test waits for before it reconnects the same player.
const LEAVE_PREFIX: &str = "P3_TRADE_LEFT";

/// The two menus the fixture's market opens: the ids a close has to match, and the
/// titles the client is shown.
const MENU_MARKET: &str = "trade-market";
const MENU_MARKET_TITLE: &str = "WASM Market";
const MENU_NEXT: &str = "trade-market-next";
const MENU_NEXT_TITLE: &str = "WASM Market Next";
/// The fixture's two buttons: slot 0 is the item the market sells, slot 8 is the
/// barrier that closes the menu, and each carries the label the fixture chose.
const MENU_BUY_SLOT: i16 = 0;
const MENU_CLOSE_SLOT: i16 = 8;
const APPLE_LABEL: &str = "Apple: buy 2 emeralds / refund 1 apple";
const BARRIER_ITEM: &str = "minecraft:barrier";
const BARRIER_LABEL: &str = "Close";
/// The items one row of a script menu holds: the nine buttons the fixture asked
/// for, then the 27 main-inventory and nine hotbar slots the server appends.
const MENU_ITEMS: usize = 45;

/// The lines the fixture reports for the menu actions it stages. Every one of them
/// is a request: nothing in the contract answers a menu command, so a test that
/// wants to know a menu opened reads the screen the server published.
const MENU_OPEN_REQUESTED: &str = "P3_MENU open-requested";
const MENU_NEXT_REQUESTED: &str = "P3_MENU next-open-requested";
const MENU_CLOSE_REQUESTED: &str = "P3_MENU close-requested";
const MENU_STALE_CLOSE_REQUESTED: &str = "P3_MENU stale-close-requested";
const MENU_STALE_SESSION_REQUESTED: &str = "P3_MENU stale-session-requested";
/// The line the observer package answers its own command with, and the line it
/// would answer a click with if a click ever reached it.
const MENU_FENCE: &str = "P3_MENU_FENCE";
const MENU_LEAK_PREFIX: &str = "P3_MENU_LEAK";

/// How long one action may spend on its real round trips before this test calls it
/// stalled. Nothing here sleeps: the wait ends on the fixture's own line.
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one join may take to publish its readiness and inventory.
const WIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// One action of the sequence, what the player must read for it, and the
/// inventory a committed action must leave on the wire.
struct Step {
    /// The action argument sent to the `trade` root.
    action: &'static str,
    /// The exact line the fixture must report for it. The fixture reads the ledger
    /// back and checks what the owners did before it reports this, so the count it
    /// carries is a value the server answered with.
    marker: &'static str,
    /// The emerald and apple totals this action must leave. A refused action
    /// publishes no inventory snapshot of its own, so it names none.
    inventory: Option<(i32, i32)>,
}

/// The actions the contract's sequence runs on one connection: two purchases that
/// commit, a third the player cannot afford, a refund, and the first purchase
/// submitted again once the ledger has moved past it.
const TRADES: &[Step] = &[
    Step {
        action: "buy",
        marker: "P3_TRADE buy committed ledger=1",
        inventory: Some((2, 1)),
    },
    Step {
        action: "buy",
        marker: "P3_TRADE buy committed ledger=2",
        inventory: Some((0, 2)),
    },
    Step {
        action: "buy",
        marker: "P3_TRADE buy refused ledger=2",
        inventory: None,
    },
    Step {
        action: "refund",
        marker: "P3_TRADE refund committed ledger=1",
        inventory: Some((2, 1)),
    },
    Step {
        action: "retry-original",
        marker: "P3_TRADE retry-original refused ledger=1",
        inventory: None,
    },
];

/// The two actions that run after the reconnect: the probe that addresses the
/// session the first trade command named, and the opt-in staging-bound action.
///
/// `wide` is not part of the acceptance sequence - no driver sends it, and no
/// other action of this fixture writes anything but the ledger - so the ordinary
/// buy/refund protocol stays exactly as the contract states it.
const AFTER_REJOIN: &[Step] = &[
    Step {
        action: "stale-session",
        marker: "P3_TRADE stale-session refused ledger=1",
        inventory: None,
    },
    Step {
        action: "wide",
        marker: "P3_TRADE wide committed ledger=2",
        inventory: Some((0, 2)),
    },
];

/// The component deployment's manifest: the `trade` root, the storage it writes,
/// the transaction capability and the menu capability. The three subscriptions are
/// the events the fixture reads itself - the join it greets, the leave that tells
/// it the connection it traded on has gone, and the click the server publishes to
/// the plugin that opened the menu. The owners' answers need no subscription: a
/// result is delivered to the plugin that asked for it.
const MANIFEST: &str = r#"
id = "inventory-compat"
name = "Inventory Compat"
version = "0.1.0"
api = "0.7.0"
events = ["player.joined", "player.left", "inventory.menu.clicked"]
player_commands = ["trade"]
capabilities = ["storage", "inventory_storage_transactions", "inventory_menus"]
"#;

/// What tells the fixture which of its two modes to run.
const TRADE_CONFIG: &str = "mode = \"inventory-storage\"\n";

/// The observer package: the same component, deployed under its own id, with the
/// click subscription and no menu of its own. It declares no capability, because
/// it asks for nothing but its own chat lines, and the one command it answers is
/// what makes its silence about another package's menu click a result.
const OBSERVER_MANIFEST: &str = r#"
id = "inventory-observer"
name = "Inventory Observer"
version = "0.1.0"
api = "0.7.0"
events = ["inventory.menu.clicked"]
player_commands = ["menu-fence"]
"#;

/// What tells the observer's instance which mode to run.
const OBSERVER_CONFIG: &str = "mode = \"menu-observer\"\n";

/// One package a run deploys: the directory it is written to, its manifest and the
/// configuration its instance reads.
struct Package {
    id: &'static str,
    manifest: &'static str,
    config: &'static str,
}

/// One log line a component guest asked its host to record.
struct LogLine {
    /// The package that logged it, so a diagnostic names which instance answered.
    plugin: String,
    /// Whether the guest logged it at error level. The fixture reports an action it
    /// could not conclude that way, so an error line means that action did not
    /// answer.
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
/// right now, so this is the production wiring's own pass-through and nothing
/// more. A test that resolved identities itself would be answering a question the
/// server owns.
struct ServerSessions(mc_net::PlayerSessionsHandle);

impl PlayerSessions for ServerSessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        self.0.session_of(player)
    }
}

/// The immutable registries every run of this test binds with, built once from
/// the embedded required data so the test needs no `data/vanilla` sidecar.
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

/// One running component server: a real host over the packages a test writes to
/// disk, and a real server bound on the world that test opened.
///
/// Every field is something a test needs while it drives the run, and `stop` is
/// the only way to end one: the server is asked to stop, the host's thread joins
/// so its last diagnostics are readable, and the error lines its guests logged are
/// answered for the caller to assert on.
struct Running {
    /// The directory the packages were discovered from. It stays alive for as long
    /// as the host runs, because that is what the host was started from.
    _deployment: tempfile::TempDir,
    address: SocketAddr,
    save: mc_net::SaveHandle,
    sessions: mc_net::PlayerSessionsHandle,
    shutdown: mc_net::ShutdownHandle,
    log: UnboundedReceiver<LogLine>,
    host: PluginHost,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Running {
    /// Write the packages, start their host, and bind a real server on the world.
    ///
    /// The order is the live server's: the host exists before the network binds,
    /// and it resolves players through the same session handle the server
    /// publishes into - so a plugin's message reaches the connection that holds a
    /// player right now, exactly as it does in production.
    async fn start(
        registries: &Registries,
        world_dir: &Path,
        packages: &[Package],
        motd: &str,
    ) -> Self {
        let deployment = tempfile::tempdir().expect("component deployment directory");
        for package in packages {
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
            expected: packages
                .iter()
                .map(|package| package.id.to_owned())
                .collect(),
            grants: BTreeMap::from([(
                OWNER.to_owned(),
                vec![
                    "storage".to_owned(),
                    "inventory_storage_transactions".to_owned(),
                    "inventory_menus".to_owned(),
                ],
            )]),
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
            motd,
        );
        let bound = mc_net::bind_with_scripts(config, host.boundary().clone())
            .await
            .expect("bind the component server");
        bound.register_player_sessions(&sessions);
        let save = bound.save_handle();
        let address = bound.local_addr().expect("component server address");
        let server = tokio::spawn(async move { bound.serve().await });

        Self {
            _deployment: deployment,
            address,
            save,
            sessions,
            shutdown,
            log,
            host,
            server,
        }
    }

    /// Stop the server and its host, and answer the lines the guests logged.
    ///
    /// The server is asked to shut down first, then its task is awaited, then the
    /// host's thread is joined - in that order, so the guests have seen everything
    /// the run produced before they are retired and their last diagnostics are in
    /// the channel this drains. The deployment directory outlives the host, because
    /// it is what the host was started from.
    async fn stop(mut self) -> Vec<LogLine> {
        self.shutdown.request();
        let server = self.server;
        tokio::time::timeout(Duration::from_secs(30), server)
            .await
            .expect("the component server shutdown timed out")
            .expect("the component server task")
            .expect("the component server result");
        // Stop the host so its thread joins and its last diagnostics are readable.
        // The counters it returns are its own bookkeeping; what these tests assert
        // is what the wire and the world's own files carry.
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
async fn one_component_trades_real_inventory_and_storage_through_the_real_owners() {
    let world = tempfile::tempdir().expect("one temporary persistent world");
    std::fs::create_dir_all(world.path().join("region")).expect("world region directory");
    let registries = Registries::new();
    // The world's own durable player file, before the server opens the world: the
    // player arrives with four emeralds and a tool that carries components.
    seed_player_state(world.path());

    let mut running = Running::start(
        &registries,
        world.path(),
        &[Package {
            id: OWNER,
            manifest: MANIFEST,
            config: TRADE_CONFIG,
        }],
        "inventory storage acceptance",
    )
    .await;

    let items = Arc::clone(&registries.items);
    let tool = expected_tool(&items);
    // What every wait of this run appends to. The sequence reads its own markers,
    // so nothing here asserts on it; the menu test is where an observation a step
    // did not wait for has to be checkable.
    let mut journal = Journal::default();

    // The sequence on the first connection.
    let mut client = login(running.address, PLAYER).await;
    let ready = wait_for_ready(&mut client, PLAYER).await;
    assert_inventory(&ready, &items, &tool, (SEED_EMERALDS, 0));
    let mut current = (SEED_EMERALDS, 0);
    for step in TRADES {
        current = run_step(
            &mut client,
            &mut running.log,
            &mut journal,
            &items,
            &tool,
            step,
            current,
        )
        .await;
    }

    // The reconnect the stale-session probe needs. The server drops a session
    // before it announces the leave, so the fixture's own line for it is also the
    // point after which the same player can connect again - and the server's live
    // lookup, read through the very handle the plugin resolves identities with,
    // must no longer answer that identity.
    drop(client);
    wait_for_leave(&mut running.log).await;
    let identity = mc_net::offline_uuid(PLAYER).to_string();
    assert!(
        running.sessions.session_of(&identity).is_none(),
        "the server still holds the connection this test just dropped"
    );
    let mut client = login(running.address, PLAYER).await;
    let rejoined = wait_for_ready(&mut client, PLAYER).await;
    assert_inventory(&rejoined, &items, &tool, current);

    for step in AFTER_REJOIN {
        current = run_step(
            &mut client,
            &mut running.log,
            &mut journal,
            &items,
            &tool,
            step,
            current,
        )
        .await;
    }

    // The durable side: the server's own save path, then the world's file.
    let report = running.save.save_all().await;
    assert!(report.is_ok(), "the save path failed: {report:?}");

    drop(client);
    let errors = error_lines(running.stop().await);
    assert!(
        errors.is_empty(),
        "the fixture reported a step it could not conclude: {errors:?}"
    );

    // The player state the save path left on disk: the last action committed both
    // sides, so the emeralds the purchases spent and the apples they granted are
    // what the file holds, and the tool is still exactly the stack it was.
    assert_saved_inventory(
        world.path(),
        (0, 2),
        "two purchases spent the four emeralds, and the refusals spent nothing",
    );
}

/// Check the player state the save path left on disk: the item totals the run
/// ended with, and the one component-bearing tool the player brought, still
/// exactly the stack it was.
///
/// Reading the world's own file is the last step of both tests: it is the state no
/// client observed, written by the server's own save path, so a total that only
/// ever appeared on the wire would not survive this.
fn assert_saved_inventory(world_dir: &Path, totals: (i32, i32), why: &str) {
    let saved = saved_inventory(world_dir);
    assert_eq!(
        saved_total(&saved, EMERALD_ITEM),
        totals.0,
        "the emeralds the saved player state holds: {why}"
    );
    assert_eq!(
        saved_total(&saved, APPLE_ITEM),
        totals.1,
        "the apples the saved player state holds: {why}"
    );
    let held = saved
        .iter()
        .find(|fields| text_field(fields, "id").as_deref() == Some(TOOL_ITEM))
        .expect("the saved player state no longer holds the unaffected tool");
    assert_eq!(
        int_field(held, "count"),
        1,
        "the unaffected tool's count must survive every action"
    );
    let components = held
        .iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("components", Tag::Compound(fields)) => Some(fields.as_slice()),
            _ => None,
        })
        .expect("the saved tool must still carry its components");
    assert_eq!(
        field(components, "minecraft:damage"),
        Some(&Tag::Int(TOOL_DAMAGE)),
        "the saved tool's damage component must be the one it carried"
    );
    assert_eq!(
        field(components, "minecraft:custom_name"),
        Some(&Tag::Compound(vec![(
            "text".to_owned(),
            Tag::String(TOOL_NAME.to_owned())
        )])),
        "the saved tool's name component must be the one it carried"
    );
    assert_eq!(
        field(components, "minecraft:enchantments"),
        Some(&Tag::Compound(vec![(
            TOOL_ENCHANTMENT.to_owned(),
            Tag::Int(TOOL_ENCHANTMENT_LEVEL)
        )])),
        "the saved tool's enchantments component must be the one it carried"
    );
}

/// The error lines one run's guests logged, as a test reports them: a guest that
/// could not conclude a step logs one instead of a marker, so a run with any of
/// them did not finish the sequence it was asked for.
fn error_lines(lines: Vec<LogLine>) -> Vec<String> {
    lines
        .into_iter()
        .filter(|line| line.error)
        .map(|line| format!("{}: {}", line.plugin, line.message))
        .collect()
}

/// The four clicks a script menu can report, one per action they mean: the plain
/// and the shifted primary click buy, the plain and the shifted secondary click
/// refund. Every one of them runs the same transaction the command-driven sequence
/// runs, so a click this table drives is a purchase or a refund a consumer of the
/// server can see.
const CLICKS: [Click; 4] = [
    Click {
        input: ContainerInput::Pickup,
        button: 0,
        marker: "P3_TRADE buy committed ledger=1",
        inventory: Some((2, 1)),
    },
    Click {
        input: ContainerInput::QuickMove,
        button: 0,
        marker: "P3_TRADE buy committed ledger=2",
        inventory: Some((0, 2)),
    },
    Click {
        input: ContainerInput::Pickup,
        button: 1,
        marker: "P3_TRADE refund committed ledger=1",
        inventory: Some((2, 1)),
    },
    Click {
        input: ContainerInput::QuickMove,
        button: 1,
        marker: "P3_TRADE refund committed ledger=0",
        inventory: Some((4, 0)),
    },
];

/// The server-owned market, driven over the wire.
///
/// The fixture's own commands open two menus and close one; a real client reads the
/// screen and content frames the server publishes and clicks the fixed slots with
/// the window and the revision it was shown; and the four clicks a script menu can
/// report each buy or refund through the same transaction the other test drives.
///
/// What a menu adds to that transaction is routing, and this test drives each
/// boundary the routing owns: a click on a window and a revision the server no
/// longer holds, a close naming a menu the server is not holding for this plugin,
/// and a menu request for a connection a reconnect left behind. Every one of those
/// is followed by a click that trades, which is what makes them mean something: a
/// stale or foreign operation that had been applied would have moved the ledger the
/// next marker names.
#[tokio::test]
async fn one_component_serves_a_real_menu_whose_clicks_trade_through_the_real_owners() {
    let world = tempfile::tempdir().expect("one temporary persistent world");
    std::fs::create_dir_all(world.path().join("region")).expect("world region directory");
    let registries = Registries::new();
    seed_player_state(world.path());

    // Two real packages: the one that owns the market, and one that subscribes to
    // the click event and owns no menu at all.
    let mut running = Running::start(
        &registries,
        world.path(),
        &[
            Package {
                id: OWNER,
                manifest: MANIFEST,
                config: TRADE_CONFIG,
            },
            Package {
                id: OBSERVER,
                manifest: OBSERVER_MANIFEST,
                config: OBSERVER_CONFIG,
            },
        ],
        "inventory menu acceptance",
    )
    .await;
    let items = Arc::clone(&registries.items);
    let tool = expected_tool(&items);
    let mut journal = Journal::default();

    let mut client = login(running.address, PLAYER).await;
    let ready = wait_for_ready(&mut client, PLAYER).await;
    assert_inventory(&ready, &items, &tool, (SEED_EMERALDS, 0));

    // The observer is live and its own line arrives. This is the control that makes
    // its silence about the market's clicks a result rather than a package that
    // never started, and the observer owns no menu, so nothing it does can open a
    // screen.
    command(&mut client, "menu-fence").await;
    let seen = wait_for_marker(
        &mut client,
        &mut running.log,
        &mut journal,
        "menu-fence",
        MENU_FENCE,
        Wants::Marker,
    )
    .await;
    assert_eq!(seen.screens, 0, "the observer opened a screen");

    // The four clicks a script menu reports, each one on a menu this fixture just
    // opened: a click that buys or refunds closes the menu it was clicked in, so
    // every action of this loop opens its own.
    let mut current = (SEED_EMERALDS, 0);
    for click in CLICKS {
        let window = open_menu(
            &mut client,
            &mut running.log,
            &mut journal,
            &items,
            "menu",
            MENU_OPEN_REQUESTED,
            MENU_MARKET_TITLE,
        )
        .await;
        current = click_trade(
            &mut client,
            &mut running.log,
            &mut journal,
            (&items, &tool),
            &window,
            &click,
            current,
        )
        .await;
    }

    // The close button and the close action ask for the same thing, and the server
    // answers both with its own close of the window it allocated.
    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu",
        MENU_OPEN_REQUESTED,
        MENU_MARKET_TITLE,
    )
    .await;
    click_menu(
        &mut client,
        &window,
        window.claims(),
        MENU_CLOSE_SLOT,
        ContainerInput::Pickup,
        0,
    )
    .await;
    wait_for_marker(
        &mut client,
        &mut running.log,
        &mut journal,
        "close-button click",
        MENU_CLOSE_REQUESTED,
        Wants::Close(window.container_id),
    )
    .await;

    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu",
        MENU_OPEN_REQUESTED,
        MENU_MARKET_TITLE,
    )
    .await;
    command(&mut client, "trade menu-close").await;
    wait_for_marker(
        &mut client,
        &mut running.log,
        &mut journal,
        "menu-close",
        MENU_CLOSE_REQUESTED,
        Wants::Close(window.container_id),
    )
    .await;

    // Two purchases that commit, then the one the player cannot afford: the
    // emeralds are gone, so the transaction is refused and the marker names the
    // ledger the refusal left exactly where it was. Each step opens its own menu,
    // because the click closes the one it traded in whether or not it committed.
    for (marker, inventory) in [
        ("P3_TRADE buy committed ledger=1", Some((2, 1))),
        ("P3_TRADE buy committed ledger=2", Some((0, 2))),
        ("P3_TRADE buy refused ledger=2", None),
    ] {
        let window = open_menu(
            &mut client,
            &mut running.log,
            &mut journal,
            &items,
            "menu",
            MENU_OPEN_REQUESTED,
            MENU_MARKET_TITLE,
        )
        .await;
        let click = Click {
            input: ContainerInput::Pickup,
            button: 0,
            marker,
            inventory,
        };
        current = click_trade(
            &mut client,
            &mut running.log,
            &mut journal,
            (&items, &tool),
            &window,
            &click,
            current,
        )
        .await;
    }

    // A refund puts two emeralds back, which is what the stale probes below spend.
    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu",
        MENU_OPEN_REQUESTED,
        MENU_MARKET_TITLE,
    )
    .await;
    let refund = Click {
        input: ContainerInput::Pickup,
        button: 1,
        marker: "P3_TRADE refund committed ledger=1",
        inventory: Some((2, 1)),
    };
    current = click_trade(
        &mut client,
        &mut running.log,
        &mut journal,
        (&items, &tool),
        &window,
        &refund,
        current,
    )
    .await;

    // A click on a revision and a click on a window the server is not holding. Both
    // name a window the client was really shown, so both are packets a client can
    // send; neither may reach the plugin, and the purchase that follows is the
    // proof - its marker names the ledger this run has just reached, so a stale
    // click that had been applied would have changed that line.
    let previous = *journal
        .closed
        .last()
        .expect("the sequence above closed a window");
    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu",
        MENU_OPEN_REQUESTED,
        MENU_MARKET_TITLE,
    )
    .await;
    assert_ne!(
        previous, window.container_id,
        "the server reused a window id, so a click naming the closed window would not be stale"
    );
    // Use refunds for the stale clicks: accepting either must not masquerade as
    // the valid purchase's identical marker, inventory and window close.
    click_menu(
        &mut client,
        &window,
        (window.container_id, window.state_id + 1),
        MENU_BUY_SLOT,
        ContainerInput::Pickup,
        1,
    )
    .await;
    click_menu(
        &mut client,
        &window,
        (previous, window.state_id),
        MENU_BUY_SLOT,
        ContainerInput::Pickup,
        1,
    )
    .await;
    let after_stale = Click {
        input: ContainerInput::Pickup,
        button: 0,
        marker: "P3_TRADE buy committed ledger=2",
        inventory: Some((0, 2)),
    };
    current = click_trade(
        &mut client,
        &mut running.log,
        &mut journal,
        (&items, &tool),
        &window,
        &after_stale,
        current,
    )
    .await;

    // A close naming the first market while the second one is what this plugin
    // holds. The server matches a close against the window it holds for that very
    // plugin, player and menu, so it publishes the content again instead of
    // closing - and the refund clicked afterwards is what proves the window was
    // still there and still this plugin's.
    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu-next",
        MENU_NEXT_REQUESTED,
        MENU_NEXT_TITLE,
    )
    .await;
    command(&mut client, "trade stale-menu-close").await;
    wait_for_marker(
        &mut client,
        &mut running.log,
        &mut journal,
        "stale-menu-close",
        MENU_STALE_CLOSE_REQUESTED,
        Wants::Marker,
    )
    .await;
    let refund = Click {
        input: ContainerInput::Pickup,
        button: 1,
        marker: "P3_TRADE refund committed ledger=1",
        inventory: Some((2, 1)),
    };
    current = click_trade(
        &mut client,
        &mut running.log,
        &mut journal,
        (&items, &tool),
        &window,
        &refund,
        current,
    )
    .await;

    // The reconnect the old-session probe needs. The server drops a session before
    // it announces the leave, so the fixture's own line for it is also the point
    // after which the same player can connect again.
    drop(client);
    wait_for_leave(&mut running.log).await;
    let identity = mc_net::offline_uuid(PLAYER).to_string();
    assert!(
        running.sessions.session_of(&identity).is_none(),
        "the server still holds the connection this test just dropped"
    );
    let mut client = login(running.address, PLAYER).await;
    let rejoined = wait_for_ready(&mut client, PLAYER).await;
    assert_inventory(&rejoined, &items, &tool, current);

    // The old-session probe: the fixture asks for the market, and for a close of
    // the second one, on the connection its first command named. Both name a
    // connection the server has finished with, so neither reaches this one - the
    // marker is the evidence that the request was made, and the screens this wait
    // read are the evidence that nothing arrived for it.
    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu-next",
        MENU_NEXT_REQUESTED,
        MENU_NEXT_TITLE,
    )
    .await;
    command(&mut client, "trade stale-session-menu").await;
    let seen = wait_for_marker(
        &mut client,
        &mut running.log,
        &mut journal,
        "stale-session-menu",
        MENU_STALE_SESSION_REQUESTED,
        Wants::Marker,
    )
    .await;
    assert_eq!(
        seen.screens, 0,
        "a menu request for a connection that has gone reached this one"
    );
    // The live connection still serves its own window: this click is delivered to
    // it and its transaction commits on the session this client holds, which is
    // also what proves the stale requests above closed and replaced nothing.
    let live = Click {
        input: ContainerInput::Pickup,
        button: 0,
        marker: "P3_TRADE buy committed ledger=2",
        inventory: Some((0, 2)),
    };
    current = click_trade(
        &mut client,
        &mut running.log,
        &mut journal,
        (&items, &tool),
        &window,
        &live,
        current,
    )
    .await;

    // A stale transaction still closes the fixture's tracked live menu on that
    // menu's session, not on the dead session addressed by the transaction.
    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu-next",
        MENU_NEXT_REQUESTED,
        MENU_NEXT_TITLE,
    )
    .await;
    trade(&mut client, "stale-session").await;
    wait_for_marker(
        &mut client,
        &mut running.log,
        &mut journal,
        "stale transaction with a live menu",
        "P3_TRADE stale-session refused ledger=2",
        Wants::Close(window.container_id),
    )
    .await;
    let window = open_menu(
        &mut client,
        &mut running.log,
        &mut journal,
        &items,
        "menu-next",
        MENU_NEXT_REQUESTED,
        MENU_NEXT_TITLE,
    )
    .await;
    current = click_trade(
        &mut client,
        &mut running.log,
        &mut journal,
        (&items, &tool),
        &window,
        &refund,
        current,
    )
    .await;

    // A click reached the plugin that opened the menu and nobody else. The observer
    // is subscribed to the event and answered its own command, so a click delivered
    // to every subscriber would have published the line it answers one with.
    assert!(
        !journal.saw_line_with_prefix(MENU_LEAK_PREFIX),
        "a click was delivered to a plugin that owns no menu: {:?}",
        journal.chats
    );

    // The durable side: the server's own save path, then the world's file. The
    // totals are the ones this run's own markers named, so the file has to agree
    // with what the wire reported before the server was stopped.
    let report = running.save.save_all().await;
    assert!(report.is_ok(), "the save path failed: {report:?}");
    drop(client);
    let errors = error_lines(running.stop().await);
    assert!(
        errors.is_empty(),
        "a fixture reported a step it could not conclude: {errors:?}"
    );
    assert_saved_inventory(
        world.path(),
        current,
        "four clicks traded, the closes and refusals moved nothing, and the stale probes changed nothing",
    );
}

/// One staged transaction record, in the contract's own records, for the checks
/// that never reach a server.
fn transaction(
    inventory: Vec<InventoryResourceDelta>,
    storage: Vec<StorageMutation>,
) -> WireCommand {
    WireCommand::InventoryStorageTransaction(InventoryStorageTransaction {
        request: "trade-txn".to_owned(),
        session: UNKNOWN_SESSION,
        inventory,
        storage,
    })
}

/// One resource delta of a staged transaction.
fn delta(resource: &str, delta: i16) -> InventoryResourceDelta {
    InventoryResourceDelta {
        resource: resource.to_owned(),
        delta,
    }
}

/// One staged menu open, in the contract's own records, for the checks that never
/// reach a server.
fn menu_open(id: &str, title: &str, slots: Vec<InventoryMenuSlot>) -> WireCommand {
    WireCommand::OpenInventoryMenu(OpenInventoryMenu {
        session: UNKNOWN_SESSION,
        menu: InventoryMenu {
            id: id.to_owned(),
            title: title.to_owned(),
            slots,
        },
    })
}

/// One staged menu close, in the contract's own records.
fn menu_close(menu: &str) -> WireCommand {
    WireCommand::CloseInventoryMenu(CloseInventoryMenu {
        session: UNKNOWN_SESSION,
        menu: menu.to_owned(),
    })
}

/// One slot of a staged menu.
fn slot(index: u8, resource: &str, count: u8, label: Option<&str>) -> InventoryMenuSlot {
    InventoryMenuSlot {
        slot: index,
        resource: resource.to_owned(),
        count,
        label: label.map(str::to_owned),
    }
}

/// One compare-and-swap mutation of a staged transaction.
fn cas(key: &str, expected_version: Option<u64>, value: &str) -> StorageMutation {
    StorageMutation::Cas(StorageCasMutation {
        key: key.to_owned(),
        expected_version,
        value: value.to_owned(),
    })
}

/// The bound every staged batch of these checks is admitted against.
fn batch_limit() -> NonZeroUsize {
    NonZeroUsize::new(mc_script::MAX_SCRIPT_COMMAND_BATCH).expect("non-zero")
}

/// The capabilities a manifest that declared the transaction capability resolves
/// to, which is what the host pre-checks a callback's answer against.
fn granted_transaction_capabilities() -> CommandCapabilities {
    ScriptPluginManifest::new(
        OWNER,
        "Inventory Compat",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_inventory_storage_transactions()
    .validate_for(mc_script::COMPONENT_PLUGIN_API_VERSION)
    .expect("the fixture's own declaration validates")
    .to_command_capabilities()
}

#[test]
fn a_transaction_without_its_grant_is_refused_by_the_capability_it_needs() {
    let limits = PluginLimits::default();
    let mut batch = CommandBatch::new();
    batch
        .push(
            transaction(
                vec![delta(EMERALD_ITEM, -2)],
                vec![cas(LEDGER_KEY, None, "1")],
            ),
            &limits,
        )
        .expect("the record stages");
    // The record converts; what refuses it is the grant its manifest would have
    // had to declare, and the refusal names that grant.
    assert_eq!(
        to_script_batch(
            batch,
            batch_limit(),
            &NoSessions,
            &CommandCapabilities::none()
        ),
        Err(AdapterError::PermissionDenied {
            capability: "inventory_storage_transactions"
        }),
    );
}

#[test]
fn a_malformed_transaction_refuses_the_whole_batch_it_arrives_in() {
    let limits = PluginLimits::default();
    let granted = granted_transaction_capabilities();

    // A transaction with no storage mutation is not one the server's DTO admits.
    // The command staged before it is never converted, so no part of that answer
    // can be applied - which is the whole-batch rule this record has to obey.
    let mut batch = CommandBatch::new();
    batch
        .push(
            WireCommand::SendMessage(SendMessage {
                target: MessageTarget::Session(UNKNOWN_SESSION),
                text: "earlier".to_owned(),
            }),
            &limits,
        )
        .expect("the message stages");
    batch
        .push(
            transaction(vec![delta(EMERALD_ITEM, -2)], Vec::new()),
            &limits,
        )
        .expect("the record stages");
    let Err(refused) = to_script_batch(batch, batch_limit(), &NoSessions, &granted) else {
        panic!("a transaction with no storage mutation must be refused");
    };
    assert!(
        matches!(refused, AdapterError::InvalidCommand { .. }),
        "a transaction with no storage mutation was refused as {refused:?}"
    );

    // The same for an amount the contract does not admit: a delta of zero changes
    // no count, so the record is refused before any owner sees it.
    let mut batch = CommandBatch::new();
    batch
        .push(
            transaction(
                vec![delta(EMERALD_ITEM, 0)],
                vec![cas(LEDGER_KEY, None, "1")],
            ),
            &limits,
        )
        .expect("the record stages");
    let Err(refused) = to_script_batch(batch, batch_limit(), &NoSessions, &granted) else {
        panic!("a transaction with a zero delta must be refused");
    };
    assert!(
        matches!(refused, AdapterError::InvalidCommand { .. }),
        "a transaction with a zero delta was refused as {refused:?}"
    );

    // The control: the same shape with one legal delta and one legal mutation
    // converts under the same grant, so the two refusals above are the record's
    // own and not a grant this test got wrong.
    let mut batch = CommandBatch::new();
    batch
        .push(
            transaction(
                vec![delta(EMERALD_ITEM, -2)],
                vec![cas(LEDGER_KEY, None, "1")],
            ),
            &limits,
        )
        .expect("the record stages");
    to_script_batch(batch, batch_limit(), &NoSessions, &granted)
        .expect("the fixture's own transaction converts");
}

/// The capabilities a manifest that declared the menu capability resolves to, which
/// is what the host pre-checks a callback's answer against.
fn granted_menu_capabilities() -> CommandCapabilities {
    ScriptPluginManifest::new(
        OWNER,
        "Inventory Compat",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_inventory_menus()
    .validate_for(mc_script::COMPONENT_PLUGIN_API_VERSION)
    .expect("the fixture's own declaration validates")
    .to_command_capabilities()
}

#[test]
fn a_menu_command_without_its_grant_is_refused_by_the_capability_it_needs() {
    let limits = PluginLimits::default();
    let mut batch = CommandBatch::new();
    batch
        .push(
            menu_open(
                MENU_MARKET,
                MENU_MARKET_TITLE,
                vec![slot(0, APPLE_ITEM, 1, Some(APPLE_LABEL))],
            ),
            &limits,
        )
        .expect("the record stages");
    // The record converts - the wire checks drive it with a session that never
    // existed, which the conversion does not look up - so what refuses it is the
    // grant its manifest would have had to declare, and the refusal names that
    // grant.
    assert_eq!(
        to_script_batch(
            batch,
            batch_limit(),
            &NoSessions,
            &CommandCapabilities::none()
        ),
        Err(AdapterError::PermissionDenied {
            capability: "inventory_menus"
        }),
    );
}

#[test]
fn a_malformed_menu_refuses_the_whole_batch_it_arrives_in() {
    let limits = PluginLimits::default();
    let granted = granted_menu_capabilities();

    // Two slots on one index is not a menu the server's own DTO admits: the client
    // could not tell which button it was shown. The message staged before it is
    // never converted, so no part of that answer can be applied - the same
    // whole-batch rule every command of a callback obeys.
    let mut batch = CommandBatch::new();
    batch
        .push(
            WireCommand::SendMessage(SendMessage {
                target: MessageTarget::Session(UNKNOWN_SESSION),
                text: "earlier".to_owned(),
            }),
            &limits,
        )
        .expect("the message stages");
    batch
        .push(
            menu_open(
                MENU_MARKET,
                MENU_MARKET_TITLE,
                vec![slot(0, APPLE_ITEM, 1, None), slot(0, BARRIER_ITEM, 1, None)],
            ),
            &limits,
        )
        .expect("the record stages");
    let Err(refused) = to_script_batch(batch, batch_limit(), &NoSessions, &granted) else {
        panic!("a menu with two slots on one index must be refused");
    };
    assert!(
        matches!(refused, AdapterError::InvalidCommand { .. }),
        "a menu with two slots on one index was refused as {refused:?}"
    );

    // The same for a button that shows nothing: a slot whose count is zero is not
    // an item the client can click.
    let mut batch = CommandBatch::new();
    batch
        .push(
            menu_open(
                MENU_MARKET,
                MENU_MARKET_TITLE,
                vec![slot(0, APPLE_ITEM, 0, None)],
            ),
            &limits,
        )
        .expect("the record stages");
    let Err(refused) = to_script_batch(batch, batch_limit(), &NoSessions, &granted) else {
        panic!("a menu with an empty slot must be refused");
    };
    assert!(
        matches!(refused, AdapterError::InvalidCommand { .. }),
        "a menu with an empty slot was refused as {refused:?}"
    );

    // The same for a close naming something a menu id cannot be: the contract
    // bounds an id, so a longer one is the plugin's own malformed answer.
    let mut batch = CommandBatch::new();
    batch
        .push(
            menu_close(&"x".repeat(mc_script::MAX_SCRIPT_ID_BYTES + 1)),
            &limits,
        )
        .expect("the record stages");
    let Err(refused) = to_script_batch(batch, batch_limit(), &NoSessions, &granted) else {
        panic!("a close naming an over-long menu id must be refused");
    };
    assert!(
        matches!(refused, AdapterError::InvalidCommand { .. }),
        "a close naming an over-long menu id was refused as {refused:?}"
    );

    // The control: the same shapes inside their bounds convert under the same
    // grant, so the refusals above are the records' own and not a grant this test
    // got wrong.
    let mut batch = CommandBatch::new();
    batch
        .push(
            menu_open(
                MENU_MARKET,
                MENU_MARKET_TITLE,
                vec![
                    slot(0, APPLE_ITEM, 1, Some(APPLE_LABEL)),
                    slot(8, BARRIER_ITEM, 1, Some(BARRIER_LABEL)),
                ],
            ),
            &limits,
        )
        .expect("the record stages");
    batch
        .push(menu_close(MENU_NEXT), &limits)
        .expect("the record stages");
    to_script_batch(batch, batch_limit(), &NoSessions, &granted)
        .expect("the fixture's own menu commands convert");
}

/// Log the fixture's player in over a fresh connection and return the client, once
/// the server has published the play entry, the command tree and the position the
/// client acknowledges. Every connection of this test is a real one: nothing here
/// is an operator shortcut.
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
}

/// Wait for the two things a join of this fixture publishes before any action: the
/// greeting it sends the joining player, which is the readiness a driver waits
/// for, and the authoritative inventory the login carries.
///
/// Both are read off the wire, in whichever order the server writes them.
async fn wait_for_ready(client: &mut Client, name: &str) -> ClientboundContainerSetContent {
    let greeting = format!("{GREETING} {name}");
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    let mut inventory = None;
    let mut greeted = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("the join of {name} never published {greeting:?} and its inventory");
        }
        let mut frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|error| {
                panic!("the join of {name} never published its readiness: {error}")
            });
        if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
            if literal_text_component_text(&chat.content_nbt) == greeting {
                greeted = true;
            }
        } else if frame.id == ClientboundContainerSetContent::ID {
            let content = ClientboundContainerSetContent::decode(&mut frame.body)
                .expect("decode authoritative inventory");
            if content.container_id == 0 {
                inventory = Some(content);
            }
        }
        if greeted && let Some(inventory) = inventory {
            return inventory;
        }
    }
}

/// What one wait must have observed before it may return.
#[derive(Clone, Copy)]
enum Wants {
    /// The guest's own line for the step, and nothing else.
    Marker,
    /// The guest's own line and the screen its command opened.
    Menu,
    /// The guest's own line and the close of one window.
    Close(i32),
}

/// Everything a run observed while it drove its client: the chat lines the guests
/// published and the windows the server closed.
///
/// A wait appends here rather than answering with each line separately, because
/// the commands of one admitted batch reach a client through more than one
/// publication lane: the menu frames and the chat lines are written by the same
/// connection task, but the order those lanes delivered them in is not something a
/// test may assume. Reading the whole journal once a run is done is what makes an
/// assertion like "that window was never closed" independent of that order, and
/// what makes a line this test never waits for - a leaked click, say - impossible
/// to miss.
#[derive(Default)]
struct Journal {
    /// Every system chat line the client read, in the order it read them.
    chats: Vec<String>,
    /// Every container the server closed, in the order it closed them.
    closed: Vec<i32>,
}

impl Journal {
    /// Whether any line read so far starts with `prefix`.
    fn saw_line_with_prefix(&self, prefix: &str) -> bool {
        self.chats.iter().any(|text| text.starts_with(prefix))
    }
}

/// One menu, as the server published it to its client: the window it allocated for
/// this open, the revision the client has to name back when it clicks, and what
/// the client was shown in it.
struct MenuWindow {
    container_id: i32,
    state_id: i32,
    /// The screen type the open frame carried: a 9xN generic container, whose type
    /// is its row count less one.
    menu_type: i32,
    title: String,
    items: Vec<ItemStack>,
}

impl MenuWindow {
    /// The window and revision a client names when it clicks this menu.
    fn claims(&self) -> (i32, i32) {
        (self.container_id, self.state_id)
    }
}

/// What one wait saw of the step it was watching.
#[derive(Default)]
struct Observed {
    /// The last authoritative inventory the step published, if it published one.
    inventory: Option<ClientboundContainerSetContent>,
    /// The menu the step opened, with the content frame that matched its screen.
    menu: Option<MenuWindow>,
    /// How many screens this wait read. A step that opens one menu reads exactly
    /// one, so a menu request that wrongly reached this connection would show up
    /// here.
    screens: usize,
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

/// Send one `trade` action and nothing else: what the action publishes is what the
/// wait that follows reads.
async fn trade(client: &mut Client, action: &str) {
    command(client, &format!("trade {action}")).await;
}

/// One action of the sequence: send it, and read what the fixture reports for it.
async fn run_step(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
    items: &mc_data::items::ItemRegistry,
    tool: &ItemStack,
    step: &Step,
    current: (i32, i32),
) -> (i32, i32) {
    trade(client, step.action).await;
    let seen = wait_for_marker(
        client,
        log,
        journal,
        step.action,
        step.marker,
        Wants::Marker,
    )
    .await;
    let expected = step.inventory.unwrap_or(current);
    match (&step.inventory, &seen.inventory) {
        (Some(_), None) => panic!(
            "the {} action committed without publishing the inventory it changed",
            step.action
        ),
        (_, Some(content)) => assert_inventory(content, items, tool, expected),
        (None, None) => {}
    }
    expected
}

/// Ask for one menu and read what the server published for it.
///
/// The fixture's line is only a request, so the server's own frames are what this
/// checks: the window it allocated, the title it shows, the two buttons the
/// fixture asked for with the labels it chose, and the player inventory the server
/// appends after them.
async fn open_menu(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
    items: &mc_data::items::ItemRegistry,
    action: &str,
    marker: &str,
    title: &str,
) -> MenuWindow {
    trade(client, action).await;
    let seen = wait_for_marker(client, log, journal, action, marker, Wants::Menu).await;
    assert_eq!(
        seen.screens, 1,
        "the {action} action published {} screens",
        seen.screens
    );
    let window = seen
        .menu
        .unwrap_or_else(|| panic!("the {action} action opened no menu"));
    assert_eq!(
        window.title, title,
        "the title of the menu the {action} action opened"
    );
    assert_eq!(
        window.menu_type, 0,
        "the screen type of the menu the {action} action opened: one row of nine slots"
    );
    assert_eq!(
        window.items.len(),
        MENU_ITEMS,
        "the items of the menu the {action} action opened"
    );
    assert_slot(&window, items, MENU_BUY_SLOT, APPLE_ITEM, APPLE_LABEL);
    assert_slot(&window, items, MENU_CLOSE_SLOT, BARRIER_ITEM, BARRIER_LABEL);
    window
}

/// Click one slot of a script menu, naming the window and the revision the client
/// believes it is looking at.
///
/// Every click this file sends goes through here, so a stale click is the same
/// packet naming a window or a revision the server no longer holds, and the item
/// the client names back is the one the server published at that slot.
async fn click_menu(
    client: &mut Client,
    window: &MenuWindow,
    claims: (i32, i32),
    slot: i16,
    input: ContainerInput,
    button: i8,
) {
    let held = &window.items[usize::try_from(slot).expect("the slot is a wire index")];
    client
        .write_packet(&ServerboundContainerClick {
            container_id: claims.0,
            state_id: claims.1,
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
        .expect("send script menu click");
}

/// One click on the market's purchase button, and what the fixture must report for
/// the transaction it starts.
#[derive(Clone, Copy)]
struct Click {
    /// The click input and the button the client holds: the four combinations a
    /// server-owned menu reports.
    input: ContainerInput,
    button: i8,
    /// The line the fixture must report for the transaction the click starts. It
    /// carries the ledger count the fixture itself checked, so a click that traded
    /// when it should not have fails on this line.
    marker: &'static str,
    /// The inventory a committed transaction must leave. A refused one publishes
    /// no snapshot, so it names none.
    inventory: Option<(i32, i32)>,
}

/// Click the purchase button of an open menu and check what the transaction it
/// started ended in: the fixture's own line, the authoritative inventory the same
/// commit published, and the close of the menu it traded in.
async fn click_trade(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
    (items, tool): (&mc_data::items::ItemRegistry, &ItemStack),
    window: &MenuWindow,
    click: &Click,
    current: (i32, i32),
) -> (i32, i32) {
    click_menu(
        client,
        window,
        window.claims(),
        MENU_BUY_SLOT,
        click.input,
        click.button,
    )
    .await;
    let seen = wait_for_marker(
        client,
        log,
        journal,
        "click",
        click.marker,
        Wants::Close(window.container_id),
    )
    .await;
    assert!(
        journal.closed.contains(&window.container_id),
        "the menu a click traded in was never closed on the wire"
    );
    let expected = click.inventory.unwrap_or(current);
    match (&click.inventory, &seen.inventory) {
        (Some(_), None) => panic!(
            "a click committed without publishing the inventory {} reports",
            click.marker
        ),
        (_, Some(content)) => assert_inventory(content, items, tool, expected),
        (None, None) => {}
    }
    expected
}

/// Check one fixed button of an open menu: the item the client was shown at that
/// slot, its count, and the label the server published with it.
fn assert_slot(
    window: &MenuWindow,
    items: &mc_data::items::ItemRegistry,
    slot: i16,
    resource: &str,
    label: &str,
) {
    let held = &window.items[usize::try_from(slot).expect("the slot is a wire index")];
    assert_eq!(
        held.item_id,
        item_id(items, resource),
        "the item at slot {slot}"
    );
    assert_eq!(held.count, 1, "the count at slot {slot}");
    assert_eq!(
        held.custom_name.as_deref(),
        Some(label),
        "the label at slot {slot}"
    );
}

/// Wait for the fixture's own report that the connection it traded on has gone.
///
/// The server deregisters a session before it announces the leave, so the line
/// this waits for is the point after which the same player can log in again.
async fn wait_for_leave(log: &mut UnboundedReceiver<LogLine>) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let line = tokio::time::timeout(remaining, log.recv())
            .await
            .expect("the fixture never reported the traded connection leaving");
        let Some(line) = line else {
            panic!("the component host left before the fixture reported the session leaving");
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

/// Read one step's frames and the guests' lines until the step has published
/// everything it is expected to, failing on the fixture's error line first.
///
/// The wait ends on a line or a frame a guest produced, never on a sleep: a marker
/// that never arrives is the step stalling, and the error line - which the fixture
/// logs instead of a marker - is the most specific failure a test can report. What
/// the wait read is appended to the journal, so a line or a window this step was
/// not waiting for is still there for the run to assert on.
async fn wait_for_marker(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
    step: &str,
    marker: &str,
    wants: Wants,
) -> Observed {
    let deadline = tokio::time::Instant::now() + ACTION_TIMEOUT;
    // Both are read from the journal: what this wait may stop on is what arrived
    // while it waited, never a line or a close an earlier step already produced.
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
            && match wants {
                Wants::Marker => true,
                Wants::Menu => observed.menu.is_some(),
                Wants::Close(id) => journal.closed[first_close..].contains(&id),
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
                        Some((container_id, screen)) if container_id == content.container_id => {
                            observed.menu = Some(MenuWindow {
                                container_id,
                                state_id: content.state_id,
                                menu_type: screen.menu_type,
                                title: literal_text_component_text(&screen.title_nbt),
                                items: content.items,
                            });
                        }
                        Some(other) => pending = Some(other),
                        // Any other content frame is either the authoritative
                        // inventory - the server's own container - or a resync of a
                        // menu this step is not opening.
                        None => {
                            if content.container_id == 0 {
                                observed.inventory = Some(content);
                            }
                        }
                    }
                } else if frame.id == ClientboundOpenScreen::ID {
                    let screen = ClientboundOpenScreen::decode(&mut frame.body)
                        .expect("decode script menu screen");
                    observed.screens += 1;
                    pending = Some((screen.container_id, screen));
                } else if frame.id == ClientboundContainerClose::ID {
                    let close = ClientboundContainerClose::decode(&mut frame.body)
                        .expect("decode container close");
                    journal.closed.push(close.container_id);
                } else if frame.id == ClientboundSystemChat::ID {
                    let chat = ClientboundSystemChat::decode(&mut frame.body)
                        .expect("decode system chat");
                    journal.chats.push(literal_text_component_text(&chat.content_nbt));
                }
            }
        }
    }
}

/// Check one authoritative inventory snapshot: the item totals the sequence
/// states, and the one component-bearing tool the player brought, still exactly
/// the stack it was.
fn assert_inventory(
    content: &ClientboundContainerSetContent,
    items: &mc_data::items::ItemRegistry,
    tool: &ItemStack,
    totals: (i32, i32),
) {
    assert_eq!(
        total_of(content, item_id(items, EMERALD_ITEM)),
        totals.0,
        "the emerald total the snapshot carries"
    );
    assert_eq!(
        total_of(content, item_id(items, APPLE_ITEM)),
        totals.1,
        "the apple total the snapshot carries"
    );
    let held = content
        .items
        .iter()
        .find(|item| item.item_id == tool.item_id);
    assert_eq!(
        held,
        Some(tool),
        "the unaffected tool must still be exactly the stack the player brought"
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

/// The stack the player's tool must be, components included.
fn expected_tool(items: &mc_data::items::ItemRegistry) -> ItemStack {
    let tool = ItemStack::new(item_id(items, TOOL_ITEM), 1)
        .with_damage(TOOL_DAMAGE)
        .with_custom_name(TOOL_NAME)
        .with_enchantment(
            Identifier::parse(TOOL_ENCHANTMENT).expect("checked enchantment identifier"),
            TOOL_ENCHANTMENT_LEVEL,
        );
    // A tool with no components would make every comparison below vacuous.
    assert!(
        tool.damage.is_some() && tool.custom_name.is_some() && !tool.enchantments.is_empty(),
        "the unaffected tool this test compares must carry components of its own"
    );
    tool
}

/// Write the world's own durable player file for `PLAYER`, the way a world an
/// operator has played in already carries that player.
///
/// This is the server's canonical player format - gzip of the named NBT the server
/// itself reads on login - so the four emeralds and the component-bearing tool are
/// state the server validates on load, not a value this test hands the guest.
/// Nothing here writes the plugin's storage: the fixture's ledger is the server's
/// own durable state and starts absent.
fn seed_player_state(world_dir: &Path) {
    let uuid = mc_net::offline_uuid(PLAYER);
    let path = world_dir.join("playerdata").join(format!("{uuid}.dat"));
    std::fs::create_dir_all(path.parent().expect("playerdata directory"))
        .expect("playerdata directory");
    let root = Tag::Compound(vec![(
        "Inventory".to_owned(),
        Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::COMPOUND,
            elements: vec![
                Tag::Compound(vec![
                    ("Slot".to_owned(), Tag::Byte(EMERALD_SLOT as i8)),
                    ("id".to_owned(), Tag::String(EMERALD_ITEM.to_owned())),
                    ("count".to_owned(), Tag::Int(SEED_EMERALDS)),
                ]),
                Tag::Compound(vec![
                    ("Slot".to_owned(), Tag::Byte(TOOL_SLOT as i8)),
                    ("id".to_owned(), Tag::String(TOOL_ITEM.to_owned())),
                    ("count".to_owned(), Tag::Int(1)),
                    ("components".to_owned(), tool_components()),
                ]),
            ],
        }),
    )]);
    let mut encoded = Vec::new();
    mc_nbt::write_named(&mut encoded, "", &root).expect("the player state encodes");
    let file = std::fs::File::create(&path).expect("player data file");
    let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    encoder.write_all(&encoded).expect("write the player state");
    encoder.finish().expect("finish the player state");
}

/// The components the player's tool carries in the world's file, in the form the
/// server's own player format stores them.
fn tool_components() -> Tag {
    Tag::Compound(vec![
        ("minecraft:damage".to_owned(), Tag::Int(TOOL_DAMAGE)),
        (
            "minecraft:custom_name".to_owned(),
            Tag::Compound(vec![("text".to_owned(), Tag::String(TOOL_NAME.to_owned()))]),
        ),
        (
            "minecraft:enchantments".to_owned(),
            Tag::Compound(vec![(
                TOOL_ENCHANTMENT.to_owned(),
                Tag::Int(TOOL_ENCHANTMENT_LEVEL),
            )]),
        ),
    ])
}

/// The inventory the world's own player file holds for `PLAYER`, as the entries
/// the server wrote.
fn saved_inventory(world_dir: &Path) -> Vec<Vec<(String, Tag)>> {
    let path = world_dir
        .join("playerdata")
        .join(format!("{}.dat", mc_net::offline_uuid(PLAYER)));
    let file = std::fs::File::open(&path).unwrap_or_else(|error| {
        panic!(
            "the save path left no player state at {}: {error}",
            path.display()
        )
    });
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(file)
        .read_to_end(&mut decoded)
        .expect("decompress the player state");
    let mut slice = decoded.as_slice();
    let (_, root) = mc_nbt::read_named(&mut slice).expect("read the player state");
    let Tag::Compound(fields) = root else {
        panic!("the saved player state root must be a compound");
    };
    let inventory = fields
        .into_iter()
        .find_map(|(name, tag)| (name == "Inventory").then_some(tag));
    let Some(Tag::List(list)) = inventory else {
        panic!("the saved player state holds no inventory list");
    };
    list.elements
        .into_iter()
        .filter_map(|element| match element {
            Tag::Compound(item) => Some(item),
            _ => None,
        })
        .collect()
}

/// How many items of one resource the saved inventory entries hold.
fn saved_total(entries: &[Vec<(String, Tag)>], resource: &str) -> i32 {
    entries
        .iter()
        .filter(|fields| text_field(fields, "id").as_deref() == Some(resource))
        .map(|fields| int_field(fields, "count"))
        .sum()
}

/// One field of one saved inventory entry, by name.
fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

/// One string field of one saved inventory entry.
fn text_field(fields: &[(String, Tag)], name: &str) -> Option<String> {
    match field(fields, name) {
        Some(Tag::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// One integer field of one saved inventory entry.
fn int_field(fields: &[(String, Tag)], name: &str) -> i32 {
    match field(fields, name) {
        Some(Tag::Int(value)) => *value,
        _ => 0,
    }
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
            .with_generator(Arc::new(mc_worldgen::TerrainGenerator::new(
                0,
                Arc::clone(&registries.blocks),
            )));
    Arc::new(tokio::sync::Mutex::new(storage))
}

/// The server configuration every run binds with: one persistent world, so the
/// real inventory and storage owners are started and their decisions are the ones
/// on disk.
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

/// The literal text of one system-chat frame, as the server's own component
/// encodes it.
fn literal_text_component_text(component: &[u8]) -> String {
    let mut bytes = Bytes::copy_from_slice(component);
    let tag = mc_nbt::read_network(&mut bytes).expect("read text component nbt");
    let Tag::Compound(fields) = tag else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("literal system chat text")
}
