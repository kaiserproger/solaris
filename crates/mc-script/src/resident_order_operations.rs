//! Resident work orders, squad orders, combat events, and demobilisation DTOs.
//!
//! These types are the closed wire contract for `solaris.assign_resident_work`,
//! `solaris.cancel_resident_work`, `solaris.issue_resident_order`,
//! `solaris.cancel_resident_order` and `solaris.demobilize_resident`. They carry
//! no authoritative world state: owner comes from the admitted plugin, actor
//! from the authenticated session, and every target reference is server-issued
//! and validated against the durable record before it can be used.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    MAX_SCRIPT_WORLD_TIME, ScriptDtoError, ScriptInventoryEndpoint, check_contract_resource_id,
    validate_bounded_nonempty, validate_bounded_value,
};

/// Maximum opaque handles one squad order may address.
pub const MAX_RESIDENT_ORDER_HANDLES: usize = 64;
/// Maximum members of one gameplay squad; core admits up to
/// [`MAX_RESIDENT_ORDER_HANDLES`].
pub const MAX_RESIDENT_SQUAD_MEMBERS: usize = 32;
/// Maximum patrol waypoints on one order.
pub const MAX_ORDER_WAYPOINTS: usize = 16;
/// Minimum patrol waypoints on one order.
pub const MIN_ORDER_WAYPOINTS: usize = 2;
/// Maximum allied affiliations on one engagement policy.
pub const MAX_ORDER_AFFILIATIONS: usize = 64;
/// Maximum target references on one attack order.
pub const MAX_ORDER_TARGETS: usize = 64;
/// Maximum byte length of an opaque server-issued target reference.
pub const MAX_TARGET_REF_BYTES: usize = 64;
/// Maximum byte length of a permitted crafting recipe id.
pub const MAX_RECIPE_ID_BYTES: usize = 128;
/// Maximum extent of one work area axis in blocks.
pub const MAX_WORK_AREA_AXIS: u32 = 16;
/// Maximum work units one assignment may commit.
pub const MAX_WORK_UNITS: u64 = 4096;
/// Maximum formation spacing in half-blocks (16 blocks).
pub const MAX_FORMATION_SPACING: u8 = 32;
/// Minimum formation spacing in half-blocks (0.5 blocks).
pub const MIN_FORMATION_SPACING: u8 = 1;
/// Maximum engagement radius in blocks.
pub const MAX_ENGAGEMENT_RADIUS: u8 = 64;
/// Minimum engagement radius in blocks.
pub const MIN_ENGAGEMENT_RADIUS: u8 = 1;
/// Largest absolute block coordinate accepted by a work area.
pub const MAX_SCRIPT_BLOCK_COORDINATE: i32 = 30_000_000;
/// Maximum ordered residents one combat event batch may carry.
pub const MAX_COMBAT_EVENTS: usize = 16;

/// One integer block coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptBlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ScriptBlockPosition {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        for value in [self.x, self.y, self.z] {
            if value.abs() > MAX_SCRIPT_BLOCK_COORDINATE {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }
}

/// One bounded axis-aligned work area the engine executes against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorkArea {
    pub dimension: String,
    pub min: ScriptBlockPosition,
    pub max: ScriptBlockPosition,
}

impl ScriptWorkArea {
    #[must_use]
    pub fn new(dimension: String, min: ScriptBlockPosition, max: ScriptBlockPosition) -> Self {
        Self {
            dimension,
            min,
            max,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("work dimension", &self.dimension, 128)?;
        self.min.validate()?;
        self.max.validate()?;
        let axes = [
            (self.min.x, self.max.x),
            (self.min.y, self.max.y),
            (self.min.z, self.max.z),
        ];
        for (min, max) in axes {
            if min > max {
                return Err(ScriptDtoError::InvalidBounds);
            }
            let extent = u32::try_from(i64::from(max) - i64::from(min) + 1)
                .map_err(|_| ScriptDtoError::InvalidBounds)?;
            if extent == 0 || extent > MAX_WORK_AREA_AXIS {
                return Err(ScriptDtoError::InvalidBounds);
            }
        }
        Ok(())
    }

    /// Number of cells in the area, already bounded by [`MAX_WORK_AREA_AXIS`].
    #[must_use]
    pub fn cell_count(&self) -> u32 {
        let x = (self.max.x - self.min.x + 1).unsigned_abs();
        let y = (self.max.y - self.min.y + 1).unsigned_abs();
        let z = (self.max.z - self.min.z + 1).unsigned_abs();
        x.saturating_mul(y).saturating_mul(z)
    }
}

/// Closed formation vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptFormationKind {
    Line,
    Column,
    Wedge,
    Square,
}

