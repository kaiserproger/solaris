#[path = "entity_parity_26_1_2/model.rs"]
mod model;
#[path = "entity_parity_26_1_2/protocol.rs"]
mod protocol;
#[path = "entity_parity_26_1_2/scenarios.rs"]
mod scenarios;
#[path = "entity_parity_26_1_2/support.rs"]
mod support;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail, ensure};
use model::{ComparisonOutcome, EntityFact, EvidenceState, ScenarioId, compare_observations};
use scenarios::run_scenario_catalog;
use support::{EntityEndpoint, EntityProtocolHarness, OracleGate, SolarisServer, probe_oracle};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn solaris_entity_scenarios_emit_normalized_or_explicitly_degraded_evidence() {
    let server = SolarisServer::spawn().await.expect("spawn Solaris fixture");
    let mut harness =
        EntityProtocolHarness::connect(server.endpoint(), "EntityParityS", Duration::from_secs(12))
            .await
            .expect("enter Solaris play state");

    let observations = run_scenario_catalog(&mut harness)
        .await
        .expect("run Solaris entity scenarios");

    assert_eq!(observations.len(), 6);
    let lifecycle = observation(&observations, ScenarioId::LifecyclePassengerCleanup);
    assert!(matches!(
        lifecycle.evidence(),
        EvidenceState::Degraded { .. }
    ));
    assert!(
        lifecycle
            .facts()
            .iter()
            .any(|fact| matches!(fact, EntityFact::Spawned { entity, .. } if entity == "subject"))
    );
    assert!(
        lifecycle
            .facts()
            .iter()
            .any(|fact| matches!(fact, EntityFact::Removed { entity } if entity == "subject"))
    );

    let metadata = observation(&observations, ScenarioId::MetadataDirtyDefault);
    assert!(has_metadata_phase(metadata.facts(), "dirty"));
    assert!(metadata.facts().iter().all(|fact| !matches!(
        fact,
        EntityFact::Metadata { values, .. } if values.is_empty()
    )));
    assert!(has_default_metadata_evidence(metadata.facts()));

    let state = observation(&observations, ScenarioId::AttributesEquipmentEffects);
    assert!(matches!(state.evidence(), EvidenceState::Degraded { .. }));

    let collision = observation(&observations, ScenarioId::CollisionStep);
    assert_eq!(collision.evidence(), &EvidenceState::Complete);
    assert!(has_collision_case(
        collision.facts(),
        "full-block",
        false,
        true
    ));
    assert!(has_collision_case(
        collision.facts(),
        "half-step",
        true,
        true
    ));

    let damage = observation(&observations, ScenarioId::DamageDeath);
    if damage.evidence() == &EvidenceState::Complete {
        assert!(has_damage(damage.facts(), "subject"));
        assert!(damage.facts().iter().any(|fact| matches!(
            fact,
            EntityFact::StatusEvent {
                entity,
                event_id: 3,
            } if entity == "subject"
        )));
        assert!(
            damage
                .facts()
                .iter()
                .any(|fact| matches!(fact, EntityFact::Removed { entity } if entity == "subject"))
        );
    }

    let passive = observation(&observations, ScenarioId::PassiveAiSchedule);
    assert!(matches!(passive.evidence(), EvidenceState::Degraded { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local .analysis/server.jar and reports current entity parity differences"]
async fn local_vanilla_and_solaris_entity_scenarios_compare_side_by_side() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let jar = match probe_oracle(&repo_root) {
        OracleGate::Available { jar } => jar,
        OracleGate::Skipped { reason } => {
            panic!(
                "vanilla oracle prerequisite failed at {}: {reason}",
                repo_root.join(".analysis/server.jar").display()
            );
        }
    };
    // Construct Solaris before launching the external oracle. If Solaris setup
    // panics or returns an error, there is no vanilla child to clean up; after
    // launch, VanillaServerProcess's Drop remains the unwind cleanup guard.
    let solaris = SolarisServer::spawn().await.expect("spawn Solaris fixture");
    let vanilla_dir = tempfile::tempdir().expect("vanilla work directory");
    let mut vanilla = mc_test_harness::parity::VanillaServerProcess::launch(
        &jar,
        vanilla_dir.path(),
        Duration::from_secs(90),
    )
    .expect("launch local vanilla oracle");
    let comparison = async {
        let mut solaris_harness = EntityProtocolHarness::connect(
            solaris.endpoint(),
            "EntityParityS",
            Duration::from_secs(12),
        )
        .await?;
        let mut vanilla_harness = EntityProtocolHarness::connect(
            EntityEndpoint {
                kind: mc_test_harness::parity::ServerKind::Vanilla,
                addr: vanilla.addr(),
                collision_fixture: false,
            },
            "EntityParityV",
            Duration::from_secs(20),
        )
        .await?;
        vanilla.send_command("op EntityParityV")?;
        vanilla.wait_for_log(Duration::from_secs(10), |line| {
            line.contains("EntityParityV")
                && (line.contains("server operator") || line.contains("Opped"))
        })?;

        let solaris_observations = run_scenario_catalog(&mut solaris_harness).await?;
        let vanilla_observations = run_scenario_catalog(&mut vanilla_harness).await?;
        require_complete_parity(&vanilla_observations, &solaris_observations)
    }
    .await;
    let stop = vanilla.stop();
    comparison.expect("strict local vanilla/Solaris entity parity gate");
    stop.expect("stop vanilla oracle");
}

fn require_complete_parity(
    expected: &[model::ScenarioObservation],
    actual: &[model::ScenarioObservation],
) -> Result<()> {
    ensure!(
        expected.len() == actual.len(),
        "scenario count mismatch: vanilla={}, Solaris={}",
        expected.len(),
        actual.len()
    );
    for (expected, actual) in expected.iter().zip(actual) {
        match compare_observations(expected, actual)? {
            ComparisonOutcome::Comparable(diff) if diff.is_empty() => {}
            ComparisonOutcome::Comparable(diff) => {
                bail!(
                    "entity parity mismatch for {:?}: {diff:?}",
                    expected.scenario
                );
            }
            ComparisonOutcome::Degraded {
                expected: expected_evidence,
                actual: actual_evidence,
            } => {
                bail!(
                    "degraded entity evidence for {:?}: vanilla={expected_evidence:?}, Solaris={actual_evidence:?}",
                    expected.scenario
                );
            }
        }
    }
    Ok(())
}

fn observation(
    observations: &[model::ScenarioObservation],
    scenario: ScenarioId,
) -> &model::ScenarioObservation {
    observations
        .iter()
        .find(|observation| observation.scenario == scenario)
        .unwrap_or_else(|| panic!("missing observation for {scenario:?}"))
}

fn has_metadata_phase(facts: &[EntityFact], phase: &str) -> bool {
    facts.iter().any(|fact| {
        matches!(
            fact,
            EntityFact::Metadata {
                phase: actual,
                entity,
                ..
            } if actual == phase && entity == "subject"
        )
    })
}

fn has_default_metadata_evidence(facts: &[EntityFact]) -> bool {
    facts.iter().any(|fact| {
        matches!(
            fact,
            EntityFact::Metadata { phase, entity, .. }
                if phase == "default" && entity == "default-subject"
        )
    }) || facts.iter().any(|fact| {
        matches!(
            fact,
            EntityFact::MetadataOmitted { phase, entity }
                if phase == "default" && entity == "default-subject"
        )
    })
}

fn has_damage(facts: &[EntityFact], entity: &str) -> bool {
    facts
        .iter()
        .any(|fact| matches!(fact, EntityFact::Damage { entity: actual, .. } if actual == entity))
}

fn has_collision_case(
    facts: &[EntityFact],
    case: &str,
    expected_on_ground: bool,
    expected_horizontal_collision: bool,
) -> bool {
    facts.iter().any(|fact| {
        matches!(
            fact,
            EntityFact::Collision {
                case: actual,
                corrected: true,
                on_ground,
                horizontal_collision,
                ..
            } if actual == case
                && *on_ground == expected_on_ground
                && *horizontal_collision == expected_horizontal_collision
        )
    })
}

#[test]
fn strict_oracle_gate_rejects_degraded_rows() {
    let mut vanilla = model::ScenarioObservation::new(ScenarioId::PassiveAiSchedule);
    vanilla.degrade("deterministic schedule control unavailable");
    let solaris = model::ScenarioObservation::new(ScenarioId::PassiveAiSchedule);

    let error = require_complete_parity(&[vanilla], &[solaris])
        .expect_err("degraded rows must fail the strict oracle gate");

    assert!(error.to_string().contains("degraded entity evidence"));
}

#[test]
fn full_block_collision_requires_a_positive_correction() {
    let facts = vec![EntityFact::Collision {
        case: "full-block".into(),
        position: model::MilliblockPosition {
            x: 4_500,
            y: 0,
            z: 500,
        },
        corrected: false,
        on_ground: false,
        horizontal_collision: true,
    }];

    assert!(!has_collision_case(&facts, "full-block", false, true));
}
