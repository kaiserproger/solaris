//! Startup-compiled legacy structure families; no template IO during generation.
use super::*;
use crate::structures::TemplateEntity;

pub(super) fn templates(
    root: &Path,
    name: &str,
    registry: &BlockRegistry,
) -> Result<(Vec<StructureTemplate>, Height), String> {
    let height = match name {
        "mineshaft" => Height::Underground,
        "shipwreck" | "ocean_ruin_warm" | "ocean_ruin_cold" => Height::OceanFloor,
        "monument" => Height::Monument,
        "buried_treasure" => Height::Buried(3),
        "desert_pyramid" => Height::Buried(12),
        "jungle_pyramid" => Height::Buried(4),
        _ => Height::Surface,
    };
    let templates = match name {
        "mineshaft" => vec![mineshaft(registry)?],
        "mineshaft_mesa" => {
            let t = mineshaft(registry)?;
            let oak = state(registry, "minecraft:oak_planks")?;
            let fence = state(registry, "minecraft:oak_fence")?;
            let mut blocks = t.blocks().to_vec();
            for b in &mut blocks {
                if b.state == oak {
                    b.state = state(registry, "minecraft:dark_oak_planks")?;
                } else if b.state == fence {
                    b.state = state(registry, "minecraft:dark_oak_fence")?;
                }
            }
            vec![
                StructureTemplate::new(t.size(), blocks)
                    .with_chests(t.chests().to_vec())
                    .with_block_entities(t.block_entities().to_vec())
                    .with_entities(t.entities().to_vec()),
            ]
        }
        "igloo" => igloos(root, registry)?,
        "shipwreck" | "shipwreck_beached" => {
            assets(root, "shipwreck", "shipwreck", registry, |_| true)?
        }
        "ocean_ruin_warm" => assets(root, "underwater_ruin", name, registry, |s| {
            s.starts_with("warm_") || s.starts_with("big_warm_")
        })?,
        "ocean_ruin_cold" => assets(root, "underwater_ruin", name, registry, |s| {
            s.starts_with("brick_") || s.starts_with("big_brick_")
        })?,
        "pillager_outpost" => vec![outpost(root, registry)?],
        "desert_pyramid" => vec![desert_pyramid(registry)?],
        "jungle_pyramid" => vec![jungle_pyramid(registry)?],
        "swamp_hut" => vec![swamp_hut(registry)?],
        "buried_treasure" => {
            let mut b = Builder::new(registry);
            b.chest([9, 0, 9], "minecraft:chests/buried_treasure")?;
            vec![b.finish([16, 1, 16])]
        }
        "monument" => vec![monument(registry)?],
        "mansion" => vec![mansion(root, registry)?],
        name if name.starts_with("ruined_portal") => {
            assets(root, "ruined_portal", name, registry, |_| true)?
        }
        _ => return Err(format!("unsupported legacy structure {name}")),
    };
    if templates.is_empty() {
        return Err(format!("no templates for {name}"));
    }
    Ok((templates, height))
}

fn asset(
    root: &Path,
    name: &str,
    family: &str,
    registry: &BlockRegistry,
) -> Result<StructureTemplate, String> {
    clean_template(
        StructureTemplate::from_nbt_file(
            root.join(format!("data/minecraft/structure/{name}.nbt")),
            registry,
        )
        .map_err(|e| e.to_string())?,
        family,
        registry,
    )
}
fn assets(
    root: &Path,
    directory: &str,
    family: &str,
    registry: &BlockRegistry,
    accept: impl Fn(&str) -> bool,
) -> Result<Vec<StructureTemplate>, String> {
    let directory_path = root.join("data/minecraft/structure").join(directory);
    let mut paths = std::fs::read_dir(&directory_path)
        .map_err(|e| e.to_string())?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    paths.sort();
    let mut templates = Vec::new();
    for path in paths {
        if path.extension().and_then(|s| s.to_str()) != Some("nbt") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("invalid structure template filename")?;
        if accept(stem) {
            templates.push(asset(
                root,
                &format!("{directory}/{stem}"),
                family,
                registry,
            )?);
        }
    }
    Ok(templates)
}

