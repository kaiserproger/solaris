//! The startup contribution of a component deployment.
//!
//! `configure` is the only phase that answers a startup contribution, and the
//! rules it resolves to are what a world opens against: their fingerprint is
//! persisted in the world contract and compared on every reopen. These cases fix
//! that contract from the component side - the conversion a component's
//! `startup-contribution` takes into the startup rules, the refusals that keep an
//! unmaterializable contribution out of a world, and what a started deployment
//! reports about the packages' answers.
//!
//! The component-path cases run the real fixture: a real Rust plugin compiled to
//! `wasm32-unknown-unknown` and encoded as a component by `wit-component`, whose
//! `mode = "nested"` answers a real `startup-contribution`. The fixture's
//! contribution is a set of tree declarations, so the bounds it can drive are the
//! tree ones; the direct cases below cover the rest of the contract's shape, which
//! no config can reach.

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use fixture::component_bytes;
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::{
    ClayRules, ItemDefinition, PlacementRules, SpawnEntry as ContributionEntry, SpawnGroup,
    SpawnRules, StartupContribution, TreeRules,
};
use mc_plugin_host::{
    ContributionOutcome, ContributionRefusal, DeploymentConfig, DiscoveryMode, FieldOverflow,
    HostQueues, NoSessions, PluginLimits, check_deployment, convert_startup_contribution,
    start_deployment,
};
use mc_script::{
    BiomeSpawns, ClayRule, GameplayRules, GameplayRulesError, SpawnEntry, SpawnPlacement, TreeRule,
};

/// A component that declares no contribution at all: every category absent.
fn no_categories() -> StartupContribution {
    StartupContribution {
        placement: None,
        trees: None,
        clay: None,
        spawning: None,
        items: None,
    }
}

#[test]
fn item_only_contribution_preserves_identity_and_refuses_invalid_definitions() {
    let mut item = ItemDefinition {
        id: "ruby-live:ruby".to_owned(),
        carrier: "minecraft:paper".to_owned(),
        name: "Ruby".to_owned(),
        max_stack_size: 16,
        max_damage: None,
        weapon: false,
        attack_damage_modifier: None,
        attack_speed_modifier: None,
        equippable_slot: None,
        crafting_ingredient: Some("minecraft:paper".to_owned()),
    };
    let contribution = |item: ItemDefinition| StartupContribution {
        items: Some(vec![item]),
        ..no_categories()
    };
    let validated = convert_startup_contribution("ruby-live", &contribution(item.clone())).unwrap();
    assert!(validated.rules().is_none());
    assert_eq!(validated.items()[0].id.as_str(), "ruby-live:ruby");
    assert_eq!(
        validated.items()[0]
            .crafting_ingredient
            .as_ref()
            .unwrap()
            .as_str(),
        "minecraft:paper"
    );

    item.max_stack_size = 0;
    assert!(matches!(
        convert_startup_contribution("ruby-live", &contribution(item.clone())),
        Err(ContributionRefusal::InvalidItem(_))
    ));
    item.max_stack_size = 16;
    item.id = "someone-else:ruby".to_owned();
    assert!(matches!(
        convert_startup_contribution("ruby-live", &contribution(item.clone())),
        Err(ContributionRefusal::InvalidItem(_))
    ));
    item.id = "ruby-live:ruby".to_owned();
    item.crafting_ingredient = Some("ruby-live:missing".to_owned());
    assert!(matches!(
        convert_startup_contribution("ruby-live", &contribution(item)),
        Err(ContributionRefusal::InvalidItem(_))
    ));
}

/// A contribution whose only category is one tree declaration over one biome.
fn one_tree(spacing: u32) -> StartupContribution {
    StartupContribution {
        placement: None,
        trees: Some(vec![TreeRules {
            biomes: vec!["minecraft:plains".to_owned()],
            spacing,
            density_threshold: 0.25,
        }]),
        clay: None,
        spawning: None,
        items: None,
    }
}

