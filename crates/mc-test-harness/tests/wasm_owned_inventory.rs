//! P3 owned-inventory acceptance through one real component and one real world.
//!
//! The component is the repository's own example plugin, built to
//! `wasm32-unknown-unknown` and encoded as a component exactly the way a published
//! package is; the world is a real directory the server opens, saves and reads
//! back. A real client logs in over the wire and runs the fixture's `inventory`
//! root one action at a time against the server's own inventory owners, and every
//! observation this test makes is one a consumer of the running server can make:
//! the authoritative inventory the server publishes, the line the fixture reports
//! to the player, and the player state the world's own file holds after the save
//! path ran.
//!
//! The player starts with four emeralds and one component-bearing tool, seeded
//! through the world's own durable player format before the server opens it - the
//! same file the server writes and validates on login, so the tool's components
//! are real state rather than a value this test hands the guest. The sequence then
//! proves, through the server's own typed answers:
//!
//! * a query reaches the player inventory owner and answers the whole canonical
//!   snapshot: every slot, with the fence a later mutation names;
//! * a transfer moves the component-bearing tool exactly once, conserves every
//!   other stack, and answers the resulting fence list - which the client can see
//!   because the commit publishes the authoritative inventory;
//! * the same durable operation id repeats byte-identically after a disconnect
//!   and a new session as a replay of the old session's recorded outcome - the
//!   replayed revision is the original commit's, never a second application;
//! * a status query retrieves that committed receipt without resubmitting the
//!   transfer, while the ended session's inventory endpoint refuses `not-found`;
//! * a committed answer held until after the player reconnects never sends its
//!   old-session report to the replacement, although the new session can query
//!   its durable outcome and observe the committed item effect;
//! * the same durable operation id with different content is refused
//!   `operation-conflict` rather than applied;
//! * a fence past the one read is refused `stale-revision`, another runtime
//!   player's inventory is refused `forbidden`, absent resident equipment/carry
//!   handles are refused `not-found`, and the warehouse query without an authored
//!   settlement runtime is refused `runtime-unavailable`, never an empty inventory;
//! * a reservation against the player inventory answers its whole state.
//!
//! The fixture checks each answer against what its own operation implies and only
//! then reports a marker; a step it could not conclude is an error log line
//! instead, and this test fails on any of them.
//!
//! A normal hotbar swap and its reversal precede the component transfer on the
//! same component-bearing tool. Both routes begin with the same inventory image
//! and must publish the same slot and component outcome; the component's
//! fenced transfer reads the owner snapshot after the ordinary client mutation.
//!
//! A second, non-network case pins the capability boundary the same way the
//! storage vertical does: the very record the fixture sends converts under a
//! manifest that declares `inventory_transfers` and is refused by *name* under one
//! that does not.

// Reuse the host tests' component builder.
#[path = "../../mc-plugin-host/tests/fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use mc_data::Identifier;
use mc_nbt::Tag;
use mc_plugin_host::bindings::solaris::plugin::commands::Command as WireCommand;
use mc_plugin_host::bindings::solaris::plugin::domain_operations::{
    DomainOperation, OperationRequest,
};
use mc_plugin_host::bindings::solaris::plugin::inventories::{
    InventoryEndpoint, InventoryExpectedRevision, InventoryFence, InventoryOperation,
    InventoryTransfer, OwnedItemTransfer,
};
use mc_plugin_host::{
    AdapterError, CommandBatch, DeploymentConfig, DiscoveryMode, HostQueues, HostServices,
    LogLevel, NoSessions, PlayerSessions, PluginHost, PluginLimits, discover,
    start_deployment_with, to_script_batch,
};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundContainerSetContent, ClientboundSystemChat,
    ConfirmTeleportation, ContainerInput, HashedStack, ItemStack, ServerboundChatCommand,
    ServerboundContainerClick, SynchronizePlayerPosition,
};
use mc_script::{CommandCapabilities, ScriptPluginManifest};
use mc_test_harness::client::Client;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// The one package this test deploys: the id the fixture's own storage is keyed by.
const OWNER: &str = "owned-inventory";
/// The login name of the fixture's one player. Offline mode derives the uuid from
/// it, which is also the name of that player's durable file.
const PLAYER: &str = "Steward";

