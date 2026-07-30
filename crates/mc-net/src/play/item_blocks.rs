use super::*;
use mc_world::plant_rules_26_1_2::{
    PlantHorizontalDirection, cocoa_state_for_use_on, is_cocoa_beans_item,
};

/// Item->default-block-state lookup for items whose identifier also
/// names a registered block.
#[derive(Debug, Clone, Default)]
pub(super) struct ItemToBlockTable {
    entries: Vec<(u32, mc_world::BlockStateId)>,
    crop_entries: Vec<CropPlacementEntry>,
    empty_bucket_item: Option<u32>,
    water_bucket_item: Option<u32>,
    lava_bucket_item: Option<u32>,
    water_source: Option<mc_world::BlockStateId>,
    lava_source: Option<mc_world::BlockStateId>,
}

#[derive(Debug, Clone)]
struct CropPlacementEntry {
    item_id: u32,
    soil_block: Identifier,
    crop_state: mc_world::BlockStateId,
}

impl ItemToBlockTable {
    pub(super) fn build(items: &ItemRegistry, blocks: &mc_world::BlockRegistry) -> Self {
        let entries = items
            .iter()
            .filter_map(|(item_name, item_pid)| {
                blocks
                    .block(item_name)
                    .map(|block| (item_pid, block.default))
            })
            .collect();
        let crop_entries = crop_placement_entries(items, blocks);
        let empty_bucket_item = item_id(items, "minecraft:bucket");
        let water_bucket_item = item_id(items, "minecraft:water_bucket");
        let lava_bucket_item = item_id(items, "minecraft:lava_bucket");
        let water_source = fluid_state_with_level(blocks, FluidKind::Water, 0);
        let lava_source = fluid_state_with_level(blocks, FluidKind::Lava, 0);
        Self {
            entries,
            crop_entries,
            empty_bucket_item,
            water_bucket_item,
            lava_bucket_item,
            water_source,
            lava_source,
        }
    }

    pub(super) fn resolve(&self, item_id: u32) -> Option<mc_world::BlockStateId> {
        self.entries
            .iter()
            .find_map(|(id, state)| (*id == item_id).then_some(*state))
    }

    pub(super) fn resolve_for_use_on(
        &self,
        items: &ItemRegistry,
        item_id: u32,
        clicked_state: mc_world::BlockStateId,
        direction: Direction,
        blocks: &mc_world::BlockRegistry,
    ) -> Option<mc_world::BlockStateId> {
        if direction.normal().1 == 1
            && let Some(clicked) = blocks.by_id(clicked_state)
            && let Some(entry) = self
                .crop_entries
                .iter()
                .find(|entry| entry.item_id == item_id && clicked.block.id == entry.soil_block)
        {
            return Some(entry.crop_state);
        }
        if self
            .crop_entries
            .iter()
            .any(|entry| entry.item_id == item_id)
        {
            return None;
        }
        if items.name_of(item_id).is_some_and(is_cocoa_beans_item) {
            let direction = match direction {
                Direction::North => PlantHorizontalDirection::North,
                Direction::South => PlantHorizontalDirection::South,
                Direction::West => PlantHorizontalDirection::West,
                Direction::East => PlantHorizontalDirection::East,
                Direction::Down | Direction::Up => return None,
            };
            return cocoa_state_for_use_on(clicked_state, direction, blocks);
        }
        if is_sign_item(items, item_id) {
            return sign_state_for_use_on(items, item_id, direction, blocks);
        }
        self.resolve(item_id)
    }

    pub(super) fn empty_bucket_item(&self) -> Option<u32> {
        self.empty_bucket_item
    }

    pub(super) fn filled_bucket_item(&self, kind: FluidKind) -> Option<u32> {
        match kind {
            FluidKind::Water => self.water_bucket_item,
            FluidKind::Lava => self.lava_bucket_item,
        }
    }

    pub(super) fn bucket_fluid_kind(&self, item_id: u32) -> Option<FluidKind> {
        if Some(item_id) == self.water_bucket_item {
            Some(FluidKind::Water)
        } else if Some(item_id) == self.lava_bucket_item {
            Some(FluidKind::Lava)
        } else {
            None
        }
    }

    pub(super) fn fluid_source_state(&self, kind: FluidKind) -> Option<mc_world::BlockStateId> {
        match kind {
            FluidKind::Water => self.water_source,
            FluidKind::Lava => self.lava_source,
        }
    }
}

fn is_sign_item(items: &ItemRegistry, item_id: u32) -> bool {
    items.name_of(item_id).is_some_and(|item| {
        let path = item.path();
        path.ends_with("_sign") && !path.ends_with("_hanging_sign")
    })
}

fn sign_state_for_use_on(
    items: &ItemRegistry,
    item_id: u32,
    direction: Direction,
    blocks: &mc_world::BlockRegistry,
) -> Option<mc_world::BlockStateId> {
    let item = items.name_of(item_id)?;
    let path = item.path();
    if !path.ends_with("_sign") || path.ends_with("_hanging_sign") {
        return None;
    }
    if direction == Direction::Down {
        return None;
    }
    if direction == Direction::Up {
        return blocks.block(item).map(|block| block.default);
    }
    let wood = path.strip_suffix("_sign")?;
    let wall = Identifier::parse(format!("{}:{}_wall_sign", item.namespace(), wood)).ok()?;
    blocks.block(&wall).map(|block| block.default)
}

fn item_id(items: &ItemRegistry, name: &str) -> Option<u32> {
    items.id_of(&Identifier::parse(name).expect("static identifier"))
}

fn crop_placement_entries(
    items: &ItemRegistry,
    blocks: &mc_world::BlockRegistry,
) -> Vec<CropPlacementEntry> {
    [
        (
            "minecraft:wheat_seeds",
            "minecraft:farmland",
            "minecraft:wheat",
        ),
        (
            "minecraft:carrot",
            "minecraft:farmland",
            "minecraft:carrots",
        ),
        (
            "minecraft:potato",
            "minecraft:farmland",
            "minecraft:potatoes",
        ),
        (
            "minecraft:beetroot_seeds",
            "minecraft:farmland",
            "minecraft:beetroots",
        ),
        (
            "minecraft:nether_wart",
            "minecraft:soul_sand",
            "minecraft:nether_wart",
        ),
    ]
    .into_iter()
    .filter_map(|(item, soil, crop)| crop_placement_entry(items, blocks, item, soil, crop))
    .collect()
}

fn crop_placement_entry(
    items: &ItemRegistry,
    blocks: &mc_world::BlockRegistry,
    item: &str,
    soil: &str,
    crop: &str,
) -> Option<CropPlacementEntry> {
    let item = Identifier::parse(item).expect("static identifier");
    let soil = Identifier::parse(soil).expect("static identifier");
    let crop = Identifier::parse(crop).expect("static identifier");
    let item_id = items.id_of(&item)?;
    blocks.block(&soil)?;
    let crop_state = crop_state_with_age(blocks, &crop, 0)?;
    Some(CropPlacementEntry {
        item_id,
        soil_block: soil,
        crop_state,
    })
}
