use super::*;
use mc_protocol::frame::Compression;
use mc_world::WorldStorage;
use std::io;
use std::pin::Pin;
use std::sync::Barrier;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::Mutex;

fn test_biome_registry() -> Registry {
    Registry {
        id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
        entries: vec![Identifier::parse("minecraft:plains").unwrap()],
    }
}

fn test_stream(control: crate::RuntimeControlHandle, view_distance: i32) -> ChunkStreamState {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(
        Arc::clone(&registry),
        1,
    )));
    let policy = ChunkPipelinePolicy {
        chunk_result_queue_size: 64,
        ..ChunkPipelinePolicy::default()
    };
    ChunkStreamState::new(
        world,
        Arc::new(test_biome_registry()),
        registry,
        None,
        Arc::new(ItemRegistry::from_report(&[])),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::block_entity_types::BlockEntityTypeRegistry::default()),
        None,
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(Vec::new()),
        Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Compression::Disabled,
        Arc::new(SessionRegistry::new()),
        1,
        0,
        0,
        0.0,
        view_distance,
        ChunkPipelineResources::with_limits(1, 1),
        policy,
    )
    .with_runtime_control(Some(control))
}

fn control(queue_pressure_percent: u8, view_distance: i32) -> crate::RuntimeControlHandle {
    crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy {
            min_view_distance: view_distance,
            max_view_distance: view_distance,
            min_chunk_send_rate: 1,
            max_chunk_send_rate: 16,
            min_chunk_load_rate: 1,
            max_chunk_load_rate: 32,
            min_chunk_generate_rate: 1,
            max_chunk_generate_rate: 16,
            target_first_chunk_ms: 1,
            queue_pressure_percent,
            scale_down_after_ticks: 1,
            ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
        },
        initial_limits: crate::RuntimeControlLimits {
            view_distance,
            chunk_send_rate: 16,
            chunk_load_rate: 32,
            chunk_generate_rate: 16,
        },
    })
}

fn absent_result(stream: &mut ChunkStreamState) {
    let request = stream.scheduler.poll_next().expect("queued chunk");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Absent,
    });
}

fn ready_result(stream: &mut ChunkStreamState) {
    let request = stream.scheduler.poll_next().expect("queued chunk");
    stream.accept_result(ChunkPrepareResult {
        request,
        prepare_claim: None,
        fetch_ms: 0,
        pressure_flush: PressureFlushTiming::default(),
        staged: Vec::new(),
        outcome: ChunkPrepareOutcome::Ready(Box::new(PreparedChunkFrame {
            frame: Bytes::from_static(b"prepared-frame"),
            light: None,
            herd_spawns: Vec::new(),
            hydrated_campfires: Vec::new(),
            packet_data_len: 1,
            build_timing: ChunkBuildTiming::default(),
            write_timing: ChunkWriteTiming::default(),
        })),
    });
}

async fn next_signal(
    signals: &mut crate::control_plane::RuntimeControlSignalReceiver,
) -> crate::control_plane::RuntimeControlSignal {
    tokio::time::timeout(Duration::from_secs(1), signals.recv())
        .await
        .expect("exact runtime-control event was not published")
        .expect("runtime-control receiver remains open")
}

fn observe_signal(
    control: &crate::RuntimeControlHandle,
    signal: crate::control_plane::RuntimeControlSignal,
) -> crate::AutoscaleDecision {
    control.observe_signal_and_apply(signal, |_decision, _draining| {})
}

#[tokio::test]
async fn completing_step_recovers_active_chunk_pressure_once() {
    let control = control(1, 0);
    let mut signals = control.take_signal_receiver().unwrap();
    let mut stream = test_stream(control, 0);
    absent_result(&mut stream);
    stream.observe_runtime_control();

    assert_eq!(
        stream
            .step(&mut tokio::io::sink(), &mut LightCache::new())
            .await
            .unwrap(),
        ChunkStreamStep::Complete
    );
    assert_eq!(
        next_signal(&mut signals).await,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        }
    );
    assert_eq!(
        next_signal(&mut signals).await,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        }
    );
    drop(stream);
    assert_eq!(signals.try_recv(), None);
}

struct FailingWriter;

