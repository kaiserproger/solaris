//! Solver tests for checkpoint B's jigsaw placement.

use mc_data::Identifier;
use mc_world::{BlockPos, BlockRegistry};

use crate::structures::BlockFace;
use crate::vanilla_features::{LegacyRandom, RandomSource};
use crate::village::load_village_closure;
use crate::village::solver::{
    Jigsaw, PlacedElement, Rotation, assemble_village, can_attach, placement_seed, shuffle,
};

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).unwrap()
}

/// `<SOLARIS_CONTENT_CACHE>`, then `<workspace>/data/vanilla`.
fn content_cache() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("SOLARIS_CONTENT_CACHE") {
        let dir = std::path::PathBuf::from(dir);
        return dir
            .join("data")
            .join("minecraft")
            .join("worldgen")
            .is_dir()
            .then_some(dir);
    }
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?;
    let dir = workspace.join("data").join("vanilla");
    dir.join("data")
        .join("minecraft")
        .join("worldgen")
        .is_dir()
        .then_some(dir)
}

/// `Rotation.transform` is vanilla's `StructureTemplate.transform` with no
/// mirror and a zero pivot.
#[test]
fn rotation_transform_matches_vanilla() {
    assert_eq!(Rotation::None.transform([1, 2, 3]), [1, 2, 3]);
    assert_eq!(Rotation::Clockwise90.transform([1, 2, 3]), [-3, 2, 1]);
    assert_eq!(Rotation::Clockwise180.transform([1, 2, 3]), [-1, 2, -3]);
    assert_eq!(
        Rotation::CounterClockwise90.transform([1, 2, 3]),
        [3, 2, -1]
    );
    // The four rotations are a group: four steps return the original.
    let mut pos = [3, -1, 7];
    for _ in 0..4 {
        pos = Rotation::Clockwise90.transform(pos);
    }
    assert_eq!(pos, [3, -1, 7]);
}

/// `Rotation.rotate(Direction)` / `JigsawBlock.rotate`: a rotation about Y fixes
/// `up`/`down` and steps the horizontals clockwise, and both faces of a jigsaw
/// rotate independently.
#[test]
fn rotating_a_jigsaw_turns_both_faces() {
    use BlockFace::{East, North, South, Up, West};
    for (rotation, expected) in [
        (Rotation::None, North),
        (Rotation::Clockwise90, East),
        (Rotation::Clockwise180, South),
        (Rotation::CounterClockwise90, West),
    ] {
        assert_eq!(rotation.rotate_face(North), expected);
        // `up` is fixed by every rotation about Y.
        assert_eq!(rotation.rotate_face(Up), Up);
    }
    // `FrontAndTop.DOWN_SOUTH` rotated a quarter turn is `DOWN_WEST`, the
    // orientation vanilla's own `FeaturePoolElement` default jigsaw is built
    // from and the piece-facing case that drives `canAttach`.
    assert_eq!(
        Rotation::Clockwise90.rotate_front_and_top(BlockFace::Down, South),
        (BlockFace::Down, West)
    );
}

/// `Util.shuffle` is the Fisher-Yates from the end, so a fixed seed fixes the
/// permutation.
#[test]
fn shuffle_matches_fisher_yates() {
    let mut random = LegacyRandom::new(7);
    let mut values = vec![0, 1, 2, 3, 4, 5, 6, 7];
    shuffle(&mut values, &mut random);
    let expected = {
        let mut random = LegacyRandom::new(7);
        let mut values = vec![0, 1, 2, 3, 4, 5, 6, 7];
        for index in (2..=values.len()).rev() {
            let swap = random.next_int_bounded(index as i32) as usize;
            values.swap(index - 1, swap);
        }
        values
    };
    assert_eq!(values, expected);
    assert_ne!(values, vec![0, 1, 2, 3, 4, 5, 6, 7]);
}

/// The placement seed is `setLargeFeatureWithSalt`, which is *not* the seed the
/// selection and the growth run on.
#[test]
fn placement_seed_matches_worldgen_random() {
    assert_eq!(
        placement_seed(0, 0, 0, 10387312),
        crate::village::set_large_feature_with_salt(0, 0, 0, 10387312)
    );
    assert_ne!(
        placement_seed(0, 5, 7, 10387312),
        crate::village::set_large_feature_seed(0, 5, 7)
    );
}

