//! Authoritative villager state; all gameplay mutations occur on the tick owner.
use crate::boundary::BlockPos;
use mc_data::ItemStack;
use uuid::Uuid;

pub(crate) mod brain;
pub(crate) mod building;
#[cfg(test)]
mod building_autonomy_tests;
#[cfg(test)]
mod building_housing_army_tests;
#[cfg(test)]
mod building_tests;
#[cfg(test)]
mod building_worksite_tests;
pub(crate) mod conversion;
#[cfg(test)]
mod conversion_tests;
pub(crate) mod defense;
#[cfg(test)]
mod defense_tests;
pub(crate) mod doors;
pub(crate) mod generated;
pub(crate) mod harvest;
#[cfg(test)]
mod harvest_tests;
pub(crate) mod lumber;
#[cfg(test)]
mod lumber_tests;
pub(crate) mod mining;
#[cfg(test)]
mod mining_tests;
pub(crate) mod shepherd;
#[cfg(test)]
mod shepherd_tests;
pub(crate) mod soldier;
pub(crate) use generated::generated_uuid;
#[cfg(test)]
mod generated_tests;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
#[path = "nameplate_tests.rs"]
mod nameplate_tests;
pub(crate) mod nameplates;
pub(crate) mod persistence;
pub mod trades;

