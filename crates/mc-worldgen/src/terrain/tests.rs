use super::*;
use mc_data::worldgen_ores::{HeightAnchor, OreFeature, OrePlacementCount, OreTarget};
use mc_data::worldgen_structures::StructureSetFacts;
use mc_world::chunk::{MAX_Y, MIN_Y, OVERWORLD_GEOMETRY};
use std::collections::BTreeMap;

pub(in crate::terrain) fn tiny_registry() -> Arc<BlockRegistry> {
    use mc_data::blocks::{BlockReport, BlockStateReport};
    let report = vec![
        BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:bedrock").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 2,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 3,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:grass_block").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 4,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:sand").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 14,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:water").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 5,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lava").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 6,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 7,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:coal_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 8,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:iron_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 9,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:copper_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 10,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_coal_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 11,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_iron_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 12,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_copper_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 13,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:gold_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 15,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:redstone_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 16,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:diamond_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 17,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lapis_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 18,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:emerald_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 19,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_gold_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 20,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_redstone_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 21,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_diamond_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 22,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_lapis_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 23,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:deepslate_emerald_ore").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 24,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:stone_bricks").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 25,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_log").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 26,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_leaves").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 27,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:short_grass").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 28,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:dandelion").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 29,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:poppy").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 30,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:pumpkin").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 31,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:sugar_cane").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 32,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:cactus").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 33,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:red_sand").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 34,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:gravel").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 35,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:podzol").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 36,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:snow_block").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 37,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:birch_log").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 38,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:birch_leaves").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 39,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:seagrass").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 40,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:kelp").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 41,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:kelp_plant").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 42,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
    ];
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn dense_plains_village_rules(templates: Vec<StructureTemplate>) -> StructureRules {
    StructureRules::plains_village_markers(templates).with_structure_set_facts(&[
        StructureSetFacts {
            id: Identifier::parse("minecraft:test_villages").unwrap(),
            structures: vec![Identifier::parse("minecraft:village_plains").unwrap()],
            placement_type: None,
            spacing: Some(1),
            separation: Some(0),
            salt: None,
        },
    ])
}

fn registry_without_block(missing: &str) -> Arc<BlockRegistry> {
    use mc_data::blocks::{BlockReport, BlockStateReport};
    let names = [
        "minecraft:air",
        "minecraft:bedrock",
        "minecraft:stone",
        "minecraft:dirt",
        "minecraft:grass_block",
        "minecraft:iron_ore",
    ];
    let report = names
        .into_iter()
        .filter(|name| *name != missing)
        .enumerate()
        .map(|(id, name)| BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: u32::try_from(id).unwrap(),
                default: true,
                properties: BTreeMap::new(),
            }],
        })
        .collect::<Vec<_>>();
    Arc::new(BlockRegistry::from_report(&report).unwrap())
}

fn required_only_registry() -> Arc<BlockRegistry> {
    registry_without_block("minecraft:not_present")
}

fn land_biome_family(g: &TerrainGenerator, biome: &Identifier) -> Option<&'static str> {
    if g.biomes.is_surface_water(biome)
        || g.biomes.is_beach_or_shore(biome)
        || g.biomes.mountain.contains(biome)
        || g.biomes.cave.contains(biome)
    {
        None
    } else if g.biomes.swamp.contains(biome) {
        Some("swamp")
    } else if g.biomes.cold.contains(biome) {
        Some("cold")
    } else if g.biomes.hot_dry.contains(biome) {
        Some("hot_dry")
    } else if g.biomes.jungle.contains(biome) {
        Some("jungle")
    } else if g.biomes.temperate_forest.contains(biome) {
        Some("temperate_forest")
    } else if g.biomes.grassland.contains(biome) {
        Some("grassland")
    } else {
        None
    }
}

#[test]
fn generated_leaf_state_is_connected_but_not_persistent() {
    use mc_data::blocks::{BlockReport, BlockStateReport};

    let properties = BTreeMap::from([
        (
            "distance".to_string(),
            (1..=7).map(|value| value.to_string()).collect(),
        ),
        (
            "persistent".to_string(),
            vec!["true".to_string(), "false".to_string()],
        ),
        (
            "waterlogged".to_string(),
            vec!["true".to_string(), "false".to_string()],
        ),
    ]);
    let leaf_properties = |distance: &str| {
        BTreeMap::from([
            ("distance".to_string(), distance.to_string()),
            ("persistent".to_string(), "false".to_string()),
            ("waterlogged".to_string(), "false".to_string()),
        ])
    };
    let registry = BlockRegistry::from_report(&[BlockReport {
        id: Identifier::parse("minecraft:oak_leaves").unwrap(),
        properties,
        states: vec![
            BlockStateReport {
                id: 0,
                default: true,
                properties: leaf_properties("7"),
            },
            BlockStateReport {
                id: 1,
                default: false,
                properties: leaf_properties("1"),
            },
        ],
    }])
    .unwrap();

    assert_eq!(
        optional_generated_leaves(&registry, "minecraft:oak_leaves"),
        Some(BlockStateId(1))
    );
}

#[test]
fn try_with_rules_reports_missing_required_block() {
    let err = match TerrainGenerator::try_with_rules(
        42,
        registry_without_block("minecraft:grass_block"),
        BiomeRules::vanilla_overworld(),
        OreRules::new(Vec::new()).expect("empty ore rules fit the admission budget"),
    ) {
        Ok(_) => panic!("missing required terrain block must fail"),
        Err(err) => err,
    };

    assert_eq!(
        err,
        TerrainGeneratorError::MissingRequiredBlock {
            name: "minecraft:grass_block"
        }
    );
    assert_eq!(
        err.to_string(),
        "block registry missing required terrain block minecraft:grass_block"
    );
}

#[test]
fn try_with_rules_allows_missing_optional_blocks_when_required_resources_exist() {
    let registry = required_only_registry();
    let iron_ore = try_resolve_block(registry.as_ref(), "minecraft:iron_ore")
        .expect("required-only registry should include iron ore");
    let generator = TerrainGenerator::try_with_rules(
        42,
        registry,
        BiomeRules::vanilla_overworld(),
        OreRules::new(Vec::new()).expect("empty ore rules fit the admission budget"),
    )
    .expect("optional terrain blocks should use fallbacks");

    assert_eq!(generator.iron_ore, iron_ore);
    assert_eq!(generator.sand, generator.stone);
    assert_eq!(generator.red_sand, generator.stone);
    assert_eq!(generator.gravel, generator.stone);
    assert_eq!(generator.podzol, generator.stone);
    assert_eq!(generator.snow_block, generator.stone);
    assert_eq!(generator.deepslate, generator.stone);
    assert_eq!(generator.water, generator.air);
    assert_eq!(generator.decorations.oak_log, None);
}

#[test]
fn tellus_like_mode_preserves_default_but_changes_explicit_generator() {
    let registry = tiny_registry();
    let default = TerrainGenerator::new(1234, Arc::clone(&registry));
    let explicit_vanilla = TerrainGenerator::with_worldgen_mode(
        1234,
        Arc::clone(&registry),
        WorldgenMode::VanillaLike,
    );
    let tellus = TerrainGenerator::with_worldgen_mode(
        1234,
        registry,
        WorldgenMode::TellusLike(TellusWorldgenSettings::default()),
    );
    assert_eq!(
        default.surface_height(512, -256),
        explicit_vanilla.surface_height(512, -256)
    );
    assert!(
        (0..16).any(|sample| {
            let x = 512 + sample * 257;
            let z = -256 - sample * 193;
            default.surface_height(x, z) != tellus.surface_height(x, z)
        }),
        "Tellus mode should change at least one sampled height"
    );
    assert!((MIN_Y + 2..=250).contains(&tellus.surface_height(1_000_000, -1_000_000)));
}

