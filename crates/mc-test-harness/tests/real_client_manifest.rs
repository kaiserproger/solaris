use std::collections::BTreeSet;

use serde_json::Value;

const M94_MANIFEST: &str =
    include_str!("../../../docs/real-client-regression/manifests/m94-regression-pack.json");

#[test]
fn m94_real_client_manifest_covers_required_regression_rows() {
    let manifest: Value = serde_json::from_str(M94_MANIFEST).expect("M94 manifest is valid JSON");

    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["quality_label"], "stabilization");
    assert!(
        manifest["client_requirement"]
            .as_str()
            .expect("client requirement is present")
            .contains("vanilla 26.1.2 client"),
        "manifest must require a real vanilla client"
    );

    let forbidden = manifest["forbidden_client_evidence"]
        .as_array()
        .expect("forbidden client evidence list is present");
    assert!(
        forbidden.iter().any(|entry| entry == "wire-probe"),
        "wire-probe must be explicitly excluded as real-client evidence"
    );
    assert!(
        forbidden
            .iter()
            .any(|entry| entry == "mc_test_harness::client::Client"),
        "protocol harness client must be explicitly excluded as real-client evidence"
    );

    let scenarios = manifest["scenarios"]
        .as_array()
        .expect("scenarios are present");
    assert!(scenarios.len() > 1, "M94 cannot be a single-scenario pack");

    let required_artifacts = manifest["required_artifacts"]
        .as_array()
        .expect("required artifacts are present");
    let mut artifact_ids = BTreeSet::new();
    for artifact in required_artifacts {
        let artifact = artifact.as_str().expect("required artifact is a string");
        assert!(!artifact.is_empty(), "required artifact cannot be empty");
        assert!(
            artifact_ids.insert(artifact),
            "duplicate required artifact {artifact}"
        );
    }
    for artifact in [
        "manifest.json",
        "client.log",
        "server.log",
        "observations.json",
        "screenshots/",
        "git.txt",
        "toolchain.txt",
    ] {
        assert!(
            artifact_ids.contains(artifact),
            "missing required artifact {artifact}"
        );
    }

    let mut covered_rows = BTreeSet::new();
    let mut scenario_ids = BTreeSet::new();
    for scenario in scenarios {
        let id = scenario["id"].as_str().expect("scenario id is present");
        assert!(scenario_ids.insert(id), "duplicate scenario id {id}");
        assert_eq!(scenario["status"], "manual-pending");
        assert_eq!(
            scenario["screenshots_required"], true,
            "scenario {} must require screenshots",
            scenario["id"]
        );
        assert!(
            scenario["bounded_time_minutes"]
                .as_u64()
                .unwrap_or_default()
                <= 25,
            "scenario {} must stay bounded",
            scenario["id"]
        );
        assert!(
            scenario["steps"]
                .as_array()
                .is_some_and(|steps| !steps.is_empty()),
            "scenario {} needs runnable steps",
            scenario["id"]
        );
        assert!(
            scenario["expected_observations"]
                .as_array()
                .is_some_and(|observations| !observations.is_empty()),
            "scenario {} needs expected observations",
            scenario["id"]
        );
        for row in scenario["ledger_rows"]
            .as_array()
            .expect("scenario ledger rows are present")
        {
            covered_rows.insert(row.as_str().expect("ledger row is a string"));
        }
    }

    let mut manual_pending_rows = BTreeSet::new();
    for row in manifest["scoped_rows_manual_pending"]
        .as_array()
        .expect("manual-pending scoped rows are present")
    {
        assert!(
            row["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "manual-pending scoped row needs a reason"
        );
        manual_pending_rows.insert(row["row"].as_str().expect("manual-pending row is a string"));
    }

    for row in [
        "P1", "P2", "P3", "P4", "W3", "C1", "C2", "B1", "B2", "B3", "B4", "B5", "B6", "F1", "F2",
        "F3", "F4", "L1", "L2", "I1", "I2", "K1", "K2", "A1", "E1", "E2", "E3", "V1", "N1", "N2",
        "G1", "G2", "G3", "G4", "S1", "S2", "O1", "O2",
    ] {
        assert!(
            covered_rows.contains(row) || manual_pending_rows.contains(row),
            "missing M94 ledger row {row}"
        );
    }
}