impl ScriptFormationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Column => "column",
            Self::Wedge => "wedge",
            Self::Square => "square",
        }
    }
}

/// Engine-rendered formation for one order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptFormation {
    pub kind: ScriptFormationKind,
    /// Spacing between slots in half-blocks (integer; `4` means two blocks).
    pub spacing: u8,
}

impl ScriptFormation {
    #[must_use]
    pub const fn new(kind: ScriptFormationKind, spacing: u8) -> Self {
        Self { kind, spacing }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if !(MIN_FORMATION_SPACING..=MAX_FORMATION_SPACING).contains(&self.spacing) {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Closed hostile category a target reference may address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptHostileCategory {
    Hostile,
    Player,
    OwnedResident,
    NeutralAnimal,
}

impl ScriptHostileCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hostile => "hostile",
            Self::Player => "player",
            Self::OwnedResident => "owned_resident",
            Self::NeutralAnimal => "neutral_animal",
        }
    }
}

/// Bounded ally/enemy policy supplied with an attack order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptEngagementPolicy {
    pub revision: u64,
    /// Opaque alliance ids (owned resident handles, allied player uuids).
    pub allies: Vec<String>,
    pub permitted: Vec<ScriptHostileCategory>,
}

impl ScriptEngagementPolicy {
    #[must_use]
    pub fn new(revision: u64, allies: Vec<String>, permitted: Vec<ScriptHostileCategory>) -> Self {
        Self {
            revision,
            allies,
            permitted,
        }
    }

    pub fn canonicalize(&mut self) {
        self.allies.sort_unstable();
        self.allies.dedup();
        self.permitted
            .sort_unstable_by_key(|category| category.as_str());
        self.permitted.dedup();
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || self.allies.len() > MAX_ORDER_AFFILIATIONS
            || self.permitted.is_empty()
            || self.permitted.len() > 4
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        let mut previous = None;
        for ally in &self.allies {
            validate_bounded_nonempty("order ally", ally, MAX_TARGET_REF_BYTES)?;
            if previous.is_some_and(|value: &str| value >= ally.as_str()) {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "order allies",
                });
            }
            previous = Some(ally.as_str());
        }
        let mut previous = None;
        for category in &self.permitted {
            if previous.is_some_and(|value: &str| value >= category.as_str()) {
                return Err(ScriptDtoError::InconsistentResult {
                    field: "permitted categories",
                });
            }
            previous = Some(category.as_str());
        }
        Ok(())
    }
}

/// One opaque, server-issued target reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOrderTargetRef {
    pub target_ref: String,
    pub policy_revision: u64,
    pub expires_revision: u64,
}