#[test]
fn component_rules_fingerprint_matches_its_independent_construction() {
    // `contract_name` is persisted in `PersistedWorldContract.gameplay_rules`
    // and compared on every reopen, so conversion and independent construction
    // must produce the same component contract fingerprint.
    let plan = StartupContribution {
        placement: Some(PlacementRules {
            land_spacing: 2,
            water_attempts: 8,
            water_depth: 4,
        }),
        trees: Some(vec![TreeRules {
            biomes: vec!["minecraft:plains".to_owned(), "minecraft:forest".to_owned()],
            spacing: 12,
            density_threshold: 0.25,
        }]),
        clay: Some(ClayRules {
            rarity: 40,
            radius_min: 1,
            radius_max: 3,
            max_water_depth: 6,
        }),
        spawning: Some(vec![SpawnRules {
            biome: "minecraft:plains".to_owned(),
            groups: vec![
                SpawnGroup {
                    group: "creature".to_owned(),
                    entries: vec![ContributionEntry {
                        entity: "minecraft:cow".to_owned(),
                        min: 2,
                        max: 4,
                        weight: 100,
                    }],
                },
                SpawnGroup {
                    group: "monster".to_owned(),
                    entries: vec![ContributionEntry {
                        entity: "minecraft:zombie".to_owned(),
                        min: 1,
                        max: 2,
                        weight: 50,
                    }],
                },
            ],
        }]),
        items: None,
    };
    let from_component = convert_startup_contribution("planner", &plan)
        .expect("a plan inside every bound is accepted")
        .rules()
        .cloned()
        .expect("rules");
    let independently_constructed = GameplayRules::new(
        vec![BiomeSpawns::new(
            "minecraft:plains",
            BTreeMap::from([
                (
                    "creature".to_owned(),
                    vec![SpawnEntry::new("minecraft:cow", 2, 4, 100)],
                ),
                (
                    "monster".to_owned(),
                    vec![SpawnEntry::new("minecraft:zombie", 1, 2, 50)],
                ),
            ]),
        )],
        vec![TreeRule::new(
            vec!["minecraft:plains".to_owned(), "minecraft:forest".to_owned()],
            12,
            0.25,
        )],
        Some(ClayRule::new(40, 1, 3, 6)),
        Some(SpawnPlacement::new(2, 8, 4)),
    );
    // Both sides are assembled by different code - one by the conversion, one by
    // the contract's own constructor - so equality is the conversion carrying
    // every field, not the two sides sharing a literal.
    assert_eq!(from_component, independently_constructed);
    assert_eq!(
        from_component.contract_name(),
        independently_constructed.contract_name(),
        "the fingerprint must not depend on the construction path"
    );
    assert!(
        from_component
            .contract_name()
            .starts_with("component-rules:"),
        "the prefix is part of the persisted value: {}",
        from_component.contract_name()
    );
    independently_constructed
        .validate()
        .expect("the independently constructed rules validate");
}

