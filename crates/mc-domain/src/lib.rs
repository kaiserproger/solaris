//! Protocol-independent gameplay value types shared across Solaris domains.
//!
//! This crate must not depend on transport, persistence, runtime, or generated
//! vanilla-data loaders. Wire codecs adapt to these values at the boundary.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Survival => 0,
            Self::Creative => 1,
            Self::Adventure => 2,
            Self::Spectator => 3,
        }
    }

    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Creative,
            2 => Self::Adventure,
            3 => Self::Spectator,
            _ => Self::Survival,
        }
    }
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Direction {
    #[must_use]
    pub const fn from_ordinal(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Down),
            1 => Some(Self::Up),
            2 => Some(Self::North),
            3 => Some(Self::South),
            4 => Some(Self::West),
            5 => Some(Self::East),
            _ => None,
        }
    }

    #[must_use]
    pub const fn normal(self) -> (i32, i32, i32) {
        match self {
            Self::Down => (0, -1, 0),
            Self::Up => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionHand {
    MainHand = 0,
    OffHand = 1,
}

impl InteractionHand {
    #[must_use]
    pub const fn from_ordinal(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::MainHand),
            1 => Some(Self::OffHand),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameplay_value_ordinals_match_java_contract() {
        assert_eq!(GameMode::from_id(3), GameMode::Spectator);
        assert_eq!(GameMode::Adventure.id(), 2);
        assert_eq!(Direction::from_ordinal(5), Some(Direction::East));
        assert_eq!(Direction::West.normal(), (-1, 0, 0));
        assert_eq!(
            InteractionHand::from_ordinal(1),
            Some(InteractionHand::OffHand)
        );
    }
}