impl ScriptOrderTargetRef {
    #[must_use]
    pub fn new(target_ref: String, policy_revision: u64, expires_revision: u64) -> Self {
        Self {
            target_ref,
            policy_revision,
            expires_revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("target ref", &self.target_ref, MAX_TARGET_REF_BYTES)?;
        if self.policy_revision > MAX_SCRIPT_WORLD_TIME
            || self.expires_revision > MAX_SCRIPT_WORLD_TIME
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Closed work-order union. Each variant names a concrete bounded target and
/// the tool, feed or recipe the engine requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptResidentWorkOrder {
    Harvest {
        area: ScriptWorkArea,
        tool: String,
    },
    Replant {
        area: ScriptWorkArea,
        seed: String,
        tool: String,
    },
    CutTree {
        area: ScriptWorkArea,
        tool: String,
    },
    Mine {
        area: ScriptWorkArea,
        tool: String,
    },
    Fish {
        area: ScriptWorkArea,
        tool: String,
    },
    TendLivestock {
        area: ScriptWorkArea,
        feed: String,
    },
    Haul {
        source: ScriptInventoryEndpoint,
        destination: ScriptInventoryEndpoint,
        /// The exact item this move takes from the source.
        ///
        /// `None` moves whatever the source holds first, in slot order. A
        /// withdrawal from a settlement warehouse names the item it takes -
        /// a worker that needs a hoe must not receive whatever happened to be
        /// in the first slot - so the plugin states the selection instead of
        /// relying on storage order.
        item: Option<String>,
    },
    Craft {
        recipe: String,
        count: u32,
        /// One loaded crafting-table cell the native engine verifies.
        station: ScriptWorkArea,
    },
    Construct {
        structure_id: String,
        stage: String,
        expected_revision: u64,
    },
}

impl ScriptResidentWorkOrder {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Harvest { area, tool }
            | Self::CutTree { area, tool }
            | Self::Mine { area, tool }
            | Self::Fish { area, tool } => {
                area.validate()?;
                check_contract_resource_id(tool)
            }
            Self::Replant { area, seed, tool } => {
                area.validate()?;
                check_contract_resource_id(seed)?;
                check_contract_resource_id(tool)
            }
            Self::TendLivestock { area, feed } => {
                area.validate()?;
                check_contract_resource_id(feed)
            }
            Self::Haul {
                source,
                destination,
                item,
            } => {
                source.validate()?;
                destination.validate()?;
                if source == destination {
                    return Err(ScriptDtoError::InconsistentResult {
                        field: "haul endpoints",
                    });
                }
                if let Some(item) = item {
                    check_contract_resource_id(item)?;
                }
                Ok(())
            }
            Self::Craft {
                recipe,
                count,
                station,
            } => {
                validate_bounded_nonempty("recipe", recipe, MAX_RECIPE_ID_BYTES)?;
                station.validate()?;
                if station.min != station.max || *count == 0 || *count > i32::MAX as u32 {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Construct {
                structure_id,
                stage,
                expected_revision,
            } => {
                validate_bounded_nonempty("structure id", structure_id, 64)?;
                validate_bounded_nonempty("structure stage", stage, 64)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
        }
    }
}

/// Closed squad-order union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptResidentOrder {
    Follow {
        target_player: u64,
        formation: ScriptFormation,
    },
    Move {
        dimension: String,
        anchor: ScriptBlockPosition,
        heading_degrees: u16,
        formation: ScriptFormation,
    },
    Hold {
        anchor: ScriptBlockPosition,
        heading_degrees: u16,
        formation: ScriptFormation,
        engagement_radius: u8,
    },
    Patrol {
        waypoints: Vec<ScriptBlockPosition>,
        formation: ScriptFormation,
        engagement_radius: u8,
    },
    Garrison {
        posts: Vec<String>,
        engagement_radius: u8,
    },
    Attack {
        targets: Vec<ScriptOrderTargetRef>,
        policy: ScriptEngagementPolicy,
    },
    Retreat {
        anchor: ScriptBlockPosition,
        formation: ScriptFormation,
    },
}

impl ScriptResidentOrder {
    /// Canonical ordering so one operation fingerprint is input-order stable.
    pub fn canonicalize(&mut self) {
        match self {
            Self::Attack { targets, policy } => {
                targets.sort_unstable_by(|left, right| left.target_ref.cmp(&right.target_ref));
                policy.canonicalize();
            }
            Self::Garrison { posts, .. } => {
                posts.sort_unstable();
                posts.dedup();
            }
            Self::Patrol { waypoints, .. } => {
                // Waypoint order is the patrol route and is never reordered.
                waypoints.dedup();
            }
            _ => {}
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Follow {
                target_player,
                formation,
            } => {
                if *target_player == 0 || *target_player > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                formation.validate()
            }
            Self::Move {
                dimension,
                anchor,
                heading_degrees,
                formation,
            } => {
                validate_bounded_nonempty("order dimension", dimension, 128)?;
                anchor.validate()?;
                validate_heading(*heading_degrees)?;
                formation.validate()
            }
            Self::Hold {
                anchor,
                heading_degrees,
                formation,
                engagement_radius,
            } => {
                anchor.validate()?;
                validate_heading(*heading_degrees)?;
                formation.validate()?;
                validate_engagement_radius(*engagement_radius)
            }
            Self::Patrol {
                waypoints,
                formation,
                engagement_radius,
            } => {
                if waypoints.len() < MIN_ORDER_WAYPOINTS || waypoints.len() > MAX_ORDER_WAYPOINTS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for waypoint in waypoints {
                    waypoint.validate()?;
                    if previous.is_some_and(|value: &ScriptBlockPosition| value == waypoint) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "patrol waypoints",
                        });
                    }
                    previous = Some(waypoint);
                }
                formation.validate()?;
                validate_engagement_radius(*engagement_radius)
            }
            Self::Garrison {
                posts,
                engagement_radius,
            } => {
                if posts.is_empty() || posts.len() > MAX_ORDER_TARGETS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for post in posts {
                    validate_bounded_nonempty("garrison post", post, 128)?;
                    if previous.is_some_and(|value: &str| value >= post.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "garrison posts",
                        });
                    }
                    previous = Some(post.as_str());
                }
                validate_engagement_radius(*engagement_radius)
            }
            Self::Attack { targets, policy } => {
                if targets.is_empty() || targets.len() > MAX_ORDER_TARGETS {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                let mut previous = None;
                for target in targets {
                    target.validate()?;
                    if previous.is_some_and(|value: &str| value >= target.target_ref.as_str()) {
                        return Err(ScriptDtoError::InconsistentResult {
                            field: "attack targets",
                        });
                    }
                    previous = Some(target.target_ref.as_str());
                }
                policy.validate()
            }
            Self::Retreat { anchor, formation } => {
                anchor.validate()?;
                formation.validate()
            }
        }
    }
}

