//! Boundary tests of the Loader presentation event mapping.
//!
//! The mapping is a translation, not a copy, and these cases pin the decisions
//! it makes rather than the fields it passes through: which connection an event
//! is reported for, which of the two shapes one open ended in, which typed value
//! a field carries, and which observations are dropped because this contract
//! carries no member for them. A field this module copies unchanged needs no
//! assertion here - a test that only restates a clone would defend the source
//! text, not a behavior.

use super::map_event;
use crate::bindings::exports::solaris::plugin::events::{
    ClientSelectionOutcome, ClientViewOutcome, Event, LoaderItemGrantFailure,
    LoaderItemGrantOutcome, ViewFailure, ViewRequestKind,
};
use crate::bindings::solaris::plugin::client_presentation::ViewFieldValue;
use mc_script::{
    ScriptClientViewFailure, ScriptClientViewField, ScriptClientViewRequestKind, ScriptEventKind,
    ScriptLoaderItemGrantFailure, ScriptPlayerId,
};

/// The one connection every case reports on.
const SESSION: u64 = 9;

#[test]
fn a_view_action_carries_the_events_own_session_and_each_typed_value_shape() {
    let kind = ScriptEventKind::LoaderViewAction {
        player_id: ScriptPlayerId::new(SESSION),
        view_instance_id: "ruby-live:instance".to_owned(),
        view_revision: 3,
        action_id: "ruby-live:confirm".to_owned(),
        action_sequence: 5,
        fields: vec![
            ScriptClientViewField::try_number("amount", 2.5).expect("a finite amount"),
            ScriptClientViewField::try_text("notes", "hello".to_owned()).expect("bounded text"),
            ScriptClientViewField::try_selected("choice", "ruby").expect("a bounded selection"),
        ],
        selection_token: Some("ctx-1".to_owned()),
    };
    let Some(Event::LoaderViewAction(action)) = map_event(&kind) else {
        panic!("the contract carries an admitted view action");
    };
    assert_eq!(
        action.session, SESSION,
        "the session is the runtime player id of the event itself, never a lookup"
    );
    assert_eq!(
        action.fields.len(),
        3,
        "no typed field of the action is dropped"
    );
    let ViewFieldValue::Number(number) = &action.fields[0].value else {
        panic!("a number field stays a number");
    };
    assert_eq!(*number, 2.5);
    let ViewFieldValue::Text(text) = &action.fields[1].value else {
        panic!("a text field stays text");
    };
    assert_eq!(text, "hello");
    let ViewFieldValue::Selected(selected) = &action.fields[2].value else {
        panic!("a selected field stays a selection");
    };
    assert_eq!(selected, "ruby");
}

#[test]
fn an_open_result_is_one_of_the_two_shapes_the_owner_decided_or_nothing() {
    let opened = ScriptEventKind::ClientViewOpened {
        request_id: "open-1".to_owned(),
        player_id: ScriptPlayerId::new(SESSION),
        view_instance_id: Some("ruby-live:instance".to_owned()),
        revision: Some(3),
        failure: None,
    };
    let Some(Event::ClientViewOpened(answer)) = map_event(&opened) else {
        panic!("an acknowledged open is carried");
    };
    assert_eq!(answer.session, SESSION);
    let ClientViewOutcome::Opened(instance) = answer.outcome else {
        panic!("an instance and a revision mean the open succeeded");
    };
    assert_eq!(
        (instance.view_instance_id.as_str(), instance.revision),
        ("ruby-live:instance", 3),
        "the revision is the compare-and-swap value a later present uses"
    );

    let refused = ScriptEventKind::ClientViewOpened {
        request_id: "open-2".to_owned(),
        player_id: ScriptPlayerId::new(SESSION),
        view_instance_id: None,
        revision: None,
        failure: Some(ScriptClientViewFailure::PlayerUnavailable),
    };
    let Some(Event::ClientViewOpened(answer)) = map_event(&refused) else {
        panic!("a refused open is carried");
    };
    let ClientViewOutcome::Refused(failure) = answer.outcome else {
        panic!("a refusal carries no instance");
    };
    assert!(
        matches!(failure, ViewFailure::PlayerUnavailable),
        "the owner's own reason is carried through"
    );

    // An instance without the revision it was created at, and neither shape at
    // all: the owner decided neither, so the event is dropped rather than
    // reported as an instance or as a refusal.
    for (view_instance_id, revision, failure) in [
        (Some("ruby-live:instance".to_owned()), None, None),
        (None, None, None),
    ] {
        let undecided = ScriptEventKind::ClientViewOpened {
            request_id: "open-3".to_owned(),
            player_id: ScriptPlayerId::new(SESSION),
            view_instance_id,
            revision,
            failure,
        };
        assert!(map_event(&undecided).is_none());
    }
}

