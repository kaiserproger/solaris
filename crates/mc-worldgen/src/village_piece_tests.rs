//! Reference-pinned tests for village piece placement (`village::piece`).
//!
//! Every expected value in this file is derived from the real 26.1.2 classes,
//! not transcribed from a file: `net.minecraft.core.PlaceRef` — a scratch runner
//! kept in `/tmp` — drove the real `StructureTemplate.transform`,
//! `StructureTemplate.placeInWorld` (with `SinglePoolElement`/
//! `LegacySinglePoolElement`'s settings) and `BlockState.rotate`/`mirror` from
//! the bundled 26.1.2 server jar, and printed the tables below. The fixtures
//! here are Solaris-authored synthetic templates in vanilla's NBT shape; only
//! the outcomes are vanilla's.
//!
//! Three reference facts back the placement layer:
//!
//! - the **transform table** (24 rows: 3 mirrors × 4 rotations × 2 pivots) and
//!   the **state table** (32 curated village block states × 3 rotations ×
//!   2 mirrors) are the reference's own printed outcomes;
//! - the **sweep**: over every distinct block state of every village template in
//!   the content cache (483 templates, 617 states) the property rule this module
//!   implements agrees with the real `BlockState.rotate`/`mirror` for all four
//!   rotations and all three mirrors, with zero disagreements;
//! - the **real placement**: `desert_meeting_point_1`, rotated
//!   `CLOCKWISE_90` at `(128, 64, -256)` with the settings its real pool element
//!   implies, produced 107 blocks unclipped and 40 under a clip that keeps
//!   `z <= -246`; the reference's positions and states for a sample of those
//!   blocks are pinned in [`REAL_PLACEMENT`] and [`REAL_CLIPPED`].

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use mc_data::Identifier;
use mc_data::vanilla_feature_closure::BlockStateSpec;
use mc_data::village_data::{
    PoolElementSpec, PosRuleTestSpec, ProcessorRef, ProcessorRuleSpec, Projection, RuleTestSpec,
    StructureProcessorSpec, VillageDataLoader,
};
use mc_nbt::{ListTag, Tag, tag_type};
use mc_world::{BlockPos, BlockRegistry, BlockStateId};

use crate::structures::{StructureTemplate, TemplateChest};
use crate::vanilla_features::{BlockSemantics, BlockTagIndex, LegacyRandom};
use crate::village::piece::{
    BlockClip, Mirror, PieceSettings, PieceWriter, mirror_state, place_piece, rotate_state,
    transform,
};
use crate::village::processors::{PieceElement, ProcessLevel};
use crate::village::solver::Rotation;

// ------------------------------------------------------- reference fixtures

/// One reference row: the mirror, the rotation, the pivot, and the four
/// transformed probe positions.
type TransformRow = (&'static str, &'static str, [i32; 3], [[i32; 3]; 4]);

/// `StructureTemplate.transform`, as the reference printed it: (mirror, rotation,
/// pivot, [transformed (0,0,0), (1,2,3), (-4,5,-6), (17,0,9)]).
const TRANSFORM_TABLE: [TransformRow; 24] = [
    (
        "NONE",
        "NONE",
        [0, 0, 0],
        [[0, 0, 0], [1, 2, 3], [-4, 5, -6], [17, 0, 9]],
    ),
    (
        "NONE",
        "NONE",
        [3, 0, -2],
        [[0, 0, 0], [1, 2, 3], [-4, 5, -6], [17, 0, 9]],
    ),
    (
        "NONE",
        "CLOCKWISE_90",
        [0, 0, 0],
        [[0, 0, 0], [-3, 2, 1], [6, 5, -4], [-9, 0, 17]],
    ),
    (
        "NONE",
        "CLOCKWISE_90",
        [3, 0, -2],
        [[1, 0, -5], [-2, 2, -4], [7, 5, -9], [-8, 0, 12]],
    ),
    (
        "NONE",
        "CLOCKWISE_180",
        [0, 0, 0],
        [[0, 0, 0], [-1, 2, -3], [4, 5, 6], [-17, 0, -9]],
    ),
    (
        "NONE",
        "CLOCKWISE_180",
        [3, 0, -2],
        [[6, 0, -4], [5, 2, -7], [10, 5, 2], [-11, 0, -13]],
    ),
    (
        "NONE",
        "COUNTERCLOCKWISE_90",
        [0, 0, 0],
        [[0, 0, 0], [3, 2, -1], [-6, 5, 4], [9, 0, -17]],
    ),
    (
        "NONE",
        "COUNTERCLOCKWISE_90",
        [3, 0, -2],
        [[5, 0, 1], [8, 2, 0], [-1, 5, 5], [14, 0, -16]],
    ),
    (
        "LEFT_RIGHT",
        "NONE",
        [0, 0, 0],
        [[0, 0, 0], [1, 2, -3], [-4, 5, 6], [17, 0, -9]],
    ),
    (
        "LEFT_RIGHT",
        "NONE",
        [3, 0, -2],
        [[0, 0, 0], [1, 2, -3], [-4, 5, 6], [17, 0, -9]],
    ),
    (
        "LEFT_RIGHT",
        "CLOCKWISE_90",
        [0, 0, 0],
        [[0, 0, 0], [3, 2, 1], [-6, 5, -4], [9, 0, 17]],
    ),
    (
        "LEFT_RIGHT",
        "CLOCKWISE_90",
        [3, 0, -2],
        [[1, 0, -5], [4, 2, -4], [-5, 5, -9], [10, 0, 12]],
    ),
    (
        "LEFT_RIGHT",
        "CLOCKWISE_180",
        [0, 0, 0],
        [[0, 0, 0], [-1, 2, 3], [4, 5, -6], [-17, 0, 9]],
    ),
    (
        "LEFT_RIGHT",
        "CLOCKWISE_180",
        [3, 0, -2],
        [[6, 0, -4], [5, 2, -1], [10, 5, -10], [-11, 0, 5]],
    ),
    (
        "LEFT_RIGHT",
        "COUNTERCLOCKWISE_90",
        [0, 0, 0],
        [[0, 0, 0], [-3, 2, -1], [6, 5, 4], [-9, 0, -17]],
    ),
    (
        "LEFT_RIGHT",
        "COUNTERCLOCKWISE_90",
        [3, 0, -2],
        [[5, 0, 1], [2, 2, 0], [11, 5, 5], [-4, 0, -16]],
    ),
    (
        "FRONT_BACK",
        "NONE",
        [0, 0, 0],
        [[0, 0, 0], [-1, 2, 3], [4, 5, -6], [-17, 0, 9]],
    ),
    (
        "FRONT_BACK",
        "NONE",
        [3, 0, -2],
        [[0, 0, 0], [-1, 2, 3], [4, 5, -6], [-17, 0, 9]],
    ),
    (
        "FRONT_BACK",
        "CLOCKWISE_90",
        [0, 0, 0],
        [[0, 0, 0], [-3, 2, -1], [6, 5, 4], [-9, 0, -17]],
    ),
    (
        "FRONT_BACK",
        "CLOCKWISE_90",
        [3, 0, -2],
        [[1, 0, -5], [-2, 2, -6], [7, 5, -1], [-8, 0, -22]],
    ),
    (
        "FRONT_BACK",
        "CLOCKWISE_180",
        [0, 0, 0],
        [[0, 0, 0], [1, 2, -3], [-4, 5, 6], [17, 0, -9]],
    ),
    (
        "FRONT_BACK",
        "CLOCKWISE_180",
        [3, 0, -2],
        [[6, 0, -4], [7, 2, -7], [2, 5, 2], [23, 0, -13]],
    ),
    (
        "FRONT_BACK",
        "COUNTERCLOCKWISE_90",
        [0, 0, 0],
        [[0, 0, 0], [3, 2, 1], [-6, 5, -4], [9, 0, 17]],
    ),
    (
        "FRONT_BACK",
        "COUNTERCLOCKWISE_90",
        [3, 0, -2],
        [[5, 0, 1], [8, 2, 2], [-1, 5, -3], [14, 0, 18]],
    ),
];

