use mc_data::mob_effects_26_1_2::{MobEffect, name_for_id};
use mc_entity::effects_26_1_2::{
    ActiveEffectChainSnapshot, ActiveEffectsSnapshot, EffectFlags, EffectId, EffectInstance,
};
use mc_nbt::{ListTag, Tag, tag_type};

use crate::play::session::player_effects::{PlayerEffectsState, effect_kind};

use super::{byte_field, field, int_field, string_field};

pub(super) fn load(tag: &Tag) -> Result<Option<PlayerEffectsState>, &'static str> {
    let Tag::List(list) = tag else {
        return Err("active_effects must be a list");
    };
    if list.elements.is_empty() {
        return Ok(None);
    }
    let mut chains = Vec::with_capacity(list.elements.len());
    let mut action_order = Vec::with_capacity(list.elements.len());
    for element in &list.elements {
        let Tag::Compound(fields) = element else {
            return Err("active effect must be a compound");
        };
        let name = string_field(fields, "id").ok_or("active effect id is missing")?;
        let effect = MobEffect::from_name(name).ok_or("unknown active effect id")?;
        let current = read_details(fields, effect)?;
        let mut hidden = Vec::new();
        let mut next = field(fields, "hidden_effect");
        while let Some(tag) = next {
            let Tag::Compound(fields) = tag else {
                return Err("hidden effect must be a compound");
            };
            hidden.push(read_details(fields, effect)?);
            next = field(fields, "hidden_effect");
        }
        action_order.push(current.id);
        chains.push(ActiveEffectChainSnapshot { current, hidden });
    }
    chains.sort_unstable_by_key(|chain| chain.current.id);
    PlayerEffectsState::from_snapshot(mc_entity::EntityActiveEffectsState {
        effects: ActiveEffectsSnapshot { chains },
        action_order,
    })
    .map(Some)
}

fn read_details(
    fields: &[(String, Tag)],
    effect: MobEffect,
) -> Result<EffectInstance, &'static str> {
    let amplifier = int_field(fields, "amplifier").unwrap_or(0);
    if !(0..=255).contains(&amplifier) {
        return Err("active effect amplifier is out of range");
    }
    let duration = int_field(fields, "duration").unwrap_or(0);
    let visible = byte_field(fields, "show_particles").is_none_or(|value| value != 0);
    Ok(EffectInstance::new(
        EffectId::new(effect as u32),
        effect_kind(effect),
        duration,
        amplifier,
        EffectFlags {
            ambient: byte_field(fields, "ambient").is_some_and(|value| value != 0),
            visible,
            show_icon: byte_field(fields, "show_icon").map_or(visible, |value| value != 0),
        },
    ))
}

pub(super) fn save(state: Option<&PlayerEffectsState>) -> Result<Tag, &'static str> {
    let mut elements = Vec::new();
    if let Some(state) = state {
        let snapshot = state.snapshot();
        elements.reserve(snapshot.action_order.len());
        for id in snapshot.action_order {
            let chain = snapshot
                .effects
                .chains
                .iter()
                .find(|chain| chain.current.id == id)
                .ok_or("active-effect action order has no matching chain")?;
            let name = name_for_id(id.raw()).ok_or("unknown active effect id")?;
            let mut hidden = None;
            for effect in chain.hidden.iter().rev() {
                hidden = Some(Tag::Compound(write_details(*effect, hidden)));
            }
            let mut fields = write_details(chain.current, hidden);
            fields.push(("id".into(), Tag::String(name.into())));
            elements.push(Tag::Compound(fields));
        }
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn write_details(effect: EffectInstance, hidden: Option<Tag>) -> Vec<(String, Tag)> {
    let mut fields = Vec::with_capacity(7);
    fields.extend([
        ("amplifier".into(), Tag::Int(i32::from(effect.amplifier))),
        ("duration".into(), Tag::Int(effect.duration)),
        ("ambient".into(), Tag::Byte(i8::from(effect.flags.ambient))),
        (
            "show_particles".into(),
            Tag::Byte(i8::from(effect.flags.visible)),
        ),
        (
            "show_icon".into(),
            Tag::Byte(i8::from(effect.flags.show_icon)),
        ),
    ]);
    if let Some(hidden) = hidden {
        fields.push(("hidden_effect".into(), hidden));
    }
    fields
}

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;
