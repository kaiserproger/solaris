//! Bounded, chunk-addressed decoration plans compiled from placed-feature data.
use super::*;

pub(super) struct Feature {
    name: String,
    salt: i64,
    count: u32,
    rarity: u64,
    height: Option<(i32, i32)>,
    biomes: HashSet<Identifier>,
    templates: Vec<StructureTemplate>,
    dungeon_air: Option<[BlockStateId; 3]>,
    fluids: [BlockStateId; 2],
    reach: [i32; 2],
}

pub(super) fn load(root: &Path, registry: &BlockRegistry) -> Result<Vec<Feature>, String> {
    let mut features = Vec::new();
    for (index, name) in [
        "monster_room",
        "monster_room_deep",
        "desert_well",
        "fossil_upper",
        "fossil_lower",
        "amethyst_geode",
    ]
    .into_iter()
    .enumerate()
    {
        let placed = json(&root.join(format!(
            "data/minecraft/worldgen/placed_feature/{name}.json"
        )))?;
        let mut count = 1;
        let mut rarity = 1;
        let mut height = None;
        for modifier in placed["placement"]
            .as_array()
            .ok_or("missing feature placement")?
        {
            match modifier["type"].as_str().ok_or("missing placement type")? {
                "minecraft:count" => {
                    count = modifier["count"]
                        .as_u64()
                        .ok_or("unsupported feature count")? as u32
                }
                "minecraft:rarity_filter" => {
                    rarity = modifier["chance"]
                        .as_u64()
                        .ok_or("invalid feature rarity")?
                }
                "minecraft:height_range" => {
                    let h = &modifier["height"];
                    if h["type"].as_str() != Some("minecraft:uniform") {
                        return Err("unsupported feature height provider".into());
                    }
                    let anchor = |v: &serde_json::Value| -> Result<i32, String> {
                        if let Some(n) = v["absolute"].as_i64() {
                            Ok(n as i32)
                        } else if let Some(n) = v["above_bottom"].as_i64() {
                            Ok(-64 + n as i32)
                        } else if let Some(n) = v["below_top"].as_i64() {
                            Ok(319 - n as i32)
                        } else {
                            Err("invalid feature height anchor".into())
                        }
                    };
                    height = Some((anchor(&h["min_inclusive"])?, anchor(&h["max_inclusive"])?));
                }
                "minecraft:biome" | "minecraft:in_square" | "minecraft:heightmap" => {}
                other => return Err(format!("unsupported feature placement {other}")),
            }
        }
        if rarity == 0 || count > 32 || height.is_some_and(|(min, max)| min > max) {
            return Err("invalid feature placement bounds".into());
        }
        let feature_id = placed["feature"]
            .as_str()
            .ok_or("missing configured feature")?;
        let config = json(&root.join(format!(
                "data/minecraft/worldgen/configured_feature/{}.json",
                feature_id
                    .strip_prefix("minecraft:")
                    .ok_or("unsupported feature namespace")?
            )))?;
        let templates = match name {
            "monster_room" | "monster_room_deep" => ["zombie", "zombie", "skeleton", "spider"]
                .into_iter()
                .map(|mob| dungeon(registry, mob))
                .collect::<Result<Vec<_>, _>>()?,
            "desert_well" => vec![well(registry)?],
            "amethyst_geode" => vec![geode(registry, &config["config"])?],
            _ => fossils(root, registry, &config["config"], name == "fossil_lower")?,
        };
        let mut biomes = HashSet::new();
        for entry in std::fs::read_dir(root.join("data/minecraft/worldgen/biome"))
            .map_err(|e| e.to_string())?
        {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let biome = json(&path)?;
            if biome["features"].as_array().is_some_and(|steps| {
                steps.iter().any(|step| {
                    step.as_array().is_some_and(|values| {
                        values
                            .iter()
                            .any(|v| v.as_str() == Some(&format!("minecraft:{name}")))
                    })
                })
            }) {
                biomes.insert(id(&format!(
                    "minecraft:{}",
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or("invalid biome name")?
                )));
            }
        }
        let mut reach = [0; 2];
        for template in &templates {
            let (_, max) = template.bounds();
            for (axis, coordinate) in [0, 2].into_iter().enumerate() {
                reach[axis] = reach[axis].max((max[coordinate] + 15).div_euclid(16));
            }
        }
        features.push(Feature {
            name: name.into(),
            salt: 0x46454154 + index as i64,
            count,
            rarity,
            height,
            biomes,
            reach,
            templates,
            dungeon_air: if name.starts_with("monster_room") {
                Some([
                    state(registry, "minecraft:air")?,
                    state(registry, "minecraft:cave_air")?,
                    state(registry, "minecraft:void_air")?,
                ])
            } else {
                None
            },
            fluids: [
                state(registry, "minecraft:water")?,
                state(registry, "minecraft:lava")?,
            ],
        });
    }
    Ok(features)
}