/// `BlockState.rotate`/`mirror` over 32 curated village-block states, as the
/// reference printed them: (state, [CLOCKWISE_90, CLOCKWISE_180,
/// COUNTERCLOCKWISE_90], [LEFT_RIGHT, FRONT_BACK]).
const STATE_TABLE: [(&str, [&str; 3], [&str; 2]); 32] = [
    (
        "minecraft:oak_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]",
        [
            "minecraft:oak_stairs[facing=south,half=bottom,shape=straight,waterlogged=false]",
            "minecraft:oak_stairs[facing=west,half=bottom,shape=straight,waterlogged=false]",
            "minecraft:oak_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
        ],
        [
            "minecraft:oak_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]",
            "minecraft:oak_stairs[facing=west,half=bottom,shape=straight,waterlogged=false]",
        ],
    ),
    (
        "minecraft:oak_stairs[facing=north,half=top,shape=inner_left,waterlogged=false]",
        [
            "minecraft:oak_stairs[facing=east,half=top,shape=inner_left,waterlogged=false]",
            "minecraft:oak_stairs[facing=south,half=top,shape=inner_left,waterlogged=false]",
            "minecraft:oak_stairs[facing=west,half=top,shape=inner_left,waterlogged=false]",
        ],
        [
            "minecraft:oak_stairs[facing=south,half=top,shape=inner_right,waterlogged=false]",
            "minecraft:oak_stairs[facing=north,half=top,shape=inner_left,waterlogged=false]",
        ],
    ),
    (
        "minecraft:oak_stairs[facing=west,half=bottom,shape=outer_right,waterlogged=true]",
        [
            "minecraft:oak_stairs[facing=north,half=bottom,shape=outer_right,waterlogged=true]",
            "minecraft:oak_stairs[facing=east,half=bottom,shape=outer_right,waterlogged=true]",
            "minecraft:oak_stairs[facing=south,half=bottom,shape=outer_right,waterlogged=true]",
        ],
        [
            "minecraft:oak_stairs[facing=west,half=bottom,shape=outer_right,waterlogged=true]",
            "minecraft:oak_stairs[facing=east,half=bottom,shape=outer_left,waterlogged=true]",
        ],
    ),
    (
        "minecraft:smooth_sandstone_stairs[facing=south,half=bottom,shape=inner_right,waterlogged=false]",
        [
            "minecraft:smooth_sandstone_stairs[facing=west,half=bottom,shape=inner_right,waterlogged=false]",
            "minecraft:smooth_sandstone_stairs[facing=north,half=bottom,shape=inner_right,waterlogged=false]",
            "minecraft:smooth_sandstone_stairs[facing=east,half=bottom,shape=inner_right,waterlogged=false]",
        ],
        [
            "minecraft:smooth_sandstone_stairs[facing=north,half=bottom,shape=inner_left,waterlogged=false]",
            "minecraft:smooth_sandstone_stairs[facing=south,half=bottom,shape=inner_right,waterlogged=false]",
        ],
    ),
    (
        "minecraft:spruce_stairs[facing=north,half=bottom,shape=outer_left,waterlogged=false]",
        [
            "minecraft:spruce_stairs[facing=east,half=bottom,shape=outer_left,waterlogged=false]",
            "minecraft:spruce_stairs[facing=south,half=bottom,shape=outer_left,waterlogged=false]",
            "minecraft:spruce_stairs[facing=west,half=bottom,shape=outer_left,waterlogged=false]",
        ],
        [
            "minecraft:spruce_stairs[facing=south,half=bottom,shape=outer_right,waterlogged=false]",
            "minecraft:spruce_stairs[facing=north,half=bottom,shape=outer_left,waterlogged=false]",
        ],
    ),
    (
        "minecraft:oak_door[facing=east,half=lower,hinge=left,open=false,powered=false]",
        [
            "minecraft:oak_door[facing=south,half=lower,hinge=left,open=false,powered=false]",
            "minecraft:oak_door[facing=west,half=lower,hinge=left,open=false,powered=false]",
            "minecraft:oak_door[facing=north,half=lower,hinge=left,open=false,powered=false]",
        ],
        [
            "minecraft:oak_door[facing=east,half=lower,hinge=right,open=false,powered=false]",
            "minecraft:oak_door[facing=west,half=lower,hinge=right,open=false,powered=false]",
        ],
    ),
    (
        "minecraft:oak_door[facing=north,half=upper,hinge=right,open=false,powered=false]",
        [
            "minecraft:oak_door[facing=east,half=upper,hinge=right,open=false,powered=false]",
            "minecraft:oak_door[facing=south,half=upper,hinge=right,open=false,powered=false]",
            "minecraft:oak_door[facing=west,half=upper,hinge=right,open=false,powered=false]",
        ],
        [
            "minecraft:oak_door[facing=south,half=upper,hinge=left,open=false,powered=false]",
            "minecraft:oak_door[facing=north,half=upper,hinge=left,open=false,powered=false]",
        ],
    ),
    (
        "minecraft:oak_trapdoor[facing=north,half=top,open=true,powered=false,waterlogged=false]",
        [
            "minecraft:oak_trapdoor[facing=east,half=top,open=true,powered=false,waterlogged=false]",
            "minecraft:oak_trapdoor[facing=south,half=top,open=true,powered=false,waterlogged=false]",
            "minecraft:oak_trapdoor[facing=west,half=top,open=true,powered=false,waterlogged=false]",
        ],
        [
            "minecraft:oak_trapdoor[facing=south,half=top,open=true,powered=false,waterlogged=false]",
            "minecraft:oak_trapdoor[facing=north,half=top,open=true,powered=false,waterlogged=false]",
        ],
    ),
    (
        "minecraft:oak_fence[east=false,north=true,south=false,waterlogged=false,west=true]",
        [
            "minecraft:oak_fence[east=true,north=true,south=false,waterlogged=false,west=false]",
            "minecraft:oak_fence[east=true,north=false,south=true,waterlogged=false,west=false]",
            "minecraft:oak_fence[east=false,north=false,south=true,waterlogged=false,west=true]",
        ],
        [
            "minecraft:oak_fence[east=false,north=false,south=true,waterlogged=false,west=true]",
            "minecraft:oak_fence[east=true,north=true,south=false,waterlogged=false,west=false]",
        ],
    ),
    (
        "minecraft:cobblestone_wall[east=none,north=tall,south=low,up=true,waterlogged=false,west=none]",
        [
            "minecraft:cobblestone_wall[east=tall,north=none,south=none,up=true,waterlogged=false,west=low]",
            "minecraft:cobblestone_wall[east=none,north=low,south=tall,up=true,waterlogged=false,west=none]",
            "minecraft:cobblestone_wall[east=low,north=none,south=none,up=true,waterlogged=false,west=tall]",
        ],
        [
            "minecraft:cobblestone_wall[east=none,north=low,south=tall,up=true,waterlogged=false,west=none]",
            "minecraft:cobblestone_wall[east=none,north=tall,south=low,up=true,waterlogged=false,west=none]",
        ],
    ),
    (
        "minecraft:glass_pane[east=false,north=true,south=false,waterlogged=false,west=true]",
        [
            "minecraft:glass_pane[east=true,north=true,south=false,waterlogged=false,west=false]",
            "minecraft:glass_pane[east=true,north=false,south=true,waterlogged=false,west=false]",
            "minecraft:glass_pane[east=false,north=false,south=true,waterlogged=false,west=true]",
        ],
        [
            "minecraft:glass_pane[east=false,north=false,south=true,waterlogged=false,west=true]",
            "minecraft:glass_pane[east=true,north=true,south=false,waterlogged=false,west=false]",
        ],
    ),
    (
        "minecraft:oak_log[axis=x]",
        [
            "minecraft:oak_log[axis=z]",
            "minecraft:oak_log[axis=x]",
            "minecraft:oak_log[axis=z]",
        ],
        ["minecraft:oak_log[axis=x]", "minecraft:oak_log[axis=x]"],
    ),
    (
        "minecraft:hay_block[axis=y]",
        [
            "minecraft:hay_block[axis=y]",
            "minecraft:hay_block[axis=y]",
            "minecraft:hay_block[axis=y]",
        ],
        ["minecraft:hay_block[axis=y]", "minecraft:hay_block[axis=y]"],
    ),
    (
        "minecraft:oak_sign[rotation=5,waterlogged=false]",
        [
            "minecraft:oak_sign[rotation=9,waterlogged=false]",
            "minecraft:oak_sign[rotation=13,waterlogged=false]",
            "minecraft:oak_sign[rotation=1,waterlogged=false]",
        ],
        [
            "minecraft:oak_sign[rotation=3,waterlogged=false]",
            "minecraft:oak_sign[rotation=11,waterlogged=false]",
        ],
    ),
    (
        "minecraft:white_banner[rotation=11]",
        [
            "minecraft:white_banner[rotation=15]",
            "minecraft:white_banner[rotation=3]",
            "minecraft:white_banner[rotation=7]",
        ],
        [
            "minecraft:white_banner[rotation=13]",
            "minecraft:white_banner[rotation=5]",
        ],
    ),
    (
        "minecraft:jigsaw[orientation=up_south]",
        [
            "minecraft:jigsaw[orientation=up_west]",
            "minecraft:jigsaw[orientation=up_north]",
            "minecraft:jigsaw[orientation=up_east]",
        ],
        [
            "minecraft:jigsaw[orientation=up_north]",
            "minecraft:jigsaw[orientation=up_south]",
        ],
    ),
    (
        "minecraft:jigsaw[orientation=north_up]",
        [
            "minecraft:jigsaw[orientation=east_up]",
            "minecraft:jigsaw[orientation=south_up]",
            "minecraft:jigsaw[orientation=west_up]",
        ],
        [
            "minecraft:jigsaw[orientation=south_up]",
            "minecraft:jigsaw[orientation=north_up]",
        ],
    ),
    (
        "minecraft:bell[attachment=floor,facing=east,powered=false]",
        [
            "minecraft:bell[attachment=floor,facing=south,powered=false]",
            "minecraft:bell[attachment=floor,facing=west,powered=false]",
            "minecraft:bell[attachment=floor,facing=north,powered=false]",
        ],
        [
            "minecraft:bell[attachment=floor,facing=east,powered=false]",
            "minecraft:bell[attachment=floor,facing=west,powered=false]",
        ],
    ),
    (
        "minecraft:chest[facing=west,type=single,waterlogged=false]",
        [
            "minecraft:chest[facing=north,type=single,waterlogged=false]",
            "minecraft:chest[facing=east,type=single,waterlogged=false]",
            "minecraft:chest[facing=south,type=single,waterlogged=false]",
        ],
        [
            "minecraft:chest[facing=west,type=single,waterlogged=false]",
            "minecraft:chest[facing=east,type=single,waterlogged=false]",
        ],
    ),
    (
        "minecraft:campfire[facing=south,lit=true,signal_fire=false,waterlogged=false]",
        [
            "minecraft:campfire[facing=west,lit=true,signal_fire=false,waterlogged=false]",
            "minecraft:campfire[facing=north,lit=true,signal_fire=false,waterlogged=false]",
            "minecraft:campfire[facing=east,lit=true,signal_fire=false,waterlogged=false]",
        ],
        [
            "minecraft:campfire[facing=north,lit=true,signal_fire=false,waterlogged=false]",
            "minecraft:campfire[facing=south,lit=true,signal_fire=false,waterlogged=false]",
        ],
    ),
    (
        "minecraft:torch",
        ["minecraft:torch", "minecraft:torch", "minecraft:torch"],
        ["minecraft:torch", "minecraft:torch"],
    ),
    (
        "minecraft:ladder[facing=west,waterlogged=false]",
        [
            "minecraft:ladder[facing=north,waterlogged=false]",
            "minecraft:ladder[facing=east,waterlogged=false]",
            "minecraft:ladder[facing=south,waterlogged=false]",
        ],
        [
            "minecraft:ladder[facing=west,waterlogged=false]",
            "minecraft:ladder[facing=east,waterlogged=false]",
        ],
    ),
    (
        "minecraft:oak_slab[type=top,waterlogged=true]",
        [
            "minecraft:oak_slab[type=top,waterlogged=true]",
            "minecraft:oak_slab[type=top,waterlogged=true]",
            "minecraft:oak_slab[type=top,waterlogged=true]",
        ],
        [
            "minecraft:oak_slab[type=top,waterlogged=true]",
            "minecraft:oak_slab[type=top,waterlogged=true]",
        ],
    ),
    (
        "minecraft:vine[east=false,north=true,south=false,up=false,west=true]",
        [
            "minecraft:vine[east=true,north=true,south=false,up=false,west=false]",
            "minecraft:vine[east=true,north=false,south=true,up=false,west=false]",
            "minecraft:vine[east=false,north=false,south=true,up=false,west=true]",
        ],
        [
            "minecraft:vine[east=false,north=false,south=true,up=false,west=true]",
            "minecraft:vine[east=true,north=true,south=false,up=false,west=false]",
        ],
    ),
    (
        "minecraft:lantern[hanging=true,waterlogged=false]",
        [
            "minecraft:lantern[hanging=true,waterlogged=false]",
            "minecraft:lantern[hanging=true,waterlogged=false]",
            "minecraft:lantern[hanging=true,waterlogged=false]",
        ],
        [
            "minecraft:lantern[hanging=true,waterlogged=false]",
            "minecraft:lantern[hanging=true,waterlogged=false]",
        ],
    ),
    (
        "minecraft:sandstone",
        [
            "minecraft:sandstone",
            "minecraft:sandstone",
            "minecraft:sandstone",
        ],
        ["minecraft:sandstone", "minecraft:sandstone"],
    ),
    (
        "minecraft:water[level=3]",
        [
            "minecraft:water[level=3]",
            "minecraft:water[level=3]",
            "minecraft:water[level=3]",
        ],
        ["minecraft:water[level=3]", "minecraft:water[level=3]"],
    ),
    (
        "minecraft:potted_cactus",
        [
            "minecraft:potted_cactus",
            "minecraft:potted_cactus",
            "minecraft:potted_cactus",
        ],
        ["minecraft:potted_cactus", "minecraft:potted_cactus"],
    ),
    (
        "minecraft:barrel[facing=up,open=false]",
        [
            "minecraft:barrel[facing=up,open=false]",
            "minecraft:barrel[facing=up,open=false]",
            "minecraft:barrel[facing=up,open=false]",
        ],
        [
            "minecraft:barrel[facing=up,open=false]",
            "minecraft:barrel[facing=up,open=false]",
        ],
    ),
    (
        "minecraft:grindstone[face=floor,facing=north]",
        [
            "minecraft:grindstone[face=floor,facing=east]",
            "minecraft:grindstone[face=floor,facing=south]",
            "minecraft:grindstone[face=floor,facing=west]",
        ],
        [
            "minecraft:grindstone[face=floor,facing=south]",
            "minecraft:grindstone[face=floor,facing=north]",
        ],
    ),
    (
        "minecraft:stonecutter[facing=north]",
        [
            "minecraft:stonecutter[facing=east]",
            "minecraft:stonecutter[facing=south]",
            "minecraft:stonecutter[facing=west]",
        ],
        [
            "minecraft:stonecutter[facing=south]",
            "minecraft:stonecutter[facing=north]",
        ],
    ),
    (
        "minecraft:oak_fence_gate[facing=east,in_wall=false,open=false,powered=false]",
        [
            "minecraft:oak_fence_gate[facing=south,in_wall=false,open=false,powered=false]",
            "minecraft:oak_fence_gate[facing=west,in_wall=false,open=false,powered=false]",
            "minecraft:oak_fence_gate[facing=north,in_wall=false,open=false,powered=false]",
        ],
        [
            "minecraft:oak_fence_gate[facing=east,in_wall=false,open=false,powered=false]",
            "minecraft:oak_fence_gate[facing=west,in_wall=false,open=false,powered=false]",
        ],
    ),
];

