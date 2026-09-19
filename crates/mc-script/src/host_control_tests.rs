//! Boundary coverage for the trusted host's own control input.
//!
//! The runtime that moves a reload envelope across this boundary lives in another
//! crate, so what is pinned here is only the boundary's own contract: control
//! input shares the one event FIFO, an event-only consumer drops it without
//! interpreting it, and a commit either lands routes, the caller's swap and
//! commands in the documented order or leaves the running generation alone.

use std::any::Any;
use std::num::NonZeroUsize;
use std::sync::mpsc as std_mpsc;

use super::*;

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("non-zero queue capacity")
}

fn command_manifest(id: &str, root: &str) -> ValidatedScriptPluginManifest {
    ScriptPluginManifest::new(id, id, "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_player_command_root(root)
        .validate()
        .expect("valid test manifest")
}

fn chat(message: &str) -> ScriptCommand {
    ScriptCommand::SendChatMessage {
        player_id: ScriptPlayerId::new(7),
        message: message.to_owned(),
    }
}

/// The payload shape only a trusted runtime knows. The boundary must move it
/// untouched, so nothing here is part of the contract the boundary checks.
struct ControlEnvelope {
    tag: &'static str,
    response: std_mpsc::Sender<&'static str>,
}

fn envelope(tag: &'static str) -> (Box<dyn Any + Send>, std_mpsc::Receiver<&'static str>) {
    let (response, received) = std_mpsc::channel();
    (Box::new(ControlEnvelope { tag, response }), received)
}

#[tokio::test]
async fn control_input_keeps_the_event_fifo_order() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(8), nonzero(4));
    let control = boundary.host_input_sender();

    boundary
        .try_enqueue_event(ScriptEvent::server_tick(1))
        .unwrap();
    let (reload, response) = envelope("reload");
    control
        .send_reload(reload)
        .await
        .expect("an open host accepts control input");
    boundary
        .try_enqueue_event(ScriptEvent::server_tick(2))
        .unwrap();

    let delivered = tokio::task::spawn_blocking(move || {
        let mut endpoint = endpoint;
        let mut delivered = Vec::new();
        while delivered.len() < 3 {
            let Some(input) = endpoint.recv_input_blocking() else {
                break;
            };
            match input {
                ScriptHostInput::Event(event) => match event.kind() {
                    ScriptEventKind::ServerTick { tick } => delivered.push(format!("tick:{tick}")),
                    other => panic!("unexpected event {other:?}"),
                },
                ScriptHostInput::Reload(payload) => {
                    let envelope = *payload
                        .downcast::<ControlEnvelope>()
                        .expect("control payload arrives as queued");
                    envelope
                        .response
                        .send(envelope.tag)
                        .expect("the queued envelope answers its requester");
                    delivered.push(envelope.tag.to_owned());
                }
                // This test queues events and reloads only: a pre-commit question
                // is admitted by the boundary's own hook API, which it never calls.
                ScriptHostInput::Precommit(_) => {
                    panic!("no pre-commit question is queued in this test")
                }
            }
        }
        delivered
    })
    .await
    .expect("host thread");

    // One mailbox, one order: control input never overtakes the events admitted
    // before it and never delays the ones admitted after it.
    assert_eq!(delivered, vec!["tick:1", "reload", "tick:2"]);
    // The envelope crossed the boundary whole: its own response half answers it.
    assert_eq!(response.try_recv(), Ok("reload"));
}

#[tokio::test]
async fn event_only_consumers_drop_control_input_unread() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(8), nonzero(4));
    let control = boundary.host_input_sender();

    boundary
        .try_enqueue_event(ScriptEvent::server_tick(1))
        .unwrap();
    let (reload, response) = envelope("async");
    control.send_reload(reload).await.unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_tick(2))
        .unwrap();

    assert_eq!(response.try_recv(), Err(std_mpsc::TryRecvError::Empty));
    assert_eq!(
        endpoint.recv_event().await.unwrap().event_name(),
        "server.tick"
    );
    // The envelope behind the first event is skipped and dropped, not answered:
    // the drop is what closes the response half its payload carries.
    assert_eq!(
        endpoint.recv_event().await.unwrap().event_name(),
        "server.tick"
    );
    assert_eq!(
        response.try_recv(),
        Err(std_mpsc::TryRecvError::Disconnected)
    );

    boundary
        .try_enqueue_event(ScriptEvent::server_tick(3))
        .unwrap();
    assert_eq!(
        endpoint.recv_event_blocking().unwrap().event_name(),
        "server.tick"
    );
    let (reload, response) = envelope("blocking");
    control.send_reload(reload).await.unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_tick(4))
        .unwrap();
    assert_eq!(
        endpoint.recv_event_blocking().unwrap().event_name(),
        "server.tick"
    );
    assert_eq!(
        response.try_recv(),
        Err(std_mpsc::TryRecvError::Disconnected)
    );
}

