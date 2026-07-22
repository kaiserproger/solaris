use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{LuaHostConfig, LuaHostError, LuaWorldgenOreProfile, prepare_lua_plugins};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempPlugins(PathBuf);

impl TempPlugins {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solaris-worldgen-plugins-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempPlugins {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_plugin(root: &std::path::Path, id: &str, worldgen: &str) {
    let directory = root.join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("plugin.toml"),
        format!(
            r#"id = "{id}"
name = "{id}"
version = "0.1.0"
api = "0.6.0"
{worldgen}
"#
        ),
    )
    .unwrap();
    fs::write(directory.join("main.lua"), "").unwrap();
}

#[test]
fn shipped_geological_plugin_selects_the_startup_ore_profile() {
    let plugins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins");
    let prepared = prepare_lua_plugins(LuaHostConfig::new(plugins)).unwrap();

    assert_eq!(
        prepared.worldgen_ore_profile(),
        Some(LuaWorldgenOreProfile::GeologicalDeposits)
    );
}

#[test]
fn ordinary_plugin_set_keeps_vanilla_ore_generation() {
    let plugins = TempPlugins::new();
    write_plugin(plugins.path(), "ordinary", "");

    let prepared = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap();

    assert_eq!(prepared.worldgen_ore_profile(), None);
}

#[test]
fn two_worldgen_profiles_are_rejected_instead_of_using_load_order() {
    let plugins = TempPlugins::new();
    let declaration = "[worldgen]\nore_profile = \"geological_deposits\"";
    write_plugin(plugins.path(), "first", declaration);
    write_plugin(plugins.path(), "second", declaration);

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(matches!(
        error,
        LuaHostError::WorldgenConflict { first, second }
            if first == "first" && second == "second"
    ));
}

#[test]
fn unknown_worldgen_profile_fails_startup_instead_of_falling_back_to_vanilla() {
    let plugins = TempPlugins::new();
    write_plugin(
        plugins.path(),
        "unknown",
        "[worldgen]\nore_profile = \"unknown\"",
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(matches!(error, LuaHostError::InvalidWorldgenPlugin { .. }));
}

#[test]
fn missing_worldgen_plugin_source_fails_startup_instead_of_falling_back_to_vanilla() {
    let plugins = TempPlugins::new();
    write_plugin(
        plugins.path(),
        "missing-source",
        "[worldgen]\nore_profile = \"geological_deposits\"",
    );
    fs::remove_file(plugins.path().join("missing-source/main.lua")).unwrap();

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(matches!(error, LuaHostError::InvalidWorldgenPlugin { .. }));
}
