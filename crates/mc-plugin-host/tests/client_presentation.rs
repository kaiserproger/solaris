//! The P4 Loader presentation vertical on the host side: the typed commands a
//! guest returns.
//!
//! Every case here pins one property of the migration rather than the plumbing
//! under it:
//!
//! * a guest's view request becomes the exact DTO the server's Loader owner
//!   applies - the session it named, the instance and revision it compared
//!   against and the model it built, with nothing invented in between and no
//!   capability needed, because the Loader manifest's bundled content and
//!   permission are the gate and the owner is what checks them;
//! * a request past a bound the contract declares is refused, never clamped,
//!   truncated or defaulted - a page the model cannot have, a row list past 64,
//!   a marker token without the action it belongs to, an amount outside the audio
//!   range and a block that is not a resource id all fail the whole batch as the
//!   plugin's own malformed answer;
//! * every nested string of a model counts against the configured text bound, so
//!   no string of a new nested record is one the bound never looked at.
//!
//! The other direction - what the Loader owners' own decisions look like once
//! they reach the guest - is covered by the module's own boundary tests
//! (`client_presentation_tests.rs`), because that mapping is the host's internal
//! fallback rather than a public surface.

use std::num::NonZeroUsize;

use mc_plugin_host::bindings::solaris::plugin::client_presentation::{
    ClientCommand, CloseClientView, GrantLoaderBlockItem, OpenClientView, PlayClientSound,
    PresentClientView, StopClientSound, ViewAction, ViewField, ViewFieldValue, ViewFormation,
    ViewMarker, ViewModel, ViewResourceEntry, ViewRow, ViewTab,
};
use mc_plugin_host::bindings::solaris::plugin::commands::{Command, MessageTarget, SendMessage};
use mc_plugin_host::bindings::solaris::plugin::types::Position;
use mc_plugin_host::{
    AdapterError, CommandBatch, NoSessions, PluginLimits, StagingError, to_script_batch,
};
use mc_script::{
    CommandCapabilities, ScriptClientSound, ScriptClientViewAction, ScriptClientViewField,
    ScriptClientViewFormation, ScriptClientViewMarker, ScriptClientViewModel, ScriptClientViewOpen,
    ScriptClientViewPresent, ScriptClientViewResourceEntry, ScriptClientViewRow,
    ScriptClientViewTab, ScriptCommand, ScriptLoaderItemGrantRequest, ScriptPlayerId,
    ScriptPosition,
};

/// The one connection every case names, and the revision it presents against.
const SESSION: u64 = 7;
const VIEW_INSTANCE: &str = "ruby-live:instance";
const REVISION: u64 = 3;

/// The grants a package that declares nothing holds.
///
/// The Loader commands need no capability, so this is the batch every real
/// package's callback produces: the manifest's Loader content and permission
/// pair is what admits the request, inside the owner that applies it.
fn no_grants() -> CommandCapabilities {
    CommandCapabilities::none()
}

/// Stage one contract command, the way a guest's callback stages its own.
fn stage(command: Command) -> CommandBatch {
    let mut batch = CommandBatch::new();
    batch
        .push(command, &PluginLimits::default())
        .expect("one command fits the staging bound");
    batch
}

/// Convert one staged batch against the package's own (empty) grants, and answer
/// the server's own batch or the refusal.
fn convert(batch: CommandBatch) -> Result<mc_script::CommandBatch, AdapterError> {
    to_script_batch(
        batch,
        NonZeroUsize::new(4).expect("non-zero"),
        // A Loader command names a session and needs no lookup, so a resolver
        // that knows nobody is enough: if the conversion had grown one, this
        // would refuse a batch a live server accepts.
        &NoSessions,
        &no_grants(),
    )
}