#[test]
fn a_value_wider_than_its_contract_field_is_refused_rather_than_truncated() {
    // WIT declares every numeric bound as `u32`. The startup contract holds the
    // three `placement-rules` bounds and clay's three in a single byte each, so
    // those are the widths a component can overflow, and `300` truncated to `44`
    // would be a different rule than the package declared - refused, not repaired.
    //
    // The other direction of the correspondence, `tree-rules.spacing` (`u32`)
    // against `TreeRule.spacing` (`u64`), has no case here and does not need one:
    // widening cannot overflow, and a guest cannot declare more than the WIT's
    // own `u32` allows, so the widest spacing a plan can carry is asserted below
    // to arrive exactly.
    let clay_and_placement = |radius_min: u32, water_depth: u32| StartupContribution {
        placement: Some(PlacementRules {
            land_spacing: 2,
            water_attempts: 8,
            water_depth,
        }),
        trees: None,
        clay: Some(ClayRules {
            rarity: 40,
            radius_min,
            radius_max: 3,
            max_water_depth: 6,
        }),
        spawning: None,
        items: None,
    };

    // 272 truncated to a byte is 16, which is a legal water depth: this is the
    // case that tells "refused" apart from "repaired into something plausible".
    // A runtime that narrowed instead of refusing would materialize a plan no
    // package declared, and the world fingerprint would record it as if it had.
    assert_eq!(
        convert_startup_contribution("planner", &clay_and_placement(3, 272))
            .expect_err("272 does not fit the contract's u8, even though its low byte does"),
        ContributionRefusal::Overflow(FieldOverflow {
            field: "placement-rules.water-depth",
            value: 272,
            max: u64::from(u8::MAX),
        })
    );
    assert_eq!(
        convert_startup_contribution("planner", &clay_and_placement(300, 4))
            .expect_err("300 does not fit the contract's u8"),
        ContributionRefusal::Overflow(FieldOverflow {
            field: "clay-rules.radius-min",
            value: 300,
            max: u64::from(u8::MAX),
        })
    );
    let accepted = convert_startup_contribution("planner", &clay_and_placement(3, 4))
        .expect("3 fits the contract's u8")
        .rules()
        .cloned()
        .expect("rules");
    assert_eq!(
        accepted.clay.expect("clay is declared").radius_min,
        3,
        "a value inside the field's width arrives unchanged"
    );

    assert_eq!(
        convert_startup_contribution("planner", &clay_and_placement(3, 300))
            .expect_err("a water depth of 300 does not fit the contract's u8"),
        ContributionRefusal::Overflow(FieldOverflow {
            field: "placement-rules.water-depth",
            value: 300,
            max: u64::from(u8::MAX),
        })
    );
    // 272 truncated to a byte is 16, which is a legal water depth: this is the
    // case that tells "refused" apart from "repaired into something plausible".
    // A runtime that narrowed instead of refusing would materialize a plan no
    // package declared, and the world fingerprint would record it as if it had.
    assert_eq!(
        convert_startup_contribution("planner", &clay_and_placement(3, 272))
            .expect_err("272 does not fit the contract's u8, even though its low byte does"),
        ContributionRefusal::Overflow(FieldOverflow {
            field: "placement-rules.water-depth",
            value: 272,
            max: u64::from(u8::MAX),
        })
    );

    let widest = convert_startup_contribution("planner", &one_tree(u32::MAX))
        .expect("a u32 spacing is always inside the contract's u64")
        .rules()
        .cloned()
        .expect("rules");
    assert_eq!(
        widest.trees[0].spacing,
        u64::from(u32::MAX),
        "the widening carries the whole value"
    );
}

