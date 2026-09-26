use super::*;
use mc_script::{
    ScriptClientViewAction, ScriptClientViewField, ScriptClientViewMarker, ScriptClientViewOpen,
    ScriptClientViewPresent, ScriptClientViewRow,
};

const OWNER: &str = "example";
const PLAYER: u64 = 7;
const VIEW: &str = "example:showcase";
const ACTION: &str = "example:place";

fn field_number(id: &str, value: f64) -> ScriptClientViewField {
    ScriptClientViewField::try_number(id, value).unwrap()
}

fn model(token: Option<&str>) -> ScriptClientViewModel {
    let marker = ScriptClientViewMarker::try_new(
        "anchor",
        token.map(str::to_owned),
        token.map(|_| ACTION.to_owned()),
        None,
        Some(4.0),
    )
    .unwrap();
    ScriptClientViewModel::try_new(
        0,
        1,
        vec![ScriptClientViewRow::try_new(vec!["Hamlet".to_owned()]).unwrap()],
        vec![
            field_number("count", 4.0),
            ScriptClientViewField::try_text("note", "hi".to_owned()).unwrap(),
            ScriptClientViewField::try_selected("blueprint", "house").unwrap(),
            field_number("x", 10.0),
            field_number("y", 64.0),
            field_number("z", -3.0),
        ],
        vec![
            ScriptClientViewAction::try_new(ACTION, true, None, None).unwrap(),
            ScriptClientViewAction::try_new("example:inspect", false, None, Some("no".to_owned()))
                .unwrap(),
        ],
        Vec::new(),
        Vec::new(),
        vec![marker],
        None,
    )
    .unwrap()
}

fn open_request(token: Option<&str>) -> ScriptClientViewOpen {
    ScriptClientViewOpen::try_new(
        "open-1",
        mc_script::ScriptPlayerId::new(PLAYER),
        VIEW,
        model(token),
    )
    .unwrap()
}

fn present_request(
    instance_id: &str,
    expected_revision: u64,
    token: Option<&str>,
) -> ScriptClientViewPresent {
    ScriptClientViewPresent::try_new(
        mc_script::ScriptPlayerId::new(PLAYER),
        instance_id,
        expected_revision,
        model(token),
    )
    .unwrap()
}

fn action(instance_id: &str, revision: u64, token: Option<&str>) -> IngressAction {
    IngressAction {
        view_instance_id: instance_id.to_owned(),
        view_revision: revision,
        action_id: ACTION.to_owned(),
        action_sequence: 1,
        fields: vec![
            field_number("count", 4.0),
            field_number("x", 10.0),
            field_number("y", 64.0),
            field_number("z", -3.0),
        ],
        selection_token: token.map(str::to_owned),
    }
}

fn constraints() -> ScriptClientSelectionConstraints {
    ScriptClientSelectionConstraints::try_new("minecraft:overworld", 64, 600, None, Some(4.0))
        .unwrap()
}

#[test]
fn present_is_a_cas_replacement_and_close_refuses_further_delivery() {
    let mut registry = LoaderViewRegistry::new();
    let opened = registry.open(OWNER, &open_request(None)).unwrap();
    assert_eq!(opened.revision, 1);

    // A stale expected revision never replaces the live model.
    assert_eq!(
        registry.present(OWNER, &present_request(&opened.instance_id, 99, None), 0),
        Err(LoaderViewError::StaleRevision)
    );
    assert_eq!(
        registry.present(OWNER, &present_request("solaris:view-999", 1, None), 0),
        Err(LoaderViewError::UnknownInstance)
    );
    assert_eq!(
        registry.present("other", &present_request(&opened.instance_id, 1, None), 0),
        Err(LoaderViewError::ForeignOwner)
    );

    let revision = registry
        .present(OWNER, &present_request(&opened.instance_id, 1, None), 0)
        .unwrap();
    assert_eq!(revision, 2);

    // An action captured before the replacement is refused.
    assert_eq!(
        registry.handle_action(PLAYER, &action(&opened.instance_id, 1, None), true, 0),
        Err(LoaderViewError::StaleRevision)
    );
    assert!(matches!(
        registry.handle_action(PLAYER, &action(&opened.instance_id, 2, None), true, 0),
        Ok(ActionOutcome { deliver: true, .. })
    ));

    registry
        .close(Some(OWNER), PLAYER, &opened.instance_id)
        .unwrap();
    assert_eq!(
        registry.handle_action(PLAYER, &action(&opened.instance_id, 2, None), true, 0),
        Err(LoaderViewError::ClosedInstance)
    );
    assert_eq!(
        registry.present(OWNER, &present_request(&opened.instance_id, 2, None), 0),
        Err(LoaderViewError::ClosedInstance)
    );
}

