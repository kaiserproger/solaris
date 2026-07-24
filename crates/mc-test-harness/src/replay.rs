use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::parity::{
    CoreAction, ObservationSet, ScenarioContext, ServerKind, observe_core_action_sequence,
};

pub const REPLAY_SCENARIO_SCHEMA: &str = "solaris.core_replay.scenario.v1";
pub const REPLAY_RESULT_SCHEMA: &str = "solaris.core_replay.result.v1";
pub const CORE_GATE_MANIFEST_SCHEMA: &str = "solaris.core_gate.manifest.v1";
pub const BLOCK_TRANSACTION_ORACLE_SCHEMA: &str = "solaris.block_transaction.oracle.v1";

const MAX_REPLAY_ACTIONS: usize = 10_000;
const MAX_REPLAY_CHECKS: usize = 128;
const MAX_CONCURRENT_GROUP_REPETITIONS: u16 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockTransactionOracleCase {
    AcceptedBreak,
    AcceptedPlace,
    OccupiedPlaceRejection,
    OutOfReachBreakRejection,
    EarlyStopBreakRejection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOraclePhase {
    pub id: String,
    pub case: BlockTransactionOracleCase,
    pub sequence: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOracleManifest {
    pub schema: String,
    pub id: String,
    pub phases: Vec<BlockTransactionOraclePhase>,
}

impl BlockTransactionOracleManifest {
    pub fn from_json(input: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(input).context("parse block transaction oracle manifest JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .context("serialize block transaction oracle manifest JSON")
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == BLOCK_TRANSACTION_ORACLE_SCHEMA,
            "unsupported block transaction oracle schema: {}",
            self.schema
        );
        validate_identifier("block transaction oracle id", &self.id)?;
        ensure!(
            self.phases.len() == 5,
            "block transaction oracle must declare exactly five focused phases"
        );
        let mut ids = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        let mut cases = BTreeSet::new();
        for phase in &self.phases {
            validate_identifier("block transaction phase id", &phase.id)?;
            ensure!(
                ids.insert(phase.id.as_str()),
                "duplicate block transaction phase id: {}",
                phase.id
            );
            ensure!(
                phase.sequence > 0,
                "block transaction phase {} has non-positive sequence",
                phase.id
            );
            ensure!(
                sequences.insert(phase.sequence),
                "duplicate block transaction sequence: {}",
                phase.sequence
            );
            ensure!(
                cases.insert(phase.case),
                "duplicate block transaction oracle case: {:?}",
                phase.case
            );
        }
        let required = BTreeSet::from([
            BlockTransactionOracleCase::AcceptedBreak,
            BlockTransactionOracleCase::AcceptedPlace,
            BlockTransactionOracleCase::OccupiedPlaceRejection,
            BlockTransactionOracleCase::OutOfReachBreakRejection,
            BlockTransactionOracleCase::EarlyStopBreakRejection,
        ]);
        ensure!(
            cases == required,
            "block transaction oracle phases do not cover the required case matrix"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockTransactionOracleEvent {
    TargetUpdate { state_id: i32 },
    Ack { sequence: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOracleTrace {
    pub manifest_id: String,
    pub phases: Vec<BlockTransactionOraclePhaseTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTransactionOraclePhaseTrace {
    pub id: String,
    pub events: Vec<BlockTransactionOracleEvent>,
}

impl BlockTransactionOracleTrace {
    pub fn validate_against(&self, manifest: &BlockTransactionOracleManifest) -> Result<()> {
        manifest.validate()?;
        ensure!(
            self.manifest_id == manifest.id,
            "block transaction trace manifest id {} does not match {}",
            self.manifest_id,
            manifest.id
        );
        ensure!(
            self.phases.len() == manifest.phases.len(),
            "block transaction trace phase count does not match manifest"
        );
        for (expected, actual) in manifest.phases.iter().zip(&self.phases) {
            ensure!(
                actual.id == expected.id,
                "block transaction trace phase order/id mismatch: expected {}, got {}",
                expected.id,
                actual.id
            );
            ensure!(
                !actual.events.is_empty(),
                "block transaction trace phase {} has no events",
                actual.id
            );
            let ack_index = actual
                .events
                .iter()
                .position(|event| matches!(event, BlockTransactionOracleEvent::Ack { sequence } if *sequence == expected.sequence))
                .with_context(|| format!("block transaction phase {} has no matching ack", actual.id))?;
            if matches!(
                expected.case,
                BlockTransactionOracleCase::OccupiedPlaceRejection
            ) {
                ensure!(
                    actual.events[..ack_index].iter().any(|event| matches!(
                        event,
                        BlockTransactionOracleEvent::TargetUpdate { .. }
                    )),
                    "rejected block transaction phase {} has no authoritative resync before ack",
                    actual.id
                );
            }
            ensure!(
                actual.events[ack_index + 1..]
                    .iter()
                    .all(|event| !matches!(event, BlockTransactionOracleEvent::Ack { sequence } if *sequence == expected.sequence)),
                "block transaction phase {} repeats its ack",
                actual.id
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayDriver {
    SolarisProtocol,
    VanillaOracle,
    RealClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayEvidenceKind {
    Unit,
    Harness,
    Oracle,
    RealClient,
    Performance,
    Soak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayCheckStatus {
    Passed,
    Failed,
    Degraded,
    Blocked,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayOutcome {
    Passed,
    Failed,
    Degraded,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreGateEvidenceLeg {
    Unit,
    Wire,
    Oracle,
    RealClient,
    Performance,
    Persistence,
    ReplayNegative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreGateRowScope {
    Focused,
    Broad,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGatePhase {
    pub id: String,
    pub scenario_id: String,
    pub ledger_rows: Vec<String>,
    pub evidence_legs: BTreeSet<CoreGateEvidenceLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGateRow {
    pub row: String,
    pub scope: CoreGateRowScope,
    pub required_phases: Vec<String>,
    pub required_evidence_legs: BTreeSet<CoreGateEvidenceLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGatePhaseEvidence {
    pub phase_id: String,
    pub passed_evidence_legs: BTreeSet<CoreGateEvidenceLeg>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreGateManifest {
    pub schema: String,
    pub phases: Vec<CoreGatePhase>,
    pub rows: Vec<CoreGateRow>,
}

impl CoreGateManifest {
    pub fn from_json(input: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(input).context("parse core gate manifest JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == CORE_GATE_MANIFEST_SCHEMA,
            "unsupported core gate manifest schema: {}",
            self.schema
        );
        ensure!(
            !self.phases.is_empty(),
            "core gate manifest phases are empty"
        );
        ensure!(
            self.phases.len() <= MAX_REPLAY_CHECKS,
            "core gate manifest has too many phases"
        );
        ensure!(!self.rows.is_empty(), "core gate manifest rows are empty");
        ensure!(
            self.rows.len() <= MAX_REPLAY_CHECKS,
            "core gate manifest has too many rows"
        );

        let mut phases_by_id = BTreeMap::new();
        for phase in &self.phases {
            validate_identifier("core gate phase id", &phase.id)?;
            validate_identifier("core gate scenario id", &phase.scenario_id)?;
            ensure!(
                phases_by_id.insert(phase.id.as_str(), phase).is_none(),
                "duplicate core gate phase id: {}",
                phase.id
            );
            ensure!(
                !phase.ledger_rows.is_empty(),
                "core gate phase {} has no ledger rows",
                phase.id
            );
            ensure!(
                !phase.evidence_legs.is_empty(),
                "core gate phase {} has no evidence legs",
                phase.id
            );
            let mut phase_rows = BTreeSet::new();
            for row in &phase.ledger_rows {
                validate_ledger_row(row)?;
                ensure!(
                    phase_rows.insert(row.as_str()),
                    "core gate phase {} repeats ledger row {}",
                    phase.id,
                    row
                );
            }
        }

        let mut row_ids = BTreeSet::new();
        for row in &self.rows {
            validate_ledger_row(&row.row)?;
            ensure!(
                row_ids.insert(row.row.as_str()),
                "duplicate core gate ledger row: {}",
                row.row
            );
            ensure!(
                !row.required_phases.is_empty(),
                "core gate row {} has no required phases",
                row.row
            );
            if row.scope == CoreGateRowScope::Broad {
                ensure!(
                    row.required_phases.len() >= 2,
                    "broad core gate row {} requires at least two focused phases",
                    row.row
                );
            }
            ensure!(
                !row.required_evidence_legs.is_empty(),
                "core gate row {} has no required evidence legs",
                row.row
            );
            let mut required_phase_ids = BTreeSet::new();
            for phase_id in &row.required_phases {
                ensure!(
                    required_phase_ids.insert(phase_id.as_str()),
                    "core gate row {} repeats required phase {}",
                    row.row,
                    phase_id
                );
                let phase = phases_by_id.get(phase_id.as_str()).with_context(|| {
                    format!(
                        "core gate row {} requires unknown phase {phase_id}",
                        row.row
                    )
                })?;
                ensure!(
                    phase
                        .ledger_rows
                        .iter()
                        .any(|candidate| candidate == &row.row),
                    "core gate phase {} does not cover required row {}",
                    phase.id,
                    row.row
                );
            }
        }
        Ok(())
    }

    pub fn validate_completion(&self, completed: &[CoreGatePhaseEvidence]) -> Result<()> {
        self.validate()?;
        let phases_by_id = self
            .phases
            .iter()
            .map(|phase| (phase.id.as_str(), phase))
            .collect::<BTreeMap<_, _>>();
        let mut completed_by_id = BTreeMap::new();
        for evidence in completed {
            ensure!(
                phases_by_id.contains_key(evidence.phase_id.as_str()),
                "completion references unknown core gate phase {}",
                evidence.phase_id
            );
            ensure!(
                completed_by_id
                    .insert(evidence.phase_id.as_str(), evidence)
                    .is_none(),
                "duplicate core gate completion for phase {}",
                evidence.phase_id
            );
        }

        for row in &self.rows {
            let mut row_evidence = BTreeSet::new();
            for phase_id in &row.required_phases {
                let phase = phases_by_id
                    .get(phase_id.as_str())
                    .expect("validated core gate phase exists");
                let evidence = completed_by_id.get(phase_id.as_str()).with_context(|| {
                    format!(
                        "core gate row {} is missing focused phase {phase_id}",
                        row.row
                    )
                })?;
                ensure!(
                    evidence
                        .passed_evidence_legs
                        .is_superset(&phase.evidence_legs),
                    "core gate phase {} is missing declared evidence legs",
                    phase.id
                );
                row_evidence.extend(evidence.passed_evidence_legs.iter().copied());
            }
            ensure!(
                row_evidence.is_superset(&row.required_evidence_legs),
                "core gate row {} is missing required evidence legs",
                row.row
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayGateRequirement {
    pub id: String,
    pub evidence_kind: ReplayEvidenceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayLane {
    pub driver: ReplayDriver,
    pub required_gates: Vec<ReplayGateRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayExpectedInvariant {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayConcurrentFixture {
    SameTargetPlacement,
    SharedChest { item: String, initial_count: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayConcurrentAction {
    PlaceBlock { actor: String, item: String },
    ChestPickup { actor: String, slot: u8 },
}

impl ReplayConcurrentAction {
    fn actor(&self) -> &str {
        match self {
            Self::PlaceBlock { actor, .. } | Self::ChestPickup { actor, .. } => actor,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayConcurrentGroup {
    pub id: String,
    pub repetitions: u16,
    pub fixture: ReplayConcurrentFixture,
    pub actions: Vec<ReplayConcurrentAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStateValue {
    pub key: String,
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStateExpectation {
    pub id: String,
    pub after_group: String,
    pub invariant_id: String,
    pub values: Vec<ReplayStateValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayStateObservation {
    pub id: String,
    pub values: Vec<ReplayStateValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayScenarioManifest {
    pub schema: String,
    pub id: String,
    pub seed: u64,
    pub actions: Vec<CoreAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concurrent_groups: Vec<ReplayConcurrentGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_expectations: Vec<ReplayStateExpectation>,
    pub lanes: Vec<ReplayLane>,
    pub expected_invariants: Vec<ReplayExpectedInvariant>,
}

impl ReplayScenarioManifest {
    pub fn from_json(input: &str) -> Result<Self> {
        let manifest: Self =
            serde_json::from_str(input).context("parse core replay scenario JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self).context("serialize core replay scenario JSON")
    }

    pub fn minimal_concurrent_failure(&self, group_id: &str) -> Result<Self> {
        self.validate()?;
        let group = self
            .concurrent_groups
            .iter()
            .find(|group| group.id == group_id)
            .with_context(|| format!("unknown failing concurrent group: {group_id}"))?
            .clone();
        let state_expectations = self
            .state_expectations
            .iter()
            .filter(|expectation| expectation.after_group == group_id)
            .cloned()
            .collect::<Vec<_>>();
        ensure!(
            !state_expectations.is_empty(),
            "failing concurrent group {group_id} has no state expectation"
        );
        let invariant_ids = state_expectations
            .iter()
            .map(|expectation| expectation.invariant_id.as_str())
            .collect::<BTreeSet<_>>();
        let expected_invariants = self
            .expected_invariants
            .iter()
            .filter(|invariant| invariant_ids.contains(invariant.id.as_str()))
            .cloned()
            .collect();
        let failure = Self {
            schema: self.schema.clone(),
            id: self.id.clone(),
            seed: self.seed,
            actions: self.actions.clone(),
            concurrent_groups: vec![group],
            state_expectations,
            lanes: self.lanes.clone(),
            expected_invariants,
        };
        failure.validate()?;
        Ok(failure)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == REPLAY_SCENARIO_SCHEMA,
            "unsupported replay scenario schema: {}",
            self.schema
        );
        validate_identifier("scenario id", &self.id)?;
        ensure!(
            !self.actions.is_empty() || !self.concurrent_groups.is_empty(),
            "replay scenario has no serial actions or concurrent groups"
        );
        ensure!(
            self.actions.len()
                + self
                    .concurrent_groups
                    .iter()
                    .map(|group| group.actions.len() * usize::from(group.repetitions))
                    .sum::<usize>()
                <= MAX_REPLAY_ACTIONS,
            "replay scenario expands beyond {MAX_REPLAY_ACTIONS} actions"
        );
        for action in &self.actions {
            validate_action(action)?;
        }

        let group_ids = validate_concurrent_groups(&self.concurrent_groups)?;

        ensure!(!self.lanes.is_empty(), "replay scenario lanes are empty");
        let mut drivers = BTreeSet::new();
        for lane in &self.lanes {
            ensure!(
                drivers.insert(lane.driver),
                "duplicate replay lane for {:?}",
                lane.driver
            );
            ensure!(
                !lane.required_gates.is_empty(),
                "replay lane {:?} has no required gates",
                lane.driver
            );
            ensure!(
                lane.required_gates.len() <= MAX_REPLAY_CHECKS,
                "replay lane {:?} has too many required gates",
                lane.driver
            );
            let mut gate_ids = BTreeSet::new();
            for gate in &lane.required_gates {
                validate_identifier("required gate id", &gate.id)?;
                ensure!(
                    gate_ids.insert(gate.id.as_str()),
                    "duplicate required gate id in {:?}: {}",
                    lane.driver,
                    gate.id
                );
            }
            let primary_evidence = match lane.driver {
                ReplayDriver::SolarisProtocol => ReplayEvidenceKind::Harness,
                ReplayDriver::VanillaOracle => ReplayEvidenceKind::Oracle,
                ReplayDriver::RealClient => ReplayEvidenceKind::RealClient,
            };
            ensure!(
                lane.required_gates
                    .iter()
                    .any(|gate| gate.evidence_kind == primary_evidence),
                "replay lane {:?} does not require {:?} evidence",
                lane.driver,
                primary_evidence
            );
        }

        ensure!(
            !self.expected_invariants.is_empty(),
            "replay scenario expected invariants are empty"
        );
        ensure!(
            self.expected_invariants.len() <= MAX_REPLAY_CHECKS,
            "replay scenario has too many expected invariants"
        );
        let mut invariant_ids = BTreeSet::new();
        for invariant in &self.expected_invariants {
            validate_identifier("expected invariant id", &invariant.id)?;
            validate_non_empty("expected invariant description", &invariant.description)?;
            ensure!(
                invariant_ids.insert(invariant.id.as_str()),
                "duplicate expected invariant id: {}",
                invariant.id
            );
        }

        if self.concurrent_groups.is_empty() {
            ensure!(
                self.state_expectations.is_empty(),
                "state expectations require concurrent groups"
            );
        } else {
            ensure!(
                !self.state_expectations.is_empty(),
                "concurrent replay scenario has no state expectations"
            );
        }
        ensure!(
            self.state_expectations.len() <= MAX_REPLAY_CHECKS,
            "replay scenario has too many state expectations"
        );
        let mut expectation_ids = BTreeSet::new();
        let mut expectation_invariants = BTreeSet::new();
        let mut covered_groups = BTreeSet::new();
        for expectation in &self.state_expectations {
            validate_identifier("state expectation id", &expectation.id)?;
            ensure!(
                expectation_ids.insert(expectation.id.as_str()),
                "duplicate state expectation id: {}",
                expectation.id
            );
            validate_identifier("state expectation group id", &expectation.after_group)?;
            ensure!(
                group_ids.contains(expectation.after_group.as_str()),
                "state expectation {} references unknown group {}",
                expectation.id,
                expectation.after_group
            );
            covered_groups.insert(expectation.after_group.as_str());
            validate_identifier("state expectation invariant id", &expectation.invariant_id)?;
            ensure!(
                invariant_ids.contains(expectation.invariant_id.as_str()),
                "state expectation {} references unknown invariant {}",
                expectation.id,
                expectation.invariant_id
            );
            ensure!(
                expectation_invariants.insert(expectation.invariant_id.as_str()),
                "state invariant {} is reused by multiple expectations",
                expectation.invariant_id
            );
            ensure!(
                !expectation.values.is_empty(),
                "state expectation {} has no values",
                expectation.id
            );
            ensure!(
                expectation.values.len() <= MAX_REPLAY_CHECKS,
                "state expectation {} has too many values",
                expectation.id
            );
            let mut value_keys = BTreeSet::new();
            for value in &expectation.values {
                validate_identifier("state expectation value key", &value.key)?;
                ensure!(
                    value_keys.insert(value.key.as_str()),
                    "state expectation {} repeats value key {}",
                    expectation.id,
                    value.key
                );
            }
        }
        ensure!(
            covered_groups.len() == group_ids.len(),
            "every concurrent group must have a state expectation"
        );
        Ok(())
    }
}

pub async fn run_protocol_replay(
    manifest: &ReplayScenarioManifest,
    context: ScenarioContext,
) -> Result<ObservationSet> {
    manifest.validate()?;
    let (driver, lane_name) = match context.kind {
        ServerKind::Solaris => (ReplayDriver::SolarisProtocol, "solaris protocol lane"),
        ServerKind::Vanilla => (ReplayDriver::VanillaOracle, "vanilla oracle lane"),
    };
    ensure!(
        manifest.lanes.iter().any(|lane| lane.driver == driver),
        "replay scenario {} has no {lane_name}",
        manifest.id
    );
    ensure!(
        manifest.concurrent_groups.is_empty(),
        "single-client protocol replay adapter does not support concurrent groups"
    );
    ensure!(
        !manifest
            .actions
            .iter()
            .any(|action| matches!(action, CoreAction::Reconnect)),
        "protocol replay adapter does not support reconnect actions"
    );
    observe_core_action_sequence(context, &manifest.id, &manifest.actions).await
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayHardwareProvenance {
    pub os: String,
    pub arch: String,
    pub cpu_model: String,
    pub logical_cpus: usize,
    pub memory_mib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayProvenance {
    pub git_commit: String,
    pub config_sha256: String,
    pub build_profile: String,
    pub sidecar_version: String,
    pub hardware: ReplayHardwareProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayGateResult {
    pub id: String,
    pub evidence_kind: ReplayEvidenceKind,
    pub status: ReplayCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayInvariantResult {
    pub id: String,
    pub status: ReplayCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRunResult {
    pub schema: String,
    pub scenario_id: String,
    pub seed: u64,
    pub driver: ReplayDriver,
    pub outcome: ReplayOutcome,
    pub actions: Vec<CoreAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concurrent_groups: Vec<ReplayConcurrentGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_observations: Vec<ReplayStateObservation>,
    pub provenance: ReplayProvenance,
    pub gates: Vec<ReplayGateResult>,
    pub invariants: Vec<ReplayInvariantResult>,
    pub observations: Vec<ObservationSet>,
}

impl ReplayRunResult {
    pub fn from_json(input: &str) -> Result<Self> {
        let result: Self = serde_json::from_str(input).context("parse core replay result JSON")?;
        result.validate()?;
        Ok(result)
    }

    pub fn to_pretty_json(&self) -> Result<String> {
        self.validate()?;
        serde_json::to_string_pretty(self).context("serialize core replay result JSON")
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == REPLAY_RESULT_SCHEMA,
            "unsupported replay result schema: {}",
            self.schema
        );
        validate_identifier("result scenario id", &self.scenario_id)?;
        ensure!(
            !self.actions.is_empty() || !self.concurrent_groups.is_empty(),
            "replay result has no serial actions or concurrent groups"
        );
        ensure!(
            self.actions.len()
                + self
                    .concurrent_groups
                    .iter()
                    .map(|group| group.actions.len() * usize::from(group.repetitions))
                    .sum::<usize>()
                <= MAX_REPLAY_ACTIONS,
            "replay result expands beyond {MAX_REPLAY_ACTIONS} actions"
        );
        for action in &self.actions {
            validate_action(action)?;
        }
        validate_concurrent_groups(&self.concurrent_groups)?;
        if self.concurrent_groups.is_empty() {
            ensure!(
                self.state_observations.is_empty(),
                "state observations require concurrent groups"
            );
        } else {
            ensure!(
                !self.state_observations.is_empty(),
                "concurrent replay result has no state observations"
            );
        }
        ensure!(
            self.state_observations.len() <= MAX_REPLAY_CHECKS,
            "replay result has too many state observations"
        );
        let mut state_observation_ids = BTreeSet::new();
        for observation in &self.state_observations {
            validate_identifier("state observation id", &observation.id)?;
            ensure!(
                state_observation_ids.insert(observation.id.as_str()),
                "duplicate state observation id: {}",
                observation.id
            );
            validate_state_values("state observation", &observation.id, &observation.values)?;
        }
        validate_provenance(&self.provenance)?;
        ensure!(!self.gates.is_empty(), "replay result gates are empty");
        ensure!(
            self.gates.len() <= MAX_REPLAY_CHECKS,
            "replay result has too many gates"
        );
        ensure!(
            !self.invariants.is_empty(),
            "replay result invariants are empty"
        );
        ensure!(
            self.invariants.len() <= MAX_REPLAY_CHECKS,
            "replay result has too many invariants"
        );

        let mut gate_ids = BTreeSet::new();
        for gate in &self.gates {
            validate_identifier("gate result id", &gate.id)?;
            ensure!(
                gate_ids.insert(gate.id.as_str()),
                "duplicate gate result id: {}",
                gate.id
            );
            validate_check_reason(gate.status, gate.reason.as_deref(), "gate", &gate.id)?;
            if gate.status == ReplayCheckStatus::Passed {
                ensure!(
                    !gate.artifacts.is_empty(),
                    "passed gate {} has no evidence artifacts",
                    gate.id
                );
            }
            for artifact in &gate.artifacts {
                validate_relative_artifact_path(artifact)?;
            }
        }

        let mut invariant_ids = BTreeSet::new();
        for invariant in &self.invariants {
            validate_identifier("invariant result id", &invariant.id)?;
            ensure!(
                invariant_ids.insert(invariant.id.as_str()),
                "duplicate invariant result id: {}",
                invariant.id
            );
            validate_check_reason(
                invariant.status,
                invariant.reason.as_deref(),
                "invariant",
                &invariant.id,
            )?;
        }

        let derived = derive_outcome(
            self.gates
                .iter()
                .map(|gate| gate.status)
                .chain(self.invariants.iter().map(|invariant| invariant.status)),
        )?;
        ensure!(
            self.outcome == derived,
            "replay result outcome {:?} does not match check outcome {:?}",
            self.outcome,
            derived
        );

        if self.outcome == ReplayOutcome::Passed {
            ensure!(
                !self.observations.is_empty(),
                "passed replay result has no observations"
            );
        }
        let mut observation_keys = BTreeSet::new();
        for observation in &self.observations {
            validate_non_empty("observation subject", &observation.subject)?;
            validate_non_empty("observation phase", &observation.phase)?;
            ensure!(
                !observation.facts().is_empty(),
                "observation {}/{} has no facts",
                observation.subject,
                observation.phase
            );
            ensure!(
                observation_keys.insert((observation.subject.as_str(), observation.phase.as_str())),
                "duplicate observation subject/phase: {}/{}",
                observation.subject,
                observation.phase
            );
        }
        Ok(())
    }

    pub fn validate_against(&self, scenario: &ReplayScenarioManifest) -> Result<()> {
        scenario.validate()?;
        self.validate()?;
        ensure!(
            self.scenario_id == scenario.id,
            "result scenario id {} does not match {}",
            self.scenario_id,
            scenario.id
        );
        ensure!(
            self.seed == scenario.seed,
            "result seed {} does not match {}",
            self.seed,
            scenario.seed
        );
        ensure!(
            self.actions == scenario.actions,
            "result action order does not match scenario"
        );
        ensure!(
            self.concurrent_groups == scenario.concurrent_groups,
            "result concurrent groups do not match scenario"
        );

        ensure!(
            self.state_observations.len() == scenario.state_expectations.len(),
            "result state observation count {} does not match expected count {}",
            self.state_observations.len(),
            scenario.state_expectations.len()
        );
        for expected in &scenario.state_expectations {
            let actual = self
                .state_observations
                .iter()
                .find(|observation| observation.id == expected.id)
                .with_context(|| format!("missing state observation: {}", expected.id))?;
            let expected_values = state_values_map(&expected.values);
            let actual_values = state_values_map(&actual.values);
            ensure!(
                expected_values.keys().eq(actual_values.keys()),
                "state observation {} keys do not match scenario",
                expected.id
            );
            let invariant = self
                .invariants
                .iter()
                .find(|invariant| invariant.id == expected.invariant_id)
                .with_context(|| {
                    format!(
                        "state observation {} is missing invariant result {}",
                        expected.id, expected.invariant_id
                    )
                })?;
            let expected_status = if actual_values == expected_values {
                ReplayCheckStatus::Passed
            } else {
                ReplayCheckStatus::Failed
            };
            ensure!(
                invariant.status == expected_status,
                "state observation {} requires invariant {} status {:?}, got {:?}",
                expected.id,
                expected.invariant_id,
                expected_status,
                invariant.status
            );
        }

        let lane = scenario
            .lanes
            .iter()
            .find(|lane| lane.driver == self.driver)
            .with_context(|| format!("scenario has no {:?} lane", self.driver))?;
        ensure!(
            self.gates.len() == lane.required_gates.len(),
            "result gate count {} does not match required count {}",
            self.gates.len(),
            lane.required_gates.len()
        );
        for required in &lane.required_gates {
            let actual = self
                .gates
                .iter()
                .find(|gate| gate.id == required.id)
                .with_context(|| format!("missing required gate result: {}", required.id))?;
            ensure!(
                actual.evidence_kind == required.evidence_kind,
                "gate {} evidence kind does not match scenario",
                required.id
            );
        }

        ensure!(
            self.invariants.len() == scenario.expected_invariants.len(),
            "result invariant count {} does not match expected count {}",
            self.invariants.len(),
            scenario.expected_invariants.len()
        );
        for expected in &scenario.expected_invariants {
            ensure!(
                self.invariants
                    .iter()
                    .any(|invariant| invariant.id == expected.id),
                "missing expected invariant result: {}",
                expected.id
            );
        }
        Ok(())
    }
}

fn validate_concurrent_groups(groups: &[ReplayConcurrentGroup]) -> Result<BTreeSet<String>> {
    let mut group_ids = BTreeSet::new();
    for group in groups {
        validate_identifier("concurrent group id", &group.id)?;
        ensure!(
            group_ids.insert(group.id.clone()),
            "duplicate concurrent group id: {}",
            group.id
        );
        ensure!(
            (1..=MAX_CONCURRENT_GROUP_REPETITIONS).contains(&group.repetitions),
            "concurrent group {} repetitions must be in 1..={MAX_CONCURRENT_GROUP_REPETITIONS}",
            group.id
        );
        ensure!(
            group.actions.len() >= 2,
            "concurrent group {} must contain at least two actions",
            group.id
        );
        let mut actors = BTreeSet::new();
        for action in &group.actions {
            validate_identifier("concurrent actor id", action.actor())?;
            ensure!(
                actors.insert(action.actor()),
                "concurrent group {} repeats actor {}",
                group.id,
                action.actor()
            );
            match action {
                ReplayConcurrentAction::PlaceBlock { item, .. } => {
                    mc_protocol::codec::Identifier::parse(item).with_context(|| {
                        format!("concurrent group {} has invalid item {item}", group.id)
                    })?;
                }
                ReplayConcurrentAction::ChestPickup { slot, .. } => ensure!(
                    *slot < 27,
                    "concurrent group {} chest slot is outside 0..27: {slot}",
                    group.id
                ),
            }
        }
        match &group.fixture {
            ReplayConcurrentFixture::SameTargetPlacement => {
                ensure!(
                    group.actions.len() == 2,
                    "same_target_placement group {} requires exactly two actions",
                    group.id
                );
                ensure!(
                    group
                        .actions
                        .iter()
                        .all(|action| matches!(action, ReplayConcurrentAction::PlaceBlock { .. })),
                    "same_target_placement group {} contains an incompatible action",
                    group.id
                );
            }
            ReplayConcurrentFixture::SharedChest {
                item,
                initial_count,
            } => {
                mc_protocol::codec::Identifier::parse(item).with_context(|| {
                    format!(
                        "concurrent group {} has invalid fixture item {item}",
                        group.id
                    )
                })?;
                ensure!(
                    *initial_count > 0,
                    "shared_chest group {} initial_count must be positive",
                    group.id
                );
                ensure!(
                    group.repetitions == 1,
                    "shared_chest group {} requires exactly one repetition",
                    group.id
                );
                ensure!(
                    group.actions.len() == 2,
                    "shared_chest group {} requires exactly two actions",
                    group.id
                );
                ensure!(
                    group
                        .actions
                        .iter()
                        .all(|action| matches!(action, ReplayConcurrentAction::ChestPickup { .. })),
                    "shared_chest group {} contains an incompatible action",
                    group.id
                );
                let slots = group
                    .actions
                    .iter()
                    .filter_map(|action| match action {
                        ReplayConcurrentAction::ChestPickup { slot, .. } => Some(slot),
                        ReplayConcurrentAction::PlaceBlock { .. } => None,
                    })
                    .collect::<BTreeSet<_>>();
                ensure!(
                    slots.len() == 1,
                    "shared_chest group {} actions must target the same slot",
                    group.id
                );
            }
        }
    }
    Ok(group_ids)
}

fn validate_state_values(label: &str, id: &str, values: &[ReplayStateValue]) -> Result<()> {
    ensure!(!values.is_empty(), "{label} {id} has no values");
    ensure!(
        values.len() <= MAX_REPLAY_CHECKS,
        "{label} {id} has too many values"
    );
    let mut keys = BTreeSet::new();
    for value in values {
        validate_identifier("state value key", &value.key)?;
        ensure!(
            keys.insert(value.key.as_str()),
            "{label} {id} repeats value key {}",
            value.key
        );
    }
    Ok(())
}

fn state_values_map(values: &[ReplayStateValue]) -> BTreeMap<&str, i64> {
    values
        .iter()
        .map(|value| (value.key.as_str(), value.value))
        .collect()
}

fn validate_action(action: &CoreAction) -> Result<()> {
    match action {
        CoreAction::WaitTicks { ticks } => {
            ensure!(*ticks > 0, "wait_ticks action must wait at least one tick");
        }
        CoreAction::Look { yaw_deg, pitch_deg } => {
            ensure!(
                (-180..=180).contains(yaw_deg),
                "look yaw is outside [-180, 180]: {yaw_deg}"
            );
            ensure!(
                (-90..=90).contains(pitch_deg),
                "look pitch is outside [-90, 90]: {pitch_deg}"
            );
        }
        CoreAction::MoveBy { .. } | CoreAction::Reconnect => {}
    }
    Ok(())
}

fn validate_provenance(provenance: &ReplayProvenance) -> Result<()> {
    ensure!(
        matches!(provenance.git_commit.len(), 40 | 64)
            && provenance
                .git_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "git_commit must be a full 40- or 64-character hexadecimal object id"
    );
    ensure!(
        provenance.config_sha256.len() == 64
            && provenance
                .config_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "config_sha256 must be a 64-character hexadecimal digest"
    );
    validate_non_empty("build profile", &provenance.build_profile)?;
    validate_non_empty("sidecar version", &provenance.sidecar_version)?;
    validate_non_empty("hardware os", &provenance.hardware.os)?;
    validate_non_empty("hardware arch", &provenance.hardware.arch)?;
    validate_non_empty("hardware cpu_model", &provenance.hardware.cpu_model)?;
    ensure!(
        provenance.hardware.logical_cpus > 0,
        "hardware logical_cpus must be positive"
    );
    ensure!(
        provenance.hardware.memory_mib > 0,
        "hardware memory_mib must be positive"
    );
    Ok(())
}

fn validate_check_reason(
    status: ReplayCheckStatus,
    reason: Option<&str>,
    kind: &str,
    id: &str,
) -> Result<()> {
    if status == ReplayCheckStatus::Passed {
        ensure!(
            reason.is_none(),
            "passed {kind} {id} must not carry a failure reason"
        );
    } else {
        let reason =
            reason.with_context(|| format!("non-passing {kind} {id} is missing reason"))?;
        validate_non_empty("non-passing check reason", reason)?;
    }
    Ok(())
}

fn derive_outcome(statuses: impl IntoIterator<Item = ReplayCheckStatus>) -> Result<ReplayOutcome> {
    let mut saw_any = false;
    let mut saw_failed = false;
    let mut saw_blocked = false;
    let mut saw_degraded = false;
    for status in statuses {
        saw_any = true;
        match status {
            ReplayCheckStatus::Passed => {}
            ReplayCheckStatus::Failed => saw_failed = true,
            ReplayCheckStatus::Blocked => saw_blocked = true,
            ReplayCheckStatus::Degraded | ReplayCheckStatus::Skipped => saw_degraded = true,
        }
    }
    ensure!(saw_any, "replay result has no gate or invariant checks");
    Ok(if saw_failed {
        ReplayOutcome::Failed
    } else if saw_blocked {
        ReplayOutcome::Blocked
    } else if saw_degraded {
        ReplayOutcome::Degraded
    } else {
        ReplayOutcome::Passed
    })
}

fn validate_ledger_row(row: &str) -> Result<()> {
    ensure!(!row.is_empty(), "core gate ledger row is empty");
    ensure!(row.len() <= 16, "core gate ledger row is too long");
    ensure!(
        row.bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()),
        "invalid core gate ledger row: {row}"
    );
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} is empty");
    ensure!(value.len() <= 128, "{label} is longer than 128 bytes");
    let mut bytes = value.bytes();
    let first = bytes.next().expect("non-empty identifier");
    ensure!(
        first.is_ascii_lowercase() || first.is_ascii_digit(),
        "{label} must start with lowercase ASCII or a digit: {value}"
    );
    ensure!(
        bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        }),
        "{label} contains unsupported characters: {value}"
    );
    Ok(())
}

fn validate_non_empty(label: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{label} is empty");
    ensure!(value.len() <= 1024, "{label} is longer than 1024 bytes");
    Ok(())
}

fn validate_relative_artifact_path(value: &str) -> Result<()> {
    validate_non_empty("artifact path", value)?;
    ensure!(
        !value.contains('\\'),
        "artifact path must use repository-style forward slashes: {value}"
    );
    let path = Path::new(value);
    ensure!(!path.is_absolute(), "artifact path is absolute: {value}");
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal = true,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                bail!("artifact path is not repository-relative: {value}");
            }
        }
    }
    ensure!(
        saw_normal,
        "artifact path has no repository component: {value}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn block_transaction_oracle_json() -> Value {
        json!({
            "schema": "solaris.block_transaction.oracle.v1",
            "id": "block-transaction-26-1-2",
            "phases": [
                {"id":"accepted-break","case":"accepted_break","sequence":1},
                {"id":"accepted-place","case":"accepted_place","sequence":2},
                {"id":"occupied-place-rejection","case":"occupied_place_rejection","sequence":3},
                {"id":"out-of-reach-break-rejection","case":"out_of_reach_break_rejection","sequence":4},
                {"id":"early-stop-break-rejection","case":"early_stop_break_rejection","sequence":5}
            ]
        })
    }

    #[test]
    fn block_transaction_oracle_manifest_requires_complete_unique_case_matrix() {
        let manifest =
            BlockTransactionOracleManifest::from_json(&block_transaction_oracle_json().to_string())
                .expect("valid block transaction oracle manifest");
        assert_eq!(manifest.phases.len(), 5);
        assert_eq!(
            BlockTransactionOracleManifest::from_json(
                &manifest.to_pretty_json().expect("encode manifest")
            )
            .expect("decode manifest"),
            manifest
        );

        let mut duplicate_case = block_transaction_oracle_json();
        duplicate_case["phases"][4]["case"] = json!("accepted_break");
        assert!(BlockTransactionOracleManifest::from_json(&duplicate_case.to_string()).is_err());

        let mut duplicate_sequence = block_transaction_oracle_json();
        duplicate_sequence["phases"][4]["sequence"] = json!(4);
        assert!(
            BlockTransactionOracleManifest::from_json(&duplicate_sequence.to_string()).is_err()
        );

        let mut unknown_field = block_transaction_oracle_json();
        unknown_field["phases"][0]["expected_state"] = json!(0);
        assert!(BlockTransactionOracleManifest::from_json(&unknown_field.to_string()).is_err());
    }

    #[test]
    fn block_transaction_trace_requires_authoritative_update_before_matching_ack() {
        let manifest =
            BlockTransactionOracleManifest::from_json(&block_transaction_oracle_json().to_string())
                .expect("valid manifest");
        let phases = manifest
            .phases
            .iter()
            .map(|phase| BlockTransactionOraclePhaseTrace {
                id: phase.id.clone(),
                events: vec![
                    BlockTransactionOracleEvent::TargetUpdate { state_id: 1 },
                    BlockTransactionOracleEvent::Ack {
                        sequence: phase.sequence,
                    },
                ],
            })
            .collect();
        let mut trace = BlockTransactionOracleTrace {
            manifest_id: manifest.id.clone(),
            phases,
        };
        trace.phases[0].events = vec![BlockTransactionOracleEvent::Ack { sequence: 1 }];
        trace.phases[1].events = vec![BlockTransactionOracleEvent::Ack { sequence: 2 }];
        trace
            .validate_against(&manifest)
            .expect("accepted phases may rely on prediction while rejections resync before ack");

        let mut ack_first = trace.clone();
        ack_first.phases[2].events.swap(0, 1);
        assert!(ack_first.validate_against(&manifest).is_err());

        let mut wrong_ack = trace;
        wrong_ack.phases[3].events[1] = BlockTransactionOracleEvent::Ack { sequence: 99 };
        assert!(wrong_ack.validate_against(&manifest).is_err());
    }

    fn core_gate_manifest_json() -> Value {
        json!({
            "schema": "solaris.core_gate.manifest.v1",
            "phases": [
                {
                    "id": "solid-edit",
                    "scenario_id": "m94-02a-solid-place-break-drop",
                    "ledger_rows": ["B1"],
                    "evidence_legs": ["unit", "wire", "real_client"]
                },
                {
                    "id": "rejected-resync",
                    "scenario_id": "m94-02b-rejected-block-resync",
                    "ledger_rows": ["B1"],
                    "evidence_legs": ["wire", "oracle", "replay_negative"]
                }
            ],
            "rows": [{
                "row": "B1",
                "scope": "broad",
                "required_phases": ["solid-edit", "rejected-resync"],
                "required_evidence_legs": [
                    "unit", "wire", "oracle", "real_client", "replay_negative"
                ]
            }]
        })
    }

    #[test]
    fn core_gate_manifest_accepts_focused_phase_and_evidence_matrix() {
        let manifest = CoreGateManifest::from_json(&core_gate_manifest_json().to_string())
            .expect("valid core gate manifest");
        manifest
            .validate_completion(&[
                CoreGatePhaseEvidence {
                    phase_id: "solid-edit".to_owned(),
                    passed_evidence_legs: [
                        CoreGateEvidenceLeg::Unit,
                        CoreGateEvidenceLeg::Wire,
                        CoreGateEvidenceLeg::RealClient,
                    ]
                    .into_iter()
                    .collect(),
                },
                CoreGatePhaseEvidence {
                    phase_id: "rejected-resync".to_owned(),
                    passed_evidence_legs: [
                        CoreGateEvidenceLeg::Wire,
                        CoreGateEvidenceLeg::Oracle,
                        CoreGateEvidenceLeg::ReplayNegative,
                    ]
                    .into_iter()
                    .collect(),
                },
            ])
            .expect("complete focused evidence matrix");
    }

    #[test]
    fn broad_core_gate_row_rejects_single_phase_manifest() {
        let mut manifest = core_gate_manifest_json();
        manifest["rows"][0]["required_phases"] = json!(["solid-edit"]);
        let error = CoreGateManifest::from_json(&manifest.to_string())
            .expect_err("broad row must require multiple focused phases");
        assert!(error.to_string().contains("at least two focused phases"));
    }

    #[test]
    fn core_gate_completion_rejects_missing_focused_phase() {
        let manifest = CoreGateManifest::from_json(&core_gate_manifest_json().to_string())
            .expect("valid core gate manifest");
        let error = manifest
            .validate_completion(&[CoreGatePhaseEvidence {
                phase_id: "solid-edit".to_owned(),
                passed_evidence_legs: [
                    CoreGateEvidenceLeg::Unit,
                    CoreGateEvidenceLeg::Wire,
                    CoreGateEvidenceLeg::RealClient,
                ]
                .into_iter()
                .collect(),
            }])
            .expect_err("missing focused phase must reject broad row completion");
        assert!(
            error
                .to_string()
                .contains("missing focused phase rejected-resync")
        );
    }

    #[test]
    fn core_gate_completion_rejects_missing_declared_evidence() {
        let manifest = CoreGateManifest::from_json(&core_gate_manifest_json().to_string())
            .expect("valid core gate manifest");
        let error = manifest
            .validate_completion(&[
                CoreGatePhaseEvidence {
                    phase_id: "solid-edit".to_owned(),
                    passed_evidence_legs: [CoreGateEvidenceLeg::Unit, CoreGateEvidenceLeg::Wire]
                        .into_iter()
                        .collect(),
                },
                CoreGatePhaseEvidence {
                    phase_id: "rejected-resync".to_owned(),
                    passed_evidence_legs: [
                        CoreGateEvidenceLeg::Wire,
                        CoreGateEvidenceLeg::Oracle,
                        CoreGateEvidenceLeg::ReplayNegative,
                    ]
                    .into_iter()
                    .collect(),
                },
            ])
            .expect_err("missing phase evidence must reject completion");
        assert!(error.to_string().contains("missing declared evidence legs"));
    }

    fn scenario_json() -> Value {
        json!({
            "schema": "solaris.core_replay.scenario.v1",
            "id": "core-actions-seed-81",
            "seed": 81,
            "actions": [
                { "type": "wait_ticks", "ticks": 2 },
                { "type": "move_by", "dx_cm": 100, "dz_cm": -50 },
                { "type": "look", "yaw_deg": 90, "pitch_deg": 0 },
                { "type": "reconnect" }
            ],
            "lanes": [
                {
                    "driver": "solaris_protocol",
                    "required_gates": [
                        { "id": "protocol-session", "evidence_kind": "harness" }
                    ]
                },
                {
                    "driver": "vanilla_oracle",
                    "required_gates": [
                        { "id": "oracle-comparison", "evidence_kind": "oracle" }
                    ]
                },
                {
                    "driver": "real_client",
                    "required_gates": [
                        { "id": "real-client-observation", "evidence_kind": "real_client" }
                    ]
                }
            ],
            "expected_invariants": [
                {
                    "id": "post-action-liveness",
                    "description": "The client remains responsive after the ordered actions."
                },
                {
                    "id": "deterministic-normalized-state",
                    "description": "Repeated Solaris runs produce the same normalized observations."
                }
            ]
        })
    }

    fn result_json() -> Value {
        json!({
            "schema": "solaris.core_replay.result.v1",
            "scenario_id": "core-actions-seed-81",
            "seed": 81,
            "driver": "solaris_protocol",
            "outcome": "passed",
            "actions": [
                { "type": "wait_ticks", "ticks": 2 },
                { "type": "move_by", "dx_cm": 100, "dz_cm": -50 },
                { "type": "look", "yaw_deg": 90, "pitch_deg": 0 },
                { "type": "reconnect" }
            ],
            "provenance": {
                "git_commit": "0123456789abcdef0123456789abcdef01234567",
                "config_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "build_profile": "debug",
                "sidecar_version": "embedded:26.1.2",
                "hardware": {
                    "os": "linux",
                    "arch": "x86_64",
                    "cpu_model": "fixture-cpu",
                    "logical_cpus": 8,
                    "memory_mib": 16384
                }
            },
            "gates": [
                {
                    "id": "protocol-session",
                    "evidence_kind": "harness",
                    "status": "passed",
                    "artifacts": [".analysis/core-replay/protocol-session.json"]
                }
            ],
            "invariants": [
                { "id": "post-action-liveness", "status": "passed" },
                { "id": "deterministic-normalized-state", "status": "passed" }
            ],
            "observations": [
                {
                    "subject": "solaris",
                    "phase": "core-actions-seed-81",
                    "facts": [
                        { "type": "note", "key": "post_action_liveness", "value": "true" }
                    ]
                }
            ]
        })
    }

    fn concurrent_scenario_json() -> Value {
        json!({
            "schema": "solaris.core_replay.scenario.v1",
            "id": "multiplayer-transactions-seed-8102",
            "seed": 8102,
            "actions": [],
            "concurrent_groups": [
                {
                    "id": "same-target-placement",
                    "repetitions": 8,
                    "fixture": { "type": "same_target_placement" },
                    "actions": [
                        {
                            "type": "place_block",
                            "actor": "dirt-player",
                            "item": "minecraft:dirt"
                        },
                        {
                            "type": "place_block",
                            "actor": "stone-player",
                            "item": "minecraft:stone"
                        }
                    ]
                }
            ],
            "state_expectations": [
                {
                    "id": "placement-state",
                    "after_group": "same-target-placement",
                    "invariant_id": "placement-conservation",
                    "values": [
                        { "key": "rounds", "value": 8 },
                        { "key": "authoritative_blocks", "value": 8 },
                        { "key": "consumed_items", "value": 8 },
                        { "key": "conservation_failures", "value": 0 }
                    ]
                }
            ],
            "lanes": [
                {
                    "driver": "solaris_protocol",
                    "required_gates": [
                        { "id": "concurrent-protocol-session", "evidence_kind": "harness" }
                    ]
                }
            ],
            "expected_invariants": [
                {
                    "id": "placement-conservation",
                    "description": "Every concurrent placement round commits one block and consumes one item."
                },
                {
                    "id": "deterministic-normalized-state",
                    "description": "Repeated runs produce the same symmetric conservation observations."
                }
            ]
        })
    }

    fn concurrent_result_json() -> Value {
        let scenario = concurrent_scenario_json();
        json!({
            "schema": "solaris.core_replay.result.v1",
            "scenario_id": "multiplayer-transactions-seed-8102",
            "seed": 8102,
            "driver": "solaris_protocol",
            "outcome": "passed",
            "actions": [],
            "concurrent_groups": scenario["concurrent_groups"].clone(),
            "state_observations": [
                {
                    "id": "placement-state",
                    "values": scenario["state_expectations"][0]["values"].clone()
                }
            ],
            "provenance": {
                "git_commit": "0123456789abcdef0123456789abcdef01234567",
                "config_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "build_profile": "debug",
                "sidecar_version": "embedded:26.1.2",
                "hardware": {
                    "os": "linux",
                    "arch": "x86_64",
                    "cpu_model": "fixture-cpu",
                    "logical_cpus": 8,
                    "memory_mib": 16384
                }
            },
            "gates": [
                {
                    "id": "concurrent-protocol-session",
                    "evidence_kind": "harness",
                    "status": "passed",
                    "artifacts": ["target/replay-results/multiplayer-transactions-seed-8102.json"]
                }
            ],
            "invariants": [
                { "id": "placement-conservation", "status": "passed" },
                { "id": "deterministic-normalized-state", "status": "passed" }
            ],
            "observations": [
                {
                    "subject": "solaris",
                    "phase": "multiplayer-transactions-seed-8102",
                    "facts": [
                        { "type": "note", "key": "conservation", "value": "passed" }
                    ]
                }
            ]
        })
    }

    #[test]
    fn minimal_complete_scenario_parses() {
        let input = serde_json::to_string(&scenario_json()).expect("scenario json");
        let scenario = ReplayScenarioManifest::from_json(&input).expect("valid scenario");

        assert_eq!(scenario.seed, 81);
        assert_eq!(scenario.actions.len(), 4);
        assert_eq!(scenario.lanes.len(), 3);
        assert_eq!(scenario.expected_invariants.len(), 2);
    }

    #[test]
    fn concurrent_scenario_parses_without_serial_actions() {
        let input = serde_json::to_string(&concurrent_scenario_json()).expect("scenario json");

        ReplayScenarioManifest::from_json(&input).expect("valid concurrent scenario");
    }

    #[test]
    fn concurrent_result_cross_validates_state_observations() {
        let scenario = ReplayScenarioManifest::from_json(&concurrent_scenario_json().to_string())
            .expect("valid concurrent scenario");
        let result = ReplayRunResult::from_json(&concurrent_result_json().to_string())
            .expect("valid concurrent result");

        result
            .validate_against(&scenario)
            .expect("concurrent result matches scenario");
    }

    #[test]
    fn concurrent_result_cannot_report_mismatched_state_as_passed() {
        let scenario = ReplayScenarioManifest::from_json(&concurrent_scenario_json().to_string())
            .expect("valid concurrent scenario");
        let mut value = concurrent_result_json();
        let consumed = value["state_observations"][0]["values"]
            .as_array_mut()
            .expect("state values")
            .iter_mut()
            .find(|value| value["key"] == "consumed_items")
            .expect("consumed item value");
        consumed["value"] = json!(7);

        let falsely_passed = ReplayRunResult::from_json(&value.to_string())
            .expect("standalone result is structurally valid");
        assert!(
            falsely_passed.validate_against(&scenario).is_err(),
            "scenario cross-validation must reject false conservation pass"
        );

        value["outcome"] = json!("failed");
        value["invariants"][0]["status"] = json!("failed");
        value["invariants"][0]["reason"] = json!("consumed_items expected 8, observed 7");
        let recorded_failure = ReplayRunResult::from_json(&value.to_string())
            .expect("failed replay result is structurally valid");
        recorded_failure
            .validate_against(&scenario)
            .expect("failed conservation result remains replayable evidence");
    }

    #[test]
    fn concurrent_failure_manifest_keeps_only_the_failing_group_contract() {
        let mut value = concurrent_scenario_json();
        value["concurrent_groups"]
            .as_array_mut()
            .expect("concurrent groups")
            .push(json!({
                "id": "other-placement",
                "repetitions": 1,
                "fixture": { "type": "same_target_placement" },
                "actions": [
                    {
                        "type": "place_block",
                        "actor": "other-a",
                        "item": "minecraft:dirt"
                    },
                    {
                        "type": "place_block",
                        "actor": "other-b",
                        "item": "minecraft:stone"
                    }
                ]
            }));
        value["state_expectations"]
            .as_array_mut()
            .expect("state expectations")
            .push(json!({
                "id": "other-state",
                "after_group": "other-placement",
                "invariant_id": "other-conservation",
                "values": [{ "key": "rounds", "value": 1 }]
            }));
        value["expected_invariants"]
            .as_array_mut()
            .expect("expected invariants")
            .push(json!({
                "id": "other-conservation",
                "description": "The unrelated group conserves its state."
            }));
        let scenario = ReplayScenarioManifest::from_json(&value.to_string())
            .expect("multi-group scenario parses");

        let failure = scenario
            .minimal_concurrent_failure("same-target-placement")
            .expect("shrink failing concurrent group");

        assert_eq!(failure.seed, scenario.seed);
        assert_eq!(failure.concurrent_groups.len(), 1);
        assert_eq!(failure.concurrent_groups[0].id, "same-target-placement");
        assert_eq!(failure.state_expectations.len(), 1);
        assert_eq!(failure.state_expectations[0].id, "placement-state");
        assert_eq!(failure.expected_invariants.len(), 1);
        assert_eq!(failure.expected_invariants[0].id, "placement-conservation");
        failure
            .validate()
            .expect("minimal failure remains replayable");
    }

    #[test]
    fn concurrent_scenario_accepts_shared_chest_group() {
        let mut value = concurrent_scenario_json();
        value["concurrent_groups"]
            .as_array_mut()
            .expect("concurrent groups")
            .push(json!({
                "id": "shared-chest-pickup",
                "repetitions": 1,
                "fixture": {
                    "type": "shared_chest",
                    "item": "minecraft:dirt",
                    "initial_count": 2
                },
                "actions": [
                    { "type": "chest_pickup", "actor": "left-player", "slot": 0 },
                    { "type": "chest_pickup", "actor": "right-player", "slot": 0 }
                ]
            }));
        value["state_expectations"]
            .as_array_mut()
            .expect("state expectations")
            .push(json!({
                "id": "chest-state",
                "after_group": "shared-chest-pickup",
                "invariant_id": "chest-conservation",
                "values": [
                    { "key": "container_items", "value": 1 },
                    { "key": "cursor_items", "value": 1 },
                    { "key": "total_items", "value": 2 },
                    { "key": "winning_cursors", "value": 1 }
                ]
            }));
        value["expected_invariants"]
            .as_array_mut()
            .expect("expected invariants")
            .push(json!({
                "id": "chest-conservation",
                "description": "One shared chest click commits and the seeded stack is conserved."
            }));

        ReplayScenarioManifest::from_json(&value.to_string())
            .expect("valid shared chest concurrent group");
    }

    #[test]
    fn concurrent_contract_rejects_group_shapes_the_executor_cannot_replay() {
        let checked = include_str!(
            "../../../tools/core-replay-scenarios/multiplayer-transactions-seed-8102.json"
        );

        let mut three_contenders: Value = serde_json::from_str(checked).expect("checked JSON");
        three_contenders["concurrent_groups"][0]["actions"]
            .as_array_mut()
            .expect("placement actions")
            .push(json!({
                "type": "place_block",
                "actor": "third-player",
                "item": "minecraft:cobblestone"
            }));
        assert!(
            ReplayScenarioManifest::from_json(&three_contenders.to_string()).is_err(),
            "same-target fixture currently supports exactly two contenders"
        );

        let mut repeated_chest: Value = serde_json::from_str(checked).expect("checked JSON");
        repeated_chest["concurrent_groups"][1]["repetitions"] = json!(2);
        assert!(
            ReplayScenarioManifest::from_json(&repeated_chest.to_string()).is_err(),
            "shared chest fixture currently represents one state-id race"
        );

        let mut split_slots: Value = serde_json::from_str(checked).expect("checked JSON");
        split_slots["concurrent_groups"][1]["actions"][1]["slot"] = json!(1);
        assert!(
            ReplayScenarioManifest::from_json(&split_slots.to_string()).is_err(),
            "shared chest contenders must target the same slot"
        );
    }

    #[test]
    fn scenario_rejects_unknown_fields_and_actions() {
        let mut unknown_field = scenario_json();
        unknown_field["unexpected"] = json!(true);
        assert!(
            ReplayScenarioManifest::from_json(&unknown_field.to_string()).is_err(),
            "unknown scenario fields must fail closed"
        );

        let mut unknown_action = scenario_json();
        unknown_action["actions"][0]["type"] = json!("teleport");
        assert!(
            ReplayScenarioManifest::from_json(&unknown_action.to_string()).is_err(),
            "unknown action variants must fail closed"
        );
    }

    #[test]
    fn scenario_rejects_empty_required_collections() {
        for field in ["actions", "lanes", "expected_invariants"] {
            let mut value = scenario_json();
            value[field] = json!([]);
            assert!(
                ReplayScenarioManifest::from_json(&value.to_string()).is_err(),
                "empty {field} must fail"
            );
        }

        let mut no_lane_gates = scenario_json();
        no_lane_gates["lanes"][0]["required_gates"] = json!([]);
        assert!(
            ReplayScenarioManifest::from_json(&no_lane_gates.to_string()).is_err(),
            "every lane must name its required gates"
        );

        let mut wrong_primary_evidence = scenario_json();
        wrong_primary_evidence["lanes"][1]["required_gates"][0]["evidence_kind"] = json!("harness");
        assert!(
            ReplayScenarioManifest::from_json(&wrong_primary_evidence.to_string()).is_err(),
            "oracle and real-client lanes must require their own evidence kind"
        );
    }

    #[test]
    fn result_requires_provenance_and_every_lane_gate() {
        let mut missing_provenance = result_json();
        missing_provenance
            .as_object_mut()
            .expect("result object")
            .remove("provenance");
        assert!(ReplayRunResult::from_json(&missing_provenance.to_string()).is_err());

        let scenario = ReplayScenarioManifest::from_json(&scenario_json().to_string())
            .expect("valid scenario");
        let mut missing_gate = result_json();
        missing_gate["gates"] = json!([]);
        assert!(ReplayRunResult::from_json(&missing_gate.to_string()).is_err());

        let mut missing_invariants = result_json();
        missing_invariants["invariants"] = json!([]);
        assert!(ReplayRunResult::from_json(&missing_invariants.to_string()).is_err());

        let mut wrong_gate = result_json();
        wrong_gate["gates"][0]["id"] = json!("unrequired-gate");
        let result = ReplayRunResult::from_json(&wrong_gate.to_string())
            .expect("non-empty standalone gate is structurally valid");
        assert!(result.validate_against(&scenario).is_err());
    }

    #[test]
    fn result_requires_reasons_for_every_non_passing_check() {
        let mut value = result_json();
        value["outcome"] = json!("degraded");
        value["gates"][0]["status"] = json!("skipped");
        assert!(ReplayRunResult::from_json(&value.to_string()).is_err());

        value["gates"][0]["reason"] = json!("local sidecar was not installed");
        assert!(ReplayRunResult::from_json(&value.to_string()).is_ok());
    }

    #[test]
    fn result_rejects_falsely_passed_aggregate_outcome() {
        let mut value = result_json();
        value["invariants"][0]["status"] = json!("failed");
        value["invariants"][0]["reason"] = json!("client stopped responding");
        assert!(ReplayRunResult::from_json(&value.to_string()).is_err());
    }

    #[test]
    fn result_must_match_scenario_seed_actions_and_lane() {
        let scenario = ReplayScenarioManifest::from_json(&scenario_json().to_string())
            .expect("valid scenario");

        for mutation in ["seed", "actions", "driver"] {
            let mut value = result_json();
            match mutation {
                "seed" => value["seed"] = json!(82),
                "actions" => value["actions"].as_array_mut().expect("actions").reverse(),
                "driver" => value["driver"] = json!("vanilla_oracle"),
                _ => unreachable!(),
            }
            match ReplayRunResult::from_json(&value.to_string()) {
                Ok(result) => assert!(
                    result.validate_against(&scenario).is_err(),
                    "{mutation} mismatch must fail"
                ),
                Err(err) => panic!("{mutation} should parse before cross-validation: {err:#}"),
            }
        }
    }

    #[test]
    fn checked_core_action_fixture_is_strict_and_complete() {
        let fixture =
            include_str!("../../../tools/core-replay-scenarios/core-actions-seed-81.json");
        let scenario = ReplayScenarioManifest::from_json(fixture).expect("checked fixture parses");

        assert_eq!(scenario.id, "core-actions-seed-81");
        assert_eq!(scenario.seed, 81);
        assert_eq!(scenario.actions.len(), 4);
        assert_eq!(
            scenario
                .lanes
                .iter()
                .map(|lane| lane.driver)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                ReplayDriver::SolarisProtocol,
                ReplayDriver::VanillaOracle,
                ReplayDriver::RealClient,
            ])
        );
        assert_eq!(scenario.expected_invariants.len(), 2);
        assert!(
            scenario
                .lanes
                .iter()
                .all(|lane| !lane.required_gates.is_empty())
        );
        let canonical = scenario
            .to_pretty_json()
            .expect("serialize checked fixture");
        assert_eq!(
            ReplayScenarioManifest::from_json(&canonical).expect("parse canonical fixture"),
            scenario
        );

        let mut wrong_schema: Value = serde_json::from_str(fixture).expect("fixture JSON");
        wrong_schema["schema"] = json!("solaris.core_replay.scenario.v2");
        assert!(ReplayScenarioManifest::from_json(&wrong_schema.to_string()).is_err());

        let mut missing_seed: Value = serde_json::from_str(fixture).expect("fixture JSON");
        missing_seed
            .as_object_mut()
            .expect("fixture object")
            .remove("seed");
        assert!(ReplayScenarioManifest::from_json(&missing_seed.to_string()).is_err());
    }

    #[test]
    fn checked_multiplayer_transaction_fixture_is_strict_and_complete() {
        let fixture = include_str!(
            "../../../tools/core-replay-scenarios/multiplayer-transactions-seed-8102.json"
        );
        let scenario = ReplayScenarioManifest::from_json(fixture)
            .expect("checked multiplayer transaction fixture parses");

        assert_eq!(scenario.id, "multiplayer-transactions-seed-8102");
        assert_eq!(scenario.seed, 8102);
        assert!(scenario.actions.is_empty());
        assert_eq!(scenario.concurrent_groups.len(), 2);
        assert_eq!(scenario.state_expectations.len(), 2);
        assert_eq!(scenario.lanes.len(), 1);
        assert_eq!(scenario.lanes[0].driver, ReplayDriver::SolarisProtocol);
        assert_eq!(scenario.expected_invariants.len(), 3);
        let canonical = scenario
            .to_pretty_json()
            .expect("serialize checked multiplayer fixture");
        assert_eq!(
            ReplayScenarioManifest::from_json(&canonical)
                .expect("parse canonical multiplayer fixture"),
            scenario
        );
    }
}
