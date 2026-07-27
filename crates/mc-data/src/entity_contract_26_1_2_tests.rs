use std::collections::HashSet;

use super::*;
use crate::Identifier;
use crate::entity_types::{
    DefaultAttributeTemplateIdentity, EntityArchetype, EntityInstanceCategory, EntityTypeRegistry,
    EntityTypeRegistryValidationError, EntityTypeReport, MINECRAFT_VERSION, MobCategory,
    PhysicalSimulationClass, ProjectileKind, SpawnDataCategory,
    ender_dragon_part_instance_contract_26_1_2, entity_type_contract_26_1_2_by_name,
    entity_type_contract_26_1_2_by_protocol_id, solaris_required_entity_types,
};

// This checksum only detects drift in the committed, reviewed contract data.
// It is not independent oracle evidence for vanilla parity.
const REVIEWED_CONTRACT_CHECKSUM: u64 = 0xe352_261a_b2a0_160b;

fn registry_report() -> Vec<EntityTypeReport> {
    entity_type_contracts_26_1_2()
        .map(|contract| EntityTypeReport {
            id: Identifier::parse(contract.name).unwrap(),
            protocol_id: contract.protocol_id,
        })
        .collect()
}

const fn category_fingerprint_code(category: MobCategory) -> u8 {
    match category {
        MobCategory::Monster => 0,
        MobCategory::Creature => 1,
        MobCategory::Ambient => 2,
        MobCategory::Axolotls => 3,
        MobCategory::UndergroundWaterCreature => 4,
        MobCategory::WaterCreature => 5,
        MobCategory::WaterAmbient => 6,
        MobCategory::Misc => 7,
    }
}

const fn archetype_fingerprint_code(archetype: EntityArchetype) -> u8 {
    match archetype {
        EntityArchetype::NonLiving => 0,
        EntityArchetype::Living => 1,
    }
}

const fn projectile_fingerprint_code(projectile: ProjectileKind) -> u8 {
    match projectile {
        ProjectileKind::None => 0,
        ProjectileKind::Arrow => 1,
        ProjectileKind::ThrowableItem => 2,
        ProjectileKind::Hurting => 3,
        ProjectileKind::WindCharge => 4,
        ProjectileKind::ShulkerBullet => 5,
        ProjectileKind::LlamaSpit => 6,
        ProjectileKind::FireworkRocket => 7,
        ProjectileKind::FishingBobber => 8,
    }
}

const fn physical_fingerprint_code(physical: PhysicalSimulationClass) -> u8 {
    match physical {
        PhysicalSimulationClass::Immobile => 0,
        PhysicalSimulationClass::LivingGround => 1,
        PhysicalSimulationClass::LivingFlying => 2,
        PhysicalSimulationClass::LivingAquatic => 3,
        PhysicalSimulationClass::LivingAmphibious => 4,
        PhysicalSimulationClass::LivingAttached => 5,
        PhysicalSimulationClass::Projectile => 6,
        PhysicalSimulationClass::Boat => 7,
        PhysicalSimulationClass::Minecart => 8,
        PhysicalSimulationClass::Item => 9,
        PhysicalSimulationClass::ExperienceOrb => 10,
        PhysicalSimulationClass::FallingBlock => 11,
        PhysicalSimulationClass::PrimedTnt => 12,
        PhysicalSimulationClass::EyeOfEnder => 13,
    }
}

const fn spawn_data_fingerprint_code(spawn_data: SpawnDataCategory) -> u8 {
    match spawn_data {
        SpawnDataCategory::Zero => 0,
        SpawnDataCategory::ProjectileOwner => 1,
        SpawnDataCategory::FallingBlockState => 2,
        SpawnDataCategory::HangingDirection => 3,
        SpawnDataCategory::WardenEmerging => 4,
        SpawnDataCategory::NeverSent => 5,
        SpawnDataCategory::ProjectileOwnerOrSelf => 6,
    }
}

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash
}

#[test]
fn dense_registry_ids_are_exhaustive_and_round_trip() {
    assert_eq!(MINECRAFT_VERSION, "26.1.2");
    assert_eq!(ENTITY_TYPE_COUNT, 157);

    for (expected_id, contract) in entity_type_contracts_26_1_2().enumerate() {
        assert_eq!(contract.protocol_id as usize, expected_id);
        assert_eq!(by_protocol_id(contract.protocol_id), Some(contract));
        assert_eq!(by_name(contract.name), Some(contract));
    }
}

