//! Pinned tests for the village voxel-shape algebra.
//!
//! Every number below came out of a scratch program (kept in `/tmp`, outside
//! the repo) that loaded the **real** 26.1.2 classes out of the bundled server
//! jar — `net.minecraft.world.phys.shapes.Shapes`, `VoxelShape`,
//! `BitSetDiscreteVoxelShape`, `DiscreteVoxelShape` and
//! `net.minecraft.world.phys.AABB` — built the same shapes, and printed
//! `Shapes.joinIsNotEmpty` / `Shapes.join(...).toAabbs()` /
//! `VoxelShape.min|max` / the reflective `DiscreteVoxelShape.getSize` values,
//! plus a plain AABB-overlap baseline computed from the shapes' own `min`/`max`.
//!
//! Doubles are pinned as integer nanounits (`Math.round(v * 1e9)` on the Java
//! side, [`units`] here), with `+inf`/`-inf` as `i64::MAX`/`i64::MIN`, so the
//! assertions are exact integer comparisons rather than decimal text.

use crate::village::shapes::{Aabb, Axis, BooleanOp, VoxelShape};

/// The Java reference printed `Math.round(v * 1e9)`; infinities are sentinels
/// because an empty shape's `min`/`max` are `+inf`/`-inf`.
fn units(value: f64) -> i64 {
    if value == f64::INFINITY {
        i64::MAX
    } else if value == f64::NEG_INFINITY {
        i64::MIN
    } else {
        (value * 1e9).round() as i64
    }
}

/// The shapes the Java reference built, by reference label.
fn shape(label: &str) -> VoxelShape {
    match label {
        "block" => VoxelShape::create_box(0.0, 0.0, 0.0, 1.0, 1.0, 1.0),
        "half" => VoxelShape::create_box(0.0, 0.0, 0.0, 0.5, 1.0, 1.0),
        "halfHi" => VoxelShape::create_box(0.5, 0.0, 0.0, 1.0, 1.0, 1.0),
        "cross" => VoxelShape::create_box(0.0, 0.0, 0.0, 0.25, 0.75, 1.0),
        "sixty" => VoxelShape::create_box(0.0625, 0.0, 0.0, 0.9375, 1.0, 1.0),
        "off" => VoxelShape::create_box(0.1875, 0.0, 0.0, 1.0625, 1.0, 1.0),
        "big" => VoxelShape::create(Aabb::of(0, 0, 0, 3, 2, 3)),
        "small" => VoxelShape::create(Aabb::of(1, 0, 1, 2, 2, 2)),
        "target" => VoxelShape::create(Aabb::of(1, 0, 1, 2, 2, 2).deflate(0.25)),
        "none" => VoxelShape::create_box(0.0, 0.0, 0.0, 0.0, 1.0, 1.0),
        "zero" => VoxelShape::create(Aabb::of(0, 0, 0, 5, 5, 5).deflate(3.0)),
        "thin" => VoxelShape::create(Aabb::of(0, 0, 0, 0, 0, 0).deflate(0.25)),
        "inverted" => VoxelShape::create(Aabb::new(0.0, 0.0, 0.0, 0.2, 1.0, 1.0).deflate(0.25)),
        "epsilon" => VoxelShape::create_box(1.0 + 5e-8, 0.0, 0.0, 2.0, 1.0, 1.0),
        "eighth" => VoxelShape::create_box(0.0, 0.0, 0.0, 0.125, 1.0, 1.0),
        "overlapEps" => VoxelShape::create_box(1.0 - 5e-8, 0.0, 0.0, 2.0, 1.0, 1.0),
        "overlapEpsWide" => VoxelShape::create_box(1.0 - 1e-6, 0.0, 0.0, 2.0, 1.0, 1.0),
        "far" => VoxelShape::create_box(2.0, 0.0, 0.0, 3.0, 1.0, 1.0),
        "free1" => shape("big").join_unoptimized(&shape("small"), BooleanOp::OnlyFirst),
        "free2" => shape("free1").join_unoptimized(&shape("target"), BooleanOp::OnlyFirst),
        other => panic!("unknown reference shape {other}"),
    }
}

fn op(label: &str) -> BooleanOp {
    match label {
        "ONLY_FIRST" => BooleanOp::OnlyFirst,
        "ONLY_SECOND" => BooleanOp::OnlySecond,
        "BOTH" => BooleanOp::Both,
        other => panic!("unknown reference op {other}"),
    }
}

