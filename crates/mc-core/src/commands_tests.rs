use mc_data::item_components::{ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;
use mc_data::{Identifier, ItemStack};

use super::{
    Command, CommandError, Coord, Destination, GameMode, Target, clear, debug_give, give, parse,
};
use crate::inventory::{ItemRules, PlayerInventory};

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}

#[test]
fn selector_nested_options_and_duplicate_score_filter() {
    assert!(
        parse(
            "kill @e[scores={a=1,b=..2},nbt={CustomName:'a,b'},nbt=!{Missing:1b}]",
            true,
        )
        .is_ok()
    );
    assert!(parse("kill @e[scores={a=1},scores={b=2}]", true).is_err());
}

#[test]
fn advancement_actions_and_scopes_parse_with_operator_permissions() {
    use super::AdvancementScope;
    for (action, grant) in [("grant", true), ("revoke", false)] {
        for (mode, scope) in [
            ("everything", AdvancementScope::Everything),
            ("only", AdvancementScope::Only),
            ("from", AdvancementScope::From),
            ("through", AdvancementScope::Through),
            ("until", AdvancementScope::Until),
        ] {
            let suffix = if scope == AdvancementScope::Everything {
                ""
            } else {
                " story/root"
            };
            assert_eq!(
                parse(&format!("advancement {action} @a {mode}{suffix}"), true),
                Ok(Command::Advancement {
                    target: Target::AllPlayers,
                    grant,
                    scope,
                    advancement: (scope != AdvancementScope::Everything)
                        .then(|| "minecraft:story/root".into()),
                    criterion: None,
                }),
            );
        }
    }
    assert!(matches!(
        parse("advancement grant @s only story/root criterion with spaces", true),
        Ok(Command::Advancement { criterion: Some(criterion), .. }) if criterion == "criterion with spaces"
    ));
    for input in [
        "advancement grant @s",
        "advancement grant @s everything story/root",
        "advancement grant @s from",
        "advancement grant @s until story/root criterion",
        "advancement grant @s through story/root criterion",
        "advancement grant @s from story/root criterion",
        "advancement remove @s everything",
    ] {
        assert!(parse(input, true).is_err(), "{input}");
    }
    assert_eq!(
        parse("advancement grant @s everything", false),
        Err(CommandError::Unknown)
    );
}

#[test]
fn harness_fixtures_parse() {
    assert_eq!(
        parse("debug give minecraft:apple 2 1", true),
        Ok(Command::DebugGive {
            item: crate::command_items::ItemArgument::plain(id("minecraft:apple")),
            count: 2,
            hotbar_slot: 1
        })
    );
    assert_eq!(
        parse("kill", true),
        Ok(Command::Kill {
            target: Target::Executor
        })
    );
}

#[test]
fn operator_commands_are_unknown_to_non_operators() {
    assert_eq!(
        parse("gamemode creative", false),
        Err(CommandError::Unknown)
    );
    assert_eq!(parse("frobnicate", true), Err(CommandError::Unknown));
}

#[test]
fn teleport_accepts_relative_coordinates_and_entity_targets() {
    assert_eq!(
        parse("tp ~ ~10 -5.5", true),
        Ok(Command::Teleport {
            target: Target::Executor,
            destination: Destination::Position([
                Coord {
                    relative: true,
                    local: false,
                    value: 0.0
                },
                Coord {
                    relative: true,
                    local: false,
                    value: 10.0
                },
                Coord {
                    relative: false,
                    local: false,
                    value: -5.5
                },
            ]),
            rotation: None,
        })
    );
    assert_eq!(
        parse("tp Alice Bob", true),
        Ok(Command::Teleport {
            target: Target::Player("Alice".into()),
            destination: Destination::Entity(Target::Player("Bob".into())),
            rotation: None,
        })
    );
    assert!(matches!(
        parse("tp 1 2 3 4 5", true),
        Ok(Command::Teleport {
            rotation: Some(_),
            ..
        })
    ));
    assert!(parse("tp Bob 1 2 3 ~90 ~", true).is_ok());
    assert!(parse("tp 1 2 3 ^ ^", true).is_err());
    assert!(matches!(
        parse("tp 1e9 0 0", true),
        Err(CommandError::Usage(_))
    ));
}