/// The item resource the seed gives the player, the resource a reservation holds,
/// and the component-bearing tool a transfer moves.
const EMERALD_ITEM: &str = "minecraft:emerald";
const TOOL_ITEM: &str = "minecraft:diamond_pickaxe";

/// The name, damage and enchantment the tool carries, and that it must still carry
/// after every action.
const TOOL_NAME: &str = "Steward's Pick";
const TOOL_DAMAGE: i32 = 5;
const TOOL_ENCHANTMENT: &str = "minecraft:efficiency";
const TOOL_ENCHANTMENT_LEVEL: i32 = 2;

/// The inventory slots the world's player file fills: the first main-inventory
/// slot the reservation's emeralds sit in, and the hotbar slot the tool sits in.
const EMERALD_SLOT: u8 = 9;
const TOOL_SLOT: usize = 36;
/// The free hotbar slot the fixture stows the tool in, and the slot it puts it
/// back in.
const STOW_SLOT: usize = 37;
/// How many emeralds the world's player file gives the player.
const SEED_EMERALDS: i32 = 4;

/// The number of slots one canonical player-inventory snapshot holds: the 36
/// slots from the first main-inventory slot through the last hotbar slot.
const PLAYER_SLOTS: usize = 36;

/// The line the fixture logs from `init`, which is what this test waits for before
/// it sends the first action.
const READY: &str = "P3_INV ready";
/// The marker every concluded step is reported under, and the prefix a step that
/// could not be concluded is logged under.
const MARKER: &str = "P3_INV ";
const UNEXPECTED: &str = "P3_INV_UNEXPECTED";
const LEFT: &str = "P3_INV_LEFT";
const LATE_MOVE: &str = "P3_INV_MOVE_ANSWERED";

/// The component deployment declares the inventory capability and listens for
/// the original session leaving before the same identity reconnects. Operation
/// answers remain targeted to the plugin that asked, not broadcast events.
const MANIFEST: &str = r#"
id = "owned-inventory"
name = "Owned Inventory"
version = "0.1.0"
api = "0.7.0"
events = ["player.left"]
player_commands = ["inventory"]
capabilities = ["inventory_transfers", "storage_batches"]
required_features = ["inventory_transfers", "storage_batches"]
"#;

/// What tells the fixture which mode to run.
const CONFIG: &str = "mode = \"owned-inventory\"\n";
/// A second, ordinary component whose command callbacks fill the host's event
/// consumer while the first component's committed item answer is in flight.
const FLOOD_MANIFEST: &str = r#"
id = "hello-flood"
name = "Hello Flood"
version = "0.1.0"
api = "0.7.0"
player_commands = ["hello"]
"#;
const FLOOD_CONFIG: &str = "greeting = \"Hi there\"\n";

/// How long one action may spend on its real round trips before this test calls it
/// stalled. Nothing here sleeps: the wait ends on the fixture's own line.
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the join may take to publish the manifest's readiness and inventory.
const WIRE_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// Whether the guest logged it at error level. The fixture reports a step it
    /// could not conclude that way, so an error line means that step did not
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
    /// Optional barrier after the real owner answers, before the guest publishes
    /// its reply to the player.
    release_move: Option<Arc<Mutex<std::sync::mpsc::Receiver<()>>>>,
}

