//! Regression contracts first pinned during the command-locality pilot.
use crate::boundary::{ChangeSet, ConnectionId, PlayerChange};
use crate::command_context::{Context, run};
use crate::command_execution::supports_store_result;
use crate::commands::{Command, parse};
use crate::survival::Difficulty;
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag, tag_type};

fn fixture() -> (TickState, Context) {
    let (mut state, executor, _) = crate::hostile_tests::setup(8.0);
    state.outbox.clear();
    let context = Context::source(&state, ConnectionId(1), executor, 2);
    (state, context)
}

#[test]
fn nested_tick_freeze_requires_level_three_native_admission() {
    let (mut state, context) = fixture();
    for permission in [0, 1, 2] {
        run(
            &mut state,
            "execute run tick freeze",
            Context {
                permission,
                ..context
            },
            &mut ChangeSet::default(),
        );
        assert!(!state.tick_control.frozen());
    }
    state.outbox.clear();

    // Permission hardening intentionally closes the old level2 nested bypass.
    run(
        &mut state,
        "execute run tick freeze",
        Context {
            permission: 3,
            ..context
        },
        &mut ChangeSet::default(),
    );
    assert!(state.tick_control.frozen());
    assert_eq!(
        state.outbox[&context.connection].last(),
        Some(&PlayerChange::StyledChat {
            content: Tag::Compound(vec![(
                "translate".into(),
                Tag::String("commands.tick.status.frozen".into()),
            )]),
        })
    );
    assert!(state.command_context.current.is_none());
}

#[test]
fn difficulty_query_store_returns_numeric_result_after_feedback() {
    let (mut state, context) = fixture();
    state.difficulty = Difficulty::Hard;
    let command = parse("difficulty", true).unwrap();
    assert!(matches!(command, Command::DifficultyQuery));
    assert!(supports_store_result(&command));
    run(
        &mut state,
        "scoreboard objectives add oracle dummy",
        context,
        &mut ChangeSet::default(),
    );
    state.outbox.clear();
    let query_feedback = PlayerChange::StyledChat {
        content: Tag::Compound(vec![
            (
                "translate".into(),
                Tag::String("commands.difficulty.query".into()),
            ),
            (
                "with".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(vec![(
                        "translate".into(),
                        Tag::String("options.difficulty.hard".into()),
                    )])],
                }),
            ),
        ]),
    };
    run(&mut state, "difficulty", context, &mut ChangeSet::default());
    assert_eq!(
        state.outbox[&context.connection],
        vec![query_feedback.clone()]
    );
    state.outbox.clear();

    // Query feedback precedes both numeric-result and success writes.
    run(
        &mut state,
        "execute store result score result oracle store success score success oracle run difficulty",
        context,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.scoreboard.scores["result"]["oracle"].value, 3);
    assert_eq!(state.scoreboard.scores["success"]["oracle"].value, 1);
    assert_eq!(state.difficulty, Difficulty::Hard);
    assert_eq!(
        state.outbox[&context.connection],
        vec![
            query_feedback,
            PlayerChange::Scoreboard {
                event: crate::scoreboard::ScoreboardEvent::Score {
                    owner: "result".into(),
                    objective: "oracle".into(),
                    score: 3,
                },
            },
            PlayerChange::Scoreboard {
                event: crate::scoreboard::ScoreboardEvent::Score {
                    owner: "success".into(),
                    objective: "oracle".into(),
                    score: 1,
                },
            },
        ]
    );
    assert!(state.command_context.current.is_none());
}
