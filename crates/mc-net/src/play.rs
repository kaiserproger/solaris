//! Play state handler.
//!
//! M1.g.3 scope: send the four packets a vanilla client expects when
//! transitioning into Play state, then run a `ClientboundKeepAlive` →
//! `ServerboundKeepAlive` loop until the client disconnects or the
//! peer-side keepalive timeout fires.
//!
//! ```text
//! S → C  Login (Play)
//! S → C  Synchronize Player Position
//! S → C  Set Default Spawn Position
//! S → C  Game Event (start_waiting_for_level_chunks)
//! S → C  Keep Alive   (every 15 s; client must echo within 30 s)
//! ```
//!
//! No chunk data is sent — the client renders a black world. That is the
//! M1.g bar; chunk streaming is M2-M3 territory.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use mc_data::block_light::BlockLightTable;
use mc_data::items::ItemRegistry;
use mc_data::{Registry, VanillaData};
use mc_protocol::codec::Identifier;
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ChunkHeightmap, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundKeepAlive, ClientboundSetHeldSlot,
    ConfirmTeleportation, GameEvent, ItemStack, LevelChunkWithLight, LightData, LightUpdate,
    LoginPlay, PlayerActionKind, ServerboundKeepAlive, ServerboundPlayerAction,
    ServerboundSetCarriedItem, ServerboundUseItemOn, SetCenterChunk, SynchronizePlayerPosition,
    unpack_block_pos,
};
use mc_world::light::{
    LightCache, LightWorkspace, apply_block_change_to_light, compute_chunk_light_in,
};
use mc_world::wire::{client_heightmaps, encode_chunk_data, encode_chunk_light};
use mc_world::{Chunk, ChunkPos};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

use crate::connection::{read_frame, write_packet};
use crate::error::ConnectionError;
use crate::login::LoggedInProfile;
use crate::server::{ServerConfig, WorldHandle};

/// How often we ping the client. Vanilla's value.
pub const KEEPALIVE_PERIOD: Duration = Duration::from_secs(15);
/// How long we wait for the client's echo before disconnecting. Vanilla's value.
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);

const SPAWN_X: f64 = 0.5;
// The bundled test world uses vanilla's flat-preset surface: bedrock
// at Y=-64, dirt at Y=-63..-62, grass at Y=-61. Spawn one block
// above the grass so the client lands cleanly without freefall.
// (M3's old SPAWN_Y=64 worked only because the chunk burst was fast
// enough to land before the client picked up physics; M4's slower
// debug-mode burst exposed the latent bug.)
const DEFAULT_SPAWN_Y: f64 = -59.0;
const SPAWN_Z: f64 = 0.5;

/// Chunk radius around spawn the server flushes before the keepalive
/// loop starts. Currently matches `LoginPlay.view_distance` so the
/// client renders right up to the announced render distance; tuning
/// is M3.e/M3.f work.
const SPAWN_VIEW_DISTANCE: i32 = 10;

/// Pack `(x, y, z)` into vanilla's `BlockPos` `i64` representation.
/// Currently used only by tests but kept here for the eventual
/// re-introduction of `SetDefaultSpawnPosition` and other block-pos
/// carrying clientbound packets.
#[allow(dead_code)]
fn pack_block_pos(x: i32, y: i32, z: i32) -> i64 {
    (((x as i64) & 0x3FF_FFFF) << 38) | (((z as i64) & 0x3FF_FFFF) << 12) | ((y as i64) & 0xFFF)
}

/// Pick the dimension that the player will spawn into. We pick the first
/// alphabetical entry of `dimension_type` for both real vanilla data
/// (`minecraft:overworld`) and test stubs (`minecraft:alpha`).
fn spawn_dimension(data: &VanillaData) -> Option<(i32, &Identifier, &[Identifier])> {
    let registry = data.registry("dimension_type")?;
    let first = registry.entries.first()?;
    Some((0, first, registry.entries.as_slice()))
}

async fn spawn_position(config: &ServerConfig) -> (f64, f64, f64) {
    let y = adaptive_spawn_y(config).await.unwrap_or(DEFAULT_SPAWN_Y);
    (SPAWN_X, y, SPAWN_Z)
}

async fn adaptive_spawn_y(config: &ServerConfig) -> Option<f64> {
    let world = config.world.as_ref()?;
    let table = config.block_light.as_deref()?;
    let mut storage = world.lock().await;
    let chunk = storage.get_chunk_mut(ChunkPos { x: 0, z: 0 }).ok()??;
    chunk.rebuild_highest_opaque(table);
    let top = chunk.highest_opaque_y(0, 0)?;
    Some((top + 2) as f64)
}

