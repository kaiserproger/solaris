use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use mc_data::Identifier;
use mc_data::loot::{
    BlockLootContext, BlockLootEvaluationError, LootContextError, LootContextItem, LootDrop,
    LootEnchantments, LootExplosion, LootRandomBinding, LootTables, MAX_LOOT_CONTEXT_ENTRIES,
};
use mc_data::loot::entity_26_1_2::{
    EntityDeathCause, EntityLootAttack, EntityLootCatalog, EntityLootContext, EntityLootEntity,
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
        [Identifier::parse(&format!("minecraft:entities/{name}")).unwrap()],
    )
    .unwrap()
}

fn entity_context(seed: u64) -> EntityLootContext<'static> {
    EntityLootContext::new(
        EntityLootEntity::new(id("minecraft:cow")),
        EntityDeathCause::new(BTreeSet::new(), EntityLootAttack::None, None),
        LootRandomBinding::new(Some(id("test:entities/context")), seed),
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
        BlockLootContext::try_new(
            &block,
            &[],
            &tool,
            LootRandomBinding::new(None, seed),
        )
        .unwrap()
    };
    let first = loot.evaluate_block(&context(0x5eED_u64)).unwrap().unwrap();
    let second = loot.evaluate_block(&context(0x5eED_u64)).unwrap().unwrap();

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
    let first = catalog.evaluate(&table, &entity_context(0x5eED_u64)).unwrap();
    let second = catalog.evaluate(&table, &entity_context(0x5eED_u64)).unwrap();

    assert_eq!(first, second);
    assert!((1..=3).contains(&first.len()));
}
