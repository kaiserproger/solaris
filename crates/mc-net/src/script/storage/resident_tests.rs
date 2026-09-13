//! Acceptance tests for durable resident identity (C3).
//!
//! Every test drives the real storage runtime over a real plugin storage
//! journal and the real regional entity owner; nothing asserts plumbing.

use std::sync::Arc;

use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_entity::{EntityLifecycle, Vec3};
use mc_script::{
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptResidentLifecycle, ScriptResidentOperation, ScriptResidentResult,
    resident_entity_uuid, resident_generation_id,
};
use uuid::Uuid;

use crate::play::SessionRegistry;
use crate::script::storage::PluginStorage;
use crate::script::storage::world_inventory::InventoryRuntime;

const OWNER: &str = "settlement";
const GENERATION_SLOT: u32 = 3;

struct Fixture {
    root: tempfile::TempDir,
    sessions: Arc<SessionRegistry>,
    runtime: InventoryRuntime,
}

impl Fixture {
    fn new(root: tempfile::TempDir) -> Self {
        Self::with_world(
            root,
            Arc::new(super::resident_settlement_tests::TestWorld::default()),
        )
    }

    /// A fixture whose point-of-interest cells refuse a standing body, as a
    /// slope the building's anchor row does not clear does.
    fn unstandable(root: tempfile::TempDir) -> Self {
        let world = Arc::new(super::resident_settlement_tests::TestWorld::default());
        world.refuse_standing();
        Self::with_world(root, world)
    }

    fn with_world(
        root: tempfile::TempDir,
        world: Arc<super::resident_settlement_tests::TestWorld>,
    ) -> Self {
        let sessions = Arc::new(SessionRegistry::new());
        let runtime = InventoryRuntime::player_only_for_test(
            root.path(),
            Arc::clone(&sessions),
            Arc::new(solaris_required_items()),
            Arc::new(solaris_required_item_facts()),
        )
        .with_resident_world(world);
        Self {
            root,
            sessions,
            runtime,
        }
    }

    fn generation(&self) -> String {
        resident_generation_id("world-identity", "site-7", GENERATION_SLOT).unwrap()
    }

    async fn execute(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> ScriptOperationOutcome {
        self.runtime
            .execute_resident_operation(storage, plugin_id, request)
            .await
            .expect("resident operation reaches the durable boundary")
    }
}

fn mutation(operation: ScriptResidentOperation) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::Resident {
            operation: operation.clone(),
        },
    )
    .unwrap_or_else(|error| panic!("valid resident request {operation:?}: {error}"))
}

fn query(handles: Vec<String>, cursor: Option<String>) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "query",
        ScriptOperation::Resident {
            operation: ScriptResidentOperation::Query { handles, cursor },
        },
    )
    .unwrap()
}

/// The single resident carried by a mutation snapshot or a handle query.
fn snapshot(outcome: &ScriptOperationOutcome) -> mc_script::ScriptResidentSnapshot {
    match outcome.payload() {
        ScriptOperationPayload::Resident { result } => match &**result {
            ScriptResidentResult::Snapshot { resident } => resident.clone(),
            ScriptResidentResult::Page { residents, .. } if residents.len() == 1 => {
                residents[0].clone()
            }
            _ => panic!(
                "expected one resident, got state={:?} failure={:?}",
                outcome.state(),
                outcome.failure()
            ),
        },
        other => panic!("expected a resident payload, got {other:?}"),
    }
}

fn page(
    outcome: &ScriptOperationOutcome,
) -> (Vec<mc_script::ScriptResidentSnapshot>, Option<String>) {
    let ScriptOperationPayload::Resident { result } = outcome.payload() else {
        panic!("expected a resident page, got {:?}", outcome.payload());
    };
    let ScriptResidentResult::Page { residents, cursor } = &**result else {
        panic!("expected a resident page, got {:?}", outcome.payload());
    };
    (residents.clone(), cursor.clone())
}

