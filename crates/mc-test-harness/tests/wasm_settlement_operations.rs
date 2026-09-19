//! P3 settlement acceptance through one real component and one real world.
//!
//! The component is the repository's own example plugin, built to
//! `wasm32-unknown-unknown` and encoded as a component exactly the way a published
//! package is; the world is a real directory the server opens with the real terrain
//! generator, and the settlement profile is the sibling `solaris-settlements`
//! package's own catalog - the same `structures/*.toml` a deployed settlement
//! profile ships - discovered from the boundary's deployed packages exactly as the
//! live server discovers it.
//!
//! Nothing here is a stub owner: every step the fixture runs is one
//! `settlements.settlement-operation` the host converts into the server's own DTO,
//! admitted under the package's own capability and answered by the server's own
//! settlement runtime. The test drives one step at a time - a real player command
//! per step - and reads the fixture's own line for what the owner answered, so a
//! step that answers `refused <reason>` is the owner's typed refusal and not a
//! value this test invented.
//!
//! One real player is logged in over the wire, and every step's command is admitted
//! for that player's own session: the materials a structure is built from are the
//! stacks the world's own durable player file carries for it, so the reservation
//! the owner holds and the portion the owner commits are charged against a player's
//! canonical inventory rather than a value this test hands the guest.
//!
//! Four observations are the world's own rather than the fixture's:
//!
//! - the site the first page lists is authored core's catalog laid out, with a
//!   durable revision of zero until a reservation moves it;
//! - the survey is `loaded` only because this test generated the region's chunks
//!   through the world's own generator between the listing and the survey, which is
//!   what the world's chunk pipeline does for a joined player;
//! - the reserve's replay answers the same token, and the site's own revision moves
//!   from zero to a commit revision, which is what `refresh` reads back and what a
//!   plan that names zero is then refused against;
//! - the block the funded advance commits is the catalog's own first stage cell at
//!   the structure's own origin, and the container the bind resolves is the one the
//!   world's own storage holds at that structure's authored container position.
//!
//! A second test in this file proves the capability gate from the other side: the
//! same component, deployed under a manifest that never declared the settlement
//! capabilities, loses its route and admits nothing when it asks.