fn validate_heading(heading_degrees: u16) -> Result<(), ScriptDtoError> {
    if heading_degrees >= 360 {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

fn validate_engagement_radius(engagement_radius: u8) -> Result<(), ScriptDtoError> {
    if !(MIN_ENGAGEMENT_RADIUS..=MAX_ENGAGEMENT_RADIUS).contains(&engagement_radius) {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

/// One bounded resident work or order call inside the durable operation
/// envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptResidentOrderOperation {
    AssignWork {
        operation_id: String,
        handle: String,
        work: ScriptResidentWorkOrder,
        work_units: u64,
        expected_revision: u64,
    },
    CancelWork {
        operation_id: String,
        handle: String,
        expected_revision: u64,
    },
    IssueOrder {
        operation_id: String,
        handles: Vec<String>,
        expected_order_revisions: Vec<u64>,
        order: ScriptResidentOrder,
    },
    CancelOrder {
        operation_id: String,
        handles: Vec<String>,
        expected_order_revisions: Vec<u64>,
    },
    Demobilize {
        operation_id: String,
        handle: String,
        expected_revision: u64,
    },
}

impl ScriptResidentOrderOperation {
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::AssignWork { operation_id, .. }
            | Self::CancelWork { operation_id, .. }
            | Self::IssueOrder { operation_id, .. }
            | Self::CancelOrder { operation_id, .. }
            | Self::Demobilize { operation_id, .. } => Some(operation_id),
        }
    }

    pub fn canonicalize(&mut self) {
        match self {
            // A work order's fields are roles, not an unordered set: a haul is
            // directed, so nothing in it may be reordered. `Craft`, the work
            // area bounds and the tool names are already canonical.
            Self::IssueOrder {
                handles,
                expected_order_revisions,
                order,
                ..
            } => {
                order.canonicalize();
                canonicalize_handles(handles, expected_order_revisions, true);
            }
            Self::CancelOrder {
                handles,
                expected_order_revisions,
                ..
            } => canonicalize_handles(handles, expected_order_revisions, true),
            _ => {}
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        if let Some(operation_id) = self.operation_id() {
            crate::validate_script_id_value(operation_id)?;
        }
        match self {
            Self::AssignWork {
                handle,
                work,
                work_units,
                expected_revision,
                ..
            } => {
                crate::validate_resident_handle(handle)?;
                work.validate()?;
                if *work_units == 0
                    || *work_units > MAX_WORK_UNITS
                    || *expected_revision > MAX_SCRIPT_WORLD_TIME
                {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::CancelWork {
                handle,
                expected_revision,
                ..
            } => {
                crate::validate_resident_handle(handle)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::IssueOrder {
                handles,
                expected_order_revisions,
                order,
                ..
            } => {
                validate_order_members(handles, expected_order_revisions)?;
                order.validate()
            }
            Self::CancelOrder {
                handles,
                expected_order_revisions,
                ..
            } => validate_order_members(handles, expected_order_revisions),
            Self::Demobilize {
                handle,
                expected_revision,
                ..
            } => {
                crate::validate_resident_handle(handle)?;
                if *expected_revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
        }
    }
}

fn canonicalize_handles(
    handles: &mut Vec<String>,
    expected_order_revisions: &mut Vec<u64>,
    sort: bool,
) {
    if !sort {
        return;
    }
    let mut paired = handles
        .iter()
        .cloned()
        .zip(expected_order_revisions.iter().copied())
        .collect::<Vec<_>>();
    paired.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    paired.dedup_by(|left, right| left.0 == right.0);
    *handles = paired.iter().map(|(handle, _)| handle.clone()).collect();
    *expected_order_revisions = paired.into_iter().map(|(_, revision)| revision).collect();
}

fn validate_order_members(
    handles: &[String],
    expected_order_revisions: &[u64],
) -> Result<(), ScriptDtoError> {
    if handles.is_empty()
        || handles.len() > MAX_RESIDENT_ORDER_HANDLES
        || handles.len() != expected_order_revisions.len()
    {
        return Err(ScriptDtoError::InvalidBounds);
    }
    let mut unique = BTreeSet::new();
    for (handle, revision) in handles.iter().zip(expected_order_revisions) {
        crate::validate_resident_handle(handle)?;
        if !unique.insert(handle.as_str()) {
            return Err(ScriptDtoError::DuplicateId {
                field: "order handle",
                actual_bytes: handle.len(),
            });
        }
        if *revision > MAX_SCRIPT_WORLD_TIME {
            return Err(ScriptDtoError::InvalidBounds);
        }
    }
    Ok(())
}

/// Closed pause reason for a work assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptWorkPauseReason {
    Unloaded,
    NoWorkers,
    MissingInput,
    MissingTool,
    MissingStation,
    BlockedRoute,
    Interrupted,
    Protected,
    Unsupported,
    NoStorage,
}

impl ScriptWorkPauseReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unloaded => "unloaded",
            Self::NoWorkers => "no_workers",
            Self::MissingInput => "missing_input",
            Self::MissingTool => "missing_tool",
            Self::MissingStation => "missing_station",
            Self::BlockedRoute => "blocked_route",
            Self::Interrupted => "interrupted",
            Self::Protected => "protected",
            Self::Unsupported => "unsupported",
            Self::NoStorage => "no_storage",
        }
    }
}

/// Lifecycle state of one work assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptWorkState {
    Accepted,
    Running,
    Paused,
    Committed,
    Cancelled,
}

impl ScriptWorkState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Committed => "committed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One committed inventory change. Only real committed deltas are reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptItemChange {
    pub item_id: String,
    /// Signed change: positive produced, negative consumed.
    pub delta: i64,
}

