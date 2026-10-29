use super::*;

fn fixture() -> TickState {
    let (mut state, _, _) = crate::hostile_tests::setup(8.0);
    state.enable_command_block = true;
    state.day_time = 0;
    state
}
fn place(state: &mut TickState, x: i32, name: &str, command: &str, auto: bool) -> BlockPos {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
    let block = state.registry.blocks.block(&id).unwrap();
    let mut properties = state
        .registry
        .blocks
        .by_id(block.default)
        .unwrap()
        .properties
        .clone();
    properties
        .iter_mut()
        .find(|(key, _)| key == "facing")
        .unwrap()
        .1 = "east".into();
    let id = state
        .registry
        .blocks
        .by_name_and_props(&id, &properties)
        .unwrap();
    let pos = BlockPos { x, y: 60, z: 0 };
    state.world.set_block(pos.x, pos.y, pos.z, id.0);
    state.world.fold_overlay();
    save(
        state,
        pos,
        vec![
            ("Command".into(), Tag::String(command.into())),
            ("auto".into(), Tag::Byte(i8::from(auto))),
        ],
    );
    pos
}
fn advance(state: &mut TickState) {
    state.tick += 1;
    crate::redstone::run(state, &mut Vec::new(), &mut Vec::new());
    run(state, &mut ChangeSet::default());
}
#[test]
fn conditional_chain_tracks_predecessor_success() {
    let mut state = fixture();
    let first = place(&mut state, 0, "command_block", "not_a_command", true);
    let second = place(&mut state, 1, "chain_command_block", "time add 3", true);
    let block_id = state.world.block_at(second.x, second.y, second.z).unwrap();
    let block = state.registry.blocks.by_id(BlockStateId(block_id)).unwrap();
    let name = block.block.id.clone();
    let mut properties = block.properties.clone();
    properties
        .iter_mut()
        .find(|(key, _)| key == "conditional")
        .unwrap()
        .1 = "true".into();
    let conditional_id = state
        .registry
        .blocks
        .by_name_and_props(&name, &properties)
        .unwrap();
    state
        .world
        .set_block(second.x, second.y, second.z, conditional_id.0);
    schedule(&mut state, first);
    advance(&mut state);
    assert_eq!(state.day_time, 0);
    let mut fields = read(&state, first).unwrap();
    put(
        &mut fields,
        "Command",
        Tag::String("forceload query 0 0".into()),
    );
    save(&mut state, first, fields);
    schedule(&mut state, first);
    advance(&mut state);
    assert_eq!(state.day_time, 0);
    let mut fields = read(&state, first).unwrap();
    put(
        &mut fields,
        "Command",
        Tag::String("scoreboard objectives add test dummy".into()),
    );
    save(&mut state, first, fields);
    schedule(&mut state, first);
    advance(&mut state);
    assert_eq!(state.day_time, 3);
    let mut fields = read(&state, first).unwrap();
    put(&mut fields, "Command", Tag::String("time add 2".into()));
    save(&mut state, first, fields);
    schedule(&mut state, first);
    advance(&mut state);
    assert_eq!(state.day_time, 8);
}

#[test]
fn conditional_chain_executes_at_shared_command_boundary() {
    let mut state = fixture();
    state.dimension_globals.active = true;
    let first = place(&mut state, 0, "command_block", "time add 2", true);
    let second = place(&mut state, 1, "chain_command_block", "time add 3", true);
    let block_id = state.world.block_at(second.x, second.y, second.z).unwrap();
    let block = state.registry.blocks.by_id(BlockStateId(block_id)).unwrap();
    let name = block.block.id.clone();
    let mut properties = block.properties.clone();
    properties
        .iter_mut()
        .find(|(key, _)| key == "conditional")
        .unwrap()
        .1 = "true".into();
    let conditional_id = state
        .registry
        .blocks
        .by_name_and_props(&name, &properties)
        .unwrap();
    state
        .world
        .set_block(second.x, second.y, second.z, conditional_id.0);
    schedule(&mut state, first);
    advance(&mut state);
    let (mut output, mut drains, _) = crate::tick::run_dimension_tick(&mut state, &[]);
    assert_eq!(state.command_context.jobs.len(), 1);
    let before = state.day_time;
    crate::dimensions::travel::finish(&mut state, &mut output, &mut drains);
    assert_eq!(state.day_time, before + 5);
}

