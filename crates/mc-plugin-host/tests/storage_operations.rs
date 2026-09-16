//! The P3 storage-operation vertical: the atomic batch and the durable probe that
//! joins it, both driven by the real component through the real boundary.
//!
//! Every case here pins one property of the migration rather than the plumbing
//! under it:
//!
//! * a guest's batch becomes exactly the DTO the server applies - the request id
//!   it chose, the durable operation id it named and every mutation, with nothing
//!   invented, reordered or dropped in between;
//! * a package that never declared `storage_batches` is refused by *name*, both by
//!   the adapter's conversion and by the deployment that loses its route;
//! * a request past a bound the server's operation DTO already declares is refused,
//!   never clamped: the batch's `1..=MAX_INVENTORY_STORAGE_MUTATIONS` (16) distinct
//!   and non-repeating mutations, its `1..=MAX_PLUGIN_STORAGE_KEY_BYTES` (128) keys,
//!   its `1..=MAX_PLUGIN_STORAGE_VALUE_BYTES` (4096) values and its
//!   `MAX_SCRIPT_WORLD_TIME` expected revisions are enforced by `mc_script`'s own
//!   storage-mutation validator (`crates/mc-script/src/operations.rs`) when the host
//!   converts the record (`crates/mc-plugin-host/src/adapter.rs`);
//! * the server's own typed answer comes back as the contract's event, correlated
//!   by the request id the plugin chose and carrying the durable operation id a
//!   later `operation-status` addresses - and the probe the guest sends names the id
//!   *the answer* carried, so a guest that hardcoded one cannot pass;
//! * a refusal reports the server's own reason and the reasons stay
//!   distinguishable: a stale revision, a reused operation id and a lookup that
//!   found nothing are three different answers, not one "refused".

mod fixture;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fixture::component_bytes;
use mc_plugin_host::bindings::exports::solaris::plugin::events::{
    Event, EventContext, PlayerJoined,
};
use mc_plugin_host::bindings::exports::solaris::plugin::lifecycle::InitContext;
use mc_plugin_host::bindings::solaris::plugin::commands::{
    Command, OperationStatus, StorageBatchCas,
};
use mc_plugin_host::bindings::solaris::plugin::storage::{
    StorageCasMutation, StorageDeleteMutation, StorageMutation,
};
use mc_plugin_host::{
    AdapterError, CommandBatch, DeploymentConfig, DiscoveryMode, HostQueues, HostServices,
    LoadedPackage, LogLevel, NoSessions, PlayerSessions, PluginInstance, PluginLimits,
    start_deployment, to_script_batch,
};
use mc_script::{
    COMPONENT_PLUGIN_API_VERSION, CommandCapabilities, HostCommandAdmission,
    MAX_INVENTORY_STORAGE_MUTATIONS, MAX_PLUGIN_STORAGE_KEY_BYTES, MAX_PLUGIN_STORAGE_VALUE_BYTES,
    MAX_SCRIPT_ID_BYTES, MAX_SCRIPT_WORLD_TIME, ScriptCommand, ScriptEvent, ScriptOperation,
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptPlayerContext, ScriptPlayerId, ScriptPluginManifest, ScriptStorageChange,
    ScriptStorageMutation,
};

/// The one player every case knows, and the session their connection holds.
const PLAYER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const SESSION: u64 = 7;

/// The declaration an operator writes to grant the storage batch, in the
/// manifest's own capability vocabulary. The same name is what the server's
/// feature check requires in `required_features`.
const STORAGE_BATCH_CAPABILITY: &str = "capabilities = [\"storage_batches\"]\n";

/// The request id the fixture's batch mode uses, and the durable operation id it
/// commits under. Both are the guest's own, so a test reads them back from what
/// the host produced rather than from what the guest was told to send.
const BATCH_REQUEST: &str = "batch-1";
const BATCH_OPERATION: &str = "op-1";

/// The revision the test's server answer commits at, and the outcome it returns
/// for a batch that wrote one key and removed another.
const COMMITTED_REVISION: u64 = 41;

/// The services the fixture plugin is given: it has an id, and it logs.
#[derive(Default)]
struct Services {
    id: String,
}

impl HostServices for Services {
    fn log(&mut self, _level: LogLevel, _message: &str) {}

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// A resolver that knows exactly one player, as a live server would.
struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == PLAYER_UUID).then_some(SESSION)
    }
}

