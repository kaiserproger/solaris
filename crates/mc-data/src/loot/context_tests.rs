use std::collections::BTreeMap;
use std::fs;

use super::*;

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

fn load_block_table(name: &str, raw: &str) -> LootTables {
    let temp = tempfile::tempdir().unwrap();
    let blocks = temp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(blocks.join(format!("{name}.json")), raw).unwrap();
    load_vanilla_subset(temp.path()).unwrap()
}

fn block_context<'a>(
    block: &'a Identifier,
    properties: &[(String, String)],
    tool: &'a LootContextItem,
    sequence: Option<&str>,
    seed: u64,
) -> BlockLootContext<'a> {
    BlockLootContext::try_new(
        block,
        properties,
        tool,
        LootRandomBinding::new(sequence.map(id), seed),
    )
    .unwrap()
}

#[test]
fn loot_enchantments_reject_invalid_levels_and_accept_vanilla_maximum() {
    for invalid in [0, u32::from(MAX_LOOT_ENCHANTMENT_LEVEL) + 1, u32::MAX] {
        let error = LootEnchantments::try_from_levels([(id("minecraft:fortune"), invalid)])
            .expect_err("levels outside the serialized vanilla range must fail");
        assert_eq!(
            error,
            LootContextError::InvalidEnchantmentLevel {
                enchantment: id("minecraft:fortune"),
                level: invalid,
            }
        );
    }

    let levels = LootEnchantments::try_from_levels([
        (
            id("minecraft:fortune"),
            u32::from(MAX_LOOT_ENCHANTMENT_LEVEL),
        ),
        (id("minecraft:silk_touch"), 1),
    ])
    .unwrap();
    assert_eq!(
        levels.level(&id("minecraft:fortune")),
        u32::from(MAX_LOOT_ENCHANTMENT_LEVEL)
    );
    assert_eq!(levels.level(&id("minecraft:looting")), 0);

    let mut replacement = LootEnchantments::default();
    assert_eq!(
        replacement.try_insert(id("minecraft:fortune"), 2).unwrap(),
        ()
    );
    assert_eq!(
        replacement.try_insert(id("minecraft:fortune"), 3),
        Err(LootContextError::DuplicateEnchantment {
            enchantment: id("minecraft:fortune"),
        })
    );
}

#[test]
fn loot_enchantments_reject_duplicate_entries() {
    let error = LootEnchantments::try_from_levels([
        (id("minecraft:fortune"), 1),
        (id("minecraft:fortune"), 1),
    ])
    .expect_err("duplicate enchantments must not silently overwrite");

    assert_eq!(
        error,
        LootContextError::DuplicateEnchantment {
            enchantment: id("minecraft:fortune"),
        }
    );
}

#[test]
fn absent_tool_context_has_zero_silk_touch_fortune_and_looting() {
    let tool = LootContextItem::empty();

    assert!(tool.item().is_none());
    assert_eq!(tool.silk_touch_level(), 0);
    assert_eq!(tool.fortune_level(), 0);
    assert_eq!(tool.looting_level(), 0);
}

