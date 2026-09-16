//! Acceptance tests for the settlement runtime wiring (C2 runtime wave).
//!
//! These prove the deployed-package catalog is discovered, validated and turned
//! into a live runtime, that a violation fails startup loudly, that a world with
//! no settlement package keeps the typed `runtime_unavailable` answer, and that
//! one home POI can never bind two residents. The live-world commit path is
//! covered next to the storage ledger in
//! `script::storage::settlement_tests`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_data::{Identifier, items::ItemRegistry};
use mc_script::{
    PluginPackage, ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome,
    ScriptOperationPayload, ScriptOperationRequest, ScriptResidentSiteReservation,
    ScriptSettlementOperation, ScriptSettlementResult, ScriptSettlementSite, ScriptSitePoiKind,
    ScriptSitePoiState,
};
use mc_world::BlockRegistry;

use crate::play::SessionRegistry;
use crate::script::storage::PluginStorage;
use crate::script::storage::world_inventory::InventoryRuntime;
use crate::server::ShutdownHandle;
use crate::settlement::{
    SettlementDeployment, discover_settlement_deployment,
    discover_settlement_deployment_with_hashes,
};

const OWNER: &str = "solaris-settlements";
const SEED: i64 = 7;
const WORLD_IDENTITY: &str = "settlement-wiring-world";

fn registry() -> BlockRegistry {
    let stone = BlockReport {
        id: Identifier::parse("minecraft:stone").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    };
    let planks = BlockReport {
        id: Identifier::parse("minecraft:oak_planks").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 1,
            default: true,
            properties: BTreeMap::new(),
        }],
    };
    BlockRegistry::from_report(&[stone, planks]).unwrap()
}

/// One authored stage: its name and its `(local position, palette index)` cells.
type AuthoredStage<'a> = (&'a str, &'a [([i32; 3], u16)]);

fn blueprint_toml(
    owner: &str,
    name: &str,
    kind: &str,
    capacity: u16,
    stages: &[AuthoredStage<'_>],
) -> String {
    let mut text = format!(
        "id = \"{owner}:{name}\"\nrevision = 1\n\
         [footprint]\nsize = [4, 4, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {{}}\n\
         [[palette]]\nindex = 1\nblock = \"minecraft:oak_planks\"\nproperties = {{}}\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n\
         [[poi]]\nid = \"{kind}\"\nkind = \"{kind}\"\nat = [1, 1, 1]\ncapacity = {capacity}\n\
         [[street_connection]]\nat = [0, 0, 0]\nfacing = \"north\"\n"
    );
    for (stage, blocks) in stages {
        text.push_str(&format!("[[stage]]\nid = \"{stage}\"\n"));
        for (at, palette) in *blocks {
            text.push_str(&format!(
                "[[stage.blocks]]\nx = {}\ny = {}\nz = {}\npalette = {palette}\n",
                at[0], at[1], at[2]
            ));
        }
    }
    text
}

/// One authored catalog that can lay out any founding variant: a home blueprint
/// holding enough capacity for a town, plus the three required roles.
fn authored_catalog(owner: &str) -> Vec<(String, String)> {
    vec![
        (
            "house_small.toml".to_owned(),
            blueprint_toml(
                owner,
                "house_small",
                "home",
                32,
                &[
                    (
                        "foundation",
                        &[([0, 0, 0], 0), ([1, 0, 0], 0), ([2, 0, 0], 0)],
                    ),
                    ("walls", &[([1, 1, 1], 1), ([2, 1, 1], 1)]),
                ],
            ),
        ),
        (
            "town_hall.toml".to_owned(),
            blueprint_toml(
                owner,
                "town_hall",
                "meeting",
                0,
                &[("body", &[([0, 1, 0], 0)])],
            ),
        ),
        (
            "smithy.toml".to_owned(),
            blueprint_toml(owner, "smithy", "work", 0, &[("body", &[([0, 1, 0], 0)])]),
        ),
        (
            "watch_post.toml".to_owned(),
            blueprint_toml(
                owner,
                "watch_post",
                "guard",
                0,
                &[("body", &[([0, 1, 0], 0)])],
            ),
        ),
    ]
}

/// Deploy one package directory: `<dir>/structures/<file>.toml`.
fn deploy_package(dir: &Path, files: &[(String, String)]) {
    let structures = dir.join("structures");
    std::fs::create_dir_all(&structures).unwrap();
    for (name, text) in files {
        std::fs::write(structures.join(name), text).unwrap();
    }
}

fn package(dir: &Path, features: &[&str]) -> PluginPackage {
    PluginPackage::new(
        OWNER,
        dir,
        features
            .iter()
            .map(|feature| (*feature).to_owned())
            .collect(),
    )
}

fn settlement_features() -> Vec<&'static str> {
    vec!["world_sites", "structure_operations"]
}

