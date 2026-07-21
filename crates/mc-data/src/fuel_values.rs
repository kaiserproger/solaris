//! Immutable furnace-fuel lookup derived from the resolved item-tag graph.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use crate::Identifier;
use crate::items::ItemRegistry;
use crate::tags::TagsData;

const BASE_UNIT: i16 = 200;
const EMBEDDED_FUEL_VALUES: &str = include_str!("../data/fuel_values_26_1_2.json");
const REQUIRED_FUEL_TAGS: &[&str] = &[
    "logs",
    "bamboo_blocks",
    "planks",
    "wooden_stairs",
    "wooden_slabs",
    "wooden_trapdoors",
    "wooden_pressure_plates",
    "wooden_shelves",
    "wooden_fences",
    "fence_gates",
    "banners",
    "signs",
    "hanging_signs",
    "wooden_doors",
    "boats",
    "wool",
    "wooden_buttons",
    "saplings",
    "wool_carpets",
    "non_flammable_wood",
];

static EMBEDDED_VALUES: OnceLock<BTreeMap<String, i16>> = OnceLock::new();

/// Default-feature-set furnace fuels for the pinned Minecraft 26.1.2 runtime.
#[derive(Debug, Clone, Default)]
pub struct FuelValues {
    burn_durations: BTreeMap<u32, i16>,
}

impl FuelValues {
    pub(crate) fn vanilla_26_1_2(items: &ItemRegistry, tags: &TagsData) -> Self {
        if Self::has_complete_fuel_tags(tags) {
            Self::from_resolved_tags_26_1_2(items, tags)
        } else {
            Self::embedded_26_1_2(items)
        }
    }

    /// Burn duration in game ticks, or `None` when the item is not fuel.
    #[must_use]
    pub fn burn_duration(&self, item_id: u32) -> Option<i16> {
        self.burn_durations.get(&item_id).copied()
    }

    #[must_use]
    pub fn is_fuel(&self, item_id: u32) -> bool {
        self.burn_durations.contains_key(&item_id)
    }

    #[must_use]
    pub fn fuel_count(&self) -> usize {
        self.burn_durations.len()
    }

    #[must_use]
    pub fn matches_default_vanilla_26_1_2(&self, items: &ItemRegistry) -> bool {
        self.burn_durations == Self::embedded_26_1_2(items).burn_durations
    }

    fn from_resolved_tags_26_1_2(items: &ItemRegistry, tags: &TagsData) -> Self {
        let mut builder = Builder::new(items, tags);
        builder.add_item("lava_bucket", BASE_UNIT * 100);
        builder.add_item("coal_block", BASE_UNIT * 8 * 10);
        builder.add_item("blaze_rod", BASE_UNIT * 12);
        builder.add_item("coal", BASE_UNIT * 8);
        builder.add_item("charcoal", BASE_UNIT * 8);
        builder.add_tag("logs", BASE_UNIT * 3 / 2);
        builder.add_tag("bamboo_blocks", BASE_UNIT * 3 / 2);
        builder.add_tag("planks", BASE_UNIT * 3 / 2);
        builder.add_item("bamboo_mosaic", BASE_UNIT * 3 / 2);
        builder.add_tag("wooden_stairs", BASE_UNIT * 3 / 2);
        builder.add_item("bamboo_mosaic_stairs", BASE_UNIT * 3 / 2);
        builder.add_tag("wooden_slabs", BASE_UNIT * 3 / 4);
        builder.add_item("bamboo_mosaic_slab", BASE_UNIT * 3 / 4);
        builder.add_tag("wooden_trapdoors", BASE_UNIT * 3 / 2);
        builder.add_tag("wooden_pressure_plates", BASE_UNIT * 3 / 2);
        builder.add_tag("wooden_shelves", BASE_UNIT * 3 / 2);
        builder.add_tag("wooden_fences", BASE_UNIT * 3 / 2);
        builder.add_tag("fence_gates", BASE_UNIT * 3 / 2);
        for item in [
            "note_block",
            "bookshelf",
            "chiseled_bookshelf",
            "lectern",
            "jukebox",
            "chest",
            "trapped_chest",
            "crafting_table",
            "daylight_detector",
        ] {
            builder.add_item(item, BASE_UNIT * 3 / 2);
        }
        builder.add_tag("banners", BASE_UNIT * 3 / 2);
        for item in ["bow", "fishing_rod", "ladder"] {
            builder.add_item(item, BASE_UNIT * 3 / 2);
        }
        builder.add_tag("signs", BASE_UNIT);
        builder.add_tag("hanging_signs", BASE_UNIT * 4);
        for item in [
            "wooden_shovel",
            "wooden_sword",
            "wooden_spear",
            "wooden_hoe",
            "wooden_axe",
            "wooden_pickaxe",
        ] {
            builder.add_item(item, BASE_UNIT);
        }
        builder.add_tag("wooden_doors", BASE_UNIT);
        builder.add_tag("boats", BASE_UNIT * 6);
        builder.add_tag("wool", BASE_UNIT / 2);
        builder.add_tag("wooden_buttons", BASE_UNIT / 2);
        builder.add_item("stick", BASE_UNIT / 2);
        builder.add_tag("saplings", BASE_UNIT / 2);
        builder.add_item("bowl", BASE_UNIT / 2);
        builder.add_tag("wool_carpets", 1 + BASE_UNIT / 3);
        builder.add_item("dried_kelp_block", 1 + BASE_UNIT * 20);
        builder.add_item("crossbow", BASE_UNIT * 3 / 2);
        builder.add_item("bamboo", BASE_UNIT / 4);
        builder.add_item("dead_bush", BASE_UNIT / 2);
        builder.add_item("short_dry_grass", BASE_UNIT / 2);
        builder.add_item("tall_dry_grass", BASE_UNIT / 2);
        builder.add_item("scaffolding", BASE_UNIT / 4);
        for item in [
            "loom",
            "barrel",
            "cartography_table",
            "fletching_table",
            "smithing_table",
            "composter",
        ] {
            builder.add_item(item, BASE_UNIT * 3 / 2);
        }
        builder.add_item("azalea", BASE_UNIT / 2);
        builder.add_item("flowering_azalea", BASE_UNIT / 2);
        builder.add_item("mangrove_roots", BASE_UNIT * 3 / 2);
        builder.add_item("leaf_litter", BASE_UNIT / 2);
        builder.remove_tag("non_flammable_wood");
        builder.build()
    }