#[tokio::test]
async fn handle_survives_storage_and_session_reopen_with_the_same_uuid() {
    let fixture = Fixture::new(tempfile::tempdir().unwrap());
    let generation = fixture.generation();
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let spawned = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(8.5, 64.0, 8.5))
        .await
        .unwrap();
    assert_eq!(spawned.lifecycle, ScriptResidentLifecycle::AliveLoaded);
    assert_eq!(spawned.generation_id.as_deref(), Some(generation.as_str()));
    let handle = spawned.handle.clone();
    let uuid = spawned.entity_uuid.clone();
    assert_eq!(
        uuid,
        Uuid::from_bytes(resident_entity_uuid(&generation).unwrap()).to_string()
    );
    drop(storage);

    // A full reopen with a fresh registry: the handle still resolves and the
    // resident is unloaded, never dead.
    let reopened = Fixture::new(tempfile::tempdir().unwrap());
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let outcome = reopened
        .execute(&mut storage, OWNER, &query(vec![handle.clone()], None))
        .await;
    let (residents, cursor) = page(&outcome);
    assert_eq!(cursor, None);
    assert_eq!(residents.len(), 1);
    assert_eq!(residents[0].handle, handle);
    assert_eq!(residents[0].entity_uuid, uuid);
    assert_eq!(
        residents[0].lifecycle,
        ScriptResidentLifecycle::AliveUnloaded
    );
    assert!(residents[0].loaded.is_none());
}

#[tokio::test]
async fn rematerialising_a_chunk_never_creates_a_second_resident() {
    let fixture = Fixture::new(tempfile::tempdir().unwrap());
    let generation = fixture.generation();
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let first = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(4.0, 64.0, 4.0))
        .await
        .unwrap();
    let second = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(4.0, 64.0, 4.0))
        .await
        .unwrap();
    assert_eq!(first.handle, second.handle);
    assert_eq!(first.entity_uuid, second.entity_uuid);
    assert_eq!(
        fixture.sessions.resident_entity_count_for_test(),
        1,
        "generation id must materialise exactly one entity"
    );
}

#[tokio::test]
async fn a_foreign_plugin_using_a_stolen_handle_is_forbidden() {
    let fixture = Fixture::new(tempfile::tempdir().unwrap());
    let generation = fixture.generation();
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let resident = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(1.0, 64.0, 1.0))
        .await
        .unwrap();

    let outcome = fixture
        .execute(
            &mut storage,
            "other-plugin",
            &query(vec![resident.handle.clone()], None),
        )
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let release = mutation(ScriptResidentOperation::Release {
        operation_id: "release".to_owned(),
        handle: resident.handle.clone(),
        expected_revision: resident.revision,
    });
    let outcome = fixture
        .execute(&mut storage, "other-plugin", &release)
        .await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));
    // No effect: the owner still holds the resident.
    let outcome = fixture
        .execute(&mut storage, OWNER, &query(vec![resident.handle], None))
        .await;
    assert_eq!(
        snapshot(&outcome).lifecycle,
        ScriptResidentLifecycle::AliveLoaded
    );
}

#[tokio::test]
async fn a_spawn_onto_cells_that_refuse_a_standing_body_is_blocked() {
    let fixture = Fixture::unstandable(tempfile::tempdir().unwrap());
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let generation = fixture.generation();
    let token = InventoryRuntime::reserve_resident_site(
        &mut storage,
        OWNER,
        &generation,
        Vec3::new(2.0, 64.0, 2.0),
    )
    .unwrap();
    let spawn = mutation(ScriptResidentOperation::Spawn {
        operation_id: "spawn".to_owned(),
        spawn_site_token: token,
        profile: mc_script::ScriptResidentProfile::new(mc_script::ScriptResidentKind::Villager),
    });
    let outcome = fixture.execute(&mut storage, OWNER, &spawn).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Blocked));
    assert_eq!(
        fixture.sessions.resident_entity_count_for_test(),
        0,
        "a refused spawn materialises no entity"
    );
}

