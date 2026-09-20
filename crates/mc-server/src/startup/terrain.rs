use super::*;

pub(crate) fn chest_loot_catalog_for_startup(
    vanilla_data_dir: &Path,
) -> Option<mc_data::loot::chest_26_1_2::ChestLootCatalog> {
    let dir = vanilla_data_dir;
    let tables = [
        mc_worldgen::structures::VILLAGE_TOOLSMITH_LOOT_TABLE,
        mc_worldgen::structures::SOLARIS_RUIN_LOOT_TABLE,
    ];
    let ids = tables
        .iter()
        .map(|table| mc_data::Identifier::parse(*table).expect("static chest loot table"))
        .collect::<Vec<_>>();
    match mc_data::loot::chest_26_1_2::ChestLootCatalog::load_vanilla_tables(dir.join("data"), &ids)
    {
        Ok(catalog) => Some(catalog),
        Err(error) => {
            tracing::warn!(
                %error,
                "structure chest loot tables unavailable; pasting fixed chest contents"
            );
            None
        }
    }
}

/// The arguments are the startup configuration's pieces, each owned by a
/// different layer (data cache, plugin, world contract, village pipeline); the
/// alternative is a bag struct the callers would fill the same way.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_terrain_generator(
    seed: i64,
    worldgen_mode: mc_worldgen::WorldgenMode,
    geometry: mc_world::ChunkGeometry,
    blocks: Arc<mc_world::BlockRegistry>,
    structure_rules: mc_worldgen::StructureRules,
    ore_profile: Option<mc_script::PluginWorldgenOreProfile>,
    chest_loot: Option<(
        mc_data::loot::chest_26_1_2::ChestLootCatalog,
        mc_data::items::ItemRegistry,
    )>,
    village_plans: Option<Arc<mc_worldgen::village::plan_source::VillagePlanSource>>,
) -> Result<Arc<mc_worldgen::TerrainGenerator>> {
    let biomes = mc_worldgen::BiomeRules::vanilla_overworld();
    let mut generator =
        mc_worldgen::TerrainGenerator::try_with_biome_rules(seed, Arc::clone(&blocks), biomes)
            .context("building terrain generator")?
            .with_geometry(geometry)
            .with_mode(worldgen_mode)
            .with_structures(structure_rules);
    if let Some(village_plans) = village_plans {
        generator = generator.with_village_plans(village_plans);
    }
    if let Some(notice) = village_terrain_analogue_notice(generator.village_plan_source().is_some())
    {
        tracing::warn!(code = notice.code, "{}", notice.message);
    }
    if let Some((catalog, items)) = chest_loot {
        generator = generator.with_chest_loot(catalog, items);
    }
    if matches!(
        ore_profile,
        Some(mc_script::PluginWorldgenOreProfile::RealisticDeposits)
    ) {
        generator = generator.with_realistic_deposits(blocks.as_ref());
    }
    Ok(Arc::new(generator))
}
pub(crate) fn structure_rules_for_startup(
    seed: i64,
    worldgen_mode: mc_server::WorldgenMode,
    vanilla_data_dir: &Path,
    blocks: &mc_world::BlockRegistry,
    items: &mc_data::items::ItemRegistry,
    settlement_plan: Option<&mc_script::PluginSettlementPlan>,
    settlement_profile: mc_server::SettlementProfile,
) -> Result<mc_worldgen::StructureRules> {
    if let Some(settlement_plan) = settlement_plan {
        let parts = settlement_plan
            .buildings()
            .iter()
            .map(|building| match building.template() {
                mc_script::PluginSettlementBuildingTemplate::PlainsFountain => {
                    Ok(mc_worldgen::PlainsVillagePrototypePart::Fountain)
                }
                mc_script::PluginSettlementBuildingTemplate::PlainsSmallHouse => {
                    Ok(mc_worldgen::PlainsVillagePrototypePart::SmallHouse)
                }
                mc_script::PluginSettlementBuildingTemplate::PlainsToolsmith => {
                    Ok(mc_worldgen::PlainsVillagePrototypePart::Toolsmith)
                }
                _ => anyhow::bail!("unsupported settlement building template"),
            })
            .collect::<Result<Vec<_>>>()?;
        let inhabitants = settlement_plan
            .inhabitants()
            .iter()
            .map(|inhabitant| {
                let entity_type = match inhabitant.kind() {
                    mc_script::PluginSettlementInhabitantKind::Villager => "minecraft:villager",
                    _ => anyhow::bail!("unsupported settlement inhabitant kind"),
                };
                let profession = match inhabitant.job() {
                    mc_script::PluginSettlementJob::Unemployed => "none",
                    mc_script::PluginSettlementJob::Toolsmith => "toolsmith",
                    _ => anyhow::bail!("unsupported settlement inhabitant job"),
                };
                Ok(mc_worldgen::StructureInhabitant {
                    id: format!("{}:{}", settlement_plan.owner_plugin_id(), inhabitant.id()),
                    entity_type: entity_type.to_owned(),
                    villager_kind: "plains".to_owned(),
                    profession: profession.to_owned(),
                    level: 1,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let rules = mc_worldgen::StructureRules::plains_village_prototype_with_plan(
            vanilla_data_dir,
            blocks,
            &parts,
            inhabitants,
        )
        .context("loading plains village prototype from vanilla structure data")?;
        tracing::info!(
            owner = settlement_plan.owner_plugin_id(),
            buildings = settlement_plan.buildings().len(),
            inhabitants = settlement_plan.inhabitants().len(),
            extensions = settlement_plan.extensions().len(),
            "materialized plugin settlement plan",
        );
        return Ok(if seed == 0 {
            rules.with_fixed_center((72, 8))
        } else {
            rules
        });
    }
    if settlement_profile == mc_server::SettlementProfile::PlainsVillagePrototype {
        // A deployed plugin settlement plan wins; this is the no-plugin path.
        let rules = mc_worldgen::StructureRules::plains_village_prototype(vanilla_data_dir, blocks)
            .context("loading plains village prototype from vanilla structure data")?;
        tracing::info!(
            profile = settlement_profile.name(),
            "materialized built-in settlement prototype",
        );
        return Ok(if seed == 0 {
            rules.with_fixed_center((72, 8))
        } else {
            rules
        });
    }
    // No plugin settlement plan and no prototype opt-in: this is the default
    // `vanilla` profile, whose villages core generates itself (the plan source
    // `village_plan_source_for_startup` builds). `structure_rules_for_startup`
    // owns no village content on this path.
    if seed == 0 && worldgen_mode == mc_server::WorldgenMode::VanillaLike {
        return mc_worldgen::StructureRules::solaris_playable_ruin(blocks, items)
            .context("resolving Solaris playable ruin");
    }
    Ok(mc_worldgen::StructureRules::none())
}

/// Core's own vanilla village generation for a world, or `None` when this world
/// is not core's to village.
///
/// Core villages apply to the default `vanilla` profile only, and only when no
/// component settlement plan is deployed: a deployed plan owns settlement
/// content and wins, so attaching core villages as well would place two villages
/// over one landscape. The `plains_village_prototype` profile keeps its own
/// bounded prototype rules and gets no plan source.
///
/// The source is built from the resolved content cache: the village closure
/// (`minecraft:villages`), the tag set the server already loaded, and the
/// closure's own biome gates. [`VillagePlanSource::new`] validates every piece
/// against the block registry before it is handed out, so a registry that cannot
/// carry the village fails here, loudly, instead of dropping blocks during chunk
/// generation.
pub(crate) fn village_plan_source_for_startup(
    seed: i64,
    vanilla_data_dir: &Path,
    blocks: &Arc<mc_world::BlockRegistry>,
    data: &Arc<mc_data::VanillaData>,
    tags: &Arc<mc_data::tags::TagsData>,
    settlement_plan: Option<&mc_script::PluginSettlementPlan>,
    settlement_profile: mc_server::SettlementProfile,
) -> Result<Option<Arc<mc_worldgen::village::plan_source::VillagePlanSource>>> {
    if settlement_plan.is_some() {
        tracing::info!(
            owner = settlement_plan.map(mc_script::PluginSettlementPlan::owner_plugin_id),
            "a deployed component settlement plan owns settlement content; core places no villages",
        );
        return Ok(None);
    }
    if settlement_profile != mc_server::SettlementProfile::Vanilla {
        return Ok(None);
    }
    let structure_set = mc_data::Identifier::parse("minecraft:villages")
        .expect("the vanilla villages structure set id is static");
    let closure = Arc::new(
        mc_worldgen::village::load_village_closure(vanilla_data_dir, &structure_set, blocks)
            .context("loading the vanilla village closure from the content cache")?,
    );
    let cache_tags = Arc::new(mc_worldgen::vanilla_features::CacheTags::new(
        Arc::clone(blocks),
        Arc::clone(data),
        Arc::clone(tags),
    ));
    let semantics =
        mc_worldgen::vanilla_features::BlockSemantics::new(blocks.as_ref(), cache_tags.as_ref());
    let decor = Arc::new(
        mc_worldgen::village::VillageDecor::compile(vanilla_data_dir, &closure, &semantics)
            .context("compiling the vanilla village decor from the content cache")?,
    );
    let source = mc_worldgen::village::plan_source::VillagePlanSource::new(
        closure,
        decor,
        seed,
        Arc::clone(blocks),
        cache_tags.clone(),
        cache_tags,
    )
    .context("validating the vanilla village against the block registry")?;
    let source = Arc::new(source);
    let unspawned_mobs = source.closure().unspawned_piece_mobs();
    if !unspawned_mobs.is_empty() {
        // The lane spawns the villagers a piece template places and nothing
        // else: the other entities a village authors need their own entity
        // state, so they are reported rather than silently missing. See
        // `docs/VILLAGE_GENERATION.md`.
        tracing::warn!(
            code = "village_piece_mobs_other_than_villagers_are_not_spawned",
            mobs = ?unspawned_mobs,
            "the vanilla village lane spawns piece villagers only; these template entities are not spawned",
        );
    }
    tracing::info!(
        structure_set = %structure_set,
        structures = source.closure().structures.len(),
        pools = source.closure().pools.len(),
        pieces = source.closure().pieces.len(),
        processor_lists = source.closure().processor_lists.len(),
        placed_features = source.closure().placed_features.len(),
        "core vanilla village generation active",
    );
    Ok(Some(source))
}
