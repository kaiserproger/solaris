//! Atomic world images. Domain codecs remain with their gameplay owners.
use super::{
    NativeChunkSnapshot, NativeEntitySnapshot, NativeError, NativeReadOnlyStore, NativeStore,
    SnapshotCommit, codec, db, state,
};
use crate::{BlockRegistry, Chunk, ChunkGenerator, ChunkGeometry, ChunkPos, OVERWORLD_GEOMETRY};
use mc_data::items::ItemRegistry;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;
use mc_nbt::Tag;
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const GLOBAL: TableDefinition<&str, &[u8]> = TableDefinition::new("global");
const PLAYERS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("players");
const DOMAINS: &[&str] = &[
    "checkpoint",
    "dimensions",
    "generation",
    "scoreboard",
    "bossbars",
    "level_metadata",
    "conversion_extras",
];
const PLUGIN_DOMAINS: &[&str] = &["plugins", "plugin_startup"];
const VERSIONS: &[u8] = b"chunk=1;dictionary=1;entity=1;tick=1;global=1;player=1";
const LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeDomainRecord {
    pub version: u32,
    pub captured_tick: u64,
    pub payload: Vec<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativePlayerSnapshot {
    pub uuid: [u8; 16],
    pub record: NativeDomainRecord,
}
pub struct NativeCheckpointImage {
    pub tick: u64,
    pub globals: BTreeMap<String, NativeDomainRecord>,
    pub players: Vec<NativePlayerSnapshot>,
    pub entities: Vec<NativeEntitySnapshot>,
}
pub struct NativeCheckpoint<'a> {
    pub tick: u64,
    pub previous_tick: u64,
    pub chunks: &'a [NativeChunkSnapshot<'a>],
    pub entities: &'a [NativeEntitySnapshot],
    pub players: &'a [NativePlayerSnapshot],
    pub globals: &'a BTreeMap<String, NativeDomainRecord>,
}

/// Committed record writes, independent of filesystem I/O and live data size.
/// Payload bytes cover encoded chunk baseline/metadata/overlay and entity rows,
/// excluding keys, indexes, dictionaries, globals and redb copy-on-write overhead.
#[derive(Debug, Default)]
pub struct NativeCheckpointIo {
    pub rewritten_chunks: usize,
    pub chunk_payload_bytes: u64,
    /// `codec::overlay` evaluations: one per block section examined for the
    /// durable overlay (each scans and, when it differs from base, encodes 4096 cells).
    pub sections_encoded: u64,
    /// Section overlay records inserted or replaced.
    pub section_records_written: u64,
    pub rewritten_entities: usize,
    pub entity_payload_bytes: u64,
    /// Entity records presented by the owner image (the O(all) part of the pass).
    pub entities_visited: usize,
    /// Wall time inside the transaction: world (chunk) stream, entity stream
    /// including its lazy encode, and the final redb commit.
    pub chunk_micros: u64,
    pub entity_micros: u64,
    pub commit_micros: u64,
}

