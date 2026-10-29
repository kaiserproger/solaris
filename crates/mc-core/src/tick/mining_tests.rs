use super::*;
use crate::crafter::tests::{block, fixture};

const CONNECTION: ConnectionId = ConnectionId(1);
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };

fn setup(name: &str) -> (TickState, EntityId, u32) {
    let (mut state, actor) = fixture(None);
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Survival;
    state.players.get_mut(&actor).unwrap().awaiting_teleport = None;
    let current = block(&state, name, &[]);
    // Ice support uses vanilla's block tag; the bootstrap fixture intentionally
    // contains only a small client-required subset of tags.
    let mut registry = if name == "ice" {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        crate::registry::CoreRegistry::load(Some(&root)).unwrap()
    } else {
        crate::registry::CoreRegistry::load(None).unwrap()
    };
    let mut hardness = Vec::new();
    // Real vanilla hardness, independently supplied rather than bootstrap defaults.
    for (name, speed) in [
        ("stone", 1.5),
        ("grass_block", 0.6),
        ("obsidian", 50.0),
        ("bedrock", -1.0),
        ("torch", 0.0),
        ("ice", 0.5),
    ] {
        let id = block(&state, name, &[]);
        if hardness.len() <= id as usize {
            hardness.resize(id as usize + 1, -1.0);
        }
        hardness[id as usize] = speed;
    }
    let correct_tool = vec![false; hardness.len()];
    registry.mining =
        mc_data::block_mining::BlockMiningTable::from_arrays("fixture", hardness, correct_tool);
    state.registry = std::sync::Arc::new(registry);
    state.world.set_block(POS.x, POS.y, POS.z, current);
    state.world.fold_overlay();
    (state, actor, current)
}

fn input(action: DigAction) -> PlayerInput {
    PlayerInput::Dig {
        connection: CONNECTION,
        position: POS,
        action,
        sequence: 1,
    }
}

fn direct(state: &mut TickState, action: DigAction) -> (Vec<Intent>, ChangeSet) {
    let mut intents = Vec::new();
    let mut changes = ChangeSet::default();
    input::interact::dig(
        state,
        (&CONNECTION, &POS, &action, &1),
        &mut intents,
        &mut changes,
    );
    (intents, changes)
}

#[test]
fn survival_finish_without_start_never_plans_a_break() {
    for name in ["stone", "obsidian", "bedrock"] {
        let (mut state, actor, _) = setup(name);
        assert!(
            plan_client_build(&state, actor, &input(DigAction::Finish)).is_none(),
            "hook planner accepted {name}"
        );
        assert!(
            direct(&mut state, DigAction::Finish).0.is_empty(),
            "direct accepted {name}"
        );
    }
}

#[test]
fn survival_start_then_immediate_finish_never_plans_a_break() {
    for name in ["stone", "obsidian", "bedrock"] {
        let (mut state, actor, _) = setup(name);
        assert!(direct(&mut state, DigAction::Start).0.is_empty());
        assert!(
            plan_client_build(&state, actor, &input(DigAction::Finish)).is_none(),
            "hook planner accepted early {name}"
        );
        assert!(
            direct(&mut state, DigAction::Finish).0.is_empty(),
            "direct accepted early {name}"
        );
    }
}

/// Other domain tests exercise the finish transaction, not its elapsed input
/// ticks. Prime it through the real mining owner, without running unrelated AI.
pub(crate) fn ready_to_finish(state: &mut TickState, connection: ConnectionId, position: BlockPos) {
    let actor = state.connections.entity_of(connection).unwrap();
    if state.players[&actor].creative() {
        return;
    }
    let current = state
        .world
        .block_at(position.x, position.y, position.z)
        .unwrap();
    if state.registry.mining.destroy_speed(current).is_none() {
        std::sync::Arc::make_mut(&mut state.registry).mining = mc_data::block_mining::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../data/vanilla/reports/block_mining.json"),
        )
        .unwrap();
    }
    assert!(
        crate::effects::mining_progress(state, actor, current) > 0.0,
        "fixture must be mineable"
    );
    mining::start(state, actor, position, current);
    for _ in 0..100_000 {
        if mining::allows(state, actor, position, current, DigAction::Finish) {
            return;
        }
        state.tick += 1;
        mining::advance(state);
    }
    panic!("fixture mining did not finish");
}