/// Instantiate the shared fixture with `config` as its package's `config.toml`.
fn guest(config: &str) -> PluginInstance<Services> {
    let limits = PluginLimits::default();
    let bytes = component_bytes();
    let engine = mc_plugin_host::engine(&limits).expect("engine");
    let compiled = mc_plugin_host::CompiledPlugin::compile(&engine, &bytes, &limits, "0.7.0")
        .expect("compile");
    let linker = mc_plugin_host::linker::<Services>(&engine).expect("linker");
    let mut instance = PluginInstance::instantiate(
        &linker,
        compiled.component(),
        Services {
            id: "hello".to_owned(),
        },
        limits,
    )
    .expect("instantiate");
    instance.configure(config).expect("configure");
    instance
        .init(
            config,
            InitContext {
                plugin_id: "hello".to_owned(),
                api_version: "0.7.0".to_owned(),
                world_fingerprint: String::new(),
            },
        )
        .expect("init");
    instance
}

/// The join one guest callback answers, in the contract's own shape.
fn join_event(session: u64) -> Event {
    Event::PlayerJoined(PlayerJoined {
        player: PLAYER_UUID.to_owned(),
        session,
        name: "Ada".to_owned(),
    })
}

/// The grants of a package that declares exactly these capabilities, by the
/// names a manifest writes.
fn grants(declarations: &[&str]) -> CommandCapabilities {
    let mut manifest =
        ScriptPluginManifest::new("hello", "Hello", "0.1.0", COMPONENT_PLUGIN_API_VERSION);
    for declaration in declarations {
        manifest = manifest
            .declare_capability(declaration)
            .expect("a capability the contract's vocabulary knows");
    }
    let validated = manifest
        .validate_for(COMPONENT_PLUGIN_API_VERSION)
        .expect("a component manifest validates");
    HostCommandAdmission::from_manifest(&validated)
        .capabilities()
        .clone()
}

/// One batch as the contract's own record spells it.
fn batch(request: &str, operation_id: &str, mutations: Vec<StorageMutation>) -> Command {
    Command::StorageBatchCas(StorageBatchCas {
        request: request.to_owned(),
        operation_id: operation_id.to_owned(),
        mutations,
    })
}

/// One probe as the contract's own record spells it.
fn probe(request: &str, operation_id: &str) -> Command {
    Command::OperationStatus(OperationStatus {
        request: request.to_owned(),
        operation_id: operation_id.to_owned(),
    })
}

/// One compare-and-swap mutation of a batch.
fn cas(key: &str, expected_version: Option<u64>, value: &str) -> StorageMutation {
    StorageMutation::Cas(StorageCasMutation {
        key: key.to_owned(),
        expected_version,
        value: value.to_owned(),
    })
}

/// One deletion mutation of a batch.
fn delete(key: &str, expected_version: Option<u64>) -> StorageMutation {
    StorageMutation::Delete(StorageDeleteMutation {
        key: key.to_owned(),
        expected_version,
    })
}

/// The mutations the fixture's batch mode builds when it is told to build `count`
/// of them: a compare-and-swap and a deletion per pair of zero-padded keys, so the
/// order the server canonicalizes by key is the order the guest built.
fn fixture_mutations(count: usize) -> Vec<ScriptStorageMutation> {
    (0..count)
        .map(|index| {
            let key = format!("key-{index:03}");
            if index % 2 == 0 {
                ScriptStorageMutation::compare_and_swap(key, None, format!("value-{index:03}"))
                    .expect("a bounded key and value")
            } else {
                ScriptStorageMutation::delete(key, Some(index as u64)).expect("a bounded key")
            }
        })
        .collect()
}

/// The DTO the server applies for the fixture's own batch, built through the DTO's
/// own public constructors rather than through the host's conversion.
fn fixture_batch_dto() -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        BATCH_REQUEST,
        ScriptOperation::StorageBatch {
            operation_id: BATCH_OPERATION.to_owned(),
            mutations: fixture_mutations(4),
        },
    )
    .expect("the fixture's own batch is inside every bound")
}

/// Stage one contract command, the way a guest's callback stages its own.
fn stage(command: Command) -> CommandBatch {
    let mut batch = CommandBatch::new();
    batch
        .push(command, &PluginLimits::default())
        .expect("one command fits the staging bound");
    batch
}