fn micros(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn failure(domain: &str, detail: impl std::fmt::Display) -> NativeError {
    NativeError::Source(format!("native {domain}: {detail}"))
}
fn json(domain: &str, record: &NativeDomainRecord) -> Result<Value, NativeError> {
    let value: Value = serde_json::from_slice(&record.payload).map_err(|e| failure(domain, e))?;
    if !value.is_object() {
        return Err(failure(domain, "expected JSON object"));
    }
    Ok(value)
}
fn validate_envelope(record: &NativeDomainRecord) -> Result<(), NativeError> {
    if record.version != 1 {
        return Err(failure(
            "envelope",
            format!("unsupported version {}", record.version),
        ));
    }
    if record.payload.len() > LIMIT {
        return Err(failure("envelope", "record exceeds budget"));
    }
    Ok(())
}
fn encode(record: &NativeDomainRecord) -> Result<Vec<u8>, NativeError> {
    validate_envelope(record)?;
    let mut bytes = Vec::with_capacity(16 + record.payload.len());
    bytes.extend_from_slice(&record.version.to_be_bytes());
    bytes.extend_from_slice(&record.captured_tick.to_be_bytes());
    bytes.extend_from_slice(&(record.payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&record.payload);
    Ok(bytes)
}
fn decode_envelope(bytes: &[u8]) -> Result<(u32, u64, &[u8]), NativeError> {
    if bytes.len() < 16 {
        return Err(failure("envelope", "truncated"));
    }
    let version = u32::from_be_bytes(bytes[..4].try_into().expect("length checked"));
    let captured_tick = u64::from_be_bytes(bytes[4..12].try_into().expect("length checked"));
    let length = u32::from_be_bytes(bytes[12..16].try_into().expect("length checked")) as usize;
    if version != 1 || length > LIMIT || bytes.len() - 16 != length {
        return Err(failure("envelope", "unsupported version or invalid length"));
    }
    Ok((version, captured_tick, &bytes[16..]))
}
fn decode(bytes: &[u8]) -> Result<NativeDomainRecord, NativeError> {
    let (version, captured_tick, payload) = decode_envelope(bytes)?;
    Ok(NativeDomainRecord {
        version,
        captured_tick,
        payload: payload.to_vec(),
    })
}
pub(super) fn initialize(tx: &redb::WriteTransaction) -> Result<(), NativeError> {
    db(tx.open_table(GLOBAL))?;
    db(tx.open_table(PLAYERS))?;
    db(db(tx.open_table(super::HEADER))?.insert("domain_versions", VERSIONS))?;
    Ok(())
}
pub(super) fn validate_tables(tx: &redb::ReadTransaction) -> Result<(), NativeError> {
    db(tx.open_table(GLOBAL))?;
    db(tx.open_table(PLAYERS))?;
    if db(db(tx.open_table(super::HEADER))?.get("domain_versions"))?
        .is_none_or(|r| r.value() != VERSIONS)
    {
        return Err(failure("world/domain_versions", "missing or unsupported"));
    }
    Ok(())
}

/// Admitted global checkpoint tick, if any checkpoint has committed. Replay
/// (WAL/batch) rows at-or-below this tick cannot overwrite retained bytes.
pub(super) fn checkpoint_floor(tx: &redb::WriteTransaction) -> Result<Option<u64>, NativeError> {
    let global = db(tx.open_table(GLOBAL))?;
    db(global.get("checkpoint"))?
        .map(|record| decode_envelope(record.value()).map(|(_, captured_tick, _)| captured_tick))
        .transpose()
}

fn geometry(record: &NativeDomainRecord, id: u32) -> Result<ChunkGeometry, NativeError> {
    let value = json("global/dimensions", record)?;
    if value.as_object().is_none_or(|m| m.len() != 3) {
        return Err(failure("global/dimensions", "requires IDs 0,1,2"));
    }
    for (id, name) in [
        (0, "minecraft:overworld"),
        (1, "minecraft:the_nether"),
        (2, "minecraft:the_end"),
    ] {
        let d = &value[id.to_string()];
        if d["name"].as_str() != Some(name)
            || d["generator"].as_str().is_none_or(str::is_empty)
            || d["seed"].as_i64().is_none()
            || d["revision"].as_u64().is_none()
        {
            return Err(failure("global/dimensions", "invalid dimension descriptor"));
        }
        let min = d["min_y"].as_i64().and_then(|v| i32::try_from(v).ok());
        let height = d["height"].as_i64().and_then(|v| i32::try_from(v).ok());
        if min
            .zip(height)
            .and_then(|(m, h)| ChunkGeometry::new(m, h))
            .is_none()
        {
            return Err(failure("global/dimensions", "invalid geometry"));
        }
    }
    if id > 2 {
        return Err(failure("global/dimensions", "unknown dimension"));
    }
    let d = &value[id.to_string()];
    Ok(ChunkGeometry::new(
        d["min_y"].as_i64().expect("validated") as i32,
        d["height"].as_i64().expect("validated") as i32,
    )
    .expect("validated"))
}
pub(super) fn read_geometry(
    tx: &redb::ReadTransaction,
    dimension: u32,
) -> Result<ChunkGeometry, NativeError> {
    let table = db(tx.open_table(GLOBAL))?;
    match db(table.get("dimensions"))? {
        Some(record) => geometry(&decode(record.value())?, dimension),
        // Legacy native callers operate on a newly created, non-world DB.
        None if dimension == 0 => Ok(OVERWORLD_GEOMETRY),
        None => Err(failure("global/dimensions", "missing dimension")),
    }
}
pub(super) fn check_read_geometry(
    tx: &redb::ReadTransaction,
    dimension: u32,
    chunk: &Chunk,
) -> Result<(), NativeError> {
    super::check_geometry(chunk)?;
    if chunk.geometry() != read_geometry(tx, dimension)? {
        return Err(failure("chunk", "persisted geometry mismatch"));
    }
    Ok(())
}
pub(super) fn check_write_geometry(
    tx: &redb::WriteTransaction,
    dimension: u32,
    chunk: &Chunk,
) -> Result<(), NativeError> {
    super::check_geometry(chunk)?;
    let table = db(tx.open_table(GLOBAL))?;
    let expected = match db(table.get("dimensions"))? {
        Some(record) => geometry(&decode(record.value())?, dimension)?,
        None if dimension == 0 => OVERWORLD_GEOMETRY,
        None => return Err(failure("global/dimensions", "missing dimension")),
    };
    if expected != chunk.geometry() {
        return Err(failure("chunk", "persisted geometry mismatch"));
    }
    Ok(())
}
fn validate_globals(
    globals: &BTreeMap<String, NativeDomainRecord>,
    tick: u64,
    identity: &[u8],
) -> Result<Value, NativeError> {
    let checkpoint_record = globals
        .get("checkpoint")
        .ok_or_else(|| failure("checkpoint", "missing"))?;
    let checkpoint = json("checkpoint", checkpoint_record)?;
    let versions = checkpoint["domain_versions"]
        .as_object()
        .ok_or_else(|| failure("checkpoint", "missing domain versions"))?;
    let plugins_versioned = PLUGIN_DOMAINS.iter().any(|key| versions.contains_key(*key));
    let plugin_domains = if plugins_versioned {
        PLUGIN_DOMAINS
    } else {
        &[]
    };
    for (key, record) in globals.iter() {
        let suffix = match key.strip_prefix("settlement/") {
            Some(suffix) => suffix,
            None => continue,
        };
        if suffix.is_empty()
            || suffix.len() > 20
            || (suffix.len() > 1 && suffix.starts_with('0'))
            || !suffix.bytes().all(|byte: u8| byte.is_ascii_digit())
            || suffix.parse::<u64>().unwrap_or(0) == 0
        {
            return Err(failure("global", "non-canonical settlement row key"));
        }
        validate_envelope(record)?;
        if record.captured_tick != tick {
            return Err(failure(key.as_str(), "capture tick mismatch"));
        }
        json(key.as_str(), record)?;
    }
    let settlement_rows = globals
        .keys()
        .filter(|key| key.starts_with("settlement/"))
        .count();
    if let Some(record) = globals.get("settlement_sequence") {
        validate_envelope(record)?;
        if record.captured_tick != tick {
            return Err(failure("settlement_sequence", "capture tick mismatch"));
        }
        let value: serde_json::Value = serde_json::from_slice(&record.payload)
            .map_err(|e| failure("settlement_sequence", e))?;
        if value.as_u64().is_none() {
            return Err(failure("settlement_sequence", "expected JSON u64"));
        }
    } else if settlement_rows != 0 {
        return Err(failure("settlement_sequence", "missing sequence row"));
    }
    let mut map_rows = 0;
    let mut next_map_id = 0_i64;
    for (key, record) in globals {
        let Some(suffix) = key.strip_prefix("map/") else {
            continue;
        };
        let id = suffix
            .parse::<i32>()
            .map_err(|_| failure("global", "non-canonical map row key"))?;
        if id < 0
            || !suffix.bytes().all(|byte| byte.is_ascii_digit())
            || (suffix.len() > 1 && suffix.starts_with('0'))
        {
            return Err(failure("global", "non-canonical map row key"));
        }
        validate_envelope(record)?;
        if record.captured_tick != tick {
            return Err(failure(key, "capture tick mismatch"));
        }
        json(key, record)?;
        map_rows += 1;
        next_map_id = next_map_id.max(i64::from(id) + 1);
    }
    if let Some(record) = globals.get("map_sequence") {
        validate_envelope(record)?;
        if record.captured_tick != tick {
            return Err(failure("map_sequence", "capture tick mismatch"));
        }
        let next: i64 = serde_json::from_slice(&record.payload)
            .map_err(|error| failure("map_sequence", error))?;
        if !(next_map_id..=i64::from(i32::MAX) + 1).contains(&next) {
            return Err(failure("map_sequence", "invalid next map id"));
        }
    } else if map_rows != 0 {
        return Err(failure("map_sequence", "missing sequence row"));
    }
    let sequence_rows = usize::from(globals.contains_key("settlement_sequence"));
    let map_sequence_rows = usize::from(globals.contains_key("map_sequence"));
    let command_storage_rows = usize::from(globals.contains_key("command_storage"));
    if globals.len()
        != DOMAINS.len()
            + plugin_domains.len()
            + settlement_rows
            + sequence_rows
            + map_rows
            + map_sequence_rows
            + command_storage_rows
    {
        return Err(NativeError::GlobalDomains);
    }
    if let Some(record) = globals.get("command_storage") {
        validate_envelope(record)?;
        if record.captured_tick != tick
            || !matches!(codec::decode(&record.payload)?, Tag::Compound(_))
        {
            return Err(failure("command_storage", "invalid command storage"));
        }
    }
    for key in DOMAINS.iter().chain(plugin_domains) {
        let record = globals.get(*key).ok_or_else(|| failure(key, "missing"))?;
        validate_envelope(record)?;
        if record.captured_tick != tick {
            return Err(failure(key, "capture tick mismatch"));
        }
        if matches!(
            *key,
            "scoreboard" | "bossbars" | "level_metadata" | "conversion_extras"
        ) {
            if !matches!(codec::decode(&record.payload)?, Tag::Compound(_)) {
                return Err(failure(key, "expected NBT compound"));
            }
        } else if *key != "checkpoint" {
            json(key, record)?;
        }
    }
    geometry(&globals["dimensions"], 0)?;
    let generation = json("generation", &globals["generation"])?;
    let expected: Vec<Value> = identity.iter().map(|b| Value::from(*b)).collect();
    if generation["identity"] != Value::Array(expected)
        || generation["base"].is_null()
        || generation["generators"].is_null()
    {
        return Err(failure(
            "generation",
            "identity or generation contract mismatch",
        ));
    }
    validate_manifest(&checkpoint, tick)?;
    if checkpoint["dimensions"]
        .as_object()
        .is_none_or(|children| children.len() != 2)
    {
        return Err(failure(
            "checkpoint/dimensions",
            "requires Nether and End images",
        ));
    }
    let names = [
        "climate",
        "gamerules",
        "block_ticks",
        "fluid_ticks",
        "claims",
        "tickets",
        "raids",
        "end_fight",
    ];
    if versions.len() != names.len() + plugin_domains.len()
        || names
            .iter()
            .chain(plugin_domains)
            .any(|key| versions.get(*key).and_then(Value::as_u64) != Some(1))
    {
        return Err(failure("checkpoint", "unsupported subdomain versions"));
    }
    Ok(checkpoint)
}
fn validate_manifest(value: &Value, tick: u64) -> Result<(), NativeError> {
    for key in [
        "tick",
        "dimensions",
        "end_fight",
        "raids",
        "tickets",
        "players",
        "world_spawn",
        "world_spawn_rotation",
        "generated_entity_claims",
        "climate",
        "border",
        "difficulty",
        "block_ticks",
        "fluid_ticks",
        "gamerules",
    ] {
        if value.get(key).is_none() {
            return Err(failure("checkpoint", format!("missing {key}")));
        }
    }
    if value["tick"].as_u64() != Some(tick) {
        return Err(failure("checkpoint", "manifest tick mismatch"));
    }
    if value["players"].as_object().is_none() {
        return Err(failure("checkpoint/players", "expected map"));
    }
    for key in ["tickets", "border", "gamerules"] {
        if !value[key].is_object() {
            return Err(failure("checkpoint", format!("{key}: expected object")));
        }
    }
    if value["generated_entity_claims"]
        .as_array()
        .is_none_or(|claims| {
            let mut seen = BTreeSet::new();
            claims
                .iter()
                .any(|claim| claim.as_str().is_none_or(|claim| !seen.insert(claim)))
        })
    {
        return Err(failure("checkpoint/claims", "invalid or duplicate claims"));
    }
    if value["world_spawn_rotation"]
        .as_array()
        .is_none_or(|values| {
            values.len() != 2
                || values.iter().any(|v| {
                    v.as_f64()
                        .is_none_or(|v| !v.is_finite() || !(v as f32).is_finite())
                })
        })
    {
        return Err(failure("checkpoint/spawn", "invalid rotation"));
    }
    let dimensions = value["dimensions"]
        .as_object()
        .ok_or_else(|| failure("checkpoint/dimensions", "expected map"))?;
    for (name, child) in dimensions {
        if !matches!(name.as_str(), "minecraft:the_nether" | "minecraft:the_end") {
            return Err(failure("checkpoint/dimensions", "unknown child"));
        }
        validate_manifest(child, tick)?;
    }
    Ok(())
}
fn validate_player(record: &NativeDomainRecord, tick: u64) -> Result<(), NativeError> {
    validate_envelope(record)?;
    if record.captured_tick > tick {
        return Err(failure("players", "future tick"));
    }
    if !matches!(codec::decode(&record.payload)?, Tag::Compound(_)) {
        return Err(failure("players", "expected compound"));
    }
    Ok(())
}
fn entities_equal_ignoring_lsn(
    stored: &NativeEntitySnapshot,
    incoming: &NativeEntitySnapshot,
) -> bool {
    // LSN is the checkpoint stamp, not entity data. Slot, dimension, position
    // and payload are the semantic content; real updates always differ here.
    stored.identity == incoming.identity
        && stored.dimension == incoming.dimension
        && stored.position == incoming.position
        && stored.generated_slot == incoming.generated_slot
        && stored.payload == incoming.payload
}

fn write_entities<E: std::borrow::Borrow<NativeEntitySnapshot>>(
    tx: &redb::WriteTransaction,
    snapshots: impl IntoIterator<Item = Result<E, NativeError>>,
    tick: u64,
    io: &mut NativeCheckpointIo,
) -> Result<(), NativeError> {
    let mut entities = db(tx.open_table(state::ENTITIES))?;
    let mut slots = db(tx.open_table(state::INITIALIZED))?;
    let mut applied = db(tx.open_table(state::ENTITY_LSN))?;
    let mut seen = BTreeSet::new();
    let mut wires = BTreeSet::new();
    for entity in snapshots {
        let entity = entity?;
        io.entities_visited += 1;
        let entity = entity.borrow();
        if !seen.insert(entity.identity) || entity.dimension > 2 || entity.journal_lsn > tick {
            return Err(failure(
                "entities",
                "duplicate identity, unknown dimension or future tick",
            ));
        }
        if let Some(Tag::Compound(fields)) = &entity.payload
            && let Some((_, Tag::Int(wire))) = fields.iter().find(|(k, _)| k == "solaris:WireId")
            && (*wire <= 0 || !wires.insert(*wire))
        {
            return Err(failure("entities", "duplicate or invalid WireId"));
        }
        if let Some(previous) = db(entities.get(entity.identity.as_slice()))? {
            let previous = state::decode(entity.identity, previous.value())?;
            if previous.journal_lsn > tick
                || previous.generated_slot != entity.generated_slot
                || (previous.payload.is_none() && entity.payload.is_some())
            {
                return Err(failure(
                    "entities",
                    "stale checkpoint, slot mutation or tombstone resurrection",
                ));
            }
            // Unchanged capture at a later global tick: retain stored bytes
            // and the prior LSN. Real updates differ in payload/slot/
            // dimension/position and always fall through to stamp the tick.
            if previous.journal_lsn <= tick && entities_equal_ignoring_lsn(&previous, entity) {
                // Slot row already claims this identity from the earlier write;
                // only verify it, never rewrite the same bytes.
                if let Some(slot) = entity.generated_slot
                    && db(slots.get(slot.as_slice()))?.is_none_or(|v| v.value() != entity.identity)
                {
                    return Err(failure("generated_slots", "slot identity conflict"));
                }
                continue;
            }
        }
        if let Some(slot) = entity.generated_slot {
            if db(slots.get(slot.as_slice()))?.is_some_and(|v| v.value() != entity.identity) {
                return Err(failure("generated_slots", "slot identity conflict"));
            }
            db(slots.insert(slot.as_slice(), entity.identity.as_slice()))?;
        }
        let bytes = state::encode_at_lsn(entity, tick)?;
        db(entities.insert(entity.identity.as_slice(), bytes.as_slice()))?;
        db(applied.insert(entity.identity.as_slice(), tick))?;
        io.rewritten_entities += 1;
        io.entity_payload_bytes += bytes.len() as u64;
    }
    let mut omitted = Vec::new();
    for row in db(entities.iter())? {
        let (key, bytes) = db(row)?;
        let id = key
            .value()
            .try_into()
            .map_err(|_| failure("entities", "invalid UUID"))?;
        if seen.contains(&id) {
            continue;
        }
        let mut previous = state::decode(id, bytes.value())?;
        if previous.journal_lsn > tick {
            return Err(failure("entities", "future stored entity"));
        }
        if !seen.contains(&id) && previous.payload.is_some() {
            previous.payload = None;
            previous.journal_lsn = tick;
            omitted.push(previous);
        }
    }
    // Omitted live entities become durable tombstones; slots never reset.
    // Tombstone writes are real membership deletes and always stamp the tick.
    for entity in omitted {
        let bytes = state::encode(&entity)?;
        db(entities.insert(entity.identity.as_slice(), bytes.as_slice()))?;
        db(applied.insert(entity.identity.as_slice(), tick))?;
        io.rewritten_entities += 1;
        io.entity_payload_bytes += bytes.len() as u64;
    }
    Ok(())
}
/// Header identity and the two immutable generation payloads of a native world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeGenerationContract {
    pub identity: [u8; 32],
    pub generation: Vec<u8>,
    pub dimensions: Vec<u8>,
}

impl NativeStore {
    /// Offline operator path: the stored header identity of a stopped world,
    /// without validating it against any configuration.
    pub fn read_identity(path: &std::path::Path) -> Result<[u8; 32], NativeError> {
        let database = super::read_only_database(path, super::DB_CACHE_BYTES)?;
        let tx = db(database.begin_read())?;
        let header = db(tx.open_table(super::HEADER))?;
        let stored =
            db(header.get("identity"))?.ok_or_else(|| failure("world/identity", "missing"))?;
        stored
            .value()
            .try_into()
            .map_err(|_| failure("world/identity", "invalid length"))
    }

    /// Offline operator path: replace the header identity and the immutable
    /// generation/dimensions payloads in one durable transaction, only when the
    /// stored contract is byte-for-byte `from`. Record versions and capture ticks
    /// are kept; chunks, entities and gameplay state are untouched. `from == to`
    /// verifies and writes nothing. Requires exclusive access to the database file.
    pub fn replace_generation_contract(
        path: &std::path::Path,
        from: &NativeGenerationContract,
        to: &NativeGenerationContract,
    ) -> Result<(), NativeError> {
        let database = db(redb::Database::builder()
            .set_cache_size(super::DB_CACHE_BYTES)
            .open(path))?;
        let mut tx = db(database.begin_write())?;
        db(tx.set_durability(redb::Durability::Immediate))?;
        {
            let mut header = db(tx.open_table(super::HEADER))?;
            if db(header.get("identity"))?.is_none_or(|v| v.value() != from.identity) {
                return Err(failure(
                    "world/identity",
                    "does not match the expected contract",
                ));
            }
            let mut global = db(tx.open_table(GLOBAL))?;
            let mut replacements = Vec::new();
            for (key, old, new) in [
                ("generation", &from.generation, &to.generation),
                ("dimensions", &from.dimensions, &to.dimensions),
            ] {
                let stored = db(global.get(key))?
                    .map(|record| decode(record.value()))
                    .transpose()?
                    .ok_or_else(|| failure(key, "missing"))?;
                if stored.payload != *old {
                    return Err(failure(key, "does not match the expected contract"));
                }
                replacements.push((
                    key,
                    encode(&NativeDomainRecord {
                        payload: new.clone(),
                        ..stored
                    })?,
                ));
            }
            if from == to {
                return Ok(());
            }
            for (key, bytes) in replacements {
                db(global.insert(key, bytes.as_slice()))?;
            }
            db(header.insert("identity", to.identity.as_slice()))?;
        }
        db(tx.commit())
    }
}

impl NativeStore {
    /// Install geometry in an unpublished converter staging DB before bulk imports.
    /// This does not constitute a recoverable world checkpoint.
    pub fn initialize_dimensions(&self, record: &NativeDomainRecord) -> Result<(), NativeError> {
        geometry(record, 0)?;
        if record.captured_tick != 0 {
            return Err(failure(
                "dimensions",
                "initial descriptor must have tick zero",
            ));
        }
        let bytes = encode(record)?;
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(redb::Durability::Immediate))?;
        {
            let mut global = db(tx.open_table(GLOBAL))?;
            if db(global.get("checkpoint"))?.is_some() || db(global.get("dimensions"))?.is_some() {
                return Err(failure("dimensions", "descriptor already initialized"));
            }
            for definition in [super::IMPORTED, super::METADATA, super::OVERLAYS] {
                if db(db(tx.open_table(definition))?.len())? != 0 {
                    return Err(failure(
                        "dimensions",
                        "cannot install geometry over stored chunks",
                    ));
                }
            }
            if db(db(tx.open_table(super::APPLIED))?.len())? != 0 {
                return Err(failure(
                    "dimensions",
                    "cannot install geometry over gameplay",
                ));
            }
            db(global.insert("dimensions", bytes.as_slice()))?;
        }
        db(tx.commit())
    }

    pub fn commit_checkpoint(
        &self,
        image: NativeCheckpoint<'_>,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<(), NativeError> {
        let entities = image.entities.iter().map(Ok);
        self.commit_checkpoint_iter(
            NativeCheckpoint {
                entities: &[],
                ..image
            },
            entities,
            blocks,
            items,
        )
    }

    /// Commit lazily encoded entity after-images in the same atomic transaction.
    /// `image.entities` must be empty; `entities` is the complete owner image.
    /// An encoding or validation error aborts every domain, just as for a slice.
    pub fn commit_checkpoint_iter<E: std::borrow::Borrow<NativeEntitySnapshot>>(
        &self,
        image: NativeCheckpoint<'_>,
        entities: impl IntoIterator<Item = Result<E, NativeError>>,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<(), NativeError> {
        let chunks = image.chunks;
        self.commit_checkpoint_stream(
            NativeCheckpoint {
                chunks: &[],
                ..image
            },
            |write| {
                for snapshot in chunks {
                    write(NativeChunkSnapshot {
                        dimension: snapshot.dimension,
                        base: snapshot.base,
                        effective: snapshot.effective,
                        dirty_sections: snapshot.dirty_sections,
                    })?;
                }
                Ok(())
            },
            entities,
            blocks,
            items,
        )
        .map(|_| ())
    }

    /// Resolve chunk pairs lazily inside the same atomic checkpoint transaction.
    /// Neither slice may be supplied alongside its stream. Chunk references are
    /// consumed before the producer prepares the next owned baseline/image.
    pub fn commit_checkpoint_stream<E: std::borrow::Borrow<NativeEntitySnapshot>>(
        &self,
        image: NativeCheckpoint<'_>,
        write_chunks: impl FnOnce(
            &mut dyn FnMut(NativeChunkSnapshot<'_>) -> Result<(), NativeError>,
        ) -> Result<(), NativeError>,
        entities: impl IntoIterator<Item = Result<E, NativeError>>,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<NativeCheckpointIo, NativeError> {
        if !image.chunks.is_empty() {
            return Err(failure(
                "chunks",
                "streaming checkpoint also supplied a slice",
            ));
        }
        if !image.entities.is_empty() {
            return Err(failure(
                "entities",
                "streaming checkpoint also supplied a slice",
            ));
        }
        let mut manifest = validate_globals(image.globals, image.tick, &self.identity)?;
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(redb::Durability::Immediate))?;
        {
            let mut global = db(tx.open_table(GLOBAL))?;
            let previous = db(global.get("checkpoint"))?
                .map(|r| decode(r.value()))
                .transpose()?;
            if image.previous_tick > image.tick
                || previous.as_ref().is_some_and(|r| {
                    r.captured_tick != image.previous_tick || image.tick < r.captured_tick
                })
                || (previous.is_none() && image.previous_tick != 0)
                || (previous.is_some() && image.tick == 0)
            {
                return Err(failure(
                    "checkpoint",
                    "stale previous tick or non-creation zero",
                ));
            }
            if let Some(previous) = &previous {
                let previous = json("checkpoint", previous)?;
                let old = previous["players"]
                    .as_object()
                    .ok_or_else(|| failure("checkpoint/players", "invalid previous map"))?;
                let current = manifest["players"].as_object_mut().expect("validated map");
                for (uuid, tick) in old {
                    current.entry(uuid.clone()).or_insert_with(|| tick.clone());
                }
            }
            for player in image.players {
                let uuid = uuid::Uuid::from_bytes(player.uuid).to_string();
                manifest["players"]
                    .as_object_mut()
                    .expect("validated map")
                    .insert(uuid, Value::from(player.record.captured_tick));
            }
            for immutable in ["generation", "dimensions"] {
                if let Some(old) = db(global.get(immutable))?
                    && decode(old.value())?.payload != image.globals[immutable].payload
                {
                    return Err(failure(immutable, "immutable descriptor changed"));
                }
            }
            for (key, record) in image.globals {
                let bytes = if key == "checkpoint" {
                    let payload =
                        serde_json::to_vec(&manifest).map_err(|e| failure("checkpoint", e))?;
                    encode(&NativeDomainRecord {
                        version: record.version,
                        captured_tick: record.captured_tick,
                        payload,
                    })?
                } else {
                    encode(record)?
                };
                if db(global.get(key.as_str()))?.is_none_or(|old| old.value() != bytes.as_slice()) {
                    db(global.insert(key.as_str(), bytes.as_slice()))?;
                }
            }
        }
        let mut io = NativeCheckpointIo::default();
        let mut keys = BTreeSet::new();
        let chunks_started = std::time::Instant::now();
        write_chunks(&mut |snapshot| {
            let key = super::chunk_key(snapshot.dimension, snapshot.base.pos);
            if !keys.insert(key) || snapshot.effective.world_journal_lsn() > image.tick {
                return Err(failure("chunks", "duplicate participant or future LSN"));
            }
            check_write_geometry(&tx, snapshot.dimension, snapshot.base)?;
            let mut base_written = false;
            {
                let mut imported = db(tx.open_table(super::IMPORTED))?;
                if db(imported.get(key.as_slice()))?.is_none() {
                    let bytes = codec::baseline(snapshot.base, blocks, items)?;
                    db(imported.insert(key.as_slice(), bytes.as_slice()))?;
                    db(db(tx.open_table(super::GENERATED))?.insert(key.as_slice(), ()))?;
                    io.chunk_payload_bytes += bytes.len() as u64;
                    base_written = true;
                }
            }
            let written = super::write_snapshots(
                &tx,
                std::slice::from_ref(&snapshot),
                blocks,
                items,
                SnapshotCommit::WorldCheckpoint(image.tick),
                None,
                Some(&mut io),
            )?;
            if base_written && written == 0 {
                io.rewritten_chunks += 1;
            }
            Ok(())
        })?;
        io.chunk_micros = micros(chunks_started);
        let entities_started = std::time::Instant::now();
        write_entities(&tx, entities, image.tick, &mut io)?;
        io.entity_micros = micros(entities_started);
        {
            let mut table = db(tx.open_table(PLAYERS))?;
            let mut seen = BTreeSet::new();
            for player in image.players {
                if !seen.insert(player.uuid) || player.record.captured_tick != image.tick {
                    return Err(failure("players", "duplicate UUID or capture mismatch"));
                }
                validate_player(&player.record, image.tick)?;
                let bytes = encode(&player.record)?;
                if db(table.get(player.uuid.as_slice()))?
                    .is_none_or(|old| old.value() != bytes.as_slice())
                {
                    db(table.insert(player.uuid.as_slice(), bytes.as_slice()))?;
                }
            }
            validate_player_map(&manifest, &table, image.tick)?;
        }
        let commit_started = std::time::Instant::now();
        db(tx.commit())?;
        io.commit_micros = micros(commit_started);
        Ok(io)
    }
    pub fn read_checkpoint(&self) -> Result<NativeCheckpointImage, NativeError> {
        self.checkpoint_reader()?.read_checkpoint()
    }
    pub fn checkpoint_reader(&self) -> Result<NativeCheckpointReader, NativeError> {
        Ok(NativeCheckpointReader {
            tx: db(self.database.begin_read())?,
        })
    }
    pub fn read_chunk(
        &self,
        dimension: u32,
        position: ChunkPos,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        chest_loot: &ChestLootCatalog,
        generator: &dyn ChunkGenerator,
    ) -> Result<Option<Chunk>, NativeError> {
        self.checkpoint_reader()?
            .read_chunk(dimension, position, blocks, items, chest_loot, generator)
    }
    pub fn visit_chunk_positions(
        &self,
        visit: impl FnMut(u32, ChunkPos) -> Result<(), NativeError>,
    ) -> Result<(), NativeError> {
        self.checkpoint_reader()?.visit_chunk_positions(visit)
    }
    pub fn fill_generated(
        &self,
        dimension: u32,
        chunks: &[Chunk],
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<(), NativeError> {
        let mut tx = db(self.database.begin_write())?;
        db(tx.set_durability(redb::Durability::Immediate))?;
        let mut seen = BTreeSet::new();
        for chunk in chunks {
            check_write_geometry(&tx, dimension, chunk)?;
            if chunk.world_journal_lsn() != 0 {
                return Err(failure("generated", "baseline has gameplay LSN"));
            }
            let key = super::chunk_key(dimension, chunk.pos);
            if !seen.insert(key) {
                return Err(failure("generated", "duplicate participant"));
            }
            let mut imported = db(tx.open_table(super::IMPORTED))?;
            if db(imported.get(key.as_slice()))?.is_some()
                || db(db(tx.open_table(super::APPLIED))?.get(key.as_slice()))?.is_some()
                || db(db(tx.open_table(super::METADATA))?.get(key.as_slice()))?.is_some()
            {
                continue;
            }
            let overlays = db(tx.open_table(super::OVERLAYS))?;
            if db(overlays.range(key.as_slice()..))?
                .next()
                .transpose()
                .map_err(|e| NativeError::Database(e.into()))?
                .is_some_and(|(k, _)| k.value().starts_with(&key))
            {
                continue;
            }
            let bytes = codec::baseline(chunk, blocks, items)?;
            db(imported.insert(key.as_slice(), bytes.as_slice()))?;
            db(db(tx.open_table(super::GENERATED))?.insert(key.as_slice(), ()))?;
        }
        db(tx.commit())
    }
}
fn validate_player_map(
    manifest: &Value,
    table: &impl ReadableTable<&'static [u8], &'static [u8]>,
    tick: u64,
) -> Result<(), NativeError> {
    let map = manifest["players"]
        .as_object()
        .ok_or_else(|| failure("checkpoint/players", "invalid map"))?;
    let mut expected = BTreeMap::new();
    for (key, value) in map {
        let uuid = uuid::Uuid::parse_str(key).map_err(|e| failure("checkpoint/players", e))?;
        let record_tick = value
            .as_u64()
            .filter(|v| *v <= tick)
            .ok_or_else(|| failure("checkpoint/players", "invalid record tick"))?;
        if expected.insert(*uuid.as_bytes(), record_tick).is_some() {
            return Err(failure("checkpoint/players", "duplicate UUID"));
        }
    }
    for row in db(table.iter())? {
        let (uuid, bytes) = db(row)?;
        let uuid: [u8; 16] = uuid
            .value()
            .try_into()
            .map_err(|_| failure("players", "invalid UUID key"))?;
        let record = decode(bytes.value())?;
        validate_player(&record, tick)?;
        if expected.remove(&uuid) != Some(record.captured_tick) {
            return Err(failure(
                "checkpoint/players",
                "record map mismatch (offline records must be retained)",
            ));
        }
    }
    if !expected.is_empty() {
        return Err(failure("checkpoint/players", "missing record"));
    }
    Ok(())
}

/// One pinned read image for offline conversion; all reads share this transaction.
pub struct NativeCheckpointReader {
    pub(super) tx: redb::ReadTransaction,
}
impl NativeReadOnlyStore {
    pub fn checkpoint_reader(&self) -> Result<NativeCheckpointReader, NativeError> {
        Ok(NativeCheckpointReader {
            tx: db(self.database.begin_read())?,
        })
    }
    pub fn read_checkpoint(&self) -> Result<NativeCheckpointImage, NativeError> {
        self.checkpoint_reader()?.read_checkpoint()
    }
    pub fn read_chunk(
        &self,
        dimension: u32,
        position: ChunkPos,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        chest_loot: &ChestLootCatalog,
        generator: &dyn ChunkGenerator,
    ) -> Result<Option<Chunk>, NativeError> {
        self.checkpoint_reader()?
            .read_chunk(dimension, position, blocks, items, chest_loot, generator)
    }
    pub fn visit_chunk_positions(
        &self,
        visit: impl FnMut(u32, ChunkPos) -> Result<(), NativeError>,
    ) -> Result<(), NativeError> {
        self.checkpoint_reader()?.visit_chunk_positions(visit)
    }
}
impl NativeCheckpointReader {
    pub fn read_checkpoint(&self) -> Result<NativeCheckpointImage, NativeError> {
        let mut image = self.read_checkpoint_metadata()?;
        self.visit_entities(|entity| {
            image.entities.push(entity);
            Ok(())
        })?;
        Ok(image)
    }

    /// Read world metadata and players without materializing the entity table.
    /// Recovery must also visit every entity before admitting the world.
    pub fn read_checkpoint_metadata(&self) -> Result<NativeCheckpointImage, NativeError> {
        let table = db(self.tx.open_table(GLOBAL))?;
        let mut globals = BTreeMap::new();
        for row in db(table.iter())? {
            let (key, bytes) = db(row)?;
            globals.insert(key.value().to_owned(), decode(bytes.value())?);
        }
        let tick = globals
            .get("checkpoint")
            .ok_or_else(|| failure("global/checkpoint", "missing"))?
            .captured_tick;
        let header = db(self.tx.open_table(super::HEADER))?;
        let identity =
            db(header.get("identity"))?.ok_or_else(|| failure("world/identity", "missing"))?;
        let manifest = validate_globals(&globals, tick, identity.value())?;
        let table = db(self.tx.open_table(PLAYERS))?;
        validate_player_map(&manifest, &table, tick)?;
        let mut players = Vec::new();
        for row in db(table.iter())? {
            let (uuid, bytes) = db(row)?;
            players.push(NativePlayerSnapshot {
                uuid: uuid
                    .value()
                    .try_into()
                    .map_err(|_| failure("players", "invalid UUID"))?,
                record: decode(bytes.value())?,
            });
        }
        Ok(NativeCheckpointImage {
            tick,
            globals,
            players,
            entities: Vec::new(),
        })
    }

    /// Validate and deliver each entity once from this pinned transaction.
    /// Participant/slot and reverse-slot checks are identical to read_checkpoint.
    pub fn visit_entities(
        &self,
        mut visit: impl FnMut(NativeEntitySnapshot) -> Result<(), NativeError>,
    ) -> Result<(), NativeError> {
        let global = db(self.tx.open_table(GLOBAL))?;
        let checkpoint =
            db(global.get("checkpoint"))?.ok_or_else(|| failure("global/checkpoint", "missing"))?;
        let tick = decode(checkpoint.value())?.captured_tick;
        let table = db(self.tx.open_table(state::ENTITIES))?;
        let applied = db(self.tx.open_table(state::ENTITY_LSN))?;
        let slots = db(self.tx.open_table(state::INITIALIZED))?;
        for row in db(table.iter())? {
            let (uuid, bytes) = db(row)?;
            let uuid: [u8; 16] = uuid
                .value()
                .try_into()
                .map_err(|_| failure("entities", "invalid UUID"))?;
            let entity = state::decode(uuid, bytes.value())?;
            if entity.journal_lsn > tick
                || entity.dimension > 2
                || db(applied.get(uuid.as_slice()))?.is_none_or(|v| v.value() != entity.journal_lsn)
            {
                return Err(failure("entities", "LSN/index mismatch"));
            }
            if let Some(slot) = entity.generated_slot
                && db(slots.get(slot.as_slice()))?.is_none_or(|v| v.value() != uuid)
            {
                return Err(failure("entities", "slot mismatch"));
            }
            visit(entity)?;
        }
        for row in db(slots.iter())? {
            let (slot, id) = db(row)?;
            let record = db(table.get(id.value()))?
                .ok_or_else(|| failure("generated_slots", "orphan index"))?;
            let id = id
                .value()
                .try_into()
                .map_err(|_| failure("generated_slots", "invalid UUID"))?;
            if state::decode(id, record.value())?
                .generated_slot
                .as_ref()
                .map(|v| v.as_slice())
                != Some(slot.value())
            {
                return Err(failure("generated_slots", "reverse index mismatch"));
            }
        }
        Ok(())
    }
    pub fn read_chunk(
        &self,
        dimension: u32,
        position: ChunkPos,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        chest_loot: &ChestLootCatalog,
        generator: &dyn ChunkGenerator,
    ) -> Result<Option<Chunk>, NativeError> {
        let geometry = read_geometry(&self.tx, dimension)?;
        let key = super::chunk_key(dimension, position);
        let imported = db(self.tx.open_table(super::IMPORTED))?;
        let base = if let Some(bytes) = db(imported.get(key.as_slice()))? {
            let root = codec::decode(bytes.value())?;
            let mut chunk = crate::anvil::chunk_nbt::chunk_from_nbt_with_items_at_position_at_tick(
                &root,
                position,
                blocks,
                Some(items),
                chest_loot,
                geometry,
                0,
            )?;
            super::ticks::restore(&root, &mut chunk, blocks)?;
            chunk
        } else {
            let mut covered = db(db(self.tx.open_table(super::APPLIED))?.get(key.as_slice()))?
                .is_some()
                || db(db(self.tx.open_table(super::METADATA))?.get(key.as_slice()))?.is_some();
            let overlays = db(self.tx.open_table(super::OVERLAYS))?;
            covered |= db(overlays.range(key.as_slice()..))?
                .next()
                .transpose()
                .map_err(|e| NativeError::Database(e.into()))?
                .is_some_and(|(k, _)| k.value().starts_with(&key));
            if !covered {
                return Ok(None);
            }
            generator.generate(position)
        };
        if base.pos != position {
            return Err(failure("chunk", "generator position mismatch"));
        }
        super::compose(&self.tx, dimension, &base, blocks, items, chest_loot).map(Some)
    }
    pub fn visit_chunk_positions(
        &self,
        mut visit: impl FnMut(u32, ChunkPos) -> Result<(), NativeError>,
    ) -> Result<(), NativeError> {
        let mut keys = BTreeSet::new();
        for definition in [super::IMPORTED, super::METADATA, super::OVERLAYS] {
            let table = db(self.tx.open_table(definition))?;
            for row in db(table.iter())? {
                let (key, _) = db(row)?;
                let key = key.value();
                if key.len() != 12 && key.len() != 16 {
                    return Err(failure("chunk", "invalid key"));
                }
                keys.insert(<[u8; 12]>::try_from(&key[..12]).expect("checked"));
            }
        }
        for row in db(db(self.tx.open_table(super::APPLIED))?.iter())? {
            let (key, _) = db(row)?;
            keys.insert(
                key.value()
                    .try_into()
                    .map_err(|_| failure("applied_lsn", "invalid key"))?,
            );
        }
        for key in keys {
            let dimension = u32::from_be_bytes(key[..4].try_into().expect("fixed key"));
            read_geometry(&self.tx, dimension)?;
            let x =
                (u32::from_be_bytes(key[4..8].try_into().expect("fixed key")) ^ 0x8000_0000) as i32;
            let z =
                (u32::from_be_bytes(key[8..].try_into().expect("fixed key")) ^ 0x8000_0000) as i32;
            visit(dimension, ChunkPos { x, z })?;
        }
        Ok(())
    }
}
