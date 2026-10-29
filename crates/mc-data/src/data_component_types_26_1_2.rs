//! Built-in component IDs are DataComponents registration order, not the
//! alphabetical IDs of dynamic Configuration registries. All 122 entries come
//! from the pinned 26.3 registry report.
use crate::Identifier;

pub const COUNT: usize = 122;
/// Components with a lossless native NBT and 26.3 stream codec. Command
/// admission and wire emission share this list so unsupported patches fail.
pub const ITEM_COMPONENT_CODECS: &[(&str, i32)] = &[
    ("minecraft:custom_data", 0),
    ("minecraft:max_damage", 2),
    ("minecraft:damage", 3),
    ("minecraft:unbreakable", 4),
    ("minecraft:custom_name", 6),
    ("minecraft:item_name", 9),
    ("minecraft:item_model", 10),
    ("minecraft:lore", 11),
    ("minecraft:rarity", 12),
    ("minecraft:enchantments", 13),
    ("minecraft:tooltip_display", 18),
    ("minecraft:repair_cost", 19),
    ("minecraft:enchantment_glint_override", 21),
    ("minecraft:use_remainder", 25),
    ("minecraft:death_protection", 36),
    ("minecraft:stored_enchantments", 45),
    ("minecraft:dyed_color", 47),
    ("minecraft:map_id", 48),
    ("minecraft:charged_projectiles", 51),
    ("minecraft:bundle_contents", 52),
    ("minecraft:potion_contents", 53),
    ("minecraft:suspicious_stew_effects", 55),
    ("minecraft:writable_book_content", 56),
    ("minecraft:written_book_content", 57),
    ("minecraft:trim", 58),
    ("minecraft:ominous_bottle_amplifier", 65),
    ("minecraft:fireworks", 71),
    ("minecraft:profile", 72),
    ("minecraft:note_block_sound", 73),
    ("minecraft:banner_patterns", 74),
    ("minecraft:base_color", 75),
    ("minecraft:pot_decorations", 76),
    ("minecraft:container", 77),
    ("minecraft:block_state", 78),
    ("minecraft:bees", 79),
    ("minecraft:lock", 81),
    ("minecraft:container_loot", 82),
    ("minecraft:attack_animation", 40),
    ("minecraft:interact_animation", 41),
    ("minecraft:block_transformer", 43),
    ("minecraft:villager_food", 44),
    ("minecraft:sulfur_cube_content", 80),
    ("minecraft:compostable", 84),
    ("minecraft:cooking_fuel", 85),
    ("minecraft:brewing_fuel", 86),
    ("minecraft:mob_visibility", 87),
    ("minecraft:provides_pottery_pattern", 117),
    ("minecraft:sign_text_front", 118),
    ("minecraft:sign_text_back", 119),
    ("minecraft:waxed", 120),
    ("minecraft:cushion/color", 121),
];
/// Wire ids of the codec-backed components, named so encoders never carry bare numbers.
pub const CUSTOM_DATA: i32 = 0;
pub const MAX_DAMAGE: i32 = 2;
pub const DAMAGE: i32 = 3;
pub const UNBREAKABLE: i32 = 4;
pub const CUSTOM_NAME: i32 = 6;
pub const ITEM_NAME: i32 = 9;
pub const ITEM_MODEL: i32 = 10;
pub const LORE: i32 = 11;
pub const RARITY: i32 = 12;
pub const ENCHANTMENTS: i32 = 13;
pub const TOOLTIP_DISPLAY: i32 = 18;
pub const REPAIR_COST: i32 = 19;
pub const ENCHANTMENT_GLINT_OVERRIDE: i32 = 21;
pub const USE_REMAINDER: i32 = 25;
pub const DEATH_PROTECTION: i32 = 36;
pub const STORED_ENCHANTMENTS: i32 = 45;
pub const DYED_COLOR: i32 = 47;
pub const MAP_ID: i32 = 48;
pub const CHARGED_PROJECTILES: i32 = 51;
pub const BUNDLE_CONTENTS: i32 = 52;
pub const POTION_CONTENTS: i32 = 53;
pub const SUSPICIOUS_STEW_EFFECTS: i32 = 55;
pub const WRITABLE_BOOK_CONTENT: i32 = 56;
pub const WRITTEN_BOOK_CONTENT: i32 = 57;
pub const TRIM: i32 = 58;
pub const OMINOUS_BOTTLE_AMPLIFIER: i32 = 65;
pub const FIREWORKS: i32 = 71;
pub const PROFILE: i32 = 72;
pub const NOTE_BLOCK_SOUND: i32 = 73;
pub const BANNER_PATTERNS: i32 = 74;
pub const BASE_COLOR: i32 = 75;
pub const POT_DECORATIONS: i32 = 76;
pub const CONTAINER: i32 = 77;
pub const BLOCK_STATE: i32 = 78;
pub const BEES: i32 = 79;
pub const LOCK: i32 = 81;
pub const CONTAINER_LOOT: i32 = 82;
pub const ATTACK_ANIMATION: i32 = 40;
pub const INTERACT_ANIMATION: i32 = 41;
pub const BLOCK_TRANSFORMER: i32 = 43;
pub const VILLAGER_FOOD: i32 = 44;
pub const SULFUR_CUBE_CONTENT: i32 = 80;
pub const COMPOSTABLE: i32 = 84;
pub const COOKING_FUEL: i32 = 85;
pub const BREWING_FUEL: i32 = 86;
pub const MOB_VISIBILITY: i32 = 87;
pub const PROVIDES_POTTERY_PATTERN: i32 = 117;
pub const SIGN_TEXT_FRONT: i32 = 118;
pub const SIGN_TEXT_BACK: i32 = 119;
pub const WAXED: i32 = 120;
pub const CUSHION_COLOR: i32 = 121;
const BY_NAME: &[(&str, u32)] = &[
    ("minecraft:additional_trade_cost", 42),
    ("minecraft:attack_animation", 40),
    ("minecraft:attack_range", 30),
    ("minecraft:attribute_modifiers", 16),
    ("minecraft:axolotl/variant", 111),
    ("minecraft:banner_patterns", 74),
    ("minecraft:base_color", 75),
    ("minecraft:bees", 79),
    ("minecraft:block_entity_data", 62),
    ("minecraft:block_state", 78),
    ("minecraft:block_transformer", 43),
    ("minecraft:blocks_attacks", 37),
    ("minecraft:break_sound", 83),
    ("minecraft:brewing_fuel", 86),
    ("minecraft:bucket_entity_data", 61),
    ("minecraft:bundle_contents", 52),
    ("minecraft:can_break", 15),
    ("minecraft:can_place_on", 14),
    ("minecraft:cat/collar", 114),
    ("minecraft:cat/sound_variant", 113),
    ("minecraft:cat/variant", 112),
    ("minecraft:charged_projectiles", 51),
    ("minecraft:chicken/sound_variant", 105),
    ("minecraft:chicken/variant", 104),
    ("minecraft:compostable", 84),
    ("minecraft:consumable", 24),
    ("minecraft:container", 77),
    ("minecraft:container_loot", 82),
    ("minecraft:cooking_fuel", 85),
    ("minecraft:cow/sound_variant", 103),
    ("minecraft:cow/variant", 102),
    ("minecraft:creative_slot_lock", 20),
    ("minecraft:cushion/color", 121),
    ("minecraft:custom_data", 0),
    ("minecraft:custom_model_data", 17),
    ("minecraft:custom_name", 6),
    ("minecraft:damage", 3),
    ("minecraft:damage_resistant", 27),
    ("minecraft:damage_type", 8),
    ("minecraft:death_protection", 36),
    ("minecraft:debug_stick_state", 59),
    ("minecraft:dye", 46),
    ("minecraft:dyed_color", 47),
    ("minecraft:enchantable", 31),
    ("minecraft:enchantment_glint_override", 21),
    ("minecraft:enchantments", 13),
    ("minecraft:entity_data", 60),
    ("minecraft:equippable", 32),
    ("minecraft:firework_explosion", 70),
    ("minecraft:fireworks", 71),
    ("minecraft:food", 23),
    ("minecraft:fox/variant", 92),
    ("minecraft:frog/variant", 107),
    ("minecraft:glider", 34),
    ("minecraft:horse/variant", 108),
    ("minecraft:instrument", 63),
    ("minecraft:intangible_projectile", 22),
    ("minecraft:interact_animation", 41),
    ("minecraft:item_model", 10),
    ("minecraft:item_name", 9),
    ("minecraft:jukebox_playable", 66),
    ("minecraft:kinetic_weapon", 39),
    ("minecraft:llama/variant", 110),
    ("minecraft:lock", 81),
    ("minecraft:lodestone_tracker", 69),
    ("minecraft:lore", 11),
    ("minecraft:map_decorations", 49),
    ("minecraft:map_id", 48),
    ("minecraft:map_post_processing", 50),
    ("minecraft:max_damage", 2),
    ("minecraft:max_stack_size", 1),
    ("minecraft:minimum_attack_charge", 7),
    ("minecraft:mob_visibility", 87),
    ("minecraft:mooshroom/variant", 98),
    ("minecraft:note_block_sound", 73),
    ("minecraft:ominous_bottle_amplifier", 65),
    ("minecraft:painting/variant", 109),
    ("minecraft:parrot/variant", 94),
    ("minecraft:piercing_weapon", 38),
    ("minecraft:pig/sound_variant", 101),
    ("minecraft:pig/variant", 100),
    ("minecraft:pot_decorations", 76),
    ("minecraft:potion_contents", 53),
    ("minecraft:potion_duration_scale", 54),
    ("minecraft:profile", 72),
    ("minecraft:provides_banner_patterns", 67),
    ("minecraft:provides_pottery_pattern", 117),
    ("minecraft:provides_trim_material", 64),
    ("minecraft:rabbit/variant", 99),
    ("minecraft:rarity", 12),
    ("minecraft:recipes", 68),
    ("minecraft:repair_cost", 19),
    ("minecraft:repairable", 33),
    ("minecraft:salmon/size", 93),
    ("minecraft:sheep/color", 115),
    ("minecraft:shulker/color", 116),
    ("minecraft:sign_text_back", 119),
    ("minecraft:sign_text_front", 118),
    ("minecraft:stored_enchantments", 45),
    ("minecraft:sulfur_cube_content", 80),
    ("minecraft:suspicious_stew_effects", 55),
    ("minecraft:tool", 28),
    ("minecraft:tooltip_display", 18),
    ("minecraft:tooltip_style", 35),
    ("minecraft:trim", 58),
    ("minecraft:tropical_fish/base_color", 96),
    ("minecraft:tropical_fish/pattern", 95),
    ("minecraft:tropical_fish/pattern_color", 97),
    ("minecraft:unbreakable", 4),
    ("minecraft:use_cooldown", 26),
    ("minecraft:use_effects", 5),
    ("minecraft:use_remainder", 25),
    ("minecraft:villager/variant", 88),
    ("minecraft:villager_food", 44),
    ("minecraft:waxed", 120),
    ("minecraft:weapon", 29),
    ("minecraft:wolf/collar", 91),
    ("minecraft:wolf/sound_variant", 90),
    ("minecraft:wolf/variant", 89),
    ("minecraft:writable_book_content", 56),
    ("minecraft:written_book_content", 57),
    ("minecraft:zombie_nautilus/variant", 106),
];
const BY_ID: [&str; COUNT] = {
    let mut names = [""; COUNT];
    let mut index = 0;
    while index < BY_NAME.len() {
        names[BY_NAME[index].1 as usize] = BY_NAME[index].0;
        index += 1;
    }
    names
};
#[must_use]
pub fn id(name: &Identifier) -> Option<u32> {
    BY_NAME
        .binary_search_by(|(entry, _)| entry.cmp(&name.as_str()))
        .ok()
        .map(|index| BY_NAME[index].1)
}
#[must_use]
pub fn name(id: u32) -> Option<&'static str> {
    BY_ID.get(id as usize).copied()
}
#[cfg(test)]
#[path = "data_component_types_26_1_2_tests.rs"]
mod tests;