    fn embedded_26_1_2(items: &ItemRegistry) -> Self {
        // This repo-owned derived snapshot is the default-feature-set output of
        // the same 26.1.2 builder. It is only the boundary for configurations
        // without the complete vanilla fuel-tag graph.
        let snapshot = EMBEDDED_VALUES.get_or_init(|| {
            serde_json::from_str(EMBEDDED_FUEL_VALUES)
                .expect("embedded 26.1.2 fuel-values JSON is valid")
        });
        let burn_durations = snapshot
            .iter()
            .filter_map(|(name, duration)| {
                let name =
                    Identifier::parse(name.clone()).expect("embedded fuel identifier is valid");
                items.id_of(&name).map(|item_id| (item_id, *duration))
            })
            .collect();
        Self { burn_durations }
    }

    fn has_complete_fuel_tags(tags: &TagsData) -> bool {
        let item_registry = Identifier::parse("minecraft:item").expect("static identifier");
        let Some(item_tags) = tags.registries.get(&item_registry) else {
            return false;
        };
        REQUIRED_FUEL_TAGS.iter().all(|tag| {
            let tag =
                Identifier::parse(format!("minecraft:{tag}")).expect("static fuel tag identifier");
            item_tags.contains_key(&tag)
        })
    }
}

struct Builder<'a> {
    items: &'a ItemRegistry,
    tags: &'a TagsData,
    burn_durations: BTreeMap<u32, i16>,
}

impl<'a> Builder<'a> {
    fn new(items: &'a ItemRegistry, tags: &'a TagsData) -> Self {
        Self {
            items,
            tags,
            burn_durations: BTreeMap::new(),
        }
    }

    fn add_item(&mut self, path: &str, duration: i16) {
        let item =
            Identifier::parse(format!("minecraft:{path}")).expect("static fuel item identifier");
        if let Some(item_id) = self.items.id_of(&item) {
            self.burn_durations.insert(item_id, duration);
        }
    }

    fn add_tag(&mut self, path: &str, duration: i16) {
        for item_id in self.tag_members(path) {
            self.burn_durations.insert(item_id, duration);
        }
    }

    fn remove_tag(&mut self, path: &str) {
        for item_id in self.tag_members(path) {
            self.burn_durations.remove(&item_id);
        }
    }

    fn tag_members(&self, path: &str) -> BTreeSet<u32> {
        let item_registry = Identifier::parse("minecraft:item").expect("static identifier");
        let tag =
            Identifier::parse(format!("minecraft:{path}")).expect("static fuel tag identifier");
        self.tags
            .registries
            .get(&item_registry)
            .and_then(|item_tags| item_tags.get(&tag))
            .into_iter()
            .flatten()
            .filter_map(|item_id| u32::try_from(*item_id).ok())
            .filter(|item_id| self.items.name_of(*item_id).is_some())
            .collect()
    }

