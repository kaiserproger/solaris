//! Engine-side group order primitives for settlement residents.
//!
//! Three focused responsibilities, all pure engine state with no plugin or
//! journal dependency:
//!
//! * stable formation slots computed from an anchor and heading, so a squad
//!   keeps the same slot across updates and a member never stacks on another
//!   member's coordinate;
//! * the group-admission state machine that linearises one batch across
//!   regional owners: every member is fenced before anything is committed, an
//!   observation that drifts between prepare and commit invalidates the whole
//!   batch, and each owner applies an accepted batch exactly once;
//! * the bounded target policy filtered against already-available local
//!   perception, with allies excluded and player targets off by default.

use std::collections::{BTreeMap, BTreeSet};

use crate::{EntityId, RegionKey, Vec3};

/// Maximum members one group order or admission batch may address.
pub const MAX_GROUP_MEMBERS: usize = 64;
/// Maximum members of one gameplay squad.
pub const MAX_SQUAD_MEMBERS: usize = 32;
/// Maximum formation spacing in blocks.
pub const MAX_FORMATION_SPACING: f64 = 16.0;
/// Minimum formation spacing in blocks.
pub const MIN_FORMATION_SPACING: f64 = 0.5;
/// Maximum patrol waypoints on one order.
pub const MAX_PATROL_WAYPOINTS: usize = 16;
/// Minimum patrol waypoints on one order.
pub const MIN_PATROL_WAYPOINTS: usize = 2;
/// Maximum engagement radius in blocks.
pub const MAX_ENGAGEMENT_RADIUS: f64 = 64.0;
/// Maximum allied affiliations one policy may carry.
pub const MAX_POLICY_AFFILIATIONS: usize = 64;
/// Maximum members selected into one combat volley.
pub const MAX_ENGAGED_TARGETS: usize = 16;

/// Closed formation vocabulary shared with the script contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FormationKind {
    Line,
    Column,
    Wedge,
    Square,
}

impl FormationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Column => "column",
            Self::Wedge => "wedge",
            Self::Square => "square",
        }
    }
}

/// Engine-computed formation slot positions for one squad.
///
/// Slot index `0` is the leader at the anchor; every other slot is a distinct
/// offset derived from the heading, so two members never share a coordinate.
#[derive(Debug, Clone, PartialEq)]
pub struct FormationSlots {
    kind: FormationKind,
    anchor: Vec3,
    heading: f64,
    spacing: f64,
    slots: Vec<Vec3>,
}