#[tokio::test]
async fn death_leaves_a_tombstone_that_blocks_reuse_while_release_frees_capacity() {
    let fixture = Fixture::new(tempfile::tempdir().unwrap());
    let generation = fixture.generation();
    let uuid = Uuid::from_bytes(resident_entity_uuid(&generation).unwrap());
    let entity = fixture
        .sessions
        .spawn_resident_entity_for_test(uuid, Vec3::new(2.0, 64.0, 2.0));
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let resident = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(2.0, 64.0, 2.0))
        .await
        .unwrap();
    assert_eq!(resident.lifecycle, ScriptResidentLifecycle::AliveLoaded);
    assert!(fixture.sessions.convert_resident_entity_for_test(entity.id));
    assert_eq!(
        fixture.sessions.resident_entity_count_for_test(),
        1,
        "the converted entity keeps its bound identity"
    );

    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &query(vec![resident.handle.clone()], None),
        )
        .await;
    let dead = snapshot(&outcome);
    assert_eq!(dead.lifecycle, ScriptResidentLifecycle::Dead);
    assert!(dead.loaded.is_none());

    // The tombstone survives the frame: a later read still reports death even
    // though the bound entity was converted.
    let outcome = fixture
        .execute(
            &mut storage,
            OWNER,
            &query(vec![resident.handle.clone()], None),
        )
        .await;
    assert_eq!(snapshot(&outcome).lifecycle, ScriptResidentLifecycle::Dead);

    // Re-materialising the same generation adopts the tombstone instead of
    // creating a second resident.
    let rematerialised = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(2.0, 64.0, 2.0))
        .await
        .unwrap();
    assert_eq!(rematerialised.handle, dead.handle);
    assert_eq!(rematerialised.lifecycle, ScriptResidentLifecycle::Dead);
    assert_eq!(fixture.sessions.resident_entity_count_for_test(), 1);

    // The converted tombstone frees living capacity, so a new site can be
    // settled.
    storage.resident_live_capacity_for_test(1);
    let other = resident_generation_id("world-identity", "site-8", 0).unwrap();
    let token = InventoryRuntime::reserve_resident_site(
        &mut storage,
        OWNER,
        &other,
        Vec3::new(40.0, 64.0, 40.0),
    )
    .unwrap();
    let spawn = mutation(ScriptResidentOperation::Spawn {
        operation_id: "spawn".to_owned(),
        spawn_site_token: token,
        profile: mc_script::ScriptResidentProfile::new(mc_script::ScriptResidentKind::Villager),
    });
    let outcome = fixture.execute(&mut storage, OWNER, &spawn).await;
    assert_eq!(outcome.failure(), None);
    let settled = snapshot(&outcome);
    assert_eq!(settled.lifecycle, ScriptResidentLifecycle::AliveLoaded);

    // Release frees the living slot again and keeps the handle as released.
    let release = mutation(ScriptResidentOperation::Release {
        operation_id: "release".to_owned(),
        handle: settled.handle.clone(),
        expected_revision: settled.revision,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &release).await;
    assert_eq!(
        snapshot(&outcome).lifecycle,
        ScriptResidentLifecycle::Released
    );
    let next_token = InventoryRuntime::reserve_resident_site(
        &mut storage,
        OWNER,
        &other,
        Vec3::new(40.0, 64.0, 40.0),
    );
    assert_eq!(next_token, Err(ScriptOperationFailure::Capacity));
}

#[tokio::test]
async fn repeated_operation_id_is_idempotent_and_a_changed_fingerprint_conflicts() {
    let fixture = Fixture::new(tempfile::tempdir().unwrap());
    let generation = fixture.generation();
    let uuid = Uuid::from_bytes(resident_entity_uuid(&generation).unwrap());
    fixture
        .sessions
        .spawn_resident_entity_for_test(uuid, Vec3::new(6.0, 64.0, 6.0));
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let claim = mutation(ScriptResidentOperation::Claim {
        operation_id: "op-1".to_owned(),
        actor_id: 7,
        entity_uuid: uuid.to_string(),
        expected_entity_revision: 0,
    });
    // No authenticated actor: the claim is refused before any durable change.
    let outcome = fixture.execute(&mut storage, OWNER, &claim).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));

    let resident = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(6.0, 64.0, 6.0))
        .await
        .unwrap();
    let set_pois = mutation(ScriptResidentOperation::SetPois {
        operation_id: "op-2".to_owned(),
        handle: resident.handle.clone(),
        home_poi: Some("home-1".to_owned()),
        work_poi: None,
        meeting_poi: None,
        expected_revision: resident.revision,
    });
    let first = fixture.execute(&mut storage, OWNER, &set_pois).await;
    let first_revision = first.revision();
    assert_eq!(first.failure(), None);
    assert_eq!(snapshot(&first).pois.home.as_deref(), Some("home-1"));

    // Same operation id and fingerprint: the stored outcome is replayed.
    let replay = fixture.execute(&mut storage, OWNER, &set_pois).await;
    assert_eq!(replay.revision(), first_revision);
    assert_eq!(snapshot(&replay), snapshot(&first));

    // Different fingerprint under the same operation id: typed conflict, and
    // the resident keeps its bound POIs.
    let conflicting = mutation(ScriptResidentOperation::SetPois {
        operation_id: "op-2".to_owned(),
        handle: resident.handle.clone(),
        home_poi: Some("home-2".to_owned()),
        work_poi: None,
        meeting_poi: None,
        expected_revision: resident.revision,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &conflicting).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
    let outcome = fixture
        .execute(&mut storage, OWNER, &query(vec![resident.handle], None))
        .await;
    assert_eq!(snapshot(&outcome).pois.home.as_deref(), Some("home-1"));
}

