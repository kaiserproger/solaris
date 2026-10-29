//! Native runtime admission, pinned recovery, and converter codec bridge.
use crate::dimensions::Dimension;
use crate::journal::{CheckpointManifest, Replay};
use crate::{CoreConfig, CoreError};
use mc_nbt::Tag;
use mc_world::native::{
    NativeCheckpointImage, NativeCheckpointReader, NativeDomainRecord, NativeReadOnlyStore,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(crate) fn native_identity(config: &CoreConfig) -> [u8; 32] {
    native_identity_at(config, mc_worldgen::WORLDGEN_REVISION)
}

/// Identity of `config` under an explicit generator revision (the offline
/// worldgen upgrade needs the stored one).
pub(crate) fn native_identity_at(config: &CoreConfig, revision: u32) -> [u8; 32] {
    // Fixed 16-byte format tag, followed by seed and generator revision (BE).
    let mut identity = [0; 32];
    const TAG: [u8; 16] = *b"SolarisV2Native\0";
    identity[..16].copy_from_slice(&TAG);
    identity[16..24].copy_from_slice(&config.seed.to_be_bytes());
    identity[24..].copy_from_slice(&u64::from(revision).to_be_bytes());
    identity
}

pub(crate) fn dimension_id(dimension: Dimension) -> u32 {
    match dimension {
        Dimension::Overworld => 0,
        Dimension::Nether => 1,
        Dimension::End => 2,
    }
}

pub(crate) fn domain_versions() -> BTreeMap<String, u32> {
    [
        "climate",
        "gamerules",
        "block_ticks",
        "fluid_ticks",
        "claims",
        "tickets",
        "raids",
        "end_fight",
        "plugins",
        "plugin_startup",
    ]
    .into_iter()
    .map(|key| (key.into(), 1))
    .collect()
}

pub(crate) fn record(tick: u64, payload: Vec<u8>) -> NativeDomainRecord {
    NativeDomainRecord {
        version: 1,
        captured_tick: tick,
        payload,
    }
}

pub(crate) fn tag_record(tick: u64, tag: &Tag) -> Result<NativeDomainRecord, CoreError> {
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", tag).map_err(|e| CoreError::Storage(e.to_string()))?;
    Ok(record(tick, bytes))
}

pub(crate) fn decode_tag(record: &NativeDomainRecord, key: &str) -> Result<Tag, CoreError> {
    let mut input = record.payload.as_slice();
    let (_, tag) = mc_nbt::read_named(&mut input)
        .map_err(|e| CoreError::Storage(format!("native {key}: {e}")))?;
    if !input.is_empty() || !matches!(tag, Tag::Compound(_)) {
        return Err(CoreError::Storage(format!(
            "native {key}: expected one complete NBT compound"
        )));
    }
    Ok(tag)
}

/// Resolve vanilla statistic names through the same registries gameplay uses.
pub(crate) fn statistics_from_json(
    value: serde_json::Value,
    registry: &crate::registry::CoreRegistry,
) -> Result<mc_data::statistics::PlayerStatistics, CoreError> {
    use mc_data::statistics::StatType;
    mc_data::statistics::PlayerStatistics::from_json(value, |kind, name| {
        let name = mc_data::Identifier::parse(name).ok()?;
        match kind {
            StatType::Mined => registry
                .blocks
                .block(&name)
                .and_then(|block| u32::try_from(block.raw_id).ok()),
            StatType::Killed | StatType::KilledBy => registry.entity_types.id_of(&name),
            StatType::Custom => None,
            _ => registry.items.id_of(&name),
        }
    })
    .map_err(CoreError::Storage)
}

pub(crate) fn statistics_to_json(
    statistics: &mc_data::statistics::PlayerStatistics,
    registry: &crate::registry::CoreRegistry,
) -> Result<serde_json::Value, CoreError> {
    use mc_data::statistics::StatType;
    statistics
        .to_json(|kind, id| {
            let name = match kind {
                StatType::Mined => {
                    // Global states are contiguous in block registration order.
                    // Find the block registry id without building another index.
                    let mut low = 0;
                    let mut high = registry.blocks.len();
                    let mut found = None;
                    while low < high {
                        let mid = low + (high - low) / 2;
                        let state = registry.blocks.by_id(mc_world::BlockStateId(mid as u32))?;
                        match i64::from(state.block.raw_id).cmp(&i64::from(id)) {
                            std::cmp::Ordering::Less => low = mid + 1,
                            std::cmp::Ordering::Greater => high = mid,
                            std::cmp::Ordering::Equal => {
                                found = Some(&state.block.id);
                                break;
                            }
                        }
                    }
                    found
                }
                StatType::Killed | StatType::KilledBy => registry.entity_types.name_of(id),
                StatType::Custom => None,
                _ => registry.items.name_of(id),
            }?;
            Some(name.as_str().to_owned())
        })
        .map_err(CoreError::Storage)
}

pub(crate) fn generation_records(
    config: &CoreConfig,
) -> Result<BTreeMap<String, NativeDomainRecord>, CoreError> {
    generation_records_at(config, mc_worldgen::WORLDGEN_REVISION)
}

pub(crate) fn generation_records_at(
    config: &CoreConfig,
    revision: u32,
) -> Result<BTreeMap<String, NativeDomainRecord>, CoreError> {
    let mut dimensions = serde_json::Map::new();
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let geometry = dimension.geometry();
        dimensions.insert(dimension_id(dimension).to_string(), serde_json::json!({
            "name": dimension.name(), "min_y": geometry.min_y(), "height": geometry.height(),
            "generator": match dimension { Dimension::Overworld => "vanilla_like", Dimension::Nether => "nether", Dimension::End => "end" },
            "seed": config.seed, "revision": revision
        }));
    }
    let encode = |value: serde_json::Value| {
        serde_json::to_vec(&value).map_err(|e| CoreError::Storage(e.to_string()))
    };
    Ok(BTreeMap::from([
        (
            "dimensions".into(),
            record(0, encode(serde_json::Value::Object(dimensions.clone()))?),
        ),
        (
            "generation".into(),
            record(
                0,
                encode(
                    serde_json::json!({"identity": native_identity_at(config, revision), "base": {"seed": config.seed, "revision": revision}, "generators": dimensions}),
                )?,
            ),
        ),
    ]))
}