#[test]
fn behavior_contract_is_total_for_all_157_rows() {
    let contracts: Vec<_> = entity_type_contracts_26_1_2().collect();
    assert_eq!(contracts.len(), ENTITY_TYPE_COUNT);

    for (expected_id, contract) in contracts.into_iter().enumerate() {
        let classified = ENTITY_BEHAVIORS[expected_id];
        assert_eq!(classified.protocol_id as usize, expected_id);
        assert_eq!(classified.name, contract.name);
        assert_eq!(classified.behavior, contract.behavior);

        let behavior = contract.behavior;
        assert!(
            !behavior.metadata_schema.as_str().is_empty(),
            "{}",
            contract.name
        );

        assert_eq!(
            behavior.archetype == EntityArchetype::Living,
            behavior.default_attributes != DefaultAttributeTemplateIdentity::None,
            "{}",
            contract.name
        );
        assert_eq!(
            behavior.projectile != ProjectileKind::None,
            behavior.physical_simulation == PhysicalSimulationClass::Projectile,
            "{}",
            contract.name
        );

        if behavior.vehicle_passenger.max_passengers == 0 {
            assert!(
                !contract.flags.is_serializable()
                    || matches!(
                        contract.name,
                        "minecraft:marker" | "minecraft:ominous_item_spawner"
                    ),
                "{}",
                contract.name
            );
        }
    }
}

#[test]
fn representative_behavior_categories_match_vanilla_classes() {
    let cow = by_name("minecraft:cow").unwrap().behavior;
    assert_eq!(cow.archetype, EntityArchetype::Living);
    assert_eq!(cow.projectile, ProjectileKind::None);
    assert_eq!(
        cow.physical_simulation,
        PhysicalSimulationClass::LivingGround
    );
    assert_eq!(
        cow.default_attributes.as_str(),
        Some("Cow.createAttributes")
    );
    assert_eq!(cow.metadata_schema.as_str(), "Cow");
    assert_eq!(cow.spawn_data, SpawnDataCategory::Zero);
    assert_eq!(cow.vehicle_passenger.max_passengers, 1);
    assert!(cow.vehicle_passenger.can_be_passenger);

    let arrow = by_name("minecraft:arrow").unwrap().behavior;
    assert_eq!(arrow.archetype, EntityArchetype::NonLiving);
    assert_eq!(arrow.projectile, ProjectileKind::Arrow);
    assert_eq!(
        arrow.physical_simulation,
        PhysicalSimulationClass::Projectile
    );
    assert_eq!(
        arrow.default_attributes,
        DefaultAttributeTemplateIdentity::None
    );
    assert_eq!(arrow.metadata_schema.as_str(), "Arrow");
    assert_eq!(arrow.spawn_data, SpawnDataCategory::ProjectileOwner);

    let boat = by_name("minecraft:acacia_boat").unwrap().behavior;
    assert_eq!(boat.physical_simulation, PhysicalSimulationClass::Boat);
    assert_eq!(boat.vehicle_passenger.max_passengers, 2);

    let chest_boat = by_name("minecraft:acacia_chest_boat").unwrap().behavior;
    assert_eq!(
        chest_boat.physical_simulation,
        PhysicalSimulationClass::Boat
    );
    assert_eq!(chest_boat.vehicle_passenger.max_passengers, 1);

    let minecart = by_name("minecraft:minecart").unwrap().behavior;
    assert_eq!(
        minecart.physical_simulation,
        PhysicalSimulationClass::Minecart
    );
    assert_eq!(minecart.vehicle_passenger.max_passengers, 1);

    // Vanilla 26.1.2 Camel::canAddPassenger accepts while the current count is <= 2.
    let camel = by_name("minecraft:camel").unwrap().behavior;
    assert_eq!(camel.vehicle_passenger.max_passengers, 3);

    let happy_ghast = by_name("minecraft:happy_ghast").unwrap().behavior;
    assert_eq!(
        happy_ghast.physical_simulation,
        PhysicalSimulationClass::LivingFlying
    );
    assert_eq!(happy_ghast.vehicle_passenger.max_passengers, 4);

    let cod = by_name("minecraft:cod").unwrap().behavior;
    assert_eq!(
        cod.physical_simulation,
        PhysicalSimulationClass::LivingAquatic
    );

    let axolotl = by_name("minecraft:axolotl").unwrap().behavior;
    assert_eq!(
        axolotl.physical_simulation,
        PhysicalSimulationClass::LivingAmphibious
    );

    let shulker = by_name("minecraft:shulker").unwrap().behavior;
    assert_eq!(
        shulker.physical_simulation,
        PhysicalSimulationClass::LivingAttached
    );

    let falling_block = by_name("minecraft:falling_block").unwrap().behavior;
    assert_eq!(
        falling_block.physical_simulation,
        PhysicalSimulationClass::FallingBlock
    );
    assert_eq!(
        falling_block.spawn_data,
        SpawnDataCategory::FallingBlockState
    );

    let item_frame = by_name("minecraft:item_frame").unwrap().behavior;
    assert_eq!(item_frame.spawn_data, SpawnDataCategory::HangingDirection);

    let warden = by_name("minecraft:warden").unwrap().behavior;
    assert_eq!(warden.spawn_data, SpawnDataCategory::WardenEmerging);
    assert!(!warden.vehicle_passenger.can_be_passenger);

    let marker = by_name("minecraft:marker").unwrap().behavior;
    assert_eq!(
        marker.physical_simulation,
        PhysicalSimulationClass::Immobile
    );
    assert_eq!(marker.vehicle_passenger.max_passengers, 0);
    assert_eq!(marker.spawn_data, SpawnDataCategory::NeverSent);

    let player = by_name("minecraft:player").unwrap().behavior;
    assert_eq!(player.archetype, EntityArchetype::Living);
    assert_eq!(
        player.default_attributes.as_str(),
        Some("Player.createAttributes")
    );
    assert_eq!(player.vehicle_passenger.max_passengers, 0);
    assert!(player.vehicle_passenger.can_be_passenger);

    let wither = by_name("minecraft:wither").unwrap().behavior;
    assert!(!wither.vehicle_passenger.can_be_passenger);

    let fishing_bobber = by_name("minecraft:fishing_bobber").unwrap().behavior;
    assert_eq!(fishing_bobber.projectile, ProjectileKind::FishingBobber);
    assert_eq!(
        fishing_bobber.spawn_data,
        SpawnDataCategory::ProjectileOwnerOrSelf
    );
    assert_eq!(fishing_bobber.vehicle_passenger.max_passengers, 0);
}

