use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::Identifier;
use crate::loot::{LootEnchantments, LootRandomBinding};

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

fn entity(entity_type: &str) -> EntityLootEntity {
    EntityLootEntity::new(id(entity_type))
}

fn context(entity_type: &str) -> EntityLootContext<'static> {
    context_with_random(entity_type, None, 0)
}

fn context_with_random(
    entity_type: &str,
    sequence: Option<&str>,
    seed: u64,
) -> EntityLootContext<'static> {
    EntityLootContext::new(
        entity(entity_type),
        EntityDeathCause::new(BTreeSet::new(), EntityLootAttack::None, None),
        LootRandomBinding::new(sequence.map(id), seed),
    )
}

fn random_context(
    entity_type: &str,
    sequence: Option<&str>,
    seed: u64,
) -> EntityLootContext<'static> {
    context_with_random(entity_type, sequence, seed)
}

fn one_item_table(table_type: &str, entry: &str) -> String {
    format!(
        r#"{{
          "type": "{table_type}",
          "pools": [{{
            "rolls": 1.0,
            "bonus_rolls": 0.0,
            "entries": [{entry}]
          }}]
        }}"#
    )
}

fn compiled(table_id: &str, raw: &str) -> CompiledEntityLootTable {
    CompiledEntityLootTable::compile(id(table_id), raw).unwrap()
}

fn catalog(table_id: &str, raw: &str) -> EntityLootCatalog {
    let table_id = id(table_id);
    EntityLootCatalog::from_tables([table_id.clone()], [compiled(table_id.as_str(), raw)]).unwrap()
}

#[test]
fn malformed_and_excessively_nested_json_fail_closed() {
    let malformed = CompiledEntityLootTable::compile(id("minecraft:entities/test"), "{")
        .expect_err("malformed JSON must fail");
    assert!(matches!(
        malformed.kind,
        EntityLootCompileErrorKind::MalformedJson { .. }
    ));

    let mut condition = r#"{"condition":"minecraft:killed_by_player"}"#.to_string();
    for _ in 0..=MAX_COMPILE_NESTING_DEPTH {
        condition = format!(r#"{{"condition":"minecraft:inverted","term":{condition}}}"#);
    }
    let raw = format!(
        r#"{{
          "type":"minecraft:entity",
          "pools":[{{
            "rolls":1,
            "conditions":[{condition}],
            "entries":[{{"type":"minecraft:empty"}}]
          }}]
        }}"#
    );
    let error = CompiledEntityLootTable::compile(id("minecraft:entities/deep"), &raw)
        .expect_err("compile nesting must be bounded");
    assert!(matches!(
        error.kind,
        EntityLootCompileErrorKind::LimitExceeded {
            limit: EntityLootLimit::CompileNesting,
            ..
        }
    ));
}

#[test]
fn source_and_recursive_json_budgets_fail_closed() {
    let oversized = "{".repeat(MAX_SOURCE_BYTES + 1);
    let error = CompiledEntityLootTable::compile(id("minecraft:entities/oversized"), &oversized)
        .expect_err("source bytes must be checked before JSON parsing");
    assert!(matches!(
        error.kind,
        EntityLootCompileErrorKind::LimitExceeded {
            limit: EntityLootLimit::SourceBytes,
            ..
        }
    ));

    let value = serde_json::json!({"long_key": ["long value", null]});
    for (limits, expected) in [
        (
            super::compile::JsonBudgetLimits {
                nodes: 3,
                ..super::compile::JsonBudgetLimits::unbounded()
            },
            EntityLootLimit::JsonNodes,
        ),
        (
            super::compile::JsonBudgetLimits {
                collection_elements: 2,
                ..super::compile::JsonBudgetLimits::unbounded()
            },
            EntityLootLimit::JsonCollectionElements,
        ),
        (
            super::compile::JsonBudgetLimits {
                string_bytes: 8,
                ..super::compile::JsonBudgetLimits::unbounded()
            },
            EntityLootLimit::JsonStringBytes,
        ),
        (
            super::compile::JsonBudgetLimits {
                array_length: 1,
                ..super::compile::JsonBudgetLimits::unbounded()
            },
            EntityLootLimit::JsonArrayLength,
        ),
    ] {
        let violation = super::compile::validate_json_budget(&value, limits)
            .expect_err("the selected recursive JSON budget must fail");
        assert_eq!(violation.limit, expected);
    }
}

