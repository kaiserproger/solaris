use std::sync::Arc;

use crate::{Identifier, ItemEnchantment};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StewEffect {
    pub id: Identifier,
    #[serde(default = "default_stew_duration")]
    pub duration: i32,
}

const fn default_stew_duration() -> i32 {
    160
}

pub const STEW_EFFECTS_COMPONENT: &str = "minecraft:suspicious_stew_effects";

pub fn decode_stew_effects(tag: &mc_nbt::Tag) -> Result<Vec<StewEffect>, &'static str> {
    use mc_nbt::Tag;
    let Tag::List(list) = tag else {
        return Err("stew effects must be a list");
    };
    list.elements
        .iter()
        .map(|entry| {
            let Tag::Compound(fields) = entry else {
                return Err("stew effect must be a compound");
            };
            let Some(Tag::String(name)) = fields
                .iter()
                .find(|(name, _)| name == "id")
                .map(|(_, tag)| tag)
            else {
                return Err("stew effect id is missing");
            };
            let id = Identifier::parse(name.clone()).map_err(|_| "invalid stew effect id")?;
            if crate::mob_effects_26_1_2::MobEffect::from_name(id.as_str()).is_none() {
                return Err("unknown stew effect id");
            }
            let duration = match fields
                .iter()
                .find(|(name, _)| name == "duration")
                .map(|(_, tag)| tag)
            {
                Some(Tag::Int(duration)) => *duration,
                _ => default_stew_duration(),
            };
            Ok(StewEffect { id, duration })
        })
        .collect()
}

#[must_use]
pub fn encode_stew_effects(effects: &[StewEffect]) -> mc_nbt::Tag {
    use mc_nbt::Tag;
    Tag::List(mc_nbt::ListTag {
        element_type: mc_nbt::tag_type::COMPOUND,
        elements: effects
            .iter()
            .map(|effect| {
                Tag::Compound(vec![
                    ("id".into(), Tag::String(effect.id.as_str().into())),
                    ("duration".into(), Tag::Int(effect.duration)),
                ])
            })
            .collect(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemStack {
    pub count: i32,
    pub item_id: u32,
    pub damage: Option<i32>,
    pub enchantments: Vec<ItemEnchantment>,
    pub custom_name: Option<String>,
    pub item_model: Option<Arc<Identifier>>,
    pub stew_effects: Vec<StewEffect>,
}

impl ItemStack {
    pub const EMPTY: ItemStack = ItemStack {
        count: 0,
        item_id: 0,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }

    #[must_use]
    pub fn new(item_id: u32, count: i32) -> Self {
        Self {
            count,
            item_id,
            damage: None,
            enchantments: Vec::new(),
            custom_name: None,
            item_model: None,
            stew_effects: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_damage(mut self, damage: i32) -> Self {
        self.damage = Some(damage.max(0));
        self
    }

    #[must_use]
    pub fn with_enchantment(mut self, id: Identifier, level: i32) -> Self {
        self.enchantments.retain(|enchantment| enchantment.id != id);
        self.enchantments.push(ItemEnchantment { id, level });
        self.enchantments
            .sort_unstable_by(|left, right| left.id.cmp(&right.id));
        self
    }

    #[must_use]
    pub fn with_custom_name(mut self, name: impl Into<String>) -> Self {
        self.custom_name = Some(name.into());
        self
    }

    #[must_use]
    pub fn with_item_model(mut self, model: Identifier) -> Self {
        self.item_model = Some(Arc::new(model));
        self
    }
}
