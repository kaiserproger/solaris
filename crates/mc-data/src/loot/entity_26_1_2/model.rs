use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

use super::super::{LootContextItem, LootEnchantments, LootRandomBinding};
use crate::Identifier;

pub const MAX_COMPILE_NESTING_DEPTH: usize = 64;
pub const MAX_SOURCE_BYTES: usize = 1_048_576;
pub const MAX_JSON_NODES: usize = 65_536;
pub const MAX_JSON_COLLECTION_ELEMENTS: usize = 65_536;
pub const MAX_JSON_STRING_BYTES: usize = 262_144;
pub const MAX_JSON_ARRAY_LENGTH: usize = 4_096;
pub const MAX_POOLS_PER_TABLE: usize = 256;
pub const MAX_REFERENCES_PER_TABLE: usize = 512;
pub const MAX_CATALOG_ROOTS: usize = 128;
pub const MAX_CATALOG_RESOURCES: usize = 512;
pub const MAX_CLOSURE_WIDTH: usize = 256;
pub const MAX_REFERENCE_DEPTH: usize = 32;
pub const MAX_CANDIDATES_PER_ROLL: usize = 1_024;
pub const MAX_RUNTIME_RECURSION: usize = 32;
pub const MAX_OUTPUT_STACKS: usize = 1_024;
pub const MAX_OUTPUT_ITEMS: u64 = 65_536;
pub const MAX_TAG_EXPANSION: usize = 512;
pub const MAX_TOTAL_OPERATIONS: usize = 65_536;
pub const MAX_CONTEXT_COMPONENTS: usize = 256;
pub const MAX_CONTEXT_ENCHANTMENTS: usize = 256;
pub const MAX_CONTEXT_ENCHANTMENT_LEVELS: usize = 256;
pub const MAX_CONTEXT_DAMAGE_TAGS: usize = 256;
pub(super) const MAX_POOL_ROLLS: i32 = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityLootLimit {
    SourceBytes,
    JsonNodes,
    JsonCollectionElements,
    JsonStringBytes,
    JsonArrayLength,
    PoolsPerTable,
    ReferencesPerTable,
    CatalogRoots,
    CatalogResources,
    ClosureWidth,
    CompileNesting,
    ReferenceDepth,
    CandidatesPerRoll,
    RuntimeRecursion,
    OutputStacks,
    OutputItems,
    TagExpansion,
    TotalOperations,
    PoolRolls,
    ContextComponents,
    ContextEnchantments,
    ContextEnchantmentLevels,
    ContextDamageTags,
}

impl EntityLootLimit {
    pub(super) fn maximum(self) -> u64 {
        match self {
            Self::SourceBytes => MAX_SOURCE_BYTES as u64,
            Self::JsonNodes => MAX_JSON_NODES as u64,
            Self::JsonCollectionElements => MAX_JSON_COLLECTION_ELEMENTS as u64,
            Self::JsonStringBytes => MAX_JSON_STRING_BYTES as u64,
            Self::JsonArrayLength => MAX_JSON_ARRAY_LENGTH as u64,
            Self::PoolsPerTable => MAX_POOLS_PER_TABLE as u64,
            Self::ReferencesPerTable => MAX_REFERENCES_PER_TABLE as u64,
            Self::CatalogRoots => MAX_CATALOG_ROOTS as u64,
            Self::CatalogResources => MAX_CATALOG_RESOURCES as u64,
            Self::ClosureWidth => MAX_CLOSURE_WIDTH as u64,
            Self::CompileNesting => MAX_COMPILE_NESTING_DEPTH as u64,
            Self::ReferenceDepth => MAX_REFERENCE_DEPTH as u64,
            Self::CandidatesPerRoll => MAX_CANDIDATES_PER_ROLL as u64,
            Self::RuntimeRecursion => MAX_RUNTIME_RECURSION as u64,
            Self::OutputStacks => MAX_OUTPUT_STACKS as u64,
            Self::OutputItems => MAX_OUTPUT_ITEMS,
            Self::TagExpansion => MAX_TAG_EXPANSION as u64,
            Self::TotalOperations => MAX_TOTAL_OPERATIONS as u64,
            Self::PoolRolls => MAX_POOL_ROLLS as u64,
            Self::ContextComponents => MAX_CONTEXT_COMPONENTS as u64,
            Self::ContextEnchantments => MAX_CONTEXT_ENCHANTMENTS as u64,
            Self::ContextEnchantmentLevels => MAX_CONTEXT_ENCHANTMENT_LEVELS as u64,
            Self::ContextDamageTags => MAX_CONTEXT_DAMAGE_TAGS as u64,
        }
    }
}