/// Convert one staged batch with the grants named in `declarations`, and answer
/// the server's own DTO or the refusal.
fn convert(
    batch: CommandBatch,
    declarations: &[&str],
) -> Result<mc_script::CommandBatch, AdapterError> {
    to_script_batch(
        batch,
        NonZeroUsize::new(4).expect("non-zero"),
        // A batch names no player and needs no lookup; a resolver that knows
        // nobody is enough, and if the conversion had grown one it would refuse a
        // batch a live server accepts.
        &NoSessions,
        &grants(declarations),
    )
}

/// The one command one staged batch converted to, and its request.
fn single(batch: CommandBatch, declarations: &[&str]) -> ScriptOperationRequest {
    let converted = convert(batch, declarations).expect("the command converts");
    let [ScriptCommand::Operation { request }] = converted.commands() else {
        panic!("expected one operation, saw {:?}", converted.commands());
    };
    request.clone()
}

#[test]
fn a_guest_batch_becomes_the_dto_the_server_applies() {
    let mut plugin = guest("mode = \"storage-batch\"\ncount = 4\n");
    let batch = plugin
        .on_events(
            EventContext {
                tick: 42,
                first_sequence: 0,
                count: 1,
            },
            &[join_event(SESSION)],
        )
        .expect("the guest answers the join");
    let converted = convert(batch, &["storage_batches"]).expect("the guest's own batch converts");

    assert_eq!(
        converted.commands(),
        [ScriptCommand::Operation {
            request: fixture_batch_dto(),
        }],
        "the request id, the durable operation id and every mutation are the guest's own"
    );

    // The envelope is read the way a later `operation-status` addresses it: the
    // request id correlates the answer, and the operation id is the durable name.
    let [ScriptCommand::Operation { request }] = converted.commands() else {
        panic!("one operation");
    };
    assert_eq!(request.request_id(), BATCH_REQUEST);
    assert_eq!(request.operation_id(), Some(BATCH_OPERATION));
    assert_eq!(
        request.operation(),
        &ScriptOperation::StorageBatch {
            operation_id: BATCH_OPERATION.to_owned(),
            mutations: fixture_mutations(4),
        },
    );
}

#[test]
fn a_batch_the_manifest_does_not_grant_is_refused_by_name() {
    let mut plugin = guest("mode = \"storage-batch\"\ncount = 4\n");
    let batch = plugin
        .on_events(
            EventContext {
                tick: 42,
                first_sequence: 0,
                count: 1,
            },
            &[join_event(SESSION)],
        )
        .expect("the guest answers the join");
    // The very same batch the granted package converts above.
    let refusal = to_script_batch(
        batch,
        NonZeroUsize::new(4).expect("non-zero"),
        &NoSessions,
        &grants(&[]),
    )
    .expect_err("a package that never declared the batch cannot write storage");
    assert_eq!(
        refusal,
        AdapterError::PermissionDenied {
            capability: "storage_batches"
        },
        "the refusal names the capability the manifest would have declared"
    );
    assert!(
        refusal.to_string().contains("storage_batches"),
        "an operator reads the same name, saw {refusal}"
    );

    // The probe needs the same capability: it reads the outcome an operation id
    // was recorded under, which is the same authority as writing one.
    let refusal = convert(stage(probe("lookup", BATCH_OPERATION)), &[])
        .expect_err("a package that never declared the batch cannot probe one");
    assert_eq!(
        refusal,
        AdapterError::PermissionDenied {
            capability: "storage_batches"
        }
    );
}

