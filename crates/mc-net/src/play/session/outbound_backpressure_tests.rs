use std::collections::HashSet;

use tokio::sync::mpsc;

use super::*;

#[test]
fn loaded_chunk_batches_entity_spawns_before_bounded_outbound_queue() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("ChunkSpawnBatch"),
        name: "ChunkSpawnBatch".to_owned(),
    };
    let (session, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );

    for offset in 0..17 {
        assert!(
            registry
                .spawn_command_entity(
                    &SimulationAuthority::for_test(),
                    4,
                    "minecraft:cow".to_owned(),
                    Vec3::new(0.5 + f64::from(offset) * 0.01, 64.0, 0.5),
                )
                .is_empty()
        );
    }

    let dispatches = registry.mark_loaded(session, (0, 0));
    assert!(matches!(
        dispatches.as_slice(),
        [VisibilityDispatch {
            command: OutboundCommand::SpawnEntities(entities),
            ..
        }] if entities.len() == 17
    ));

    dispatch_visibility_commands(dispatches);
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::SpawnEntities(entities)) if entities.len() == 17
    ));
    assert!(rx.try_recv().is_err());
    let pressure = registry.pressure_snapshot();
    assert_eq!(pressure.entity_dispatches.spawn, 17);
    assert_eq!(pressure.reliable_command_drops, 0);
    assert_eq!(pressure.slow_client_pressure_sheds, 0);
}
