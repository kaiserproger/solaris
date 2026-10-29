use super::*;
use crate::boundary::ChangeSet;
use mc_data::{Identifier, ItemStack};

fn fixture() -> (TickState, EntityId, Context) {
    let (mut state, id, _) = crate::hostile_tests::setup(8.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [1.75, 60.0, 2.25]);
    let context = Context::source(&state, crate::command_console::CONNECTION, id, 2);
    (state, id, context)
}
fn run(state: &mut TickState, context: Context, command: &str) {
    crate::command_context::run(state, command, context, &mut ChangeSet::default());
}
fn score(state: &TickState, holder: &str) -> i32 {
    state
        .scoreboard
        .scores
        .get(holder)
        .and_then(|scores| scores.get("oracle"))
        .unwrap_or_else(|| {
            panic!(
                "missing {holder} score; objectives={:?}; feedback={:?}",
                state.scoreboard.objectives, state.outbox
            )
        })
        .value
}
fn item(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}
#[test]
fn data_paths_cover_filters_quoted_keys_arrays_and_result_types() {
    let root = crate::command_snbt::parse("{Inventory:[{Slot:0b,id:'minecraft:stone',count:3},{Slot:1b,id:'minecraft:dirt',count:4}],Pos:[-1.25d,60.0d,2.0d],UUID:[I;1,2,3,4],foo:{'a.b':{text:'a😀'}}}").unwrap();
    assert_eq!(
        *get(&root, "Inventory[{Slot:1b}].count").unwrap(),
        Tag::Int(4)
    );
    assert_eq!(*get(&root, "Pos[-1]").unwrap(), Tag::Double(2.0));
    assert_eq!(*get(&root, "UUID[0]").unwrap(), Tag::Int(1));
    assert_eq!(result(&get(&root, "Pos[0]").unwrap()), -2);
    assert_eq!(result(&get(&root, "Inventory").unwrap()), 2);
    assert_eq!(result(&get(&root, "foo.\"a.b\".text").unwrap()), 3);
    assert!(
        get(&root, "Inventory[].count")
            .unwrap_err()
            .contains("single NBT")
    );
    assert!(get(&root, "Inventory[{Slot:9b}]").is_err());
    assert!(get(&root, "Pos[100]").is_err());
    let mut input = " Inventory[{id: 'minecraft:stone'}].count 2";
    assert_eq!(
        argument(&mut input).unwrap(),
        "Inventory[{id: 'minecraft:stone'}].count"
    );
    assert_eq!(argument(&mut input).unwrap(), "2");
    assert!(argument(&mut input).is_err());
    assert_eq!(snbt(&root), "{Inventory: [{Slot: 0b, count: 3, id: \"minecraft:stone\"}, {Slot: 1b, count: 4, id: \"minecraft:dirt\"}], Pos: [-1.25d, 60.0d, 2.0d], UUID: [I; 1, 2, 3, 4], foo: {\"a.b\": {text: \"a😀\"}}}".replace("\"a.b\":", "a.b:"));
}
#[test]
fn data_player_queries_read_live_inventory_vitals_and_absent_paths() {
    let (mut state, id, context) = fixture();
    let stack = item(&state, "minecraft:stone", 7);
    let player = state.players.get_mut(&id).unwrap();
    player.inventory.slots[0] = stack;
    player.inventory.selected = 0;
    player.vitals.health = 13.5;
    player.vitals.food = 9;
    assert_eq!(
        query(&mut state, context, "data get entity @s Health").unwrap(),
        13
    );
    assert_eq!(
        query(&mut state, context, "data get entity @s Health 10").unwrap(),
        135
    );
    assert_eq!(
        query(&mut state, context, "data get entity @s foodLevel").unwrap(),
        9
    );
    assert_eq!(
        query(&mut state, context, "data get entity @s SelectedItem.count").unwrap(),
        7
    );
    assert_eq!(
        query(
            &mut state,
            context,
            "data get entity @s Inventory[{Slot:0b}].count"
        )
        .unwrap(),
        7
    );
    assert_eq!(
        query(&mut state, context, "data get entity @s Pos[0] -10").unwrap(),
        -18
    );
    assert_eq!(
        query(&mut state, context, "data get entity @s EnderItems").unwrap(),
        0
    );
    assert_eq!(query(&mut state, context, "data get entity @s").unwrap(), 1);
    let tag = entity(&state, id).unwrap();
    assert_eq!(
        *get(&tag, "Dimension").unwrap(),
        Tag::String("minecraft:overworld".into())
    );
    assert!(get(&tag, "SolarisStatistics").is_err());
    assert!(query(&mut state, context, "data get entity @s Pos 2").is_err());
    assert!(query(&mut state, context, "data get entity @s Missing").is_err());
    assert!(query(&mut state, context, "data get entity @a Health").is_err());
    assert!(
        query(
            &mut state,
            Context {
                permission: 1,
                ..context
            },
            "data get entity @s Health"
        )
        .is_err()
    );
    assert_eq!(state.players[&id].inventory.slots[0].count, 7);
}
#[test]
fn data_block_accepts_every_registered_block_entity_kind() {
    let (mut state, _, context) = fixture();
    for kind in mc_data::block_entity_kinds::BlockEntityKind::ALL {
        let path = kind.block_paths()[0];
        run(
            &mut state,
            context,
            &format!("setblock 4 60 4 minecraft:{path}"),
        );
        let tag =
            block(&state, BlockPos { x: 4, y: 60, z: 4 }).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(
            *get(&tag, "id").unwrap(),
            Tag::String(kind.vanilla_id().into()),
            "{path}"
        );
        assert_eq!(
            query(&mut state, context, "data get block 4 60 4 x").unwrap(),
            4
        );
    }
    assert!(query(&mut state, context, "data get block 1 100 1").is_err());
    assert!(query(&mut state, context, "data get block 10000 60 10000").is_err());
}
#[test]
fn data_block_inventory_uses_live_owner_and_furnace_clock() {
    let (mut state, _, context) = fixture();
    run(&mut state, context, "setblock 4 60 4 minecraft:furnace");
    let stack = item(&state, "minecraft:iron_ingot", 3);
    let live =
        state
            .block_entities
            .entry((4, 60, 4))
            .or_insert_with(|| crate::tick::BlockEntityState {
                slots: vec![ItemStack::EMPTY; 3],
                furnace: Some(Default::default()),
            });
    live.slots[2] = stack;
    live.furnace.as_mut().unwrap().cook_progress = 37;
    assert_eq!(
        query(
            &mut state,
            context,
            "data get block 4 60 4 Items[{Slot:2b}].count"
        )
        .unwrap(),
        3
    );
    assert_eq!(
        query(
            &mut state,
            context,
            "data get block 4 60 4 cooking_time_spent"
        )
        .unwrap(),
        37
    );
    assert_eq!(state.block_entities[&(4, 60, 4)].slots[2].count, 3);
}
#[test]
fn data_mob_query_projects_runtime_health_and_summoned_metadata() {
    let (mut state, _, context) = fixture();
    run(
        &mut state,
        context,
        "summon minecraft:villager 4 60 4 {Age:-123,VillagerData:{type:\"minecraft:plains\",profession:\"minecraft:none\",level:1}}",
    );
    assert_eq!(
        query(
            &mut state,
            context,
            "data get entity @e[type=minecraft:villager,limit=1] Age"
        )
        .unwrap(),
        -123
    );
    assert_eq!(
        query(
            &mut state,
            context,
            "data get entity @e[type=minecraft:villager,limit=1] VillagerData.level"
        )
        .unwrap(),
        1
    );
    assert_eq!(
        query(
            &mut state,
            context,
            "data get entity @e[type=minecraft:villager,limit=1] Health"
        )
        .unwrap(),
        20
    );
}
#[test]
fn execute_store_chains_results_success_and_failure_without_inventing_success() {
    let (mut state, _, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    run(
        &mut state,
        context,
        "execute store result score hp oracle store success score ok oracle run data get entity @s Health 10",
    );
    assert_eq!(score(&state, "hp"), 200);
    assert_eq!(score(&state, "ok"), 1);
    run(
        &mut state,
        context,
        "execute store result score hp oracle store success score ok oracle run data get entity @s absent",
    );
    assert_eq!(score(&state, "hp"), 0);
    assert_eq!(score(&state, "ok"), 0);
    run(
        &mut state,
        context,
        "execute store success score ok oracle run data get entity @s EnderItems",
    );
    assert_eq!(
        score(&state, "ok"),
        1,
        "successful zero result is not failure"
    );
    let before = state.day_time;
    run(
        &mut state,
        context,
        "execute store success score ok oracle run time add 123",
    );
    assert_eq!(
        state.day_time, before,
        "unsupported mutating producer must not execute"
    );
    let health = query(&mut state, context, "data get entity @s Health").unwrap();
    run(
        &mut state,
        context,
        "execute store success score ok oracle run damage @s 1 minecraft:generic_kill",
    );
    assert_eq!(
        query(&mut state, context, "data get entity @s Health").unwrap(),
        health,
        "damage without a typed result must be rejected before native admission"
    );
    assert_eq!(score(&state, "ok"), 0);
    assert!(state.scoreboard_dirty);
    assert!(state.command_context.current.is_none());
}
#[test]
fn execute_store_handles_query_producers_and_terminal_conditions() {
    let (mut state, _, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    state.day_time = 12345;
    for (command, expected) in [
        ("time query time", 12345),
        ("gamerule minecraft:advance_time", 1),
        ("worldborder get", 59999968),
    ] {
        run(
            &mut state,
            context,
            &format!("execute store result score value oracle run {command}"),
        );
        assert_eq!(score(&state, "value"), expected, "{command}");
    }
    run(
        &mut state,
        context,
        "execute store result score copy oracle run scoreboard players get value oracle",
    );
    assert_eq!(score(&state, "copy"), 59999968);
    run(
        &mut state,
        context,
        "execute store result score count oracle if entity @a",
    );
    assert_eq!(score(&state, "count"), 1);
    run(
        &mut state,
        context,
        "execute store result score count oracle unless entity @e[type=minecraft:villager]",
    );
    assert_eq!(score(&state, "count"), 1);
    run(
        &mut state,
        context,
        "execute store result score count oracle if entity @e[type=minecraft:villager]",
    );
    assert_eq!(score(&state, "count"), 0);
}
#[test]
fn execute_store_forks_bind_holders_at_modifier_and_preserve_cross_world_context() {
    let (mut state, _, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    run(
        &mut state,
        context,
        "execute as @a store result score @s oracle run data get entity @s Pos[0] 100",
    );
    assert_eq!(
        state
            .scoreboard
            .scores
            .values()
            .filter_map(|v| v.get("oracle"))
            .next()
            .unwrap()
            .value,
        175
    );
    let (mut nether, _, _) = fixture();
    nether.dimension = crate::dimensions::Dimension::Nether;
    nether.day_time = 678;
    state
        .other_dimensions
        .insert(nether.dimension, Box::new(nether));
    // Cross-world native dispatch runs at the shared tick boundary.
    state.command_context.shared = true;
    run(
        &mut state,
        context,
        "execute store result score nether oracle in minecraft:the_nether run time query time",
    );
    assert_eq!(score(&state, "nether"), 678);
    run(
        &mut state,
        context,
        "execute store success score missing oracle as @e[type=minecraft:villager] run data get entity @s Health",
    );
    assert_eq!(score(&state, "missing"), 0);
}

fn stored(state: &mut TickState, context: Context, command: &str, expected: Option<i32>) {
    run(
        state,
        context,
        &format!(
            "execute store result score result oracle store success score success oracle run {command}"
        ),
    );
    assert_eq!(score(state, "result"), expected.unwrap_or(0), "{command}");
    assert_eq!(
        score(state, "success"),
        i32::from(expected.is_some()),
        "{command}"
    );
}

#[test]
fn execute_store_give_and_clear_execute_once_and_return_real_counts() {
    let (mut state, id, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    stored(&mut state, context, "give @s minecraft:stone 7", Some(1));
    let stone = item(&state, "minecraft:stone", 1).item_id;
    let count = |state: &TickState| {
        state.players[&id]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == stone)
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    assert_eq!(count(&state), 7);
    stored(&mut state, context, "clear @s minecraft:stone 0", Some(7));
    assert_eq!(count(&state), 7);
    stored(&mut state, context, "clear @s minecraft:stone 3", Some(3));
    assert_eq!(count(&state), 4);
    stored(&mut state, context, "clear @s minecraft:stone", Some(4));
    assert_eq!(count(&state), 0);
    stored(&mut state, context, "clear @s minecraft:stone", None);
    stored(&mut state, context, "give @s minecraft:stone 6401", None);
    assert_eq!(count(&state), 0);
}

#[test]
fn execute_store_score_mutations_preserve_signed_zero_and_failed_outcomes() {
    let (mut state, _, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    for (command, expected) in [
        ("scoreboard players set value oracle -7", Some(-7)),
        ("scoreboard players add value oracle 7", Some(0)),
        ("scoreboard players remove value oracle 3", Some(-3)),
        ("scoreboard players set source oracle 4", Some(4)),
        (
            "scoreboard players operation value oracle *= source oracle",
            Some(-12),
        ),
        ("scoreboard players set source oracle 0", Some(0)),
        (
            "scoreboard players operation value oracle /= source oracle",
            None,
        ),
    ] {
        stored(&mut state, context, command, expected);
    }
    assert_eq!(
        score(&state, "value"),
        -12,
        "failed arithmetic must not mutate scores"
    );
    stored(
        &mut state,
        context,
        "scoreboard players reset value oracle",
        Some(1),
    );
    assert!(!state.scoreboard.scores.contains_key("value"));
}

#[test]
fn execute_store_entity_mutations_report_affected_entities_and_failures() {
    let (mut state, _, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    stored(&mut state, context, "summon minecraft:pig 4 60 4", Some(1));
    stored(&mut state, context, "summon minecraft:pig 5 60 4", Some(1));
    stored(
        &mut state,
        context,
        "effect give @e[type=minecraft:pig] minecraft:speed 30",
        Some(2),
    );
    stored(
        &mut state,
        context,
        "effect clear @e[type=minecraft:pig]",
        Some(2),
    );
    stored(
        &mut state,
        context,
        "effect clear @e[type=minecraft:pig]",
        None,
    );
    stored(
        &mut state,
        context,
        "tp @e[type=minecraft:pig] 9 61 9",
        Some(2),
    );
    assert_eq!(
        query(
            &mut state,
            context,
            "data get entity @e[type=minecraft:pig,limit=1] Pos[0]"
        )
        .unwrap(),
        9,
    );
    let queued = state.merge_damage.len();
    stored(&mut state, context, "kill @e[type=minecraft:pig]", Some(2));
    assert_eq!(state.merge_damage.len(), queued + 2);
    stored(
        &mut state,
        context,
        "kill @e[type=minecraft:villager]",
        None,
    );
    assert_eq!(state.merge_damage.len(), queued + 2);
    stored(
        &mut state,
        context,
        "summon minecraft:pig 30000000 60 4",
        None,
    );
}

#[test]
fn execute_store_experience_uses_target_counts_not_experience_values() {
    let (mut state, id, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    stored(&mut state, context, "experience add @s 7 levels", Some(1));
    assert_eq!(state.players[&id].vitals.experience.level, 7);
    stored(&mut state, context, "experience set @s 0 levels", Some(1));
    assert_eq!(state.players[&id].vitals.experience.level, 0);
    stored(&mut state, context, "experience set @s 999 points", None);
    assert_eq!(state.players[&id].vitals.experience.level, 0);
}

#[test]
fn execute_store_block_mutations_report_counts_and_unchanged_data_failure() {
    let (mut state, _, context) = fixture();
    run(
        &mut state,
        context,
        "scoreboard objectives add oracle dummy",
    );
    stored(
        &mut state,
        context,
        "setblock 4 60 4 minecraft:stone",
        Some(1),
    );
    stored(&mut state, context, "setblock 4 60 4 minecraft:stone", None);
    stored(
        &mut state,
        context,
        "fill 4 60 4 6 60 4 minecraft:dirt",
        Some(3),
    );
    stored(
        &mut state,
        context,
        "setblock 4 60 4 minecraft:command_block",
        Some(1),
    );
    stored(
        &mut state,
        context,
        "data merge block 4 60 4 {Command:'say stored'}",
        Some(1),
    );
    stored(
        &mut state,
        context,
        "data merge block 4 60 4 {Command:'say stored'}",
        None,
    );
    assert_eq!(
        query(&mut state, context, "data get block 4 60 4 Command").unwrap(),
        10,
    );
}

#[test]
fn tag_commands_update_selector_data_and_report_counts() {
    use crate::command_execution::CommandResult::{Failed, Success};
    let (mut state, _, context) = fixture();
    let mut changes = ChangeSet::default();
    let mut command = |state: &mut TickState, text| {
        crate::command_context::native(state, text, context, &mut changes, true).unwrap()
    };
    assert_eq!(command(&mut state, "tag @s add map_test"), Success(1));
    assert_eq!(command(&mut state, "tag @s add map_test"), Failed);
    assert_eq!(command(&mut state, "tag @s list"), Success(1));
    assert_eq!(
        command(&mut state, "tag @s[tag=map_test] remove map_test"),
        Success(1)
    );
    assert_eq!(command(&mut state, "tag @s remove map_test"), Failed);
    assert_eq!(command(&mut state, "tag @s list"), Success(0));
    state.spawn_mob(state.registry.cow_type_id, [2.0, 60.0, 2.0], 10.0);
    assert_eq!(
        command(&mut state, "tag @e[type=minecraft:cow] add herd"),
        Success(1)
    );
    assert_eq!(
        command(&mut state, "tag @e[type=minecraft:cow,tag=herd] list"),
        Success(1)
    );
    assert_eq!(
        command(
            &mut state,
            "tag @e[type=minecraft:cow,tag=herd] remove herd"
        ),
        Success(1)
    );
}

#[test]
fn map_sound_and_particle_commands_report_delivery() {
    use crate::command_execution::CommandResult::{Failed, Success};
    let (mut state, _, context) = fixture();
    let mut changes = ChangeSet::default();
    for command in [
        "playsound minecraft:block.bell.use master @s ~ ~ ~ 1 1 0",
        "particle minecraft:sweep_attack ~ ~ ~ 0 0 0 0 1 normal @s",
    ] {
        assert_eq!(
            crate::command_context::native(&mut state, command, context, &mut changes, true)
                .unwrap(),
            Success(1),
            "{command}"
        );
    }
    assert!(state.outbox[&crate::boundary::ConnectionId(1)].iter().any(|change| matches!(change, crate::boundary::PlayerChange::CommandSound { sound, .. } if sound.as_str() == "minecraft:block.bell.use")));
    assert!(
        state.outbox[&crate::boundary::ConnectionId(1)]
            .iter()
            .any(|change| matches!(
                change,
                crate::boundary::PlayerChange::CommandParticle {
                    particle_id: 77,
                    count: 1,
                    ..
                }
            ))
    );
    assert_eq!(
        crate::command_context::native(
            &mut state,
            "particle minecraft:unsupported ~ ~ ~",
            context,
            &mut changes,
            true
        )
        .unwrap(),
        Failed
    );
}

#[test]
fn data_storage_modify_reports_results_and_survives_reads() {
    use crate::command_execution::CommandResult::{Failed, Success};
    let (mut state, _, context) = fixture();
    let mut changes = ChangeSet::default();
    let mut command = |state: &mut TickState, text| {
        crate::command_context::native(state, text, context, &mut changes, true).unwrap()
    };
    assert_eq!(
        command(
            &mut state,
            "data merge storage test:map {list:[1,2],value:3}"
        ),
        Success(1)
    );
    assert_eq!(
        command(&mut state, "data modify storage test:map value set value 7"),
        Success(1)
    );
    assert_eq!(
        command(&mut state, "data get storage test:map value"),
        Success(7)
    );
    assert_eq!(
        command(
            &mut state,
            "data modify storage test:map list append value 4"
        ),
        Success(1)
    );
    assert_eq!(
        command(&mut state, "data get storage test:map list"),
        Success(3)
    );
    assert_eq!(
        command(&mut state, "data modify storage test:map value set value 7"),
        Failed
    );
    assert_eq!(
        command(
            &mut state,
            "data modify storage test:map list prepend value 0"
        ),
        Success(1)
    );
    assert_eq!(
        command(
            &mut state,
            "data modify storage test:map list insert 1 value 9"
        ),
        Success(1)
    );
    assert_eq!(
        command(
            &mut state,
            "data modify storage test:map copied set from storage test:map value"
        ),
        Success(1)
    );
    assert_eq!(
        command(&mut state, "data get storage test:map copied"),
        Success(7)
    );
    assert_eq!(
        command(
            &mut state,
            "data modify storage test:map compound merge value {key:1}"
        ),
        Success(1)
    );
    assert_eq!(
        command(&mut state, "data remove storage test:map copied"),
        Success(1)
    );
    assert_eq!(
        command(&mut state, "data get storage test:map copied"),
        Failed
    );
    let image = crate::command_storage::encode(&state.command_storage);
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(
            1,
            1,
            vec![crate::journal::JournalEntry::CommandStorage { value: image }],
        )],
    );
    assert_eq!(
        crate::command_storage::decode(replay.command_storage.as_ref().unwrap()).unwrap(),
        state.command_storage
    );
}