#[test]
fn mining_server_ticks_admit_finish_and_commit_consumes_progress() {
    let (mut state, actor, stone) = setup("stone");
    run_tick(&mut state, &[input(DigAction::Start)]);
    assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(stone));
    for _ in 0..45 {
        run_tick(&mut state, &[]);
    }
    assert!(plan_client_build(&state, actor, &input(DigAction::Finish)).is_some());
    run_tick(&mut state, &[input(DigAction::Finish)]);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert!(state.players[&actor].dig_progress.is_none());
}

#[test]
fn mining_bedrock_and_missing_hardness_stay_unbreakable_after_server_ticks() {
    for name in ["bedrock", "chest"] {
        let (mut state, actor, current) = setup(name);
        if name == "chest" {
            std::sync::Arc::make_mut(&mut state.registry).mining = Default::default();
        }
        direct(&mut state, DigAction::Start);
        for _ in 0..200 {
            state.tick += 1;
            mining::advance(&mut state);
        }
        assert!(plan_client_build(&state, actor, &input(DigAction::Finish)).is_none());
        let (intents, correction) = direct(&mut state, DigAction::Finish);
        assert!(intents.is_empty());
        assert_eq!(
            correction.blocks,
            vec![crate::boundary::BlockChange {
                position: POS,
                state: current
            }]
        );
    }
}

#[test]
fn mining_cancel_replacement_reach_teleport_and_packet_spam_invalidate_credit() {
    for invalidation in ["cancel", "replacement", "reach", "teleport", "spam"] {
        let (mut state, actor, _) = setup("stone");
        direct(&mut state, DigAction::Start);
        for _ in 0..50 {
            if invalidation == "spam" {
                direct(&mut state, DigAction::Start);
            } else {
                state.tick += 1;
                mining::advance(&mut state);
            }
        }
        match invalidation {
            "cancel" => {
                direct(&mut state, DigAction::Cancel);
            }
            "replacement" => {
                let obsidian = block(&state, "obsidian", &[]);
                state.world.set_block(POS.x, POS.y, POS.z, obsidian);
            }
            "reach" => {
                let slot = state.entities.slot_of(actor).unwrap();
                state.entities.set_pos(slot, [100.0, 201.0, 100.0]);
                mining::advance(&mut state);
                state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
            }
            "teleport" => {
                let player = state.players.get_mut(&actor).unwrap();
                player.begin_teleport();
                player.awaiting_teleport = None;
            }
            "spam" => {}
            _ => unreachable!(),
        }
        if invalidation != "spam" {
            assert!(
                client_build_refused(&state, actor, &input(DigAction::Finish)),
                "{invalidation}"
            );
        }
        assert!(!mining::allows(
            &state,
            actor,
            POS,
            state.world.block_at(POS.x, POS.y, POS.z).unwrap(),
            DigAction::Finish
        ));
        assert!(
            direct(&mut state, DigAction::Finish).0.is_empty(),
            "{invalidation}"
        );
    }
}

#[test]
fn mining_creative_and_zero_hardness_start_remain_instant() {
    let (mut state, actor, _) = setup("bedrock");
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(plan_client_build(&state, actor, &input(DigAction::Start)).is_some());
    assert_eq!(direct(&mut state, DigAction::Start).0.len(), 1);
    let (mut state, actor, _) = setup("torch");
    assert!(plan_client_build(&state, actor, &input(DigAction::Start)).is_some());
    assert_eq!(direct(&mut state, DigAction::Start).0.len(), 1);
}

#[test]
fn mining_finish_packet_burst_and_tool_switch_do_not_credit_unelapsed_ticks() {
    let (mut state, actor, stone) = setup("stone");
    direct(&mut state, DigAction::Start);
    let mut pick = crate::crafter::tests::stack(&state, "diamond_pickaxe", 1);
    pick.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:efficiency").unwrap(),
        level: 5,
    });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = pick;
    for _ in 0..100 {
        assert!(direct(&mut state, DigAction::Finish).0.is_empty());
        mining::advance(&mut state);
    }
    assert!(!mining::allows(
        &state,
        actor,
        POS,
        stone,
        DigAction::Finish
    ));
    let other = BlockPos {
        x: POS.x + 1,
        ..POS
    };
    ready_to_finish(&mut state, CONNECTION, POS);
    state.world.set_block(other.x, other.y, other.z, stone);
    assert!(!mining::allows(
        &state,
        actor,
        other,
        stone,
        DigAction::Finish
    ));
}