fn minmax(shape: &VoxelShape) -> [i64; 6] {
    [
        units(shape.min(Axis::X)),
        units(shape.min(Axis::Y)),
        units(shape.min(Axis::Z)),
        units(shape.max(Axis::X)),
        units(shape.max(Axis::Y)),
        units(shape.max(Axis::Z)),
    ]
}

fn boxes(shape: &VoxelShape) -> Vec<[i64; 6]> {
    shape
        .to_aabbs()
        .iter()
        .map(|aabb| {
            [
                units(aabb.min_x),
                units(aabb.min_y),
                units(aabb.min_z),
                units(aabb.max_x),
                units(aabb.max_y),
                units(aabb.max_z),
            ]
        })
        .collect()
}

/// The plain AABB-overlap baseline the Java reference computed from the same
/// shape `min`/`max` values: intersecting open intervals on all three axes.
fn naive_overlap(first: &VoxelShape, second: &VoxelShape) -> bool {
    Axis::ALL
        .iter()
        .all(|&axis| first.min(axis) < second.max(axis) && second.min(axis) < first.max(axis))
}

/// One reference shape: label, empty, cell counts, min/max, maximal boxes.
type ShapePin = (
    &'static str,
    bool,
    [usize; 3],
    [i64; 6],
    &'static [[i64; 6]],
);

