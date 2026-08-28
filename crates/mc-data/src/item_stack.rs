use std::sync::Arc;

use crate::{Identifier, ItemEnchantment};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemStack {
    pub count: i32,
    pub item_id: u32,
    pub damage: Option<i32>,
    pub enchantments: Vec<ItemEnchantment>,
    pub custom_name: Option<String>,
    pub item_model: Option<Arc<Identifier>>,
}

impl ItemStack {
    pub const EMPTY: ItemStack = ItemStack {
        count: 0,
        item_id: 0,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_mutation_helpers_are_domain_data_not_wire_behaviour() {
        let enchantment = Identifier::parse("minecraft:sharpness").unwrap();
        let model = Identifier::parse("minecraft:diamond_sword").unwrap();
        let stack = ItemStack::new(7, 1)
            .with_damage(5)
            .with_enchantment(enchantment.clone(), 3)
            .with_enchantment(enchantment, 4)
            .with_custom_name("Blade")
            .with_item_model(model.clone());
        assert_eq!(stack.count, 1);
        assert_eq!(stack.item_id, 7);
        assert_eq!(stack.damage, Some(5));
        assert_eq!(stack.enchantments.len(), 1);
        assert_eq!(stack.enchantments[0].level, 4);
        assert_eq!(stack.custom_name.as_deref(), Some("Blade"));
        assert_eq!(stack.item_model.as_deref(), Some(&model));
        assert!(ItemStack::EMPTY.is_empty());
    }
}