/// One page that exercises every part of the model the contract carries.
fn showcase_model() -> ViewModel {
    ViewModel {
        page: 0,
        page_count: 2,
        rows: vec![ViewRow {
            cells: vec!["Ruby Loader Fixture".to_owned(), "row two".to_owned()],
        }],
        fields: vec![
            ViewField {
                id: "amount".to_owned(),
                value: ViewFieldValue::Number(12.5),
            },
            ViewField {
                id: "notes".to_owned(),
                value: ViewFieldValue::Text("hello".to_owned()),
            },
            ViewField {
                id: "choice".to_owned(),
                value: ViewFieldValue::Selected("ruby".to_owned()),
            },
        ],
        actions: vec![ViewAction {
            action_id: "ruby-live:confirm".to_owned(),
            enabled: true,
            label: Some("Confirm Ruby".to_owned()),
            deny_reason: None,
        }],
        tabs: vec![ViewTab {
            id: "tab".to_owned(),
            label: "Overview".to_owned(),
        }],
        resource_entries: vec![ViewResourceEntry {
            id: "rubies".to_owned(),
            have: 4.0,
            need: 9.0,
        }],
        markers: vec![ViewMarker {
            marker_id: "anchor".to_owned(),
            selection_token: Some("ctx-1".to_owned()),
            action_id: Some("ruby-live:confirm".to_owned()),
            formation: Some(ViewFormation::Wedge),
            radius: Some(16.0),
        }],
        reason: Some("choose where".to_owned()),
    }
}

/// The same page as the server's own DTO, built by the same constructors the
/// adapter must use.
fn showcase_dto() -> ScriptClientViewModel {
    ScriptClientViewModel::try_new(
        0,
        2,
        vec![
            ScriptClientViewRow::try_new(vec![
                "Ruby Loader Fixture".to_owned(),
                "row two".to_owned(),
            ])
            .expect("two bounded cells"),
        ],
        vec![
            ScriptClientViewField::try_number("amount", 12.5).expect("a finite amount"),
            ScriptClientViewField::try_text("notes", "hello".to_owned()).expect("bounded text"),
            ScriptClientViewField::try_selected("choice", "ruby").expect("a bounded selection"),
        ],
        vec![
            ScriptClientViewAction::try_new(
                "ruby-live:confirm",
                true,
                Some("Confirm Ruby".to_owned()),
                None,
            )
            .expect("a bounded action"),
        ],
        vec![ScriptClientViewTab::try_new("tab", "Overview").expect("a bounded tab")],
        vec![
            ScriptClientViewResourceEntry::try_new("rubies", 4.0, 9.0)
                .expect("finite resource amounts"),
        ],
        vec![
            ScriptClientViewMarker::try_new(
                "anchor",
                Some("ctx-1".to_owned()),
                Some("ruby-live:confirm".to_owned()),
                Some(ScriptClientViewFormation::Wedge),
                Some(16.0),
            )
            .expect("a bounded marker"),
        ],
        Some("choose where".to_owned()),
    )
    .expect("the fixture page is inside every bound")
}

/// The smallest page the contract admits.
fn empty_page() -> ViewModel {
    ViewModel {
        page: 0,
        page_count: 1,
        rows: Vec::new(),
        fields: Vec::new(),
        actions: Vec::new(),
        tabs: Vec::new(),
        resource_entries: Vec::new(),
        markers: Vec::new(),
        reason: None,
    }
}

fn open_view(model: ViewModel) -> Command {
    Command::ClientPresentation(ClientCommand::OpenClientView(OpenClientView {
        request: "open-1".to_owned(),
        session: SESSION,
        owned_view_id: "ruby-live:showcase".to_owned(),
        model,
    }))
}

#[test]
fn an_opened_view_reaches_the_owner_with_the_guests_own_session_and_model() {
    let converted = convert(stage(open_view(showcase_model()))).expect("the view converts");
    assert_eq!(
        converted.commands(),
        [ScriptCommand::OpenClientView {
            request: ScriptClientViewOpen::try_new(
                "open-1",
                ScriptPlayerId::new(SESSION),
                "ruby-live:showcase",
                showcase_dto(),
            )
            .expect("the same request validates natively"),
        }],
        "the session, the owned view id and every part of the model are the guest's own"
    );
}

