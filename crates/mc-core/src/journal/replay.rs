//! Replay reducer: batch images, chunk clocks and entity state images.
//! Moved verbatim from `journal.rs`.

use super::*;

/// Replayed state: absolute post-images keyed by domain.
#[derive(Debug, Default)]
pub struct Replay {
    pub dimensions: BTreeMap<crate::dimensions::Dimension, Box<Replay>>,
    pub(crate) plugins: crate::plugin::PluginState,
    /// Block post-images.
    pub blocks: BTreeMap<(i32, i32, i32), u32>,
    /// Player post-images by uuid string.
    pub players: BTreeMap<String, ReplayPlayer>,
    /// Block-entity post-images by block position.
    pub block_entities: BTreeMap<(i32, i32, i32), ReplayBlockEntity>,
    pub opaque_block_entities: BTreeMap<(i32, i32, i32), Option<Vec<u8>>>,
    /// Entity post-images by wire id.
    pub entities: BTreeMap<i32, ReplayEntity>,
    /// Ticket post-images by owner: centre, (tick, radius).
    pub tickets: BTreeMap<String, ([i32; 2], (u64, i32))>,
    /// Latest scoreboard image, including an empty-board reset.
    pub scoreboard: Option<crate::scoreboard::Scoreboard>,
    pub command_storage: Option<Vec<u8>>,
    pub saved_maps: BTreeMap<i32, Arc<crate::maps::SavedMap>>,
    /// Latest bossbar image (named-NBT bytes), including an empty-registry reset.
    pub bossbars: Option<Vec<u8>>,
    pub end_fight: Option<crate::end_fight::Fight>,
    pub raids: Option<crate::raids::Raids>,
    pub wandering_trader: Option<crate::mob_lanes::TraderSpawner>,
    pub scheduled_functions: Vec<(u64, String)>,
    pub datapack_selection: Option<crate::datapacks::Selection>,
    pub generated_entity_claims: std::collections::BTreeSet<String>,
    pub block_ticks: crate::block_ticks::BlockTicksImage,
    pub fluid_ticks: crate::fluids::FluidTicksImage,
    pub inhabited_time: BTreeMap<[i32; 2], InhabitedTimeImage>,
    /// 001 slice0 per-chunk catch-up debt (authority, active-only clock).
    pub sim_debt: BTreeMap<[i32; 2], SimDebtImage>,
    /// Transient cumulative active time while folding the existing debt images.
    pub(crate) simulation_clocks: BTreeMap<[i32; 2], ReplayChunkClock>,
    pub climate: Option<crate::weather::ClimateImage>,
    pub border: crate::command_border::Border,
    pub difficulty: crate::survival::Difficulty,
    pub gamerules: BTreeMap<String, String>,
    pub settlements: BTreeMap<u64, crate::settlements::SettlementRecord>,
    pub settlement_removed: BTreeSet<u64>,
    pub settlement_high_water: u64,
    /// Block-log table rows followed by WAL-tail deltas, in order.
    pub block_log: Vec<crate::block_log::Delta>,
    /// Highest tick seen.
    pub tick: u64,
}

#[derive(Debug)]
pub(crate) struct ReplayChunkClock {
    pub(super) points: Vec<ReplayClockPoint>,
}

#[derive(Debug)]
pub(super) struct ReplayClockPoint {
    pub(super) tick: u64,
    pub(super) elapsed: u64,
    pub(super) active: bool,
}

impl ReplayChunkClock {
    pub(super) fn new(tick: u64, active: bool) -> Self {
        Self {
            points: vec![ReplayClockPoint {
                tick,
                elapsed: 0,
                active,
            }],
        }
    }

    pub(super) fn through(&self, tick: u64) -> u64 {
        let index = self.points.partition_point(|point| point.tick <= tick);
        let Some(point) = index.checked_sub(1).map(|index| &self.points[index]) else {
            return 0;
        };
        point.elapsed + if point.active { tick - point.tick } else { 0 }
    }

    pub(super) fn between(&self, from: u64, to: u64) -> u64 {
        self.through(to).saturating_sub(self.through(from))
    }

    pub(super) fn observe(&mut self, image: SimDebtImage) {
        if self
            .points
            .last()
            .is_some_and(|point| point.active == image.active)
        {
            return;
        }
        // Inactive images may be drained later than the actual exit. Their
        // last simulated tick, not their publication tick, closes the interval.
        let tick = if image.active {
            image.tick
        } else {
            image.tick.min(image.last_sim.saturating_add(1))
        };
        let elapsed = if image.active {
            self.through(tick.saturating_sub(1)) + 1
        } else {
            self.through(image.last_sim)
        };
        self.points.push(ReplayClockPoint {
            tick,
            elapsed,
            active: image.active,
        });
    }