/// The reference's real placement of `village/desert/town_centers/
/// desert_meeting_point_1` at `(128, 64, -256)`, `CLOCKWISE_90`, unclipped: one
/// block per distinct state it produced, plus the jigsaw position whose
/// `final_state` replaced it.
const REAL_PLACEMENT: [([i32; 3], &str); 9] = [
    // A `minecraft:jigsaw` block whose `final_state` is
    // `minecraft:smooth_sandstone`: the processor chain replaced it.
    ([124, 64, -248], "minecraft:smooth_sandstone"),
    (
        [124, 66, -246],
        "minecraft:bell[attachment=floor,facing=south,powered=false]",
    ),
    ([123, 65, -246], "minecraft:cut_sandstone"),
    ([126, 66, -245], "minecraft:potted_cactus"),
    ([123, 64, -246], "minecraft:sand"),
    ([120, 64, -249], "minecraft:smooth_sandstone"),
    ([124, 68, -244], "minecraft:terracotta"),
    ([124, 69, -244], "minecraft:torch"),
    ([123, 65, -245], "minecraft:water[level=0]"),
];

/// Positions the reference dropped: the `structure_void` jigsaws (inside the
/// box, so only the processor chain can remove them).
const REAL_VOID_JIGSAWS: [[i32; 3]; 3] = [[126, 65, -249], [124, 65, -240], [122, 65, -247]];

