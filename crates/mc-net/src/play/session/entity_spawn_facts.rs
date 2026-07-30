use mc_entity::{AnimalBreedingState, AttributeKind, SheepColor, SpawnEntity};

use super::interaction_geometry;

pub(in crate::play::session) fn apply_entity_facts(entity: &mut SpawnEntity) {
    let Some(facts) = interaction_geometry::canonical_entity_facts(&entity.type_name) else {
        return;
    };
    if let Some(value) = facts.attributes.max_health {
        entity.attributes.set_base(AttributeKind::MaxHealth, value);
    }
    if let Some(value) = facts.attributes.movement_speed {
        entity
            .attributes
            .set_base(AttributeKind::MovementSpeed, value);
    }
    if let Some(value) = facts.attributes.follow_range {
        entity
            .attributes
            .set_base(AttributeKind::FollowRange, value);
    }
    if let Some(value) = facts.attributes.attack_damage {
        entity
            .attributes
            .set_base(AttributeKind::AttackDamage, value);
    }
    match entity.type_name.as_str() {
        "minecraft:sheep" => {
            entity.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));
        }
        "minecraft:cow" | "minecraft:chicken" => {
            entity.animal = Some(AnimalBreedingState::adult());
        }
        _ => {}
    }
}