fn deploy(dir: &Path, features: &[&str], files: &[(String, String)]) -> PluginPackage {
    deploy_package(dir, files);
    package(dir, features)
}

/// Flat ground for the deployment tests that only exercise site bookkeeping.
struct FlatGround;

impl mc_world::ChunkGenerator for FlatGround {
    fn generate(&self, _pos: mc_world::ChunkPos) -> mc_world::Chunk {
        panic!("the settlement tests never ask a generator for a chunk")
    }

    fn surface_height(&self, _world_x: i32, _world_z: i32) -> Option<i32> {
        Some(63)
    }
}

fn runtime_for(deployment: &SettlementDeployment) -> InventoryRuntime {
    InventoryRuntime::new(
        None,
        &ShutdownHandle::default(),
        Arc::new(SessionRegistry::new()),
        Arc::new(solaris_required_items()) as Arc<ItemRegistry>,
        Arc::new(solaris_required_item_facts()),
    )
    .with_settlement_runtime(Arc::new(deployment.runtime(
        SEED,
        WORLD_IDENTITY,
        [0, 0],
        Arc::new(FlatGround),
        None,
    )))
}

fn settlement(operation: ScriptSettlementOperation) -> ScriptOperationRequest {
    let fallback = format!("{operation:?}");
    ScriptOperationRequest::try_new("request", ScriptOperation::Settlement { operation })
        .unwrap_or_else(|error| panic!("valid settlement request {fallback}: {error}"))
}

fn settlement_payload(outcome: &ScriptOperationOutcome) -> &ScriptSettlementResult {
    match outcome.payload() {
        ScriptOperationPayload::Settlement { result } => result,
        other => panic!("expected a settlement payload, got {other:?}"),
    }
}

fn page_of(outcome: &ScriptOperationOutcome) -> Vec<ScriptSettlementSite> {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Sites { page } => page.sites.clone(),
        other => panic!("expected a site page, got {other:?}"),
    }
}

fn resident_site_of(outcome: &ScriptOperationOutcome) -> ScriptResidentSiteReservation {
    match settlement_payload(outcome) {
        ScriptSettlementResult::ResidentSite { reservation } => reservation.as_ref().clone(),
        other => panic!("expected a resident site reservation, got {other:?}"),
    }
}

fn site_of(outcome: &ScriptOperationOutcome) -> ScriptSettlementSite {
    match settlement_payload(outcome) {
        ScriptSettlementResult::Site { site } => site.as_ref().clone(),
        other => panic!("expected a site, got {other:?}"),
    }
}

#[tokio::test]
async fn deployed_package_yields_a_runtime_with_stable_site_ids() {
    let dir = tempfile::tempdir().unwrap();
    let deployed = deploy(dir.path(), &settlement_features(), &authored_catalog(OWNER));
    let deployment = discover_settlement_deployment(&[deployed], &registry())
        .expect("a valid catalog discovers")
        .expect("the package declares the settlement profile");
    assert_eq!(deployment.plugin_id(), OWNER);
    assert_ne!(deployment.profile_revision(), 0);
    let catalog_len = authored_catalog(OWNER).len();
    let runtime = runtime_for(&deployment);
    let mut storage = PluginStorage::open(dir.path()).unwrap();

    let list = settlement(ScriptSettlementOperation::ListSites {
        cursor: None,
        limit: 64,
    });
    let first = runtime
        .execute_settlement_operation(&mut storage, OWNER, &list)
        .await
        .unwrap();
    assert_eq!(first.failure(), None, "list: {first:?}");
    let first_ids = page_of(&first)
        .into_iter()
        .map(|site| site.site_id)
        .collect::<Vec<_>>();
    assert!(
        !first_ids.is_empty(),
        "the deterministic selector finds sites in the first window"
    );

    // A repeated call returns the same deterministic page.
    let second = runtime
        .execute_settlement_operation(&mut storage, OWNER, &list)
        .await
        .unwrap();
    let second_ids = page_of(&second)
        .into_iter()
        .map(|site| site.site_id)
        .collect::<Vec<_>>();
    assert_eq!(first_ids, second_ids);
    assert_eq!(catalog_len, 4, "the deployed catalog is whole");
}