#[test]
fn every_behavior_category_has_an_exact_representative() {
    let archetypes = [
        ("minecraft:arrow", EntityArchetype::NonLiving),
        ("minecraft:cow", EntityArchetype::Living),
    ];
    for (name, expected) in archetypes {
        assert_eq!(
            by_name(name).unwrap().behavior.archetype,
            expected,
            "{name}"
        );
    }

    let projectiles = [
        ("minecraft:cow", ProjectileKind::None),
        ("minecraft:arrow", ProjectileKind::Arrow),
        ("minecraft:egg", ProjectileKind::ThrowableItem),
        ("minecraft:fireball", ProjectileKind::Hurting),
        ("minecraft:wind_charge", ProjectileKind::WindCharge),
        ("minecraft:shulker_bullet", ProjectileKind::ShulkerBullet),
        ("minecraft:llama_spit", ProjectileKind::LlamaSpit),
        ("minecraft:firework_rocket", ProjectileKind::FireworkRocket),
        ("minecraft:fishing_bobber", ProjectileKind::FishingBobber),
    ];
    for (name, expected) in projectiles {
        assert_eq!(
            by_name(name).unwrap().behavior.projectile,
            expected,
            "{name}"
        );
    }

    let physical_simulations = [
        ("minecraft:marker", PhysicalSimulationClass::Immobile),
        ("minecraft:cow", PhysicalSimulationClass::LivingGround),
        ("minecraft:ghast", PhysicalSimulationClass::LivingFlying),
        ("minecraft:cod", PhysicalSimulationClass::LivingAquatic),
        (
            "minecraft:axolotl",
            PhysicalSimulationClass::LivingAmphibious,
        ),
        ("minecraft:shulker", PhysicalSimulationClass::LivingAttached),
        ("minecraft:arrow", PhysicalSimulationClass::Projectile),
        ("minecraft:oak_boat", PhysicalSimulationClass::Boat),
        ("minecraft:minecart", PhysicalSimulationClass::Minecart),
        ("minecraft:item", PhysicalSimulationClass::Item),
        (
            "minecraft:experience_orb",
            PhysicalSimulationClass::ExperienceOrb,
        ),
        (
            "minecraft:falling_block",
            PhysicalSimulationClass::FallingBlock,
        ),
        ("minecraft:tnt", PhysicalSimulationClass::PrimedTnt),
        (
            "minecraft:eye_of_ender",
            PhysicalSimulationClass::EyeOfEnder,
        ),
    ];
    for (name, expected) in physical_simulations {
        assert_eq!(
            by_name(name).unwrap().behavior.physical_simulation,
            expected,
            "{name}"
        );
    }

    let spawn_data = [
        ("minecraft:cow", SpawnDataCategory::Zero),
        ("minecraft:arrow", SpawnDataCategory::ProjectileOwner),
        (
            "minecraft:fishing_bobber",
            SpawnDataCategory::ProjectileOwnerOrSelf,
        ),
        (
            "minecraft:falling_block",
            SpawnDataCategory::FallingBlockState,
        ),
        ("minecraft:item_frame", SpawnDataCategory::HangingDirection),
        ("minecraft:warden", SpawnDataCategory::WardenEmerging),
        ("minecraft:marker", SpawnDataCategory::NeverSent),
    ];
    for (name, expected) in spawn_data {
        assert_eq!(
            by_name(name).unwrap().behavior.spawn_data,
            expected,
            "{name}"
        );
    }

    for (name, expected_capacity) in [
        ("minecraft:marker", 0),
        ("minecraft:cow", 1),
        ("minecraft:oak_boat", 2),
        ("minecraft:camel", 3),
        ("minecraft:happy_ghast", 4),
    ] {
        assert_eq!(
            by_name(name)
                .unwrap()
                .behavior
                .vehicle_passenger
                .max_passengers,
            expected_capacity,
            "{name}"
        );
    }

    assert_eq!(
        by_name("minecraft:arrow")
            .unwrap()
            .behavior
            .default_attributes,
        DefaultAttributeTemplateIdentity::None
    );
    assert_eq!(
        by_name("minecraft:cow")
            .unwrap()
            .behavior
            .default_attributes
            .as_str(),
        Some("Cow.createAttributes")
    );
    assert!(
        by_name("minecraft:cow")
            .unwrap()
            .behavior
            .vehicle_passenger
            .can_be_passenger
    );
    assert!(
        !by_name("minecraft:warden")
            .unwrap()
            .behavior
            .vehicle_passenger
            .can_be_passenger
    );
}