impl AsyncWrite for FailingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("injected write failure")))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn aborted_step_recovers_active_chunk_pressure_before_drop() {
    let control = control(1, 1);
    let mut signals = control.take_signal_receiver().unwrap();
    let mut stream = test_stream(control, 1);
    ready_result(&mut stream);
    stream.observe_runtime_control();
    assert!(matches!(
        next_signal(&mut signals).await,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1
        }
    ));

    assert!(
        stream
            .step(&mut FailingWriter, &mut LightCache::new())
            .await
            .is_err()
    );
    assert_eq!(
        next_signal(&mut signals).await,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        }
    );
    drop(stream);
    assert_eq!(signals.try_recv(), None);
}

#[tokio::test]
async fn dropping_stream_recovers_active_chunk_pressure_once() {
    let control = control(1, 1);
    let mut signals = control.take_signal_receiver().unwrap();
    let mut stream = test_stream(control, 1);
    absent_result(&mut stream);
    stream.observe_runtime_control();
    let _ = next_signal(&mut signals).await;

    drop(stream);
    assert_eq!(
        next_signal(&mut signals).await,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        }
    );
    assert_eq!(signals.try_recv(), None);
}

#[tokio::test]
async fn first_actual_chunk_send_publishes_sla_pressure_and_source_recovery() {
    let control = control(100, 1);
    let mut signals = control.take_signal_receiver().unwrap();
    let mut stream = test_stream(control.clone(), 1);
    stream.started = Instant::now() - Duration::from_millis(5);
    ready_result(&mut stream);

    assert_eq!(
        stream
            .emit_next_ready(&mut tokio::io::sink(), &mut LightCache::new())
            .await
            .unwrap(),
        EmitReadyResult::SentPacket
    );
    drop(stream);

    let pressure = next_signal(&mut signals).await;
    assert_eq!(
        control
            .observe_signal_and_apply(pressure, |_decision, _draining| {})
            .pressure,
        Some(crate::AutoscalePressure::FirstChunkSla)
    );
    let recovery = next_signal(&mut signals).await;
    assert_eq!(
        control
            .observe_signal_and_apply(recovery, |_decision, _draining| {})
            .pressure,
        None
    );
}

#[tokio::test]
async fn replan_recovers_active_queue_and_first_chunk_tokens_once_before_receiver_runs() {
    let control = control(1, 1);
    let mut signals = control.take_signal_receiver().unwrap();
    let mut stream = test_stream(control, 1);
    stream.started = Instant::now() - Duration::from_millis(5);
    ready_result(&mut stream);

    stream.observe_runtime_control();
    assert!(stream.chunk_queue_saturated);
    assert_eq!(
        stream
            .emit_next_ready(&mut tokio::io::sink(), &mut LightCache::new())
            .await
            .unwrap(),
        EmitReadyResult::SentPacket
    );
    assert!(stream.first_chunk_sla_active);

    stream.replan_center(1, 0, 0.0);
    stream.replan_center(2, 0, 0.0);

    assert!(!stream.chunk_queue_saturated);
    assert!(!stream.first_chunk_sla_active);
    assert_eq!(
        signals.try_recv(),
        Some(crate::control_plane::RuntimeControlSignal::FirstChunkSla { active_sources: 1 })
    );
    assert_eq!(
        signals.try_recv(),
        Some(crate::control_plane::RuntimeControlSignal::FirstChunkSla { active_sources: 0 })
    );
    assert_eq!(
        signals.try_recv(),
        Some(crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        })
    );
    assert_eq!(
        signals.try_recv(),
        Some(crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        })
    );
    assert_eq!(signals.try_recv(), None);

    drop(stream);
    assert_eq!(signals.try_recv(), None);
}

