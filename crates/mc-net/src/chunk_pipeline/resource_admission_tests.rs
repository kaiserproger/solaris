use super::*;
use std::future::Future;
use std::task::Poll;
use std::time::Duration;

#[tokio::test]
async fn background_scale_up_wakes_queued_preparation() {
    let resources = ChunkPipelineResources::with_limits(1, 5);
    let first = resources.acquire_prepare_request().await.unwrap();
    let second = resources.acquire_prepare_request().await.unwrap();
    resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false);
    assert!(resources.try_acquire_prepare_request().is_none());

    let mut third = std::pin::pin!(resources.acquire_prepare_request());
    std::future::poll_fn(|cx| {
        assert!(third.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleUp, false);
    let third = tokio::time::timeout(Duration::from_secs(1), third)
        .await
        .expect("scale-up wakes queued preparation")
        .unwrap();
    let fourth = resources.try_acquire_prepare_request().unwrap();
    assert!(resources.try_acquire_prepare_request().is_none());
    drop((first, second, third, fourth));
}

#[tokio::test]
async fn background_scale_down_preserves_inflight_and_wakes_after_release() {
    let resources = ChunkPipelineResources::with_limits(1, 3);
    let first = resources.acquire_prepare_request().await.unwrap();
    let second = resources.acquire_prepare_request().await.unwrap();
    resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false);
    let mut waiting = std::pin::pin!(resources.acquire_prepare_request());
    std::future::poll_fn(|cx| {
        assert!(waiting.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(first);
    std::future::poll_fn(|cx| {
        assert!(
            waiting.as_mut().poll(cx).is_pending(),
            "in-flight preparation must fall below the reduced limit"
        );
        Poll::Ready(())
    })
    .await;
    drop(second);
    let waiting = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .expect("release wakes queued preparation")
        .unwrap();
    assert!(resources.try_acquire_prepare_request().is_none());
    drop(waiting);
}

#[tokio::test]
async fn foreground_cpu_progresses_while_background_preparation_is_limited() {
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let background_request = resources.acquire_prepare_request().await.unwrap();
    let background_cpu = resources.acquire_cpu().await.unwrap();
    resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false);

    let mut waiting_background = std::pin::pin!(resources.acquire_prepare_request());
    std::future::poll_fn(|cx| {
        assert!(waiting_background.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    let mut foreground = std::pin::pin!(resources.acquire_cpu());
    let foreground = std::future::poll_fn(|cx| match foreground.as_mut().poll(cx) {
        Poll::Ready(result) => Poll::Ready(result.unwrap()),
        Poll::Pending => panic!("background reduction must not withhold free shared CPU capacity"),
    })
    .await;
    assert!(
        resources.try_acquire_cpu().is_none(),
        "foreground work must still respect the shared physical CPU ceiling"
    );
    drop(foreground);
    std::future::poll_fn(|cx| {
        assert!(
            waiting_background.as_mut().poll(cx).is_pending(),
            "free CPU capacity must not bypass background preparation admission"
        );
        Poll::Ready(())
    })
    .await;
    drop(background_request);
    let next_background = tokio::time::timeout(Duration::from_secs(1), waiting_background)
        .await
        .expect("background release wakes its own queue")
        .unwrap();
    drop((background_cpu, next_background));
}

#[tokio::test]
async fn background_saturation_preserves_foreground_cpu_headroom() {
    let resources = ChunkPipelineResources::with_limits(1, 3);
    for action in [
        crate::AutoscaleAction::Hold,
        crate::AutoscaleAction::ScaleUp,
    ] {
        resources.apply_runtime_control_action(action, false);
        let mut background = Vec::new();
        while let Some(request) = resources.try_acquire_prepare_request() {
            let cpu = resources
                .try_acquire_cpu()
                .expect("admitted background CPU");
            background.push((request, cpu));
        }
        let foreground = resources
            .try_acquire_cpu()
            .expect("background saturation must leave CPU capacity for foreground work");
        assert!(resources.try_acquire_cpu().is_none());
        drop((foreground, background));
    }
}
