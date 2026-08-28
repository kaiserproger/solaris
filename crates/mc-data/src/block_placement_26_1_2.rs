//! Protocol-neutral block placement state rules for Java Edition 26.1.2.
//!
//! This module owns deterministic placement math only. World snapshot reads,
//! support checks, edit preconditions, and commits remain in mc-net.

use mc_domain::Direction;

/// Whether the direction is on the west-east axis.
#[must_use]
pub fn horizontal_axis(direction: Direction) -> bool {
    matches!(direction, Direction::West | Direction::East)
}

#[must_use]
pub fn opposite(direction: Direction) -> Direction {
    match direction {
        Direction::Down => Direction::Up,
        Direction::Up => Direction::Down,
        Direction::North => Direction::South,
        Direction::South => Direction::North,
        Direction::West => Direction::East,
        Direction::East => Direction::West,
    }
}

/// Counter-clockwise rotation in the horizontal plane. Vertical directions
/// have no horizontal rotation.
#[must_use]
pub fn counter_clockwise(direction: Direction) -> Option<Direction> {
    match direction {
        Direction::North => Some(Direction::West),
        Direction::South => Some(Direction::East),
        Direction::West => Some(Direction::South),
        Direction::East => Some(Direction::North),
        Direction::Down | Direction::Up => None,
    }
}

/// Facing and half of a valid stair block state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StairProperties {
    /// Which way the stair faces.
    pub facing: Direction,
    /// Whether the stair is the top half.
    pub top: bool,
}

/// Classification of one cell the stair shape resolution reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StairCell {
    NotStair,
    Stair(StairProperties),
    /// A state that looks like a stair but violates the canonical schema.
    Malformed,
}

/// The four horizontal neighbor cells of a stair, keyed by direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StairNeighborState {
    pub north: StairCell,
    pub south: StairCell,
    pub west: StairCell,
    pub east: StairCell,
}

impl StairNeighborState {
    #[must_use]
    pub fn cell(&self, direction: Direction) -> StairCell {
        match direction {
            Direction::North => self.north,
            Direction::South => self.south,
            Direction::West => self.west,
            Direction::East => self.east,
            Direction::Down | Direction::Up => StairCell::NotStair,
        }
    }
}

/// Vanilla stair corner shape. The string form is the `shape` property value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StairShape {
    Straight,
    InnerLeft,
    InnerRight,
    OuterLeft,
    OuterRight,
}

impl StairShape {
    #[must_use]
    pub const fn property_value(self) -> &'static str {
        match self {
            Self::Straight => "straight",
            Self::InnerLeft => "inner_left",
            Self::InnerRight => "inner_right",
            Self::OuterLeft => "outer_left",
            Self::OuterRight => "outer_right",
        }
    }
}

/// Resolve the `shape` property for a stair from the states of its four
/// horizontal neighbors. Returns `None` when any read cell is malformed or
/// when the current stair cannot be interpreted.
#[must_use]
pub fn resolve_stair_shape(
    current: StairProperties,
    neighbors: StairNeighborState,
) -> Option<StairShape> {
    let behind = neighbors.cell(current.facing);
    match behind {
        StairCell::Malformed => return None,
        StairCell::Stair(behind)
            if current.top == behind.top
                && horizontal_axis(current.facing) != horizontal_axis(behind.facing) =>
        {
            let guard = neighbors.cell(opposite(behind.facing));
            if can_take_stair_shape(guard, current)? {
                return Some(if behind.facing == counter_clockwise(current.facing)? {
                    StairShape::OuterLeft
                } else {
                    StairShape::OuterRight
                });
            }
        }
        StairCell::NotStair | StairCell::Stair(_) => {}
    }

    let front = neighbors.cell(opposite(current.facing));
    match front {
        StairCell::Malformed => return None,
        StairCell::Stair(front)
            if current.top == front.top
                && horizontal_axis(current.facing) != horizontal_axis(front.facing) =>
        {
            let guard = neighbors.cell(front.facing);
            if can_take_stair_shape(guard, current)? {
                return Some(if front.facing == counter_clockwise(current.facing)? {
                    StairShape::InnerLeft
                } else {
                    StairShape::InnerRight
                });
            }
        }
        StairCell::NotStair | StairCell::Stair(_) => {}
    }

    Some(StairShape::Straight)
}

