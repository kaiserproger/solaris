//! Play-state packet bodies whose layout or registry ids differ between 26.3
//! (canonical) and 26.1.2. Every other packet keeps its bytes; only its id is
//! remapped by the caller. Sources: ADR 0015 and the 26.3 javap/datagen delta.

use anyhow::{Context, Result, bail, ensure};
use bytes::Buf;
use mc_protocol::codec::{ReadMc, WriteMc};

use super::chunk::{Container, rewrite_biome_sections, rewrite_sections};
use super::items::{self, To, copy_nbt};
use super::tables::{Metadata, Tables};
use super::{Session, Teleport};

const LEVEL_EVENT_BLOCK_BREAK: i32 = 2001;
const METADATA_END: u8 = 0xFF;

/// The 26.1.2 packet a canonical clientbound packet is sent as.
pub(super) fn legacy_name(canonical: &str) -> &str {
    match canonical {
        "minecraft:swing_animation" => "minecraft:animate",
        other => other,
    }
}

/// Translate one canonical clientbound Play body. `Ok(None)` drops it.
pub(super) fn clientbound(
    session: &mut Session,
    name: &str,
    body: &[u8],
) -> Result<Option<Vec<u8>>> {
    let t = &session.profile.tables;
    let mut r = body;
    let mut w = Vec::with_capacity(body.len() + 8);
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        // 26.3-only packets.
        "add_transient_block" | "post_effects" => return Ok(None),
        // Play-state tags carry canonical ids; the Configuration tags stand.
        "update_tags" => return Ok(None),
        "login" => login(&mut r, &mut w)?,
        "respawn" => {
            spawn_info(&mut r, &mut w)?;
            w.extend_from_slice(r);
            r = &[];
        }
        "level_chunk_with_light" => chunk(t, &mut r, &mut w)?,
        "light_update" => {
            w.write_varint(r.read_varint()?);
            w.write_varint(r.read_varint()?);
            light_data(&mut r, &mut w)?;
        }
        "chunks_biomes" => {
            let n = r.read_varint()?;
            w.write_varint(n);
            for _ in 0..n {
                w.write_i64(r.read_i64()?);
                let data = r.read_byte_array(2 * 1024 * 1024)?;
                let mut out = Vec::with_capacity(data.len());
                rewrite_biome_sections(&data, &biomes(t), &mut out)?;
                w.write_byte_array(&out)?;
            }
        }
        "block_update" => {
            w.write_i64(r.read_i64()?);
            w.write_varint(state(t, r.read_varint()?));
        }
        "section_blocks_update" => {
            w.write_i64(r.read_i64()?);
            let n = r.read_varint()?;
            w.write_varint(n);
            for _ in 0..n {
                let entry = r.read_varlong()?;
                let id = state(t, (entry >> 12) as i32);
                w.write_varlong((i64::from(id) << 12) | (entry & 0xFFF));
            }
        }
        "block_entity_data" => {
            w.write_i64(r.read_i64()?);
            let native_kind = r.read_varint()?;
            let Some(kind) = t.block_entities.get(native_kind) else {
                return Ok(None);
            };
            w.write_varint(kind);
            copy_block_entity_nbt(native_kind, &mut r, &mut w)?;
        }
        "block_event" => {
            w.write_i64(r.read_i64()?);
            w.write_u8(r.read_u8()?);
            w.write_u8(r.read_u8()?);
            w.write_varint(t.blocks.get(r.read_varint()?).unwrap_or(1));
        }
        "level_event" => {
            let event = r.read_i32()?;
            w.write_i32(event);
            w.write_i64(r.read_i64()?);
            let data = r.read_i32()?;
            w.write_i32(if event == LEVEL_EVENT_BLOCK_BREAK {
                state(t, data)
            } else {
                data
            });
            w.write_bool(r.read_bool()?);
        }
        "add_entity" => {
            let entity = r.read_varint()?;
            w.write_varint(entity);
            w.write_uuid(r.read_uuid()?);
            let kind = r.read_varint()?;
            let Some(legacy_kind) = t.entities.get(kind) else {
                return Ok(None);
            };
            w.write_varint(legacy_kind);
            match t.entity_metadata.get(&kind) {
                Some(policy) => {
                    session.entities.insert(entity, *policy);
                }
                None => {
                    session.entities.remove(&entity);
                }
            }
            for _ in 0..3 {
                w.write_f64(r.read_f64()?);
            }
            copy_lp_vec3(&mut r, &mut w)?;
            for _ in 0..3 {
                w.write_u8(r.read_u8()?);
            }
            let data = r.read_varint()?;
            w.write_varint(if kind == t.falling_block {
                state(t, data)
            } else {
                data
            });
        }
        "remove_entities" => {
            let n = r.read_varint()?;
            w.write_varint(n);
            for _ in 0..n {
                let entity = r.read_varint()?;
                session.entities.remove(&entity);
                w.write_varint(entity);
            }
        }
        "set_entity_data" => {
            let entity = r.read_varint()?;
            w.write_varint(entity);
            let policy = session
                .entities
                .get(&entity)
                .copied()
                .unwrap_or(Metadata::Same);
            entity_data(t, policy, &mut r, &mut w)?;
        }
        "entity_position_sync" => {
            w.write_varint(r.read_varint()?);
            let position = match r.read_varint()? {
                0 => [r.read_f64()?, r.read_f64()?, r.read_f64()?],
                1 => bail!("stepped position path"),
                kind => bail!("position path type {kind}"),
            };
            for value in position {
                w.write_f64(value);
            }
            for _ in 0..3 {
                w.write_f64(0.0);
            }
            w.write_f32(r.read_f32()?);
            w.write_f32(r.read_f32()?);
            w.write_bool(r.read_bool()?);
        }
        "move_entity_pos" | "move_entity_pos_rot" => {
            w.write_varint(r.read_varint()?);
            let on_ground = moved_delta(&mut r, &mut w)?;
            if name.ends_with("_rot") {
                w.write_u8(r.read_u8()?);
                w.write_u8(r.read_u8()?);
            }
            w.write_bool(on_ground);
        }
        "move_entity_rot" => {
            w.write_varint(r.read_varint()?);
            let on_ground = r.read_bool()?;
            w.write_u8(r.read_u8()?);
            w.write_u8(r.read_u8()?);
            w.write_bool(on_ground);
        }
        "animate" => {
            w.write_varint(r.read_varint()?);
            // 26.3: 0 wake up, 1 critical hit, 2 magic critical hit.
            w.write_u8(match r.read_u8()? {
                0 => 2,
                1 => 4,
                2 => 5,
                _ => return Ok(None),
            });
        }
        "swing_animation" => {
            // -> 26.1.2 `animate` swing main (0) / off hand (3) (see `legacy_name`).
            w.write_varint(r.read_varint()?);
            w.write_u8(if r.read_varint()? == 0 { 0 } else { 3 });
            r.read_varint()?;
            r.read_varint()?;
        }
        "level_particles" => {
            let mut particle = Vec::new();
            if !particle_options(t, &mut r, &mut particle)? {
                return Ok(None);
            }
            w.write_bool(r.read_bool()?);
            w.write_bool(r.read_bool()?);
            for _ in 0..3 {
                w.write_f64(r.read_f64()?);
            }
            for _ in 0..3 {
                w.write_f32(r.read_f32()?);
            }
            let speed = r.read_f32()?;
            r.read_f32()?;
            r.read_f32()?;
            w.write_f32(speed);
            w.write_i32(r.read_varint()?);
            r.read_varint()?;
            w.extend_from_slice(&particle);
        }
        "explode" => {
            for _ in 0..3 {
                w.write_f64(r.read_f64()?);
            }
            w.write_f32(r.read_f32()?);
            w.write_i32(r.read_i32()?);
            if r.read_bool()? {
                w.write_bool(true);
                for _ in 0..3 {
                    w.write_f64(r.read_f64()?);
                }
            } else {
                w.write_bool(false);
            }
            if !particle_options(t, &mut r, &mut w)? {
                return Ok(None);
            }
            sound_holder(t, &mut r, &mut w)?;
            let n = r.read_varint()?;
            ensure!((0..=256).contains(&n), "explosion particle count");
            w.write_varint(n);
            for _ in 0..n {
                if !particle_options(t, &mut r, &mut w)? {
                    return Ok(None);
                }
                w.write_f32(r.read_f32()?);
                w.write_f32(r.read_f32()?);
                w.write_varint(r.read_varint()?);
            }
            r.read_bool()?;
        }
        "sound" | "sound_entity" => {
            sound_holder(t, &mut r, &mut w)?;
            w.extend_from_slice(r);
            r = &[];
        }
        "update_attributes" => {
            w.write_varint(r.read_varint()?);
            let n = r.read_varint()?;
            ensure!((0..=128).contains(&n), "attribute count");
            let mut kept = 0;
            let mut entries = Vec::new();
            for _ in 0..n {
                let attribute = t.attributes.get(r.read_varint()?);
                let mut entry = Vec::new();
                entry.write_f64(r.read_f64()?);
                let modifiers = r.read_varint()?;
                ensure!((0..=1024).contains(&modifiers), "modifier count");
                entry.write_varint(modifiers);
                for _ in 0..modifiers {
                    entry.write_identifier(&r.read_identifier()?)?;
                    entry.write_f64(r.read_f64()?);
                    entry.write_varint(r.read_varint()?);
                }
                if let Some(attribute) = attribute {
                    entries.write_varint(attribute);
                    entries.extend_from_slice(&entry);
                    kept += 1;
                }
            }
            w.write_varint(kept);
            w.extend_from_slice(&entries);
        }
        "damage_event" => {
            w.write_varint(r.read_varint()?);
            w.write_varint(t.damage_types.get(r.read_varint()?).unwrap_or(0));
            w.extend_from_slice(r);
            r = &[];
        }
        "set_player_team" => team(&mut r, &mut w)?,
        "container_set_content" => {
            let window = r.read_varint()?;
            w.write_varint(window);
            w.write_varint(r.read_varint()?);
            let n = r.read_varint()?;
            ensure!((0..=1024).contains(&n), "container size");
            w.write_varint(n);
            if window == 0 {
                session.inventory_polyfills.lock().clear();
            }
            for slot in 0..n {
                inventory_stack(
                    session,
                    (window == 0).then_some(slot as i16),
                    &mut r,
                    &mut w,
                )?;
            }
            items::stack(t, To::Legacy, &mut r, &mut w)?;
        }
        "container_set_slot" => {
            let window = r.read_varint()?;
            w.write_varint(window);
            w.write_varint(r.read_varint()?);
            let slot = r.read_i16()?;
            w.write_i16(slot);
            inventory_stack(session, (window == 0).then_some(slot), &mut r, &mut w)?;
        }
        "set_cursor_item" => items::stack(t, To::Legacy, &mut r, &mut w)?,
        "set_player_inventory" => {
            let slot = r.read_varint()?;
            w.write_varint(slot);
            let menu_slot = match slot {
                0..=8 => Some((slot + 36) as i16),
                9..=35 => Some(slot as i16),
                36..=39 => Some((44 - slot) as i16),
                40 => Some(45),
                _ => None,
            };
            inventory_stack(session, menu_slot, &mut r, &mut w)?;
        }
        "set_equipment" => {
            w.write_varint(r.read_varint()?);
            loop {
                let slot = r.read_u8()?;
                w.write_u8(slot);
                items::stack(t, To::Legacy, &mut r, &mut w)?;
                if slot & 0x80 == 0 {
                    break;
                }
            }
        }
        "player_position" => {
            let id = r.read_varint()?;
            let position = [r.read_f64()?, r.read_f64()?, r.read_f64()?];
            let rest = r;
            let mut tail = rest;
            for _ in 0..3 {
                tail.read_f64()?;
            }
            let rotation = [tail.read_f32()?, tail.read_f32()?];
            let relative = tail.read_i32()?;
            session.teleports.lock().sent.insert(
                id,
                Teleport {
                    position,
                    rotation,
                    relative,
                },
            );
            return Ok(Some(body.to_vec()));
        }
        "merchant_offers" => {
            w.write_varint(r.read_varint()?);
            let n = r.read_varint()?;
            ensure!((0..=256).contains(&n), "offer count");
            w.write_varint(n);
            for _ in 0..n {
                item_cost(t, &mut r, &mut w)?;
                items::stack(t, To::Legacy, &mut r, &mut w)?;
                let cost_b = r.read_bool()?;
                w.write_bool(cost_b);
                if cost_b {
                    item_cost(t, &mut r, &mut w)?;
                }
                w.write_bool(r.read_bool()?);
                for _ in 0..4 {
                    w.write_i32(r.read_i32()?);
                }
                w.write_f32(r.read_f32()?);
                w.write_i32(r.read_i32()?);
            }
            w.write_varint(r.read_varint()?);
            w.write_varint(r.read_varint()?);
            w.write_bool(r.read_bool()?);
            w.write_bool(r.read_bool()?);
        }
        "update_recipes"
        | "recipe_book_add"
        | "place_ghost_recipe"
        | "update_advancements"
        | "award_stats"
        | "commands" => {
            super::displays::clientbound(
                t,
                name.strip_prefix("minecraft:").unwrap_or(name),
                &mut r,
                &mut w,
            )?;
        }
        _ => return Ok(Some(body.to_vec())),
    }
    ensure!(r.is_empty(), "{} trailing bytes", r.len());
    Ok(Some(w))
}

