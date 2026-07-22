//! Vanilla 26.1.2 context-free collision boxes for every embedded block state.
//!
//! Entity-dependent rules such as leather boots walking on powder snow belong
//! in the entity collision layer on top of this static baseline.

use std::sync::OnceLock;

use crate::Identifier;

const RAW_COLLISION_SHAPES: &[u8] = include_bytes!("../data/block_collision_shapes_26_1_2.bin");
const EXPECTED_VERSION: &str = "26.1.2";
pub const COLLISION_UNITS_PER_BLOCK: i16 = 4096;
const HEADER_LEN: usize = 32;
const MISSING_SHAPE: u16 = u16::MAX;
static VANILLA_COLLISION_SHAPES: OnceLock<CollisionShapeTable> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollisionBox {
    coordinates: [i16; 6],
}

impl CollisionBox {
    #[must_use]
    pub const fn new(
        min_x: i16,
        min_y: i16,
        min_z: i16,
        max_x: i16,
        max_y: i16,
        max_z: i16,
    ) -> Self {
        assert!(min_x < max_x && min_y < max_y && min_z < max_z);
        Self {
            coordinates: [min_x, min_y, min_z, max_x, max_y, max_z],
        }
    }

    #[must_use]
    pub const fn coordinates(self) -> [i16; 6] {
        self.coordinates
    }

    #[must_use]
    pub fn as_blocks(self) -> [f64; 6] {
        self.coordinates
            .map(|value| f64::from(value) / f64::from(COLLISION_UNITS_PER_BLOCK))
    }
}

#[derive(Debug)]
pub struct CollisionShapeTable {
    bytes: &'static [u8],
    state_count: usize,
    shape_count: usize,
    box_count: usize,
    fingerprints_offset: usize,
    shape_offsets_offset: usize,
    boxes_offset: usize,
    max_box_y: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionShape<'a> {
    bytes: &'a [u8],
}

impl<'a> CollisionShape<'a> {
    #[must_use]
    pub const fn len(self) -> usize {
        self.bytes.len() / 12
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    pub fn iter(self) -> impl ExactSizeIterator<Item = CollisionBox> + 'a {
        self.bytes.chunks_exact(12).map(|bytes| CollisionBox {
            coordinates: [
                read_i16(bytes, 0),
                read_i16(bytes, 2),
                read_i16(bytes, 4),
                read_i16(bytes, 6),
                read_i16(bytes, 8),
                read_i16(bytes, 10),
            ],
        })
    }

    #[must_use]
    pub fn is_full_cube(self) -> bool {
        self.len() == 1
            && self.iter().next().is_some_and(|collision_box| {
                collision_box.coordinates()
                    == [
                        0,
                        0,
                        0,
                        COLLISION_UNITS_PER_BLOCK,
                        COLLISION_UNITS_PER_BLOCK,
                        COLLISION_UNITS_PER_BLOCK,
                    ]
            })
    }
}

impl CollisionShapeTable {
    #[must_use]
    pub fn version(&self) -> &str {
        EXPECTED_VERSION
    }