pub(crate) fn validate_manifest(manifest: &CheckpointManifest) -> Result<(), CoreError> {
    let mut expected_versions = domain_versions();
    if !manifest.domain_versions.contains_key("plugins")
        && !manifest.domain_versions.contains_key("plugin_startup")
    {
        expected_versions.remove("plugins");
        expected_versions.remove("plugin_startup");
    }
    if manifest.domain_versions != expected_versions {
        return Err(CoreError::Storage(
            "native checkpoint: unsupported required domain versions".into(),
        ));
    }
    if manifest.dimensions.len() != 2
        || !manifest.dimensions.contains_key(&Dimension::Nether)
        || !manifest.dimensions.contains_key(&Dimension::End)
    {
        return Err(CoreError::Storage(
            "native checkpoint: expected Nether and End child domains".into(),
        ));
    }
    for image in std::iter::once(manifest).chain(manifest.dimensions.values().map(|image| &**image))
    {
        if image.tick != manifest.tick
            || image.domain_versions != expected_versions
            || !image.dimensions.is_empty() && !std::ptr::eq(image, manifest)
        {
            return Err(CoreError::Storage(
                "native checkpoint: child tick, required versions or nesting mismatch".into(),
            ));
        }
        if image
            .world_spawn_rotation
            .iter()
            .any(|value| !value.is_finite())
        {
            return Err(CoreError::Storage(
                "native checkpoint: invalid spawn postimage".into(),
            ));
        }
        if image.tickets.iter().any(|(owner, (_, (tick, radius)))| {
            owner.is_empty()
                || *tick > manifest.tick
                || *radius < 0
                || owner.starts_with("solaris:portal:")
        }) {
            return Err(CoreError::Storage(
                "native checkpoint: invalid durable ticket postimage".into(),
            ));
        }
        let mut inhabited = std::collections::BTreeSet::new();
        if image.inhabited_time.iter().any(|counter| {
            !counter.active
                || counter.tick != image.tick
                || counter.value < 0
                || !inhabited.insert(counter.chunk)
        }) {
            return Err(CoreError::Storage(
                "native checkpoint: invalid inhabited interval".into(),
            ));
        }

        let mut debt = std::collections::BTreeSet::new();
        if image
            .debt
            .iter()
            .any(|record| record.tick != image.tick || !debt.insert(record.chunk))
        {
            return Err(CoreError::Storage(
                "native checkpoint: invalid sim debt image".into(),
            ));
        }
        image.block_ticks.validate()?;
        image.fluid_ticks.validate()?;
        image.border.validate()?;
        if let Some(raids) = &image.raids {
            raids.validate()?;
        }
        if let Some(fight) = &image.end_fight {
            fight.validate().map_err(CoreError::Storage)?;
        }
    }
    Ok(())
}

