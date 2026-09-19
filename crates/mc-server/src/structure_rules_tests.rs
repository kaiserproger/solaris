//! The built-in settlement profile's structure rules.
//!
//! `[data] settlement_profile = "plains_village_prototype"` is the explicit
//! opt-in to village generation: `structure_rules_for_startup` builds a bounded
//! plains prototype from the vanilla sidecar's template layout and
//! `build_terrain_generator` generates with it. The layout is all the profile
//! needs, so the fixture below writes synthetic templates into a temp directory
//! and `data/vanilla/` stays out of Git; a live run against the extracted
//! Mojang templates stays a manual field check.
//!
//! The default `vanilla` profile *is* core vanilla village generation: startup
//! builds a `VillagePlanSource` from the resolved content cache and attaches it
//! to the generator, so the world gets real jigsaw villages. A deployed component
//! settlement plan owns settlement content instead, and the explicit
//! `plains_village_prototype` opt-in keeps its own bounded prototype rules; both
//! leave core villages detached, which is what keeps one landscape from carrying
//! two settlements.

use std::path::Path;
use std::sync::Arc;

use mc_data::Identifier;
use mc_server::ServerConfig;
use mc_worldgen::village::plan_source::PlanElement;

use crate::{build_terrain_generator, structure_rules_for_startup};

/// Write the minimal vanilla-layout sidecar the built-in settlement profile
/// reads: one synthetic structure template per prototype part plus one
/// structure-set fact file.
///
/// The profile path needs the vanilla *layout*, not Mojang data: the templates
/// below are synthetic, so this fixture copies nothing out of `data/vanilla/`,
/// which stays out of Git.
fn write_synthetic_vanilla_sidecar(root: &Path) {
    let structure_root = root.join("data/minecraft/structure/village/plains");
    let blocks = [[0, 0, 0], [8, 0, 0], [0, 0, 8], [8, 0, 8], [4, 1, 4]];
    for relative in [
        "town_centers/plains_fountain_01.nbt",
        "houses/plains_small_house_1.nbt",
        "houses/plains_tool_smith_1.nbt",
    ] {
        write_synthetic_structure_template(&structure_root.join(relative), [9, 4, 9], &blocks);
    }
    let set_root = root.join("data/minecraft/worldgen/structure_set");
    std::fs::create_dir_all(&set_root).unwrap();
    std::fs::write(
        set_root.join("villages.json"),
        concat!(
            r#"{"structures":[{"structure":"minecraft:village_plains"}],"#,
            r#""placement":{"spacing":34,"separation":8}}"#,
        ),
    )
    .unwrap();
}

/// One synthetic structure template whose every block is
/// `minecraft:bookshelf`, the marker a generated chunk is checked for: a
/// property-less block no terrain or decoration ever places.
fn write_synthetic_structure_template(path: &Path, size: [i32; 3], blocks: &[[i32; 3]]) {
    use mc_nbt::{ListTag, Tag, tag_type};
    let triplet = |values: [i32; 3]| {
        Tag::List(ListTag {
            element_type: tag_type::INT,
            elements: values.into_iter().map(Tag::Int).collect(),
        })
    };
    let root = Tag::Compound(vec![
        ("size".to_owned(), triplet(size)),
        (
            "palette".to_owned(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![(
                    "Name".to_owned(),
                    Tag::String("minecraft:bookshelf".to_owned()),
                )])],
            }),
        ),
        (
            "blocks".to_owned(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: blocks
                    .iter()
                    .map(|pos| {
                        Tag::Compound(vec![
                            ("pos".to_owned(), triplet(*pos)),
                            ("state".to_owned(), Tag::Int(0)),
                        ])
                    })
                    .collect(),
            }),
        ),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &root).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// How many marker blocks the generated village chunks hold. Seed zero pins
/// the prototype at its fixed center, so the village is these nine chunks
/// whatever the worldgen mode is.
fn village_marker_blocks(
    generator: &mc_worldgen::TerrainGenerator,
    marker: mc_world::BlockStateId,
) -> usize {
    let mut found = 0;
    for chunk_x in 3..=5 {
        for chunk_z in -1..=1 {
            let chunk = mc_world::ChunkGenerator::generate(
                generator,
                mc_world::ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                },
            );
            for y in mc_world::OVERWORLD_GEOMETRY.min_y()..mc_world::OVERWORLD_GEOMETRY.max_y() {
                for local_z in 0..16 {
                    for local_x in 0..16 {
                        found += usize::from(chunk.get_block(local_x, y, local_z) == Some(marker));
                    }
                }
            }
        }
    }
    found
}