// Reuse the host tests' component builder.
#[path = "../../mc-plugin-host/tests/fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mc_nbt::Tag;
use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, HostServices, LogLevel, PluginLimits, discover,
    start_deployment, start_deployment_with,
};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundKeepAlive, ConfirmTeleportation, PlayDisconnect,
    ServerboundKeepAlive, SynchronizePlayerPosition,
};
use mc_script::{ScriptBoundary, ScriptPlayerContext, ScriptPlayerId};
use mc_test_harness::client::Client;
use mc_worldgen::{
    BlockEntitySeedKind, Blueprint, BlueprintCatalog, BlueprintInstance, QuarterTurn,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// The one package this test deploys: the id the fixture's answers are keyed by.
const OWNER: &str = "settlement-compat";
/// The sibling package whose authored catalog this fixture deploys, so the profile
/// core installs is the shipped one rather than a fixture.
const PROFILE: &str = "solaris-settlements";
/// The login name of the fixture's one player. The server derives the identity
/// from it in offline mode, which is also the name of the durable player file this
/// test seeds: the inventory every step names is the one the world itself loads
/// for that connection.
const PLAYER: &str = "Settler";
/// The blueprint the fixture plans. It is the shipped catalog's own warehouse, so
/// the plan the owner validates is one a deployed settlement profile really holds.
const BLUEPRINT: &str = "solaris:warehouse";
/// The namespace the shipped catalog authors its blueprints in, which is the one
/// every file of that catalog must agree with.
const NAMESPACE: &str = "solaris";
/// The player inventory slots the endpoint's own window covers: the first main
/// slot through the last hotbar slot.
const FIRST_PLAYER_SLOT: usize = 9;
const LAST_PLAYER_SLOT: usize = 44;
/// How many units of one resource fit one canonical player inventory slot.
const STACK: u64 = 64;

/// The fixture's own configuration: the mode it runs and the blueprint it plans.
///
/// Both runs write the same text, so the only difference between the accepted run
/// and the capability-gate run is what their manifests declared.
fn settlement_config() -> String {
    format!("mode = \"settlement-operations\"\nblueprint = \"{BLUEPRINT}\"\n")
}

/// The manifest of the accepted run: the settlement and owned-inventory capability
/// families the steps need, the features core must read before it opens a world,
/// and the one command root that drives them.
///
/// The fixture's own package is the settlement profile owner: it ships the
/// shipped package's `structures/` catalog next to this manifest, so core
/// discovers the profile from the deployed packages the host publishes rather
/// than from a package facts value this test injected.
const MANIFEST: &str = r#"
id = "settlement-compat"
name = "Settlement Compat"
version = "0.1.0"
api = "0.7.0"
events = ["player.joined"]
player_commands = ["settle"]
capabilities = ["world_sites", "structure_operations", "inventory_transfers"]
required_features = ["world_sites", "structure_operations", "inventory_transfers"]
"#;

/// The manifest of the capability-gate run: the same fixture, asking for durable
/// settlement work it never declared.
const DENIED_MANIFEST: &str = r#"
id = "settlement-compat"
name = "Settlement Compat"
version = "0.1.0"
api = "0.7.0"
events = ["player.joined"]
player_commands = ["settle"]
"#;

/// How long one step may spend on its real round trips before this test calls it
/// stalled. Nothing here sleeps: the wait ends on the fixture's own line.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the deferred host checks in the capability-gate run wait for a batch
/// that must never arrive.
const REFUSAL_WAIT: Duration = Duration::from_millis(500);

/// One log line a component guest asked its host to record.
struct LogLine {
    plugin: String,
    error: bool,
    message: String,
}

/// The host-services implementation this test hands the component host: the guest's
/// diagnostics land in this test's own channel instead of a global subscriber, so
/// this binary never touches process-wide tracing state.
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

/// One persistent world handle opening the real terrain generator.
///
/// The capacity is the world's resident chunk cache: the test keeps the site it
/// observes resident while the joined player's own view distance publishes the
/// chunks around its spawn, so a site far from that spawn cannot be evicted
/// between the plan and the portion built from it.
fn open_world(
    world_dir: &Path,
    registries: &Registries,
) -> Arc<tokio::sync::Mutex<mc_world::WorldStorage>> {
    let storage =
        mc_world::WorldStorage::open_with_capacity(world_dir, Arc::clone(&registries.blocks), 256)
            .expect("open the persistent world")
            .with_item_registry(Arc::clone(&registries.items))
            .with_generator(Arc::new(mc_worldgen::TerrainGenerator::new(
                0,
                Arc::clone(&registries.blocks),
            )));
    Arc::new(tokio::sync::Mutex::new(storage))
}

/// The server configuration every run binds with: one persistent world, so the real
/// settlement owner is started and its catalog is the shipped one.
fn server_config(
    registries: &Registries,
    world: Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    shutdown: &mc_net::ShutdownHandle,
) -> mc_net::ServerConfig {
    mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "settlement acceptance".to_owned(),
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

/// One running component server: a real host over the package this test writes to
/// disk, a real server bound on the world the test opened, and the one real player
/// connection every step is admitted for.
struct Running {
    /// The directory the package was discovered from. It stays alive for as long as
    /// the host runs, because that is what the host was started from and where core
    /// reads the settlement catalog from.
    _deployment: tempfile::TempDir,
    /// The host side of the boundary, kept so this test can drive one step at a
    /// time.
    boundary: ScriptBoundary,
    /// The world, so this test can generate the region's chunks through the world's
    /// own generator before it surveys them.
    world: Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    /// The joined player's connection, read while a step's answer is outstanding so
    /// the session the steps name stays alive.
    client: Client,
    /// The session the joined player holds: the runtime player id every step's
    /// command names.
    session: u64,
    /// The identity the server derived for that player, so a step's command carries
    /// the context the connection does.
    player: String,
    log: UnboundedReceiver<LogLine>,
    host: mc_plugin_host::PluginHost,
    shutdown: mc_net::ShutdownHandle,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Running {
    /// Write the package and its authored catalog, start the host, bind a real
    /// server on the world, and log the fixture's own player in over the wire.
    ///
    /// The order is the live server's: the host exists before the network binds, and
    /// it publishes the deployed package's own facts to the boundary, so the
    /// settlement profile the server discovers is the one the package declares and
    /// the catalog it ships.
    async fn start(
        registries: &Registries,
        world: Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    ) -> Self {
        let deployment = tempfile::tempdir().expect("component deployment directory");
        let directory = deployment.path().join(OWNER);
        std::fs::create_dir_all(&directory).expect("component package directory");
        std::fs::write(directory.join("plugin.toml"), MANIFEST).expect("component manifest");
        std::fs::write(directory.join("plugin.wasm"), fixture::component_bytes())
            .expect("component artifact");
        std::fs::write(directory.join("config.toml"), settlement_config())
            .expect("component config");

        // The fixture's own package is the settlement profile owner: the shipped
        // package's authored catalog is deployed next to the manifest it declares
        // the profile with, so core discovers it from the deployed packages the
        // host publishes and not from a value this test injected.
        copy_directory(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../solaris-default-plugins")
                .join(PROFILE)
                .join("structures"),
            &directory.join("structures"),
        );

        let limits = PluginLimits::default();
        let config = DeploymentConfig {
            root: deployment.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec![OWNER.to_owned()],
            grants: BTreeMap::from([(
                OWNER.to_owned(),
                vec![
                    "world_sites".to_owned(),
                    "structure_operations".to_owned(),
                    "inventory_transfers".to_owned(),
                ],
            )]),
            require_grants: true,
            precommit_hooks: Vec::new(),
        };
        let discovered = discover(&config, &limits)
            .expect("the component package is discovered")
            .into_packages();
        let (lines, log) = tokio::sync::mpsc::unbounded_channel();
        let host = start_deployment_with(
            discovered,
            limits,
            HostQueues::default(),
            Arc::new(mc_plugin_host::NoSessions),
            move |id: &str| RecordingLog {
                id: id.to_owned(),
                lines: lines.clone(),
            },
        )
        .expect("the component host starts");

        let boundary = host.boundary().clone();

        let shutdown = mc_net::ShutdownHandle::default();
        let config = server_config(registries, Arc::clone(&world), &shutdown);
        let bound = mc_net::bind_with_scripts(config, boundary.clone())
            .await
            .expect("bind the component server");
        let address = bound.local_addr().expect("the bound server's address");
        let server = tokio::spawn(async move { bound.serve().await });

        // The one player the fixture's steps are admitted for: a real connection,
        // whose durable inventory the world loads from its own player file, and
        // whose session is the runtime player id a materials reservation names.
        let (client, session, player) = join(address, PLAYER).await;

        Self {
            _deployment: deployment,
            boundary,
            world,
            client,
            session,
            player,
            log,
            host,
            shutdown,
            server,
        }
    }

    /// Run one fixture step and answer the line the owner's answer produced.
    ///
    /// The command entry is the server's own admission path, so a step the server
    /// would not route to this package fails here rather than being reported as an
    /// owner refusal; the player it names is the connection this run joined, so a
    /// step that addresses the player's own inventory resolves to that session.
    async fn step(&mut self, step: &str) -> String {
        let context = ScriptPlayerContext::try_new(&self.player, PLAYER, false, 0.0, 64.0, 0.0)
            .expect("player context");
        let admission = self
            .boundary
            .try_enqueue_player_command_with_context(
                ScriptPlayerId::new(self.session),
                context,
                &format!("settle {step}"),
            )
            .expect("the command queue accepts one command");
        assert_eq!(
            admission,
            mc_script::PlayerCommandAdmission::Enqueued,
            "the server routes `settle` to this package"
        );
        self.next_line(step).await
    }

    /// The next line the guest logged for one step.
    ///
    /// The wait ends on the fixture's own line, never on a sleep: the joined
    /// player's stream is read while the step is outstanding and its liveness
    /// checks are answered, because a connection that never reads would be dropped
    /// by the server and would take the session the steps name with it.
    async fn next_line(&mut self, step: &str) -> String {
        let deadline = tokio::time::Instant::now() + STEP_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                remaining > Duration::ZERO,
                "the fixture answered {step} within the wait"
            );
            let Running { log, client, .. } = self;
            tokio::select! {
                line = tokio::time::timeout(remaining, log.recv()) => {
                    let line = line
                        .unwrap_or_else(|_| panic!("the fixture answered {step} within the wait"))
                        .expect("the guest logged a line");
                    assert!(
                        !line.error,
                        "the fixture reported {step} as a diagnostic: {}",
                        line.message
                    );
                    assert_eq!(line.plugin, OWNER, "the line came from this package");
                    if line.message.starts_with("P3_SETTLE ") {
                        assert!(
                            line.message.contains(step),
                            "the line for {step} names another step: {}",
                            line.message
                        );
                        return line.message;
                    }
                }
                frame = tokio::time::timeout(remaining, client.read_frame()) => {
                    let frame = frame
                        .unwrap_or_else(|_| panic!("the fixture answered {step} within the wait"))
                        .unwrap_or_else(|error| {
                            panic!("the run lost its connection while waiting for {step}: {error}")
                        });
                    keep_alive(client, frame).await;
                }
            }
        }
    }

    /// Generate every chunk one region covers, through the world's own generator.
    ///
    /// A survey reads the world's published chunks, exactly as a player's view
    /// distance publishes them, so a region nobody generated is the owner's own
    /// `unloaded` and nothing else: this is what makes the survey step answer
    /// `loaded` rather than a value the fixture assumed.
    async fn generate_region(&self, origin: [i32; 3]) {
        let mut world = self.world.lock().await;
        let first = mc_world::ChunkPos {
            x: (origin[0] - 16).div_euclid(16),
            z: (origin[2] - 16).div_euclid(16),
        };
        let last = mc_world::ChunkPos {
            x: (origin[0] + 48).div_euclid(16),
            z: (origin[2] + 48).div_euclid(16),
        };
        for x in first.x..=last.x {
            for z in first.z..=last.z {
                let position = mc_world::ChunkPos { x, z };
                let generated = world
                    .get_chunk(position)
                    .expect("the world generates the region's chunks");
                assert!(generated.is_some(), "the generator answered every chunk");
            }
        }
    }

    /// The block state the world holds at one position.
    ///
    /// This reads the same storage the server committed the portion into, so the
    /// state it answers is the one the owner's own placement left behind rather
    /// than a value this test wrote.
    async fn block_state(&self, position: [i32; 3]) -> mc_world::BlockStateId {
        let mut world = self.world.lock().await;
        let chunk = world
            .get_chunk(mc_world::ChunkPos {
                x: position[0].div_euclid(16),
                z: position[2].div_euclid(16),
            })
            .expect("the world reads the observed chunk")
            .expect("the observed position is resident");
        chunk
            .get_block(
                position[0].rem_euclid(16) as u8,
                position[1],
                position[2].rem_euclid(16) as u8,
            )
            .expect("the observed height is inside the world")
    }

    /// Operator setup for binding: materialize the authored container block and
    /// its inventory. This is not evidence that the one funded work unit built it.
    async fn place_container(&self, position: [i32; 3], state: mc_world::BlockStateId) {
        let position = mc_world::BlockPos {
            x: position[0],
            y: position[1],
            z: position[2],
        };
        let mut world = self.world.lock().await;
        assert!(
            world
                .set_block_at(position, state)
                .expect("materialize the authored container block")
                .is_some(),
            "the container block has a resident cell"
        );
        assert!(
            world
                .set_chest_block_entity(position, mc_world::ChestBlockEntity::default())
                .expect("materialize the container inventory"),
            "the container's chunk is resident"
        );
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

/// One field of one marker line, by name.
fn field(marker: &str, key: &str) -> String {
    for entry in marker.split_whitespace() {
        if let Some(value) = entry.strip_prefix(&format!("{key}=")) {
            return value.to_owned();
        }
    }
    panic!("marker {marker:?} carries no {key}");
}

/// One field of one marker line as a whole number.
fn number(marker: &str, key: &str) -> i64 {
    field(marker, key)
        .parse()
        .unwrap_or_else(|_| panic!("marker {marker:?} carries a non-numeric {key}"))
}

/// The three comma-separated numbers one marker field carries.
fn triple(marker: &str, key: &str) -> [i32; 3] {
    let value = field(marker, key);
    let mut parts = value.split(',').map(|part| {
        part.parse()
            .unwrap_or_else(|_| panic!("marker {marker:?} carries a malformed {key}"))
    });
    let axes = [parts.next(), parts.next(), parts.next()];
    assert!(
        parts.next().is_none(),
        "marker {marker:?} carries more than three axes"
    );
    let [Some(x), Some(y), Some(z)] = axes else {
        panic!("marker {marker:?} carries fewer than three axes")
    };
    [x, y, z]
}

/// One marker's `resource:quantity` list as the materials it names.
fn materials(marker: &str, key: &str) -> BTreeMap<String, u64> {
    let value = field(marker, key);
    if value.is_empty() {
        return BTreeMap::new();
    }
    value
        .split(',')
        .map(|entry| {
            let (resource, quantity) = entry
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("marker {marker:?} carries a malformed {key}: {entry}"));
            let quantity = quantity
                .parse()
                .unwrap_or_else(|_| panic!("marker {marker:?} carries a malformed {key}: {entry}"));
            (resource.to_owned(), quantity)
        })
        .collect()
}

/// The shipped blueprint this run plans, loaded from the same catalog files core
/// loads it from: `<profile>/structures/*.toml`, decoded by the same
/// [`BlueprintCatalog`] the server's own settlement runtime reads.
///
/// The plan a structure's materials are charged against is a pure function of this
/// blueprint, so this test can hold the player's inventory to it without asking the
/// guest what it planned.
fn shipped_blueprint(blocks: &mc_world::BlockRegistry) -> Arc<Blueprint> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../solaris-default-plugins")
        .join(PROFILE)
        .join("structures");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&directory).unwrap_or_else(|error| {
        panic!(
            "sibling plugin checkout missing at {}: {error}",
            directory.display()
        )
    }) {
        let entry = entry.expect("read the shipped catalog entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".toml") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).expect("read the shipped blueprint");
        files.push((name, text));
    }
    files.sort();
    let catalog = BlueprintCatalog::from_files(blocks, NAMESPACE, &files)
        .unwrap_or_else(|error| panic!("the shipped catalog decodes: {error}"));
    catalog
        .get(BLUEPRINT)
        .cloned()
        .unwrap_or_else(|| panic!("the shipped catalog authors {BLUEPRINT}"))
}

/// The materials one authored blueprint's own staged plan consumes, per resource.
///
/// This is what the owner plans: one portion per authored stage, and every cell of
/// a stage is one unit of the palette block it names, with the last authored cell of
/// a position the one that stands. The totals are what a reservation of that plan
/// holds, so the inventory this test seeds and the conservation it asserts are the
/// plan's own numbers.
fn plan_totals(blueprint: &Blueprint) -> BTreeMap<String, u64> {
    let palette: BTreeMap<u16, String> = blueprint
        .palette()
        .iter()
        .map(|entry| (entry.index, entry.block.as_str().to_owned()))
        .collect();
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    for stage in blueprint.stages() {
        let mut standing: BTreeMap<(i32, i32, i32), u16> = BTreeMap::new();
        for block in &stage.blocks {
            standing.insert((block.x, block.y, block.z), block.palette);
        }
        for palette_index in standing.values() {
            let resource = palette
                .get(palette_index)
                .unwrap_or_else(|| panic!("stage cell {palette_index} names a palette entry"));
            *totals.entry(resource.clone()).or_insert(0) += 1;
        }
    }
    totals
}

/// Write the world's own durable player file for this test's player, holding
/// exactly the materials the shipped blueprint's staged plan consumes.
///
/// This is the server's canonical player format - gzip of the named NBT the server
/// itself reads on login - so the stacks the owner's reservation holds are state the
/// server validates on load rather than a value this test hands the guest.
fn seed_player_state(world_dir: &Path, plan: &BTreeMap<String, u64>) {
    let mut elements = Vec::new();
    let mut slot = FIRST_PLAYER_SLOT;
    for (resource, quantity) in plan {
        let mut remaining = *quantity;
        while remaining > 0 {
            let count = remaining.min(STACK);
            remaining -= count;
            assert!(
                slot <= LAST_PLAYER_SLOT,
                "the plan's materials fit one player inventory"
            );
            elements.push(Tag::Compound(vec![
                ("Slot".to_owned(), Tag::Byte(slot as i8)),
                ("id".to_owned(), Tag::String(resource.clone())),
                ("count".to_owned(), Tag::Int(count as i32)),
            ]));
            slot += 1;
        }
    }
    let root = Tag::Compound(vec![(
        "Inventory".to_owned(),
        Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::COMPOUND,
            elements,
        }),
    )]);
    let uuid = mc_net::offline_uuid(PLAYER);
    let path = world_dir.join("playerdata").join(format!("{uuid}.dat"));
    std::fs::create_dir_all(path.parent().expect("playerdata directory"))
        .expect("playerdata directory");
    let mut encoded = Vec::new();
    mc_nbt::write_named(&mut encoded, "", &root).expect("the player state encodes");
    let file = std::fs::File::create(&path).expect("player data file");
    let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    encoder.write_all(&encoded).expect("write the player state");
    encoder.finish().expect("finish the player state");
}