#[test]
fn table_root_reference_resource_and_closure_width_budgets_fail_closed() {
    let pools = (0..=MAX_POOLS_PER_TABLE)
        .map(|_| r#"{"rolls":1,"entries":[{"type":"minecraft:empty"}]}"#)
        .collect::<Vec<_>>()
        .join(",");
    let pool_error = CompiledEntityLootTable::compile(
        id("minecraft:entities/pools"),
        &format!(r#"{{"type":"minecraft:entity","pools":[{pools}]}}"#),
    )
    .expect_err("pools per table must be bounded");
    assert!(matches!(
        pool_error.kind,
        EntityLootCompileErrorKind::LimitExceeded {
            limit: EntityLootLimit::PoolsPerTable,
            ..
        }
    ));

    let references = (0..=MAX_REFERENCES_PER_TABLE)
        .map(|index| format!(r#"{{"type":"minecraft:loot_table","value":"test:child_{index}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let reference_error = CompiledEntityLootTable::compile(
        id("minecraft:entities/references"),
        &format!(
            r#"{{"type":"minecraft:entity","pools":[{{"rolls":1,"entries":[{references}]}}]}}"#
        ),
    )
    .expect_err("references per table must be bounded");
    assert!(matches!(
        reference_error.kind,
        EntityLootCompileErrorKind::LimitExceeded {
            limit: EntityLootLimit::ReferencesPerTable,
            ..
        }
    ));

    let root_error = EntityLootCatalog::from_tables(
        (0..=MAX_CATALOG_ROOTS).map(|index| id(&format!("test:root_{index}"))),
        std::iter::empty::<CompiledEntityLootTable>(),
    )
    .expect_err("configured roots must be bounded before catalog construction");
    assert!(matches!(
        root_error,
        EntityLootLoadError::LimitExceeded {
            limit: EntityLootLimit::CatalogRoots,
            ..
        }
    ));

    let root = compiled(
        "minecraft:entities/root",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:item","name":"minecraft:bone"}"#,
        ),
    );
    let tables = std::iter::once(root).chain((0..MAX_CATALOG_RESOURCES).map(|index| {
        compiled(
            &format!("test:unused_{index}"),
            &one_item_table(
                "minecraft:entity",
                r#"{"type":"minecraft:item","name":"minecraft:bone"}"#,
            ),
        )
    }));
    let resource_error = EntityLootCatalog::from_tables([id("minecraft:entities/root")], tables)
        .expect_err("compiled resources must be bounded");
    assert!(matches!(
        resource_error,
        EntityLootLoadError::LimitExceeded {
            limit: EntityLootLimit::CatalogResources,
            ..
        }
    ));

    let temp = tempfile::tempdir().unwrap();
    let entries = (0..=MAX_CLOSURE_WIDTH)
        .map(|index| format!(r#"{{"type":"minecraft:loot_table","value":"test:closure_{index}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    write_resource(
        temp.path(),
        "minecraft:entities/wide",
        &format!(r#"{{"type":"minecraft:entity","pools":[{{"rolls":1,"entries":[{entries}]}}]}}"#),
    );
    let closure_error =
        EntityLootCatalog::compile_resources(temp.path(), [id("minecraft:entities/wide")])
            .expect_err("one reference frontier must be bounded");
    assert!(matches!(
        closure_error,
        EntityLootLoadError::LimitExceeded {
            limit: EntityLootLimit::ClosureWidth,
            ..
        }
    ));
}

#[test]
fn resource_loader_rejects_an_oversize_file_before_parse_or_partial_catalog() {
    let temp = tempfile::tempdir().unwrap();
    write_resource(
        temp.path(),
        "minecraft:entities/root",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:loot_table","value":"test:oversized"}"#,
        ),
    );
    write_resource(
        temp.path(),
        "test:oversized",
        &"{".repeat(MAX_SOURCE_BYTES + 1),
    );

    let error = EntityLootCatalog::compile_resources(temp.path(), [id("minecraft:entities/root")])
        .expect_err("a failed closure member must prevent returning a partial catalog");
    assert!(matches!(
        error,
        EntityLootLoadError::Compile {
            source: EntityLootCompileError {
                kind: EntityLootCompileErrorKind::LimitExceeded {
                    limit: EntityLootLimit::SourceBytes,
                    ..
                },
                ..
            },
            ..
        }
    ));
}

#[test]
fn configured_roots_must_be_nonempty_and_present() {
    let empty = EntityLootCatalog::from_tables(
        std::iter::empty::<Identifier>(),
        std::iter::empty::<CompiledEntityLootTable>(),
    )
    .expect_err("an empty configured root set must fail");
    assert!(matches!(empty, EntityLootLoadError::EmptyRoots));

    let missing = EntityLootCatalog::from_tables(
        [id("minecraft:entities/missing")],
        std::iter::empty::<CompiledEntityLootTable>(),
    )
    .expect_err("a missing configured root must fail");
    assert!(matches!(missing, EntityLootLoadError::MissingRoot { .. }));

    let duplicate_error = EntityLootCatalog::from_tables(
        std::iter::repeat_n(id("minecraft:entities/duplicate"), MAX_CATALOG_ROOTS + 1),
        std::iter::empty::<CompiledEntityLootTable>(),
    )
    .expect_err("every configured root iterator element must consume the root budget");
    assert!(matches!(
        duplicate_error,
        EntityLootLoadError::LimitExceeded {
            limit: EntityLootLimit::CatalogRoots,
            actual,
            ..
        } if actual == (MAX_CATALOG_ROOTS + 1) as u64
    ));
}

#[test]
fn references_are_closed_and_cycles_are_rejected_at_catalog_build() {
    let parent = compiled(
        "minecraft:entities/parent",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:loot_table","value":"minecraft:entities/child"}"#,
        ),
    );
    let missing =
        EntityLootCatalog::from_tables([id("minecraft:entities/parent")], [parent.clone()])
            .expect_err("missing references must fail before evaluation");
    assert!(matches!(
        missing,
        EntityLootLoadError::MissingReference { .. }
    ));

    let child = compiled(
        "minecraft:entities/child",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:loot_table","value":"minecraft:entities/parent"}"#,
        ),
    );
    let cycle = EntityLootCatalog::from_tables([id("minecraft:entities/parent")], [parent, child])
        .expect_err("reference cycles must fail before evaluation");
    assert!(matches!(cycle, EntityLootLoadError::ReferenceCycle { .. }));
}

#[test]
fn reference_depth_is_bounded_at_catalog_build() {
    let mut tables = Vec::new();
    for depth in 0..=MAX_REFERENCE_DEPTH {
        let table_id = format!("minecraft:entities/depth_{depth}");
        let raw = if depth == MAX_REFERENCE_DEPTH {
            one_item_table(
                "minecraft:entity",
                r#"{"type":"minecraft:item","name":"minecraft:bone"}"#,
            )
        } else {
            one_item_table(
                "minecraft:entity",
                &format!(
                    r#"{{"type":"minecraft:loot_table","value":"minecraft:entities/depth_{}"}}"#,
                    depth + 1
                ),
            )
        };
        tables.push(compiled(&table_id, &raw));
    }

    let error = EntityLootCatalog::from_tables([id("minecraft:entities/depth_0")], tables)
        .expect_err("reference depth must be bounded");
    assert!(matches!(
        error,
        EntityLootLoadError::ReferenceDepthExceeded { .. }
    ));
}

#[test]
fn resource_loader_uses_complete_ids_and_loads_fishing_reference_closure() {
    let temp = tempfile::tempdir().unwrap();
    write_resource(
        temp.path(),
        "example:entities/cod_eater",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:loot_table","value":"minecraft:gameplay/fishing/fish"}"#,
        ),
    );
    write_resource(
        temp.path(),
        "minecraft:gameplay/fishing/fish",
        &one_item_table(
            "minecraft:fishing",
            r#"{"type":"minecraft:item","name":"minecraft:cod"}"#,
        ),
    );

    let catalog =
        EntityLootCatalog::compile_resources(temp.path(), [id("example:entities/cod_eater")])
            .unwrap();
    assert_eq!(catalog.table_count(), 2);
    assert!(catalog.contains(&id("example:entities/cod_eater")));
    assert!(catalog.contains(&id("minecraft:gameplay/fishing/fish")));
    assert_eq!(
        catalog
            .evaluate(
                &id("example:entities/cod_eater"),
                &context("example:cod_eater"),
            )
            .unwrap()[0]
            .item,
        id("minecraft:cod")
    );

    let closure_only = catalog
        .evaluate(
            &id("minecraft:gameplay/fishing/fish"),
            &context("example:cod_eater"),
        )
        .expect_err("closure-only fishing tables must not be public evaluation roots");
    assert!(matches!(
        closure_only,
        EntityLootEvaluationError::NotConfiguredRoot { .. }
    ));
}

