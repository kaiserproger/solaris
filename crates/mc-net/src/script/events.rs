use mc_script::{ScriptEvent, ScriptQueueError};

use crate::server::{ScriptEventSink, ShutdownHandle};

pub(crate) enum TargetedEventDelivery {
    Delivered,
    Closed,
    Shutdown,
}

pub(crate) async fn deliver_targeted_event(
    events: &ScriptEventSink,
    event: ScriptEvent,
    shutdown: &ShutdownHandle,
) -> TargetedEventDelivery {
    tokio::select! {
        biased;
        () = shutdown.notified() => TargetedEventDelivery::Shutdown,
        result = events.enqueue_targeted_event(event) => match result {
            Ok(()) => TargetedEventDelivery::Delivered,
            Err(ScriptQueueError::Closed) => TargetedEventDelivery::Closed,
            Err(ScriptQueueError::Full) => unreachable!("awaited script event delivery cannot report a full queue"),
            Err(_) => TargetedEventDelivery::Closed,
        }
    }
}

pub(crate) async fn deliver_required_targeted_event(
    events: &ScriptEventSink,
    event: ScriptEvent,
) -> TargetedEventDelivery {
    match events.enqueue_targeted_event(event).await {
        Ok(()) => TargetedEventDelivery::Delivered,
        Err(ScriptQueueError::Closed) => TargetedEventDelivery::Closed,
        Err(ScriptQueueError::Full) => {
            unreachable!("awaited script event delivery cannot report a full queue")
        }
        Err(_) => TargetedEventDelivery::Closed,
    }
}
