//! Boundary coverage for the native pre-commit question.
//!
//! The runtime that answers a question lives in another crate, so what is pinned
//! here is only the boundary's own contract: one bounded FIFO shared with events
//! and control input, one absolute deadline, one in-flight bound, one live ticket
//! per decision, and one commit point that refuses everything a game owner may not
//! apply. Nothing here sleeps or waits on a clock: every deadline is injected, and
//! every host answer is taken from the boundary's own FIFO by this test itself.

use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use super::*;
use crate::precommit::{
    self, HookActor, HookContext, HookDecision, HookFailure, HookFailurePolicy, HookKind,
    HookRegistration, HookRosterError, MAX_PRECOMMIT_BUILD_EDITS, MAX_PRECOMMIT_REQUESTS,
};

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("non-zero queue capacity")
}

fn registration(
    plugin_id: &str,
    kind: HookKind,
    order: i32,
    on_failure: HookFailurePolicy,
) -> HookRegistration {
    HookRegistration::new(plugin_id, kind, order, on_failure)
}

fn judge(kind: HookKind) -> HookRegistration {
    registration("judge", kind, 0, HookFailurePolicy::Deny)
}

fn player() -> precommit::HookPlayer {
    precommit::HookPlayer::try_new("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", 7)
        .expect("a player identity")
}

fn build_question() -> HookContext {
    HookContext::Build(
        precommit::BuildContext::try_new(
            HookActor::Environment,
            "minecraft:overworld",
            vec![precommit::BuildEdit {
                x: 1,
                y: 64,
                z: -2,
                previous_state: 0,
                proposed_state: 4,
            }],
        )
        .expect("a bounded build question"),
    )
}

fn damage_question(amount: f32) -> HookContext {
    HookContext::Damage(
        precommit::DamageContext::try_new(
            HookActor::Environment,
            precommit::DamageTarget::Player(player()),
            "minecraft:generic",
            "minecraft:overworld",
            ScriptPosition::try_new(0.0, 64.0, 0.0).expect("a bounded position"),
            amount,
        )
        .expect("a bounded damage question"),
    )
}

fn plugin_damage_question(plugin_id: &str, amount: f32) -> HookContext {
    HookContext::Damage(
        precommit::DamageContext::try_new(
            HookActor::Plugin(plugin_id.to_owned()),
            precommit::DamageTarget::Player(player()),
            "minecraft:generic",
            "minecraft:overworld",
            ScriptPosition::try_new(0.0, 64.0, 0.0).expect("a bounded position"),
            amount,
        )
        .expect("a bounded damage question"),
    )
}

/// One package's manifest, with the grant a programmatic source needs.
fn source_manifest(id: &str, capability: &str) -> ValidatedScriptPluginManifest {
    ScriptPluginManifest::new(id, id, "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_capability(capability)
        .expect("a known capability")
        .validate()
        .expect("a valid test manifest")
}

/// Take the one question a test just admitted, as the host owns it.
fn take_request(endpoint: &mut ScriptHostEndpoint) -> precommit::Request {
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request,
        _ => panic!("expected the queued pre-commit question on the host's FIFO"),
    }
}

/// Answer the next question on the FIFO, the way the runtime does.
fn answer_next(endpoint: &mut ScriptHostEndpoint, decision: HookDecision) {
    take_request(endpoint)
        .answer(decision)
        .expect("a live requester");
}

#[tokio::test]
async fn a_boundary_without_registrations_answers_the_direct_path() {
    // One event slot, already taken: a question that was queued would have to wait
    // for room, so the answer this test reads is the direct path itself.
    let (boundary, _endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    boundary
        .try_enqueue_event(ScriptEvent::server_tick(1))
        .expect("an empty FIFO takes one event");

    assert!(!boundary.has_precommit_hooks(HookKind::Build));
    assert!(!boundary.has_precommit_hooks(HookKind::Damage));

    let mut approval = boundary
        .begin_precommit(build_question())
        .expect("no registration is no question")
        .resolve()
        .await
        .expect("the direct path decides immediately");
    assert_eq!(approval.decision(), HookDecision::Keep);
    assert_eq!(approval.consume(), Ok(HookDecision::Keep));
    // The direct path still hands out one ticket, not a reusable one.
    assert_eq!(approval.consume(), Err(HookFailure::Stale));
}

#[tokio::test]
async fn a_registered_question_reaches_the_host_and_commits_once() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Build)])
        .expect("a roster of one");
    assert!(boundary.has_precommit_hooks(HookKind::Build));
    assert!(!boundary.has_precommit_hooks(HookKind::Damage));

    let pending = boundary
        .begin_precommit(build_question())
        .expect("a registered boundary queues the question");
    let request = take_request(&mut endpoint);
    assert_eq!(request.kind(), HookKind::Build);
    assert!(
        !request.is_expired(),
        "the question arrives inside its bound"
    );
    let HookContext::Build(context) = request.context() else {
        panic!("a build question arrives as a build question");
    };
    assert_eq!(context.dimension(), "minecraft:overworld");
    assert_eq!(context.edits().len(), 1);
    assert_eq!(context.edits()[0].proposed_state, 4);
    request
        .answer(HookDecision::Keep)
        .expect("a live requester");

    let mut approval = pending.resolve().await.expect("the host answered");
    assert_eq!(approval.kind(), HookKind::Build);
    assert_eq!(approval.decision(), HookDecision::Keep);
    // One live ticket decides it, whichever copy of the approval consumes it.
    let mut copy = approval.clone();
    assert_eq!(copy.consume(), Ok(HookDecision::Keep));
    assert_eq!(approval.consume(), Err(HookFailure::Stale));
}