#[test]
fn catalog_violations_fail_loudly_with_a_typed_error() {
    let registry = registry();

    // A non-blueprint entry in the authored directory is refused.
    let stray = tempfile::tempdir().unwrap();
    let deployed = deploy(
        stray.path(),
        &settlement_features(),
        &authored_catalog(OWNER),
    );
    std::fs::write(stray.path().join("structures/notes.txt"), "hello").unwrap();
    let error = discover_settlement_deployment(&[deployed], &registry).unwrap_err();
    assert!(
        error.to_string().contains("not a .toml blueprint"),
        "{error}"
    );

    // A blueprint id outside the package namespace is refused.
    let foreign = tempfile::tempdir().unwrap();
    let mut files = authored_catalog(OWNER);
    files[1].1 = blueprint_toml("other-plugin", "town_hall", "meeting", 0, &[]);
    let deployed = deploy(foreign.path(), &settlement_features(), &files);
    let error = discover_settlement_deployment(&[deployed], &registry).unwrap_err();
    assert!(error.to_string().contains("namespace"), "{error}");

    // A body outside the frozen footprint bound is refused.
    let oversized = tempfile::tempdir().unwrap();
    let mut files = authored_catalog(OWNER);
    files[0].1 = files[0].1.replace("size = [4, 4, 4]", "size = [65, 4, 4]");
    let deployed = deploy(oversized.path(), &settlement_features(), &files);
    let error = discover_settlement_deployment(&[deployed], &registry).unwrap_err();
    assert!(error.to_string().contains("exceeds 64"), "{error}");

    // A catalog that provides no blueprint for a required role fails startup, so
    // a settlement can never start and then fail every request as unavailable.
    let roleless = tempfile::tempdir().unwrap();
    let mut files = authored_catalog(OWNER);
    let before = files.len();
    files.retain(|(_, text)| !text.contains("\"guard\""));
    assert_eq!(
        files.len(),
        before - 1,
        "the authored catalog ships one guard blueprint"
    );
    let deployed = deploy(roleless.path(), &settlement_features(), &files);
    let error = discover_settlement_deployment(&[deployed], &registry).unwrap_err();
    assert!(
        error.to_string().contains("provides no guard blueprint"),
        "{error}"
    );

    // A deployment-recorded hash that does not match the derived one is refused.
    let hashed = tempfile::tempdir().unwrap();
    let deployed = deploy(
        hashed.path(),
        &settlement_features(),
        &authored_catalog(OWNER),
    );
    let expected = BTreeMap::from([(
        format!("{OWNER}:house_small"),
        "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
    )]);
    let error =
        discover_settlement_deployment_with_hashes(&[deployed], &registry, &expected).unwrap_err();
    assert!(error.to_string().contains("content hash"), "{error}");

    // Two packages claiming the profile, and a claim with no catalog at all.
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let one = deploy(
        first.path(),
        &settlement_features(),
        &authored_catalog(OWNER),
    );
    let two = deploy(
        second.path(),
        &settlement_features(),
        &authored_catalog(OWNER),
    );
    let error = discover_settlement_deployment(&[one, two], &registry).unwrap_err();
    assert!(error.to_string().contains("at most one"), "{error}");

    let empty = tempfile::tempdir().unwrap();
    let claimed = package(empty.path(), &settlement_features());
    let error = discover_settlement_deployment(&[claimed], &registry).unwrap_err();
    assert!(error.to_string().contains("no authored catalog"), "{error}");
}

