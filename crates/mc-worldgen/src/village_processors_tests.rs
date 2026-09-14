//! Reference-pinned tests for the village structure-processor layer.
//!
//! Every expected value in this file is a derived number, not a transcribed
//! file: `net.minecraft.core.VpRef` — a scratch program kept in `/tmp` — drove
//! the real `RuleProcessor`, `BlockAgeProcessor`, `GravityProcessor`,
//! `JigsawReplacementProcessor`, `ProtectedBlockProcessor` and
//! `BlockIgnoreProcessor` classes from the bundled 26.1.2 server jar over the
//! same fixed block lists through the real
//! `StructureTemplate.processBlockInfos` application surface, and printed the
//! outcome table the constants below hold. The block states and tags here are
//! synthetic Solaris fixtures built on the same vanilla ids; only the outcomes
//! are vanilla's.
//!
//! The lists are the reachable village processor lists: `street_plains` (four
//! rules, first-match wins), `zombie_plains` (tag, block and three sequential
//! random rules at one position) and the `mossify`-style age pass, plus the
//! `terrain_matching` gravity step, the jigsaw replacement, the protected-block
//! filter and the block-ignore filters the piece settings add.
//!
//! The twelve sweep positions are `(120 + 13i, 64 + i % 3, -300 - 7i)` and the
//! piece reference position is `(128, 64, -256)`; the reference used exactly
//! these, because the whole layer's randomness is seeded from the position.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use mc_data::Identifier;
use mc_data::vanilla_feature_closure::BlockStateSpec;
use mc_data::village_data::{
    Axis, HeightmapType, PosRuleTestSpec, ProcessorRuleSpec, Projection, RuleTestSpec,
    StructureProcessorSpec,
};
use mc_world::{BlockPos, BlockRegistry, BlockStateId};

use crate::vanilla_features::{BlockSemantics, BlockTagIndex, CompileError};
use crate::village::processors::{
    PieceBlock, PieceElement, ProcessLevel, Processor, apply_processors, compile_processors,
    get_seed, piece_processors,
};

// ------------------------------------------------------- reference fixtures

/// `Mth.getSeed` as the reference printed it: (x, y, z, seed).
const SEEDS: [(i32, i32, i32, i64); 18] = [
    (120, 64, -300, -105487874808371),
    (133, 65, -307, 83979695796551),
    (146, 66, -314, 10404694437070),
    (159, 64, -321, -73694119953663),
    (172, 65, -328, -82007882453972),
    (185, 66, -335, -123807513972512),
    (198, 64, -342, 49958067905537),
    (211, 65, -349, -32317793188288),
    (224, 66, -356, 30843310256821),
    (237, 64, -363, -113225464858331),
    (250, 65, -370, -30568766582856),
    (263, 66, -377, -5061840365712),
    (1234567, 70, -987654321, -124002568974199),
    (-1234567, -70, 987654321, 41294629579641),
    (1000000, 5, -1000000, 83182354596077),
    (2147483647, 2147483647, 2147483647, 9811763908338),
    (-2147483648, -2147483648, -2147483648, 28509996875776),
    (0, 0, 0, 0),
];

/// Java reference case `street_plains`.
const STREET_PLAINS: [&str; 12] = [
    "minecraft:oak_planks",
    "minecraft:grass_block[snowy=false]",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
];

/// Java reference case `street_plains_swapped`.
const STREET_PLAINS_SWAPPED: [&str; 12] = [
    "minecraft:oak_planks",
    "minecraft:grass_block[snowy=false]",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:grass_block[snowy=false]",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
];

/// Java reference case `street_plains_dry`.
const STREET_PLAINS_DRY: [&str; 12] = [
    "minecraft:dirt_path",
    "minecraft:grass_block[snowy=false]",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:grass_block[snowy=false]",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
];

/// Java reference case `street_plains_grass`.
const STREET_PLAINS_GRASS: [&str; 12] = [
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
    "minecraft:water[level=0]",
    "minecraft:grass_block[snowy=false]",
];

/// Java reference case `street_plains_dirt`.
const STREET_PLAINS_DIRT: [&str; 12] = [
    "minecraft:water[level=0]",
    "minecraft:dirt",
    "minecraft:water[level=0]",
    "minecraft:dirt",
    "minecraft:water[level=0]",
    "minecraft:dirt",
    "minecraft:water[level=0]",
    "minecraft:dirt",
    "minecraft:water[level=0]",
    "minecraft:dirt",
    "minecraft:water[level=0]",
    "minecraft:dirt",
];

/// Java reference case `street_plains_stone`.
const STREET_PLAINS_STONE: [&str; 12] = [
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
    "minecraft:stone",
];

/// Java reference case `zombie_plains_cobble`.
const ZOMBIE_PLAINS_COBBLE: [&str; 12] = [
    "minecraft:cobblestone",
    "minecraft:mossy_cobblestone",
    "minecraft:mossy_cobblestone",
    "minecraft:cobblestone",
    "minecraft:mossy_cobblestone",
    "minecraft:mossy_cobblestone",
    "minecraft:mossy_cobblestone",
    "minecraft:mossy_cobblestone",
    "minecraft:cobblestone",
    "minecraft:mossy_cobblestone",
    "minecraft:cobblestone",
    "minecraft:mossy_cobblestone",
];

/// Java reference case `zombie_plains_wheat`.
const ZOMBIE_PLAINS_WHEAT: [&str; 12] = [
    "minecraft:wheat[age=0]",
    "minecraft:carrots[age=0]",
    "minecraft:potatoes[age=0]",
    "minecraft:wheat[age=0]",
    "minecraft:carrots[age=0]",
    "minecraft:potatoes[age=0]",
    "minecraft:carrots[age=0]",
    "minecraft:wheat[age=0]",
    "minecraft:wheat[age=0]",
    "minecraft:wheat[age=0]",
    "minecraft:wheat[age=0]",
    "minecraft:wheat[age=0]",
];

/// Java reference case `zombie_plains_door`.
const ZOMBIE_PLAINS_DOOR: [&str; 12] = [
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
];

/// Java reference case `zombie_plains_torch`.
const ZOMBIE_PLAINS_TORCH: [&str; 12] = [
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
    "minecraft:air",
];

/// Java reference case `pane_north_south`.
const PANE_NORTH_SOUTH: [&str; 12] = [
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
    "minecraft:brown_stained_glass_pane[east=false,north=true,south=true,waterlogged=false,west=false]",
];

