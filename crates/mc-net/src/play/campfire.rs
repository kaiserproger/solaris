use std::collections::HashSet;

use mc_data::ItemStack;
use mc_data::items::ItemRegistry;
use mc_data::tags::TagsData;
use mc_entity::EntityItemStack;
use mc_nbt::{ListTag, Tag};
use mc_protocol::codec::Identifier;
use mc_world::{BlockRegistry, BlockStateId, Chunk};
use md5::{Digest, Md5};
use tracing::warn;

use crate::play::containers::find_campfire_recipe_in;
use crate::play::persistence::{entity_item_stack_tag, read_entity_item_stack};

pub(in crate::play) const CAMPFIRE_COOKING_SLOT_COUNT: usize = 4;
pub(in crate::play) const CAMPFIRE_NBT_COOKING_TIMES: &str = "CookingTimes";
pub(in crate::play) const CAMPFIRE_NBT_COOKING_TOTAL_TIMES: &str = "CookingTotalTimes";
const CAMPFIRE_NBT_PENDING_OUTPUTS: &str = "SolarisPendingCampfireOutputs";
pub(in crate::play) const LEGACY_CAMPFIRE_NBT_REMAINING: &str = "solaris_cooking_remaining";
pub(in crate::play) const LEGACY_CAMPFIRE_NBT_TOTAL: &str = "solaris_cooking_total";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) struct CampfireCookingEntry {
    pub(in crate::play) input: ItemStack,
    pub(in crate::play) result: ItemStack,
    pub(in crate::play) ticks_remaining: u32,
    pub(in crate::play) cooking_time_total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) struct PendingCampfireOutput {
    pub(in crate::play) uuid: uuid::Uuid,
    pub(in crate::play) stack: EntityItemStack,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::play) struct CampfireCookingTick {
    pub(in crate::play) completed: Vec<ItemStack>,
    pub(in crate::play) changed: bool,
    pub(in crate::play) dirty: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::play) struct CampfireCookingState {
    pub(in crate::play) slots: [Option<CampfireCookingEntry>; CAMPFIRE_COOKING_SLOT_COUNT],
    pub(in crate::play) pending_outputs: Vec<PendingCampfireOutput>,
}

impl CampfireCookingState {
    pub(in crate::play) fn insert(
        &mut self,
        input: ItemStack,
        result: ItemStack,
        cooking_time: u32,
    ) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        let cooking_time = cooking_time.max(1);
        *slot = Some(CampfireCookingEntry {
            input,
            result,
            ticks_remaining: cooking_time,
            cooking_time_total: cooking_time,
        });
        true
    }

    pub(in crate::play) fn tick_for_decision(
        &mut self,
        world_decision_id: u64,
        position: mc_world::BlockPos,
    ) -> CampfireCookingTick {
        let mut tick = CampfireCookingTick::default();
        for (slot_index, slot) in self.slots.iter_mut().enumerate() {
            let Some(entry) = slot.as_mut() else {
                continue;
            };
            entry.ticks_remaining = entry.ticks_remaining.saturating_sub(1);
            tick.dirty = true;
            if entry.ticks_remaining == 0 {
                let entry = slot.take().expect("entry existed before completion");
                self.pending_outputs.push(PendingCampfireOutput {
                    uuid: campfire_output_uuid(world_decision_id, position, slot_index),
                    stack: entity_item_stack(entry.result.clone()),
                });
                tick.completed.push(entry.result);
                tick.changed = true;
            }
        }
        tick
    }

    #[cfg(test)]
    pub(in crate::play) fn tick(&mut self) -> CampfireCookingTick {
        self.tick_for_decision(0, mc_world::BlockPos { x: 0, y: 0, z: 0 })
    }

    pub(in crate::play) fn cool_down(&mut self) -> bool {
        let mut dirty = false;
        for slot in &mut self.slots {
            let Some(entry) = slot.as_mut() else {
                continue;
            };
            let cooled = entry
                .ticks_remaining
                .saturating_add(2)
                .min(entry.cooking_time_total);
            dirty |= cooled != entry.ticks_remaining;
            entry.ticks_remaining = cooled;
        }
        dirty
    }

    pub(in crate::play) fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none) && self.pending_outputs.is_empty()
    }
}