#[test]
fn tellus_like_latitude_and_altitude_drive_climate() {
    let g = TerrainGenerator::with_worldgen_mode(
        77,
        tiny_registry(),
        WorldgenMode::TellusLike(TellusWorldgenSettings::default()),
    );
    let equator = g.tellus_climate(0, 72, 0);
    let arctic = g.tellus_climate(0, 72, -10_000_000);
    let summit = g.tellus_climate(0, 180, 0);
    assert!(equator.temperature > arctic.temperature);
    assert!(equator.temperature > summit.temperature);
    assert!(equator.latitude_degrees.abs() < arctic.latitude_degrees.abs());
}

#[test]
fn tellus_like_biomes_use_projected_climate_bands() {
    let settings = TellusWorldgenSettings::default();
    let g = TerrainGenerator::with_worldgen_mode(
        77,
        tiny_registry(),
        WorldgenMode::TellusLike(settings),
    );
    let sea = settings.sea_level;
    let equator_biome = g.biome_for(0, 0, sea + 24);
    let arctic_z = -10_000_000;
    let arctic_biome = (-4096..=4096)
        .step_by(256)
        .map(|x| g.biome_for(x, arctic_z, sea + 24))
        .find(|biome| g.biomes.cold.contains(biome))
        .expect("high-latitude Tellus sample should include a cold climate biome");

    assert!(
        !g.biomes.cold.contains(&equator_biome),
        "equatorial Tellus biome should not be cold: {equator_biome}"
    );
    assert!(
        g.biomes.cold.contains(&arctic_biome),
        "high-latitude Tellus biome should use cold climate band: {arctic_biome}"
    );
}

#[test]
fn tellus_like_water_uses_configured_sea_level() {
    let settings = TellusWorldgenSettings {
        sea_level: 120,
        ..TellusWorldgenSettings::default()
    };
    let g = TerrainGenerator::with_worldgen_mode(
        91,
        tiny_registry(),
        WorldgenMode::TellusLike(settings),
    );
    let (wx, wz, height) = (-60_000..=60_000)
        .step_by(2_000)
        .flat_map(|x| (-60_000..=60_000).step_by(2_000).map(move |z| (x, z)))
        .map(|(x, z)| (x, z, g.surface_height(x, z)))
        .find(|(_, _, height)| *height < settings.sea_level)
        .expect("Tellus sample should include below-sea terrain");
    let chunk = g.generate(ChunkPos {
        x: wx.div_euclid(16),
        z: wz.div_euclid(16),
    });
    let lx = wx.rem_euclid(16) as u8;
    let lz = wz.rem_euclid(16) as u8;

    assert!(height < settings.sea_level);
    assert_eq!(
        chunk.get_block(lx, settings.sea_level, lz),
        Some(BlockStateId(5))
    );
    assert_eq!(
        chunk.get_block(lx, settings.sea_level + 1, lz),
        Some(BlockStateId(0))
    );
}

#[test]
fn tellus_like_can_disable_water_fill_without_changing_vanilla_default() {
    let settings = TellusWorldgenSettings {
        water_enabled: false,
        sea_level: 120,
        ..TellusWorldgenSettings::default()
    };
    let g = TerrainGenerator::with_worldgen_mode(
        91,
        tiny_registry(),
        WorldgenMode::TellusLike(settings),
    );
    let (wx, wz, height) = (-60_000..=60_000)
        .step_by(2_000)
        .flat_map(|x| (-60_000..=60_000).step_by(2_000).map(move |z| (x, z)))
        .map(|(x, z)| (x, z, g.surface_height(x, z)))
        .find(|(_, _, height)| *height < settings.sea_level)
        .expect("Tellus sample should include below-sea terrain");
    let chunk = g.generate(ChunkPos {
        x: wx.div_euclid(16),
        z: wz.div_euclid(16),
    });
    let lx = wx.rem_euclid(16) as u8;
    let lz = wz.rem_euclid(16) as u8;
    let biome = g.biome_for(wx, wz, height);

    assert!(!g.biomes.is_surface_water(&biome));
    assert_eq!(chunk.get_block(lx, height + 1, lz), Some(BlockStateId(0)));
    assert_eq!(
        chunk.get_block(lx, settings.sea_level, lz),
        Some(BlockStateId(0))
    );
}

#[test]
fn tellus_like_keeps_local_terrain_smooth() {
    let settings = TellusWorldgenSettings::default();
    let g = TerrainGenerator::with_worldgen_mode(
        91,
        tiny_registry(),
        WorldgenMode::TellusLike(settings),
    );
    let mut total_step = 0i64;
    let mut samples = 0i64;
    let mut max_step = 0;

    for wx in (-256..=256).step_by(8) {
        for wz in (-256..=256).step_by(8) {
            if g.tellus_mountain_factor(wx, wz) > 0.05 {
                continue;
            }
            let h = g.surface_height(wx, wz);
            let hx = g.surface_height(wx + 1, wz);
            let hz = g.surface_height(wx, wz + 1);
            let step = (h - hx).abs().max((h - hz).abs());
            total_step += i64::from((h - hx).abs() + (h - hz).abs());
            samples += 2;
            max_step = max_step.max(step);
        }
    }

    let average_step = total_step as f64 / samples as f64;
    assert!(
        average_step <= 1.35,
        "Tellus average local step {average_step}"
    );
    assert!(max_step <= 5, "Tellus local non-mountain step {max_step}");
}

#[test]
fn tellus_like_mountains_are_rare_but_giant() {
    let settings = TellusWorldgenSettings::default();
    let g = TerrainGenerator::with_worldgen_mode(
        91,
        tiny_registry(),
        WorldgenMode::TellusLike(settings),
    );
    let mut samples = 0usize;
    let mut mountain_samples = 0usize;
    let mut max_height = i32::MIN;

    for wx in (-160_000..=160_000).step_by(4_096) {
        for wz in (-160_000..=160_000).step_by(4_096) {
            samples += 1;
            let height = g.surface_height(wx, wz);
            max_height = max_height.max(height);
            if g.tellus_mountain_factor(wx, wz) > 0.22 {
                mountain_samples += 1;
            }
        }
    }

    assert!(
        max_height >= settings.sea_level + 118,
        "Tellus sample should contain giant mountains; max height {max_height}"
    );
    assert!(
        mountain_samples > 0,
        "Tellus mountain mask should be reachable"
    );
    assert!(
        mountain_samples * 8 < samples,
        "Tellus mountains should stay rare: {mountain_samples}/{samples}"
    );
}

#[test]
fn generated_column_has_bedrock_and_biome_surface() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let chunk = g.generate(ChunkPos { x: 0, z: 0 });
    let air = BlockStateId(0);
    let bedrock = BlockStateId(1);
    let water = BlockStateId(5);

    // Bedrock at MIN_Y.
    assert_eq!(chunk.get_block(8, MIN_Y, 8), Some(bedrock));
    // Find the terrain surface. Biome selection decides whether it
    // is grassland, forest, cold, dry, mountain, or water/coast material.
    let height = g.surface_height(8, 8);
    assert_ne!(chunk.get_block(8, height, 8), Some(air));
    if height < SEA_LEVEL {
        assert_eq!(chunk.get_block(8, SEA_LEVEL, 8), Some(water));
        assert_eq!(chunk.get_block(8, SEA_LEVEL + 1, 8), Some(air));
    }

    // Heightmap value matches the height field.
    let hm = chunk.heightmaps.get("MOTION_BLOCKING").unwrap();
    assert_eq!(hm.get(8, 8), (height + 1 - MIN_Y) as u32);
    assert_eq!(chunk.highest_opaque_y(8, 8), Some(height));

    // Dirty flag set so M6 flush picks it up.
    assert!(chunk.dirty);
}