#[test]
fn closure_only_child_tables_cannot_be_evaluated_directly() {
    let parent = compiled(
        "minecraft:entities/parent",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:loot_table","value":"minecraft:entities/child"}"#,
        ),
    );
    let child = compiled(
        "minecraft:entities/child",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:item","name":"minecraft:bone"}"#,
        ),
    );
    let loot =
        EntityLootCatalog::from_tables([id("minecraft:entities/parent")], [parent, child]).unwrap();

    assert_eq!(
        loot.evaluate(&id("minecraft:entities/parent"), &context("minecraft:cow"),)
            .unwrap()[0]
            .item,
        id("minecraft:bone")
    );
    let error = loot
        .evaluate(&id("minecraft:entities/child"), &context("minecraft:cow"))
        .expect_err("a closure dependency must not become a public root");
    assert!(matches!(
        error,
        EntityLootEvaluationError::NotConfiguredRoot { .. }
    ));
}

#[test]
fn configured_catalog_roots_must_be_entity_tables() {
    let fishing = compiled(
        "minecraft:gameplay/fishing/fish",
        &one_item_table(
            "minecraft:fishing",
            r#"{"type":"minecraft:item","name":"minecraft:cod"}"#,
        ),
    );
    let error = EntityLootCatalog::from_tables([id("minecraft:gameplay/fishing/fish")], [fishing])
        .expect_err("fishing tables are closure dependencies, not entity roots");
    assert!(matches!(error, EntityLootLoadError::InvalidRootType { .. }));
}