#[tokio::test]
async fn no_settlement_package_keeps_the_unavailable_answer() {
    let registry = registry();
    assert!(
        discover_settlement_deployment(&[], &registry)
            .unwrap()
            .is_none(),
        "an empty deployed set owns no settlement profile"
    );

    // A package that does not declare both features is not the profile owner.
    let dir = tempfile::tempdir().unwrap();
    let unrelated = deploy(dir.path(), &["storage"], &authored_catalog(OWNER));
    assert!(
        discover_settlement_deployment(&[unrelated], &registry)
            .unwrap()
            .is_none()
    );

    // Without a discovered deployment the runtime is never installed, so every
    // settlement call answers the typed unavailable failure and nothing panics.
    let runtime = InventoryRuntime::new(
        None,
        &ShutdownHandle::default(),
        Arc::new(SessionRegistry::new()),
        Arc::new(solaris_required_items()) as Arc<ItemRegistry>,
        Arc::new(solaris_required_item_facts()),
    );
    let mut storage = PluginStorage::open(dir.path()).unwrap();
    for request in [
        settlement(ScriptSettlementOperation::ListSites {
            cursor: None,
            limit: 8,
        }),
        settlement(ScriptSettlementOperation::Status {
            structure_id: "missing".to_owned(),
        }),
    ] {
        let outcome = runtime
            .execute_settlement_operation(&mut storage, OWNER, &request)
            .await
            .unwrap();
        assert_eq!(
            outcome.failure(),
            Some(ScriptOperationFailure::RuntimeUnavailable),
            "{request:?}"
        );
    }
}

#[tokio::test]
async fn one_home_poi_binds_at_most_one_resident_site() {
    let dir = tempfile::tempdir().unwrap();
    let deployed = deploy(dir.path(), &settlement_features(), &authored_catalog(OWNER));
    let deployment = discover_settlement_deployment(&[deployed], &registry())
        .unwrap()
        .unwrap();
    let runtime = runtime_for(&deployment);
    let mut storage = PluginStorage::open(dir.path()).unwrap();

    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ListSites {
                cursor: None,
                limit: 64,
            }),
        )
        .await
        .unwrap();
    let site = page_of(&outcome)
        .into_iter()
        .find(|site| {
            site.pois.iter().any(|poi| {
                poi.kind == ScriptSitePoiKind::Home && poi.state == ScriptSitePoiState::Free
            })
        })
        .expect("the deterministic layout exposes a free home POI");
    let home = site
        .pois
        .iter()
        .find(|poi| poi.kind == ScriptSitePoiKind::Home)
        .expect("a home POI exists")
        .poi_id
        .clone();
    let revision = site.revision;

    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ReserveResidentSite {
                operation_id: "reserve-1".to_owned(),
                site_id: site.site_id.clone(),
                poi_id: home.clone(),
                expected_site_revision: revision,
            }),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "first reservation: {outcome:?}");
    let first = resident_site_of(&outcome);
    assert!(!first.spawn_site_token.is_empty());

    // The durable site now shows one reserved POI at a moved revision.
    let query = settlement(ScriptSettlementOperation::QuerySite {
        site_id: site.site_id.clone(),
        cursor: None,
        limit: 64,
    });
    let queried = site_of(
        &runtime
            .execute_settlement_operation(&mut storage, OWNER, &query)
            .await
            .unwrap(),
    );
    let reserved = queried
        .pois
        .iter()
        .filter(|poi| poi.state == ScriptSitePoiState::Reserved)
        .count();
    assert_eq!(reserved, 1, "the home is reserved exactly once");
    assert_ne!(queried.revision, revision, "the site advanced");

    // The same POI cannot bind a second resident.
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ReserveResidentSite {
                operation_id: "reserve-2".to_owned(),
                site_id: site.site_id.clone(),
                poi_id: home.clone(),
                expected_site_revision: queried.revision,
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::Blocked),
        "{outcome:?}"
    );

    // Re-opening the settlement ledger keeps the single binding: the home stays
    // reserved and the same POI still cannot take a second resident.
    drop(storage);
    let mut storage = PluginStorage::open(dir.path()).unwrap();
    let replayed = site_of(
        &runtime
            .execute_settlement_operation(&mut storage, OWNER, &query)
            .await
            .unwrap(),
    );
    assert_eq!(
        replayed
            .pois
            .iter()
            .filter(|poi| poi.state == ScriptSitePoiState::Reserved)
            .count(),
        1,
        "re-opening the ledger mints no second inhabitant"
    );
    assert_eq!(replayed.revision, queried.revision);
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ReserveResidentSite {
                operation_id: "reserve-replayed".to_owned(),
                site_id: site.site_id.clone(),
                poi_id: home.clone(),
                expected_site_revision: replayed.revision,
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::Blocked),
        "{outcome:?}"
    );

    // Releasing hands the slot back, and it can be reserved again.
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ReleaseResidentSite {
                operation_id: "release-1".to_owned(),
                spawn_site_token: first.spawn_site_token.clone(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "release: {outcome:?}");
    let released = site_of(
        &runtime
            .execute_settlement_operation(&mut storage, OWNER, &query)
            .await
            .unwrap(),
    );
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            OWNER,
            &settlement(ScriptSettlementOperation::ReserveResidentSite {
                operation_id: "reserve-3".to_owned(),
                site_id: site.site_id.clone(),
                poi_id: home,
                expected_site_revision: released.revision,
            }),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "re-reservation: {outcome:?}");
}