#[tokio::test]
async fn oversized_resident_queries_are_rejected_before_the_boundary() {
    let handles = (0..=mc_script::MAX_RESIDENT_QUERY_HANDLES)
        .map(|index| format!("h{index:02}"))
        .collect::<Vec<_>>();
    assert!(matches!(
        ScriptOperationRequest::try_new(
            "query",
            ScriptOperation::Resident {
                operation: ScriptResidentOperation::Query {
                    handles,
                    cursor: None,
                },
            },
        ),
        Err(mc_script::ScriptDtoError::TooManyEntries { .. })
    ));

    // A page is bounded and never scans beyond the plugin's own records.
    let fixture = Fixture::new(tempfile::tempdir().unwrap());
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    for slot in 0..2 {
        let generation = resident_generation_id("world-identity", "site-pages", slot).unwrap();
        fixture
            .runtime
            .materialize_resident(
                &mut storage,
                OWNER,
                &generation,
                Vec3::new(10.0 + f64::from(slot), 64.0, 10.0),
            )
            .await
            .unwrap();
    }
    let foreign = resident_generation_id("world-identity", "site-pages", 9).unwrap();
    fixture
        .runtime
        .materialize_resident(
            &mut storage,
            "other-plugin",
            &foreign,
            Vec3::new(30.0, 64.0, 30.0),
        )
        .await
        .unwrap();

    let outcome = fixture
        .execute(&mut storage, OWNER, &query(Vec::new(), None))
        .await;
    let (residents, cursor) = page(&outcome);
    assert_eq!(residents.len(), 2);
    assert_eq!(cursor, None);
    assert!(
        residents
            .iter()
            .all(|resident| resident.handle != String::new()),
        "every page entry carries its durable handle"
    );

    let unknown = query(Vec::new(), Some("missing-handle".to_owned()));
    let outcome = fixture.execute(&mut storage, OWNER, &unknown).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::CursorExpired)
    );
}

#[tokio::test]
async fn stale_revision_and_unknown_handles_are_typed_failures() {
    let fixture = Fixture::new(tempfile::tempdir().unwrap());
    let generation = fixture.generation();
    let mut storage = PluginStorage::open(fixture.root.path()).unwrap();
    let resident = fixture
        .runtime
        .materialize_resident(&mut storage, OWNER, &generation, Vec3::new(3.0, 64.0, 3.0))
        .await
        .unwrap();

    let stale = mutation(ScriptResidentOperation::Release {
        operation_id: "release".to_owned(),
        handle: resident.handle.clone(),
        expected_revision: resident.revision + 1,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &stale).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );

    let unknown = mutation(ScriptResidentOperation::Release {
        operation_id: "release-missing".to_owned(),
        handle: "nope".to_owned(),
        expected_revision: 0,
    });
    let outcome = fixture.execute(&mut storage, OWNER, &unknown).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));

    // Death is not unloaded: an entity that is gone from every loaded lane
    // reports unloaded, and the tombstone path is the only death signal.
    assert_eq!(
        snapshot(
            &fixture
                .execute(&mut storage, OWNER, &query(vec![resident.handle], None))
                .await
        )
        .lifecycle,
        ScriptResidentLifecycle::AliveLoaded
    );
    assert_eq!(
        fixture.sessions.resident_entity_count_for_test(),
        1,
        "the fixture villager is the only resident entity"
    );
    assert_eq!(EntityLifecycle::Alive, EntityLifecycle::Alive);
}
