//! Exact vanilla 26.1.2 collision boxes for movement-critical non-full blocks.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use serde::Deserialize;
use thiserror::Error;

const RAW_COLLISION_SHAPES: &str = include_str!("../data/block_collision_shapes_26_1_2.json");
const EXPECTED_VERSION: &str = "26.1.2";
const UNITS_PER_BLOCK: u8 = 16;
static VANILLA_COLLISION_SHAPES: OnceLock<CollisionShapeTable> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollisionBox {
    coordinates: [u8; 6],
}

impl CollisionBox {
    #[must_use]
    pub const fn new(min_x: u8, min_y: u8, min_z: u8, max_x: u8, max_y: u8, max_z: u8) -> Self {
        assert!(min_x < max_x && min_y < max_y && min_z < max_z);
        Self {
            coordinates: [min_x, min_y, min_z, max_x, max_y, max_z],
        }
    }

    #[must_use]
    pub const fn coordinates(self) -> [u8; 6] {
        self.coordinates
    }

    #[must_use]
    pub fn as_blocks(self) -> [f64; 6] {
        self.coordinates
            .map(|value| f64::from(value) / f64::from(UNITS_PER_BLOCK))
    }
}

#[derive(Debug, Clone)]
pub struct CollisionShapeTable {
    version: String,
    family_state_counts: BTreeMap<String, u32>,
    shapes: Vec<Box<[CollisionBox]>>,
    state_shapes: HashMap<u32, usize>,
    max_box_y: u8,
}

impl CollisionShapeTable {
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn get(&self, state_id: u32) -> Option<&[CollisionBox]> {
        let shape = *self.state_shapes.get(&state_id)?;
        self.shapes.get(shape).map(Box::as_ref)
    }

    #[must_use]
    pub fn covered_state_count(&self) -> usize {
        self.state_shapes.len()
    }

    #[must_use]
    pub fn family_state_count(&self, family: &str) -> Option<u32> {
        self.family_state_counts.get(family).copied()
    }

    #[must_use]
    pub const fn max_box_y(&self) -> u8 {
        self.max_box_y
    }

    fn from_json(json: &str) -> Result<Self, CollisionShapeError> {
        let raw: RawTable = serde_json::from_str(json)?;
        if raw.version != EXPECTED_VERSION {
            return Err(CollisionShapeError::Version(raw.version));
        }
        if raw.units_per_block != UNITS_PER_BLOCK {
            return Err(CollisionShapeError::Units(raw.units_per_block));
        }

        let mut shapes = Vec::with_capacity(raw.shapes.len());
        let mut max_box_y = 0;
        for (shape_index, raw_shape) in raw.shapes.into_iter().enumerate() {
            if raw_shape.is_empty() {
                return Err(CollisionShapeError::EmptyShape(shape_index));
            }
            let mut shape = Vec::with_capacity(raw_shape.len());
            for coordinates in raw_shape {
                if coordinates[0] >= coordinates[3]
                    || coordinates[1] >= coordinates[4]
                    || coordinates[2] >= coordinates[5]
                {
                    return Err(CollisionShapeError::InvalidBox {
                        shape: shape_index,
                        coordinates,
                    });
                }
                max_box_y = max_box_y.max(coordinates[4]);
                shape.push(CollisionBox { coordinates });
            }
            shapes.push(shape.into_boxed_slice());
        }

        let mut state_shapes = HashMap::with_capacity(raw.entries.len());
        let mut previous = None;
        for [state_id, shape_index] in raw.entries {
            if previous.is_some_and(|previous| state_id <= previous) {
                return Err(CollisionShapeError::StateOrder(state_id));
            }
            let shape_index = usize::try_from(shape_index)
                .map_err(|_| CollisionShapeError::ShapeIndex(shape_index))?;
            if shape_index >= shapes.len() {
                return Err(CollisionShapeError::ShapeIndex(shape_index as u32));
            }
            state_shapes.insert(state_id, shape_index);
            previous = Some(state_id);
        }

        Ok(Self {
            version: raw.version,
            family_state_counts: raw.family_state_counts,
            shapes,
            state_shapes,
            max_box_y,
        })
    }
}

#[must_use]
pub fn vanilla_collision_shapes() -> &'static CollisionShapeTable {
    VANILLA_COLLISION_SHAPES.get_or_init(|| {
        CollisionShapeTable::from_json(RAW_COLLISION_SHAPES)
            .expect("embedded vanilla 26.1.2 collision shape data is valid")
    })
}

