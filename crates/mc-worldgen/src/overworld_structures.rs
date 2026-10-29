//! Immutable generation-time structure plans. Locate and chunk paste share starts.
//! Shapes are Solaris-authored, not copies of decompiled game implementation.
use crate::vanilla_features::{LegacyRandom, RandomSource};
use crate::{
    structures::{StructureTemplate, TemplateBlock, TemplateBlockEntity, TemplateChest},
    village::placement::RandomSpreadPlacement,
};
use mc_data::Identifier;
use mc_nbt::Tag;
use mc_world::{
    BlockPos, BlockRegistry, BlockStateId, ChestBlockEntity, Chunk, chunk::GeneratedStructureSite,
};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

struct Family {
    id: Identifier,
    placement: RandomSpreadPlacement,
    frequency: f64,
    exclusion: Option<(RandomSpreadPlacement, i32)>,
    biomes: HashSet<Identifier>,
    templates: Vec<StructureTemplate>,
    height: Height,
}

#[derive(Clone, Copy)]
enum Height {
    Surface,
    OceanFloor,
    Monument,
    Underground,
    Buried(i32),
}

pub(crate) struct OverworldStructures {
    seed: i64,
    strongholds: Vec<(i32, i32)>,
    stronghold: StructureTemplate,
    stronghold_eye_states: [(BlockStateId, BlockStateId); 4],
    stronghold_portal: BlockStateId,
    families: Vec<Family>,
    features: Vec<features::Feature>,
    reach_before: [i32; 2],
    reach_after: [i32; 2],
}

fn json(path: &Path) -> Result<serde_json::Value, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn id(name: &str) -> Identifier {
    Identifier::parse(name).expect("validated structure identifier")
}
fn hash(seed: i64, x: i32, z: i32) -> u64 {
    let mut v = (seed as u64)
        ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (z as u64).wrapping_mul(0xd1b54a32d192ed03);
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
    v ^ (v >> 31)
}
fn biome_members(
    root: &Path,
    reference: &str,
    out: &mut HashSet<Identifier>,
    depth: usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err("recursive structure biome tag".into());
    }
    if let Some(tag) = reference.strip_prefix('#') {
        let key = Identifier::parse(tag).map_err(|e| format!("invalid biome tag: {e:?}"))?;
        let path = root
            .join("data")
            .join(key.namespace())
            .join("tags/worldgen/biome")
            .join(format!("{}.json", key.path()));
        let data = json(&path)?;
        for value in data["values"]
            .as_array()
            .ok_or("missing biome tag values")?
        {
            let name = value
                .as_str()
                .or_else(|| value["id"].as_str())
                .ok_or("invalid biome tag member")?;
            biome_members(root, name, out, depth + 1)?;
        }
    } else {
        out.insert(Identifier::parse(reference).map_err(|e| format!("invalid biome: {e:?}"))?);
    }
    Ok(())
}

fn random_spread(value: &serde_json::Value) -> Result<RandomSpreadPlacement, String> {
    let integer = |key: &str| {
        value[key]
            .as_i64()
            .ok_or_else(|| format!("missing placement {key}"))
    };
    let placement = RandomSpreadPlacement {
        spacing: i32::try_from(integer("spacing")?).map_err(|_| "placement spacing overflow")?,
        separation: i32::try_from(integer("separation")?)
            .map_err(|_| "placement separation overflow")?,
        salt: integer("salt")?,
        triangular: value["spread_type"].as_str() == Some("triangular"),
    };
    if placement.spacing <= placement.separation || placement.separation < 0 {
        return Err("invalid placement spacing".into());
    }
    Ok(placement)
}

fn near_placement(
    seed: i64,
    placement: RandomSpreadPlacement,
    radius: i32,
    x: i32,
    z: i32,
) -> bool {
    let step = placement.spacing;
    for gx in (x - radius).div_euclid(step)..=(x + radius).div_euclid(step) {
        for gz in (z - radius).div_euclid(step)..=(z + radius).div_euclid(step) {
            let (cx, cz) = placement.potential_chunk(seed, gx * step, gz * step);
            if (cx - x).abs() <= radius && (cz - z).abs() <= radius {
                return true;
            }
        }
    }
    false
}

