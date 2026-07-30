use std::sync::Arc;

use mc_world::BlockRegistry;

use crate::{MosaicConfig, TellusWorldgenSettings, TerrainGenerator, WorldgenMode, render_mosaic};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn generator() -> TerrainGenerator {
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    TerrainGenerator::with_worldgen_mode(
        712_816,
        registry,
        WorldgenMode::TellusLike(TellusWorldgenSettings::default()),
    )
}

#[test]
fn public_alpha_mosaic_has_expected_png_signature_and_dimensions() {
    let images = render_mosaic(
        &generator(),
        &MosaicConfig {
            center_x: 0,
            center_z: 0,
            extent_blocks: 2_048,
            blocks_per_pixel: 8,
        },
    )
    .unwrap();
    assert_eq!((images.width, images.height), (256, 256));
    for png in [
        &images.height_png,
        &images.biome_png,
        &images.vegetation_png,
    ] {
        assert_eq!(&png[..8], PNG_SIGNATURE);
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 256);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 256);
    }
}

#[test]
fn repeated_rendering_is_byte_identical() {
    let config = MosaicConfig {
        center_x: 0,
        center_z: 0,
        extent_blocks: 2_048,
        blocks_per_pixel: 32,
    };
    let first = render_mosaic(&generator(), &config).unwrap();
    let second = render_mosaic(&generator(), &config).unwrap();
    assert_eq!(first.height_png, second.height_png);
    assert_eq!(first.biome_png, second.biome_png);
    assert_eq!(first.vegetation_png, second.vegetation_png);
}