impl HostServices for RecordingLog {
    fn log(&mut self, level: LogLevel, message: &str) {
        let _ = self.lines.send(LogLine {
            plugin: self.id.clone(),
            error: matches!(level, LogLevel::Error),
            message: message.to_owned(),
        });
        if message == LATE_MOVE
            && let Some(release) = &self.release_move
        {
            release
                .lock()
                .expect("the late-answer barrier is available")
                .recv()
                .expect("the test releases the late answer");
        }
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
/// more.
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

/// One running component server: a real host over the packages a test writes to
/// disk, and a real server bound on the world that test opened.
struct Running {
    /// The directory the packages were discovered from. It stays alive for as long
    /// as the host runs, because that is what the host was started from.
    _deployment: tempfile::TempDir,
    address: SocketAddr,
    save: mc_net::SaveHandle,
    shutdown: mc_net::ShutdownHandle,
    log: UnboundedReceiver<LogLine>,
    host: PluginHost,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Running {
    /// Write the packages, start their host, and bind a real server on the world.
    async fn start(
        registries: &Registries,
        world_dir: &Path,
        packages: &[Package],
        motd: &str,
        release_move: Option<Arc<Mutex<std::sync::mpsc::Receiver<()>>>>,
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
                    "inventory_transfers".to_owned(),
                    "storage_batches".to_owned(),
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
                release_move: release_move.clone(),
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

/// Everything a run observed while it drove its client: the chat lines the guests
/// published, the authoritative inventory the server last published, and how many
/// inventory frames it has published at all.
#[derive(Default)]
struct Journal {
    /// Every system chat line the client read, in the order it read them.
    chats: Vec<String>,
    /// The last authoritative player inventory the server published.
    inventory: Option<ClientboundContainerSetContent>,
    /// How many authoritative player inventory frames the client has read.
    frames: usize,
}

impl Journal {
    /// The last authoritative inventory the server published.
    fn last_inventory(&self) -> &ClientboundContainerSetContent {
        self.inventory
            .as_ref()
            .expect("the server published no authoritative inventory")
    }
}

#[tokio::test]
async fn one_component_moves_and_reserves_real_owned_inventory_through_the_real_owners() {
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
            config: CONFIG,
        }],
        "owned inventory acceptance",
        None,
    )
    .await;
    let items = Arc::clone(&registries.items);
    let tool = expected_tool(&items);
    let mut journal = Journal::default();

    let mut client = login(running.address, PLAYER).await;
    let ready = wait_for_ready(&mut client, &mut running.log, &mut journal).await;
    assert_inventory(&ready, &items, &tool, SEED_EMERALDS);

    // The read: the owner answers the whole canonical snapshot and the fence a
    // later mutation names. A query carries no durable operation id, so the
    // fixture would have failed on one.
    let query = action(&mut client, &mut running.log, &mut journal, "query").await;
    assert!(
        query.starts_with(&format!(
            "{MARKER}query committed slots={PLAYER_SLOTS} revision="
        )),
        "the snapshot's own slot count and fence the owner answered: {query}"
    );

    // Ordinary client input moves the same tool the component will move below.
    // No component hash is supplied by the client: Swap is admitted and applied
    // by the vanilla container owner, then the guest reads the owner's new fence.
    for (source, button, destination) in [(TOOL_SLOT, 1, STOW_SLOT), (STOW_SLOT, 0, TOOL_SLOT)] {
        let before = journal.frames;
        client
            .write_packet(&ServerboundContainerClick {
                container_id: 0,
                state_id: journal.last_inventory().state_id,
                slot_num: i16::try_from(source).unwrap(),
                button_num: button,
                container_input: ContainerInput::Swap,
                changed_slots: Vec::new(),
                carried_item: HashedStack::empty(),
            })
            .await
            .expect("send ordinary hotbar swap");
        wait_for_inventory(&mut client, &mut running.log, &mut journal, before).await;
        assert_eq!(&journal.last_inventory().items[destination], &tool);
        assert!(journal.last_inventory().items[source].is_empty());
        assert_eq!(
            total_of(journal.last_inventory(), item_id(&items, EMERALD_ITEM)),
            SEED_EMERALDS
        );
    }

    // The transfer: exactly one item moves, and the authoritative inventory the
    // commit publishes shows it - the tool in the destination slot, the slot it
    // left empty, and the emeralds exactly where they were.
    let before_move = journal.frames;
    let moved = action(&mut client, &mut running.log, &mut journal, "move").await;
    assert!(
        moved.starts_with(&format!("{MARKER}move committed revision=")),
        "the fence revision the transfer answered: {moved}"
    );
    wait_for_inventory(&mut client, &mut running.log, &mut journal, before_move).await;
    assert_eq!(
        &journal.last_inventory().items[STOW_SLOT],
        &tool,
        "the transferred tool must arrive as exactly the stack it was, components included"
    );
    assert_eq!(
        journal.last_inventory().items[TOOL_SLOT].count,
        0,
        "the slot the tool left must be empty"
    );
    assert_eq!(
        total_of(journal.last_inventory(), item_id(&items, EMERALD_ITEM)),
        SEED_EMERALDS,
        "a transfer must conserve every stack it does not name"
    );

    // The transfer back, whose revision a replay then has to reproduce.
    let before_restore = journal.frames;
    let restored = action(&mut client, &mut running.log, &mut journal, "restore").await;
    let restore_revision = parse_revision(&restored);
    assert!(
        restore_revision > 0,
        "the fence revision the transfer answered: {restored}"
    );
    wait_for_inventory(&mut client, &mut running.log, &mut journal, before_restore).await;
    assert_eq!(
        &journal.last_inventory().items[TOOL_SLOT],
        &tool,
        "the tool must be back in the slot it came from"
    );
    assert_eq!(
        journal.last_inventory().items[STOW_SLOT].count,
        0,
        "the slot it was stowed in must be empty again"
    );

    // The old session must be gone before a new connection takes its identity.
    drop(client);
    loop {
        let line = tokio::time::timeout(WIRE_TIMEOUT, running.log.recv())
            .await
            .expect("the original inventory session did not leave")
            .expect("component host stopped before the player left");
        assert!(
            !line.error,
            "guest failed during disconnect: {}",
            line.message
        );
        if line.message.starts_with(LEFT) {
            break;
        }
    }
    let mut client = login(running.address, PLAYER).await;
    let fresh = loop {
        let mut frame = client
            .read_frame_with_timeout(WIRE_TIMEOUT)
            .await
            .expect("rejoined player inventory was not published");
        if frame.id == ClientboundContainerSetContent::ID {
            let content = ClientboundContainerSetContent::decode(&mut frame.body)
                .expect("decode rejoined inventory");
            if content.container_id == 0 {
                break content;
            }
        }
    };
    journal.frames += 1;
    journal.inventory = Some(fresh);
    assert_tool_intact(&journal, &items, &tool);

    // The old numeric player endpoint is not the same stable person's new
    // session. Reading it must refuse rather than return the new inventory.
    let expired = action(&mut client, &mut running.log, &mut journal, "expired").await;
    assert_eq!(expired, format!("{MARKER}expired refused not-found"));
    assert_tool_intact(&journal, &items, &tool);

    // The status request names only the durable id. It returns the old
    // session's typed outcome without moving the new session's items.
    let status = action(&mut client, &mut running.log, &mut journal, "status").await;
    assert_eq!(
        status,
        format!("{MARKER}status committed revision={restore_revision}")
    );
    assert_tool_intact(&journal, &items, &tool);

    // The replay: the same durable record, byte for byte, under a new correlation
    // id. A second application would have had nothing to move from the empty slot
    // and would have been refused; the recorded revision is what answers, and the
    // tool is still in exactly one place afterwards.
    let replayed = action(&mut client, &mut running.log, &mut journal, "replay").await;
    assert_eq!(
        replayed,
        format!("{MARKER}replay committed revision={restore_revision} replayed=true"),
        "a replay must answer the outcome the original commit recorded"
    );
    assert_tool_intact(&journal, &items, &tool);

    // The conflict: the same durable id with different content.
    let conflicted = action(&mut client, &mut running.log, &mut journal, "conflict").await;
    assert_eq!(
        conflicted,
        format!("{MARKER}conflict refused operation-conflict"),
        "an operation id reused for different content must be refused, not applied"
    );
    assert_tool_intact(&journal, &items, &tool);

    // The fence read is current; one revision past it is not.
    let stale = action(&mut client, &mut running.log, &mut journal, "stale").await;
    assert_eq!(stale, format!("{MARKER}stale refused stale-revision"));
    assert_tool_intact(&journal, &items, &tool);

    // Another runtime player's inventory is not this actor's to move from.
    let foreign = action(&mut client, &mut running.log, &mut journal, "foreign").await;
    assert_eq!(foreign, format!("{MARKER}foreign refused forbidden"));
    assert_tool_intact(&journal, &items, &tool);

    // The resident ledger exists but does not hold these handles.
    for absent in ["absent-resident", "absent-carry"] {
        let refused = action(&mut client, &mut running.log, &mut journal, absent).await;
        assert_eq!(refused, format!("{MARKER}{absent} refused not-found"));
    }
    // No settlement profile was deployed, so no warehouse owner can answer.
    let unavailable = action(
        &mut client,
        &mut running.log,
        &mut journal,
        "absent-warehouse",
    )
    .await;
    assert_eq!(
        unavailable,
        format!("{MARKER}absent-warehouse refused runtime-unavailable")
    );
    assert_tool_intact(&journal, &items, &tool);

    // The reservation: the owner answers its whole state, held against the fence
    // the read above named.
    let reserved = action(&mut client, &mut running.log, &mut journal, "reserve").await;
    assert_eq!(
        reserved,
        format!("{MARKER}reserve committed resources=1 reserved=1 released=false"),
        "a reservation must answer its own quantities"
    );

    // The durable side: the server's own save path, then the world's file.
    let report = running.save.save_all().await;
    assert!(report.is_ok(), "the save path failed: {report:?}");

    drop(client);
    let errors = error_lines(running.stop().await);
    assert!(
        errors.is_empty(),
        "the fixture reported a step it could not conclude: {errors:?}"
    );

    // Nothing this sequence did moved an item for good, so the saved player state
    // holds the emeralds it started with and the tool exactly as it arrived.
    let saved = saved_inventory(world.path());
    assert_eq!(
        saved_total(&saved, EMERALD_ITEM),
        SEED_EMERALDS,
        "the reservation holds units, it does not spend them"
    );
    assert_saved_tool(&saved);
}

/// Hold the actual owner's committed answer before guest callback output can
/// reach the client. The replacement session must not receive the old reply.
#[tokio::test]
async fn late_committed_inventory_answer_does_not_target_the_rejoined_session() {
    let world = tempfile::tempdir().expect("one temporary persistent world");
    std::fs::create_dir_all(world.path().join("region")).expect("world region directory");
    let registries = Registries::new();
    seed_player_state(world.path());
    let (release, wait) = std::sync::mpsc::channel();
    let mut running = Running::start(
        &registries,
        world.path(),
        &[
            Package {
                id: OWNER,
                manifest: MANIFEST,
                config: CONFIG,
            },
            Package {
                id: "hello-flood",
                manifest: FLOOD_MANIFEST,
                config: FLOOD_CONFIG,
            },
        ],
        "late owned inventory answer",
        Some(Arc::new(Mutex::new(wait))),
    )
    .await;
    let items = Arc::clone(&registries.items);
    let tool = expected_tool(&items);
    let mut journal = Journal::default();
    let mut client = login(running.address, PLAYER).await;
    wait_for_ready(&mut client, &mut running.log, &mut journal).await;
    action(&mut client, &mut running.log, &mut journal, "query").await;
    let mut flood = login(running.address, "Flood").await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "inventory move".to_owned(),
        })
        .await
        .expect("stage the original session's transfer");
    loop {
        let line = tokio::time::timeout(ACTION_TIMEOUT, running.log.recv())
            .await
            .expect("the real owner never answered the transfer")
            .expect("the component host stopped before the owner answered");
        assert!(
            !line.error,
            "guest failed before delivery: {}",
            line.message
        );
        if line.message == LATE_MOVE {
            break;
        }
    }