fn mob(kind: &str, pos: [i32; 3]) -> TemplateEntity {
    TemplateEntity {
        entity_type: format!("minecraft:{kind}"),
        persistent: true,
        position: [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]),
            f64::from(pos[2]) + 0.5,
        ],
        block_position: pos,
        yaw: 0.0,
        pitch: 0.0,
        villager: None,
    }
}

fn combine(parts: Vec<(StructureTemplate, [i32; 3])>, size: [i32; 3]) -> StructureTemplate {
    let mut blocks = BTreeMap::new();
    let mut chests = Vec::new();
    let mut entities = Vec::new();
    let mut mobs = Vec::new();
    for (t, o) in parts {
        let p = |a: [i32; 3]| [a[0] + o[0], a[1] + o[1], a[2] + o[2]];
        for b in t.blocks() {
            blocks.insert(p(b.pos), b.state);
        }
        for c in t.chests() {
            let mut c = c.clone();
            c.pos = p(c.pos);
            chests.push(c);
        }
        for e in t.block_entities() {
            let mut e = e.clone();
            e.pos = p(e.pos);
            entities.push(e);
        }
        for e in t.entities() {
            let mut e = e.clone();
            e.block_position = p(e.block_position);
            for (v, offset) in e.position.iter_mut().zip(o) {
                *v += f64::from(offset);
            }
            mobs.push(e);
        }
    }
    StructureTemplate::new(
        size,
        blocks
            .into_iter()
            .map(|(pos, state)| TemplateBlock { pos, state })
            .collect(),
    )
    .with_chests(chests)
    .with_block_entities(entities)
    .with_entities(mobs)
}

