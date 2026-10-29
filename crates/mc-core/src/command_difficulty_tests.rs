//! Difficulty query results are numeric outcomes, not translated feedback.
use crate::boundary::{ChangeSet, ConnectionId, PlayerChange, PlayerInput};
use crate::command_context::{Context, run};
use crate::command_execution::{CommandResult, execute, supports_store_result};
use crate::commands::{Command, parse};
use crate::scoreboard::ScoreboardEvent;
use crate::survival::Difficulty;
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag, tag_type};

// Independently established from the official Java 26.1.2 Difficulty IDs.
const DIFFICULTIES: [(Difficulty, &str, i32); 4] = [
    (Difficulty::Peaceful, "peaceful", 0),
    (Difficulty::Easy, "easy", 1),
    (Difficulty::Normal, "normal", 2),
    (Difficulty::Hard, "hard", 3),
];

fn fixture() -> (TickState, Context) {
    let (mut state, executor, _) = crate::hostile_tests::setup(8.0);
    state.outbox.clear();
    let context = Context::source(&state, ConnectionId(1), executor, 2);
    (state, context)
}

fn feedback(key: &str, difficulty: &str) -> PlayerChange {
    PlayerChange::StyledChat {
        content: Tag::Compound(vec![
            ("translate".into(), Tag::String(key.into())),
            (
                "with".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(vec![(
                        "translate".into(),
                        Tag::String(format!("options.difficulty.{difficulty}")),
                    )])],
                }),
            ),
        ]),
    }
}

fn score(owner: &str, value: i32) -> PlayerChange {
    PlayerChange::Scoreboard {
        event: ScoreboardEvent::Score {
            owner: owner.into(),
            objective: "oracle".into(),
            score: value,
        },
    }
}

fn no_gameplay_changes(changes: &ChangeSet) {
    assert!(changes.blocks.is_empty());
    assert!(changes.chunks_loaded.is_empty());
    assert!(changes.chunks_unloaded.is_empty());
    assert!(changes.entities_spawned.is_empty());
    assert!(changes.entities_appearance_changed.is_empty());
    assert!(changes.entities_removed.is_empty());
    assert!(changes.entity_events.is_empty());
    assert!(changes.explosions.is_empty());
}

#[test]
fn difficulty_query_returns_all_vanilla_ids_without_gameplay_mutation() {
    let (mut state, context) = fixture();
    let command = parse("difficulty", true).unwrap();
    assert!(matches!(command, Command::DifficultyQuery));
    assert!(supports_store_result(&command));
    state.dimension_globals.active = true;
    let before = (state.tick, state.day_time, state.gamerules.clone());
    let dirty = (state.gamerules_dirty, state.scoreboard_dirty);
    let slot = state.entities.slot_of(context.executor).unwrap();
    let position = state.entities.pos(slot);
    for (difficulty, name, id) in DIFFICULTIES {
        state.difficulty = difficulty;
        state.outbox.clear();
        let mut changes = ChangeSet::default();
        assert_eq!(
            execute(
                &mut state,
                context.connection,
                context.executor,
                &command,
                &mut changes,
            ),
            CommandResult::Success(id),
            "{name} query must succeed even when its ID is zero"
        );
        assert_eq!(
            state.outbox[&context.connection],
            vec![feedback("commands.difficulty.query", name)]
        );
        assert_eq!(state.outbox.len(), 1);
        assert_eq!(state.difficulty, difficulty);
        assert_eq!(
            (state.tick, state.day_time, state.gamerules.clone()),
            before
        );
        assert_eq!((state.gamerules_dirty, state.scoreboard_dirty), dirty);
        assert_eq!(state.entities.pos(slot), position);
        assert!(state.dimension_globals.broadcasts.is_empty());
        no_gameplay_changes(&changes);
    }
}

fn store_matrix(mode: &str) {
    let (mut state, context) = fixture();
    run(
        &mut state,
        "scoreboard objectives add oracle dummy",
        context,
        &mut ChangeSet::default(),
    );
    for (difficulty, name, id) in DIFFICULTIES {
        state.difficulty = difficulty;
        run(
            &mut state,
            "scoreboard players set value oracle -99",
            context,
            &mut ChangeSet::default(),
        );
        state.outbox.clear();
        state.scoreboard_dirty = false;
        let before = (state.tick, state.day_time, state.gamerules.clone());
        let mut changes = ChangeSet::default();
        run(
            &mut state,
            &format!("execute store {mode} score value oracle run difficulty"),
            context,
            &mut changes,
        );
        let expected = if mode == "result" { id } else { 1 };
        assert_eq!(state.scoreboard.scores["value"]["oracle"].value, expected);
        assert!(state.scoreboard_dirty);
        assert_eq!(
            state.outbox[&context.connection],
            vec![
                feedback("commands.difficulty.query", name),
                score("value", expected)
            ]
        );
        assert_eq!(state.difficulty, difficulty);
        assert_eq!(
            (state.tick, state.day_time, state.gamerules.clone()),
            before
        );
        assert!(state.dimension_globals.broadcasts.is_empty());
        assert!(state.command_context.current.is_none());
        no_gameplay_changes(&changes);
    }
}