    fn build(self) -> FuelValues {
        FuelValues {
            burn_durations: self.burn_durations,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use crate::items::{ItemRegistry, ItemReport, load_items_report};
    use crate::tags::TagsData;
    use crate::{Identifier, VanillaData};

    use super::{FuelValues, REQUIRED_FUEL_TAGS};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    fn fixture(item_names: &[&str], tags: &[(&str, &[&str])]) -> (ItemRegistry, TagsData) {
        let reports = item_names
            .iter()
            .enumerate()
            .map(|(index, name)| ItemReport {
                id: Identifier::parse(*name).unwrap(),
                protocol_id: u32::try_from(index + 1).unwrap(),
            })
            .collect::<Vec<_>>();
        let items = ItemRegistry::from_report(&reports);
        let item_tags = tags
            .iter()
            .map(|(tag, entries)| {
                let ids = entries
                    .iter()
                    .map(|entry| {
                        i32::try_from(
                            items
                                .id_of(&Identifier::parse(*entry).unwrap())
                                .expect("fixture item exists"),
                        )
                        .unwrap()
                    })
                    .collect();
                (Identifier::parse(*tag).unwrap(), ids)
            })
            .collect();
        let registries =
            BTreeMap::from([(Identifier::parse("minecraft:item").unwrap(), item_tags)]);
        (items, TagsData::from_registries(registries))
    }

    #[test]
    fn vanilla_snapshot_adds_direct_items() {
        let (items, tags) = fixture(&["minecraft:lava_bucket"], &[]);
        let fuels = FuelValues::from_resolved_tags_26_1_2(&items, &tags);
        let lava_bucket = items
            .id_of(&Identifier::parse("minecraft:lava_bucket").unwrap())
            .unwrap();

        assert_eq!(fuels.burn_duration(lava_bucket), Some(20_000));
        assert!(fuels.is_fuel(lava_bucket));
    }

    #[test]
    fn vanilla_snapshot_adds_resolved_tag_members() {
        let (items, tags) = fixture(
            &["minecraft:test_log"],
            &[("minecraft:logs", &["minecraft:test_log"])],
        );
        let fuels = FuelValues::from_resolved_tags_26_1_2(&items, &tags);
        let item = items
            .id_of(&Identifier::parse("minecraft:test_log").unwrap())
            .unwrap();

        assert_eq!(fuels.burn_duration(item), Some(300));
    }

    #[test]
    fn later_builder_entries_overwrite_earlier_values() {
        let (items, tags) = fixture(
            &["minecraft:test_wood"],
            &[
                ("minecraft:logs", &["minecraft:test_wood"]),
                ("minecraft:wooden_slabs", &["minecraft:test_wood"]),
            ],
        );
        let fuels = FuelValues::from_resolved_tags_26_1_2(&items, &tags);
        let item = items
            .id_of(&Identifier::parse("minecraft:test_wood").unwrap())
            .unwrap();

        assert_eq!(fuels.burn_duration(item), Some(150));
    }

    #[test]
    fn non_flammable_wood_is_removed_after_all_additions() {
        let (items, tags) = fixture(
            &["minecraft:test_nether_wood"],
            &[
                ("minecraft:logs", &["minecraft:test_nether_wood"]),
                (
                    "minecraft:non_flammable_wood",
                    &["minecraft:test_nether_wood"],
                ),
            ],
        );
        let fuels = FuelValues::from_resolved_tags_26_1_2(&items, &tags);
        let item = items
            .id_of(&Identifier::parse("minecraft:test_nether_wood").unwrap())
            .unwrap();

        assert_eq!(fuels.burn_duration(item), None);
        assert!(!fuels.is_fuel(item));
    }

    #[test]
    fn complete_tag_graph_selects_data_driven_membership() {
        let reports = [ItemReport {
            id: Identifier::parse("minecraft:test_log").unwrap(),
            protocol_id: 1,
        }];
        let items = ItemRegistry::from_report(&reports);
        let item_tags = REQUIRED_FUEL_TAGS
            .iter()
            .map(|tag| {
                let members = if *tag == "logs" { vec![1] } else { Vec::new() };
                (
                    Identifier::parse(format!("minecraft:{tag}")).unwrap(),
                    members,
                )
            })
            .collect();
        let tags = TagsData::from_registries(BTreeMap::from([(
            Identifier::parse("minecraft:item").unwrap(),
            item_tags,
        )]))
        .with_vanilla_fuel_values(&items);

        assert_eq!(tags.fuel_values().burn_duration(1), Some(300));
    }

    #[test]
    fn full_local_2612_tags_match_embedded_snapshot_when_available() {
        let root = workspace_path("data/vanilla");
        let report_path = root.join("reports/registries.json");
        if !report_path.is_file() {
            eprintln!("skipping: {} not present", report_path.display());
            return;
        }
        let items = ItemRegistry::from_report(&load_items_report(report_path).unwrap());
        let tags = crate::tags::load(&root, &VanillaData::from_registries("", vec![])).unwrap();
        let from_tags = FuelValues::from_resolved_tags_26_1_2(&items, &tags);
        let embedded = FuelValues::embedded_26_1_2(&items);

        let tag_ids = items
            .iter()
            .filter_map(|(_, item_id)| from_tags.is_fuel(item_id).then_some(item_id))
            .collect::<Vec<_>>();
        let embedded_ids = items
            .iter()
            .filter_map(|(_, item_id)| embedded.is_fuel(item_id).then_some(item_id))
            .collect::<Vec<_>>();
        assert_eq!(tag_ids.len(), 280);
        assert_eq!(tag_ids, embedded_ids);
        for item_id in tag_ids {
            assert_eq!(
                from_tags.burn_duration(item_id),
                embedded.burn_duration(item_id),
                "{}",
                items.name_of(item_id).unwrap()
            );
        }
    }
}