/// Java reference case `pane_east_west`.
const PANE_EAST_WEST: [&str; 12] = [
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
    "minecraft:brown_stained_glass_pane[east=true,north=false,south=false,waterlogged=false,west=true]",
];

/// Java reference case `pane_plain`.
const PANE_PLAIN: [&str; 12] = [
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
    "minecraft:glass_pane[east=false,north=false,south=false,waterlogged=false,west=false]",
];

/// Java reference case `shortcircuit_nodraw`.
const SHORTCIRCUIT_NODRAW: [&str; 12] = [
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
];

/// Java reference case `shortcircuit_onedraw`.
const SHORTCIRCUIT_ONEDRAW: [&str; 12] = [
    "minecraft:stone_bricks",
    "minecraft:stone_bricks",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
];

/// Java reference case `shortcircuit_onepredicate`.
const SHORTCIRCUIT_ONEPREDICATE: [&str; 12] = [
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:stone_bricks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
];

/// Java reference case `pos_linear`.
const POS_LINEAR: [&str; 12] = [
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
];

/// Java reference case `pos_axis_y`.
const POS_AXIS_Y: [&str; 12] = [
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
];

/// Java reference case `pos_axis_x`.
const POS_AXIS_X: [&str; 12] = [
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:dirt_path",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
    "minecraft:oak_planks",
];

/// Java reference case `block_age_05`.
const BLOCK_AGE_05: [&str; 48] = [
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_bricks",
    "minecraft:chiseled_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:crying_obsidian",
    "minecraft:oak_planks",
    "minecraft:stone_bricks",
    "minecraft:stone",
    "minecraft:chiseled_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:stone_brick_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:stone",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:stone_bricks",
    "minecraft:stone_brick_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:cracked_stone_bricks",
    "minecraft:stone",
    "minecraft:chiseled_stone_bricks",
    "minecraft:mossy_stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_bricks",
    "minecraft:chiseled_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
];

/// Java reference case `block_age_09`.
const BLOCK_AGE_09: [&str; 48] = [
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_bricks",
    "minecraft:chiseled_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:crying_obsidian",
    "minecraft:oak_planks",
    "minecraft:stone_bricks",
    "minecraft:stone",
    "minecraft:chiseled_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:mossy_stone_brick_stairs[facing=east,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:stone",
    "minecraft:mossy_stone_brick_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_brick_stairs[facing=west,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:mossy_stone_bricks",
    "minecraft:stone",
    "minecraft:chiseled_stone_bricks",
    "minecraft:mossy_stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_bricks",
    "minecraft:chiseled_stone_bricks",
    "minecraft:stone_brick_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_wall[east=none,north=none,south=none,up=true,waterlogged=false,west=none]",
    "minecraft:obsidian",
    "minecraft:oak_planks",
];

/// Java reference case `block_age_shaped`.
const BLOCK_AGE_SHAPED: [&str; 48] = [
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:stone_slab[type=top,waterlogged=true]",
    "minecraft:stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:stone_bricks",
    "minecraft:stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:stone_slab[type=top,waterlogged=true]",
    "minecraft:stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:stone_bricks",
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:stone_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:cracked_stone_bricks",
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:stone_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:cracked_stone_bricks",
    "minecraft:stone_slab[type=bottom,waterlogged=false]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:cracked_stone_bricks",
    "minecraft:mossy_stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:stone_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:mossy_stone_bricks",
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:mossy_stone_bricks",
];