#[test]
fn difficulty_query_store_result_writes_all_four_ids_after_feedback() {
    store_matrix("result");
}

#[test]
fn difficulty_query_store_success_is_one_including_peaceful_zero() {
    store_matrix("success");
}

#[test]
fn difficulty_query_nested_execute_binds_executor_keeps_feedback_source_and_restores_context() {
    let (mut state, context) = fixture();
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::new_v4(),
            name: "Bob".into(),
            permission_level: 0,
        }],
    );
    run(
        &mut state,
        "scoreboard objectives add oracle dummy",
        context,
        &mut ChangeSet::default(),
    );
    state.difficulty = Difficulty::Hard;
    state.outbox.clear();
    let bob = state.connections.entity_of(ConnectionId(2)).unwrap();
    let slot = state.entities.slot_of(bob).unwrap();
    let position = state.entities.pos(slot);
    let outer = Context {
        position: [90.0, 75.0, -30.0],
        permission: 3,
        ..context
    };
    state.command_context.current = Some(outer);
    let mut changes = ChangeSet::default();
    run(
        &mut state,
        "execute as Bob at @s positioned 10 70 20 run execute store result score @s oracle store success score success oracle run difficulty",
        context,
        &mut changes,
    );
    assert_eq!(state.scoreboard.scores["Bob"]["oracle"].value, 3);
    assert_eq!(state.scoreboard.scores["success"]["oracle"].value, 1);
    assert_eq!(
        state.outbox[&context.connection],
        vec![
            feedback("commands.difficulty.query", "hard"),
            score("Bob", 3),
            score("success", 1)
        ]
    );
    assert_eq!(
        state.outbox[&ConnectionId(2)],
        vec![score("Bob", 3), score("success", 1)]
    );
    let restored = state.command_context.current.unwrap();
    assert_eq!(restored.executor, outer.executor);
    assert_eq!(restored.connection, outer.connection);
    assert_eq!(restored.permission, outer.permission);
    assert_eq!(restored.position, outer.position);
    assert_eq!(restored.rotation, outer.rotation);
    assert_eq!(restored.dimension, outer.dimension);
    assert_eq!(restored.eyes, outer.eyes);
    assert_eq!(restored.eye_height, outer.eye_height);
    assert_eq!(state.entities.pos(slot), position);
    assert_eq!(state.difficulty, Difficulty::Hard);
    no_gameplay_changes(&changes);
}

#[test]
fn difficulty_setter_reports_success_and_same_value_failure() {
    let (mut state, context) = fixture();
    assert_ne!(state.difficulty, Difficulty::Peaceful);
    state.dimension_globals.active = true;
    for (difficulty, name, id) in DIFFICULTIES {
        let command = parse(&format!("difficulty {name}"), true).unwrap();
        assert!(!supports_store_result(&command));
        state.outbox.clear();
        state.dimension_globals.broadcasts.clear();
        assert_eq!(
            execute(
                &mut state,
                context.connection,
                context.executor,
                &command,
                &mut ChangeSet::default()
            ),
            CommandResult::Success(1)
        );
        assert_eq!(state.difficulty, difficulty);
        assert!(
            matches!(state.dimension_globals.broadcasts.as_slice(), [crate::dimensions::globals::Broadcast::Difficulty(value)] if i32::from(*value) == id)
        );
        assert_eq!(
            state.outbox[&context.connection],
            vec![
                PlayerChange::DifficultyChanged {
                    difficulty: id as u8
                },
                feedback("commands.difficulty.success", name)
            ]
        );
        state.outbox.clear();
        state.dimension_globals.broadcasts.clear();
        assert_eq!(
            execute(
                &mut state,
                context.connection,
                context.executor,
                &command,
                &mut ChangeSet::default()
            ),
            CommandResult::Failed
        );
        assert_eq!(state.difficulty, difficulty);
        assert!(state.dimension_globals.broadcasts.is_empty());
        assert_eq!(
            state.outbox[&context.connection],
            vec![feedback("commands.difficulty.failure", name)]
        );
    }
}

#[test]
fn difficulty_setter_store_still_rejects_before_mutation() {
    let (mut state, context) = fixture();
    run(
        &mut state,
        "scoreboard objectives add oracle dummy",
        context,
        &mut ChangeSet::default(),
    );
    state.outbox.clear();
    let before = state.difficulty;
    assert_ne!(before, Difficulty::Hard);
    let mut changes = ChangeSet::default();
    run(
        &mut state,
        "execute store result score result oracle store success score success oracle run difficulty hard",
        context,
        &mut changes,
    );
    assert_eq!(state.difficulty, before);
    assert_eq!(state.scoreboard.scores["result"]["oracle"].value, 0);
    assert_eq!(state.scoreboard.scores["success"]["oracle"].value, 0);
    assert_eq!(
        state.outbox[&context.connection],
        vec![
            PlayerChange::Chat {
                message: "Command does not support execute store".into()
            },
            score("result", 0),
            score("success", 0)
        ]
    );
    assert!(state.command_context.current.is_none());
    no_gameplay_changes(&changes);
}
