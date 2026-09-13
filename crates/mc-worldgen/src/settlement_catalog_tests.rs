//! Behaviour tests for the authored settlement blueprint catalog.

use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_world::{BlockRegistry, BlockStateId};

use crate::settlement_catalog::{
    Blueprint, BlueprintCatalog, BlueprintError, BlueprintInstance, CatalogError, MAX_BLUEPRINTS,
    MAX_CATALOG_DECODED_BYTES, PaletteEntry, QuarterTurn, rotate_block_state,
};

type BlockSpec<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>);

const ROTATIONS: [&str; 16] = [
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15",
];

fn cartesian(properties: &[(&str, Vec<&str>)]) -> Vec<BTreeMap<String, String>> {
    let mut combos = vec![BTreeMap::new()];
    for (key, values) in properties {
        let mut next = Vec::new();
        for combo in &combos {
            for value in values {
                let mut extended = combo.clone();
                extended.insert((*key).to_owned(), (*value).to_owned());
                next.push(extended);
            }
        }
        combos = next;
    }
    combos
}

fn registry(blocks: Vec<BlockSpec<'_>>) -> BlockRegistry {
    let mut report = Vec::new();
    let mut next_state = 0u32;
    for (id, properties) in blocks {
        let combos = cartesian(&properties);
        let states = combos
            .iter()
            .enumerate()
            .map(|(index, combo)| BlockStateReport {
                id: next_state + index as u32,
                default: index == 0,
                properties: combo.clone(),
            })
            .collect::<Vec<_>>();
        next_state += combos.len() as u32;
        report.push(BlockReport {
            id: Identifier::parse(id).unwrap(),
            properties: properties
                .iter()
                .map(|(key, values)| {
                    (
                        (*key).to_owned(),
                        values.iter().map(|value| (*value).to_owned()).collect(),
                    )
                })
                .collect(),
            states,
        });
    }
    BlockRegistry::from_report(&report).expect("test registry is well formed")
}

fn stone_registry() -> BlockRegistry {
    registry(vec![("minecraft:stone", Vec::new())])
}

fn file(name: &str, text: String) -> (String, String) {
    (name.to_owned(), text)
}

fn base_toml(id: &str) -> String {
    format!(
        "id = \"{id}\"\nrevision = 1\n\
         [footprint]\nsize = [4, 4, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {{}}\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n"
    )
}

fn ruined_toml(id: &str, base: &str) -> String {
    format!(
        "id = \"{id}\"\nrevision = 1\nvariant_of = \"{base}\"\n\
         [footprint]\nsize = [4, 4, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {{}}\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n\
         [[restoration_stage]]\nid = \"rebuild\"\nblocks = [ {{ x = 1, y = 0, z = 0, palette = 0 }} ]\n"
    )
}

fn blueprint_error_of(error: &CatalogError) -> &BlueprintError {
    match error {
        CatalogError::Blueprint { source, .. } => source,
        other => panic!("expected a blueprint error, got {other}"),
    }
}

#[test]
fn foreign_id_is_rejected() {
    let registry = stone_registry();
    let files = vec![file("structures/house.toml", base_toml("other:house"))];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::ForeignId { id, owner } if id == "other:house" && owner == "solaris"
    ));
}

#[test]
fn unknown_key_is_rejected() {
    let registry = stone_registry();
    let text = format!("bogus = 1\n{}", base_toml("solaris:house"));
    let files = vec![file("structures/house.toml", text)];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::UnknownKey { key, .. } if key == "bogus"
    ));
}

#[test]
fn unknown_nested_key_is_rejected() {
    let registry = stone_registry();
    let text = base_toml("solaris:house")
        .replace("[[blocks]]\nx = 0", "[[blocks]]\ncolour = \"red\"\nx = 0");
    let files = vec![file("structures/house.toml", text)];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::UnknownKey { key, .. } if key == "colour"
    ));
}

#[test]
fn out_of_range_property_is_rejected() {
    let registry = registry(vec![(
        "minecraft:oak_stairs",
        vec![
            ("facing", vec!["north", "east", "south", "west"]),
            ("half", vec!["bottom", "top"]),
            ("shape", vec!["straight"]),
        ],
    )]);
    let text = "id = \"solaris:stairs\"\nrevision = 1\n\
         [footprint]\nsize = [4, 4, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:oak_stairs\"\n\
         properties = { facing = \"up\", half = \"bottom\", shape = \"straight\" }\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n";
    let files = vec![file("structures/stairs.toml", text.to_owned())];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::OutOfRangeProperty { property, value, .. }
            if property == "facing" && value == "up"
    ));
}