#[test]
fn a_present_compares_against_the_guests_own_revision() {
    let present =
        Command::ClientPresentation(ClientCommand::PresentClientView(PresentClientView {
            session: SESSION,
            view_instance_id: VIEW_INSTANCE.to_owned(),
            expected_revision: REVISION,
            model: showcase_model(),
        }));
    let converted = convert(stage(present)).expect("the replacement converts");
    assert_eq!(
        converted.commands(),
        [ScriptCommand::PresentClientView {
            request: ScriptClientViewPresent::try_new(
                ScriptPlayerId::new(SESSION),
                VIEW_INSTANCE,
                REVISION,
                showcase_dto(),
            )
            .expect("the same replacement validates natively"),
        }],
        "the compare-and-swap revision is the guest's own and is not advanced here"
    );
}

#[test]
fn a_close_a_play_a_stop_and_a_grant_each_become_their_own_native_command() {
    let close = Command::ClientPresentation(ClientCommand::CloseClientView(CloseClientView {
        session: SESSION,
        view_instance_id: VIEW_INSTANCE.to_owned(),
    }));
    let converted = convert(stage(close)).expect("the close converts");
    assert_eq!(
        converted.commands(),
        [ScriptCommand::CloseClientView {
            player_id: ScriptPlayerId::new(SESSION),
            view_instance_id: VIEW_INSTANCE.to_owned(),
        }],
    );

    let play = Command::ClientPresentation(ClientCommand::PlayClientSound(PlayClientSound {
        session: SESSION,
        sound_id: "ruby-live:chime".to_owned(),
        volume: 0.25,
        pitch: 1.5,
        position: Some(Position {
            x: 1.0,
            y: 64.0,
            z: -2.0,
        }),
    }));
    let converted = convert(stage(play)).expect("the play converts");
    assert_eq!(
        converted.commands(),
        [ScriptCommand::ClientSound {
            player_id: ScriptPlayerId::new(SESSION),
            sound: ScriptClientSound::play(
                "ruby-live:chime",
                0.25,
                1.5,
                Some(ScriptPosition::try_new(1.0, 64.0, -2.0).expect("a bounded position")),
            )
            .expect("the same playback validates natively"),
        }],
    );

    let stop = Command::ClientPresentation(ClientCommand::StopClientSound(StopClientSound {
        session: SESSION,
        sound_id: "ruby-live:chime".to_owned(),
    }));
    let converted = convert(stage(stop)).expect("the stop converts");
    assert_eq!(
        converted.commands(),
        [ScriptCommand::ClientSound {
            player_id: ScriptPlayerId::new(SESSION),
            sound: ScriptClientSound::stop("ruby-live:chime")
                .expect("the same stop validates natively"),
        }],
        "a stop carries no playback, so it cannot carry a volume or a pitch"
    );

    let grant =
        Command::ClientPresentation(ClientCommand::GrantLoaderBlockItem(GrantLoaderBlockItem {
            request: "grant-1".to_owned(),
            session: SESSION,
            block: "ruby-live:ruby_block".to_owned(),
            count: 3,
        }));
    let converted = convert(stage(grant)).expect("the grant converts");
    assert_eq!(
        converted.commands(),
        [ScriptCommand::GrantLoaderBlockItem {
            request: ScriptLoaderItemGrantRequest::try_new(
                "grant-1",
                ScriptPlayerId::new(SESSION),
                "ruby-live:ruby_block",
                3,
            )
            .expect("the same grant validates natively"),
        }],
        "the correlation id, the session, the block and the count are the guest's own"
    );
}

