use std::collections::BTreeMap;
use std::fs;

use mc_data::Identifier;
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