    // The log barrier holds the host before callback output is admitted. The
    // server can publish the replacement's inventory while that worker is held.
    drop(client);
    let mut client = login(running.address, PLAYER).await;
    let fresh = loop {
        let mut frame = client
            .read_frame_with_timeout(WIRE_TIMEOUT)
            .await
            .expect("the new session's inventory was not published");
        if frame.id == ClientboundContainerSetContent::ID {
            let content = ClientboundContainerSetContent::decode(&mut frame.body)
                .expect("decode new session inventory");
            if content.container_id == 0 {
                break content;
            }
        }
    };
    journal.inventory = Some(fresh);
    assert_eq!(&journal.last_inventory().items[STOW_SLOT], &tool);
    assert!(journal.last_inventory().items[TOOL_SLOT].is_empty());
    assert_eq!(
        total_of(journal.last_inventory(), item_id(&items, EMERALD_ITEM)),
        SEED_EMERALDS
    );

    // The other component receives real commands from another live player
    // while the owner's committed response remains held at the guest boundary.
    for _ in 0..32 {
        flood
            .write_packet(&ServerboundChatCommand {
                command: "hello".to_owned(),
            })
            .await
            .expect("send another player's component command");
    }

    let healthy_started = std::time::Instant::now();
    release.send(()).expect("release the old session's answer");
    let expired = action(&mut client, &mut running.log, &mut journal, "expired").await;
    println!(
        "accepted item decision alongside 32 other-component commands: healthy response {:.3} ms",
        healthy_started.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(expired, format!("{MARKER}expired refused not-found"));

    let status = action(&mut client, &mut running.log, &mut journal, "status").await;
    assert!(
        status.starts_with(&format!("{MARKER}status committed revision=")),
        "the durable operation remains queryable after a lost reply: {status}"
    );
    assert!(
        !journal
            .chats
            .iter()
            .any(|line| line.starts_with(&format!("{MARKER}move "))),
        "the old session's late callback must not address the replacement"
    );
    assert_eq!(&journal.last_inventory().items[STOW_SLOT], &tool);
    assert!(journal.last_inventory().items[TOOL_SLOT].is_empty());
    // A reply to the other player proves that this was delivered workload,
    // not just network bytes written beside an already-completed transfer.
    let deadline = tokio::time::Instant::now() + ACTION_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let mut frame = tokio::time::timeout(remaining, flood.read_frame())
            .await
            .expect("the other component never answered its accepted command")
            .expect("the flood player's connection closed before a command reply");
        if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode flood reply");
            if literal_text_component_text(&chat.content_nbt) == "Hello from a WASM plugin." {
                break;
            }
        }
    }
    drop(client);
    assert!(error_lines(running.stop().await).is_empty());
}

