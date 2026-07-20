use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ScenarioId {
    LifecyclePassengerCleanup,
    MetadataDirtyDefault,
    AttributesEquipmentEffects,
    CollisionStep,
    DamageDeath,
    PassiveAiSchedule,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntityFact {
    Collision {
        case: String,
        position: MilliblockPosition,
        corrected: bool,
        on_ground: bool,
        horizontal_collision: bool,
    },
    Damage {
        entity: String,
        source_type: u32,
        cause: Option<String>,
        direct: Option<String>,
        source_position: Option<MilliblockPosition>,
    },
    Metadata {
        phase: String,
        entity: String,
        values: Vec<MetadataEntry>,
    },
    MetadataOmitted {
        phase: String,
        entity: String,
    },
    PacketPayload {
        phase: String,
        entity: String,
        kind: EntityStatePacket,
        payload: Vec<u8>,
    },
    Passengers {
        vehicle: String,
        passengers: Vec<String>,
    },
    Removed {
        entity: String,
    },
    ScheduleEvent {
        entity: String,
        event_id: i8,
    },
    Spawned {
        entity: String,
        kind: String,
        position: MilliblockPosition,
    },
    StatusEvent {
        entity: String,
        event_id: i8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MetadataEntry {
    pub(crate) index: u8,
    pub(crate) value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntityStatePacket {
    Attributes,
    Damage,
    EffectRemoved,
    EffectUpdated,
    Equipment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MilliblockPosition {
    pub(crate) x: i64,
    pub(crate) y: i64,
    pub(crate) z: i64,
}

pub(crate) struct EntityAliases {
    anchor: [f64; 3],
    by_runtime_id: BTreeMap<i32, String>,
    by_alias: BTreeMap<String, i32>,
}

impl EntityAliases {
    pub(crate) fn new(anchor: [f64; 3]) -> Self {
        Self {
            anchor,
            by_runtime_id: BTreeMap::new(),
            by_alias: BTreeMap::new(),
        }
    }

    pub(crate) fn bind_spawn(
        &mut self,
        alias: &str,
        runtime_id: i32,
        kind: &str,
        position: [f64; 3],
    ) -> Result<EntityFact> {
        if self.by_runtime_id.contains_key(&runtime_id) {
            bail!("runtime entity id {runtime_id} is already bound");
        }
        if self.by_alias.contains_key(alias) {
            bail!("alias {alias} is already bound");
        }
        self.by_runtime_id.insert(runtime_id, alias.to_owned());
        self.by_alias.insert(alias.to_owned(), runtime_id);
        Ok(EntityFact::Spawned {
            entity: alias.to_owned(),
            kind: kind.to_owned(),
            position: MilliblockPosition::relative(position, self.anchor),
        })
    }

    pub(crate) fn bind_existing(&mut self, alias: &str, runtime_id: i32) -> Result<()> {
        if self.by_runtime_id.contains_key(&runtime_id) {
            bail!("runtime entity id {runtime_id} is already bound");
        }
        if self.by_alias.contains_key(alias) {
            bail!("alias {alias} is already bound");
        }
        self.by_runtime_id.insert(runtime_id, alias.to_owned());
        self.by_alias.insert(alias.to_owned(), runtime_id);
        Ok(())
    }

    pub(crate) fn alias(&self, runtime_id: i32) -> Option<&str> {
        self.by_runtime_id.get(&runtime_id).map(String::as_str)
    }

    pub(crate) fn relative_position(&self, position: [f64; 3]) -> MilliblockPosition {
        MilliblockPosition::relative(position, self.anchor)
    }
}

impl MilliblockPosition {
    pub(crate) fn relative(position: [f64; 3], origin: [f64; 3]) -> Self {
        MilliblockPosition {
            x: ((position[0] - origin[0]) * 1_000.0).round() as i64,
            y: ((position[1] - origin[1]) * 1_000.0).round() as i64,
            z: ((position[2] - origin[2]) * 1_000.0).round() as i64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScenarioObservation {
    pub(crate) scenario: ScenarioId,
    evidence: EvidenceState,
    facts: Vec<EntityFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EvidenceState {
    Complete,
    Degraded { reasons: BTreeSet<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservationDiff {
    pub(crate) expected_sequence: Vec<EntityFact>,
    pub(crate) actual_sequence: Vec<EntityFact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComparisonOutcome {
    Comparable(ObservationDiff),
    Degraded {
        expected: EvidenceState,
        actual: EvidenceState,
    },
}

impl ScenarioObservation {
    pub(crate) fn new(scenario: ScenarioId) -> Self {
        Self {
            scenario,
            evidence: EvidenceState::Complete,
            facts: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, fact: EntityFact) {
        self.facts.push(fact);
    }

    pub(crate) fn extend(&mut self, facts: impl IntoIterator<Item = EntityFact>) {
        self.facts.extend(facts);
    }

    pub(crate) fn degrade(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        match &mut self.evidence {
            EvidenceState::Complete => {
                self.evidence = EvidenceState::Degraded {
                    reasons: BTreeSet::from([reason]),
                };
            }
            EvidenceState::Degraded { reasons } => {
                reasons.insert(reason);
            }
        }
    }

    pub(crate) fn facts(&self) -> &[EntityFact] {
        &self.facts
    }

    pub(crate) fn evidence(&self) -> &EvidenceState {
        &self.evidence
    }
}

pub(crate) fn compare_observations(
    expected: &ScenarioObservation,
    actual: &ScenarioObservation,
) -> Result<ComparisonOutcome> {
    if expected.scenario != actual.scenario {
        bail!(
            "scenario mismatch: expected {:?}, actual {:?}",
            expected.scenario,
            actual.scenario
        );
    }
    if !matches!(expected.evidence, EvidenceState::Complete)
        || !matches!(actual.evidence, EvidenceState::Complete)
    {
        return Ok(ComparisonOutcome::Degraded {
            expected: expected.evidence.clone(),
            actual: actual.evidence.clone(),
        });
    }
    Ok(ComparisonOutcome::Comparable(ObservationDiff {
        expected_sequence: expected.facts.clone(),
        actual_sequence: actual.facts.clone(),
    }))
}

impl ObservationDiff {
    pub(crate) fn is_empty(&self) -> bool {
        self.expected_sequence == self.actual_sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_observations_preserve_order_and_packet_multiplicity() {
        let spawned = EntityFact::Spawned {
            entity: "subject".into(),
            kind: "minecraft:sheep".into(),
            position: MilliblockPosition { x: 0, y: 0, z: 0 },
        };
        let removed = EntityFact::Removed {
            entity: "subject".into(),
        };
        let mut observations = ScenarioObservation::new(ScenarioId::LifecyclePassengerCleanup);
        observations.push(spawned.clone());
        observations.push(removed.clone());
        observations.push(spawned.clone());

        assert_eq!(observations.facts(), &[spawned.clone(), removed, spawned]);
    }

    #[test]
    fn comparison_rejects_reordered_packet_facts() {
        let spawned = EntityFact::Spawned {
            entity: "subject".into(),
            kind: "minecraft:pig".into(),
            position: MilliblockPosition { x: 0, y: 0, z: 0 },
        };
        let removed = EntityFact::Removed {
            entity: "subject".into(),
        };
        let mut expected = ScenarioObservation::new(ScenarioId::LifecyclePassengerCleanup);
        expected.extend([spawned.clone(), removed.clone()]);
        let mut actual = ScenarioObservation::new(ScenarioId::LifecyclePassengerCleanup);
        actual.extend([removed, spawned]);

        let ComparisonOutcome::Comparable(diff) =
            compare_observations(&expected, &actual).expect("comparable scenarios")
        else {
            panic!("complete rows must be comparable");
        };

        assert!(!diff.is_empty());
    }

    #[test]
    fn comparison_rejects_lost_packet_multiplicity() {
        let removed = EntityFact::Removed {
            entity: "subject".into(),
        };
        let mut expected = ScenarioObservation::new(ScenarioId::LifecyclePassengerCleanup);
        expected.extend([removed.clone(), removed.clone()]);
        let mut actual = ScenarioObservation::new(ScenarioId::LifecyclePassengerCleanup);
        actual.push(removed);

        let ComparisonOutcome::Comparable(diff) =
            compare_observations(&expected, &actual).expect("comparable scenarios")
        else {
            panic!("complete rows must be comparable");
        };

        assert!(!diff.is_empty());
    }

    #[test]
    fn spawn_facts_ignore_runtime_ids_and_absolute_origins() {
        let mut solaris = EntityAliases::new([10.0, 64.0, -20.0]);
        let mut vanilla = EntityAliases::new([-30.0, 80.0, 14.0]);

        let solaris_fact = solaris
            .bind_spawn("subject", 17, "minecraft:sheep", [11.25, 64.0, -18.0])
            .expect("bind Solaris entity");
        let vanilla_fact = vanilla
            .bind_spawn("subject", 904, "minecraft:sheep", [-28.75, 80.0, 16.0])
            .expect("bind vanilla entity");

        assert_eq!(solaris_fact, vanilla_fact);
        assert_eq!(
            solaris_fact,
            EntityFact::Spawned {
                entity: "subject".into(),
                kind: "minecraft:sheep".into(),
                position: MilliblockPosition {
                    x: 1_250,
                    y: 0,
                    z: 2_000,
                },
            }
        );
    }

    #[test]
    fn comparison_reports_both_sides_of_a_complete_mismatch() {
        let mut expected = ScenarioObservation::new(ScenarioId::DamageDeath);
        expected.push(EntityFact::Removed {
            entity: "subject".into(),
        });
        let mut actual = ScenarioObservation::new(ScenarioId::DamageDeath);
        actual.push(EntityFact::Spawned {
            entity: "subject".into(),
            kind: "minecraft:chicken".into(),
            position: MilliblockPosition {
                x: 0,
                y: 0,
                z: 1_000,
            },
        });

        let comparison = compare_observations(&expected, &actual).expect("comparable scenarios");

        assert_eq!(
            comparison,
            ComparisonOutcome::Comparable(ObservationDiff {
                expected_sequence: expected.facts,
                actual_sequence: actual.facts,
            })
        );
    }

    #[test]
    fn degraded_evidence_is_not_reported_as_a_successful_comparison() {
        let mut expected = ScenarioObservation::new(ScenarioId::AttributesEquipmentEffects);
        expected.degrade("vanilla effect control unavailable");
        let mut actual = ScenarioObservation::new(ScenarioId::AttributesEquipmentEffects);
        actual.degrade("Solaris equipment publication unavailable");
        actual.degrade("Solaris equipment publication unavailable");

        let comparison = compare_observations(&expected, &actual).expect("matching scenario ids");

        assert_eq!(
            comparison,
            ComparisonOutcome::Degraded {
                expected: expected.evidence().clone(),
                actual: actual.evidence().clone(),
            }
        );
        let EvidenceState::Degraded { reasons } = actual.evidence() else {
            panic!("actual evidence should be degraded");
        };
        assert_eq!(reasons.len(), 1);
    }

    #[test]
    fn aliases_reject_two_runtime_entities_for_one_scenario_role() {
        let mut aliases = EntityAliases::new([0.0; 3]);
        aliases
            .bind_spawn("subject", 17, "minecraft:sheep", [0.0; 3])
            .expect("bind first subject");

        let error = aliases
            .bind_spawn("subject", 18, "minecraft:sheep", [0.0; 3])
            .expect_err("duplicate scenario role must fail");

        assert!(error.to_string().contains("alias subject is already bound"));
    }

    #[test]
    fn existing_player_binding_is_available_to_packet_normalization() {
        let mut aliases = EntityAliases::new([0.0; 3]);
        aliases
            .bind_existing("player", 41)
            .expect("bind login player id");

        assert_eq!(aliases.alias(41), Some("player"));
        assert_eq!(aliases.alias(42), None);
    }

    #[test]
    fn collision_and_schedule_facts_remain_stable_after_normalization() {
        let mut observation = ScenarioObservation::new(ScenarioId::CollisionStep);
        observation.extend([
            EntityFact::ScheduleEvent {
                entity: "subject".into(),
                event_id: 10,
            },
            EntityFact::Collision {
                case: "half-step".into(),
                position: MilliblockPosition {
                    x: 6_500,
                    y: 500,
                    z: 500,
                },
                corrected: false,
                on_ground: true,
                horizontal_collision: false,
            },
        ]);

        assert_eq!(observation.facts().len(), 2);
    }

    #[test]
    fn empty_diff_reports_a_comparable_match() {
        let diff = ObservationDiff {
            expected_sequence: Vec::new(),
            actual_sequence: Vec::new(),
        };

        assert!(diff.is_empty());
    }
}