/// A stock server that sets `[data] settlement_profile =
/// "plains_village_prototype"` with a vanilla sidecar present generates village
/// structures through the real generation path, while the default `vanilla`
/// profile generates none of them (and reports the gap; see
/// `default_settlement_profile_reports_missing_core_village_generation`).
///
/// The sidecar here is synthetic, so the live run against extracted Mojang data
/// stays a manual field check on a machine that has `data/vanilla/`.
#[test]
fn builtin_settlement_profile_generates_village_structures_from_the_sidecar() {
    let sidecar = tempfile::tempdir().unwrap();
    write_synthetic_vanilla_sidecar(sidecar.path());
    // The stock configuration an operator writes; the same profile name is what
    // startup records as the world identity.
    let configured = |profile: &str| -> ServerConfig {
        toml::from_str(&format!(
            r#"
                [server]
                name = "Settlement"
                motd = "Settlement"
                [network]
                bind_address = "127.0.0.1"
                port = 0
                [data]
                seed = 0
                worldgen_mode = "tellus_like"
                vanilla_data_dir = "{}"
                settlement_profile = "{profile}"
                "#,
            sidecar.path().display()
        ))
        .unwrap()
    };
    let prototype_config = configured("plains_village_prototype");
    let vanilla_config = configured("vanilla");
    assert_eq!(
        prototype_config.data.settlement_profile.name(),
        "plains_village_prototype"
    );

    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let items = mc_data::items::solaris_required_items();
    let marker = blocks
        .block(&Identifier::parse("minecraft:bookshelf").unwrap())
        .expect("the baseline registry holds the marker block")
        .default;

    let profile_rules = structure_rules_for_startup(
        prototype_config.data.seed,
        prototype_config.data.worldgen_mode,
        prototype_config
            .data
            .vanilla_data_dir
            .as_deref()
            .expect("synthetic sidecar"),
        &blocks,
        &items,
        None,
        prototype_config.data.settlement_profile,
    )
    .unwrap();
    assert_eq!(
        profile_rules.templates().len(),
        1,
        "the three prototype parts combine into one bounded village"
    );
    let vanilla_rules = structure_rules_for_startup(
        vanilla_config.data.seed,
        vanilla_config.data.worldgen_mode,
        vanilla_config
            .data
            .vanilla_data_dir
            .as_deref()
            .expect("synthetic sidecar"),
        &blocks,
        &items,
        None,
        vanilla_config.data.settlement_profile,
    )
    .unwrap();
    assert!(vanilla_rules.is_empty());

    let profile = build_terrain_generator(
        prototype_config.data.seed,
        prototype_config.data.worldgen_mode.to_worldgen(),
        mc_world::OVERWORLD_GEOMETRY,
        Arc::clone(&blocks),
        profile_rules,
        None,
        None,
        None,
    )
    .unwrap();
    let vanilla = build_terrain_generator(
        vanilla_config.data.seed,
        vanilla_config.data.worldgen_mode.to_worldgen(),
        mc_world::OVERWORLD_GEOMETRY,
        blocks,
        vanilla_rules,
        None,
        None,
        None,
    )
    .unwrap();

    assert_eq!(
        village_marker_blocks(profile.as_ref(), marker),
        15,
        "plains_village_prototype must paste all three prototype parts"
    );
    assert_eq!(
        village_marker_blocks(vanilla.as_ref(), marker),
        0,
        "the default vanilla profile must generate no village blocks"
    );
}