#[test]
fn a_contribution_that_breaks_an_enforced_bound_is_refused_with_the_check_that_refuses_it() {
    // Each refusal names the production check that produces it, so a component
    // author gets the same actionable reason for an invalid declaration.
    let declarations = |count: usize| StartupContribution {
        placement: None,
        trees: Some(
            (0..count)
                .map(|index| TreeRules {
                    biomes: vec![format!("minecraft:biome_{index}")],
                    spacing: 1,
                    density_threshold: 0.0,
                })
                .collect(),
        ),
        clay: None,
        spawning: None,
        items: None,
    };
    convert_startup_contribution("planner", &declarations(64))
        .expect("64 tree declarations are the bound");
    assert_eq!(
        convert_startup_contribution("planner", &declarations(65))
            .expect_err("65 declarations exceed the bound"),
        ContributionRefusal::Invalid(GameplayRulesError::TooManyDeclarations)
    );

    let spawn_group = |group: &str, min: u32, max: u32| StartupContribution {
        placement: None,
        trees: None,
        clay: None,
        spawning: Some(vec![SpawnRules {
            biome: "minecraft:plains".to_owned(),
            groups: vec![SpawnGroup {
                group: group.to_owned(),
                entries: vec![ContributionEntry {
                    entity: "minecraft:cow".to_owned(),
                    min,
                    max,
                    weight: 10,
                }],
            }],
        }]),
        items: None,
    };
    convert_startup_contribution("planner", &spawn_group("creature", 1, 4))
        .expect("a supported group is accepted");
    assert_eq!(
        convert_startup_contribution("planner", &spawn_group("boss", 1, 4))
            .expect_err("boss is not a spawn group"),
        ContributionRefusal::Invalid(GameplayRulesError::UnsupportedSpawnGroup)
    );
    assert_eq!(
        convert_startup_contribution("planner", &spawn_group("creature", 4, 1))
            .expect_err("min may not exceed max"),
        ContributionRefusal::Invalid(GameplayRulesError::InvalidSpawnEntry)
    );

    // A repeated group name has no second slot in the contract's map: keeping
    // either replacement would silently drop the other one the package asked for.
    let duplicated = StartupContribution {
        spawning: Some(vec![SpawnRules {
            biome: "minecraft:plains".to_owned(),
            groups: vec![
                SpawnGroup {
                    group: "creature".to_owned(),
                    entries: vec![ContributionEntry {
                        entity: "minecraft:cow".to_owned(),
                        min: 1,
                        max: 4,
                        weight: 10,
                    }],
                },
                SpawnGroup {
                    group: "creature".to_owned(),
                    entries: vec![ContributionEntry {
                        entity: "minecraft:sheep".to_owned(),
                        min: 1,
                        max: 2,
                        weight: 5,
                    }],
                },
            ],
        }]),
        items: None,
        ..no_categories()
    };
    assert_eq!(
        convert_startup_contribution("planner", &duplicated)
            .expect_err("a group name declared twice is refused"),
        ContributionRefusal::DuplicateSpawnGroup {
            biome: "minecraft:plains".to_owned(),
            group: "creature".to_owned(),
        }
    );
}

#[test]
fn an_empty_contribution_is_refused() {
    // WIT's own comment on `startup-contribution`: "the host rejects a
    // contribution that sets none".
    assert_eq!(
        convert_startup_contribution("planner", &no_categories())
            .expect_err("a plan with no category is refused"),
        ContributionRefusal::Invalid(GameplayRulesError::NoCategory)
    );
    // An empty *list* is a set category that sets nothing, and `validate` refuses
    // it by the same check: `option<list<..>>` says whether the package set the
    // category, not whether the category ended up empty.
    assert_eq!(
        convert_startup_contribution(
            "planner",
            &StartupContribution {
                trees: Some(Vec::new()),
                ..no_categories()
            }
        )
        .expect_err("an empty category list is still an empty plan"),
        ContributionRefusal::Invalid(GameplayRulesError::NoCategory)
    );
}

/// Write one package of the fixture's component into `root`.
fn write_package(root: &Path, id: &str, config: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n"),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    if !config.is_empty() {
        std::fs::write(directory.join("config.toml"), config).expect("config");
    }
}

fn deployment(root: &Path, expected: &[&str]) -> DeploymentConfig {
    DeploymentConfig {
        root: root.to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: expected.iter().map(|id| (*id).to_owned()).collect(),
        grants: BTreeMap::new(),
        require_grants: false,
        precommit_hooks: Vec::new(),
    }
}

/// The fixture's `nested` mode answers a startup contribution: `count` tree
/// declarations whose one biome is a name of `size` bytes repeated 64 times.
fn nested_plan(size: usize, count: usize) -> String {
    format!("mode = \"nested\"\nsize = {size}\ncount = {count}\n")
}