/// Log the fixture's one player in over a real connection, and answer the client
/// together with the session the server admitted it under and the identity it
/// derived for it - the two values every step's command carries.
async fn join(address: SocketAddr, name: &str) -> (Client, u64, String) {
    let mut client = Client::connect(address).await.expect("client connect");
    let login = client
        .drive_login(address, name)
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let play = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition =
        client.read_typed().await.expect("initial player position");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("confirm initial position");
    let session = u64::try_from(play.entity_id).expect("the play login names a session");
    (client, session, login.uuid.to_string())
}

/// Answer one frame of a joined player's stream while a step is outstanding.
///
/// Nothing else is a step's business: the world's own publications are read and
/// dropped, and a closed connection is reported as the run losing its player rather
/// than as a step that never answered.
async fn keep_alive(client: &mut Client, mut frame: mc_protocol::RawFrame) {
    if frame.id == ClientboundKeepAlive::ID {
        let ping = ClientboundKeepAlive::decode(&mut frame.body).expect("decode keepalive");
        client
            .write_packet(&ServerboundKeepAlive { id: ping.id })
            .await
            .expect("answer keepalive");
    } else if frame.id == PlayDisconnect::ID {
        let disconnect = PlayDisconnect::decode(&mut frame.body).expect("decode disconnect");
        panic!(
            "the joined player was disconnected: {}",
            String::from_utf8_lossy(&disconnect.reason_nbt)
        );
    }
}