/// `JigsawBlock.canAttach`: opposed fronts, a matching (name, target) pair, and
/// either the source is rollable or the tops agree.
#[test]
fn can_attach_follows_the_jigsaw_rules() {
    let jigsaw = |front: BlockFace, top: BlockFace, joint, name: &str, target: &str| Jigsaw {
        pos: BlockPos { x: 0, y: 0, z: 0 },
        front,
        top,
        joint,
        name: identifier(name),
        target: identifier(target),
        pool: identifier("minecraft:empty"),
        placement_priority: 0,
    };
    use crate::structures::Joint;
    let source = jigsaw(
        BlockFace::North,
        BlockFace::Up,
        Joint::Aligned,
        "minecraft:street",
        "minecraft:street",
    );
    let matching = jigsaw(
        BlockFace::South,
        BlockFace::Up,
        Joint::Aligned,
        "minecraft:street",
        "minecraft:street",
    );
    assert!(can_attach(&source, &matching));
    // Opposed fronts but differing tops only pass when the source is rollable.
    let rolled = jigsaw(
        BlockFace::South,
        BlockFace::East,
        Joint::Aligned,
        "minecraft:street",
        "minecraft:street",
    );
    assert!(!can_attach(&source, &rolled));
    let rollable = jigsaw(
        BlockFace::North,
        BlockFace::Up,
        Joint::Rollable,
        "minecraft:street",
        "minecraft:street",
    );
    assert!(can_attach(&rollable, &rolled));
    // A name/target mismatch never attaches.
    let mismatched = jigsaw(
        BlockFace::South,
        BlockFace::Up,
        Joint::Aligned,
        "minecraft:building_entrance",
        "minecraft:street",
    );
    assert!(!can_attach(&source, &mismatched));
    // Fronts must be opposed.
    let same_front = jigsaw(
        BlockFace::North,
        BlockFace::Up,
        Joint::Aligned,
        "minecraft:street",
        "minecraft:street",
    );
    assert!(!can_attach(&source, &same_front));
}

/// A jigsaw of a feature element: the synthetic `bottom`/`empty` connector
/// `FeaturePoolElement.getShuffledJigsawBlocks` reports, which attaches only to
/// a source whose front points up and whose target is `minecraft:bottom`.
#[test]
fn feature_jigsaws_are_the_synthetic_bottom_connector() {
    let feature = PlacedElement::Feature {
        placed_feature: identifier("minecraft:oak"),
        owner: identifier("minecraft:village/plains/decor"),
        projection: mc_data::village_data::Projection::Rigid,
    };
    let closure = crate::village::closure::VillageClosure {
        structure_set: identifier("minecraft:villages"),
        placement: crate::village::placement::RandomSpreadPlacement {
            spacing: 34,
            separation: 8,
            salt: 10387312,
            triangular: false,
        },
        structures: Vec::new(),
        pools: std::collections::BTreeMap::new(),
        processor_lists: std::collections::BTreeMap::new(),
        pieces: std::collections::BTreeMap::new(),
        placed_features: Vec::new(),
    };
    let mut random = LegacyRandom::new(1);
    let jigsaws = crate::village::solver::jigsaws_for(
        &feature,
        &closure,
        BlockPos { x: 9, y: 70, z: -4 },
        Rotation::Clockwise180,
        &mut random,
    );
    assert_eq!(jigsaws.len(), 1);
    let jigsaw = &jigsaws[0];
    // The feature ignores the rotation: it sits at the position it was given.
    assert_eq!(jigsaw.pos, BlockPos { x: 9, y: 70, z: -4 });
    assert_eq!(jigsaw.front, BlockFace::Down);
    assert_eq!(jigsaw.top, BlockFace::South);
    assert_eq!(jigsaw.name, identifier("minecraft:bottom"));
    assert_eq!(jigsaw.target, identifier("minecraft:empty"));
    assert_eq!(jigsaw.pool, identifier("minecraft:empty"));
    // Nothing attaches to the empty pool, which is what terminates the branch.
    assert!(closure.pool(&jigsaw.pool).is_none());
    // A one-element list means the shuffle consumed nothing.
    assert_eq!(random.next_int(), LegacyRandom::new(1).next_int());
}

