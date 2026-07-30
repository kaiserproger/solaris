pub(super) fn entity_type_uses_aquatic_physics(type_name: &str) -> bool {
    mc_entity::natural_spawn_26_1_2::entity_type_uses_aquatic_physics(type_name)
}

pub(super) fn entity_type_walks_on_powder_snow(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:rabbit" | "minecraft:endermite" | "minecraft:silverfish" | "minecraft:fox"
    )
}
