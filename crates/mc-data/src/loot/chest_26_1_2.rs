//! Compiled `minecraft:chest` loot tables with deterministic paste-time rolls.
//!
//! Structure chests (the seed-zero Solaris ruin, village toolsmith houses)
//! roll real vanilla chest tables at paste time instead of embedding fixed
//! contents. Only the JSON surface those tables use is compiled:
//!
//! - pool `rolls`: a constant or `minecraft:uniform` `{min, max}` range
//!   (`bonus_rolls` is ignored: worldgen rolls with no luck modifier);
//! - entries: `minecraft:item` (with an optional `minecraft:set_count`
//!   constant/uniform count function) or `minecraft:empty`;
//! - `minecraft:enchant_randomly` is a known cosmetic drop: paste-time rolls
//!   keep the plain item because enchantment synthesis needs a live player
//!   context the generator does not have.
//!
//! Anything else (pool/entry `conditions`, pool `functions`, nested
//! `minecraft:loot_table` references, unknown item functions, non-uniform
//! count/roll providers) fails closed at compile time so a future vanilla
//! table that outgrows this surface can never silently roll wrong loot.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use thiserror::Error;

use super::{LootCount, LootDrop};
use crate::Identifier;

/// Rejects chest-table sources larger than this before JSON parsing.
pub const MAX_CHEST_TABLE_BYTES: usize = 65_536;
/// Bounds pools per `minecraft:chest` table.
pub const MAX_CHEST_POOLS: usize = 16;
/// Bounds entries per pool.
pub const MAX_CHEST_ENTRIES: usize = 64;
/// Bounds constant/uniform rolls and counts (vanilla chest values are small).
pub const MAX_CHEST_ROLLS: u32 = 64;
/// Bounds stack counts produced by `minecraft:set_count`.
pub const MAX_CHEST_COUNT: u32 = 64;
/// Bounds a single entry weight; the per-pool total is `u64`-summed.
pub const MAX_CHEST_WEIGHT: u64 = 1_000_000;
/// Slot count of a vanilla chest block entity; rolls scatter into these.
pub const CHEST_SLOT_COUNT: u64 = 27;
#[derive(Debug, Error)]
pub enum ChestLootError {
    #[error("chest loot table at {path} is not valid UTF-8")]
    NonUtf8 { path: PathBuf },
    #[error("chest loot table {id} is missing at {path}")]
    MissingTable { id: Identifier, path: PathBuf },
    #[error("filesystem error reading chest loot table {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("chest loot table at {path} exceeds {max} bytes")]
    TooLarge { path: PathBuf, max: usize },
    #[error("chest loot table at {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("chest loot table {id} is unsupported: {reason}")]
    Unsupported { id: Identifier, reason: String },
}

/// Deterministic SplitMix64 stream for chest rolls and slot scatter.
///
/// The stream matches the `LootRandom` generator in `loot::context` so chest
/// loot shares the repo's single deterministic RNG shape; one stream drives
/// both the drop roll and the slot scatter from a single per-chest seed.
#[derive(Debug, Clone)]
pub struct ChestRng {
    state: u64,
}

