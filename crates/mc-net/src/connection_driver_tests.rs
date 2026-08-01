use std::time::Duration;

use super::*;

#[tokio::test(start_paused = true)]
async fn pre_play_deadline_is_absolute_across_intermediate_progress() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let result = before_pre_play_deadline(deadline, async {
        tokio::time::sleep(Duration::from_secs(6)).await;
        tokio::time::sleep(Duration::from_secs(6)).await;
        Ok::<(), ConnectionError>(())
    })
    .await;

    assert!(matches!(
        result,
        Err(ConnectionError::PrePlayDeadlineExceeded {
            timeout: PRE_PLAY_TOTAL_TIMEOUT
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn pre_play_deadline_allows_completion_before_absolute_limit() {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let result = before_pre_play_deadline(deadline, async {
        tokio::time::sleep(Duration::from_secs(9)).await;
        Ok::<u8, ConnectionError>(7)
    })
    .await;

    assert_eq!(result.unwrap(), 7);
}