pub(crate) async fn handle<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    profile: &LoggedInProfile,
    config: &ServerConfig,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // `blocks` rides along on the config — currently unused by the
    // Play handler because the chunk encoder reads palette IDs straight
    // from the chunk; it'll matter once we synthesise placeholder
    // chunks or do block-update packets.
    let _ = &config.blocks;
    let data: &VanillaData = &config.data;
    let (dim_id, dim_name, dim_names) = spawn_dimension(data).ok_or_else(|| {
        ConnectionError::Codec(mc_protocol::CodecError::InvalidIdentifier(
            "no dimension_type entries available".into(),
        ))
    })?;

    info!(
        player = %profile.name,
        uuid = %profile.uuid,
        spawn_dimension = %dim_name,
        "entering Play state"
    );

    let (spawn_x, spawn_y, spawn_z) = spawn_position(config).await;

    // 1. Login (Play).
    let login = LoginPlay {
        entity_id: 1,
        is_hardcore: false,
        dimension_names: dim_names.to_vec(),
        max_players: 20,
        view_distance: 10,
        simulation_distance: 10,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        do_limited_crafting: false,
        dimension_type_id: dim_id,
        dimension_name: dim_name.clone(),
        hashed_seed: 0,
        game_mode: 0, // survival
        previous_game_mode: -1,
        is_debug: false,
        is_flat: true,
        death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
        enforces_secure_chat: false,
    };
    write_packet(writer, &login, Compression::Disabled).await?;

    // 2. Synchronize Player Position. teleport_id=1; we'll watch for
    //    `ConfirmTeleportation(1)` in the loop below but don't block
    //    on it — if the client ignores it the world still loads, just
    //    desynced.
    write_packet(
        writer,
        &SynchronizePlayerPosition {
            teleport_id: 1,
            x: spawn_x,
            y: spawn_y,
            z: spawn_z,
            dx: 0.0,
            dy: 0.0,
            dz: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            relative_flags: 0,
        },
        Compression::Disabled,
    )
    .await?;

    // 3. Set Default Spawn Position — was historically sent here to set
    //    the compass anchor. Skipped in M1.g: in the 26.1.2 wire capture
    //    the matching 8-byte clientbound packet looks like its layout
    //    changed (no `angle` field), and its ID is uncertain. The
    //    client renders without a configured compass target — minor
    //    cosmetic regression, not a protocol error. Re-introduce once
    //    the new shape is verified.

    // 4. Game Event: start waiting for chunks. Tells the client to
    //    drop the loading screen even though no chunks are coming.
    write_packet(
        writer,
        &GameEvent {
            event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
            value: 0.0,
        },
        Compression::Disabled,
    )
    .await?;

    // 5. Set Center Chunk + view-distance window. Spawn is at
    //    (SPAWN_X, SPAWN_Z); the chunk anchor is the chunk that
    //    contains it, and we stream ±SPAWN_VIEW_DISTANCE around it.
    let (spawn_cx, spawn_cz) = spawn_chunk_pos();
    write_packet(
        writer,
        &SetCenterChunk {
            chunk_x: spawn_cx,
            chunk_z: spawn_cz,
        },
        Compression::Disabled,
    )
    .await?;

    let mut light_cache = LightCache::new();
    if let Some(world) = config.world.as_ref() {
        emit_chunks_around(
            writer,
            world,
            data,
            config.block_light.as_deref(),
            &mut light_cache,
            spawn_cx,
            spawn_cz,
            SPAWN_VIEW_DISTANCE,
        )
        .await?;
    }

    // 6. Seed the player inventory (M6). Send SetHeldSlot{0} +
    //    ContainerSetContent with the starter kit. Doing this even
    //    when there's no world configured so the hotbar UI fills in
    //    on connect-without-world (the M6 manual gate uses a world,
    //    but tests / chunk-less debug runs should still display the
    //    kit). When the item registry is empty (test stubs) the
    //    starter inventory is mostly empty stacks — packets ship
    //    anyway, just with `count == 0` slots.
    let starter = build_starter_inventory(&config.items);
    write_packet(
        writer,
        &ClientboundSetHeldSlot { slot: 0 },
        Compression::Disabled,
    )
    .await?;
    write_packet(
        writer,
        &ClientboundContainerSetContent {
            container_id: 0,
            state_id: 1,
            items: starter.as_wire_list(),
            carried_item: ItemStack::EMPTY,
        },
        Compression::Disabled,
    )
    .await?;

    // 7. Keepalive loop. Runs until the connection drops or the client
    //    misses a heartbeat by more than `KEEPALIVE_TIMEOUT`. The
    //    interaction state passes the M5.d/M5.e/M6.f break/place
    //    handlers everything they need to mutate the world and emit
    //    relight + container packets back to the client.
    let mut interaction = config.world.as_ref().map(|world| InteractionState {
        world: Arc::clone(world),
        blocks: Arc::clone(&config.blocks),
        block_light: config.block_light.as_ref().map(Arc::clone),
        workspace: LightWorkspace::new(),
        light_cache: std::mem::take(&mut light_cache),
        selected_hotbar_slot: 0,
        inventory: starter,
        inventory_state_id: 1,
        item_to_block: ItemToBlockTable::build(&config.items, &config.blocks),
    });
    keepalive_loop(reader, writer, buf, interaction.as_mut()).await
}