fn igloos(root: &Path, r: &BlockRegistry) -> Result<Vec<StructureTemplate>, String> {
    let top = asset(root, "igloo/top", "igloo", r)?;
    let mut parts = vec![
        (top.clone(), [0, 0, 0]),
        (asset(root, "igloo/bottom", "igloo", r)?, [0, -24, -2]),
    ];
    for y in (-18..=-3).step_by(3) {
        parts.push((asset(root, "igloo/middle", "igloo", r)?, [2, y, 4]));
    }
    Ok(vec![top, combine(parts, [7, 5, 9])])
}
fn outpost(root: &Path, r: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut parts = vec![(
        asset(root, "pillager_outpost/watchtower", "pillager_outpost", r)?,
        [16, 0, 16],
    )];
    for (name, pos) in [
        ("feature_cage_with_allays", [0, 0, 0]),
        ("feature_cage1", [32, 0, 0]),
        ("feature_tent1", [0, 0, 24]),
        ("feature_logs", [32, 0, 28]),
    ] {
        parts.push((
            asset(
                root,
                &format!("pillager_outpost/{name}"),
                "pillager_outpost",
                r,
            )?,
            pos,
        ));
    }
    let t = combine(parts, [48, 21, 48]);
    let mut entities = t.entities().to_vec();
    entities.push(mob("pillager", [23, 1, 29]));
    Ok(t.with_entities(entities))
}
fn desert_pyramid(r: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(r);
    b.fill([0, 11, 0], [20, 11, 20], "minecraft:sandstone")?;
    for y in 12..=21 {
        let inset = y - 12;
        b.fill(
            [inset, y, inset],
            [20 - inset, y, 20 - inset],
            "minecraft:sandstone",
        )?;
    }
    b.fill([5, 12, 5], [15, 16, 15], "minecraft:air")?;
    for x in [0, 16] {
        b.fill([x, 12, 0], [x + 4, 23, 4], "minecraft:cut_sandstone")?;
        b.fill([x + 1, 12, 1], [x + 3, 21, 3], "minecraft:air")?;
    }
    b.fill([9, 12, 0], [11, 14, 7], "minecraft:air")?;
    b.fill([7, 0, 7], [13, 5, 13], "minecraft:sandstone")?;
    b.fill([8, 1, 8], [12, 4, 12], "minecraft:air")?;
    b.fill([10, 5, 10], [10, 10, 10], "minecraft:air")?;
    b.fill([9, -1, 9], [11, -1, 11], "minecraft:tnt")?;
    b.put([10, 1, 10], "minecraft:stone_pressure_plate")?;
    b.put([10, 11, 10], "minecraft:blue_terracotta")?;
    for p in [[9, 11, 10], [11, 11, 10], [10, 11, 9], [10, 11, 11]] {
        b.put(p, "minecraft:orange_terracotta")?;
    }
    for p in [[8, 1, 10], [12, 1, 10], [10, 1, 8], [10, 1, 12]] {
        b.chest(p, "minecraft:chests/desert_pyramid")?;
    }
    // Buried archaeology chamber at the rear of the pyramid.
    b.fill([2, 5, 15], [6, 5, 19], "minecraft:sandstone")?;
    b.fill([2, 6, 15], [6, 10, 19], "minecraft:sand")?;
    for pos in [
        [2, 6, 15],
        [4, 6, 15],
        [6, 6, 15],
        [3, 6, 17],
        [5, 6, 17],
        [2, 6, 19],
        [6, 6, 19],
    ] {
        b.archaeology(
            pos,
            "minecraft:suspicious_sand",
            "minecraft:archaeology/desert_pyramid",
        )?;
    }
    Ok(b.finish([21, 24, 21]))
}
fn jungle_pyramid(r: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(r);
    b.fill([0, 0, 0], [11, 10, 14], "minecraft:mossy_cobblestone")?;
    for (min, max) in [
        ([1, 1, 1], [10, 3, 13]),
        ([1, 5, 1], [10, 7, 13]),
        ([3, 9, 3], [8, 9, 11]),
    ] {
        b.fill(min, max, "minecraft:air")?;
    }
    b.fill([4, 5, 0], [6, 7, 2], "minecraft:air")?;
    for n in 0..5 {
        b.fill([4, n, 8 - n], [6, n, 8 - n], "minecraft:cobblestone")?;
    }
    for z in [4, 9] {
        for x in 2..10 {
            b.put_properties(
                [x, 1, z],
                "minecraft:tripwire",
                &[
                    ("attached", "true"),
                    ("east", "true"),
                    ("west", "true"),
                    ("north", "false"),
                    ("south", "false"),
                    ("powered", "false"),
                    ("disarmed", "false"),
                ],
            )?;
        }
        for (x, facing) in [(1, "east"), (10, "west")] {
            b.put_properties(
                [x, 1, z],
                "minecraft:tripwire_hook",
                &[
                    ("attached", "true"),
                    ("facing", facing),
                    ("powered", "false"),
                ],
            )?;
        }
        let (x, facing) = if z == 4 { (0, "east") } else { (11, "west") };
        let pos = [x, 1, z];
        let state = b.put_properties(
            pos,
            "minecraft:dispenser",
            &[("facing", facing), ("triggered", "false")],
        )?;
        b.entities.push(TemplateBlockEntity {
            pos,
            state,
            nbt: vec![
                ("id".into(), Tag::String("minecraft:dispenser".into())),
                (
                    "Items".into(),
                    Tag::List(mc_nbt::ListTag {
                        element_type: 10,
                        elements: vec![Tag::Compound(vec![
                            ("Slot".into(), Tag::Byte(0)),
                            ("id".into(), Tag::String("minecraft:arrow".into())),
                            ("count".into(), Tag::Int(9)),
                        ])],
                    }),
                ),
            ],
        });
    }
    for x in 2..5 {
        b.put([x, 6, 12], "minecraft:lever")?;
    }
    b.chest([8, 1, 12], "minecraft:chests/jungle_temple")?;
    b.chest([2, 1, 12], "minecraft:chests/jungle_temple")?;
    Ok(b.finish([12, 11, 15]))
}
fn swamp_hut(r: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(r);
    for x in [0, 6] {
        for z in [0, 8] {
            b.fill([x, -4, z], [x, 3, z], "minecraft:oak_log")?;
        }
    }
    b.fill([0, 0, 0], [6, 4, 8], "minecraft:spruce_planks")?;
    b.fill([1, 1, 1], [5, 3, 7], "minecraft:air")?;
    b.fill([0, 1, 3], [0, 2, 5], "minecraft:air")?;
    b.fill([2, 1, 0], [4, 2, 0], "minecraft:air")?;
    b.put([1, 1, 6], "minecraft:crafting_table")?;
    b.put([5, 1, 6], "minecraft:cauldron")?;
    b.put([1, 1, 2], "minecraft:potted_red_mushroom")?;
    Ok(b.finish([7, 5, 9])
        .with_entities(vec![mob("witch", [3, 1, 5]), mob("cat", [3, 1, 3])]))
}
fn monument(r: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(r);
    b.fill([0, 0, 0], [57, 0, 57], "minecraft:prismarine")?;
    for x in [2, 21, 40] {
        for z in [4, 23, 42] {
            b.fill(
                [x, 1, z],
                [x + 15, 9, z + 13],
                "minecraft:prismarine_bricks",
            )?;
            b.fill([x + 1, 2, z + 1], [x + 14, 8, z + 12], "minecraft:water")?;
            b.fill([x + 6, 2, z], [x + 8, 5, z + 13], "minecraft:water")?;
            b.fill([x, 2, z + 5], [x + 15, 5, z + 7], "minecraft:water")?;
            b.put([x + 2, 7, z + 2], "minecraft:sea_lantern")?;
        }
    }
    b.fill([22, 10, 23], [35, 16, 36], "minecraft:prismarine_bricks")?;
    b.fill([23, 11, 24], [34, 15, 35], "minecraft:water")?;
    b.fill([26, 2, 27], [31, 6, 32], "minecraft:dark_prismarine")?;
    b.fill([28, 3, 29], [29, 4, 30], "minecraft:gold_block")?;
    b.fill([6, 2, 8], [8, 5, 10], "minecraft:wet_sponge")?;
    Ok(b.finish([58, 17, 58]).with_entities(vec![
        mob("elder_guardian", [8, 3, 12]),
        mob("elder_guardian", [48, 3, 12]),
        mob("elder_guardian", [28, 12, 30]),
    ]))
}
fn mansion(root: &Path, r: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(r);
    b.fill([0, 0, 0], [47, 17, 47], "minecraft:dark_oak_planks")?;
    for y in [1, 9] {
        b.fill([1, y, 1], [46, y + 6, 46], "minecraft:air")?;
    }
    b.fill([20, 1, 0], [26, 4, 47], "minecraft:air")?;
    for y in [3, 11] {
        for x in (3..45).step_by(8) {
            b.fill([x, y, 0], [x + 3, y + 2, 0], "minecraft:glass_pane")?;
            b.fill([x, y, 47], [x + 3, y + 2, 47], "minecraft:glass_pane")?;
        }
    }
    for n in 0..8 {
        b.fill(
            [21, n, 12 + n],
            [25, n, 12 + n],
            "minecraft:dark_oak_planks",
        )?;
    }
    let mut parts = vec![(b.finish([48, 18, 48]), [0, 0, 0])];
    for (i, name) in ["1x1_a1", "1x1_a2", "1x1_a3", "1x1_a4", "1x1_b1", "1x1_b2"]
        .iter()
        .enumerate()
    {
        parts.push((
            asset(root, &format!("woodland_mansion/{name}"), "mansion", r)?,
            [2 + (i as i32 % 2) * 30, 1, (i as i32 / 2) * 12 + 2],
        ));
    }
    let t = combine(parts, [48, 18, 48]);
    let mut entities = t.entities().to_vec();
    entities.extend([
        mob("evoker", [30, 9, 30]),
        mob("vindicator", [10, 1, 20]),
        mob("vindicator", [35, 1, 20]),
    ]);
    Ok(t.with_entities(entities))
}