/// Translate one legacy serverbound Play body. `Ok(None)` drops it.
pub(super) fn serverbound(
    session: &mut Session,
    name: &str,
    body: &[u8],
) -> Result<Option<Vec<u8>>> {
    let t = &session.profile.tables;
    let mut r = body;
    let mut w = Vec::with_capacity(body.len() + 40);
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "swing" => {
            let hand = r.read_varint()?;
            ensure!((0..=1).contains(&hand), "invalid swing hand");
            // 26.3 handles punching without a hand field.
        }
        "spectate_entity" => {
            let entity = r.read_varint()?;
            w.write_varint(entity.checked_add(1).context("spectator entity overflow")?);
        }
        "accept_teleportation" => {
            let id = r.read_varint()?;
            w.write_varint(id);
            let mut teleports = session.teleports.lock();
            let teleport = teleports.sent.remove(&id);
            let (position, rotation) = match (teleport, teleports.pose) {
                (Some(t), pose) => resolve(t, pose),
                (None, Some(pose)) => pose,
                (None, None) => ([0.0; 3], [0.0; 2]),
            };
            teleports.pose = Some((position, rotation));
            for value in position {
                w.write_f64(value);
            }
            w.write_f32(rotation[0]);
            w.write_f32(rotation[1]);
        }
        "move_player_pos" | "move_player_pos_rot" | "move_player_rot" => {
            let mut teleports = session.teleports.lock();
            let (mut position, mut rotation) = teleports.pose.unwrap_or(([0.0; 3], [0.0; 2]));
            let mut peek = body;
            if name.contains("_pos") {
                position = [peek.read_f64()?, peek.read_f64()?, peek.read_f64()?];
            }
            if name.ends_with("_rot") {
                rotation = [peek.read_f32()?, peek.read_f32()?];
            }
            teleports.pose = Some((position, rotation));
            return Ok(Some(body.to_vec()));
        }
        "player_action" => {
            // 26.3 inserts CHANGE_DESTROY_DIRECTION at ordinal 1.
            let action = r.read_varint()?;
            w.write_varint(if action >= 1 { action + 1 } else { action });
            w.extend_from_slice(r);
            r = &[];
        }
        "sign_update" => {
            w.write_i64(r.read_i64()?);
            let front = r.read_bool()?;
            for _ in 0..4 {
                w.write_string(&r.read_string(384)?, 384)?;
            }
            w.write_varint(i32::from(front));
        }
        "set_creative_mode_slot" => {
            let slot = r.read_i16()?;
            let mut polyfills = session.inventory_polyfills.lock();
            if polyfills
                .get(&slot)
                .is_some_and(|sent| sent.as_slice() == r)
            {
                return Ok(None);
            }
            polyfills.remove(&slot);
            w.write_i16(slot);
            items::stack(t, To::Canonical, &mut r, &mut w)?;
        }
        "container_click" => {
            w.write_varint(r.read_varint()?);
            w.write_varint(r.read_varint()?);
            w.write_i16(r.read_i16()?);
            w.write_i8(r.read_i8()?);
            w.write_varint(r.read_varint()?);
            let n = r.read_varint()?;
            ensure!((0..=128).contains(&n), "changed slot count");
            w.write_varint(n);
            for _ in 0..n {
                w.write_i16(r.read_i16()?);
                items::hashed_stack(t, &mut r, &mut w)?;
            }
            items::hashed_stack(t, &mut r, &mut w)?;
        }
        _ => return Ok(Some(body.to_vec())),
    }
    ensure!(r.is_empty(), "{} trailing bytes", r.len());
    Ok(Some(w))
}