impl ChestRng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    /// Uniform value in `0..bound`. Panics on a zero bound; callers only pass
    /// validated non-zero totals and the chest slot count.
    #[must_use]
    pub fn next_bounded(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "chest RNG bound must be non-zero");
        self.next_u64() % bound
    }

    #[must_use]
    pub fn next_range_inclusive(&mut self, min: u32, max: u32) -> u32 {
        debug_assert!(min <= max, "chest RNG range must be ordered");
        min + (self.next_bounded(u64::from(max - min) + 1) as u32)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChestLootEntry {
    Empty {
        weight: u64,
    },
    Item {
        weight: u64,
        item: Identifier,
        count_min: u32,
        count_max: u32,
    },
}

impl ChestLootEntry {
    #[must_use]
    pub fn weight(&self) -> u64 {
        match self {
            Self::Empty { weight } | Self::Item { weight, .. } => *weight,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChestLootPool {
    pub rolls_min: u32,
    pub rolls_max: u32,
    pub entries: Vec<ChestLootEntry>,
    pub total_weight: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChestLootTable {
    id: Identifier,
    pools: Vec<ChestLootPool>,
}

impl ChestLootTable {
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    #[must_use]
    pub fn pools(&self) -> &[ChestLootPool] {
        &self.pools
    }

    /// Compile a vanilla `minecraft:chest` table source. Fails closed on any
    /// construct outside the surface documented at the top of this module.
    ///
    /// # Errors
    ///
    /// Returns [`ChestLootError::Unsupported`] when the source is not a
    /// supported chest table.
    pub fn compile(id: Identifier, raw: &str) -> Result<Self, ChestLootError> {
        let unsupported = |reason: String| ChestLootError::Unsupported {
            id: id.clone(),
            reason,
        };
        if raw.len() > MAX_CHEST_TABLE_BYTES {
            return Err(ChestLootError::TooLarge {
                path: PathBuf::from(id.as_str()),
                max: MAX_CHEST_TABLE_BYTES,
            });
        }
        let value: Value =
            serde_json::from_str(raw).map_err(|source| ChestLootError::Malformed {
                path: PathBuf::from(id.as_str()),
                source,
            })?;
        let root = value
            .as_object()
            .ok_or_else(|| unsupported("chest table root must be a JSON object".to_string()))?;
        let table_type = root
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| unsupported("chest table is missing $.type".to_string()))?;
        if table_type != "minecraft:chest" {
            return Err(unsupported(format!(
                "expected $.type \"minecraft:chest\", found {table_type:?}"
            )));
        }
        let pools = root
            .get("pools")
            .and_then(Value::as_array)
            .ok_or_else(|| unsupported("chest table is missing $.pools".to_string()))?;
        if pools.is_empty() || pools.len() > MAX_CHEST_POOLS {
            return Err(unsupported(format!(
                "chest table must declare 1..={} pools, found {}",
                MAX_CHEST_POOLS,
                pools.len()
            )));
        }
        let mut compiled = Vec::with_capacity(pools.len());
        for (pool_index, pool) in pools.iter().enumerate() {
            compiled.push(compile_pool(&id, pool_index, pool)?);
        }
        Ok(Self {
            id,
            pools: compiled,
        })
    }

    /// Roll the table once, returning one stack per successful pick.
    /// Deterministic in the RNG stream: the same seed yields the same drops.
    #[must_use]
    pub fn roll(&self, rng: &mut ChestRng) -> Vec<LootDrop> {
        let mut out = Vec::new();
        for pool in &self.pools {
            let rolls = rng.next_range_inclusive(pool.rolls_min, pool.rolls_max);
            for _ in 0..rolls {
                let mut pick = rng.next_bounded(pool.total_weight);
                let mut chosen: Option<&ChestLootEntry> = None;
                for entry in &pool.entries {
                    let weight = entry.weight();
                    if pick < weight {
                        chosen = Some(entry);
                        break;
                    }
                    pick -= weight;
                }
                let Some(entry) = chosen else {
                    continue;
                };
                if let ChestLootEntry::Item {
                    item,
                    count_min,
                    count_max,
                    ..
                } = entry
                {
                    let count = rng.next_range_inclusive(*count_min, *count_max);
                    if count > 0 {
                        out.push(LootDrop {
                            item: item.clone(),
                            count: LootCount::Fixed(count),
                        });
                    }
                }
            }
        }
        out
    }
}

fn compile_pool(
    id: &Identifier,
    pool_index: usize,
    pool: &Value,
) -> Result<ChestLootPool, ChestLootError> {
    let unsupported = |reason: String| ChestLootError::Unsupported {
        id: id.clone(),
        reason,
    };
    let root = pool.as_object().ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} must be a JSON object"
        ))
    })?;
    if root.contains_key("conditions") {
        return Err(unsupported(format!(
            "chest table pool {pool_index} has unsupported conditions"
        )));
    }
    if root.contains_key("functions") {
        return Err(unsupported(format!(
            "chest table pool {pool_index} has unsupported functions"
        )));
    }
    let rolls = root
        .get("rolls")
        .ok_or_else(|| unsupported(format!("chest table pool {pool_index} is missing rolls")))?;
    let (rolls_min, rolls_max) = compile_range(rolls, MAX_CHEST_ROLLS).map_err(|detail| {
        unsupported(format!(
            "chest table pool {pool_index} has invalid rolls: {detail}"
        ))
    })?;
    let entries = root
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| unsupported(format!("chest table pool {pool_index} is missing entries")))?;
    if entries.is_empty() || entries.len() > MAX_CHEST_ENTRIES {
        return Err(unsupported(format!(
            "chest table pool {pool_index} must declare 1..={} entries, found {}",
            MAX_CHEST_ENTRIES,
            entries.len()
        )));
    }
    let mut compiled = Vec::with_capacity(entries.len());
    let mut total_weight = 0_u64;
    for (entry_index, entry) in entries.iter().enumerate() {
        let entry = compile_entry(id, pool_index, entry_index, entry)?;
        total_weight = total_weight.checked_add(entry.weight()).ok_or_else(|| {
            unsupported(format!("chest table pool {pool_index} overflows weights"))
        })?;
        if total_weight > MAX_CHEST_ENTRIES as u64 * MAX_CHEST_WEIGHT {
            return Err(unsupported(format!(
                "chest table pool {pool_index} overflows weights"
            )));
        }
        compiled.push(entry);
    }
    if total_weight == 0 {
        return Err(unsupported(format!(
            "chest table pool {pool_index} has zero total weight"
        )));
    }
    Ok(ChestLootPool {
        rolls_min,
        rolls_max,
        entries: compiled,
        total_weight,
    })
}