/// Pure recovered images plus one pinned native reader; no owners or writes.
pub(crate) struct RecoveredWorld {
    pub(crate) registry: Arc<crate::registry::CoreRegistry>,
    pub(crate) config: CoreConfig,
    /// Pinned metadata/player images only; entities hydrate directly into `dimensions`.
    pub(crate) checkpoint: NativeCheckpointImage,
    pub(crate) manifest: CheckpointManifest,
    pub(crate) dimensions: BTreeMap<Dimension, Replay>,
    pub(crate) reader: NativeCheckpointReader,
}

pub(crate) fn recover_offline(config: &CoreConfig) -> Result<RecoveredWorld, CoreError> {
    let mut registry = crate::registry::CoreRegistry::load(config.vanilla_data_dir.as_deref())?;
    // Offline recovery resolves the same pinned native types as live startup.
    // No tick events are started and no guest command is applied.
    let runtime = crate::plugin::PluginRuntime::open(config)?;
    match &runtime {
        Some(runtime) => crate::plugin_startup::apply(
            &mut registry,
            runtime.contribution(),
            runtime.ore_profile(),
            runtime.client_bundles(),
        )?,
        None => crate::plugin_startup::apply(
            &mut registry,
            &mc_plugin_host::DeploymentContribution::default(),
            None,
            &[],
        )?,
    }
    drop(runtime);
    let registry = Arc::new(registry);
    let path = config.world_dir.join("native/state.redb");
    if !path.is_file() {
        return Err(import_required(config));
    }
    let store =
        NativeReadOnlyStore::open_with_cache(&path, native_identity(config), 16 * 1024 * 1024)
            .map_err(|e| open_refusal(config, &path, e))?;
    let reader = store
        .checkpoint_reader()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    recover_with_reader(config, registry, reader)
}

pub(crate) fn import_required(config: &CoreConfig) -> CoreError {
    CoreError::Storage(format!(
        "{} has persisted history but no native checkpoint; use solaris-v2 world import-anvil --source ANVIL --destination NATIVE --receipt JSON",
        config.world_dir.display()
    ))
}

/// Revision stored in `identity` when it is `config`'s identity at some other
/// generator revision, i.e. nothing but the worldgen revision differs.
pub(crate) fn revision_only_difference(config: &CoreConfig, identity: [u8; 32]) -> Option<u32> {
    let revision = u32::try_from(u64::from_be_bytes(identity[24..].try_into().ok()?)).ok()?;
    (native_identity_at(config, revision) == identity).then_some(revision)
}

/// Refusal for a store that failed identity admission. A world that differs only
/// in an older worldgen revision gets the explicit operator path; everything else
/// keeps the plain storage refusal.
pub(crate) fn open_refusal(
    config: &CoreConfig,
    path: &std::path::Path,
    error: mc_world::native::NativeError,
) -> CoreError {
    let current = mc_worldgen::WORLDGEN_REVISION;
    if matches!(
        error,
        mc_world::native::NativeError::Invalid("identity mismatch")
    ) && let Ok(identity) = mc_world::native::NativeStore::read_identity(path)
        && let Some(stored) = revision_only_difference(config, identity)
        && stored < current
    {
        // Only offer the revision upgrade when it would lead to a world that opens.
        if let Err(unreadable) = stored_state_readable(config, path, stored) {
            return read_refusal(config, unreadable);
        }
        return CoreError::Storage(format!(
            "{error}: world was generated with worldgen revision {stored}, this build is \
             revision {current}; to keep it, stop the server and run `solaris-v2 world \
             upgrade-worldgen --config <server.toml>` (existing chunks stay as generated, only \
             not-yet-generated chunks use revision {current}), or use a fresh world_dir"
        ));
    }
    CoreError::Storage(error.to_string())
}

/// Whether the stored checkpoint of the world at `path`, recorded at worldgen
/// `revision`, is otherwise readable by this build. The offline worldgen upgrade and its
/// startup hint only make sense for such worlds; a revision rewrite cannot fix the rest.
pub(crate) fn stored_state_readable(
    config: &CoreConfig,
    path: &std::path::Path,
    revision: u32,
) -> Result<(), mc_world::native::NativeError> {
    NativeReadOnlyStore::open_with_cache(
        path,
        native_identity_at(config, revision),
        16 * 1024 * 1024,
    )?
    .checkpoint_reader()?
    .read_checkpoint_metadata()
    .map(drop)
}

