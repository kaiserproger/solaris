use super::{
    EquipmentMutationError, EquipmentSlot, EquipmentState, ItemKey, ItemStackState, SlotRevision,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessedDurabilityChange {
    InfiniteMaterials,
    /// Amount after caller-owned enchantment processing.
    Apply(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurabilityUnchangedReason {
    NotDamageable,
    InfiniteMaterials,
    ZeroProcessedAmount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurabilityOutcome {
    Unchanged {
        reason: DurabilityUnchangedReason,
        stack: ItemStackState,
    },
    Changed {
        damage: i32,
        remaining: ItemStackState,
    },
    Broken {
        broken_item: ItemKey,
        damage: i32,
        remaining: ItemStackState,
        event_id: u8,
        stop_location_effects: bool,
    },
}

/// Mirrors Java's float-to-int conversion in `LivingEntity.doHurtEquipment`:
/// NaN enters vanilla's branch, but `Math.max` remains NaN and Java casts that
/// to zero; negative infinity is rejected and positive infinity casts to
/// `Integer.MAX_VALUE`. Zero is represented here as no mutation.
pub fn durability_damage_from_hurt(damage: f32) -> Option<i32> {
    if damage.is_nan() || damage <= 0.0 {
        return None;
    }
    Some((damage / 4.0).max(1.0) as i32)
}

impl EquipmentState {
    /// Applies the state-owned portion of `ItemStack.applyDamage` under a slot
    /// revision fence. Enchantment processing remains caller-owned.
    pub fn apply_equipment_durability(
        &mut self,
        slot: EquipmentSlot,
        expected_revision: SlotRevision,
        change: ProcessedDurabilityChange,
    ) -> Result<DurabilityOutcome, EquipmentMutationError> {
        self.check_revision(slot, expected_revision)?;
        let mut stack = self.get(slot).clone();
        if !stack.is_damageable() {
            return Ok(DurabilityOutcome::Unchanged {
                reason: DurabilityUnchangedReason::NotDamageable,
                stack,
            });
        }
        let amount = match change {
            ProcessedDurabilityChange::InfiniteMaterials => {
                return Ok(DurabilityOutcome::Unchanged {
                    reason: DurabilityUnchangedReason::InfiniteMaterials,
                    stack,
                });
            }
            ProcessedDurabilityChange::Apply(0) => {
                return Ok(DurabilityOutcome::Unchanged {
                    reason: DurabilityUnchangedReason::ZeroProcessedAmount,
                    stack,
                });
            }
            ProcessedDurabilityChange::Apply(amount) => amount,
        };

        let max_damage = stack.max_damage().expect("damageable stack has max damage");
        let damage = stack
            .damage_value()
            .expect("damageable stack has damage")
            .wrapping_add(amount)
            .clamp(0, max_damage);
        stack
            .set_damage_clamped(damage)
            .map_err(EquipmentMutationError::Component)?;
        if damage < max_damage {
            self.set(slot, stack.clone());
            return Ok(DurabilityOutcome::Changed {
                damage,
                remaining: stack,
            });
        }

        let broken_item = stack.item_key().expect("non-empty damageable stack");
        stack.shrink(1);
        self.set(slot, stack.clone());
        Ok(DurabilityOutcome::Broken {
            broken_item,
            damage,
            remaining: stack,
            event_id: slot.break_event_id(),
            stop_location_effects: true,
        })
    }
}