/// The capability boundary: the very transfer record the fixture sends converts
/// under a manifest that declares `inventory_transfers`, and is refused by name
/// under one that does not.
///
/// This is the same boundary the running server checks before it applies a
/// callback's batch, driven directly so the refusal names the grant a consumer
/// reads.
#[test]
fn an_owned_inventory_command_without_its_grant_is_refused_by_name() {
    let limits = PluginLimits::default();
    let mut batch = CommandBatch::new();
    batch
        .push(inventory_command(), &limits)
        .expect("the record stages");
    assert_eq!(
        to_script_batch(
            batch,
            batch_limit(),
            &NoSessions,
            &CommandCapabilities::none()
        ),
        Err(AdapterError::PermissionDenied {
            capability: "inventory_transfers"
        }),
        "the refusal names the capability the manifest would have declared"
    );

    // The control: the same record converts under the grant, so the refusal above
    // is the capability's and not a record this test got wrong.
    let mut batch = CommandBatch::new();
    batch
        .push(inventory_command(), &limits)
        .expect("the record stages");
    let converted = to_script_batch(
        batch,
        batch_limit(),
        &NoSessions,
        &granted_transfer_capabilities(),
    )
    .expect("the fixture's own transfer converts");
    let [mc_script::ScriptCommand::Operation { request }] = converted.commands() else {
        panic!("expected one operation, saw {:?}", converted.commands());
    };
    assert_eq!(request.request_id(), TRANSFER_REQUEST);
    assert_eq!(request.operation_id(), Some(TRANSFER_OPERATION));
}