/// Operator guidance for a world whose stored globals this build does not read: written
/// by an older Solaris storage format, so the supported route is an Anvil round trip.
pub(crate) fn older_format_refusal(config: &CoreConfig) -> CoreError {
    let world = config.world_dir.display();
    CoreError::Storage(format!(
        "world {world} was written by an older Solaris storage format that this build cannot \
         read (its native checkpoint lacks a domain this build requires); to keep it, with the \
         Solaris binary that created it run `solaris world export-anvil --config <server.toml> \
         --source {world} --destination <ANVIL_DIR> --receipt export.json`, then with this \
         binary run `solaris-v2 world import-anvil --config <server.toml> --source <ANVIL_DIR> \
         --destination <NEW_WORLD_DIR> --receipt import.json`"
    ))
}

/// Storage error for a failed checkpoint read, with operator guidance where known.
pub(crate) fn read_refusal(config: &CoreConfig, error: mc_world::native::NativeError) -> CoreError {
    match error {
        mc_world::native::NativeError::GlobalDomains => older_format_refusal(config),
        error => CoreError::Storage(error.to_string()),
    }
}

/// Validate and hydrate one stored native entity exactly as recovery does,
/// with the identifying context (dimension, store path, chunk, wire-id, UUID,
/// type) on every refusal. Shared by recovery and the read-only census.
pub(crate) fn hydrate_visited_entity(
    registry: &crate::registry::CoreRegistry,
    config: &CoreConfig,
    replay: &mut Replay,
    dimension: Dimension,
    entity: &mc_world::native::NativeEntitySnapshot,
    payload: &Tag,
    manifest_tick: u64,
) -> Result<(), mc_world::native::NativeError> {
    (|| {
        if let Tag::Compound(fields) = payload
            && let Some(Tag::IntArray(words)) = mc_data::item_nbt::field(fields, "UUID")
            && words.len() == 4
        {
            let mut bytes = [0; 16];
            for (bytes, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
                bytes.copy_from_slice(&word.to_be_bytes());
            }
            if bytes != entity.identity {
                return Err(CoreError::Storage(format!(
                    "payload UUID {} differs from native entity record key {}",
                    uuid::Uuid::from_bytes(bytes),
                    uuid::Uuid::from_bytes(entity.identity),
                )));
            }
        }
        crate::dimensions::recovery::hydrate_entity(registry, replay, payload, manifest_tick)
    })()
    .map_err(|error| {
        let fields = match payload {
            Tag::Compound(fields) => fields.as_slice(),
            _ => &[],
        };
        let field = |key| mc_data::item_nbt::field(fields, key);
        let name = match field("id") {
            Some(Tag::String(name)) => name.as_str(),
            _ => "<invalid>",
        };
        let type_id = mc_data::Identifier::parse(name)
            .ok()
            .and_then(|name| registry.entity_types.id_of(&name))
            .map(i64::from)
            .unwrap_or(-1);
        let wire_id = match field("solaris:WireId") {
            Some(Tag::Int(id)) => id.to_string(),
            value => format!("{value:?}"),
        };
        let uuid = match field("UUID") {
            Some(Tag::IntArray(words)) if words.len() == 4 => {
                let mut bytes = [0; 16];
                for (bytes, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
                    bytes.copy_from_slice(&word.to_be_bytes());
                }
                uuid::Uuid::from_bytes(bytes).to_string()
            }
            Some(tag) => format!("{tag:?}"),
            None => "<missing>".into(),
        };
        mc_world::native::NativeError::Source(format!(
            "{}: {}: chunk ({}, {}), entity wire-id {wire_id} UUID {uuid} \
             type-id {type_id} ({name}): {error}",
            dimension.name(),
            config.world_dir.join("native/state.redb").display(),
            entity.position.x,
            entity.position.z,
        ))
    })
}