    #[must_use]
    pub fn get(&self, state_id: u32) -> Option<CollisionShape<'_>> {
        let state_id = usize::try_from(state_id).ok()?;
        if state_id >= self.state_count {
            return None;
        }
        let shape = read_u16(self.bytes, HEADER_LEN + state_id * 2);
        if shape == MISSING_SHAPE {
            return None;
        }
        self.shape(shape as usize)
    }

    #[must_use]
    pub fn get_for_state(
        &self,
        state_id: u32,
        block: &Identifier,
        properties: &[(String, String)],
    ) -> Option<CollisionShape<'_>> {
        let index = usize::try_from(state_id).ok()?;
        if index >= self.state_count
            || read_u64(self.bytes, self.fingerprints_offset + index * 8)
                != state_fingerprint(block, properties)
        {
            return None;
        }
        self.get(state_id)
    }

    /// Checks exact farmland semantics independently of a state's numeric ID.
    #[must_use]
    pub fn is_exact_farmland_state(
        &self,
        block: &Identifier,
        properties: &[(String, String)],
    ) -> bool {
        block.as_str() == "minecraft:farmland"
            && properties.len() == 1
            && properties.iter().any(|(name, value)| {
                name == "moisture" && value.parse::<u8>().is_ok_and(|value| value <= 7)
            })
    }

    #[must_use]
    pub fn covered_state_count(&self) -> usize {
        self.state_count
    }

    #[must_use]
    pub const fn max_box_y(&self) -> i16 {
        self.max_box_y
    }

    #[must_use]
    pub fn max_box_y_blocks(&self) -> f64 {
        f64::from(self.max_box_y) / f64::from(COLLISION_UNITS_PER_BLOCK)
    }

    fn from_binary(bytes: &'static [u8]) -> Self {
        assert!(
            bytes.len() >= HEADER_LEN,
            "collision table header is truncated"
        );
        assert_eq!(&bytes[..8], b"SOLCOLL1", "collision table magic mismatch");
        assert_eq!(read_i16(bytes, 8), COLLISION_UNITS_PER_BLOCK);
        assert_eq!(read_u32(bytes, 24), 1, "unsupported collision table format");
        assert_eq!(
            read_u32(bytes, 28),
            0,
            "collision table reserved field is set"
        );
        let state_count = read_u32(bytes, 12) as usize;
        let shape_count = read_u32(bytes, 16) as usize;
        let box_count = read_u32(bytes, 20) as usize;
        let fingerprints_offset = HEADER_LEN + state_count * 2;
        let shape_offsets_offset = fingerprints_offset + state_count * 8;
        let boxes_offset = shape_offsets_offset + (shape_count + 1) * 4;
        let expected_len = boxes_offset + box_count * 12;
        assert_eq!(bytes.len(), expected_len, "collision table length mismatch");
        assert_eq!(
            read_u32(bytes, shape_offsets_offset + shape_count * 4) as usize,
            box_count,
            "collision table terminal box offset mismatch"
        );
        for state in 0..state_count {
            let shape = read_u16(bytes, HEADER_LEN + state * 2);
            assert!(
                shape == MISSING_SHAPE || usize::from(shape) < shape_count,
                "collision table state shape index is out of range"
            );
        }
        let mut previous_offset = 0;
        for shape in 0..=shape_count {
            let offset = read_u32(bytes, shape_offsets_offset + shape * 4) as usize;
            assert!(
                offset >= previous_offset && offset <= box_count,
                "collision table shape offsets are invalid"
            );
            previous_offset = offset;
        }
        let mut actual_max_y = 0;
        for index in 0..box_count {
            let offset = boxes_offset + index * 12;
            let coordinates = [
                read_i16(bytes, offset),
                read_i16(bytes, offset + 2),
                read_i16(bytes, offset + 4),
                read_i16(bytes, offset + 6),
                read_i16(bytes, offset + 8),
                read_i16(bytes, offset + 10),
            ];
            assert!(
                coordinates[0] < coordinates[3]
                    && coordinates[1] < coordinates[4]
                    && coordinates[2] < coordinates[5],
                "collision table contains an invalid box"
            );
            actual_max_y = actual_max_y.max(coordinates[4]);
        }
        assert_eq!(
            read_i16(bytes, 10),
            actual_max_y,
            "collision table maximum Y mismatch"
        );
        Self {
            bytes,
            state_count,
            shape_count,
            box_count,
            fingerprints_offset,
            shape_offsets_offset,
            boxes_offset,
            max_box_y: read_i16(bytes, 10),
        }
    }

    fn shape(&self, shape: usize) -> Option<CollisionShape<'_>> {
        if shape >= self.shape_count {
            return None;
        }
        let start = read_u32(self.bytes, self.shape_offsets_offset + shape * 4) as usize;
        let end = read_u32(self.bytes, self.shape_offsets_offset + (shape + 1) * 4) as usize;
        if start > end || end > self.box_count {
            return None;
        }
        Some(CollisionShape {
            bytes: &self.bytes[self.boxes_offset + start * 12..self.boxes_offset + end * 12],
        })
    }
}