impl ScriptItemChange {
    #[must_use]
    pub fn new(item_id: String, delta: i64) -> Self {
        Self { item_id, delta }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        check_contract_resource_id(&self.item_id)?;
        if self.delta == 0 || self.delta == i64::MIN {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// One work assignment result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptWorkAssignment {
    pub handle: String,
    pub state: ScriptWorkState,
    pub reason: Option<ScriptWorkPauseReason>,
    pub work_units_done: u64,
    pub work_units_planned: u64,
    pub changes: Vec<ScriptItemChange>,
    pub revision: u64,
}

impl ScriptWorkAssignment {
    #[must_use]
    pub fn new(
        handle: String,
        state: ScriptWorkState,
        reason: Option<ScriptWorkPauseReason>,
        work_units_done: u64,
        work_units_planned: u64,
        mut changes: Vec<ScriptItemChange>,
        revision: u64,
    ) -> Self {
        changes.sort_unstable_by(|left, right| left.item_id.cmp(&right.item_id));
        Self {
            handle,
            state,
            reason,
            work_units_done,
            work_units_planned,
            changes,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_resident_handle(&self.handle)?;
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || self.work_units_planned == 0
            || self.work_units_planned > MAX_WORK_UNITS
            || self.work_units_done > self.work_units_planned
            || (self.state == ScriptWorkState::Paused) != self.reason.is_some()
            || self.changes.len() > MAX_ORDER_TARGETS
        {
            return Err(ScriptDtoError::InconsistentResult {
                field: "work assignment",
            });
        }
        for change in &self.changes {
            change.validate()?;
        }
        Ok(())
    }
}

/// Closed observable state of one squad member after an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptOrderMemberState {
    Applied,
    BlockedRoute,
    Unloaded,
    Dead,
    Migrating,
    Forbidden,
    StaleRevision,
}

impl ScriptOrderMemberState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::BlockedRoute => "blocked_route",
            Self::Unloaded => "unloaded",
            Self::Dead => "dead",
            Self::Migrating => "migrating",
            Self::Forbidden => "forbidden",
            Self::StaleRevision => "stale_revision",
        }
    }
}

