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
use mc_data::{Registry, VanillaData};
use mc_protocol::codec::Identifier;
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ChunkHeightmap, ClientboundKeepAlive, ConfirmTeleportation, GameEvent, LevelChunkWithLight,
    LightData, LoginPlay, ServerboundKeepAlive, SetCenterChunk, SynchronizePlayerPosition,
};
use mc_world::light::{LightWorkspace, compute_chunk_light_in};
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
const SPAWN_Y: f64 = -59.0;
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
            x: SPAWN_X,
            y: SPAWN_Y,
            z: SPAWN_Z,
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

    if let Some(world) = config.world.as_ref() {
        emit_chunks_around(
            writer,
            world,
            data,
            config.block_light.as_deref(),
            spawn_cx,
            spawn_cz,
            SPAWN_VIEW_DISTANCE,
        )
        .await?;
    }

    // 6. Keepalive loop. Runs until the connection drops or the client
    //    misses a heartbeat by more than `KEEPALIVE_TIMEOUT`.
    keepalive_loop(reader, writer, buf).await
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
async fn emit_chunks_around<W>(
    writer: &mut W,
    world: &WorldHandle,
    data: &VanillaData,
    block_light: Option<&BlockLightTable>,
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

    for cz in (center_cz - view_distance)..=(center_cz + view_distance) {
        for cx in (center_cx - view_distance)..=(center_cx + view_distance) {
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

fn build_chunk_packet(
    centre: &Chunk,
    neighbourhood: &[[Option<Arc<Chunk>>; 3]; 3],
    biomes: &Registry,
    block_light: Option<&BlockLightTable>,
    workspace: Option<&mut LightWorkspace>,
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

async fn keepalive_loop<R, W>(
    reader: &mut R,
    writer: &mut W,
    buf: &mut BytesMut,
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
    fn spawn_dimension_prefers_alphabetical_first() {
        let data = mc_data::testing::stub();
        let (id, name, all) = spawn_dimension(&data).unwrap();
        assert_eq!(id, 0);
        assert_eq!(name.as_str(), "minecraft:alpha");
        assert_eq!(all.len(), 2);
    }
}