/// Blocks the reference placed unclipped but not under the clip
/// `(120, -64, -256)..(143, 319, -246)`.
const REAL_CLIPPED: [([i32; 3], &str); 4] = [
    ([122, 64, -245], "minecraft:sand"),
    ([126, 64, -245], "minecraft:sand"),
    ([122, 64, -244], "minecraft:sand"),
    ([124, 64, -244], "minecraft:sand"),
];

/// The reference's counts for that template.
const REAL_PLACEMENT_COUNT: usize = 107;
const REAL_CLIPPED_COUNT: usize = 40;

// ---------------------------------------------------------------- fixtures

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

/// Resolve a state by name. Omitted properties keep the block's default value,
/// as `BlockState.CODEC` reads a JSON state.
fn state_of(blocks: &BlockRegistry, name: &str, properties: &[(&str, &str)]) -> BlockStateId {
    let id = identifier(name);
    let block = blocks
        .block(&id)
        .unwrap_or_else(|| panic!("{name} is a registered block"));
    let default = blocks.by_id(block.default).expect("default states resolve");
    let resolved: Vec<(String, String)> = default
        .properties
        .iter()
        .map(|(key, value)| {
            let value = properties
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| (*value).to_owned())
                .unwrap_or_else(|| value.clone());
            (key.clone(), value)
        })
        .collect();
    blocks
        .by_name_and_props(&id, &resolved)
        .unwrap_or_else(|| panic!("{name}{resolved:?} is a registered state"))
}

/// The reference printed `namespace:path[prop=value,...]` with the properties
/// sorted; mirror it so the fixtures below are the reference's own text.
fn render(blocks: &BlockRegistry, state: BlockStateId) -> String {
    let state = blocks.by_id(state).expect("placed states resolve");
    let mut properties: Vec<String> = state
        .properties
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    properties.sort();
    if properties.is_empty() {
        state.block.id.as_str().to_owned()
    } else {
        format!("{}[{}]", state.block.id.as_str(), properties.join(","))
    }
}

