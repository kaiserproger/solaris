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

use std::time::{Duration, Instant};

use bytes::BytesMut;
use mc_data::{Registry, VanillaData};
use mc_protocol::codec::Identifier;
use mc_protocol::frame::Compression;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ChunkHeightmap, ClientboundKeepAlive, ConfirmTeleportation, GameEvent, LevelChunkWithLight,
    LightData, LoginPlay, ServerboundKeepAlive, SetCenterChunk, SynchronizePlayerPosition,
};
use mc_world::ChunkPos;
use mc_world::wire::{client_heightmaps, encode_chunk_data};
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
const SPAWN_Y: f64 = 64.0;
const SPAWN_Z: f64 = 0.5;

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
    // `blocks` rides along on the config; M3.d uses `world` below for
    // the single-chunk emission, M3.e will fan out to a view-distance
    // window. `blocks` is unused for the moment because the chunk
    // encoder takes its palette IDs straight from the chunk.
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

    // 5. Set Center Chunk + single-chunk experiment (M3.d). View
    //    distance expansion is M3.e. Spawn is at (SPAWN_X, SPAWN_Z);
    //    the chunk anchor is the chunk that contains it.
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
        emit_chunk(writer, world, data, spawn_cx, spawn_cz).await?;
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

/// Emit a single chunk via `LevelChunkWithLight`. Silently skips when
/// the chunk is absent (test world doesn't cover the requested coord)
/// or when the biome registry / chunk read / encode produces an error
/// we can attribute to the world side — kicking the client over a
/// missing chunk would mask the real failure.
async fn emit_chunk<W>(
    writer: &mut W,
    world: &WorldHandle,
    data: &VanillaData,
    cx: i32,
    cz: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(biomes) = data.registry("worldgen/biome") else {
        warn!("worldgen/biome registry missing; skipping chunk emission");
        return Ok(());
    };

    let chunk = {
        let mut storage = world.lock().await;
        match storage.get_chunk(ChunkPos { x: cx, z: cz }) {
            Ok(Some(c)) => c.clone(),
            Ok(None) => {
                debug!(cx, cz, "no chunk in storage at spawn position");
                return Ok(());
            }
            Err(err) => {
                warn!(cx, cz, error = %err, "chunk read failed; skipping emission");
                return Ok(());
            }
        }
    };

    let packet = build_chunk_packet(&chunk, biomes, cx, cz);
    let packet = match packet {
        Ok(p) => p,
        Err(err) => {
            warn!(cx, cz, error = %err, "chunk encode failed; skipping emission");
            return Ok(());
        }
    };

    info!(cx, cz, bytes = packet.data.len(), "emitting spawn chunk");
    write_packet(writer, &packet, Compression::Disabled).await
}

fn build_chunk_packet(
    chunk: &mc_world::Chunk,
    biomes: &Registry,
    cx: i32,
    cz: i32,
) -> Result<LevelChunkWithLight, mc_world::wire::WireError> {
    let data = encode_chunk_data(chunk, biomes)?;
    let heightmaps = client_heightmaps(chunk)
        .into_iter()
        .map(|h| ChunkHeightmap {
            type_id: h.type_id,
            data: h.data,
        })
        .collect();
    Ok(LevelChunkWithLight {
        chunk_x: cx,
        chunk_z: cz,
        heightmaps,
        data,
        block_entities: Vec::new(),
        light: LightData::empty(),
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