impl OverworldStructures {
    pub(crate) fn load(root: &Path, seed: i64, blocks: &BlockRegistry) -> Result<Self, String> {
        let p = json(&root.join("data/minecraft/worldgen/structure_set/strongholds.json"))?;
        let p = &p["placement"];
        let count = p["count"].as_u64().ok_or("missing stronghold count")? as usize;
        let distance = p["distance"]
            .as_f64()
            .ok_or("missing stronghold distance")?;
        let mut spread = p["spread"].as_u64().ok_or("missing stronghold spread")? as usize;
        if count > 4096 || spread == 0 || distance <= 0.0 {
            return Err("invalid stronghold rings".into());
        }
        let mut strongholds = Vec::with_capacity(count);
        let mut ring = 0;
        let mut angle = (hash(seed, 0, 0) as f64 / u64::MAX as f64) * std::f64::consts::TAU;
        while strongholds.len() < count {
            let n = spread.min(count - strongholds.len());
            for i in 0..n {
                let jitter = hash(seed, strongholds.len() as i32, 1) as f64 / u64::MAX as f64;
                let radius =
                    distance * (4.0 + 6.0 * f64::from(ring)) + (jitter - 0.5) * distance * 2.5;
                let theta = angle + std::f64::consts::TAU * i as f64 / n as f64;
                strongholds.push((
                    (theta.cos() * radius).round() as i32,
                    (theta.sin() * radius).round() as i32,
                ));
            }
            ring += 1;
            spread += 2 * spread / (ring as usize + 1);
            angle += hash(seed, ring, 2) as f64 / u64::MAX as f64 * std::f64::consts::TAU;
        }
        let mut families = Vec::new();
        for set in [
            "mineshafts",
            "igloos",
            "shipwrecks",
            "ocean_ruins",
            "ruined_portals",
            "pillager_outposts",
            "desert_pyramids",
            "jungle_temples",
            "swamp_huts",
            "buried_treasures",
            "ocean_monuments",
            "woodland_mansions",
        ] {
            let data =
                json(&root.join(format!("data/minecraft/worldgen/structure_set/{set}.json")))?;
            let p = &data["placement"];
            let placement = random_spread(p)?;
            let exclusion = if let Some(zone) = p.get("exclusion_zone") {
                let other = zone["other_set"]
                    .as_str()
                    .and_then(|id| id.strip_prefix("minecraft:"))
                    .ok_or("unsupported exclusion structure set")?;
                let radius = zone["chunk_count"]
                    .as_i64()
                    .filter(|radius| (0..=64).contains(radius))
                    .ok_or("unsupported exclusion radius")? as i32;
                let other = json(&root.join(format!(
                    "data/minecraft/worldgen/structure_set/{other}.json"
                )))?;
                Some((random_spread(&other["placement"])?, radius))
            } else {
                None
            };
            for entry in data["structures"]
                .as_array()
                .ok_or("missing structure entries")?
            {
                let name = entry["structure"]
                    .as_str()
                    .ok_or("missing structure identifier")?
                    .strip_prefix("minecraft:")
                    .ok_or("unsupported structure namespace")?;
                if name == "ruined_portal_nether" {
                    continue;
                }
                let spec =
                    json(&root.join(format!("data/minecraft/worldgen/structure/{name}.json")))?;
                let mut biomes = HashSet::new();
                biome_members(
                    root,
                    spec["biomes"].as_str().ok_or("missing structure biomes")?,
                    &mut biomes,
                    0,
                )?;
                let (templates, height) = catalog::templates(root, name, blocks)?;
                families.push(Family {
                    id: id(&format!("minecraft:{name}")),
                    placement,
                    frequency: p["frequency"].as_f64().unwrap_or(1.0),
                    exclusion,
                    biomes,
                    templates,
                    height,
                });
            }
        }
        let stronghold = stronghold(blocks)?;
        let mut reach_before = [0; 2];
        let mut reach_after = [0; 2];
        for template in
            std::iter::once(&stronghold).chain(families.iter().flat_map(|family| &family.templates))
        {
            let (min, max) = template.bounds();
            for (axis, coordinate) in [0, 2].into_iter().enumerate() {
                reach_before[axis] = reach_before[axis].max(max[coordinate].div_euclid(16));
                reach_after[axis] = reach_after[axis].max((-min[coordinate] + 15).div_euclid(16));
            }
        }
        Ok(Self {
            seed,
            strongholds,
            stronghold,
            stronghold_eye_states: stronghold_eye_states(blocks)?,
            stronghold_portal: state(blocks, "minecraft:end_portal")?,
            families,
            features: features::load(root, blocks)?,
            reach_before,
            reach_after,
        })
    }

