//! `/enchant` held-item validation for Java Edition 26.1.2.
//!
//! Source: `minecraft-patched-26.1.2.76-sources.jar`,
//! `net/minecraft/server/commands/EnchantCommand.java` (resource argument,
//! default level 1, integer minimum 0, maximum-level and compatibility checks),
//! `net/minecraft/world/item/enchantment/Enchantment.java::areCompatible`, and
//! `ItemEnchantments.java::Mutable.upgrade` (level 0 succeeds without mutation).
//! The complete enchantment list, maximum levels and supported item types below
//! come from `data/minecraft/enchantment/*.json`; item groups and exclusive sets
//! come from `data/minecraft/tags/{item/enchantable,enchantment/exclusive_set}/*.json`
//! in that same source archive. Command support is not table eligibility:
//! treasure enchantments work, but books are not universally supported items.

use mc_data::Identifier;
use mc_data::ItemEnchantment;

use crate::boundary::{ChangeSet, ConnectionId, EntityId};
use crate::command_execution::{
    CommandResult, entity_name, error, feedback, resolve_target, targets_label,
};
use crate::command_feedback::translated;
use crate::commands::{Command, CommandError, Target, target_of};
use crate::inventory::{ItemRules, PlayerInventory};
use crate::tick::TickState;

#[derive(Debug, Clone, PartialEq)]
pub struct EnchantCommand {
    pub target: Target,
    pub enchantment: Identifier,
    pub level: i32,
}

pub fn parse(args: &[&str]) -> Result<EnchantCommand, CommandError> {
    let (target, enchantment, level) = match args {
        [target, enchantment] => (*target, *enchantment, 1),
        [target, enchantment, level] => (
            *target,
            *enchantment,
            level
                .parse::<i32>()
                .ok()
                .filter(|level| *level >= 0)
                .ok_or(CommandError::Usage(
                    "enchant <targets> <enchantment> [<level>]",
                ))?,
        ),
        _ => {
            return Err(CommandError::Usage(
                "enchant <targets> <enchantment> [<level>]",
            ));
        }
    };
    let enchantment = Identifier::parse(enchantment)
        .map_err(|_| CommandError::Message("Invalid enchantment identifier".to_owned()))?;
    if definition(&enchantment).is_none() {
        return Err(CommandError::Message(format!(
            "Unknown enchantment: {enchantment}"
        )));
    }
    Ok(EnchantCommand {
        target: target_of(target)?,
        enchantment,
        level,
    })
}

/// Resolve and apply targets through the existing player/tick effects path.
pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &Command,
    enchant_command: &EnchantCommand,
    changes: &mut ChangeSet,
) -> CommandResult {
    use CommandResult::{Failed, Success};

    let targets = resolve_target(state, executor, &enchant_command.target);
    if targets.is_empty() {
        feedback(
            state,
            connection,
            "argument.entity.notfound.entity",
            Vec::new(),
        );
        return Failed;
    }
    let single = targets.len() == 1;
    let mut successful = Vec::new();
    for target in targets {
        let Some(player) = state.players.get(&target) else {
            if single {
                feedback(
                    state,
                    connection,
                    "commands.enchant.failed.itemless",
                    vec![entity_name(state, target)],
                );
                return Failed;
            }
            continue;
        };
        if let Err(message) = validate(
            &state.registry.item_rules(),
            &player.inventory,
            enchant_command,
        ) {
            if single {
                error(state, connection, message);
            }
            continue;
        }
        let effects = state
            .players
            .get_mut(&target)
            .expect("checked player exists")
            .apply_command(&state.registry.item_rules(), command);
        if let Some(target_connection) = state.connections.entity_connection(target) {
            crate::tick::apply_player_effects(state, target_connection, target, effects, changes);
        }
        successful.push(target);
    }
    if !successful.is_empty() {
        let enchantment = translated(
            format!(
                "enchantment.{}",
                enchant_command.enchantment.as_str().replace(':', ".")
            ),
            Vec::new(),
        );
        feedback(
            state,
            connection,
            if successful.len() == 1 {
                "commands.enchant.success.single"
            } else {
                "commands.enchant.success.multiple"
            },
            vec![enchantment, targets_label(state, &successful)],
        );
        return Success(successful.len() as i32);
    } else if !single {
        feedback(state, connection, "commands.enchant.failed", Vec::new());
    }
    Failed
}