/// Core villages attach to the default `vanilla` profile and to nothing else:
/// the prototype profile keeps its own rules, and a deployed plugin settlement
/// plan owns settlement content, so neither may also carry core villages.
///
/// The live half (a plan source actually built from the cache) needs the real
/// content cache and is covered by
/// `live_activated_path_places_a_vanilla_village`; this test pins the profile
/// gate, which needs no cache.
#[test]
fn core_villages_attach_only_for_the_vanilla_profile_without_a_plugin_plan() {
    let configured = |profile: &str| -> ServerConfig {
        toml::from_str(&format!(
            r#"
                [server]
                name = "Settlement"
                motd = "Settlement"
                [network]
                bind_address = "127.0.0.1"
                port = 0
                [data]
                seed = 7
                worldgen_mode = "tellus_like"
                settlement_profile = "{profile}"
                "#
        ))
        .unwrap()
    };
    let vanilla = configured("vanilla");
    assert_eq!(
        vanilla.data.settlement_profile,
        mc_server::SettlementProfile::Vanilla
    );

    // The old "core places no villages" operator warning is gone: the profile
    // now has villages to report, and the analogue notice is the honest one.
    assert!(
        !crate::operator_warnings(&vanilla)
            .iter()
            .any(|warning| warning.code == "settlement_profile_vanilla_generates_no_villages"),
        "the vanilla profile must no longer report missing core village generation"
    );
    assert_eq!(
        crate::village_terrain_analogue_notice(true).map(|notice| notice.code),
        Some("village_terrain_adaptation_beard_thin_is_a_column_height_analogue"),
    );

    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let data = Arc::new(mc_data::VanillaData::from_registries("", Vec::new()));
    let tags = Arc::new(mc_data::tags::TagsData::default());
    let plan = mc_script::PluginSettlementPlan::plains_village_prototype("test-settlement");
    let prototype = configured("plains_village_prototype");

    // The prototype profile never takes core villages.
    assert!(
        crate::village_plan_source_for_startup(
            7,
            Path::new("resolved-content-cache"),
            &blocks,
            &data,
            &tags,
            None,
            prototype.data.settlement_profile,
        )
        .unwrap()
        .is_none(),
        "the prototype profile keeps its own rules"
    );
    // A deployed plugin settlement plan never takes core villages, even under
    // the default profile.
    assert!(
        crate::village_plan_source_for_startup(
            7,
            Path::new("resolved-content-cache"),
            &blocks,
            &data,
            &tags,
            Some(&plan),
            vanilla.data.settlement_profile,
        )
        .unwrap()
        .is_none(),
        "a deployed plugin settlement plan owns settlement content"
    );
}

/// A deployed plugin settlement plan still replaces core villages: with the plan
/// present under the default `vanilla` profile, the sidecar's prototype
/// templates are selected and pasted, and no missing-generation notice applies.
#[test]
fn deployed_settlement_plan_replaces_core_villages() {
    let sidecar = tempfile::tempdir().unwrap();
    write_synthetic_vanilla_sidecar(sidecar.path());
    let config: ServerConfig = toml::from_str(&format!(
        r#"
            [server]
            name = "Settlement"
            motd = "Settlement"
            [network]
            bind_address = "127.0.0.1"
            port = 0
            [data]
            seed = 0
            worldgen_mode = "tellus_like"
            vanilla_data_dir = "{}"
            settlement_profile = "vanilla"
            "#,
        sidecar.path().display()
    ))
    .unwrap();

    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let items = mc_data::items::solaris_required_items();
    let marker = blocks
        .block(&Identifier::parse("minecraft:bookshelf").unwrap())
        .expect("the baseline registry holds the marker block")
        .default;
    let plan = mc_script::PluginSettlementPlan::plains_village_prototype("test-settlement");

    let rules = structure_rules_for_startup(
        config.data.seed,
        config.data.worldgen_mode,
        config
            .data
            .vanilla_data_dir
            .as_deref()
            .expect("synthetic sidecar"),
        &blocks,
        &items,
        Some(&plan),
        config.data.settlement_profile,
    )
    .unwrap();
    assert_eq!(
        rules.templates().len(),
        1,
        "the plugin plan's three parts combine into one bounded village"
    );

    let generator = build_terrain_generator(
        config.data.seed,
        config.data.worldgen_mode.to_worldgen(),
        mc_world::OVERWORLD_GEOMETRY,
        blocks,
        rules,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        village_marker_blocks(generator.as_ref(), marker),
        15,
        "a deployed plugin settlement plan must paste village blocks under the default profile"
    );
}

// ------------------------------------------------- the activated path, live