/// Java reference case `block_age_shaped_09`.
const BLOCK_AGE_SHAPED_09: [&str; 48] = [
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:stone_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:stone_bricks",
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:stone_bricks",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:mossy_stone_bricks",
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:stone_slab[type=bottom,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:mossy_stone_bricks",
    "minecraft:mossy_stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:mossy_stone_bricks",
    "minecraft:mossy_stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:stone_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:mossy_stone_bricks",
    "minecraft:stone_brick_stairs[facing=east,half=top,shape=outer_left,waterlogged=true]",
    "minecraft:oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
    "minecraft:mossy_stone_brick_slab[type=top,waterlogged=true]",
    "minecraft:mossy_stone_brick_wall[east=tall,north=low,south=none,up=false,waterlogged=true,west=low]",
    "minecraft:mossy_stone_brick_slab[type=double,waterlogged=true]",
    "minecraft:mossy_stone_bricks",
];

/// Java reference case `gravity`: the resulting `y` for each sweep position.
const GRAVITY_Y: [i32; 12] = [127, 131, 130, 131, 130, 129, 130, 129, 133, 129, 128, 132];

// ---------------------------------------------------------------- fixtures

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

/// The processor list every test resolves states against.
fn owner() -> Identifier {
    identifier("minecraft:village/plains/houses")
}

/// The reference position the reference run placed every piece with.
const REF: BlockPos = BlockPos {
    x: 128,
    y: 64,
    z: -256,
};

/// The sweep positions, identical to the reference run's: randomness is seeded
/// from them, so they are part of the pin.
fn positions(count: usize) -> Vec<BlockPos> {
    (0..count)
        .map(|index| {
            let index = index as i32;
            BlockPos {
                x: 120 + index * 13,
                y: 64 + index % 3,
                z: -300 - index * 7,
            }
        })
        .collect()
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
    let state = blocks.by_id(state).expect("processed states resolve");
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

/// The state spec a processor list writes.
fn spec(name: &str, properties: &[(&str, &str)]) -> BlockStateSpec {
    BlockStateSpec {
        block: identifier(name),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

/// Synthetic tag index: the ids are vanilla's, the contents are ours and are
/// the vanilla memberships of the blocks these tests use.
#[derive(Default)]
struct TestTags {
    tags: BTreeMap<Identifier, BTreeSet<Identifier>>,
}

impl TestTags {
    fn new(tags: &[(&str, &[&str])]) -> Self {
        Self {
            tags: tags
                .iter()
                .map(|(tag, blocks)| {
                    (
                        identifier(tag),
                        blocks.iter().map(|block| identifier(block)).collect(),
                    )
                })
                .collect(),
        }
    }

    /// Every tag the processor layer consults, with the vanilla blocks the
    /// fixtures use.
    fn vanilla_surface() -> Self {
        Self::new(&[
            (
                "minecraft:stairs",
                &[
                    "minecraft:stone_brick_stairs",
                    "minecraft:mossy_stone_brick_stairs",
                    "minecraft:oak_stairs",
                ],
            ),
            (
                "minecraft:slabs",
                &[
                    "minecraft:stone_slab",
                    "minecraft:stone_brick_slab",
                    "minecraft:mossy_stone_brick_slab",
                ],
            ),
            ("minecraft:walls", &["minecraft:stone_brick_wall"]),
            ("minecraft:doors", &["minecraft:oak_door"]),
            (
                "minecraft:features_cannot_replace",
                &["minecraft:bedrock", "minecraft:spawner", "minecraft:chest"],
            ),
        ])
    }
}

impl BlockTagIndex for TestTags {
    fn block_in_tag(&self, tag: &Identifier, block: &Identifier) -> bool {
        self.tags
            .get(tag)
            .is_some_and(|blocks| blocks.contains(block))
    }
}

/// In-memory level: the `loc` map, air everywhere else, and the reference run's
/// stand-in heightmap `64 + floorMod(x, 5)`.
struct ProbeLevel {
    air: BlockStateId,
    loc: HashMap<BlockPos, BlockStateId>,
}

impl ProcessLevel for ProbeLevel {
    fn block_state(&self, pos: BlockPos) -> BlockStateId {
        self.loc.get(&pos).copied().unwrap_or(self.air)
    }

    fn height(&self, _heightmap: HeightmapType, x: i32, _z: i32) -> i32 {
        64 + x.rem_euclid(5)
    }
}

struct Fixture {
    blocks: BlockRegistry,
    tags: TestTags,
    level: ProbeLevel,
}

impl Fixture {
    fn new() -> Self {
        let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("the embedded required-block report is well formed");
        let air = state_of(&blocks, "minecraft:air", &[]);
        Self {
            blocks,
            tags: TestTags::vanilla_surface(),
            level: ProbeLevel {
                air,
                loc: HashMap::new(),
            },
        }
    }

    fn semantics(&self) -> BlockSemantics<'_> {
        BlockSemantics::new(&self.blocks, &self.tags)
    }

    fn state(&self, name: &str, properties: &[(&str, &str)]) -> BlockStateId {
        state_of(&self.blocks, name, properties)
    }

    fn set_loc(&mut self, pos: BlockPos, state: BlockStateId) {
        self.level.loc.insert(pos, state);
    }

    fn render(&self, state: BlockStateId) -> String {
        render(&self.blocks, state)
    }
}

/// The pipeline the reference ran every rule case through:
/// `SinglePoolElement.getSettings` without the projection processors.
fn village_processors(fixture: &Fixture, specs: &[StructureProcessorSpec]) -> Vec<Processor> {
    let semantics = fixture.semantics();
    piece_processors(
        &semantics,
        &owner(),
        specs,
        Projection::Rigid,
        false,
        PieceElement::Single,
    )
    .expect("the fixture block states resolve")
}

fn run(fixture: &Fixture, processors: &[Processor], blocks: &[PieceBlock]) -> Vec<PieceBlock> {
    let semantics = fixture.semantics();
    apply_processors(&semantics, &fixture.level, blocks, processors, REF)
}

fn outcomes(fixture: &Fixture, processors: &[Processor], blocks: &[PieceBlock]) -> Vec<String> {
    run(fixture, processors, blocks)
        .into_iter()
        .map(|block| fixture.render(block.state))
        .collect()
}

fn assert_outcomes(case: &str, got: &[String], expected: &[&str]) {
    assert_eq!(got.len(), expected.len(), "{case}: surviving block count");
    for (index, (got, expected)) in got.iter().zip(expected).enumerate() {
        assert_eq!(got, expected, "{case}: position {index}");
    }
}

/// One block per sweep position, with `jigsaw_final_state` left unset.
fn pieces(count: usize, state: impl Fn(usize) -> BlockStateId) -> Vec<PieceBlock> {
    positions(count)
        .into_iter()
        .enumerate()
        .map(|(index, pos)| PieceBlock {
            pos,
            // The reference runner passes each sweep position as both the
            // template-local and the world position (`processBlockInfos` with
            // `BlockPos.ZERO`), so the two readings coincide there.
            template_y: pos.y,
            state: state(index),
            jigsaw_final_state: None,
        })
        .collect()
}

fn always_true() -> PosRuleTestSpec {
    PosRuleTestSpec::AlwaysTrue
}

/// `street_plains`: rule 1 (dirt path over water) must win whenever it can,
/// before rule 2's random grass rule.
fn street_plains_rules(swapped: bool) -> Vec<StructureProcessorSpec> {
    let water_path = ProcessorRuleSpec {
        input_predicate: RuleTestSpec::BlockMatch {
            block: identifier("minecraft:dirt_path"),
        },
        location_predicate: RuleTestSpec::BlockMatch {
            block: identifier("minecraft:water"),
        },
        position_predicate: always_true(),
        output_state: spec("minecraft:oak_planks", &[]),
    };
    let path_to_grass = ProcessorRuleSpec {
        input_predicate: RuleTestSpec::RandomBlockMatch {
            block: identifier("minecraft:dirt_path"),
            probability: 0.1,
        },
        location_predicate: RuleTestSpec::AlwaysTrue,
        position_predicate: always_true(),
        output_state: spec("minecraft:grass_block", &[("snowy", "false")]),
    };
    let grass_to_water = ProcessorRuleSpec {
        input_predicate: RuleTestSpec::BlockMatch {
            block: identifier("minecraft:grass_block"),
        },
        location_predicate: RuleTestSpec::BlockMatch {
            block: identifier("minecraft:water"),
        },
        position_predicate: always_true(),
        output_state: spec("minecraft:water", &[("level", "0")]),
    };
    let dirt_to_water = ProcessorRuleSpec {
        input_predicate: RuleTestSpec::BlockMatch {
            block: identifier("minecraft:dirt"),
        },
        location_predicate: RuleTestSpec::BlockMatch {
            block: identifier("minecraft:water"),
        },
        position_predicate: always_true(),
        output_state: spec("minecraft:water", &[("level", "0")]),
    };
    let mut rules = if swapped {
        vec![path_to_grass, water_path]
    } else {
        vec![water_path, path_to_grass]
    };
    rules.push(grass_to_water);
    rules.push(dirt_to_water);
    vec![StructureProcessorSpec::Rule { rules }]
}

/// `zombie_plains`: a tag rule, a block rule, and three random rules that draw
/// in sequence at one position.
fn zombie_plains_processors() -> Vec<StructureProcessorSpec> {
    let rule = |input: RuleTestSpec, output: BlockStateSpec| ProcessorRuleSpec {
        input_predicate: input,
        location_predicate: RuleTestSpec::AlwaysTrue,
        position_predicate: always_true(),
        output_state: output,
    };
    vec![StructureProcessorSpec::Rule {
        rules: vec![
            rule(
                RuleTestSpec::RandomBlockMatch {
                    block: identifier("minecraft:cobblestone"),
                    probability: 0.8,
                },
                spec("minecraft:mossy_cobblestone", &[]),
            ),
            rule(
                RuleTestSpec::TagMatch {
                    tag: identifier("minecraft:doors"),
                },
                spec("minecraft:air", &[]),
            ),
            rule(
                RuleTestSpec::BlockMatch {
                    block: identifier("minecraft:torch"),
                },
                spec("minecraft:air", &[]),
            ),
            rule(
                RuleTestSpec::RandomBlockMatch {
                    block: identifier("minecraft:wheat"),
                    probability: 0.3,
                },
                spec("minecraft:carrots", &[("age", "0")]),
            ),
            rule(
                RuleTestSpec::RandomBlockMatch {
                    block: identifier("minecraft:wheat"),
                    probability: 0.2,
                },
                spec("minecraft:potatoes", &[("age", "0")]),
            ),
            rule(
                RuleTestSpec::RandomBlockMatch {
                    block: identifier("minecraft:wheat"),
                    probability: 0.1,
                },
                spec("minecraft:beetroots", &[("age", "0")]),
            ),
        ],
    }]
}

/// The two glass-pane rules of `zombie_plains`, plus a rule list that only
/// matches the exact two states the reference used.
fn pane_processors() -> Vec<StructureProcessorSpec> {
    let north_south = |name: &str| {
        spec(
            name,
            &[
                ("east", "false"),
                ("north", "true"),
                ("south", "true"),
                ("waterlogged", "false"),
                ("west", "false"),
            ],
        )
    };
    let east_west = |name: &str| {
        spec(
            name,
            &[
                ("east", "true"),
                ("north", "false"),
                ("south", "false"),
                ("waterlogged", "false"),
                ("west", "true"),
            ],
        )
    };
    vec![StructureProcessorSpec::Rule {
        rules: vec![
            ProcessorRuleSpec {
                input_predicate: RuleTestSpec::BlockStateMatch {
                    state: north_south("minecraft:glass_pane"),
                },
                location_predicate: RuleTestSpec::AlwaysTrue,
                position_predicate: always_true(),
                output_state: north_south("minecraft:brown_stained_glass_pane"),
            },
            ProcessorRuleSpec {
                input_predicate: RuleTestSpec::BlockStateMatch {
                    state: east_west("minecraft:glass_pane"),
                },
                location_predicate: RuleTestSpec::AlwaysTrue,
                position_predicate: always_true(),
                output_state: east_west("minecraft:brown_stained_glass_pane"),
            },
        ],
    }]
}

// ------------------------------------------------------------------- tests

#[test]
fn get_seed_matches_the_vanilla_reference() {
    for (x, y, z, seed) in SEEDS {
        assert_eq!(
            get_seed(BlockPos { x, y, z }),
            seed,
            "Mth.getSeed({x}, {y}, {z})"
        );
    }
}

#[test]
fn street_plains_rules_follow_the_vanilla_reference() {
    let mut fixture = Fixture::new();
    let water = fixture.state("minecraft:water", &[]);
    let stone = fixture.state("minecraft:stone", &[]);
    for (index, pos) in positions(12).into_iter().enumerate() {
        fixture.set_loc(pos, if index % 2 == 0 { water } else { stone });
    }
    let blocks = pieces(12, |_| fixture.state("minecraft:dirt_path", &[]));
    let processors = village_processors(&fixture, &street_plains_rules(false));
    let got = outcomes(&fixture, &processors, &blocks);
    assert_outcomes("street_plains", &got, &STREET_PLAINS);
}

#[test]
fn rule_list_order_decides_the_outcome() {
    let mut fixture = Fixture::new();
    let water = fixture.state("minecraft:water", &[]);
    let stone = fixture.state("minecraft:stone", &[]);
    for (index, pos) in positions(12).into_iter().enumerate() {
        fixture.set_loc(pos, if index % 2 == 0 { water } else { stone });
    }
    let blocks = pieces(12, |_| fixture.state("minecraft:dirt_path", &[]));

    let ordered = outcomes(
        &fixture,
        &village_processors(&fixture, &street_plains_rules(false)),
        &blocks,
    );
    let swapped = outcomes(
        &fixture,
        &village_processors(&fixture, &street_plains_rules(true)),
        &blocks,
    );
    assert_outcomes("street_plains", &ordered, &STREET_PLAINS);
    assert_outcomes("street_plains_swapped", &swapped, &STREET_PLAINS_SWAPPED);
    // At position 6 the locState is water, so both the water rule and the
    // random grass rule match and only the list order decides which one wins.
    assert_eq!(ordered[6], "minecraft:oak_planks");
    assert_eq!(swapped[6], "minecraft:grass_block[snowy=false]");
}

#[test]
fn rule_location_matches_follow_the_vanilla_reference() {
    let mut fixture = Fixture::new();
    let water = fixture.state("minecraft:water", &[]);
    let air = fixture.state("minecraft:air", &[]);
    for (index, pos) in positions(12).into_iter().enumerate() {
        fixture.set_loc(pos, if index % 2 == 0 { water } else { air });
    }
    let processors = village_processors(&fixture, &street_plains_rules(false));

    let grass = pieces(12, |_| fixture.state("minecraft:grass_block", &[]));
    assert_outcomes(
        "street_plains_grass",
        &outcomes(&fixture, &processors, &grass),
        &STREET_PLAINS_GRASS,
    );
    let dirt = pieces(12, |_| fixture.state("minecraft:dirt", &[]));
    assert_outcomes(
        "street_plains_dirt",
        &outcomes(&fixture, &processors, &dirt),
        &STREET_PLAINS_DIRT,
    );
    let stone = pieces(12, |_| fixture.state("minecraft:stone", &[]));
    assert_outcomes(
        "street_plains_stone",
        &outcomes(&fixture, &processors, &stone),
        &STREET_PLAINS_STONE,
    );
}

#[test]
fn rule_randomness_is_seeded_per_position() {
    let mut fixture = Fixture::new();
    let stone = fixture.state("minecraft:stone", &[]);
    for pos in positions(12) {
        fixture.set_loc(pos, stone);
    }
    let blocks = pieces(12, |_| fixture.state("minecraft:dirt_path", &[]));
    let processors = village_processors(&fixture, &street_plains_rules(false));
    let got = outcomes(&fixture, &processors, &blocks);
    assert_outcomes("street_plains_dry", &got, &STREET_PLAINS_DRY);

    // Every position enters with the same block state and the same locState, so
    // only the per-position seed is left: the reference has positions 1 and 6
    // turn into grass while the other ten stay dirt paths.
    assert_eq!(got[1], "minecraft:grass_block[snowy=false]");
    assert_eq!(got[6], "minecraft:grass_block[snowy=false]");
    assert_eq!(got[3], "minecraft:dirt_path");
    let sweep = positions(12);
    assert_ne!(get_seed(sweep[1]), get_seed(sweep[6]));
}

#[test]
fn zombie_plains_rules_follow_the_vanilla_reference() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(12) {
        fixture.set_loc(pos, air);
    }
    let processors = village_processors(&fixture, &zombie_plains_processors());

    let cobble = pieces(12, |_| fixture.state("minecraft:cobblestone", &[]));
    assert_outcomes(
        "zombie_plains_cobble",
        &outcomes(&fixture, &processors, &cobble),
        &ZOMBIE_PLAINS_COBBLE,
    );
    // Three random rules draw in sequence at one position: the first match wins,
    // and a rule that fails has still consumed its draw.
    let wheat = pieces(12, |_| fixture.state("minecraft:wheat", &[]));
    assert_outcomes(
        "zombie_plains_wheat",
        &outcomes(&fixture, &processors, &wheat),
        &ZOMBIE_PLAINS_WHEAT,
    );
    let door = pieces(12, |_| fixture.state("minecraft:oak_door", &[]));
    assert_outcomes(
        "zombie_plains_door",
        &outcomes(&fixture, &processors, &door),
        &ZOMBIE_PLAINS_DOOR,
    );
    let torch = pieces(12, |_| fixture.state("minecraft:torch", &[]));
    assert_outcomes(
        "zombie_plains_torch",
        &outcomes(&fixture, &processors, &torch),
        &ZOMBIE_PLAINS_TORCH,
    );
}

#[test]
fn blockstate_match_needs_the_exact_state() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(12) {
        fixture.set_loc(pos, air);
    }
    let processors = village_processors(&fixture, &pane_processors());

    let north_south = pieces(12, |_| {
        fixture.state(
            "minecraft:glass_pane",
            &[("north", "true"), ("south", "true")],
        )
    });
    assert_outcomes(
        "pane_north_south",
        &outcomes(&fixture, &processors, &north_south),
        &PANE_NORTH_SOUTH,
    );
    let east_west = pieces(12, |_| {
        fixture.state(
            "minecraft:glass_pane",
            &[("east", "true"), ("west", "true")],
        )
    });
    assert_outcomes(
        "pane_east_west",
        &outcomes(&fixture, &processors, &east_west),
        &PANE_EAST_WEST,
    );
    // The default pane state matches neither rule.
    let plain = pieces(12, |_| fixture.state("minecraft:glass_pane", &[]));
    assert_outcomes(
        "pane_plain",
        &outcomes(&fixture, &processors, &plain),
        &PANE_PLAIN,
    );
}

#[test]
fn predicates_short_circuit_and_only_reached_ones_draw() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(12) {
        fixture.set_loc(pos, air);
    }
    let blocks = pieces(12, |_| fixture.state("minecraft:dirt_path", &[]));
    let probe = ProcessorRuleSpec {
        input_predicate: RuleTestSpec::RandomBlockMatch {
            block: identifier("minecraft:dirt_path"),
            probability: 0.5,
        },
        location_predicate: RuleTestSpec::AlwaysTrue,
        position_predicate: always_true(),
        output_state: spec("minecraft:stone_bricks", &[]),
    };

    // Rule 1 fails on its location predicate: the input predicate passed
    // without drawing, so the probe's rule draws the first float.
    let no_draw = vec![StructureProcessorSpec::Rule {
        rules: vec![
            ProcessorRuleSpec {
                input_predicate: RuleTestSpec::BlockMatch {
                    block: identifier("minecraft:dirt_path"),
                },
                location_predicate: RuleTestSpec::BlockMatch {
                    block: identifier("minecraft:obsidian"),
                },
                position_predicate: always_true(),
                output_state: spec("minecraft:stone", &[]),
            },
            probe.clone(),
        ],
    }];
    // Rule 1's input predicate draws (probability 1.0) before its location
    // predicate fails, so the probe's rule draws the second float.
    let one_draw = vec![StructureProcessorSpec::Rule {
        rules: vec![
            ProcessorRuleSpec {
                input_predicate: RuleTestSpec::RandomBlockMatch {
                    block: identifier("minecraft:dirt_path"),
                    probability: 1.0,
                },
                location_predicate: RuleTestSpec::BlockMatch {
                    block: identifier("minecraft:obsidian"),
                },
                position_predicate: always_true(),
                output_state: spec("minecraft:stone", &[]),
            },
            probe,
        ],
    }];
    let one_predicate = vec![StructureProcessorSpec::Rule {
        rules: vec![ProcessorRuleSpec {
            input_predicate: RuleTestSpec::RandomBlockStateMatch {
                state: spec("minecraft:dirt_path", &[]),
                probability: 0.5,
            },
            location_predicate: RuleTestSpec::AlwaysTrue,
            position_predicate: always_true(),
            output_state: spec("minecraft:stone_bricks", &[]),
        }],
    }];

    let no_draw = outcomes(&fixture, &village_processors(&fixture, &no_draw), &blocks);
    let one_draw = outcomes(&fixture, &village_processors(&fixture, &one_draw), &blocks);
    let one_predicate = outcomes(
        &fixture,
        &village_processors(&fixture, &one_predicate),
        &blocks,
    );
    assert_outcomes("shortcircuit_nodraw", &no_draw, &SHORTCIRCUIT_NODRAW);
    assert_outcomes("shortcircuit_onedraw", &one_draw, &SHORTCIRCUIT_ONEDRAW);
    assert_outcomes(
        "shortcircuit_onepredicate",
        &one_predicate,
        &SHORTCIRCUIT_ONEPREDICATE,
    );
    // The two lists differ only in whether rule 1 draws, and that shifts the
    // stream the probe's rule reads: the outcomes diverge at six positions.
    let differing = no_draw
        .iter()
        .zip(&one_draw)
        .filter(|(no_draw, one_draw)| no_draw != one_draw)
        .count();
    assert_eq!(differing, 7);
}