/// The request and operation ids the staged transfer names. They are the
/// fixture's own spelling, so this case reads back what a guest decided to send.
const TRANSFER_REQUEST: &str = "inv-move";
const TRANSFER_OPERATION: &str = "inv-op-move";

/// One owned-inventory transfer record, as the contract's own bindings spell it.
fn inventory_command() -> WireCommand {
    let endpoint = InventoryEndpoint::PlayerInventory(7);
    WireCommand::Operation(OperationRequest {
        request: TRANSFER_REQUEST.to_owned(),
        operation: DomainOperation::Inventory(InventoryOperation::Transfer(InventoryTransfer {
            operation_id: TRANSFER_OPERATION.to_owned(),
            actor_id: 7,
            transfers: vec![OwnedItemTransfer {
                source: endpoint.clone(),
                source_slot: 36,
                destination: endpoint.clone(),
                destination_slot: 37,
                count: 1,
            }],
            expected_revisions: vec![InventoryExpectedRevision {
                endpoint,
                fence: InventoryFence {
                    revision: 1,
                    snapshot_hash: "a".repeat(64),
                },
            }],
        })),
    })
}

/// The capabilities a manifest that declared the transfer capability resolves to,
/// which is what the host pre-checks a callback's answer against.
fn granted_transfer_capabilities() -> CommandCapabilities {
    ScriptPluginManifest::new(
        OWNER,
        "Owned Inventory",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_inventory_transfers()
    .validate_for(mc_script::COMPONENT_PLUGIN_API_VERSION)
    .expect("the fixture's own declaration validates")
    .to_command_capabilities()
}

/// The bound every staged batch of the capability case is admitted against.
fn batch_limit() -> NonZeroUsize {
    NonZeroUsize::new(mc_script::MAX_SCRIPT_COMMAND_BATCH).expect("non-zero")
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

/// Wait for the two things the join publishes before any action: the fixture's own
/// readiness line, and the authoritative inventory the login carries.
async fn wait_for_ready(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
) -> ClientboundContainerSetContent {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let line = tokio::time::timeout(remaining, log.recv())
            .await
            .unwrap_or_else(|_| panic!("the fixture never logged {READY:?}"));
        let Some(line) = line else {
            panic!("the component host left before the fixture logged {READY:?}");
        };
        assert!(
            !line.error,
            "{} reported an error before it logged {READY:?}: {}",
            line.plugin, line.message
        );
        if line.message == READY {
            break;
        }
    }
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let mut frame = tokio::time::timeout(remaining, client.read_frame())
            .await
            .unwrap_or_else(|_| panic!("the join never published its inventory"))
            .unwrap_or_else(|error| panic!("the join lost its connection: {error}"));
        if frame.id == ClientboundContainerSetContent::ID {
            let content = ClientboundContainerSetContent::decode(&mut frame.body)
                .expect("decode authoritative inventory");
            if content.container_id == 0 {
                journal.frames += 1;
                journal.inventory = Some(content.clone());
                return content;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            record_chat(journal, &mut frame.body);
        }
    }
}

/// Send one `inventory` action and answer the line the fixture reported for it.
///
/// The wait ends on a line the fixture produced, never on a sleep: a marker that
/// never arrives is the step stalling, and an error line - which the fixture logs
/// instead of a marker - is the most specific failure a test can report. Every
/// chat line and inventory frame read while waiting is kept, so a later assertion
/// can still see what an earlier step did not wait for.
async fn action(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
    name: &str,
) -> String {
    client
        .write_packet(&ServerboundChatCommand {
            command: format!("inventory {name}"),
        })
        .await
        .unwrap_or_else(|error| panic!("send /inventory {name}: {error}"));
    let expected = format!("{MARKER}{name} ");
    let deadline = tokio::time::Instant::now() + ACTION_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "the {name} action never reported a line; saw {:?}",
                journal.chats
            );
        }
        if let Some(line) = journal
            .chats
            .iter()
            .find(|text| text.starts_with(&expected))
        {
            return line.clone();
        }
        tokio::select! {
            line = log.recv() => {
                let Some(line) = line else {
                    panic!("the component host left before the {name} action reported a line");
                };
                assert!(
                    !line.error,
                    "{} reported a plugin error instead of the {name} action reporting a line: {}",
                    line.plugin,
                    line.message
                );
            }
            frame = tokio::time::timeout(remaining, client.read_frame()) => {
                let mut frame = match frame {
                    Ok(Ok(frame)) => frame,
                    Ok(Err(error)) => panic!("the {name} action lost its connection: {error}"),
                    Err(_) => panic!(
                        "the {name} action never reported a line; saw {:?}",
                        journal.chats
                    ),
                };
                if frame.id == ClientboundContainerSetContent::ID {
                    let content = ClientboundContainerSetContent::decode(&mut frame.body)
                        .expect("decode authoritative inventory");
                    if content.container_id == 0 {
                        journal.frames += 1;
                        journal.inventory = Some(content);
                    }
                } else if frame.id == ClientboundSystemChat::ID {
                    record_chat(journal, &mut frame.body);
                }
            }
        }
    }
}

