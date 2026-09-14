//! Solver tests for checkpoint B's jigsaw placement.

use mc_data::Identifier;
use mc_world::BlockRegistry;

use crate::structures::{BlockFace, Joint, TemplateJigsaw};
use crate::vanilla_features::{LegacyRandom, RandomSource};
use crate::village::load_village_closure;
use crate::village::solver::{Rotation, assemble_village, can_attach, placement_seed, shuffle};

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

/// The placement seed is `setLargeFeatureWithSalt`.
#[test]
fn placement_seed_matches_worldgen_random() {
    assert_eq!(
        placement_seed(0, 0, 0, 10387312),
        crate::village::set_large_feature_with_salt(0, 0, 0, 10387312)
    );
}

/// `JigsawBlock.canAttach`: opposed fronts, a matching (name, target) pair, and
/// either the source is rollable or the tops agree.
#[test]
fn can_attach_follows_the_jigsaw_rules() {
    let jigsaw =
        |front: BlockFace, top: BlockFace, joint: Joint, name: &str, target: &str| TemplateJigsaw {
            pos: [0, 0, 0],
            state: mc_world::BlockStateId(0),
            front,
            top,
            name: identifier(name),
            target: identifier(target),
            pool: identifier("minecraft:empty"),
            joint,
            final_state: mc_world::BlockStateId(0),
            selection_priority: 0,
        };
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

    // Search placement chunks for one that actually places a desert village:
    // vanilla returns "no structure" when the weighted roll picks an empty town
    // centre, so the first candidate is not guaranteed to place.
    let seed = 4242;
    let desert = identifier("minecraft:has_structure/village_desert");
    let is_desert = |tag: &Identifier| tag.as_str().trim_start_matches('#') == desert.as_str();
    let surface = |_x: i32, _z: i32| 64;
    let mut found = None;
    'search: for chunk_x in 0..400 {
        for chunk_z in 0..400 {
            if let Some(assembly) =
                assemble_village(&closure, seed, chunk_x, chunk_z, is_desert, surface)
            {
                found = Some((chunk_x, chunk_z, assembly));
                break 'search;
            }
        }
    }
    let (chunk_x, chunk_z, assembly) =
        found.expect("the desert rolls and places somewhere in the searched range");
    let assemble = || assemble_village(&closure, seed, chunk_x, chunk_z, is_desert, surface);

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
    assert!(is_desert(&assembly.biome_tag));
    let size = closure
        .structures
        .iter()
        .find(|structure| structure.id == assembly.structure)
        .map(|structure| structure.spec.size)
        .expect("the desert structure");
    assert!(assembly.pieces.iter().all(|piece| piece.depth <= size));
    assert_eq!(assembly.junctions.len() % 2, 0, "junctions come in pairs");
    assert!(
        assembly
            .beard_pieces
            .iter()
            .all(|piece| piece.min.y <= piece.max.y)
    );
    // Every placed piece is a template the closure loaded.
    for piece in &assembly.pieces {
        assert!(
            closure.pieces.contains_key(&piece.element),
            "{} is not a loaded piece",
            piece.element
        );
    }

    // Determinism: the same seed and chunk replay the same plan.
    let again = assemble().expect("the same chunk still places");
    assert_eq!(again.pieces, assembly.pieces);
    assert_eq!(again.junctions, assembly.junctions);
    assert_eq!(again.beard_pieces, assembly.beard_pieces);
    assert_eq!(again.origin, assembly.origin);
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
    assert!(assemble_village(&closure, seed, non_placement, 0, |_| true, |_, _| 64).is_none());
}