/// Per-connection state the M5.d / M5.e / M6 interaction handlers
/// carry.
struct InteractionState {
    world: WorldHandle,
    blocks: Arc<mc_world::BlockRegistry>,
    block_light: Option<Arc<BlockLightTable>>,
    /// Reused across all interaction-driven relight computes for
    /// the lifetime of the connection (same amortisation pattern
    /// as `emit_chunks_around`).
    workspace: LightWorkspace,
    /// M9.a: per-chunk computed light, populated during the spawn
    /// burst and mutated in place by [`apply_block_change_to_light`]
    /// on every edit. Replaces the M5-era pattern of recomputing
    /// the full 3×3 neighbourhood for each affected chunk on every
    /// break/place.
    light_cache: LightCache,
    /// M6.d: which item the player is currently holding. Bumped by
    /// `ServerboundSetCarriedItem` (0..=8) and consulted by
    /// `handle_use_item_on` to resolve the placed block.
    selected_hotbar_slot: u8,
    /// M6.e: a 46-slot window-0 inventory. Indices follow vanilla's
    /// numbering: 0..4 crafting (output + 2×2 input), 5..8 armor,
    /// 9..35 main rows, 36..44 hotbar, 45 offhand.
    inventory: PlayerInventory,
    /// M6.e: per-vanilla, the server bumps this counter on every
    /// inventory mutation it ships to the client; the client uses
    /// it to detect desyncs. Starts at 1 (after the seed
    /// ContainerSetContent on login).
    inventory_state_id: i32,
    /// M6.f: hard-coded item→block resolver for the M6 starter
    /// kit. Resolved once from the block registry at construction
    /// time; lookups are an `if`-ladder on the item id.
    item_to_block: ItemToBlockTable,
}

/// 46-slot player inventory (window 0).
///
/// Layout (vanilla wire numbering):
///   0       crafting result
///   1..=4   crafting 2×2 input
///   5..=8   armor (head, chest, legs, feet)
///   9..=35  main inventory rows
///   36..=44 hotbar
///   45      offhand
#[derive(Debug, Clone)]
struct PlayerInventory {
    slots: [ItemStack; 46],
}

impl PlayerInventory {
    /// Slot index where the hotbar begins on the wire.
    const HOTBAR_BASE: usize = 36;

    fn empty() -> Self {
        Self {
            slots: std::array::from_fn(|_| ItemStack::EMPTY),
        }
    }

    fn held(&self, hotbar_slot: u8) -> &ItemStack {
        &self.slots[Self::HOTBAR_BASE + hotbar_slot as usize]
    }

    fn held_mut(&mut self, hotbar_slot: u8) -> &mut ItemStack {
        &mut self.slots[Self::HOTBAR_BASE + hotbar_slot as usize]
    }

    fn as_wire_list(&self) -> Vec<ItemStack> {
        self.slots.to_vec()
    }
}

/// Hard-coded item→default-block-state lookup for the M6 starter
/// kit. Resolved once at connection setup from the block registry.
#[derive(Debug, Clone, Default)]
struct ItemToBlockTable {
    /// `(item_id, BlockStateId)` pairs. Stays tiny — full coverage
    /// is M9+ work.
    entries: Vec<(u32, mc_world::BlockStateId)>,
}

impl ItemToBlockTable {
    fn build(items: &ItemRegistry, blocks: &mc_world::BlockRegistry) -> Self {
        let starter_pairs = &[
            ("minecraft:stone", "minecraft:stone"),
            ("minecraft:dirt", "minecraft:dirt"),
            ("minecraft:oak_planks", "minecraft:oak_planks"),
            ("minecraft:torch", "minecraft:torch"),
        ];
        let mut entries = Vec::with_capacity(starter_pairs.len());
        for (item_name, block_name) in starter_pairs {
            let item_id = match mc_data::Identifier::parse(*item_name) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let block_id = match mc_data::Identifier::parse(*block_name) {
                Ok(id) => id,
                Err(_) => continue,
            };
            let Some(item_pid) = items.id_of(&item_id) else {
                continue;
            };
            let Some(block) = blocks.block(&block_id) else {
                continue;
            };
            entries.push((item_pid, block.default));
        }
        Self { entries }
    }

    fn resolve(&self, item_id: u32) -> Option<mc_world::BlockStateId> {
        self.entries
            .iter()
            .find_map(|(id, state)| (*id == item_id).then_some(*state))
    }
}

/// Build a starter-kit inventory: hotbar 0..=3 = stone, dirt, oak
/// planks, torch; everything else empty. Items the registry doesn't
/// resolve (test stubs without the real `items.json`) are quietly
/// dropped — the slot stays empty.
fn build_starter_inventory(items: &ItemRegistry) -> PlayerInventory {
    let mut inv = PlayerInventory::empty();
    let starter_kit: [(&str, i32); 4] = [
        ("minecraft:stone", 64),
        ("minecraft:dirt", 64),
        ("minecraft:oak_planks", 64),
        ("minecraft:torch", 64),
    ];
    for (i, (name, count)) in starter_kit.iter().enumerate() {
        if let Ok(id) = mc_data::Identifier::parse(*name)
            && let Some(item_id) = items.id_of(&id)
        {
            inv.slots[PlayerInventory::HOTBAR_BASE + i] = ItemStack::new(item_id, *count);
        }
    }
    inv
}

