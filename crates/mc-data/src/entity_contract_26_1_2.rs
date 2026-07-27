//! Dense vanilla entity-type facts for Java Edition 26.1.2.
//!
//! Registry names and protocol IDs come from the 26.1.2 data-generator
//! `registries.json` report. The remaining scalar facts were independently
//! transcribed from the registration builder values and defaults in the local
//! clean-room oracle. No fallback row exists: unknown IDs and names stay
//! unknown. Validation and public access live in [`crate::entity_types`].
//! Behavior facts come from the constructor targets in `EntityType`, the
//! `DefaultAttributes` supplier map, entity inheritance and riding overrides,
//! and spawn packet construction/recreation methods in the same local oracle.

pub const MINECRAFT_VERSION: &str = "26.1.2";
pub const ENTITY_TYPE_COUNT: usize = 157;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EntityDimensions {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) eye_height: f32,
    pub(crate) spawn_dimensions_scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MobCategory {
    Monster,
    Creature,
    Ambient,
    Axolotls,
    UndergroundWaterCreature,
    WaterCreature,
    WaterAmbient,
    Misc,
}

impl MobCategory {
    #[must_use]
    pub const fn max_instances_per_chunk(self) -> i16 {
        match self {
            Self::Monster => 70,
            Self::Creature => 10,
            Self::Ambient => 15,
            Self::Axolotls | Self::UndergroundWaterCreature | Self::WaterCreature => 5,
            Self::WaterAmbient => 20,
            Self::Misc => -1,
        }
    }

    #[must_use]
    pub const fn is_friendly(self) -> bool {
        !matches!(self, Self::Monster)
    }

    #[must_use]
    pub const fn is_persistent(self) -> bool {
        matches!(self, Self::Creature | Self::Misc)
    }

    #[must_use]
    pub const fn no_despawn_distance(self) -> u16 {
        32
    }