#[test]
fn death_cause_has_one_attacker_path_and_validated_player_attribution() {
    let invalid = EntityLootPlayerAttribution::try_new(entity("minecraft:skeleton"))
        .expect_err("only a living player can receive player kill attribution");
    assert!(matches!(
        invalid,
        EntityLootContextError::InvalidPlayerAttribution { .. }
    ));

    let player = entity("minecraft:player");
    let projectile = entity("minecraft:arrow");
    let attribution = EntityLootPlayerAttribution::try_new(player.clone()).unwrap();
    let cause = EntityDeathCause::new(
        BTreeSet::from([id("minecraft:is_projectile")]),
        EntityLootAttack::Indirect {
            source: Some(player.clone()),
            direct: projectile.clone(),
        },
        Some(attribution),
    );

    assert_eq!(cause.source_attacker(), Some(&player));
    assert_eq!(cause.direct_attacker(), Some(&projectile));
    assert_eq!(cause.attributed_player(), Some(&player));
    assert!(!cause.is_direct());

    let no_attack = EntityDeathCause::new(BTreeSet::new(), EntityLootAttack::None, None);
    assert!(no_attack.is_direct());
    let direct = EntityDeathCause::new(
        BTreeSet::new(),
        EntityLootAttack::Direct(entity("minecraft:zombie")),
        None,
    );
    assert!(direct.is_direct());
}

#[test]
fn canonical_death_cause_drives_all_attacker_targets_and_looting() {
    let raw = r#"{
      "type":"minecraft:entity",
      "pools":[
        {
          "rolls":1,
          "conditions":[{"condition":"minecraft:killed_by_player"}],
          "entries":[{"type":"minecraft:item","name":"minecraft:bone"}]
        },
        {
          "rolls":1,
          "conditions":[{
            "condition":"minecraft:entity_properties",
            "entity":"attacker",
            "predicate":{"type":"minecraft:player"}
          }],
          "entries":[{"type":"minecraft:item","name":"minecraft:string"}]
        },
        {
          "rolls":1,
          "conditions":[{
            "condition":"minecraft:entity_properties",
            "entity":"direct_attacker",
            "predicate":{"type":"minecraft:arrow"}
          }],
          "entries":[{"type":"minecraft:item","name":"minecraft:arrow"}]
        },
        {
          "rolls":1,
          "conditions":[{
            "condition":"minecraft:damage_source_properties",
            "predicate":{
              "tags":[{"id":"minecraft:is_projectile","expected":true}],
              "source_entity":{"type":"minecraft:player"},
              "direct_entity":{"type":"minecraft:arrow"},
              "is_direct":false
            }
          }],
          "entries":[{"type":"minecraft:item","name":"minecraft:flint"}]
        },
        {
          "rolls":1,
          "entries":[{
            "type":"minecraft:item",
            "name":"minecraft:rotten_flesh",
            "functions":[{
              "function":"minecraft:enchanted_count_increase",
              "enchantment":"minecraft:looting",
              "count":1
            }]
          }]
        }
      ]
    }"#;
    let loot = catalog("minecraft:entities/canonical", raw);
    let mut player = entity("minecraft:player");
    player.active_enchantments =
        LootEnchantments::try_from_levels([(id("minecraft:looting"), 2)]).unwrap();
    let attribution = EntityLootPlayerAttribution::try_new(player.clone()).unwrap();
    let cause = EntityDeathCause::new(
        BTreeSet::from([id("minecraft:is_projectile")]),
        EntityLootAttack::Indirect {
            source: Some(player),
            direct: entity("minecraft:arrow"),
        },
        Some(attribution),
    );
    let ctx = EntityLootContext::new(
        entity("minecraft:zombie"),
        cause,
        LootRandomBinding::new(None, 9),
    );

    let drops = loot
        .evaluate(&id("minecraft:entities/canonical"), &ctx)
        .unwrap();
    assert_eq!(
        drops
            .iter()
            .map(|stack| stack.item.as_str())
            .collect::<Vec<_>>(),
        [
            "minecraft:bone",
            "minecraft:string",
            "minecraft:arrow",
            "minecraft:flint",
            "minecraft:rotten_flesh",
        ]
    );
    assert_eq!(drops[4].count, 3);
}

#[test]
fn absent_killer_player_and_fire_context_fail_closed_without_looting() {
    let raw = r#"{
      "type":"minecraft:entity",
      "pools":[
        {
          "rolls":1,
          "conditions":[{"condition":"minecraft:killed_by_player"}],
          "entries":[{"type":"minecraft:item","name":"minecraft:player_drop"}]
        },
        {
          "rolls":1,
          "conditions":[{
            "condition":"minecraft:entity_properties",
            "entity":"this",
            "predicate":{"flags":{"is_on_fire":true}}
          }],
          "entries":[{"type":"minecraft:item","name":"minecraft:cooked_meat"}]
        },
        {
          "rolls":1,
          "entries":[{
            "type":"minecraft:item",
            "name":"minecraft:raw_meat",
            "functions":[{
              "function":"minecraft:enchanted_count_increase",
              "enchantment":"minecraft:looting",
              "count":1
            }]
          }]
        }
      ]
    }"#;
    let loot = catalog("minecraft:entities/context_absence", raw);
    let table = id("minecraft:entities/context_absence");

    let absent = loot
        .evaluate(&table, &context_with_random("minecraft:cow", None, 3))
        .unwrap();
    assert_eq!(
        absent,
        [EntityLootStack {
            item: id("minecraft:raw_meat"),
            count: 1,
            components: EntityLootComponents::default(),
        }]
    );

    let mut burning_cow = entity("minecraft:cow");
    burning_cow.flags.is_on_fire = true;
    let burning = loot
        .evaluate(
            &table,
            &EntityLootContext::new(
                burning_cow,
                EntityDeathCause::new(BTreeSet::new(), EntityLootAttack::None, None),
                LootRandomBinding::new(None, 3),
            ),
        )
        .unwrap();
    assert_eq!(
        burning
            .iter()
            .map(|stack| stack.item.as_str())
            .collect::<Vec<_>>(),
        ["minecraft:cooked_meat", "minecraft:raw_meat"]
    );

    let mut killer = entity("minecraft:zombie");
    killer.active_enchantments =
        LootEnchantments::try_from_levels([(id("minecraft:looting"), 2)]).unwrap();
    let killed_by_mob = EntityLootContext::new(
        entity("minecraft:cow"),
        EntityDeathCause::new(BTreeSet::new(), EntityLootAttack::Direct(killer), None),
        LootRandomBinding::new(None, 3),
    );
    let drops = loot.evaluate(&table, &killed_by_mob).unwrap();
    assert_eq!(drops.len(), 1, "an attacking mob is not player attribution");
    assert_eq!(drops[0].count, 3, "Looting comes from the living killer");
}