    fn visit(
        &self,
        min: (i32, i32),
        max: (i32, i32),
        surface: &impl Fn(i32, i32) -> i32,
        ocean_floor: &impl Fn(i32, i32) -> i32,
        biome: &impl Fn(i32, i32, i32) -> Identifier,
        mut emit: impl FnMut(&Identifier, (i32, i32), BlockPos, &StructureTemplate),
    ) {
        for &(x, z) in &self.strongholds {
            if x < min.0 || z < min.1 || x > max.0 || z > max.1 {
                continue;
            }
            let y = (surface(x * 16 + 8, z * 16 + 8) - 32).clamp(-48, 24);
            emit(
                &id("minecraft:stronghold"),
                (x, z),
                BlockPos {
                    x: x * 16,
                    y,
                    z: z * 16,
                },
                &self.stronghold,
            );
        }
        for family in &self.families {
            let step = family.placement.spacing;
            for gx in min.0.div_euclid(step)..=max.0.div_euclid(step) {
                for gz in min.1.div_euclid(step)..=max.1.div_euclid(step) {
                    let (x, z) = family
                        .placement
                        .potential_chunk(self.seed, gx * step, gz * step);
                    if x < min.0 || z < min.1 || x > max.0 || z > max.1 {
                        continue;
                    }
                    if hash(self.seed ^ family.placement.salt, x, z) as f64 / u64::MAX as f64
                        >= family.frequency
                    {
                        continue;
                    }
                    if family.exclusion.is_some_and(|(placement, radius)| {
                        near_placement(self.seed, placement, radius, x, z)
                    }) {
                        continue;
                    }
                    let h = match family.height {
                        Height::OceanFloor | Height::Monument => {
                            ocean_floor(x * 16 + 8, z * 16 + 8)
                        }
                        _ => surface(x * 16 + 8, z * 16 + 8),
                    };
                    let y = match family.height {
                        Height::Underground => (h - 28).clamp(-40, 32),
                        Height::Surface | Height::OceanFloor => h + 1,
                        Height::Monument => h.min(crate::terrain::SEA_LEVEL - 17),
                        Height::Buried(depth) => h - depth,
                    };
                    if !family.biomes.contains(&biome(x * 16 + 8, y, z * 16 + 8)) {
                        continue;
                    }
                    emit(
                        &family.id,
                        (x, z),
                        BlockPos {
                            x: x * 16,
                            y,
                            z: z * 16,
                        },
                        &family.templates[(hash(self.seed ^ 0x53545255, x, z) as usize)
                            % family.templates.len()],
                    );
                }
            }
        }
    }

    pub(crate) fn sites(
        &self,
        min: (i32, i32),
        max: (i32, i32),
        surface: impl Fn(i32, i32) -> i32,
        ocean_floor: impl Fn(i32, i32) -> i32,
        biome: impl Fn(i32, i32, i32) -> Identifier,
    ) -> Vec<GeneratedStructureSite> {
        let mut sites = Vec::new();
        self.visit(
            min,
            max,
            &surface,
            &ocean_floor,
            &biome,
            |structure, start_chunk, position, _| {
                sites.push(GeneratedStructureSite {
                    structure: structure.clone(),
                    position,
                    start_chunk,
                })
            },
        );
        sites
    }