#[test]
fn non_selection_actions_deliver_once_per_sequence_across_model_replacements() {
    let mut registry = LoaderViewRegistry::new();
    let opened = registry.open(OWNER, &open_request(None)).unwrap();
    let first = action(&opened.instance_id, 1, None);
    assert!(
        registry
            .handle_action(PLAYER, &first, true, 0)
            .unwrap()
            .deliver
    );
    assert!(
        !registry
            .handle_action(PLAYER, &first, true, 0)
            .unwrap()
            .deliver
    );
    let mut next = first.clone();
    next.action_sequence = 2;
    assert!(
        registry
            .handle_action(PLAYER, &next, true, 0)
            .unwrap()
            .deliver
    );
    assert!(
        !registry
            .handle_action(PLAYER, &first, true, 0)
            .unwrap()
            .deliver
    );
    registry
        .present(OWNER, &present_request(&opened.instance_id, 1, None), 0)
        .unwrap();
    assert_eq!(
        registry.handle_action(PLAYER, &next, true, 0),
        Err(LoaderViewError::StaleRevision)
    );
    let mut refreshed = first.clone();
    refreshed.view_revision = 2;
    assert!(
        registry
            .handle_action(PLAYER, &refreshed, true, 0)
            .unwrap()
            .deliver
    );
    assert!(
        !registry
            .handle_action(PLAYER, &refreshed, true, 0)
            .unwrap()
            .deliver
    );
    refreshed.action_sequence = 2;
    assert!(
        registry
            .handle_action(PLAYER, &refreshed, true, 0)
            .unwrap()
            .deliver
    );
}

#[test]
fn substituted_fields_and_disabled_actions_are_refused() {
    let mut registry = LoaderViewRegistry::new();
    let opened = registry.open(OWNER, &open_request(None)).unwrap();
    let mut substituted = action(&opened.instance_id, 1, None);
    // A client-minted price or actor is never part of the presented field schema.
    substituted.fields = vec![field_number("price", 1.0)];
    assert_eq!(
        registry.handle_action(PLAYER, &substituted, true, 0),
        Err(LoaderViewError::SubstitutedField)
    );
    substituted.fields = vec![ScriptClientViewField::try_text("count", "4".to_owned()).unwrap()];
    assert_eq!(
        registry.handle_action(PLAYER, &substituted, true, 0),
        Err(LoaderViewError::SubstitutedField)
    );
    let mut disabled = action(&opened.instance_id, 1, None);
    disabled.action_id = "example:inspect".to_owned();
    assert_eq!(
        registry.handle_action(PLAYER, &disabled, true, 0),
        Err(LoaderViewError::DisabledAction)
    );
    let mut unknown = action(&opened.instance_id, 1, None);
    unknown.action_id = "example:not-declared".to_owned();
    assert_eq!(
        registry.handle_action(PLAYER, &unknown, true, 0),
        Err(LoaderViewError::UnknownAction)
    );
    // Rights are re-read per action: a revoked permission blocks the old action.
    assert_eq!(
        registry.handle_action(PLAYER, &action(&opened.instance_id, 1, None), false, 0),
        Err(LoaderViewError::PermissionRevoked)
    );
}

