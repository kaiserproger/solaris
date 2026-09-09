use super::*;
use mlua::LuaSerdeExt;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LuaGameplayRules {
    #[serde(default)]
    pub spawning: Vec<LuaBiomeSpawns>,
    #[serde(default)]
    pub trees: Vec<LuaTreeRule>,
    pub clay: Option<LuaClayRule>,
    pub placement: Option<LuaSpawnPlacement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LuaBiomeSpawns {
    pub biome: String,
    pub groups: BTreeMap<String, Vec<LuaSpawnEntry>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LuaSpawnEntry {
    pub entity: String,
    pub min: u32,
    pub max: u32,
    pub weight: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LuaTreeRule {
    pub biomes: Vec<String>,
    pub spacing: u64,
    pub density_threshold: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LuaClayRule {
    pub rarity: u64,
    pub radius_min: u8,
    pub radius_max: u8,
    pub max_water_depth: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LuaSpawnPlacement {
    pub land_spacing: u8,
    pub water_attempts: u8,
    pub water_depth: u8,
}

impl LuaGameplayRules {
    pub fn contract_name(&self) -> String {
        let canonical = toml::to_string(self).expect("validated rule plan is serializable");
        format!("luau-rules:{:x}", Sha256::digest(canonical.as_bytes()))
    }

    fn validate(&self) -> mlua::Result<()> {
        let invalid = |message| mlua::Error::runtime(message);
        if self.spawning.is_empty()
            && self.trees.is_empty()
            && self.clay.is_none()
            && self.placement.is_none()
        {
            return Err(invalid(
                "rules.lua must define spawning, trees, clay, or placement",
            ));
        }
        if self.spawning.len() > 64 || self.trees.len() > 64 {
            return Err(invalid("rule plan exceeds 64 biome declarations"));
        }
        let mut spawn_biomes = BTreeSet::new();
        for rule in &self.spawning {
            if rule.biome.len() > 128 || !spawn_biomes.insert(&rule.biome) {
                return Err(invalid("invalid or duplicate spawning biome"));
            }
            for (group, entries) in &rule.groups {
                if !matches!(
                    group.as_str(),
                    "creature" | "monster" | "water_ambient" | "water_creature"
                ) || entries.len() > 32
                {
                    return Err(invalid("unsupported spawn group or more than 32 entries"));
                }
                let mut entities = BTreeSet::new();
                for entry in entries {
                    if entry.entity.len() > 128
                        || !entities.insert(&entry.entity)
                        || entry.min == 0
                        || entry.min > entry.max
                        || entry.max > 6
                        || entry.weight == 0
                        || entry.weight > 10_000
                    {
                        return Err(invalid("invalid, duplicate, or over-budget spawn entry"));
                    }
                }
            }
        }
        let mut tree_biomes = BTreeSet::new();
        for rule in &self.trees {
            if rule.biomes.is_empty()
                || rule.biomes.len() > 64
                || rule.spacing == 0
                || !rule.density_threshold.is_finite()
                || !(-1.0..=1.0).contains(&rule.density_threshold)
            {
                return Err(invalid("invalid tree rule"));
            }
            for biome in &rule.biomes {
                if biome.len() > 128 || !tree_biomes.insert(biome) {
                    return Err(invalid("invalid or duplicate tree biome"));
                }
            }
        }
        if let Some(rule) = &self.clay
            && (rule.rarity == 0
                || rule.radius_min == 0
                || rule.radius_min > rule.radius_max
                || rule.radius_max > 3
                || !(1..=32).contains(&rule.max_water_depth))
        {
            return Err(invalid("clay rule exceeds bounded deposit dimensions"));
        }
        if let Some(rule) = &self.placement
            && (!(1..=4).contains(&rule.land_spacing)
                || !(1..=32).contains(&rule.water_attempts)
                || !(1..=16).contains(&rule.water_depth))
        {
            return Err(invalid("spawn placement exceeds bounded search dimensions"));
        }
        Ok(())
    }
}

pub(super) fn read_gameplay_rules(
    directory: &Path,
    config: &toml::Table,
) -> Result<Option<Arc<LuaGameplayRules>>, String> {
    let path = directory.join("rules.lua");
    match fs::metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading {}: {error}", path.display())),
        Ok(_) => {}
    }
    let source = read_utf8_file_limited(&path, MAX_PLUGIN_SOURCE_BYTES)?;
    luaur::check(&format!("--!strict\nlocal config = {{}} :: any\n{source}"))
        .map_err(|error| format!("rules.lua type check failed: {error:?}"))?;
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(lua_error)?;
    lua.set_memory_limit(MEMORY_BYTES_PER_PLUGIN)
        .map_err(lua_error)?;
    lua.globals()
        .set(
            "config",
            config_table_to_lua(&lua, config).map_err(lua_error)?,
        )
        .map_err(lua_error)?;
    lua.sandbox(true).map_err(lua_error)?;
    run_with_runtime_budget(
        &lua,
        NonZeroU64::new(INSTRUCTIONS_PER_EVENT).unwrap(),
        Some(HOST_EVENT_WALL_BUDGET),
        || {
            let value: Value = lua.load(&source).set_name(path.to_string_lossy()).eval()?;
            let rules: LuaGameplayRules = lua.from_value(value)?;
            rules.validate()?;
            Ok(Some(Arc::new(rules)))
        },
    )
    .map_err(lua_error)
}