/// Record only lossy item-id substitutions, not ordinary legacy stacks.
fn inventory_stack(
    session: &Session,
    slot: Option<i16>,
    r: &mut &[u8],
    w: &mut Vec<u8>,
) -> Result<()> {
    let t = &session.profile.tables;
    let mut peek = *r;
    let count = peek.read_varint()?;
    let lossy = if count > 0 {
        let item = peek.read_varint()?;
        let legacy = t.items.get(item).unwrap_or(1);
        t.items.back(legacy) != Some(item)
    } else {
        false
    };
    let start = w.len();
    items::stack(t, To::Legacy, r, w)?;
    if let Some(slot @ 5..=45) = slot {
        let mut polyfills = session.inventory_polyfills.lock();
        if lossy {
            polyfills.insert(slot, w[start..].to_vec());
        } else {
            polyfills.remove(&slot);
        }
    }
    Ok(())
}

/// Absolute target of a teleport, resolving relative components (vanilla
/// `Relative` bits: x 0, y 1, z 2, yaw 3, pitch 4) against the last pose.
fn resolve(t: Teleport, pose: Option<([f64; 3], [f32; 2])>) -> ([f64; 3], [f32; 2]) {
    let (base_position, base_rotation) = pose.unwrap_or(([0.0; 3], [0.0; 2]));
    let mut position = t.position;
    for (axis, value) in position.iter_mut().enumerate() {
        if t.relative & (1 << axis) != 0 {
            *value += base_position[axis];
        }
    }
    let mut rotation = t.rotation;
    for (axis, value) in rotation.iter_mut().enumerate() {
        if t.relative & (1 << (3 + axis)) != 0 {
            *value += base_rotation[axis];
        }
    }
    (position, rotation)
}