#[test]
fn probability_boundaries_and_explicit_seed_replay_are_deterministic() {
    let raw = r#"{
      "type":"minecraft:entity",
      "random_sequence":"test:entities/probability",
      "pools":[
        {
          "rolls":1,
          "conditions":[{"condition":"minecraft:random_chance","chance":0.0}],
          "entries":[{"type":"minecraft:item","name":"minecraft:never"}]
        },
        {
          "rolls":1,
          "conditions":[{"condition":"minecraft:random_chance","chance":1.0}],
          "entries":[{"type":"minecraft:item","name":"minecraft:always"}]
        },
        {
          "rolls":1,
          "entries":[
            {"type":"minecraft:item","name":"minecraft:a","weight":1},
            {"type":"minecraft:item","name":"minecraft:b","weight":1}
          ]
        }
      ]
    }"#;
    let loot = catalog("minecraft:entities/probability", raw);
    let table = id("minecraft:entities/probability");
    assert_eq!(
        loot.random_sequence(&table),
        Some(&id("test:entities/probability"))
    );

    for actual in [None, Some("test:entities/wrong")] {
        let error = loot
            .evaluate(&table, &random_context("minecraft:cow", actual, 0x5eed))
            .unwrap_err();
        assert_eq!(
            error,
            EntityLootEvaluationError::RandomSequenceMismatch {
                expected: Some(id("test:entities/probability")),
                actual: actual.map(id),
            }
        );
    }

    let first = loot
        .evaluate(
            &table,
            &random_context("minecraft:cow", Some("test:entities/probability"), 0x5eed),
        )
        .unwrap();
    let second = loot
        .evaluate(
            &table,
            &random_context("minecraft:cow", Some("test:entities/probability"), 0x5eed),
        )
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first[0].item, id("minecraft:always"));
    assert_eq!(first.len(), 2);
}

#[test]
fn nested_empty_tables_and_empty_entries_are_no_ops() {
    let empty_child = compiled(
        "test:empty_child",
        r#"{
          "type":"minecraft:fishing",
          "pools":[{
            "rolls":1,
            "entries":[{"type":"minecraft:empty"}]
          }]
        }"#,
    );
    let parent = compiled(
        "minecraft:entities/nested_empty",
        r#"{
          "type":"minecraft:entity",
          "pools":[
            {
              "rolls":1,
              "entries":[{"type":"minecraft:loot_table","value":"test:empty_child"}]
            },
            {
              "rolls":1,
              "entries":[{"type":"minecraft:item","name":"minecraft:bone"}]
            }
          ]
        }"#,
    );
    let loot = EntityLootCatalog::from_tables(
        [id("minecraft:entities/nested_empty")],
        [parent, empty_child],
    )
    .unwrap();

    let drops = loot
        .evaluate(
            &id("minecraft:entities/nested_empty"),
            &context("minecraft:skeleton"),
        )
        .unwrap();
    assert_eq!(
        drops,
        [EntityLootStack {
            item: id("minecraft:bone"),
            count: 1,
            components: EntityLootComponents::default(),
        }]
    );
}

#[derive(Default)]
struct TestTags {
    items: BTreeMap<Identifier, Vec<Identifier>>,
    entity_types: BTreeSet<(Identifier, Identifier)>,
    enchantments: BTreeSet<(Identifier, Identifier)>,
}

impl EntityLootTagLookup for TestTags {
    fn item_tag(&self, tag: &Identifier) -> Option<&[Identifier]> {
        self.items.get(tag).map(Vec::as_slice)
    }

    fn entity_type_in_tag(&self, tag: &Identifier, entity_type: &Identifier) -> bool {
        self.entity_types
            .contains(&(tag.clone(), entity_type.clone()))
    }

    fn enchantment_in_tag(&self, tag: &Identifier, enchantment: &Identifier) -> bool {
        self.enchantments
            .contains(&(tag.clone(), enchantment.clone()))
    }
}

#[derive(Default)]
struct TestRecipes(BTreeMap<Identifier, EntityLootSmeltingRecipe>);

impl EntityLootRecipeLookup for TestRecipes {
    fn smelting_recipe(&self, input: &Identifier) -> Option<&EntityLootSmeltingRecipe> {
        self.0.get(input)
    }
}