#[test]
fn a_grant_for_a_block_that_is_not_a_resource_id_is_refused_by_name() {
    let grant =
        Command::ClientPresentation(ClientCommand::GrantLoaderBlockItem(GrantLoaderBlockItem {
            request: "grant-1".to_owned(),
            session: SESSION,
            block: "ruby_block".to_owned(),
            count: 1,
        }));
    assert_eq!(
        convert(stage(grant)),
        Err(AdapterError::InvalidCommand {
            field: "resource id"
        }),
        "the refusal names the native value the plugin got wrong"
    );
}

#[test]
fn a_playback_outside_the_audio_range_is_refused_before_anything_is_applied() {
    let play = Command::ClientPresentation(ClientCommand::PlayClientSound(PlayClientSound {
        session: SESSION,
        sound_id: "ruby-live:chime".to_owned(),
        volume: 1.5,
        pitch: 1.0,
        position: None,
    }));
    // A batch that also carries a valid command: the malformed request fails the
    // whole answer, so the valid one is not applied on its own either.
    let mut batch = CommandBatch::new();
    batch
        .push(
            Command::SendMessage(SendMessage {
                target: MessageTarget::Session(SESSION),
                text: "still here".to_owned(),
            }),
            &PluginLimits::default(),
        )
        .expect("the chat line fits the staging bound");
    batch
        .push(play, &PluginLimits::default())
        .expect("the play fits the staging bound");
    assert_eq!(
        convert(batch),
        Err(AdapterError::InvalidCommand { field: "command" }),
        "a volume past 1.0 is the plugin's own malformed answer, not a clamped one"
    );
}

/// One model per nested string site, each carrying `text` at exactly that site
/// and short values everywhere else.
fn models_with_a_value_at_every_site(text: &str) -> Vec<(&'static str, ViewModel)> {
    let page = || ViewModel {
        page: 0,
        page_count: 1,
        rows: Vec::new(),
        fields: Vec::new(),
        actions: Vec::new(),
        tabs: Vec::new(),
        resource_entries: Vec::new(),
        markers: Vec::new(),
        reason: None,
    };
    vec![
        (
            "a row cell",
            ViewModel {
                rows: vec![ViewRow {
                    cells: vec![text.to_owned()],
                }],
                ..page()
            },
        ),
        (
            "a text field",
            ViewModel {
                fields: vec![ViewField {
                    id: "notes".to_owned(),
                    value: ViewFieldValue::Text(text.to_owned()),
                }],
                ..page()
            },
        ),
        (
            "a selected field",
            ViewModel {
                fields: vec![ViewField {
                    id: "choice".to_owned(),
                    value: ViewFieldValue::Selected(text.to_owned()),
                }],
                ..page()
            },
        ),
        (
            "an action label",
            ViewModel {
                actions: vec![ViewAction {
                    action_id: "act".to_owned(),
                    enabled: true,
                    label: Some(text.to_owned()),
                    deny_reason: None,
                }],
                ..page()
            },
        ),
        (
            "an action deny reason",
            ViewModel {
                actions: vec![ViewAction {
                    action_id: "act".to_owned(),
                    enabled: false,
                    label: None,
                    deny_reason: Some(text.to_owned()),
                }],
                ..page()
            },
        ),
        (
            "a tab label",
            ViewModel {
                tabs: vec![ViewTab {
                    id: "tab".to_owned(),
                    label: text.to_owned(),
                }],
                ..page()
            },
        ),
        (
            "a resource id",
            ViewModel {
                resource_entries: vec![ViewResourceEntry {
                    id: text.to_owned(),
                    have: 1.0,
                    need: 2.0,
                }],
                ..page()
            },
        ),
        (
            "a marker token",
            ViewModel {
                markers: vec![ViewMarker {
                    marker_id: "anchor".to_owned(),
                    selection_token: Some(text.to_owned()),
                    action_id: Some("act".to_owned()),
                    formation: None,
                    radius: None,
                }],
                ..page()
            },
        ),
        (
            "the model reason",
            ViewModel {
                reason: Some(text.to_owned()),
                ..page()
            },
        ),
    ]
}