#[test]
fn a_batch_past_a_contract_bound_is_refused_rather_than_clamped() {
    let long_id = "r".repeat(MAX_SCRIPT_ID_BYTES + 1);
    let long_key = "k".repeat(MAX_PLUGIN_STORAGE_KEY_BYTES + 1);
    let long_value = "v".repeat(MAX_PLUGIN_STORAGE_VALUE_BYTES + 1);
    let seventeen: Vec<StorageMutation> = (0..=MAX_INVENTORY_STORAGE_MUTATIONS)
        .map(|index| cas(&format!("key-{index:03}"), None, "value"))
        .collect();

    for (case, command) in [
        (
            "a batch with no mutations at all",
            batch(BATCH_REQUEST, BATCH_OPERATION, Vec::new()),
        ),
        (
            "a batch past the DTO's own mutation bound",
            batch(BATCH_REQUEST, BATCH_OPERATION, seventeen),
        ),
        (
            "a key past the contract's key bound",
            batch(
                BATCH_REQUEST,
                BATCH_OPERATION,
                vec![cas(&long_key, None, "value")],
            ),
        ),
        (
            "an empty key",
            batch(BATCH_REQUEST, BATCH_OPERATION, vec![cas("", None, "value")]),
        ),
        (
            "a value past the contract's value bound",
            batch(
                BATCH_REQUEST,
                BATCH_OPERATION,
                vec![cas("coins", None, &long_value)],
            ),
        ),
        (
            "a key repeated inside one batch",
            batch(
                BATCH_REQUEST,
                BATCH_OPERATION,
                vec![cas("coins", None, "3"), delete("coins", Some(1))],
            ),
        ),
        (
            "an expected revision past the server's world time",
            batch(
                BATCH_REQUEST,
                BATCH_OPERATION,
                vec![cas("coins", Some(MAX_SCRIPT_WORLD_TIME + 1), "3")],
            ),
        ),
        (
            "an over-long correlation id",
            batch(&long_id, BATCH_OPERATION, vec![cas("coins", None, "3")]),
        ),
        (
            "a correlation id the contract does not accept",
            batch("batch one", BATCH_OPERATION, vec![cas("coins", None, "3")]),
        ),
        (
            "an over-long durable operation id",
            batch(BATCH_REQUEST, &long_id, vec![cas("coins", None, "3")]),
        ),
        (
            "a durable operation id the contract does not accept",
            batch(BATCH_REQUEST, "op one", vec![cas("coins", None, "3")]),
        ),
        (
            "an over-long id in a probe",
            probe(&long_id, BATCH_OPERATION),
        ),
        (
            "a probe whose operation id the contract does not accept",
            probe("lookup", "op one"),
        ),
    ] {
        let refusal = convert(stage(command), &["storage_batches"]).expect_err(
            "a value outside the contract's bound is the plugin's own malformed answer",
        );
        assert!(
            matches!(refusal, AdapterError::InvalidCommand { .. }),
            "{case} must be refused as an invalid command, saw {refusal:?}"
        );
    }

    // The bounds themselves are the DTO's, so a value at them is carried
    // unchanged: refused is not the same as moved to the edge, and a plugin that
    // stays inside the contract reads its own mutations back.
    let at_the_bound: Vec<StorageMutation> = (0..MAX_INVENTORY_STORAGE_MUTATIONS)
        .map(|index| {
            cas(
                &format!(
                    "{index:02}-{}",
                    "k".repeat(MAX_PLUGIN_STORAGE_KEY_BYTES - 3)
                ),
                Some(MAX_SCRIPT_WORLD_TIME),
                &"v".repeat(MAX_PLUGIN_STORAGE_VALUE_BYTES),
            )
        })
        .collect();
    let expected: Vec<ScriptStorageMutation> = (0..MAX_INVENTORY_STORAGE_MUTATIONS)
        .map(|index| {
            ScriptStorageMutation::compare_and_swap(
                format!(
                    "{index:02}-{}",
                    "k".repeat(MAX_PLUGIN_STORAGE_KEY_BYTES - 3)
                ),
                Some(MAX_SCRIPT_WORLD_TIME),
                "v".repeat(MAX_PLUGIN_STORAGE_VALUE_BYTES),
            )
            .expect("the contract's own bounds are inside themselves")
        })
        .collect();
    let request = single(
        stage(batch(
            &"r".repeat(MAX_SCRIPT_ID_BYTES),
            &"o".repeat(MAX_SCRIPT_ID_BYTES),
            at_the_bound,
        )),
        &["storage_batches"],
    );
    assert_eq!(
        request,
        ScriptOperationRequest::try_new(
            "r".repeat(MAX_SCRIPT_ID_BYTES),
            ScriptOperation::StorageBatch {
                operation_id: "o".repeat(MAX_SCRIPT_ID_BYTES),
                mutations: expected,
            },
        )
        .expect("a batch at every bound is admissible"),
        "a batch at every bound is carried unchanged"
    );

    let request = single(
        stage(probe(
            &"r".repeat(MAX_SCRIPT_ID_BYTES),
            &"o".repeat(MAX_SCRIPT_ID_BYTES),
        )),
        &["storage_batches"],
    );
    assert_eq!(request.request_id(), "r".repeat(MAX_SCRIPT_ID_BYTES));
    assert_eq!(
        request.operation_id(),
        Some("o".repeat(MAX_SCRIPT_ID_BYTES).as_str())
    );
}