#[test]
fn generated_chunk_uses_explicit_geometry() {
    let geometry = mc_world::ChunkGeometry::new(0, 256).unwrap();
    let generator = TerrainGenerator::new(42, tiny_registry()).with_geometry(geometry);

    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

    assert_eq!(chunk.geometry(), geometry);
    assert_eq!(chunk.sections.len(), 16);
    assert_eq!(chunk.get_block(8, 0, 8), Some(generator.bedrock));
    let surface = generator.surface_height(8, 8);
    assert_eq!(
        chunk.heightmaps["MOTION_BLOCKING"].get(8, 8),
        (surface + 1) as u32
    );
}

#[test]
fn generated_chunk_handles_geometry_above_vanilla_surface_band() {
    let geometry = mc_world::ChunkGeometry::new(256, 256).unwrap();
    let generator = TerrainGenerator::new(42, tiny_registry()).with_geometry(geometry);

    let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

    assert_eq!(chunk.geometry(), geometry);
    assert_eq!(
        chunk.get_block(8, geometry.min_y(), 8),
        Some(generator.bedrock)
    );
    assert!(generator.surface_height(8, 8) >= geometry.min_y());
}

#[test]
fn chunk_geometry_typed_boundary_rejects_invalid_vertical_ranges() {
    assert_eq!(mc_world::ChunkGeometry::new(0, 0), None);
    assert_eq!(mc_world::ChunkGeometry::new(1, 16), None);
    assert_eq!(mc_world::ChunkGeometry::new(0, 15), None);
    assert_eq!(mc_world::ChunkGeometry::new(0, 512), None);
    assert_eq!(mc_world::ChunkGeometry::new(i32::MAX - 15, 16), None);
}

#[test]
fn generated_chunks_respect_short_and_tall_geometry_boundaries() {
    for geometry in [
        mc_world::ChunkGeometry::new(-16, 16).expect("one section"),
        mc_world::ChunkGeometry::new(-128, 496).expect("maximum section-aligned height"),
    ] {
        let generator = TerrainGenerator::new(42, tiny_registry()).with_geometry(geometry);
        let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

        assert_eq!(chunk.geometry(), geometry);
        assert_eq!(chunk.sections.len(), geometry.section_count());
        assert_eq!(
            chunk.get_block(8, geometry.min_y(), 8),
            Some(generator.bedrock)
        );
        assert_eq!(chunk.get_block(8, geometry.min_y() - 1, 8), None);
        assert_eq!(chunk.get_block(8, geometry.max_y(), 8), None);
        assert!(
            chunk.heightmaps["WORLD_SURFACE"].get(8, 8)
                <= u32::try_from(geometry.height()).unwrap()
        );
    }
}

#[test]
fn generated_chunks_handle_extreme_valid_geometry_without_y_overflow() {
    for (geometry, sea_level) in [
        (
            mc_world::ChunkGeometry::new(i32::MIN, 16).expect("lowest aligned section"),
            i32::MIN,
        ),
        (
            mc_world::ChunkGeometry::new(i32::MAX - 31, 16).expect("highest aligned section"),
            i32::MAX,
        ),
    ] {
        let settings = TellusWorldgenSettings {
            sea_level,
            ..TellusWorldgenSettings::default()
        };
        let generator = TerrainGenerator::with_worldgen_mode(
            42,
            tiny_registry(),
            WorldgenMode::TellusLike(settings),
        )
        .with_geometry(geometry);

        let chunk = generator.generate(ChunkPos { x: 0, z: 0 });

        assert_eq!(chunk.geometry(), geometry);
        assert_eq!(chunk.sections.len(), 1);
        assert_eq!(
            chunk.get_block(8, geometry.min_y(), 8),
            Some(generator.bedrock)
        );
        if let Some(below) = geometry.min_y().checked_sub(1) {
            assert_eq!(chunk.get_block(8, below, 8), None);
        }
        assert_eq!(chunk.get_block(8, geometry.max_y(), 8), None);
    }
}

fn generated_block_fingerprint(generator: &TerrainGenerator, positions: &[ChunkPos]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &pos in positions {
        let chunk = generator.generate(pos);
        for value in [
            pos.x,
            pos.z,
            chunk.geometry().min_y(),
            chunk.geometry().max_y(),
        ] {
            for byte in value.to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
        for y in chunk.geometry().min_y()..chunk.geometry().max_y() {
            for z in 0..16u8 {
                for x in 0..16u8 {
                    let state = chunk.get_block(x, y, z).expect("Y is inside geometry");
                    for byte in state.0.to_le_bytes() {
                        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
                    }
                }
            }
        }
    }
    hash
}

fn generated_serialized_fingerprint(
    generator: &TerrainGenerator,
    registry: &BlockRegistry,
    items: &mc_data::items::ItemRegistry,
    positions: &[ChunkPos],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut hash_bytes = |bytes: &[u8]| {
        for &byte in bytes {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    for &pos in positions {
        let chunk = generator.generate(pos);
        let payload =
            mc_world::anvil::chunk_to_payload_with_items(&chunk, registry, Some(items), 0)
                .expect("generated chunk serializes");
        hash_bytes(&[payload.local_x, payload.local_z]);
        hash_bytes(&payload.uncompressed_nbt);
        for word in chunk.highest_opaque.to_long_array() {
            hash_bytes(&word.to_le_bytes());
        }
    }
    hash
}

#[test]
fn explicit_overworld_geometry_preserves_deterministic_serialized_chunk_output() {
    let positions = [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: 4, z: 0 },
        ChunkPos { x: 5, z: -3 },
    ];
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded block registry"),
    );
    let items = mc_data::items::solaris_required_items();
    let structures = StructureRules::solaris_playable_ruin(registry.as_ref(), &items)
        .expect("playable ruin resolves embedded data");
    let default =
        TerrainGenerator::new(0, Arc::clone(&registry)).with_structures(structures.clone());
    let explicit = TerrainGenerator::new(0, Arc::clone(&registry))
        .with_geometry(OVERWORLD_GEOMETRY)
        .with_structures(structures);
    let default_fingerprint =
        generated_serialized_fingerprint(&default, registry.as_ref(), &items, &positions);

    assert!(!default.generate(ChunkPos { x: 4, z: 0 }).chests.is_empty());

    assert_eq!(
        generated_serialized_fingerprint(&explicit, registry.as_ref(), &items, &positions),
        default_fingerprint
    );
}

#[test]
fn custom_geometry_generation_is_deterministic_for_the_same_seed() {
    let geometry = mc_world::ChunkGeometry::new(0, 32).expect("two sections");
    let positions = [ChunkPos { x: -2, z: 3 }, ChunkPos { x: 7, z: -5 }];
    let first = TerrainGenerator::new(99, tiny_registry()).with_geometry(geometry);
    let second = TerrainGenerator::new(99, tiny_registry()).with_geometry(geometry);

    assert_eq!(
        generated_block_fingerprint(&first, &positions),
        generated_block_fingerprint(&second, &positions)
    );
}

#[test]
fn determinism_across_repeated_generate_calls() {
    let g = TerrainGenerator::new(99, tiny_registry());
    let a = g.generate(ChunkPos { x: 5, z: -3 });
    let b = g.generate(ChunkPos { x: 5, z: -3 });
    for y in MIN_Y..=80 {
        for x in 0..16u8 {
            for z in 0..16u8 {
                assert_eq!(a.get_block(x, y, z), b.get_block(x, y, z));
            }
        }
    }
}

#[test]
fn different_seeds_change_generated_chunks() {
    let a = TerrainGenerator::new(0, tiny_registry());
    let b = TerrainGenerator::new(1, tiny_registry());
    let positions = [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: 5, z: -3 },
        ChunkPos { x: -12, z: 8 },
    ];

    for pos in positions {
        let chunk_a = a.generate(pos);
        let chunk_b = b.generate(pos);
        for y in MIN_Y..=96 {
            for x in 0..16u8 {
                for z in 0..16u8 {
                    if chunk_a.get_block(x, y, z) != chunk_b.get_block(x, y, z) {
                        return;
                    }
                }
            }
        }
    }

    panic!("different world seeds should alter at least one sampled generated chunk");
}

