use super::*;
use mc_world::ChunkPos;

fn registry() -> BlockRegistry {
    BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap()
}

#[test]
fn archaeology_well_generated_with_loot() {
    let r = registry();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        state(&r, "minecraft:stone").unwrap(),
        id("minecraft:desert"),
    );
    paste_template(
        &mut chunk,
        &well(&r).unwrap(),
        BlockPos { x: 0, y: 64, z: 0 },
        42,
    );
    let found = chunk.block_entities.iter().filter(|(pos, bytes)| {
        let tag = mc_nbt::read_network(&mut bytes.as_slice()).unwrap();
        chunk.get_block(pos.x as u8, pos.y, pos.z as u8) == Some(state(&r, "minecraft:suspicious_sand").unwrap())
            && matches!(tag, Tag::Compound(fields) if fields.iter().any(|(key, value)| key == "LootTable" && *value == Tag::String("minecraft:archaeology/desert_well".into())) && fields.iter().any(|(key, value)| key == "LootTableSeed" && matches!(value, Tag::Long(_))))
    }).count();
    assert_eq!(
        found, 2,
        "well must generate two suspicious sand blocks with loot"
    );
}
#[test]
fn dungeon_requires_cavity_opening_and_preserves_it_without_neighbor_reads() {
    let r = registry();
    let air = state(&r, "minecraft:air").unwrap();
    let stone = state(&r, "minecraft:stone").unwrap();
    let biome = id("minecraft:plains");
    let feature = Feature {
        name: "monster_room".into(),
        salt: 3,
        count: 1,
        rarity: 1,
        height: Some((20, 20)),
        biomes: HashSet::from([biome.clone()]),
        templates: vec![dungeon(&r, "zombie").unwrap()],
        reach: [0, 0],
        dungeon_air: Some([
            air,
            state(&r, "minecraft:cave_air").unwrap(),
            state(&r, "minecraft:void_air").unwrap(),
        ]),
        fluids: [
            state(&r, "minecraft:water").unwrap(),
            state(&r, "minecraft:lava").unwrap(),
        ],
    };
    let random = hash(hash(42 ^ 3, 0, 0) as i64, 0, 1);
    let x = (random & 7) as u8;
    let z = ((random >> 4) & 7) as u8;
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, biome.clone());
    for xx in 0..16 {
        for zz in 0..16 {
            for y in 20..=25 {
                chunk.set_block(xx, y, zz, stone).unwrap();
            }
        }
    }
    assert!(
        !paste(
            std::slice::from_ref(&feature),
            42,
            &mut chunk,
            &|_, _| 64,
            &|_, _, _| biome.clone(),
        )
        .into_iter()
        .any(|v| v)
    );
    assert!(
        chunk.block_entities.is_empty(),
        "sealed rock cannot admit a dungeon"
    );
    for y in [21, 22] {
        chunk.set_block(x, y, z + 3, air).unwrap();
    }
    assert!(
        paste(&[feature], 42, &mut chunk, &|_, _| 64, &|_, _, _| biome
            .clone(),)
        .into_iter()
        .any(|v| v)
    );
    assert_eq!(chunk.block_entities.len(), 1);
    assert_eq!(chunk.chests.len(), 2);
    for y in [21, 22] {
        assert_eq!(chunk.get_block(x, y, z + 3), Some(air));
    }
}

#[test]
fn vanilla_features_compile_dungeon_well_fossil_and_geode_content() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    if !root.exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let r = registry();
    let features = load(&root, &r).unwrap();
    assert_eq!(features.len(), 6);
    for f in &features {
        assert!(!f.biomes.is_empty(), "{}", f.name);
        assert!(!f.templates.is_empty(), "{}", f.name);
    }
    let geode = features
        .iter()
        .find(|f| f.name == "amethyst_geode")
        .unwrap();
    for name in [
        "minecraft:smooth_basalt",
        "minecraft:calcite",
        "minecraft:amethyst_block",
        "minecraft:budding_amethyst",
    ] {
        assert!(
            geode.templates[0]
                .blocks()
                .iter()
                .any(|b| r.by_id(b.state).unwrap().block.id == id(name)),
            "{name}"
        );
    }
    for name in ["fossil_upper", "fossil_lower"] {
        assert_eq!(
            features
                .iter()
                .find(|f| f.name == name)
                .unwrap()
                .templates
                .len(),
            8
        );
    }
}