#[test]
fn context_borrows_tag_and_recipe_lookups() {
    let raw = one_item_table(
        "minecraft:entity",
        r##"{
          "type":"minecraft:tag",
          "name":"minecraft:test_meats",
          "expand":false,
          "functions":[{
            "function":"minecraft:furnace_smelt",
            "conditions":[{
              "condition":"minecraft:entity_properties",
              "entity":"this",
              "predicate":{"type":"#minecraft:test_animals"}
            }]
          }]
        }"##,
    );
    let loot = catalog("minecraft:entities/test", &raw);
    let mut tags = TestTags::default();
    tags.items
        .insert(id("minecraft:test_meats"), vec![id("minecraft:beef")]);
    tags.entity_types
        .insert((id("minecraft:test_animals"), id("minecraft:cow")));
    let mut recipes = TestRecipes::default();
    recipes.0.insert(
        id("minecraft:beef"),
        EntityLootSmeltingRecipe {
            output: id("minecraft:cooked_beef"),
            output_count: 1,
            max_stack_size: 64,
        },
    );
    let ctx = context_with_random("minecraft:cow", None, 7).with_lookups(&tags, &recipes);

    let drops = loot.evaluate(&id("minecraft:entities/test"), &ctx).unwrap();
    assert_eq!(drops[0].item, id("minecraft:cooked_beef"));
}

#[test]
fn nested_decorators_apply_per_output_in_structural_rng_order() {
    let child = compiled(
        "minecraft:entities/child",
        r#"{
          "type":"minecraft:entity",
          "functions":[{"function":"minecraft:set_count","count":{"type":"minecraft:uniform","min":40,"max":49},"add":true}],
          "pools":[{
            "rolls":1,
            "functions":[{"function":"minecraft:set_count","count":{"type":"minecraft:uniform","min":20,"max":29},"add":true}],
            "entries":[{
              "type":"minecraft:tag",
              "name":"minecraft:two_items",
              "expand":false,
              "functions":[{"function":"minecraft:set_count","count":{"type":"minecraft:uniform","min":1,"max":9}}]
            }]
          }]
        }"#,
    );
    let parent = compiled(
        "minecraft:entities/parent",
        r#"{
          "type":"minecraft:entity",
          "functions":[{"function":"minecraft:set_count","count":{"type":"minecraft:uniform","min":1000,"max":1009},"add":true}],
          "pools":[{
            "rolls":1,
            "functions":[{"function":"minecraft:set_count","count":{"type":"minecraft:uniform","min":100,"max":109},"add":true}],
            "entries":[{
              "type":"minecraft:loot_table",
              "value":"minecraft:entities/child",
              "functions":[{"function":"minecraft:set_count","count":{"type":"minecraft:uniform","min":10,"max":19},"add":true}]
            }]
          }]
        }"#,
    );
    let loot =
        EntityLootCatalog::from_tables([id("minecraft:entities/parent")], [parent, child]).unwrap();
    let mut tags = TestTags::default();
    tags.items.insert(
        id("minecraft:two_items"),
        vec![id("minecraft:bone"), id("minecraft:string")],
    );
    let recipes = TestRecipes::default();
    let seed = 0x5eed;
    let ctx = context_with_random("minecraft:cow", None, seed).with_lookups(&tags, &recipes);

    let drops = loot
        .evaluate(&id("minecraft:entities/parent"), &ctx)
        .unwrap();
    let mut random = ExpectedRandom::new(seed);
    let expected_first = random.uniform(1, 9)
        + random.uniform(20, 29)
        + random.uniform(40, 49)
        + random.uniform(10, 19)
        + random.uniform(100, 109)
        + random.uniform(1000, 1009);
    let expected_second = random.uniform(1, 9)
        + random.uniform(20, 29)
        + random.uniform(40, 49)
        + random.uniform(10, 19)
        + random.uniform(100, 109)
        + random.uniform(1000, 1009);

    assert_eq!(drops.len(), 2);
    assert_eq!(drops[0].count, expected_first);
    assert_eq!(drops[1].count, expected_second);
}

#[test]
fn component_bearing_output_retains_typed_components() {
    let raw = one_item_table(
        "minecraft:entity",
        r#"{
          "type":"minecraft:item",
          "name":"minecraft:tipped_arrow",
          "functions":[{"function":"minecraft:set_potion","id":"minecraft:slowness"}]
        }"#,
    );
    let drop = catalog("minecraft:entities/stray", &raw)
        .evaluate(&id("minecraft:entities/stray"), &context("minecraft:stray"))
        .unwrap()
        .pop()
        .unwrap();

    assert_eq!(drop.components.potion, Some(id("minecraft:slowness")));
}

