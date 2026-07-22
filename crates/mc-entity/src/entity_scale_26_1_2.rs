/// The Java Edition 26.1.2 `minecraft:scale` ranged-attribute contract.
///
/// The values come from `Attributes.SCALE` in the bundled 26.1.2 server:
/// default `1.0`, minimum `0.0625`, maximum `16.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityScale26_1_2(f32);

impl EntityScale26_1_2 {
    pub const DEFAULT: Self = Self(1.0);
    pub const MIN: Self = Self(0.0625);
    pub const MAX: Self = Self(16.0);

    pub fn try_new(value: f64) -> Result<Self, EntityScaleError> {
        if !value.is_finite() {
            return Err(EntityScaleError::NonFinite);
        }
        if !(f64::from(Self::MIN.0)..=f64::from(Self::MAX.0)).contains(&value) {
            return Err(EntityScaleError::OutOfRange);
        }
        Ok(Self(value as f32))
    }

    #[must_use]
    pub const fn factor(self) -> f32 {
        self.0
    }

    pub(crate) fn from_attribute_value(value: Option<f64>) -> Self {
        let Some(value) = value else {
            return Self::DEFAULT;
        };
        if value.is_nan() || value <= f64::from(Self::MIN.0) {
            return Self::MIN;
        }
        if value >= f64::from(Self::MAX.0) {
            return Self::MAX;
        }
        Self(value as f32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityScaleError {
    NonFinite,
    OutOfRange,
}
