use super::*;
use crate::boundary::BlockPos;

fn fixture() -> (TickState, EntityId) {
    let (mut state, entity, _) = crate::hostile_tests::setup(8.0);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [1.75, 60.0, 2.25]);
    state.entities.set_rot(slot, 0.0, 0.0);
    state.day_time = 0;
    (state, entity)
}
fn command(state: &mut TickState, entity: EntityId, text: &str) {
    let context = Context::source(state, crate::command_console::CONNECTION, entity, 2);
    run(state, text, context, &mut ChangeSet::default());
}
fn diamond(state: &TickState, position: [i32; 3]) -> bool {
    let [x, y, z] = position;
    state
        .world
        .block_at(x, y, z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path() == "diamond_block")
}

#[test]
fn selector_self_type_filters_command_path() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute if entity @s[type=minecraft:player] run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute if entity @s[type=!minecraft:player] run time add 10",
    );
    assert_eq!(state.day_time, 1);
}

#[test]
fn selector_nested_scores_and_repeated_nbt_command_path() {
    let (mut state, entity) = fixture();
    command(&mut state, entity, "scoreboard objectives add a dummy");
    command(&mut state, entity, "scoreboard objectives add b dummy");
    let name = state.connections.name_of(entity).unwrap().to_owned();
    command(
        &mut state,
        entity,
        &format!("scoreboard players set {name} a 1"),
    );
    command(
        &mut state,
        entity,
        &format!("scoreboard players set {name} b 2"),
    );
    command(
        &mut state,
        entity,
        "execute if entity @s[scores={a=1,b=..2},nbt={},nbt=!{Missing:1b}] run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute if entity @s[nbt={Missing:1b}] run time add 10",
    );
    assert_eq!(state.day_time, 1);
}

#[test]
fn selector_empty_tag_command_path() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute if entity @s[tag=] run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute if entity @s[tag=!] run time add 10",
    );
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 60.0, 2.0], 10.0);
    let slot = state.entities.slot_of(cow).unwrap();
    mc_nbt::write_named(
        &mut state.entities.row_mut(slot).opaque_entity_fields,
        "",
        &mc_nbt::Tag::Compound(vec![
            (
                "Tags".into(),
                mc_nbt::Tag::List(mc_nbt::ListTag {
                    element_type: 8,
                    elements: vec![mc_nbt::Tag::String("probe".into())],
                }),
            ),
            ("Silent".into(), mc_nbt::Tag::Byte(1)),
            ("NoGravity".into(), mc_nbt::Tag::Byte(1)),
        ]),
    )
    .unwrap();
    command(
        &mut state,
        entity,
        "execute if entity @e[type=minecraft:cow,tag=!,tag=probe,nbt={Silent:1b,NoGravity:1b},nbt=!{Missing:1b}] run time add 2",
    );
    command(
        &mut state,
        entity,
        "execute if entity @e[type=minecraft:cow,tag=] run time add 10",
    );
    assert_eq!(state.day_time, 3);
}

#[test]
fn execute_blocks_matches_and_rejects_regions() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "setblock 4 60 4 minecraft:diamond_block",
    );
    command(
        &mut state,
        entity,
        "setblock 5 60 4 minecraft:diamond_block",
    );
    command(&mut state, entity, "setblock 6 60 4 minecraft:stone");
    command(
        &mut state,
        entity,
        "execute if blocks 4 60 4 4 60 4 5 60 4 all run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute unless blocks 4 60 4 4 60 4 6 60 4 all run time add 1",
    );
    assert_eq!(state.day_time, 2);
}

#[test]
fn clone_is_operator_only() {
    assert!(crate::commands::parse("clone 4 60 4 4 60 4 5 60 4", false).is_err());
    assert!(crate::commands::parse("clone 4 60 4 4 60 4 5 60 4", true).is_ok());
}

#[test]
fn clone_filtered_accepts_block_tags() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "setblock 4 60 4 minecraft:diamond_block",
    );
    command(
        &mut state,
        entity,
        "clone 4 60 4 4 60 4 5 60 4 filtered #minecraft:mineable/pickaxe",
    );
    assert!(diamond(&state, [5, 60, 4]));
}

#[test]
fn region_compare_and_clone_follow_command_path() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "setblock 4 60 4 minecraft:diamond_block",
    );
    command(&mut state, entity, "setblock 6 60 4 minecraft:stone");
    command(&mut state, entity, "clone 4 60 4 4 60 4 5 60 4");
    assert!(diamond(&state, [5, 60, 4]));
    command(
        &mut state,
        entity,
        "execute if blocks 4 60 4 4 60 4 5 60 4 all run time add 1",
    );
    assert_eq!(state.day_time, 1);
    command(
        &mut state,
        entity,
        "execute unless blocks 4 60 4 4 60 4 6 60 4 all run time add 1",
    );
    assert_eq!(state.day_time, 2);
}