#[test]
fn tag_and_candidate_expansion_are_bounded() {
    let raw = one_item_table(
        "minecraft:entity",
        r#"{"type":"minecraft:tag","name":"minecraft:huge","expand":true}"#,
    );
    let loot = catalog("minecraft:entities/test", &raw);
    let mut tags = TestTags::default();
    tags.items.insert(
        id("minecraft:huge"),
        (0..=MAX_TAG_EXPANSION)
            .map(|index| id(&format!("test:item_{index}")))
            .collect(),
    );
    let recipes = TestRecipes::default();
    let ctx = context_with_random("minecraft:cow", None, 1).with_lookups(&tags, &recipes);
    let tag_error = loot
        .evaluate(&id("minecraft:entities/test"), &ctx)
        .expect_err("tag expansion must be bounded");
    assert!(matches!(
        tag_error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::TagExpansion,
            ..
        }
    ));

    let entries = (0..=MAX_CANDIDATES_PER_ROLL)
        .map(|_| r#"{"type":"minecraft:empty"}"#)
        .collect::<Vec<_>>()
        .join(",");
    let raw =
        format!(r#"{{"type":"minecraft:entity","pools":[{{"rolls":1,"entries":[{entries}]}}]}}"#);
    let error = CompiledEntityLootTable::compile(id("minecraft:entities/candidates"), &raw)
        .expect_err("static candidates per roll must be bounded");
    assert!(matches!(
        error.kind,
        EntityLootCompileErrorKind::LimitExceeded {
            limit: EntityLootLimit::CandidatesPerRoll,
            ..
        }
    ));
}

#[test]
fn context_scan_collections_are_bounded_before_any_output() {
    let loot = catalog(
        "minecraft:entities/context_caps",
        &one_item_table(
            "minecraft:entity",
            r#"{"type":"minecraft:item","name":"minecraft:bone"}"#,
        ),
    );
    let table = id("minecraft:entities/context_caps");

    let mut components = context("minecraft:cow");
    components.this_entity.components = (0..=MAX_CONTEXT_COMPONENTS)
        .map(|index| (id(&format!("test:component_{index}")), "value".to_string()))
        .collect();
    let error = loot
        .evaluate(&table, &components)
        .expect_err("component scans must be bounded before output");
    assert!(matches!(
        error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::ContextComponents,
            ..
        }
    ));

    let too_many_enchantments = LootEnchantments::try_from_levels(
        (0..=MAX_CONTEXT_ENCHANTMENTS).map(|index| (id(&format!("test:enchantment_{index}")), 1)),
    )
    .expect_err("context enchantments must be bounded at construction");
    assert!(matches!(
        too_many_enchantments,
        crate::loot::LootContextError::TooManyEnchantments { .. }
    ));

    let too_many_levels = LootEnchantments::try_from_levels(
        (0..=MAX_CONTEXT_ENCHANTMENT_LEVELS).map(|index| (id(&format!("test:level_{index}")), 1)),
    )
    .expect_err("active enchantment levels must be bounded at construction");
    assert!(matches!(
        too_many_levels,
        crate::loot::LootContextError::TooManyEnchantments { .. }
    ));

    let damage_tags = (0..=MAX_CONTEXT_DAMAGE_TAGS)
        .map(|index| id(&format!("test:damage_{index}")))
        .collect();
    let damage = EntityLootContext::new(
        entity("minecraft:cow"),
        EntityDeathCause::new(damage_tags, EntityLootAttack::None, None),
        LootRandomBinding::new(None, 1),
    );
    let error = loot
        .evaluate(&table, &damage)
        .expect_err("damage tag scans must be bounded before output");
    assert!(matches!(
        error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::ContextDamageTags,
            ..
        }
    ));
}

#[test]
fn runtime_recursion_is_bounded_without_panicking() {
    let mut entry = r#"{"type":"minecraft:item","name":"minecraft:bone"}"#.to_string();
    for _ in 0..=MAX_RUNTIME_RECURSION {
        entry = format!(r#"{{"type":"minecraft:alternatives","children":[{entry}]}}"#);
    }
    let raw = one_item_table("minecraft:entity", &entry);
    let loot = catalog("minecraft:entities/deep_runtime", &raw);
    let error = loot
        .evaluate(
            &id("minecraft:entities/deep_runtime"),
            &context("minecraft:cow"),
        )
        .expect_err("runtime recursion must be bounded");
    assert!(matches!(
        error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::RuntimeRecursion,
            ..
        }
    ));
}