    pub(crate) fn paste(
        &self,
        chunk: &mut Chunk,
        surface: impl Fn(i32, i32) -> i32,
        ocean_floor: impl Fn(i32, i32) -> i32,
        biome: impl Fn(i32, i32, i32) -> Identifier,
    ) -> [bool; 256] {
        let pos = chunk.pos;
        let mut touched = features::paste(&self.features, self.seed, chunk, &surface, &biome);
        // Only starts whose cached horizontal bounds can intersect this chunk.
        self.visit(
            (pos.x - self.reach_before[0], pos.z - self.reach_before[1]),
            (pos.x + self.reach_after[0], pos.z + self.reach_after[1]),
            &surface,
            &ocean_floor,
            &biome,
            |structure, start_chunk, origin, template| {
                let (min, max) = template.bounds();
                mc_world::structure_starts::insert(
                    chunk,
                    &GeneratedStructureSite {
                        structure: structure.clone(),
                        start_chunk,
                        position: origin,
                    },
                    BlockPos {
                        x: origin.x + min[0],
                        y: origin.y + min[1],
                        z: origin.z + min[2],
                    },
                    BlockPos {
                        x: origin.x + max[0],
                        y: origin.y + max[1],
                        z: origin.z + max[2],
                    },
                );
                let eyes = (structure.path() == "stronghold")
                    .then_some((&self.stronghold_eye_states, self.stronghold_portal));
                let changed = paste_template_with_eyes(chunk, template, origin, self.seed, eyes);
                for (t, c) in touched.iter_mut().zip(changed) {
                    *t |= c;
                }
            },
        );
        touched
    }
}

fn paste_template(
    chunk: &mut Chunk,
    template: &StructureTemplate,
    origin: BlockPos,
    seed: i64,
) -> [bool; 256] {
    paste_template_with_eyes(chunk, template, origin, seed, None)
}

