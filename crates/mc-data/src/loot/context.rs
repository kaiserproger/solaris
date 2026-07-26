use std::collections::BTreeMap;

use thiserror::Error;

use crate::Identifier;

pub const MAX_LOOT_ENCHANTMENT_LEVEL: u8 = 255;
pub const MAX_LOOT_CONTEXT_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum LootContextError {
    #[error("enchantment {enchantment} has invalid level {level}; expected 1..=255")]
    InvalidEnchantmentLevel { enchantment: Identifier, level: u32 },
    #[error("loot context has too many enchantments: {actual} > {maximum}")]
    TooManyEnchantments { actual: usize, maximum: usize },
    #[error("loot context contains duplicate enchantment {enchantment}")]
    DuplicateEnchantment { enchantment: Identifier },
    #[error("loot context cannot attach enchantments to an empty item")]
    EnchantmentsWithoutItem,
    #[error("loot context has too many block properties: {actual} > {maximum}")]
    TooManyBlockProperties { actual: usize, maximum: usize },
    #[error("loot context contains duplicate block property {property:?}")]
    DuplicateBlockProperty { property: String },
    #[error("block property {property:?} has contradictory values {first:?} and {second:?}")]
    ConflictingBlockProperty {
        property: String,
        first: String,
        second: String,
    },
    #[error("explosion radius must be finite and positive, got {radius}")]
    InvalidExplosionRadius { radius: f32 },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LootEnchantments(BTreeMap<Identifier, u8>);

impl LootEnchantments {
    pub fn try_from_levels(
        levels: impl IntoIterator<Item = (Identifier, u32)>,
    ) -> Result<Self, LootContextError> {
        let levels = levels.into_iter();
        let (minimum, _) = levels.size_hint();
        if minimum > MAX_LOOT_CONTEXT_ENTRIES {
            return Err(LootContextError::TooManyEnchantments {
                actual: minimum,
                maximum: MAX_LOOT_CONTEXT_ENTRIES,
            });
        }
        let mut enchantments = Self::default();
        for (enchantment, level) in levels {
            if enchantments.len() == MAX_LOOT_CONTEXT_ENTRIES
                && !enchantments.contains(&enchantment)
            {
                return Err(LootContextError::TooManyEnchantments {
                    actual: MAX_LOOT_CONTEXT_ENTRIES + 1,
                    maximum: MAX_LOOT_CONTEXT_ENTRIES,
                });
            }
            enchantments.try_insert(enchantment, level)?;
        }
        Ok(enchantments)
    }

    pub fn try_insert(
        &mut self,
        enchantment: Identifier,
        level: u32,
    ) -> Result<(), LootContextError> {
        let validated = u8::try_from(level)
            .ok()
            .filter(|level| *level > 0)
            .ok_or_else(|| LootContextError::InvalidEnchantmentLevel {
                enchantment: enchantment.clone(),
                level,
            })?;
        if self.0.contains_key(&enchantment) {
            return Err(LootContextError::DuplicateEnchantment { enchantment });
        }
        self.0.insert(enchantment, validated);
        Ok(())
    }

    #[must_use]
    pub fn level(&self, enchantment: &Identifier) -> u32 {
        self.0.get(enchantment).copied().map_or(0, u32::from)
    }

    #[must_use]
    pub fn contains(&self, enchantment: &Identifier) -> bool {
        self.0.contains_key(enchantment)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Identifier, u32)> {
        self.0
            .iter()
            .map(|(enchantment, level)| (enchantment, u32::from(*level)))
    }

    pub(crate) fn identifiers(&self) -> impl Iterator<Item = &Identifier> {
        self.0.keys()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LootBlockProperties(BTreeMap<String, String>);

impl LootBlockProperties {
    fn try_from_pairs(properties: &[(String, String)]) -> Result<Self, LootContextError> {
        if properties.len() > MAX_LOOT_CONTEXT_ENTRIES {
            return Err(LootContextError::TooManyBlockProperties {
                actual: properties.len(),
                maximum: MAX_LOOT_CONTEXT_ENTRIES,
            });
        }
        for (index, (property, value)) in properties.iter().enumerate() {
            if let Some((_, first)) = properties[..index]
                .iter()
                .find(|(candidate, _)| candidate == property)
            {
                if first == value {
                    return Err(LootContextError::DuplicateBlockProperty {
                        property: property.clone(),
                    });
                }
                return Err(LootContextError::ConflictingBlockProperty {
                    property: property.clone(),
                    first: first.clone(),
                    second: value.clone(),
                });
            }
        }
        Ok(Self(properties.iter().cloned().collect()))
    }

    fn get(&self, property: &str) -> Option<&str> {
        self.0.get(property).map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootRandomBinding {
    sequence: Option<Identifier>,
    seed: u64,
}

impl LootRandomBinding {
    #[must_use]
    pub fn new(sequence: Option<Identifier>, seed: u64) -> Self {
        Self { sequence, seed }
    }

    #[must_use]
    pub fn sequence(&self) -> Option<&Identifier> {
        self.sequence.as_ref()
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub(crate) fn random(&self) -> LootRandom {
        LootRandom::new(self.seed)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LootContextItem {
    item: Option<Identifier>,
    enchantments: LootEnchantments,
}

impl LootContextItem {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn new(item: Identifier) -> Self {
        Self {
            item: Some(item),
            enchantments: LootEnchantments::default(),
        }
    }

    pub fn try_with_enchantment(
        mut self,
        enchantment: Identifier,
        level: u32,
    ) -> Result<Self, LootContextError> {
        self.enchantments.try_insert(enchantment, level)?;
        Ok(self)
    }

    #[must_use]
    pub fn with_enchantments(mut self, enchantments: LootEnchantments) -> Self {
        self.enchantments = enchantments;
        self
    }

    #[must_use]
    pub fn item(&self) -> Option<&Identifier> {
        self.item.as_ref()
    }

    #[must_use]
    pub fn enchantments(&self) -> &LootEnchantments {
        &self.enchantments
    }

    #[must_use]
    pub fn enchantment_level(&self, enchantment: &Identifier) -> u32 {
        self.enchantments.level(enchantment)
    }

    #[must_use]
    pub fn silk_touch_level(&self) -> u32 {
        self.level_by_name("minecraft:silk_touch")
    }

    #[must_use]
    pub fn fortune_level(&self) -> u32 {
        self.level_by_name("minecraft:fortune")
    }

    #[must_use]
    pub fn looting_level(&self) -> u32 {
        self.level_by_name("minecraft:looting")
    }

    fn level_by_name(&self, enchantment: &str) -> u32 {
        self.enchantments
            .iter()
            .find_map(|(id, level)| (id.as_str() == enchantment).then_some(level))
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LootExplosion {
    radius: f32,
}

impl LootExplosion {
    pub fn try_new(radius: f32) -> Result<Self, LootContextError> {
        if !radius.is_finite() || radius <= 0.0 {
            return Err(LootContextError::InvalidExplosionRadius { radius });
        }
        Ok(Self { radius })
    }

    #[must_use]
    pub fn radius(self) -> f32 {
        self.radius
    }

    #[must_use]
    pub fn survives(self, roll: f32) -> bool {
        roll <= 1.0 / self.radius
    }
}

#[derive(Debug, Clone)]
pub struct BlockLootContext<'a> {
    block: &'a Identifier,
    properties: LootBlockProperties,
    tool: &'a LootContextItem,
    explosion: Option<LootExplosion>,
    random: LootRandomBinding,
}

impl<'a> BlockLootContext<'a> {
    pub fn try_new(
        block: &'a Identifier,
        properties: &[(String, String)],
        tool: &'a LootContextItem,
        random: LootRandomBinding,
    ) -> Result<Self, LootContextError> {
        if tool.item().is_none() && !tool.enchantments().is_empty() {
            return Err(LootContextError::EnchantmentsWithoutItem);
        }
        Ok(Self {
            block,
            properties: LootBlockProperties::try_from_pairs(properties)?,
            tool,
            explosion: None,
            random,
        })
    }

    #[must_use]
    pub fn with_explosion(mut self, explosion: LootExplosion) -> Self {
        self.explosion = Some(explosion);
        self
    }

    #[must_use]
    pub fn block(&self) -> &'a Identifier {
        self.block
    }

    pub(crate) fn property(&self, property: &str) -> Option<&str> {
        self.properties.get(property)
    }

    #[must_use]
    pub fn tool(&self) -> &'a LootContextItem {
        self.tool
    }

    #[must_use]
    pub fn explosion(&self) -> Option<LootExplosion> {
        self.explosion
    }

    #[must_use]
    pub fn random_binding(&self) -> &LootRandomBinding {
        &self.random
    }

    pub(crate) fn random(&self) -> LootRandom {
        self.random.random()
    }
}

pub(crate) struct LootRandom {
    state: u64,
}

impl LootRandom {
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    pub(crate) fn next_float(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / 16_777_216.0
    }
}