#[test]
fn gamemode_give_time_and_xp_parse_vanilla_forms() {
    assert_eq!(
        parse("gamemode creative @a", true),
        Ok(Command::GameMode {
            mode: GameMode::Creative,
            target: Target::AllPlayers
        })
    );
    assert_eq!(
        parse("give @s stone 64", true),
        Ok(Command::Give {
            target: Target::Executor,
            item: crate::command_items::ItemArgument::plain(id("minecraft:stone")),
            count: 64
        })
    );
    assert_eq!(
        parse("time set noon", true),
        Ok(Command::TimeMarker {
            name: "minecraft:noon".into(),
            ticks: 6_000,
        })
    );
    assert_eq!(
        parse("time add 2d", true),
        Ok(Command::Time {
            ticks: 48_000,
            add: true
        })
    );
    assert_eq!(
        parse("xp add @s 5 levels", true),
        Ok(Command::Experience {
            target: Target::Executor,
            amount: 5,
            levels: true
        })
    );
    assert!(matches!(
        parse("give @s stone 0", true),
        Err(CommandError::Message(_))
    ));
}

struct Data {
    items: ItemRegistry,
    facts: ItemFactsTable,
    tags: TagsData,
}

impl Data {
    fn new() -> Self {
        Self {
            items: ItemRegistry::from_report(&[
                ItemReport {
                    id: id("minecraft:stone"),
                    protocol_id: 1,
                },
                ItemReport {
                    id: id("minecraft:ender_pearl"),
                    protocol_id: 2,
                },
            ]),
            facts: ItemFactsTable::from_entries([
                (
                    id("minecraft:stone"),
                    ItemFacts {
                        max_stack_size: Some(64),
                        ..ItemFacts::default()
                    },
                ),
                (
                    id("minecraft:ender_pearl"),
                    ItemFacts {
                        max_stack_size: Some(16),
                        ..ItemFacts::default()
                    },
                ),
            ]),
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
}

#[test]
fn give_splits_into_max_stacks_and_drops_what_does_not_fit() {
    let data = Data::new();
    let mut inventory = PlayerInventory::default();
    for slot in 1..36 {
        inventory.slots[slot] = ItemStack::new(1, 64);
    }
    let dropped = give(&data.rules(), &mut inventory, &ItemStack::new(2, 1), 40);
    assert_eq!(inventory.slots[0], ItemStack::new(2, 16));
    let dropped_total: i32 = dropped.iter().map(|stack| stack.count).sum();
    assert_eq!(dropped_total, 24);
    assert!(dropped.iter().all(|stack| stack.count <= 16));
}

#[test]
fn debug_give_sets_the_hotbar_slot_outright() {
    let mut inventory = PlayerInventory::default();
    inventory.slots[1] = ItemStack::new(1, 5);
    debug_give(&mut inventory, ItemStack::new(2, 1), 2, 1);
    assert_eq!(inventory.slots[1], ItemStack::new(2, 2));
    debug_give(&mut inventory, ItemStack::new(2, 1), 0, 1);
    assert!(inventory.slots[1].is_empty());
}

#[test]
fn clear_counts_with_zero_and_removes_up_to_max() {
    let mut inventory = PlayerInventory::default();
    inventory.slots[0] = ItemStack::new(1, 10);
    inventory.slots[5] = ItemStack::new(1, 10);
    inventory.slots[6] = ItemStack::new(2, 3);
    assert_eq!(
        clear(&mut inventory, |stack| stack.item_id == 1, Some(0)),
        20
    );
    assert_eq!(inventory.slots[0].count, 10);
    assert_eq!(
        clear(&mut inventory, |stack| stack.item_id == 1, Some(15)),
        15
    );
    assert_eq!(inventory.slots[5].count, 5);
    assert_eq!(clear(&mut inventory, |_| true, None), 8);
    assert!(inventory.slots.iter().all(ItemStack::is_empty));
}

#[test]
fn world_time_weather_difficulty_and_summon_parse_complete_forms() {
    for command in [
        "time query time",
        "time query gametime",
        "time set minecraft:noon",
        "time add -1s",
        "weather rain 10s",
        "weather clear",
        "difficulty",
        "difficulty peaceful",
        "gamerule random_tick_speed 0",
        "gamerule minecraft:keep_inventory true",
        "summon cow ~ ~1 ~",
        "setblock ~ ~ ~ stone",
        "fill 0 80 0 1 81 1 stone hollow",
        "spawnpoint @a ~ ~ ~ 0 0",
        "setworldspawn 0 80 0 0 0",
    ] {
        assert!(parse(command, true).is_ok(), "{command}");
    }
    for command in [
        "weather rain 10s extra",
        "time set -1",
        "time query unknown",
        "difficulty hard extra",
        "summon cow ~ ~",
        "gamerule made_up true",
    ] {
        assert!(parse(command, true).is_err(), "{command}");
    }
}

#[test]
fn experience_set_query_and_public_chat_are_typed() {
    assert!(matches!(
        parse("experience set @a 8 levels", true),
        Ok(Command::ExperienceSet {
            amount: 8,
            levels: true,
            ..
        })
    ));
    assert!(matches!(
        parse("xp query @s points", true),
        Ok(Command::ExperienceQuery { levels: false, .. })
    ));
    assert!(matches!(
        parse("xp set @s -1 levels", true),
        Err(CommandError::Message(_))
    ));
    assert!(matches!(
        parse("me hello ] unmatched", false),
        Ok(Command::Me {
            selectors: false,
            ..
        })
    ));
    for name in ["msg", "tell", "w"] {
        assert!(matches!(
            parse(&format!("{name} Alice hello  world ]"), false),
            Ok(Command::Message {
                selectors: false,
                ..
            })
        ));
    }
    assert!(parse("say hello", false).is_err());
    assert!(parse("msg @a hi", false).is_err());
}

#[test]
fn selectors_validate_filters_bounds_and_balanced_quoted_names() {
    for target in [
        "@p",
        "@a",
        "@r",
        "@s",
        "@e",
        "@e[type=cow,distance=..5,sort=nearest,limit=2]",
        "@a[name=\"A B\"]",
        "@e[type=!player,distance=2..]",
    ] {
        assert!(super::target_of(target).is_ok(), "{target}");
    }
    assert!(parse("give @a[name=\"A B\"] stone", true).is_ok());
    for target in [
        "@x",
        "@e[limit=0]",
        "@e[distance=-1]",
        "@e[distance=5..2]",
        "@e[sort=oops]",
        "@e[name=A,name=B]",
        "@e[missing]",
        "@e[limit=1",
    ] {
        assert!(super::target_of(target).is_err(), "{target}");
    }
}

#[test]
fn local_coordinates_follow_source_yaw_pitch_and_reject_mixed_frames() {
    let coords = super::coords(&["^1", "^2", "^3"]).unwrap();
    let at = super::resolve_coords(coords, [10.0, 20.0, 30.0], [0.0, 0.0]);
    for (actual, expected) in at.into_iter().zip([11.0, 22.0, 33.0]) {
        assert!((actual - expected).abs() < 1.0e-10);
    }
    let at = super::resolve_coords(coords, [10.0, 20.0, 30.0], [90.0, 0.0]);
    for (actual, expected) in at.into_iter().zip([7.0, 22.0, 31.0]) {
        assert!((actual - expected).abs() < 1.0e-10);
    }
    assert!(parse("tp ^ ^ ^2", true).is_ok());
    assert!(parse("tp ^ ~ ^2", true).is_err());
    assert!(parse("summon cow ^ ^ ^1", true).is_ok());
}

#[test]
fn time_query_day_daytime_gametime_and_absolute_are_distinct() {
    for (name, expected) in [
        ("day", 2),
        ("daytime", 1234),
        ("gametime", 7),
        ("time", 49234),
    ] {
        let Command::TimeQuery(query) = parse(&format!("time query {name}"), true).unwrap() else {
            panic!("time query expected")
        };
        assert_eq!(query.value(i32::MAX as u64 + 7, 49234), expected);
    }
}