fn can_take_stair_shape(cell: StairCell, current: StairProperties) -> Option<bool> {
    match cell {
        StairCell::NotStair => Some(true),
        StairCell::Malformed => None,
        StairCell::Stair(neighbor) => {
            Some(neighbor.facing != current.facing || neighbor.top != current.top)
        }
    }
}

/// Minimal protocol-neutral block state used by placement math.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementBlockState {
    /// Full registry id, e.g. `minecraft:oak_slab`.
    pub block_id: String,
    /// Property values in schema order.
    pub properties: Vec<(String, String)>,
}

impl PlacementBlockState {
    /// Block path with the namespace stripped.
    #[must_use]
    pub fn path(&self) -> &str {
        self.block_id.rsplit(':').next().unwrap_or(&self.block_id)
    }

    #[must_use]
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }

    #[must_use]
    pub fn with_property(&self, name: &str, value: &str) -> Self {
        let mut properties = self.properties.clone();
        if let Some((_, current)) = properties.iter_mut().find(|(key, _)| key == name) {
            *current = value.to_string();
        }
        Self {
            block_id: self.block_id.clone(),
            properties,
        }
    }
}

/// Whether `existing` (the block already in the world) can merge with `placed`
/// (the held slab) into a double slab.
#[must_use]
pub fn can_merge_slab(existing: &PlacementBlockState, placed: &PlacementBlockState) -> bool {
    placed.path().ends_with("_slab")
        && existing.block_id == placed.block_id
        && matches!(existing.property("type"), Some("bottom" | "top"))
}

/// Merge a slab placed against an existing same-type slab into a dry double
/// slab. Returns `None` when the two states cannot merge.
#[must_use]
pub fn merge_slab_state(
    existing: &PlacementBlockState,
    placed: &PlacementBlockState,
) -> Option<PlacementBlockState> {
    if !can_merge_slab(existing, placed) {
        return None;
    }
    let merged = existing
        .with_property("type", "double")
        .with_property("waterlogged", "false");
    Some(merged)
}

/// Set the `waterlogged` property for slab and stair placements based on
/// whether the replaced cell holds a water source. Other blocks are returned
/// unchanged.
#[must_use]
pub fn apply_waterlogged_state(
    placed: &PlacementBlockState,
    existing: &PlacementBlockState,
) -> PlacementBlockState {
    if !(placed.path().ends_with("_slab") || placed.path().ends_with("_stairs")) {
        return placed.clone();
    }
    let waterlogged = existing.block_id == "minecraft:water";
    placed.with_property("waterlogged", if waterlogged { "true" } else { "false" })
}

/// State to build for a torch placed against the clicked face `direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TorchPlacement {
    /// `minecraft:torch` for the floor variant, `minecraft:wall_torch` for walls.
    pub block_id: &'static str,
    /// `facing` property value for wall torches, `None` for the floor torch.
    pub facing: Option<&'static str>,
}

/// Map a clicked face to the torch block state to place. `None` means the
/// face cannot hold a torch. The support block is always `pos + opposite(direction)`.
#[must_use]
pub fn torch_state_for_direction(direction: Direction) -> Option<TorchPlacement> {
    match direction {
        Direction::Up => Some(TorchPlacement {
            block_id: "minecraft:torch",
            facing: None,
        }),
        Direction::North => Some(TorchPlacement {
            block_id: "minecraft:wall_torch",
            facing: Some("north"),
        }),
        Direction::South => Some(TorchPlacement {
            block_id: "minecraft:wall_torch",
            facing: Some("south"),
        }),
        Direction::West => Some(TorchPlacement {
            block_id: "minecraft:wall_torch",
            facing: Some("west"),
        }),
        Direction::East => Some(TorchPlacement {
            block_id: "minecraft:wall_torch",
            facing: Some("east"),
        }),
        Direction::Down => None,
    }
}

/// The oriented property a sign placement should take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignState {
    /// A wall sign facing the given direction.
    Wall { facing: &'static str },
    /// A standing sign rotated by yaw into one of 16 directions.
    Standing { rotation: u8 },
}