/// Validate the resource and level before resolving or applying to targets.
/// Vanilla reports an excessive level even when the selected entity is itemless.
pub fn validate_level(command: &EnchantCommand) -> Result<(), String> {
    let enchantment = definition(&command.enchantment)
        .ok_or_else(|| format!("Unknown enchantment: {}", command.enchantment))?;
    if command.level < 0 {
        return Err("Enchantment level must be at least 0".to_owned());
    }
    if command.level > enchantment.max_level {
        return Err(format!(
            "Level {} is too high for {} (maximum {})",
            command.level, command.enchantment, enchantment.max_level,
        ));
    }
    Ok(())
}

/// Read-only preflight for the executor; failure never mutates the inventory.
pub fn validate(
    rules: &ItemRules<'_>,
    inventory: &PlayerInventory,
    command: &EnchantCommand,
) -> Result<(), String> {
    validate_level(command)?;
    let enchantment = definition(&command.enchantment).expect("validated enchantment");
    let stack = &inventory.slots[usize::from(inventory.selected.min(8))];
    if stack.is_empty() {
        return Err("Target is not holding an item".to_owned());
    }
    let item = rules
        .items
        .name_of(stack.item_id)
        .ok_or_else(|| "Held item is not in the item registry".to_owned())?;
    if !supports(rules, stack.item_id, item, enchantment.supported_tag) {
        return Err(format!(
            "{} cannot be applied to {item}",
            command.enchantment
        ));
    }
    for existing in &stack.enchantments {
        if existing.id == command.enchantment
            || definition(&existing.id).is_some_and(|other| incompatible(enchantment, other))
        {
            return Err(format!(
                "{} is incompatible with {} on {item}",
                command.enchantment, existing.id,
            ));
        }
    }
    Ok(())
}

/// Enchant only the selected main-hand stack, preserving every other component.
/// The caller owns inventory publication and target-specific chat feedback.
pub fn apply(
    rules: &ItemRules<'_>,
    inventory: &mut PlayerInventory,
    command: &EnchantCommand,
) -> Result<(), String> {
    validate(rules, inventory, command)?;
    let stack = &mut inventory.slots[usize::from(inventory.selected.min(8))];
    // ItemStack.enchant delegates to Mutable.upgrade: zero is a successful
    // command without a new enchantment. It does not remove existing enchants.
    if command.level > 0 {
        stack.enchantments.push(ItemEnchantment {
            id: command.enchantment.clone(),
            level: command.level,
        });
        stack
            .enchantments
            .sort_unstable_by(|left, right| left.id.cmp(&right.id));
    }
    Ok(())
}

struct EnchantmentDefinition {
    name: &'static str,
    max_level: i32,
    supported_tag: &'static str,
    exclusive_set: &'static str,
}