fn paste_template_with_eyes(
    chunk: &mut Chunk,
    template: &StructureTemplate,
    origin: BlockPos,
    seed: i64,
    eyes: Option<(&[(BlockStateId, BlockStateId); 4], BlockStateId)>,
) -> [bool; 256] {
    // Recreate the start's stream for every clipped paste, so frame outcomes
    // do not depend on chunk generation order.
    let mut random = eyes.map(|_| LegacyRandom::new(hash(seed, origin.x, origin.z) as i64));
    let mut frame_count = 0;
    let mut filled_count = 0;
    let mut touched = [false; 256];
    let chunk_pos = chunk.pos;
    let geometry = chunk.geometry();
    let local = |p: [i32; 3]| {
        let pos = BlockPos {
            x: origin.x + p[0],
            y: origin.y + p[1],
            z: origin.z + p[2],
        };
        let (x, z) = (pos.x - chunk_pos.x * 16, pos.z - chunk_pos.z * 16);
        if (0..16).contains(&x)
            && (0..16).contains(&z)
            && geometry.world_y_to_section(pos.y).is_some()
        {
            Some((pos, x as u8, z as u8))
        } else {
            None
        }
    };
    for block in template.blocks() {
        let mut block_state = block.state;
        if let Some((_, filled)) = eyes.and_then(|(states, _)| {
            states
                .iter()
                .find(|(empty, filled)| *empty == block.state || *filled == block.state)
        }) {
            frame_count += 1;
            if block.state == *filled
                || random
                    .as_mut()
                    .expect("eye states have a random stream")
                    .next_float()
                    < 0.1
            {
                block_state = *filled;
                filled_count += 1;
            }
        }
        let pos = BlockPos {
            x: origin.x + block.pos[0],
            y: origin.y + block.pos[1],
            z: origin.z + block.pos[2],
        };
        let (x, z) = (pos.x - chunk_pos.x * 16, pos.z - chunk_pos.z * 16);
        if (0..16).contains(&x)
            && (0..16).contains(&z)
            && chunk
                .set_block(x as u8, pos.y, z as u8, block_state)
                .is_some()
        {
            touched[z as usize * 16 + x as usize] = true;
            chunk.block_entities.remove(&pos);
            chunk.chests.remove(&pos);
            chunk.furnaces.remove(&pos);
            chunk.hoppers.remove(&pos);
            chunk.brewing_stands.remove(&pos);
        }
    }
    if frame_count == 12
        && filled_count == 12
        && let Some((_, portal)) = eyes
    {
        for x in 6..=8 {
            for z in 6..=8 {
                if let Some((pos, x, z)) = local([x, 3, z])
                    && chunk.set_block(x, pos.y, z, portal).is_some()
                {
                    touched[z as usize * 16 + x as usize] = true;
                    chunk.block_entities.remove(&pos);
                    chunk.chests.remove(&pos);
                    chunk.furnaces.remove(&pos);
                    chunk.hoppers.remove(&pos);
                    chunk.brewing_stands.remove(&pos);
                }
            }
        }
    }
    for chest in template.chests() {
        if let Some((pos, _, _)) = local(chest.pos) {
            chunk.chests.insert(
                pos,
                chest.unopened_contents(hash(seed, pos.x, pos.z) ^ pos.y as u64),
            );
        }
    }
    for entity in template.block_entities() {
        if let Some((pos, _, _)) = local(entity.pos) {
            let bytes = if entity.nbt.iter().any(|(key, _)| key == "LootTableSeed") {
                let mut seeded = entity.clone();
                if let Some((_, Tag::Long(value))) = seeded
                    .nbt
                    .iter_mut()
                    .find(|(key, _)| key == "LootTableSeed")
                {
                    *value = (hash(seed, pos.x, pos.z) ^ pos.y as u64) as i64;
                }
                seeded.at_world_position(pos)
            } else {
                entity.at_world_position(pos)
            };
            chunk.block_entities.insert(pos, bytes);
        }
    }
    if !template.entities().is_empty() {
        let mut inhabitants = chunk.generated_entities();
        for (index, entity) in template.entities().iter().enumerate() {
            if local(entity.block_position).is_none() {
                continue;
            }
            let villager = entity.villager.as_ref();
            inhabitants.push(mc_world::chunk::GeneratedEntityMarker {
                claim: if entity.entity_type == "minecraft:chest_minecart" {
                    format!(
                        "mineshaft:{seed}:{}:{}:{}#{index}",
                        origin.x, origin.y, origin.z
                    )
                } else {
                    format!("overworld:{}:{}:{}#{index}", origin.x, origin.y, origin.z)
                },
                entity_type: entity.entity_type.clone(),
                position: [
                    entity.position[0] + f64::from(origin.x),
                    entity.position[1] + f64::from(origin.y),
                    entity.position[2] + f64::from(origin.z),
                ],
                persistent: entity.persistent,
                villager_kind: villager.map_or("plains", |v| v.kind.as_str()).to_owned(),
                profession: villager
                    .map_or("none", |v| v.profession.as_str())
                    .to_owned(),
                level: villager.map_or(1, |v| v.level),
                age: villager.map_or(0, |v| v.age),
                yaw: entity.yaw,
                pitch: entity.pitch,
            });
        }
        chunk.set_generated_entities(&inhabitants);
    }
    touched
}
fn state(blocks: &BlockRegistry, name: &str) -> Result<BlockStateId, String> {
    blocks
        .block(&id(name))
        .map(|block| block.default)
        .ok_or_else(|| format!("missing structure block {name}"))
}
fn clean_template(
    template: StructureTemplate,
    name: &str,
    registry: &BlockRegistry,
) -> Result<StructureTemplate, String> {
    let marker = id("minecraft:structure_block");
    let is_marker = |state| {
        registry
            .by_id(state)
            .is_some_and(|state| state.block.id == marker)
    };
    let air = state(registry, "minecraft:air")?;
    let mut blocks: Vec<_> = template
        .blocks()
        .iter()
        .filter(|b| !is_marker(b.state))
        .copied()
        .collect();
    blocks.extend(template.air_cells().iter().map(|pos| TemplateBlock {
        pos: *pos,
        state: air,
    }));
    blocks.extend(template.jigsaws().iter().map(|j| TemplateBlock {
        pos: j.pos,
        state: j.final_state,
    }));
    let mut chests = template.chests().to_vec();
    for entity in template
        .block_entities()
        .iter()
        .filter(|e| is_marker(e.state))
    {
        let metadata = entity.nbt.iter().find_map(|(k, v)| {
            if k == "metadata" {
                if let Tag::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            } else {
                None
            }
        });
        let table = match (name, metadata) {
            ("shipwreck", Some("supply_chest")) => Some("minecraft:chests/shipwreck_supply"),
            ("shipwreck", Some("map_chest")) => Some("minecraft:chests/shipwreck_map"),
            ("shipwreck", Some("treasure_chest")) => Some("minecraft:chests/shipwreck_treasure"),
            ("ocean_ruin_warm" | "ocean_ruin_cold", Some("chest")) => {
                Some(if template.size()[0] > 10 {
                    "minecraft:chests/underwater_ruin_big"
                } else {
                    "minecraft:chests/underwater_ruin_small"
                })
            }
            ("igloo", Some("chest")) => Some("minecraft:chests/igloo_chest"),
            _ => None,
        };
        blocks.push(TemplateBlock {
            pos: entity.pos,
            state: air,
        });
        if let Some(table) = table {
            if let Some(chest) = chests
                .iter_mut()
                .find(|c| c.pos == [entity.pos[0], entity.pos[1] - 1, entity.pos[2]])
            {
                chest.loot_table = Some(id(table));
            } else {
                blocks.push(TemplateBlock {
                    pos: entity.pos,
                    state: state(registry, "minecraft:chest")?,
                });
                chests.push(TemplateChest {
                    pos: entity.pos,
                    chest: ChestBlockEntity::default(),
                    loot_table: Some(id(table)),
                });
            }
        }
    }
    let mut entities: Vec<_> = template
        .block_entities()
        .iter()
        .filter(|e| !is_marker(e.state))
        .cloned()
        .collect();
    if matches!(name, "ocean_ruin_warm" | "ocean_ruin_cold") {
        let warm = name == "ocean_ruin_warm";
        let ordinary = state(
            registry,
            if warm {
                "minecraft:sand"
            } else {
                "minecraft:gravel"
            },
        )?;
        let suspicious = state(
            registry,
            if warm {
                "minecraft:suspicious_sand"
            } else {
                "minecraft:suspicious_gravel"
            },
        )?;
        let table = if warm {
            "minecraft:archaeology/ocean_ruin_warm"
        } else {
            "minecraft:archaeology/ocean_ruin_cold"
        };
        let mut candidates: Vec<_> = blocks
            .iter_mut()
            .filter(|block| block.state == ordinary)
            .collect();
        candidates.sort_by_key(|block| hash(i64::from(block.pos[1]), block.pos[0], block.pos[2]));
        for block in candidates.into_iter().take(5) {
            block.state = suspicious;
            entities.push(TemplateBlockEntity {
                pos: block.pos,
                state: suspicious,
                nbt: vec![
                    ("id".into(), Tag::String("minecraft:brushable_block".into())),
                    ("LootTable".into(), Tag::String(table.into())),
                    ("LootTableSeed".into(), Tag::Long(0)),
                ],
            });
        }
    }
    Ok(StructureTemplate::new(template.size(), blocks)
        .with_chests(chests)
        .with_entities(template.entities().to_vec())
        .with_block_entities(entities))
}