impl FormationSlots {
    /// Compute `count` slots for `kind` around `anchor`.
    ///
    /// Returns `None` when the inputs are not finite, `count` is zero or above
    /// [`MAX_GROUP_MEMBERS`], or `spacing` leaves the closed range.
    #[must_use]
    pub fn compute(
        kind: FormationKind,
        anchor: Vec3,
        heading: f64,
        spacing: f64,
        count: usize,
    ) -> Option<Self> {
        if !anchor.is_finite()
            || !heading.is_finite()
            || !spacing.is_finite()
            || count == 0
            || count > MAX_GROUP_MEMBERS
            || !(MIN_FORMATION_SPACING..=MAX_FORMATION_SPACING).contains(&spacing)
        {
            return None;
        }
        let forward = Vec3::new(heading.sin(), 0.0, heading.cos());
        let right = Vec3::new(heading.cos(), 0.0, -heading.sin());
        let mut slots = Vec::with_capacity(count);
        slots.push(anchor);
        for index in 1..count {
            let (back, side) = match kind {
                FormationKind::Line => {
                    let side = if index % 2 == 1 {
                        (index as f64 + 1.0) / 2.0
                    } else {
                        -(index as f64 / 2.0)
                    };
                    (0.0, side)
                }
                FormationKind::Column => (index as f64, 0.0),
                FormationKind::Wedge => {
                    let row = (index as f64 + 1.0) / 2.0;
                    let side = if index % 2 == 1 { row } else { -row };
                    (row, side)
                }
                FormationKind::Square => {
                    let columns = (count as f64).sqrt().ceil().max(1.0);
                    let row = ((index as f64 - 1.0) / columns).floor() + 1.0;
                    let column = (index as f64 - 1.0) % columns;
                    (row, column - (columns - 1.0) / 2.0)
                }
            };
            slots.push(Vec3::new(
                anchor.x - forward.x * back * spacing + right.x * side * spacing,
                anchor.y,
                anchor.z - forward.z * back * spacing + right.z * side * spacing,
            ));
        }
        Some(Self {
            kind,
            anchor,
            heading,
            spacing,
            slots,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> FormationKind {
        self.kind
    }

    #[must_use]
    pub const fn anchor(&self) -> Vec3 {
        self.anchor
    }

    #[must_use]
    pub const fn heading(&self) -> f64 {
        self.heading
    }

    #[must_use]
    pub const fn spacing(&self) -> f64 {
        self.spacing
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Position of one slot index, or `None` past the end.
    #[must_use]
    pub fn slot(&self, index: usize) -> Option<Vec3> {
        self.slots.get(index).copied()
    }

    #[must_use]
    pub fn slots(&self) -> &[Vec3] {
        &self.slots
    }

    /// Re-derive the same squad in another formation with the same anchor and
    /// heading, preserving slot count.
    #[must_use]
    pub fn reform(&self, kind: FormationKind) -> Option<Self> {
        Self::compute(
            kind,
            self.anchor,
            self.heading,
            self.spacing,
            self.slots.len(),
        )
    }

    /// Assign members to distinct standable slots.
    ///
    /// Every member gets the lowest free slot whose position passes the
    /// obstacle predicate. When a member cannot be placed in a distinct,
    /// standable slot the whole formation reports [`FormationPlacement::BlockedRoute`]
    /// rather than stacking members on one coordinate.
    #[must_use]
    pub fn place_all(&self, count: usize, standable: impl Fn(Vec3) -> bool) -> FormationPlacement {
        if count == 0 || count > self.slots.len() {
            return FormationPlacement::BlockedRoute;
        }
        let mut used = BTreeSet::new();
        let mut placements = Vec::with_capacity(count);
        for _ in 0..count {
            let Some(index) = (0..self.slots.len())
                .find(|index| !used.contains(index) && standable(self.slots[*index]))
            else {
                return FormationPlacement::BlockedRoute;
            };
            used.insert(index);
            placements.push(FormationSlot {
                slot: index,
                position: self.slots[index],
            });
        }
        FormationPlacement::Placed(placements)
    }
}

/// One member's engine-computed slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormationSlot {
    pub slot: usize,
    pub position: Vec3,
}

/// Result of placing a squad into formation slots.
#[derive(Debug, Clone, PartialEq)]
pub enum FormationPlacement {
    Placed(Vec<FormationSlot>),
    BlockedRoute,
}

/// One member's fence inside a group-admission batch.
///
/// `K` is the durable member key (a stable handle or entity id); the batch is
/// keyed by it so a replay after restart addresses the same members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupMemberFence<K> {
    pub entity: K,
    pub region: RegionKey,
    pub order_revision: u64,
}

/// Observed member state used to validate a fence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupMemberObservation {
    pub alive: bool,
    pub loaded: bool,
    pub region: RegionKey,
    pub order_revision: u64,
}

/// Why one member cannot join a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupMemberFailure {
    NotFound,
    Dead,
    Unloaded,
    RegionChanged,
    StaleOrderRevision,
}

impl GroupMemberFailure {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Dead => "dead",
            Self::Unloaded => "unloaded",
            Self::RegionChanged => "region_changed",
            Self::StaleOrderRevision => "stale_order_revision",
        }
    }
}

/// One member's rejection reason. A rejected batch carries one for every member
/// it considered, so the plugin sees the whole picture rather than the first
/// failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupMemberRejection<K> {
    pub entity: K,
    pub failure: GroupMemberFailure,
}

/// Why a whole batch was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAdmissionRejection<K> {
    pub members: Vec<GroupMemberRejection<K>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupAdmissionPhase {
    Prepared,
    Committed,
    Rejected,
}

/// Outcome of applying one committed admission to one member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupApplyOutcome {
    Applied,
    AlreadyApplied,
    NotCommitted,
    NotMember,
}