// (registry path, max_level, supported_items tag, exclusive_set tag).
// 43 entries, including the 26.1.2 spear enchantment lunge. The source
// enchantment JSON's supported_items is used, NOT its primary_items.
const DEFINITIONS: &[EnchantmentDefinition] = &[
    enchantment("aqua_affinity", 1, "minecraft:enchantable/head_armor", ""),
    enchantment(
        "bane_of_arthropods",
        5,
        "minecraft:enchantable/weapon",
        "damage",
    ),
    enchantment("binding_curse", 1, "minecraft:enchantable/equippable", ""),
    enchantment(
        "blast_protection",
        4,
        "minecraft:enchantable/armor",
        "armor",
    ),
    enchantment("breach", 4, "minecraft:enchantable/mace", "damage"),
    enchantment("channeling", 1, "minecraft:enchantable/trident", ""),
    enchantment("density", 5, "minecraft:enchantable/mace", "damage"),
    enchantment(
        "depth_strider",
        3,
        "minecraft:enchantable/foot_armor",
        "boots",
    ),
    enchantment("efficiency", 5, "minecraft:enchantable/mining", ""),
    enchantment("feather_falling", 4, "minecraft:enchantable/foot_armor", ""),
    enchantment("fire_aspect", 2, "minecraft:enchantable/fire_aspect", ""),
    enchantment("fire_protection", 4, "minecraft:enchantable/armor", "armor"),
    enchantment("flame", 1, "minecraft:enchantable/bow", ""),
    enchantment("fortune", 3, "minecraft:enchantable/mining_loot", "mining"),
    enchantment(
        "frost_walker",
        2,
        "minecraft:enchantable/foot_armor",
        "boots",
    ),
    enchantment("impaling", 5, "minecraft:enchantable/trident", "damage"),
    enchantment("infinity", 1, "minecraft:enchantable/bow", "bow"),
    enchantment("knockback", 2, "minecraft:enchantable/melee_weapon", ""),
    enchantment("looting", 3, "minecraft:enchantable/melee_weapon", ""),
    enchantment("loyalty", 3, "minecraft:enchantable/trident", ""),
    enchantment("luck_of_the_sea", 3, "minecraft:enchantable/fishing", ""),
    enchantment("lunge", 3, "minecraft:enchantable/lunge", ""),
    enchantment("lure", 3, "minecraft:enchantable/fishing", ""),
    enchantment("mending", 1, "minecraft:enchantable/durability", ""),
    enchantment("multishot", 1, "minecraft:enchantable/crossbow", "crossbow"),
    enchantment("piercing", 4, "minecraft:enchantable/crossbow", "crossbow"),
    enchantment("power", 5, "minecraft:enchantable/bow", ""),
    enchantment(
        "projectile_protection",
        4,
        "minecraft:enchantable/armor",
        "armor",
    ),
    enchantment("protection", 4, "minecraft:enchantable/armor", "armor"),
    enchantment("punch", 2, "minecraft:enchantable/bow", ""),
    enchantment("quick_charge", 3, "minecraft:enchantable/crossbow", ""),
    enchantment("respiration", 3, "minecraft:enchantable/head_armor", ""),
    enchantment("riptide", 3, "minecraft:enchantable/trident", "riptide"),
    enchantment(
        "sharpness",
        5,
        "minecraft:enchantable/sharp_weapon",
        "damage",
    ),
    enchantment(
        "silk_touch",
        1,
        "minecraft:enchantable/mining_loot",
        "mining",
    ),
    enchantment("smite", 5, "minecraft:enchantable/weapon", "damage"),
    enchantment("soul_speed", 3, "minecraft:enchantable/foot_armor", ""),
    enchantment("sweeping_edge", 3, "minecraft:enchantable/sweeping", ""),
    enchantment("swift_sneak", 3, "minecraft:enchantable/leg_armor", ""),
    enchantment("thorns", 3, "minecraft:enchantable/armor", ""),
    enchantment("unbreaking", 3, "minecraft:enchantable/durability", ""),
    enchantment("vanishing_curse", 1, "minecraft:enchantable/vanishing", ""),
    enchantment("wind_burst", 3, "minecraft:enchantable/mace", ""),
];

const fn enchantment(
    name: &'static str,
    max_level: i32,
    supported_tag: &'static str,
    exclusive_set: &'static str,
) -> EnchantmentDefinition {
    EnchantmentDefinition {
        name,
        max_level,
        supported_tag,
        exclusive_set,
    }
}

fn definition(id: &Identifier) -> Option<&'static EnchantmentDefinition> {
    if id.namespace() != "minecraft" {
        return None;
    }
    DEFINITIONS
        .binary_search_by(|definition| definition.name.cmp(id.path()))
        .ok()
        .map(|index| &DEFINITIONS[index])
}

fn incompatible(left: &EnchantmentDefinition, right: &EnchantmentDefinition) -> bool {
    // areCompatible checks BOTH exclusive sets. Mending's own set is empty,
    // and riptide excludes loyalty/channeling without excluding their pairing.
    excludes(left.exclusive_set, right.name) || excludes(right.exclusive_set, left.name)
}

fn excludes(set: &str, name: &str) -> bool {
    match set {
        "armor" => matches!(
            name,
            "protection" | "blast_protection" | "fire_protection" | "projectile_protection"
        ),
        "boots" => matches!(name, "frost_walker" | "depth_strider"),
        "bow" => matches!(name, "infinity" | "mending"),
        "crossbow" => matches!(name, "multishot" | "piercing"),
        "damage" => matches!(
            name,
            "sharpness" | "smite" | "bane_of_arthropods" | "impaling" | "density" | "breach"
        ),
        "mining" => matches!(name, "fortune" | "silk_touch"),
        "riptide" => matches!(name, "loyalty" | "channeling"),
        _ => false,
    }
}