/// One package directory of the fixture, with the manifest an operator writes.
fn write_package(root: &Path, id: &str, manifest_extra: &str, config: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n\
             events = [\"player.joined\", \"operation.result\"]\n\
             player_commands = [\"{id}\"]\n{manifest_extra}"
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), config).expect("config");
}

/// Discover the packages an operator placed under `root`.
fn deployment(root: &Path, ids: &[&str]) -> Vec<LoadedPackage> {
    let limits = PluginLimits::default();
    mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: ids.iter().map(|id| (*id).to_owned()).collect(),
            grants: BTreeMap::new(),
            require_grants: false,
        },
        &limits,
    )
    .expect("deployment")
    .into_packages()
}

/// The join a live server would hand the host, for one connection.
fn server_join(session: u64) -> ScriptEvent {
    let context = ScriptPlayerContext::try_new(PLAYER_UUID, "Ada", false, 0.0, 64.0, 0.0)
        .expect("player context");
    ScriptEvent::player_joined_with_context(ScriptPlayerId::new(session), context)
}

/// The chat line one admitted command carries, with the package that answered.
///
/// The provenance is the host's own: a guest cannot choose it, which is what makes
/// "who answered" a fact of the test rather than of the guest's story.
fn chat_answer(command: &ScriptCommand) -> (&str, &str) {
    let ScriptCommand::HostAttached {
        provenance,
        request,
    } = command
    else {
        panic!("expected an admitted command, saw {command:?}");
    };
    let ScriptCommand::SendChatMessage { message, .. } = request.as_ref() else {
        panic!("expected the guest's chat answer, saw {request:?}");
    };
    (provenance.plugin_id(), message)
}

/// The outcome the test's server returns for the fixture's batch: the two keys it
/// named, one written and one removed, all at one revision.
fn committed_outcome() -> ScriptOperationOutcome {
    ScriptOperationOutcome::committed(
        COMMITTED_REVISION,
        ScriptOperationPayload::StorageBatch {
            changes: vec![
                ScriptStorageChange::new("key-000".to_owned(), false),
                ScriptStorageChange::new("key-001".to_owned(), true),
            ],
        },
    )
    .expect("the server's own answer is consistent")
}

/// The line the fixture reports a committed answer with, for one request.
fn committed_line(request: &str) -> String {
    format!(
        "Settled {request} {BATCH_OPERATION} committed at {COMMITTED_REVISION} \
         key-000:written,key-001:deleted"
    )
}

#[tokio::test]
async fn the_servers_committed_answer_returns_to_the_guest_that_asked_and_the_probe_names_it() {
    // Three phases of the same operation: the guest asks for one batch, the
    // server's own typed answer comes back carrying the durable id, and the guest
    // probes *that* id with `operation-status`. The probe is named after the id it
    // received, so the case fails if the host drops, renames or invents the durable
    // id on the way to the guest.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        STORAGE_BATCH_CAPABILITY,
        "greeting = \"Settled\"\nmode = \"storage-batch\"\ncount = 2\n",
    );
    let limits = PluginLimits::default();
    let packages = deployment(root.path(), &["hello"]);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "the package starts registered, so the answers below are what it asked for"
    );

    boundary
        .try_enqueue_event(server_join(SESSION))
        .expect("event queued");
    let asked = boundary
        .recv_command()
        .await
        .expect("the batch reaches the boundary");
    let admitted = boundary
        .accept_host_command(asked)
        .expect("the server accepts what the host attached");
    let ScriptCommand::Operation { request } = admitted.request() else {
        panic!("expected the guest's batch, saw {:?}", admitted.request());
    };
    assert_eq!(
        request,
        &ScriptOperationRequest::try_new(
            BATCH_REQUEST,
            ScriptOperation::StorageBatch {
                operation_id: BATCH_OPERATION.to_owned(),
                mutations: fixture_mutations(2),
            },
        )
        .expect("the fixture's own batch"),
        "the admitted batch is the guest's own, mutation for mutation"
    );

    boundary
        .try_enqueue_event(
            admitted
                .operation_result(committed_outcome())
                .expect("an operation result answers an operation"),
        )
        .expect("result queued");
    let reported = boundary
        .recv_command()
        .await
        .expect("the guest reports the commit");
    assert_eq!(
        chat_answer(&reported),
        ("hello", committed_line(BATCH_REQUEST).as_str()),
        "the plugin reads the commit back with the ids and the keys it changed"
    );

    let probing = boundary
        .recv_command()
        .await
        .expect("the guest probes the operation it committed");
    let admitted = boundary
        .accept_host_command(probing)
        .expect("the server accepts the probe");
    let ScriptCommand::Operation { request } = admitted.request() else {
        panic!("expected the guest's probe, saw {:?}", admitted.request());
    };
    assert_eq!(
        request.request_id(),
        format!("status-{BATCH_OPERATION}"),
        "the probe's request id names the durable id the answer carried"
    );
    assert_eq!(
        request.operation(),
        &ScriptOperation::Status {
            operation_id: BATCH_OPERATION.to_owned(),
        },
        "the probe addresses the durable operation id, and nothing else"
    );

    // The server answers the probe with the outcome it recorded under that id.
    boundary
        .try_enqueue_event(
            admitted
                .operation_result(committed_outcome())
                .expect("an operation result answers an operation"),
        )
        .expect("result queued");
    let reported = boundary
        .recv_command()
        .await
        .expect("the guest reports the probe's answer");
    assert_eq!(
        chat_answer(&reported),
        (
            "hello",
            committed_line(&format!("status-{BATCH_OPERATION}")).as_str(),
        ),
        "the outcome came back under the same durable id, and the guest asked about that id"
    );

    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "asking and probing its own operation costs the instance nothing"
    );
    host.stop();
}

