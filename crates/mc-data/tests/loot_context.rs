use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use mc_data::Identifier;
use mc_data::loot::entity_26_1_2::{
    EntityDeathCause, EntityLootAttack, EntityLootCatalog, EntityLootContext, EntityLootEntity,
    EntityLootEvaluationError, EntityLootPlayerAttribution, EntityLootRecipeLookup,
    EntityLootSmeltingRecipe, EntityLootTagLookup,
};
use mc_data::loot::{
    BlockLootContext, BlockLootEvaluationError, LootContextError, LootContextItem, LootDrop,
    LootEnchantments, LootExplosion, LootRandomBinding, LootTables, MAX_LOOT_CONTEXT_ENTRIES,
};

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

fn load_block_table(name: &str, raw: &str) -> LootTables {
    let temp = tempfile::tempdir().unwrap();
    let blocks = temp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(blocks.join(format!("{name}.json")), raw).unwrap();
    mc_data::loot::load_vanilla_subset(temp.path()).unwrap()
}

fn load_entity_table(name: &str, raw: &str) -> EntityLootCatalog {
    let temp = tempfile::tempdir().unwrap();
    let entities = temp
        .path()
        .join("minecraft")
        .join("loot_table")
        .join("entities");
    fs::create_dir_all(&entities).unwrap();
    fs::write(entities.join(format!("{name}.json")), raw).unwrap();
    EntityLootCatalog::compile_resources(
        temp.path(),
        [Identifier::parse(format!("minecraft:entities/{name}")).unwrap()],
    )
    .unwrap()
}

fn entity_context(seed: u64) -> EntityLootContext<'static> {
    EntityLootContext::new(
        EntityLootEntity::new(id("minecraft:cow")),
        EntityDeathCause::new(BTreeSet::new(), EntityLootAttack::None, None),
        LootRandomBinding::new(None, seed),
    )
}

fn unsequenced(seed: u64) -> LootRandomBinding {
    LootRandomBinding::new(None, seed)
}

#[test]
fn simple_fallback_block_drops_observe_explosion_survival_context() {
    let block = id("minecraft:test_block");
    let loot = LootTables::from_drop_maps(
        BTreeMap::new(),
        BTreeMap::from([(block.clone(), LootDrop::single(id("minecraft:fragment")))]),
    );
    let tool = LootContextItem::empty();

    let certain = LootExplosion::try_new(1.0).unwrap();
    assert_eq!(
        loot.evaluate_block(
            &BlockLootContext::try_new(&block, &[], &tool, unsequenced(1))
                .unwrap()
                .with_explosion(certain),
        )
        .unwrap(),
        Some(vec![LootDrop::single(id("minecraft:fragment"))])
    );

    let destructive = LootExplosion::try_new(f32::MAX).unwrap();
    assert_eq!(
        loot.evaluate_block(
            &BlockLootContext::try_new(&block, &[], &tool, unsequenced(1))
                .unwrap()
                .with_explosion(destructive),
        )
        .unwrap(),
        Some(Vec::new())
    );
}