#[test]
fn clone_modes_and_masked_region_comparison() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "setblock 4 60 4 minecraft:diamond_block",
    );
    command(&mut state, entity, "setblock 5 60 4 minecraft:air");
    command(&mut state, entity, "setblock 7 60 4 minecraft:stone");
    command(&mut state, entity, "clone 4 60 4 5 60 4 6 60 4 masked");
    assert!(diamond(&state, [6, 60, 4]));
    command(
        &mut state,
        entity,
        "execute if blocks 4 60 4 5 60 4 6 60 4 masked run time add 1",
    );
    assert_eq!(state.day_time, 1);
    command(
        &mut state,
        entity,
        "execute unless blocks 4 60 4 5 60 4 6 60 4 all run time add 1",
    );
    assert_eq!(state.day_time, 2);
    command(&mut state, entity, "clone 4 60 4 5 60 4 5 60 4 replace");
    assert!(!diamond(&state, [5, 60, 4]), "normal overlap must fail");
    command(
        &mut state,
        entity,
        "clone 4 60 4 5 60 4 5 60 4 filtered minecraft:diamond_block move",
    );
    assert!(!diamond(&state, [4, 60, 4]));
    assert!(diamond(&state, [5, 60, 4]));
}

#[test]
fn clone_copies_live_container_contents() {
    let (mut state, entity) = fixture();
    command(&mut state, entity, "setblock 4 60 4 minecraft:chest");
    crate::tick::run_tick(&mut state, &[]);
    let item = crate::crafter::tests::stack(&state, "diamond", 13);
    state.block_entities.get_mut(&(4, 60, 4)).unwrap().slots[0] = item.clone();
    command(&mut state, entity, "clone 4 60 4 4 60 4 5 60 4");
    crate::tick::run_tick(&mut state, &[]);
    assert_eq!(state.block_entities[&(5, 60, 4)].slots[0], item);
    let before = state.day_time;
    command(
        &mut state,
        entity,
        "execute if blocks 4 60 4 4 60 4 5 60 4 all run time add 1",
    );
    assert_eq!(state.day_time, before + 1);
    state.block_entities.get_mut(&(5, 60, 4)).unwrap().slots[0] = mc_data::ItemStack::EMPTY;
    command(
        &mut state,
        entity,
        "execute unless blocks 4 60 4 4 60 4 5 60 4 all run time add 1",
    );
    assert_eq!(state.day_time, before + 2);
}

#[test]
fn command_parity_configured_fork_budget_rejects_before_execution() {
    let (mut state, entity) = fixture();
    command(&mut state, entity, "gamerule minecraft:max_command_forks 0");
    command(&mut state, entity, "execute as @s run time add 1");
    assert_eq!(state.day_time, 0);
}

#[test]
fn command_parity_fork_limit_is_strict_and_sequence_costs_modifier_stages() {
    let (mut state, entity) = fixture();
    command(&mut state, entity, "gamerule minecraft:max_command_forks 1");
    command(&mut state, entity, "execute as @s run time add 1");
    assert_eq!(state.day_time, 0);
    command(&mut state, entity, "gamerule minecraft:max_command_forks 2");
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 2",
    );
    command(&mut state, entity, "execute as @s run time add 1");
    assert_eq!(state.day_time, 1);
    command(&mut state, entity, "execute as @s at @s run time add 1");
    assert_eq!(state.day_time, 1);
    command(&mut state, entity, "time add 1");
    assert_eq!(state.day_time, 2);
}

#[test]
fn command_parity_zero_sequence_budget_still_admits_one_root_command() {
    // COMMAND-ZERO-BUDGET-LOCKOUT: a zero gamerule clamps the root quota to
    // one so ordinary and console administration can restore a positive
    // limit instead of rejecting every native root command.
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 0",
    );
    command(&mut state, entity, "time add 5");
    assert_eq!(state.day_time, 5);
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 2",
    );
    command(&mut state, entity, "time add 1");
    assert_eq!(state.day_time, 6);
}