/// Wait for the next authoritative inventory frame after `before`, so a commit's
/// own publication is read even when it arrived behind the marker.
async fn wait_for_inventory(
    client: &mut Client,
    log: &mut UnboundedReceiver<LogLine>,
    journal: &mut Journal,
    before: usize,
) {
    let deadline = tokio::time::Instant::now() + ACTION_TIMEOUT;
    while journal.frames <= before {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("the commit never published the inventory it changed");
        }
        tokio::select! {
            line = log.recv() => {
                let Some(line) = line else {
                    panic!("the component host left before the commit published its inventory");
                };
                assert!(
                    !line.error,
                    "{} reported a plugin error instead of publishing its inventory: {}",
                    line.plugin,
                    line.message
                );
            }
            frame = tokio::time::timeout(remaining, client.read_frame()) => {
                let mut frame = match frame {
                    Ok(Ok(frame)) => frame,
                    Ok(Err(error)) => panic!("the run lost its connection: {error}"),
                    Err(_) => panic!("the commit never published the inventory it changed"),
                };
                if frame.id == ClientboundContainerSetContent::ID {
                    let content = ClientboundContainerSetContent::decode(&mut frame.body)
                        .expect("decode authoritative inventory");
                    journal.frames += 1;
                    if content.container_id == 0 {
                        journal.inventory = Some(content);
                    }
                } else if frame.id == ClientboundSystemChat::ID {
                    record_chat(journal, &mut frame.body);
                }
            }
        }
    }
}