/// One server-issued target reference surfaced to the plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOrderTarget {
    pub target_ref: String,
    pub category: ScriptHostileCategory,
    pub position: ScriptBlockPosition,
}

impl ScriptOrderTarget {
    #[must_use]
    pub fn new(
        target_ref: String,
        category: ScriptHostileCategory,
        position: ScriptBlockPosition,
    ) -> Self {
        Self {
            target_ref,
            category,
            position,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        validate_bounded_nonempty("target ref", &self.target_ref, MAX_TARGET_REF_BYTES)?;
        self.position.validate()
    }
}

/// One member's observable outcome after a squad order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptOrderMemberOutcome {
    pub handle: String,
    pub state: ScriptOrderMemberState,
    pub formation_slot: Option<u16>,
    pub targets: Vec<ScriptOrderTarget>,
}

impl ScriptOrderMemberOutcome {
    #[must_use]
    pub fn new(
        handle: String,
        state: ScriptOrderMemberState,
        formation_slot: Option<u16>,
        mut targets: Vec<ScriptOrderTarget>,
    ) -> Self {
        targets.sort_unstable_by(|left, right| left.target_ref.cmp(&right.target_ref));
        Self {
            handle,
            state,
            formation_slot,
            targets,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_resident_handle(&self.handle)?;
        if (self.state == ScriptOrderMemberState::Applied) != self.formation_slot.is_some() {
            return Err(ScriptDtoError::InconsistentResult {
                field: "order member outcome",
            });
        }
        if self.targets.len() > MAX_ORDER_TARGETS {
            return Err(ScriptDtoError::TooManyEntries {
                field: "order member targets",
                max: MAX_ORDER_TARGETS,
            });
        }
        for target in &self.targets {
            target.validate()?;
        }
        Ok(())
    }
}

/// Demobilisation state of one resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScriptDemobilizeState {
    Demobilizing,
    Civilian,
}

impl ScriptDemobilizeState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Demobilizing => "demobilizing",
            Self::Civilian => "civilian",
        }
    }
}

/// One demobilisation result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptDemobilizeResult {
    pub handle: String,
    pub state: ScriptDemobilizeState,
    pub reason: Option<ScriptWorkPauseReason>,
    pub returned: Vec<ScriptItemChange>,
    pub revision: u64,
}

impl ScriptDemobilizeResult {
    #[must_use]
    pub fn new(
        handle: String,
        state: ScriptDemobilizeState,
        reason: Option<ScriptWorkPauseReason>,
        mut returned: Vec<ScriptItemChange>,
        revision: u64,
    ) -> Self {
        returned.sort_unstable_by(|left, right| left.item_id.cmp(&right.item_id));
        Self {
            handle,
            state,
            reason,
            returned,
            revision,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_resident_handle(&self.handle)?;
        if self.revision > MAX_SCRIPT_WORLD_TIME
            || (self.state == ScriptDemobilizeState::Demobilizing) != self.reason.is_some()
        {
            return Err(ScriptDtoError::InconsistentResult {
                field: "demobilize result",
            });
        }
        for change in &self.returned {
            change.validate()?;
        }
        Ok(())
    }
}

/// Closed typed payload for one committed resident work/order call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum ScriptResidentOrderResult {
    Work {
        assignment: Box<ScriptWorkAssignment>,
    },
    WorkCancelled {
        handle: String,
        revision: u64,
    },
    Order {
        order_revision: u64,
        members: Vec<ScriptOrderMemberOutcome>,
        /// Committed combat outcomes correlated to this order revision, so the
        /// plugin awards experience exactly once per event id.
        combat: Vec<ScriptCombatEvent>,
    },
    OrderCancelled {
        order_revision: u64,
        members: Vec<ScriptOrderMemberOutcome>,
    },
    Demobilized {
        resident: Box<ScriptDemobilizeResult>,
    },
}