#[test]
fn block_context_applies_silk_touch_fortune_and_probability_boundaries() {
    let loot = load_block_table(
        "test_ore",
        r#"{
          "random_sequence":"test:blocks/test_ore",
          "pools":[{
            "entries":[{
              "type":"minecraft:alternatives",
              "children":[{
                "type":"minecraft:item",
                "conditions":[{
                  "condition":"minecraft:match_tool",
                  "predicate":{"predicates":{"minecraft:enchantments":[{
                    "enchantments":"minecraft:silk_touch",
                    "levels":{"min":1}
                  }]}}
                }],
                "name":"minecraft:test_ore"
              },{
                "type":"minecraft:item",
                "functions":[{
                  "enchantment":"minecraft:fortune",
                  "formula":"minecraft:uniform_bonus_count",
                  "function":"minecraft:apply_bonus",
                  "parameters":{"bonusMultiplier":1}
                }],
                "name":"minecraft:gem"
              }]
            }]
          },{
            "entries":[{
              "type":"minecraft:item",
              "conditions":[{"condition":"minecraft:random_chance","chance":0.0}],
              "name":"minecraft:never"
            }]
          },{
            "entries":[{
              "type":"minecraft:item",
              "conditions":[{"condition":"minecraft:random_chance","chance":1.0}],
              "name":"minecraft:always"
            }]
          }]
        }"#,
    );
    let block = id("minecraft:test_ore");
    assert_eq!(
        loot.block_random_sequence(&block),
        Some(&id("test:blocks/test_ore"))
    );

    let empty = LootContextItem::empty();
    let regular = loot
        .evaluate_block(&block_context(
            &block,
            &[],
            &empty,
            Some("test:blocks/test_ore"),
            7,
        ))
        .unwrap()
        .unwrap();
    assert_eq!(
        regular
            .iter()
            .map(|drop| drop.item.as_str())
            .collect::<Vec<_>>(),
        ["minecraft:gem", "minecraft:always"]
    );

    let fortune = LootContextItem::new(id("minecraft:diamond_pickaxe"))
        .try_with_enchantment(id("minecraft:fortune"), 3)
        .unwrap();
    let first = loot
        .evaluate_block(&block_context(
            &block,
            &[],
            &fortune,
            Some("test:blocks/test_ore"),
            0x5eed,
        ))
        .unwrap()
        .unwrap();
    let second = loot
        .evaluate_block(&block_context(
            &block,
            &[],
            &fortune,
            Some("test:blocks/test_ore"),
            0x5eed,
        ))
        .unwrap()
        .unwrap();
    assert_eq!(
        first, second,
        "an explicit seed must make evaluation replayable"
    );
    assert!(matches!(first[0].count, LootCount::Fixed(1..=4)));

    let silk = LootContextItem::new(id("minecraft:diamond_pickaxe"))
        .try_with_enchantment(id("minecraft:silk_touch"), 1)
        .unwrap();
    let silk_drops = loot
        .evaluate_block(&block_context(
            &block,
            &[],
            &silk,
            Some("test:blocks/test_ore"),
            7,
        ))
        .unwrap()
        .unwrap();
    assert_eq!(silk_drops[0], LootDrop::single(block));
}

#[test]
fn explosion_context_validates_radius_and_covers_survival_boundaries() {
    for radius in [0.0, -1.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        assert!(matches!(
            LootExplosion::try_new(radius),
            Err(LootContextError::InvalidExplosionRadius { .. })
        ));
    }

    let certain = LootExplosion::try_new(1.0).unwrap();
    assert!(certain.survives(0.0));
    assert!(certain.survives(1.0));

    let minimum = LootExplosion::try_new(f32::MAX).unwrap();
    assert!(minimum.survives(0.0));
    assert!(!minimum.survives(f32::EPSILON));
}

#[test]
fn block_context_explosion_decay_is_seeded_and_count_overflow_is_bounded() {
    let loot = load_block_table(
        "dense",
        r#"{
          "pools":[{
            "entries":[{
              "type":"minecraft:item",
              "functions":[
                {"function":"minecraft:set_count","count":8},
                {"function":"minecraft:explosion_decay"}
              ],
              "name":"minecraft:fragment"
            }]
          }]
        }"#,
    );
    let block = id("minecraft:dense");
    let empty = LootContextItem::empty();
    let explosion = LootExplosion::try_new(4.0).unwrap();
    let context = block_context(&block, &[], &empty, None, 42).with_explosion(explosion);
    let first = loot.evaluate_block(&context).unwrap().unwrap();
    let second = loot.evaluate_block(&context).unwrap().unwrap();
    assert_eq!(first, second);

    let oversized = load_block_table(
        "oversized_decay",
        r#"{
          "pools":[{
            "entries":[{
              "type":"minecraft:item",
              "functions":[
                {"function":"minecraft:set_count","count":64},
                {
                  "enchantment":"minecraft:fortune",
                  "formula":"minecraft:uniform_bonus_count",
                  "function":"minecraft:apply_bonus",
                  "parameters":{"bonusMultiplier":4294967295}
                },
                {"function":"minecraft:explosion_decay"}
              ],
              "name":"minecraft:fragment"
            }]
          }]
        }"#,
    );
    let oversized_block = id("minecraft:oversized_decay");
    let max_fortune = LootContextItem::new(id("minecraft:diamond_pickaxe"))
        .try_with_enchantment(
            id("minecraft:fortune"),
            u32::from(MAX_LOOT_ENCHANTMENT_LEVEL),
        )
        .unwrap();
    let error = oversized
        .evaluate_block(
            &block_context(&oversized_block, &[], &max_fortune, None, 42).with_explosion(explosion),
        )
        .expect_err("decay work must be bounded before per-item iteration");
    assert!(matches!(
        error,
        BlockLootEvaluationError::ArithmeticOverflow { .. }
    ));

    assert!(matches!(
        FortuneBonus::UniformBonusCount {
            bonus_multiplier: u32::MAX,
        }
        .try_apply(
            u32::MAX,
            u32::from(MAX_LOOT_ENCHANTMENT_LEVEL),
            &mut context::LootRandom::new(u64::MAX),
        ),
        Err(BlockLootEvaluationError::ArithmeticOverflow { .. })
    ));
    assert_eq!(
        LootCount::UniformInclusive {
            min: 0,
            max: u32::MAX,
        }
        .try_sample(u64::MAX)
        .unwrap(),
        u32::MAX
    );
}