pub(in crate::play) fn campfire_output_uuid(
    world_decision_id: u64,
    position: mc_world::BlockPos,
    slot: usize,
) -> uuid::Uuid {
    let mut hasher = Md5::new();
    hasher.update(b"solaris:campfire-output:v1");
    hasher.update(world_decision_id.to_be_bytes());
    hasher.update(position.x.to_be_bytes());
    hasher.update(position.y.to_be_bytes());
    hasher.update(position.z.to_be_bytes());
    hasher.update((slot as u64).to_be_bytes());
    let mut bytes: [u8; 16] = hasher.finalize().into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

fn entity_item_stack(stack: ItemStack) -> EntityItemStack {
    EntityItemStack {
        item_id: stack.item_id,
        count: stack.count,
        damage: stack.damage,
        enchantments: stack.enchantments,
        custom_name: stack.custom_name.map(Box::new),
        item_model: stack.item_model.as_deref().cloned().map(Box::new),
    }
}

pub(in crate::play) fn is_campfire_block(
    blocks: &BlockRegistry,
    block_state: BlockStateId,
) -> bool {
    blocks.by_id(block_state).is_some_and(|block_state| {
        matches!(
            block_state.block.id.as_str(),
            "minecraft:campfire" | "minecraft:soul_campfire"
        )
    })
}

pub(in crate::play) fn is_lit_campfire_block(
    blocks: &BlockRegistry,
    block_state: BlockStateId,
) -> bool {
    blocks.by_id(block_state).is_some_and(|block_state| {
        matches!(
            block_state.block.id.as_str(),
            "minecraft:campfire" | "minecraft:soul_campfire"
        ) && block_state
            .properties
            .iter()
            .any(|(key, value)| key == "lit" && value == "true")
    })
}

pub(in crate::play) fn campfire_recipe_result_stack(
    items: &ItemRegistry,
    recipe: &mc_data::recipes::Recipe,
) -> Option<ItemStack> {
    let item_id = items.id_of(&recipe.result.item)?;
    let count = i32::try_from(recipe.result.count).ok()?;
    (count > 0).then(|| ItemStack::new(item_id, count))
}

pub(in crate::play) fn campfire_cooking_states_from_chunk(
    chunk: &Chunk,
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
) -> Vec<(mc_world::BlockPos, CampfireCookingState)> {
    let mut entries: Vec<_> = chunk.block_entities.iter().collect();
    entries.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    entries
        .into_iter()
        .filter_map(|(position, bytes)| {
            campfire_cooking_state_from_persistent_nbt(bytes, recipes, items, tags)
                .map(|cooking| (*position, cooking))
        })
        .collect()
}

pub(in crate::play) fn campfire_cooking_states_from_chunk_strict(
    chunk: &Chunk,
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
) -> Result<Vec<(mc_world::BlockPos, CampfireCookingState)>, String> {
    let mut entries: Vec<_> = chunk.block_entities.iter().collect();
    entries.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    let mut cooking = Vec::new();
    for (position, bytes) in entries {
        if let Some(state) =
            campfire_cooking_state_from_persistent_nbt_strict(bytes, recipes, items, tags)
                .map_err(|error| format!("campfire at {position:?}: {error}"))?
        {
            cooking.push((*position, state));
        }
    }
    Ok(cooking)
}

pub(in crate::play) fn campfire_cooking_state_from_persistent_nbt(
    bytes: &[u8],
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
) -> Option<CampfireCookingState> {
    campfire_cooking_state_from_persistent_nbt_strict(bytes, recipes, items, tags)
        .ok()
        .flatten()
}

pub(in crate::play) fn campfire_cooking_state_from_persistent_nbt_strict(
    bytes: &[u8],
    recipes: &[mc_data::recipes::Recipe],
    items: &ItemRegistry,
    tags: &TagsData,
) -> Result<Option<CampfireCookingState>, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    let tag = mc_nbt::read_network(&mut cursor)
        .map_err(|error| format!("campfire block entity NBT decode failed: {error}"))?;
    let Some(id) = compound_string_field(&tag, "id") else {
        return Ok(None);
    };
    if !matches!(id, "minecraft:campfire" | "minecraft:soul_campfire") {
        return Ok(None);
    }
    let Some(Tag::List(item_list)) = compound_field(&tag, "Items") else {
        return Ok(None);
    };
    let mut cooking = CampfireCookingState::default();
    for item in &item_list.elements {
        let Some((slot, input)) = campfire_persistent_input_stack(item, items) else {
            continue;
        };
        if cooking.slots[slot].is_some() {
            continue;
        }
        let Some((ticks_remaining, cooking_time_total)) = campfire_persistent_timing(&tag, slot)
        else {
            continue;
        };
        let Some(recipe) = find_campfire_recipe_in(recipes, items, tags, input.item_id) else {
            continue;
        };
        let Some(result) = campfire_recipe_result_stack(items, &recipe) else {
            continue;
        };
        cooking.slots[slot] = Some(CampfireCookingEntry {
            input,
            result,
            ticks_remaining,
            cooking_time_total,
        });
    }
    cooking.pending_outputs = pending_campfire_outputs_from_nbt(&tag, items)?;
    Ok((!cooking.is_empty()).then_some(cooking))
}

