use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};

/// Immutable gameplay data validated before world preparation or network startup.
pub struct StartupData {
    pub data: Arc<mc_data::VanillaData>,
    pub blocks: Arc<mc_world::BlockRegistry>,
    pub block_light: Arc<mc_data::block_light::BlockLightTable>,
    pub items: Arc<mc_data::items::ItemRegistry>,
    pub item_facts: Arc<mc_data::item_components::ItemFactsTable>,
    pub tags: Arc<mc_data::tags::TagsData>,
    pub recipes: Arc<Vec<mc_data::recipes::Recipe>>,
    pub loot: Arc<mc_data::loot::LootTables>,
    pub block_facts: Arc<mc_data::block_facts::BlockFactsTable>,
    pub entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
    pub biome_spawns: Arc<mc_data::biomes::BiomeSpawnRules>,
}

impl StartupData {
    pub fn load(
        vanilla_data_dir: &Path,
        loader_manifest: Option<&mc_net::LoaderManifest>,
    ) -> Result<Self> {
        let source = "vanilla_content_cache";
        let data = Arc::new(load_effective_protocol_data(vanilla_data_dir)?);
        tracing::info!(
            registries = data.registry_count(),
            entries = data.entry_count(),
            source,
            "registry index loaded"
        );
        let mut blocks_report = mc_data::blocks::solaris_required_blocks_report();
        let mut block_light = load_effective_block_light(vanilla_data_dir, &blocks_report)?;
        tracing::info!(version = %block_light.version, states = block_light.len(), source, "block-light table loaded");
        let block_mining = load_effective_block_mining(vanilla_data_dir, &blocks_report)?;
        tracing::info!(
            states = block_mining.len(),
            source,
            "block-mining table loaded"
        );
        if let Some(manifest) = loader_manifest {
            for state_id in manifest
                .append_world_block_report(&mut blocks_report)
                .context("registering Solaris Loader world blocks")?
            {
                anyhow::ensure!(
                    usize::try_from(state_id).ok() == Some(block_light.len()),
                    "Solaris Loader block state must follow the validated light table"
                );
                block_light.append_opaque_state();
            }
        }
        let blocks = Arc::new(
            mc_world::BlockRegistry::from_report(&blocks_report)
                .context("building block-state registry from embedded JSON")?,
        );
        tracing::info!(
            blocks = blocks_report.len(),
            states = blocks_report
                .iter()
                .map(|block| block.states.len())
                .sum::<usize>(),
            "server block registry loaded"
        );
        let block_explosion = load_effective_block_explosion(vanilla_data_dir)?;
        tracing::info!(
            states = block_explosion.len(),
            source,
            "block-explosion table loaded"
        );
        let items = Arc::new(mc_data::items::solaris_required_items());
        tracing::info!(entries = items.len(), "embedded item registry loaded");
        let item_facts = Arc::new(load_effective_item_facts(vanilla_data_dir)?);
        tracing::info!(
            entries = item_facts.len(),
            source,
            "item component facts loaded"
        );
        let tags = Arc::new(load_effective_tags(vanilla_data_dir, &data, &items)?);
        tracing::info!(
            tags = tags.total_tags(),
            entries = tags.total_entries(),
            source,
            "tags loaded"
        );
        let recipes = load_effective_recipes(vanilla_data_dir)?;
        validate_recipe_result_stacks(&recipes, &item_facts)?;
        tracing::info!(
            entries = recipes.len(),
            source = "vanilla_content_cache+stable_embedded_prefix",
            "recipe registry loaded"
        );
        let loot = Arc::new(load_effective_loot(vanilla_data_dir)?);
        tracing::info!(
            drops = loot.total_drops(),
            source = "vanilla_content_cache_simple_subset+embedded_fallback",
            "survival loot tables loaded"
        );
        let mut block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report_with_mining(
            &blocks_report,
            Some(&block_mining),
        );
        block_facts = block_facts.with_explosion_table(block_explosion);
        tracing::info!(
            states = block_facts.len(),
            random_tick_states = block_facts.eligible_states(),
            "block simulation facts built from blocks report"
        );
        let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
        tracing::info!(
            entries = entity_types.len(),
            "embedded entity type registry loaded"
        );
        let biome_spawns = Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules());
        tracing::info!(
            biomes = biome_spawns.len(),
            "embedded biome spawn rules loaded"
        );
        Ok(Self {
            data,
            blocks,
            block_light: Arc::new(block_light),
            items,
            item_facts,
            tags,
            recipes: Arc::new(recipes),
            loot,
            block_facts: Arc::new(block_facts),
            entity_types,
            biome_spawns,
        })
    }
}