#[tokio::test]
async fn a_cancelled_decision_is_refused_at_the_commit() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Build)])
        .expect("a roster of one");

    let pending = boundary
        .begin_precommit(build_question())
        .expect("a registered boundary queues the question");
    answer_next(&mut endpoint, HookDecision::Cancel);

    let mut approval = pending.resolve().await.expect("the host answered");
    // The decision is readable before the commit, so an owner can refuse the
    // effect early; the commit refuses it as well, so an owner that only consumes
    // cannot apply it either.
    assert_eq!(approval.decision(), HookDecision::Cancel);
    assert!(approval.decision().is_cancelled());
    assert_eq!(approval.consume(), Err(HookFailure::Cancelled));
}

#[tokio::test]
async fn a_decision_the_question_cannot_carry_is_refused() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Build)])
        .expect("a roster of one");

    let pending = boundary
        .begin_precommit(build_question())
        .expect("a registered boundary queues the question");
    // A replacement is a damage answer: a build question cannot be answered with
    // one, whatever a host tries to answer it with.
    answer_next(&mut endpoint, HookDecision::Replace(1.0));
    assert_eq!(pending.resolve().await.err(), Some(HookFailure::Invalid));

    assert!(!HookDecision::Replace(1.0).is_valid_for(HookKind::Build));
    assert!(HookDecision::Replace(0.0).is_valid_for(HookKind::Damage));
    assert_eq!(HookDecision::replacement(f32::NAN), None);
    assert_eq!(HookDecision::replacement(-1.0), None);
}

#[tokio::test]
async fn an_expired_approval_is_refused_at_the_commit() {
    // The deadline is injected as already passed. The direct path answers at once,
    // so what is exercised here is the commit's own expiry fence rather than a
    // race against a clock.
    let (boundary, _endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let mut approval = boundary
        .begin_precommit_with_deadline(build_question(), Instant::now())
        .expect("no registration is no question")
        .resolve()
        .await
        .expect("the direct path decides immediately");
    assert_eq!(approval.decision(), HookDecision::Keep);
    assert_eq!(approval.consume(), Err(HookFailure::Expired));
}

#[tokio::test]
async fn a_question_the_host_never_answers_ends_at_its_deadline() {
    // A registered question with no consumer and an already passed deadline: the
    // owner's wait ends by itself instead of blocking gameplay forever.
    let (boundary, _endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Damage)])
        .expect("a roster of one");
    assert_eq!(
        boundary
            .begin_precommit_with_deadline(damage_question(20.0), Instant::now())
            .expect("a registered boundary queues the question")
            .resolve()
            .await
            .err(),
        Some(HookFailure::Expired)
    );
}

#[tokio::test]
async fn a_roster_change_makes_a_live_ticket_stale() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let hooks = vec![judge(HookKind::Damage)];
    boundary
        .set_precommit_hooks(hooks.clone())
        .expect("a roster of one");

    let pending = boundary
        .begin_precommit(damage_question(20.0))
        .expect("a registered boundary queues the question");
    answer_next(&mut endpoint, HookDecision::Replace(5.0));
    let mut approval = pending.resolve().await.expect("the host answered");
    assert_eq!(approval.amount(), Some(5.0));

    // The same registrations published again are a new roster: a decision admitted
    // under the old one is not a decision this boundary may still commit.
    boundary
        .set_precommit_hooks(hooks)
        .expect("the roster is still valid");
    assert_eq!(approval.consume(), Err(HookFailure::Stale));
}