impl fmt::Display for EntityLootLimit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SourceBytes => "source bytes",
            Self::JsonNodes => "JSON nodes",
            Self::JsonCollectionElements => "JSON collection elements",
            Self::JsonStringBytes => "JSON string bytes",
            Self::JsonArrayLength => "JSON array length",
            Self::PoolsPerTable => "pools per table",
            Self::ReferencesPerTable => "references per table",
            Self::CatalogRoots => "catalog roots",
            Self::CatalogResources => "catalog resources",
            Self::ClosureWidth => "reference closure width",
            Self::CompileNesting => "compile nesting",
            Self::ReferenceDepth => "reference depth",
            Self::CandidatesPerRoll => "candidates per roll",
            Self::RuntimeRecursion => "runtime recursion",
            Self::OutputStacks => "output stacks",
            Self::OutputItems => "output items",
            Self::TagExpansion => "tag expansion",
            Self::TotalOperations => "total operations",
            Self::PoolRolls => "pool rolls",
            Self::ContextComponents => "context components",
            Self::ContextEnchantments => "context item enchantments",
            Self::ContextEnchantmentLevels => "context enchantment levels",
            Self::ContextDamageTags => "context damage tags",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLootCompileError {
    pub table: Identifier,
    pub path: String,
    pub kind: EntityLootCompileErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityLootCompileErrorKind {
    MalformedJson { message: String },
    Expected { expected: &'static str },
    MissingField { field: String },
    UnsupportedField { field: String },
    UnsupportedCondition { condition: String },
    UnsupportedFunction { function: String },
    UnsupportedEntry { entry: String },
    UnsupportedNumberProvider { provider: String },
    UnsupportedEntityPredicate { predicate: String },
    InvalidIdentifier { value: String },
    InvalidValue { message: String },
    NumericOverflow { value: String },
    LimitExceeded { limit: EntityLootLimit, actual: u64 },
}

impl fmt::Display for EntityLootCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "entity loot table {} at {}: ",
            self.table, self.path
        )?;
        match &self.kind {
            EntityLootCompileErrorKind::MalformedJson { message } => {
                write!(formatter, "malformed JSON: {message}")
            }
            EntityLootCompileErrorKind::Expected { expected } => {
                write!(formatter, "expected {expected}")
            }
            EntityLootCompileErrorKind::MissingField { field } => {
                write!(formatter, "missing required field {field:?}")
            }
            EntityLootCompileErrorKind::UnsupportedField { field } => {
                write!(formatter, "unsupported field {field:?}")
            }
            EntityLootCompileErrorKind::UnsupportedCondition { condition } => {
                write!(formatter, "unsupported condition {condition:?}")
            }
            EntityLootCompileErrorKind::UnsupportedFunction { function } => {
                write!(formatter, "unsupported function {function:?}")
            }
            EntityLootCompileErrorKind::UnsupportedEntry { entry } => {
                write!(formatter, "unsupported entry {entry:?}")
            }
            EntityLootCompileErrorKind::UnsupportedNumberProvider { provider } => {
                write!(formatter, "unsupported number provider {provider:?}")
            }
            EntityLootCompileErrorKind::UnsupportedEntityPredicate { predicate } => {
                write!(formatter, "unsupported entity predicate {predicate:?}")
            }
            EntityLootCompileErrorKind::InvalidIdentifier { value } => {
                write!(formatter, "invalid identifier {value:?}")
            }
            EntityLootCompileErrorKind::InvalidValue { message } => formatter.write_str(message),
            EntityLootCompileErrorKind::NumericOverflow { value } => {
                write!(
                    formatter,
                    "numeric value {value} exceeds the supported range"
                )
            }
            EntityLootCompileErrorKind::LimitExceeded { limit, actual } => write!(
                formatter,
                "{limit} limit exceeded: {actual} > {}",
                limit.maximum()
            ),
        }
    }
}