pub(in crate::play) fn pending_campfire_outputs_from_nbt(
    tag: &Tag,
    items: &ItemRegistry,
) -> Result<Vec<PendingCampfireOutput>, String> {
    let Some(pending) = compound_field(tag, CAMPFIRE_NBT_PENDING_OUTPUTS) else {
        return Ok(Vec::new());
    };
    let Tag::List(pending) = pending else {
        return Err(format!("{CAMPFIRE_NBT_PENDING_OUTPUTS} is not a list"));
    };
    let mut outputs = Vec::with_capacity(pending.elements.len());
    let mut uuids = HashSet::with_capacity(pending.elements.len());
    for element in &pending.elements {
        let Tag::Compound(fields) = element else {
            return Err(format!(
                "{CAMPFIRE_NBT_PENDING_OUTPUTS} contains a non-compound element"
            ));
        };
        let uuid = pending_campfire_output_uuid(fields)
            .ok_or_else(|| format!("{CAMPFIRE_NBT_PENDING_OUTPUTS} contains an invalid Uuid"))?;
        if !uuids.insert(uuid) {
            return Err(format!(
                "{CAMPFIRE_NBT_PENDING_OUTPUTS} contains duplicate Uuid {uuid}"
            ));
        }
        let Some(Tag::Compound(item_fields)) = fields
            .iter()
            .find_map(|(name, value)| (name == "Item").then_some(value))
        else {
            return Err(format!(
                "{CAMPFIRE_NBT_PENDING_OUTPUTS} contains an invalid Item"
            ));
        };
        let stack = read_entity_item_stack(item_fields, items)
            .map_err(|error| format!("pending campfire output item decode failed: {error}"))?
            .filter(|stack| !stack.is_empty())
            .ok_or_else(|| format!("{CAMPFIRE_NBT_PENDING_OUTPUTS} contains an empty Item"))?;
        outputs.push(PendingCampfireOutput { uuid, stack });
    }
    Ok(outputs)
}

fn pending_campfire_output_uuid(fields: &[(String, Tag)]) -> Option<uuid::Uuid> {
    let Tag::IntArray(values) = fields
        .iter()
        .find_map(|(name, value)| (name == "Uuid").then_some(value))?
    else {
        return None;
    };
    if values.len() != 4 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (index, value) in values.iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    Some(uuid::Uuid::from_bytes(bytes))
}

fn pending_campfire_output_uuid_tag(uuid: uuid::Uuid) -> Tag {
    let bytes = uuid.as_u128().to_be_bytes();
    Tag::IntArray(
        bytes
            .chunks_exact(4)
            .map(|chunk| i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

fn campfire_persistent_input_stack(item: &Tag, items: &ItemRegistry) -> Option<(usize, ItemStack)> {
    let slot = usize::try_from(compound_int_field(item, "Slot")?).ok()?;
    if slot >= CAMPFIRE_COOKING_SLOT_COUNT {
        return None;
    }
    let item_id = Identifier::parse(compound_string_field(item, "id")?.to_string()).ok()?;
    let item_id = items.id_of(&item_id)?;
    let count = compound_int_field(item, "count")?;
    (count > 0).then_some((slot, ItemStack::new(item_id, count)))
}

pub(in crate::play) fn compound_field<'a>(tag: &'a Tag, name: &str) -> Option<&'a Tag> {
    let Tag::Compound(fields) = tag else {
        return None;
    };
    fields
        .iter()
        .find_map(|(field, value)| (field == name).then_some(value))
}

fn compound_string_field<'a>(tag: &'a Tag, name: &str) -> Option<&'a str> {
    match compound_field(tag, name)? {
        Tag::String(value) => Some(value.as_str()),
        _ => None,
    }
}

fn compound_int_field(tag: &Tag, name: &str) -> Option<i32> {
    match compound_field(tag, name)? {
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}

pub(in crate::play) fn compound_int_array_field<'a>(tag: &'a Tag, name: &str) -> Option<&'a [i32]> {
    match compound_field(tag, name)? {
        Tag::IntArray(values) => Some(values.as_slice()),
        _ => None,
    }
}

fn campfire_persistent_timing(tag: &Tag, slot: usize) -> Option<(u32, u32)> {
    campfire_vanilla_persistent_timing(tag, slot)
        .or_else(|| campfire_legacy_persistent_timing(tag, slot))
}

fn campfire_vanilla_persistent_timing(tag: &Tag, slot: usize) -> Option<(u32, u32)> {
    let progress =
        u32::try_from(*compound_int_array_field(tag, CAMPFIRE_NBT_COOKING_TIMES)?.get(slot)?)
            .ok()?;
    let total =
        u32::try_from(*compound_int_array_field(tag, CAMPFIRE_NBT_COOKING_TOTAL_TIMES)?.get(slot)?)
            .ok()?;
    if total == 0 {
        return None;
    }
    Some((total.saturating_sub(progress).max(1), total))
}