#[test]
fn undeclared_property_and_unknown_block_are_rejected() {
    let registry = registry(vec![("minecraft:stone", Vec::new())]);
    let text = base_toml("solaris:house").replace(
        "block = \"minecraft:stone\"\nproperties = {}",
        "block = \"minecraft:stone\"\nproperties = { facing = \"north\" }",
    );
    let files = vec![file("structures/house.toml", text)];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::UnknownProperty { property, .. } if property == "facing"
    ));

    let text = base_toml("solaris:house").replace("minecraft:stone", "minecraft:granite");
    let files = vec![file("structures/house.toml", text)];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::UnknownBlock { block, .. } if block == "minecraft:granite"
    ));
}

#[test]
fn oversized_body_is_rejected() {
    let registry = stone_registry();
    let mut text = "id = \"solaris:big\"\nrevision = 1\n\
         [footprint]\nsize = [64, 64, 64]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {}\n"
        .to_owned();
    for _ in 0..65_537 {
        text.push_str("[[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n");
    }
    let files = vec![file("structures/big.toml", text)];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::TooManyBlocks { count: 65_537, .. }
    ));
}

#[test]
fn hash_mismatch_is_rejected() {
    let registry = stone_registry();
    let files = vec![file("structures/house.toml", base_toml("solaris:house"))];
    let expected = BTreeMap::from([("solaris:house".to_owned(), "0".repeat(64))]);
    let error = BlueprintCatalog::from_files_with_hashes(&registry, "solaris", &files, &expected)
        .unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::HashMismatch { .. }
    ));
}

#[test]
fn catalog_limits_fail_closed() {
    let registry = stone_registry();

    let files: Vec<(String, String)> = (0..=MAX_BLUEPRINTS)
        .map(|index| file(&format!("structures/{index}.toml"), String::new()))
        .collect();
    assert!(matches!(
        BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err(),
        CatalogError::TooManyBlueprints { .. }
    ));

    let files = vec![file(
        "structures/big.toml",
        "x".repeat(MAX_CATALOG_DECODED_BYTES + 1),
    )];
    assert!(matches!(
        BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err(),
        CatalogError::CatalogTooLarge { .. }
    ));

    let mut entries = vec![file("structures/base.toml", base_toml("solaris:base"))];
    for index in 0..65 {
        entries.push(file(
            &format!("structures/ruin_{index}.toml"),
            ruined_toml(&format!("solaris:ruin_{index}"), "solaris:base"),
        ));
    }
    assert!(matches!(
        BlueprintCatalog::from_files(&registry, "solaris", &entries).unwrap_err(),
        CatalogError::TooManyVariants { .. }
    ));
}

#[test]
fn stray_duplicate_and_foreign_variant_are_rejected() {
    let registry = stone_registry();

    let files = vec![file("structures/house.txt", base_toml("solaris:house"))];
    assert!(matches!(
        BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err(),
        CatalogError::StrayFile { .. }
    ));

    let files = vec![
        file("structures/house.toml", base_toml("solaris:house")),
        file("structures/house_copy.toml", base_toml("solaris:house")),
    ];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::DuplicateId { id } if id == "solaris:house"
    ));

    let files = vec![file(
        "structures/ruin.toml",
        ruined_toml("solaris:ruin", "solaris:absent"),
    )];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::UnknownVariant { base, .. } if base == "solaris:absent"
    ));
}

#[test]
fn variant_footprint_mismatch_and_ruin_rules_are_rejected() {
    let registry = stone_registry();
    let variant =
        ruined_toml("solaris:ruin", "solaris:base").replace("size = [4, 4, 4]", "size = [5, 4, 4]");
    let files = vec![
        file("structures/base.toml", base_toml("solaris:base")),
        file("structures/ruin.toml", variant),
    ];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::VariantFootprintMismatch { .. }
    ));

    let variant_without_restoration = base_toml("solaris:ruin").replace(
        "revision = 1",
        "revision = 1\nvariant_of = \"solaris:house\"",
    );
    let files = vec![
        file("structures/house.toml", base_toml("solaris:house")),
        file("structures/ruin.toml", variant_without_restoration),
    ];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::MissingRestorationStage { .. }
    ));

    let base_with_restoration = format!(
        "{}[[restoration_stage]]\nid = \"fix\"\nblocks = []\n",
        base_toml("solaris:house")
    );
    let files = vec![file("structures/house.toml", base_with_restoration)];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::UnexpectedRestorationStage { .. }
    ));
}