#[test]
fn selection_contexts_are_single_use_expiring_and_revocable() {
    let mut registry = LoaderViewRegistry::new();
    let opened = registry.open(OWNER, &open_request(None)).unwrap();
    let started = registry
        .begin_selection(
            OWNER,
            PLAYER,
            &opened.instance_id,
            1,
            ACTION,
            &constraints(),
            100,
        )
        .unwrap();
    assert_eq!(started.expires_at_tick, 700);

    // The context only reaches the client through a marker of the presented model.
    let revision = registry
        .present(
            OWNER,
            &present_request(&opened.instance_id, 1, Some(&started.context_id)),
            100,
        )
        .unwrap();
    assert_eq!(revision, 2);

    let first = registry
        .handle_action(
            PLAYER,
            &action(&opened.instance_id, 2, Some(&started.context_id)),
            true,
            100,
        )
        .unwrap();
    assert!(first.deliver);
    let admission = first.admission.unwrap();
    assert_eq!(admission.context_id, started.context_id);
    assert_eq!(admission.target, Some([10.0, 64.0, -3.0]));

    // Re-sending a consumed context returns the same admission, no new effect.
    let replay = registry
        .handle_action(
            PLAYER,
            &action(&opened.instance_id, 2, Some(&started.context_id)),
            true,
            100,
        )
        .unwrap();
    assert!(!replay.deliver);
    assert_eq!(replay.admission, Some(admission));

    // A replacement without the token invalidates the context.
    let revision = registry
        .present(OWNER, &present_request(&opened.instance_id, 2, None), 100)
        .unwrap();
    assert_eq!(revision, 3);
    assert_eq!(
        registry.handle_action(
            PLAYER,
            &action(&opened.instance_id, 3, Some(&started.context_id)),
            true,
            100
        ),
        Err(LoaderViewError::SelectionRequired)
    );

    // Expiry is measured in simulation ticks.
    let expiring = registry
        .begin_selection(
            OWNER,
            PLAYER,
            &opened.instance_id,
            3,
            ACTION,
            &constraints(),
            100,
        )
        .unwrap();
    registry
        .present(
            OWNER,
            &present_request(&opened.instance_id, 3, Some(&expiring.context_id)),
            100,
        )
        .unwrap();
    assert_eq!(
        registry.handle_action(
            PLAYER,
            &action(&opened.instance_id, 4, Some(&expiring.context_id)),
            true,
            1_000
        ),
        Err(LoaderViewError::SelectionExpired)
    );

    // Revoking the owner drops its views and contexts entirely.
    registry.revoke_owner(OWNER);
    assert!(registry.instance(&opened.instance_id).is_none());
}

#[test]
fn close_and_disconnect_invalidate_armed_contexts() {
    let mut registry = LoaderViewRegistry::new();
    let opened = registry.open(OWNER, &open_request(None)).unwrap();
    let started = registry
        .begin_selection(
            OWNER,
            PLAYER,
            &opened.instance_id,
            1,
            ACTION,
            &constraints(),
            0,
        )
        .unwrap();
    registry
        .present(
            OWNER,
            &present_request(&opened.instance_id, 1, Some(&started.context_id)),
            0,
        )
        .unwrap();
    registry
        .close(Some(OWNER), PLAYER, &opened.instance_id)
        .unwrap();
    assert_eq!(
        registry.handle_action(
            PLAYER,
            &action(&opened.instance_id, 2, Some(&started.context_id)),
            true,
            0
        ),
        Err(LoaderViewError::ClosedInstance)
    );

    let reopened = registry.open(OWNER, &open_request(None)).unwrap();
    let started = registry
        .begin_selection(
            OWNER,
            PLAYER,
            &reopened.instance_id,
            reopened.revision,
            ACTION,
            &constraints(),
            0,
        )
        .unwrap();
    registry
        .present(
            OWNER,
            &present_request(
                &reopened.instance_id,
                reopened.revision,
                Some(&started.context_id),
            ),
            0,
        )
        .unwrap();
    registry.disconnect(PLAYER);
    assert!(registry.instance(&reopened.instance_id).is_none());
    assert_eq!(
        registry.cancel_selection(PLAYER, &started.context_id),
        Err(LoaderViewError::UnknownInstance)
    );
}

#[test]
fn selection_requires_a_live_context_and_foreign_clients_are_refused() {
    let mut registry = LoaderViewRegistry::new();
    let opened = registry.open(OWNER, &open_request(None)).unwrap();
    // No marker token is armed, so no world point is required for this action.
    assert!(matches!(
        registry.handle_action(PLAYER, &action(&opened.instance_id, 1, None), true, 0),
        Ok(ActionOutcome { deliver: true, .. })
    ));
    // Another player may not act on this session's instance.
    assert_eq!(
        registry.handle_action(9, &action(&opened.instance_id, 1, None), true, 0),
        Err(LoaderViewError::ForeignOwner)
    );
    assert_eq!(
        registry.cancel_selection(PLAYER, "solaris:selection-404"),
        Err(LoaderViewError::UnknownInstance)
    );
}