fn state(t: &Tables, id: i32) -> i32 {
    t.block_states.get(id).unwrap_or(1)
}

/// `ItemCost`: item id, count, exact component predicate (Solaris sends none).
fn item_cost(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_varint(t.items.get(r.read_varint()?).unwrap_or(1));
    w.write_varint(r.read_varint()?);
    let components = r.read_varint()?;
    ensure!(components == 0, "merchant cost component predicate");
    w.write_varint(0);
    Ok(())
}

fn blocks(t: &Tables) -> Container<'_> {
    Container::blocks(&t.block_states, t.legacy_block_bits)
}

fn biomes(t: &Tables) -> Container<'_> {
    Container::biomes(&t.biomes, t.legacy_biome_bits)
}

fn copy_identifier(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_identifier(&r.read_identifier()?)?;
    Ok(())
}

fn login(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_i32(r.read_i32()?);
    w.write_bool(r.read_bool()?);
    let levels = r.read_varint()?;
    ensure!((0..=1024).contains(&levels), "level count");
    w.write_varint(levels);
    for _ in 0..levels {
        copy_identifier(r, w)?;
    }
    for _ in 0..3 {
        w.write_varint(r.read_varint()?);
    }
    for _ in 0..3 {
        w.write_bool(r.read_bool()?);
    }
    spawn_info(r, w)?;
    r.read_bool()?; // 26.3 onlineMode
    w.write_bool(r.read_bool()?);
    Ok(())
}