impl ScriptResidentOrderResult {
    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        match self {
            Self::Work { assignment } => assignment.validate(),
            Self::WorkCancelled { handle, revision } => {
                crate::validate_resident_handle(handle)?;
                if *revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(ScriptDtoError::InvalidBounds);
                }
                Ok(())
            }
            Self::Order {
                members, combat, ..
            } => {
                validate_order_member_outcomes(members)?;
                if combat.len() > MAX_COMBAT_EVENTS {
                    return Err(ScriptDtoError::TooManyEntries {
                        field: "combat events",
                        max: MAX_COMBAT_EVENTS,
                    });
                }
                for event in combat {
                    event.validate()?;
                }
                Ok(())
            }
            Self::OrderCancelled { members, .. } => validate_order_member_outcomes(members),
            Self::Demobilized { resident } => resident.validate(),
        }
    }
}

/// One committed combat outcome. Damage is in thousandths of a health point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptCombatEvent {
    pub event_id: u64,
    pub revision: u64,
    pub attacker_handle: String,
    pub victim_target_ref: String,
    pub order_revision: u64,
    pub damage_milli: u32,
    pub killed: bool,
}

impl ScriptCombatEvent {
    #[must_use]
    pub fn new(
        event_id: u64,
        revision: u64,
        attacker_handle: String,
        victim_target_ref: String,
        order_revision: u64,
        damage_milli: u32,
        killed: bool,
    ) -> Self {
        Self {
            event_id,
            revision,
            attacker_handle,
            victim_target_ref,
            order_revision,
            damage_milli,
            killed,
        }
    }

    pub fn validate(&self) -> Result<(), ScriptDtoError> {
        crate::validate_resident_handle(&self.attacker_handle)?;
        validate_bounded_nonempty("target ref", &self.victim_target_ref, MAX_TARGET_REF_BYTES)?;
        if self.event_id == 0
            || self.event_id > MAX_SCRIPT_WORLD_TIME
            || self.revision > MAX_SCRIPT_WORLD_TIME
            || self.order_revision > MAX_SCRIPT_WORLD_TIME
            || self.damage_milli == 0
            || self.damage_milli > u32::try_from(MAX_WORK_UNITS * 1000).unwrap_or(u32::MAX)
        {
            return Err(ScriptDtoError::InvalidBounds);
        }
        Ok(())
    }
}

/// Validate a bounded, sorted, strictly increasing member list.
fn validate_order_member_outcomes(
    members: &[ScriptOrderMemberOutcome],
) -> Result<(), ScriptDtoError> {
    if members.len() > MAX_RESIDENT_ORDER_HANDLES {
        return Err(ScriptDtoError::TooManyEntries {
            field: "order members",
            max: MAX_RESIDENT_ORDER_HANDLES,
        });
    }
    let mut previous = None;
    for member in members {
        member.validate()?;
        if previous.is_some_and(|handle: &str| handle >= member.handle.as_str()) {
            return Err(ScriptDtoError::InconsistentResult {
                field: "order member order",
            });
        }
        previous = Some(member.handle.as_str());
    }
    Ok(())
}

/// Validate one bounded work unit budget for an assignment.
pub fn validate_work_units(work_units: u64) -> Result<(), ScriptDtoError> {
    if work_units == 0 || work_units > MAX_WORK_UNITS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

/// Validate one bounded work-area axis extent.
pub fn validate_work_area_axis(extent: u32) -> Result<(), ScriptDtoError> {
    if extent == 0 || extent > MAX_WORK_AREA_AXIS {
        return Err(ScriptDtoError::InvalidBounds);
    }
    Ok(())
}

/// Validate a bounded recipe identifier.
pub fn validate_recipe(value: &str) -> Result<(), ScriptDtoError> {
    validate_bounded_value("recipe", value, MAX_RECIPE_ID_BYTES)
}