#[test]
fn a_started_deployment_reports_a_refused_contribution_and_an_absent_one_apart() {
    let root = tempfile::tempdir().expect("deployment root");
    // One package answers no plan, which is ordinary; the other answers a plan
    // whose 64 biomes all share one name, which the startup contract refuses.
    write_package(root.path(), "quiet", "");
    write_package(root.path(), "noisy", &nested_plan(4, 1));
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(&deployment(root.path(), &["noisy", "quiet"]), &limits)
        .expect("the deployment discovery succeeds")
        .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect(
        "a package whose plan is refused still starts: failing startup belongs to the \
             deployment that reads the contribution, not to the runtime",
    );

    let contribution = host.contribution();
    let package = |id: &str| {
        contribution
            .packages()
            .iter()
            .find(|package| package.id() == id)
            .unwrap_or_else(|| panic!("{id} is part of the deployment"))
    };
    assert_eq!(
        package("quiet").outcome(),
        &ContributionOutcome::NoContribution,
        "a package that declares nothing is not a refusal"
    );
    assert!(package("quiet").refusal().is_none());
    assert!(
        package("quiet").rules().is_none(),
        "and it contributes no rules"
    );
    assert_eq!(
        package("noisy").refusal(),
        Some(&ContributionRefusal::Invalid(
            GameplayRulesError::InvalidTreeBiome
        )),
        "the refusal names the check that produced it"
    );
    assert!(package("noisy").rules().is_none());
    assert_eq!(
        contribution
            .refusal()
            .map(|(id, refusal)| (id, refusal.clone())),
        Some((
            "noisy",
            ContributionRefusal::Invalid(GameplayRulesError::InvalidTreeBiome)
        )),
        "a caller that fails startup closed reads the refusal and the package that caused it"
    );
    assert_eq!(
        contribution.rules().count(),
        0,
        "a refused plan contributes no rules to a world"
    );
    host.stop();
}

#[test]
fn a_check_refuses_the_same_plans_the_run_path_refuses() {
    // `--check` reports what the run path would materialize, so a plan the startup
    // contract refuses has to fail the check: a check that answered "fine" for a
    // deployment that cannot open its world would be the one lie it must not tell.
    for (config, check) in [
        (nested_plan(4, 1), GameplayRulesError::InvalidTreeBiome),
        (nested_plan(1, 65), GameplayRulesError::TooManyDeclarations),
        (nested_plan(1, 0), GameplayRulesError::NoCategory),
    ] {
        let root = tempfile::tempdir().expect("deployment root");
        write_package(root.path(), "noisy", &config);
        let error = check_deployment(
            &deployment(root.path(), &["noisy"]),
            &PluginLimits::default(),
        )
        .expect_err("a plan the startup contract refuses fails the check");
        let message = format!("{error}");
        assert!(message.contains("noisy"), "{message}");
        assert!(
            message.contains(check.message()),
            "the check reports {check:?}, saw {message}"
        );
    }
}

#[test]
fn a_check_reports_the_rules_the_run_path_materializes() {
    // Both paths run the same two startup phases in the same two stores, so the
    // rules a `--check` reports have to be the rules a started deployment records:
    // a world opened after a green check must be the world the check verified.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path(), "planner", "mode = \"placement\"\n");
    let limits = PluginLimits::default();
    let configuration = deployment(root.path(), &["planner"]);

    let report = check_deployment(&configuration, &limits).expect("the deployment checks");
    let checked = report
        .checked()
        .first()
        .expect("the check reports the one package");
    let checked_rules = checked
        .plan
        .as_ref()
        .expect("the fixture's own contribution validates");

    let packages = mc_plugin_host::discover(&configuration, &limits)
        .expect("the deployment discovery succeeds")
        .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("the deployment starts");
    assert!(host.contribution().refusal().is_none());
    let (id, rules) = host
        .contribution()
        .rules()
        .next()
        .expect("the run path records the contribution it ran configure for");
    assert_eq!(id, "planner");
    assert_eq!(
        rules, checked_rules,
        "the rules a check reports are the rules the run path records"
    );
    host.stop();
}