    pub(super) fn advance_villager(
        &self,
        villager: &mut crate::villager::VillagerState,
        from: u64,
        to: u64,
    ) {
        for (index, point) in self.points.iter().enumerate() {
            if !point.active {
                continue;
            }
            let start = from.max(point.tick.saturating_sub(1));
            let end = self
                .points
                .get(index + 1)
                .map_or(to, |next| to.min(next.tick.saturating_sub(1)));
            if start < end {
                crate::villager::persistence::advance_clocks(villager, start, end);
            }
        }
    }
}

pub(super) fn replay_entity_chunk(pos: [f64; 3]) -> [i32; 2] {
    [
        (pos[0].floor() as i32).div_euclid(16),
        (pos[2].floor() as i32).div_euclid(16),
    ]
}

/// One replayed entity record. Ordinary intra-chunk mob motion is checkpoint
/// durable; crossing images advance the position for chunk clock attribution.
/// Projectile images also advance motion with retained custody/payload,
/// preventing a replay from rewinding a missile across an already committed hit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReplayEntity {
    pub uuid: String,
    pub type_id: u32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub ground: bool,
    pub health: f32,
    pub alive: bool,
    pub age: u32,
    pub breed_cooldown: u32,
    pub pickup_delay: u32,
    pub item: Option<mc_data::ItemStack>,
    pub appearance: EntityAppearanceImage,
    #[serde(default)]
    pub active_effects: crate::effects::ActiveEffects,
    #[serde(
        default,
        deserialize_with = "crate::villager::persistence::deserialize_optional_state"
    )]
    pub villager: Option<crate::villager::VillagerState>,
    #[serde(default)]
    pub state: EntityStateImage,
    #[serde(
        default,
        deserialize_with = "crate::projectiles::persistence::deserialize_optional_state"
    )]
    pub projectile: Option<crate::projectiles::ProjectileState>,
    pub tick: u64,
}

/// Native living state shared by full birth images and compact timer updates.
/// Nonplayer protection deadlines are durable; player joins/respawns rearm
/// protection separately. Attack cooldowns, hurt windows, horizontal impulses
/// and combat credit intentionally remain restart-local.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EntityStateImage {
    /// Original named NBT compound; typed checkpoint fields override these keys.
    pub opaque_entity_fields: Vec<u8>,
    /// Lifetime age at this image's journal tick; not an age-only dirty key.
    pub age: u32,
    pub protected_until: u64,
    pub persistent: bool,
    /// Checkpoint-durable idle/despawn counter; never a WAL dirty key by itself.
    pub no_action_time: u32,
    pub air_supply: i32,
    pub burn_ticks: u32,
    pub love: u32,
    pub breed_cooldown: u32,
    pub entity_life: crate::entity_life::EntityLifeState,
    pub loot: crate::entity_loot::EntityLootState,
    pub controls: crate::entity_controls::Controls,
    pub falling: Option<crate::falling::FallingState>,
    pub tnt: Option<crate::tnt::PrimedTntState>,
    #[serde(deserialize_with = "crate::special_mobs::persistence::deserialize")]
    pub special_mob: Option<crate::special_mobs::Retained>,
    #[serde(deserialize_with = "crate::bees::deserialize")]
    pub bee: Option<crate::bees::BeeState>,
    #[serde(deserialize_with = "crate::allay::deserialize")]
    pub allay: Option<crate::allay::AllayState>,
}

impl Default for EntityStateImage {
    fn default() -> Self {
        Self {
            age: 0,
            opaque_entity_fields: Vec::new(),
            protected_until: 0,
            persistent: false,
            no_action_time: 0,
            air_supply: 300,
            burn_ticks: 0,
            love: 0,
            breed_cooldown: 0,
            entity_life: Default::default(),
            loot: Default::default(),
            controls: Default::default(),
            falling: None,
            tnt: None,
            special_mob: None,
            bee: None,
            allay: None,
        }
    }
}

impl EntityStateImage {
    pub(crate) fn from_row(body: &crate::entities::Body, row: &crate::entities::ColdRow) -> Self {
        Self {
            age: row.age,
            opaque_entity_fields: row.opaque_entity_fields.clone(),
            protected_until: row.protected_until,
            persistent: row.persistent,
            no_action_time: row.no_action_time,
            air_supply: body.air_supply,
            burn_ticks: body.burn_ticks,
            love: row.love,
            breed_cooldown: row.breed_cooldown,
            entity_life: row.entity_life.clone(),
            loot: row.loot.clone(),
            controls: row.controls,
            falling: row.falling,
            tnt: row.tnt,
            special_mob: row.special_mob.as_deref().cloned(),
            bee: row.bee.as_deref().cloned(),
            allay: row.allay.as_deref().cloned(),
        }
    }