pub(crate) fn recover_with_reader(
    config: &CoreConfig,
    registry: Arc<crate::registry::CoreRegistry>,
    reader: NativeCheckpointReader,
) -> Result<RecoveredWorld, CoreError> {
    let checkpoint = reader
        .read_checkpoint_metadata()
        .map_err(|e| read_refusal(config, e))?;
    let manifest: CheckpointManifest =
        serde_json::from_slice(&checkpoint.globals["checkpoint"].payload)
            .map_err(|e| CoreError::Storage(format!("native checkpoint: {e}")))?;
    validate_manifest(&manifest)?;
    let expected = generation_records(config)?;
    for key in ["generation", "dimensions"] {
        if checkpoint.globals[key].payload != expected[key].payload {
            return Err(CoreError::Worldgen(format!(
                "native {key}: configured generation identity mismatch"
            )));
        }
    }
    crate::level_metadata::from_tag(&decode_tag(
        &checkpoint.globals["level_metadata"],
        "level_metadata",
    )?)?;
    decode_tag(
        &checkpoint.globals["conversion_extras"],
        "conversion_extras",
    )?;
    let scoreboard = crate::scoreboard::Scoreboard::from_nbt(&decode_tag(
        &checkpoint.globals["scoreboard"],
        "scoreboard",
    )?)
    .map_err(CoreError::Storage)?;
    let bossbars = crate::bossbar::BossBars::from_nbt(&decode_tag(
        &checkpoint.globals["bossbars"],
        "bossbars",
    )?)
    .map_err(CoreError::Storage)?;
    let mut dimensions = BTreeMap::new();
    let mut players = BTreeMap::new();
    for player in &checkpoint.players {
        let decoded = crate::journal::player_from_tag(
            &decode_tag(&player.record, "player")?,
            &registry.items,
        )?;
        players.insert(uuid::Uuid::from_bytes(player.uuid).to_string(), decoded);
    }
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let image = if dimension == Dimension::Overworld {
            &manifest
        } else {
            manifest.dimensions.get(&dimension).ok_or_else(|| {
                CoreError::Storage(format!("native checkpoint: missing {}", dimension.name()))
            })?
        };
        let mut replay = Replay {
            tick: manifest.tick,
            players: players.clone(),
            scoreboard: Some(scoreboard.clone()),
            command_storage: checkpoint
                .globals
                .get("command_storage")
                .map(|record| record.payload.clone()),
            saved_maps: if dimension == Dimension::Overworld {
                crate::maps::recover(&checkpoint.globals)?
            } else {
                BTreeMap::new()
            },
            bossbars: Some(bossbars.to_bytes()),
            raids: image.raids.clone(),
            end_fight: image.end_fight.clone(),
            wandering_trader: image.wandering_trader,
            scheduled_functions: image.scheduled_functions.clone(),
            generated_entity_claims: image.generated_entity_claims.clone(),
            tickets: image.tickets.clone(),
            block_ticks: image.block_ticks.clone(),
            fluid_ticks: image.fluid_ticks.clone(),
            inhabited_time: image
                .inhabited_time
                .iter()
                .map(|image| (image.chunk, *image))
                .collect(),
            sim_debt: image
                .debt
                .iter()
                .map(|image| (image.chunk, *image))
                .collect(),
            climate: manifest.climate,
            border: image.border,
            difficulty: manifest.difficulty,
            gamerules: manifest.gamerules.clone(),
            ..Default::default()
        };
        if dimension == Dimension::Overworld && manifest.domain_versions.contains_key("plugins") {
            let record = checkpoint.globals.get("plugins").ok_or_else(|| {
                CoreError::Storage("native plugins: missing required checkpoint domain".into())
            })?;
            if record.version != 1 || record.captured_tick != manifest.tick {
                return Err(CoreError::Storage(
                    "native plugins: invalid checkpoint record header".into(),
                ));
            }
            replay.plugins = serde_json::from_slice(&record.payload)
                .map_err(|e| CoreError::Storage(format!("native plugins: {e}")))?;
            replay.plugins.validate()?;
        }
        dimensions.insert(dimension, replay);
    }
    // Checkpointed block-log rows precede the WAL-tail deltas folded below.
    for (dimension, rows) in crate::block_log::load(&reader, &registry.blocks, &registry.items)? {
        if let Some(replay) = dimensions.get_mut(&dimension) {
            replay.block_log.push(rows);
        }
    }
    reader
        .visit_entities(|mut entity| {
            let dimension = match entity.dimension {
                0 => Dimension::Overworld,
                1 => Dimension::Nether,
                2 => Dimension::End,
                _ => unreachable!("native reader validates dimension"),
            };
            let replay = dimensions
                .get_mut(&dimension)
                .expect("initialized dimensions");
            if let Some(payload) = entity.payload.take() {
                hydrate_visited_entity(
                    &registry,
                    config,
                    replay,
                    dimension,
                    &entity,
                    &payload,
                    manifest.tick,
                )?;
            }
            Ok(())
        })
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let recovered_tick = crate::journal::visit_complete_batches_after(
        &config.world_dir.join("journal"),
        manifest.tick,
        |batch| {
            let batches = [batch];
            for (&dimension, replay) in &mut dimensions {
                let mut routed = crate::dimensions::journal_for_dimension(&batches, dimension);
                if dimension != Dimension::Overworld {
                    for (_, _, entries) in &mut routed {
                        entries.retain(|entry| {
                            !matches!(
                                entry,
                                crate::journal::JournalEntry::Player { .. }
                                    | crate::journal::JournalEntry::Statistics { .. }
                            )
                        });
                    }
                }
                for (lo, hi, entries) in &routed {
                    for entry in entries {
                        if let crate::journal::JournalEntry::InhabitedTime { image } = entry
                            && (image.value < 0 || image.tick < *lo || image.tick > *hi)
                        {
                            return Err(CoreError::Storage(format!(
                                "native inhabited interval recovery dimension {dimension:?}: invalid boundary"
                            )));
                        }
                        if let crate::journal::JournalEntry::SimDebt { image } = entry
                            && (image.tick < *lo || image.tick > *hi)
                        {
                            return Err(CoreError::Storage(format!(
                                "native sim debt recovery dimension {dimension:?}: invalid boundary"
                            )));
                        }
                    }
                }
                *replay = crate::journal::apply_batch_images(std::mem::take(replay), &routed);
            }
            Ok(())
        },
    )?;
    for (&dimension, replay) in &mut dimensions {
        crate::journal::advance_replay_clocks(replay);
        replay.plugins.validate()?;
        replay.block_ticks.validate().map_err(|error| {
            CoreError::Storage(format!(
                "native scheduler recovery dimension {dimension:?}: {error}"
            ))
        })?;
        replay.fluid_ticks.validate().map_err(|error| {
            CoreError::Storage(format!(
                "native scheduler recovery dimension {dimension:?}: {error}"
            ))
        })?;
        crate::mounts::persistence::validate_replay(replay)?;
    }
    let root = &dimensions[&Dimension::Overworld];
    let mut players = root.players.clone();
    for player in players.values_mut() {
        player.statistics.freeze_at_tick(recovered_tick);
        player.warden_warning.freeze_at_tick(recovered_tick);
    }
    let scoreboard = root.scoreboard.clone();
    let bossbars = root.bossbars.clone();
    let climate = root.climate;
    let difficulty = root.difficulty;
    let gamerules = root.gamerules.clone();
    let mut ids = std::collections::BTreeSet::new();
    let mut uuids = std::collections::BTreeSet::new();
    for replay in dimensions.values_mut() {
        replay.players = players.clone();
        replay.scoreboard = scoreboard.clone();
        replay.bossbars = bossbars.clone();
        replay.climate = climate;
        replay.difficulty = difficulty;
        replay.gamerules = gamerules.clone();
        for (&id, entity) in &replay.entities {
            if !ids.insert(id) || !uuids.insert(entity.uuid.clone()) {
                return Err(CoreError::Storage(format!(
                    "native recovery: duplicate entity identity WireId {id} UUID {}",
                    entity.uuid
                )));
            }
        }
    }
    Ok(RecoveredWorld {
        config: config.clone(),
        registry,
        checkpoint,
        manifest,
        dimensions,
        reader,
    })
}

