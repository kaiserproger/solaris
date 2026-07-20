use super::*;
use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_protocol::frame::Compression;
use mc_world::{BlockStateId, ChunkPos, WorldStorage};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};

fn air_block_registry() -> BlockRegistry {
    BlockRegistry::from_report(&[BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    }])
    .expect("air registry builds")
}

fn biome_registry() -> Registry {
    Registry {
        id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
        entries: vec![Identifier::parse("minecraft:plains").unwrap()],
    }
}

async fn drive_stream_to_completion(
    stream: &mut ChunkStreamState,
    writer: &mut (impl AsyncWriteExt + Unpin),
    light_cache: &mut LightCache,
) {
    let progress_notify = stream.progress_notify();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let progress = progress_notify.notified();
            tokio::pin!(progress);
            progress.as_mut().enable();
            if stream.step(writer, light_cache).await.unwrap() == ChunkStreamStep::Complete {
                return;
            }
            if !stream.has_immediate_work() {
                progress.await;
            }
        }
    })
    .await
    .expect("resident chunk preparation and delivery waited for the global world writer");
}

#[tokio::test]
async fn resident_chunk_is_prepared_and_delivered_while_world_writer_is_held() {
    let registry = Arc::new(air_block_registry());
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut storage = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16);
    for z in -1..=1 {
        for x in -1..=1 {
            let position = ChunkPos { x, z };
            storage
                .insert_generated_chunk(
                    position,
                    Chunk::empty(position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
    }
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let chunk_source = storage.chunk_source_view();
    let world = Arc::new(Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (tx, _rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "resident-stream".to_string(),
    };
    let (session_id, _) = sessions.register(
        &profile,
        (0, 0),
        0,
        desired_chunk_set(0, 0, 0),
        tx,
        PlayerPose::new(0.5, DEFAULT_SPAWN_Y, 0.5),
    );
    let (held_tx, held_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let holder_world = Arc::clone(&world);
    let holder = tokio::spawn(async move {
        let writer_guard = holder_world.lock().await;
        held_tx.send(()).expect("test observes held world writer");
        release_rx.await.expect("test releases held world writer");
        drop(writer_guard);
    });
    held_rx.await.expect("world writer task acquired the mutex");
    let mut stream = ChunkStreamState::new(
        Arc::clone(&world),
        Arc::new(biome_registry()),
        Arc::clone(&registry),
        Some(Arc::new(BlockLightTable::from_arrays(
            "resident stream test",
            vec![0],
            vec![0],
            vec![true],
        ))),
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
        sessions,
        session_id,
        0,
        0,
        0.0,
        0,
        ChunkPipelineResources::with_limits(1, 1),
        ChunkPipelinePolicy {
            chunk_prepare_batch_size: 1,
            chunk_result_queue_size: 1,
            ..ChunkPipelinePolicy::default()
        },
    )
    .with_world_read(Some(world_read))
    .with_world_mutation(Some(world_mutation))
    .with_chunk_source(Some(chunk_source));
    let mut writer = tokio::io::sink();
    let mut light_cache = LightCache::new();

    drive_stream_to_completion(&mut stream, &mut writer, &mut light_cache).await;

    assert_eq!(stream.emitted, 1);
    assert!(stream.framed_bytes > 0);
    release_tx.send(()).expect("world writer task is waiting");
    holder.await.expect("world writer task exits cleanly");
}