    /// Deterministic lifecycle clocks are derived from batch ticks. Feeding,
    /// age-lock changes, egg resets and all other events still differ from the
    /// projection and retain an absolute image; random intervals are not replayed.
    pub(crate) fn advance_clocks(&mut self, type_id: u32, baby: bool, elapsed: u64) {
        let elapsed = elapsed.min(u64::from(u32::MAX)) as u32;
        self.breed_cooldown = self.breed_cooldown.saturating_sub(elapsed);
        self.love = self.love.saturating_sub(elapsed);
        if self.entity_life.child_ticks > 0 && !self.entity_life.age_lock.locked {
            self.entity_life.child_ticks = self.entity_life.child_ticks.saturating_sub(elapsed);
            self.love = 0;
        }
        if !self.entity_life.age_lock.locked
            && mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .is_some_and(|contract| contract.name == "minecraft:tadpole")
        {
            self.entity_life.age_lock.tadpole_age = self
                .entity_life
                .age_lock
                .tadpole_age
                .saturating_add(elapsed);
        }
        if self.entity_life.harvest_ticks != 0
            && self.entity_life.child_ticks == 0
            && !baby
            && mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .is_some_and(|contract| contract.name == "minecraft:chicken")
        {
            self.entity_life.harvest_ticks = self.entity_life.harvest_ticks.saturating_sub(elapsed);
        }
    }

    /// Whether [`Self::advance_clocks`] can change this image over time.
    /// Live lifecycle clocks advance the same projection without dirty marks;
    /// stopped (dead) rows need rechecks while their image remains sensitive.
    /// Keep in step with `advance_clocks`.
    pub(crate) fn clock_sensitive(&self, type_id: u32, baby: bool) -> bool {
        let life = &self.entity_life;
        let named = |name: &str| {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .is_some_and(|contract| contract.name == name)
        };
        self.breed_cooldown > 0
            || self.love > 0
            || (life.child_ticks > 0 && !life.age_lock.locked)
            || (!life.age_lock.locked && named("minecraft:tadpole"))
            || (life.harvest_ticks != 0
                && life.child_ticks == 0
                && !baby
                && named("minecraft:chicken"))
    }

    pub(crate) fn matches(
        &self,
        body: &crate::entities::Body,
        row: &crate::entities::ColdRow,
    ) -> bool {
        self.protected_until == row.protected_until
            && self.persistent == row.persistent
            && self.air_supply == body.air_supply
            && self.burn_ticks == body.burn_ticks
            && self.love == row.love
            && self.breed_cooldown == row.breed_cooldown
            && self.entity_life == row.entity_life
            && self.loot == row.loot
            && self.controls.journal_eq(&row.controls)
            && self.falling == row.falling
            && self.tnt == row.tnt
            && self.special_mob.as_ref() == row.special_mob.as_deref()
            && self.bee.as_ref() == row.bee.as_deref()
            && self.allay.as_ref() == row.allay.as_deref()
            && self.opaque_entity_fields == row.opaque_entity_fields
    }