/// `<SOLARIS_CONTENT_CACHE>`, else `/tmp/jdk-cold2`, else `<workspace>/data/vanilla`.
///
/// The real-cache tests read the vanilla content cache from one of these and
/// skip loudly when none is present.
fn content_cache() -> Option<PathBuf> {
    let env = std::env::var("SOLARIS_CONTENT_CACHE")
        .ok()
        .map(PathBuf::from);
    let scratch = Some(PathBuf::from("/tmp/jdk-cold2"));
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("data").join("vanilla"));
    env.into_iter()
        .chain(scratch)
        .chain(workspace)
        .find(|dir| dir.join("data").join("minecraft").join("worldgen").is_dir())
}

/// Synthetic tag index: no village test here consults a tag, so the index is
/// empty and any lookup is a miss.
#[derive(Default)]
struct TestTags;

impl BlockTagIndex for TestTags {
    fn block_in_tag(&self, _tag: &Identifier, _block: &Identifier) -> bool {
        false
    }
}

/// In-memory world plus the piece sink: [`ProcessLevel`] reads `world`, and the
/// write records what the piece placed.
struct RecordingWriter {
    air: BlockStateId,
    world: HashMap<BlockPos, BlockStateId>,
    blocks: Vec<(BlockPos, BlockStateId)>,
    chests: Vec<(BlockPos, u64)>,
}

impl RecordingWriter {
    fn new(air: BlockStateId) -> Self {
        Self {
            air,
            world: HashMap::new(),
            blocks: Vec::new(),
            chests: Vec::new(),
        }
    }

    fn state_at(&self, pos: [i32; 3]) -> Option<BlockStateId> {
        let pos = BlockPos {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
        self.blocks
            .iter()
            .find(|(placed, _)| *placed == pos)
            .map(|(_, state)| *state)
    }
}

impl ProcessLevel for RecordingWriter {
    fn block_state(&self, pos: BlockPos) -> BlockStateId {
        self.world.get(&pos).copied().unwrap_or(self.air)
    }

    fn height(&self, _heightmap: mc_data::village_data::HeightmapType, _x: i32, _z: i32) -> i32 {
        64
    }
}

impl PieceWriter for RecordingWriter {
    fn set_block(&mut self, pos: BlockPos, state: BlockStateId) {
        self.world.insert(pos, state);
        self.blocks.push((pos, state));
    }

    fn set_chest(&mut self, pos: BlockPos, _chest: &TemplateChest, loot_seed: u64) {
        self.chests.push((pos, loot_seed));
    }
}

struct Fixture {
    blocks: BlockRegistry,
    tags: TestTags,
}

impl Fixture {
    fn new() -> Self {
        let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("the embedded required-block report is well formed");
        Self {
            blocks,
            tags: TestTags,
        }
    }

    fn semantics(&self) -> BlockSemantics<'_> {
        BlockSemantics::new(&self.blocks, &self.tags)
    }

    fn air(&self) -> BlockStateId {
        state_of(&self.blocks, "minecraft:air", &[])
    }

    fn render(&self, state: BlockStateId) -> String {
        render(&self.blocks, state)
    }

    fn writer(&self) -> RecordingWriter {
        RecordingWriter::new(self.air())
    }
}

// ------------------------------------------------- synthetic NBT templates

fn compound(fields: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(
        fields
            .into_iter()
            .map(|(name, tag)| (name.to_owned(), tag))
            .collect(),
    )
}

fn text(value: &str) -> Tag {
    Tag::String(value.to_owned())
}

fn int(value: i32) -> Tag {
    Tag::Int(value)
}

fn list(element_type: u8, elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type,
        elements,
    })
}

fn triplet(values: [i32; 3]) -> Tag {
    list(
        tag_type::INT,
        values.iter().map(|value| int(*value)).collect(),
    )
}

fn palette_entry(name: &str, properties: &[(&str, &str)]) -> Tag {
    let mut fields = vec![("Name", text(name))];
    if !properties.is_empty() {
        fields.push((
            "Properties",
            compound(
                properties
                    .iter()
                    .map(|(key, value)| (*key, text(value)))
                    .collect(),
            ),
        ));
    }
    compound(fields)
}

/// A jigsaw block entity's payload, in the shape the loader reads.
fn jigsaw_nbt(final_state: &str) -> Tag {
    compound(vec![
        ("name", text("minecraft:street")),
        ("target", text("minecraft:street")),
        ("pool", text("minecraft:village/desert/streets")),
        ("joint", text("aligned")),
        ("final_state", text(final_state)),
        ("selection_priority", int(0)),
    ])
}

/// Write a synthetic template to a temp file and load it through the real
/// loader, so the fixtures exercise palette/block/jigsaw parsing too.
fn load_template(blocks: &BlockRegistry, root: Tag) -> StructureTemplate {
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &root).expect("the synthetic template serialises");
    let mut file = tempfile::NamedTempFile::new().expect("a scratch template file");
    file.write_all(&bytes).expect("the template bytes write");
    file.flush().expect("the template bytes flush");
    StructureTemplate::from_nbt_file(file.path(), blocks).expect("the synthetic template loads")
}

/// The synthetic village piece every placement test places.
///
/// `size` is `[4, 2, 2]`; the palette and block list are:
///
/// ```text
/// (0,0,0) oak_stairs[facing=east]                     rotated by the piece
/// (1,0,0) jigsaw, final_state "minecraft:oak_stairs[facing=east]]"
///         (the trailing `]` and the missing half/shape/waterlogged are the
///          real NBT shape the loader tolerates)
/// (2,0,0) jigsaw, final_state "minecraft:structure_void"
/// (3,0,0) stone
/// (0,0,1) oak_slab[type=bottom,waterlogged=false]     the waterlogging case
/// (1,0,1) structure_block[mode=save]                  the ignore processors
/// (2,0,1) chest[facing=north]                         plus a chest block entity
/// (3,0,1) oak_door[facing=east]                       the air-producing rule
/// ```
fn synthetic_piece(blocks: &BlockRegistry) -> StructureTemplate {
    let root = compound(vec![
        ("size", triplet([4, 2, 2])),
        (
            "palette",
            list(
                tag_type::COMPOUND,
                vec![
                    palette_entry(
                        "minecraft:oak_stairs",
                        &[
                            ("facing", "east"),
                            ("half", "bottom"),
                            ("shape", "straight"),
                            ("waterlogged", "false"),
                        ],
                    ),
                    palette_entry("minecraft:jigsaw", &[("orientation", "east_up")]),
                    palette_entry("minecraft:stone", &[]),
                    palette_entry("minecraft:structure_block", &[("mode", "save")]),
                    palette_entry(
                        "minecraft:oak_slab",
                        &[("type", "bottom"), ("waterlogged", "false")],
                    ),
                    palette_entry(
                        "minecraft:oak_door",
                        &[
                            ("facing", "east"),
                            ("half", "lower"),
                            ("hinge", "left"),
                            ("open", "false"),
                            ("powered", "false"),
                        ],
                    ),
                    palette_entry(
                        "minecraft:chest",
                        &[
                            ("facing", "north"),
                            ("type", "single"),
                            ("waterlogged", "false"),
                        ],
                    ),
                ],
            ),
        ),
        (
            "blocks",
            list(
                tag_type::COMPOUND,
                vec![
                    compound(vec![("pos", triplet([0, 0, 0])), ("state", int(0))]),
                    compound(vec![
                        ("pos", triplet([1, 0, 0])),
                        ("state", int(1)),
                        ("nbt", jigsaw_nbt("minecraft:oak_stairs[facing=east]]")),
                    ]),
                    compound(vec![
                        ("pos", triplet([2, 0, 0])),
                        ("state", int(1)),
                        ("nbt", jigsaw_nbt("minecraft:structure_void")),
                    ]),
                    compound(vec![("pos", triplet([3, 0, 0])), ("state", int(2))]),
                    compound(vec![("pos", triplet([0, 0, 1])), ("state", int(4))]),
                    compound(vec![("pos", triplet([1, 0, 1])), ("state", int(3))]),
                    compound(vec![("pos", triplet([2, 0, 1])), ("state", int(6))]),
                    compound(vec![("pos", triplet([3, 0, 1])), ("state", int(5))]),
                ],
            ),
        ),
    ]);
    load_template(blocks, root).with_chests(vec![TemplateChest {
        pos: [2, 0, 1],
        chest: mc_world::ChestBlockEntity::default(),
        loot_table: Some(identifier("minecraft:chests/village/village_house")),
    }])
}