#[test]
fn block_context_bounds_binomial_rolls_and_handles_zero_one_probabilities() {
    let loot = load_block_table(
        "bounded_bonus",
        &format!(
            r#"{{
              "pools":[
                {{
                  "entries":[{{
                    "type":"minecraft:item",
                    "functions":[{{
                      "enchantment":"minecraft:fortune",
                      "formula":"minecraft:binomial_with_bonus_count",
                      "function":"minecraft:apply_bonus",
                      "parameters":{{"extra":3,"probability":0.0}}
                    }}],
                    "name":"minecraft:zero"
                  }}]
                }},
                {{
                  "entries":[{{
                    "type":"minecraft:item",
                    "functions":[{{
                      "enchantment":"minecraft:fortune",
                      "formula":"minecraft:binomial_with_bonus_count",
                      "function":"minecraft:apply_bonus",
                      "parameters":{{"extra":3,"probability":1.0}}
                    }}],
                    "name":"minecraft:one"
                  }}]
                }},
                {{
                  "entries":[{{
                    "type":"minecraft:item",
                    "functions":[{{
                      "enchantment":"minecraft:fortune",
                      "formula":"minecraft:binomial_with_bonus_count",
                      "function":"minecraft:apply_bonus",
                      "parameters":{{"extra":{},"probability":0.5}}
                    }}],
                    "name":"minecraft:too_many"
                  }}]
                }}
              ]
            }}"#,
            MAX_BLOCK_BONUS_ROLLS + 1
        ),
    );
    let block = id("minecraft:bounded_bonus");
    let tool = LootContextItem::empty();
    let error = loot
        .evaluate_block(&block_context(&block, &[], &tool, None, 13))
        .expect_err("crafted bonus rounds must fail before iteration");
    assert_eq!(
        error,
        BlockLootEvaluationError::BonusRollLimitExceeded {
            actual: u64::from(MAX_BLOCK_BONUS_ROLLS) + 1,
            maximum: u64::from(MAX_BLOCK_BONUS_ROLLS),
        }
    );

    let bounded = load_block_table(
        "probabilities",
        r#"{
          "pools":[
            {
              "entries":[{
                "type":"minecraft:item",
                "functions":[{
                  "enchantment":"minecraft:fortune",
                  "formula":"minecraft:binomial_with_bonus_count",
                  "function":"minecraft:apply_bonus",
                  "parameters":{"extra":3,"probability":0.0}
                }],
                "name":"minecraft:zero"
              }]
            },
            {
              "entries":[{
                "type":"minecraft:item",
                "functions":[{
                  "enchantment":"minecraft:fortune",
                  "formula":"minecraft:binomial_with_bonus_count",
                  "function":"minecraft:apply_bonus",
                  "parameters":{"extra":3,"probability":1.0}
                }],
                "name":"minecraft:one"
              }]
            }
          ]
        }"#,
    );
    let block = id("minecraft:probabilities");
    let drops = bounded
        .evaluate_block(&block_context(&block, &[], &tool, None, 13))
        .unwrap()
        .unwrap();
    assert_eq!(drops[0].count, LootCount::Fixed(1));
    assert_eq!(drops[1].count, LootCount::Fixed(4));
}

