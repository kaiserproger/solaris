use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};

use tokio::sync::futures::OwnedNotified;
use tokio::sync::oneshot;
use tokio::time::Sleep;

use super::queue::{
    SIMULATION_OWNER_HEALTHY, SIMULATION_OWNER_SHUTTING_DOWN, SIMULATION_OWNER_STOPPED,
    SimulationQueueMetrics,
};
use super::{SIMULATION_RESPONSE_TIMEOUT, SimulationOutcome, SimulationRequestError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) struct SimulationResponseChannelClosed;

pub(in crate::play) struct SimulationResponseReceiver {
    receiver: oneshot::Receiver<SimulationOutcome>,
    metrics: Arc<SimulationQueueMetrics>,
    deadline: std::time::Instant,
    timeout: Option<Pin<Box<Sleep>>>,
    owner_unhealthy: Pin<Box<OwnedNotified>>,
}

impl std::fmt::Debug for SimulationResponseReceiver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SimulationResponseReceiver")
            .finish_non_exhaustive()
    }
}

impl SimulationResponseReceiver {
    pub(super) fn new(
        receiver: oneshot::Receiver<SimulationOutcome>,
        metrics: Arc<SimulationQueueMetrics>,
    ) -> Self {
        let owner_unhealthy = Arc::clone(&metrics.owner_state_notify).notified_owned();
        Self {
            receiver,
            metrics,
            deadline: std::time::Instant::now() + SIMULATION_RESPONSE_TIMEOUT,
            timeout: None,
            owner_unhealthy: Box::pin(owner_unhealthy),
        }
    }

    fn owner_state_error(&self) -> Option<SimulationRequestError> {
        match self.metrics.owner_state.load(Ordering::Acquire) {
            SIMULATION_OWNER_HEALTHY => None,
            SIMULATION_OWNER_SHUTTING_DOWN => Some(SimulationRequestError::ShuttingDown),
            SIMULATION_OWNER_STOPPED => Some(SimulationRequestError::OwnerStopped),
            _ => Some(SimulationRequestError::OwnerStopped),
        }
    }
}

impl Future for SimulationResponseReceiver {
    type Output = Result<SimulationOutcome, SimulationResponseChannelClosed>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Poll::Ready(outcome) = Pin::new(&mut self.receiver).poll(cx) {
            return Poll::Ready(outcome.map_err(|_| SimulationResponseChannelClosed));
        }
        if let Some(error) = self.owner_state_error() {
            return Poll::Ready(Ok(Err(error)));
        }
        if self.owner_unhealthy.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Ok(Err(self
                .owner_state_error()
                .unwrap_or(SimulationRequestError::OwnerStopped))));
        }
        if self.timeout.is_none() {
            let deadline = tokio::time::Instant::from_std(self.deadline);
            self.timeout = Some(Box::pin(tokio::time::sleep_until(deadline)));
        }
        if self
            .timeout
            .as_mut()
            .expect("response timeout initialized before poll")
            .as_mut()
            .poll(cx)
            .is_ready()
        {
            self.metrics
                .response_timeouts
                .fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Ok(Err(SimulationRequestError::ResponseTimeout)));
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::play::simulation::{
        SimulationCommand, SimulationResponse, simulation_channel_with_capacity,
    };

    #[tokio::test(start_paused = true)]
    async fn response_timeout_is_typed_and_counted() {
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let receiver = handle
            .enqueue(SimulationCommand::SaveBarrier {
                capture_world: false,
            })
            .unwrap();
        tokio::pin!(receiver);
        tokio::time::advance(SIMULATION_RESPONSE_TIMEOUT + Duration::from_millis(1)).await;

        assert_eq!(
            receiver.await.unwrap().unwrap_err(),
            SimulationRequestError::ResponseTimeout
        );
        assert_eq!(handle.snapshot().response_timeouts, 1);
        owner.shutdown();
    }

    #[tokio::test]
    async fn owner_shutdown_wakes_pending_response_without_waiting_for_deadline() {
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let receiver = handle
            .enqueue(SimulationCommand::SaveBarrier {
                capture_world: false,
            })
            .unwrap();
        owner.shutdown();

        assert_eq!(
            receiver.await.unwrap().unwrap_err(),
            SimulationRequestError::ShuttingDown
        );
    }

    #[tokio::test]
    async fn valid_response_wins_before_owner_health_or_deadline() {
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let receiver = handle
            .enqueue(SimulationCommand::SaveBarrier {
                capture_world: false,
            })
            .unwrap();
        let envelope = owner.drain_ready_batch(1).pop().unwrap();
        envelope.respond(Ok(SimulationResponse::SaveSnapshot(Err(
            SimulationRequestError::WorldUnavailable,
        ))));

        assert!(matches!(
            receiver.await,
            Ok(Ok(SimulationResponse::SaveSnapshot(Err(
                SimulationRequestError::WorldUnavailable
            ))))
        ));
    }

    #[tokio::test]
    async fn owner_drop_wakes_pending_response_as_stopped() {
        let (handle, owner) = simulation_channel_with_capacity(1);
        let receiver = handle
            .enqueue(SimulationCommand::SaveBarrier {
                capture_world: false,
            })
            .unwrap();
        drop(owner);

        assert_eq!(
            receiver.await.unwrap().unwrap_err(),
            SimulationRequestError::OwnerStopped
        );
    }

    #[tokio::test(start_paused = true)]
    async fn full_queue_admission_times_out_and_is_counted() {
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let _occupied = handle
            .enqueue(SimulationCommand::SaveBarrier {
                capture_world: false,
            })
            .unwrap();
        let session = handle.for_session(7);
        let waiting = tokio::spawn(async move {
            session
                .enqueue_player_command_wait(SimulationCommand::SaveBarrier {
                    capture_world: false,
                })
                .await
        });
        tokio::task::yield_now().await;
        tokio::time::advance(
            super::super::SIMULATION_QUEUE_ADMISSION_TIMEOUT + Duration::from_millis(1),
        )
        .await;

        assert_eq!(
            waiting.await.unwrap().unwrap_err(),
            SimulationRequestError::QueueAdmissionTimeout
        );
        assert_eq!(handle.snapshot().queue_admission_timeouts, 1);
        owner.shutdown();
    }
}
