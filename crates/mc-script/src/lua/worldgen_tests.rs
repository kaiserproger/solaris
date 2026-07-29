use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    LuaHostConfig, LuaHostError, LuaSettlementBuildingRole, LuaSettlementBuildingTemplate,
    LuaSettlementInhabitantKind, LuaSettlementJob, LuaWorldgenOreProfile,
    LuaWorldgenSettlementProfile, MAX_SETTLEMENT_INHABITANTS, prepare_lua_plugins,
};

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
fn duplicate_plugin_ids_fail_before_runtime_metadata_can_diverge() {
    let plugins = TempPlugins::new();
    write_plugin(plugins.path(), "first", "");
    write_plugin(plugins.path(), "second", "");
    fs::write(
        plugins.path().join("second/plugin.toml"),
        r#"id = "first"
name = "Duplicate First"
version = "0.1.0"
api = "0.6.0"
"#,
    )
    .unwrap();

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();
    assert!(matches!(
        error,
        LuaHostError::PluginIdConflict { id } if id == "first"
    ));
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
fn shipped_settlement_plugin_selects_the_village_prototype() {
    let plugins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins");
    let prepared = prepare_lua_plugins(LuaHostConfig::new(plugins)).unwrap();

    assert_eq!(
        prepared.worldgen_settlement_profile(),
        Some(LuaWorldgenSettlementProfile::PlainsVillagePrototype)
    );
    let plan = prepared.worldgen_settlement_plan().unwrap();
    assert_eq!(plan.owner_plugin_id(), "settlement-prototype");
    assert_eq!(
        plan.buildings()
            .iter()
            .map(|building| (building.id(), building.template(), building.role()))
            .collect::<Vec<_>>(),
        vec![
            (
                "square",
                LuaSettlementBuildingTemplate::PlainsFountain,
                LuaSettlementBuildingRole::MeetingPoint,
            ),
            (
                "home",
                LuaSettlementBuildingTemplate::PlainsSmallHouse,
                LuaSettlementBuildingRole::Home,
            ),
            (
                "smithy",
                LuaSettlementBuildingTemplate::PlainsToolsmith,
                LuaSettlementBuildingRole::Workplace,
            ),
        ]
    );
    assert_eq!(
        plan.inhabitants()
            .iter()
            .map(|inhabitant| (
                inhabitant.id(),
                inhabitant.kind(),
                inhabitant.building_id(),
                inhabitant.job(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                "resident",
                LuaSettlementInhabitantKind::Villager,
                "home",
                LuaSettlementJob::Unemployed,
            ),
            (
                "smith",
                LuaSettlementInhabitantKind::Villager,
                "smithy",
                LuaSettlementJob::Toolsmith,
            ),
        ]
    );
    assert_eq!(
        plan.extensions()[0].id(),
        "settlement-prototype:smithy-work-orders"
    );
    assert_eq!(plan.extensions()[0].building_id(), "smithy");
    assert_eq!(
        plan.contract_name(),
        "plains_village_prototype|owner=settlement-prototype|buildings=\
square,plains_fountain,meeting_point;home,plains_small_house,home;\
smithy,plains_toolsmith,workplace;|inhabitants=\
resident,minecraft:villager,home,unemployed;\
smith,minecraft:villager,smithy,toolsmith;|extensions=\
settlement-prototype:smithy-work-orders,smithy;"
    );
}

#[test]
fn ordinary_plugin_set_keeps_vanilla_ore_generation() {
    let plugins = TempPlugins::new();
    write_plugin(plugins.path(), "ordinary", "");

    let prepared = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap();

    assert_eq!(prepared.worldgen_ore_profile(), None);
    assert_eq!(prepared.worldgen_settlement_profile(), None);
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
        LuaHostError::WorldgenConflict { kind: "ore", first, second }
            if first == "first" && second == "second"
    ));
}

