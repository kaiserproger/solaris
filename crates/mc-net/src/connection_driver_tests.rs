use std::time::Duration;

use super::*;

#[tokio::test(start_paused = true)]
async fn pre_play_deadline_is_absolute_across_intermediate_progress() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (first_progress_tx, first_progress_rx) = tokio::sync::oneshot::channel();
    let (_second_progress_tx, second_progress_rx) = tokio::sync::oneshot::channel::<()>();
    let result = before_pre_play_deadline(deadline, async {
        first_progress_rx.await.unwrap();
        second_progress_rx.await.unwrap();
        Ok::<(), ConnectionError>(())
    });
    tokio::pin!(result);
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);

    assert!(matches!(
        std::future::Future::poll(result.as_mut(), &mut context),
        std::task::Poll::Pending
    ));
    tokio::time::advance(Duration::from_secs(6)).await;
    first_progress_tx.send(()).unwrap();
    assert!(matches!(
        std::future::Future::poll(result.as_mut(), &mut context),
        std::task::Poll::Pending
    ));
    tokio::time::advance(Duration::from_secs(6)).await;

    assert!(matches!(
        result.await,
        Err(ConnectionError::PrePlayDeadlineExceeded {
            timeout: PRE_PLAY_TOTAL_TIMEOUT
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn pre_play_deadline_allows_completion_before_absolute_limit() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (complete_tx, complete_rx) = tokio::sync::oneshot::channel();
    let result = before_pre_play_deadline(deadline, async {
        Ok::<u8, ConnectionError>(complete_rx.await.unwrap())
    });
    tokio::pin!(result);
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);

    assert!(matches!(
        std::future::Future::poll(result.as_mut(), &mut context),
        std::task::Poll::Pending
    ));
    tokio::time::advance(Duration::from_secs(9)).await;
    complete_tx.send(7).unwrap();

    assert_eq!(result.await.unwrap(), 7);
}