struct Builder<'a> {
    registry: &'a BlockRegistry,
    blocks: BTreeMap<[i32; 3], BlockStateId>,
    chests: Vec<TemplateChest>,
    entities: Vec<TemplateBlockEntity>,
}
impl<'a> Builder<'a> {
    fn new(registry: &'a BlockRegistry) -> Self {
        Self {
            registry,
            blocks: BTreeMap::new(),
            chests: Vec::new(),
            entities: Vec::new(),
        }
    }
    fn put(&mut self, p: [i32; 3], name: &str) -> Result<(), String> {
        self.blocks.insert(p, state(self.registry, name)?);
        Ok(())
    }
    fn put_properties(
        &mut self,
        pos: [i32; 3],
        name: &str,
        properties: &[(&str, &str)],
    ) -> Result<BlockStateId, String> {
        let block = self
            .registry
            .block(&id(name))
            .ok_or_else(|| format!("missing block {name}"))?;
        let state = block
            .states
            .iter()
            .copied()
            .find(|s| {
                self.registry.by_id(*s).is_some_and(|s| {
                    properties.iter().all(|(k, v)| {
                        s.properties
                            .iter()
                            .any(|(key, value)| key == k && value == v)
                    })
                })
            })
            .ok_or_else(|| format!("missing state {name} {properties:?}"))?;
        self.blocks.insert(pos, state);
        Ok(state)
    }
    fn fill(&mut self, min: [i32; 3], max: [i32; 3], name: &str) -> Result<(), String> {
        let s = state(self.registry, name)?;
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    self.blocks.insert([x, y, z], s);
                }
            }
        }
        Ok(())
    }
    fn room(&mut self, min: [i32; 3], max: [i32; 3]) -> Result<(), String> {
        self.fill(min, max, "minecraft:stone_bricks")?;
        self.fill(
            [min[0] + 1, min[1] + 1, min[2] + 1],
            [max[0] - 1, max[1] - 1, max[2] - 1],
            "minecraft:air",
        )
    }
    fn chest(&mut self, pos: [i32; 3], table: &str) -> Result<(), String> {
        self.put(pos, "minecraft:chest")?;
        self.chests.push(TemplateChest {
            pos,
            chest: ChestBlockEntity::default(),
            loot_table: Some(id(table)),
        });
        Ok(())
    }
    fn archaeology(&mut self, pos: [i32; 3], material: &str, table: &str) -> Result<(), String> {
        self.put(pos, material)?;
        self.entities.push(TemplateBlockEntity {
            pos,
            state: state(self.registry, material)?,
            nbt: vec![
                ("id".into(), Tag::String("minecraft:brushable_block".into())),
                ("LootTable".into(), Tag::String(table.into())),
                ("LootTableSeed".into(), Tag::Long(0)),
            ],
        });
        Ok(())
    }
    fn spawner(&mut self, pos: [i32; 3], mob: &str) -> Result<(), String> {
        self.put(pos, "minecraft:spawner")?;
        let mut nbt = vec![
            ("id".into(), Tag::String("minecraft:mob_spawner".into())),
            (
                "SpawnData".into(),
                Tag::Compound(vec![(
                    "entity".into(),
                    Tag::Compound(vec![("id".into(), Tag::String(mob.into()))]),
                )]),
            ),
        ];
        for (name, value) in [
            ("Delay", 20),
            ("MinSpawnDelay", 200),
            ("MaxSpawnDelay", 800),
            ("SpawnCount", 4),
            ("MaxNearbyEntities", 6),
            ("RequiredPlayerRange", 16),
            ("SpawnRange", 4),
        ] {
            nbt.push((name.into(), Tag::Short(value)));
        }
        self.entities.push(TemplateBlockEntity {
            pos,
            state: state(self.registry, "minecraft:spawner")?,
            nbt,
        });
        Ok(())
    }
    fn finish(self, size: [i32; 3]) -> StructureTemplate {
        StructureTemplate::new(
            size,
            self.blocks
                .into_iter()
                .map(|(pos, state)| TemplateBlock { pos, state })
                .collect(),
        )
        .with_chests(self.chests)
        .with_block_entities(self.entities)
    }
}
fn stronghold_eye_states(
    registry: &BlockRegistry,
) -> Result<[(BlockStateId, BlockStateId); 4], String> {
    let mut states = [(BlockStateId(0), BlockStateId(0)); 4];
    for (index, facing) in ["south", "north", "east", "west"].into_iter().enumerate() {
        let frame = |eye: &str| {
            registry
                .by_name_and_props(
                    &id("minecraft:end_portal_frame"),
                    &[("eye".into(), eye.into()), ("facing".into(), facing.into())],
                )
                .ok_or_else(|| "missing oriented end portal frame".to_owned())
        };
        states[index] = (frame("false")?, frame("true")?);
    }
    Ok(states)
}

