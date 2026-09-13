//! Deterministic settlement site selection and layout.
//!
//! Selection and layout are pure functions of `(seed, profile revision,
//! coordinates)`: the same seed and revision produce identical site ids,
//! origins, variants, layouts and inhabitant slots regardless of chunk
//! generation order, and re-running discovery or crash-adopting produces no
//! duplicates or seams.
//!
//! Candidates live on a [`SITE_CELL_BLOCKS`]-block grid. Presence, variant,
//! placement jitter and layout picks all come from one documented mixer: FNV-1a
//! 64 over `seed ^ revision ^ x ^ z ^ salt`, finalised with the splitmix64
//! avalanche. Site ids additionally carry a `sha256` digest so the runtime can
//! reverse an id back to its grid cell without trusting the caller.

use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::settlement_catalog::{
    Blueprint, BlueprintCatalog, BlueprintInstance, MAX_PLACEMENTS_PER_SETTLEMENT, PoiKind,
    QuarterTurn,
};

/// Candidate grid spacing in blocks.
pub const SITE_CELL_BLOCKS: i32 = 512;
/// Upper bound on the cells one discovery page scans.
pub const MAX_SITES_PER_PAGE: usize = 64;
/// Longest straight-line settlement road, in blocks.
pub const MAX_ROAD_RADIUS_BLOCKS: i32 = 1024;
/// Neighbouring candidates are at most this many cells apart.
pub const ROAD_NEIGHBOUR_CELLS: i32 = 2;
/// Waypoints in one canonical road edge.
pub const MAX_ROAD_WAYPOINTS: usize = 64;

/// Waypoint spacing derived from the road budget: at most
/// `MAX_ROAD_RADIUS_BLOCKS / MAX_ROAD_WAYPOINTS` blocks between waypoints.
const WAYPOINT_SPACING: i64 = (MAX_ROAD_RADIUS_BLOCKS / MAX_ROAD_WAYPOINTS as i32) as i64;

/// POIs one settlement layout may expose.
const MAX_SITE_POIS: usize = 128;

/// Roles a deployed catalog must supply for every variant to lay out.
const REQUIRED_ROLES: [PoiKind; 4] = [
    PoiKind::Home,
    PoiKind::Meeting,
    PoiKind::Work,
    PoiKind::Guard,
];

const DOMAIN: &[u8] = b"solaris.settlement.site.v1";
const DEFAULT_SALT: u64 = 0x5d1e_5e77_1e5e_77a1;

const SALT_PRESENCE: u64 = 0x01;
const SALT_VARIANT: u64 = 0x02;
const SALT_ORIGIN_X: u64 = 0x03;
const SALT_ORIGIN_Z: u64 = 0x04;
const SALT_HOUSES: u64 = 0x05;
const SALT_SLOTS: u64 = 0x06;
const SALT_PICK: u64 = 0x07;
const SALT_TURN: u64 = 0x08;

/// Founding size class of a settlement candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteVariant {
    Hamlet,
    Village,
    Town,
}

impl SiteVariant {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Hamlet => "hamlet",
            Self::Village => "village",
            Self::Town => "town",
        }
    }

    /// Type weight, in percent: 65 / 30 / 5.
    #[must_use]
    pub fn weight(self) -> u32 {
        match self {
            Self::Hamlet => 65,
            Self::Village => 30,
            Self::Town => 5,
        }
    }

    /// Inclusive `(min, max)` house count.
    #[must_use]
    pub fn houses(self) -> (u32, u32) {
        match self {
            Self::Hamlet => (4, 5),
            Self::Village => (9, 12),
            Self::Town => (20, 28),
        }
    }

    /// Inclusive `(min, max)` residents at founding.
    #[must_use]
    pub fn residents(self) -> (u32, u32) {
        match self {
            Self::Hamlet => (8, 16),
            Self::Village => (24, 40),
            Self::Town => (50, 80),
        }
    }

    /// Surveyed candidate footprint in blocks.
    #[must_use]
    pub fn footprint(self) -> [i32; 3] {
        match self {
            Self::Hamlet => [128, 32, 128],
            Self::Village => [192, 32, 192],
            Self::Town => [256, 40, 256],
        }
    }
}

/// One deterministically occupied grid cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteCandidate {
    pub cell: [i32; 2],
    /// `site_<x>_<z>_<hash8>`; reversible through
    /// [`SettlementSelector::cell_from_site_id`].
    pub site_id: String,
    pub variant: SiteVariant,
    /// World footprint origin; the y coordinate is resolved by the caller.
    pub origin: [i32; 3],
    pub size: [i32; 3],
}

