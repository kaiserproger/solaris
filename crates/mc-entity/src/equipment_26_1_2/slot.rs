use core::fmt;

pub const EQUIPMENT_SLOT_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EquipmentSlotType {
    Hand,
    HumanoidArmor,
    AnimalArmor,
    Saddle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EquipmentSlot {
    MainHand,
    OffHand,
    Feet,
    Legs,
    Chest,
    Head,
    Body,
    Saddle,
}

impl EquipmentSlot {
    pub const VALUES: [Self; EQUIPMENT_SLOT_COUNT] = [
        Self::MainHand,
        Self::OffHand,
        Self::Feet,
        Self::Legs,
        Self::Chest,
        Self::Head,
        Self::Body,
        Self::Saddle,
    ];

    pub const fn ordinal(self) -> usize {
        match self {
            Self::MainHand => 0,
            Self::OffHand => 1,
            Self::Feet => 2,
            Self::Legs => 3,
            Self::Chest => 4,
            Self::Head => 5,
            Self::Body => 6,
            Self::Saddle => 7,
        }
    }

    pub const fn slot_type(self) -> EquipmentSlotType {
        match self {
            Self::MainHand | Self::OffHand => EquipmentSlotType::Hand,
            Self::Feet | Self::Legs | Self::Chest | Self::Head => EquipmentSlotType::HumanoidArmor,
            Self::Body => EquipmentSlotType::AnimalArmor,
            Self::Saddle => EquipmentSlotType::Saddle,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::MainHand | Self::Feet | Self::Body | Self::Saddle => 0,
            Self::OffHand | Self::Legs => 1,
            Self::Chest => 2,
            Self::Head => 3,
        }
    }

    pub const fn index_from(self, base: i32) -> i32 {
        base + self.index() as i32
    }

    pub const fn count_limit(self) -> u32 {
        match self.slot_type() {
            EquipmentSlotType::Hand => 0,
            EquipmentSlotType::HumanoidArmor
            | EquipmentSlotType::AnimalArmor
            | EquipmentSlotType::Saddle => 1,
        }
    }

    pub const fn id(self) -> u8 {
        match self {
            Self::MainHand => 0,
            Self::Feet => 1,
            Self::Legs => 2,
            Self::Chest => 3,
            Self::Head => 4,
            Self::OffHand => 5,
            Self::Body => 6,
            Self::Saddle => 7,
        }
    }

    /// Matches vanilla's continuous ID mapper with `ZERO` fallback.
    pub const fn by_id(id: i32) -> Self {
        match id {
            0 => Self::MainHand,
            1 => Self::Feet,
            2 => Self::Legs,
            3 => Self::Chest,
            4 => Self::Head,
            5 => Self::OffHand,
            6 => Self::Body,
            7 => Self::Saddle,
            _ => Self::MainHand,
        }
    }

    pub const fn serialized_name(self) -> &'static str {
        match self {
            Self::MainHand => "mainhand",
            Self::OffHand => "offhand",
            Self::Feet => "feet",
            Self::Legs => "legs",
            Self::Chest => "chest",
            Self::Head => "head",
            Self::Body => "body",
            Self::Saddle => "saddle",
        }
    }

    pub fn by_name(name: &str) -> Result<Self, EquipmentSlotNameError> {
        Self::VALUES
            .into_iter()
            .find(|slot| slot.serialized_name() == name)
            .ok_or(EquipmentSlotNameError)
    }

    pub const fn is_armor(self) -> bool {
        matches!(
            self.slot_type(),
            EquipmentSlotType::HumanoidArmor | EquipmentSlotType::AnimalArmor
        )
    }

    pub const fn can_increase_experience(self) -> bool {
        !matches!(self, Self::Saddle)
    }

    pub const fn filter_bit(self, offset: i32) -> i32 {
        self.id() as i32 + offset
    }

    pub const fn from_living_slot(raw_slot: i32) -> Option<Self> {
        match raw_slot {
            98 => Some(Self::MainHand),
            99 => Some(Self::OffHand),
            100 => Some(Self::Feet),
            101 => Some(Self::Legs),
            102 => Some(Self::Chest),
            103 => Some(Self::Head),
            105 => Some(Self::Body),
            106 => Some(Self::Saddle),
            _ => None,
        }
    }

    pub const fn break_event_id(self) -> u8 {
        match self {
            Self::MainHand => 47,
            Self::OffHand => 48,
            Self::Head => 49,
            Self::Chest => 50,
            Self::Legs => 51,
            Self::Feet => 52,
            Self::Body => 65,
            Self::Saddle => 68,
        }
    }
}