fn stronghold(registry: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(registry);
    b.room([0, 0, 0], [14, 7, 16])?;
    b.room([32, 0, 0], [48, 7, 16])?;
    b.room([16, 0, 18], [30, 7, 32])?;
    b.room([13, 0, 5], [33, 4, 9])?;
    b.room([20, 0, 7], [24, 4, 20])?;
    b.fill([13, 1, 6], [33, 3, 8], "minecraft:air")?;
    b.fill([21, 1, 8], [23, 3, 20], "minecraft:air")?;
    b.fill([5, 1, 5], [9, 2, 9], "minecraft:lava")?;
    for (facing, positions) in [
        ("south", vec![[6, 3, 5], [7, 3, 5], [8, 3, 5]]),
        ("north", vec![[6, 3, 9], [7, 3, 9], [8, 3, 9]]),
        ("east", vec![[5, 3, 6], [5, 3, 7], [5, 3, 8]]),
        ("west", vec![[9, 3, 6], [9, 3, 7], [9, 3, 8]]),
    ] {
        let props = vec![
            ("eye".into(), "false".into()),
            ("facing".into(), facing.into()),
        ];
        let frame = registry
            .by_name_and_props(&id("minecraft:end_portal_frame"), &props)
            .ok_or("missing oriented end portal frame")?;
        for pos in positions {
            b.blocks.insert(pos, frame);
        }
    }
    for step in 0..3 {
        b.fill(
            [6, 1, 12 - step],
            [8, 1 + step, 12 - step],
            "minecraft:stone_bricks",
        )?;
    }
    b.spawner([7, 1, 14], "minecraft:silverfish")?;
    for x in [34, 37, 40, 43, 46] {
        b.fill([x, 1, 2], [x, 3, 5], "minecraft:bookshelf")?;
    }
    b.chest([34, 1, 13], "minecraft:chests/stronghold_library")?;
    b.chest([18, 1, 28], "minecraft:chests/stronghold_crossing")?;
    for p in [
        [2, 1, 2],
        [12, 1, 14],
        [34, 1, 7],
        [46, 1, 14],
        [18, 1, 20],
        [28, 1, 30],
    ] {
        b.put(p, "minecraft:torch")?;
    }
    Ok(b.finish([49, 8, 33]))
}
fn mineshaft(registry: &BlockRegistry) -> Result<StructureTemplate, String> {
    let mut b = Builder::new(registry);
    for (min, max) in [([0, 0, 22], [48, 4, 26]), ([22, 0, 0], [26, 4, 48])] {
        b.fill(min, max, "minecraft:oak_planks")?;
        b.fill([min[0], 1, min[2]], [max[0], 3, max[2]], "minecraft:air")?;
    }
    for n in (2..48).step_by(5) {
        for pos in [[n, 1, 22], [n, 1, 26], [22, 1, n], [26, 1, n]] {
            b.fill(pos, [pos[0], 2, pos[2]], "minecraft:oak_fence")?;
        }
        b.fill([n, 3, 22], [n, 3, 26], "minecraft:oak_planks")?;
        b.fill([22, 3, n], [26, 3, n], "minecraft:oak_planks")?;
    }
    for n in 0..49 {
        b.put([24, 1, n], "minecraft:rail")?;
    }
    for n in 4..10 {
        b.put([n, 2, 23], "minecraft:cobweb")?;
    }
    b.spawner([5, 1, 24], "minecraft:cave_spider")?;
    b.put([40, 1, 25], "minecraft:rail")?;
    let cart = crate::structures::TemplateEntity {
        entity_type: "minecraft:chest_minecart".into(),
        persistent: true,
        position: [40.5, 1.0, 25.5],
        block_position: [40, 1, 25],
        yaw: 0.0,
        pitch: 0.0,
        villager: None,
    };
    Ok(b.finish([49, 5, 49]).with_entities(vec![cart]))
}

#[path = "overworld_structures/catalog.rs"]
mod catalog;

#[path = "overworld_structures/features.rs"]
mod features;

#[cfg(test)]
#[path = "overworld_structures_tests.rs"]
mod tests;