fn campfire_legacy_persistent_timing(tag: &Tag, slot: usize) -> Option<(u32, u32)> {
    let remaining =
        u32::try_from(*compound_int_array_field(tag, LEGACY_CAMPFIRE_NBT_REMAINING)?.get(slot)?)
            .ok()?;
    let total =
        u32::try_from(*compound_int_array_field(tag, LEGACY_CAMPFIRE_NBT_TOTAL)?.get(slot)?)
            .ok()?;
    (remaining > 0 && total > 0).then_some((remaining, total))
}

pub(in crate::play) fn campfire_block_entity_update_nbt(
    items: &ItemRegistry,
    cooking: &CampfireCookingState,
) -> Option<Tag> {
    let mut item_tags = Vec::new();
    for (slot, entry) in cooking.slots.iter().enumerate() {
        let Some(entry) = entry else {
            continue;
        };
        let name = items.name_of(entry.input.item_id)?;
        item_tags.push(Tag::Compound(vec![
            ("Slot".into(), Tag::Int(slot as i32)),
            ("id".into(), Tag::String(name.as_str().to_string())),
            ("count".into(), Tag::Int(entry.input.count)),
        ]));
    }
    Some(Tag::Compound(vec![(
        "Items".into(),
        Tag::List(ListTag {
            element_type: if item_tags.is_empty() {
                mc_nbt::tag_type::END
            } else {
                mc_nbt::tag_type::COMPOUND
            },
            elements: item_tags,
        }),
    )]))
}

pub(in crate::play) fn campfire_block_entity_persistent_nbt(
    block_entity_id: &str,
    position: mc_world::BlockPos,
    items: &ItemRegistry,
    cooking: &CampfireCookingState,
) -> Option<Tag> {
    let Tag::Compound(mut fields) = campfire_block_entity_update_nbt(items, cooking)? else {
        return None;
    };
    fields.push(("id".into(), Tag::String(block_entity_id.to_string())));
    fields.push(("x".into(), Tag::Int(position.x)));
    fields.push(("y".into(), Tag::Int(position.y)));
    fields.push(("z".into(), Tag::Int(position.z)));
    fields.push((
        CAMPFIRE_NBT_COOKING_TIMES.into(),
        Tag::IntArray(
            cooking
                .slots
                .iter()
                .map(|slot| {
                    slot.as_ref().map_or(0, |entry| {
                        i32::try_from(
                            entry
                                .cooking_time_total
                                .saturating_sub(entry.ticks_remaining),
                        )
                        .unwrap_or(i32::MAX)
                    })
                })
                .collect(),
        ),
    ));
    fields.push((
        CAMPFIRE_NBT_COOKING_TOTAL_TIMES.into(),
        Tag::IntArray(
            cooking
                .slots
                .iter()
                .map(|slot| {
                    slot.as_ref().map_or(0, |entry| {
                        i32::try_from(entry.cooking_time_total).unwrap_or(i32::MAX)
                    })
                })
                .collect(),
        ),
    ));
    let mut pending_outputs = Vec::with_capacity(cooking.pending_outputs.len());
    for output in &cooking.pending_outputs {
        let item = entity_item_stack_tag(items, &output.stack).ok()?;
        pending_outputs.push(Tag::Compound(vec![
            ("Uuid".into(), pending_campfire_output_uuid_tag(output.uuid)),
            ("Item".into(), item),
        ]));
    }
    fields.push((
        CAMPFIRE_NBT_PENDING_OUTPUTS.into(),
        Tag::List(ListTag {
            element_type: if pending_outputs.is_empty() {
                mc_nbt::tag_type::END
            } else {
                mc_nbt::tag_type::COMPOUND
            },
            elements: pending_outputs,
        }),
    ));
    Some(Tag::Compound(fields))
}

pub(in crate::play) fn campfire_block_entity_id(
    blocks: &BlockRegistry,
    state: BlockStateId,
) -> Option<&'static str> {
    match blocks.by_id(state)?.block.id.as_str() {
        "minecraft:campfire" => Some("minecraft:campfire"),
        "minecraft:soul_campfire" => Some("minecraft:soul_campfire"),
        _ => None,
    }
}

pub(in crate::play) fn campfire_block_entity_persistent_bytes(
    block_entity_id: &str,
    position: mc_world::BlockPos,
    items: &ItemRegistry,
    cooking: &CampfireCookingState,
) -> Option<Vec<u8>> {
    let Some(tag) = campfire_block_entity_persistent_nbt(block_entity_id, position, items, cooking)
    else {
        warn!(
            ?position,
            "campfire block entity persistence skipped for unknown item id"
        );
        return None;
    };
    let mut bytes = Vec::new();
    if let Err(err) = mc_nbt::write_network(&mut bytes, &tag) {
        warn!(error = %err, ?position, "campfire block entity NBT encode failed");
        return None;
    }
    Some(bytes)
}