/// `(chunk_x, chunk_z)` for the constant spawn point. Implemented as a
/// fn rather than inlined so the math is unit-testable and so M3.e can
/// share the formula when it computes the view-distance ring.
fn spawn_chunk_pos() -> (i32, i32) {
    let cx = (SPAWN_X.floor() as i32).div_euclid(16);
    let cz = (SPAWN_Z.floor() as i32).div_euclid(16);
    (cx, cz)
}

/// Stream every chunk in the `(center ± view_distance)` window. Order
/// is row-major (z outer, x inner) — vanilla uses a spiral so the
/// player's spawn tile renders first, but for a static spawn this
/// distinction is invisible. Switch to spiral if a client visibly
/// hitches.
///
/// Per-chunk failure (missing region file, decode error, encode error)
/// is logged and skipped rather than killing the connection: the same
/// posture as the M3.d single-chunk path. The final summary log
/// records how many chunks made it onto the wire.
#[allow(clippy::too_many_arguments)]
async fn emit_chunks_around<W>(
    writer: &mut W,
    world: &WorldHandle,
    data: &VanillaData,
    block_light: Option<&BlockLightTable>,
    light_cache: &mut LightCache,
    center_cx: i32,
    center_cz: i32,
    view_distance: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(biomes) = data.registry("worldgen/biome") else {
        warn!("worldgen/biome registry missing; skipping chunk emission");
        return Ok(());
    };

    let started = Instant::now();

    // Pre-fetch every chunk in the (view_distance + 1) ring so each
    // emit_one_chunk can build its 3×3 neighbourhood from local
    // memory without re-locking the storage. The +1 captures the
    // outer neighbour ring needed to feed the lighting engine for
    // the cells at the edge of the view-distance window.
    let mut staged: HashMap<(i32, i32), Arc<Chunk>> = HashMap::new();
    let pre_fetch_radius = view_distance + 1;
    {
        let mut storage = world.lock().await;
        for cz in (center_cz - pre_fetch_radius)..=(center_cz + pre_fetch_radius) {
            for cx in (center_cx - pre_fetch_radius)..=(center_cx + pre_fetch_radius) {
                match storage.get_chunk(ChunkPos { x: cx, z: cz }) {
                    Ok(Some(c)) => {
                        staged.insert((cx, cz), Arc::new(c.clone()));
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!(cx, cz, error = %err, "chunk read failed; skipping");
                    }
                }
            }
        }
    }
    let staged_count = staged.len();
    let fetch_ms = started.elapsed().as_millis() as u64;

    let mut emitted = 0usize;
    let mut absent = 0usize;
    let mut bytes = 0usize;
    // One workspace reused for every chunk in the burst — without
    // this the per-chunk ~4 MB alloc + zero-fill blows the M4.f
    // harness timeout in debug builds.
    let mut workspace = block_light.is_some().then(LightWorkspace::new);

    // M8.b: spiral from the centre outwards so the player's chunk
    // renders first and the rings fill in behind. Same coverage as
    // the previous row-major loop.
    for (cx, cz) in spiral_chunks(center_cx, center_cz, view_distance) {
        let Some(centre) = staged.get(&(cx, cz)) else {
            absent += 1;
            debug!(cx, cz, "no chunk in storage");
            continue;
        };
        let neighbours = build_neighbourhood(&staged, cx, cz);
        let centre_ref = centre.as_ref();
        let packet = match build_chunk_packet(
            centre_ref,
            &neighbours,
            biomes,
            block_light,
            workspace.as_mut(),
            Some(light_cache),
            cx,
            cz,
        ) {
            Ok(p) => p,
            Err(err) => {
                warn!(cx, cz, error = %err, "chunk encode failed; skipping");
                continue;
            }
        };
        let n = packet.data.len();
        write_packet(writer, &packet, Compression::Disabled).await?;
        emitted += 1;
        bytes += n;
    }

    info!(
        center_cx,
        center_cz,
        view_distance,
        staged = staged_count,
        emitted,
        absent,
        bytes,
        fetch_ms,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "view-distance window flushed",
    );
    Ok(())
}