/// The desert village, placed end to end: pieces grow from the start pool, the
/// junctions and beard pieces come out of the rigid connectors, the depth bound
/// holds, and the same seed replays identically.
#[test]
fn desert_village_assembles_deterministically() {
    let Some(cache) = content_cache() else {
        println!(
            "SKIP desert_village_assembles_deterministically: no vanilla content cache found \
             (set SOLARIS_CONTENT_CACHE)"
        );
        return;
    };
    let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
        .expect("embedded block report");
    let closure = load_village_closure(&cache, &identifier("minecraft:villages"), &blocks)
        .expect("the village closure loads");

    // The biome gate is decided at the stub position, so the test supplies the
    // biome the gate must see: a desert world. Vanilla re-draws the set's
    // entries until one passes, so the first candidate chunk that the placement
    // formula accepts holds a desert village.
    let seed = 4242;
    let desert_tag = identifier("minecraft:has_structure/village_desert");
    let biome_at = |_x: i32, _z: i32| identifier("minecraft:desert");
    let biome_matches = |tag: &Identifier, _biome: &Identifier| {
        tag.as_str().trim_start_matches('#') == desert_tag.as_str()
    };
    let surface = |_x: i32, _z: i32| 64;
    let mut found = None;
    'search: for chunk_x in 0..400 {
        for chunk_z in 0..400 {
            if let Some(assembly) = assemble_village(
                &closure,
                seed,
                chunk_x,
                chunk_z,
                biome_at,
                biome_matches,
                surface,
            ) {
                found = Some((chunk_x, chunk_z, assembly));
                break 'search;
            }
        }
    }
    let (chunk_x, chunk_z, assembly) =
        found.expect("the desert rolls and places somewhere in the searched range");
    let assemble = || {
        assemble_village(
            &closure,
            seed,
            chunk_x,
            chunk_z,
            biome_at,
            biome_matches,
            surface,
        )
    };

    println!(
        "desert village at chunk ({chunk_x}, {chunk_z}): {} pieces, {} junctions, {} beard pieces, \
         elements {:?}",
        assembly.pieces.len(),
        assembly.junctions.len(),
        assembly.beard_pieces.len(),
        assembly.elements().len()
    );
    assert!(!assembly.pieces.is_empty());
    assert_eq!(assembly.structure, identifier("minecraft:village_desert"));
    assert!(biome_matches(&assembly.biome_tag, &biome_at(0, 0)));
    let size = closure
        .structures
        .iter()
        .find(|structure| structure.id == assembly.structure)
        .map(|structure| structure.spec.size)
        .expect("the desert structure");
    // `StructureStart.placeInChunk` places a piece a source at `maxDepth`
    // attached even though it is not queued, so the deepest piece is one past
    // the structure size.
    assert!(assembly.pieces.iter().all(|piece| piece.depth <= size + 1));
    assert_eq!(assembly.junctions.len() % 2, 0, "junctions come in pairs");
    assert!(
        assembly
            .beard_pieces
            .iter()
            .all(|piece| piece.min.y <= piece.max.y)
    );
    // Every placed piece is a template the closure loaded, or a feature element
    // the closure reached.
    for piece in &assembly.pieces {
        match &piece.element {
            PlacedElement::Single { template, .. } => {
                assert!(
                    closure.pieces.contains_key(template),
                    "{template} is not a loaded piece"
                );
            }
            PlacedElement::Feature { placed_feature, .. } => {
                assert!(
                    closure.placed_features.contains(placed_feature),
                    "{placed_feature} is not a closure feature"
                );
            }
            PlacedElement::Empty => panic!("an empty element is never placed"),
        }
    }

    // Determinism: the same seed and chunk replay the same plan.
    let again = assemble().expect("the same chunk still places");
    assert_eq!(again.pieces, assembly.pieces);
    assert_eq!(again.junctions, assembly.junctions);
    assert_eq!(again.beard_pieces, assembly.beard_pieces);
    assert_eq!(again.origin, assembly.origin);
}