#[test]
fn concurrent_independent_sources_preserve_counts_and_recovery_isolation() {
    let control = control(50, 1);
    let mut signals = control.take_signal_receiver().unwrap();
    let queue_first = control.chunk_pressure_source();
    let queue_second = control.chunk_pressure_source();
    let first_chunk_first = control.first_chunk_sla_source();
    let first_chunk_second = control.first_chunk_sla_source();
    let activate = Arc::new(Barrier::new(5));

    let (mut queue_first, queue_second, mut first_chunk_first, first_chunk_second) =
        std::thread::scope(|scope| {
            let queue_first_barrier = Arc::clone(&activate);
            let queue_first = scope.spawn(move || {
                let mut source = queue_first;
                queue_first_barrier.wait();
                assert!(source.set_saturated(true));
                source
            });
            let queue_second_barrier = Arc::clone(&activate);
            let queue_second = scope.spawn(move || {
                let mut source = queue_second;
                queue_second_barrier.wait();
                assert!(source.set_saturated(true));
                source
            });
            let first_chunk_first_barrier = Arc::clone(&activate);
            let first_chunk_first = scope.spawn(move || {
                let mut source = first_chunk_first;
                first_chunk_first_barrier.wait();
                assert!(source.set_active(true));
                source
            });
            let first_chunk_second_barrier = Arc::clone(&activate);
            let first_chunk_second = scope.spawn(move || {
                let mut source = first_chunk_second;
                first_chunk_second_barrier.wait();
                assert!(source.set_active(true));
                source
            });

            activate.wait();
            (
                queue_first.join().unwrap(),
                queue_second.join().unwrap(),
                first_chunk_first.join().unwrap(),
                first_chunk_second.join().unwrap(),
            )
        });

    let first_chunk_active = signals.try_recv().unwrap();
    assert_eq!(
        first_chunk_active,
        crate::control_plane::RuntimeControlSignal::FirstChunkSla { active_sources: 2 }
    );
    assert_eq!(
        observe_signal(&control, first_chunk_active).pressure,
        Some(crate::AutoscalePressure::FirstChunkSla)
    );
    let queue_active = signals.try_recv().unwrap();
    assert_eq!(
        queue_active,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 2,
        }
    );
    assert_eq!(
        observe_signal(&control, queue_active).pressure,
        Some(crate::AutoscalePressure::FirstChunkSla)
    );
    assert_eq!(signals.try_recv(), None);

    let recover = Arc::new(Barrier::new(3));
    (queue_first, first_chunk_first) = std::thread::scope(|scope| {
        let queue_barrier = Arc::clone(&recover);
        let queue = scope.spawn(move || {
            queue_barrier.wait();
            assert!(queue_first.set_saturated(false));
            queue_first
        });
        let first_chunk_barrier = Arc::clone(&recover);
        let first_chunk = scope.spawn(move || {
            first_chunk_barrier.wait();
            assert!(first_chunk_first.set_active(false));
            first_chunk_first
        });

        recover.wait();
        (queue.join().unwrap(), first_chunk.join().unwrap())
    });

    let first_chunk_recovery = signals.try_recv().unwrap();
    assert_eq!(
        first_chunk_recovery,
        crate::control_plane::RuntimeControlSignal::FirstChunkSla { active_sources: 1 }
    );
    assert_eq!(
        observe_signal(&control, first_chunk_recovery).pressure,
        Some(crate::AutoscalePressure::FirstChunkSla)
    );
    let queue_recovery = signals.try_recv().unwrap();
    assert_eq!(
        queue_recovery,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        }
    );
    assert_eq!(
        observe_signal(&control, queue_recovery).pressure,
        Some(crate::AutoscalePressure::FirstChunkSla)
    );
    assert_eq!(signals.try_recv(), None);

    drop(first_chunk_second);
    drop(queue_second);
    let last_first_chunk_recovery = signals.try_recv().unwrap();
    assert_eq!(
        last_first_chunk_recovery,
        crate::control_plane::RuntimeControlSignal::FirstChunkSla { active_sources: 0 }
    );
    assert_eq!(
        observe_signal(&control, last_first_chunk_recovery).pressure,
        Some(crate::AutoscalePressure::ChunkQueue)
    );
    let last_queue_recovery = signals.try_recv().unwrap();
    assert_eq!(
        last_queue_recovery,
        crate::control_plane::RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        }
    );
    assert_eq!(observe_signal(&control, last_queue_recovery).pressure, None);
    assert_eq!(signals.try_recv(), None);

    drop(first_chunk_first);
    drop(queue_first);
    assert_eq!(signals.try_recv(), None);
}