#[test]
fn position_predicates_follow_the_vanilla_reference() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(12) {
        fixture.set_loc(pos, air);
    }
    let blocks = pieces(12, |_| fixture.state("minecraft:dirt_path", &[]));
    let position_rule = |position_predicate: PosRuleTestSpec| {
        vec![StructureProcessorSpec::Rule {
            rules: vec![ProcessorRuleSpec {
                input_predicate: RuleTestSpec::AlwaysTrue,
                location_predicate: RuleTestSpec::AlwaysTrue,
                position_predicate,
                output_state: spec("minecraft:oak_planks", &[]),
            }],
        }]
    };

    let linear = position_rule(PosRuleTestSpec::LinearPos {
        min_chance: 0.1,
        max_chance: 0.9,
        min_dist: 0,
        max_dist: 64,
    });
    assert_outcomes(
        "pos_linear",
        &outcomes(&fixture, &village_processors(&fixture, &linear), &blocks),
        &POS_LINEAR,
    );
    let axis_y = position_rule(PosRuleTestSpec::AxisAlignedLinearPos {
        min_chance: 0.1,
        max_chance: 0.9,
        min_dist: 0,
        max_dist: 64,
        axis: Axis::Y,
    });
    assert_outcomes(
        "pos_axis_y",
        &outcomes(&fixture, &village_processors(&fixture, &axis_y), &blocks),
        &POS_AXIS_Y,
    );
    let axis_x = position_rule(PosRuleTestSpec::AxisAlignedLinearPos {
        min_chance: 0.1,
        max_chance: 0.9,
        min_dist: 0,
        max_dist: 64,
        axis: Axis::X,
    });
    assert_outcomes(
        "pos_axis_x",
        &outcomes(&fixture, &village_processors(&fixture, &axis_x), &blocks),
        &POS_AXIS_X,
    );
}