/// One building placed by a site layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitePlacement {
    pub blueprint_id: String,
    pub origin: [i32; 3],
    pub rotation: u16,
}

/// One point of interest of a laid-out settlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitePoi {
    /// Deterministic and stable for the site.
    pub poi_id: String,
    pub kind: PoiKind,
    pub at: [i32; 3],
    pub capacity: u16,
    /// Blueprint id of the building carrying the POI.
    pub building: String,
}

/// One canonical inter-settlement road edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoadEdge {
    /// One canonical id per unordered site pair.
    pub edge_id: String,
    pub from_site: String,
    pub to_site: String,
    /// Bounded straight line between the pair's connection points.
    pub waypoints: Vec<[i32; 3]>,
}

/// A laid-out settlement: buildings, POIs and founding population.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteLayout {
    pub candidate: SiteCandidate,
    pub placements: Vec<SitePlacement>,
    pub pois: Vec<SitePoi>,
    pub inhabitant_slots: u32,
}

/// Everything that can make a variant impossible to lay out.
#[derive(Debug, thiserror::Error)]
pub enum SiteError {
    #[error("catalog has no blueprint providing the required {role} role")]
    MissingRole { role: &'static str },
    #[error("settlement layout places {count} buildings, limit is {max}")]
    TooManyPlacements { count: usize, max: usize },
    #[error("settlement layout exposes {count} POIs, limit is {max}")]
    TooManyPois { count: usize, max: usize },
    #[error("buildings do not fit inside site footprint {size:?}")]
    SiteOutOfBounds { size: [i32; 3] },
    #[error("variant needs {requested} inhabitants but home POIs hold {capacity}")]
    ExcessInhabitants { requested: u32, capacity: u32 },
    #[error("terrain height is unavailable at ({x}, {z})")]
    Ungrounded { x: i32, z: i32 },
}

/// Deterministic settlement selector for one world seed and profile revision.
#[derive(Debug, Clone)]
pub struct SettlementSelector {
    seed: u64,
    revision: u64,
    salt: u64,
}

impl SettlementSelector {
    #[must_use]
    pub fn new(seed: i64, profile_revision: u64) -> Self {
        Self {
            seed: seed as u64,
            revision: profile_revision,
            salt: DEFAULT_SALT,
        }
    }