#[test]
fn dragon_part_is_an_explicit_nonregistered_never_sent_instance() {
    assert_eq!(by_name("minecraft:ender_dragon_part"), None);

    let registered_dragon = by_name("minecraft:ender_dragon")
        .unwrap()
        .registered_instance_contract();
    assert_eq!(
        registered_dragon.category,
        EntityInstanceCategory::Registered
    );
    assert_eq!(registered_dragon.metadata_schema.as_str(), "EnderDragon");
    assert_eq!(registered_dragon.spawn_data, SpawnDataCategory::Zero);

    let dragon_part = ender_dragon_part_instance_contract_26_1_2();
    assert_eq!(
        dragon_part.category,
        EntityInstanceCategory::NonRegisteredDragonPart
    );
    assert_eq!(dragon_part.metadata_schema.as_str(), "EnderDragonPart");
    assert_eq!(dragon_part.spawn_data, SpawnDataCategory::NeverSent);
}

#[test]
fn public_strict_contract_lookup_has_no_unknown_or_alias_fallback() {
    assert_eq!(
        entity_type_contract_26_1_2_by_protocol_id(110)
            .unwrap()
            .name,
        "minecraft:salmon"
    );
    assert_eq!(
        entity_type_contract_26_1_2_by_name("minecraft:salmon")
            .unwrap()
            .protocol_id,
        110
    );

    assert_eq!(
        entity_type_contract_26_1_2_by_protocol_id(ENTITY_TYPE_COUNT as u32),
        None
    );
    assert_eq!(entity_type_contract_26_1_2_by_protocol_id(u32::MAX), None);
    assert_eq!(
        entity_type_contract_26_1_2_by_name("minecraft:unknown"),
        None
    );
    assert_eq!(
        entity_type_contract_26_1_2_by_name("minecraft:xp_orb"),
        None
    );
    assert_eq!(
        entity_type_contract_26_1_2_by_name("minecraft:tipped_arrow"),
        None
    );
    assert_eq!(entity_type_contract_26_1_2_by_name("salmon"), None);
}