/// The `mossify`-style palette the reference swept the age pass over.
fn age_palette(fixture: &Fixture) -> [BlockStateId; 8] {
    [
        fixture.state("minecraft:stone_bricks", &[]),
        fixture.state("minecraft:stone", &[]),
        fixture.state("minecraft:chiseled_stone_bricks", &[]),
        fixture.state("minecraft:stone_brick_stairs", &[]),
        fixture.state("minecraft:stone_brick_slab", &[]),
        fixture.state("minecraft:stone_brick_wall", &[]),
        fixture.state("minecraft:obsidian", &[]),
        fixture.state("minecraft:oak_planks", &[]),
    ]
}

#[test]
fn block_age_follows_the_vanilla_reference() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(48) {
        fixture.set_loc(pos, air);
    }
    let palette = age_palette(&fixture);
    let blocks = pieces(48, |index| palette[index % palette.len()]);

    let half = village_processors(
        &fixture,
        &[StructureProcessorSpec::BlockAge { mossiness: 0.5 }],
    );
    assert_outcomes(
        "block_age_05",
        &outcomes(&fixture, &half, &blocks),
        &BLOCK_AGE_05,
    );
    let mostly = village_processors(
        &fixture,
        &[StructureProcessorSpec::BlockAge { mossiness: 0.9 }],
    );
    assert_outcomes(
        "block_age_09",
        &outcomes(&fixture, &mostly, &blocks),
        &BLOCK_AGE_09,
    );
}