    #[must_use]
    pub fn seed(&self) -> i64 {
        self.seed as i64
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Pure function of `(seed, revision, cell)`. Most cells carry no
    /// settlement, and the answer never depends on generation order.
    #[must_use]
    pub fn candidate(&self, cell: [i32; 2]) -> Option<SiteCandidate> {
        let presence = self.mix(cell, SALT_PRESENCE);
        if !presence.is_multiple_of(8) {
            return None;
        }
        let variant = match self.mix(cell, SALT_VARIANT) % 100 {
            0..=64 => SiteVariant::Hamlet,
            65..=94 => SiteVariant::Village,
            _ => SiteVariant::Town,
        };
        let size = variant.footprint();
        let jitter_x = (self.mix(cell, SALT_ORIGIN_X) % (SITE_CELL_BLOCKS - size[0]) as u64) as i32;
        let jitter_z = (self.mix(cell, SALT_ORIGIN_Z) % (SITE_CELL_BLOCKS - size[2]) as u64) as i32;
        let origin = [
            cell[0]
                .saturating_mul(SITE_CELL_BLOCKS)
                .saturating_add(jitter_x),
            0,
            cell[1]
                .saturating_mul(SITE_CELL_BLOCKS)
                .saturating_add(jitter_z),
        ];
        Some(SiteCandidate {
            cell,
            site_id: self.site_id(cell),
            variant,
            origin,
            size,
        })
    }

    /// The reversible id of the settlement at `cell`, if it carries one.
    #[must_use]
    pub fn site_id_for_cell(&self, cell: [i32; 2]) -> Option<String> {
        self.candidate(cell).map(|candidate| candidate.site_id)
    }

    /// Reverse a site id back to its grid cell.
    ///
    /// Parses the embedded cell, recomputes the candidate, and only returns a
    /// cell when the recomputed id matches exactly, so forged or foreign ids
    /// are rejected.
    #[must_use]
    pub fn cell_from_site_id(&self, site_id: &str) -> Option<[i32; 2]> {
        let rest = site_id.strip_prefix("site_")?;
        let mut parts = rest.split('_');
        let x: i32 = parts.next()?.parse().ok()?;
        let z: i32 = parts.next()?.parse().ok()?;
        parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let cell = [x, z];
        let candidate = self.candidate(cell)?;
        (candidate.site_id == site_id).then_some(cell)
    }

    /// Deterministic ordered scan over `limit` cells from `start_cell`.
    ///
    /// Cells are walked row-major with [`MAX_SITES_PER_PAGE`] columns per row,
    /// so the caller's cursor is the next cell in scan order. Each cell yields
    /// at most one candidate, so the page never exceeds `limit` entries.
    #[must_use]
    pub fn discover(&self, start_cell: [i32; 2], limit: usize) -> Vec<SiteCandidate> {
        let mut candidates = Vec::new();
        for index in 0..limit {
            let cell = scan_cell(start_cell, index);
            if let Some(candidate) = self.candidate(cell) {
                candidates.push(candidate);
            }
        }
        candidates
    }

    /// Deterministic layout of one candidate from the catalog, grounded on the
    /// world's own terrain.
    ///
    /// `ground` answers the topmost solid terrain row of an absolute column —
    /// the startup generator the world itself generates from. Every building is
    /// anchored so its authored anchor meets the terrain row at its own anchor
    /// column, which leaves the base row the anchor's own local height below
    /// that row; POIs, residents and structures therefore keep the authored
    /// vertical relationships to the ground they stand on. A variant that cannot
    /// be laid out (missing required blueprint role, too many placements) and a
    /// column the resolver cannot answer are both explicit `Err`, never a silent
    /// downgrade.
    pub fn layout(
        &self,
        candidate: &SiteCandidate,
        catalog: &BlueprintCatalog,
        ground: &dyn Fn(i32, i32) -> Option<i32>,
    ) -> Result<SiteLayout, SiteError> {
        let mut roles: [Vec<&Arc<Blueprint>>; REQUIRED_ROLES.len()] = Default::default();
        for (slot, kind) in REQUIRED_ROLES.into_iter().enumerate() {
            let blueprints = role_blueprints(catalog, kind);
            if blueprints.is_empty() {
                return Err(SiteError::MissingRole {
                    role: kind.as_str(),
                });
            }
            roles[slot] = blueprints;
        }
        let [homes, meetings, work, guards] = roles;

        let houses = self.pick_range(candidate.cell, SALT_HOUSES, candidate.variant.houses());
        let mut chosen: Vec<(&Arc<Blueprint>, QuarterTurn)> = Vec::new();
        for index in 0..houses {
            let blueprint = homes[self.pick(
                candidate.cell,
                SALT_PICK.wrapping_add(index as u64),
                homes.len(),
            )];
            chosen.push((
                blueprint,
                self.placement_turn(candidate.cell, index as usize),
            ));
        }
        let role_offset = u64::from(houses);
        chosen.push((
            meetings[self.pick(
                candidate.cell,
                SALT_PICK.wrapping_add(role_offset),
                meetings.len(),
            )],
            self.placement_turn(candidate.cell, role_offset as usize),
        ));
        chosen.push((
            work[self.pick(
                candidate.cell,
                SALT_PICK.wrapping_add(role_offset + 1),
                work.len(),
            )],
            self.placement_turn(candidate.cell, role_offset as usize + 1),
        ));
        chosen.push((
            guards[self.pick(
                candidate.cell,
                SALT_PICK.wrapping_add(role_offset + 2),
                guards.len(),
            )],
            self.placement_turn(candidate.cell, role_offset as usize + 2),
        ));
        if chosen.len() > MAX_PLACEMENTS_PER_SETTLEMENT {
            return Err(SiteError::TooManyPlacements {
                count: chosen.len(),
                max: MAX_PLACEMENTS_PER_SETTLEMENT,
            });
        }

        let [origin_x, _, origin_z] = candidate.origin;
        let [size_x, _, size_z] = candidate.size;
        let mut placements = Vec::with_capacity(chosen.len());
        let mut pois = Vec::new();
        let mut capacity: u32 = 0;
        let mut x = origin_x;
        let mut z = origin_z;
        let mut row_depth = 0;
        for (index, (blueprint, turn)) in chosen.iter().enumerate() {
            let extent = rotated_extent(blueprint.size(), *turn);
            if x + extent[0] > origin_x + size_x {
                x = origin_x;
                z += row_depth + 2;
                row_depth = 0;
            }
            if z + extent[2] > origin_z + size_z {
                return Err(SiteError::SiteOutOfBounds {
                    size: candidate.size,
                });
            }
            // A blueprint's authored anchor locates the placement's column; the
            // base row is the terrain row there, so the blueprint's local y=0
            // layer lands on the ground the settlement stands on (a plaza paves
            // the top soil, a hollow-base house keeps its void there and floors
            // the row above), exactly like the recorded accepted placement of
            // `house_small` (terrain top 91, floor planks 92). The anchor's own
            // local height is deliberately unused: an authored deck anchor would
            // otherwise pull the building below the terrain row, and the
            // prepare-time fit gate rejects any footprint whose terrain rises
            // above the base row, so every such placement would be refused. That
            // gate re-checks every footprint column, so terrain rising into the
            // building still refuses the placement.
            let anchor =
                BlueprintInstance::new(Arc::clone(blueprint), *turn, [x, 0, z]).placed_anchor();
            let base_y = ground(anchor[0], anchor[2]).ok_or(SiteError::Ungrounded {
                x: anchor[0],
                z: anchor[2],
            })?;
            let building_origin = [x, base_y, z];
            placements.push(SitePlacement {
                blueprint_id: blueprint.id().to_owned(),
                origin: building_origin,
                rotation: turn.degrees(),
            });
            let instance = BlueprintInstance::new(Arc::clone(blueprint), *turn, building_origin);
            for poi in instance.placed_pois() {
                pois.push(SitePoi {
                    poi_id: format!("{}.{}.{}", candidate.site_id, index, poi.id),
                    kind: poi.kind,
                    at: poi.at,
                    capacity: poi.capacity,
                    building: blueprint.id().to_owned(),
                });
                if poi.kind == PoiKind::Home {
                    capacity += u32::from(poi.capacity);
                }
            }
            x += extent[0] + 2;
            row_depth = row_depth.max(extent[2]);
        }
        if pois.len() > MAX_SITE_POIS {
            return Err(SiteError::TooManyPois {
                count: pois.len(),
                max: MAX_SITE_POIS,
            });
        }

        let inhabitant_slots =
            self.pick_range(candidate.cell, SALT_SLOTS, candidate.variant.residents());
        if capacity < inhabitant_slots {
            return Err(SiteError::ExcessInhabitants {
                requested: inhabitant_slots,
                capacity,
            });
        }

        // The reported site origin is the first free row above the terrain at
        // the site's own origin column; every building is grounded separately at
        // its own anchor column.
        let mut candidate = candidate.clone();
        candidate.origin[1] = ground(origin_x, origin_z)
            .ok_or(SiteError::Ungrounded {
                x: origin_x,
                z: origin_z,
            })?
            .saturating_add(1);
        Ok(SiteLayout {
            candidate,
            placements,
            pois,
            inhabitant_slots,
        })
    }

    /// Canonical road edge between two neighbours within
    /// [`ROAD_NEIGHBOUR_CELLS`]. `None` when the pair is too far apart or the
    /// road would exceed [`MAX_ROAD_RADIUS_BLOCKS`].
    ///
    /// The edge is canonically oriented by site id, so `road(a, b)` and
    /// `road(b, a)` are the same edge, including its [`RoadEdge::edge_id`].
    #[must_use]
    pub fn road(&self, a: &SiteCandidate, b: &SiteCandidate) -> Option<RoadEdge> {
        if a.site_id == b.site_id {
            return None;
        }
        let dx = (b.cell[0] - a.cell[0]).abs();
        let dz = (b.cell[1] - a.cell[1]).abs();
        if dx.max(dz) > ROAD_NEIGHBOUR_CELLS {
            return None;
        }
        let (from, to) = if a.site_id < b.site_id {
            (a, b)
        } else {
            (b, a)
        };
        let start = connection_point(from, to);
        let end = connection_point(to, from);
        let delta = [
            i64::from(end[0] - start[0]),
            i64::from(end[1] - start[1]),
            i64::from(end[2] - start[2]),
        ];
        let length =
            ((delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]) as f64).sqrt();
        if length > f64::from(MAX_ROAD_RADIUS_BLOCKS) {
            return None;
        }
        let segments = (length as i64 / WAYPOINT_SPACING) as usize + 1;
        let count = (segments + 1).clamp(2, MAX_ROAD_WAYPOINTS);
        let steps = (count - 1) as i64;
        let waypoints = (0..count)
            .map(|index| {
                let index = index as i64;
                [
                    start[0] + (delta[0] * index / steps) as i32,
                    start[1] + (delta[1] * index / steps) as i32,
                    start[2] + (delta[2] * index / steps) as i32,
                ]
            })
            .collect();
        Some(RoadEdge {
            edge_id: format!("road:{}|{}", from.site_id, to.site_id),
            from_site: from.site_id.clone(),
            to_site: to.site_id.clone(),
            waypoints,
        })
    }