#[test]
fn mining_early_finish_completes_without_another_packet() {
    let (mut state, _, stone) = setup("stone");
    run_tick(
        &mut state,
        &[input(DigAction::Start), input(DigAction::Finish)],
    );
    assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(stone));
    for _ in 0..45 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air),
        "an early Finish must remain pending until server progress catches up"
    );
}

#[test]
fn mining_committed_break_preserves_newer_target_in_same_batch() {
    let (mut state, actor, stone) = setup("stone");
    let other = BlockPos {
        x: POS.x + 1,
        ..POS
    };
    state.world.set_block(other.x, other.y, other.z, stone);
    state.world.fold_overlay();
    ready_to_finish(&mut state, CONNECTION, POS);
    run_tick(
        &mut state,
        &[
            input(DigAction::Finish),
            PlayerInput::Dig {
                connection: CONNECTION,
                position: other,
                action: DigAction::Start,
                sequence: 2,
            },
        ],
    );
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert!(
        state.players[&actor].dig_progress.is_some(),
        "break A consumed Start B"
    );
    for _ in 0..45 {
        run_tick(&mut state, &[]);
    }
    run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: CONNECTION,
            position: other,
            action: DigAction::Finish,
            sequence: 3,
        }],
    );
    assert_eq!(
        state.world.block_at(other.x, other.y, other.z),
        Some(state.registry.states.air)
    );
}

#[test]
fn mining_delayed_finish_survives_new_start_and_waits_for_full_progress() {
    let (mut state, actor, stone) = setup("stone");
    let other = BlockPos {
        x: POS.x + 1,
        ..POS
    };
    state.world.set_block(other.x, other.y, other.z, stone);
    state.world.fold_overlay();
    run_tick(
        &mut state,
        &[
            input(DigAction::Start),
            input(DigAction::Finish),
            PlayerInput::Dig {
                connection: CONNECTION,
                position: other,
                action: DigAction::Start,
                sequence: 2,
            },
        ],
    );
    for _ in 0..32 {
        run_tick(&mut state, &[]);
    }
    assert!(mining::allows(&state, actor, POS, stone, DigAction::Finish));
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(stone),
        "delayed destruction must wait beyond the immediate 70% threshold"
    );
    for _ in 0..15 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert!(
        state.players[&actor].dig_progress.is_some(),
        "delayed A consumed Start B"
    );
    run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: CONNECTION,
            position: other,
            action: DigAction::Finish,
            sequence: 3,
        }],
    );
    assert_eq!(
        state.world.block_at(other.x, other.y, other.z),
        Some(state.registry.states.air)
    );
    assert!(state.players[&actor].delayed_dig.is_none());
}

#[test]
fn mining_delayed_finish_aborts_on_cancel_replacement_reach_and_teleport() {
    for invalidation in ["cancel", "replacement", "reach", "teleport"] {
        let (mut state, actor, stone) = setup("stone");
        run_tick(
            &mut state,
            &[input(DigAction::Start), input(DigAction::Finish)],
        );
        assert!(state.players[&actor].delayed_dig.is_some());
        match invalidation {
            "cancel" => {
                run_tick(&mut state, &[input(DigAction::Cancel)]);
            }
            "replacement" => {
                let obsidian = block(&state, "obsidian", &[]);
                state.world.set_block(POS.x, POS.y, POS.z, obsidian);
                mining::advance(&mut state);
                state.world.set_block(POS.x, POS.y, POS.z, stone);
            }
            "reach" => {
                let slot = state.entities.slot_of(actor).unwrap();
                state.entities.set_pos(slot, [100.0, 201.0, 100.0]);
                mining::advance(&mut state);
                state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
            }
            "teleport" => {
                state.players.get_mut(&actor).unwrap().begin_teleport();
            }
            _ => unreachable!(),
        }
        for _ in 0..50 {
            run_tick(&mut state, &[]);
        }
        assert_eq!(
            state.world.block_at(POS.x, POS.y, POS.z),
            Some(stone),
            "{invalidation}"
        );
        assert!(state.players[&actor].delayed_dig.is_none());
    }
}