pub fn load_effective_protocol_data(vanilla_data_dir: &Path) -> Result<mc_data::VanillaData> {
    validate_vanilla_sidecar_version(vanilla_data_dir)?;
    mc_data::load(vanilla_data_dir).with_context(|| {
        format!(
            "loading vanilla registry data from {}",
            vanilla_data_dir.display()
        )
    })
}

pub fn load_effective_tags(
    vanilla_data_dir: &Path,
    data: &mc_data::VanillaData,
    items: &mc_data::items::ItemRegistry,
) -> Result<mc_data::tags::TagsData> {
    let tags = mc_data::tags::load(vanilla_data_dir, data)
        .with_context(|| format!("loading vanilla tags from {}", vanilla_data_dir.display()))?
        .with_vanilla_fuel_values(items);
    if tags.total_tags() == 0 {
        bail!(
            "vanilla tags from {} were empty; import a complete content cache with `mc-server content import`",
            vanilla_data_dir.display()
        );
    }
    for registry in ["minecraft:block", "minecraft:item", "minecraft:entity_type"] {
        let registry_id = mc_data::Identifier::parse(registry).expect("static registry id");
        let missing = match tags.registries.get(&registry_id) {
            Some(entries) => !entries.values().any(|ids| !ids.is_empty()),
            None => true,
        };
        if missing {
            bail!(
                "vanilla tags from {} missing required resolved entries for tag registry {registry}",
                vanilla_data_dir.display()
            );
        }
    }
    if !tags.fuel_values().matches_default_vanilla_26_1_2(items) {
        bail!(
            "vanilla tags from {} resolved {} furnace fuels instead of the canonical 26.1.2 default set; reimport the content cache",
            vanilla_data_dir.display(),
            tags.fuel_values().fuel_count(),
        );
    }
    Ok(tags)
}

pub fn load_effective_loot(vanilla_data_dir: &Path) -> Result<mc_data::loot::LootTables> {
    let root = vanilla_data_dir
        .join("data")
        .join("minecraft")
        .join("loot_table");
    let mut tables = mc_data::loot::load_vanilla_subset(&root)
        .with_context(|| format!("loading vanilla loot tables from {}", root.display()))?;
    if tables.total_drops() > 0 {
        tables.fill_missing_from(mc_data::loot::builtin());
        tables.fill_missing_entity_items_from(mc_data::loot::builtin());
        return Ok(tables);
    }
    bail!(
        "vanilla loot tables from {} had no supported simple drops; import a complete content cache with `mc-server content import`",
        root.display()
    );
}

pub(crate) fn validate_recipe_result_stacks(
    recipes: &[mc_data::recipes::Recipe],
    item_facts: &mc_data::item_components::ItemFactsTable,
) -> Result<()> {
    for recipe in recipes {
        let Some(max_stack_size) = item_facts
            .get(&recipe.result.item)
            .and_then(|facts| facts.max_stack_size)
        else {
            continue;
        };
        if recipe.result.count > max_stack_size {
            bail!(
                "recipe {} produces {} x {}, exceeding the item's max stack size {}",
                recipe.id,
                recipe.result.count,
                recipe.result.item,
                max_stack_size
            );
        }
    }
    Ok(())
}

pub fn load_effective_recipes(vanilla_data_dir: &Path) -> Result<Vec<mc_data::recipes::Recipe>> {
    let root = vanilla_data_dir
        .join("data")
        .join("minecraft")
        .join("recipe");
    let recipes_from_cache = mc_data::recipes::load_recipes(&root)
        .with_context(|| format!("loading vanilla recipes from {}", root.display()))?;
    if recipes_from_cache.is_empty() {
        bail!(
            "vanilla recipes from {} had no supported recipes; import a complete content cache with `mc-server content import`",
            root.display()
        );
    }
    let mut cache_by_id: BTreeMap<_, _> = recipes_from_cache
        .into_iter()
        .map(|recipe| (recipe.id.clone(), recipe))
        .collect();
    let embedded = mc_data::recipes::solaris_required_recipes();
    let mut recipes = Vec::with_capacity(embedded.len() + cache_by_id.len());
    for fallback in embedded {
        let recipe = cache_by_id.remove(&fallback.id).unwrap_or(fallback);
        recipes.push(recipe);
    }
    recipes.extend(cache_by_id.into_values());
    Ok(recipes)
}