#[test]
fn reviewed_contract_checksum_detects_committed_data_drift() {
    let mut checksum = 14_695_981_039_346_656_037_u64;

    for contract in entity_type_contracts_26_1_2() {
        checksum = hash_bytes(checksum, &contract.protocol_id.to_le_bytes());
        checksum = hash_bytes(checksum, contract.name.as_bytes());
        checksum = hash_bytes(checksum, &[0]);
        checksum = hash_bytes(checksum, &contract.dimensions.width.to_bits().to_le_bytes());
        checksum = hash_bytes(
            checksum,
            &contract.dimensions.height.to_bits().to_le_bytes(),
        );
        checksum = hash_bytes(
            checksum,
            &contract.dimensions.eye_height.to_bits().to_le_bytes(),
        );
        checksum = hash_bytes(
            checksum,
            &contract
                .dimensions
                .spawn_dimensions_scale
                .to_bits()
                .to_le_bytes(),
        );
        checksum = hash_bytes(checksum, &[contract.tracking_range]);
        checksum = hash_bytes(checksum, &contract.update_interval.to_le_bytes());
        checksum = hash_bytes(
            checksum,
            &[
                category_fingerprint_code(contract.category),
                contract.flags.bits(),
            ],
        );
        checksum = hash_bytes(
            checksum,
            &[
                archetype_fingerprint_code(contract.behavior.archetype),
                projectile_fingerprint_code(contract.behavior.projectile),
                contract.behavior.vehicle_passenger.max_passengers,
                u8::from(contract.behavior.vehicle_passenger.can_be_passenger),
                physical_fingerprint_code(contract.behavior.physical_simulation),
                spawn_data_fingerprint_code(contract.behavior.spawn_data),
            ],
        );
        match contract.behavior.default_attributes {
            DefaultAttributeTemplateIdentity::None => {
                checksum = hash_bytes(checksum, &[0]);
            }
            DefaultAttributeTemplateIdentity::Vanilla(identity) => {
                checksum = hash_bytes(checksum, &[1]);
                checksum = hash_bytes(checksum, identity.as_bytes());
            }
        }
        checksum = hash_bytes(checksum, &[0]);
        checksum = hash_bytes(
            checksum,
            contract.behavior.metadata_schema.as_str().as_bytes(),
        );
        checksum = hash_bytes(checksum, &[0]);
    }

    assert_eq!(checksum, REVIEWED_CONTRACT_CHECKSUM);
}

#[test]
fn boundary_and_non_default_facts_are_exact() {
    let boat = by_protocol_id(0).unwrap();
    assert_eq!(boat.name, "minecraft:acacia_boat");
    assert_eq!(boat.dimensions.width, 1.375);
    assert_eq!(boat.dimensions.height, 0.5625);
    assert_eq!(boat.dimensions.eye_height, 0.5625);
    assert_eq!(boat.tracking_range, 10);
    assert_eq!(boat.update_interval, 3);
    assert_eq!(boat.category, MobCategory::Misc);
    assert!(boat.flags.can_spawn_far_from_player());

    let cloud = by_protocol_id(3).unwrap();
    assert_eq!(cloud.update_interval, i32::MAX as u32);
    assert!(cloud.flags.is_fire_immune());

    let dolphin = by_protocol_id(35).unwrap();
    assert_eq!(dolphin.tracking_range, 5);
    assert_eq!(dolphin.category, MobCategory::WaterCreature);

    let magma_cube = by_protocol_id(80).unwrap();
    assert_eq!(magma_cube.dimensions.spawn_dimensions_scale, 4.0);

    let marker = by_protocol_id(84).unwrap();
    assert_eq!(marker.dimensions.width, 0.0);
    assert_eq!(marker.dimensions.height, 0.0);
    assert_eq!(marker.tracking_range, 0);

    let pillager = by_protocol_id(103).unwrap();
    assert!(pillager.flags.can_spawn_far_from_player());
    assert!(!pillager.flags.is_allowed_in_peaceful());

    let player = by_protocol_id(155).unwrap();
    assert_eq!(player.name, "minecraft:player");
    assert_eq!(player.tracking_range, 32);
    assert_eq!(player.update_interval, 2);
    assert!(!player.flags.is_serializable());
    assert!(!player.flags.is_summonable());

    let fishing_bobber = by_protocol_id(156).unwrap();
    assert_eq!(fishing_bobber.name, "minecraft:fishing_bobber");
    assert_eq!(fishing_bobber.update_interval, 5);
    assert!(!fishing_bobber.flags.is_serializable());
    assert!(!fishing_bobber.flags.is_summonable());
}