#[tokio::test]
async fn commit_reload_swaps_routes_before_the_caller_swap_and_then_delivers() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let manifest = command_manifest("reloaded", "hello");
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let mut batch = CommandBatch::new(nonzero(4));
    batch.try_push(chat("after-swap")).unwrap();

    let mut routes_during_swap = None;
    endpoint
        .commit_reload(&[manifest], vec![(admission, batch)], || {
            routes_during_swap = Some(boundary.player_command_roots());
        })
        .expect("commit succeeds");

    // The caller's swap observes the committed routes: ownership flips before the
    // generation is published, never after it.
    assert_eq!(routes_during_swap, Some(vec!["hello".to_owned()]));

    let command = boundary.recv_command().await.expect("committed command");
    let ScriptCommand::HostAttached {
        provenance,
        request,
    } = command
    else {
        panic!("expected an admitted command, got {command:?}");
    };
    assert_eq!(provenance.plugin_id(), "reloaded");
    assert_eq!(request.as_ref(), &chat("after-swap"));
}

#[test]
fn refused_commit_leaves_routes_and_the_live_generation_untouched() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    endpoint
        .register_plugin_routes(&command_manifest("live", "hello"))
        .unwrap();

    // A candidate cannot give one root two owners: nothing is swapped, nothing is
    // queued and the live routes stay exactly as they were.
    let clashing = vec![
        command_manifest("alpha", "shared"),
        command_manifest("beta", "shared"),
    ];
    let mut swapped = false;
    assert!(matches!(
        endpoint.commit_reload(&clashing, Vec::new(), || swapped = true),
        Err(ScriptReloadCommitError::Ownership {
            error: ScriptRouteRegistrationError::RootConflict { .. }
        })
    ));
    assert!(!swapped);
    assert_eq!(boundary.player_command_roots(), vec!["hello".to_owned()]);

    // A full command queue refuses before the swap as well.
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(1));
    endpoint
        .try_submit_command(chat("fills the queue"))
        .unwrap();
    let manifest = command_manifest("reloaded", "hello");
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let mut batch = CommandBatch::new(nonzero(2));
    batch.try_push(chat("cannot fit")).unwrap();
    let mut swapped = false;
    assert!(matches!(
        endpoint.commit_reload(&[manifest], vec![(admission, batch)], || swapped = true),
        Err(ScriptReloadCommitError::QueueFull)
    ));
    assert!(!swapped);
    assert!(boundary.player_command_roots().is_empty());
}

#[test]
fn a_server_that_dropped_its_half_refuses_every_host_submission() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(2));
    let manifest = command_manifest("reloaded", "hello");
    let admission = HostCommandAdmission::from_manifest(&manifest);

    // The command queue closes exactly when the server side goes away, which is
    // what a shutting-down server does to a host that is still running.
    drop(boundary);
    assert_eq!(
        endpoint.try_submit_command(chat("late")),
        Err(ScriptCommandSubmissionError::QueueClosed)
    );
    let mut batch = CommandBatch::new(nonzero(2));
    batch.try_push(chat("late")).unwrap();
    let mut swapped = false;
    assert!(matches!(
        endpoint.commit_reload(&[manifest], vec![(admission, batch)], || swapped = true),
        Err(ScriptReloadCommitError::QueueClosed)
    ));
    assert!(!swapped);
}

#[tokio::test]
async fn control_input_sender_closes_with_the_host_side() {
    let (boundary, _endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let control = boundary.host_input_sender();

    let (reload, _response) = envelope("open");
    assert_eq!(control.send_reload(reload).await, Ok(()));

    boundary.close_event_admission();
    let (reload, response) = envelope("closed");
    assert_eq!(
        control.send_reload(reload).await,
        Err(ScriptQueueError::Closed)
    );
    // A refused envelope drops its payload, so the reload caller observes a closed
    // host instead of an envelope queued for a runtime that is gone.
    assert_eq!(
        response.try_recv(),
        Err(std_mpsc::TryRecvError::Disconnected)
    );

    drop(boundary);
    let (reload, response) = envelope("gone");
    assert_eq!(
        control.send_reload(reload).await,
        Err(ScriptQueueError::Closed)
    );
    assert_eq!(
        response.try_recv(),
        Err(std_mpsc::TryRecvError::Disconnected)
    );
}

#[test]
fn a_poisoned_admission_ledger_refuses_the_next_batch() {
    let (boundary, endpoint) = script_boundary_pair(nonzero(4), nonzero(4));
    let manifest = command_manifest("probe", "hello");
    let admission = HostCommandAdmission::from_manifest(&manifest);

    // A poisoned ledger is the one state that refuses an issue, and only a panic
    // while its lock is held can poison it. The fault is injected here, against
    // the boundary's own internals, so no host-facing API exists just for it.
    let ledger = Arc::clone(&boundary.host_admissions);
    let poisoned = std::thread::spawn(move || {
        let _guard = ledger.pending.lock().unwrap();
        panic!("poison host admission ledger");
    });
    assert!(poisoned.join().is_err());

    let mut batch = CommandBatch::new(nonzero(2));
    batch.try_push(chat("unadmittable")).unwrap();
    let refused = endpoint
        .try_submit_plugin_batch(&admission, batch)
        .expect_err("a poisoned ledger cannot issue an admission");
    assert!(matches!(
        refused,
        ScriptBatchSubmissionError::Rejected {
            error: CommandBatchError::AdmissionUnavailable,
            ..
        }
    ));
}