    pub(crate) fn from_record(
        registry: &crate::registry::CoreRegistry,
        record: &mc_data::entity_nbt::VanillaEntity,
    ) -> Result<Self, CoreError> {
        if record.id == "minecraft:armor_stand" {
            crate::armor_stand::validate_fields(&record.extra_fields)?;
        }
        let ageable = registry
            .entity_type_id(&record.id)
            .and_then(|id| registry.blueprint(id))
            .is_some_and(|blueprint| {
                blueprint.capabilities.ageable
                    || blueprint
                        .goals
                        .iter()
                        .any(|goal| goal.kind == crate::blueprint::GoalKind::Breed)
            });
        let (entity_life, love) = crate::entity_life::from_record(record, ageable);
        let mut image = Self {
            age: if ageable { 0 } else { record.age.max(0) as u32 },
            entity_life,
            love,
            breed_cooldown: record
                .breed_cooldown
                .max(if ageable { record.age.max(0) } else { 0 })
                as u32,
            loot: crate::entity_loot::from_fields(&record.extra_fields)?,
            falling: crate::falling::from_record(record, registry).map_err(CoreError::Storage)?,
            tnt: crate::tnt::from_record(registry, record)?,
            special_mob: crate::special_mobs::persistence::from_record(record)?,
            bee: crate::bees::from_record(record)?,
            allay: crate::allay::from_record(&registry.items, record)?,
            ..Default::default()
        };
        crate::mounts::persistence::from_fields(&mut image.entity_life, &record.extra_fields)?;
        crate::mounts::persistence::cargo_from_record(
            &mut image.entity_life.mount,
            &registry.items,
            record,
        )?;
        image.entity_life.age_lock = crate::golden_dandelion::from_fields(&record.extra_fields)?;
        if record.id == "minecraft:tadpole" {
            image.entity_life.age_lock.tadpole_age = u32::try_from(record.age).map_err(|_| {
                CoreError::Storage("tadpole Age: expected nonnegative TAG_Int".into())
            })?;
        }
        image.entity_life.portal = crate::portals::from_fields(&record.extra_fields)?;
        image.entity_life.vehicle = crate::vehicles::from_record(registry, record)?.map(Box::new);
        image.entity_life.dolphin = crate::dolphin::from_record(record)?;
        crate::entity_loot::extra_equipment_from_fields(
            &registry.items,
            &record.extra_fields,
            &mut image.loot,
        )?;
        for (key, tag) in &record.extra_fields {
            match (key.as_str(), tag) {
                ("CustomNameVisible", mc_nbt::Tag::Byte(_)) => {}
                ("CustomNameVisible", _) => {
                    return Err(CoreError::Storage(
                        "CustomNameVisible: expected byte".into(),
                    ));
                }
                ("solaris:LifetimeAge", mc_nbt::Tag::Long(value)) => {
                    image.age = u32::try_from(*value).map_err(|_| {
                        CoreError::Storage("solaris:LifetimeAge: expected u32".into())
                    })?;
                }
                ("solaris:LifetimeAge", _) => {
                    return Err(CoreError::Storage(
                        "solaris:LifetimeAge: expected long".into(),
                    ));
                }
                ("solaris:ProtectedUntil", mc_nbt::Tag::Long(value)) => {
                    image.protected_until = *value as u64;
                }
                ("solaris:ProtectedUntil", _) => {
                    return Err(CoreError::Storage(
                        "solaris:ProtectedUntil: expected long".into(),
                    ));
                }
                ("PersistenceRequired", mc_nbt::Tag::Byte(value)) => image.persistent = *value != 0,
                ("solaris:NoActionTime", mc_nbt::Tag::Long(value)) => {
                    image.no_action_time = u32::try_from(*value)
                        .map_err(|_| CoreError::Storage("invalid entity idle timer".into()))?;
                }
                ("Air", mc_nbt::Tag::Short(value)) => image.air_supply = i32::from(*value),
                ("Air", mc_nbt::Tag::Int(value)) => image.air_supply = *value,
                ("Fire", mc_nbt::Tag::Short(value)) => image.burn_ticks = (*value).max(0) as u32,
                _ => {}
            }
        }
        image.controls = crate::entity_controls::Controls::from_fields(&record.extra_fields)?;
        // Keep only fields outside runtime authority. In particular optional
        // typed fields must not be resurrected after their resident value clears.
        let mut owned = image.to_fields(&registry.blocks);
        if let Some(allay) = &image.allay {
            owned.extend(crate::allay::to_fields(allay, &registry.items));
        }
        if let Some(vehicle) = &image.entity_life.vehicle {
            owned.extend(crate::vehicles::to_fields(vehicle, &registry.items));
        }
        let projectile_kind = crate::projectiles::ProjectileKind::from_name(&record.id);
        let villager = matches!(
            record.id.as_str(),
            "minecraft:villager" | "minecraft:zombie_villager" | "minecraft:wandering_trader"
        );
        let retained: Vec<_> = record
            .extra_fields
            .iter()
            .filter(|(key, _)| {
                !(owned.iter().any(|(typed, _)| typed == key)
                    || projectile_kind
                        .is_some_and(|kind| crate::projectiles::persistence::owns_field(kind, key))
                    || villager && crate::villager::persistence::owns_field(key)
                    || key == "IsBaby" && crate::entity_life::uses_is_baby(&record.id)
                    || image.entity_life.vehicle.is_some() && key == "Items"
                    || crate::mounts::persistence::owns_cargo_field(&record.id, key)
                    || matches!(
                        key.as_str(),
                        "CustomName"
                            | "CustomNameVisible"
                            | "equipment"
                            | "RaidId"
                            | "Wave"
                            | "CanJoinRaid"
                            | "DragonPhase"
                            | "DragonDeathTime"
                            | "ShowBottom"
                            | "beam_target"
                    ))
            })
            .cloned()
            .collect();
        if !retained.is_empty() {
            mc_nbt::write_named(
                &mut image.opaque_entity_fields,
                "",
                &mc_nbt::Tag::Compound(retained),
            )
            .map_err(|err| CoreError::Storage(format!("entity opaque fields: {err}")))?;
        }
        Ok(image)
    }

