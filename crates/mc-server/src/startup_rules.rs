use anyhow::{Context, Result, bail};
use mc_data::{
    Identifier,
    biomes::{BiomeSpawnEntry, BiomeSpawnRules, SpawnPlacement},
    entity_types::EntityTypeRegistry,
};
use mc_script::LuaGameplayRules;
use mc_worldgen::{ClayRule, TerrainGenerator, TreeRule};

pub(super) fn apply(
    plan: &LuaGameplayRules,
    generator: &mut TerrainGenerator,
    spawns: &mut BiomeSpawnRules,
    entity_types: &EntityTypeRegistry,
) -> Result<()> {
    for definition in &plan.spawning {
        let biome =
            Identifier::parse(&definition.biome).context("invalid spawn biome identifier")?;
        let mut groups = std::collections::BTreeMap::new();
        for (group, entries) in &definition.groups {
            let mut native = Vec::with_capacity(entries.len());
            for entry in entries {
                let entity =
                    Identifier::parse(&entry.entity).context("invalid spawn entity identifier")?;
                if entity_types.id_of(&entity).is_none() {
                    bail!("startup rule references unavailable entity {entity}");
                }
                native.push(BiomeSpawnEntry {
                    entity_type: entity,
                    min_count: entry.min,
                    max_count: entry.max,
                    weight: entry.weight,
                });
            }
            groups.insert(group.clone(), native);
        }
        spawns.define_biome_spawns(biome, groups);
    }
    if let Some(rule) = &plan.placement {
        let placement =
            SpawnPlacement::new(rule.land_spacing, rule.water_attempts, rule.water_depth)
                .ok_or_else(|| anyhow::anyhow!("invalid native spawn placement"))?;
        spawns.set_placement(placement);
    }
    for definition in &plan.trees {
        let rule = TreeRule::new(definition.spacing, definition.density_threshold)
            .ok_or_else(|| anyhow::anyhow!("invalid native tree rule"))?;
        for name in &definition.biomes {
            generator
                .define_tree_rule(Identifier::parse(name)?, rule)
                .map_err(anyhow::Error::msg)?;
        }
    }
    if let Some(rule) = &plan.clay {
        let rule = ClayRule::new(
            rule.rarity,
            rule.radius_min,
            rule.radius_max,
            rule.max_water_depth,
        )
        .ok_or_else(|| anyhow::anyhow!("invalid native clay rule"))?;
        generator
            .define_clay_rule(rule)
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "startup_rules_tests.rs"]
mod tests;