fn supports(rules: &ItemRules<'_>, raw_id: u32, item: &Identifier, tag: &str) -> bool {
    // Reuse loaded enchantable tags when populated. Embedded client fallback
    // tags are intentionally empty, so use the exact vanilla tag definitions
    // below in that configuration rather than treating all items as supported.
    if let Some(tags) = rules.tags.registries.get("minecraft:item") {
        let loaded = tags.get(tag);
        if let Some(members) = loaded.filter(|members| !members.is_empty()) {
            return i32::try_from(raw_id).is_ok_and(|raw_id| members.contains(&raw_id));
        }
    }
    item.namespace() == "minecraft"
        && vanilla_supports(
            tag.strip_prefix("minecraft:enchantable/").unwrap_or(tag),
            item.path(),
        )
}

// Resolved from enchantable/*.json and its foot_armor, leg_armor, chest_armor,
// head_armor, swords, spears, axes, pickaxes, shovels, hoes and skulls references.
// Material whitelists ensure e.g. stone_helmet or modded_axe cannot match.
fn vanilla_supports(tag: &str, item: &str) -> bool {
    let (material, kind) = item.rsplit_once('_').unwrap_or(("", item));
    let armor_material = matches!(
        material,
        "leather" | "chainmail" | "copper" | "iron" | "golden" | "diamond" | "netherite"
    );
    let tool_material = matches!(
        material,
        "wooden" | "stone" | "copper" | "iron" | "golden" | "diamond" | "netherite"
    );
    let armor = armor_material && matches!(kind, "boots" | "leggings" | "chestplate" | "helmet")
        || item == "turtle_helmet";
    let melee = tool_material && matches!(kind, "sword" | "spear");
    let mining = tool_material && matches!(kind, "axe" | "pickaxe" | "shovel" | "hoe");
    let durable = armor
        || melee
        || mining
        || matches!(
            item,
            "elytra"
                | "shield"
                | "bow"
                | "crossbow"
                | "trident"
                | "flint_and_steel"
                | "shears"
                | "brush"
                | "fishing_rod"
                | "carrot_on_a_stick"
                | "warped_fungus_on_a_stick"
                | "mace"
        );
    let skull = matches!(
        item,
        "creeper_head"
            | "dragon_head"
            | "piglin_head"
            | "player_head"
            | "skeleton_skull"
            | "wither_skeleton_skull"
            | "zombie_head"
    );
    match tag {
        "armor" => armor,
        "foot_armor" => armor_material && kind == "boots",
        "leg_armor" => armor_material && kind == "leggings",
        "head_armor" => armor_material && kind == "helmet" || item == "turtle_helmet",
        "melee_weapon" => melee,
        "sharp_weapon" => melee || tool_material && kind == "axe",
        "weapon" => melee || tool_material && kind == "axe" || item == "mace",
        "fire_aspect" => melee || item == "mace",
        "sweeping" => tool_material && kind == "sword",
        "mining" => mining || item == "shears",
        "mining_loot" => mining,
        "durability" => durable,
        "equippable" => armor || item == "elytra" || skull || item == "carved_pumpkin",
        "vanishing" => durable || skull || matches!(item, "compass" | "carved_pumpkin"),
        "bow" => item == "bow",
        "crossbow" => item == "crossbow",
        "trident" => item == "trident",
        "fishing" => item == "fishing_rod",
        "mace" => item == "mace",
        "lunge" => tool_material && kind == "spear",
        _ => false,
    }
}