    pub(crate) fn to_fields(&self, blocks: &mc_world::BlockRegistry) -> Vec<(String, mc_nbt::Tag)> {
        let mut fields = crate::entity_life::to_fields(&self.entity_life, self.love);
        fields.extend(crate::entity_loot::to_fields(&self.loot));
        fields.extend(self.controls.to_fields());
        if let Some(dolphin) = &self.entity_life.dolphin {
            fields.extend(crate::dolphin::to_fields(dolphin));
        }
        if let Some(value) = &self.special_mob {
            fields.extend(crate::special_mobs::persistence::to_fields(value));
        }
        if let Some(bee) = &self.bee {
            fields.extend(crate::bees::to_fields(bee));
        }
        if let Some(falling) = &self.falling {
            fields.extend(crate::falling::to_fields(falling, blocks));
        }
        if let Some(tnt) = &self.tnt {
            fields.extend(crate::tnt::to_fields(tnt, blocks));
        }
        fields.extend([
            (
                "solaris:LifetimeAge".into(),
                mc_nbt::Tag::Long(i64::from(self.age)),
            ),
            (
                "solaris:ProtectedUntil".into(),
                mc_nbt::Tag::Long(self.protected_until as i64),
            ),
            (
                "Fire".into(),
                mc_nbt::Tag::Short(self.burn_ticks.min(i16::MAX as u32) as i16),
            ),
            (
                "PersistenceRequired".into(),
                mc_nbt::Tag::Byte(i8::from(self.persistent)),
            ),
            (
                "solaris:NoActionTime".into(),
                mc_nbt::Tag::Long(i64::from(self.no_action_time)),
            ),
            (
                "Air".into(),
                mc_nbt::Tag::Short(self.air_supply.clamp(i16::MIN as i32, i16::MAX as i32) as i16),
            ),
        ]);
        fields
    }
}

/// Fold batches into a [`Replay`]. Later batches overwrite earlier ones
/// (absolute post-images converge).
#[must_use]
/// Fold journal batches over a checkpoint base (one fold function).
/// The base holds the checkpoint's absolute post-images (entities with
/// motion, block entities, players); each batch entry overwrites or merges
/// over it, so tail removals delete base records, tail vitals update base
/// health, and intra-chunk entity images keep base motion. A default [`Replay`] is
/// an empty base (fresh world).
pub fn replay_batches(replay: Replay, batches: &[(u64, u64, Vec<JournalEntry>)]) -> Replay {
    let mut replay = apply_batch_images(replay, batches);
    advance_replay_clocks(&mut replay);
    replay
}