pub(super) fn paste(
    features: &[Feature],
    seed: i64,
    chunk: &mut Chunk,
    surface: &impl Fn(i32, i32) -> i32,
    biome: &impl Fn(i32, i32, i32) -> Identifier,
) -> [bool; 256] {
    let mut touched = [false; 256];
    let pos = chunk.pos;
    // Include every source chunk whose cached template bounds can overlap.
    for f in features {
        for cx in pos.x - f.reach[0]..=pos.x {
            for cz in pos.z - f.reach[1]..=pos.z {
                if f.dungeon_air.is_some() && (cx != pos.x || cz != pos.z) {
                    continue;
                }
                let random = hash(seed ^ f.salt, cx, cz);
                if !random.is_multiple_of(f.rarity) {
                    continue;
                }
                for attempt in 0..f.count {
                    let random = hash(random as i64, attempt as i32, 1);
                    // Dungeon admission reads the already generated local cavity. Keep
                    // its nine-block envelope inside one chunk; no neighbour generation.
                    let mask = if f.dungeon_air.is_some() { 7 } else { 15 };
                    let x = cx * 16 + (random & mask) as i32;
                    let z = cz * 16 + ((random >> 4) & mask) as i32;
                    let h = surface(x, z);
                    let y = f.height.map_or(h + 1, |(min, max)| {
                        min + ((random >> 8) % ((max - min + 1) as u64)) as i32
                    });
                    if f.height.is_some() && y > h - 8 {
                        continue;
                    }
                    if !f.biomes.contains(&biome(x, y, z)) {
                        continue;
                    }
                    // A well must be exposed desert terrain, never the seafloor.
                    if f.name == "desert_well" && h < 63 {
                        continue;
                    }
                    let template = &f.templates[((random >> 32) as usize) % f.templates.len()];
                    let mut openings = [(0u8, 0i32, 0u8, BlockStateId(0)); 56];
                    let mut opening_cells = 0;
                    if let Some(air) = &f.dungeon_air {
                        let local = |dx: i32, dy: i32, dz: i32| {
                            chunk.get_block(
                                (x + dx).rem_euclid(16) as u8,
                                y + dy,
                                (z + dz).rem_euclid(16) as u8,
                            )
                        };
                        let solid = |s: Option<BlockStateId>| {
                            s.is_some_and(|s| !air.contains(&s) && !f.fluids.contains(&s))
                        };
                        if !(0..9).all(|dx| {
                            (0..9).all(|dz| solid(local(dx, 0, dz)) && solid(local(dx, 5, dz)))
                        }) {
                            continue;
                        }
                        let mut doors = 0;
                        for a in 1..8 {
                            for (dx, dz) in [(0, a), (8, a), (a, 0), (a, 8)] {
                                if [1, 2]
                                    .into_iter()
                                    .all(|dy| local(dx, dy, dz).is_some_and(|s| air.contains(&s)))
                                {
                                    doors += 1;
                                    for dy in [1, 2] {
                                        openings[opening_cells] = (
                                            (x + dx).rem_euclid(16) as u8,
                                            y + dy,
                                            (z + dz).rem_euclid(16) as u8,
                                            local(dx, dy, dz).expect("checked opening"),
                                        );
                                        opening_cells += 1;
                                    }
                                }
                            }
                        }
                        if !(1..=5).contains(&doors) {
                            continue;
                        }
                    }
                    let changed = paste_template(chunk, template, BlockPos { x, y, z }, seed);
                    for &(lx, ly, lz, state) in &openings[..opening_cells] {
                        chunk
                            .set_block(lx, ly, lz, state)
                            .expect("admitted dungeon opening");
                    }
                    for (t, c) in touched.iter_mut().zip(changed) {
                        *t |= c;
                    }
                }
            }
        }
    }
    touched
}