/// `CommonPlayerSpawnInfo`: 26.3 game modes are VarInt + optional VarInt
/// (0 none, id + 1); 26.1.2 writes byte + byte (-1 none).
fn spawn_info(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_varint(r.read_varint()?);
    copy_identifier(r, w)?;
    w.write_i64(r.read_i64()?);
    let mode = r.read_varint()?;
    ensure!((0..=3).contains(&mode), "game mode {mode}");
    w.write_u8(mode as u8);
    let previous = r.read_varint()?;
    ensure!((0..=4).contains(&previous), "previous game mode {previous}");
    w.write_i8(previous as i8 - 1);
    w.write_bool(r.read_bool()?);
    w.write_bool(r.read_bool()?);
    if r.read_bool()? {
        w.write_bool(true);
        copy_identifier(r, w)?;
        w.write_i64(r.read_i64()?);
    } else {
        w.write_bool(false);
    }
    w.write_varint(r.read_varint()?);
    w.write_varint(r.read_varint()?);
    Ok(())
}

fn chunk(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_i32(r.read_i32()?);
    w.write_i32(r.read_i32()?);
    let heightmaps = r.read_varint()?;
    ensure!((0..=16).contains(&heightmaps), "heightmap count");
    w.write_varint(heightmaps);
    for _ in 0..heightmaps {
        w.write_varint(r.read_varint()?);
        let longs = r.read_varint()?;
        ensure!((0..=4096).contains(&longs), "heightmap length");
        w.write_varint(longs);
        let bytes = longs as usize * 8;
        ensure!(r.remaining() >= bytes, "truncated heightmap");
        w.extend_from_slice(&r[..bytes]);
        r.advance(bytes);
    }
    let data = r.read_byte_array(2 * 1024 * 1024)?;
    let mut sections = Vec::with_capacity(data.len());
    rewrite_sections(&data, &blocks(t), &biomes(t), &mut sections)?;
    w.write_byte_array(&sections)?;
    let entities = r.read_varint()?;
    ensure!((0..=1_000_000).contains(&entities), "block entity count");
    let mut kept = 0;
    let mut out = Vec::new();
    for _ in 0..entities {
        let xz = r.read_u8()?;
        let y = r.read_i16()?;
        let native_kind = r.read_varint()?;
        let kind = t.block_entities.get(native_kind);
        let mut nbt = Vec::new();
        copy_block_entity_nbt(native_kind, r, &mut nbt)?;
        if let Some(kind) = kind {
            out.write_u8(xz);
            out.write_i16(y);
            out.write_varint(kind);
            out.extend_from_slice(&nbt);
            kept += 1;
        }
    }
    w.write_varint(kept);
    w.extend_from_slice(&out);
    light_data(r, w)?;
    Ok(())
}