/// Map the clicked face and player yaw to the sign state property. `None` when
/// the face cannot hold a sign.
#[must_use]
pub fn sign_state_for_direction(direction: Direction, yaw: f32) -> Option<SignState> {
    match direction {
        Direction::Down => None,
        Direction::Up => Some(SignState::Standing {
            rotation: sign_rotation_from_yaw(yaw),
        }),
        Direction::North => Some(SignState::Wall { facing: "north" }),
        Direction::South => Some(SignState::Wall { facing: "south" }),
        Direction::West => Some(SignState::Wall { facing: "west" }),
        Direction::East => Some(SignState::Wall { facing: "east" }),
    }
}

fn sign_rotation_from_yaw(yaw: f32) -> u8 {
    ((yaw.rem_euclid(360.0) / 22.5).round() as i32).rem_euclid(16) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stair(facing: Direction, top: bool) -> StairCell {
        StairCell::Stair(StairProperties { facing, top })
    }

    fn neighbors(cells: &[(Direction, StairCell)]) -> StairNeighborState {
        let mut state = StairNeighborState {
            north: StairCell::NotStair,
            south: StairCell::NotStair,
            west: StairCell::NotStair,
            east: StairCell::NotStair,
        };
        for (direction, cell) in cells {
            match direction {
                Direction::North => state.north = *cell,
                Direction::South => state.south = *cell,
                Direction::West => state.west = *cell,
                Direction::East => state.east = *cell,
                Direction::Down | Direction::Up => {}
            }
        }
        state
    }

    fn slab_state(slab_type: &str, waterlogged: &str) -> PlacementBlockState {
        PlacementBlockState {
            block_id: "minecraft:oak_slab".to_string(),
            properties: vec![
                ("type".to_string(), slab_type.to_string()),
                ("waterlogged".to_string(), waterlogged.to_string()),
            ],
        }
    }

    #[test]
    fn horizontal_turns_preserve_axis_rules() {
        assert!(horizontal_axis(Direction::East));
        assert!(!horizontal_axis(Direction::North));
        assert!(!horizontal_axis(Direction::Up));
        assert_eq!(opposite(Direction::North), Direction::South);
        assert_eq!(counter_clockwise(Direction::North), Some(Direction::West));
        assert_eq!(counter_clockwise(Direction::South), Some(Direction::East));
        assert_eq!(counter_clockwise(Direction::Up), None);
        assert_eq!(counter_clockwise(Direction::Down), None);
    }

    #[test]
    fn straight_stair_has_no_shaped_neighbors() {
        let current = StairProperties {
            facing: Direction::North,
            top: false,
        };
        assert_eq!(
            resolve_stair_shape(current, neighbors(&[])),
            Some(StairShape::Straight)
        );
    }

    #[test]
    fn outer_corner_shape_matches_perpendicular_behind_stair() {
        let current = StairProperties {
            facing: Direction::North,
            top: false,
        };
        // Behind = north neighbor facing west (perpendicular). Guard = the
        // cell opposite the behind stair's facing (east) stays empty.
        let state = neighbors(&[(Direction::North, stair(Direction::West, false))]);
        assert_eq!(
            resolve_stair_shape(current, state),
            Some(StairShape::OuterLeft)
        );
    }

    #[test]
    fn inner_corner_shape_matches_perpendicular_front_stair() {
        let current = StairProperties {
            facing: Direction::North,
            top: false,
        };
        // Front = south neighbor facing east (perpendicular, clockwise of the
        // stair). Guard = the cell in front of the front stair (east) stays empty.
        let state = neighbors(&[(Direction::South, stair(Direction::East, false))]);
        assert_eq!(
            resolve_stair_shape(current, state),
            Some(StairShape::InnerRight)
        );
    }

    #[test]
    fn opposite_half_and_parallel_stairs_stay_straight() {
        let current = StairProperties {
            facing: Direction::North,
            top: false,
        };
        let different_half = neighbors(&[(Direction::North, stair(Direction::West, true))]);
        assert_eq!(
            resolve_stair_shape(current, different_half),
            Some(StairShape::Straight)
        );
        let parallel = neighbors(&[(Direction::North, stair(Direction::North, false))]);
        assert_eq!(
            resolve_stair_shape(current, parallel),
            Some(StairShape::Straight)
        );
    }

    #[test]
    fn guard_stair_blocking_corner_keeps_straight_shape() {
        let current = StairProperties {
            facing: Direction::North,
            top: false,
        };
        // Behind is a west-facing stair, but the east guard is itself a
        // north-facing stair with the same half -> cannot take the corner.
        let state = neighbors(&[
            (Direction::North, stair(Direction::West, false)),
            (Direction::East, stair(Direction::North, false)),
        ]);
        assert_eq!(
            resolve_stair_shape(current, state),
            Some(StairShape::Straight)
        );
    }

    #[test]
    fn malformed_neighbor_fails_closed() {
        let current = StairProperties {
            facing: Direction::North,
            top: false,
        };
        let state = neighbors(&[(Direction::South, StairCell::Malformed)]);
        assert_eq!(resolve_stair_shape(current, state), None);
    }

    #[test]
    fn top_and_bottom_slabs_merge_into_a_dry_double() {
        let existing = slab_state("bottom", "true");
        let placed = slab_state("bottom", "false");
        let merged = merge_slab_state(&existing, &placed).expect("same slab merges");
        assert_eq!(merged.block_id, "minecraft:oak_slab");
        assert_eq!(merged.property("type"), Some("double"));
        assert_eq!(merged.property("waterlogged"), Some("false"));
    }

    #[test]
    fn double_slab_does_not_merge_again() {
        let existing = slab_state("double", "false");
        let placed = slab_state("bottom", "false");
        assert_eq!(merge_slab_state(&existing, &placed), None);
    }

    #[test]
    fn different_block_ids_do_not_merge() {
        let existing = slab_state("bottom", "false");
        let placed = PlacementBlockState {
            block_id: "minecraft:stone".to_string(),
            properties: Vec::new(),
        };
        assert_eq!(merge_slab_state(&existing, &placed), None);
    }

    #[test]
    fn waterlogged_placement_marks_water_cells() {
        let placed = slab_state("bottom", "false");
        let existing = PlacementBlockState {
            block_id: "minecraft:water".to_string(),
            properties: vec![("level".to_string(), "0".to_string())],
        };
        let wet = apply_waterlogged_state(&placed, &existing);
        assert_eq!(wet.property("waterlogged"), Some("true"));
    }

    #[test]
    fn non_water_cell_stays_dry() {
        let placed = slab_state("bottom", "false");
        let existing = PlacementBlockState {
            block_id: "minecraft:stone".to_string(),
            properties: Vec::new(),
        };
        let dry = apply_waterlogged_state(&placed, &existing);
        assert_eq!(dry.property("waterlogged"), Some("false"));
    }

    #[test]
    fn non_waterlogged_blocks_pass_through_unchanged() {
        let placed = PlacementBlockState {
            block_id: "minecraft:stone".to_string(),
            properties: Vec::new(),
        };
        let existing = PlacementBlockState {
            block_id: "minecraft:water".to_string(),
            properties: Vec::new(),
        };
        assert_eq!(apply_waterlogged_state(&placed, &existing), placed);
    }

    #[test]
    fn wall_torch_facing_matches_clicked_face() {
        assert_eq!(
            torch_state_for_direction(Direction::North),
            Some(TorchPlacement {
                block_id: "minecraft:wall_torch",
                facing: Some("north"),
            })
        );
        assert_eq!(
            torch_state_for_direction(Direction::East),
            Some(TorchPlacement {
                block_id: "minecraft:wall_torch",
                facing: Some("east"),
            })
        );
    }

    #[test]
    fn floor_torch_and_vertical_faces() {
        assert_eq!(
            torch_state_for_direction(Direction::Up),
            Some(TorchPlacement {
                block_id: "minecraft:torch",
                facing: None,
            })
        );
        assert_eq!(torch_state_for_direction(Direction::Down), None);
    }

    #[test]
    fn sign_state_follows_face_and_yaw() {
        assert_eq!(
            sign_state_for_direction(Direction::West, 0.0),
            Some(SignState::Wall { facing: "west" })
        );
        assert_eq!(sign_state_for_direction(Direction::Down, 0.0), None);
        let standing = sign_state_for_direction(Direction::Up, 90.0);
        assert_eq!(standing, Some(SignState::Standing { rotation: 4 }));
    }
}
