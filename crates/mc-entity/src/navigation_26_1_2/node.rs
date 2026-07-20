use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodePos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl NodePos {
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn checked_offset(self, dx: i32, dy: i32, dz: i32) -> Option<Self> {
        Some(Self {
            x: self.x.checked_add(dx)?,
            y: self.y.checked_add(dy)?,
            z: self.z.checked_add(dz)?,
        })
    }

    #[must_use]
    pub fn distance(self, other: Self) -> f32 {
        self.distance_f64(other) as f32
    }

    #[must_use]
    pub fn manhattan_distance(self, other: Self) -> u64 {
        let dx = (i64::from(self.x) - i64::from(other.x)).unsigned_abs();
        let dy = (i64::from(self.y) - i64::from(other.y)).unsigned_abs();
        let dz = (i64::from(self.z) - i64::from(other.z)).unsigned_abs();
        dx + dy + dz
    }

    fn distance_f64(self, other: Self) -> f64 {
        let dx = f64::from(self.x) - f64::from(other.x);
        let dy = f64::from(self.y) - f64::from(other.y);
        let dz = f64::from(self.z) - f64::from(other.z);
        dx.hypot(dy).hypot(dz)
    }
}

/// Classification metadata carried through a path without interpretation by
/// the generic search kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PathType {
    Blocked,
    Open,
    Walkable,
    WalkableDoor,
    Trapdoor,
    PowderSnow,
    OnTopOfPowderSnow,
    Fence,
    Lava,
    Water,
    WaterBorder,
    Rail,
    UnpassableRail,
    FireInNeighbor,
    Fire,
    DamagingInNeighbor,
    Damaging,
    DoorOpen,
    DoorWoodClosed,
    DoorIronClosed,
    Breach,
    Leaves,
    StickyHoney,
    Cocoa,
    DamageCautious,
    OnTopOfTrapdoor,
    BigMobsCloseToDanger,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathNode {
    pub pos: NodePos,
    pub path_type: PathType,
    pub malus: f32,
}

impl PathNode {
    #[must_use]
    pub const fn new(pos: NodePos, path_type: PathType, malus: f32) -> Self {
        Self {
            pos,
            path_type,
            malus,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchGoal {
    center: NodePos,
    reach_radius: f32,
}

impl SearchGoal {
    pub fn new(center: NodePos, reach_radius: f32) -> Result<Self, SearchGoalError> {
        if !reach_radius.is_finite() {
            return Err(SearchGoalError::NonFiniteReachRadius);
        }
        if reach_radius < 0.0 {
            return Err(SearchGoalError::NegativeReachRadius);
        }
        Ok(Self {
            center,
            reach_radius,
        })
    }

    #[must_use]
    pub const fn center(self) -> NodePos {
        self.center
    }

    #[must_use]
    pub const fn reach_radius(self) -> f32 {
        self.reach_radius
    }

    #[must_use]
    pub fn contains(self, pos: NodePos) -> bool {
        pos.manhattan_distance(self.center) as f64 <= f64::from(self.reach_radius)
    }

    /// Euclidean lower bound to the Manhattan reach region.
    ///
    /// Every point in that region is at most `reach_radius` Euclidean units
    /// from its center, so subtracting the radius cannot overestimate the
    /// Euclidean distance to any accepted endpoint.
    #[must_use]
    pub fn euclidean_lower_bound(self, pos: NodePos) -> f32 {
        let exact = (pos.distance_f64(self.center) - f64::from(self.reach_radius)).max(0.0);
        let rounded = exact as f32;
        if f64::from(rounded) > exact {
            rounded.next_down()
        } else {
            rounded
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchGoalError {
    NonFiniteReachRadius,
    NegativeReachRadius,
}

impl fmt::Display for SearchGoalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteReachRadius => formatter.write_str("reach radius must be finite"),
            Self::NegativeReachRadius => formatter.write_str("reach radius must be nonnegative"),
        }
    }
}

impl std::error::Error for SearchGoalError {}