#[derive(Debug, Error)]
enum CollisionShapeError {
    #[error("collision shape data is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("collision shape data version is {0}, expected 26.1.2")]
    Version(String),
    #[error("collision shape units_per_block is {0}, expected 16")]
    Units(u8),
    #[error("collision shape {0} is empty")]
    EmptyShape(usize),
    #[error("collision shape {shape} has invalid box {coordinates:?}")]
    InvalidBox { shape: usize, coordinates: [u8; 6] },
    #[error("collision state entries are duplicate or unsorted at state {0}")]
    StateOrder(u32),
    #[error("collision shape index {0} is out of range")]
    ShapeIndex(u32),
}

#[derive(Deserialize)]
struct RawTable {
    version: String,
    units_per_block: u8,
    family_state_counts: BTreeMap<String, u32>,
    shapes: Vec<Vec<[u8; 6]>>,
    entries: Vec<[u32; 2]>,
}

#[cfg(test)]
mod tests {
    use crate::blocks::solaris_required_blocks_report;

    use super::{CollisionBox, vanilla_collision_shapes};

    fn state_id(block_name: &str, properties: &[(&str, &str)]) -> u32 {
        let blocks = solaris_required_blocks_report();
        let block = blocks
            .iter()
            .find(|block| block.id.as_str() == block_name)
            .unwrap_or_else(|| panic!("missing block {block_name}"));
        block
            .states
            .iter()
            .find(|state| {
                properties.iter().all(|(name, value)| {
                    state.properties.get(*name).map(String::as_str) == Some(*value)
                })
            })
            .unwrap_or_else(|| panic!("missing state for {block_name}: {properties:?}"))
            .id
    }

    #[test]
    fn oracle_table_has_exact_farmland_shape_for_every_moisture_state() {
        let blocks = solaris_required_blocks_report();
        let farmland = blocks
            .iter()
            .find(|block| block.id.as_str() == "minecraft:farmland")
            .unwrap();
        let expected = [CollisionBox::new(0, 0, 0, 16, 15, 16)];

        assert_eq!(farmland.states.len(), 8);
        for state in &farmland.states {
            assert_eq!(
                vanilla_collision_shapes().get(state.id),
                Some(expected.as_slice()),
                "farmland state {} must retain the vanilla 15/16 collision top",
                state.id
            );
        }
    }

    #[test]
    fn oracle_table_is_pinned_to_the_named_pareto_families() {
        let table = vanilla_collision_shapes();

        assert_eq!(table.version(), "26.1.2");
        assert_eq!(table.family_state_count("farmland"), Some(8));
        for family in ["fence", "slab", "stairs"] {
            assert!(
                table
                    .family_state_count(family)
                    .is_some_and(|count| count > 0)
            );
        }
        assert_eq!(
            table.covered_state_count(),
            ["farmland", "fence", "slab", "stairs"]
                .into_iter()
                .map(|family| table.family_state_count(family).unwrap() as usize)
                .sum::<usize>()
        );
    }

    #[test]
    fn oracle_table_keeps_state_dependent_slab_stair_and_fence_boxes() {
        let shapes = vanilla_collision_shapes();
        let bottom_slab = state_id(
            "minecraft:stone_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        let top_slab = state_id(
            "minecraft:stone_slab",
            &[("type", "top"), ("waterlogged", "false")],
        );
        let straight_stair = state_id(
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        );
        let isolated_fence = state_id(
            "minecraft:oak_fence",
            &[
                ("east", "false"),
                ("north", "false"),
                ("south", "false"),
                ("west", "false"),
                ("waterlogged", "false"),
            ],
        );

        assert_eq!(
            shapes.get(bottom_slab),
            Some([CollisionBox::new(0, 0, 0, 16, 8, 16)].as_slice())
        );
        assert_eq!(
            shapes.get(top_slab),
            Some([CollisionBox::new(0, 8, 0, 16, 16, 16)].as_slice())
        );
        assert_eq!(
            shapes.get(straight_stair),
            Some(
                [
                    CollisionBox::new(0, 0, 0, 16, 8, 16),
                    CollisionBox::new(0, 8, 0, 16, 16, 8),
                ]
                .as_slice()
            )
        );
        assert_eq!(
            shapes.get(isolated_fence),
            Some([CollisionBox::new(6, 0, 6, 10, 24, 10)].as_slice())
        );
    }

    #[test]
    fn oracle_table_reports_maximum_local_collision_y() {
        assert_eq!(vanilla_collision_shapes().max_box_y(), 24);
    }
}