#[test]
fn block_age_copies_the_properties_it_replaces() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(48) {
        fixture.set_loc(pos, air);
    }
    // Non-default property variants: the `withPropertiesOf` copies must carry
    // `facing`/`half`/`shape`/`waterlogged` across, and `type`/`waterlogged` or
    // the whole wall side set.
    let palette = [
        fixture.state(
            "minecraft:stone_brick_stairs",
            &[
                ("facing", "east"),
                ("half", "top"),
                ("shape", "outer_left"),
                ("waterlogged", "true"),
            ],
        ),
        fixture.state(
            "minecraft:oak_stairs",
            &[("facing", "west"), ("half", "top")],
        ),
        fixture.state(
            "minecraft:stone_slab",
            &[("type", "top"), ("waterlogged", "true")],
        ),
        fixture.state(
            "minecraft:stone_brick_wall",
            &[
                ("east", "tall"),
                ("north", "low"),
                ("south", "none"),
                ("up", "false"),
                ("waterlogged", "true"),
                ("west", "low"),
            ],
        ),
        fixture.state(
            "minecraft:mossy_stone_brick_slab",
            &[("type", "double"), ("waterlogged", "true")],
        ),
        fixture.state("minecraft:stone_bricks", &[]),
    ];
    let blocks = pieces(48, |index| palette[index % palette.len()]);

    let half = village_processors(
        &fixture,
        &[StructureProcessorSpec::BlockAge { mossiness: 0.5 }],
    );
    assert_outcomes(
        "block_age_shaped",
        &outcomes(&fixture, &half, &blocks),
        &BLOCK_AGE_SHAPED,
    );
    let mostly = village_processors(
        &fixture,
        &[StructureProcessorSpec::BlockAge { mossiness: 0.9 }],
    );
    assert_outcomes(
        "block_age_shaped_09",
        &outcomes(&fixture, &mostly, &blocks),
        &BLOCK_AGE_SHAPED_09,
    );
}