/// Iterate chunk positions around `(center_x, center_z)` outwards
/// to `view_distance` in chebyshev-ring order. The first cell is the
/// centre; subsequent yields are every cell on ring `r = 1`, then
/// every cell on ring `r = 2`, etc. Within a ring the order is
/// row-major over the bounding square — perceptually this still
/// "spreads" because each ring fills before the next starts.
/// Coverage is identical to a row-major scan: `(2*view_distance +
/// 1)²` cells total, each yielded exactly once.
fn spiral_chunks(
    center_x: i32,
    center_z: i32,
    view_distance: i32,
) -> impl Iterator<Item = (i32, i32)> {
    let vd = view_distance.max(0);
    let mut out = Vec::with_capacity(((2 * vd + 1).pow(2)) as usize);
    out.push((center_x, center_z));
    for r in 1..=vd {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dz.abs()) == r {
                    out.push((center_x + dx, center_z + dz));
                }
            }
        }
    }
    out.into_iter()
}

/// Build the 3×3 neighbourhood centred on `(cx, cz)`. The centre is
/// not populated here — the caller already has it as `centre_ref` and
/// passes it through `build_chunk_packet` separately.
fn build_neighbourhood(
    staged: &HashMap<(i32, i32), Arc<Chunk>>,
    cx: i32,
    cz: i32,
) -> [[Option<Arc<Chunk>>; 3]; 3] {
    std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            let nx = cx + (dx as i32 - 1);
            let nz = cz + (dz as i32 - 1);
            staged.get(&(nx, nz)).cloned()
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn build_chunk_packet(
    centre: &Chunk,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
    biomes: &Registry,
    block_light: Option<&BlockLightTable>,
    workspace: Option<&mut LightWorkspace>,
    light_cache: Option<&mut LightCache>,
    cx: i32,
    cz: i32,
) -> Result<LevelChunkWithLight, mc_world::wire::WireError> {
    let data = encode_chunk_data(centre, biomes)?;
    let heightmaps = client_heightmaps(centre)
        .into_iter()
        .map(|h| ChunkHeightmap {
            type_id: h.type_id,
            data: h.data,
        })
        .collect();
    let light = match (block_light, workspace) {
        (Some(table), Some(ws)) => {
            // Centre slot is the chunk we already have a reference to;
            // off-centre slots come from the staged map.
            let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
            for (dz, row) in neighbourhood.iter().enumerate() {
                for (dx, slot) in row.iter().enumerate() {
                    refs[dz][dx] = slot.as_deref();
                }
            }
            refs[1][1] = Some(centre);
            let computed = compute_chunk_light_in(ws, refs, table);
            let wire = encode_chunk_light(&computed);
            // M9.a: stash the computed light in the per-connection
            // cache so subsequent edits hit the incremental path.
            if let Some(cache) = light_cache {
                cache.insert(ChunkPos { x: cx, z: cz }, computed);
            }
            LightData {
                sky_y_mask: wire.sky_y_mask,
                block_y_mask: wire.block_y_mask,
                empty_sky_y_mask: wire.empty_sky_y_mask,
                empty_block_y_mask: wire.empty_block_y_mask,
                sky_updates: wire.sky_updates,
                block_updates: wire.block_updates,
            }
        }
        _ => LightData::empty(),
    };
    Ok(LevelChunkWithLight {
        chunk_x: cx,
        chunk_z: cz,
        heightmaps,
        data,
        block_entities: Vec::new(),
        light,
    })
}

/// M5.d: handle a serverbound `PlayerAction`. Acts on
/// `START_DESTROY_BLOCK` / `STOP_DESTROY_BLOCK` (creative-style
/// instant break — survival mining timing arrives with a future
/// inventory milestone) by setting the target cell to air,
/// recomputing light over the affected 1+4 chunks, and emitting
/// the resulting wire packets back to the client. Every action
/// regardless of kind gets an ack so the client's prediction
/// state doesn't hang.
async fn handle_player_action<W>(
    state: &mut InteractionState,
    writer: &mut W,
    action: ServerboundPlayerAction,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let is_destroy = matches!(
        action.action,
        PlayerActionKind::StartDestroyBlock | PlayerActionKind::StopDestroyBlock
    );
    if !is_destroy {
        // DROP_*, RELEASE_USE_ITEM, SWAP_ITEM_WITH_OFFHAND, STAB —
        // all out of scope for M5. Ack so the client doesn't hang
        // on a prediction.
        debug!(
            action = ?action.action,
            sequence = action.sequence,
            "non-destroy player action ignored"
        );
        write_packet(
            writer,
            &BlockChangedAck {
                sequence: action.sequence,
            },
            Compression::Disabled,
        )
        .await?;
        return Ok(());
    }

    let (x, y, z) = unpack_block_pos(action.position);
    let air = air_state_id(&state.blocks);

    apply_block_edit(state, writer, action.sequence, x, y, z, air).await
}

/// Shared back-half of the break / place flows: mutates the world
/// at `(x, y, z)` to `new_state`, broadcasts `BlockUpdate` +
/// recomputed `LightUpdate`s + `BlockChangedAck` to the connected
/// client. The actual edit is applied via
/// `WorldStorage::set_block_at` so heightmap recompute lands in
/// the same atomic step.
async fn apply_block_edit<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    x: i32,
    y: i32,
    z: i32,
    new_state: mc_world::BlockStateId,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let pos = mc_world::BlockPos { x, y, z };
    let table = state.block_light.as_ref().map(Arc::clone);

    // 1. Apply the mutation. Drop the lock as soon as it's done so
    //    the light-recompute path can re-acquire it for the
    //    neighbourhood read without deadlocking.
    let prev = {
        let mut storage = state.world.lock().await;
        match storage.set_block_at(pos, new_state) {
            Ok(p) => {
                if let (Some(prev), Some(table)) = (p, table.as_deref())
                    && prev != new_state
                    && let Err(err) = storage.update_highest_opaque_at(pos, table)
                {
                    warn!(error = %err, x, y, z, "highest-opaque heightmap update failed");
                }
                p
            }
            Err(err) => {
                warn!(error = %err, x, y, z, "set_block_at failed; skipping edit");
                write_packet(writer, &BlockChangedAck { sequence }, Compression::Disabled).await?;
                return Ok(());
            }
        }
    };

    // Absent chunk or no-op edit: ack the sequence so the client
    // doesn't roll back forever, but skip the BlockUpdate /
    // LightUpdate ripple.
    let Some(prev) = prev else {
        write_packet(writer, &BlockChangedAck { sequence }, Compression::Disabled).await?;
        return Ok(());
    };
    if prev == new_state {
        write_packet(writer, &BlockChangedAck { sequence }, Compression::Disabled).await?;
        return Ok(());
    }

    // 2. Tell the client about the new block.
    write_packet(
        writer,
        &BlockUpdate {
            position: mc_protocol::packets::play::pack_block_pos(x, y, z),
            state_id: new_state.0 as i32,
        },
        Compression::Disabled,
    )
    .await?;

    // 3. Incremental relight (M9). Update cached per-chunk light in
    //    place via bounded BFS, then emit one `LightUpdate` per
    //    chunk whose stored arrays actually changed. Falls back to
    //    no-op if the cache is empty (e.g. edits before the spawn
    //    burst populated it) or the block-light table isn't loaded.
    if let Some(table) = table {
        let cx = x.div_euclid(16);
        let cz = z.div_euclid(16);
        send_incremental_relight(state, writer, &table, cx, cz, x, y, z, prev, new_state).await?;
    }

    // 4. Ack last — vanilla expects update-before-ack so the
    //    prediction reconciles against a known state.
    write_packet(writer, &BlockChangedAck { sequence }, Compression::Disabled).await?;
    Ok(())
}