/// PotDecorations changed from an ordered item-id list to a named template
/// record. Only this typed field is downgraded; custom data and stored contents
/// must never be recursively rewritten.
fn copy_block_entity_nbt(kind: i32, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    if kind != mc_data::block_entity_kinds::BlockEntityKind::DecoratedPot.protocol_id() as i32 {
        return copy_nbt(r, w);
    }
    let mut tag = mc_nbt::read_network(r)?;
    let mc_nbt::Tag::Compound(fields) = &mut tag else {
        bail!("decorated pot block entity must be a compound");
    };
    if let Some((_, value)) = fields.iter_mut().find(|(key, _)| key == "sherds") {
        let mc_nbt::Tag::Compound(sides) = value else {
            bail!("canonical decorated pot sherds must be a named template record");
        };
        let mut items = Vec::with_capacity(4);
        for side in ["back", "left", "right", "front"] {
            let item = match mc_data::item_nbt::field(sides, side) {
                None => "minecraft:brick",
                Some(mc_nbt::Tag::Compound(template)) => {
                    let Some(mc_nbt::Tag::String(id)) = mc_data::item_nbt::field(template, "id")
                    else {
                        bail!("decorated pot {side} template has no item identifier");
                    };
                    id
                }
                Some(_) => bail!("decorated pot {side} must be an item template"),
            };
            items.push(mc_nbt::Tag::String(item.into()));
        }
        *value = mc_nbt::Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::STRING,
            elements: items,
        });
    }
    mc_nbt::write_network(w, &tag)?;
    Ok(())
}

/// 26.3 ByteBufCodecs.BIT_SET uses little-endian byte arrays; 26.1.2
/// FriendlyByteBuf.writeBitSet uses a big-endian long array instead.
fn light_data(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    for _ in 0..4 {
        let len = r.read_varint()?;
        ensure!((0..=32768).contains(&len), "light mask byte length {len}");
        let len = len as usize;
        ensure!(r.len() >= len, "truncated light mask");
        let (mask, rest) = r.split_at(len);
        *r = rest;
        w.write_varint(len.div_ceil(8) as i32);
        for bytes in mask.chunks(8) {
            let mut word = [0; 8];
            word[..bytes.len()].copy_from_slice(bytes);
            w.write_i64(i64::from_le_bytes(word));
        }
    }
    // Sky/block layer lists have the same count + byte-array layout.
    w.extend_from_slice(r);
    *r = &[];
    Ok(())
}

