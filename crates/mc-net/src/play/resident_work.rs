//! Authoritative world adapter for resident physical work, routes and combat.
//!
//! Resident work orders execute against real blocks through the same world
//! storage kernel a player edit uses; routes, line of sight and protected zones
//! come from the same loaded chunks the rest of the server reads. The trait is
//! the seam the order executor depends on: a resident path fails closed
//! (`unloaded`/`unsupported`) when no adapter is installed, and the tests drive
//! the real [`LiveResidentWorld`] over a real world storage.

use std::sync::Arc;

use mc_data::block_light::BlockLightTable;
use mc_data::item_components::ItemFactsTable;
use mc_data::item_stack::ItemStack;
use mc_data::items::ItemRegistry;
use mc_entity::Vec3;
use mc_protocol::codec;
use mc_script::ScriptOperationFailure;
use mc_world::{
    BlockPos, BlockRegistry, BlockStateId, ResidentBlockEdit, ResidentBlockEditBatchResult,
    ResidentBlockPrecondition, WorldReadView,
};

use crate::play::survival::block_drop_stacks_with_tool_and_facts_from_seeded;
use crate::script::PluginZoneAdapter;
use crate::server::WorldHandle;

/// One resident has to be an overworld actor: the work, route and combat
/// adapters all read the region the resident owner simulates.
pub(crate) const RESIDENT_WORLD_DIMENSION: &str = "minecraft:overworld";
/// Longest straight route a formation may walk in one order, in blocks.
const MAX_ROUTE_STEPS: i32 = 128;
/// Longest line-of-sight ray a resident may test, in blocks.
const MAX_SIGHT_STEPS: i32 = 96;

/// One block read: raw state plus the canonical block path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidentBlock {
    pub state: u32,
    pub path: String,
}

/// One canonical drop of a committed block break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidentDrop {
    pub item_id: String,
    pub count: u32,
}

