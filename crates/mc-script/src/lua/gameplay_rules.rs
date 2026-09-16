//! Reading `rules.lua`: the Luau half of the startup rule contract.
//!
//! The contract types themselves are runtime independent (`crate::gameplay_rules`);
//! what stays here is the one thing only the Luau host can do - evaluate a
//! sandboxed `rules.lua` against the directory's config and deserialize the
//! answer into them.

use super::*;
use mlua::LuaSerdeExt;

pub(super) fn read_gameplay_rules(
    directory: &Path,
    config: &toml::Table,
) -> Result<Option<Arc<GameplayRules>>, String> {
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
            let rules: GameplayRules = lua.from_value(value)?;
            rules
                .validate()
                .map_err(|error| mlua::Error::runtime(error.to_string()))?;
            Ok(Some(Arc::new(rules)))
        },
    )
    .map_err(lua_error)
}
