/// Prepared placement policy; the entity hot path never calls the rule script.
#[derive(Debug, Clone, Copy)]
pub struct SpawnPlacement {
    land_spacing: u8,
    water_attempts: u8,
    water_depth: u8,
}

impl Default for SpawnPlacement {
    fn default() -> Self {
        Self {
            land_spacing: 4,
            water_attempts: 16,
            water_depth: 2,
        }
    }
}

impl SpawnPlacement {
    #[must_use]
    pub fn new(land_spacing: u8, water_attempts: u8, water_depth: u8) -> Option<Self> {
        ((1..=4).contains(&land_spacing)
            && (1..=32).contains(&water_attempts)
            && (1..=16).contains(&water_depth))
        .then_some(Self {
            land_spacing,
            water_attempts,
            water_depth,
        })
    }

    pub const fn land_spacing(self) -> u8 {
        self.land_spacing
    }
    pub const fn water_attempts(self) -> u8 {
        self.water_attempts
    }
    pub const fn water_depth(self) -> u8 {
        self.water_depth
    }
}
