//! Native operational morale reacts to distinct physical events. The regional
//! owner still owns health and movement; this durable state cannot alter either
//! without an accepted regional goal.

use std::collections::BTreeSet;

use mc_script::ScriptBlockPosition;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MoralePhase {
    Steady,
    Shaken,
    Wavering,
    Routing,
    Rallied,
    Surrendered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OperationalMorale {
    pub(super) phase: MoralePhase,
    pub(super) observed_health_bits: u32,
    /// Allies previously observed alive, so dead-before-order is not a casualty.
    pub(super) observed_allies: BTreeSet<String>,
    pub(super) witnessed_losses: BTreeSet<String>,
    pub(super) was_flanked: bool,
    pub(super) officer_was_lost: bool,
    /// Terminal movement remains retryable until the regional owner accepts it.
    pub(super) goal_applied: bool,
    pub(super) rally: ScriptBlockPosition,
    pub(super) officer: Option<String>,
}

pub(super) struct MoraleObservation {
    pub(super) health: f32,
    pub(super) flanked: bool,
    pub(super) new_losses: usize,
    pub(super) officer_present: bool,
    pub(super) officer_lost: bool,
    pub(super) at_rally: bool,
}

impl OperationalMorale {
    pub(super) fn new(health: f32, rally: ScriptBlockPosition, officer: Option<String>) -> Self {
        Self {
            phase: MoralePhase::Steady,
            observed_health_bits: health.to_bits(),
            witnessed_losses: BTreeSet::new(),
            observed_allies: BTreeSet::new(),
            was_flanked: false,
            officer_was_lost: false,
            rally,
            officer,
            goal_applied: false,
        }
    }

    /// Each distinct source advances one category. Quiet ticks do not undo
    /// routing; rally requires actual arrival, and a named officer must attend.
    pub(super) fn advance(&mut self, observed: MoraleObservation) {
        if matches!(self.phase, MoralePhase::Rallied | MoralePhase::Surrendered) {
            return;
        }
        let injured = observed.health < f32::from_bits(self.observed_health_bits);
        let newly_flanked = observed.flanked && !self.was_flanked;
        let officer_lost = observed.officer_lost && !self.officer_was_lost;
        self.observed_health_bits = observed.health.to_bits();
        self.was_flanked = observed.flanked;
        self.officer_was_lost |= observed.officer_lost;
        for _ in 0..usize::from(injured)
            + observed.new_losses
            + usize::from(newly_flanked)
            + usize::from(officer_lost)
        {
            self.phase = match self.phase {
                MoralePhase::Steady => MoralePhase::Shaken,
                MoralePhase::Shaken => MoralePhase::Wavering,
                MoralePhase::Wavering => MoralePhase::Routing,
                phase => phase,
            };
        }
        if self.phase == MoralePhase::Routing {
            self.goal_applied = false;
        }
        if self.phase == MoralePhase::Routing
            && observed.at_rally
            && (self.officer.is_none() || observed.officer_present)
        {
            self.phase = MoralePhase::Rallied;
            self.goal_applied = false;
        }
    }
}

impl super::InventoryRuntime {
    /// Observe only indexed serving combatants. The storage WAL records each
    /// categorical transition before the regional owner receives its goal.
    pub(super) async fn advance_active_morale(
        &self,
        storage: &mut super::PluginStorage,
    ) -> Result<(), super::PluginStorageMutationError> {
        use super::resident_orders::{DurableAssignment, DurableResidentOrderChange};
        use crate::play::ResidentGoal;
        use mc_entity::{EntityLifecycle, GoalState, Vec3};

        let active = storage.resident_orders().active_morale_records();
        for mut record in active {
            let Some(uuid) = uuid::Uuid::parse_str(&record.entity_uuid).ok() else {
                continue;
            };
            let Some(snapshot) = self
                .sessions()
                .resident_entity_snapshots(&[uuid])
                .await
                .into_iter()
                .next()
                .flatten()
                .filter(|snapshot| snapshot.lifecycle == EntityLifecycle::Alive)
            else {
                continue;
            };
            if record.assignment == DurableAssignment::Prisoner {
                let morale = record.morale.as_mut().expect("prisoner surrendered");
                if !morale.goal_applied
                    && (snapshot.goal == GoalState::Idle
                        || self
                            .sessions()
                            .apply_resident_goals(vec![ResidentGoal {
                                uuid,
                                goal: GoalState::Idle,
                            }])
                            .await
                            == 1)
                {
                    if let Some(journal) = self.sessions().world_chunk_journal() {
                        tokio::task::spawn_blocking(move || journal.writer.flush())
                            .await
                            .map_err(|error| {
                                super::PluginStorageMutationError::Io(std::io::Error::other(error))
                            })?
                            .map_err(|error| {
                                super::PluginStorageMutationError::Io(std::io::Error::other(error))
                            })?;
                    }
                    morale.goal_applied = true;
                    storage.append_resident_order_change(
                        DurableResidentOrderChange::CombatProgress {
                            record: Box::new(record),
                        },
                    )?;
                }
                continue;
            }
            let Some(order) = record.order.as_ref() else {
                continue;
            };
            let mc_script::ScriptResidentOrder::Attack { policy, .. } = &order.order else {
                continue;
            };
            let companions = policy
                .allies
                .iter()
                .filter(|handle| *handle != &record.handle)
                .filter_map(|handle| {
                    let ally = storage.resident_orders().record(handle)?;
                    (ally.plugin_id == record.plugin_id
                        && ally.assignment == DurableAssignment::Military)
                        .then(|| {
                            (
                                handle.clone(),
                                uuid::Uuid::parse_str(&ally.entity_uuid).ok(),
                            )
                        })
                        .and_then(|(handle, uuid)| uuid.map(|uuid| (handle, uuid)))
                })
                .collect::<Vec<_>>();
            let companion_uuids = companions.iter().map(|(_, uuid)| *uuid).collect::<Vec<_>>();
            let companion_snapshots = self
                .sessions()
                .resident_entity_snapshots(&companion_uuids)
                .await;
            let morale = record
                .morale
                .as_mut()
                .expect("indexed combatant has morale");
            let prior = morale.clone();
            let mut losses = 0;
            let mut officer_present = false;
            let mut officer_lost = false;
            for ((handle, _), companion) in companions.iter().zip(&companion_snapshots) {
                let alive = companion
                    .as_ref()
                    .is_some_and(|companion| companion.lifecycle == EntityLifecycle::Alive);
                if alive {
                    morale.observed_allies.insert(handle.clone());
                    if morale.officer.as_ref() == Some(handle)
                        && companion.as_ref().is_some_and(|companion| {
                            let dx = companion.position.x - snapshot.position.x;
                            let dy = companion.position.y - snapshot.position.y;
                            let dz = companion.position.z - snapshot.position.z;
                            dx * dx + dy * dy + dz * dz <= 144.0
                        })
                    {
                        officer_present = true;
                    }
                } else if companion.is_some()
                    && morale.observed_allies.contains(handle)
                    && morale.witnessed_losses.insert(handle.clone())
                {
                    if morale.officer.as_ref() == Some(handle) {
                        officer_lost = true;
                    } else {
                        losses += 1;
                    }
                }
            }
            let target_uuid = order
                .active_target_ref
                .as_ref()
                .and_then(|target| {
                    storage
                        .resident_orders()
                        .reference(&record.plugin_id, target)
                })
                .and_then(|target| uuid::Uuid::parse_str(&target.entity_uuid).ok());
            let target = if let Some(target_uuid) = target_uuid {
                self.sessions()
                    .resident_entity_snapshots(&[target_uuid])
                    .await
                    .into_iter()
                    .next()
                    .flatten()
            } else {
                None
            };
            let flanked = target.as_ref().is_some_and(|target| {
                if target.lifecycle != EntityLifecycle::Alive {
                    return false;
                }
                let dx = target.position.x - snapshot.position.x;
                let dz = target.position.z - snapshot.position.z;
                let distance = dx.hypot(dz);
                if distance <= f64::EPSILON || distance > 16.0 {
                    return false;
                }
                let yaw = f64::from(snapshot.rotation.yaw).to_radians();
                ((-yaw.sin() * dx + yaw.cos() * dz) / distance) < 0.25
            });
            let rally = Vec3::new(
                f64::from(morale.rally.x) + 0.5,
                f64::from(morale.rally.y),
                f64::from(morale.rally.z) + 0.5,
            );
            let dx = rally.x - snapshot.position.x;
            let dz = rally.z - snapshot.position.z;
            let at_rally = dx * dx + dz * dz <= 4.0 && (rally.y - snapshot.position.y).abs() <= 2.0;
            morale.advance(MoraleObservation {
                health: snapshot.health,
                flanked,
                new_losses: losses,
                officer_present,
                officer_lost,
                at_rally,
            });
            let phase = morale.phase;
            if *morale != prior {
                storage.append_resident_order_change(
                    DurableResidentOrderChange::CombatProgress {
                        record: Box::new(record.clone()),
                    },
                )?;
            }
            let goal = match phase {
                MoralePhase::Routing => Some(
                    if self.route_blocked("minecraft:overworld", snapshot.position, rally) {
                        GoalState::Idle
                    } else {
                        GoalState::FollowPosition {
                            target: rally,
                            speed: 1.2,
                        }
                    },
                ),
                MoralePhase::Rallied => Some(GoalState::Idle),
                _ => None,
            };
            if let Some(goal) = goal {
                let applied = if snapshot.goal == goal {
                    true
                } else {
                    self.sessions()
                        .apply_resident_goals(vec![ResidentGoal { uuid, goal }])
                        .await
                        == 1
                };
                if applied && matches!(phase, MoralePhase::Rallied | MoralePhase::Surrendered) {
                    if let Some(journal) = self.sessions().world_chunk_journal() {
                        tokio::task::spawn_blocking(move || journal.writer.flush())
                            .await
                            .map_err(|error| {
                                super::PluginStorageMutationError::Io(std::io::Error::other(error))
                            })?
                            .map_err(|error| {
                                super::PluginStorageMutationError::Io(std::io::Error::other(error))
                            })?;
                    }
                    record
                        .morale
                        .as_mut()
                        .expect("terminal morale")
                        .goal_applied = true;
                    storage.append_resident_order_change(
                        DurableResidentOrderChange::CombatProgress {
                            record: Box::new(record),
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(health: f32) -> MoraleObservation {
        MoraleObservation {
            health,
            flanked: false,
            new_losses: 0,
            officer_present: false,
            officer_lost: false,
            at_rally: false,
        }
    }

    #[test]
    fn distinct_losses_and_flank_route_only_once_then_require_physical_rally() {
        let rally = ScriptBlockPosition::new(0, 64, 0);
        let mut morale = OperationalMorale::new(14.0, rally, Some("officer".into()));
        morale.advance(MoraleObservation {
            flanked: true,
            ..seen(14.0)
        });
        assert_eq!(morale.phase, MoralePhase::Shaken);
        morale.advance(MoraleObservation {
            flanked: true,
            ..seen(14.0)
        });
        assert_eq!(morale.phase, MoralePhase::Shaken);
        morale.advance(MoraleObservation {
            new_losses: 1,
            ..seen(14.0)
        });
        assert_eq!(morale.phase, MoralePhase::Wavering);
        morale.advance(seen(12.0));
        assert_eq!(morale.phase, MoralePhase::Routing);
        morale.advance(MoraleObservation {
            at_rally: true,
            ..seen(12.0)
        });
        assert_eq!(morale.phase, MoralePhase::Routing);
        morale.advance(MoraleObservation {
            at_rally: true,
            officer_present: true,
            ..seen(12.0)
        });
        assert_eq!(morale.phase, MoralePhase::Rallied);
    }
}
