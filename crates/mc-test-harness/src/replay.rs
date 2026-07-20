use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::parity::{
    CoreAction, ObservationSet, ScenarioContext, ServerKind, observe_core_action_sequence,
};

pub const REPLAY_SCENARIO_SCHEMA: &str = "solaris.core_replay.scenario.v1";
pub const REPLAY_RESULT_SCHEMA: &str = "solaris.core_replay.result.v1";

const MAX_REPLAY_ACTIONS: usize = 10_000;
const MAX_REPLAY_CHECKS: usize = 128;
const MAX_CONCURRENT_GROUP_REPETITIONS: u16 = 1_000;

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