/// Bounded world adapter for resident work, routes and combat.
///
/// Every method answers `None` when the addressed cell is not loaded, so the
/// caller reports the typed `unloaded` reason instead of guessing about terrain.
pub(crate) trait ResidentWorld: Send + Sync {
    /// Whether the adapter simulates this dimension at all.
    fn dimension_loaded(&self, dimension: &str) -> bool;
    /// State and canonical path of one block, or `None` when not loaded.
    fn block(&self, dimension: &str, pos: [i32; 3]) -> Option<ResidentBlock>;
    /// Whether a member can stand with feet at `pos`.
    fn standable(&self, dimension: &str, pos: [i32; 3]) -> Option<bool>;
    /// Whether a bounded straight walk between two standable cells is clear.
    fn route_open(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> Option<bool>;
    /// Whether no opaque block interrupts the segment.
    fn line_of_sight(&self, dimension: &str, from: Vec3, to: Vec3) -> Option<bool>;
    /// Whether a foreign protected zone covers the inclusive rectangle.
    fn foreign_zone_overlaps(
        &self,
        plugin_id: &str,
        dimension: &str,
        min: [i32; 3],
        max: [i32; 3],
    ) -> bool;
    /// Canonical block state id for one block path, when it is registered.
    fn state_for(&self, block_path: &str) -> Option<u32>;
    /// Commit one conditional break and return the canonical loot.
    fn break_block(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<Vec<ResidentDrop>, ScriptOperationFailure>;
    /// Commit one conditional placement of `state` over an air cell.
    fn place_block(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<(), ScriptOperationFailure>;
}

/// The production [`ResidentWorld`] over live world storage.
pub(crate) struct LiveResidentWorld {
    world: WorldHandle,
    read: WorldReadView,
    blocks: Arc<BlockRegistry>,
    light: Option<Arc<BlockLightTable>>,
    zones: Option<PluginZoneAdapter>,
    items: Arc<ItemRegistry>,
    item_facts: Arc<ItemFactsTable>,
}

impl LiveResidentWorld {
    pub(crate) fn new(
        world: WorldHandle,
        read: WorldReadView,
        blocks: Arc<BlockRegistry>,
        light: Option<Arc<BlockLightTable>>,
        zones: Option<PluginZoneAdapter>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
    ) -> Self {
        Self {
            world,
            read,
            blocks,
            light,
            zones,
            items,
            item_facts,
        }
    }

    fn block_path(&self, state: BlockStateId) -> Option<String> {
        self.blocks
            .by_id(state)
            .map(|state| state.block.id.path().to_owned())
    }

    fn air_state(&self) -> Option<BlockStateId> {
        self.state_for("air").map(BlockStateId)
    }

    fn is_air(path: &str) -> bool {
        matches!(path, "air" | "cave_air" | "void_air")
    }

    fn is_solid_support(path: &str) -> bool {
        !Self::is_air(path)
            && !matches!(
                path,
                "water" | "lava" | "short_grass" | "tall_grass" | "fern" | "torch" | "fire"
            )
    }

    /// Blocks a sight ray: anything that is not air, water or glass.
    fn is_opaque(path: &str) -> bool {
        !matches!(
            path,
            "air"
                | "cave_air"
                | "void_air"
                | "water"
                | "glass"
                | "glass_pane"
                | "short_grass"
                | "tall_grass"
                | "torch"
        )
    }

    fn loaded_block(&self, dimension: &str, pos: [i32; 3]) -> Option<(BlockStateId, String)> {
        if !self.dimension_loaded(dimension) {
            return None;
        }
        let state = self.read.get_cached_block(BlockPos {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        })?;
        let path = self.block_path(state)?;
        Some((state, path))
    }

    /// Apply one conditional edit through the shared world storage kernel.
    fn commit_edit(
        &self,
        pos: [i32; 3],
        expected_state: BlockStateId,
        new_state: BlockStateId,
    ) -> Result<(), ScriptOperationFailure> {
        let position = BlockPos {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
        let Some(token) = self.read.block_mutation_token(position) else {
            return Err(ScriptOperationFailure::Unloaded);
        };
        let edits = [ResidentBlockEdit {
            pos: position,
            new_state,
            preserve_light: false,
        }];
        let preconditions = [ResidentBlockPrecondition {
            pos: position,
            expected_state,
            expected_token: token,
        }];
        let Ok(mut storage) = self.world.try_lock() else {
            return Err(ScriptOperationFailure::Busy);
        };
        match storage.apply_block_edits_conditionally(
            &edits,
            &preconditions,
            &[],
            self.light.as_deref(),
            None,
        ) {
            Ok(ResidentBlockEditBatchResult::Applied(_)) => Ok(()),
            // A concurrently changed cell is a stale target, not a silent break.
            Ok(ResidentBlockEditBatchResult::Stale) => Err(ScriptOperationFailure::StaleRevision),
            Ok(ResidentBlockEditBatchResult::Missing) => Err(ScriptOperationFailure::Unloaded),
            Ok(ResidentBlockEditBatchResult::CrossRegion) => Err(ScriptOperationFailure::Blocked),
            Err(_) => Err(ScriptOperationFailure::RuntimeUnavailable),
        }
    }

    /// Walk a bounded integer ray, invoking `visit` per cell; `None` means the
    /// ray left loaded terrain.
    fn ray(
        from: [i32; 3],
        to: [i32; 3],
        limit: i32,
        mut visit: impl FnMut([i32; 3]) -> Option<bool>,
    ) -> Option<bool> {
        let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let steps = delta[0].abs().max(delta[1].abs()).max(delta[2].abs());
        if steps > limit {
            return None;
        }
        let steps = steps.max(1);
        for step in 0..=steps {
            let pos = [
                from[0] + delta[0] * step / steps,
                from[1] + delta[1] * step / steps,
                from[2] + delta[2] * step / steps,
            ];
            let done = visit(pos)?;
            if done {
                return Some(true);
            }
        }
        Some(true)
    }
}

impl ResidentWorld for LiveResidentWorld {
    fn dimension_loaded(&self, dimension: &str) -> bool {
        dimension == RESIDENT_WORLD_DIMENSION
    }

    fn block(&self, dimension: &str, pos: [i32; 3]) -> Option<ResidentBlock> {
        self.loaded_block(dimension, pos)
            .map(|(state, path)| ResidentBlock {
                state: state.0,
                path,
            })
    }

    fn standable(&self, dimension: &str, pos: [i32; 3]) -> Option<bool> {
        let (_, feet) = self.loaded_block(dimension, pos)?;
        let (_, head) = self.loaded_block(dimension, [pos[0], pos[1] + 1, pos[2]])?;
        let (_, floor) = self.loaded_block(dimension, [pos[0], pos[1] - 1, pos[2]])?;
        Some(
            (Self::is_air(&feet) || feet == "water")
                && (Self::is_air(&head) || head == "water")
                && Self::is_solid_support(&floor),
        )
    }

    fn route_open(&self, dimension: &str, from: [i32; 3], to: [i32; 3]) -> Option<bool> {
        let mut clear = true;
        Self::ray(from, to, MAX_ROUTE_STEPS, |pos| {
            clear = self.standable(dimension, pos)?;
            Some(!clear)
        })?;
        Some(clear)
    }

    fn line_of_sight(&self, dimension: &str, from: Vec3, to: Vec3) -> Option<bool> {
        if !from.is_finite() || !to.is_finite() {
            return None;
        }
        let start = [
            from.x.floor() as i32,
            from.y.floor() as i32,
            from.z.floor() as i32,
        ];
        let end = [
            to.x.floor() as i32,
            to.y.floor() as i32,
            to.z.floor() as i32,
        ];
        let mut visible = true;
        Self::ray(start, end, MAX_SIGHT_STEPS, |pos| {
            let (_, path) = self.loaded_block(dimension, pos)?;
            visible = !Self::is_opaque(&path);
            Some(!visible)
        })?;
        Some(visible)
    }

    fn foreign_zone_overlaps(
        &self,
        plugin_id: &str,
        dimension: &str,
        min: [i32; 3],
        max: [i32; 3],
    ) -> bool {
        self.zones
            .as_ref()
            .is_some_and(|zones| zones.foreign_zone_overlaps(plugin_id, dimension, min, max))
    }

    fn state_for(&self, block_path: &str) -> Option<u32> {
        let name = codec::Identifier::parse(format!("minecraft:{block_path}")).ok()?;
        // The canonical state of a block path is its registered default, which is
        // the state a fresh placement uses.
        self.blocks.block(&name).map(|block| block.default.0)
    }

    fn break_block(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<Vec<ResidentDrop>, ScriptOperationFailure> {
        let air = self
            .air_state()
            .ok_or(ScriptOperationFailure::RuntimeUnavailable)?;
        let Some((current, _)) = self.loaded_block(dimension, pos) else {
            return Err(ScriptOperationFailure::Unloaded);
        };
        if current.0 != expected_state {
            return Err(ScriptOperationFailure::StaleRevision);
        }
        self.commit_edit(pos, BlockStateId(expected_state), air)?;
        let held = tool.and_then(|path| self.item_stack(path));
        let stacks = block_drop_stacks_with_tool_and_facts_from_seeded(
            mc_data::loot::builtin(),
            &self.items,
            &self.item_facts,
            &self.blocks,
            BlockStateId(expected_state),
            held.as_ref(),
            block_break_seed(pos, expected_state),
        );
        let mut drops = Vec::with_capacity(stacks.len());
        for stack in stacks {
            let Some(name) = self.items.name_of(stack.item_id) else {
                continue;
            };
            let Ok(count) = u32::try_from(stack.count) else {
                continue;
            };
            if count > 0 {
                drops.push(ResidentDrop {
                    item_id: name.as_str().to_owned(),
                    count,
                });
            }
        }
        Ok(drops)
    }

    fn place_block(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<(), ScriptOperationFailure> {
        let Some((current, path)) = self.loaded_block(dimension, pos) else {
            return Err(ScriptOperationFailure::Unloaded);
        };
        if !Self::is_air(&path) {
            return Err(ScriptOperationFailure::Blocked);
        }
        self.commit_edit(pos, current, BlockStateId(state))
    }
}

impl LiveResidentWorld {
    fn item_stack(&self, block_path: &str) -> Option<ItemStack> {
        let name = codec::Identifier::parse(format!("minecraft:{block_path}")).ok()?;
        let item_id = self.items.id_of(&name)?;
        let mut stack = ItemStack::EMPTY;
        stack.item_id = item_id;
        stack.count = 1;
        Some(stack)
    }
}

/// Weapon damage of one resident attack, resolved through the engine's own
/// player combat damage table so a resident hit uses the canonical weapon value
/// instead of a fabricated number.
#[must_use]
pub(crate) fn resident_weapon_damage(
    item_facts: &ItemFactsTable,
    items: &ItemRegistry,
    item_id: Option<u32>,
) -> f32 {
    crate::play::combat::attack_damage_for_item(item_facts, items, item_id)
}

/// Deterministic loot seed of one resident break, derived from the cell and
/// state so a replay of the same committed break rolls the same drops.
fn block_break_seed(pos: [i32; 3], state: u32) -> u64 {
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64 ^ u64::from(state);
    for value in pos {
        seed = seed
            .rotate_left(17)
            .wrapping_mul(0xBF58_476D_1CE4_E5B9)
            .wrapping_add(value as u64);
    }
    seed ^ (seed >> 31)
}