pub(crate) fn encode_players<'a>(
    players: impl Iterator<Item = &'a crate::core::CheckpointPlayerData>,
    items: &mc_data::items::ItemRegistry,
    tick: u64,
) -> Result<Vec<mc_world::native::NativePlayerSnapshot>, CoreError> {
    players
        .map(|player| {
            let uuid = player
                .uuid
                .parse::<uuid::Uuid>()
                .map_err(|e| CoreError::Storage(e.to_string()))?;
            Ok(mc_world::native::NativePlayerSnapshot {
                uuid: *uuid.as_bytes(),
                record: tag_record(tick, &crate::journal::player_to_tag(&player.record, items)?)?,
            })
        })
        .collect()
}

pub(crate) fn encode_entities(
    dimension: Dimension,
    chunks: &BTreeMap<(i32, i32), mc_world::anvil::EntityChunk>,
    items: &mc_data::items::ItemRegistry,
    tick: u64,
) -> Result<Vec<mc_world::native::NativeEntitySnapshot>, CoreError> {
    entity_snapshots(dimension, chunks, items, tick).collect()
}

/// Encode one frozen owner record at a time, without retaining a second world of NBT.
pub(crate) fn entity_snapshots<'a>(
    dimension: Dimension,
    chunks: &'a BTreeMap<(i32, i32), mc_world::anvil::EntityChunk>,
    items: &'a mc_data::items::ItemRegistry,
    tick: u64,
) -> impl Iterator<Item = Result<mc_world::native::NativeEntitySnapshot, CoreError>> + 'a {
    chunks
        .iter()
        .flat_map(|(&(x, z), chunk)| chunk.entities.iter().map(move |entity| (x, z, entity)))
        .map(move |(x, z, entity)| {
            Ok(mc_world::native::NativeEntitySnapshot {
                identity: *entity.uuid.as_bytes(),
                dimension: dimension_id(dimension),
                position: mc_world::ChunkPos { x, z },
                generated_slot: None,
                journal_lsn: tick,
                payload: Some(Tag::Compound(
                    mc_data::entity_nbt::entity_to_fields(items, entity)
                        .map_err(|e| CoreError::Storage(e.to_string()))?,
                )),
            })
        })
}