fn dungeon(r: &BlockRegistry, mob: &str) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(r);
    b.fill([0, 0, 0], [8, 5, 8], "minecraft:cobblestone")?;
    b.fill([1, 1, 1], [7, 4, 7], "minecraft:air")?;
    for x in 1..8 {
        for z in 1..8 {
            if (x + z) % 4 != 0 {
                b.put([x, 0, z], "minecraft:mossy_cobblestone")?;
            }
        }
    }
    b.spawner([4, 1, 4], &format!("minecraft:{mob}"))?;
    b.chest([1, 1, 2], "minecraft:chests/simple_dungeon")?;
    b.chest([7, 1, 6], "minecraft:chests/simple_dungeon")?;
    Ok(b.finish([9, 6, 9]))
}
fn well(r: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(r);
    b.fill([0, -1, 0], [4, 0, 4], "minecraft:sandstone")?;
    for p in [[2, 0, 2], [1, 0, 2], [3, 0, 2], [2, 0, 1], [2, 0, 3]] {
        b.put(p, "minecraft:water")?;
    }
    b.put([2, -3, 2], "minecraft:sandstone")?;
    for pos in [[2, -1, 2], [2, -2, 2]] {
        b.archaeology(
            pos,
            "minecraft:suspicious_sand",
            "minecraft:archaeology/desert_well",
        )?;
    }
    for x in [0, 4] {
        for z in [0, 4] {
            b.fill([x, 1, z], [x, 3, z], "minecraft:sandstone")?;
        }
    }
    b.fill([0, 4, 0], [4, 4, 4], "minecraft:sandstone_slab")?;
    b.put([2, 4, 2], "minecraft:sandstone")?;
    Ok(b.finish([5, 5, 5]))
}
fn geode(r: &BlockRegistry, config: &serde_json::Value) -> Result<StructureTemplate, String> {
    let providers = &config["blocks"];
    let resolve = |key: &str| -> Result<BlockStateId, String> {
        state(
            r,
            providers[key]["state"]["Name"]
                .as_str()
                .ok_or("invalid geode state provider")?,
        )
    };
    let outer = resolve("outer_layer_provider")?;
    let middle = resolve("middle_layer_provider")?;
    let inner = resolve("inner_layer_provider")?;
    let budding = resolve("alternate_inner_layer_provider")?;
    let air = resolve("filling_provider")?;
    let mut b = Builder::new(r);
    for x in 0i32..=12 {
        for y in 0i32..=12 {
            for z in 0i32..=12 {
                let d = (x - 6).pow(2) + (y - 6).pow(2) + (z - 6).pow(2);
                if d > 36 {
                    continue;
                }
                let s = if d > 25 {
                    outer
                } else if d > 16 {
                    middle
                } else if d > 9 {
                    if (x * 13 + y * 7 + z) % 12 == 0 {
                        budding
                    } else {
                        inner
                    }
                } else {
                    air
                };
                b.blocks.insert([x, y, z], s);
            }
        }
    }
    b.fill([6, 4, 0], [6, 6, 3], "minecraft:air")?;
    b.put([6, 3, 6], "minecraft:amethyst_cluster")?;
    Ok(b.finish([13, 13, 13]))
}
fn fossils(
    root: &Path,
    r: &BlockRegistry,
    config: &serde_json::Value,
    diamonds: bool,
) -> Result<Vec<StructureTemplate>, String> {
    let structures = config["fossil_structures"]
        .as_array()
        .ok_or("missing fossil structures")?;
    let overlays = config["overlay_structures"]
        .as_array()
        .ok_or("missing fossil overlays")?;
    if structures.len() != overlays.len() {
        return Err("mismatched fossil overlays".into());
    }
    let mut result = Vec::new();
    for (structure, overlay) in structures.iter().zip(overlays) {
        let load = |v: &serde_json::Value| -> Result<StructureTemplate, String> {
            let name = v
                .as_str()
                .and_then(|s| s.strip_prefix("minecraft:"))
                .ok_or("invalid fossil reference")?;
            StructureTemplate::from_nbt_file(
                root.join(format!("data/minecraft/structure/{name}.nbt")),
                r,
            )
            .map_err(|e| e.to_string())
        };
        let t = load(structure)?;
        let overlay = load(overlay)?;
        let mut blocks = t.blocks().to_vec();
        let ore = state(
            r,
            if diamonds {
                "minecraft:deepslate_diamond_ore"
            } else {
                "minecraft:coal_ore"
            },
        )?;
        for block in overlay.blocks() {
            if (block.pos[0] + block.pos[1] + block.pos[2]) % 2 == 0 {
                blocks.push(TemplateBlock {
                    pos: block.pos,
                    state: ore,
                });
            }
        }
        result.push(StructureTemplate::new(t.size(), blocks));
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../overworld_features_tests.rs"]
mod tests;