#[tokio::test]
async fn a_full_fifo_reports_queue_full_without_waiting() {
    let (boundary, _endpoint) = script_boundary_pair(nonzero(2), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Build)])
        .expect("a roster of one");

    let _first = boundary
        .begin_precommit(build_question())
        .expect("room for one");
    let _second = boundary
        .begin_precommit(build_question())
        .expect("room for two");
    assert!(matches!(
        boundary.begin_precommit(build_question()),
        Err(HookFailure::QueueFull)
    ));
}

#[tokio::test]
async fn in_flight_questions_are_bounded_and_their_slots_come_back() {
    let (boundary, mut endpoint) =
        script_boundary_pair(nonzero(MAX_SCRIPT_EVENT_QUEUE_CAPACITY), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Damage)])
        .expect("a roster of one");

    let mut pending = Vec::new();
    for _ in 0..MAX_PRECOMMIT_REQUESTS {
        pending.push(
            boundary
                .begin_precommit(damage_question(20.0))
                .expect("the in-flight bound is not reached yet"),
        );
    }
    assert!(matches!(
        boundary.begin_precommit(damage_question(20.0)),
        Err(HookFailure::QueueFull)
    ));

    // The host takes the whole backlog and drops it unread: an unanswered question
    // is a refused one, and taking it is what releases its in-flight slot.
    for _ in 0..MAX_PRECOMMIT_REQUESTS {
        drop(take_request(&mut endpoint));
    }
    for pending in pending {
        assert!(matches!(
            pending.resolve().await,
            Err(HookFailure::Unavailable)
        ));
    }

    let pending = boundary
        .begin_precommit(damage_question(20.0))
        .expect("the released slot admits one more question");
    answer_next(&mut endpoint, HookDecision::Keep);
    let mut approval = pending.resolve().await.expect("the host answered");
    assert_eq!(approval.consume(), Ok(HookDecision::Keep));
}

#[tokio::test]
async fn an_answered_request_holds_its_slot_until_approval_is_terminal() {
    let (boundary, mut endpoint) =
        script_boundary_pair(nonzero(MAX_SCRIPT_EVENT_QUEUE_CAPACITY), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Damage)])
        .expect("a roster of one");

    let mut approvals = Vec::new();
    for _ in 0..MAX_PRECOMMIT_REQUESTS {
        let pending = boundary
            .begin_precommit(damage_question(20.0))
            .expect("the in-flight bound is not reached yet");
        answer_next(&mut endpoint, HookDecision::Keep);
        approvals.push(pending.resolve().await.expect("the host answered"));
    }
    assert!(matches!(
        boundary.begin_precommit(damage_question(20.0)),
        Err(HookFailure::QueueFull)
    ));

    drop(approvals.pop());
    let pending = boundary
        .begin_precommit(damage_question(20.0))
        .expect("dropping the terminal approval releases exactly one slot");
    answer_next(&mut endpoint, HookDecision::Keep);
    let mut approval = pending.resolve().await.expect("the host answered");
    assert_eq!(approval.consume(), Ok(HookDecision::Keep));
}

#[tokio::test]
async fn a_revoked_source_registration_makes_the_decision_stale() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    endpoint
        .register_plugin_routes(&source_manifest("actor-plugin", "entity_damage"))
        .expect("the source package holds a live registration");
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Damage)])
        .expect("a roster of one");
    let context = plugin_damage_question("actor-plugin", 20.0);

    // While the registration is live, the package's own request commits.
    let pending = boundary
        .begin_precommit(context.clone())
        .expect("the source holds a live registration");
    answer_next(&mut endpoint, HookDecision::Keep);
    let mut approval = pending.resolve().await.expect("the host answered");
    assert_eq!(approval.consume(), Ok(HookDecision::Keep));

    // The same question asked again, answered by the host, and committed after the
    // authority it came from was withdrawn: the decision is refused, so authority
    // that no longer exists can never be spent.
    let pending = boundary
        .begin_precommit(context)
        .expect("the source still held its registration when it asked");
    answer_next(&mut endpoint, HookDecision::Keep);
    let mut later = pending.resolve().await.expect("the host answered");
    endpoint.unregister_plugin_routes("actor-plugin");
    assert_eq!(later.consume(), Err(HookFailure::Stale));
}

#[tokio::test]
async fn a_question_without_a_live_source_registration_is_refused_before_the_host() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    // The concrete operation that created a plugin request was already admitted
    // against its own capability. Hook kind is not a second capability oracle:
    // a live registration is the source fence, whatever operation produced it.
    endpoint
        .register_plugin_routes(&source_manifest("actor-plugin", "player_queries"))
        .expect("the source package holds a live registration");
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Damage)])
        .expect("a roster of one");

    assert!(
        boundary
            .begin_precommit(plugin_damage_question("actor-plugin", 20.0))
            .is_ok()
    );
    // A package with no live registration at all has nothing to ask under.
    assert!(matches!(
        boundary.begin_precommit(plugin_damage_question("retired-plugin", 20.0)),
        Err(HookFailure::Stale)
    ));
}

