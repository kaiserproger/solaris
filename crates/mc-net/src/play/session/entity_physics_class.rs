use mc_data::entity_types::{PhysicalSimulationClass, entity_type_contract_26_1_2_by_name};

pub(super) fn entity_type_uses_aquatic_physics(type_name: &str) -> bool {
    entity_type_contract_26_1_2_by_name(type_name).is_some_and(|contract| {
        matches!(
            contract.behavior.physical_simulation,
            PhysicalSimulationClass::LivingAquatic | PhysicalSimulationClass::LivingAmphibious
        )
    })
}

pub(super) fn entity_type_walks_on_powder_snow(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:rabbit" | "minecraft:endermite" | "minecraft:silverfish" | "minecraft:fox"
    )
}