#[test]
fn positioned_align_rotated_and_anchor_affect_native_local_coordinates() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 4.8 60 4.9 align xz rotated 0 0 run setblock ^ ^ ^1 minecraft:diamond_block",
    );
    assert!(diamond(&state, [4, 60, 5]));
    command(
        &mut state,
        entity,
        "execute positioned 6 60 6 anchored eyes run setblock ^ ^ ^ minecraft:diamond_block",
    );
    assert!(diamond(&state, [6, 61, 6]));
    command(
        &mut state,
        entity,
        "execute positioned 7 60 7 anchored eyes run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(
        diamond(&state, [7, 60, 7]),
        "anchor applies to local, not relative coordinates"
    );
    command(
        &mut state,
        entity,
        "execute positioned 8 60 8 anchored eyes anchored feet run setblock ^ ^ ^ minecraft:diamond_block",
    );
    assert!(diamond(&state, [8, 60, 8]));
    assert!(state.command_context.current.is_none());
}
#[test]
fn as_changes_executor_at_changes_pose_and_positioned_as_keeps_rotation() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 10 60 10 as @s run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(diamond(&state, [10, 60, 10]), "as must not move the source");
    command(
        &mut state,
        entity,
        "execute positioned 10 60 10 at @s run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(diamond(&state, [1, 60, 2]));
    command(
        &mut state,
        entity,
        "execute rotated 90 0 positioned as @s run setblock ^ ^ ^2 minecraft:diamond_block",
    );
    assert!(diamond(&state, [-1, 60, 2]));
    command(
        &mut state,
        entity,
        "execute positioned 10 60 10 rotated as @s run setblock ^ ^ ^1 minecraft:diamond_block",
    );
    assert!(diamond(&state, [10, 60, 11]));
}
#[test]
fn facing_coordinates_and_entity_anchor_rotate_native_source() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 4 60 4 facing 4 60 8 run setblock ^ ^ ^2 minecraft:diamond_block",
    );
    assert!(diamond(&state, [4, 60, 6]));
    command(
        &mut state,
        entity,
        "execute positioned 1.75 60 0.25 facing entity @s feet run setblock ^ ^ ^1 minecraft:diamond_block",
    );
    assert!(diamond(&state, [1, 60, 1]));
}
#[test]
fn if_unless_entity_block_and_score_filter_without_mutating_source() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "setblock 4 60 4 minecraft:diamond_block",
    );
    command(
        &mut state,
        entity,
        "execute if entity @s if block 4 60 4 minecraft:diamond_block run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute unless entity @e[type=minecraft:zombie] unless block 4 60 4 minecraft:stone run time add 2",
    );
    command(&mut state, entity, "scoreboard objectives add test dummy");
    command(&mut state, entity, "scoreboard players set left test 7");
    command(&mut state, entity, "scoreboard players set right test 9");
    command(
        &mut state,
        entity,
        "execute if score left test matches 5..8 if score left test < right test run time add 4",
    );
    command(
        &mut state,
        entity,
        "execute unless score left test >= right test run time add 8",
    );
    command(
        &mut state,
        entity,
        "execute if score absent test matches 0 run time add 100",
    );
    command(
        &mut state,
        entity,
        "execute if block 4 60 4 minecraft:stone run time add 100",
    );
    assert_eq!(state.day_time, 15);
}
#[test]
fn selector_origin_uses_positioned_context() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 1.75 60 2.25 if entity @e[distance=..0.1] run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute positioned 12 60 12 if entity @e[distance=..0.1] run time add 100",
    );
    assert_eq!(state.day_time, 1);
}
#[test]
fn in_uses_shared_world_authority_without_moving_executor_and_commits_blocks() {
    let (mut state, entity) = fixture();
    let (mut nether, _) = fixture();
    nether.dimension = Dimension::Nether;
    state
        .other_dimensions
        .insert(Dimension::Nether, Box::new(nether));
    state.command_context.shared = true;
    command(
        &mut state,
        entity,
        "execute positioned 8 60 8 in minecraft:the_nether run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(diamond(
        &state.other_dimensions[&Dimension::Nether],
        [1, 60, 1]
    ));
    assert_eq!(state.dimension, Dimension::Overworld);
    assert!(state.entities.slot_of(entity).is_some());
    let changes = &state.command_context.changes[&Dimension::Nether];
    assert!(
        changes
            .blocks
            .iter()
            .any(|change| change.position == BlockPos { x: 1, y: 60, z: 1 })
    );
    assert!(
        state.other_dimensions[&Dimension::Nether]
            .merge_block_writes
            .is_empty()
    );
    state.command_context.shared = false;
    command(
        &mut state,
        entity,
        "execute positioned 16 60 16 in minecraft:the_nether run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(
        !diamond(&state.other_dimensions[&Dimension::Nether], [2, 60, 2]),
        "no bypass outside shared boundary"
    );
}
#[test]
fn nested_execute_budget_and_permissions_fail_closed() {
    let (mut state, entity) = fixture();
    let mut context = Context::source(&state, crate::command_console::CONNECTION, entity, 0);
    run(
        &mut state,
        "execute run time add 100",
        context,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.day_time, 0);
    context.permission = 2;
    // Zero clamps the root quota to one command: a single chain leaf still
    // executes (no lockout), while a modifier stage plus two leaves exceeds
    // the single quota and fails closed (see the zero-budget test above).
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 0",
    );
    run(
        &mut state,
        "execute as @s run execute as @s run time add 100",
        context,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.day_time, 0);
    assert!(state.command_context.current.is_none());
    assert_eq!(state.command_context.depth, 0);
}