#[test]
fn category_spawn_policy_matches_26_1_2() {
    let cases = [
        (MobCategory::Monster, 70, false, false, 128),
        (MobCategory::Creature, 10, true, true, 128),
        (MobCategory::Ambient, 15, true, false, 128),
        (MobCategory::Axolotls, 5, true, false, 128),
        (MobCategory::UndergroundWaterCreature, 5, true, false, 128),
        (MobCategory::WaterCreature, 5, true, false, 128),
        (MobCategory::WaterAmbient, 20, true, false, 64),
        (MobCategory::Misc, -1, true, true, 128),
    ];

    for (category, max_per_chunk, friendly, persistent, despawn_distance) in cases {
        assert_eq!(category.max_instances_per_chunk(), max_per_chunk);
        assert_eq!(category.is_friendly(), friendly);
        assert_eq!(category.is_persistent(), persistent);
        assert_eq!(category.no_despawn_distance(), 32);
        assert_eq!(category.despawn_distance(), despawn_distance);
    }
}

#[test]
fn contract_has_no_duplicate_names_or_generic_facts() {
    let zero_sized = [
        "minecraft:block_display",
        "minecraft:interaction",
        "minecraft:item_display",
        "minecraft:lightning_bolt",
        "minecraft:marker",
        "minecraft:text_display",
    ];
    let mut names = HashSet::with_capacity(ENTITY_TYPE_COUNT);

    for contract in entity_type_contracts_26_1_2() {
        assert!(names.insert(contract.name), "duplicate {}", contract.name);
        assert!(contract.name.starts_with("minecraft:"));
        assert_ne!(contract.name, "minecraft:unknown");
        assert!(contract.dimensions.width.is_finite());
        assert!(contract.dimensions.height.is_finite());
        assert!(contract.dimensions.eye_height.is_finite());
        assert!(contract.dimensions.spawn_dimensions_scale.is_finite());
        assert!(contract.dimensions.width >= 0.0);
        assert!(contract.dimensions.height >= 0.0);
        assert!(contract.dimensions.eye_height >= 0.0);
        assert!(contract.dimensions.spawn_dimensions_scale > 0.0);
        assert!(contract.update_interval > 0);

        if zero_sized.contains(&contract.name) {
            assert_eq!(contract.dimensions.width, 0.0, "{}", contract.name);
            assert_eq!(contract.dimensions.height, 0.0, "{}", contract.name);
        } else {
            assert!(contract.dimensions.width > 0.0, "{}", contract.name);
            assert!(contract.dimensions.height > 0.0, "{}", contract.name);
        }
    }

    assert_eq!(names.len(), ENTITY_TYPE_COUNT);
}

#[test]
fn strict_registry_accepts_complete_report_in_any_order() {
    let report = registry_report();
    let registry = EntityTypeRegistry::try_from_report_26_1_2(&report).unwrap();
    assert_eq!(registry.len(), ENTITY_TYPE_COUNT);

    let reversed: Vec<_> = report.into_iter().rev().collect();
    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&reversed)
            .unwrap()
            .len(),
        ENTITY_TYPE_COUNT
    );
}

#[test]
fn embedded_production_registry_is_strict_and_canonical() {
    let registry = solaris_required_entity_types();
    assert_eq!(registry.len(), ENTITY_TYPE_COUNT);

    let salmon = registry
        .facts_of(&Identifier::parse("minecraft:salmon").unwrap())
        .unwrap();
    assert_eq!(salmon.protocol_id, 110);
    assert_eq!(salmon.mob_category, Some(MobCategory::WaterAmbient));
    assert_eq!(salmon.dimensions.width, f64::from(0.7_f32));
}

#[test]
fn strict_registry_rejects_empty_report() {
    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&[]).unwrap_err(),
        EntityTypeRegistryValidationError::MissingProtocolId {
            protocol_id: 0,
            expected_name: Identifier::parse("minecraft:acacia_boat").unwrap(),
        }
    );
}

#[test]
fn strict_registry_rejects_partial_report() {
    let report = registry_report();

    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&report[..2]).unwrap_err(),
        EntityTypeRegistryValidationError::MissingProtocolId {
            protocol_id: 2,
            expected_name: Identifier::parse("minecraft:allay").unwrap(),
        }
    );
}