/// Copy one package directory, so the profile this test installs is a real
/// deployed package rather than a path into a sibling checkout.
fn copy_directory(source: &Path, destination: &Path) {
    assert!(
        source.is_dir(),
        "sibling plugin checkout missing at {}: clone solaris-default-plugins next to solaris",
        source.display()
    );
    std::fs::create_dir(destination).expect("create the copied package directory");
    for entry in std::fs::read_dir(source).expect("read the shipped package") {
        let entry = entry.expect("read the shipped package entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("entry type").is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy the shipped package file");
        }
    }
}

#[tokio::test]
async fn one_component_drives_the_real_settlement_owner_through_a_real_world() {
    let world_dir = tempfile::tempdir().expect("one temporary persistent world");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("world region directory");
    let registries = Registries::new();
    // The blueprint this run plans, and the materials its own staged plan consumes:
    // the world's durable player file holds exactly those before the server opens
    // it, so the reservation the owner holds is covered by the inventory the server
    // loads for the joined player.
    let blueprint = shipped_blueprint(&registries.blocks);
    let plan = plan_totals(&blueprint);
    seed_player_state(world_dir.path(), &plan);
    let world = open_world(world_dir.path(), &registries);
    let mut running = Running::start(&registries, world).await;

    // The first page of sites: core's own authored catalog for this world.
    let listed = running.step("list").await;
    let first = field(&listed, "first");
    assert!(
        number(&listed, "sites") >= 1,
        "a deployed settlement profile lists sites: {listed}"
    );
    assert_eq!(
        field(&listed, "cursor"),
        "some",
        "a walk the owner can continue names its cursor: {listed}"
    );
    assert!(
        number(&listed, "pois") >= 1,
        "an authored site describes its own points of interest: {listed}"
    );
    assert_eq!(
        number(&listed, "revision"),
        0,
        "a site no reservation touched reports revision zero: {listed}"
    );
    let origin = triple(&listed, "origin");

    // The site's own points of interest, read back by the id the page named.
    let queried = running.step("query").await;
    assert_eq!(
        field(&queried, "site"),
        first,
        "the query named the listed site"
    );
    assert!(
        number(&queried, "pois") >= 1,
        "the site page carries its points of interest: {queried}"
    );

    // A cursor the site does not hold is the owner's own cursor-expired.
    assert_eq!(
        running.step("query-stale").await,
        "P3_SETTLE query-stale-refused cursor-expired"
    );

    // One durable reservation of a free home, and its replay under the same id.
    let reserved = running.step("reserve").await;
    let token = field(&reserved, "token");
    assert!(
        number(&reserved, "revision") > 0,
        "the reservation reports the revision its commit landed at: {reserved}"
    );
    let replayed = running.step("reserve-replay").await;
    assert_eq!(
        field(&replayed, "token"),
        token,
        "a replay answers the reservation the first call recorded: {replayed}"
    );
    assert_eq!(
        field(&replayed, "same"),
        "yes",
        "the replay minted no second reservation: {replayed}"
    );

    // The hand-back, and the owner's refusal of a second one.
    let released = running.step("release").await;
    assert_eq!(
        field(&released, "token"),
        token,
        "the release answers the token it handed back: {released}"
    );
    assert_eq!(
        running.step("release-again").await,
        "P3_SETTLE release-again-refused not-found"
    );

    // The revision the release left: the fence a plan must name now that the site
    // holds durable reservation state.
    let refreshed = running.step("refresh").await;
    let site_revision = number(&refreshed, "revision");
    assert!(
        site_revision > 0,
        "the released site carries a durable revision: {refreshed}"
    );

    // No view distance has published the survey's region yet, so a survey of it is
    // the owner's own `unloaded` - which is what makes the generated survey below
    // an observation of the world rather than a formality.
    assert_eq!(
        running.step("survey-far").await,
        "P3_SETTLE survey-far-refused unloaded"
    );
    running.generate_region(origin).await;
    let surveyed = running.step("survey").await;
    assert_eq!(
        field(&surveyed, "availability"),
        "loaded",
        "the surveyed region is the one this test generated: {surveyed}"
    );
    let usable = number(&surveyed, "usable");
    let water = number(&surveyed, "water");
    assert!(
        usable + water <= 32 * 32,
        "the survey aggregates the columns it read: {surveyed}"
    );
    let survey_token = field(&surveyed, "token");
    assert_ne!(survey_token, token, "the survey token is core's own");

    // A plan that names the revision the site has moved past is refused; the same
    // plan naming the revision `refresh` read is committed.
    assert_eq!(
        running.step("prepare-stale").await,
        "P3_SETTLE prepare-stale-refused stale-revision"
    );
    let prepared = running.step("prepare").await;
    let structure = field(&prepared, "structure");
    assert_eq!(
        field(&prepared, "state"),
        "prepared",
        "a plan commits as a prepared structure: {prepared}"
    );
    assert!(
        number(&prepared, "stages") >= 1,
        "the catalog's own blueprint plans stages: {prepared}"
    );
    // The structure's own material plan, which is what a reservation has to answer
    // to fund it.
    let plan_hash = field(&prepared, "plan");
    assert!(
        !plan_hash.is_empty(),
        "a prepared structure carries the hash of its own plan: {prepared}"
    );

    // The structure reads back as the owner holds it, with nothing built yet.
    let status = running.step("status").await;
    assert_eq!(field(&status, "structure"), structure);
    assert_eq!(field(&status, "state"), "prepared");
    assert_eq!(
        number(&status, "watermark"),
        0,
        "a prepared structure committed no portion: {status}"
    );

    // A portion whose reservation the owner does not hold is its own not-found.
    assert_eq!(
        running.step("advance").await,
        "P3_SETTLE advance-refused not-found"
    );

    // An id the owner never held and an authored container the world has not
    // materialized are both not-found: the container the funded bind below resolves
    // is a container the world holds, not the structure's authored seed alone.
    assert_eq!(
        running.step("bind-missing").await,
        "P3_SETTLE bind-missing-refused not-found"
    );
    assert_eq!(
        running.step("bind").await,
        "P3_SETTLE bind-refused not-found"
    );

    // The cell the plan's first stage commits first, and the container the fixture
    // binds: both projected from the shipped blueprint through the structure's own
    // origin and rotation, exactly as the owner projects them.
    let anchor = triple(&prepared, "origin");
    assert_eq!(
        number(&prepared, "rotation"),
        0,
        "the fixture plans the catalog's own authored facing: {prepared}"
    );
    let instance = BlueprintInstance::new(Arc::clone(&blueprint), QuarterTurn::None, anchor);
    let first_stage = blueprint
        .stages()
        .first()
        .expect("the blueprint plans stages");
    let mut cells = instance.placed_stage_blocks(first_stage);
    cells.sort_by_key(|cell| cell.pos);
    let cell = *cells.first().expect("the first stage plans cells");
    let planned = registries
        .blocks
        .by_id(cell.state)
        .expect("the planned cell names a registered block")
        .block
        .id
        .as_str()
        .to_owned();
    let container = instance
        .placed_block_entities()
        .into_iter()
        .find(|seed| seed.kind == BlockEntitySeedKind::EmptyContainer)
        .expect("the blueprint authors the container the fixture binds");
    assert_ne!(
        running.block_state(cell.pos).await,
        cell.state,
        "the planned cell is not what the world holds before the portion: {}",
        cell.pos.map(|axis| axis.to_string()).join(",")
    );

    // The player's own inventory, which is the endpoint the materials are held
    // against, and the reservation of the structure's own plan: the owner answers
    // the plan's hash, so the reservation the structure spends from is the plan it
    // was prepared against.
    let read = running.step("inventory").await;
    assert_eq!(
        number(&read, "session") as u64,
        running.session,
        "the inventory read answers for the session every step names: {read}"
    );
    assert_eq!(
        number(&read, "slots") as usize,
        LAST_PLAYER_SLOT - FIRST_PLAYER_SLOT + 1,
        "one player inventory snapshot holds the endpoint's own window: {read}"
    );
    let held = running.step("reserve-materials").await;
    assert!(
        !field(&held, "ref").is_empty(),
        "a reservation names its own opaque reference: {held}"
    );
    assert_eq!(
        field(&held, "hash"),
        plan_hash,
        "the reservation holds the plan the structure was fenced against: {held}"
    );
    assert_eq!(
        number(&held, "resources") as usize,
        plan.len(),
        "the plan holds one quantity per resource it needs: {held}"
    );
    assert_eq!(
        materials(&held, "reserved"),
        plan,
        "the reservation holds exactly the structure's own plan: {held}"
    );

    // The funded portion: the owner charges the plan the reservation holds, commits
    // the cell, and answers the receipt of what it spent.
    let funded = running.step("advance-materials").await;
    assert_eq!(field(&funded, "receipt"), structure);
    assert_eq!(
        field(&funded, "stage"),
        first_stage.id,
        "the portion commits the structure's own first stage: {funded}"
    );
    assert_eq!(
        number(&funded, "work"),
        1,
        "the portion carries the work the step authorized: {funded}"
    );
    assert_eq!(
        materials(&funded, "consumed"),
        BTreeMap::from([(planned.clone(), 1)]),
        "the portion consumes the cell's own resource: {funded}"
    );
    assert!(
        number(&funded, "revision") > 0,
        "the receipt carries the revision its commit landed at: {funded}"
    );

    // The world's own effect: the block the portion committed is the state the
    // blueprint's own plan places at that cell.
    let placed = running.block_state(cell.pos).await;
    assert_eq!(
        placed,
        cell.state,
        "the world holds the state this structure's plan placed at {}",
        cell.pos.map(|axis| axis.to_string()).join(",")
    );
    assert_eq!(
        registries
            .blocks
            .by_id(placed)
            .expect("the placed cell names a registered block")
            .block
            .id
            .as_str(),
        planned,
        "the placed block is the resource the receipt charged"
    );

    // The material effect: the structure counts the portion and its two halves
    // still add up to the plan the reservation holds.
    let built = running.step("status").await;
    assert_eq!(field(&built, "structure"), structure);
    assert_eq!(
        field(&built, "state"),
        "running",
        "a funded portion leaves the structure active: {built}"
    );
    assert_eq!(
        number(&built, "watermark"),
        1,
        "the structure counts the portion it committed: {built}"
    );
    assert_eq!(
        field(&built, "plan"),
        plan_hash,
        "the structure still names the plan it was prepared with: {built}"
    );
    let consumed = materials(&built, "consumed");
    assert_eq!(
        consumed,
        BTreeMap::from([(planned.clone(), 1)]),
        "the structure consumed what the receipt charged: {built}"
    );
    let mut conserved: BTreeMap<String, u64> = consumed;
    for (resource, quantity) in materials(&built, "remaining") {
        *conserved.entry(resource).or_insert(0) += quantity;
    }
    assert_eq!(
        conserved, plan,
        "consumed plus remaining is the plan the reservation holds: {built}"
    );

    // Materialize the actual authored container as operator setup. Binding is a
    // separate owner operation; the funded portion above built only its first cell.
    let container_state = blueprint
        .stages()
        .iter()
        .rev()
        .find_map(|stage| {
            instance
                .placed_stage_blocks(stage)
                .into_iter()
                .rev()
                .find(|cell| cell.pos == container.at)
        })
        .expect("the authored container has a planned block")
        .state;
    running.place_container(container.at, container_state).await;
    let bound = running.step("bind-container").await;
    assert!(
        bound.contains(&format!("structure-id: \"{structure}\"")),
        "the authored binding records its source structure: {bound}"
    );
    assert!(
        bound.contains("container-id: 0"),
        "the binding records the authored container ordinal: {bound}"
    );
    assert!(
        !field(&bound, "handle").is_empty(),
        "a binding carries the owner's own opaque handle: {bound}"
    );
    assert!(
        number(&bound, "revision") > 0,
        "the binding carries the revision its commit landed at: {bound}"
    );

    // Pausing an active structure answers the paused snapshot.
    let paused = running.step("pause").await;
    assert_eq!(
        field(&paused, "state"),
        "paused",
        "the owner paused the active structure: {paused}"
    );
    let reason = field(&paused, "reason");
    assert!(
        reason == "none" || reason == "site_changed",
        "a pause reason is one of the owner's own: {paused}"
    );

    // A cancelled structure is no longer placed, so its own authored container is
    // refused as blocked rather than answered as a container of a live structure.
    let cancelled = running.step("cancel").await;
    assert_eq!(
        field(&cancelled, "state"),
        "cancelled",
        "the owner cancelled the active structure: {cancelled}"
    );
    assert_eq!(
        running.step("bind-cancelled").await,
        "P3_SETTLE bind-cancelled-refused blocked"
    );
    assert_eq!(running.step("done").await, "P3_SETTLE done");

    // Nothing of the sequence logged a diagnostic, including the steps that only a
    // real owner could answer.
    let lines = running.stop().await;
    for line in &lines {
        assert!(
            !line.error,
            "the fixture logged a diagnostic: {}",
            line.message
        );
    }
}