fn compile_entry(
    id: &Identifier,
    pool_index: usize,
    entry_index: usize,
    entry: &Value,
) -> Result<ChestLootEntry, ChestLootError> {
    let unsupported = |reason: String| ChestLootError::Unsupported {
        id: id.clone(),
        reason,
    };
    let root = entry.as_object().ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} must be a JSON object"
        ))
    })?;
    if root.contains_key("conditions") {
        return Err(unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} has unsupported conditions"
        )));
    }
    let entry_type = root.get("type").and_then(Value::as_str).ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} is missing a type"
        ))
    })?;
    let weight = match root.get("weight") {
        None => 1_u64,
        Some(value) => integer_value(value, 0, MAX_CHEST_WEIGHT).map_err(|detail| {
            unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has invalid weight: {detail}"
            ))
        })?,
    };
    match entry_type {
        "minecraft:empty" => Ok(ChestLootEntry::Empty { weight }),
        "minecraft:item" => {
            let name = root.get("name").and_then(Value::as_str).ok_or_else(|| {
                unsupported(format!(
                    "chest table pool {pool_index} entry {entry_index} is missing a name"
                ))
            })?;
            let item = Identifier::parse(name.to_string()).map_err(|_| {
                unsupported(format!(
                    "chest table pool {pool_index} entry {entry_index} has invalid name {name:?}"
                ))
            })?;
            let (count_min, count_max) =
                compile_functions(root.get("functions"), pool_index, entry_index, id)?;
            Ok(ChestLootEntry::Item {
                weight,
                item,
                count_min,
                count_max,
            })
        }
        other => Err(unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} has unsupported type {other:?}"
        ))),
    }
}

/// Resolve the effective stack-size range from an entry's `functions` list.
/// `minecraft:set_count` sets the range; `minecraft:enchant_randomly` is a
/// known cosmetic drop (plain item kept); anything else fails closed.
fn compile_functions(
    functions: Option<&Value>,
    pool_index: usize,
    entry_index: usize,
    id: &Identifier,
) -> Result<(u32, u32), ChestLootError> {
    let unsupported = |reason: String| ChestLootError::Unsupported {
        id: id.clone(),
        reason,
    };
    let Some(functions) = functions else {
        return Ok((1, 1));
    };
    let functions = functions.as_array().ok_or_else(|| {
        unsupported(format!(
            "chest table pool {pool_index} entry {entry_index} has malformed functions"
        ))
    })?;
    let mut count = (1_u32, 1_u32);
    for function in functions {
        let root = function.as_object().ok_or_else(|| {
            unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has malformed functions"
            ))
        })?;
        let name = root.get("function").and_then(Value::as_str).ok_or_else(|| {
            unsupported(format!(
                "chest table pool {pool_index} entry {entry_index} has a function without a name"
            ))
        })?;
        match name {
            "minecraft:set_count" => {
                let add = root.get("add").and_then(Value::as_bool).unwrap_or(false);
                if add {
                    return Err(unsupported(format!(
                        "chest table pool {pool_index} entry {entry_index} uses additive set_count"
                    )));
                }
                let value = root.get("count").ok_or_else(|| {
                    unsupported(format!(
                        "chest table pool {pool_index} entry {entry_index} set_count is missing a count"
                    ))
                })?;
                count = compile_range(value, MAX_CHEST_COUNT).map_err(|detail| {
                    unsupported(format!(
                        "chest table pool {pool_index} entry {entry_index} has invalid set_count: {detail}"
                    ))
                })?;
            }
            // Paste-time rolls have no player context to synthesize
            // enchantments with; the unenchanted item is kept instead.
            "minecraft:enchant_randomly" => {}
            other => {
                return Err(unsupported(format!(
                    "chest table pool {pool_index} entry {entry_index} has unsupported function {other:?}"
                )));
            }
        }
    }
    Ok(count)
}

