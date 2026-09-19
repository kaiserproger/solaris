//! Authoritative world adapter for resident physical work, routes and combat.
//!
//! Resident work orders execute against real blocks through the same world
//! storage kernel a player edit uses; routes, line of sight and protected zones
//! come from the same loaded chunks the rest of the server reads. The trait is
//! the seam the order executor depends on: a resident path fails closed
//! (`unloaded`/`unsupported`) when no adapter is installed, and the tests drive
//! the real [`LiveResidentWorld`] over a real world storage.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use mc_data::item_components::ItemFactsTable;
use mc_data::item_stack::ItemStack;
use mc_data::items::ItemRegistry;
use mc_entity::Vec3;
use mc_protocol::codec;
use mc_script::ScriptOperationFailure;
use mc_world::{BlockPos, BlockRegistry, BlockStateId, ResidentBlockPrecondition, WorldReadView};

use crate::play::survival::block_drop_stacks_with_tool_and_facts_from_seeded;
use crate::script::PluginZoneAdapter;

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

/// One immutable resident world edit: exact source image, destination state
/// and any canonical loot calculated before a storage batch is prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidentWorldEdit {
    pub(crate) precondition: ResidentBlockPrecondition,
    pub(crate) new_state: BlockStateId,
    pub(crate) drops: Vec<ResidentDrop>,
    /// Only felled logs schedule their neighbouring leaf updates.
    pub(crate) triggers_leaf_updates: bool,
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
    /// Canonical loot and source image of one conditional break, computed
    /// **without** touching the world. The returned precondition is the exact
    /// image the later durable decision must consume.
    ///
    /// Loot depends only on the block state, held tool and position seed, so
    /// the preview and the committed record agree exactly. It lets the caller
    /// refuse a break whose loot cannot fit before the block leaves the world.
    fn preview_break(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure>;

    /// Preview a seed placement over its exact air image without touching the
    /// world. The later decision consumes the returned precondition.
    fn preview_place(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure>;
    /// Commit prepared resident edits and their encoded storage receipt under
    /// one world decision. A stale source image leaves both world and receipt
    /// untouched; after append, recovery projects the same receipt.
    fn commit_world_edits<'a>(
        &'a self,
        plugin_id: &'a str,
        dimension: &'a str,
        edits: &'a [ResidentWorldEdit],
        receipt: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>>;
}

/// The production [`ResidentWorld`] over live world storage.
pub(crate) struct LiveResidentWorld {
    read: WorldReadView,
    blocks: Arc<BlockRegistry>,
    zones: Option<PluginZoneAdapter>,
    simulation: crate::play::SimulationHandle,
    items: Arc<ItemRegistry>,
    item_facts: Arc<ItemFactsTable>,
}

impl LiveResidentWorld {
    pub(crate) fn new(
        read: WorldReadView,
        blocks: Arc<BlockRegistry>,
        zones: Option<PluginZoneAdapter>,
        simulation: crate::play::SimulationHandle,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
    ) -> Self {
        Self {
            read,
            blocks,
            zones,
            simulation,
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

    /// The canonical loot one break of `state` at `pos` with `tool` yields.
    ///
    /// Both the preview and the commit read it, so a caller's capacity decision
    /// and the committed drops can never disagree.
    fn break_loot(&self, pos: [i32; 3], state: u32, tool: Option<&str>) -> Vec<ResidentDrop> {
        let held = tool.and_then(|path| self.item_stack(path));
        let stacks = block_drop_stacks_with_tool_and_facts_from_seeded(
            mc_data::loot::builtin(),
            &self.items,
            &self.item_facts,
            &self.blocks,
            BlockStateId(state),
            held.as_ref(),
            block_break_seed(pos, state),
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
        drops
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

    fn preview_break(
        &self,
        dimension: &str,
        pos: [i32; 3],
        expected_state: u32,
        tool: Option<&str>,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        let Some((current, path)) = self.loaded_block(dimension, pos) else {
            return Err(ScriptOperationFailure::Unloaded);
        };
        if current.0 != expected_state {
            return Err(ScriptOperationFailure::StaleRevision);
        }
        let position = BlockPos {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
        let expected_token = self
            .read
            .block_mutation_token(position)
            .ok_or(ScriptOperationFailure::Unloaded)?;
        let new_state = self
            .air_state()
            .ok_or(ScriptOperationFailure::RuntimeUnavailable)?;
        Ok(ResidentWorldEdit {
            precondition: ResidentBlockPrecondition {
                pos: position,
                expected_state: current,
                expected_token,
            },
            new_state,
            drops: self.break_loot(pos, expected_state, tool),
            triggers_leaf_updates: path.ends_with("_log"),
        })
    }

    fn preview_place(
        &self,
        dimension: &str,
        pos: [i32; 3],
        state: u32,
    ) -> Result<ResidentWorldEdit, ScriptOperationFailure> {
        let Some((current, path)) = self.loaded_block(dimension, pos) else {
            return Err(ScriptOperationFailure::Unloaded);
        };
        if !Self::is_air(&path) {
            return Err(ScriptOperationFailure::Blocked);
        }
        let position = BlockPos {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
        let expected_token = self
            .read
            .block_mutation_token(position)
            .ok_or(ScriptOperationFailure::Unloaded)?;
        Ok(ResidentWorldEdit {
            precondition: ResidentBlockPrecondition {
                pos: position,
                expected_state: current,
                expected_token,
            },
            new_state: BlockStateId(state),
            drops: Vec::new(),
            triggers_leaf_updates: false,
        })
    }

    fn commit_world_edits<'a>(
        &'a self,
        plugin_id: &'a str,
        dimension: &'a str,
        world_edits: &'a [ResidentWorldEdit],
        receipt: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, ScriptOperationFailure>> + Send + 'a>> {
        Box::pin(async move {
            if dimension != RESIDENT_WORLD_DIMENSION || world_edits.is_empty() {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            if world_edits.iter().any(|preview| {
                self.zones.as_ref().is_some_and(|zones| {
                    let position = preview.precondition.pos;
                    zones.foreign_zone_overlaps(
                        plugin_id,
                        dimension,
                        [position.x, position.y, position.z],
                        [position.x, position.y, position.z],
                    )
                })
            }) {
                return Err(ScriptOperationFailure::Forbidden);
            }
            let preconditions = world_edits
                .iter()
                .map(|preview| crate::play::BlockEditPrecondition {
                    pos: preview.precondition.pos,
                    expected_state: preview.precondition.expected_state,
                    expected_token: preview.precondition.expected_token,
                })
                .collect();
            let edits = world_edits
                .iter()
                .map(|preview| {
                    crate::play::BlockEdit::new(preview.precondition.pos, preview.new_state)
                })
                .collect();
            let triggers_leaf_updates = world_edits
                .iter()
                .any(|preview| preview.triggers_leaf_updates);
            let zone_fence = self
                .zones
                .as_ref()
                .map(PluginZoneAdapter::capture_protection_fence);
            match self
                .simulation
                .commit_server_owned_block_edits_with_preconditions(
                    plugin_id,
                    edits,
                    Some(preconditions),
                    triggers_leaf_updates,
                    zone_fence,
                    receipt,
                )
                .await
            {
                Ok(Some(decision_id)) => Ok(decision_id),
                Ok(None) => Err(ScriptOperationFailure::StaleRevision),
                Err(crate::play::SimulationRequestError::Precommit(
                    mc_script::precommit::HookFailure::PermissionDenied,
                )) => Err(ScriptOperationFailure::Forbidden),
                Err(crate::play::SimulationRequestError::Precommit(
                    mc_script::precommit::HookFailure::Stale,
                )) => Err(ScriptOperationFailure::StaleRevision),
                Err(_) => Err(ScriptOperationFailure::RuntimeUnavailable),
            }
        })
    }
}

impl LiveResidentWorld {
    /// One held item as the engine's own stack, from a resource id or a bare
    /// path. A work order names tools as `minecraft:iron_pickaxe`; resolving
    /// that as another `minecraft:` path would silently leave the worker
    /// empty-handed, and a tool-gated block would drop nothing.
    fn item_stack(&self, item: &str) -> Option<ItemStack> {
        let name = if item.contains(':') {
            codec::Identifier::parse(item.to_owned())
        } else {
            codec::Identifier::parse(format!("minecraft:{item}"))
        }
        .ok()?;
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