/// `<SOLARIS_CONTENT_CACHE>`, then `/tmp/jdk-cold2`, then
/// `<workspace>/data/vanilla`: the first directory holding a vanilla content
/// cache. A directory without one selects nothing, so the live test below skips
/// loudly instead of passing against an empty cache.
fn content_cache() -> Option<std::path::PathBuf> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    [
        std::env::var("SOLARIS_CONTENT_CACHE")
            .ok()
            .map(std::path::PathBuf::from)
            .unwrap_or_default(),
        std::path::PathBuf::from("/tmp/jdk-cold2"),
        workspace.join("data").join("vanilla"),
    ]
    .into_iter()
    .find(|dir| dir.join("data").join("minecraft").join("worldgen").is_dir())
}

/// The whole activated path, from the real content cache: startup's own helper
/// builds the plan source, the generator attaches it, and a placement chunk
/// carries a real jigsaw village with its terrain analogue applied.
///
/// Loud skip, never a silent pass: without a cache the test prints why it did
/// not run. Point it at a cache with `SOLARIS_CONTENT_CACHE=/path/to/cache`.
#[test]
fn live_activated_path_places_a_vanilla_village() {
    let Some(cache) = content_cache() else {
        println!(
            "SKIP live_activated_path_places_a_vanilla_village: no vanilla content cache found \
             (set SOLARIS_CONTENT_CACHE, or run with /tmp/jdk-cold2 present)"
        );
        return;
    };
    let seed = 4242;
    let mode =
        mc_worldgen::WorldgenMode::TellusLike(mc_worldgen::TellusWorldgenSettings::default());
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let data = Arc::new(mc_data::load(&cache).expect("cache registry index loads"));
    let tags = Arc::new(mc_data::tags::load(&cache, &data).expect("cache tags load"));

    let source = crate::village_plan_source_for_startup(
        seed,
        &cache,
        &blocks,
        &data,
        &tags,
        None,
        mc_server::SettlementProfile::Vanilla,
    )
    .expect("the activated path builds the village plan source")
    .expect("the vanilla profile attaches core villages");
    let closure = source.closure();
    assert_eq!(
        closure.structures.len(),
        5,
        "the vanilla villages set holds five structures"
    );
    assert!(
        closure.pieces.len() > 400,
        "the closure walks the real village templates, got {}",
        closure.pieces.len()
    );
    assert!(
        closure.processor_lists.len() >= 16,
        "the closure resolves the village processor lists, got {}",
        closure.processor_lists.len()
    );

    let plain = build_terrain_generator(
        seed,
        mode,
        mc_world::OVERWORLD_GEOMETRY,
        Arc::clone(&blocks),
        mc_worldgen::StructureRules::none(),
        None,
        None,
        None,
    )
    .unwrap();
    let generator = build_terrain_generator(
        seed,
        mode,
        mc_world::OVERWORLD_GEOMETRY,
        blocks,
        mc_worldgen::StructureRules::none(),
        None,
        None,
        Some(Arc::clone(&source)),
    )
    .unwrap();

    // The biome gate is the lookup's own: a plan exists only where the structure
    // set's biome tag accepted the world's biome at the start column.
    let free_height = |x: i32, z: i32| plain.surface_height(x, z).saturating_add(1);
    let biome_at = |x: i32, z: i32| plain.base_biome(x, z);
    let placement = closure.placement;
    let mut found = None;
    'cells: for grid_z in 0..16 {
        for grid_x in 0..16 {
            let candidate = placement.potential_chunk(
                seed,
                grid_x * placement.spacing,
                grid_z * placement.spacing,
            );
            if !placement.is_placement_chunk(seed, candidate.0, candidate.1) {
                continue;
            }
            let Some(set) =
                source.plans_for_chunk(candidate.0, candidate.1, &free_height, &biome_at)
            else {
                continue;
            };
            // The candidate's *own* village, not just any village the
            // neighbourhood reaches: a set can hold a neighbour's plan too, and
            // only the village started here has its start piece in this chunk.
            let own = set
                .plans()
                .iter()
                .find(|plan| plan.start_chunk() == candidate)
                .map(Arc::clone);
            if let Some(plan) = own {
                found = Some((candidate, set, plan));
                break 'cells;
            }
        }
    }
    let (chunk, set, plan) =
        found.expect("a placement chunk in the first 16x16 cells holds a village");
    assert_eq!(plan.start_chunk(), chunk);
    assert!(
        plan.pieces().len() > 1,
        "the village grew past its start piece: {} pieces",
        plan.pieces().len()
    );
    // The village pools declare two kinds: `legacy_single_pool_element`
    // templates and `feature_pool_element` decor leaves. Every *template* piece
    // must be the legacy kind; a decor leaf carries its feature instead.
    assert!(
        plan.pieces().iter().all(|piece| !matches!(
            &piece.element,
            PlanElement::Single { kind, .. } if *kind != piece_element_legacy()
        )),
        "every village template piece is a legacy single pool element"
    );

    // The chunk the start piece's own box centres in, not the candidate: the
    // start piece is anchored by its jigsaw at the candidate's middle block and
    // its box extends west of that middle, so the candidate chunk can hold only
    // a sliver of it while the piece's own chunk holds the village.
    let start_piece = plan
        .pieces()
        .first()
        .expect("a village starts with a piece");
    let pos = mc_world::ChunkPos {
        x: (start_piece.bounds_min.x + start_piece.bounds_max.x)
            .div_euclid(2)
            .div_euclid(16),
        z: (start_piece.bounds_min.z + start_piece.bounds_max.z)
            .div_euclid(2)
            .div_euclid(16),
    };
    let planned_chunk = mc_world::ChunkGenerator::generate(generator.as_ref(), pos);
    let plain_chunk = mc_world::ChunkGenerator::generate(plain.as_ref(), pos);
    let (min, max) = set.affected_box();
    let mut changed = 0usize;
    for y in planned_chunk.geometry().min_y()..planned_chunk.geometry().max_y() {
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                let before = plain_chunk.get_block(lx, y, lz);
                let after = planned_chunk.get_block(lx, y, lz);
                if before != after {
                    // `terrain_matching` pieces follow the terrain: the gravity
                    // processor snaps their blocks to the height at their own
                    // column, which can leave the piece's pre-processor box
                    // vertically. Locality is therefore a horizontal property.
                    let world = mc_world::BlockPos {
                        x: pos.x * 16 + i32::from(lx),
                        y,
                        z: pos.z * 16 + i32::from(lz),
                    };
                    assert!(
                        (min.x..=max.x).contains(&world.x) && (min.z..=max.z).contains(&world.z),
                        "block {world:?} changed outside the plan region's columns {min:?}..{max:?}",
                    );
                    changed += 1;
                }
            }
        }
    }
    assert!(
        changed > 32,
        "the village's start chunk must carry the village, changed {changed} blocks"
    );

    // The terrain analogue is applied: columns of this chunk report the moved
    // surface, and the unplanned generator reports the router's.
    let moved = (0..16)
        .flat_map(|lz| (0..16).map(move |lx| (lx, lz)))
        .filter(|(lx, lz)| {
            let (x, z) = (chunk.0 * 16 + lx, chunk.1 * 16 + lz);
            generator.surface_height(x, z) != plain.surface_height(x, z)
        })
        .count();
    assert!(
        moved > 0,
        "the beard analogue must move columns of the village chunk"
    );

    // Deterministic replay through the activated path.
    let replay = mc_world::ChunkGenerator::generate(generator.as_ref(), pos);
    for y in planned_chunk.geometry().min_y()..planned_chunk.geometry().max_y() {
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                assert_eq!(
                    replay.get_block(lx, y, lz),
                    planned_chunk.get_block(lx, y, lz),
                    "the activated path must replay identically at ({lx}, {y}, {lz})"
                );
            }
        }
    }

    println!(
        "live proof: seed {seed}, village at chunk {chunk:?}, {} pieces, {} junctions, {} beard \
         pieces, region {min:?}..{max:?}, {changed} village blocks, {moved} columns moved",
        plan.pieces().len(),
        plan.junctions().len(),
        plan.assembly().beard_pieces.len(),
    );
}

/// The element kind every reachable village pool element uses.
fn piece_element_legacy() -> mc_worldgen::village::processors::PieceElement {
    mc_worldgen::village::processors::PieceElement::LegacySingle
}