/// `ChunkGenerator.createStructures` re-draws the set's entries when the drawn
/// one fails its biome gate, from the same stream, until one generates: a chunk
/// where only the last entry of the set is eligible still holds a village.
#[test]
fn selection_retries_until_an_entry_passes_its_biome_gate() {
    let Some(cache) = content_cache() else {
        println!("SKIP selection_retries_until_an_entry_passes_its_biome_gate: no cache");
        return;
    };
    let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
        .expect("embedded block report");
    let closure = load_village_closure(&cache, &identifier("minecraft:villages"), &blocks)
        .expect("the village closure loads");
    let seed = 4242;
    let taiga_tag = identifier("minecraft:has_structure/village_taiga");
    let biome_matches = |tag: &Identifier, _biome: &Identifier| {
        tag.as_str().trim_start_matches('#') == taiga_tag.as_str()
    };
    let surface = |_x: i32, _z: i32| 64;
    let biome_at = |_x: i32, _z: i32| identifier("minecraft:taiga");
    let candidate = (0..400)
        .find(|chunk_x| closure.placement.is_placement_chunk(seed, *chunk_x, 0))
        .expect("the seed has a placement chunk near the origin");
    let assembly = assemble_village(
        &closure,
        seed,
        candidate,
        0,
        biome_at,
        biome_matches,
        surface,
    )
    .expect("the eligible entry is reached by re-drawing");
    assert_eq!(assembly.structure, identifier("minecraft:village_taiga"));
}

/// A chunk that is not a placement chunk never produces a village.
#[test]
fn non_placement_chunks_produce_no_village() {
    let Some(cache) = content_cache() else {
        println!("SKIP non_placement_chunks_produce_no_village: no vanilla content cache found");
        return;
    };
    let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
        .expect("embedded block report");
    let closure = load_village_closure(&cache, &identifier("minecraft:villages"), &blocks)
        .expect("the village closure loads");
    let seed = 4242;
    let non_placement = (0..200)
        .find(|chunk_x| !closure.placement.is_placement_chunk(seed, *chunk_x, 0))
        .expect("some chunk is not a placement chunk");
    assert!(
        assemble_village(
            &closure,
            seed,
            non_placement,
            0,
            |_, _| identifier("minecraft:plains"),
            |_, _| true,
            |_, _| 64,
        )
        .is_none()
    );
}