#[test]
fn persisted_chunk_edit_wins_after_seed_change() {
    let registry = tiny_registry();
    let generator_a = Arc::new(TerrainGenerator::new(0, Arc::clone(&registry)));
    let generator_b = Arc::new(TerrainGenerator::new(1, Arc::clone(&registry)));
    let cpos = ChunkPos { x: 5, z: -3 };
    let world_x = cpos.x * 16 + 8;
    let world_z = cpos.z * 16 + 8;
    let edit_pos = mc_world::chunk::BlockPos {
        x: world_x,
        y: generator_a.surface_height(world_x, world_z) + 1,
        z: world_z,
    };
    let marker = BlockStateId(25);
    let root = unique_temp_world_dir();
    std::fs::create_dir_all(root.join("region")).unwrap();

    let mut storage = mc_world::WorldStorage::open(&root, Arc::clone(&registry))
        .unwrap()
        .with_generator(Arc::clone(&generator_a) as Arc<dyn ChunkGenerator>);
    assert_ne!(
        storage.get_block(edit_pos).unwrap(),
        Some(marker),
        "generated fallback should not already contain the edit marker"
    );
    storage.set_block_at(edit_pos, marker).unwrap().unwrap();
    assert!(storage.flush_dirty().unwrap() >= 1);
    drop(storage);

    let mut reopened = mc_world::WorldStorage::open(&root, Arc::clone(&registry))
        .unwrap()
        .with_generator(Arc::clone(&generator_b) as Arc<dyn ChunkGenerator>);
    assert_eq!(reopened.get_block(edit_pos).unwrap(), Some(marker));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn far_chunks_still_have_terrain() {
    let g = TerrainGenerator::new(1234, tiny_registry());
    let chunk = g.generate(ChunkPos {
        x: 1_000,
        z: -1_000,
    });
    let height = g.surface_height(1_000 * 16 + 8, -1_000 * 16 + 8);
    let biome = g.biome_for(1_000 * 16 + 8, -1_000 * 16 + 8, height);
    let (surface, _) = g.surface_materials(&biome);
    assert!(matches!(chunk.get_block(8, height, 8), Some(state) if state == surface));
    assert_eq!(chunk.status, "minecraft:full");
}

#[test]
fn default_seed_origin_remains_land_spawn() {
    let g = TerrainGenerator::new(0, tiny_registry());
    let height = g.surface_height(0, 0);
    let biome = g.biome_for(0, 0, height);

    assert!(height > SEA_LEVEL, "spawn origin should not be underwater");
    assert!(!g.biomes.is_ocean(&biome));
}

#[test]
fn continental_mask_produces_water_coasts_and_land_biomes() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut saw_ocean = false;
    let mut saw_beach = false;
    let mut saw_forest_or_jungle = false;
    let mut saw_grass_or_dry = false;
    let mut saw_mountain = false;
    let mut ocean_column = None;

    for wx in (-4096..=4096).step_by(64) {
        for wz in (-4096..=4096).step_by(64) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            if g.biomes.is_ocean(&biome) {
                saw_ocean = true;
                ocean_column.get_or_insert((wx, wz, height));
            } else if g.biomes.is_beach_or_shore(&biome) {
                saw_beach = true;
            } else if g.biomes.temperate_forest.contains(&biome) || g.biomes.jungle.contains(&biome)
            {
                saw_forest_or_jungle = true;
            } else if g.biomes.grassland.contains(&biome) || g.biomes.hot_dry.contains(&biome) {
                saw_grass_or_dry = true;
            } else if g.biomes.mountain.contains(&biome) {
                saw_mountain = true;
            }
        }
    }

    assert!(saw_ocean, "expected ocean cells in the sampled area");
    assert!(saw_beach, "expected beach cells around coastlines");
    assert!(
        saw_forest_or_jungle,
        "expected forest/jungle cells in the sampled area"
    );
    assert!(
        saw_grass_or_dry,
        "expected grassland/dry cells in the sampled area"
    );
    assert!(saw_mountain, "expected mountain cells in the sampled area");

    let (wx, wz, height) = ocean_column.unwrap();
    let chunk = g.generate(ChunkPos {
        x: wx.div_euclid(16),
        z: wz.div_euclid(16),
    });
    let lx = wx.rem_euclid(16) as u8;
    let lz = wz.rem_euclid(16) as u8;
    assert!(height < SEA_LEVEL);
    assert_eq!(chunk.get_block(lx, SEA_LEVEL, lz), Some(BlockStateId(5)));
    assert_eq!(
        chunk.get_block(lx, SEA_LEVEL + 1, lz),
        Some(BlockStateId(0))
    );
}

#[test]
fn default_seed_coastline_has_bounded_steps() {
    let g = TerrainGenerator::new(0, tiny_registry());
    for wx in (-512..=512).step_by(8) {
        for wz in (-512..=512).step_by(8) {
            let h = g.surface_height(wx, wz);
            let hx = g.surface_height(wx + 1, wz);
            let hz = g.surface_height(wx, wz + 1);
            let near_coast = (h - SEA_LEVEL).abs() <= 12
                || (hx - SEA_LEVEL).abs() <= 12
                || (hz - SEA_LEVEL).abs() <= 12;
            if near_coast {
                assert!(
                    (h - hx).abs() <= 6,
                    "sharp x coastline step at ({wx}, {wz}): {h} -> {hx}"
                );
                assert!(
                    (h - hz).abs() <= 6,
                    "sharp z coastline step at ({wx}, {wz}): {h} -> {hz}"
                );
            }
        }
    }
}

#[test]
fn solaris_owned_land_biomes_form_chunk_scale_regions() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut stable_windows = 0usize;

    for wx in (-1_536..=1_536).step_by(64) {
        for wz in (-1_536..=1_536).step_by(64) {
            let mut expected = None;
            let mut stable = true;
            for dx in [0, 16, 32, 48] {
                for dz in [0, 16, 32, 48] {
                    let x = wx + dx;
                    let z = wz + dz;
                    let height = g.surface_height(x, z);
                    if height <= SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA + 2 {
                        stable = false;
                        continue;
                    }
                    let biome = g.biome_for(x, z, height);
                    if land_biome_family(&g, &biome).is_none() {
                        stable = false;
                        continue;
                    }
                    match &expected {
                        Some(expected) if expected != &biome => stable = false,
                        None => expected = Some(biome),
                        _ => {}
                    }
                }
            }
            if stable && expected.is_some() {
                stable_windows += 1;
            }
        }
    }

    assert!(
        stable_windows >= 12,
        "expected multiple 4x4-chunk land windows to keep one exact biome, saw {stable_windows}"
    );
}