/// Durable group-admission record.
///
/// The batch is all-or-nothing: `prepare` validates every member fence and
/// refuses the whole batch with per-member reasons, and `commit` re-validates
/// the fences so a migration, unload, death or order change between prepare
/// and commit invalidates the batch wholesale instead of applying a partial
/// set. Once committed, every member is applied exactly once; replaying the
/// same member returns [`GroupApplyOutcome::AlreadyApplied`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupAdmission<K> {
    admission_id: u64,
    fingerprint: [u8; 32],
    phase: GroupAdmissionPhase,
    members: BTreeMap<K, GroupMemberFence<K>>,
    applied: BTreeSet<K>,
}

impl<K: Ord + Clone> GroupAdmission<K> {
    /// Prepare a batch after fencing every member.
    pub fn prepare(
        admission_id: u64,
        fingerprint: [u8; 32],
        requests: &[(GroupMemberFence<K>, Option<GroupMemberObservation>)],
    ) -> Result<Self, GroupAdmissionRejection<K>> {
        if requests.is_empty() || requests.len() > MAX_GROUP_MEMBERS {
            return Err(GroupAdmissionRejection {
                members: Vec::new(),
            });
        }
        let mut members = BTreeMap::new();
        let mut reasons = Vec::new();
        for (fence, observation) in requests {
            if members
                .insert(fence.entity.clone(), fence.clone())
                .is_some()
            {
                return Err(GroupAdmissionRejection {
                    members: vec![GroupMemberRejection {
                        entity: fence.entity.clone(),
                        failure: GroupMemberFailure::StaleOrderRevision,
                    }],
                });
            }
            if let Some(failure) = fence_failure(fence, *observation) {
                reasons.push(GroupMemberRejection {
                    entity: fence.entity.clone(),
                    failure,
                });
            }
        }
        if !reasons.is_empty() {
            reasons.sort_unstable_by(|left, right| left.entity.cmp(&right.entity));
            return Err(GroupAdmissionRejection { members: reasons });
        }
        Ok(Self {
            admission_id,
            fingerprint,
            phase: GroupAdmissionPhase::Prepared,
            members,
            applied: BTreeSet::new(),
        })
    }

    /// Commit the batch, re-validating every fence against fresh observations.
    ///
    /// Any drift invalidates the whole batch and marks it rejected.
    pub fn commit(
        &mut self,
        observations: &BTreeMap<K, GroupMemberObservation>,
    ) -> Result<(), GroupAdmissionRejection<K>> {
        if self.phase != GroupAdmissionPhase::Prepared {
            return Err(GroupAdmissionRejection {
                members: Vec::new(),
            });
        }
        let mut reasons = Vec::new();
        for (entity, fence) in &self.members {
            if let Some(failure) = fence_failure(fence, observations.get(entity).copied()) {
                reasons.push(GroupMemberRejection {
                    entity: entity.clone(),
                    failure,
                });
            }
        }
        if !reasons.is_empty() {
            reasons.sort_unstable_by(|left, right| left.entity.cmp(&right.entity));
            self.phase = GroupAdmissionPhase::Rejected;
            return Err(GroupAdmissionRejection { members: reasons });
        }
        self.phase = GroupAdmissionPhase::Committed;
        Ok(())
    }

    /// Mark the batch rejected without committing it.
    pub fn reject(&mut self) {
        if self.phase == GroupAdmissionPhase::Prepared {
            self.phase = GroupAdmissionPhase::Rejected;
        }
    }

    /// Apply the committed batch to one member exactly once.
    pub fn apply(&mut self, entity: K) -> GroupApplyOutcome {
        if self.phase != GroupAdmissionPhase::Committed {
            return GroupApplyOutcome::NotCommitted;
        }
        if !self.members.contains_key(&entity) {
            return GroupApplyOutcome::NotMember;
        }
        if !self.applied.insert(entity) {
            return GroupApplyOutcome::AlreadyApplied;
        }
        GroupApplyOutcome::Applied
    }

    #[must_use]
    pub const fn phase(&self) -> GroupAdmissionPhase {
        self.phase
    }

    #[must_use]
    pub const fn admission_id(&self) -> u64 {
        self.admission_id
    }