#[test]
fn impulse_rising_edge_drives_unconditional_chain_once() {
    let mut state = fixture();
    let impulse = place(&mut state, 0, "command_block", "time add 2", false);
    place(&mut state, 1, "chain_command_block", "time add 3", true);
    let power = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:redstone_block").unwrap())
        .unwrap()
        .default;
    state.world.set_block(0, 60, -1, power.0);
    changed(&mut state, impulse);
    advance(&mut state);
    assert_eq!(state.day_time, 5);
    changed(&mut state, impulse);
    advance(&mut state);
    assert_eq!(state.day_time, 5);
    state.world.set_block(0, 60, -1, state.registry.states.air);
    changed(&mut state, impulse);
    state.world.set_block(0, 60, -1, power.0);
    changed(&mut state, impulse);
    advance(&mut state);
    assert_eq!(state.day_time, 10);
}
#[test]
fn repeating_auto_uses_durable_ticks_and_disable_gate() {
    let mut state = fixture();
    let pos = place(&mut state, 0, "repeating_command_block", "time add 1", true);
    changed(&mut state, pos);
    assert!(!state.block_ticks.image().chunks.is_empty());
    advance(&mut state);
    advance(&mut state);
    assert_eq!(state.day_time, 2);
    state.enable_command_block = false;
    advance(&mut state);
    assert_eq!(state.day_time, 2);
}
#[test]
fn data_merge_admits_only_supported_fields_and_requires_operator() {
    let mut state = fixture();
    let pos = place(
        &mut state,
        0,
        "repeating_command_block",
        "time add 1",
        false,
    );
    let connection = crate::command_console::CONNECTION;
    let command = "data merge block 0 60 0 {Command:\"time add 7\",auto:1b}";
    crate::command_console::execute(
        &mut state,
        connection,
        command,
        0,
        &mut ChangeSet::default(),
    );
    assert!(!flag(&read(&state, pos).unwrap(), "auto"));
    crate::command_console::execute(
        &mut state,
        connection,
        command,
        2,
        &mut ChangeSet::default(),
    );
    advance(&mut state);
    assert_eq!(state.day_time, 7);
    let before = read(&state, pos).unwrap();
    crate::command_console::execute(
        &mut state,
        connection,
        "data merge block 0 60 0 {Command:\"time set 0\",SuccessCount:12}",
        2,
        &mut ChangeSet::default(),
    );
    assert_eq!(read(&state, pos).unwrap(), before);
}
#[test]
fn commands_use_block_origin_and_opaque_wal() {
    let mut state = fixture();
    let pos = place(
        &mut state,
        0,
        "command_block",
        "setblock ~ ~1 ~ minecraft:diamond_block",
        true,
    );
    schedule(&mut state, pos);
    advance(&mut state);
    let id = state.world.block_at(0, 61, 0).unwrap();
    assert_eq!(
        state
            .registry
            .blocks
            .by_id(BlockStateId(id))
            .unwrap()
            .block
            .id
            .path(),
        "diamond_block"
    );
    assert!(
        state
            .opaque_changes
            .iter()
            .any(|(at, nbt)| *at == pos && nbt.is_some())
    );
    assert!(state.datapacks.origin.is_none());
}

#[test]
fn placement_requires_creative_and_operator_permission() {
    let (mut state, player, _) = crate::hostile_tests::setup(8.0);
    let connection = state.players[&player].connection;
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(!can_place(&state, player));
    state.plugin_players.observe_permission(connection, 2);
    assert!(can_place(&state, player));
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Survival;
    assert!(!can_place(&state, player));
}