/// The Java reference's `Shapes.create` shapes.
const SHAPE_PINS: &[ShapePin] = &[
    (
        "block",
        false,
        [1, 1, 1],
        [0, 0, 0, 1000000000, 1000000000, 1000000000],
        &[[0, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    (
        "half",
        false,
        [2, 1, 1],
        [0, 0, 0, 500000000, 1000000000, 1000000000],
        &[[0, 0, 0, 500000000, 1000000000, 1000000000]],
    ),
    (
        "halfHi",
        false,
        [2, 1, 1],
        [500000000, 0, 0, 1000000000, 1000000000, 1000000000],
        &[[500000000, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    (
        "cross",
        false,
        [4, 4, 1],
        [0, 0, 0, 250000000, 750000000, 1000000000],
        &[[0, 0, 0, 250000000, 750000000, 1000000000]],
    ),
    (
        "sixty",
        false,
        [1, 1, 1],
        [62500000, 0, 0, 937500000, 1000000000, 1000000000],
        &[[62500000, 0, 0, 937500000, 1000000000, 1000000000]],
    ),
    (
        "off",
        false,
        [1, 1, 1],
        [187500000, 0, 0, 1062500000, 1000000000, 1000000000],
        &[[187500000, 0, 0, 1062500000, 1000000000, 1000000000]],
    ),
    (
        "big",
        false,
        [1, 1, 1],
        [0, 0, 0, 4000000000, 3000000000, 4000000000],
        &[[0, 0, 0, 4000000000, 3000000000, 4000000000]],
    ),
    (
        "small",
        false,
        [1, 1, 1],
        [
            1000000000, 0, 1000000000, 3000000000, 3000000000, 3000000000,
        ],
        &[[
            1000000000, 0, 1000000000, 3000000000, 3000000000, 3000000000,
        ]],
    ),
    (
        "target",
        false,
        [1, 1, 1],
        [
            1250000000, 250000000, 1250000000, 2750000000, 2750000000, 2750000000,
        ],
        &[[
            1250000000, 250000000, 1250000000, 2750000000, 2750000000, 2750000000,
        ]],
    ),
    (
        "none",
        true,
        [0, 0, 0],
        [i64::MAX, i64::MAX, i64::MAX, i64::MIN, i64::MIN, i64::MIN],
        &[],
    ),
    (
        "zero",
        true,
        [0, 0, 0],
        [i64::MAX, i64::MAX, i64::MAX, i64::MIN, i64::MIN, i64::MIN],
        &[],
    ),
    (
        "thin",
        false,
        [4, 4, 4],
        [
            250000000, 250000000, 250000000, 750000000, 750000000, 750000000,
        ],
        &[[
            250000000, 250000000, 250000000, 750000000, 750000000, 750000000,
        ]],
    ),
    (
        "inverted",
        false,
        [1, 1, 1],
        [
            -50000000, 250000000, 250000000, 250000000, 750000000, 750000000,
        ],
        &[[
            -50000000, 250000000, 250000000, 250000000, 750000000, 750000000,
        ]],
    ),
    (
        "epsilon",
        false,
        [1, 1, 1],
        [1000000050, 0, 0, 2000000000, 1000000000, 1000000000],
        &[[1000000050, 0, 0, 2000000000, 1000000000, 1000000000]],
    ),
    (
        "eighth",
        false,
        [8, 1, 1],
        [0, 0, 0, 125000000, 1000000000, 1000000000],
        &[[0, 0, 0, 125000000, 1000000000, 1000000000]],
    ),
    (
        "overlapEps",
        false,
        [1, 1, 1],
        [999999950, 0, 0, 2000000000, 1000000000, 1000000000],
        &[[999999950, 0, 0, 2000000000, 1000000000, 1000000000]],
    ),
    (
        "overlapEpsWide",
        false,
        [1, 1, 1],
        [999999000, 0, 0, 2000000000, 1000000000, 1000000000],
        &[[999999000, 0, 0, 2000000000, 1000000000, 1000000000]],
    ),
    (
        "far",
        false,
        [1, 1, 1],
        [2000000000, 0, 0, 3000000000, 1000000000, 1000000000],
        &[[2000000000, 0, 0, 3000000000, 1000000000, 1000000000]],
    ),
    (
        "free1",
        false,
        [3, 1, 3],
        [0, 0, 0, 4000000000, 3000000000, 4000000000],
        &[
            [0, 0, 0, 1000000000, 3000000000, 4000000000],
            [1000000000, 0, 0, 4000000000, 3000000000, 1000000000],
            [
                1000000000, 0, 3000000000, 4000000000, 3000000000, 4000000000,
            ],
            [
                3000000000, 0, 1000000000, 4000000000, 3000000000, 3000000000,
            ],
        ],
    ),
    (
        "free2",
        false,
        [5, 3, 5],
        [0, 0, 0, 4000000000, 3000000000, 4000000000],
        &[
            [0, 0, 0, 1000000000, 3000000000, 4000000000],
            [1000000000, 0, 0, 4000000000, 3000000000, 1000000000],
            [
                1000000000, 0, 3000000000, 4000000000, 3000000000, 4000000000,
            ],
            [
                3000000000, 0, 1000000000, 4000000000, 3000000000, 3000000000,
            ],
        ],
    ),
];

/// One reference join: first, second, op, isNotEmpty, cell counts, empty, boxes.
type JoinPin = (
    &'static str,
    &'static str,
    &'static str,
    bool,
    [usize; 3],
    bool,
    &'static [[i64; 6]],
);

/// The Java reference's `Shapes.join` / `Shapes.joinIsNotEmpty` rows.
const JOIN_PINS: &[JoinPin] = &[
    (
        "block",
        "sixty",
        "ONLY_FIRST",
        true,
        [3, 1, 1],
        false,
        &[
            [0, 0, 0, 62500000, 1000000000, 1000000000],
            [937500000, 0, 0, 1000000000, 1000000000, 1000000000],
        ],
    ),
    ("block", "sixty", "ONLY_SECOND", false, [0, 0, 0], true, &[]),
    (
        "block",
        "sixty",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[62500000, 0, 0, 937500000, 1000000000, 1000000000]],
    ),
    (
        "half",
        "cross",
        "ONLY_FIRST",
        true,
        [4, 4, 1],
        false,
        &[
            [250000000, 0, 0, 500000000, 1000000000, 1000000000],
            [0, 750000000, 0, 250000000, 1000000000, 1000000000],
        ],
    ),
    ("half", "cross", "ONLY_SECOND", false, [0, 0, 0], true, &[]),
    (
        "half",
        "cross",
        "BOTH",
        true,
        [4, 4, 1],
        false,
        &[[0, 0, 0, 250000000, 750000000, 1000000000]],
    ),
    (
        "big",
        "target",
        "ONLY_FIRST",
        true,
        [3, 3, 3],
        false,
        &[
            [0, 0, 0, 4000000000, 250000000, 4000000000],
            [0, 250000000, 0, 1250000000, 3000000000, 4000000000],
            [1250000000, 250000000, 0, 4000000000, 3000000000, 1250000000],
            [
                1250000000, 250000000, 2750000000, 4000000000, 3000000000, 4000000000,
            ],
            [
                2750000000, 250000000, 1250000000, 4000000000, 3000000000, 2750000000,
            ],
            [
                1250000000, 2750000000, 1250000000, 2750000000, 3000000000, 2750000000,
            ],
        ],
    ),
    ("big", "target", "ONLY_SECOND", false, [0, 0, 0], true, &[]),
    (
        "big",
        "target",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[
            1250000000, 250000000, 1250000000, 2750000000, 2750000000, 2750000000,
        ]],
    ),
    (
        "big",
        "small",
        "ONLY_FIRST",
        true,
        [3, 1, 3],
        false,
        &[
            [0, 0, 0, 1000000000, 3000000000, 4000000000],
            [1000000000, 0, 0, 4000000000, 3000000000, 1000000000],
            [
                1000000000, 0, 3000000000, 4000000000, 3000000000, 4000000000,
            ],
            [
                3000000000, 0, 1000000000, 4000000000, 3000000000, 3000000000,
            ],
        ],
    ),
    ("big", "small", "ONLY_SECOND", false, [0, 0, 0], true, &[]),
    (
        "big",
        "small",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[
            1000000000, 0, 1000000000, 3000000000, 3000000000, 3000000000,
        ]],
    ),
    (
        "block",
        "epsilon",
        "ONLY_FIRST",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    (
        "block",
        "epsilon",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[1000000050, 0, 0, 2000000000, 1000000000, 1000000000]],
    ),
    ("block", "epsilon", "BOTH", false, [0, 0, 0], true, &[]),
    (
        "half",
        "halfHi",
        "ONLY_FIRST",
        true,
        [2, 1, 1],
        false,
        &[[0, 0, 0, 500000000, 1000000000, 1000000000]],
    ),
    (
        "half",
        "halfHi",
        "ONLY_SECOND",
        true,
        [2, 1, 1],
        false,
        &[[500000000, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    ("half", "halfHi", "BOTH", false, [0, 0, 0], true, &[]),
    ("block", "block", "ONLY_FIRST", false, [0, 0, 0], true, &[]),
    ("block", "block", "ONLY_SECOND", false, [0, 0, 0], true, &[]),
    (
        "block",
        "block",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    ("big", "big", "ONLY_FIRST", false, [0, 0, 0], true, &[]),
    ("big", "big", "ONLY_SECOND", false, [0, 0, 0], true, &[]),
    (
        "big",
        "big",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 4000000000, 3000000000, 4000000000]],
    ),
    ("none", "block", "ONLY_FIRST", false, [0, 0, 0], true, &[]),
    (
        "none",
        "block",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    ("none", "block", "BOTH", false, [0, 0, 0], true, &[]),
    (
        "half",
        "far",
        "ONLY_FIRST",
        true,
        [2, 1, 1],
        false,
        &[[0, 0, 0, 500000000, 1000000000, 1000000000]],
    ),
    (
        "half",
        "far",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[2000000000, 0, 0, 3000000000, 1000000000, 1000000000]],
    ),
    ("half", "far", "BOTH", false, [0, 0, 0], true, &[]),
    ("zero", "block", "ONLY_FIRST", false, [0, 0, 0], true, &[]),
    (
        "zero",
        "block",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    ("zero", "block", "BOTH", false, [0, 0, 0], true, &[]),
    (
        "off",
        "epsilon",
        "ONLY_FIRST",
        true,
        [1, 1, 1],
        false,
        &[[187500000, 0, 0, 1000000050, 1000000000, 1000000000]],
    ),
    (
        "off",
        "epsilon",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[1062500000, 0, 0, 2000000000, 1000000000, 1000000000]],
    ),
    (
        "off",
        "epsilon",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[1000000050, 0, 0, 1062500000, 1000000000, 1000000000]],
    ),
    (
        "block",
        "overlapEps",
        "ONLY_FIRST",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    (
        "block",
        "overlapEps",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[999999950, 0, 0, 2000000000, 1000000000, 1000000000]],
    ),
    ("block", "overlapEps", "BOTH", false, [0, 0, 0], true, &[]),
    (
        "block",
        "overlapEpsWide",
        "ONLY_FIRST",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 999999000, 1000000000, 1000000000]],
    ),
    (
        "block",
        "overlapEpsWide",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[1000000000, 0, 0, 2000000000, 1000000000, 1000000000]],
    ),
    (
        "block",
        "overlapEpsWide",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[999999000, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
    ("eighth", "half", "ONLY_FIRST", false, [0, 0, 0], true, &[]),
    (
        "eighth",
        "half",
        "ONLY_SECOND",
        true,
        [8, 1, 1],
        false,
        &[[125000000, 0, 0, 500000000, 1000000000, 1000000000]],
    ),
    (
        "eighth",
        "half",
        "BOTH",
        true,
        [8, 1, 1],
        false,
        &[[0, 0, 0, 125000000, 1000000000, 1000000000]],
    ),
    (
        "free1",
        "target",
        "ONLY_FIRST",
        true,
        [3, 1, 3],
        false,
        &[
            [0, 0, 0, 1000000000, 3000000000, 4000000000],
            [1000000000, 0, 0, 4000000000, 3000000000, 1000000000],
            [
                1000000000, 0, 3000000000, 4000000000, 3000000000, 4000000000,
            ],
            [
                3000000000, 0, 1000000000, 4000000000, 3000000000, 3000000000,
            ],
        ],
    ),
    (
        "free1",
        "target",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[
            1250000000, 250000000, 1250000000, 2750000000, 2750000000, 2750000000,
        ]],
    ),
    ("free1", "target", "BOTH", false, [0, 0, 0], true, &[]),
    (
        "free1",
        "small",
        "ONLY_FIRST",
        true,
        [3, 1, 3],
        false,
        &[
            [0, 0, 0, 1000000000, 3000000000, 4000000000],
            [1000000000, 0, 0, 4000000000, 3000000000, 1000000000],
            [
                1000000000, 0, 3000000000, 4000000000, 3000000000, 4000000000,
            ],
            [
                3000000000, 0, 1000000000, 4000000000, 3000000000, 3000000000,
            ],
        ],
    ),
    (
        "free1",
        "small",
        "ONLY_SECOND",
        true,
        [1, 1, 1],
        false,
        &[[
            1000000000, 0, 1000000000, 3000000000, 3000000000, 3000000000,
        ]],
    ),
    ("free1", "small", "BOTH", false, [0, 0, 0], true, &[]),
    (
        "free1",
        "block",
        "ONLY_FIRST",
        true,
        [3, 2, 3],
        false,
        &[
            [0, 0, 1000000000, 1000000000, 3000000000, 4000000000],
            [1000000000, 0, 0, 4000000000, 3000000000, 1000000000],
            [
                1000000000, 0, 3000000000, 4000000000, 3000000000, 4000000000,
            ],
            [
                3000000000, 0, 1000000000, 4000000000, 3000000000, 3000000000,
            ],
            [0, 1000000000, 0, 1000000000, 3000000000, 1000000000],
        ],
    ),
    ("free1", "block", "ONLY_SECOND", false, [0, 0, 0], true, &[]),
    (
        "free1",
        "block",
        "BOTH",
        true,
        [1, 1, 1],
        false,
        &[[0, 0, 0, 1000000000, 1000000000, 1000000000]],
    ),
];

/// The Java reference's plain AABB-overlap baseline.
const NAIVE_PINS: &[(&str, &str, bool)] = &[
    ("block", "sixty", true),
    ("half", "cross", true),
    ("big", "target", true),
    ("big", "small", true),
    ("block", "epsilon", false),
    ("half", "halfHi", false),
    ("block", "block", true),
    ("big", "big", true),
    ("none", "block", false),
    ("half", "far", false),
    ("zero", "block", false),
    ("off", "epsilon", true),
    ("block", "overlapEps", true),
    ("block", "overlapEpsWide", true),
    ("eighth", "half", true),
    ("free1", "target", true),
    ("free1", "small", true),
    ("free1", "block", true),
];

/// `Shapes.create`'s grid, empty flag, `min`/`max` and boxes, exactly as the
/// Java reference reported them.
#[test]
fn shape_construction_matches_java_reference() {
    for &(label, empty, grid, min_max, expected) in SHAPE_PINS {
        let shape = shape(label);
        assert_eq!(shape.is_empty(), empty, "{label}: empty");
        assert_eq!(
            [
                shape.grid_size(Axis::X),
                shape.grid_size(Axis::Y),
                shape.grid_size(Axis::Z),
            ],
            grid,
            "{label}: grid"
        );
        assert_eq!(minmax(&shape), min_max, "{label}: min/max");
        assert_eq!(boxes(&shape), expected, "{label}: boxes");
    }
}

/// `Shapes.join` (optimised) and `Shapes.joinIsNotEmpty` for every reference
/// pair and mode.
#[test]
fn join_and_join_is_not_empty_match_java_reference() {
    for &(first, second, op_label, not_empty, grid, empty, expected) in JOIN_PINS {
        let (a, b) = (shape(first), shape(second));
        let op = op(op_label);
        assert_eq!(
            VoxelShape::join_is_not_empty(&a, &b, op),
            not_empty,
            "{first}|{second} {op_label}: isNotEmpty"
        );
        let joined = a.join(&b, op);
        assert_eq!(
            joined.is_empty(),
            empty,
            "{first}|{second} {op_label}: empty"
        );
        assert_eq!(
            [
                joined.grid_size(Axis::X),
                joined.grid_size(Axis::Y),
                joined.grid_size(Axis::Z),
            ],
            grid,
            "{first}|{second} {op_label}: grid"
        );
        assert_eq!(
            boxes(&joined),
            expected,
            "{first}|{second} {op_label}: boxes"
        );
    }
}

/// The plain AABB-overlap baseline the reference computed from the same shapes.
#[test]
fn plain_box_overlap_matches_java_reference() {
    for &(first, second, overlap) in NAIVE_PINS {
        assert_eq!(
            naive_overlap(&shape(first), &shape(second)),
            overlap,
            "{first}|{second}: plain overlap"
        );
    }
}

/// The bug class the solver would otherwise inherit: the free-space test runs on
/// the merged discretised grid with a `1e-7` tolerance, so it is *not* an AABB
/// intersection test — in either direction.
#[test]
fn discretised_overlap_is_not_plain_box_overlap() {
    let block = shape("block");

    // Two boxes overlapping by 5e-8: `IndirectMerger` folds the coordinates onto
    // one grid line and the overlap disappears, while plain boxes intersect.
    let overlap_epsilon = shape("overlapEps");
    assert!(naive_overlap(&block, &overlap_epsilon));
    assert!(!VoxelShape::join_is_not_empty(
        &block,
        &overlap_epsilon,
        BooleanOp::Both
    ));
    assert!(block.join(&overlap_epsilon, BooleanOp::Both).is_empty());

    // A 1e-6 overlap is wider than the tolerance and survives as a sliver cell.
    let overlap_wide = shape("overlapEpsWide");
    assert!(naive_overlap(&block, &overlap_wide));
    assert!(VoxelShape::join_is_not_empty(
        &block,
        &overlap_wide,
        BooleanOp::Both
    ));
    assert_eq!(
        boxes(&block.join(&overlap_wide, BooleanOp::Both)),
        [[
            999_999_000,
            0,
            0,
            1_000_000_000,
            1_000_000_000,
            1_000_000_000
        ]]
    );

    // Overlapping boxes that add nothing: the 1/16 box sits inside the block's
    // single cell, so "second but not first" is empty.
    let sixty = shape("sixty");
    assert!(naive_overlap(&block, &sixty));
    assert!(!VoxelShape::join_is_not_empty(
        &block,
        &sixty,
        BooleanOp::OnlySecond
    ));

    // The reverse direction: a 5e-8 gap between the boxes still leaves the first
    // shape whole under `OnlyFirst`.
    let epsilon = shape("epsilon");
    assert!(!naive_overlap(&block, &epsilon));
    assert!(VoxelShape::join_is_not_empty(
        &block,
        &epsilon,
        BooleanOp::OnlyFirst
    ));
    assert_eq!(
        boxes(&block.join(&epsilon, BooleanOp::OnlyFirst)),
        [[0, 0, 0, 1_000_000_000, 1_000_000_000, 1_000_000_000]]
    );
}

/// The `JigsawPlacement$Placer` sequence end to end: the deflated target is
/// accepted while the free space covers it, and placing it removes its box from
/// the free space so the same spot is no longer free.
#[test]
fn jigsaw_free_space_accept_then_remove() {
    let big = shape("big");
    let target = shape("target");

    // `Shapes.create(AABB.of(sourceBB))` is one cell spanning the whole bounding
    // box, and the deflated target is inside it: accepted (no uncovered part).
    assert!(!VoxelShape::join_is_not_empty(
        &big,
        &target,
        BooleanOp::OnlySecond
    ));

    // `joinUnoptimized(childrenFree, create(AABB.of(targetBB)), ONLY_FIRST)`.
    let free = big.join_unoptimized(&shape("small"), BooleanOp::OnlyFirst);
    assert_eq!(
        [
            free.grid_size(Axis::X),
            free.grid_size(Axis::Y),
            free.grid_size(Axis::Z),
        ],
        [3, 1, 3]
    );
    assert_eq!(
        boxes(&free),
        [
            [0, 0, 0, 1_000_000_000, 3_000_000_000, 4_000_000_000],
            [
                1_000_000_000,
                0,
                0,
                4_000_000_000,
                3_000_000_000,
                1_000_000_000
            ],
            [
                1_000_000_000,
                0,
                3_000_000_000,
                4_000_000_000,
                3_000_000_000,
                4_000_000_000
            ],
            [
                3_000_000_000,
                0,
                1_000_000_000,
                4_000_000_000,
                3_000_000_000,
                3_000_000_000
            ],
        ]
    );
    assert!(VoxelShape::join_is_not_empty(
        &free,
        &target,
        BooleanOp::OnlySecond
    ));

    // Removing the deflated target splits the grid again, and the target's own
    // region is then reported as not free.
    let free = free.join_unoptimized(&target, BooleanOp::OnlyFirst);
    assert_eq!(
        [
            free.grid_size(Axis::X),
            free.grid_size(Axis::Y),
            free.grid_size(Axis::Z),
        ],
        [5, 3, 5]
    );
    assert!(VoxelShape::join_is_not_empty(
        &free,
        &target,
        BooleanOp::OnlySecond
    ));
}

/// `Shapes.findBits` reaches eighth-of-a-block cells and no finer: a 1/16 offset
/// misses every grid line and falls back to the exact-coordinate single cell,
/// and so does any box outside `[0, 1]`.
#[test]
fn grid_reaches_eighths_and_sixteenths_keep_exact_coords() {
    assert_eq!(shape("half").grid_size(Axis::X), 2);
    assert_eq!(shape("cross").grid_size(Axis::X), 4);
    assert_eq!(shape("eighth").grid_size(Axis::X), 8);
    assert_eq!(shape("sixty").grid_size(Axis::X), 1);
    assert_eq!(
        boxes(&shape("sixty")),
        [[62_500_000, 0, 0, 937_500_000, 1_000_000_000, 1_000_000_000]]
    );
    assert_eq!(shape("big").grid_size(Axis::X), 1);
    assert_eq!(
        boxes(&shape("big")),
        [[0, 0, 0, 4_000_000_000, 3_000_000_000, 4_000_000_000]]
    );
}

/// `AABB.of`, `deflate`, `inflate` and the constructor's clamp.
#[test]
fn aabb_bounds_and_deflate_match_vanilla() {
    let block = Aabb::of(1, 0, 1, 2, 2, 2);
    assert_eq!(
        [
            units(block.min_x),
            units(block.min_y),
            units(block.min_z),
            units(block.max_x),
            units(block.max_y),
            units(block.max_z),
        ],
        [
            1_000_000_000,
            0,
            1_000_000_000,
            3_000_000_000,
            3_000_000_000,
            3_000_000_000,
        ]
    );
    assert_eq!(units(block.min(Axis::Y)), 0);
    assert_eq!(units(block.max(Axis::Z)), 3_000_000_000);

    let target = block.deflate(0.25);
    assert_eq!(
        [
            units(target.min_x),
            units(target.min_y),
            units(target.min_z),
            units(target.max_x),
            units(target.max_y),
            units(target.max_z),
        ],
        [
            1_250_000_000,
            250_000_000,
            1_250_000_000,
            2_750_000_000,
            2_750_000_000,
            2_750_000_000,
        ]
    );
    assert_eq!(units(target.inflate(0.25).min_x), units(block.min_x));

    // A deflate wider than the box is clamped by the constructor into a swapped
    // box, which is what the solver sees as an out-of-range target box.
    let inverted = Aabb::new(0.0, 0.0, 0.0, 0.2, 1.0, 1.0).deflate(0.25);
    assert_eq!(units(inverted.min_x), -50_000_000);
    assert_eq!(units(inverted.max_x), 250_000_000);
    assert_eq!(
        boxes(&VoxelShape::create(inverted)),
        [[
            -50_000_000,
            250_000_000,
            250_000_000,
            250_000_000,
            750_000_000,
            750_000_000
        ]]
    );

    // A zero-width box is empty, and so is a `create` whose min exceeds its max.
    assert!(VoxelShape::create(Aabb::of(0, 0, 0, 5, 5, 5).deflate(3.0)).is_empty());
    assert!(VoxelShape::create_box(0.5, 0.0, 0.0, 0.25, 1.0, 1.0).is_empty());
}