    fn placement_turn(&self, cell: [i32; 2], index: usize) -> QuarterTurn {
        match self.mix(cell, SALT_TURN.wrapping_add(index as u64)) % 4 {
            0 => QuarterTurn::None,
            1 => QuarterTurn::Cw90,
            2 => QuarterTurn::Cw180,
            _ => QuarterTurn::Cw270,
        }
    }

    fn mix(&self, cell: [i32; 2], salt: u64) -> u64 {
        mix(self.seed, self.revision, cell[0], cell[1], self.salt ^ salt)
    }

    fn pick(&self, cell: [i32; 2], salt: u64, modulo: usize) -> usize {
        (self.mix(cell, salt) % modulo as u64) as usize
    }

    fn pick_range(&self, cell: [i32; 2], salt: u64, range: (u32, u32)) -> u32 {
        let (min, max) = range;
        let span = max - min + 1;
        min + (self.mix(cell, salt) % u64::from(span)) as u32
    }

    fn site_id(&self, cell: [i32; 2]) -> String {
        format!("site_{}_{}_{}", cell[0], cell[1], self.site_digest(cell))
    }

    /// First 8 lowercase hex digits of
    /// `sha256("solaris.settlement.site.v1" | seed | revision | x | z)`.
    fn site_digest(&self, cell: [i32; 2]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN);
        hasher.update(self.seed.to_le_bytes());
        hasher.update(self.revision.to_le_bytes());
        hasher.update(cell[0].to_le_bytes());
        hasher.update(cell[1].to_le_bytes());
        hex8(&hasher.finalize())
    }
}