/// M9: incremental relight. Pulls the post-edit 3×3 chunk
/// neighbourhood out of storage once, runs a bounded BFS that
/// mutates the per-chunk cached light in place, and emits one
/// `LightUpdate` per chunk whose stored arrays changed.
///
/// Falls back to a single-chunk full recompute when the cache
/// hasn't been pre-warmed (e.g. an edit lands before the spawn
/// burst's `build_chunk_packet` got to that chunk) — same coverage
/// as the old `send_relight_around` for the centre tile, but
/// without the 5× cost.
#[allow(clippy::too_many_arguments)]
async fn send_incremental_relight<W>(
    state: &mut InteractionState,
    writer: &mut W,
    table: &BlockLightTable,
    cx: i32,
    cz: i32,
    edit_x: i32,
    edit_y: i32,
    edit_z: i32,
    prev_state: mc_world::BlockStateId,
    new_state: mc_world::BlockStateId,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    // 1. Pull the 3×3 chunks around the edit out of storage. The
    //    edit has already been applied, so these are post-edit.
    let mut chunks: HashMap<(i32, i32), Arc<Chunk>> = HashMap::new();
    {
        let mut storage = state.world.lock().await;
        for dcz in -1i32..=1 {
            for dcx in -1i32..=1 {
                let pos = ChunkPos {
                    x: cx + dcx,
                    z: cz + dcz,
                };
                match storage.get_chunk(pos) {
                    Ok(Some(c)) => {
                        chunks.insert((cx + dcx, cz + dcz), Arc::new(c.clone()));
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!(error = %err, cx = pos.x, cz = pos.z, "relight neighbour read failed");
                    }
                }
            }
        }
    }

    let centre_pos = ChunkPos { x: cx, z: cz };

    // 2. If the edit chunk isn't in the cache yet (rare: edit before
    //    spawn burst reached it), seed it via a single full compute.
    if !state.light_cache.contains(centre_pos) {
        let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                refs[(dz + 1) as usize][(dx + 1) as usize] =
                    chunks.get(&(cx + dx, cz + dz)).map(|a| a.as_ref());
            }
        }
        let Some(centre) = refs[1][1] else {
            return Ok(()); // edit chunk vanished from storage — nothing to relight
        };
        let _ = centre;
        let light = compute_chunk_light_in(&mut state.workspace, refs, table);
        state.light_cache.insert(centre_pos, light);
    }

    // 3. Build the 3×3 reference array for the incremental update.
    let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            refs[(dz + 1) as usize][(dx + 1) as usize] =
                chunks.get(&(cx + dx, cz + dz)).map(|a| a.as_ref());
        }
    }

    let local_x = edit_x.rem_euclid(16) as u8;
    let local_z = edit_z.rem_euclid(16) as u8;

    let touched = apply_block_change_to_light(
        &mut state.light_cache,
        &refs,
        table,
        centre_pos,
        local_x,
        edit_y,
        local_z,
        prev_state,
        new_state,
    );

    // 4. Emit one LightUpdate per chunk whose cached light changed.
    for pos in touched {
        let Some(light) = state.light_cache.get(pos) else {
            continue;
        };
        let wire = encode_chunk_light(light);
        let light_data = LightData {
            sky_y_mask: wire.sky_y_mask,
            block_y_mask: wire.block_y_mask,
            empty_sky_y_mask: wire.empty_sky_y_mask,
            empty_block_y_mask: wire.empty_block_y_mask,
            sky_updates: wire.sky_updates,
            block_updates: wire.block_updates,
        };
        write_packet(
            writer,
            &LightUpdate {
                chunk_x: pos.x,
                chunk_z: pos.z,
                light: light_data,
            },
            Compression::Disabled,
        )
        .await?;
    }
    Ok(())
}