#[test]
fn solaris_owned_biome_choices_do_not_track_192_block_grid_lines() {
    const OLD_GRID_SIZE: i32 = 192;

    let g = TerrainGenerator::new(42, tiny_registry());
    let mut aligned_pairs = 0usize;
    let mut aligned_flips = 0usize;
    let mut control_pairs = 0usize;
    let mut control_flips = 0usize;

    for grid_x in (-1_536..=1_536).step_by(OLD_GRID_SIZE as usize) {
        for wz in (-2_048..=2_048).step_by(16) {
            for (left, right, pairs, flips) in [
                (
                    grid_x - 4,
                    grid_x + 4,
                    &mut aligned_pairs,
                    &mut aligned_flips,
                ),
                (
                    grid_x + 72,
                    grid_x + 80,
                    &mut control_pairs,
                    &mut control_flips,
                ),
            ] {
                let left_height = g.surface_height(left, wz);
                let right_height = g.surface_height(right, wz);
                let left_biome = g.biome_for(left, wz, left_height);
                let right_biome = g.biome_for(right, wz, right_height);
                let Some(left_family) = land_biome_family(&g, &left_biome) else {
                    continue;
                };
                let Some(right_family) = land_biome_family(&g, &right_biome) else {
                    continue;
                };
                if left_family != right_family {
                    continue;
                }
                *pairs += 1;
                if left_biome != right_biome {
                    *flips += 1;
                }
            }
        }
    }

    assert!(
        aligned_pairs > 150,
        "sample should include enough grid-line land pairs"
    );
    assert!(
        control_pairs > 150,
        "sample should include enough control land pairs"
    );
    assert!(
        aligned_flips * control_pairs
            <= (control_flips * aligned_pairs) + (aligned_pairs * control_pairs / 50),
        "biome flips should not concentrate on 192-block grid lines: aligned {aligned_flips}/{aligned_pairs}, control {control_flips}/{control_pairs}"
    );
}

#[test]
fn solaris_owned_nearby_land_samples_avoid_excessive_family_flips() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut comparable = 0usize;
    let mut flips = 0usize;

    for wx in (-1_024..=1_024).step_by(16) {
        for wz in (-1_024..=1_024).step_by(16) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            let Some(family) = land_biome_family(&g, &biome) else {
                continue;
            };
            for (nx, nz) in [(wx + 16, wz), (wx, wz + 16)] {
                let nheight = g.surface_height(nx, nz);
                let nbiome = g.biome_for(nx, nz, nheight);
                let Some(nfamily) = land_biome_family(&g, &nbiome) else {
                    continue;
                };
                comparable += 1;
                if family != nfamily {
                    flips += 1;
                }
            }
        }
    }

    assert!(comparable > 200, "sample should include enough land pairs");
    assert!(
        flips * 5 <= comparable,
        "too many 16-block land biome family flips: {flips}/{comparable}"
    );
}

#[test]
fn solaris_owned_river_masks_are_broad_when_reachable() {
    let g = TerrainGenerator::new(42, tiny_registry());

    for wx in (-2_048..=2_048).step_by(4) {
        for wz in (-2_048..=2_048).step_by(4) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                continue;
            }

            let mut width_x = 1usize;
            for dx in 1..=32 {
                let height = g.surface_height(wx + dx, wz);
                let biome = g.biome_for(wx + dx, wz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_x += 1;
            }
            for dx in 1..=32 {
                let height = g.surface_height(wx - dx, wz);
                let biome = g.biome_for(wx - dx, wz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_x += 1;
            }
            let mut width_z = 1usize;
            for dz in 1..=32 {
                let height = g.surface_height(wx, wz + dz);
                let biome = g.biome_for(wx, wz + dz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_z += 1;
            }
            for dz in 1..=32 {
                let height = g.surface_height(wx, wz - dz);
                let biome = g.biome_for(wx, wz - dz, height);
                if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                    break;
                }
                width_z += 1;
            }
            if width_x < 8 || width_z < 8 {
                continue;
            }

            let mut nearby_river_water = 0usize;
            for dx in -4..=4 {
                for dz in -4..=4 {
                    let height = g.surface_height(wx + dx, wz + dz);
                    let biome = g.biome_for(wx + dx, wz + dz, height);
                    if g.biomes.is_river(&biome) && height <= SEA_LEVEL {
                        nearby_river_water += 1;
                    }
                }
            }
            assert!(
                nearby_river_water >= 24,
                "river should occupy a broad local cross-section near ({wx}, {wz}), saw {nearby_river_water}/81 water-level river columns"
            );

            let chunk = g.generate(ChunkPos {
                x: wx.div_euclid(16),
                z: wz.div_euclid(16),
            });
            assert_eq!(
                chunk.get_block(wx.rem_euclid(16) as u8, SEA_LEVEL, wz.rem_euclid(16) as u8),
                Some(BlockStateId(5)),
                "river centre at ({wx}, {wz}) with surface {}",
                g.surface_height(wx, wz)
            );
            return;
        }
    }

    panic!("sampled area should contain a reachable broad river cross-section");
}

#[test]
fn solaris_owned_river_valleys_keep_continuous_water_floors() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut checked = 0usize;

    for wx in (-2_048..=2_048).step_by(4) {
        for wz in (-2_048..=2_048).step_by(4) {
            let height = g.surface_height(wx, wz);
            let biome = g.biome_for(wx, wz, height);
            if !g.biomes.is_river(&biome) || height > SEA_LEVEL {
                continue;
            }

            for (axis_x, axis_z) in [(1, 0), (0, 1)] {
                let mut wet_floor = true;
                let mut center_height = i32::MAX;
                for offset in [-12, -8, -4, 0, 4, 8, 12] {
                    let x = wx + axis_x * offset;
                    let z = wz + axis_z * offset;
                    let sample_height = g.surface_height(x, z);
                    let sample_biome = g.biome_for(x, z, sample_height);
                    center_height = center_height.min(sample_height);
                    if !g.biomes.is_river(&sample_biome) || sample_height > SEA_LEVEL {
                        wet_floor = false;
                        break;
                    }
                }
                if !wet_floor {
                    continue;
                }

                let mut previous_height = None;
                for offset in (-28..=28).step_by(4) {
                    let x = wx + axis_x * offset;
                    let z = wz + axis_z * offset;
                    let sample_height = g.surface_height(x, z);
                    if let Some(previous_height) = previous_height {
                        let step: i32 = sample_height - previous_height;
                        assert!(
                            step.abs() <= 10,
                            "river cross-section should rise gradually near ({wx}, {wz}) on axis ({axis_x}, {axis_z}): offset {offset}, step {step}"
                        );
                    }
                    previous_height = Some(sample_height);
                }

                for offset in [-28, 28] {
                    let x = wx + axis_x * offset;
                    let z = wz + axis_z * offset;
                    let bank_height = g.surface_height(x, z);
                    assert!(
                        bank_height >= center_height,
                        "river bank should not undercut wet floor near ({wx}, {wz}) on axis ({axis_x}, {axis_z}): center {center_height}, bank {bank_height}"
                    );
                }
                checked += 1;
                if checked >= 6 {
                    return;
                }
            }
        }
    }

    assert!(
        checked >= 3,
        "sampled area should contain several continuous river water-floor cross-sections, saw {checked}"
    );
}

#[test]
fn solaris_owned_local_terrain_steps_stay_bounded() {
    let g = TerrainGenerator::new(42, tiny_registry());
    for wx in (-512..=512).step_by(4) {
        for wz in (-512..=512).step_by(4) {
            let h = g.surface_height(wx, wz);
            let hx = g.surface_height(wx + 4, wz);
            let hz = g.surface_height(wx, wz + 4);
            assert!(
                (h - hx).abs() <= 10,
                "sharp x terrain step at ({wx}, {wz}): {h} -> {hx}"
            );
            assert!(
                (h - hz).abs() <= 10,
                "sharp z terrain step at ({wx}, {wz}): {h} -> {hz}"
            );
        }
    }
}

#[test]
fn generated_chunk_biomes_are_not_all_default() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut saw_non_default = false;
    for cx in -8..=8 {
        for cz in -8..=8 {
            let chunk = g.generate(ChunkPos { x: cx, z: cz });
            for section in &chunk.biomes {
                match section {
                    BiomeSection::Single(id) => {
                        saw_non_default |= id.as_str() != "minecraft:plains";
                    }
                    BiomeSection::Indirect { palette, .. } => {
                        saw_non_default |=
                            palette.iter().any(|id| id.as_str() != "minecraft:plains");
                    }
                }
            }
        }
    }
    assert!(saw_non_default, "generated chunks should carry biome data");
}