impl fmt::Display for EquipmentSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.serialized_name())
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EquipmentSlotGroup {
    Any,
    MainHand,
    OffHand,
    Hand,
    Feet,
    Legs,
    Chest,
    Head,
    Armor,
    Body,
    Saddle,
}

const MAIN_HAND: [EquipmentSlot; 1] = [EquipmentSlot::MainHand];
const OFF_HAND: [EquipmentSlot; 1] = [EquipmentSlot::OffHand];
const HAND: [EquipmentSlot; 2] = [EquipmentSlot::MainHand, EquipmentSlot::OffHand];
const FEET: [EquipmentSlot; 1] = [EquipmentSlot::Feet];
const LEGS: [EquipmentSlot; 1] = [EquipmentSlot::Legs];
const CHEST: [EquipmentSlot; 1] = [EquipmentSlot::Chest];
const HEAD: [EquipmentSlot; 1] = [EquipmentSlot::Head];
const ARMOR: [EquipmentSlot; 5] = [
    EquipmentSlot::Feet,
    EquipmentSlot::Legs,
    EquipmentSlot::Chest,
    EquipmentSlot::Head,
    EquipmentSlot::Body,
];
const BODY: [EquipmentSlot; 1] = [EquipmentSlot::Body];
const SADDLE: [EquipmentSlot; 1] = [EquipmentSlot::Saddle];

impl EquipmentSlotGroup {
    pub const VALUES: [Self; 11] = [
        Self::Any,
        Self::MainHand,
        Self::OffHand,
        Self::Hand,
        Self::Feet,
        Self::Legs,
        Self::Chest,
        Self::Head,
        Self::Armor,
        Self::Body,
        Self::Saddle,
    ];

    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Matches vanilla's continuous ID mapper with `ZERO` fallback.
    pub const fn by_id(id: i32) -> Self {
        match id {
            1 => Self::MainHand,
            2 => Self::OffHand,
            3 => Self::Hand,
            4 => Self::Feet,
            5 => Self::Legs,
            6 => Self::Chest,
            7 => Self::Head,
            8 => Self::Armor,
            9 => Self::Body,
            10 => Self::Saddle,
            _ => Self::Any,
        }
    }

    pub const fn serialized_name(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::MainHand => "mainhand",
            Self::OffHand => "offhand",
            Self::Hand => "hand",
            Self::Feet => "feet",
            Self::Legs => "legs",
            Self::Chest => "chest",
            Self::Head => "head",
            Self::Armor => "armor",
            Self::Body => "body",
            Self::Saddle => "saddle",
        }
    }

    pub fn by_name(name: &str) -> Result<Self, EquipmentSlotGroupNameError> {
        Self::VALUES
            .into_iter()
            .find(|group| group.serialized_name() == name)
            .ok_or(EquipmentSlotGroupNameError)
    }

    pub const fn by_slot(slot: EquipmentSlot) -> Self {
        match slot {
            EquipmentSlot::MainHand => Self::MainHand,
            EquipmentSlot::OffHand => Self::OffHand,
            EquipmentSlot::Feet => Self::Feet,
            EquipmentSlot::Legs => Self::Legs,
            EquipmentSlot::Chest => Self::Chest,
            EquipmentSlot::Head => Self::Head,
            EquipmentSlot::Body => Self::Body,
            EquipmentSlot::Saddle => Self::Saddle,
        }
    }

    pub const fn test(self, slot: EquipmentSlot) -> bool {
        match self {
            Self::Any => true,
            Self::MainHand => matches!(slot, EquipmentSlot::MainHand),
            Self::OffHand => matches!(slot, EquipmentSlot::OffHand),
            Self::Hand => matches!(slot.slot_type(), EquipmentSlotType::Hand),
            Self::Feet => matches!(slot, EquipmentSlot::Feet),
            Self::Legs => matches!(slot, EquipmentSlot::Legs),
            Self::Chest => matches!(slot, EquipmentSlot::Chest),
            Self::Head => matches!(slot, EquipmentSlot::Head),
            Self::Armor => slot.is_armor(),
            Self::Body => matches!(slot, EquipmentSlot::Body),
            Self::Saddle => matches!(slot, EquipmentSlot::Saddle),
        }
    }

    pub const fn slots(self) -> &'static [EquipmentSlot] {
        match self {
            Self::Any => &EquipmentSlot::VALUES,
            Self::MainHand => &MAIN_HAND,
            Self::OffHand => &OFF_HAND,
            Self::Hand => &HAND,
            Self::Feet => &FEET,
            Self::Legs => &LEGS,
            Self::Chest => &CHEST,
            Self::Head => &HEAD,
            Self::Armor => &ARMOR,
            Self::Body => &BODY,
            Self::Saddle => &SADDLE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquipmentSlotNameError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EquipmentSlotGroupNameError;