fn air_state_id(registry: &mc_world::BlockRegistry) -> mc_world::BlockStateId {
    let air_id = mc_data::Identifier::parse("minecraft:air").expect("static identifier");
    registry
        .block(&air_id)
        .map(|b| b.default)
        .unwrap_or(mc_world::BlockStateId(0))
}

/// M6.f: handle a serverbound `UseItemOn`. Resolves the placed
/// block via the player's currently-held hotbar slot through the
/// item→block table. Drops the placement silently (still acking) if
/// the held stack is empty, if the held item has no block mapping
/// (e.g. food, tool), or if the target cell is non-air. On a
/// successful placement decrements the held stack's count and emits
/// `ContainerSetSlot` so the client sees the new count.
async fn handle_use_item_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    action: ServerboundUseItemOn,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (cx, cy, cz) = unpack_block_pos(action.position);
    let (dx, dy, dz) = action.direction.normal();
    let (tx, ty, tz) = (cx + dx, cy + dy, cz + dz);

    let air = air_state_id(&state.blocks);

    // M6.f: resolve the placed block from the held item.
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot).clone();
    let placed_state = if held.is_empty() {
        None
    } else {
        state.item_to_block.resolve(held.item_id)
    };
    let Some(placed_state) = placed_state else {
        debug!(
            sequence = action.sequence,
            held_item = held.item_id,
            held_count = held.count,
            "UseItemOn: held item is empty or not placeable; skipping"
        );
        write_packet(
            writer,
            &BlockChangedAck {
                sequence: action.sequence,
            },
            Compression::Disabled,
        )
        .await?;
        return Ok(());
    };

    // Validate: target cell must currently be air. We can borrow
    // the world briefly to peek; if the cell is non-air or absent,
    // skip the edit but still ack.
    let target_is_air = {
        let mut storage = state.world.lock().await;
        match storage.get_block(mc_world::BlockPos {
            x: tx,
            y: ty,
            z: tz,
        }) {
            Ok(Some(current)) => current == air,
            Ok(None) => false,
            Err(err) => {
                warn!(error = %err, x = tx, y = ty, z = tz, "UseItemOn target read failed");
                false
            }
        }
    };
    if !target_is_air {
        debug!(
            x = tx,
            y = ty,
            z = tz,
            "UseItemOn target not air; skipping placement"
        );
        write_packet(
            writer,
            &BlockChangedAck {
                sequence: action.sequence,
            },
            Compression::Disabled,
        )
        .await?;
        return Ok(());
    }

    apply_block_edit(state, writer, action.sequence, tx, ty, tz, placed_state).await?;

    // M6.f: decrement the held stack's count + tell the client the
    // new slot contents. Empty stacks ship as `count == 0`.
    {
        let held = state.inventory.held_mut(held_slot);
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
    }
    state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
    let new_slot_value = state.inventory.held(held_slot).clone();
    write_packet(
        writer,
        &ClientboundContainerSetSlot {
            container_id: 0,
            state_id: state.inventory_state_id,
            slot: (PlayerInventory::HOTBAR_BASE + held_slot as usize) as i16,
            item_stack: new_slot_value,
        },
        Compression::Disabled,
    )
    .await?;
    Ok(())
}