#[test]
fn ocean_chunks_can_contain_underwater_vegetation_columns() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let mut saw_seagrass = false;
    for cx in -32..=32 {
        for cz in -32..=32 {
            let chunk = g.generate(ChunkPos { x: cx, z: cz });
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = cx * 16 + lx as i32;
                    let wz = cz * 16 + lz as i32;
                    let height = g.surface_height(wx, wz);
                    let biome = g.biome_for(wx, wz, height);
                    if g.biomes.is_ocean(&biome)
                        && matches!(
                            chunk.get_block(lx, height + 1, lz),
                            Some(BlockStateId(40 | 41))
                        )
                    {
                        assert_eq!(chunk.get_block(lx, SEA_LEVEL, lz), Some(BlockStateId(5)));
                        saw_seagrass = true;
                    }
                    if g.biomes.is_ocean(&biome)
                        && chunk.get_block(lx, height + 1, lz) == Some(BlockStateId(42))
                    {
                        let mut y = height + 1;
                        while chunk.get_block(lx, y, lz) == Some(BlockStateId(42)) {
                            y += 1;
                        }
                        assert_eq!(chunk.get_block(lx, y, lz), Some(BlockStateId(41)));
                        assert!(y > height + 1, "kelp should be a visible column");
                        return;
                    }
                }
            }
        }
    }
    assert!(saw_seagrass, "sampled ocean chunks should contain seagrass");
    panic!("sampled ocean chunks should contain kelp columns");
}

#[test]
fn biome_surface_rules_are_visibly_distinct() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let cases = [
        ("minecraft:plains", BlockStateId(4), BlockStateId(3)),
        ("minecraft:birch_forest", BlockStateId(4), BlockStateId(3)),
        ("minecraft:badlands", BlockStateId(34), BlockStateId(34)),
        ("minecraft:desert", BlockStateId(14), BlockStateId(14)),
        ("minecraft:jagged_peaks", BlockStateId(35), BlockStateId(2)),
        ("minecraft:snowy_plains", BlockStateId(37), BlockStateId(3)),
        ("minecraft:beach", BlockStateId(14), BlockStateId(14)),
    ];

    for (biome, surface, fill) in cases {
        let biome = Identifier::parse(biome).unwrap();
        assert_eq!(g.surface_materials(&biome), (surface, fill));
    }
}

#[test]
fn surface_decorations_are_visible_and_refresh_heightmaps() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let decorations = [
        BlockStateId(26),
        BlockStateId(27),
        BlockStateId(28),
        BlockStateId(29),
        BlockStateId(30),
        BlockStateId(31),
        BlockStateId(32),
        BlockStateId(33),
    ];
    let mut saw_decoration = false;

    for cx in -2..=2 {
        for cz in -2..=2 {
            let chunk = g.generate(ChunkPos { x: cx, z: cz });
            let again = g.generate(ChunkPos { x: cx, z: cz });
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = cx * 16 + lx as i32;
                    let wz = cz * 16 + lz as i32;
                    let height = g.surface_height(wx, wz);
                    for y in (height + 1)..=(height + 8).min(MAX_Y - 1) {
                        assert_eq!(chunk.get_block(lx, y, lz), again.get_block(lx, y, lz));
                        if chunk
                            .get_block(lx, y, lz)
                            .is_some_and(|state| decorations.contains(&state))
                        {
                            saw_decoration = true;
                            assert!(chunk.highest_opaque_y(lx, lz).is_some_and(|top| top >= y));
                            assert!(
                                chunk.heightmaps["WORLD_SURFACE"].get(lx, lz)
                                    >= (y + 1 - MIN_Y) as u32
                            );
                        }
                    }
                }
            }
        }
    }

    assert!(
        saw_decoration,
        "sampled chunks should contain surface decorations"
    );
}

#[test]
fn generated_overlays_survive_flush_and_reopen() {
    let registry = tiny_registry();
    let marker = BlockStateId(25);
    let template = StructureTemplate::new(
        [1, 1, 1],
        vec![crate::structures::TemplateBlock {
            pos: [0, 0, 0],
            state: marker,
        }],
    );
    let structures = dense_plains_village_rules(vec![template]);
    let generator =
        Arc::new(TerrainGenerator::new(42, Arc::clone(&registry)).with_structures(structures));
    let mut structure_target = None;
    'cells: for gx in -64..=64 {
        for gz in -64..=64 {
            let Some((_template, center_x, center_z)) = generator.structure_plan(gx, gz) else {
                continue;
            };
            let height = generator.surface_height(center_x, center_z);
            let biome = generator.biome_for(center_x, center_z, height);
            if height > SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA
                && generator.biomes.grassland.contains(&biome)
            {
                structure_target = Some((center_x, height + 1, center_z));
                break 'cells;
            }
        }
    }
    let (structure_x, structure_y, structure_z) = structure_target.expect("structure target");

    let root = unique_temp_world_dir();
    std::fs::create_dir_all(root.join("region")).unwrap();
    let mut storage = mc_world::WorldStorage::open(&root, Arc::clone(&registry))
        .unwrap()
        .with_generator(Arc::clone(&generator) as Arc<dyn ChunkGenerator>);
    let structure_pos = ChunkPos {
        x: structure_x.div_euclid(16),
        z: structure_z.div_euclid(16),
    };
    let structure_lx = structure_x.rem_euclid(16) as u8;
    let structure_lz = structure_z.rem_euclid(16) as u8;
    let chunk = storage
        .get_chunk(structure_pos)
        .unwrap()
        .expect("generated structure chunk");
    assert_eq!(
        chunk.get_block(structure_lx, structure_y, structure_lz),
        Some(marker)
    );

    let (decor_pos, decor_lx, decor_y, decor_lz, decor_state) =
        find_decoration_in_storage(&mut storage, &generator);
    assert!(storage.dirty_count() >= 1);
    assert!(storage.flush_dirty().unwrap() >= 1);
    drop(storage);

    let mut fresh = mc_world::WorldStorage::open(&root, Arc::clone(&registry)).unwrap();
    let structure_chunk = fresh
        .get_chunk(structure_pos)
        .unwrap()
        .expect("reopened structure chunk");
    assert_eq!(
        structure_chunk.get_block(structure_lx, structure_y, structure_lz),
        Some(marker)
    );
    assert!(
        structure_chunk
            .highest_opaque_y(structure_lx, structure_lz)
            .is_some_and(|top| top >= structure_y)
    );

    let decoration_chunk = fresh
        .get_chunk(decor_pos)
        .unwrap()
        .expect("reopened decoration chunk");
    assert_eq!(
        decoration_chunk.get_block(decor_lx, decor_y, decor_lz),
        Some(decor_state)
    );
    assert!(
        decoration_chunk
            .highest_opaque_y(decor_lx, decor_lz)
            .is_some_and(|top| top >= decor_y)
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn seed_zero_playable_ruin_chest_stays_empty_after_flush_and_reopen() {
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let generator = Arc::new(
        TerrainGenerator::new(0, Arc::clone(&registry)).with_structures(
            StructureRules::solaris_playable_ruin(&registry, &items)
                .expect("playable ruin resolves embedded data"),
        ),
    );
    let root = unique_temp_world_dir();
    std::fs::create_dir_all(root.join("region")).unwrap();
    let ruin_chunk = ChunkPos { x: 4, z: 0 };

    let mut storage = mc_world::WorldStorage::open(&root, Arc::clone(&registry))
        .unwrap()
        .with_item_registry(Arc::clone(&items))
        .with_generator(Arc::clone(&generator) as Arc<dyn ChunkGenerator>);
    let chest_pos = storage
        .get_chunk(ruin_chunk)
        .unwrap()
        .expect("generated ruin chunk")
        .chests
        .keys()
        .next()
        .copied()
        .expect("generated ruin chest");
    assert!(
        storage
            .chest_block_entity(chest_pos)
            .unwrap()
            .expect("generated chest entity")
            .slots
            .iter()
            .any(|slot| !slot.is_empty())
    );

    storage
        .set_chest_block_entity(chest_pos, mc_world::ChestBlockEntity::default())
        .expect("empty chest after loot");
    assert!(storage.flush_dirty().unwrap() >= 1);
    drop(storage);

    let mut reopened = mc_world::WorldStorage::open(&root, registry)
        .unwrap()
        .with_item_registry(items);
    let chest = reopened
        .chest_block_entity(chest_pos)
        .unwrap()
        .expect("persisted chest entity");
    assert!(chest.slots.iter().all(mc_world::FurnaceSlot::is_empty));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn playable_ruin_does_not_insert_chest_entity_above_chunk_geometry() {
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded block registry"),
    );
    let items = mc_data::items::solaris_required_items();
    let geometry = mc_world::ChunkGeometry::new(0, 16).expect("single-section geometry");
    let generator = TerrainGenerator::new(0, Arc::clone(&registry))
        .with_geometry(geometry)
        .with_structures(
            StructureRules::solaris_playable_ruin(&registry, &items)
                .expect("playable ruin resolves embedded data"),
        );

    let chunk = generator.generate(ChunkPos { x: 4, z: 0 });

    assert!(chunk.chests.is_empty());
}

fn unique_temp_world_dir() -> std::path::PathBuf {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("solaris-worldgen-{suffix}"))
}