#[test]
fn mining_grounded_rotation_status_and_level_move_keep_vanilla_hand_rate() {
    for packet in ["rotation", "status", "position"] {
        let (mut state, actor, grass) = setup("grass_block");
        let stone = block(&state, "stone", &[]);
        let slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
        state.entities.set_ground(slot, false);
        state.world.set_block(8, 200, 10, stone);
        state.world.fold_overlay();
        let movement = match packet {
            "rotation" => PlayerInput::MoveRotation {
                connection: CONNECTION,
                yaw: 10.0,
                pitch: 20.0,
                on_ground: true,
            },
            "status" => PlayerInput::MoveStatus {
                connection: CONNECTION,
                on_ground: true,
            },
            "position" => PlayerInput::Move {
                connection: CONNECTION,
                position: [8.75, 201.0, 10.5],
                yaw: 10.0,
                pitch: 20.0,
                on_ground: true,
            },
            _ => unreachable!(),
        };
        run_tick(&mut state, &[movement, input(DigAction::Start)]);
        let rate = crate::effects::mining_progress(&state, actor, grass);
        assert!(
            state.entities.ground(slot),
            "{packet}: on_ground=false, rate={rate}"
        );
        assert!(
            (rate - 1.0 / 18.0).abs() < 0.000001,
            "{packet}: rate={rate}"
        );
        for _ in 0..17 {
            run_tick(&mut state, &[]);
        }
        run_tick(&mut state, &[input(DigAction::Finish)]);
        assert_eq!(
            state.world.block_at(POS.x, POS.y, POS.z),
            Some(state.registry.states.air)
        );
    }
}

#[test]
fn mining_airborne_ground_claim_does_not_remove_hand_rate_penalty() {
    let (mut state, actor, grass) = setup("grass_block");
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, [8.5, 203.0, 10.5]);
    state.entities.set_ground(slot, false);
    run_tick(
        &mut state,
        &[PlayerInput::MoveStatus {
            connection: CONNECTION,
            on_ground: true,
        }],
    );
    assert!(!state.entities.ground(slot));
    assert!((crate::effects::mining_progress(&state, actor, grass) - 1.0 / 90.0).abs() < 0.000001);
}

#[test]
fn zero_hardness_break_does_not_damage_tool() {
    let (mut state, actor, _) = setup("torch");
    let pickaxe = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(pickaxe, 1);
    run_tick(&mut state, &[input(DigAction::Start)]);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert_eq!(state.players[&actor].inventory.slots[0].damage, None);
}

#[test]
fn breaking_supported_ice_leaves_water() {
    let (mut state, _, _) = setup("ice");
    state
        .world
        .set_block(POS.x, POS.y - 1, POS.z, state.registry.states.stone);
    state.world.fold_overlay();
    ready_to_finish(&mut state, CONNECTION, POS);
    run_tick(&mut state, &[input(DigAction::Finish)]);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.water)
    );
}

#[test]
fn ice_break_respects_support_silk_touch_creative_and_nether() {
    let (mut state, actor, ice) = setup("ice");
    let below = BlockPos {
        y: POS.y - 1,
        ..POS
    };
    let air = state.registry.states.air;
    let water = state.registry.states.water;
    state.world.set_block(below.x, below.y, below.z, air);
    assert_eq!(
        build::plan_break(&state, actor, POS, ice)
            .footprint
            .block_write[0]
            .1,
        air
    );
    state.world.set_block(below.x, below.y, below.z, water);
    assert_eq!(
        build::plan_break(&state, actor, POS, ice)
            .footprint
            .block_write[0]
            .1,
        water
    );
    let stone = state.registry.states.stone;
    state.world.set_block(below.x, below.y, below.z, stone);
    let tool = &mut state.players.get_mut(&actor).unwrap().inventory.slots[0];
    tool.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:silk_touch").unwrap(),
        level: 1,
    });
    assert_eq!(
        build::plan_break(&state, actor, POS, ice)
            .footprint
            .block_write[0]
            .1,
        air
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[0]
        .enchantments
        .clear();
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert_eq!(
        build::plan_break(&state, actor, POS, ice)
            .footprint
            .block_write[0]
            .1,
        air
    );
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Survival;
    state.dimension = crate::dimensions::Dimension::Nether;
    assert_eq!(
        build::plan_break(&state, actor, POS, ice)
            .footprint
            .block_write[0]
            .1,
        air
    );
}