#[cfg(test)]
#[path = "command_enchant_execution_tests.rs"]
mod execution_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::item_components::ItemFactsTable;
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_data::tags::TagsData;

    fn id(name: &str) -> Identifier {
        Identifier::parse(name).unwrap()
    }

    struct Data {
        items: ItemRegistry,
        facts: ItemFactsTable,
        tags: TagsData,
    }

    impl Data {
        fn new() -> Self {
            let names = [
                "diamond_chestplate",
                "diamond_sword",
                "diamond_pickaxe",
                "bow",
                "trident",
                "stone",
                "book",
                "wooden_spear",
                "mace",
                "modded_axe",
            ];
            Self {
                items: ItemRegistry::from_report(
                    &names
                        .iter()
                        .enumerate()
                        .map(|(index, name)| ItemReport {
                            id: id(name),
                            protocol_id: index as u32 + 1,
                        })
                        .collect::<Vec<_>>(),
                ),
                facts: ItemFactsTable::default(),
                tags: TagsData::default(),
            }
        }

        fn rules(&self) -> ItemRules<'_> {
            ItemRules {
                items: &self.items,
                facts: &self.facts,
                tags: &self.tags,
                recipes: &[],
            }
        }

        fn holding(&self, item: &str) -> PlayerInventory {
            let mut inventory = PlayerInventory {
                selected: 4,
                ..Default::default()
            };
            inventory.slots[4] = mc_data::ItemStack::new(self.items.id_of(&id(item)).unwrap(), 1);
            inventory
        }
    }

    fn command(enchantment: &str, level: i32) -> EnchantCommand {
        EnchantCommand {
            target: Target::Executor,
            enchantment: id(enchantment),
            level,
        }
    }

    #[test]
    fn parses_default_level_explicit_level_and_vanilla_zero() {
        assert_eq!(
            parse(&["@s", "protection"]).unwrap(),
            command("protection", 1)
        );
        assert_eq!(
            parse(&["@s", "minecraft:protection", "4"]).unwrap(),
            command("protection", 4)
        );
        assert_eq!(parse(&["@s", "protection", "0"]).unwrap().level, 0);
        for args in [
            &["@s", "protection", "-1"][..],
            &["@s", "protection", "2147483648"],
            &["@s", "protection", "1", "extra"],
        ] {
            assert!(parse(args).is_err());
        }
    }

    #[test]
    fn valid_held_item_preserves_selected_slot_and_components() {
        let data = Data::new();
        let mut inventory = data.holding("diamond_chestplate");
        inventory.slots[4].custom_name = Some("Named armor".into());
        apply(&data.rules(), &mut inventory, &command("protection", 4)).unwrap();
        assert_eq!(
            inventory.slots[4].enchantments,
            vec![ItemEnchantment {
                id: id("protection"),
                level: 4
            }]
        );
        assert_eq!(
            inventory.slots[4].custom_name.as_deref(),
            Some("Named armor")
        );
        assert!(inventory.slots[0].is_empty());
    }

    #[test]
    fn empty_hand_is_rejected_without_mutation() {
        let data = Data::new();
        let mut inventory = PlayerInventory::default();
        let before = inventory.clone();
        assert!(
            apply(&data.rules(), &mut inventory, &command("protection", 1))
                .unwrap_err()
                .contains("not holding")
        );
        assert_eq!(inventory, before);
    }

    #[test]
    fn unknown_registry_ids_are_rejected_in_parse_and_apply() {
        let data = Data::new();
        for name in ["unknown", "other:protection"] {
            assert!(matches!(
                parse(&["@s", name]),
                Err(CommandError::Message(_))
            ));
            let mut inventory = data.holding("diamond_chestplate");
            assert!(apply(&data.rules(), &mut inventory, &command(name, 1)).is_err());
            assert!(inventory.slots[4].enchantments.is_empty());
        }
    }

    #[test]
    fn unsupported_items_and_books_are_not_universally_enchantable() {
        let data = Data::new();
        for item in ["stone", "book", "diamond_sword", "modded_axe"] {
            let mut inventory = data.holding(item);
            assert!(apply(&data.rules(), &mut inventory, &command("protection", 1)).is_err());
        }
        assert!(!vanilla_supports("mining", "modded_axe"));
        assert!(!vanilla_supports("armor", "stone_helmet"));
        assert!(!vanilla_supports("armor", "wolf_armor"));
    }

    #[test]
    fn exclusive_and_same_enchantments_are_rejected_without_mutation() {
        let data = Data::new();
        for (item, existing, requested) in [
            ("diamond_chestplate", "fire_protection", "protection"),
            ("diamond_pickaxe", "silk_touch", "fortune"),
            ("diamond_sword", "smite", "sharpness"),
            ("bow", "mending", "infinity"),
            ("bow", "infinity", "mending"),
            ("trident", "loyalty", "riptide"),
            ("trident", "riptide", "channeling"),
            ("diamond_chestplate", "protection", "protection"),
        ] {
            let mut inventory = data.holding(item);
            inventory.slots[4] = inventory.slots[4].clone().with_enchantment(id(existing), 1);
            let before = inventory.clone();
            let error = apply(&data.rules(), &mut inventory, &command(requested, 1)).unwrap_err();
            assert!(error.contains("incompatible"));
            assert_eq!(inventory, before);
        }
    }

    #[test]
    fn maxlevel_rejection_precedes_hand_validation() {
        let data = Data::new();
        let mut inventory = data.holding("diamond_chestplate");
        let before = inventory.clone();
        assert!(
            apply(&data.rules(), &mut inventory, &command("protection", 5))
                .unwrap_err()
                .contains("maximum 4")
        );
        assert_eq!(inventory, before);
        let mut empty = PlayerInventory::default();
        assert!(
            apply(&data.rules(), &mut empty, &command("protection", 5))
                .unwrap_err()
                .contains("maximum 4")
        );
    }

    #[test]
    fn valid_level_update_preserves_other_enchantment_levels() {
        let data = Data::new();
        let mut inventory = data.holding("diamond_chestplate");
        inventory.slots[4] = inventory.slots[4]
            .clone()
            .with_enchantment(id("unbreaking"), 3);
        apply(&data.rules(), &mut inventory, &command("protection", 4)).unwrap();
        assert_eq!(
            inventory.slots[4].enchantments,
            vec![
                ItemEnchantment {
                    id: id("protection"),
                    level: 4
                },
                ItemEnchantment {
                    id: id("unbreaking"),
                    level: 3
                },
            ]
        );
        // Vanilla does not permit upgrading an enchant already on the item:
        // areCompatible explicitly rejects the same registry holder.
        assert!(apply(&data.rules(), &mut inventory, &command("protection", 2)).is_err());
    }

    #[test]
    fn zero_succeeds_without_mutation_and_does_not_remove_enchantments() {
        let data = Data::new();
        let mut inventory = data.holding("diamond_chestplate");
        inventory.slots[4] = inventory.slots[4]
            .clone()
            .with_enchantment(id("unbreaking"), 3);
        let before = inventory.clone();
        apply(&data.rules(), &mut inventory, &command("protection", 0)).unwrap();
        assert_eq!(inventory, before);
    }

    #[test]
    fn supported_types_cover_spears_mace_and_treasure_enchants() {
        let data = Data::new();
        for (item, name, level) in [
            ("wooden_spear", "lunge", 3),
            ("wooden_spear", "sharpness", 5),
            ("mace", "density", 5),
            ("mace", "wind_burst", 3),
            ("diamond_pickaxe", "mending", 1),
            ("trident", "loyalty", 3),
        ] {
            let mut inventory = data.holding(item);
            apply(&data.rules(), &mut inventory, &command(name, level)).unwrap();
        }
        let mut inventory = data.holding("trident");
        apply(&data.rules(), &mut inventory, &command("loyalty", 3)).unwrap();
        apply(&data.rules(), &mut inventory, &command("channeling", 1)).unwrap();
    }

    #[test]
    fn preflight_matches_apply_without_mutating_inventory() {
        let data = Data::new();
        let mut inventory = data.holding("diamond_chestplate");
        let before = inventory.clone();
        let command = command("protection", 4);
        validate(&data.rules(), &inventory, &command).unwrap();
        assert_eq!(inventory, before);
        apply(&data.rules(), &mut inventory, &command).unwrap();
        let failure = validate(&data.rules(), &inventory, &command).unwrap_err();
        let after = inventory.clone();
        assert_eq!(
            apply(&data.rules(), &mut inventory, &command).unwrap_err(),
            failure
        );
        assert_eq!(inventory, after);
    }

    #[test]
    fn populated_item_tags_are_reused_and_empty_client_tags_use_vanilla() {
        use std::collections::BTreeMap;

        let mut data = Data::new();
        let stone_id = data.items.id_of(&id("stone")).unwrap() as i32;
        data.tags = TagsData::from_registries(BTreeMap::from([(
            id("minecraft:item"),
            BTreeMap::from([(id("minecraft:enchantable/armor"), vec![stone_id])]),
        )]));
        let mut stone = data.holding("stone");
        apply(&data.rules(), &mut stone, &command("protection", 1)).unwrap();
        let mut armor = data.holding("diamond_chestplate");
        assert!(apply(&data.rules(), &mut armor, &command("protection", 1)).is_err());

        data.tags = TagsData::from_registries(BTreeMap::from([(
            id("minecraft:item"),
            BTreeMap::from([(id("minecraft:enchantable/armor"), Vec::new())]),
        )]));
        apply(&data.rules(), &mut armor, &command("protection", 1)).unwrap();
    }
}