#[test]
fn context_collections_are_bounded_before_element_validation() {
    let invalid = id("minecraft:fortune");
    let oversized = (0..=MAX_LOOT_CONTEXT_ENTRIES)
        .map(|index| {
            let enchantment = if index == 0 {
                invalid.clone()
            } else {
                id(&format!("test:enchantment_{index}"))
            };
            (enchantment, 0)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        LootEnchantments::try_from_levels(oversized),
        Err(LootContextError::TooManyEnchantments {
            actual: MAX_LOOT_CONTEXT_ENTRIES + 1,
            maximum: MAX_LOOT_CONTEXT_ENTRIES,
        })
    );

    let block = id("minecraft:test_block");
    let tool = LootContextItem::empty();
    let oversized_properties = (0..=MAX_LOOT_CONTEXT_ENTRIES)
        .map(|index| (format!("property_{index}"), String::new()))
        .collect::<Vec<_>>();
    assert!(matches!(
        BlockLootContext::try_new(&block, &oversized_properties, &tool, unsequenced(1)),
        Err(LootContextError::TooManyBlockProperties {
            actual,
            maximum: MAX_LOOT_CONTEXT_ENTRIES,
        }) if actual == MAX_LOOT_CONTEXT_ENTRIES + 1
    ));
}

#[test]
fn contradictory_duplicate_block_properties_are_rejected() {
    let block = id("minecraft:wheat");
    let tool = LootContextItem::empty();
    let error = BlockLootContext::try_new(
        &block,
        &[
            ("age".to_string(), "6".to_string()),
            ("age".to_string(), "7".to_string()),
        ],
        &tool,
        unsequenced(1),
    )
    .unwrap_err();

    assert_eq!(
        error,
        LootContextError::ConflictingBlockProperty {
            property: "age".to_string(),
            first: "6".to_string(),
            second: "7".to_string(),
        }
    );
}

#[test]
fn identical_duplicate_block_properties_are_rejected() {
    let block = id("minecraft:wheat");
    let tool = LootContextItem::empty();
    let error = BlockLootContext::try_new(
        &block,
        &[
            ("age".to_string(), "7".to_string()),
            ("age".to_string(), "7".to_string()),
        ],
        &tool,
        unsequenced(1),
    )
    .unwrap_err();

    assert_eq!(
        error,
        LootContextError::DuplicateBlockProperty {
            property: "age".to_string(),
        }
    );
}

#[test]
fn block_evaluation_rejects_absent_or_mismatched_random_sequence_identity() {
    let loot = load_block_table(
        "sequenced",
        r#"{
          "random_sequence":"test:blocks/sequenced",
          "pools":[{"entries":[{
            "type":"minecraft:item",
            "name":"minecraft:fragment"
          }]}]
        }"#,
    );
    let block = id("minecraft:sequenced");
    let tool = LootContextItem::empty();

    for actual in [None, Some(id("test:blocks/wrong"))] {
        let context = BlockLootContext::try_new(
            &block,
            &[],
            &tool,
            LootRandomBinding::new(actual.clone(), 0x5eed),
        )
        .unwrap();
        assert_eq!(
            loot.evaluate_block(&context),
            Err(BlockLootEvaluationError::RandomSequenceMismatch {
                expected: Some(id("test:blocks/sequenced")),
                actual,
            })
        );
    }

    let binding = LootRandomBinding::new(Some(id("test:blocks/sequenced")), 0x5eed);
    let context = BlockLootContext::try_new(&block, &[], &tool, binding).unwrap();
    assert_eq!(loot.evaluate_block(&context), loot.evaluate_block(&context));
}

#[test]
fn block_roll_range_is_sampled_deterministically_with_a_seed() {
    let loot = load_block_table(
        "range_rolls",
        r#"{
          "pools": [{
            "rolls": {"type":"minecraft:uniform","min":1.0,"max":3.0},
            "bonus_rolls": 0.0,
            "entries": [{"type":"minecraft:item","name":"minecraft:gold_nugget"}]
          }]
        }"#,
    );
    let block = id("minecraft:range_rolls");
    let tool = LootContextItem::empty();
    let context = |seed| {
        BlockLootContext::try_new(&block, &[], &tool, LootRandomBinding::new(None, seed)).unwrap()
    };
    let first = loot.evaluate_block(&context(0x5EED_u64)).unwrap().unwrap();
    let second = loot.evaluate_block(&context(0x5EED_u64)).unwrap().unwrap();

    assert_eq!(first, second);
    assert!((1..=3).contains(&first.len()));
}

#[test]
fn entity_roll_range_is_sampled_deterministically_with_a_seed() {
    let catalog = load_entity_table(
        "range_rolls",
        r#"{
          "type": "minecraft:entity",
          "pools": [{
            "rolls": {"type":"minecraft:uniform","min":1.0,"max":3.0},
            "bonus_rolls": 0.0,
            "entries": [{"type":"minecraft:item","name":"minecraft:nether_star","weight":1}]
          }]
        }"#,
    );
    let table = id("minecraft:entities/range_rolls");
    let first = catalog
        .evaluate(&table, &entity_context(0x5EED_u64))
        .unwrap();
    let second = catalog
        .evaluate(&table, &entity_context(0x5EED_u64))
        .unwrap();

    assert_eq!(first, second);
    assert!((1..=3).contains(&first.len()));
}

#[test]
fn authoritative_block_tool_context_applies_silk_and_fortune_oracle() {
    let loot = load_block_table(
        "diamond_ore",
        r#"{
          "type": "minecraft:block",
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
            }],
            "rolls": 1.0
          }],
          "random_sequence": "minecraft:blocks/diamond_ore"
        }"#,
    );
    let block = id("minecraft:diamond_ore");
    let pickaxe = id("minecraft:iron_pickaxe");
    let silk = LootContextItem::new(pickaxe.clone())
        .try_with_enchantment(id("minecraft:silk_touch"), 1)
        .unwrap();
    let fortune = LootContextItem::new(pickaxe)
        .try_with_enchantment(id("minecraft:fortune"), 3)
        .unwrap();

    let evaluate = |tool: &LootContextItem| {
        loot.evaluate_block(
            &BlockLootContext::try_new(
                &block,
                &[],
                tool,
                LootRandomBinding::new(Some(id("minecraft:blocks/diamond_ore")), 3),
            )
            .unwrap(),
        )
        .unwrap()
        .unwrap()
    };
    assert_eq!(evaluate(&silk), [LootDrop::single(block.clone())]);
    assert_eq!(
        evaluate(&fortune),
        [LootDrop::fixed(id("minecraft:diamond"), 3)]
    );
}