/// Compile a constant (`3.0`) or `minecraft:uniform` `{min, max}` range into
/// an ordered inclusive `(min, max)` pair bounded by `max`.
fn compile_range(value: &Value, max: u32) -> Result<(u32, u32), String> {
    if let Some(number) = value.as_f64() {
        let single = integer_in_range(number, 0, max)
            .ok_or_else(|| format!("expected an integral constant in 0..={max}, found {number}"))?;
        return Ok((single, single));
    }
    let root = value
        .as_object()
        .ok_or_else(|| "expected a constant or uniform range".to_string())?;
    if root.get("type").and_then(Value::as_str) != Some("minecraft:uniform") {
        return Err(format!(
            "expected range type \"minecraft:uniform\", found {:?}",
            root.get("type")
        ));
    }
    let min = root
        .get("min")
        .and_then(Value::as_f64)
        .and_then(|number| integer_in_range(number, 0, max))
        .ok_or_else(|| format!("expected integral min in 0..={max}"))?;
    let high = root
        .get("max")
        .and_then(Value::as_f64)
        .and_then(|number| integer_in_range(number, 0, max))
        .ok_or_else(|| format!("expected integral max in 0..={max}"))?;
    if min > high {
        return Err(format!("unordered range {min}..={high}"));
    }
    Ok((min, high))
}

fn integer_value(value: &Value, min: u64, max: u64) -> Result<u64, String> {
    let number = value
        .as_f64()
        .ok_or_else(|| format!("expected a number, found {value}"))?;
    if number.is_finite() && number.fract() == 0.0 && number >= min as f64 && number <= max as f64 {
        Ok(number as u64)
    } else {
        Err(format!(
            "expected an integral value in {min}..={max}, found {number}"
        ))
    }
}

fn integer_in_range(number: f64, min: u32, max: u32) -> Option<u32> {
    if number.is_finite()
        && number.fract() == 0.0
        && number >= f64::from(min)
        && number <= f64::from(max)
    {
        Some(number as u32)
    } else {
        None
    }
}

/// Compiled chest tables keyed by loot-table id, loaded from vanilla data.
#[derive(Debug, Clone, Default)]
pub struct ChestLootCatalog {
    tables: BTreeMap<Identifier, ChestLootTable>,
}

