mod player_actions;
mod player_damage;

#[cfg(test)]
pub(in crate::play) use player_actions::{
    SHIELD_ACTIVATION_DELAY_TICKS, SHIELD_FALLBACK_MAX_DAMAGE, attack_damage_for_item,
    held_attack_damage, held_attack_damage_at_tick, held_attack_speed, is_durability_tool_path,
    shield_durability_damage,
};
pub(in crate::play) use player_actions::{
    ShieldUseState, begin_player_attack_attempt, damage_active_shield_slot,
    damage_active_shield_slots, damage_held_weapon_stack, max_tool_damage_for_path,
    player_horizontal_look_direction, shield_blocks_damage, shield_blocks_damage_since,
    shield_disable_ticks, shield_hand_slot, shield_use_flags, shield_use_from_stack,
    shield_use_matches, shield_use_matches_slot, stack_is_shield, weapon_attacks_damage_held_item,
};
pub(in crate::play) use player_damage::{
    ActiveShield, MeleeKnockback, PlayerDamageKind, PlayerDamageRequest, PlayerHurtResistance,
    PlayerHurtResolution, melee_knockback, shield_block_knockback,
};