/// The piece settings a village element produces: `rigid` projection, no
/// element processor list, `legacy_single_pool_element`.
fn piece_settings<'a>(
    position: BlockPos,
    rotation: Rotation,
    element: PieceElement,
    processors: &'a [StructureProcessorSpec],
    owner: &'a Identifier,
) -> PieceSettings<'a> {
    PieceSettings {
        position,
        reference_pos: BlockPos {
            x: 100,
            y: 64,
            z: 200,
        },
        rotation,
        mirror: Mirror::None,
        projection: Projection::Rigid,
        element,
        processors,
        owner,
        clip: None,
    }
}

fn spec(name: &str, properties: &[(&str, &str)]) -> BlockStateSpec {
    BlockStateSpec {
        block: identifier(name),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

/// A one-rule `minecraft:rule` processor: `input` matches, `output` is written.
fn rule(input: RuleTestSpec, output: &str, properties: &[(&str, &str)]) -> StructureProcessorSpec {
    StructureProcessorSpec::Rule {
        rules: vec![ProcessorRuleSpec {
            input_predicate: input,
            location_predicate: RuleTestSpec::AlwaysTrue,
            position_predicate: PosRuleTestSpec::AlwaysTrue,
            output_state: spec(output, properties),
        }],
    }
}

fn block_match(name: &str) -> RuleTestSpec {
    RuleTestSpec::BlockMatch {
        block: identifier(name),
    }
}

/// The position the synthetic piece is placed at, and the world positions its
/// blocks land on under `CLOCKWISE_90`: `(x, y, z) -> (100 - z, 64, 200 + x)`.
const ORIGIN: BlockPos = BlockPos {
    x: 100,
    y: 64,
    z: 200,
};

// ------------------------------------------------------------------ tests

#[test]
fn transform_matches_the_vanilla_reference() {
    for (mirror, rotation, pivot, expected) in TRANSFORM_TABLE {
        let mirror = match mirror {
            "NONE" => Mirror::None,
            "LEFT_RIGHT" => Mirror::LeftRight,
            "FRONT_BACK" => Mirror::FrontBack,
            other => panic!("unknown mirror {other}"),
        };
        let rotation = match rotation {
            "NONE" => Rotation::None,
            "CLOCKWISE_90" => Rotation::Clockwise90,
            "CLOCKWISE_180" => Rotation::Clockwise180,
            "COUNTERCLOCKWISE_90" => Rotation::CounterClockwise90,
            other => panic!("unknown rotation {other}"),
        };
        for (position, want) in [[0, 0, 0], [1, 2, 3], [-4, 5, -6], [17, 0, 9]]
            .into_iter()
            .zip(expected)
        {
            assert_eq!(
                transform(position, mirror, rotation, pivot),
                want,
                "{mirror:?} {rotation:?} pivot={pivot:?} {position:?}"
            );
        }
    }
}

/// Parse the reference's rendered state text (`minecraft:oak_stairs[facing=east,
/// ...]`) back into a registry state.
fn parse_render(blocks: &BlockRegistry, text: &str) -> BlockStateId {
    let (name, rest) = match text.split_once('[') {
        Some((name, rest)) => (name, Some(rest.trim_end_matches(']'))),
        None => (text, None),
    };
    let properties: Vec<(&str, &str)> = rest
        .map(|inner| {
            inner
                .split(',')
                .map(|pair| pair.split_once('=').expect("a rendered property"))
                .collect()
        })
        .unwrap_or_default();
    state_of(blocks, name, &properties)
}

#[test]
fn rotate_and_mirror_state_match_the_vanilla_reference() {
    let fixture = Fixture::new();
    for (state, rotations, mirrors) in STATE_TABLE {
        let state = parse_render(&fixture.blocks, state);
        for (rotation, want) in [
            Rotation::Clockwise90,
            Rotation::Clockwise180,
            Rotation::CounterClockwise90,
        ]
        .into_iter()
        .zip(rotations)
        {
            let got = rotate_state(&fixture.blocks, state, rotation)
                .expect("the rotated state is registered");
            assert_eq!(fixture.render(got), want, "{state:?} {rotation:?}");
        }
        for (mirror, want) in [Mirror::LeftRight, Mirror::FrontBack]
            .into_iter()
            .zip(mirrors)
        {
            let got = mirror_state(&fixture.blocks, state, mirror)
                .expect("the mirrored state is registered");
            assert_eq!(fixture.render(got), want, "{state:?} {mirror:?}");
        }
        assert_eq!(
            rotate_state(&fixture.blocks, state, Rotation::None),
            Some(state)
        );
        assert_eq!(
            mirror_state(&fixture.blocks, state, Mirror::None),
            Some(state)
        );
    }
}

#[test]
fn place_piece_rotates_the_layout_and_substitutes_final_states() {
    let fixture = Fixture::new();
    let template = synthetic_piece(&fixture.blocks);
    let semantics = fixture.semantics();
    let processors: Vec<StructureProcessorSpec> = Vec::new();
    let owner = identifier("minecraft:village/desert/town_centers");
    let mut writer = fixture.writer();
    let written = place_piece(
        &semantics,
        &template,
        &piece_settings(
            ORIGIN,
            Rotation::Clockwise90,
            PieceElement::LegacySingle,
            &processors,
            &owner,
        ),
        &mut writer,
        Some(&mut LegacyRandom::new(0)),
    )
    .expect("the synthetic piece places");

    assert_eq!(written, 6, "two of the eight entries are dropped");
    let expected = [
        (
            [100, 64, 200],
            "minecraft:oak_stairs[facing=south,half=bottom,shape=straight,waterlogged=false]",
        ),
        (
            [100, 64, 201],
            "minecraft:oak_stairs[facing=south,half=bottom,shape=straight,waterlogged=false]",
        ),
        ([100, 64, 203], "minecraft:stone"),
        (
            [99, 64, 200],
            "minecraft:oak_slab[type=bottom,waterlogged=false]",
        ),
        (
            [99, 64, 202],
            "minecraft:chest[facing=east,type=single,waterlogged=false]",
        ),
        (
            [99, 64, 203],
            "minecraft:oak_door[facing=south,half=lower,hinge=left,open=false,powered=false]",
        ),
    ];
    for (position, state) in expected {
        assert_eq!(
            writer.state_at(position).map(|state| fixture.render(state)),
            Some(state.to_owned()),
            "at {position:?}"
        );
    }
    assert_eq!(
        writer.state_at([100, 64, 202]),
        None,
        "structure_void jigsaw"
    );
    assert_eq!(writer.state_at([99, 64, 201]), None, "structure_block");
    // The chest's block entity carries the `LootTableSeed` vanilla draws for it
    // from the placing structure's random (`StructureTemplate.placeInWorld`):
    // the first draw of the source this test handed `place_piece`.
    assert_eq!(
        writer.chests,
        vec![(
            BlockPos {
                x: 99,
                y: 64,
                z: 202
            },
            13_483_975_608_033_169_720
        )]
    );
}

#[test]
fn place_piece_clips_to_the_bounding_box() {
    let fixture = Fixture::new();
    let template = synthetic_piece(&fixture.blocks);
    let semantics = fixture.semantics();
    let processors: Vec<StructureProcessorSpec> = Vec::new();
    let owner = identifier("minecraft:village/desert/town_centers");
    let mut writer = fixture.writer();
    let mut piece = piece_settings(
        ORIGIN,
        Rotation::Clockwise90,
        PieceElement::LegacySingle,
        &processors,
        &owner,
    );
    // Keeps only the `x = 100` column, i.e. the piece's `z = 0` row.
    piece.clip = Some(BlockClip::new([100, 64, 200], [100, 64, 203]));
    let mut random = LegacyRandom::new(0);
    let before = random.state();
    let written = place_piece(
        &semantics,
        &template,
        &piece,
        &mut writer,
        Some(&mut random),
    )
    .expect("the piece places");

    assert_eq!(written, 3);
    assert_eq!(
        writer
            .state_at([100, 64, 200])
            .map(|state| fixture.render(state)),
        Some(
            "minecraft:oak_stairs[facing=south,half=bottom,shape=straight,waterlogged=false]"
                .to_owned()
        )
    );
    assert_eq!(
        writer
            .state_at([100, 64, 201])
            .map(|state| fixture.render(state)),
        Some(
            "minecraft:oak_stairs[facing=south,half=bottom,shape=straight,waterlogged=false]"
                .to_owned()
        )
    );
    assert_eq!(
        writer
            .state_at([100, 64, 203])
            .map(|state| fixture.render(state)),
        Some("minecraft:stone".to_owned())
    );
    assert_eq!(writer.state_at([99, 64, 200]), None, "outside the clip");
    assert_eq!(writer.state_at([99, 64, 202]), None, "outside the clip");
    assert!(
        writer.chests.is_empty(),
        "a clipped chest block is not written, so its block entity is not either"
    );
    assert_eq!(
        random.state(),
        before,
        "a chest the clip dropped must not draw a loot seed from the stream"
    );
}

#[test]
fn waterlogged_states_follow_the_world_water() {
    let fixture = Fixture::new();
    let template = synthetic_piece(&fixture.blocks);
    let semantics = fixture.semantics();
    let processors: Vec<StructureProcessorSpec> = Vec::new();
    let owner = identifier("minecraft:village/desert/town_centers");
    let water = state_of(&fixture.blocks, "minecraft:water", &[]);

    let mut writer = fixture.writer();
    writer.world.insert(
        BlockPos {
            x: 99,
            y: 64,
            z: 200,
        },
        water,
    );
    place_piece(
        &semantics,
        &template,
        &piece_settings(
            ORIGIN,
            Rotation::Clockwise90,
            PieceElement::LegacySingle,
            &processors,
            &owner,
        ),
        &mut writer,
        Some(&mut LegacyRandom::new(0)),
    )
    .expect("the piece places");
    assert_eq!(
        writer
            .state_at([99, 64, 200])
            .map(|state| fixture.render(state)),
        Some("minecraft:oak_slab[type=bottom,waterlogged=true]".to_owned()),
        "a waterlogged-able block written into water is waterlogged"
    );
    assert_eq!(
        writer
            .state_at([100, 64, 203])
            .map(|state| fixture.render(state)),
        Some("minecraft:stone".to_owned()),
        "a block without a waterlogged property is untouched"
    );
}

#[test]
fn processors_apply_in_list_order_per_block() {
    let fixture = Fixture::new();
    let template = synthetic_piece(&fixture.blocks);
    let semantics = fixture.semantics();
    let owner = identifier("minecraft:village/desert/town_centers");
    let stairs_to_stone = rule(block_match("minecraft:oak_stairs"), "minecraft:stone", &[]);
    let stone_to_cobblestone = rule(block_match("minecraft:stone"), "minecraft:cobblestone", &[]);

    let place = |processors: &[StructureProcessorSpec]| {
        let mut writer = fixture.writer();
        place_piece(
            &semantics,
            &template,
            &piece_settings(
                ORIGIN,
                Rotation::Clockwise90,
                PieceElement::LegacySingle,
                processors,
                &owner,
            ),
            &mut writer,
            Some(&mut LegacyRandom::new(0)),
        )
        .expect("the piece places");
        writer
            .state_at([100, 64, 200])
            .map(|state| fixture.render(state))
    };

    // First match wins per processor, and the list is applied in order: the
    // stair is stone for the second rule to see only when that rule comes last.
    assert_eq!(
        place(&[stairs_to_stone.clone(), stone_to_cobblestone.clone()]),
        Some("minecraft:cobblestone".to_owned())
    );
    assert_eq!(
        place(&[stone_to_cobblestone, stairs_to_stone]),
        Some("minecraft:stone".to_owned())
    );
}

#[test]
fn legacy_elements_ignore_air_and_single_elements_do_not() {
    let fixture = Fixture::new();
    let template = synthetic_piece(&fixture.blocks);
    let semantics = fixture.semantics();
    let owner = identifier("minecraft:village/desert/town_centers");
    // The zombie village lists turn doors into air; that is the only way a piece
    // state can become air (the template loader never yields air blocks).
    let doors_to_air = [rule(
        block_match("minecraft:oak_door"),
        "minecraft:air",
        &[],
    )];

    let place = |element: PieceElement| {
        let mut writer = fixture.writer();
        let written = place_piece(
            &semantics,
            &template,
            &piece_settings(
                ORIGIN,
                Rotation::Clockwise90,
                element,
                &doors_to_air,
                &owner,
            ),
            &mut writer,
            Some(&mut LegacyRandom::new(0)),
        )
        .expect("the piece places");
        (written, writer.state_at([99, 64, 203]))
    };

    let (single_written, single_door) = place(PieceElement::Single);
    assert_eq!(single_written, 6);
    assert_eq!(
        single_door.map(|state| fixture.render(state)),
        Some("minecraft:air".to_owned()),
        "single_pool_element writes the air the rule produced"
    );

    let (legacy_written, legacy_door) = place(PieceElement::LegacySingle);
    assert_eq!(legacy_written, 5);
    assert_eq!(
        legacy_door, None,
        "legacy_single_pool_element's STRUCTURE_AND_AIR ignore drops it"
    );
}

#[test]
fn a_template_without_blocks_writes_nothing() {
    let fixture = Fixture::new();
    let root = compound(vec![
        ("size", triplet([2, 2, 2])),
        (
            "palette",
            list(
                tag_type::COMPOUND,
                vec![palette_entry("minecraft:stone", &[])],
            ),
        ),
        ("blocks", list(tag_type::COMPOUND, Vec::new())),
    ]);
    let template = load_template(&fixture.blocks, root);
    let semantics = fixture.semantics();
    let processors: Vec<StructureProcessorSpec> = Vec::new();
    let owner = identifier("minecraft:village/desert/town_centers");
    let mut writer = fixture.writer();
    let written = place_piece(
        &semantics,
        &template,
        &piece_settings(
            ORIGIN,
            Rotation::None,
            PieceElement::LegacySingle,
            &processors,
            &owner,
        ),
        &mut writer,
        Some(&mut LegacyRandom::new(0)),
    )
    .expect("an empty template places");
    assert_eq!(written, 0);
    assert!(writer.blocks.is_empty());
}

// ------------------------------------------------------------ real cache

/// `minecraft:village/desert/town_centers`' first element, from the real cache:
/// its piece, its processor list and its projection, exactly as the pool JSON
/// and the closure read them.
struct RealElement {
    template: StructureTemplate,
    processors: Vec<StructureProcessorSpec>,
    projection: Projection,
    element: PieceElement,
    owner: Identifier,
}

fn real_element(cache: &Path, blocks: &BlockRegistry, pool: &str, index: usize) -> RealElement {
    let loader = VillageDataLoader::new(cache.join("data").join("minecraft").join("worldgen"));
    let pool = loader
        .load_template_pool(&identifier(pool))
        .expect("the desert town centre pool loads");
    let (projection, element, location, processors) = match &pool.elements[index].element {
        PoolElementSpec::LegacySingle(element) => (
            element.projection,
            PieceElement::LegacySingle,
            element.location.clone(),
            match &element.processors {
                ProcessorRef::Inline(specs) => specs.clone(),
                ProcessorRef::List(id) => {
                    loader
                        .load_processor_list(id)
                        .expect("the named processor list loads")
                        .processors
                }
            },
        ),
        other => panic!("the desert town centre is legacy single, not {other:?}"),
    };
    let path = cache
        .join("data")
        .join("minecraft")
        .join("structure")
        .join(location.path())
        .with_extension("nbt");
    let template =
        StructureTemplate::from_nbt_file(&path, blocks).expect("the town centre template loads");
    RealElement {
        template,
        processors,
        projection,
        element,
        owner: location,
    }
}

#[test]
fn real_cache_places_the_desert_town_centre() {
    let Some(cache) = content_cache() else {
        println!(
            "SKIP real_cache_places_the_desert_town_centre: no vanilla content cache at \
             /tmp/jdk-cold2 or $SOLARIS_CONTENT_CACHE"
        );
        return;
    };
    let fixture = Fixture::new();
    let element = real_element(
        &cache,
        &fixture.blocks,
        "minecraft:village/desert/town_centers",
        0,
    );
    assert_eq!(
        element.owner.as_str(),
        "minecraft:village/desert/town_centers/desert_meeting_point_1"
    );
    assert_eq!(element.projection, Projection::Rigid);
    assert!(
        element.processors.is_empty(),
        "the element has no processor list"
    );
    assert_eq!(element.element, PieceElement::LegacySingle);

    let semantics = fixture.semantics();
    let position = BlockPos {
        x: 128,
        y: 64,
        z: -256,
    };
    let mut settings = piece_settings(
        position,
        Rotation::Clockwise90,
        element.element,
        &element.processors,
        &element.owner,
    );

    let mut writer = fixture.writer();
    let written = place_piece(
        &semantics,
        &element.template,
        &settings,
        &mut writer,
        Some(&mut LegacyRandom::new(0)),
    )
    .expect("the town centre places");
    assert_eq!(written, REAL_PLACEMENT_COUNT);
    for (position, state) in REAL_PLACEMENT {
        assert_eq!(
            writer.state_at(position).map(|state| fixture.render(state)),
            Some(state.to_owned()),
            "at {position:?}"
        );
    }
    for position in REAL_VOID_JIGSAWS {
        assert_eq!(
            writer.state_at(position),
            None,
            "a structure_void jigsaw leaves no block at {position:?}"
        );
    }

    // The same piece, twice: placement is deterministic and draws nothing.
    let mut repeat = fixture.writer();
    let again = place_piece(
        &semantics,
        &element.template,
        &settings,
        &mut repeat,
        Some(&mut LegacyRandom::new(0)),
    )
    .expect("the town centre places again");
    assert_eq!(again, written);
    assert_eq!(repeat.blocks, writer.blocks);

    // The chunk-box clip `SinglePoolElement.place` passes: `z <= -246`.
    settings.clip = Some(BlockClip::new([120, -64, -256], [143, 319, -246]));
    let mut clipped = fixture.writer();
    let clipped_written = place_piece(
        &semantics,
        &element.template,
        &settings,
        &mut clipped,
        Some(&mut LegacyRandom::new(0)),
    )
    .expect("the clipped town centre places");
    assert_eq!(clipped_written, REAL_CLIPPED_COUNT);
    for (position, state) in REAL_CLIPPED {
        assert_eq!(
            clipped.state_at(position),
            None,
            "{position:?} ({state}) is outside the clip"
        );
    }
    // The clip removes exactly the blocks outside it: every placement position
    // the box contains is still written, and the positions it removes are the
    // ones the reference listed.
    let clip = settings.clip.expect("the clip is set");
    for (position, state) in REAL_PLACEMENT {
        let inside = clip.contains(BlockPos {
            x: position[0],
            y: position[1],
            z: position[2],
        });
        assert_eq!(
            clipped
                .state_at(position)
                .map(|state| fixture.render(state)),
            inside.then(|| state.to_owned()),
            "{position:?} is {} the clip",
            if inside { "inside" } else { "outside" },
        );
    }
}