#[test]
fn a_question_outside_its_bounds_cannot_be_built() {
    let edits = vec![
        precommit::BuildEdit {
            x: 0,
            y: 0,
            z: 0,
            previous_state: 1,
            proposed_state: 2,
        };
        MAX_PRECOMMIT_BUILD_EDITS + 1
    ];
    assert_eq!(
        precommit::BuildContext::try_new(HookActor::Environment, "minecraft:overworld", edits),
        Err(precommit::HookContextError::TooManyEdits {
            count: MAX_PRECOMMIT_BUILD_EDITS + 1,
            max: MAX_PRECOMMIT_BUILD_EDITS,
        })
    );
    assert_eq!(
        precommit::BuildContext::try_new(HookActor::Environment, "", Vec::new()),
        Err(precommit::HookContextError::EmptyValue {
            field: "build dimension",
        })
    );
    for amount in [f32::NAN, f32::INFINITY, -0.5] {
        let refused = precommit::DamageContext::try_new(
            HookActor::Environment,
            precommit::DamageTarget::Entity(3),
            "minecraft:generic",
            "minecraft:overworld",
            ScriptPosition::try_new(0.0, 64.0, 0.0).expect("a bounded position"),
            amount,
        );
        // The refusal names the amount it refused, compared by bits so a
        // not-a-number amount is still an exact answer.
        let Err(precommit::HookContextError::InvalidAmount { amount: refused }) = refused else {
            panic!("only a finite, non-negative amount is a damage request");
        };
        assert_eq!(refused.to_bits(), amount.to_bits());
    }
    assert_eq!(
        precommit::HookPlayer::try_new("", 1),
        Err(precommit::HookContextError::EmptyValue {
            field: "hook player uuid",
        })
    );
}

#[test]
fn a_malformed_roster_is_refused_and_leaves_the_live_one_alone() {
    let (boundary, _endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    boundary
        .set_precommit_hooks(vec![
            registration("first", HookKind::Build, 0, HookFailurePolicy::Deny),
            registration("second", HookKind::Build, 10, HookFailurePolicy::Keep),
        ])
        .expect("a roster of two");
    let generation = boundary.precommit.generation();

    assert_eq!(
        boundary.set_precommit_hooks(vec![
            registration("first", HookKind::Build, 0, HookFailurePolicy::Deny),
            registration("first", HookKind::Build, 1, HookFailurePolicy::Keep),
        ]),
        Err(HookRosterError::Duplicate {
            plugin_id: "first".to_owned(),
            kind: HookKind::Build,
        })
    );
    assert_eq!(
        boundary.set_precommit_hooks(vec![registration(
            "",
            HookKind::Damage,
            0,
            HookFailurePolicy::Deny
        )]),
        Err(HookRosterError::EmptyPluginId)
    );

    // The refused rosters published nothing: the live one still answers, and its
    // generation is the one every ticket admitted under it was created with.
    assert!(boundary.has_precommit_hooks(HookKind::Build));
    assert!(!boundary.has_precommit_hooks(HookKind::Damage));
    assert_eq!(boundary.precommit.generation(), generation);
}

#[tokio::test]
async fn the_deadline_created_for_a_question_covers_the_whole_chain() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    boundary
        .set_precommit_hooks(vec![judge(HookKind::Damage)])
        .expect("a roster of one");

    let pending = boundary
        .begin_precommit(damage_question(20.0))
        .expect("a registered boundary queues the question");
    assert!(pending.deadline() > Instant::now());
    let request = take_request(&mut endpoint);
    // What the host is told is left of the deadline is exactly the budget it may
    // spend, and it is inside the conservative bound the request was admitted
    // under: the queue wait and the whole chain share one absolute deadline,
    // rather than each handler arming one of its own.
    assert!(request.remaining() <= precommit::PRECOMMIT_DEADLINE);
    assert!(request.remaining() > Duration::ZERO);
    let HookContext::Damage(context) = request.context() else {
        panic!("a damage question arrives as a damage question");
    };
    assert_eq!(context.amount(), 20.0, "the raw request reaches the chain");
    assert_eq!(context.kind(), "minecraft:generic");
    assert_eq!(context.source(), &HookActor::Environment);
    request
        .answer(HookDecision::Keep)
        .expect("a live requester");

    let mut approval = pending.resolve().await.expect("the host answered");
    assert_eq!(approval.consume(), Ok(HookDecision::Keep));
}