#[tokio::test]
async fn a_settlement_command_without_its_capability_is_refused_and_retires_the_instance() {
    // The capability gate is the server's, not the guest's: the fixture asks for
    // settlement work its manifest never declared, so the batch is refused under
    // the capability's own name and the instance loses its route instead of
    // silently losing the commands.
    let root = tempfile::tempdir().expect("deployment root");
    let directory = root.path().join(OWNER);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(directory.join("plugin.toml"), DENIED_MANIFEST).expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), fixture::component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), settlement_config()).expect("config");

    let limits = PluginLimits::default();
    let packages = discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec![OWNER.to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("deployment")
    .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(mc_plugin_host::NoSessions),
    )
    .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["settle".to_owned()],
        "the package starts registered, so losing its route is the event under test"
    );

    // The gate is the server's, not a player's: this run joins nobody, so the
    // identity the refused command carries is only the shape the admission accepts.
    let context = ScriptPlayerContext::try_new(
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        PLAYER,
        false,
        0.0,
        64.0,
        0.0,
    )
    .expect("player context");
    boundary
        .try_enqueue_player_command_with_context(ScriptPlayerId::new(7), context, "settle list")
        .expect("the command queue accepts one command");
    tokio::time::sleep(REFUSAL_WAIT).await;
    assert!(
        boundary.player_command_roots().is_empty(),
        "a batch the server refused for a missing capability retires the instance"
    );
    assert!(
        tokio::time::timeout(REFUSAL_WAIT, boundary.recv_command())
            .await
            .is_err(),
        "nothing of a refused batch is admitted"
    );
    let counters = host.stop();
    assert_eq!(
        counters[0].1.commands_refused, 1,
        "the refused settlement command is counted as refused"
    );
}