/// 26.3 movement deltas: VarInt `onGround | steps << 1`, then one delta or
/// `steps` (ticks, delta) pairs. 26.1.2 takes one delta; steps are summed.
fn moved_delta(r: &mut &[u8], w: &mut Vec<u8>) -> Result<bool> {
    let props = r.read_varint()?;
    let steps = props >> 1;
    let mut delta = [0i32; 3];
    if steps == 0 {
        for value in &mut delta {
            *value = i32::from(r.read_i16()?);
        }
    } else {
        ensure!(steps <= 64, "movement steps {steps}");
        for _ in 0..steps {
            r.read_varint()?;
            for value in &mut delta {
                *value += i32::from(r.read_i16()?);
            }
        }
    }
    for value in delta {
        w.write_i16(value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
    }
    Ok(props & 1 != 0)
}

fn copy_lp_vec3(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    let first = r.read_u8()?;
    w.write_u8(first);
    if first == 0 {
        return Ok(());
    }
    w.write_u8(r.read_u8()?);
    w.write_u32(r.read_u32()?);
    if first & 4 != 0 {
        w.write_varint(r.read_varint()?);
    }
    Ok(())
}

/// `Holder<SoundEvent>`: VarInt id + 1, or 0 then an inline event. A sound
/// 26.1.2 lacks is sent inline by name (the client plays nothing).
fn sound_holder(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    let holder = r.read_varint()?;
    if holder == 0 {
        w.write_varint(0);
        copy_identifier(r, w)?;
        if r.read_bool()? {
            w.write_bool(true);
            w.write_f32(r.read_f32()?);
        } else {
            w.write_bool(false);
        }
        return Ok(());
    }
    let id = holder - 1;
    if let Some(legacy) = t.sounds.get(id) {
        w.write_varint(legacy + 1);
    } else {
        let name = t.sound_names.get(id as usize).context("unknown sound id")?;
        w.write_varint(0);
        w.write_string(name, 32767)?;
        w.write_bool(false);
    }
    Ok(())
}

/// `ParticleOptions`: type id then its payload. `Ok(false)` when 26.1.2 has
/// no such particle (the packet is dropped).
fn particle_options(t: &Tables, r: &mut &[u8], w: &mut Vec<u8>) -> Result<bool> {
    let kind = r.read_varint()?;
    let name = t
        .particle_names
        .get(kind as usize)
        .context("unknown particle type")?
        .as_str();
    let legacy = t.particles.get(kind);
    let mut payload = Vec::new();
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "block" | "block_marker" | "falling_dust" | "dust_pillar" | "block_crumble" => {
            payload.write_varint(state(t, r.read_varint()?));
        }
        "dust" => {
            payload.write_i32(r.read_i32()?);
            payload.write_f32(r.read_f32()?);
        }
        "dust_color_transition" => {
            payload.write_i32(r.read_i32()?);
            payload.write_i32(r.read_i32()?);
            payload.write_f32(r.read_f32()?);
        }
        "entity_effect" | "tinted_leaves" | "flash" => payload.write_i32(r.read_i32()?),
        "sculk_charge" => payload.write_f32(r.read_f32()?),
        "shriek" => payload.write_varint(r.read_varint()?),
        "item" => items::stack(t, To::Legacy, r, &mut payload)?,
        "vibration" | "trail" | "dragon_breath" | "effect" | "instant_effect" => {
            return items::unsupported("particle payload");
        }
        _ => {}
    }
    let Some(legacy) = legacy else {
        return Ok(false);
    };
    w.write_varint(legacy);
    w.extend_from_slice(&payload);
    Ok(true)
}

/// `SetEntityData` entries until the 0xFF terminator.
fn entity_data(t: &Tables, policy: Metadata, r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    loop {
        let index = r.read_u8()?;
        if index == METADATA_END {
            w.write_u8(METADATA_END);
            return Ok(());
        }
        let serializer = r.read_varint()?;
        let mut value = Vec::new();
        let keep = metadata_value(t, serializer, r, &mut value)?;
        let index = match policy {
            Metadata::Same => Some(index),
            Metadata::Below(limit) => (index < limit).then_some(index),
            Metadata::Cube => match index {
                0..=15 => Some(index),
                18 => Some(16),
                _ => None,
            },
        };
        if keep && let Some(index) = index {
            w.write_u8(index);
            w.write_varint(serializer);
            w.extend_from_slice(&value);
        }
    }
}