/// Fold absolute images without scanning unrelated retained entities per batch.
pub(crate) fn apply_batch_images(
    mut replay: Replay,
    batches: &[(u64, u64, Vec<JournalEntry>)],
) -> Replay {
    if replay.simulation_clocks.is_empty() {
        for (&chunk, image) in &replay.sim_debt {
            replay
                .simulation_clocks
                .insert(chunk, ReplayChunkClock::new(replay.tick, image.active));
        }
    }
    for (lo, hi, entries) in batches {
        replay.tick = replay.tick.max(*hi);
        let _ = lo;
        // Debt is emitted after entity images. Fold its boundaries first so
        // even a compact vitals anchor sees this batch's complete activity.
        for entry in entries {
            if let JournalEntry::SimDebt { image } = entry {
                if replay
                    .sim_debt
                    .get(&image.chunk)
                    .is_some_and(|old| image.tick <= old.tick)
                {
                    continue;
                }
                replay
                    .simulation_clocks
                    .entry(image.chunk)
                    .or_insert_with(|| ReplayChunkClock::new(0, false))
                    .observe(*image);
                replay.sim_debt.insert(image.chunk, *image);
            }
        }
        for entry in entries {
            match entry {
                JournalEntry::Plugins { value } => {
                    replay.plugins.apply_delta(value);
                }
                JournalEntry::EndFight { value } => {
                    replay.end_fight = Some(value.clone());
                }
                JournalEntry::Dimension { dimension, entries } => {
                    if *dimension == crate::dimensions::Dimension::Overworld {
                        replay = apply_batch_images(replay, &[(*lo, *hi, entries.clone())]);
                    } else {
                        let base = replay
                            .dimensions
                            .remove(dimension)
                            .map(|base| *base)
                            .unwrap_or_default();
                        replay.dimensions.insert(
                            *dimension,
                            Box::new(apply_batch_images(base, &[(*lo, *hi, entries.clone())])),
                        );
                    }
                }
                JournalEntry::GameRules { values } => {
                    replay.gamerules.clone_from(values);
                }
                JournalEntry::BlockTicks {
                    image,
                    next_sequence,
                } => {
                    replay.block_ticks.apply(image.clone(), *next_sequence);
                }
                JournalEntry::FluidTicks { image } => {
                    replay.fluid_ticks = image.clone();
                }
                JournalEntry::InhabitedTime { image } => {
                    replay.inhabited_time.insert(image.chunk, *image);
                }
                JournalEntry::SimDebt { .. } => {}
                JournalEntry::GeneratedEntityClaim { claim } => {
                    replay.generated_entity_claims.insert(claim.clone());
                }
                JournalEntry::Climate { value } => {
                    replay.climate = Some(*value);
                }
                JournalEntry::Border { value } => {
                    replay.border = *value;
                }
                JournalEntry::Difficulty { value } => {
                    replay.difficulty = *value;
                }
                JournalEntry::Scoreboard { value } => {
                    replay.scoreboard = Some(value.clone());
                }
                JournalEntry::CommandStorage { value } => {
                    replay.command_storage = Some(value.clone());
                }
                JournalEntry::SavedMap { id, value } => {
                    replay.saved_maps.insert(*id, Arc::clone(value));
                }
                JournalEntry::Bossbars { value } => {
                    replay.bossbars = Some(value.clone());
                }
                JournalEntry::Raids { value } => {
                    replay.raids = Some(value.clone());
                }
                JournalEntry::WanderingTrader { value } => {
                    replay.wandering_trader = Some(*value);
                }
                JournalEntry::ScheduledFunctions { value } => {
                    replay.scheduled_functions.clone_from(value);
                }
                JournalEntry::DatapackSelection { value } => {
                    replay.datapack_selection = Some(value.clone());
                }
                JournalEntry::Block { pos, state } => {
                    replay.blocks.insert((pos[0], pos[1], pos[2]), *state);
                }
                JournalEntry::BlockLog { value } => {
                    replay.block_log.push((**value).clone());
                }
                JournalEntry::Statistics {
                    uuid,
                    changes,
                    timers,
                    tick,
                } => {
                    if let Some(player) = replay.players.get_mut(uuid) {
                        player.statistics.apply_changes(changes);
                        if let Some(timers) = timers {
                            player.statistics.apply_timers(*timers);
                        }
                        player.statistics.advance_tick(*tick);
                        player.tick = *tick;
                    }
                }
                JournalEntry::Player {
                    uuid,
                    opaque_player_fields,
                    statistics,
                    advancement_progress,
                    unlocked_recipes,
                    dimension,
                    pos,
                    yaw,
                    pitch,
                    health,
                    food,
                    saturation,
                    exhaustion,
                    air,
                    experience_level,
                    experience_progress,
                    experience_total,
                    game_mode,
                    selected,
                    inventory,
                    carried,
                    grid,
                    respawn,
                    respawn_dimension,
                    respawn_forced,
                    respawn_yaw,
                    respawn_pitch,
                    active_effects,
                    enchantment_seed,
                    enchantment_offer_seed,
                    warden_warning,
                    current_impulse_impact_pos,
                    fall_flying,
                    burn_ticks,
                    portal,
                    tick,
                } => {
                    let mut statistics = statistics.as_ref().clone();
                    statistics.advance_tick(*tick);
                    let previous = replay.players.get(uuid);
                    let advancement_progress = advancement_progress
                        .clone()
                        .or_else(|| previous.map(|player| Arc::clone(&player.advancement_progress)))
                        .unwrap_or_else(empty_advancement_progress);
                    let unlocked_recipes = unlocked_recipes
                        .clone()
                        .or_else(|| previous.map(|player| Arc::clone(&player.unlocked_recipes)))
                        .unwrap_or_default();
                    replay.players.insert(
                        uuid.clone(),
                        ReplayPlayer {
                            opaque_player_fields: opaque_player_fields.clone(),
                            statistics,
                            advancement_progress,
                            unlocked_recipes,
                            dimension: *dimension,
                            pos: *pos,
                            yaw: *yaw,
                            pitch: *pitch,
                            health: *health,
                            food: *food,
                            saturation: *saturation,
                            exhaustion: *exhaustion,
                            air: *air,
                            experience_level: *experience_level,
                            experience_progress: *experience_progress,
                            experience_total: *experience_total,
                            game_mode: *game_mode,
                            selected: *selected,
                            inventory: inventory.clone(),
                            carried: carried.clone(),
                            grid: grid.clone(),
                            respawn: *respawn,
                            respawn_dimension: *respawn_dimension,
                            respawn_forced: *respawn_forced,
                            respawn_yaw: *respawn_yaw,
                            respawn_pitch: *respawn_pitch,
                            active_effects: active_effects.clone(),
                            enchantment_seed: *enchantment_seed,
                            enchantment_offer_seed: *enchantment_offer_seed,
                            warden_warning: *warden_warning,
                            current_impulse_impact_pos: *current_impulse_impact_pos,
                            fall_flying: *fall_flying,
                            burn_ticks: *burn_ticks,
                            portal: *portal,
                            tick: *tick,
                        },
                    );
                }
                JournalEntry::BlockEntity {
                    pos,
                    slots,
                    furnace,
                    tick,
                } => {
                    replay.block_entities.insert(
                        (pos[0], pos[1], pos[2]),
                        ReplayBlockEntity {
                            slots: slots.clone(),
                            furnace: furnace.clone(),
                            tick: *tick,
                        },
                    );
                }
                JournalEntry::RemoveBlockEntity { pos, .. } => {
                    replay.block_entities.remove(&(pos[0], pos[1], pos[2]));
                    replay
                        .opaque_block_entities
                        .insert((pos[0], pos[1], pos[2]), None);
                }
                JournalEntry::OpaqueBlockEntity { pos, nbt } => {
                    replay
                        .opaque_block_entities
                        .insert((pos[0], pos[1], pos[2]), nbt.clone());
                }
                JournalEntry::Entity {
                    id,
                    uuid,
                    type_id,
                    health,
                    alive,
                    age,
                    breed_cooldown,
                    pickup_delay,
                    item,
                    appearance,
                    active_effects,
                    villager,
                    state,
                    projectile,
                    tick,
                    pos: spawn_pos,
                    vel: flight_vel,
                    yaw: spawn_yaw,
                    pitch: spawn_pitch,
                } => {
                    if replay
                        .entities
                        .get(id)
                        .is_some_and(|record| *tick < record.tick)
                    {
                        continue;
                    }
                    if *alive {
                        // Ordinary intra-chunk motion stays checkpoint-only;
                        // chunk crossings and retained body motion advance together.
                        let base = replay.entities.get(id);
                        let (pos, vel, yaw, pitch, ground) = if projectile.is_some()
                            || state.tnt.is_some()
                            || state.falling.is_some()
                            || state.entity_life.vehicle.is_some()
                            || crate::end_fight::dragon_type(*type_id)
                            || crate::piglin::name(*type_id) == "minecraft:enderman"
                            || base.is_some_and(|record| {
                                replay_entity_chunk(record.pos) != replay_entity_chunk(*spawn_pos)
                            }) {
                            (
                                *spawn_pos,
                                *flight_vel,
                                *spawn_yaw,
                                *spawn_pitch,
                                projectile.as_ref().is_some_and(|record| record.stuck)
                                    || state.tnt.is_some_and(|record| record.on_ground)
                                    || state
                                        .entity_life
                                        .vehicle
                                        .as_ref()
                                        .is_some_and(|v| v.on_ground),
                            )
                        } else {
                            match base {
                                Some(record) => (
                                    record.pos,
                                    record.vel,
                                    record.yaw,
                                    record.pitch,
                                    record.ground,
                                ),
                                None => (*spawn_pos, [0.0; 3], *spawn_yaw, *spawn_pitch, false),
                            }
                        };
                        replay.entities.insert(
                            *id,
                            ReplayEntity {
                                uuid: uuid.clone(),
                                type_id: *type_id,
                                pos,
                                vel,
                                yaw,
                                pitch,
                                ground,
                                health: *health,
                                alive: *alive,
                                age: *age,
                                breed_cooldown: *breed_cooldown,
                                pickup_delay: *pickup_delay,
                                item: item.clone(),
                                appearance: appearance.clone(),
                                active_effects: active_effects.clone(),
                                villager: villager.as_deref().cloned(),
                                state: EntityStateImage {
                                    age: *age,
                                    ..state.as_ref().clone()
                                },
                                projectile: projectile.as_deref().cloned(),
                                tick: *tick,
                            },
                        );
                    } else {
                        replay.entities.remove(id);
                    }
                }
                JournalEntry::EntityState { id, state, tick } => {
                    if let Some(record) = replay.entities.get_mut(id) {
                        if *tick < record.tick {
                            continue;
                        }
                        if let Some(villager) = &mut record.villager
                            && let Some(clock) = replay
                                .simulation_clocks
                                .get(&replay_entity_chunk(record.pos))
                        {
                            clock.advance_villager(villager, record.tick, *tick);
                        }
                        record.age = state.age;
                        record.state = state.as_ref().clone();
                        record.breed_cooldown = state.breed_cooldown;
                        record.tick = *tick;
                    }
                }
                JournalEntry::EntityVitals {
                    id,
                    health,
                    alive,
                    tick,
                } => {
                    if let Some(record) = replay.entities.get_mut(id) {
                        if *tick < record.tick {
                            continue;
                        }
                        if *alive {
                            let clock = replay
                                .simulation_clocks
                                .get(&replay_entity_chunk(record.pos));
                            let elapsed =
                                clock.map_or(0, |clock| clock.between(record.tick, *tick));
                            record.state.advance_clocks(
                                record.type_id,
                                record.appearance.baby,
                                elapsed,
                            );
                            if let Some(villager) = &mut record.villager
                                && let Some(clock) = clock
                            {
                                clock.advance_villager(villager, record.tick, *tick);
                            }
                            record.breed_cooldown = record.state.breed_cooldown;
                            record.age = record.age.wrapping_add(elapsed as u32);
                            record.state.age = record.age;
                            record.health = *health;
                            record.appearance.health = *health;
                            record.tick = *tick;
                        } else {
                            replay.entities.remove(id);
                        }
                    }
                }
                JournalEntry::RemoveEntity { id, tick } => {
                    if replay
                        .entities
                        .get(id)
                        .is_none_or(|record| *tick >= record.tick)
                    {
                        replay.entities.remove(id);
                    }
                }
                JournalEntry::Ticket {
                    owner,
                    chunk,
                    tick,
                    radius,
                } => {
                    replay
                        .tickets
                        .insert(owner.clone(), (*chunk, (*tick, *radius)));
                }
                JournalEntry::TicketRemoved { owner, .. } => {
                    replay.tickets.remove(owner);
                }
                JournalEntry::Settlement {
                    image, high_water, ..
                } => {
                    replay.settlement_high_water = replay
                        .settlement_high_water
                        .max(*high_water)
                        .max(image.id.0);
                    match &image.value {
                        Some(record) => {
                            replay.settlements.insert(image.id.0, record.clone());
                            replay.settlement_removed.remove(&image.id.0);
                        }
                        None => {
                            replay.settlements.remove(&image.id.0);
                            replay.settlement_removed.insert(image.id.0);
                        }
                    }
                }
            }
        }
    }
    replay
}

