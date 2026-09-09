use super::*;

#[test]
fn dry_inland_lowlands_do_not_become_swamps() {
    let registry = tests::tiny_registry();
    for (seed, x, z) in [
        (5_617_830, -128, -64),
        (-17_711, 1408, -64),
        (-17_711, 1472, -320),
    ] {
        let generator = TerrainGenerator::with_worldgen_mode(
            seed,
            Arc::clone(&registry),
            WorldgenMode::TellusLike(TellusWorldgenSettings::default()),
        );
        let sample = generator.diagnostic_sample(x, z);
        assert!(
            !generator.biomes.swamp.contains(&sample.biome),
            "dry inland ({seed}, {x}, {z}) became {}",
            sample.biome
        );
    }
}

#[test]
fn generated_riparian_wetlands_have_matching_ground_and_living_trees() {
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded block registry"),
    );
    let settings = TellusWorldgenSettings::default();
    for (seed, x, z, expected) in [
        (5_617_830, -128_i32, -192_i32, "mangrove_swamp"),
        (712_816, 2304, -832, "swamp"),
        (-17_711, 1408, 256, "mangrove_swamp"),
    ] {
        let generator = TerrainGenerator::with_worldgen_mode(
            seed,
            Arc::clone(&registry),
            WorldgenMode::TellusLike(settings),
        );
        let mut native_logs = 0;
        let mut wet_roots = 0;
        let mut orchids = 0;
        let mut water = 0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let pos = ChunkPos {
                    x: x.div_euclid(16) + dx,
                    z: z.div_euclid(16) + dz,
                };
                let chunk = generator.generate(pos);
                for lz in 0..16_u8 {
                    for lx in 0..16_u8 {
                        let wx = pos.x * 16 + i32::from(lx);
                        let wz = pos.z * 16 + i32::from(lz);
                        let sample = generator.diagnostic_sample(wx, wz);
                        assert_ne!(
                            sample.biome.path(),
                            "frozen_river",
                            "frozen river beside a warm wetland at ({seed}, {wx}, {wz})"
                        );
                        if !generator.biomes.swamp.contains(&sample.biome) {
                            continue;
                        }
                        assert_eq!(
                            sample.biome.path(),
                            expected,
                            "wrong wetland climate at ({seed}, {wx}, {wz})"
                        );
                        assert!(
                            (settings.sea_level - 2..=settings.sea_level + 1)
                                .contains(&sample.surface_y)
                        );
                        let ground = registry
                            .by_id(chunk.get_block(lx, sample.surface_y, lz).unwrap())
                            .unwrap();
                        if expected == "mangrove_swamp" {
                            assert!(
                                matches!(ground.block.id.path(), "mud" | "muddy_mangrove_roots"),
                                "mangrove ground at ({wx}, {wz}) is {}",
                                ground.block.id
                            );
                        }
                        for y in sample.surface_y + 1..=sample.surface_y + 18 {
                            let state =
                                registry.by_id(chunk.get_block(lx, y, lz).unwrap()).unwrap();
                            match state.block.id.path() {
                                "water" => water += 1,
                                "mangrove_log" if expected == "mangrove_swamp" => native_logs += 1,
                                "oak_log" if expected == "swamp" => native_logs += 1,
                                "blue_orchid" => orchids += 1,
                                "mangrove_roots" => {
                                    let wet = state.properties.iter().any(|(key, value)| {
                                        key == "waterlogged" && value == "true"
                                    });
                                    assert_eq!(
                                        wet,
                                        y <= settings.sea_level,
                                        "root lost or invented water at ({seed}, {wx}, {y}, {wz})"
                                    );
                                    wet_roots += usize::from(wet);
                                }
                                "mangrove_leaves" => {
                                    let distance = state
                                        .properties
                                        .iter()
                                        .find(|(key, _)| key == "distance")
                                        .unwrap()
                                        .1
                                        .parse::<u8>()
                                        .unwrap();
                                    assert!(
                                        distance < 7,
                                        "unsupported mangrove leaf would decay at ({seed}, {wx}, {y}, {wz})"
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        assert!(
            water > 0 && native_logs > 0,
            "wetland {seed} lacks water or its native trees: water={water}, logs={native_logs}"
        );
        if expected == "mangrove_swamp" {
            assert!(
                wet_roots > 0,
                "mangroves at {seed} never establish in shallow water"
            );
        } else {
            assert!(
                orchids > 0,
                "temperate wetland at {seed} lacks its native ground cover"
            );
        }
    }
}