impl std::error::Error for EntityLootCompileError {}

#[derive(Debug, Error)]
pub enum EntityLootLoadError {
    #[error("entity loot catalog requires at least one configured root")]
    EmptyRoots,
    #[error("entity loot {limit} limit exceeded: {actual} > {maximum}")]
    LimitExceeded {
        limit: EntityLootLimit,
        actual: u64,
        maximum: u64,
    },
    #[error("configured entity loot root {root} was not loaded")]
    MissingRoot { root: Identifier },
    #[error("configured entity loot root {root} has non-entity table type {table_type}")]
    InvalidRootType {
        root: Identifier,
        table_type: String,
    },
    #[error("duplicate compiled entity loot table {table}")]
    DuplicateTable { table: Identifier },
    #[error("entity loot table {table} references missing table {referenced}")]
    MissingReference {
        table: Identifier,
        referenced: Identifier,
    },
    #[error("entity loot reference depth from {root} exceeded {maximum} while visiting {table}")]
    ReferenceDepthExceeded {
        root: Identifier,
        table: Identifier,
        maximum: usize,
    },
    #[error("entity loot reference cycle: {tables:?}")]
    ReferenceCycle { tables: Vec<Identifier> },
    #[error("filesystem error reading entity loot resource {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to compile entity loot resource {path}: {source}")]
    Compile {
        path: PathBuf,
        #[source]
        source: EntityLootCompileError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EntityLootContextError {
    #[error("player loot attribution requires a living minecraft:player, got {entity_type}")]
    InvalidPlayerAttribution { entity_type: Identifier },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EntityLootEvaluationError {
    #[error("unknown entity loot table {table}")]
    UnknownTable { table: Identifier },
    #[error("entity loot table {table} is a closure dependency, not a configured entity root")]
    NotConfiguredRoot { table: Identifier },
    #[error("entity loot table random sequence mismatch: expected {expected:?}, got {actual:?}")]
    RandomSequenceMismatch {
        expected: Option<Identifier>,
        actual: Option<Identifier>,
    },
    #[error("entity loot context has no binding for item tag {tag}")]
    MissingItemTag { tag: Identifier },
    #[error("invalid entity loot context: {message}")]
    InvalidContext { message: String },
    #[error("entity loot arithmetic overflow while {operation}")]
    ArithmeticOverflow { operation: &'static str },
    #[error("entity loot {limit} limit exceeded: {actual} > {maximum}")]
    LimitExceeded {
        limit: EntityLootLimit,
        actual: u64,
        maximum: u64,
    },
    #[error("compiled entity loot catalog invariant failed: {message}")]
    CatalogInvariant { message: String },
}

impl EntityLootEvaluationError {
    pub(super) fn limit(limit: EntityLootLimit, actual: u64) -> Self {
        Self::LimitExceeded {
            limit,
            actual,
            maximum: limit.maximum(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityLootFlags {
    pub is_baby: bool,
    pub is_on_fire: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLootEntity {
    pub entity_type: Identifier,
    pub is_living: bool,
    pub components: BTreeMap<Identifier, String>,
    pub flags: EntityLootFlags,
    pub mainhand: Option<LootContextItem>,
    pub active_enchantments: LootEnchantments,
    pub vehicle: Option<Box<EntityLootEntity>>,
    pub sheep_sheared: Option<bool>,
    pub slime_size: Option<i32>,
    pub raider_is_captain: Option<bool>,
}

impl EntityLootEntity {
    #[must_use]
    pub fn new(entity_type: Identifier) -> Self {
        Self {
            entity_type,
            is_living: true,
            components: BTreeMap::new(),
            flags: EntityLootFlags::default(),
            mainhand: None,
            active_enchantments: LootEnchantments::default(),
            vehicle: None,
            sheep_sheared: None,
            slime_size: None,
            raider_is_captain: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityLootAttack {
    None,
    Direct(EntityLootEntity),
    Indirect {
        source: Option<EntityLootEntity>,
        direct: EntityLootEntity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLootPlayerAttribution {
    player: EntityLootEntity,
}

impl EntityLootPlayerAttribution {
    pub fn try_new(player: EntityLootEntity) -> Result<Self, EntityLootContextError> {
        if !player.is_living || player.entity_type.as_str() != "minecraft:player" {
            return Err(EntityLootContextError::InvalidPlayerAttribution {
                entity_type: player.entity_type.clone(),
            });
        }
        Ok(Self { player })
    }

    #[must_use]
    pub fn player(&self) -> &EntityLootEntity {
        &self.player
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityDeathCause {
    tags: BTreeSet<Identifier>,
    attack: EntityLootAttack,
    player_attribution: Option<EntityLootPlayerAttribution>,
}

impl EntityDeathCause {
    #[must_use]
    pub fn new(
        tags: BTreeSet<Identifier>,
        attack: EntityLootAttack,
        player_attribution: Option<EntityLootPlayerAttribution>,
    ) -> Self {
        Self {
            tags,
            attack,
            player_attribution,
        }
    }

    #[must_use]
    pub fn tags(&self) -> &BTreeSet<Identifier> {
        &self.tags
    }

    #[must_use]
    pub fn source_attacker(&self) -> Option<&EntityLootEntity> {
        match &self.attack {
            EntityLootAttack::None => None,
            EntityLootAttack::Direct(attacker) => Some(attacker),
            EntityLootAttack::Indirect { source, .. } => source.as_ref(),
        }
    }

    #[must_use]
    pub fn direct_attacker(&self) -> Option<&EntityLootEntity> {
        match &self.attack {
            EntityLootAttack::None => None,
            EntityLootAttack::Direct(attacker) => Some(attacker),
            EntityLootAttack::Indirect { direct, .. } => Some(direct),
        }
    }

    #[must_use]
    pub fn attributed_player(&self) -> Option<&EntityLootEntity> {
        self.player_attribution
            .as_ref()
            .map(EntityLootPlayerAttribution::player)
    }

    #[must_use]
    pub fn is_direct(&self) -> bool {
        !matches!(self.attack, EntityLootAttack::Indirect { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLootSmeltingRecipe {
    pub output: Identifier,
    pub output_count: u32,
    pub max_stack_size: u32,
}

pub trait EntityLootTagLookup {
    fn item_tag(&self, tag: &Identifier) -> Option<&[Identifier]>;

    fn entity_type_in_tag(&self, tag: &Identifier, entity_type: &Identifier) -> bool;

    fn enchantment_in_tag(&self, tag: &Identifier, enchantment: &Identifier) -> bool;
}

pub trait EntityLootRecipeLookup {
    fn smelting_recipe(&self, input: &Identifier) -> Option<&EntityLootSmeltingRecipe>;
}

struct EmptyTagLookup;

impl EntityLootTagLookup for EmptyTagLookup {
    fn item_tag(&self, _tag: &Identifier) -> Option<&[Identifier]> {
        None
    }

    fn entity_type_in_tag(&self, _tag: &Identifier, _entity_type: &Identifier) -> bool {
        false
    }

    fn enchantment_in_tag(&self, _tag: &Identifier, _enchantment: &Identifier) -> bool {
        false
    }
}

struct EmptyRecipeLookup;

impl EntityLootRecipeLookup for EmptyRecipeLookup {
    fn smelting_recipe(&self, _input: &Identifier) -> Option<&EntityLootSmeltingRecipe> {
        None
    }
}

static EMPTY_TAGS: EmptyTagLookup = EmptyTagLookup;
static EMPTY_RECIPES: EmptyRecipeLookup = EmptyRecipeLookup;

pub struct EntityLootContext<'a> {
    pub this_entity: EntityLootEntity,
    pub origin: [f64; 3],
    pub cause: EntityDeathCause,
    pub luck: f32,
    random: LootRandomBinding,
    pub(super) tags: &'a dyn EntityLootTagLookup,
    pub(super) recipes: &'a dyn EntityLootRecipeLookup,
}

impl EntityLootContext<'static> {
    #[must_use]
    pub fn new(
        this_entity: EntityLootEntity,
        cause: EntityDeathCause,
        random: LootRandomBinding,
    ) -> Self {
        Self {
            this_entity,
            origin: [0.0; 3],
            cause,
            luck: 0.0,
            random,
            tags: &EMPTY_TAGS,
            recipes: &EMPTY_RECIPES,
        }
    }
}

impl EntityLootContext<'_> {
    #[must_use]
    pub fn with_lookups<'a>(
        self,
        tags: &'a dyn EntityLootTagLookup,
        recipes: &'a dyn EntityLootRecipeLookup,
    ) -> EntityLootContext<'a> {
        EntityLootContext {
            this_entity: self.this_entity,
            origin: self.origin,
            cause: self.cause,
            luck: self.luck,
            random: self.random,
            tags,
            recipes,
        }
    }

    #[must_use]
    pub fn random_binding(&self) -> &LootRandomBinding {
        &self.random
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityLootComponents {
    pub potion: Option<Identifier>,
    pub ominous_bottle_amplifier: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLootStack {
    pub item: Identifier,
    pub count: u32,
    pub components: EntityLootComponents,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLootInventory {
    pub root_count: usize,
    pub table_count: usize,
    pub reference_count: usize,
    pub table_types: BTreeSet<String>,
    pub condition_families: BTreeSet<String>,
    pub function_families: BTreeSet<String>,
    pub entry_families: BTreeSet<String>,
    pub number_provider_families: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LootTableType {
    Entity,
    Fishing,
}

impl LootTableType {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "minecraft:entity",
            Self::Fishing => "minecraft:fishing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledEntityLootTable {
    pub(super) id: Identifier,
    pub(super) table_type: LootTableType,
    pub(super) random_sequence: Option<Identifier>,
    pub(super) pools: Vec<LootPool>,
    pub(super) functions: Vec<LootFunction>,
    pub(super) condition_families: BTreeSet<String>,
    pub(super) function_families: BTreeSet<String>,
    pub(super) entry_families: BTreeSet<String>,
    pub(super) number_provider_families: BTreeSet<String>,
    pub(super) references: BTreeSet<Identifier>,
}

impl CompiledEntityLootTable {
    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.id
    }

    #[must_use]
    pub fn random_sequence(&self) -> Option<&Identifier> {
        self.random_sequence.as_ref()
    }
}

#[derive(Debug, Clone, Default)]
pub struct EntityLootCatalog {
    pub(super) roots: BTreeSet<Identifier>,
    pub(super) tables: BTreeMap<Identifier, CompiledEntityLootTable>,
}

impl EntityLootCatalog {
    #[must_use]
    pub fn random_sequence(&self, table: &Identifier) -> Option<&Identifier> {
        self.roots
            .contains(table)
            .then(|| self.tables.get(table))
            .flatten()
            .and_then(CompiledEntityLootTable::random_sequence)
    }
}

#[derive(Debug, Clone)]
pub(super) struct LootPool {
    pub(super) conditions: Vec<LootCondition>,
    pub(super) functions: Vec<LootFunction>,
    pub(super) rolls: NumberProvider,
    pub(super) bonus_rolls: NumberProvider,
    pub(super) entries: Vec<LootEntry>,
}

#[derive(Debug, Clone)]
pub(super) enum LootEntry {
    Item {
        singleton: SingletonEntry,
        item: Identifier,
    },
    Table {
        singleton: SingletonEntry,
        table: Identifier,
    },
    Empty {
        singleton: SingletonEntry,
    },
    Tag {
        singleton: SingletonEntry,
        tag: Identifier,
        expand: bool,
    },
    Alternatives {
        conditions: Vec<LootCondition>,
        children: Vec<LootEntry>,
    },
}

impl LootEntry {
    pub(super) fn singleton(&self) -> Option<&SingletonEntry> {
        match self {
            Self::Item { singleton, .. }
            | Self::Table { singleton, .. }
            | Self::Empty { singleton }
            | Self::Tag { singleton, .. } => Some(singleton),
            Self::Alternatives { .. } => None,
        }
    }

    pub(super) fn static_candidate_count(&self) -> Option<usize> {
        match self {
            Self::Tag { expand: true, .. } => None,
            Self::Alternatives { children, .. } => children
                .iter()
                .map(Self::static_candidate_count)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .max(),
            _ => Some(1),
        }
    }

    pub(super) fn static_expanded_weight(&self) -> Option<i32> {
        match self {
            Self::Tag { expand: true, .. } => None,
            Self::Alternatives { children, .. } => children
                .iter()
                .map(Self::static_expanded_weight)
                .collect::<Option<Vec<_>>>()?
                .into_iter()
                .max(),
            _ => Some(self.singleton()?.weight.max(0)),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SingletonEntry {
    pub(super) weight: i32,
    pub(super) quality: i32,
    pub(super) conditions: Vec<LootCondition>,
    pub(super) functions: Vec<LootFunction>,
}

#[derive(Debug, Clone)]
pub(super) enum LootCondition {
    AnyOf(Vec<LootCondition>),
    DamageSource(DamageSourcePredicate),
    EntityProperties {
        target: EntityTarget,
        predicate: EntityPredicate,
    },
    Inverted(Box<LootCondition>),
    KilledByPlayer,
    RandomChance(NumberProvider),
    RandomChanceWithEnchantedBonus {
        unenchanted_chance: f64,
        base: f64,
        per_level_above_first: f64,
        enchantment: Identifier,
    },
}

#[derive(Debug, Clone, Copy)]
pub(super) enum EntityTarget {
    This,
    Attacker,
    DirectAttacker,
    AttackingPlayer,
}

#[derive(Debug, Clone, Default)]
pub(super) struct EntityPredicate {
    pub(super) entity_type: Option<TaggedIdentifier>,
    pub(super) components: BTreeMap<Identifier, String>,
    pub(super) is_baby: Option<bool>,
    pub(super) is_on_fire: Option<bool>,
    pub(super) mainhand_enchantments: Option<Vec<TaggedIdentifier>>,
    pub(super) vehicle: Option<Box<EntityPredicate>>,
    pub(super) type_specific: Option<TypeSpecificPredicate>,
}

#[derive(Debug, Clone)]
pub(super) enum TypeSpecificPredicate {
    Sheep { sheared: bool },
    Slime { size: IntRange },
    Raider { is_captain: bool },
}

#[derive(Debug, Clone, Copy)]
pub(super) struct IntRange {
    pub(super) min: i32,
    pub(super) max: i32,
}

#[derive(Debug, Clone)]
pub(super) enum TaggedIdentifier {
    Exact(Identifier),
    Tag(Identifier),
}

#[derive(Debug, Clone, Default)]
pub(super) struct DamageSourcePredicate {
    pub(super) tags: Vec<(Identifier, bool)>,
    pub(super) direct_entity: Option<EntityPredicate>,
    pub(super) source_entity: Option<EntityPredicate>,
    pub(super) is_direct: Option<bool>,
}

#[derive(Debug, Clone)]
pub(super) enum LootFunction {
    SetCount {
        conditions: Vec<LootCondition>,
        count: NumberProvider,
        add: bool,
    },
    EnchantedCountIncrease {
        conditions: Vec<LootCondition>,
        enchantment: Identifier,
        count: NumberProvider,
        limit: Option<i32>,
    },
    FurnaceSmelt {
        conditions: Vec<LootCondition>,
        use_input_count: bool,
    },
    SetPotion {
        conditions: Vec<LootCondition>,
        potion: Identifier,
    },
    SetOminousBottleAmplifier {
        conditions: Vec<LootCondition>,
        amplifier: NumberProvider,
    },
}

#[derive(Debug, Clone)]
pub(super) enum NumberProvider {
    Constant(f64),
    Uniform { min: f64, max: f64 },
}

impl NumberProvider {
    pub(super) fn integer_upper_bound(&self) -> i32 {
        let bound = match self {
            Self::Constant(value) => *value,
            Self::Uniform { max, .. } => *max,
        };
        let bound = bound.floor();
        if bound.is_finite() && bound >= f64::from(i32::MIN) && bound <= f64::from(i32::MAX) {
            bound as i32
        } else {
            i32::MAX
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct WorkingStack {
    pub(super) item: Identifier,
    pub(super) count: i64,
    pub(super) components: EntityLootComponents,
}

impl WorkingStack {
    pub(super) fn one(item: Identifier) -> Self {
        Self {
            item,
            count: 1,
            components: EntityLootComponents::default(),
        }
    }
}