    #[must_use]
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    pub fn members(&self) -> impl Iterator<Item = (&K, &GroupMemberFence<K>)> + '_ {
        self.members.iter()
    }

    #[must_use]
    pub fn applied(&self) -> usize {
        self.applied.len()
    }

    /// Members still waiting for their owner to apply the committed batch.
    #[must_use]
    pub fn pending(&self) -> Vec<K> {
        self.members
            .keys()
            .filter(|entity| !self.applied.contains(entity))
            .cloned()
            .collect()
    }

    /// Regions that still have unapplied members.
    #[must_use]
    pub fn pending_regions(&self) -> BTreeSet<RegionKey> {
        self.members
            .iter()
            .filter(|(entity, _)| !self.applied.contains(entity))
            .map(|(_, fence)| fence.region)
            .collect()
    }
}

fn fence_failure<K>(
    fence: &GroupMemberFence<K>,
    observation: Option<GroupMemberObservation>,
) -> Option<GroupMemberFailure> {
    let Some(observation) = observation else {
        return Some(GroupMemberFailure::NotFound);
    };
    if !observation.alive {
        return Some(GroupMemberFailure::Dead);
    }
    if !observation.loaded {
        return Some(GroupMemberFailure::Unloaded);
    }
    if observation.region != fence.region {
        return Some(GroupMemberFailure::RegionChanged);
    }
    if observation.order_revision != fence.order_revision {
        return Some(GroupMemberFailure::StaleOrderRevision);
    }
    None
}

/// Closed hostile category used by the bounded target policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TargetCategory {
    Hostile,
    Player,
    OwnedResident,
    NeutralAnimal,
}

impl TargetCategory {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hostile => "hostile",
            Self::Player => "player",
            Self::OwnedResident => "owned_resident",
            Self::NeutralAnimal => "neutral_animal",
        }
    }
}

/// Bounded ally/enemy policy supplied by the plugin with a revision.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetPolicy {
    revision: u64,
    engagement_radius: f64,
    affiliations: BTreeSet<String>,
    permitted: BTreeSet<TargetCategory>,
}

impl TargetPolicy {
    /// Build a policy. `Player` is only hostile when it is explicitly listed in
    /// `permitted`, so player targets default off.
    pub fn new(
        revision: u64,
        engagement_radius: f64,
        affiliations: impl IntoIterator<Item = String>,
        permitted: impl IntoIterator<Item = TargetCategory>,
    ) -> Option<Self> {
        let affiliations = affiliations.into_iter().collect::<BTreeSet<_>>();
        let permitted = permitted.into_iter().collect::<BTreeSet<_>>();
        if !engagement_radius.is_finite()
            || engagement_radius <= 0.0
            || engagement_radius > MAX_ENGAGEMENT_RADIUS
            || affiliations.len() > MAX_POLICY_AFFILIATIONS
            || affiliations
                .iter()
                .any(|value| value.is_empty() || value.len() > 64)
        {
            return None;
        }
        Some(Self {
            revision,
            engagement_radius,
            affiliations,
            permitted,
        })
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn engagement_radius(&self) -> f64 {
        self.engagement_radius
    }

    /// Whether one affiliation id is allied and therefore never targeted.
    #[must_use]
    pub fn is_allied(&self, affiliation: Option<&str>) -> bool {
        affiliation.is_some_and(|affiliation| self.affiliations.contains(affiliation))
    }

    #[must_use]
    pub fn permits(&self, category: TargetCategory) -> bool {
        self.permitted.contains(&category)
    }
}

/// One candidate already visible in the local perception radius.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetCandidate {
    pub entity: EntityId,
    pub position: Vec3,
    pub category: TargetCategory,
    pub affiliation: Option<String>,
}

/// Select the nearest permitted, non-allied candidate inside the engagement
/// radius. Perception is supplied by the caller; this never scans the world.
#[must_use]
pub fn select_target(
    policy: &TargetPolicy,
    origin: Vec3,
    candidates: &[TargetCandidate],
) -> Option<EntityId> {
    if !origin.is_finite() {
        return None;
    }
    let radius_squared = policy.engagement_radius * policy.engagement_radius;
    let mut best: Option<(f64, EntityId)> = None;
    for candidate in candidates {
        if !policy.permits(candidate.category)
            || policy.is_allied(candidate.affiliation.as_deref())
            || !candidate.position.is_finite()
        {
            continue;
        }
        let distance = distance_squared(origin, candidate.position);
        if distance > radius_squared {
            continue;
        }
        let key = (distance, candidate.entity);
        if best.is_none_or(|current| key < current) {
            best = Some(key);
        }
    }
    best.map(|(_, entity)| entity)
}

