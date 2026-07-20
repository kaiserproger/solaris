use crate::living_26_1_2::{LivingLifecycle, LivingState, StateError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    Killed,
    Discarded,
    UnloadedToChunk,
    UnloadedWithPlayer,
    ChangedDimension,
}

impl RemovalReason {
    #[must_use]
    pub const fn should_destroy(self) -> bool {
        matches!(self, Self::Killed | Self::Discarded)
    }

    #[must_use]
    pub const fn should_save(self) -> bool {
        matches!(self, Self::UnloadedToChunk)
    }

    #[must_use]
    pub const fn triggers_effect_removal_callbacks(self) -> bool {
        matches!(self, Self::Killed | Self::Discarded)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct StateRevision(u64);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuntimeState {
    living: LivingState,
    removal_reason: Option<RemovalReason>,
    revision: StateRevision,
}

impl RuntimeState {
    pub fn try_new(
        living: LivingState,
        removal_reason: Option<RemovalReason>,
    ) -> Result<Self, RuntimeStateError> {
        validate(living, removal_reason)?;
        Ok(Self {
            living,
            removal_reason,
            revision: StateRevision::default(),
        })
    }

    #[must_use]
    pub const fn living(&self) -> LivingState {
        self.living
    }

    #[must_use]
    pub const fn removal_reason(&self) -> Option<RemovalReason> {
        self.removal_reason
    }

    #[must_use]
    pub const fn revision(&self) -> StateRevision {
        self.revision
    }

    pub fn replace_living(&mut self, living: LivingState) -> Result<(), RuntimeStateError> {
        validate(living, self.removal_reason)?;
        self.living = living;
        self.revision.0 = self.revision.0.wrapping_add(1);
        Ok(())
    }

    pub(super) fn commit(&mut self, living: LivingState, removal_reason: Option<RemovalReason>) {
        self.living = living;
        self.removal_reason = removal_reason;
        self.revision.0 = self.revision.0.wrapping_add(1);
    }
}

fn validate(
    living: LivingState,
    removal_reason: Option<RemovalReason>,
) -> Result<(), RuntimeStateError> {
    living
        .validate()
        .map_err(RuntimeStateError::InvalidLiving)?;
    match (living.lifecycle, removal_reason) {
        (LivingLifecycle::Removed, None) => Err(RuntimeStateError::RemovedWithoutReason),
        (LivingLifecycle::Removed, Some(_)) | (_, None) => Ok(()),
        (_, Some(_)) => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStateError {
    InvalidLiving(StateError),
    RemovedWithoutReason,
}