#[test]
fn every_nested_model_string_obeys_the_configured_text_bound() {
    // 64 bytes is inside every native bound of a model, so the refusal below is
    // the configured text bound and nothing else.
    let value = "v".repeat(64);
    for (site, model) in models_with_a_value_at_every_site(&value) {
        let mut refused = CommandBatch::new();
        assert_eq!(
            refused.push(
                open_view(model.clone()),
                &PluginLimits {
                    text_bytes: 32,
                    ..PluginLimits::default()
                },
            ),
            Err(StagingError::TextTooLong),
            "{site} must count against the configured text bound"
        );
        assert!(
            refused.is_empty(),
            "{site}: an oversized answer must not be staged at all"
        );

        let mut accepted = CommandBatch::new();
        accepted
            .push(
                open_view(model),
                &PluginLimits {
                    text_bytes: 64,
                    ..PluginLimits::default()
                },
            )
            .expect("a value at the exact bound is staged");
        convert(accepted).expect("and the native DTO accepts it at the same bound");
    }
}

#[test]
fn an_owned_view_id_obeys_the_same_text_bound() {
    let owned_view_id = format!("p4:{}", "a".repeat(61));
    assert_eq!(owned_view_id.len(), 64, "a valid resource id at the bound");
    let view = || {
        Command::ClientPresentation(ClientCommand::OpenClientView(OpenClientView {
            request: "open-1".to_owned(),
            session: SESSION,
            owned_view_id: owned_view_id.clone(),
            model: empty_page(),
        }))
    };

    let mut refused = CommandBatch::new();
    assert_eq!(
        refused.push(
            view(),
            &PluginLimits {
                text_bytes: 32,
                ..PluginLimits::default()
            },
        ),
        Err(StagingError::TextTooLong)
    );
    let mut accepted = CommandBatch::new();
    accepted
        .push(
            view(),
            &PluginLimits {
                text_bytes: 64,
                ..PluginLimits::default()
            },
        )
        .expect("the exact bound is accepted");
    convert(accepted).expect("and the native DTO accepts the same id");
}

#[test]
fn a_model_past_a_native_bound_is_refused_and_the_whole_batch_falls_with_it() {
    // A page the model cannot have.
    assert_eq!(
        convert(stage(open_view(ViewModel {
            page: 1,
            page_count: 1,
            ..empty_page()
        }))),
        Err(AdapterError::InvalidCommand { field: "command" }),
    );

    // One row past the 64 the contract admits.
    let rows = (0..65)
        .map(|index| ViewRow {
            cells: vec![index.to_string()],
        })
        .collect();
    assert_eq!(
        convert(stage(open_view(ViewModel {
            rows,
            ..empty_page()
        }))),
        Err(AdapterError::InvalidCommand { field: "view rows" }),
    );

    // Two fields with one id.
    assert_eq!(
        convert(stage(open_view(ViewModel {
            fields: vec![
                ViewField {
                    id: "amount".to_owned(),
                    value: ViewFieldValue::Number(1.0),
                },
                ViewField {
                    id: "amount".to_owned(),
                    value: ViewFieldValue::Number(2.0),
                },
            ],
            ..empty_page()
        }))),
        Err(AdapterError::InvalidCommand {
            field: "view field"
        }),
    );

    // A marker that carries a selection token without the action it belongs to.
    assert_eq!(
        convert(stage(open_view(ViewModel {
            markers: vec![ViewMarker {
                marker_id: "anchor".to_owned(),
                selection_token: Some("ctx-1".to_owned()),
                action_id: None,
                formation: None,
                radius: None,
            }],
            ..empty_page()
        }))),
        Err(AdapterError::InvalidCommand {
            field: "view marker action"
        }),
        "the refusal names the field the plugin got wrong, and nothing is clamped"
    );
}
