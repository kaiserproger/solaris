use super::super::containers::{
    BLAST_FURNACE_MENU_TYPE_ID, ENCHANTING_MENU_TYPE_ID, FURNACE_MENU_TYPE_ID, FurnaceKind,
    SMOKER_MENU_TYPE_ID, STONECUTTER_MENU_TYPE_ID,
};

#[test]
fn common_container_paper_cuts_resolve_to_existing_menus() {
    assert_eq!(
        super::super::containers::furnace_menu_title_for_block_id("minecraft:furnace"),
        Some("Furnace")
    );
    assert_eq!(
        super::super::containers::furnace_menu_title_for_block_id("minecraft:smoker"),
        Some("Smoker")
    );
    assert_eq!(
        super::super::containers::furnace_menu_title_for_block_id("minecraft:blast_furnace"),
        Some("Blast Furnace")
    );
    assert_eq!(FurnaceKind::Furnace.menu_type(), FURNACE_MENU_TYPE_ID);
    assert_eq!(FurnaceKind::Smoker.menu_type(), SMOKER_MENU_TYPE_ID);
    assert_eq!(
        FurnaceKind::BlastFurnace.menu_type(),
        BLAST_FURNACE_MENU_TYPE_ID
    );
    assert_eq!(ENCHANTING_MENU_TYPE_ID, 13);
    assert_eq!(STONECUTTER_MENU_TYPE_ID, 24);
    assert_eq!(
        super::super::containers::unsupported_survival_station_for_block_id(
            "minecraft:enchanting_table"
        ),
        None
    );
    assert_eq!(
        super::super::containers::unsupported_survival_station_for_block_id(
            "minecraft:stonecutter"
        ),
        None
    );

    let expected_unsupported_stations = [
        ("minecraft:brewing_stand", "brewing stand"),
        ("minecraft:anvil", "anvil"),
        ("minecraft:chipped_anvil", "anvil"),
        ("minecraft:damaged_anvil", "anvil"),
        ("minecraft:smithing_table", "smithing table"),
        ("minecraft:grindstone", "grindstone"),
        ("minecraft:loom", "loom"),
        ("minecraft:cartography_table", "cartography table"),
        ("minecraft:composter", "composter"),
        ("minecraft:cauldron", "cauldron"),
        ("minecraft:water_cauldron", "cauldron"),
        ("minecraft:lava_cauldron", "cauldron"),
        ("minecraft:powder_snow_cauldron", "cauldron"),
        ("minecraft:lectern", "lectern"),
        ("minecraft:fletching_table", "fletching table"),
        ("minecraft:beacon", "beacon"),
        ("minecraft:crafter", "crafter"),
    ];
    for (block_id, station) in expected_unsupported_stations {
        assert_eq!(
            super::super::containers::unsupported_survival_station_for_block_id(block_id),
            Some(station),
            "{block_id} must be covered by the M87 safe-rejection policy"
        );
    }
}

#[test]
fn cauldron_variants_are_safe_interaction_targets() {
    let cauldron_variants = [
        "minecraft:cauldron",
        "minecraft:water_cauldron",
        "minecraft:lava_cauldron",
        "minecraft:powder_snow_cauldron",
    ];

    for block_id in cauldron_variants {
        assert_eq!(
            super::super::containers::unsupported_survival_station_for_block_id(block_id),
            Some("cauldron"),
            "{block_id} must not fall through into adjacent block placement"
        );
    }
}