#[tokio::test]
async fn a_refusal_returns_the_servers_own_reason_and_the_reasons_stay_distinguishable() {
    // Three refusals of the same operation, each with a different reason from the
    // server's own vocabulary. The three reported lines are three different
    // strings: a mapping that folded them into one "refused" fails all but one.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        STORAGE_BATCH_CAPABILITY,
        "greeting = \"Settled\"\nmode = \"storage-batch\"\ncount = 2\n",
    );
    let limits = PluginLimits::default();
    let packages = deployment(root.path(), &["hello"]);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();

    for (case, session, failure, expected) in [
        (
            "a mutation that expected a revision a key no longer holds",
            SESSION,
            ScriptOperationFailure::StaleRevision,
            "Settled batch-1 op-1 refused stale-revision",
        ),
        (
            "an operation id already recorded for different content",
            SESSION + 1,
            ScriptOperationFailure::OperationConflict,
            "Settled batch-1 op-1 refused operation-conflict",
        ),
        (
            "a probe of an operation id the server never recorded",
            SESSION + 2,
            ScriptOperationFailure::NotFound,
            "Settled batch-1 op-1 refused not-found",
        ),
    ] {
        boundary
            .try_enqueue_event(server_join(session))
            .expect("event queued");
        let asked = boundary
            .recv_command()
            .await
            .expect("the batch reaches the boundary");
        let admitted = boundary
            .accept_host_command(asked)
            .expect("the server accepts what the host attached");
        boundary
            .try_enqueue_event(
                admitted
                    .operation_result(ScriptOperationOutcome::rejected(failure))
                    .expect("an operation result answers an operation"),
            )
            .expect("result queued");
        let reported = boundary
            .recv_command()
            .await
            .expect("the guest reports the refusal");
        assert_eq!(
            chat_answer(&reported),
            ("hello", expected),
            "{case} must reach the guest as the server's own reason"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(300), boundary.recv_command())
                .await
                .is_err(),
            "{case} still recorded no durable operation, so there is nothing to probe"
        );
    }

    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "a refusal the server made is not the plugin's misbehaviour"
    );
    host.stop();
}

#[tokio::test]
async fn a_package_that_never_declared_the_capability_loses_its_route_instead_of_writing() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        "",
        "greeting = \"Settled\"\nmode = \"storage-batch\"\ncount = 2\n",
    );
    let limits = PluginLimits::default();
    let packages = deployment(root.path(), &["hello"]);
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();
    assert_eq!(
        boundary.player_command_roots(),
        vec!["hello".to_owned()],
        "the package starts registered, so losing the route is the event under test"
    );

    boundary
        .try_enqueue_event(server_join(SESSION))
        .expect("event queued");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), boundary.recv_command())
            .await
            .is_err(),
        "an ungranted batch is refused before admission, not handed to the server"
    );
    assert!(
        boundary.player_command_roots().is_empty(),
        "a batch refused for a missing capability retires the instance that cannot be granted it"
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1, "one instance ran");
    assert_eq!(
        counters[0].1.commands_refused, 1,
        "the refusal is counted against the batch, not the instance's health"
    );
    assert_eq!(counters[0].1.commands_submitted, 0);
}