/// Record one system chat frame's literal text.
///
/// The fixture reports a step it could not conclude by logging, not by chatting,
/// so every line here is a marker this run produced.
fn record_chat(journal: &mut Journal, body: &mut Bytes) {
    let chat = ClientboundSystemChat::decode(body).expect("decode system chat");
    let text = literal_text_component_text(&chat.content_nbt);
    if text.starts_with(UNEXPECTED) {
        panic!("the fixture could not conclude a step: {text}");
    }
    journal.chats.push(text);
}

/// The revision one transfer or query marker names.
fn parse_revision(line: &str) -> u64 {
    line.split_whitespace()
        .find_map(|word| word.strip_prefix("revision="))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("no revision in {line:?}"))
}

/// Check one authoritative inventory snapshot: the emerald total the sequence
/// states, and the one component-bearing tool the player brought, still exactly
/// the stack it is.
fn assert_inventory(
    content: &ClientboundContainerSetContent,
    items: &mc_data::items::ItemRegistry,
    tool: &ItemStack,
    emeralds: i32,
) {
    assert_eq!(
        total_of(content, item_id(items, EMERALD_ITEM)),
        emeralds,
        "the emerald total the snapshot carries"
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

/// The tool must still be in exactly one place - the slot the last commit put it
/// in - and the emeralds exactly as the sequence left them. A replay, a conflict,
/// a stale or foreign refusal and every absent probe must leave the inventory
/// exactly as it was.
fn assert_tool_intact(journal: &Journal, items: &mc_data::items::ItemRegistry, tool: &ItemStack) {
    let content = journal.last_inventory();
    assert_eq!(
        &content.items[TOOL_SLOT], tool,
        "the tool must still be in the slot the last commit put it in"
    );
    assert_eq!(
        content.items[STOW_SLOT].count, 0,
        "nothing may have moved the tool into the slot it was stowed in"
    );
    assert_eq!(
        content
            .items
            .iter()
            .filter(|held| held.item_id == tool.item_id)
            .count(),
        1,
        "the transfer must not have duplicated the tool"
    );
    assert_eq!(
        total_of(content, item_id(items, EMERALD_ITEM)),
        SEED_EMERALDS,
        "the emeralds must be exactly where the sequence left them"
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

/// The tool the saved player state holds must still be the stack it arrived as,
/// components included.
fn assert_saved_tool(entries: &[Vec<(String, Tag)>]) {
    let held = entries
        .iter()
        .find(|fields| text_field(fields, "id").as_deref() == Some(TOOL_ITEM))
        .expect("the saved player state no longer holds the tool");
    assert_eq!(
        int_field(held, "count"),
        1,
        "the tool's count must survive the transfers"
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
/// real inventory owners are started and their decisions are the ones on disk.
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
        max_players: 2,
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