#[test]
fn settlement_profile_is_exposed_independently_from_ore_generation() {
    let plugins = TempPlugins::new();
    write_plugin(
        plugins.path(),
        "settlements",
        "[worldgen]\nsettlement_profile = \"plains_village_prototype\"",
    );

    let prepared = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap();

    assert_eq!(prepared.worldgen_ore_profile(), None);
    assert_eq!(
        prepared.worldgen_settlement_profile(),
        Some(LuaWorldgenSettlementProfile::PlainsVillagePrototype)
    );
}

#[test]
fn ore_and_settlement_profiles_can_have_different_plugin_owners() {
    let plugins = TempPlugins::new();
    write_plugin(
        plugins.path(),
        "ores",
        "[worldgen]\nore_profile = \"geological_deposits\"",
    );
    write_plugin(
        plugins.path(),
        "settlements",
        "[worldgen]\nsettlement_profile = \"plains_village_prototype\"",
    );

    let prepared = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap();

    assert_eq!(
        prepared.worldgen_ore_profile(),
        Some(LuaWorldgenOreProfile::GeologicalDeposits)
    );
    assert_eq!(
        prepared.worldgen_settlement_profile(),
        Some(LuaWorldgenSettlementProfile::PlainsVillagePrototype)
    );
}

#[test]
fn two_settlement_profiles_are_rejected_instead_of_using_load_order() {
    let plugins = TempPlugins::new();
    let declaration = "[worldgen]\nsettlement_profile = \"plains_village_prototype\"";
    write_plugin(plugins.path(), "first", declaration);
    write_plugin(plugins.path(), "second", declaration);

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(matches!(
        error,
        LuaHostError::WorldgenConflict { kind: "settlement", first, second }
            if first == "first" && second == "second"
    ));
}

#[test]
fn empty_worldgen_declaration_fails_startup() {
    let plugins = TempPlugins::new();
    write_plugin(plugins.path(), "empty", "[worldgen]");

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(matches!(error, LuaHostError::InvalidStartupPlugin { .. }));
}

#[test]
fn settlement_descriptors_require_a_profile() {
    let plugins = TempPlugins::new();
    write_plugin(
        plugins.path(),
        "detached",
        r#"[[worldgen.settlement_buildings]]
id = "home"
template = "plains_small_house"
role = "home""#,
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(
        matches!(error, LuaHostError::InvalidStartupPlugin { message, .. }
            if message.contains("settlement descriptors require settlement_profile"))
    );
}

#[test]
fn settlement_descriptors_reject_unknown_building_references() {
    let plugins = TempPlugins::new();
    write_plugin(
        plugins.path(),
        "dangling",
        r#"[worldgen]
settlement_profile = "plains_village_prototype"

[[worldgen.settlement_inhabitants]]
id = "smith"
kind = "villager"
building = "missing"
job = "toolsmith""#,
    );

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(
        matches!(error, LuaHostError::InvalidStartupPlugin { message, .. }
            if message.contains("references unknown building"))
    );
}

#[test]
fn settlement_inhabitant_records_are_bounded() {
    let plugins = TempPlugins::new();
    let mut declaration =
        "[worldgen]\nsettlement_profile = \"plains_village_prototype\"\n".to_owned();
    for index in 0..=MAX_SETTLEMENT_INHABITANTS {
        declaration.push_str(&format!(
            r#"
[[worldgen.settlement_inhabitants]]
id = "resident-{index}"
kind = "villager"
building = "home"
job = "unemployed"
"#
        ));
    }
    write_plugin(plugins.path(), "crowded", &declaration);

    let error = prepare_lua_plugins(LuaHostConfig::new(plugins.path())).unwrap_err();

    assert!(
        matches!(error, LuaHostError::InvalidStartupPlugin { message, .. }
            if message.contains("settlement_inhabitants exceeds"))
    );
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

    assert!(matches!(error, LuaHostError::InvalidStartupPlugin { .. }));
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

    assert!(matches!(error, LuaHostError::InvalidStartupPlugin { .. }));
}