impl ChestLootCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tables.len()
    }

    #[must_use]
    pub fn get(&self, id: &Identifier) -> Option<&ChestLootTable> {
        self.tables.get(id)
    }

    pub fn insert(&mut self, table: ChestLootTable) {
        self.tables.insert(table.id.clone(), table);
    }

    /// Load the named tables from a vanilla data root, where table
    /// `namespace:path` is read from
    /// `<data_root>/<namespace>/loot_table/<path>.json`. Missing files fail
    /// closed so a misconfigured data directory can never silently fall back
    /// to fixed chest contents.
    ///
    /// # Errors
    ///
    /// Returns [`ChestLootError`] when a table file is missing, too large,
    /// malformed, or unsupported.
    pub fn load_vanilla_tables(
        data_root: impl AsRef<Path>,
        ids: &[Identifier],
    ) -> Result<Self, ChestLootError> {
        let mut catalog = Self::new();
        for id in ids {
            catalog.insert(Self::load_vanilla_table(data_root.as_ref(), id)?);
        }
        Ok(catalog)
    }

    fn load_vanilla_table(
        data_root: &Path,
        id: &Identifier,
    ) -> Result<ChestLootTable, ChestLootError> {
        let relative = Path::new(id.namespace())
            .join("loot_table")
            .join(format!("{}.json", id.path()));
        let path = data_root.join(&relative);
        if !path.is_file() {
            return Err(ChestLootError::MissingTable {
                id: id.clone(),
                path,
            });
        }
        let raw = std::fs::read(&path).map_err(|source| ChestLootError::Io {
            path: path.clone(),
            source,
        })?;
        if raw.len() > MAX_CHEST_TABLE_BYTES {
            return Err(ChestLootError::TooLarge {
                path,
                max: MAX_CHEST_TABLE_BYTES,
            });
        }
        let text =
            String::from_utf8(raw).map_err(|_| ChestLootError::NonUtf8 { path: path.clone() })?;
        ChestLootTable::compile(id.clone(), &text).map_err(|error| match error {
            ChestLootError::Unsupported { reason, .. } => ChestLootError::Unsupported {
                id: id.clone(),
                reason,
            },
            other => other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn id(value: &str) -> Identifier {
        Identifier::parse(value).unwrap()
    }

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    const SIMPLE_TABLE: &str = r#"{
        "type": "minecraft:chest",
        "pools": [
            {"rolls": 2.0, "entries": [
                {"type": "minecraft:item", "name": "minecraft:bread"},
                {"type": "minecraft:empty"}
            ]}
        ]
    }"#;

    #[test]
    fn rejects_non_chest_tables() {
        let error = ChestLootTable::compile(
            id("minecraft:chests/test"),
            r#"{"type":"minecraft:entity","pools":[]}"#,
        )
        .expect_err("entity tables must not compile as chest tables");
        assert!(matches!(error, ChestLootError::Unsupported { .. }));
    }

    #[test]
    fn rejects_unsupported_entry_types_conditions_and_functions() {
        for raw in [
            r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"entries":[{"type":"minecraft:loot_table","value":"minecraft:chests/other"}]}]}"#,
            r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"conditions":[{"condition":"minecraft:random_chance"}],"entries":[{"type":"minecraft:item","name":"minecraft:bread"}]}]}"#,
            r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"entries":[{"type":"minecraft:item","name":"minecraft:bread","conditions":[{"condition":"minecraft:random_chance"}]}]}]}"#,
            r#"{"type":"minecraft:chest","pools":[{"rolls":1.0,"entries":[{"type":"minecraft:item","name":"minecraft:bread","functions":[{"function":"minecraft:set_contents"}]}]}]}"#,
        ] {
            let error = ChestLootTable::compile(id("minecraft:chests/test"), raw)
                .expect_err("unsupported constructs must fail closed");
            assert!(matches!(error, ChestLootError::Unsupported { .. }), "{raw}");
        }
    }

    #[test]
    fn keeps_plain_item_for_enchant_randomly_and_applies_set_count() {
        let table = ChestLootTable::compile(
            id("minecraft:chests/test"),
            r#"{"type":"minecraft:chest","pools":[
                {"rolls":1.0,"entries":[
                    {"type":"minecraft:item","name":"minecraft:book","functions":[{"function":"minecraft:enchant_randomly"}]},
                    {"type":"minecraft:item","name":"minecraft:coal","weight":3,"functions":[{"add":false,"count":{"type":"minecraft:uniform","min":2.0,"max":4.0},"function":"minecraft:set_count"}]}
                ]}
            ]}"#,
        )
        .expect("enchant_randomly and set_count must compile");
        assert_eq!(table.pools.len(), 1);
        assert_eq!(table.pools[0].total_weight, 4);
        let mut book_count = None;
        let mut coal_range = None;
        for entry in &table.pools[0].entries {
            if let ChestLootEntry::Item {
                item,
                count_min,
                count_max,
                ..
            } = entry
            {
                if item.as_str() == "minecraft:book" {
                    book_count = Some((*count_min, *count_max));
                } else {
                    coal_range = Some((*count_min, *count_max));
                }
            }
        }
        assert_eq!(book_count, Some((1, 1)));
        assert_eq!(coal_range, Some((2, 4)));
    }

    #[test]
    fn rolls_are_deterministic_per_seed() {
        let table = ChestLootTable::compile(id("minecraft:chests/test"), SIMPLE_TABLE)
            .expect("simple table");
        let first = table.roll(&mut ChestRng::new(7));
        let second = table.roll(&mut ChestRng::new(7));
        assert_eq!(first, second);
        let third = table.roll(&mut ChestRng::new(8));
        assert_ne!(first, third);
    }

    #[test]
    fn all_empty_pool_rolls_nothing() {
        let table = ChestLootTable::compile(
            id("minecraft:chests/test"),
            r#"{"type":"minecraft:chest","pools":[{"rolls":4.0,"entries":[{"type":"minecraft:empty"}]}]}"#,
        )
        .expect("empty pool compiles");
        assert!(table.roll(&mut ChestRng::new(1)).is_empty());
    }

    #[test]
    fn loads_real_vanilla_chest_tables_when_present() {
        let root = workspace_path("data/vanilla/data");
        if !root.join("minecraft/loot_table/chests").is_dir() {
            return;
        }
        for table in [
            "minecraft:chests/village/village_toolsmith",
            "minecraft:chests/simple_dungeon",
        ] {
            let catalog = ChestLootCatalog::load_vanilla_tables(&root, &[id(table)])
                .expect("real table loads");
            let compiled = catalog.get(&id(table)).expect("catalog holds the table");
            assert!(!compiled.pools().is_empty());
            let drops = compiled.roll(&mut ChestRng::new(0));
            assert!(!drops.is_empty(), "{table} seed-0 roll must yield loot");
        }
    }

    #[test]
    fn missing_table_file_fails_closed() {
        let missing = id("minecraft:chests/no_such_table");
        let error =
            ChestLootCatalog::load_vanilla_tables(workspace_path("data/vanilla/data"), &[missing])
                .expect_err("missing tables must not fall back silently");
        assert!(matches!(error, ChestLootError::MissingTable { .. }));
    }
}