/// The first role a catalog cannot supply, if any.
///
/// A deployed package whose catalog provides no blueprint for a required role
/// can never lay out a settlement, so startup rejects it instead of answering
/// every request with an unavailable runtime.
#[must_use]
pub fn missing_required_role(catalog: &BlueprintCatalog) -> Option<PoiKind> {
    REQUIRED_ROLES
        .into_iter()
        .find(|kind| role_blueprints(catalog, *kind).is_empty())
}

fn role_blueprints(catalog: &BlueprintCatalog, kind: PoiKind) -> Vec<&Arc<Blueprint>> {
    let mut roles: Vec<&Arc<Blueprint>> = catalog
        .blueprints()
        .filter(|blueprint| blueprint.pois().iter().any(|poi| poi.kind == kind))
        .collect();
    roles.sort_by(|left, right| {
        left.content_hash()
            .cmp(right.content_hash())
            .then_with(|| left.id().cmp(right.id()))
    });
    roles
}

fn rotated_extent(size: [i32; 3], turn: QuarterTurn) -> [i32; 3] {
    match turn {
        QuarterTurn::None | QuarterTurn::Cw180 => size,
        QuarterTurn::Cw90 | QuarterTurn::Cw270 => [size[2], size[1], size[0]],
    }
}

/// Midpoint of the footprint side of `from` that faces `toward`.
fn connection_point(from: &SiteCandidate, toward: &SiteCandidate) -> [i32; 3] {
    let [size_x, _, size_z] = from.size;
    let center_x = from.origin[0] + size_x / 2;
    let center_z = from.origin[2] + size_z / 2;
    let other_x = toward.origin[0] + toward.size[0] / 2;
    let other_z = toward.origin[2] + toward.size[2] / 2;
    let delta_x = other_x - center_x;
    let delta_z = other_z - center_z;
    if delta_x.abs() >= delta_z.abs() {
        let x = if delta_x >= 0 {
            from.origin[0] + size_x - 1
        } else {
            from.origin[0]
        };
        [x, from.origin[1], center_z]
    } else {
        let z = if delta_z >= 0 {
            from.origin[2] + size_z - 1
        } else {
            from.origin[2]
        };
        [center_x, from.origin[1], z]
    }
}

fn scan_cell(start: [i32; 2], index: usize) -> [i32; 2] {
    [
        start[0] + (index % MAX_SITES_PER_PAGE) as i32,
        start[1] + (index / MAX_SITES_PER_PAGE) as i32,
    ]
}

/// FNV-1a 64 over `seed`, `revision`, `x`, `z` and `salt`, then a splitmix64
/// finaliser. Overflow is intentional and wrapping.
fn mix(seed: u64, revision: u64, x: i32, z: i32, salt: u64) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for value in [
        seed,
        revision,
        u64::from(x as u32),
        u64::from(z as u32),
        salt,
    ] {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    splitmix64(hash)
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn hex8(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(8);
    for byte in bytes.iter().take(4) {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}