/// Build the immutable configured generator without reading or publishing world metadata.
pub(crate) fn generator(
    config: &CoreConfig,
    registry: &crate::registry::CoreRegistry,
    dimension: Dimension,
) -> Result<Arc<dyn mc_world::ChunkGenerator>, CoreError> {
    Ok(match dimension {
        Dimension::Overworld => {
            let mut terrain =
                mc_worldgen::TerrainGenerator::new(config.seed, Arc::clone(&registry.blocks));
            if let Some(path) = config.vanilla_data_dir.as_deref() {
                terrain
                    .load_vanilla_worldgen(path)
                    .map_err(CoreError::Worldgen)?;
            }
            Arc::new(terrain)
        }
        Dimension::Nether => Arc::new(crate::core::nether_terrain(config, registry)?),
        Dimension::End => Arc::new(crate::core::end_terrain(config, registry)?),
    })
}

/// Encode the recovered WAL-tail image for offline conversion, without writes or ticks.
pub(crate) fn encode_checkpoint(
    world: &RecoveredWorld,
) -> Result<NativeCheckpointImage, CoreError> {
    let mut manifest = world.manifest.clone();
    let tick = world.dimensions[&Dimension::Overworld].tick;
    let mut entities = Vec::new();
    let mut players = Vec::new();
    for (&dimension, replay) in &world.dimensions {
        let image = if dimension == Dimension::Overworld {
            &mut manifest
        } else {
            manifest
                .dimensions
                .get_mut(&dimension)
                .expect("validated dimensions")
        };
        image.tick = tick;
        image.border = replay.border;
        image.block_ticks = replay.block_ticks.clone();
        image.fluid_ticks = replay.fluid_ticks.clone();
        image.inhabited_time = replay
            .inhabited_time
            .values()
            .filter(|image| image.active)
            .map(|image| crate::journal::InhabitedTimeImage {
                value: image.value_at(tick),
                tick,
                ..*image
            })
            .collect();

        image.debt = replay
            .sim_debt
            .values()
            .map(|image| crate::journal::SimDebtImage {
                last_sim: image.last_sim_at(tick),
                remaining: image.remaining,
                tick,
                ..*image
            })
            .collect();
        image.generated_entity_claims = replay.generated_entity_claims.clone();
        image.tickets = replay.tickets.clone();
        image.raids = replay.raids.clone();
        image.wandering_trader = replay.wandering_trader;
        image
            .scheduled_functions
            .clone_from(&replay.scheduled_functions);
        image.end_fight = replay.end_fight.clone();
        let memory = crate::world::ChunkWorld::new(
            world.config.seed,
            generator(&world.config, &world.registry, dimension)?,
            &world.registry,
            mc_world::BlockStateId(world.registry.states.air),
            image.world_spawn.unwrap_or([0, 80, 0]),
            dimension.geometry(),
        );
        let mut state = crate::tick::TickState::new(Arc::clone(&world.registry), memory, 0, 1);
        state.dimension = dimension;
        state.tick = tick;
        state.raids = replay.raids.clone().unwrap_or_default();
        state.trader_spawner = replay.wandering_trader.unwrap_or_default();
        state
            .datapacks
            .restore(replay.scheduled_functions.clone())?;
        state.end_fight = replay.end_fight.clone().unwrap_or_default();
        for (&id, entity) in &replay.entities {
            crate::core::restore_entity(&mut state, id, entity)?;
        }
        let (_, captured, _) = crate::core::checkpoint_data_for(&state, tick);
        entities.extend(encode_entities(
            dimension,
            &captured,
            &world.registry.items,
            tick,
        )?);
    }
    let root = &world.dimensions[&Dimension::Overworld];
    manifest.climate = root.climate;
    manifest.difficulty = root.difficulty;
    manifest.gamerules = root.gamerules.clone();
    for (uuid, player) in &root.players {
        let mut player = player.clone();
        player.tick = tick;
        manifest.players.insert(uuid.clone(), tick);
        players.push(mc_world::native::NativePlayerSnapshot {
            uuid: *uuid
                .parse::<uuid::Uuid>()
                .map_err(|e| CoreError::Storage(e.to_string()))?
                .as_bytes(),
            record: tag_record(
                tick,
                &crate::journal::player_to_tag(&player, &world.registry.items)?,
            )?,
        });
    }
    validate_manifest(&manifest)?;
    let mut globals = world.checkpoint.globals.clone();
    crate::maps::checkpoint(&root.saved_maps, tick, &mut globals)?;
    // Settlement rows follow every dimension WAL tail: live images overwrite
    // rows tagged with their home dimension, tombstones delete them. Sequence
    // is the max over rows, tails, and the durable row so exported worlds
    // never reuse an id.
    let mut sequence =
        crate::settlements::persistence::checkpoint_sequence(&globals, world.manifest.tick)
            .unwrap_or(0);
    for (&dimension, replay) in &world.dimensions {
        sequence = sequence.max(replay.settlement_high_water);
        for (&id, settlement) in &replay.settlements {
            sequence = sequence.max(id).max(settlement.id.0);
            globals.insert(
                crate::settlements::persistence::row_key(settlement.id),
                record(
                    tick,
                    serde_json::to_vec(&crate::settlements::persistence::Row {
                        dimension,
                        value: settlement,
                    })
                    .map_err(|e| CoreError::Storage(e.to_string()))?,
                ),
            );
        }
        for id in &replay.settlement_removed {
            if !replay.settlements.contains_key(id) {
                globals.remove(&format!("settlement/{id}"));
            }
        }
    }
    globals.insert(
        "settlement_sequence".into(),
        record(
            tick,
            serde_json::to_vec(&sequence).map_err(|e| CoreError::Storage(e.to_string()))?,
        ),
    );
    globals.insert(
        "checkpoint".into(),
        record(
            tick,
            serde_json::to_vec(&manifest).map_err(|e| CoreError::Storage(e.to_string()))?,
        ),
    );
    globals.insert(
        "scoreboard".into(),
        tag_record(
            tick,
            &root
                .scoreboard
                .as_ref()
                .expect("required scoreboard")
                .to_nbt(),
        )?,
    );
    if let Some(storage) = &root.command_storage {
        let tag = crate::command_storage::to_tag(
            &crate::command_storage::decode(storage).map_err(CoreError::Storage)?,
        );
        globals.insert("command_storage".into(), tag_record(tick, &tag)?);
    }
    globals.insert(
        "bossbars".into(),
        tag_record(
            tick,
            &crate::bossbar::BossBars::from_bytes(
                root.bossbars.as_ref().expect("required bossbars"),
            )
            .map_err(CoreError::Storage)?
            .to_nbt(),
        )?,
    );
    refresh_metadata(&mut globals, &manifest)?;
    for global in globals.values_mut() {
        global.captured_tick = tick;
    }
    Ok(NativeCheckpointImage {
        tick,
        globals,
        players,
        entities,
    })
}

pub(crate) fn refresh_metadata(
    globals: &mut BTreeMap<String, NativeDomainRecord>,
    manifest: &CheckpointManifest,
) -> Result<(), CoreError> {
    let generation_record = globals
        .get("generation")
        .ok_or_else(|| CoreError::Storage("missing native generation record".into()))?;
    let generation: serde_json::Value = serde_json::from_slice(&generation_record.payload)
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let seed = generation["base"]["seed"]
        .as_i64()
        .ok_or_else(|| CoreError::Storage("native generation missing seed".into()))?;
    let mut templates = crate::level_metadata::from_tag(&decode_tag(
        globals
            .get("level_metadata")
            .ok_or_else(|| CoreError::Storage("missing native level metadata record".into()))?,
        "level_metadata",
    )?)?;
    crate::level_metadata::checkpoint_tags(&mut templates, seed, manifest)?;
    globals.insert(
        "level_metadata".into(),
        tag_record(manifest.tick, &crate::level_metadata::to_tag(&templates))?,
    );
    Ok(())
}

#[cfg(test)]
#[path = "native_checkpoint_tests.rs"]
pub(crate) mod tests;