#[must_use]
pub fn vanilla_collision_shapes() -> &'static CollisionShapeTable {
    VANILLA_COLLISION_SHAPES.get_or_init(|| CollisionShapeTable::from_binary(RAW_COLLISION_SHAPES))
}

fn state_fingerprint(block: &Identifier, properties: &[(String, String)]) -> u64 {
    let mut hash = fnv1a(0xcbf2_9ce4_8422_2325, block.as_str().as_bytes());
    for (name, value) in properties {
        hash = fnv1a(hash, b"\0");
        hash = fnv1a(hash, name.as_bytes());
        hash = fnv1a(hash, b"=");
        hash = fnv1a(hash, value.as_bytes());
    }
    hash
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("u16 slice"))
}

fn read_i16(bytes: &[u8], offset: usize) -> i16 {
    i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("i16 slice"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32 slice"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(bytes[offset..offset + 8].try_into().expect("u64 slice"))
}

#[cfg(test)]
mod tests {
    use crate::Identifier;
    use crate::blocks::solaris_required_blocks_report;

    use super::{CollisionBox, vanilla_collision_shapes};

    fn box_16(
        min_x: i16,
        min_y: i16,
        min_z: i16,
        max_x: i16,
        max_y: i16,
        max_z: i16,
    ) -> CollisionBox {
        CollisionBox::new(
            min_x * 256,
            min_y * 256,
            min_z * 256,
            max_x * 256,
            max_y * 256,
            max_z * 256,
        )
    }

    fn shape(state_id: u32) -> Vec<CollisionBox> {
        vanilla_collision_shapes()
            .get(state_id)
            .expect("state collision shape")
            .iter()
            .collect()
    }

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

    fn state_identity(
        block_name: &str,
        properties: &[(&str, &str)],
    ) -> (u32, Identifier, Vec<(String, String)>) {
        let blocks = solaris_required_blocks_report();
        let block = blocks
            .iter()
            .find(|block| block.id.as_str() == block_name)
            .unwrap_or_else(|| panic!("missing block {block_name}"));
        let state = block
            .states
            .iter()
            .find(|state| {
                properties.iter().all(|(name, value)| {
                    state.properties.get(*name).map(String::as_str) == Some(*value)
                })
            })
            .unwrap_or_else(|| panic!("missing state for {block_name}: {properties:?}"));
        (
            state.id,
            block.id.clone(),
            state
                .properties
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        )
    }

    #[test]
    fn oracle_table_has_exact_farmland_shape_for_every_moisture_state() {
        let blocks = solaris_required_blocks_report();
        let farmland = blocks
            .iter()
            .find(|block| block.id.as_str() == "minecraft:farmland")
            .unwrap();
        let expected = [box_16(0, 0, 0, 16, 15, 16)];

        assert_eq!(farmland.states.len(), 8);
        for state in &farmland.states {
            assert_eq!(
                shape(state.id),
                expected,
                "farmland state {} must retain the vanilla 15/16 collision top",
                state.id
            );
        }
    }

    #[test]
    fn oracle_table_covers_the_complete_vanilla_state_registry() {
        let table = vanilla_collision_shapes();

        assert_eq!(table.version(), "26.1.2");
        assert_eq!(table.covered_state_count(), 29_873);
    }

    #[test]
    fn oracle_table_keeps_state_dependent_slab_stair_and_fence_boxes() {
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

        assert_eq!(shape(bottom_slab), [box_16(0, 0, 0, 16, 8, 16)]);
        assert_eq!(shape(top_slab), [box_16(0, 8, 0, 16, 16, 16)]);
        assert_eq!(
            shape(straight_stair),
            [box_16(0, 0, 0, 16, 8, 16), box_16(0, 8, 0, 16, 16, 8)]
        );
        assert_eq!(shape(isolated_fence), [box_16(6, 0, 6, 10, 24, 10)]);
    }

    #[test]
    fn oracle_table_keeps_closed_and_open_door_planes() {
        let shapes = vanilla_collision_shapes();
        let closed = state_id(
            "minecraft:oak_door",
            &[
                ("facing", "north"),
                ("half", "lower"),
                ("hinge", "left"),
                ("open", "false"),
                ("powered", "false"),
            ],
        );
        let open = state_id(
            "minecraft:oak_door",
            &[
                ("facing", "north"),
                ("half", "lower"),
                ("hinge", "left"),
                ("open", "true"),
                ("powered", "false"),
            ],
        );

        assert_ne!(shapes.get(closed), shapes.get(open));
        assert!(shapes.get(closed).is_some_and(|boxes| boxes.len() == 1));
        assert!(shapes.get(open).is_some_and(|boxes| boxes.len() == 1));
    }

    #[test]
    fn exact_lookup_accepts_canonical_slab_stair_fence_and_farmland_states() {
        let table = vanilla_collision_shapes();
        for (block_name, properties) in [
            (
                "minecraft:stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")][..],
            ),
            (
                "minecraft:oak_stairs",
                &[
                    ("facing", "north"),
                    ("half", "bottom"),
                    ("shape", "straight"),
                    ("waterlogged", "false"),
                ][..],
            ),
            (
                "minecraft:oak_fence",
                &[
                    ("east", "false"),
                    ("north", "false"),
                    ("south", "false"),
                    ("west", "false"),
                    ("waterlogged", "false"),
                ][..],
            ),
            ("minecraft:farmland", &[("moisture", "0")][..]),
        ] {
            let (state_id, identifier, properties) = state_identity(block_name, properties);
            assert_eq!(
                table.get_for_state(state_id, &identifier, &properties),
                table.get(state_id),
                "canonical state identity must unlock its table shape for {block_name}"
            );
        }
    }

    #[test]
    fn exact_lookup_rejects_overlapping_id_name_and_property_mismatches() {
        let table = vanilla_collision_shapes();
        let (state_id, identifier, properties) = state_identity(
            "minecraft:stone_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        let synthetic_slab = Identifier::parse("minecraft:synthetic_slab").unwrap();
        let mut wrong_properties = properties.clone();
        wrong_properties[0].1 = "synthetic".to_string();
        let mut reordered_properties = properties.clone();
        reordered_properties.reverse();

        assert!(
            table
                .get_for_state(state_id, &synthetic_slab, &properties)
                .is_none()
        );
        assert!(
            table
                .get_for_state(state_id, &identifier, &wrong_properties)
                .is_none()
        );
        assert!(
            table
                .get_for_state(state_id, &identifier, &reordered_properties)
                .is_none()
        );
    }

    #[test]
    fn farmland_fallback_requires_exact_canonical_semantics() {
        let table = vanilla_collision_shapes();
        let (_, farmland, properties) = state_identity("minecraft:farmland", &[("moisture", "0")]);
        let fake_properties = vec![
            ("type".to_string(), "bottom".to_string()),
            ("waterlogged".to_string(), "false".to_string()),
        ];

        assert!(table.is_exact_farmland_state(&farmland, &properties));
        assert!(!table.is_exact_farmland_state(&farmland, &fake_properties));
        assert!(
            !table.is_exact_farmland_state(
                &Identifier::parse("solaris:farmland").unwrap(),
                &properties,
            )
        );
    }

    #[test]
    fn oracle_table_reports_maximum_local_collision_y() {
        assert_eq!(vanilla_collision_shapes().max_box_y(), 24 * 256);
    }
}
