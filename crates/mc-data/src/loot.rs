use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

use crate::{Identifier, read_json_file, visit_json_files};

const BUILTIN_SURVIVAL_LOOT: &str = include_str!("../data/survival_loot.json");

#[derive(Debug, Error)]
pub enum LootError {
    #[error("loot file {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid loot identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("filesystem error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct LootTables {
    entity_drops: BTreeMap<Identifier, Vec<LootDrop>>,
    block_drops: BTreeMap<Identifier, Vec<LootDrop>>,
    block_loot: BTreeMap<Identifier, BlockLoot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootDrop {
    pub item: Identifier,
    pub count: LootCount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LootCount {
    Fixed(u32),
    UniformInclusive { min: u32, max: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockLoot {
    silk_touch_drops: Vec<BlockLootDrop>,
    regular_drops: Vec<BlockLootDrop>,
    conditional_pools: Vec<BlockStateLootPool>,
}

impl BlockLoot {
    #[must_use]
    pub fn drops_for_tool(&self, silk_touch: bool) -> &[BlockLootDrop] {
        if silk_touch && !self.silk_touch_drops.is_empty() {
            &self.silk_touch_drops
        } else {
            &self.regular_drops
        }
    }

    #[must_use]
    pub fn drops_for_context(
        &self,
        silk_touch: bool,
        block: &Identifier,
        properties: &[(String, String)],
    ) -> Vec<&BlockLootDrop> {
        let mut drops: Vec<_> = self.drops_for_tool(silk_touch).iter().collect();
        for pool in &self.conditional_pools {
            if !pool.predicate.matches(block, properties) {
                continue;
            }
            if let Some(entry) = pool
                .entries
                .iter()
                .find(|entry| entry.predicate.matches(block, properties))
            {
                drops.push(&entry.drop);
            }
        }
        drops
    }
}

#[derive(Debug, Clone, PartialEq)]
struct BlockStateLootPool {
    predicate: BlockStatePredicate,
    entries: Vec<BlockStateLootEntry>,
}

#[derive(Debug, Clone, PartialEq)]
struct BlockStateLootEntry {
    predicate: BlockStatePredicate,
    drop: BlockLootDrop,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BlockStatePredicate {
    block: Option<Identifier>,
    properties: BTreeMap<String, String>,
}

impl BlockStatePredicate {
    fn matches(&self, block: &Identifier, properties: &[(String, String)]) -> bool {
        self.block.as_ref().is_none_or(|expected| expected == block)
            && self.properties.iter().all(|(key, value)| {
                properties
                    .iter()
                    .any(|(actual_key, actual_value)| actual_key == key && actual_value == value)
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockLootDrop {
    pub drop: LootDrop,
    pub fortune_bonus: Option<FortuneBonus>,
    random_chance: Option<f32>,
    survives_explosion: bool,
    explosion_decay: bool,
}

impl BlockLootDrop {
    #[must_use]
    pub fn plain(drop: LootDrop) -> Self {
        Self {
            drop,
            fortune_bonus: None,
            random_chance: None,
            survives_explosion: false,
            explosion_decay: false,
        }
    }

    #[must_use]
    pub fn sample_count(&self, count_roll: u64, fortune_level: u32, bonus_roll: u64) -> u32 {
        let baseline = self.drop.count.sample(count_roll);
        self.fortune_bonus.map_or(baseline, |bonus| {
            bonus.apply(baseline, fortune_level, bonus_roll)
        })
    }

    #[must_use]
    pub fn passes_random_chance(&self, roll: u64) -> bool {
        self.random_chance.is_none_or(|chance| {
            let sample = (roll >> 40) as f32 / 16_777_216.0;
            sample < chance
        })
    }

    fn sample_simple_explosion_count(
        &self,
        explosion_radius: Option<f32>,
        next_float: &mut impl FnMut() -> f32,
    ) -> Option<u32> {
        let count = self.simple_explosion_input_count(explosion_radius.is_some())?;
        let Some(explosion_radius) = explosion_radius else {
            return Some(count);
        };
        if !explosion_radius.is_finite() || explosion_radius <= 0.0 {
            return None;
        }

        let probability = 1.0 / explosion_radius;
        if self.survives_explosion && next_float() > probability {
            return Some(0);
        }
        if self.explosion_decay {
            return Some((0..count).filter(|_| next_float() <= probability).count() as u32);
        }
        Some(count)
    }

    fn simple_explosion_input_count(&self, require_explosion_modifier: bool) -> Option<u32> {
        if self.fortune_bonus.is_some()
            || self.random_chance.is_some()
            || (require_explosion_modifier && !self.survives_explosion && !self.explosion_decay)
        {
            return None;
        }
        match self.drop.count {
            LootCount::Fixed(count) => Some(count),
            LootCount::UniformInclusive { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FortuneBonus {
    OreDrops,
    UniformBonusCount { bonus_multiplier: u32 },
    BinomialWithBonusCount { extra_rounds: u32, probability: f32 },
}

impl FortuneBonus {
    #[must_use]
    pub fn apply(self, count: u32, fortune_level: u32, roll: u64) -> u32 {
        match self {
            Self::OreDrops => {
                if fortune_level == 0 {
                    return count;
                }
                let width = u64::from(fortune_level) + 2;
                let multiplier = (roll % width).saturating_sub(1) + 1;
                count.saturating_mul(u32::try_from(multiplier).unwrap_or(u32::MAX))
            }
            Self::UniformBonusCount { bonus_multiplier } => {
                if fortune_level == 0 {
                    return count;
                }
                let max_bonus = bonus_multiplier.saturating_mul(fortune_level);
                let bonus = roll % (u64::from(max_bonus) + 1);
                count.saturating_add(u32::try_from(bonus).unwrap_or(u32::MAX))
            }
            Self::BinomialWithBonusCount {
                extra_rounds,
                probability,
            } => {
                let rounds = fortune_level.saturating_add(extra_rounds);
                let mut count = count;
                let mut roll = roll;
                for _ in 0..rounds {
                    let sample = (roll >> 40) as f32 / 16_777_216.0;
                    if sample < probability {
                        count = count.saturating_add(1);
                    }
                    roll = splitmix64(roll);
                }
                count
            }
        }
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

impl LootCount {
    #[must_use]
    pub fn sample(self, roll: u64) -> u32 {
        match self {
            Self::Fixed(count) => count,
            Self::UniformInclusive { min, max } => {
                let width = u64::from(max.saturating_sub(min)) + 1;
                min + u32::try_from(roll % width).expect("loot count range fits u32")
            }
        }
    }
}

impl LootDrop {
    #[must_use]
    pub fn single(item: Identifier) -> Self {
        Self {
            item,
            count: LootCount::Fixed(1),
        }
    }

    #[must_use]
    pub fn fixed(item: Identifier, count: u32) -> Self {
        Self {
            item,
            count: LootCount::Fixed(count),
        }
    }

    #[must_use]
    pub fn uniform(item: Identifier, min: u32, max: u32) -> Self {
        debug_assert!(min <= max);
        Self {
            item,
            count: LootCount::UniformInclusive { min, max },
        }
    }
}

impl LootTables {
    #[must_use]
    pub fn from_maps(
        entity_drops: BTreeMap<Identifier, Identifier>,
        block_drops: BTreeMap<Identifier, Identifier>,
    ) -> Self {
        Self::from_drop_maps(
            entity_drops
                .into_iter()
                .map(|(source, item)| (source, LootDrop::single(item)))
                .collect(),
            block_drops
                .into_iter()
                .map(|(source, item)| (source, LootDrop::single(item)))
                .collect(),
        )
    }

    #[must_use]
    pub fn from_drop_maps(
        entity_drops: BTreeMap<Identifier, LootDrop>,
        block_drops: BTreeMap<Identifier, LootDrop>,
    ) -> Self {
        Self::from_drop_lists(
            entity_drops
                .into_iter()
                .map(|(source, drop)| (source, vec![drop]))
                .collect(),
            block_drops
                .into_iter()
                .map(|(source, drop)| (source, vec![drop]))
                .collect(),
        )
    }

    #[must_use]
    pub fn from_drop_lists(
        entity_drops: BTreeMap<Identifier, Vec<LootDrop>>,
        block_drops: BTreeMap<Identifier, Vec<LootDrop>>,
    ) -> Self {
        Self {
            entity_drops,
            block_drops,
            block_loot: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn entity_drop(&self, entity: &Identifier) -> Option<&Identifier> {
        self.entity_drop_stack(entity).map(|drop| &drop.item)
    }

    #[must_use]
    pub fn entity_drop_stack(&self, entity: &Identifier) -> Option<&LootDrop> {
        self.entity_drop_stacks(entity)?.first()
    }

    #[must_use]
    pub fn entity_drop_stacks(&self, entity: &Identifier) -> Option<&[LootDrop]> {
        self.entity_drops.get(entity).map(Vec::as_slice)
    }

    #[must_use]
    pub fn block_drop(&self, block: &Identifier) -> Option<&Identifier> {
        self.block_drop_stack(block).map(|drop| &drop.item)
    }

    #[must_use]
    pub fn block_drop_stack(&self, block: &Identifier) -> Option<&LootDrop> {
        self.block_drop_stacks(block)?.first()
    }

    #[must_use]
    pub fn block_drop_stacks(&self, block: &Identifier) -> Option<&[LootDrop]> {
        self.block_drops.get(block).map(Vec::as_slice)
    }

    #[must_use]
    pub fn block_loot(&self, block: &Identifier) -> Option<&BlockLoot> {
        self.block_loot.get(block)
    }

    /// Samples the exact simple block-explosion rules used by common fixed-count tables.
    /// `None` models `DESTROY`, where vanilla omits the explosion radius and explosion
    /// modifiers are identities. `Some(radius)` models `DESTROY_WITH_DECAY`. Complex
    /// counts, Fortune, and random-chance entries fail closed.
    pub fn block_explosion_drops(
        &self,
        block: &Identifier,
        properties: &[(String, String)],
        explosion_radius: Option<f32>,
        mut next_float: impl FnMut() -> f32,
    ) -> Option<Vec<LootDrop>> {
        let common_fallbacks;
        let drops = if let Some(rule) = self.block_loot(block) {
            rule.drops_for_context(false, block, properties)
        } else {
            let fallback_drops = self
                .block_drop_stacks(block)
                .map(<[LootDrop]>::to_vec)
                .or_else(|| {
                    (block.as_str() == "minecraft:dirt")
                        .then(|| vec![LootDrop::single(block.clone())])
                })?;
            common_fallbacks = fallback_drops
                .into_iter()
                .map(|drop| BlockLootDrop {
                    drop,
                    fortune_bonus: None,
                    random_chance: None,
                    survives_explosion: true,
                    explosion_decay: false,
                })
                .collect::<Vec<_>>();
            common_fallbacks.iter().collect()
        };

        if drops.iter().any(|drop| {
            drop.simple_explosion_input_count(explosion_radius.is_some())
                .is_none()
        }) {
            return None;
        }

        let mut sampled = Vec::new();
        for drop in drops {
            let count = drop.sample_simple_explosion_count(explosion_radius, &mut next_float)?;
            if count > 0 {
                sampled.push(LootDrop {
                    item: drop.drop.item.clone(),
                    count: LootCount::Fixed(count),
                });
            }
        }
        Some(sampled)
    }

    #[must_use]
    pub fn total_drops(&self) -> usize {
        self.entity_drops.values().map(Vec::len).sum::<usize>()
            + self.block_drops.values().map(Vec::len).sum::<usize>()
    }

    pub fn fill_missing_from(&mut self, fallback: &Self) {
        for (source, drop) in &fallback.entity_drops {
            self.entity_drops
                .entry(source.clone())
                .or_insert_with(|| drop.clone());
        }
        for (source, drops) in &fallback.block_drops {
            if !self.block_drops.contains_key(source) {
                self.block_drops.insert(source.clone(), drops.clone());
                if let Some(rule) = fallback.block_loot.get(source) {
                    self.block_loot.insert(source.clone(), rule.clone());
                }
            }
        }
    }

    pub fn fill_missing_entity_items_from(&mut self, fallback: &Self) {
        for (source, fallback_drops) in &fallback.entity_drops {
            let drops = self.entity_drops.entry(source.clone()).or_default();
            for fallback_drop in fallback_drops {
                if drops.iter().all(|drop| drop.item != fallback_drop.item) {
                    drops.push(fallback_drop.clone());
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct RawLootTables {
    #[serde(default)]
    entities: BTreeMap<String, RawDropList>,
    #[serde(default)]
    blocks: BTreeMap<String, RawDropList>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawDropList {
    Single(String),
    Multiple(Vec<String>),
}

impl RawDropList {
    fn into_items(self) -> Vec<String> {
        match self {
            Self::Single(item) => vec![item],
            Self::Multiple(items) => items,
        }
    }
}

#[must_use]
pub fn builtin() -> &'static LootTables {
    static BUILTIN: OnceLock<LootTables> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        from_str(
            BUILTIN_SURVIVAL_LOOT,
            Path::new("crates/mc-data/data/survival_loot.json"),
        )
        .expect("built-in Solaris survival loot JSON is valid")
    })
}

pub fn load(path: impl AsRef<Path>) -> Result<LootTables, LootError> {
    let path = path.as_ref();
    let bytes = std::fs::read_to_string(path).map_err(|source| LootError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    from_str(&bytes, path)
}

pub fn load_vanilla_subset(root: impl AsRef<Path>) -> Result<LootTables, LootError> {
    let root = root.as_ref();
    if !root.is_dir() {
        return Ok(LootTables::default());
    }

    let blocks = load_vanilla_blocks(&root.join("blocks"))?;
    let entity_drops = load_vanilla_kind(&root.join("entities"), true)?;
    Ok(LootTables {
        entity_drops,
        block_drops: blocks.drops,
        block_loot: blocks.rules,
    })
}

struct LoadedBlockLoot {
    drops: BTreeMap<Identifier, Vec<LootDrop>>,
    rules: BTreeMap<Identifier, BlockLoot>,
}

fn load_vanilla_blocks(dir: &Path) -> Result<LoadedBlockLoot, LootError> {
    if !dir.is_dir() {
        return Ok(LoadedBlockLoot {
            drops: BTreeMap::new(),
            rules: BTreeMap::new(),
        });
    }

    let mut paths = Vec::new();
    visit_json_files(
        dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| LootError::Io { path, source },
    )?;
    paths.sort();

    let mut drops = BTreeMap::new();
    let mut rules = BTreeMap::new();
    for path in paths {
        let source = id_from_file(dir, &path)?;
        let value: serde_json::Value = read_json_file(
            &path,
            &|path, source| LootError::Io { path, source },
            &|path, source| LootError::Malformed { path, source },
        )?;
        if let Some(table_drops) = simple_drops_from_table(&path, &value, true)? {
            drops.insert(source.clone(), table_drops);
        }
        if let Some(rule) = block_loot_from_table(&path, &value)? {
            rules.insert(source, rule);
        }
    }
    Ok(LoadedBlockLoot { drops, rules })
}

fn load_vanilla_kind(
    dir: &Path,
    allow_uniform_counts: bool,
) -> Result<BTreeMap<Identifier, Vec<LootDrop>>, LootError> {
    if !dir.is_dir() {
        return Ok(BTreeMap::new());
    }

    let mut paths = Vec::new();
    visit_json_files(
        dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| LootError::Io { path, source },
    )?;
    paths.sort();

    let mut drops = BTreeMap::new();
    for path in paths {
        let source = id_from_file(dir, &path)?;
        let value: serde_json::Value = read_json_file(
            &path,
            &|path, source| LootError::Io { path, source },
            &|path, source| LootError::Malformed { path, source },
        )?;
        if let Some(table_drops) = simple_drops_from_table(&path, &value, allow_uniform_counts)? {
            drops.insert(source, table_drops);
        }
    }
    Ok(drops)
}

fn id_from_file(root: &Path, path: &Path) -> Result<Identifier, LootError> {
    let rel = path
        .strip_prefix(root)
        .expect("walk yields paths under loot root")
        .with_extension("");
    let mut joined = String::from("minecraft:");
    for component in rel.components() {
        if !joined.ends_with(':') {
            joined.push('/');
        }
        joined.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    parse_id(path, joined)
}

fn simple_drops_from_table(
    path: &Path,
    value: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Result<Option<Vec<LootDrop>>, LootError> {
    let Some(pools) = value.get("pools").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    if pools.iter().any(has_unsupported_pool_rolls) {
        return Ok(None);
    }

    let mut drops = Vec::new();
    for pool in pools {
        if has_unsupported_conditions(pool) {
            continue;
        }
        let Some(pool_count) = supported_count_from_functions(pool, allow_uniform_counts) else {
            continue;
        };
        let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for entry in entries {
            if let Some(mut drop) = simple_drop_from_entry(path, entry, allow_uniform_counts)? {
                if let Some(count) = pool_count {
                    drop.count = count;
                }
                drops.push(drop);
                break;
            }
        }
    }
    Ok((!drops.is_empty()).then_some(drops))
}

fn simple_drop_from_entry(
    path: &Path,
    entry: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Result<Option<LootDrop>, LootError> {
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            if has_unsupported_conditions(entry) {
                return Ok(None);
            }
            let Some(count) = supported_count_from_functions(entry, allow_uniform_counts) else {
                return Ok(None);
            };
            let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
                return Ok(None);
            };
            parse_id(path, name.to_string()).map(|item| {
                Some(LootDrop {
                    item,
                    count: count.unwrap_or(LootCount::Fixed(1)),
                })
            })
        }
        Some("minecraft:alternatives") => {
            if has_unsupported_conditions(entry) || entry.get("features").is_some() {
                return Ok(None);
            }
            let Some(children) = entry.get("children").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            for child in children {
                if let Some(drop) = simple_drop_from_entry(path, child, allow_uniform_counts)? {
                    return Ok(Some(drop));
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn block_loot_from_table(
    path: &Path,
    value: &serde_json::Value,
) -> Result<Option<BlockLoot>, LootError> {
    let Some(pools) = value.get("pools").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    if pools.iter().any(has_unsupported_pool_rolls) {
        return Ok(None);
    }

    let mut silk_touch_drops = Vec::new();
    let mut regular_drops = Vec::new();
    let mut conditional_pools = Vec::new();
    for pool in pools {
        if contains_block_state_condition(pool) {
            if let Some(pool) = block_state_loot_pool(path, pool)? {
                conditional_pools.push(pool);
            }
            continue;
        }
        if has_unsupported_conditions(pool) {
            continue;
        }
        let Some(pool_count) = supported_count_from_functions(pool, true) else {
            continue;
        };
        let pool_survives_explosion = has_survives_explosion_condition(pool);
        let pool_explosion_decay = has_explosion_decay_function(pool);
        let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for entry in entries {
            let Some((mut silk, mut regular)) = block_loot_from_entry(path, entry)? else {
                continue;
            };
            if let Some(count) = pool_count {
                if let Some(drop) = silk.as_mut() {
                    drop.drop.count = count;
                }
                regular.drop.count = count;
            }
            if let Some(drop) = silk.as_mut() {
                drop.survives_explosion |= pool_survives_explosion;
                drop.explosion_decay |= pool_explosion_decay;
            }
            regular.survives_explosion |= pool_survives_explosion;
            regular.explosion_decay |= pool_explosion_decay;
            if let Some(drop) = silk {
                silk_touch_drops.push(drop);
            }
            regular_drops.push(regular);
            break;
        }
    }

    Ok(
        (!regular_drops.is_empty() || !conditional_pools.is_empty()).then_some(BlockLoot {
            silk_touch_drops,
            regular_drops,
            conditional_pools,
        }),
    )
}

fn contains_block_state_condition(value: &serde_json::Value) -> bool {
    value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition
                    .get("condition")
                    .and_then(serde_json::Value::as_str)
                    == Some("minecraft:block_state_property")
            })
        })
        || ["entries", "children"].into_iter().any(|field| {
            value
                .get(field)
                .and_then(serde_json::Value::as_array)
                .is_some_and(|values| values.iter().any(contains_block_state_condition))
        })
}

fn block_state_loot_pool(
    path: &Path,
    pool: &serde_json::Value,
) -> Result<Option<BlockStateLootPool>, LootError> {
    let Some(predicate) = block_state_predicate(path, pool, false)? else {
        return Ok(None);
    };
    let Some(pool_count) = supported_count_from_functions(pool, true) else {
        return Ok(None);
    };
    let Some(entries) = pool.get("entries").and_then(serde_json::Value::as_array) else {
        return Ok(None);
    };
    let Some(entry) = entries.first() else {
        return Ok(None);
    };
    let mut parsed = Vec::new();
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            if let Some(entry) = block_state_loot_entry(path, entry, pool_count)? {
                parsed.push(entry);
            }
        }
        Some("minecraft:alternatives") if entry.get("features").is_none() => {
            let Some(children) = entry.get("children").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            for child in children {
                let Some(entry) = block_state_loot_entry(path, child, pool_count)? else {
                    return Ok(None);
                };
                parsed.push(entry);
            }
        }
        _ => return Ok(None),
    }
    for entry in &mut parsed {
        entry.drop.survives_explosion |= has_survives_explosion_condition(pool);
        entry.drop.explosion_decay |= has_explosion_decay_function(pool);
    }
    Ok((!parsed.is_empty()).then_some(BlockStateLootPool {
        predicate,
        entries: parsed,
    }))
}

fn block_state_loot_entry(
    path: &Path,
    entry: &serde_json::Value,
    pool_count: Option<LootCount>,
) -> Result<Option<BlockStateLootEntry>, LootError> {
    if entry.get("type").and_then(serde_json::Value::as_str) != Some("minecraft:item") {
        return Ok(None);
    }
    let Some(predicate) = block_state_predicate(path, entry, true)? else {
        return Ok(None);
    };
    let Some(random_chance) = random_chance_condition(entry) else {
        return Ok(None);
    };
    let mut unconditional = entry.clone();
    unconditional
        .as_object_mut()
        .expect("loot entry is an object")
        .remove("conditions");
    let Some(mut drop) = contextual_regular_drop(path, &unconditional)? else {
        return Ok(None);
    };
    if let Some(count) = pool_count {
        drop.drop.count = count;
    }
    drop.random_chance = random_chance;
    drop.survives_explosion |= has_survives_explosion_condition(entry);
    drop.explosion_decay |= has_explosion_decay_function(entry);
    Ok(Some(BlockStateLootEntry { predicate, drop }))
}

fn random_chance_condition(value: &serde_json::Value) -> Option<Option<f32>> {
    let Some(conditions) = value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
    else {
        return Some(None);
    };
    let mut chance = None;
    for condition in conditions {
        if condition
            .get("condition")
            .and_then(serde_json::Value::as_str)
            != Some("minecraft:random_chance")
        {
            continue;
        }
        if chance.is_some()
            || condition
                .as_object()?
                .keys()
                .any(|key| !matches!(key.as_str(), "condition" | "chance"))
        {
            return None;
        }
        let value = condition.get("chance")?.as_f64()?;
        if !(0.0..=1.0).contains(&value) {
            return None;
        }
        chance = Some(value as f32);
    }
    Some(chance)
}

fn block_state_predicate(
    path: &Path,
    value: &serde_json::Value,
    allow_random_chance: bool,
) -> Result<Option<BlockStatePredicate>, LootError> {
    let Some(conditions) = value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(Some(BlockStatePredicate::default()));
    };
    let mut predicate = BlockStatePredicate::default();
    for condition in conditions {
        match condition
            .get("condition")
            .and_then(serde_json::Value::as_str)
        {
            Some("minecraft:survives_explosion") => {}
            Some("minecraft:random_chance") if allow_random_chance => {}
            Some("minecraft:block_state_property") => {
                let Some(block) = condition.get("block").and_then(serde_json::Value::as_str) else {
                    return Ok(None);
                };
                let block = parse_id(path, block.to_string())?;
                if predicate
                    .block
                    .as_ref()
                    .is_some_and(|expected| expected != &block)
                {
                    return Ok(None);
                }
                predicate.block = Some(block);
                let Some(properties) = condition
                    .get("properties")
                    .and_then(serde_json::Value::as_object)
                else {
                    return Ok(None);
                };
                for (key, value) in properties {
                    let Some(value) = value.as_str() else {
                        return Ok(None);
                    };
                    if predicate
                        .properties
                        .insert(key.clone(), value.to_string())
                        .is_some()
                    {
                        return Ok(None);
                    }
                }
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(predicate))
}

fn block_loot_from_entry(
    path: &Path,
    entry: &serde_json::Value,
) -> Result<Option<(Option<BlockLootDrop>, BlockLootDrop)>, LootError> {
    match entry.get("type").and_then(serde_json::Value::as_str) {
        Some("minecraft:item") => {
            Ok(contextual_regular_drop(path, entry)?.map(|drop| (None, drop)))
        }
        Some("minecraft:alternatives") => {
            if has_unsupported_conditions(entry) || entry.get("features").is_some() {
                return Ok(None);
            }
            let Some(children) = entry.get("children").and_then(serde_json::Value::as_array) else {
                return Ok(None);
            };
            let mut silk_touch_drop = None;
            let mut regular_drop = None;
            for child in children {
                if silk_touch_drop.is_none() {
                    silk_touch_drop = silk_touch_drop_from_entry(path, child)?;
                    if silk_touch_drop.is_some() {
                        continue;
                    }
                }
                if regular_drop.is_none() {
                    regular_drop = contextual_regular_drop(path, child)?;
                }
            }
            Ok(regular_drop.map(|regular| (silk_touch_drop, regular)))
        }
        _ => Ok(None),
    }
}

fn silk_touch_drop_from_entry(
    path: &Path,
    entry: &serde_json::Value,
) -> Result<Option<BlockLootDrop>, LootError> {
    if !has_silk_touch_condition(entry) {
        return Ok(None);
    }
    let mut unconditional = entry.clone();
    unconditional
        .as_object_mut()
        .expect("loot entry with conditions is an object")
        .remove("conditions");
    contextual_regular_drop(path, &unconditional)
}

fn contextual_regular_drop(
    path: &Path,
    entry: &serde_json::Value,
) -> Result<Option<BlockLootDrop>, LootError> {
    let Some(drop) = simple_drop_from_entry(path, entry, true)? else {
        return Ok(None);
    };
    let Some(fortune_bonus) = fortune_bonus_from_functions(entry) else {
        return Ok(None);
    };
    Ok(Some(BlockLootDrop {
        drop,
        fortune_bonus,
        random_chance: None,
        survives_explosion: has_survives_explosion_condition(entry),
        explosion_decay: has_explosion_decay_function(entry),
    }))
}

fn has_survives_explosion_condition(value: &serde_json::Value) -> bool {
    value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition
                    .get("condition")
                    .and_then(serde_json::Value::as_str)
                    == Some("minecraft:survives_explosion")
            })
        })
}

fn has_explosion_decay_function(value: &serde_json::Value) -> bool {
    value
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|functions| {
            functions.iter().any(|function| {
                function.get("function").and_then(serde_json::Value::as_str)
                    == Some("minecraft:explosion_decay")
            })
        })
}

fn has_silk_touch_condition(entry: &serde_json::Value) -> bool {
    let Some(conditions) = entry
        .get("conditions")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    if conditions.len() != 1 {
        return false;
    }
    let condition = &conditions[0];
    if condition
        .get("condition")
        .and_then(serde_json::Value::as_str)
        != Some("minecraft:match_tool")
    {
        return false;
    }
    condition
        .pointer("/predicate/predicates/minecraft:enchantments")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|enchantments| {
            enchantments.iter().any(|enchantment| {
                enchantment
                    .get("enchantments")
                    .and_then(serde_json::Value::as_str)
                    == Some("minecraft:silk_touch")
                    && enchantment
                        .pointer("/levels/min")
                        .and_then(supported_count_bound)
                        .is_some_and(|min| min >= 1)
            })
        })
}

fn fortune_bonus_from_functions(value: &serde_json::Value) -> Option<Option<FortuneBonus>> {
    let Some(functions) = value.get("functions").and_then(serde_json::Value::as_array) else {
        return Some(None);
    };
    let mut bonus = None;
    for function in functions {
        if function.get("function").and_then(serde_json::Value::as_str)
            != Some("minecraft:apply_bonus")
        {
            continue;
        }
        if function
            .get("enchantment")
            .and_then(serde_json::Value::as_str)
            != Some("minecraft:fortune")
            || bonus.is_some()
        {
            return None;
        }
        bonus = match function.get("formula").and_then(serde_json::Value::as_str) {
            Some("minecraft:ore_drops") if function.get("parameters").is_none() => {
                Some(FortuneBonus::OreDrops)
            }
            Some("minecraft:uniform_bonus_count") => {
                let parameters = function.get("parameters")?.as_object()?;
                if parameters.keys().any(|key| key != "bonusMultiplier") {
                    return None;
                }
                let bonus_multiplier = supported_count_bound(parameters.get("bonusMultiplier")?)?;
                Some(FortuneBonus::UniformBonusCount { bonus_multiplier })
            }
            Some("minecraft:binomial_with_bonus_count") => {
                let parameters = function.get("parameters")?.as_object()?;
                if parameters
                    .keys()
                    .any(|key| !matches!(key.as_str(), "extra" | "probability"))
                {
                    return None;
                }
                let extra_rounds = supported_count_bound(parameters.get("extra")?)?;
                let probability = parameters.get("probability")?.as_f64()?;
                if !(0.0..=1.0).contains(&probability) {
                    return None;
                }
                Some(FortuneBonus::BinomialWithBonusCount {
                    extra_rounds,
                    probability: probability as f32,
                })
            }
            _ => return None,
        };
    }
    Some(bonus)
}

fn has_unsupported_pool_rolls(pool: &serde_json::Value) -> bool {
    !is_supported_constant_roll(pool.get("rolls"), 1)
        || !is_supported_constant_roll(pool.get("bonus_rolls"), 0)
}

fn is_supported_constant_roll(value: Option<&serde_json::Value>, supported: u32) -> bool {
    let Some(value) = value else {
        return true;
    };
    if let Some(value) = value.as_u64() {
        return value == u64::from(supported);
    }
    value
        .as_f64()
        .is_some_and(|value| value == f64::from(supported))
}

fn has_unsupported_conditions(value: &serde_json::Value) -> bool {
    value
        .get("conditions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                !matches!(
                    condition
                        .get("condition")
                        .and_then(serde_json::Value::as_str),
                    Some("minecraft:survives_explosion")
                )
            })
        })
}

fn supported_count_from_functions(
    value: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Option<Option<LootCount>> {
    let Some(functions) = value.get("functions").and_then(serde_json::Value::as_array) else {
        return Some(None);
    };
    let mut count = None;
    for function in functions {
        let fields = function.as_object()?;
        match function.get("function").and_then(serde_json::Value::as_str) {
            Some("minecraft:set_count") => {
                if fields
                    .keys()
                    .any(|key| !matches!(key.as_str(), "function" | "count" | "add"))
                    || function
                        .get("add")
                        .is_some_and(|add| add.as_bool() != Some(false))
                {
                    return None;
                }
                count = Some(supported_count_provider(
                    function.get("count")?,
                    allow_uniform_counts,
                )?);
            }
            // With no looting enchantment these functions do not change the count.
            Some("minecraft:enchanted_count_increase") => {}
            // Solaris currently evaluates baseline raw drops. A conditional smelt
            // therefore leaves the item unchanged when no burning context is supplied.
            Some("minecraft:furnace_smelt") if function.get("conditions").is_some() => {}
            // The baseline block-loot context has Fortune level zero and no explosion.
            // Both functions are therefore identity operations.
            Some("minecraft:apply_bonus")
                if supported_zero_fortune_bonus_function(fields, function) => {}
            Some("minecraft:explosion_decay") if fields.len() == 1 => {}
            _ => return None,
        }
    }
    Some(count)
}

fn supported_zero_fortune_bonus_function(
    fields: &serde_json::Map<String, serde_json::Value>,
    function: &serde_json::Value,
) -> bool {
    if fields.keys().any(|key| {
        !matches!(
            key.as_str(),
            "function" | "enchantment" | "formula" | "parameters"
        )
    }) || function
        .get("enchantment")
        .and_then(serde_json::Value::as_str)
        != Some("minecraft:fortune")
    {
        return false;
    }
    let formula = function.get("formula").and_then(serde_json::Value::as_str);
    if !matches!(
        formula,
        Some(
            "minecraft:ore_drops"
                | "minecraft:uniform_bonus_count"
                | "minecraft:binomial_with_bonus_count"
        )
    ) {
        return false;
    }
    function
        .get("parameters")
        .is_none_or(serde_json::Value::is_object)
}

fn supported_count_provider(
    value: &serde_json::Value,
    allow_uniform_counts: bool,
) -> Option<LootCount> {
    if let Some(count) = supported_count_bound(value) {
        return (count <= 64).then_some(LootCount::Fixed(count));
    }

    if !allow_uniform_counts {
        return None;
    }
    let fields = value.as_object()?;
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "type" | "min" | "max"))
        || value.get("type").and_then(serde_json::Value::as_str) != Some("minecraft:uniform")
    {
        return None;
    }
    let min = supported_count_bound(value.get("min")?)?;
    let max = supported_count_bound(value.get("max")?)?;
    (min <= max && max <= 64).then_some(LootCount::UniformInclusive { min, max })
}

fn supported_count_bound(value: &serde_json::Value) -> Option<u32> {
    if let Some(value) = value.as_u64() {
        return u32::try_from(value).ok();
    }
    let value = value.as_f64()?;
    (value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX))
        .then_some(value as u32)
}

fn from_str(raw: &str, path: &Path) -> Result<LootTables, LootError> {
    let raw: RawLootTables = serde_json::from_str(raw).map_err(|source| LootError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(LootTables {
        entity_drops: parse_map(path, raw.entities)?,
        block_drops: parse_map(path, raw.blocks)?,
        block_loot: BTreeMap::new(),
    })
}

fn parse_map(
    path: &Path,
    raw: BTreeMap<String, RawDropList>,
) -> Result<BTreeMap<Identifier, Vec<LootDrop>>, LootError> {
    raw.into_iter()
        .map(|(source, drops)| {
            let source_id = parse_id(path, source)?;
            let drops = drops
                .into_items()
                .into_iter()
                .map(|drop| parse_id(path, drop).map(LootDrop::single))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((source_id, drops))
        })
        .collect()
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, LootError> {
    Identifier::parse(value.clone()).map_err(|_| LootError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn builtin_survival_loot_loads_from_repo_json() {
        let loot = builtin();

        assert_eq!(
            loot.entity_drop_stacks(&Identifier::parse("minecraft:cow").unwrap()),
            Some(
                [
                    LootDrop::single(Identifier::parse("minecraft:leather").unwrap()),
                    LootDrop::single(Identifier::parse("minecraft:beef").unwrap()),
                ]
                .as_slice()
            )
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:podzol").unwrap()),
            Some(&Identifier::parse("minecraft:dirt").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:short_grass").unwrap()),
            Some(&Identifier::parse("minecraft:wheat_seeds").unwrap())
        );
        assert_eq!(
            loot.entity_drop_stack(&Identifier::parse("minecraft:sheep").unwrap()),
            Some(&LootDrop::single(
                Identifier::parse("minecraft:white_wool").unwrap()
            ))
        );
        assert_eq!(
            loot.entity_drop_stacks(&Identifier::parse("minecraft:sheep").unwrap()),
            Some(
                [
                    LootDrop::single(Identifier::parse("minecraft:white_wool").unwrap()),
                    LootDrop::single(Identifier::parse("minecraft:mutton").unwrap()),
                ]
                .as_slice()
            )
        );
        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:skeleton").unwrap()),
            Some(&Identifier::parse("minecraft:bone").unwrap())
        );
        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:spider").unwrap()),
            Some(&Identifier::parse("minecraft:string").unwrap())
        );
    }

    #[test]
    fn builtin_destroy_explosion_uses_all_repo_owned_block_drops() {
        let loot = builtin();
        for (block, expected) in [
            ("minecraft:oak_log", "minecraft:oak_log"),
            ("minecraft:diamond_ore", "minecraft:diamond"),
        ] {
            let drops = loot
                .block_explosion_drops(&Identifier::parse(block).unwrap(), &[], None, || {
                    panic!("DESTROY must not consume explosion loot RNG")
                })
                .expect("repo-owned block drop is supported for DESTROY");
            assert_eq!(
                drops,
                [LootDrop::single(Identifier::parse(expected).unwrap())],
                "{block}"
            );
        }
    }

    #[test]
    fn loads_custom_repo_owned_loot_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("loot.json");
        fs::write(
            &path,
            r#"{
              "entities": { "minecraft:cow": "minecraft:beef" },
              "blocks": { "minecraft:stone": "minecraft:cobblestone" }
            }"#,
        )
        .unwrap();

        let loot = load(&path).unwrap();

        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:cow").unwrap()),
            Some(&Identifier::parse("minecraft:beef").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
    }

    #[test]
    fn sidecar_entries_override_only_their_fallback_keys() {
        let stone = Identifier::parse("minecraft:stone").unwrap();
        let mut sidecar = LootTables::from_drop_maps(
            BTreeMap::new(),
            BTreeMap::from([(
                stone.clone(),
                LootDrop::single(Identifier::parse("minecraft:diamond").unwrap()),
            )]),
        );

        sidecar.fill_missing_from(builtin());

        assert_eq!(
            sidecar.block_drop(&stone),
            Some(&Identifier::parse("minecraft:diamond").unwrap())
        );
        assert_eq!(
            sidecar
                .entity_drop_stacks(&Identifier::parse("minecraft:cow").unwrap())
                .map(|drops| drops.iter().map(|drop| &drop.item).collect::<Vec<_>>()),
            Some(vec![
                &Identifier::parse("minecraft:leather").unwrap(),
                &Identifier::parse("minecraft:beef").unwrap(),
            ])
        );
        assert_eq!(sidecar.total_drops(), builtin().total_drops());
    }

    #[test]
    fn partial_entity_table_keeps_sidecar_count_and_adds_missing_fallback_item() {
        let cow = Identifier::parse("minecraft:cow").unwrap();
        let leather = Identifier::parse("minecraft:leather").unwrap();
        let mut sidecar = LootTables::from_drop_lists(
            BTreeMap::from([(cow.clone(), vec![LootDrop::uniform(leather.clone(), 0, 2)])]),
            BTreeMap::new(),
        );

        sidecar.fill_missing_entity_items_from(builtin());

        assert_eq!(
            sidecar.entity_drop_stacks(&cow),
            Some(
                [
                    LootDrop::uniform(leather, 0, 2),
                    LootDrop::single(Identifier::parse("minecraft:beef").unwrap()),
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn loads_simple_vanilla_subset_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&blocks).unwrap();
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            blocks.join("stone.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:alternatives",
                  "children": [
                    {
                      "type": "minecraft:item",
                      "conditions": [{ "condition": "minecraft:match_tool" }],
                      "name": "minecraft:stone"
                    },
                    {
                      "type": "minecraft:item",
                      "conditions": [{ "condition": "minecraft:survives_explosion" }],
                      "name": "minecraft:cobblestone"
                    }
                  ]
                }]
              }]
            }"#,
        )
        .unwrap();
        fs::create_dir_all(entities.join("passive")).unwrap();
        fs::write(
            entities.join("passive").join("cow.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "name": "minecraft:beef"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
        assert_eq!(
            loot.block_drop_stack(&Identifier::parse("minecraft:stone").unwrap())
                .map(|drop| drop.count),
            Some(LootCount::Fixed(1))
        );
        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:passive/cow").unwrap()),
            Some(&Identifier::parse("minecraft:beef").unwrap())
        );
    }

    #[test]
    fn loads_vanilla_subset_set_count_constant() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            entities.join("zombie.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "function": "minecraft:set_count",
                    "count": 2
                  }],
                  "name": "minecraft:rotten_flesh"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.entity_drop_stack(&Identifier::parse("minecraft:zombie").unwrap()),
            Some(&LootDrop {
                item: Identifier::parse("minecraft:rotten_flesh").unwrap(),
                count: LootCount::Fixed(2),
            })
        );
    }

    #[test]
    fn loads_independent_entity_pools_with_uniform_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            entities.join("cow.json"),
            r#"{
              "pools": [
                {
                  "rolls": 1.0,
                  "bonus_rolls": 0.0,
                  "entries": [{
                    "type": "minecraft:item",
                    "functions": [
                      {
                        "function": "minecraft:set_count",
                        "add": false,
                        "count": {
                          "type": "minecraft:uniform",
                          "min": 0.0,
                          "max": 2.0
                        }
                      },
                      {
                        "function": "minecraft:enchanted_count_increase",
                        "enchantment": "minecraft:looting",
                        "count": {
                          "type": "minecraft:uniform",
                          "min": 0.0,
                          "max": 1.0
                        }
                      }
                    ],
                    "name": "minecraft:leather"
                  }]
                },
                {
                  "rolls": 1.0,
                  "bonus_rolls": 0.0,
                  "entries": [{
                    "type": "minecraft:item",
                    "functions": [
                      {
                        "function": "minecraft:set_count",
                        "add": false,
                        "count": {
                          "type": "minecraft:uniform",
                          "min": 1.0,
                          "max": 3.0
                        }
                      },
                      {
                        "function": "minecraft:furnace_smelt",
                        "conditions": [{
                          "condition": "minecraft:entity_properties"
                        }]
                      },
                      {
                        "function": "minecraft:enchanted_count_increase",
                        "enchantment": "minecraft:looting",
                        "count": {
                          "type": "minecraft:uniform",
                          "min": 0.0,
                          "max": 1.0
                        }
                      }
                    ],
                    "name": "minecraft:beef"
                  }]
                }
              ]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.entity_drop_stacks(&Identifier::parse("minecraft:cow").unwrap()),
            Some(
                [
                    LootDrop::uniform(Identifier::parse("minecraft:leather").unwrap(), 0, 2,),
                    LootDrop::uniform(Identifier::parse("minecraft:beef").unwrap(), 1, 3),
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn loads_block_uniform_count_with_no_fortune_or_explosion_baseline() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("lapis_ore.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [
                    {
                      "function": "minecraft:set_count",
                      "count": {
                        "type": "minecraft:uniform",
                        "min": 4.0,
                        "max": 9.0
                      }
                    },
                    {
                      "function": "minecraft:apply_bonus",
                      "enchantment": "minecraft:fortune",
                      "formula": "minecraft:ore_drops"
                    },
                    { "function": "minecraft:explosion_decay" }
                  ],
                  "name": "minecraft:lapis_lazuli"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop_stack(&Identifier::parse("minecraft:lapis_ore").unwrap()),
            Some(&LootDrop::uniform(
                Identifier::parse("minecraft:lapis_lazuli").unwrap(),
                4,
                9,
            ))
        );
    }

    #[test]
    fn skips_vanilla_tables_with_unsupported_functions() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            entities.join("zombie.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{ "function": "minecraft:looting_enchant" }],
                  "name": "minecraft:rotten_flesh"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.entity_drop(&Identifier::parse("minecraft:zombie").unwrap()),
            None
        );
    }

    #[test]
    fn skips_vanilla_tables_with_unsupported_set_count_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        fs::write(
            entities.join("zombie.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "function": "minecraft:set_count",
                    "count": { "min": 0, "max": 2 }
                  }],
                  "name": "minecraft:rotten_flesh"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.entity_drop_stack(&Identifier::parse("minecraft:zombie").unwrap()),
            None
        );
    }

    #[test]
    fn loads_pool_level_set_count_constant() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("oak_leaves.json"),
            r#"{
              "pools": [{
                "functions": [{ "function": "minecraft:set_count", "count": 3 }],
                "entries": [{
                  "type": "minecraft:item",
                  "name": "minecraft:apple"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop_stack(&Identifier::parse("minecraft:oak_leaves").unwrap()),
            Some(&LootDrop {
                item: Identifier::parse("minecraft:apple").unwrap(),
                count: LootCount::Fixed(3),
            })
        );
    }

    #[test]
    fn unsupported_rolls_or_bonus_rolls_fail_closed_for_whole_table() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("bad_rolls.json"),
            r#"{
              "pools": [
                {
                  "rolls": 2,
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                },
                {
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:cobblestone" }]
                }
              ]
            }"#,
        )
        .unwrap();
        fs::write(
            blocks.join("bad_bonus_rolls.json"),
            r#"{
              "pools": [
                {
                  "bonus_rolls": { "type": "minecraft:uniform", "min": 0, "max": 1 },
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                },
                {
                  "entries": [{ "type": "minecraft:item", "name": "minecraft:dirt" }]
                }
              ]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:bad_rolls").unwrap()),
            None
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:bad_bonus_rolls").unwrap()),
            None
        );
    }

    #[test]
    fn skips_set_count_functions_with_unsupported_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp.path().join("entities");
        fs::create_dir_all(&entities).unwrap();
        for (name, function) in [
            (
                "conditioned",
                r#"{
                  "function": "minecraft:set_count",
                  "conditions": [{ "condition": "minecraft:survives_explosion" }],
                  "count": 2
                }"#,
            ),
            (
                "additive",
                r#"{
                  "function": "minecraft:set_count",
                  "add": true,
                  "count": 2
                }"#,
            ),
            (
                "extra_field",
                r#"{
                  "function": "minecraft:set_count",
                  "count": 2,
                  "quality": 1
                }"#,
            ),
        ] {
            fs::write(
                entities.join(format!("{name}.json")),
                format!(
                    r#"{{
                      "pools": [{{
                        "entries": [{{
                          "type": "minecraft:item",
                          "functions": [{function}],
                          "name": "minecraft:rotten_flesh"
                        }}]
                      }}]
                    }}"#
                ),
            )
            .unwrap();
        }

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        for name in ["conditioned", "additive", "extra_field"] {
            assert_eq!(
                loot.entity_drop_stack(&Identifier::parse(format!("minecraft:{name}")).unwrap()),
                None
            );
        }
    }

    #[test]
    fn skips_alternatives_wrappers_with_unsupported_conditions_or_features() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("conditioned_alternatives.json"),
            r#"{
              "pools": [{
                "entries": [
                  {
                    "type": "minecraft:alternatives",
                    "conditions": [{ "condition": "minecraft:match_tool" }],
                    "children": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                  },
                  { "type": "minecraft:item", "name": "minecraft:cobblestone" }
                ]
              }]
            }"#,
        )
        .unwrap();
        fs::write(
            blocks.join("featured_alternatives.json"),
            r#"{
              "pools": [{
                "entries": [
                  {
                    "type": "minecraft:alternatives",
                    "features": ["minecraft:update_1_21"],
                    "children": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
                  },
                  { "type": "minecraft:item", "name": "minecraft:dirt" }
                ]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:conditioned_alternatives").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:featured_alternatives").unwrap()),
            Some(&Identifier::parse("minecraft:dirt").unwrap())
        );
    }

    #[test]
    fn loads_real_vanilla_subset_when_present() {
        let path = workspace_path("data/vanilla/data/minecraft/loot_table");
        if !path.is_dir() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }

        let loot = load_vanilla_subset(path).unwrap();

        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
        assert_eq!(
            loot.block_drop(&Identifier::parse("minecraft:grass_block").unwrap()),
            Some(&Identifier::parse("minecraft:dirt").unwrap())
        );
        assert_eq!(
            loot.block_drop_stack(&Identifier::parse("minecraft:lapis_ore").unwrap()),
            Some(&LootDrop::uniform(
                Identifier::parse("minecraft:lapis_lazuli").unwrap(),
                4,
                9,
            ))
        );
        assert_eq!(
            loot.block_drop_stack(&Identifier::parse("minecraft:redstone_ore").unwrap()),
            Some(&LootDrop::uniform(
                Identifier::parse("minecraft:redstone").unwrap(),
                4,
                5,
            ))
        );
        assert_eq!(
            loot.block_loot(&Identifier::parse("minecraft:stone").unwrap())
                .unwrap()
                .drops_for_tool(true),
            [BlockLootDrop::plain(LootDrop::single(
                Identifier::parse("minecraft:stone").unwrap()
            ))]
        );
        let diamond = &loot
            .block_loot(&Identifier::parse("minecraft:diamond_ore").unwrap())
            .unwrap()
            .drops_for_tool(false)[0];
        assert_eq!(diamond.fortune_bonus, Some(FortuneBonus::OreDrops));
        assert_eq!(diamond.sample_count(0, 1, 2), 2);
        let redstone = &loot
            .block_loot(&Identifier::parse("minecraft:redstone_ore").unwrap())
            .unwrap()
            .drops_for_tool(false)[0];
        assert_eq!(
            redstone.fortune_bonus,
            Some(FortuneBonus::UniformBonusCount {
                bonus_multiplier: 1
            })
        );
        assert_eq!(redstone.sample_count(0, 3, 3), 7);
        assert_eq!(
            loot.block_drop_stacks(&Identifier::parse("minecraft:potted_oak_sapling").unwrap()),
            Some(
                [
                    LootDrop::single(Identifier::parse("minecraft:flower_pot").unwrap()),
                    LootDrop::single(Identifier::parse("minecraft:oak_sapling").unwrap()),
                ]
                .as_slice()
            )
        );
        assert_eq!(
            loot.entity_drop_stacks(&Identifier::parse("minecraft:cow").unwrap()),
            Some(
                [
                    LootDrop::uniform(Identifier::parse("minecraft:leather").unwrap(), 0, 2,),
                    LootDrop::uniform(Identifier::parse("minecraft:beef").unwrap(), 1, 3),
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn keeps_silk_touch_alternative_from_vanilla_block_table() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("stone.json"),
            r#"{
              "pools": [{
                "bonus_rolls": 0.0,
                "entries": [{
                  "type": "minecraft:alternatives",
                  "children": [
                    {
                      "type": "minecraft:item",
                      "conditions": [{
                        "condition": "minecraft:match_tool",
                        "predicate": {
                          "predicates": {
                            "minecraft:enchantments": [{
                              "enchantments": "minecraft:silk_touch",
                              "levels": { "min": 1 }
                            }]
                          }
                        }
                      }],
                      "name": "minecraft:stone"
                    },
                    {
                      "type": "minecraft:item",
                      "conditions": [{ "condition": "minecraft:survives_explosion" }],
                      "name": "minecraft:cobblestone"
                    }
                  ]
                }],
                "rolls": 1.0
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();
        let rule = loot
            .block_loot(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap();

        assert_eq!(
            rule.drops_for_tool(true),
            [BlockLootDrop::plain(LootDrop::single(
                Identifier::parse("minecraft:stone").unwrap()
            ))]
        );
        let regular = &rule.drops_for_tool(false)[0];
        assert_eq!(regular.drop.item.as_str(), "minecraft:cobblestone");
        assert_eq!(
            regular.sample_simple_explosion_count(Some(4.0), &mut || 0.25),
            Some(1)
        );
        assert_eq!(
            regular.sample_simple_explosion_count(Some(4.0), &mut || 0.250_001),
            Some(0)
        );
        assert_eq!(
            regular.sample_simple_explosion_count(None, &mut || {
                panic!("DESTROY must not consume explosion loot RNG")
            }),
            Some(1)
        );
    }

    #[test]
    fn explosion_decay_rolls_once_per_fixed_count_item() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("test_ore.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [
                    { "function": "minecraft:set_count", "count": 4 },
                    { "function": "minecraft:explosion_decay" }
                  ],
                  "name": "minecraft:diamond"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();
        let block = Identifier::parse("minecraft:test_ore").unwrap();
        let mut rolls = [0.1, 0.9, 0.25, 0.5].into_iter();
        let drops = loot
            .block_explosion_drops(&block, &[], Some(4.0), || rolls.next().unwrap())
            .expect("simple fixed-count explosion table is supported");

        assert_eq!(
            drops,
            [LootDrop::fixed(
                Identifier::parse("minecraft:diamond").unwrap(),
                2
            )]
        );
        assert_eq!(rolls.next(), None, "one roll is consumed per input item");
    }

    #[test]
    fn ore_drops_fortune_level_changes_sampled_count() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("diamond_ore.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:alternatives",
                  "children": [
                    {
                      "type": "minecraft:item",
                      "conditions": [{
                        "condition": "minecraft:match_tool",
                        "predicate": {
                          "predicates": {
                            "minecraft:enchantments": [{
                              "enchantments": "minecraft:silk_touch",
                              "levels": { "min": 1 }
                            }]
                          }
                        }
                      }],
                      "name": "minecraft:diamond_ore"
                    },
                    {
                      "type": "minecraft:item",
                      "functions": [{
                        "enchantment": "minecraft:fortune",
                        "formula": "minecraft:ore_drops",
                        "function": "minecraft:apply_bonus"
                      }, {
                        "function": "minecraft:explosion_decay"
                      }],
                      "name": "minecraft:diamond"
                    }
                  ]
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();
        let drop = &loot
            .block_loot(&Identifier::parse("minecraft:diamond_ore").unwrap())
            .unwrap()
            .drops_for_tool(false)[0];

        assert_eq!(drop.fortune_bonus, Some(FortuneBonus::OreDrops));
        assert_eq!(drop.sample_count(0, 0, 2), 1);
        assert_eq!(drop.sample_count(0, 1, 2), 2);
        assert_eq!(drop.sample_count(0, 3, 4), 4);
    }

    #[test]
    fn parses_and_samples_binomial_with_bonus_count() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("test_crop.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "enchantment": "minecraft:fortune",
                    "formula": "minecraft:binomial_with_bonus_count",
                    "function": "minecraft:apply_bonus",
                    "parameters": {
                      "extra": 3,
                      "probability": 1.0
                    }
                  }],
                  "name": "minecraft:wheat_seeds"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();
        let drop = &loot
            .block_loot(&Identifier::parse("minecraft:test_crop").unwrap())
            .unwrap()
            .drops_for_tool(false)[0];

        assert_eq!(
            drop.fortune_bonus,
            Some(FortuneBonus::BinomialWithBonusCount {
                extra_rounds: 3,
                probability: 1.0,
            })
        );
        assert_eq!(drop.sample_count(0, 0, 7), 4);
        assert_eq!(drop.sample_count(0, 1, 7), 5);
        assert_eq!(drop.sample_count(0, 3, 7), 7);
    }

    #[test]
    fn block_state_crop_pools_choose_mature_and_fallback_alternatives() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp.path().join("blocks");
        fs::create_dir_all(&blocks).unwrap();
        fs::write(
            blocks.join("wheat.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:alternatives",
                  "children": [{
                    "type": "minecraft:item",
                    "conditions": [{
                      "block": "minecraft:wheat",
                      "condition": "minecraft:block_state_property",
                      "properties": { "age": "7" }
                    }],
                    "name": "minecraft:wheat"
                  }, {
                    "type": "minecraft:item",
                    "name": "minecraft:wheat_seeds"
                  }]
                }]
              }, {
                "conditions": [{
                  "block": "minecraft:wheat",
                  "condition": "minecraft:block_state_property",
                  "properties": { "age": "7" }
                }],
                "entries": [{
                  "type": "minecraft:item",
                  "functions": [{
                    "enchantment": "minecraft:fortune",
                    "formula": "minecraft:binomial_with_bonus_count",
                    "function": "minecraft:apply_bonus",
                    "parameters": { "extra": 3, "probability": 1.0 }
                  }],
                  "name": "minecraft:wheat_seeds"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_vanilla_subset(tmp.path()).unwrap();
        let wheat = Identifier::parse("minecraft:wheat").unwrap();
        let rule = loot.block_loot(&wheat).unwrap();
        let young = vec![("age".to_string(), "6".to_string())];
        let mature = vec![("age".to_string(), "7".to_string())];

        let young_drops = rule.drops_for_context(false, &wheat, &young);
        assert_eq!(young_drops.len(), 1);
        assert_eq!(young_drops[0].drop.item.as_str(), "minecraft:wheat_seeds");

        let mature_drops = rule.drops_for_context(false, &wheat, &mature);
        assert_eq!(mature_drops.len(), 2);
        assert_eq!(mature_drops[0].drop.item.as_str(), "minecraft:wheat");
        assert_eq!(mature_drops[1].drop.item.as_str(), "minecraft:wheat_seeds");
        assert_eq!(mature_drops[1].sample_count(0, 0, 0), 4);
        assert_eq!(mature_drops[1].sample_count(0, 2, 0), 6);
    }

    #[test]
    fn real_binomial_fixture_has_expected_parameters_when_present() {
        let path = workspace_path("data/vanilla/data/minecraft/loot_table/blocks/carrots.json");
        if !path.is_file() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }

        let carrots: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        let entry = carrots.pointer("/pools/1/entries/0").unwrap();
        let binomial = fortune_bonus_from_functions(entry).unwrap();

        assert_eq!(
            binomial,
            Some(FortuneBonus::BinomialWithBonusCount {
                extra_rounds: 3,
                probability: 0.571_428_6,
            })
        );
    }

    #[test]
    fn real_crop_fixtures_load_state_conditioned_pools_when_present() {
        let root = workspace_path("data/vanilla/data/minecraft/loot_table");
        if !root.join("blocks/wheat.json").is_file() {
            eprintln!("skipping: {} not present", root.display());
            return;
        }

        let loot = load_vanilla_subset(&root).unwrap();
        for (block, young_age, mature_age, young_expected, mature_expected) in [
            (
                "minecraft:wheat",
                "6",
                "7",
                &["minecraft:wheat_seeds"][..],
                &["minecraft:wheat", "minecraft:wheat_seeds"][..],
            ),
            (
                "minecraft:carrots",
                "6",
                "7",
                &["minecraft:carrot"][..],
                &["minecraft:carrot", "minecraft:carrot"][..],
            ),
            (
                "minecraft:potatoes",
                "6",
                "7",
                &["minecraft:potato"][..],
                &[
                    "minecraft:potato",
                    "minecraft:potato",
                    "minecraft:poisonous_potato",
                ][..],
            ),
            (
                "minecraft:beetroots",
                "2",
                "3",
                &["minecraft:beetroot_seeds"][..],
                &["minecraft:beetroot", "minecraft:beetroot_seeds"][..],
            ),
        ] {
            let block = Identifier::parse(block).unwrap();
            let rule = loot.block_loot(&block).expect("real crop block loot");
            let items = |age: &str| {
                rule.drops_for_context(false, &block, &[("age".to_string(), age.to_string())])
                    .into_iter()
                    .map(|drop| drop.drop.item.as_str())
                    .collect::<Vec<_>>()
            };
            assert_eq!(items(young_age), young_expected, "young {block}");
            assert_eq!(items(mature_age), mature_expected, "mature {block}");
        }
    }

    #[test]
    fn fortune_level_zero_preserves_baseline_count_sampling() {
        let drop = BlockLootDrop {
            drop: LootDrop::uniform(Identifier::parse("minecraft:lapis_lazuli").unwrap(), 4, 9),
            fortune_bonus: Some(FortuneBonus::OreDrops),
            random_chance: None,
            survives_explosion: false,
            explosion_decay: false,
        };

        for roll in 0..12 {
            assert_eq!(
                drop.sample_count(roll, 0, u64::MAX - roll),
                drop.drop.count.sample(roll)
            );
        }
    }

    #[test]
    fn completes_real_vanilla_sheep_table_when_present() {
        let path = workspace_path("data/vanilla/data/minecraft/loot_table");
        if !path.is_dir() {
            eprintln!("skipping: {} not present", path.display());
            return;
        }
        let sheep = Identifier::parse("minecraft:sheep").unwrap();
        let mut loot = load_vanilla_subset(path).unwrap();

        loot.fill_missing_from(builtin());
        loot.fill_missing_entity_items_from(builtin());

        assert_eq!(
            loot.entity_drop_stacks(&sheep),
            Some(
                [
                    LootDrop::uniform(Identifier::parse("minecraft:mutton").unwrap(), 1, 2),
                    LootDrop::single(Identifier::parse("minecraft:white_wool").unwrap()),
                ]
                .as_slice()
            )
        );
    }

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }
}