/// Deterministic nearest-first selection order for a whole volley, bounded by
/// [`MAX_ENGAGED_TARGETS`].
#[must_use]
pub fn select_targets(
    policy: &TargetPolicy,
    origin: Vec3,
    candidates: &[TargetCandidate],
) -> Vec<EntityId> {
    if !origin.is_finite() {
        return Vec::new();
    }
    let radius_squared = policy.engagement_radius * policy.engagement_radius;
    let mut permitted = candidates
        .iter()
        .filter(|candidate| {
            policy.permits(candidate.category)
                && !policy.is_allied(candidate.affiliation.as_deref())
                && candidate.position.is_finite()
                && distance_squared(origin, candidate.position) <= radius_squared
        })
        .map(|candidate| {
            (
                distance_squared(origin, candidate.position),
                candidate.entity,
            )
        })
        .collect::<Vec<_>>();
    permitted.sort_unstable_by(|left, right| {
        left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
    });
    permitted.truncate(MAX_ENGAGED_TARGETS);
    permitted.into_iter().map(|(_, entity)| entity).collect()
}

fn distance_squared(left: Vec3, right: Vec3) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    dx.mul_add(dx, dy.mul_add(dy, dz * dz))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn formation_slots_are_distinct_and_stable_across_reform() {
        let line = FormationSlots::compute(FormationKind::Line, v(0.0, 64.0, 0.0), 0.0, 2.0, 6)
            .expect("line");
        let mut positions = line.slots().to_vec();
        positions.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        positions.dedup();
        assert_eq!(positions.len(), 6, "every slot is a distinct coordinate");
        let reform = line.reform(FormationKind::Column).expect("column");
        assert_eq!(reform.len(), 6);
        // Slot 0 stays the leader anchor across a reform.
        assert_eq!(reform.slot(0), line.slot(0));
        assert_eq!(line.slot(0), Some(v(0.0, 64.0, 0.0)));
        // Column stacks behind the leader, never on it.
        assert_ne!(reform.slot(1), reform.slot(0));
    }

    #[test]
    fn closed_passage_blocks_the_whole_formation_without_stacking() {
        let slots =
            FormationSlots::compute(FormationKind::Line, v(0.0, 64.0, 0.0), 0.0, 2.0, 4).unwrap();
        assert!(matches!(
            slots.place_all(4, |_| true),
            FormationPlacement::Placed(_)
        ));
        // All slots east of x = 1 are behind a wall; the rest fit.
        let placement = slots.place_all(4, |position| position.x <= 1.0);
        assert_eq!(placement, FormationPlacement::BlockedRoute);
        // A squad that fits in the open part still gets distinct slots.
        match slots.place_all(2, |position| position.x <= 3.0) {
            FormationPlacement::Placed(placed) => {
                assert_eq!(placed.len(), 2);
                assert_ne!(placed[0].slot, placed[1].slot);
            }
            FormationPlacement::BlockedRoute => panic!("two slots fit"),
        }
    }

    #[test]
    fn partial_unavailability_rejects_the_whole_batch_with_reasons() {
        let a = EntityId(1);
        let b = EntityId(2);
        let fence = |entity| GroupMemberFence {
            entity,
            region: RegionKey::new(0, 0),
            order_revision: 7,
        };
        let observation = GroupMemberObservation {
            alive: true,
            loaded: true,
            region: RegionKey::new(0, 0),
            order_revision: 7,
        };
        let mut dead = observation;
        dead.alive = false;
        let rejection = GroupAdmission::<EntityId>::prepare(
            1,
            [0; 32],
            &[(fence(a), Some(observation)), (fence(b), Some(dead))],
        )
        .expect_err("one dead member rejects the batch");
        assert_eq!(
            rejection.members,
            vec![GroupMemberRejection {
                entity: b,
                failure: GroupMemberFailure::Dead
            }]
        );
    }

    #[test]
    fn drift_between_prepare_and_commit_invalidates_the_batch() {
        let a = EntityId(1);
        let fence = GroupMemberFence {
            entity: a,
            region: RegionKey::new(0, 0),
            order_revision: 3,
        };
        let observation = GroupMemberObservation {
            alive: true,
            loaded: true,
            region: RegionKey::new(0, 0),
            order_revision: 3,
        };
        let mut admission =
            GroupAdmission::<EntityId>::prepare(5, [1; 32], &[(fence, Some(observation))])
                .expect("prepare");
        let migrated = GroupMemberObservation {
            region: RegionKey::new(1, 0),
            ..observation
        };
        let rejection = admission
            .commit(&BTreeMap::from([(a, migrated)]))
            .expect_err("migration invalidates");
        assert_eq!(
            rejection.members[0].failure,
            GroupMemberFailure::RegionChanged
        );
        assert_eq!(admission.phase(), GroupAdmissionPhase::Rejected);
        assert_eq!(admission.apply(a), GroupApplyOutcome::NotCommitted);
    }

    #[test]
    fn commit_applies_every_member_exactly_once() {
        let members = [EntityId(1), EntityId(2)];
        let requests = members
            .iter()
            .map(|entity| {
                (
                    GroupMemberFence {
                        entity: *entity,
                        region: RegionKey::new(0, 0),
                        order_revision: 2,
                    },
                    Some(GroupMemberObservation {
                        alive: true,
                        loaded: true,
                        region: RegionKey::new(0, 0),
                        order_revision: 2,
                    }),
                )
            })
            .collect::<Vec<_>>();
        let mut admission =
            GroupAdmission::<EntityId>::prepare(9, [2; 32], &requests).expect("prepare");
        assert_eq!(admission.pending().len(), 2);
        admission
            .commit(
                &members
                    .iter()
                    .map(|entity| {
                        (
                            *entity,
                            GroupMemberObservation {
                                alive: true,
                                loaded: true,
                                region: RegionKey::new(0, 0),
                                order_revision: 2,
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
            )
            .expect("commit");
        assert_eq!(admission.apply(members[0]), GroupApplyOutcome::Applied);
        assert_eq!(
            admission.apply(members[0]),
            GroupApplyOutcome::AlreadyApplied
        );
        assert_eq!(admission.apply(members[1]), GroupApplyOutcome::Applied);
        assert_eq!(admission.pending(), Vec::<EntityId>::new());
        assert_eq!(admission.apply(EntityId(3)), GroupApplyOutcome::NotMember);
    }

    #[test]
    fn target_policy_excludes_allies_and_keeps_players_off_by_default() {
        let policy = TargetPolicy::new(
            4,
            16.0,
            ["resident:1".to_owned()],
            [TargetCategory::Hostile],
        )
        .expect("policy");
        let candidates = vec![
            TargetCandidate {
                entity: EntityId(1),
                position: v(2.0, 64.0, 0.0),
                category: TargetCategory::Hostile,
                affiliation: None,
            },
            TargetCandidate {
                entity: EntityId(2),
                position: v(1.0, 64.0, 0.0),
                category: TargetCategory::OwnedResident,
                affiliation: Some("resident:1".to_owned()),
            },
            TargetCandidate {
                entity: EntityId(3),
                position: v(0.5, 64.0, 0.0),
                category: TargetCategory::Player,
                affiliation: None,
            },
        ];
        assert_eq!(
            select_target(&policy, v(0.0, 64.0, 0.0), &candidates),
            Some(EntityId(1))
        );
        assert!(policy.is_allied(Some("resident:1")));
        assert!(!policy.permits(TargetCategory::Player));
    }

    #[test]
    fn target_selection_is_bounded_and_nearest_first() {
        let policy = TargetPolicy::new(
            1,
            32.0,
            std::iter::empty::<String>(),
            [TargetCategory::Hostile],
        )
        .expect("policy");
        let candidates = (0..32)
            .map(|index| TargetCandidate {
                entity: EntityId(index),
                position: v(f64::from(index), 64.0, 0.0),
                category: TargetCategory::Hostile,
                affiliation: None,
            })
            .collect::<Vec<_>>();
        let selected = select_targets(&policy, v(0.0, 64.0, 0.0), &candidates);
        assert_eq!(selected.len(), MAX_ENGAGED_TARGETS);
        assert_eq!(selected[0], EntityId(0));
    }
}