/// Copy one metadata value (26.3 `EntityDataSerializers` order). `false`
/// drops the entry (a value 26.1.2 cannot represent).
fn metadata_value(t: &Tables, serializer: i32, r: &mut &[u8], w: &mut Vec<u8>) -> Result<bool> {
    match serializer {
        0 => w.write_u8(r.read_u8()?),
        1 | 12 | 19..=32 | 35..=38 | 42 => w.write_varint(r.read_varint()?),
        2 => w.write_varlong(r.read_varlong()?),
        3 => w.write_f32(r.read_f32()?),
        4 => w.write_string(&r.read_string(32767)?, 32767)?,
        5 => copy_nbt(r, w)?,
        6 => {
            let present = r.read_bool()?;
            w.write_bool(present);
            if present {
                copy_nbt(r, w)?;
            }
        }
        7 => items::stack(t, To::Legacy, r, w)?,
        8 => w.write_bool(r.read_bool()?),
        9 => {
            for _ in 0..3 {
                w.write_f32(r.read_f32()?);
            }
        }
        10 => w.write_i64(r.read_i64()?),
        11 => {
            let present = r.read_bool()?;
            w.write_bool(present);
            if present {
                w.write_i64(r.read_i64()?);
            }
        }
        13 => {
            let present = r.read_bool()?;
            w.write_bool(present);
            if present {
                w.write_uuid(r.read_uuid()?);
            }
        }
        14 => w.write_varint(state(t, r.read_varint()?)),
        15 => {
            let id = r.read_varint()?;
            w.write_varint(if id == 0 { 0 } else { state(t, id - 1) + 1 });
        }
        16 => return particle_options(t, r, w),
        17 => {
            let n = r.read_varint()?;
            ensure!((0..=256).contains(&n), "particle list length");
            let mut list = Vec::new();
            let mut kept = 0;
            for _ in 0..n {
                if particle_options(t, r, &mut list)? {
                    kept += 1;
                }
            }
            w.write_varint(kept);
            w.extend_from_slice(&list);
        }
        18 => {
            for _ in 0..3 {
                w.write_varint(r.read_varint()?);
            }
        }
        33 => {
            let present = r.read_bool()?;
            w.write_bool(present);
            if present {
                copy_identifier(r, w)?;
                w.write_i64(r.read_i64()?);
            }
        }
        34 => {
            let holder = r.read_varint()?;
            ensure!(holder != 0, "inline painting variant");
            w.write_varint(holder);
        }
        39 => {
            for _ in 0..3 {
                w.write_f32(r.read_f32()?);
            }
        }
        40 => {
            for _ in 0..4 {
                w.write_f32(r.read_f32()?);
            }
        }
        43 => {
            r.read_varint()?;
            return Ok(false);
        }
        _ => bail!("entity data serializer {serializer}"),
    }
    Ok(true)
}

/// `SetPlayerTeam`. 26.3 parameters: name, prefix, suffix, visibility,
/// collision, Optional<TeamColor>, options; 26.1.2: name, options,
/// visibility, collision, ChatFormatting colour (21 = reset), prefix, suffix.
fn team(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_string(&r.read_string(32767)?, 32767)?;
    let method = r.read_u8()?;
    w.write_u8(method);
    if matches!(method, 0 | 2) {
        let mut display = Vec::new();
        copy_nbt(r, &mut display)?;
        let mut prefix = Vec::new();
        copy_nbt(r, &mut prefix)?;
        let mut suffix = Vec::new();
        copy_nbt(r, &mut suffix)?;
        let visibility = r.read_varint()?;
        let collision = r.read_varint()?;
        let colour = if r.read_bool()? { r.read_varint()? } else { 21 };
        let options = r.read_u8()?;
        w.extend_from_slice(&display);
        w.write_u8(options);
        w.write_varint(visibility);
        w.write_varint(collision);
        w.write_varint(colour);
        w.extend_from_slice(&prefix);
        w.extend_from_slice(&suffix);
    }
    if matches!(method, 0 | 3 | 4) {
        let n = r.read_varint()?;
        ensure!((0..=65536).contains(&n), "team player count");
        w.write_varint(n);
        for _ in 0..n {
            w.write_string(&r.read_string(32767)?, 32767)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "play_tests.rs"]
mod tests;