#[test]
fn empty_block_tool_cannot_supply_silk_or_fortune() {
    let block = id("minecraft:diamond_ore");
    let enchantments = LootEnchantments::try_from_levels([
        (id("minecraft:silk_touch"), 1),
        (id("minecraft:fortune"), 3),
    ])
    .unwrap();
    let empty_enchanted = LootContextItem::empty().with_enchantments(enchantments);

    assert!(matches!(
        BlockLootContext::try_new(&block, &[], &empty_enchanted, unsequenced(3)),
        Err(LootContextError::EnchantmentsWithoutItem)
    ));
}

#[derive(Default)]
struct ContextTags;

impl EntityLootTagLookup for ContextTags {
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

#[derive(Default)]
struct ContextRecipes(BTreeMap<Identifier, EntityLootSmeltingRecipe>);

impl EntityLootRecipeLookup for ContextRecipes {
    fn smelting_recipe(&self, input: &Identifier) -> Option<&EntityLootSmeltingRecipe> {
        self.0.get(input)
    }
}

#[test]
fn authoritative_killer_and_burning_context_apply_looting_and_smelt_oracle() {
    let catalog = load_entity_table(
        "contextual_cow",
        r#"{
          "type":"minecraft:entity",
          "pools":[{
            "rolls":1,
            "entries":[{
              "type":"minecraft:item",
              "name":"minecraft:beef",
              "functions":[
                {
                  "function":"minecraft:enchanted_count_increase",
                  "enchantment":"minecraft:looting",
                  "count":1
                },
                {
                  "function":"minecraft:furnace_smelt",
                  "conditions":[{
                    "condition":"minecraft:entity_properties",
                    "entity":"this",
                    "predicate":{"flags":{"is_on_fire":true}}
                  }]
                }
              ]
            }]
          }]
        }"#,
    );
    let table = id("minecraft:entities/contextual_cow");
    let mut killer = EntityLootEntity::new(id("minecraft:player"));
    killer.mainhand = Some(
        LootContextItem::new(id("minecraft:diamond_sword"))
            .try_with_enchantment(id("minecraft:looting"), 2)
            .unwrap(),
    );
    killer.active_enchantments =
        LootEnchantments::try_from_levels([(id("minecraft:looting"), 2)]).unwrap();
    let attribution = EntityLootPlayerAttribution::try_new(killer.clone()).unwrap();
    let cause = EntityDeathCause::new(
        BTreeSet::new(),
        EntityLootAttack::Direct(killer),
        Some(attribution),
    );
    let mut victim = EntityLootEntity::new(id("minecraft:cow"));
    victim.flags.is_on_fire = true;
    let mut recipes = ContextRecipes::default();
    recipes.0.insert(
        id("minecraft:beef"),
        EntityLootSmeltingRecipe {
            output: id("minecraft:cooked_beef"),
            output_count: 1,
            max_stack_size: 64,
        },
    );
    let context =
        EntityLootContext::new(victim, cause, unsequenced(9)).with_lookups(&ContextTags, &recipes);

    let drops = catalog.evaluate(&table, &context).unwrap();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].item, id("minecraft:cooked_beef"));
    assert_eq!(drops[0].count, 3);
}

#[test]
fn non_living_killer_cannot_smuggle_looting_equipment_context() {
    let catalog = load_entity_table(
        "invalid_killer",
        r#"{
          "type":"minecraft:entity",
          "pools":[{
            "rolls":1,
            "entries":[{
              "type":"minecraft:item",
              "name":"minecraft:beef",
              "functions":[{
                "function":"minecraft:enchanted_count_increase",
                "enchantment":"minecraft:looting",
                "count":1
              }]
            }]
          }]
        }"#,
    );
    let mut projectile = EntityLootEntity::new(id("minecraft:arrow"));
    projectile.is_living = false;
    projectile.active_enchantments =
        LootEnchantments::try_from_levels([(id("minecraft:looting"), 2)]).unwrap();
    let context = EntityLootContext::new(
        EntityLootEntity::new(id("minecraft:cow")),
        EntityDeathCause::new(BTreeSet::new(), EntityLootAttack::Direct(projectile), None),
        unsequenced(9),
    );

    assert!(matches!(
        catalog.evaluate(&id("minecraft:entities/invalid_killer"), &context),
        Err(EntityLootEvaluationError::InvalidContext { message })
            if message.contains("non-living entity minecraft:arrow")
    ));
}
