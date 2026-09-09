use crate::startup_validation::{
    WorldSource, ensure_world_contract_with_spawn, world_contract_path,
};
use std::path::Path;

fn open_world(path: &Path, rules: Option<&str>) -> anyhow::Result<WorldSource> {
    ensure_world_contract_with_spawn(
        path,
        mc_world::OVERWORLD_GEOMETRY,
        712_816,
        "tellus_like",
        "vanilla",
        "vanilla",
        mc_world::WorldSpawn::default(),
        rules,
    )
}

#[test]
fn startup_rules_cannot_change_or_disappear_from_an_existing_world() {
    let world = tempfile::tempdir().unwrap();
    assert_eq!(
        open_world(world.path(), Some("original")).unwrap(),
        WorldSource::SolarisGenerated
    );
    let original = std::fs::read(world_contract_path(world.path())).unwrap();
    assert_eq!(
        open_world(world.path(), Some("original")).unwrap(),
        WorldSource::SolarisGenerated
    );
    assert!(open_world(world.path(), Some("changed")).is_err());
    assert!(open_world(world.path(), None).is_err());
    assert_eq!(
        std::fs::read(world_contract_path(world.path())).unwrap(),
        original
    );
}

#[test]
fn startup_rules_cannot_be_added_to_an_existing_unconfigured_world() {
    let world = tempfile::tempdir().unwrap();
    open_world(world.path(), None).unwrap();
    let original = std::fs::read(world_contract_path(world.path())).unwrap();
    assert!(open_world(world.path(), Some("added")).is_err());
    assert_eq!(
        std::fs::read(world_contract_path(world.path())).unwrap(),
        original
    );
}
