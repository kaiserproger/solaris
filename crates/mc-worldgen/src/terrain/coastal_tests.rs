use super::{
    SEA_LEVEL, TellusWorldgenSettings, TerrainGenerator, WorldgenMode, tests::tiny_registry,
};
use mc_data::Identifier;

#[test]
fn coastal_variants_follow_temperature_in_both_modes() {
    for mode in [
        WorldgenMode::VanillaLike,
        WorldgenMode::TellusLike(Default::default()),
    ] {
        // (-448, -32) sits at the water's edge on this seed: Tellus beaches now
        // require adjacent water, so a synthetic inland column is not enough.
        let g = TerrainGenerator::with_worldgen_mode(712_816, tiny_registry(), mode);
        let mut sample = g.density_router().sample(0, 0);
        sample.continentalness = -0.01;
        sample.river = 1.0;
        sample.wetland = 0.0;
        sample.ridges = 0.0;
        sample.erosion = 0.5;
        sample.climate_domain = 0.0;
        for (temperature, ocean, deep, shore) in [
            (-0.4, "frozen_ocean", "deep_frozen_ocean", "snowy_beach"),
            (-0.2, "cold_ocean", "deep_cold_ocean", "beach"),
            (0.0, "ocean", "deep_ocean", "beach"),
            (0.2, "lukewarm_ocean", "deep_lukewarm_ocean", "beach"),
            (0.4, "warm_ocean", "deep_lukewarm_ocean", "beach"),
        ] {
            sample.temperature = temperature;
            for (height, expected) in [
                (SEA_LEVEL - 30, deep),
                (SEA_LEVEL - 3, ocean),
                (SEA_LEVEL + 1, shore),
            ] {
                let (x, z) = (-448, -32);
                let biome = match mode {
                    WorldgenMode::VanillaLike => g.vanilla_biome_for(x, z, height, sample),
                    WorldgenMode::TellusLike(settings) => {
                        g.tellus_biome_for(x, z, height, settings, sample)
                    }
                };
                assert_eq!(
                    biome.path(),
                    expected,
                    "{mode:?}, temperature {temperature}, height {height}"
                );
            }
        }
    }
}

#[test]
fn rocky_shore_has_rocky_ground_but_raised_bank_is_not_beach() {
    // A Tellus beach needs adjacent water, so probe at a shoreline column.
    let g = TerrainGenerator::with_worldgen_mode(
        712_816,
        tiny_registry(),
        WorldgenMode::TellusLike(Default::default()),
    );
    let mut sample = g.density_router().sample(-448, -32);
    sample.continentalness = 0.0;
    sample.temperature = 0.3;
    sample.erosion = 0.2;
    sample.river = 1.0;
    sample.wetland = 0.0;
    let shore = g.tellus_biome_for(
        -448,
        -32,
        SEA_LEVEL + 1,
        TellusWorldgenSettings::default(),
        sample,
    );
    assert_eq!(shore.path(), "stony_shore");
    assert_eq!(g.surface_materials(&shore), (g.gravel, g.stone));
    let bank = g.tellus_biome_for(
        -448,
        -32,
        SEA_LEVEL + 3,
        TellusWorldgenSettings::default(),
        sample,
    );
    assert!(!g.biomes.is_beach_or_shore(&bank));
    let beach = Identifier::parse("minecraft:beach").unwrap();
    assert_eq!(g.surface_materials(&beach), (g.sand, g.sand));
}