#[test]
fn fortune_formulas_preserve_structural_draws_and_fail_closed_on_invalid_arithmetic() {
    fn next_after(seed: u64, draws: usize) -> u64 {
        let mut random = context::LootRandom::new(seed);
        for _ in 0..draws {
            random.next_u64();
        }
        random.next_u64()
    }

    let seed = 0x5eed;
    let mut uniform_zero = context::LootRandom::new(seed);
    assert_eq!(
        FortuneBonus::UniformBonusCount {
            bonus_multiplier: 17,
        }
        .try_apply(4, 0, &mut uniform_zero),
        Ok(4)
    );
    assert_eq!(uniform_zero.next_u64(), next_after(seed, 0));

    let mut ore_zero = context::LootRandom::new(seed);
    assert_eq!(FortuneBonus::OreDrops.try_apply(4, 0, &mut ore_zero), Ok(4));
    assert_eq!(ore_zero.next_u64(), next_after(seed, 0));

    for probability in [0.0, 1.0] {
        let mut binomial = context::LootRandom::new(seed);
        assert_eq!(
            FortuneBonus::BinomialWithBonusCount {
                extra_rounds: 3,
                probability,
            }
            .try_apply(4, 0, &mut binomial),
            Ok(if probability == 0.0 { 4 } else { 7 })
        );
        assert_eq!(binomial.next_u64(), next_after(seed, 0));
    }

    let mut nonfinite = context::LootRandom::new(seed);
    assert_eq!(
        FortuneBonus::BinomialWithBonusCount {
            extra_rounds: 1,
            probability: f32::NAN,
        }
        .try_apply(1, 0, &mut nonfinite),
        Err(BlockLootEvaluationError::InvalidProbability)
    );
    assert_eq!(nonfinite.next_u64(), next_after(seed, 0));

    let mut overflow = context::LootRandom::new(seed);
    assert_eq!(
        FortuneBonus::BinomialWithBonusCount {
            extra_rounds: 1,
            probability: 1.0,
        }
        .try_apply(u32::MAX, 0, &mut overflow),
        Err(BlockLootEvaluationError::ArithmeticOverflow {
            operation: "applying binomial Fortune bonus",
        })
    );
    assert_eq!(overflow.next_u64(), next_after(seed, 0));

    let mut oversized = context::LootRandom::new(seed);
    assert_eq!(
        FortuneBonus::BinomialWithBonusCount {
            extra_rounds: u32::MAX,
            probability: 0.5,
        }
        .try_apply(1, u32::from(MAX_LOOT_ENCHANTMENT_LEVEL), &mut oversized),
        Err(BlockLootEvaluationError::BonusRollLimitExceeded {
            actual: u64::from(u32::MAX) + u64::from(MAX_LOOT_ENCHANTMENT_LEVEL),
            maximum: u64::from(MAX_BLOCK_BONUS_ROLLS),
        })
    );
    assert_eq!(oversized.next_u64(), next_after(seed, 0));
}

#[test]
fn block_context_reports_unknown_blocks_and_samples_simple_fallbacks() {
    let known = id("minecraft:known");
    let unknown = id("minecraft:unknown");
    let loot = LootTables::from_drop_maps(
        BTreeMap::new(),
        BTreeMap::from([(
            known.clone(),
            LootDrop::uniform(id("minecraft:fragment"), u32::MAX, u32::MAX),
        )]),
    );
    let tool = LootContextItem::empty();

    assert_eq!(
        loot.evaluate_block(&block_context(&unknown, &[], &tool, None, 1))
            .unwrap(),
        None
    );
    assert_eq!(loot.block_random_sequence(&unknown), None);
    assert_eq!(
        loot.evaluate_block(&block_context(&known, &[], &tool, None, u64::MAX))
            .expect_err("oversized fallback output must fail before publication"),
        BlockLootEvaluationError::OutputItemLimitExceeded {
            actual: u64::from(u32::MAX),
            maximum: MAX_BLOCK_OUTPUT_ITEMS,
        }
    );

    let fallback = LootTables::from_drop_maps(
        BTreeMap::new(),
        BTreeMap::from([(known.clone(), LootDrop::single(id("minecraft:fragment")))]),
    );
    let destructive = LootExplosion::try_new(f32::MAX).unwrap();
    assert_eq!(
        fallback
            .evaluate_block(&block_context(&known, &[], &tool, None, 1).with_explosion(destructive))
            .unwrap(),
        Some(Vec::new())
    );
}