async fn keepalive_loop<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
    mut interaction: Option<&mut InteractionState>,
) -> Result<(), ConnectionError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut ticker = interval(KEEPALIVE_PERIOD);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // The first `tick()` resolves immediately; drop it so we don't
    // race-send a keepalive before the client has finished processing
    // the spawn burst.
    ticker.tick().await;

    let mut next_id: i64 = 0;
    let mut last_response_at = Instant::now();
    let mut pending_id: Option<i64> = None;

    loop {
        tokio::select! {
            biased;
            _ = ticker.tick() => {
                if last_response_at.elapsed() > KEEPALIVE_TIMEOUT {
                    warn!(
                        elapsed_ms = last_response_at.elapsed().as_millis() as u64,
                        "client missed keepalive deadline; closing"
                    );
                    return Ok(());
                }
                next_id = next_id.wrapping_add(1).max(1);
                pending_id = Some(next_id);
                write_packet(
                    writer,
                    &ClientboundKeepAlive { id: next_id },
                    Compression::Disabled,
                )
                .await?;
            }
            result = read_frame(reader, buf, Compression::Disabled) => {
                let frame = result?;
                if frame.id == ServerboundKeepAlive::ID {
                    let mut body = frame.body;
                    let echo = ServerboundKeepAlive::decode(&mut body)?;
                    if pending_id == Some(echo.id) {
                        last_response_at = Instant::now();
                        pending_id = None;
                    } else {
                        warn!(
                            expected = ?pending_id,
                            received = echo.id,
                            "keepalive id mismatch"
                        );
                    }
                } else if frame.id == ConfirmTeleportation::ID {
                    let mut body = frame.body;
                    let confirm = ConfirmTeleportation::decode(&mut body)?;
                    debug!(teleport_id = confirm.teleport_id, "teleport confirmed");
                } else if frame.id == ServerboundPlayerAction::ID {
                    let mut body = frame.body;
                    let action = ServerboundPlayerAction::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_player_action(state, writer, action).await?;
                    } else {
                        debug!(
                            action = ?action.action,
                            sequence = action.sequence,
                            "PlayerAction ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundUseItemOn::ID {
                    let mut body = frame.body;
                    let use_on = ServerboundUseItemOn::decode(&mut body)?;
                    if let Some(state) = interaction.as_deref_mut() {
                        handle_use_item_on(state, writer, use_on).await?;
                    } else {
                        debug!(
                            sequence = use_on.sequence,
                            "UseItemOn ignored — no world configured"
                        );
                    }
                } else if frame.id == ServerboundSetCarriedItem::ID {
                    let mut body = frame.body;
                    let pick = ServerboundSetCarriedItem::decode(&mut body)?;
                    let slot = pick.slot.clamp(0, 8) as u8;
                    if let Some(state) = interaction.as_deref_mut() {
                        state.selected_hotbar_slot = slot;
                        debug!(slot, "hotbar selection updated");
                    }
                } else {
                    debug!(
                        id = format!("{:#04x}", frame.id),
                        "play packet ignored"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_block_pos_round_trip() {
        // The packed-i64 representation is bit-exact what vanilla wants.
        // Just confirm the formula does not panic and that nominal
        // origin packs to 0.
        assert_eq!(pack_block_pos(0, 0, 0), 0);
        assert_ne!(pack_block_pos(1, 0, 0), 0);
        assert_ne!(pack_block_pos(0, 1, 0), 0);
        assert_ne!(pack_block_pos(0, 0, 1), 0);
    }

    #[test]
    fn spawn_chunk_pos_matches_origin() {
        // SPAWN_(X,Z) = (0.5, 0.5); the containing chunk is (0, 0).
        assert_eq!(spawn_chunk_pos(), (0, 0));
    }

    #[test]
    fn spiral_chunks_starts_at_centre() {
        let mut iter = spiral_chunks(3, -7, 2);
        assert_eq!(iter.next(), Some((3, -7)));
    }

    #[test]
    fn spiral_chunks_covers_every_cell_in_window() {
        for vd in 0..=4 {
            let collected: std::collections::HashSet<(i32, i32)> =
                spiral_chunks(0, 0, vd).collect();
            let expected_count = ((2 * vd + 1) as usize).pow(2);
            assert_eq!(
                collected.len(),
                expected_count,
                "vd={vd}: spiral should yield {expected_count} unique cells",
            );
            for dz in -vd..=vd {
                for dx in -vd..=vd {
                    assert!(
                        collected.contains(&(dx, dz)),
                        "vd={vd}: missing cell ({dx},{dz})"
                    );
                }
            }
        }
    }

    #[test]
    fn spiral_chunks_ring_order_monotonic() {
        // Within the iteration, the chebyshev distance must be
        // non-decreasing. That's the property that makes the
        // perceptual spread feel like a spiral rather than a scan.
        let mut last_ring = -1i32;
        for (dx, dz) in spiral_chunks(0, 0, 3) {
            let r = dx.abs().max(dz.abs());
            assert!(
                r >= last_ring,
                "non-monotonic ring sequence at cell ({dx},{dz}): r={r} < last={last_ring}"
            );
            last_ring = r;
        }
    }

    #[test]
    fn spawn_dimension_prefers_alphabetical_first() {
        let data = mc_data::testing::stub();
        let (id, name, all) = spawn_dimension(&data).unwrap();
        assert_eq!(id, 0);
        assert_eq!(name.as_str(), "minecraft:alpha");
        assert_eq!(all.len(), 2);
    }
}