/// The gravity processor adds the block's *template* `y`, not its world `y`.
///
/// Vanilla's `processBlockInfos` passes the template-local `blockInfo` as
/// `originalBlockInfo` and the world position as `processedBlockInfo`
/// (`StructureTemplate.java:452-460` in the bundled 26.1.2 sources), and
/// `GravityProcessor` computes `level.getHeight(heightmap, x, z) + offset +
/// originalBlockInfo.pos().getY()`. A scratch runner outside the repository
/// drove the real classes with the two readings made distinct — one template
/// block at local `y = 0` and one at local `y = 5`, both with world `y = 70` —
/// and printed:
///
/// ```text
/// CASE piece-at-y70 localY=0 -> y=64
/// CASE piece-at-y70 localY=5 -> y=69
/// ```
///
/// i.e. `height(x=120) + local y`, never `+ world y` (which would have been
/// `134`/`139`). Feeding the world `y` instead would fling every
/// `terrain_matching` village block to `height + piece.y`, the piece's own
/// altitude above the terrain it is supposed to follow.
#[test]
fn gravity_uses_the_template_y_not_the_world_y() {
    let fixture = Fixture::new();
    let state = fixture.state("minecraft:oak_planks", &[]);
    let processors = village_processors(
        &fixture,
        &[StructureProcessorSpec::Gravity {
            heightmap: HeightmapType::WorldSurfaceWg,
            offset: 0,
        }],
    );
    // The reference's own position: the probe heightmap is `64 + x mod 5`, so
    // `x = 120` is height 64 and the two readings cannot coincide.
    let world = BlockPos {
        x: 120,
        y: 70,
        z: -300,
    };
    let height = 64;
    let moved = |template_y: i32| {
        let block = PieceBlock {
            pos: world,
            template_y,
            state,
            jigsaw_final_state: None,
        };
        run(&fixture, &processors, &[block])[0].pos.y
    };
    assert_eq!(moved(0), height, "the reference's local-y-0 case");
    assert_eq!(moved(5), height + 5, "the reference's local-y-5 case");
    assert_ne!(moved(5), height + world.y, "the world y must not be added");
    assert_eq!(
        run(
            &fixture,
            &processors,
            &[PieceBlock {
                pos: world,
                template_y: 0,
                state,
                jigsaw_final_state: None,
            }],
        )[0]
        .pos
        .x,
        world.x,
        "x keeps the processed position"
    );
}

#[test]
fn gravity_follows_the_vanilla_reference() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    let sweep = positions(12);
    for pos in &sweep {
        fixture.set_loc(*pos, air);
    }
    let blocks = pieces(12, |_| fixture.state("minecraft:oak_planks", &[]));
    let processors = village_processors(
        &fixture,
        &[StructureProcessorSpec::Gravity {
            heightmap: HeightmapType::WorldSurfaceWg,
            offset: -1,
        }],
    );
    let moved = run(&fixture, &processors, &blocks);
    let got: Vec<i32> = moved.iter().map(|block| block.pos.y).collect();
    assert_eq!(got, GRAVITY_Y.to_vec());
    // `x`/`z` keep the processed position and the state is untouched.
    for (index, block) in moved.iter().enumerate() {
        assert_eq!(block.pos.x, sweep[index].x);
        assert_eq!(block.pos.z, sweep[index].z);
        assert_eq!(fixture.render(block.state), "minecraft:oak_planks");
    }

    // `Projection.TERRAIN_MATCHING` appends exactly that processor, so the same
    // reference numbers come out of the projection's own list.
    let semantics = fixture.semantics();
    let projected = piece_processors(
        &semantics,
        &owner(),
        &[],
        Projection::TerrainMatching,
        false,
        PieceElement::Single,
    )
    .expect("the projection's processor compiles");
    let got: Vec<i32> = run(&fixture, &projected, &blocks)
        .iter()
        .map(|block| block.pos.y)
        .collect();
    assert_eq!(got, GRAVITY_Y.to_vec());
}

#[test]
fn jigsaw_replacement_follows_the_resolved_final_state() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(5) {
        fixture.set_loc(pos, air);
    }
    let jigsaw = fixture.state("minecraft:jigsaw", &[]);
    let planks = fixture.state("minecraft:oak_planks", &[]);
    let structure_void = fixture.state("minecraft:structure_void", &[]);
    let processors = village_processors(&fixture, &[]);

    let cases = [
        (jigsaw, Some(planks)),
        (jigsaw, Some(structure_void)),
        (jigsaw, Some(air)),
        (jigsaw, None),
        (planks, Some(planks)),
    ];
    let expected = [
        vec!["minecraft:oak_planks"],
        vec![],
        vec!["minecraft:air"],
        vec!["minecraft:jigsaw[orientation=north_up]"],
        vec!["minecraft:oak_planks"],
    ];
    for (index, ((state, jigsaw_final_state), expected)) in
        cases.into_iter().zip(expected).enumerate()
    {
        let block = PieceBlock {
            pos: positions(5)[index],
            template_y: positions(5)[index].y,
            state,
            jigsaw_final_state,
        };
        let got = outcomes(&fixture, &processors, &[block]);
        assert_eq!(got, expected, "jigsaw case {index}");
    }

    // `keep_jigsaws` omits the replacement processor entirely.
    let kept = piece_processors(
        &fixture.semantics(),
        &owner(),
        &[],
        Projection::Rigid,
        true,
        PieceElement::Single,
    )
    .expect("an empty processor list compiles");
    let block = PieceBlock {
        pos: positions(5)[0],
        template_y: positions(5)[0].y,
        state: jigsaw,
        jigsaw_final_state: Some(planks),
    };
    assert_eq!(
        outcomes(&fixture, &kept, &[block]),
        vec!["minecraft:jigsaw[orientation=north_up]"]
    );
}

#[test]
fn protected_blocks_read_the_world_state_only() {
    let mut fixture = Fixture::new();
    let bedrock = fixture.state("minecraft:bedrock", &[]);
    let stone = fixture.state("minecraft:stone", &[]);
    fixture.set_loc(positions(12)[0], bedrock);
    fixture.set_loc(positions(12)[1], stone);
    fixture.set_loc(positions(12)[2], stone);
    let processors = village_processors(
        &fixture,
        &[StructureProcessorSpec::ProtectedBlocks {
            cannot_replace: identifier("minecraft:features_cannot_replace"),
        }],
    );

    let planks = fixture.state("minecraft:oak_planks", &[]);
    let blocks = vec![
        PieceBlock {
            pos: positions(12)[0],
            template_y: positions(12)[0].y,
            state: planks,
            jigsaw_final_state: None,
        },
        PieceBlock {
            pos: positions(12)[1],
            template_y: positions(12)[1].y,
            state: planks,
            jigsaw_final_state: None,
        },
    ];
    let got = outcomes(&fixture, &processors, &blocks);
    assert_eq!(got, vec!["minecraft:oak_planks"]);

    // A protected block being *placed* does not protect it: only the state
    // already in the world is consulted.
    let block = PieceBlock {
        pos: positions(12)[2],
        template_y: positions(12)[2].y,
        state: bedrock,
        jigsaw_final_state: None,
    };
    assert_eq!(
        outcomes(&fixture, &processors, &[block]),
        vec!["minecraft:bedrock"]
    );
}