#[test]
fn a_grant_result_carries_the_authoritys_own_outcome() {
    let decided = |failure| ScriptEventKind::LoaderItemGrantResult {
        request_id: "grant-1".to_owned(),
        player_id: ScriptPlayerId::new(SESSION),
        block_id: "ruby-live:ruby_block".to_owned(),
        count: 1,
        failure,
    };

    let Some(Event::LoaderItemGrantResult(answer)) = map_event(&decided(None)) else {
        panic!("a decided grant is carried");
    };
    assert!(
        matches!(answer.outcome, LoaderItemGrantOutcome::Granted),
        "no failure means the authority committed the items"
    );

    let refused = decided(Some(ScriptLoaderItemGrantFailure::InventoryFull));
    let Some(Event::LoaderItemGrantResult(answer)) = map_event(&refused) else {
        panic!("a refused grant is carried");
    };
    assert!(
        matches!(
            answer.outcome,
            LoaderItemGrantOutcome::Refused(LoaderItemGrantFailure::InventoryFull)
        ),
        "the authority's own reason is carried through"
    );
}

#[test]
fn a_routed_view_request_carries_the_events_own_session_and_the_declared_kind() {
    let kind = ScriptEventKind::LoaderViewRequest {
        player_id: ScriptPlayerId::new(SESSION),
        request_kind: ScriptClientViewRequestKind::Settlement,
    };
    let Some(Event::LoaderViewRequest(request)) = map_event(&kind) else {
        panic!("a routed view request is carried");
    };
    assert_eq!(request.session, SESSION);
    assert!(
        matches!(request.request_kind, ViewRequestKind::Settlement),
        "the declared kind is the owner's own"
    );
}

#[test]
fn selection_start_exposes_only_the_owners_decided_context_or_refusal() {
    let started = ScriptEventKind::ClientSelectionStarted {
        request_id: "select-1".to_owned(),
        player_id: ScriptPlayerId::new(SESSION),
        selection_context_id: Some("solaris:selection-1".to_owned()),
        expires_at_tick: Some(1200),
        failure: None,
    };
    let Some(Event::ClientSelectionStarted(answer)) = map_event(&started) else {
        panic!("an admitted selection is carried");
    };
    let ClientSelectionOutcome::Started(context) = answer.outcome else {
        panic!("an admitted selection has a single-use context");
    };
    assert_eq!(answer.session, SESSION);
    assert_eq!(context.id, "solaris:selection-1");
    assert_eq!(context.expires_at_tick, 1200);

    let refused = ScriptEventKind::ClientSelectionStarted {
        request_id: "select-2".to_owned(),
        player_id: ScriptPlayerId::new(SESSION),
        selection_context_id: None,
        expires_at_tick: None,
        failure: Some(ScriptClientViewFailure::StaleRevision),
    };
    let Some(Event::ClientSelectionStarted(answer)) = map_event(&refused) else {
        panic!("owner refusal is carried");
    };
    assert!(matches!(
        answer.outcome,
        ClientSelectionOutcome::Refused(ViewFailure::StaleRevision)
    ));

    let inconsistent = ScriptEventKind::ClientSelectionStarted {
        request_id: "select-3".to_owned(),
        player_id: ScriptPlayerId::new(SESSION),
        selection_context_id: Some("solaris:selection-2".to_owned()),
        expires_at_tick: None,
        failure: None,
    };
    assert!(map_event(&inconsistent).is_none());
}