    #[must_use]
    pub const fn despawn_distance(self) -> u16 {
        match self {
            Self::WaterAmbient => 64,
            _ => 128,
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityTypeFlags(u8);

impl EntityTypeFlags {
    pub const SERIALIZABLE: Self = Self(1 << 0);
    pub const SUMMONABLE: Self = Self(1 << 1);
    pub const FIRE_IMMUNE: Self = Self(1 << 2);
    pub const CAN_SPAWN_FAR_FROM_PLAYER: Self = Self(1 << 3);
    pub const ALLOWED_IN_PEACEFUL: Self = Self(1 << 4);

    #[cfg(test)]
    pub(crate) const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    #[must_use]
    pub const fn is_serializable(self) -> bool {
        self.contains(Self::SERIALIZABLE)
    }

    #[must_use]
    pub const fn is_summonable(self) -> bool {
        self.contains(Self::SUMMONABLE)
    }

    #[must_use]
    pub const fn is_fire_immune(self) -> bool {
        self.contains(Self::FIRE_IMMUNE)
    }

    #[must_use]
    pub const fn can_spawn_far_from_player(self) -> bool {
        self.contains(Self::CAN_SPAWN_FAR_FROM_PLAYER)
    }

    #[must_use]
    pub const fn is_allowed_in_peaceful(self) -> bool {
        self.contains(Self::ALLOWED_IN_PEACEFUL)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityArchetype {
    NonLiving,
    Living,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectileKind {
    None,
    Arrow,
    ThrowableItem,
    Hurting,
    WindCharge,
    ShulkerBullet,
    LlamaSpit,
    FireworkRocket,
    FishingBobber,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VehiclePassengerCapabilities {
    /// Maximum accepted riders when dynamic conditions such as fluid state permit it.
    /// Zero also covers entity types rejected as vehicles by the serialization gate.
    pub max_passengers: u8,
    /// Whether the entity's type-level riding rule can permit it to be a passenger.
    pub can_be_passenger: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicalSimulationClass {
    Immobile,
    LivingGround,
    LivingFlying,
    LivingAquatic,
    LivingAmphibious,
    LivingAttached,
    Projectile,
    Boat,
    Minecart,
    Item,
    ExperienceOrb,
    FallingBlock,
    PrimedTnt,
    EyeOfEnder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultAttributeTemplateIdentity {
    None,
    Vanilla(&'static str),
}

impl DefaultAttributeTemplateIdentity {
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Vanilla(identity) => Some(identity),
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataSchemaIdentity(&'static str);

impl MetadataSchemaIdentity {
    const fn new(identity: &'static str) -> Self {
        Self(identity)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpawnDataCategory {
    /// The add-entity packet carries a zero data value.
    Zero,
    /// Projectile owner entity ID, or zero when there is no owner.
    ProjectileOwner,
    /// Fishing-hook owner entity ID, or the hook's own entity ID when ownerless.
    ProjectileOwnerOrSelf,
    FallingBlockState,
    HangingDirection,
    WardenEmerging,
    /// Vanilla rejects add-entity packet construction for this instance.
    NeverSent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityInstanceCategory {
    Registered,
    /// An `EnderDragonPart` uses the ender-dragon `EntityType` internally but
    /// is not a registry instance and rejects add-entity packet construction.
    NonRegisteredDragonPart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityInstanceContract {
    pub category: EntityInstanceCategory,
    pub metadata_schema: MetadataSchemaIdentity,
    pub spawn_data: SpawnDataCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityBehaviorContract {
    pub archetype: EntityArchetype,
    pub projectile: ProjectileKind,
    pub vehicle_passenger: VehiclePassengerCapabilities,
    pub physical_simulation: PhysicalSimulationClass,
    pub default_attributes: DefaultAttributeTemplateIdentity,
    pub metadata_schema: MetadataSchemaIdentity,
    pub spawn_data: SpawnDataCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClassifiedEntityBehavior {
    protocol_id: u32,
    name: &'static str,
    behavior: EntityBehaviorContract,
}

macro_rules! default_attribute_template {
    (None) => {
        DefaultAttributeTemplateIdentity::None
    };
    ($identity:literal) => {
        DefaultAttributeTemplateIdentity::Vanilla($identity)
    };
}

// Row fields: protocol ID, canonical name, archetype, projectile kind,
// physical simulation, maximum passengers, can be a passenger, default
// attribute supplier identity, concrete metadata schema, spawn packet data.
macro_rules! define_entity_behaviors {
    ($(
        $protocol_id:literal, $name:literal, $archetype:ident, $projectile:ident,
        $physical:ident, $max_passengers:literal, $can_be_passenger:literal,
        $attributes:tt, $metadata:literal, $spawn_data:ident;
    )+) => {
        static ENTITY_BEHAVIORS: [ClassifiedEntityBehavior; ENTITY_TYPE_COUNT] = [$(
            ClassifiedEntityBehavior {
                protocol_id: $protocol_id,
                name: $name,
                behavior: EntityBehaviorContract {
                    archetype: EntityArchetype::$archetype,
                    projectile: ProjectileKind::$projectile,
                    vehicle_passenger: VehiclePassengerCapabilities {
                        max_passengers: $max_passengers,
                        can_be_passenger: $can_be_passenger,
                    },
                    physical_simulation: PhysicalSimulationClass::$physical,
                    default_attributes: default_attribute_template!($attributes),
                    metadata_schema: MetadataSchemaIdentity::new($metadata),
                    spawn_data: SpawnDataCategory::$spawn_data,
                },
            }
        ),+];
    };
}

define_entity_behaviors! {
    0, "minecraft:acacia_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    1, "minecraft:acacia_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    2, "minecraft:allay", Living, None, LivingFlying, 1, true, "Allay.createAttributes", "Allay", Zero;
    3, "minecraft:area_effect_cloud", NonLiving, None, Immobile, 1, true, None, "AreaEffectCloud", Zero;
    4, "minecraft:armadillo", Living, None, LivingGround, 1, true, "Armadillo.createAttributes", "Armadillo", Zero;
    5, "minecraft:armor_stand", Living, None, LivingGround, 1, true, "ArmorStand.createAttributes", "ArmorStand", Zero;
    6, "minecraft:arrow", NonLiving, Arrow, Projectile, 1, true, None, "Arrow", ProjectileOwner;
    7, "minecraft:axolotl", Living, None, LivingAmphibious, 1, true, "Axolotl.createAttributes", "Axolotl", Zero;
    8, "minecraft:bamboo_chest_raft", NonLiving, None, Boat, 1, true, None, "ChestRaft", Zero;
    9, "minecraft:bamboo_raft", NonLiving, None, Boat, 2, true, None, "Raft", Zero;
    10, "minecraft:bat", Living, None, LivingFlying, 1, true, "Bat.createAttributes", "Bat", Zero;
    11, "minecraft:bee", Living, None, LivingFlying, 1, true, "Bee.createAttributes", "Bee", Zero;
    12, "minecraft:birch_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    13, "minecraft:birch_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    14, "minecraft:blaze", Living, None, LivingFlying, 1, true, "Blaze.createAttributes", "Blaze", Zero;
    15, "minecraft:block_display", NonLiving, None, Immobile, 1, true, None, "Display.BlockDisplay", Zero;
    16, "minecraft:bogged", Living, None, LivingGround, 1, true, "Bogged.createAttributes", "Bogged", Zero;
    17, "minecraft:breeze", Living, None, LivingGround, 1, true, "Breeze.createAttributes", "Breeze", Zero;
    18, "minecraft:breeze_wind_charge", NonLiving, WindCharge, Projectile, 1, true, None, "BreezeWindCharge", ProjectileOwner;
    19, "minecraft:camel", Living, None, LivingGround, 3, true, "Camel.createAttributes", "Camel", Zero;
    20, "minecraft:camel_husk", Living, None, LivingGround, 3, true, "Camel.createAttributes", "CamelHusk", Zero;
    21, "minecraft:cat", Living, None, LivingGround, 1, true, "Cat.createAttributes", "Cat", Zero;
    22, "minecraft:cave_spider", Living, None, LivingGround, 1, true, "CaveSpider.createCaveSpider", "CaveSpider", Zero;
    23, "minecraft:cherry_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    24, "minecraft:cherry_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    25, "minecraft:chest_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartChest", Zero;
    26, "minecraft:chicken", Living, None, LivingGround, 1, true, "Chicken.createAttributes", "Chicken", Zero;
    27, "minecraft:cod", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "Cod", Zero;
    28, "minecraft:copper_golem", Living, None, LivingGround, 1, true, "CopperGolem.createAttributes", "CopperGolem", Zero;
    29, "minecraft:command_block_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartCommandBlock", Zero;
    30, "minecraft:cow", Living, None, LivingGround, 1, true, "Cow.createAttributes", "Cow", Zero;
    31, "minecraft:creaking", Living, None, LivingGround, 1, true, "Creaking.createAttributes", "Creaking", Zero;
    32, "minecraft:creeper", Living, None, LivingGround, 1, true, "Creeper.createAttributes", "Creeper", Zero;
    33, "minecraft:dark_oak_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    34, "minecraft:dark_oak_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    35, "minecraft:dolphin", Living, None, LivingAquatic, 1, true, "Dolphin.createAttributes", "Dolphin", Zero;
    36, "minecraft:donkey", Living, None, LivingGround, 1, true, "AbstractChestedHorse.createBaseChestedHorseAttributes", "Donkey", Zero;
    37, "minecraft:dragon_fireball", NonLiving, Hurting, Projectile, 1, true, None, "DragonFireball", ProjectileOwner;
    38, "minecraft:drowned", Living, None, LivingAmphibious, 1, true, "Drowned.createAttributes", "Drowned", Zero;
    39, "minecraft:egg", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownEgg", ProjectileOwner;
    40, "minecraft:elder_guardian", Living, None, LivingAquatic, 1, true, "ElderGuardian.createAttributes", "ElderGuardian", Zero;
    41, "minecraft:enderman", Living, None, LivingGround, 1, true, "EnderMan.createAttributes", "EnderMan", Zero;
    42, "minecraft:endermite", Living, None, LivingGround, 1, true, "Endermite.createAttributes", "Endermite", Zero;
    43, "minecraft:ender_dragon", Living, None, LivingFlying, 1, false, "EnderDragon.createAttributes", "EnderDragon", Zero;
    44, "minecraft:ender_pearl", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownEnderpearl", ProjectileOwner;
    45, "minecraft:end_crystal", NonLiving, None, Immobile, 1, true, None, "EndCrystal", Zero;
    46, "minecraft:evoker", Living, None, LivingGround, 1, true, "Evoker.createAttributes", "Evoker", Zero;
    47, "minecraft:evoker_fangs", NonLiving, None, Immobile, 1, true, None, "EvokerFangs", Zero;
    48, "minecraft:experience_bottle", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownExperienceBottle", ProjectileOwner;
    49, "minecraft:experience_orb", NonLiving, None, ExperienceOrb, 1, true, None, "ExperienceOrb", Zero;
    50, "minecraft:eye_of_ender", NonLiving, None, EyeOfEnder, 1, true, None, "EyeOfEnder", Zero;
    51, "minecraft:falling_block", NonLiving, None, FallingBlock, 1, true, None, "FallingBlockEntity", FallingBlockState;
    52, "minecraft:fireball", NonLiving, Hurting, Projectile, 1, true, None, "LargeFireball", ProjectileOwner;
    53, "minecraft:firework_rocket", NonLiving, FireworkRocket, Projectile, 1, true, None, "FireworkRocketEntity", ProjectileOwner;
    54, "minecraft:fox", Living, None, LivingGround, 1, true, "Fox.createAttributes", "Fox", Zero;
    55, "minecraft:frog", Living, None, LivingAmphibious, 1, true, "Frog.createAttributes", "Frog", Zero;
    56, "minecraft:furnace_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartFurnace", Zero;
    57, "minecraft:ghast", Living, None, LivingFlying, 1, true, "Ghast.createAttributes", "Ghast", Zero;
    58, "minecraft:happy_ghast", Living, None, LivingFlying, 4, true, "HappyGhast.createAttributes", "HappyGhast", Zero;
    59, "minecraft:giant", Living, None, LivingGround, 1, true, "Giant.createAttributes", "Giant", Zero;
    60, "minecraft:glow_item_frame", NonLiving, None, Immobile, 1, true, None, "GlowItemFrame", HangingDirection;
    61, "minecraft:glow_squid", Living, None, LivingAquatic, 1, true, "GlowSquid.createAttributes", "GlowSquid", Zero;
    62, "minecraft:goat", Living, None, LivingGround, 1, true, "Goat.createAttributes", "Goat", Zero;
    63, "minecraft:guardian", Living, None, LivingAquatic, 1, true, "Guardian.createAttributes", "Guardian", Zero;
    64, "minecraft:hoglin", Living, None, LivingGround, 1, true, "Hoglin.createAttributes", "Hoglin", Zero;
    65, "minecraft:hopper_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartHopper", Zero;
    66, "minecraft:horse", Living, None, LivingGround, 1, true, "AbstractHorse.createBaseHorseAttributes", "Horse", Zero;
    67, "minecraft:husk", Living, None, LivingGround, 1, true, "Zombie.createAttributes", "Husk", Zero;
    68, "minecraft:illusioner", Living, None, LivingGround, 1, true, "Illusioner.createAttributes", "Illusioner", Zero;
    69, "minecraft:interaction", NonLiving, None, Immobile, 1, true, None, "Interaction", Zero;
    70, "minecraft:iron_golem", Living, None, LivingGround, 1, true, "IronGolem.createAttributes", "IronGolem", Zero;
    71, "minecraft:item", NonLiving, None, Item, 1, true, None, "ItemEntity", Zero;
    72, "minecraft:item_display", NonLiving, None, Immobile, 1, true, None, "Display.ItemDisplay", Zero;
    73, "minecraft:item_frame", NonLiving, None, Immobile, 1, true, None, "ItemFrame", HangingDirection;
    74, "minecraft:jungle_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    75, "minecraft:jungle_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    76, "minecraft:leash_knot", NonLiving, None, Immobile, 0, true, None, "LeashFenceKnotEntity", Zero;
    77, "minecraft:lightning_bolt", NonLiving, None, Immobile, 0, true, None, "LightningBolt", Zero;
    78, "minecraft:llama", Living, None, LivingGround, 1, true, "Llama.createAttributes", "Llama", Zero;
    79, "minecraft:llama_spit", NonLiving, LlamaSpit, Projectile, 1, true, None, "LlamaSpit", ProjectileOwner;
    80, "minecraft:magma_cube", Living, None, LivingGround, 1, true, "MagmaCube.createAttributes", "MagmaCube", Zero;
    81, "minecraft:mangrove_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    82, "minecraft:mangrove_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    83, "minecraft:mannequin", Living, None, LivingGround, 1, true, "LivingEntity.createLivingAttributes", "Mannequin", Zero;
    84, "minecraft:marker", NonLiving, None, Immobile, 0, true, None, "Marker", NeverSent;
    85, "minecraft:minecart", NonLiving, None, Minecart, 1, true, None, "Minecart", Zero;
    86, "minecraft:mooshroom", Living, None, LivingGround, 1, true, "Cow.createAttributes", "MushroomCow", Zero;
    87, "minecraft:mule", Living, None, LivingGround, 1, true, "AbstractChestedHorse.createBaseChestedHorseAttributes", "Mule", Zero;
    88, "minecraft:nautilus", Living, None, LivingAquatic, 1, true, "Nautilus.createAttributes", "Nautilus", Zero;
    89, "minecraft:oak_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    90, "minecraft:oak_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    91, "minecraft:ocelot", Living, None, LivingGround, 1, true, "Ocelot.createAttributes", "Ocelot", Zero;
    92, "minecraft:ominous_item_spawner", NonLiving, None, Immobile, 0, true, None, "OminousItemSpawner", Zero;
    93, "minecraft:painting", NonLiving, None, Immobile, 1, true, None, "Painting", HangingDirection;
    94, "minecraft:pale_oak_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    95, "minecraft:pale_oak_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    96, "minecraft:panda", Living, None, LivingGround, 1, true, "Panda.createAttributes", "Panda", Zero;
    97, "minecraft:parched", Living, None, LivingGround, 1, true, "Parched.createAttributes", "Parched", Zero;
    98, "minecraft:parrot", Living, None, LivingFlying, 1, true, "Parrot.createAttributes", "Parrot", Zero;
    99, "minecraft:phantom", Living, None, LivingFlying, 1, true, "Monster.createMonsterAttributes", "Phantom", Zero;
    100, "minecraft:pig", Living, None, LivingGround, 1, true, "Pig.createAttributes", "Pig", Zero;
    101, "minecraft:piglin", Living, None, LivingGround, 1, true, "Piglin.createAttributes", "Piglin", Zero;
    102, "minecraft:piglin_brute", Living, None, LivingGround, 1, true, "PiglinBrute.createAttributes", "PiglinBrute", Zero;
    103, "minecraft:pillager", Living, None, LivingGround, 1, true, "Pillager.createAttributes", "Pillager", Zero;
    104, "minecraft:polar_bear", Living, None, LivingGround, 1, true, "PolarBear.createAttributes", "PolarBear", Zero;
    105, "minecraft:splash_potion", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownSplashPotion", ProjectileOwner;
    106, "minecraft:lingering_potion", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownLingeringPotion", ProjectileOwner;
    107, "minecraft:pufferfish", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "Pufferfish", Zero;
    108, "minecraft:rabbit", Living, None, LivingGround, 1, true, "Rabbit.createAttributes", "Rabbit", Zero;
    109, "minecraft:ravager", Living, None, LivingGround, 1, true, "Ravager.createAttributes", "Ravager", Zero;
    110, "minecraft:salmon", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "Salmon", Zero;
    111, "minecraft:sheep", Living, None, LivingGround, 1, true, "Sheep.createAttributes", "Sheep", Zero;
    112, "minecraft:shulker", Living, None, LivingAttached, 1, true, "Shulker.createAttributes", "Shulker", Zero;
    113, "minecraft:shulker_bullet", NonLiving, ShulkerBullet, Projectile, 1, true, None, "ShulkerBullet", ProjectileOwner;
    114, "minecraft:silverfish", Living, None, LivingGround, 1, true, "Silverfish.createAttributes", "Silverfish", Zero;
    115, "minecraft:skeleton", Living, None, LivingGround, 1, true, "AbstractSkeleton.createAttributes", "Skeleton", Zero;
    116, "minecraft:skeleton_horse", Living, None, LivingGround, 1, true, "SkeletonHorse.createAttributes", "SkeletonHorse", Zero;
    117, "minecraft:slime", Living, None, LivingGround, 1, true, "Monster.createMonsterAttributes", "Slime", Zero;
    118, "minecraft:small_fireball", NonLiving, Hurting, Projectile, 1, true, None, "SmallFireball", ProjectileOwner;
    119, "minecraft:sniffer", Living, None, LivingGround, 1, true, "Sniffer.createAttributes", "Sniffer", Zero;
    120, "minecraft:snowball", NonLiving, ThrowableItem, Projectile, 1, true, None, "Snowball", ProjectileOwner;
    121, "minecraft:snow_golem", Living, None, LivingGround, 1, true, "SnowGolem.createAttributes", "SnowGolem", Zero;
    122, "minecraft:spawner_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartSpawner", Zero;
    123, "minecraft:spectral_arrow", NonLiving, Arrow, Projectile, 1, true, None, "SpectralArrow", ProjectileOwner;
    124, "minecraft:spider", Living, None, LivingGround, 1, true, "Spider.createAttributes", "Spider", Zero;
    125, "minecraft:spruce_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    126, "minecraft:spruce_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    127, "minecraft:squid", Living, None, LivingAquatic, 1, true, "Squid.createAttributes", "Squid", Zero;
    128, "minecraft:stray", Living, None, LivingGround, 1, true, "AbstractSkeleton.createAttributes", "Stray", Zero;
    129, "minecraft:strider", Living, None, LivingGround, 1, true, "Strider.createAttributes", "Strider", Zero;
    130, "minecraft:tadpole", Living, None, LivingAquatic, 1, true, "Tadpole.createAttributes", "Tadpole", Zero;
    131, "minecraft:text_display", NonLiving, None, Immobile, 1, true, None, "Display.TextDisplay", Zero;
    132, "minecraft:tnt", NonLiving, None, PrimedTnt, 1, true, None, "PrimedTnt", Zero;
    133, "minecraft:tnt_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartTNT", Zero;
    134, "minecraft:trader_llama", Living, None, LivingGround, 1, true, "Llama.createAttributes", "TraderLlama", Zero;
    135, "minecraft:trident", NonLiving, Arrow, Projectile, 1, true, None, "ThrownTrident", ProjectileOwner;
    136, "minecraft:tropical_fish", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "TropicalFish", Zero;
    137, "minecraft:turtle", Living, None, LivingAmphibious, 1, true, "Turtle.createAttributes", "Turtle", Zero;
    138, "minecraft:vex", Living, None, LivingFlying, 1, true, "Vex.createAttributes", "Vex", Zero;
    139, "minecraft:villager", Living, None, LivingGround, 1, true, "Villager.createAttributes", "Villager", Zero;
    140, "minecraft:vindicator", Living, None, LivingGround, 1, true, "Vindicator.createAttributes", "Vindicator", Zero;
    141, "minecraft:wandering_trader", Living, None, LivingGround, 1, true, "Mob.createMobAttributes", "WanderingTrader", Zero;
    142, "minecraft:warden", Living, None, LivingGround, 1, false, "Warden.createAttributes", "Warden", WardenEmerging;
    143, "minecraft:wind_charge", NonLiving, WindCharge, Projectile, 1, true, None, "WindCharge", ProjectileOwner;
    144, "minecraft:witch", Living, None, LivingGround, 1, true, "Witch.createAttributes", "Witch", Zero;
    145, "minecraft:wither", Living, None, LivingFlying, 1, false, "WitherBoss.createAttributes", "WitherBoss", Zero;
    146, "minecraft:wither_skeleton", Living, None, LivingGround, 1, true, "AbstractSkeleton.createAttributes", "WitherSkeleton", Zero;
    147, "minecraft:wither_skull", NonLiving, Hurting, Projectile, 1, true, None, "WitherSkull", ProjectileOwner;
    148, "minecraft:wolf", Living, None, LivingGround, 1, true, "Wolf.createAttributes", "Wolf", Zero;
    149, "minecraft:zoglin", Living, None, LivingGround, 1, true, "Zoglin.createAttributes", "Zoglin", Zero;
    150, "minecraft:zombie", Living, None, LivingGround, 1, true, "Zombie.createAttributes", "Zombie", Zero;
    151, "minecraft:zombie_horse", Living, None, LivingGround, 1, true, "ZombieHorse.createAttributes", "ZombieHorse", Zero;
    152, "minecraft:zombie_nautilus", Living, None, LivingAquatic, 1, true, "ZombieNautilus.createAttributes", "ZombieNautilus", Zero;
    153, "minecraft:zombie_villager", Living, None, LivingGround, 1, true, "Zombie.createAttributes", "ZombieVillager", Zero;
    154, "minecraft:zombified_piglin", Living, None, LivingGround, 1, true, "ZombifiedPiglin.createAttributes", "ZombifiedPiglin", Zero;
    155, "minecraft:player", Living, None, LivingGround, 0, true, "Player.createAttributes", "Player", Zero;
    156, "minecraft:fishing_bobber", NonLiving, FishingBobber, Projectile, 0, true, None, "FishingHook", ProjectileOwnerOrSelf;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityTypeContract {
    pub protocol_id: u32,
    pub name: &'static str,
    pub behavior: EntityBehaviorContract,
    pub(crate) dimensions: EntityDimensions,
    pub(crate) tracking_range: u8,
    pub(crate) update_interval: u32,
    pub(crate) category: MobCategory,
    pub(crate) flags: EntityTypeFlags,
}

impl EntityTypeContract {
    /// Classifies a normal instance created from this registered entity type.
    #[must_use]
    pub const fn registered_instance_contract(self) -> EntityInstanceContract {
        EntityInstanceContract {
            category: EntityInstanceCategory::Registered,
            metadata_schema: self.behavior.metadata_schema,
            spawn_data: self.behavior.spawn_data,
        }
    }
}

/// Classifies an `EnderDragonPart`, which is deliberately outside the 157-row
/// entity-type registry even though vanilla stores `EntityType.ENDER_DRAGON`
/// on each part instance.
#[must_use]
pub const fn ender_dragon_part_instance_contract_26_1_2() -> EntityInstanceContract {
    EntityInstanceContract {
        category: EntityInstanceCategory::NonRegisteredDragonPart,
        metadata_schema: MetadataSchemaIdentity::new("EnderDragonPart"),
        spawn_data: SpawnDataCategory::NeverSent,
    }
}

const FLAGS_HOSTILE: EntityTypeFlags = EntityTypeFlags(0x03);
const FLAGS_FIRE_HOSTILE: EntityTypeFlags = EntityTypeFlags(0x07);
const FLAGS_FAR_HOSTILE: EntityTypeFlags = EntityTypeFlags(0x0b);
const FLAGS_STANDARD: EntityTypeFlags = EntityTypeFlags(0x13);
const FLAGS_FIRE_STANDARD: EntityTypeFlags = EntityTypeFlags(0x17);
const FLAGS_RUNTIME_ONLY: EntityTypeFlags = EntityTypeFlags(0x18);
const FLAGS_TRANSIENT_SUMMONABLE: EntityTypeFlags = EntityTypeFlags(0x1a);
const FLAGS_FAR_STANDARD: EntityTypeFlags = EntityTypeFlags(0x1b);
const FLAGS_FIRE_FAR_STANDARD: EntityTypeFlags = EntityTypeFlags(0x1f);

macro_rules! eye_height {
    (default, $height:expr) => {
        $height * 0.85_f32
    };
    ($eye_height:literal, $height:expr) => {
        $eye_height
    };
}

// Row fields: protocol ID, name, width, height, eye height (or `default`),
// spawn-dimensions scale, tracking range, update interval, category, flags.
// The row form keeps the independently verified source representation
// auditable while emitting one simple immutable table.
macro_rules! define_entity_contract {
    ($(
        $protocol_id:literal, $name:literal, $width:literal, $height:literal,
        $eye_height:tt, $spawn_scale:literal, $tracking_range:literal,
        $update_interval:expr, $category:ident, $flags:ident;
    )+) => {
        static ENTITY_TYPES: [EntityTypeContract; ENTITY_TYPE_COUNT] = [$(
            EntityTypeContract {
                protocol_id: $protocol_id,
                name: $name,
                behavior: ENTITY_BEHAVIORS[$protocol_id].behavior,
                dimensions: EntityDimensions {
                    width: $width,
                    height: $height,
                    eye_height: eye_height!($eye_height, $height),
                    spawn_dimensions_scale: $spawn_scale,
                },
                tracking_range: $tracking_range,
                update_interval: $update_interval,
                category: MobCategory::$category,
                flags: $flags,
            }
        ),+];
    };
}

define_entity_contract! {
    0, "minecraft:acacia_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    1, "minecraft:acacia_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    2, "minecraft:allay", 0.35, 0.6, 0.36, 1.0, 8, 2, Creature, FLAGS_FAR_STANDARD;
    3, "minecraft:area_effect_cloud", 6.0, 0.5, default, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FIRE_FAR_STANDARD;
    4, "minecraft:armadillo", 0.7, 0.65, 0.26, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    5, "minecraft:armor_stand", 0.5, 1.975, 1.7775, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    6, "minecraft:arrow", 0.5, 0.5, 0.13, 1.0, 4, 20, Misc, FLAGS_FAR_STANDARD;
    7, "minecraft:axolotl", 0.75, 0.42, 0.2751, 1.0, 10, 3, Axolotls, FLAGS_STANDARD;
    8, "minecraft:bamboo_chest_raft", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    9, "minecraft:bamboo_raft", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    10, "minecraft:bat", 0.5, 0.9, 0.45, 1.0, 5, 3, Ambient, FLAGS_STANDARD;
    11, "minecraft:bee", 0.7, 0.6, 0.3, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    12, "minecraft:birch_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    13, "minecraft:birch_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    14, "minecraft:blaze", 0.6, 1.8, default, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    15, "minecraft:block_display", 0.0, 0.0, default, 1.0, 10, 1, Misc, FLAGS_FAR_STANDARD;
    16, "minecraft:bogged", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    17, "minecraft:breeze", 0.6, 1.77, 1.3452, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    18, "minecraft:breeze_wind_charge", 0.3125, 0.3125, 0.0, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    19, "minecraft:camel", 1.7, 2.375, 2.275, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    20, "minecraft:camel_husk", 1.7, 2.375, 2.275, 1.0, 10, 3, Monster, FLAGS_STANDARD;
    21, "minecraft:cat", 0.6, 0.7, 0.35, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    22, "minecraft:cave_spider", 0.7, 0.5, 0.45, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    23, "minecraft:cherry_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    24, "minecraft:cherry_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    25, "minecraft:chest_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    26, "minecraft:chicken", 0.4, 0.7, 0.644, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    27, "minecraft:cod", 0.5, 0.3, 0.195, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    28, "minecraft:copper_golem", 0.49, 0.98, 0.8125, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    29, "minecraft:command_block_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    30, "minecraft:cow", 0.9, 1.4, 1.3, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    31, "minecraft:creaking", 0.9, 2.7, 2.3, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    32, "minecraft:creeper", 0.6, 1.7, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    33, "minecraft:dark_oak_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    34, "minecraft:dark_oak_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    35, "minecraft:dolphin", 0.9, 0.6, 0.3, 1.0, 5, 3, WaterCreature, FLAGS_STANDARD;
    36, "minecraft:donkey", 1.3964844, 1.5, 1.425, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    37, "minecraft:dragon_fireball", 1.0, 1.0, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    38, "minecraft:drowned", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    39, "minecraft:egg", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    40, "minecraft:elder_guardian", 1.9975, 1.9975, 0.99875, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    41, "minecraft:enderman", 0.6, 2.9, 2.55, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    42, "minecraft:endermite", 0.4, 0.3, 0.13, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    43, "minecraft:ender_dragon", 16.0, 8.0, default, 1.0, 10, 3, Monster, FLAGS_FIRE_STANDARD;
    44, "minecraft:ender_pearl", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    45, "minecraft:end_crystal", 2.0, 2.0, default, 1.0, 16, i32::MAX as u32, Misc, FLAGS_FIRE_FAR_STANDARD;
    46, "minecraft:evoker", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    47, "minecraft:evoker_fangs", 0.5, 0.8, default, 1.0, 6, 2, Misc, FLAGS_FAR_STANDARD;
    48, "minecraft:experience_bottle", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    49, "minecraft:experience_orb", 0.5, 0.5, default, 1.0, 6, 20, Misc, FLAGS_FAR_STANDARD;
    50, "minecraft:eye_of_ender", 0.25, 0.25, default, 1.0, 4, 4, Misc, FLAGS_FAR_STANDARD;
    51, "minecraft:falling_block", 0.98, 0.98, default, 1.0, 10, 20, Misc, FLAGS_FAR_STANDARD;
    52, "minecraft:fireball", 1.0, 1.0, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    53, "minecraft:firework_rocket", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    54, "minecraft:fox", 0.6, 0.7, 0.4, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    55, "minecraft:frog", 0.5, 0.5, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    56, "minecraft:furnace_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    57, "minecraft:ghast", 4.0, 4.0, 2.6, 1.0, 10, 3, Monster, FLAGS_FIRE_HOSTILE;
    58, "minecraft:happy_ghast", 4.0, 4.0, 2.6, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    59, "minecraft:giant", 3.6, 12.0, 10.44, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    60, "minecraft:glow_item_frame", 0.5, 0.5, 0.0, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FAR_STANDARD;
    61, "minecraft:glow_squid", 0.8, 0.8, 0.4, 1.0, 10, 3, UndergroundWaterCreature, FLAGS_STANDARD;
    62, "minecraft:goat", 0.9, 1.3, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    63, "minecraft:guardian", 0.85, 0.85, 0.425, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    64, "minecraft:hoglin", 1.3964844, 1.4, default, 1.0, 8, 3, Monster, FLAGS_STANDARD;
    65, "minecraft:hopper_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    66, "minecraft:horse", 1.3964844, 1.6, 1.52, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    67, "minecraft:husk", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    68, "minecraft:illusioner", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    69, "minecraft:interaction", 0.0, 0.0, default, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    70, "minecraft:iron_golem", 1.4, 2.7, default, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    71, "minecraft:item", 0.25, 0.25, 0.2125, 1.0, 6, 20, Misc, FLAGS_FAR_STANDARD;
    72, "minecraft:item_display", 0.0, 0.0, default, 1.0, 10, 1, Misc, FLAGS_FAR_STANDARD;
    73, "minecraft:item_frame", 0.5, 0.5, 0.0, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FAR_STANDARD;
    74, "minecraft:jungle_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    75, "minecraft:jungle_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    76, "minecraft:leash_knot", 0.375, 0.5, 0.0625, 1.0, 10, i32::MAX as u32, Misc, FLAGS_TRANSIENT_SUMMONABLE;
    77, "minecraft:lightning_bolt", 0.0, 0.0, default, 1.0, 16, i32::MAX as u32, Misc, FLAGS_TRANSIENT_SUMMONABLE;
    78, "minecraft:llama", 0.9, 1.87, 1.7765, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    79, "minecraft:llama_spit", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    80, "minecraft:magma_cube", 0.52, 0.52, 0.325, 4.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    81, "minecraft:mangrove_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    82, "minecraft:mangrove_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    83, "minecraft:mannequin", 0.6, 1.8, 1.62, 1.0, 32, 2, Misc, FLAGS_FAR_STANDARD;
    84, "minecraft:marker", 0.0, 0.0, default, 1.0, 0, 3, Misc, FLAGS_FAR_STANDARD;
    85, "minecraft:minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    86, "minecraft:mooshroom", 0.9, 1.4, 1.3, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    87, "minecraft:mule", 1.3964844, 1.6, 1.52, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    88, "minecraft:nautilus", 0.875, 0.95, 0.2751, 1.0, 10, 3, WaterCreature, FLAGS_STANDARD;
    89, "minecraft:oak_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    90, "minecraft:oak_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    91, "minecraft:ocelot", 0.6, 0.7, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    92, "minecraft:ominous_item_spawner", 0.25, 0.25, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    93, "minecraft:painting", 0.5, 0.5, default, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FAR_STANDARD;
    94, "minecraft:pale_oak_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    95, "minecraft:pale_oak_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    96, "minecraft:panda", 1.3, 1.25, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    97, "minecraft:parched", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    98, "minecraft:parrot", 0.5, 0.9, 0.54, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    99, "minecraft:phantom", 0.9, 0.5, 0.175, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    100, "minecraft:pig", 0.9, 0.9, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    101, "minecraft:piglin", 0.6, 1.95, 1.79, 1.0, 8, 3, Monster, FLAGS_STANDARD;
    102, "minecraft:piglin_brute", 0.6, 1.95, 1.79, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    103, "minecraft:pillager", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_FAR_HOSTILE;
    104, "minecraft:polar_bear", 1.4, 1.4, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    105, "minecraft:splash_potion", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    106, "minecraft:lingering_potion", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    107, "minecraft:pufferfish", 0.7, 0.7, 0.455, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    108, "minecraft:rabbit", 0.49, 0.6, 0.59, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    109, "minecraft:ravager", 1.95, 2.2, default, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    110, "minecraft:salmon", 0.7, 0.4, 0.26, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    111, "minecraft:sheep", 0.9, 1.3, 1.235, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    112, "minecraft:shulker", 1.0, 1.0, 0.5, 1.0, 10, 3, Monster, FLAGS_FIRE_FAR_STANDARD;
    113, "minecraft:shulker_bullet", 0.3125, 0.3125, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    114, "minecraft:silverfish", 0.4, 0.3, 0.13, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    115, "minecraft:skeleton", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    116, "minecraft:skeleton_horse", 1.3964844, 1.6, 1.52, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    117, "minecraft:slime", 0.52, 0.52, 0.325, 4.0, 10, 3, Monster, FLAGS_HOSTILE;
    118, "minecraft:small_fireball", 0.3125, 0.3125, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    119, "minecraft:sniffer", 1.9, 1.75, 1.05, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    120, "minecraft:snowball", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    121, "minecraft:snow_golem", 0.7, 1.9, 1.7, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    122, "minecraft:spawner_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    123, "minecraft:spectral_arrow", 0.5, 0.5, 0.13, 1.0, 4, 20, Misc, FLAGS_FAR_STANDARD;
    124, "minecraft:spider", 1.4, 0.9, 0.65, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    125, "minecraft:spruce_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    126, "minecraft:spruce_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    127, "minecraft:squid", 0.8, 0.8, 0.4, 1.0, 8, 3, WaterCreature, FLAGS_STANDARD;
    128, "minecraft:stray", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    129, "minecraft:strider", 0.9, 1.7, default, 1.0, 10, 3, Creature, FLAGS_FIRE_FAR_STANDARD;
    130, "minecraft:tadpole", 0.4, 0.3, 0.19500001, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    131, "minecraft:text_display", 0.0, 0.0, default, 1.0, 10, 1, Misc, FLAGS_FAR_STANDARD;
    132, "minecraft:tnt", 0.98, 0.98, 0.15, 1.0, 10, 10, Misc, FLAGS_FIRE_FAR_STANDARD;
    133, "minecraft:tnt_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    134, "minecraft:trader_llama", 0.9, 1.87, 1.7765, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    135, "minecraft:trident", 0.5, 0.5, 0.13, 1.0, 4, 20, Misc, FLAGS_FAR_STANDARD;
    136, "minecraft:tropical_fish", 0.5, 0.4, 0.26, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    137, "minecraft:turtle", 1.2, 0.4, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    138, "minecraft:vex", 0.4, 0.8, 0.51875, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    139, "minecraft:villager", 0.6, 1.95, 1.62, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    140, "minecraft:vindicator", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    141, "minecraft:wandering_trader", 0.6, 1.95, 1.62, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    142, "minecraft:warden", 0.9, 2.9, default, 1.0, 16, 3, Monster, FLAGS_FIRE_HOSTILE;
    143, "minecraft:wind_charge", 0.3125, 0.3125, 0.0, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    144, "minecraft:witch", 0.6, 1.95, 1.62, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    145, "minecraft:wither", 0.9, 3.5, default, 1.0, 10, 3, Monster, FLAGS_FIRE_HOSTILE;
    146, "minecraft:wither_skeleton", 0.7, 2.4, 2.1, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    147, "minecraft:wither_skull", 0.3125, 0.3125, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    148, "minecraft:wolf", 0.6, 0.85, 0.68, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    149, "minecraft:zoglin", 1.3964844, 1.4, default, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    150, "minecraft:zombie", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    151, "minecraft:zombie_horse", 1.3964844, 1.6, 1.52, 1.0, 10, 3, Monster, FLAGS_STANDARD;
    152, "minecraft:zombie_nautilus", 0.875, 0.95, 0.2751, 1.0, 10, 3, Monster, FLAGS_STANDARD;
    153, "minecraft:zombie_villager", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    154, "minecraft:zombified_piglin", 0.6, 1.95, 1.79, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    155, "minecraft:player", 0.6, 1.8, 1.62, 1.0, 32, 2, Misc, FLAGS_RUNTIME_ONLY;
    156, "minecraft:fishing_bobber", 0.25, 0.25, default, 1.0, 4, 5, Misc, FLAGS_RUNTIME_ONLY;
}

#[must_use]
pub(crate) fn by_protocol_id(protocol_id: u32) -> Option<EntityTypeContract> {
    let index = usize::try_from(protocol_id).ok()?;
    ENTITY_TYPES.get(index).copied()
}

#[must_use]
pub(crate) fn by_name(name: &str) -> Option<EntityTypeContract> {
    ENTITY_TYPES
        .iter()
        .find(|candidate| candidate.name == name)
        .copied()
}

#[must_use]
pub fn entity_type_contract_26_1_2_by_protocol_id(protocol_id: u32) -> Option<EntityTypeContract> {
    by_protocol_id(protocol_id)
}

#[must_use]
pub fn entity_type_contract_26_1_2_by_name(name: &str) -> Option<EntityTypeContract> {
    by_name(name)
}

pub(crate) fn entity_type_contracts_26_1_2()
-> impl ExactSizeIterator<Item = EntityTypeContract> + DoubleEndedIterator + Clone {
    ENTITY_TYPES.iter().copied()
}

#[cfg(test)]
#[path = "entity_contract_26_1_2_tests.rs"]
mod tests;