fn find_decoration_in_storage(
    storage: &mut mc_world::WorldStorage,
    generator: &TerrainGenerator,
) -> (ChunkPos, u8, i32, u8, BlockStateId) {
    let decorations = [
        BlockStateId(26),
        BlockStateId(27),
        BlockStateId(28),
        BlockStateId(29),
        BlockStateId(30),
        BlockStateId(31),
        BlockStateId(32),
        BlockStateId(33),
    ];
    for cx in -2..=2 {
        for cz in -2..=2 {
            let pos = ChunkPos { x: cx, z: cz };
            let chunk = storage.get_chunk(pos).unwrap().expect("generated chunk");
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = cx * 16 + lx as i32;
                    let wz = cz * 16 + lz as i32;
                    let height = generator.surface_height(wx, wz);
                    for y in (height + 1)..=(height + 8).min(MAX_Y - 1) {
                        if let Some(state) = chunk.get_block(lx, y, lz)
                            && decorations.contains(&state)
                        {
                            return (pos, lx, y, lz, state);
                        }
                    }
                }
            }
        }
    }
    panic!("sampled chunks should contain a decoration");
}

#[test]
fn structure_rules_paste_intersecting_template_blocks() {
    let marker = BlockStateId(25);
    let template = StructureTemplate::new(
        [1, 1, 1],
        vec![crate::structures::TemplateBlock {
            pos: [0, 0, 0],
            state: marker,
        }],
    );
    let structures = dense_plains_village_rules(vec![template]);
    let g = TerrainGenerator::new(42, tiny_registry()).with_structures(structures);

    let mut target = None;
    'cells: for gx in -64..=64 {
        for gz in -64..=64 {
            let Some((_template, center_x, center_z)) = g.structure_plan(gx, gz) else {
                continue;
            };
            let height = g.surface_height(center_x, center_z);
            let biome = g.biome_for(center_x, center_z, height);
            if height > SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA && g.biomes.grassland.contains(&biome) {
                target = Some((center_x, center_z, height + 1));
                break 'cells;
            }
        }
    }
    let (wx, wz, y) = target.expect("dense structure grid should find a land grassland cell");
    let chunk = g.generate(ChunkPos {
        x: wx.div_euclid(16),
        z: wz.div_euclid(16),
    });
    let lx = wx.rem_euclid(16) as u8;
    let lz = wz.rem_euclid(16) as u8;

    assert_eq!(chunk.get_block(lx, y, lz), Some(marker));
    assert_eq!(
        chunk.heightmaps["WORLD_SURFACE"].get(lx, lz),
        (y + 1 - MIN_Y) as u32
    );
}

#[test]
fn structure_paste_clips_blocks_and_chests_to_chunk_geometry() {
    let geometry = mc_world::ChunkGeometry::new(0, 16).expect("one section");
    let marker = BlockStateId(25);
    let positions = [[0, -2, 0], [0, -1, 0], [0, 14, 0], [0, 15, 0]];
    let template = StructureTemplate::new(
        [1, 18, 1],
        positions
            .into_iter()
            .map(|pos| crate::structures::TemplateBlock { pos, state: marker })
            .collect(),
    )
    .with_chests(
        positions
            .into_iter()
            .map(|pos| crate::structures::TemplateChest {
                pos,
                chest: mc_world::ChestBlockEntity::default(),
            })
            .collect(),
    );
    let mut chunk = Chunk::empty_with_geometry(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
        geometry,
    );
    let mut touched = [false; 256];

    paste_template(&mut chunk, &template, 0, 1, 0, &mut touched);

    assert_eq!(chunk.get_block(0, geometry.min_y(), 0), Some(marker));
    assert_eq!(chunk.get_block(0, geometry.max_y() - 1, 0), Some(marker));
    assert_eq!(chunk.chests.len(), 2);
    assert!(
        chunk
            .chests
            .contains_key(&mc_world::BlockPos { x: 0, y: 0, z: 0 })
    );
    assert!(
        chunk
            .chests
            .contains_key(&mc_world::BlockPos { x: 0, y: 15, z: 0 })
    );
    assert!(touched[0]);
}

#[test]
fn structure_paste_ignores_overflowing_vertical_offsets() {
    let geometry = mc_world::ChunkGeometry::new(0, 16).expect("one section");
    let marker = BlockStateId(25);
    let positions = [[0, i32::MAX, 0], [0, i32::MIN, 0]];
    let template = StructureTemplate::new(
        [1, 1, 1],
        positions
            .into_iter()
            .map(|pos| crate::structures::TemplateBlock { pos, state: marker })
            .collect(),
    )
    .with_chests(
        positions
            .into_iter()
            .map(|pos| crate::structures::TemplateChest {
                pos,
                chest: mc_world::ChestBlockEntity::default(),
            })
            .collect(),
    );
    let mut chunk = Chunk::empty_with_geometry(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
        geometry,
    );
    let mut touched = [false; 256];

    paste_template(&mut chunk, &template, 0, 1, 0, &mut touched);

    assert!(chunk.chests.is_empty());
    assert!(!touched.into_iter().any(|column| column));
}

fn find_ore_cell(
    g: &TerrainGenerator,
    y: i32,
    biome: &Identifier,
    expected: BlockStateId,
) -> (i32, i32) {
    for x in -256..=256 {
        for z in -256..=256 {
            if g.ore_for(x, y, z, g.stone, biome) == expected {
                return (x, z);
            }
        }
    }
    panic!("could not find ore {expected:?} at y={y} for biome {biome}");
}