#[test]
fn home_without_entrance_is_rejected() {
    let registry = stone_registry();
    let text = format!(
        "{}[[poi]]\nid = \"home\"\nkind = \"home\"\nat = [1, 1, 1]\ncapacity = 2\n",
        base_toml("solaris:house")
    );
    let files = vec![file("structures/house.toml", text)];
    let error = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap_err();
    assert!(matches!(
        blueprint_error_of(&error),
        BlueprintError::MissingEntrance { poi, .. } if poi == "home"
    ));
}

#[test]
fn valid_catalog_hashes_and_variants() {
    let registry = stone_registry();
    let files = vec![
        file("structures/house.toml", base_toml("solaris:house")),
        file(
            "structures/house_ruined.toml",
            ruined_toml("solaris:house_ruined", "solaris:house"),
        ),
    ];
    let catalog = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap();
    assert_eq!(catalog.len(), 2);
    assert!(!catalog.is_empty());
    assert!(catalog.get("solaris:absent").is_none());

    let house = catalog.get("solaris:house").unwrap();
    assert_eq!(house.id(), "solaris:house");
    assert_eq!(house.size(), [4, 4, 4]);
    assert_eq!(house.anchor(), [0, 0, 0]);
    assert_eq!(house.blocks().len(), 1);
    assert_eq!(house.palette().len(), 1);
    assert!(!house.is_ruined_variant());
    assert_eq!(house.content_hash().len(), 64);
    assert!(
        house
            .content_hash()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(catalog.variants_of("solaris:house").len(), 1);
    assert!(catalog.variants_of("solaris:absent").is_empty());

    let variant = catalog.get("solaris:house_ruined").unwrap();
    assert!(variant.is_ruined_variant());
    assert_eq!(variant.restoration_stages().len(), 1);
    assert_eq!(variant.restoration_stages()[0].id, "rebuild");

    let expected = BTreeMap::from([("solaris:house".to_owned(), house.content_hash().to_owned())]);
    assert!(
        BlueprintCatalog::from_files_with_hashes(&registry, "solaris", &files, &expected).is_ok()
    );

    let expected = BTreeMap::from([("solaris:absent".to_owned(), "0".repeat(64))]);
    assert!(matches!(
        BlueprintCatalog::from_files_with_hashes(&registry, "solaris", &files, &expected)
            .unwrap_err(),
        CatalogError::MissingBlueprint { .. }
    ));

    // The content hash ignores authored ordering but tracks revisions.
    let reordered = base_toml("solaris:house").replace("revision = 1", "revision = 2");
    let files = vec![file("structures/house.toml", reordered)];
    let other = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap();
    assert_ne!(
        house.content_hash(),
        other.get("solaris:house").unwrap().content_hash()
    );
}

fn property(registry: &BlockRegistry, state: BlockStateId, name: &str) -> String {
    registry
        .by_id(state)
        .expect("placed state is registered")
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .expect("property is declared")
        .1
        .clone()
}

fn state_of(registry: &BlockRegistry, block: &str, properties: &[(&str, &str)]) -> BlockStateId {
    let name = Identifier::parse(block).unwrap();
    let properties: Vec<(String, String)> = properties
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    registry
        .by_name_and_props(&name, &properties)
        .expect("test state is registered")
}

#[test]
fn quarter_turn_basics() {
    assert_eq!(QuarterTurn::from_degrees(0), Some(QuarterTurn::None));
    assert_eq!(QuarterTurn::from_degrees(90), Some(QuarterTurn::Cw90));
    assert_eq!(QuarterTurn::from_degrees(180), Some(QuarterTurn::Cw180));
    assert_eq!(QuarterTurn::from_degrees(270), Some(QuarterTurn::Cw270));
    assert_eq!(QuarterTurn::from_degrees(45), None);
    assert_eq!(QuarterTurn::Cw180.degrees(), 180);
    assert_eq!(QuarterTurn::Cw270.turns(), 3);

    let size = [4, 5, 6];
    assert_eq!(QuarterTurn::None.rotate_offset([1, 2, 3], size), [1, 2, 3]);
    assert_eq!(QuarterTurn::Cw90.rotate_offset([0, 0, 0], size), [5, 0, 0]);
    assert_eq!(QuarterTurn::Cw90.rotate_offset([1, 2, 3], size), [2, 2, 1]);
    assert_eq!(QuarterTurn::Cw180.rotate_offset([1, 2, 3], size), [2, 2, 2]);
    assert_eq!(QuarterTurn::Cw270.rotate_offset([1, 2, 3], size), [3, 2, 2]);
    // A quarter turn is a bijection onto the rotated box.
    let mut corners = Vec::new();
    for x in [0, size[0] - 1] {
        for z in [0, size[2] - 1] {
            let rotated = QuarterTurn::Cw90.rotate_offset([x, 0, z], size);
            assert!(rotated[0] < size[2] && rotated[2] < size[0]);
            corners.push(rotated);
        }
    }
    corners.sort();
    corners.dedup();
    assert_eq!(corners.len(), 4);

    use crate::settlement_catalog::Cardinal;
    assert_eq!(Cardinal::North.rotate(QuarterTurn::Cw90), Cardinal::East);
    assert_eq!(Cardinal::West.rotate(QuarterTurn::Cw90), Cardinal::North);
    assert_eq!(Cardinal::North.rotate(QuarterTurn::Cw270), Cardinal::West);
    assert_eq!(Cardinal::South.rotate(QuarterTurn::None), Cardinal::South);
    assert_eq!(Cardinal::North.as_str(), "north");
}

#[test]
fn rotate_block_state_moves_dependent_properties() {
    let registry = registry(vec![
        ("minecraft:oak_log", vec![("axis", vec!["x", "y", "z"])]),
        ("minecraft:oak_sign", vec![("rotation", ROTATIONS.to_vec())]),
        (
            "minecraft:test_oriented",
            vec![(
                "orientation",
                vec![
                    "north_up", "east_up", "south_up", "west_up", "up_north", "up_east",
                    "up_south", "up_west",
                ],
            )],
        ),
        (
            "minecraft:test_wire",
            vec![
                ("north", vec!["false", "true"]),
                ("east", vec!["false", "true"]),
                ("south", vec!["false", "true"]),
                ("west", vec!["false", "true"]),
            ],
        ),
    ]);

    let log_x = state_of(&registry, "minecraft:oak_log", &[("axis", "x")]);
    assert_eq!(
        rotate_block_state(&registry, log_x, QuarterTurn::None),
        Some(log_x)
    );
    let rotated = rotate_block_state(&registry, log_x, QuarterTurn::Cw90).unwrap();
    assert_eq!(property(&registry, rotated, "axis"), "z");
    assert_eq!(
        property(
            &registry,
            rotate_block_state(&registry, log_x, QuarterTurn::Cw180).unwrap(),
            "axis"
        ),
        "x"
    );
    assert_eq!(
        property(
            &registry,
            rotate_block_state(&registry, log_x, QuarterTurn::Cw270).unwrap(),
            "axis"
        ),
        "z"
    );

    let sign = state_of(&registry, "minecraft:oak_sign", &[("rotation", "3")]);
    assert_eq!(
        property(
            &registry,
            rotate_block_state(&registry, sign, QuarterTurn::Cw90).unwrap(),
            "rotation"
        ),
        "7"
    );
    assert_eq!(
        property(
            &registry,
            rotate_block_state(&registry, sign, QuarterTurn::Cw270).unwrap(),
            "rotation"
        ),
        "15"
    );
    assert_eq!(
        property(
            &registry,
            rotate_block_state(&registry, sign, QuarterTurn::Cw90).unwrap(),
            "rotation"
        ),
        "7"
    );

    let oriented = state_of(
        &registry,
        "minecraft:test_oriented",
        &[("orientation", "north_up")],
    );
    assert_eq!(
        property(
            &registry,
            rotate_block_state(&registry, oriented, QuarterTurn::Cw90).unwrap(),
            "orientation"
        ),
        "east_up"
    );
    let up_north = state_of(
        &registry,
        "minecraft:test_oriented",
        &[("orientation", "up_north")],
    );
    assert_eq!(
        property(
            &registry,
            rotate_block_state(&registry, up_north, QuarterTurn::Cw180).unwrap(),
            "orientation"
        ),
        "up_south"
    );

    let wire = state_of(
        &registry,
        "minecraft:test_wire",
        &[
            ("north", "true"),
            ("east", "false"),
            ("south", "false"),
            ("west", "true"),
        ],
    );
    let rotated = rotate_block_state(&registry, wire, QuarterTurn::Cw90).unwrap();
    assert_eq!(property(&registry, rotated, "north"), "true");
    assert_eq!(property(&registry, rotated, "east"), "true");
    assert_eq!(property(&registry, rotated, "south"), "false");
    assert_eq!(property(&registry, rotated, "west"), "false");
}

fn rotation_catalog() -> (BlockRegistry, Arc<Blueprint>) {
    let registry = registry(vec![
        ("minecraft:stone", Vec::new()),
        (
            "minecraft:oak_stairs",
            vec![
                ("facing", vec!["north", "east", "south", "west"]),
                ("half", vec!["bottom", "top"]),
                ("shape", vec!["straight"]),
            ],
        ),
        (
            "minecraft:oak_door",
            vec![
                ("facing", vec!["north", "east", "south", "west"]),
                ("hinge", vec!["left", "right"]),
                ("half", vec!["lower", "upper"]),
                ("open", vec!["false", "true"]),
                ("powered", vec!["false", "true"]),
            ],
        ),
        (
            "minecraft:red_bed",
            vec![
                ("facing", vec!["north", "east", "south", "west"]),
                ("part", vec!["foot", "head"]),
                ("occupied", vec!["false", "true"]),
            ],
        ),
    ]);
    let text = "id = \"solaris:cottage\"\nrevision = 1\n\
         [footprint]\nsize = [4, 5, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:oak_stairs\"\n\
         properties = { facing = \"north\", half = \"bottom\", shape = \"straight\" }\n\
         [[palette]]\nindex = 1\nblock = \"minecraft:oak_door\"\n\
         properties = { facing = \"north\", hinge = \"left\", half = \"lower\", open = \"false\", powered = \"false\" }\n\
         [[palette]]\nindex = 2\nblock = \"minecraft:oak_door\"\n\
         properties = { facing = \"north\", hinge = \"left\", half = \"upper\", open = \"false\", powered = \"false\" }\n\
         [[palette]]\nindex = 3\nblock = \"minecraft:red_bed\"\n\
         properties = { facing = \"north\", part = \"foot\", occupied = \"false\" }\n\
         [[palette]]\nindex = 4\nblock = \"minecraft:red_bed\"\n\
         properties = { facing = \"north\", part = \"head\", occupied = \"false\" }\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n\
         [[blocks]]\nx = 1\ny = 0\nz = 0\npalette = 1\n\
         [[blocks]]\nx = 1\ny = 1\nz = 0\npalette = 2\n\
         [[blocks]]\nx = 2\ny = 0\nz = 0\npalette = 3\n\
         [[blocks]]\nx = 2\ny = 0\nz = 1\npalette = 4\n";
    let files = vec![file("structures/cottage.toml", text.to_owned())];
    let catalog = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap();
    let blueprint = Arc::clone(catalog.get("solaris:cottage").unwrap());
    assert_eq!(blueprint.blocks().len(), 5);
    assert_eq!(blueprint.palette().len(), 5);
    (registry, blueprint)
}

#[test]
fn quarter_turns_rotate_stair_door_and_bed_groups() {
    let (registry, blueprint) = rotation_catalog();
    for (turn, expected) in [
        (QuarterTurn::None, "north"),
        (QuarterTurn::Cw90, "east"),
        (QuarterTurn::Cw180, "south"),
        (QuarterTurn::Cw270, "west"),
    ] {
        let instance = BlueprintInstance::new(Arc::clone(&blueprint), turn, [10, 20, 30]);
        let placed = instance.placed_blocks();
        assert_eq!(placed.len(), 5);
        let mut stair_facing = None;
        let mut door_facings = Vec::new();
        let mut door_halves = Vec::new();
        let mut bed_facings = Vec::new();
        let mut bed_parts = Vec::new();
        for block in &placed {
            let state = registry
                .by_id(block.state)
                .expect("rotated state resolves in the registry");
            match state.block.id.as_str() {
                "minecraft:oak_stairs" => {
                    stair_facing = Some(property(&registry, block.state, "facing"));
                    assert_eq!(property(&registry, block.state, "half"), "bottom");
                    assert_eq!(property(&registry, block.state, "shape"), "straight");
                }
                "minecraft:oak_door" => {
                    door_facings.push(property(&registry, block.state, "facing"));
                    door_halves.push(property(&registry, block.state, "half"));
                    assert_eq!(property(&registry, block.state, "hinge"), "left");
                }
                "minecraft:red_bed" => {
                    bed_facings.push(property(&registry, block.state, "facing"));
                    bed_parts.push(property(&registry, block.state, "part"));
                }
                other => panic!("unexpected block {other}"),
            }
        }
        assert_eq!(stair_facing.as_deref(), Some(expected));
        assert!(!door_facings.is_empty());
        assert!(
            door_facings
                .iter()
                .all(|facing| facing.as_str() == expected)
        );
        assert!(bed_facings.iter().all(|facing| facing.as_str() == expected));
        door_halves.sort();
        assert_eq!(door_halves, vec!["lower", "upper"]);
        bed_parts.sort();
        assert_eq!(bed_parts, vec!["foot", "head"]);
    }
}

#[test]
fn instances_project_pois_connections_and_hashes() {
    let registry = stone_registry();
    let text = "id = \"solaris:house\"\nrevision = 1\n\
         [footprint]\nsize = [4, 4, 4]\nanchor = [0, 0, 0]\n\
         [[palette]]\nindex = 0\nblock = \"minecraft:stone\"\nproperties = {}\n\
         [[blocks]]\nx = 0\ny = 0\nz = 0\npalette = 0\n\
         [[poi]]\nid = \"home\"\nkind = \"home\"\nat = [1, 1, 1]\ncapacity = 4\n\
         [[street_connection]]\nat = [0, 0, 0]\nfacing = \"north\"\n\
         [[block_entity]]\nat = [1, 1, 1]\nkind = \"bed\"\n\
         [[stage]]\nid = \"frame\"\nblocks = [ { x = 2, y = 0, z = 0, palette = 0 } ]\n";
    let files = vec![file("structures/house.toml", text.to_owned())];
    let catalog = BlueprintCatalog::from_files(&registry, "solaris", &files).unwrap();
    let blueprint = Arc::clone(catalog.get("solaris:house").unwrap());
    assert_eq!(blueprint.revision(), 1);
    assert_eq!(blueprint.variant_of(), None);
    assert_eq!(blueprint.stages().len(), 1);
    assert_eq!(blueprint.block_entities().len(), 1);

    let instance = BlueprintInstance::new(Arc::clone(&blueprint), QuarterTurn::Cw90, [100, 5, 200]);
    assert_eq!(instance.blueprint().id(), "solaris:house");
    assert_eq!(instance.turn(), QuarterTurn::Cw90);
    assert_eq!(instance.origin(), [100, 5, 200]);
    assert_eq!(
        instance.placed_blocks(),
        vec![crate::settlement_catalog::PlacedBlock {
            pos: [103, 5, 200],
            state: blueprint.palette()[0].state,
        }]
    );
    assert_eq!(
        instance.placed_stage_blocks(&blueprint.stages()[0]).len(),
        1
    );
    let pois = instance.placed_pois();
    assert_eq!(pois.len(), 1);
    assert_eq!(pois[0].at, [102, 6, 201]);
    assert_eq!(pois[0].kind, crate::settlement_catalog::PoiKind::Home);
    assert_eq!(
        instance.placed_street_connections(),
        vec![([103, 5, 200], crate::settlement_catalog::Cardinal::East)]
    );
    assert_eq!(instance.placed_block_entities()[0].at, [102, 6, 201]);

    let hash = instance.projection_hash();
    assert_eq!(hash.len(), 64);
    assert_eq!(
        hash,
        BlueprintInstance::new(Arc::clone(&blueprint), QuarterTurn::Cw90, [100, 5, 200])
            .projection_hash()
    );
    assert_ne!(
        hash,
        BlueprintInstance::new(Arc::clone(&blueprint), QuarterTurn::None, [100, 5, 200])
            .projection_hash()
    );

    // Palette entries expose canonical properties and a resolved state.
    let entry: &PaletteEntry = &blueprint.palette()[0];
    assert_eq!(entry.index, 0);
    assert_eq!(entry.block.as_str(), "minecraft:stone");
    assert!(entry.properties.is_empty());
    assert_eq!(registry.by_id(entry.state).unwrap().block.id, entry.block);
}