#[test]
fn strict_registry_rejects_a_missing_id() {
    let mut report = registry_report();
    report.remove(71);

    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&report).unwrap_err(),
        EntityTypeRegistryValidationError::MissingProtocolId {
            protocol_id: 71,
            expected_name: Identifier::parse("minecraft:item").unwrap(),
        }
    );
}

#[test]
fn strict_registry_rejects_a_duplicate_id() {
    let mut report = registry_report();
    report.push(report[0].clone());

    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&report).unwrap_err(),
        EntityTypeRegistryValidationError::DuplicateProtocolId { protocol_id: 0 }
    );
}

#[test]
fn strict_registry_rejects_a_duplicate_name() {
    let mut report = registry_report();
    report[1].id = report[0].id.clone();

    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&report).unwrap_err(),
        EntityTypeRegistryValidationError::DuplicateName {
            name: Identifier::parse("minecraft:acacia_boat").unwrap(),
        }
    );
}

#[test]
fn strict_registry_rejects_an_id_name_mismatch_with_owned_error() {
    let error = {
        let mut report = registry_report();
        report[0].id = Identifier::parse("minecraft:not_acacia_boat").unwrap();
        EntityTypeRegistry::try_from_report_26_1_2(&report).unwrap_err()
    };

    assert_eq!(
        error,
        EntityTypeRegistryValidationError::NameMismatch {
            protocol_id: 0,
            expected_name: Identifier::parse("minecraft:acacia_boat").unwrap(),
            actual_name: Identifier::parse("minecraft:not_acacia_boat").unwrap(),
        }
    );
}

#[test]
fn strict_registry_rejects_a_canonical_name_at_the_wrong_id() {
    let mut report = registry_report();
    report[27].id = Identifier::parse("minecraft:salmon").unwrap();

    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&report).unwrap_err(),
        EntityTypeRegistryValidationError::NameMismatch {
            protocol_id: 27,
            expected_name: Identifier::parse("minecraft:cod").unwrap(),
            actual_name: Identifier::parse("minecraft:salmon").unwrap(),
        }
    );
}

#[test]
fn strict_registry_rejects_an_out_of_range_id() {
    let mut report = registry_report();
    report.push(EntityTypeReport {
        id: Identifier::parse("minecraft:future_entity").unwrap(),
        protocol_id: ENTITY_TYPE_COUNT as u32,
    });

    assert_eq!(
        EntityTypeRegistry::try_from_report_26_1_2(&report).unwrap_err(),
        EntityTypeRegistryValidationError::ProtocolIdOutOfRange {
            protocol_id: ENTITY_TYPE_COUNT as u32,
            name: Identifier::parse("minecraft:future_entity").unwrap(),
        }
    );
}

#[test]
fn strict_registry_exposes_canonical_facts() {
    let registry = solaris_required_entity_types();

    let salmon = registry
        .facts_of(&Identifier::parse("minecraft:salmon").unwrap())
        .unwrap();
    assert_eq!(salmon.dimensions.width, f64::from(0.7_f32));
    assert_eq!(salmon.dimensions.height, f64::from(0.4_f32));
    assert_eq!(salmon.dimensions.eye_height, Some(f64::from(0.26_f32)));
    assert_eq!(salmon.dimensions.spawn_dimensions_scale, 1.0);
    assert_eq!(salmon.mob_category, Some(MobCategory::WaterAmbient));
    assert_eq!(salmon.tracking_range, Some(4));
    assert_eq!(salmon.update_interval, Some(3));
    assert!(salmon.flags.unwrap().is_serializable());

    let marker = registry
        .facts_of(&Identifier::parse("minecraft:marker").unwrap())
        .unwrap();
    assert_eq!(marker.dimensions.width, 0.0);
    assert_eq!(marker.dimensions.height, 0.0);
    assert_eq!(marker.tracking_range, Some(0));
}

#[test]
fn lookups_reject_unknown_ids_names_and_aliases() {
    assert_eq!(by_protocol_id(ENTITY_TYPE_COUNT as u32), None);
    assert_eq!(by_protocol_id(u32::MAX), None);
    assert_eq!(by_name("minecraft:future_entity"), None);
    assert_eq!(by_name("pig"), None);
    assert_eq!(by_name("minecraft:xp_orb"), None);
}