pub(in crate::terrain) fn ore_feature(
    feature: &str,
    normal: &str,
    deepslate: &str,
    min_y: i32,
    max_y: i32,
    count: u32,
) -> OreFeature {
    mc_data::worldgen_ores::OreFeature {
        placed_feature: Identifier::parse(feature).unwrap(),
        configured_feature: Identifier::parse(feature).unwrap(),
        placement: mc_data::worldgen_ores::OrePlacement {
            count: Some(OrePlacementCount::Constant(count)),
            rarity_chance: None,
            height: Some(mc_data::worldgen_ores::HeightRange {
                kind: Identifier::parse("minecraft:uniform").unwrap(),
                min: HeightAnchor::Absolute(min_y),
                max: HeightAnchor::Absolute(max_y),
            }),
        },
        size: 4,
        discard_chance_on_air_exposure: 0.0,
        targets: vec![
            OreTarget {
                state: Identifier::parse(normal).unwrap(),
                replaceable_tag: Some(
                    Identifier::parse("minecraft:stone_ore_replaceables").unwrap(),
                ),
            },
            OreTarget {
                state: Identifier::parse(deepslate).unwrap(),
                replaceable_tag: Some(
                    Identifier::parse("minecraft:deepslate_ore_replaceables").unwrap(),
                ),
            },
        ],
    }
}

#[test]
fn ore_growth_never_uses_a_rejected_cell_as_a_bridge() {
    let mut offsets = [[0_i8; 3]; MAX_ORE_VEIN_SIZE];
    let offset_count = connected_ore_offsets(&mut offsets, 0x51, 0, 4, |offset, _hash| {
        offset != [1, 0, 0]
    });
    let offsets = &offsets[..offset_count];

    assert!(!offsets.contains(&[1, 0, 0]));
    assert!(offsets.iter().all(|offset| {
        *offset == [0, 0, 0]
            || ORE_DIRECTIONS.iter().any(|direction| {
                offsets.contains(&[
                    offset[0] - direction[0],
                    offset[1] - direction[1],
                    offset[2] - direction[2],
                ])
            })
    }));
}

#[test]
fn expanded_ore_families_are_reachable_and_biome_scoped() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let mountain = Identifier::parse("minecraft:jagged_peaks").unwrap();
    let hot_dry = Identifier::parse("minecraft:badlands").unwrap();

    for (y, biome, stone_ore, deepslate_ore) in [
        (-16, &plains, BlockStateId(15), BlockStateId(20)),
        (15, &plains, BlockStateId(16), BlockStateId(21)),
        (-56, &plains, BlockStateId(17), BlockStateId(22)),
        (64, &plains, BlockStateId(18), BlockStateId(23)),
        (224, &mountain, BlockStateId(19), BlockStateId(24)),
        (80, &hot_dry, BlockStateId(15), BlockStateId(20)),
    ] {
        let (x, z) = find_ore_cell(&g, y, biome, stone_ore);
        assert_eq!(g.ore_for(x, y, z, g.deepslate, biome), deepslate_ore);
    }

    let (emerald_x, emerald_z) = find_ore_cell(&g, 224, &mountain, BlockStateId(19));
    assert_eq!(
        g.ore_for(emerald_x, 224, emerald_z, g.stone, &plains),
        g.stone,
        "emerald should stay mountain-scoped"
    );

    let (gold_x, gold_z) = find_ore_cell(&g, 80, &hot_dry, BlockStateId(15));
    assert_ne!(
        g.ore_for(gold_x, 80, gold_z, g.stone, &plains),
        BlockStateId(15),
        "high gold boost should stay hot-dry scoped"
    );
}

#[test]
fn data_fed_ore_rules_reach_generation() {
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    let mountain = Identifier::parse("minecraft:jagged_peaks").unwrap();
    let hot_dry = Identifier::parse("minecraft:badlands").unwrap();
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let biome_data = mc_data::biomes::BiomeWorldgenData::from_parts(
        BTreeMap::from([
            (
                mountain.clone(),
                vec![Identifier::parse("minecraft:ore_emerald").unwrap()],
            ),
            (
                hot_dry.clone(),
                vec![Identifier::parse("minecraft:ore_gold_extra").unwrap()],
            ),
            (
                plains.clone(),
                vec![Identifier::parse("minecraft:ore_diamond").unwrap()],
            ),
        ]),
        BTreeMap::new(),
    );
    let features = vec![
        ore_feature(
            "minecraft:ore_emerald",
            "minecraft:emerald_ore",
            "minecraft:deepslate_emerald_ore",
            200,
            240,
            64,
        ),
        ore_feature(
            "minecraft:ore_gold_extra",
            "minecraft:gold_ore",
            "minecraft:deepslate_gold_ore",
            72,
            96,
            64,
        ),
        ore_feature(
            "minecraft:ore_diamond",
            "minecraft:diamond_ore",
            "minecraft:deepslate_diamond_ore",
            -64,
            -48,
            64,
        ),
    ];
    let ores = OreRules::from_features(
        registry.as_ref(),
        &biomes,
        &features,
        Some(&biome_data),
        OVERWORLD_GEOMETRY,
    )
    .expect("sidecar ore features should fit the admission budget")
    .expect("sidecar ore features should become rules");
    let g = TerrainGenerator::with_rules(42, registry, biomes, ores);

    for (y, biome, stone_ore, deepslate_ore) in [
        (224, &mountain, BlockStateId(19), BlockStateId(24)),
        (80, &hot_dry, BlockStateId(15), BlockStateId(20)),
        (-56, &plains, BlockStateId(17), BlockStateId(22)),
    ] {
        let (x, z) = find_ore_cell(&g, y, biome, stone_ore);
        assert_eq!(g.ore_for(x, y, z, g.deepslate, biome), deepslate_ore);
    }

    let (emerald_x, emerald_z) = find_ore_cell(&g, 224, &mountain, BlockStateId(19));
    assert_eq!(
        g.ore_for(emerald_x, 224, emerald_z, g.stone, &plains),
        g.stone
    );
}

#[test]
fn feature_layer_adds_caves_and_ores_without_cave_fluids() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let chunks = [
        g.generate(ChunkPos { x: 0, z: 0 }),
        g.generate(ChunkPos { x: 1, z: 0 }),
        g.generate(ChunkPos { x: 0, z: 1 }),
        g.generate(ChunkPos { x: -1, z: 0 }),
    ];
    let mut saw_cave_air = false;
    let mut saw_ore = false;
    let mut saw_deepslate = false;
    for chunk in chunks {
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                let wx = chunk.pos.x * 16 + lx as i32;
                let wz = chunk.pos.z * 16 + lz as i32;
                let top = g.surface_height(wx, wz);
                for y in (MIN_Y + 1)..top - CAVE_SURFACE_CLEARANCE {
                    match chunk.get_block(lx, y, lz) {
                        Some(BlockStateId(0)) => saw_cave_air = true,
                        Some(BlockStateId(7)) => saw_deepslate = true,
                        Some(BlockStateId(8..=13 | 15..=24)) => saw_ore = true,
                        _ => {}
                    }
                }
            }
        }
    }

    assert!(saw_cave_air, "expected at least one carved cave cell");
    assert!(saw_ore, "expected at least one ore cell");
    assert!(
        saw_deepslate,
        "expected deepslate below the transition band"
    );
}

#[test]
#[ignore = "debug-build throughput probe for M31 closeout"]
fn generated_spawn_window_debug_budget_reports_throughput() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let started = std::time::Instant::now();
    let mut chunks = 0usize;
    for x in -2..=2 {
        for z in -2..=2 {
            let chunk = g.generate(ChunkPos { x, z });
            assert_eq!(chunk.status, "minecraft:full");
            chunks += 1;
        }
    }
    let elapsed = started.elapsed();
    let chunks_per_second = chunks as f64 / elapsed.as_secs_f64().max(0.001);
    eprintln!(
        "generated {chunks} chunks in {elapsed_ms} ms ({chunks_per_second:.1} chunks/s)",
        elapsed_ms = elapsed.as_millis()
    );
    assert!(elapsed < std::time::Duration::from_secs(10));
}