/// Refresh the vanilla 26.1 projection only when it actually changes.
pub(crate) fn sync_appearance(
    state: &mut crate::tick::TickState,
    slot: crate::entities::EntitySlot,
) {
    let row = state.entities.row(slot);
    let Some(villager) = row.villager.as_ref() else {
        return;
    };
    if villager.wandering {
        return;
    }
    let baby = villager.age < 0;
    let zombie = villager.conversion_time.is_some();
    let pose = if villager.sleeping { 2 } else { 0 };
    // Living bed position and subclass ids come from the local 26.1.2
    // entity-sync tests / javap oracle, not a guessed protocol layout.
    let projected = [
        Some((
            14,
            crate::boundary::EntityDataValue::OptionalBlockPos(
                villager
                    .home
                    .filter(|_| villager.sleeping)
                    .map(|pos| [pos.x, pos.y, pos.z]),
            ),
        )),
        Some((
            if zombie { 20 } else { 19 },
            crate::boundary::EntityDataValue::VillagerData {
                kind: villager.data.kind as i32,
                profession: villager.data.profession.profession_id(),
                level: i32::from(villager.data.level),
            },
        )),
        zombie.then_some((
            19,
            crate::boundary::EntityDataValue::Boolean(
                villager
                    .conversion_time
                    .is_some_and(|remaining| remaining >= 0),
            ),
        )),
        zombie.then_some((
            21,
            crate::boundary::EntityDataValue::Boolean(villager.data_finalized),
        )),
    ];
    let same = row.appearance.baby == baby
        && row.appearance.pose == pose
        && projected
            .iter()
            .flatten()
            .all(|value| row.appearance.extra.contains(value))
        && row
            .appearance
            .extra
            .iter()
            .filter(|(index, _)| matches!(index, 14 | 19 | 20 | 21))
            .count()
            == if zombie { 4 } else { 2 };
    let (half_width, height) = if villager.sleeping {
        (0.1, 0.2)
    } else if baby && zombie {
        (0.245, 0.99)
    } else if baby {
        (0.15, 0.975)
    } else {
        (0.3, 1.95)
    };
    let speed = if zombie {
        Some(if baby { 6.45 } else { 4.3 })
    } else {
        None
    };
    if same {
        let body = state.entities.body(slot);
        if body.half_width == half_width
            && body.height == height
            && speed.is_none_or(|speed| speed == row.speed)
        {
            return;
        }
        state.entities.set_half_width(slot, half_width);
        let (body, row) = state.entities.parts_mut(slot);
        body.height = height;
        if let Some(speed) = speed {
            row.speed = speed;
        }
        return;
    }
    let mut appearance = (*row.appearance).clone();
    appearance.baby = baby;
    appearance.pose = pose;
    appearance
        .extra
        .retain(|(index, _)| !matches!(index, 14 | 19 | 20 | 21));
    appearance.extra.extend(projected.into_iter().flatten());
    state.entities.set_half_width(slot, half_width);
    let (body, row) = state.entities.parts_mut(slot);
    body.height = height;
    if let Some(speed) = speed {
        row.speed = speed;
    }
    if appearance != *row.appearance {
        let id = body.id;
        state.entities.row_mut(slot).appearance = std::sync::Arc::new(appearance);
        state.appearance_changed.push(id);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum VillagerKind {
    Desert,
    Jungle,
    #[default]
    Plains,
    Savanna,
    Snow,
    Swamp,
    Taiga,
}
impl VillagerKind {
    pub const ALL: [Self; 7] = [
        Self::Desert,
        Self::Jungle,
        Self::Plains,
        Self::Savanna,
        Self::Snow,
        Self::Swamp,
        Self::Taiga,
    ];
    pub fn name(self) -> &'static str {
        [
            "desert", "jungle", "plains", "savanna", "snow", "swamp", "taiga",
        ][self as usize]
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|v| v.name() == name.strip_prefix("minecraft:").unwrap_or(name))
    }
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[repr(u8)]
pub enum VillagerProfession {
    #[default]
    None,
    Armorer,
    Butcher,
    Cartographer,
    Cleric,
    Farmer,
    Fisherman,
    Fletcher,
    Leatherworker,
    Librarian,
    Mason,
    Nitwit,
    Shepherd,
    Toolsmith,
    Weaponsmith,
    Miner,
    Lumberjack,
    Builder,
    Soldier,
}
impl VillagerProfession {
    pub const ALL: [Self; 19] = [
        Self::None,
        Self::Armorer,
        Self::Butcher,
        Self::Cartographer,
        Self::Cleric,
        Self::Farmer,
        Self::Fisherman,
        Self::Fletcher,
        Self::Leatherworker,
        Self::Librarian,
        Self::Mason,
        Self::Nitwit,
        Self::Shepherd,
        Self::Toolsmith,
        Self::Weaponsmith,
        Self::Miner,
        Self::Lumberjack,
        Self::Builder,
        Self::Soldier,
    ];
    pub fn name(self) -> &'static str {
        [
            "none",
            "armorer",
            "butcher",
            "cartographer",
            "cleric",
            "farmer",
            "fisherman",
            "fletcher",
            "leatherworker",
            "librarian",
            "mason",
            "nitwit",
            "shepherd",
            "toolsmith",
            "weaponsmith",
            "miner",
            "lumberjack",
            "builder",
            "soldier",
        ][self as usize]
    }
    /// Vanilla projection shared by merchant titles and Anvil profession ids.
    pub fn title_name(self) -> &'static str {
        match self {
            Self::Miner => Self::Toolsmith.name(),
            Self::Lumberjack => Self::Fletcher.name(),
            Self::Builder => Self::Mason.name(),
            Self::Soldier => Self::Armorer.name(),
            _ => self.name(),
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(path) = name.strip_prefix("solaris:") {
            return match path {
                "miner" => Some(Self::Miner),
                "lumberjack" => Some(Self::Lumberjack),
                "builder" => Some(Self::Builder),
                "soldier" => Some(Self::Soldier),
                _ => None,
            };
        }
        if let Some(path) = name.strip_prefix("minecraft:") {
            if matches!(path, "miner" | "lumberjack" | "builder" | "soldier") {
                return None;
            }
            return Self::ALL.into_iter().find(|v| v.name() == path);
        }
        Self::ALL.into_iter().find(|v| v.name() == name)
    }
    pub const fn profession_id(self) -> i32 {
        match self {
            Self::Miner => Self::Toolsmith as i32,
            Self::Lumberjack => Self::Fletcher as i32,
            Self::Builder => Self::Mason as i32,
            Self::Soldier => Self::Armorer as i32,
            _ => self as i32,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VillagerData {
    pub kind: VillagerKind,
    pub profession: VillagerProfession,
    pub level: u8,
}
impl Default for VillagerData {
    fn default() -> Self {
        Self {
            kind: VillagerKind::Plains,
            profession: VillagerProfession::None,
            level: 1,
        }
    }
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VillagerTradeOffer {
    pub cost_a: ItemStack,
    pub cost_b: Option<ItemStack>,
    pub result: ItemStack,
    pub uses: i32,
    pub max_uses: i32,
    pub reward_exp: bool,
    pub xp: i32,
    pub special_price: i32,
    pub demand: i32,
    pub price_multiplier: f32,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gossip {
    pub target: Uuid,
    pub kind: String,
    pub value: i32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Activity {
    #[default]
    Idle,
    Work,
    Meet,
    Rest,
    Play,
    Panic,
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VillagerState {
    /// Native settlement membership; no parallel registry roster is persisted.
    pub settlement: Option<crate::settlements::SettlementId>,
    /// Social membership; households do not retain a second resident roster.
    pub household: Option<String>,
    pub birth_origin: Option<mc_script::ScriptNativeBirthOrigin>,
    /// Stable monotonic birth key `birth_tick * 65536 + same-tick ordinal`;
    /// None for spawned/converted villagers. Survives removals and restarts.
    pub birth_sequence: Option<u64>,
    pub courtship_started_tick: Option<u64>,
    pub wandering: bool,
    pub despawn_at: Option<u64>,
    pub data: VillagerData,
    pub data_finalized: bool,
    pub xp: i32,
    pub offers: Vec<VillagerTradeOffer>,
    pub gossips: Vec<Gossip>,
    #[serde(with = "persistence::block_pos_option")]
    pub job_site: Option<BlockPos>,
    #[serde(with = "persistence::block_pos_option")]
    pub home: Option<BlockPos>,
    #[serde(with = "persistence::block_pos_option")]
    pub meeting_point: Option<BlockPos>,
    pub job_site_dimension: String,
    pub home_dimension: String,
    pub meeting_point_dimension: String,
    pub inventory: [Option<ItemStack>; 8],
    /// Durable extraction cursors share the entity's inventory/WAL after-image.
    pub(crate) mining_progress: mining::Progress,
    pub(crate) lumber_progress: lumber::Progress,
    pub(crate) soldier: Option<crate::settlements::army::Soldier>,
    pub food_level: i32,
    /// Vanilla Age: negative baby growth, positive breeding cooldown.
    pub age: i32,
    pub last_restock: i64,
    pub last_restock_day: i64,
    pub restocks_today: i32,
    pub last_work: i64,
    pub last_slept: Option<u64>,
    pub golem_detected_until: Option<u64>,
    pub last_gossip: u64,
    pub activity: Activity,
    pub sleeping: bool,
    /// Presentation-only stair seat; never retained across recovery.
    #[serde(skip)]
    pub(crate) seat: Option<BlockPos>,
    pub courtship_until: Option<u64>,
    #[serde(with = "persistence::entity_id_option")]
    pub partner: Option<crate::boundary::EntityId>,
    /// Transient navigation and reactive brain memories, owned by the tick.
    #[serde(skip)]
    pub(crate) navigation_path: Vec<BlockPos>,
    #[serde(skip)]
    pub(crate) navigation_target: Option<BlockPos>,
    #[serde(skip)]
    pub(crate) navigation_chunks: Vec<(crate::boundary::ChunkPos, u64)>,
    #[serde(skip)]
    pub(crate) motion_geometry: brain::MotionGeometryCache,
    #[serde(skip)]
    pub(crate) navigation_repath_at: u64,
    #[serde(skip)]
    pub(crate) navigation_can_open_doors: bool,
    #[serde(skip)]
    pub(crate) navigation_progress_len: usize,
    #[serde(skip)]
    pub(crate) navigation_progress_next: Option<BlockPos>,
    #[serde(skip)]
    pub(crate) navigation_progress_dist_sq: f64,
    /// Wooden doors opened by this brain, closed again after passage.
    #[serde(skip)]
    pub(crate) opened_doors: Vec<BlockPos>,
    /// Earliest tick of this farmer's next harvest attempt.
    #[serde(skip)]
    pub(crate) harvest_at: u64,
    /// Resumable certified-farm sweep and selected crop, owned by the tick.
    #[serde(skip)]
    pub(crate) harvest_progress: harvest::Progress,
    #[serde(skip)]
    pub(crate) poi_validation: crate::settlements::allocator::PoiValidation,
    #[serde(skip)]
    pub(crate) trade_work: Option<trades::WorkState>,
    pub(crate) panic_until: u64,
    /// Whether the resident brain clock ran at the last durable image. A
    /// loaded/unloaded or NoAI boundary must not turn age into a dirty key.
    #[serde(default)]
    pub(crate) brain_clock: bool,
    pub(crate) last_gossip_decay: u64,
    /// Present only while this entity is a zombie villager; -1 means not curing.
    pub conversion_time: Option<i32>,
    pub conversion_player: Option<Uuid>,
}
impl Default for VillagerState {
    fn default() -> Self {
        Self {
            settlement: None,
            household: None,
            birth_origin: None,
            birth_sequence: None,
            courtship_started_tick: None,
            wandering: false,
            despawn_at: None,
            data: VillagerData::default(),
            data_finalized: false,
            xp: 0,
            offers: Vec::new(),
            gossips: Vec::new(),
            job_site: None,
            home: None,
            meeting_point: None,
            job_site_dimension: "minecraft:overworld".into(),
            home_dimension: "minecraft:overworld".into(),
            meeting_point_dimension: "minecraft:overworld".into(),
            inventory: std::array::from_fn(|_| None),
            mining_progress: mining::Progress::default(),
            lumber_progress: lumber::Progress::default(),
            soldier: None,
            food_level: 0,
            age: 0,
            last_restock: 0,
            last_restock_day: i64::MIN,
            restocks_today: 0,
            last_work: 0,
            last_slept: None,
            golem_detected_until: None,
            last_gossip: 0,
            activity: Activity::Idle,
            sleeping: false,
            seat: None,
            courtship_until: None,
            partner: None,
            navigation_path: Vec::new(),
            navigation_target: None,
            navigation_chunks: Vec::new(),
            motion_geometry: brain::MotionGeometryCache::default(),
            navigation_repath_at: 0,
            navigation_can_open_doors: false,
            navigation_progress_len: 0,
            navigation_progress_next: None,
            navigation_progress_dist_sq: 0.0,
            opened_doors: Vec::new(),
            harvest_at: 0,
            harvest_progress: harvest::Progress::default(),
            poi_validation: crate::settlements::allocator::PoiValidation::default(),
            trade_work: None,
            panic_until: 0,
            brain_clock: true,
            last_gossip_decay: 0,
            conversion_time: None,
            conversion_player: None,
        }
    }
}