/// The start piece is anchored so its named connector lands exactly on the start
/// position: `JigsawPlacement.addPieces` computes
/// `localAnchor = anchoredPosition − position` and places the piece at
/// `position − localAnchor`, which is the start chunk's minimum block corner at
/// the sampled start height.
///
/// No village structure names a start jigsaw (`start_jigsaw_name` is absent from
/// all five), so a connector at a *non-origin* local offset is the only fixture
/// that can see the convention: anchoring the piece on the connector's own world
/// position instead displaces the whole village by twice that offset. The Y is
/// not asserted because the heightmap projection moves the whole piece after the
/// anchor is applied.
#[test]
fn the_start_piece_is_anchored_on_its_named_connector() {
    use std::collections::BTreeMap;

    use mc_data::village_data::{
        BiomeSet, DimensionPadding, HeightProviderSpec, LiquidSettings, MaxDistance, StructureSpec,
        StructureStep, TerrainAdaptation, VerticalAnchor,
    };

    use crate::structures::{Joint, StructureTemplate, TemplateBlock, TemplateJigsaw};
    use crate::village::closure::{ClosureElement, ClosurePool, ClosureStructure, VillageClosure};
    use crate::village::placement::start_origin;
    use crate::village::solver::jigsaws_for;

    let blocks = BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
        .expect("embedded block report");
    let marker = blocks
        .block(&identifier("minecraft:stone"))
        .expect("stone is a required block")
        .default;
    let template_id = identifier("test:village/center");
    let pool_id = identifier("test:village/centers");
    let structure_id = identifier("test:village_plains");
    // Deliberately not the template's origin, and inside the template's size.
    let anchor_local = [5, 2, 3];
    let template = StructureTemplate::new(
        [9, 5, 9],
        vec![TemplateBlock {
            pos: [0, 0, 0],
            state: marker,
        }],
    )
    .with_jigsaws(vec![TemplateJigsaw {
        pos: anchor_local,
        state: marker,
        front: BlockFace::Down,
        top: BlockFace::North,
        name: identifier("test:anchor"),
        target: identifier("test:empty"),
        // A pool the closure does not carry: the branch terminates, so the
        // assembly's first piece is the only piece.
        pool: identifier("test:absent"),
        joint: Joint::Rollable,
        final_state: marker,
        selection_priority: 0,
        placement_priority: 0,
    }]);
    let spec = StructureSpec {
        id: structure_id.clone(),
        structure_type: identifier("minecraft:jigsaw"),
        biomes: BiomeSet::Biomes(vec![identifier("minecraft:plains")]),
        spawn_overrides: Vec::new(),
        step: StructureStep::SurfaceStructures,
        terrain_adaptation: TerrainAdaptation::BeardThin,
        start_pool: pool_id.clone(),
        start_jigsaw_name: Some(identifier("test:anchor")),
        size: 0,
        start_height: HeightProviderSpec::Constant(VerticalAnchor::Absolute(0)),
        use_expansion_hack: true,
        project_start_to_heightmap: Some(mc_data::village_data::HeightmapType::WorldSurfaceWg),
        max_distance_from_center: MaxDistance {
            horizontal: 80,
            vertical: 80,
        },
        dimension_padding: DimensionPadding { bottom: 0, top: 0 },
        liquid_settings: LiquidSettings::ApplyWaterlogging,
    };
    let closure = VillageClosure {
        structure_set: identifier("test:villages"),
        // Every chunk is a placement chunk, so the test does not search.
        placement: crate::village::placement::RandomSpreadPlacement {
            spacing: 1,
            separation: 0,
            salt: 10_387_312,
            triangular: false,
        },
        structures: vec![ClosureStructure {
            id: structure_id.clone(),
            weight: 1,
            spec,
            start_height: 0,
        }],
        pools: BTreeMap::from([(
            pool_id.clone(),
            ClosurePool {
                id: pool_id,
                fallback: identifier("minecraft:empty"),
                max_size: 1,
                elements: vec![(
                    1,
                    ClosureElement::Single {
                        piece: template_id.clone(),
                        projection: mc_data::village_data::Projection::Rigid,
                        processors: mc_data::village_data::ProcessorRef::Inline(Vec::new()),
                        legacy: true,
                    },
                )],
            },
        )]),
        processor_lists: BTreeMap::new(),
        pieces: BTreeMap::from([(template_id, template)]),
        placed_features: Vec::new(),
    };

    let (chunk_x, chunk_z) = (12, -7);
    let surface = 64;
    let assembly = assemble_village(
        &closure,
        4242,
        chunk_x,
        chunk_z,
        |_x, _z| identifier("minecraft:plains"),
        |_tag, _biome| true,
        |_x, _z| surface,
    )
    .expect("the synthetic structure starts");
    assert_eq!(assembly.structure, structure_id);
    assert_eq!(
        assembly.pieces.len(),
        1,
        "an absent connector pool grows nothing"
    );
    let piece = &assembly.pieces[0];
    let anchor = jigsaws_for(
        &piece.element,
        &closure,
        BlockPos { x: 0, y: 0, z: 0 },
        piece.rotation,
        &mut LegacyRandom::new(1),
    )
    .into_iter()
    .find(|jigsaw| jigsaw.name == identifier("test:anchor"))
    .expect("the template's named connector");
    assert_ne!(
        anchor.pos,
        BlockPos { x: 0, y: 0, z: 0 },
        "the fixture's connector must be off the template origin"
    );
    let origin = start_origin(chunk_x, chunk_z, 0, surface);
    assert_eq!(
        BlockPos {
            x: piece.position.x + anchor.pos.x,
            y: piece.position.y + anchor.pos.y,
            z: piece.position.z + anchor.pos.z,
        }
        .x,
        origin.x,
        "the named connector must land on the start chunk's minimum block X"
    );
    assert_eq!(
        BlockPos {
            x: piece.position.x + anchor.pos.x,
            y: piece.position.y + anchor.pos.y,
            z: piece.position.z + anchor.pos.z,
        }
        .z,
        origin.z,
        "the named connector must land on the start chunk's minimum block Z"
    );
}