pub fn load_effective_block_explosion(
    vanilla_data_dir: &Path,
) -> Result<mc_data::block_explosion::BlockExplosionTable> {
    let path = vanilla_data_dir
        .join("reports")
        .join("block_explosion.json");
    mc_data::block_explosion::load_block_explosion_report(&path).with_context(|| {
        format!(
            "loading vanilla block-explosion table from {}",
            path.display()
        )
    })
}

pub fn load_effective_block_mining(
    vanilla_data_dir: &Path,
    blocks_report: &[mc_data::blocks::BlockReport],
) -> Result<mc_data::block_mining::BlockMiningTable> {
    let path = vanilla_data_dir.join("reports").join("block_mining.json");
    let table = mc_data::block_mining::load(&path)
        .with_context(|| format!("loading vanilla block-mining table from {}", path.display()))?;
    if let Some(max_state_id) = blocks_report
        .iter()
        .flat_map(|block| block.states.iter().map(|state| state.id as usize))
        .max()
        && table.len() <= max_state_id
    {
        bail!(
            "vanilla block-mining table from {} has {} states but blocks report requires state id {max_state_id}",
            path.display(),
            table.len()
        );
    }
    if table.version != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla block-mining table from {} targets {} but Solaris targets {}",
            path.display(),
            table.version,
            mc_protocol::TARGET_RELEASE
        );
    }
    Ok(table)
}

pub fn load_effective_item_facts(
    vanilla_data_dir: &Path,
) -> Result<mc_data::item_components::ItemFactsTable> {
    let path = vanilla_data_dir
        .join("reports")
        .join("minecraft")
        .join("components")
        .join("item");
    let table = mc_data::item_components::load_item_facts(&path).with_context(|| {
        format!(
            "loading vanilla item component facts from {}",
            path.display()
        )
    })?;
    if table.is_empty() {
        bail!(
            "vanilla item component facts from {} were empty; reimport the content cache",
            path.display()
        );
    }
    Ok(table)
}

pub fn load_effective_block_light(
    vanilla_data_dir: &Path,
    blocks_report: &[mc_data::blocks::BlockReport],
) -> Result<mc_data::block_light::BlockLightTable> {
    let path = vanilla_data_dir.join("reports").join("block_light.json");
    let table = mc_data::block_light::load(&path)
        .with_context(|| format!("loading vanilla block-light table from {}", path.display()))?;
    if let Some(max_state_id) = blocks_report
        .iter()
        .flat_map(|block| block.states.iter().map(|state| state.id as usize))
        .max()
        && table.len() <= max_state_id
    {
        bail!(
            "vanilla block-light table from {} has {} states but blocks report requires state id {max_state_id}",
            path.display(),
            table.len()
        );
    }
    if table.version != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla block-light table from {} targets {} but Solaris targets {}",
            path.display(),
            table.version,
            mc_protocol::TARGET_RELEASE
        );
    }
    Ok(table)
}

#[derive(serde::Deserialize)]
struct VanillaVersionMetadata {
    id: String,
    world_version: u32,
    protocol_version: i32,
}

pub fn validate_vanilla_sidecar_version(vanilla_data_dir: &Path) -> Result<()> {
    let metadata = std::fs::metadata(vanilla_data_dir).with_context(|| {
        format!(
            "reading vanilla sidecar directory metadata for {}",
            vanilla_data_dir.display()
        )
    })?;
    if !metadata.is_dir() {
        bail!(
            "data.vanilla_data_dir is not a directory: {}",
            vanilla_data_dir.display()
        );
    }

    let path = vanilla_data_dir.join("version.json");
    let raw = std::fs::read(&path)
        .with_context(|| format!("reading vanilla sidecar version from {}", path.display()))?;
    let version = serde_json::from_slice::<VanillaVersionMetadata>(&raw)
        .with_context(|| format!("parsing vanilla sidecar version from {}", path.display()))?;

    if version.id != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla sidecar release id {:?} does not match Solaris target {:?}",
            version.id,
            mc_protocol::TARGET_RELEASE
        );
    }
    if version.world_version != mc_protocol::WORLD_VERSION {
        bail!(
            "vanilla sidecar world_version {} does not match Solaris world version {}",
            version.world_version,
            mc_protocol::WORLD_VERSION
        );
    }
    if version.protocol_version != mc_protocol::PROTOCOL_VERSION {
        bail!(
            "vanilla sidecar protocol_version {} does not match Solaris protocol version {}",
            version.protocol_version,
            mc_protocol::PROTOCOL_VERSION
        );
    }

    Ok(())
}

#[cfg(test)]
#[path = "startup_data_tests.rs"]
mod tests;