/// Project each retained anchor once after the complete streamed tail is folded.
pub(crate) fn advance_replay_clocks(replay: &mut Replay) {
    for record in replay.entities.values_mut() {
        if record.tick < replay.tick {
            let clock = replay
                .simulation_clocks
                .get(&replay_entity_chunk(record.pos));
            let elapsed = clock.map_or(0, |clock| clock.between(record.tick, replay.tick));
            record
                .state
                .advance_clocks(record.type_id, record.appearance.baby, elapsed);
            if let Some(villager) = &mut record.villager
                && let Some(clock) = clock
            {
                clock.advance_villager(villager, record.tick, replay.tick);
            }
            record.breed_cooldown = record.state.breed_cooldown;
            record.age = record.age.wrapping_add(elapsed as u32);
            record.tick = replay.tick;
        }
        record.state.age = record.age;
    }
    for player in replay.players.values_mut() {
        player.warden_warning.advance_tick(replay.tick);
    }
    // 001 slice0: active debt intervals advance through the recovered WAL
    // tick (never `remaining`, never wall time); inactive images freeze.
    for image in replay.sim_debt.values_mut() {
        image.last_sim = image.last_sim_at(replay.tick);
        image.tick = replay.tick;
    }
    for clock in replay.simulation_clocks.values_mut() {
        let active = clock.points.last().is_some_and(|point| point.active);
        clock.points.clear();
        clock.points.push(ReplayClockPoint {
            tick: replay.tick,
            elapsed: 0,
            active,
        });
    }
    for dimension in replay.dimensions.values_mut() {
        advance_replay_clocks(dimension);
    }
}