/// The vanilla block report the shipped catalog is validated against.
///
/// The search order is the server's own (`crates/mc-server/src/content_cache.rs`,
/// `ContentSearch::candidates`): an explicit `SOLARIS_CONTENT_CACHE` first — an
/// operator who names a cache does not want another standing in for it — then the
/// standard managed cache (`$XDG_DATA_HOME`/`~/.local/share` +
/// `solaris/content/<release>`), then the workspace sidecar `data/vanilla`. This
/// helper only looks for `reports/blocks.json`, so unlike the server it does not
/// re-run `validate_content_cache`'s completeness check; a machine with no cache
/// is told exactly that instead of failing on a missing path.
fn vanilla_blocks_report(repository: &Path) -> Vec<BlockReport> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("SOLARIS_CONTENT_CACHE") {
        roots.push(std::path::PathBuf::from(dir));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        })
    {
        roots.push(
            data_home
                .join("solaris/content")
                .join(mc_protocol::TARGET_RELEASE),
        );
    }
    roots.push(repository.join("data/vanilla"));
    let Some(path) = roots
        .iter()
        .map(|root| root.join("reports/blocks.json"))
        .find(|path| path.is_file())
    else {
        panic!(
            "no vanilla block report to validate the shipped catalog against; checked {roots:?} \
             (run `mc-server content import` or tools/extract-vanilla-data.sh, or set \
             SOLARIS_CONTENT_CACHE)"
        );
    };
    mc_data::blocks::load_blocks_report(path).expect("the vanilla block report parses")
}

/// The shipped first-party package must pass the frozen loader unchanged.
///
/// Skipped only when the sibling plugin checkout is absent: the core repository
/// never compiles authored data, so a missing sibling is not a failure. When it
/// is present this pins the real `structures/*.toml` contract, including the
/// `solaris:` content namespace of a `solaris-settlements` package.
#[tokio::test]
async fn shipped_settlement_package_is_accepted() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let package_dir = repository.join("../solaris-default-plugins/solaris-settlements");
    if !package_dir.is_dir() {
        return;
    }
    let report = vanilla_blocks_report(&repository);
    let registry = BlockRegistry::from_report(&report).unwrap();
    let deployed = PluginPackage::new(
        "solaris-settlements",
        package_dir,
        vec!["world_sites".to_owned(), "structure_operations".to_owned()],
    );
    let deployment = discover_settlement_deployment(&[deployed], &registry)
        .expect("the shipped catalog validates")
        .expect("the shipped package declares the settlement profile");
    assert_eq!(deployment.plugin_id(), "solaris-settlements");

    let runtime = runtime_for(&deployment);
    let dir = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(dir.path()).unwrap();
    let outcome = runtime
        .execute_settlement_operation(
            &mut storage,
            "solaris-settlements",
            &settlement(ScriptSettlementOperation::ListSites {
                cursor: None,
                limit: 64,
            }),
        )
        .await
        .unwrap();
    assert_eq!(outcome.failure(), None, "list: {outcome:?}");
    assert!(
        !page_of(&outcome).is_empty(),
        "the shipped catalog lays out at least one site"
    );
}