#[test]
fn block_ignore_drops_only_the_listed_blocks() {
    let mut fixture = Fixture::new();
    let air = fixture.state("minecraft:air", &[]);
    for pos in positions(4) {
        fixture.set_loc(pos, air);
    }
    // The reference's list for these cases repeats the structure-block ignore
    // the piece settings add.
    let processors = vec![
        Processor::BlockIgnore {
            blocks: vec![identifier("minecraft:structure_block")],
        },
        Processor::JigsawReplacement,
        Processor::BlockIgnore {
            blocks: vec![identifier("minecraft:structure_block")],
        },
    ];
    let cases = [
        (fixture.state("minecraft:structure_block", &[]), None),
        (fixture.state("minecraft:jigsaw", &[]), None),
        (fixture.state("minecraft:oak_planks", &[]), None),
        (fixture.state("minecraft:structure_void", &[]), None),
    ];
    let expected = [
        vec![],
        vec!["minecraft:jigsaw[orientation=north_up]"],
        vec!["minecraft:oak_planks"],
        vec!["minecraft:structure_void"],
    ];
    for (index, ((state, jigsaw_final_state), expected)) in
        cases.into_iter().zip(expected).enumerate()
    {
        let block = PieceBlock {
            pos: positions(4)[index],
            template_y: positions(4)[index].y,
            state,
            jigsaw_final_state,
        };
        assert_eq!(
            outcomes(&fixture, &processors, &[block]),
            expected,
            "block_ignore case {index}"
        );
    }
}

#[test]
fn legacy_single_pieces_also_ignore_air() {
    let mut fixture = Fixture::new();
    let stone = fixture.state("minecraft:stone", &[]);
    for pos in positions(4) {
        fixture.set_loc(pos, stone);
    }
    let semantics = fixture.semantics();
    let processors = piece_processors(
        &semantics,
        &owner(),
        &[],
        Projection::Rigid,
        false,
        PieceElement::LegacySingle,
    )
    .expect("an empty processor list compiles");
    let cases = [
        fixture.state("minecraft:structure_block", &[]),
        fixture.state("minecraft:air", &[]),
        fixture.state("minecraft:cave_air", &[]),
        fixture.state("minecraft:stone", &[]),
    ];
    let expected = [
        vec![],
        vec![],
        // `BlockIgnoreProcessor` compares blocks, not `isAir()`.
        vec!["minecraft:cave_air"],
        vec!["minecraft:stone"],
    ];
    for (index, (state, expected)) in cases.into_iter().zip(expected).enumerate() {
        let block = PieceBlock {
            pos: positions(4)[index],
            template_y: positions(4)[index].y,
            state,
            jigsaw_final_state: None,
        };
        assert_eq!(
            outcomes(&fixture, &processors, &[block]),
            expected,
            "legacy ignore case {index}"
        );
    }
}

#[test]
fn piece_processors_follow_vanilla_settings_composition() {
    let fixture = Fixture::new();
    let semantics = fixture.semantics();
    let age_spec = StructureProcessorSpec::BlockAge { mossiness: 0.1 };

    let single = piece_processors(
        &semantics,
        &owner(),
        std::slice::from_ref(&age_spec),
        Projection::TerrainMatching,
        false,
        PieceElement::Single,
    )
    .expect("the age states resolve");
    assert_eq!(single.len(), 4);
    assert!(matches!(
        single[0],
        Processor::BlockIgnore { ref blocks } if blocks == &[identifier("minecraft:structure_block")]
    ));
    assert!(matches!(single[1], Processor::JigsawReplacement));
    assert!(matches!(single[2], Processor::BlockAge { .. }));
    assert!(matches!(
        single[3],
        Processor::Gravity {
            heightmap: HeightmapType::WorldSurfaceWg,
            offset: -1
        }
    ));

    // A legacy element pops the structure-block ignore and appends
    // `STRUCTURE_AND_AIR` after the projection's processors; keeping the
    // jigsaws drops the replacement processor.
    let legacy = piece_processors(
        &semantics,
        &owner(),
        &[age_spec],
        Projection::TerrainMatching,
        true,
        PieceElement::LegacySingle,
    )
    .expect("the age states resolve");
    assert_eq!(legacy.len(), 3);
    assert!(matches!(legacy[0], Processor::BlockAge { .. }));
    assert!(matches!(
        legacy[1],
        Processor::Gravity {
            heightmap: HeightmapType::WorldSurfaceWg,
            offset: -1
        }
    ));
    assert!(matches!(
        legacy[2],
        Processor::BlockIgnore { ref blocks }
            if blocks == &[
                identifier("minecraft:air"),
                identifier("minecraft:structure_block"),
            ]
    ));
}

#[test]
fn unresolvable_states_fail_closed_naming_the_owner() {
    let fixture = Fixture::new();
    let semantics = fixture.semantics();
    let specs = vec![StructureProcessorSpec::Rule {
        rules: vec![ProcessorRuleSpec {
            input_predicate: RuleTestSpec::BlockStateMatch {
                state: spec("minecraft:not_a_block", &[]),
            },
            location_predicate: RuleTestSpec::AlwaysTrue,
            position_predicate: always_true(),
            output_state: spec("minecraft:oak_planks", &[]),
        }],
    }];
    match compile_processors(&semantics, &owner(), &specs) {
        Err(CompileError::UnknownBlockState { owner, block }) => {
            assert_eq!(owner, self::owner());
            assert_eq!(block, identifier("minecraft:not_a_block"));
        }

        other => panic!("expected a fail-closed compile error, got {other:?}"),
    }

    // A property value the block does not have fails closed too.
    let specs = vec![StructureProcessorSpec::Rule {
        rules: vec![ProcessorRuleSpec {
            input_predicate: RuleTestSpec::AlwaysTrue,
            location_predicate: RuleTestSpec::AlwaysTrue,
            position_predicate: always_true(),
            output_state: spec("minecraft:grass_block", &[("snowy", "maybe")]),
        }],
    }];
    assert!(matches!(
        compile_processors(&semantics, &owner(), &specs),
        Err(CompileError::UnknownBlockState { .. })
    ));
}