#[test]
fn output_stack_item_and_operation_budgets_are_independent() {
    let stack_raw = format!(
        r#"{{
          "type":"minecraft:entity",
          "pools":[{{
            "rolls":{},
            "entries":[{{"type":"minecraft:item","name":"minecraft:bone"}}]
          }}]
        }}"#,
        MAX_OUTPUT_STACKS + 1
    );
    let stack_error = catalog("minecraft:entities/stacks", &stack_raw)
        .evaluate(&id("minecraft:entities/stacks"), &context("minecraft:cow"))
        .expect_err("output stacks must be bounded");
    assert!(matches!(
        stack_error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::OutputStacks,
            ..
        }
    ));

    let item_raw = one_item_table(
        "minecraft:entity",
        &format!(
            r#"{{
              "type":"minecraft:item",
              "name":"minecraft:bone",
              "functions":[{{"function":"minecraft:set_count","count":{}}}]
            }}"#,
            MAX_OUTPUT_ITEMS + 1
        ),
    );
    let item_error = catalog("minecraft:entities/items", &item_raw)
        .evaluate(&id("minecraft:entities/items"), &context("minecraft:cow"))
        .expect_err("total output items must be bounded");
    assert!(matches!(
        item_error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::OutputItems,
            ..
        }
    ));

    let entries = (0..64)
        .map(|index| {
            format!(r#"{{"type":"minecraft:item","name":"test:item_{index}","weight":0}}"#)
        })
        .chain(std::iter::once(
            r#"{"type":"minecraft:item","name":"minecraft:bone"}"#.to_string(),
        ))
        .collect::<Vec<_>>()
        .join(",");
    let operations_raw = format!(
        r#"{{
          "type":"minecraft:entity",
          "pools":[{{"rolls":{},"entries":[{entries}]}}]
        }}"#,
        MAX_OUTPUT_STACKS
    );
    let operation_error = catalog("minecraft:entities/operations", &operations_raw)
        .evaluate(
            &id("minecraft:entities/operations"),
            &context("minecraft:cow"),
        )
        .expect_err("total operations must be bounded independently of output stacks");
    assert!(matches!(
        operation_error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::TotalOperations,
            ..
        }
    ));
}

#[test]
fn weighted_candidate_selection_second_scan_consumes_operations() {
    let entries =
        (0..30)
            .map(|index| {
                format!(r#"{{"type":"minecraft:item","name":"test:zero_{index}","weight":0}}"#)
            })
            .chain((0..2).map(|index| {
                format!(r#"{{"type":"minecraft:item","name":"test:positive_{index}"}}"#)
            }))
            .collect::<Vec<_>>()
            .join(",");
    let raw = format!(
        r#"{{
          "type":"minecraft:entity",
          "pools":[{{"rolls":512,"entries":[{entries}]}}]
        }}"#
    );
    let error = catalog("minecraft:entities/weighted_scan", &raw)
        .evaluate(
            &id("minecraft:entities/weighted_scan"),
            &context("minecraft:cow"),
        )
        .expect_err("the second weighted candidate scan must consume the operation budget");
    assert!(matches!(
        error,
        EntityLootEvaluationError::LimitExceeded {
            limit: EntityLootLimit::TotalOperations,
            ..
        }
    ));
}

#[test]
#[ignore = "requires a local 26.1.2 entity loot-table sidecar"]
fn local_26_1_2_entity_corpus_has_closed_references_when_sidecar_is_present() {
    let data_root = local_data_root().expect(
        "a 26.1.2 sidecar data root is required under .analysis/client-automation, \
         .analysis/decompiled, or data/vanilla",
    );
    let entity_dir = data_root.join("minecraft/loot_table/entities");
    let roots = resource_ids_below(&data_root, &entity_dir);
    let catalog = EntityLootCatalog::compile_resources(&data_root, roots).unwrap();
    let inventory = catalog.inventory();

    assert_eq!(
        inventory.root_count,
        108,
        "local corpus at {}",
        data_root.display()
    );
    assert_eq!(
        inventory.table_count, 109,
        "entity closure includes fishing/fish"
    );
    assert_eq!(
        inventory.table_types,
        BTreeSet::from([
            "minecraft:entity".to_string(),
            "minecraft:fishing".to_string(),
        ])
    );
    assert_eq!(inventory.reference_count, 18);
}

fn write_resource(root: &Path, table_id: &str, raw: &str) {
    let table_id = id(table_id);
    let path = root
        .join(table_id.namespace())
        .join("loot_table")
        .join(table_id.path())
        .with_extension("json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, raw).unwrap();
}

fn local_data_root() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        ".analysis/client-automation/versions/26.1.2/sidecar/data",
        ".analysis/decompiled/server-26.1.2/data",
        "data/vanilla/data",
    ];
    let workspace = option_env!("CARGO_MANIFEST_DIR")
        .and_then(|manifest| Path::new(manifest).parent()?.parent())
        .map(Path::to_path_buf);
    CANDIDATES.iter().find_map(|candidate| {
        let candidate = PathBuf::from(candidate);
        if candidate.is_dir() {
            return Some(candidate);
        }
        workspace
            .as_ref()
            .map(|workspace| workspace.join(candidate))
            .filter(|path| path.is_dir())
    })
}

fn resource_ids_below(data_root: &Path, root: &Path) -> Vec<Identifier> {
    let mut pending = vec![root.to_path_buf()];
    let mut paths = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else if entry.path().extension().is_some_and(|ext| ext == "json") {
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(data_root).unwrap();
            let namespace = relative.components().next().unwrap().as_os_str();
            let table_path = relative
                .strip_prefix(namespace)
                .unwrap()
                .strip_prefix("loot_table")
                .unwrap()
                .with_extension("")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            id(&format!("{}:{table_path}", namespace.to_string_lossy()))
        })
        .collect()
}

struct ExpectedRandom {
    state: u64,
}

impl ExpectedRandom {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn uniform(&mut self, min: u32, max: u32) -> u32 {
        min + u32::try_from(self.bounded(u64::from(max - min + 1))).unwrap()
    }

    fn bounded(&mut self, bound: u64) -> u64 {
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let value = self.next_u64();
            if value >= threshold {
                return value % bound;
            }
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }
}
