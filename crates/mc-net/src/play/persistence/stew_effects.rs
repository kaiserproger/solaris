use mc_data::item_stack::{
    STEW_EFFECTS_COMPONENT, StewEffect, decode_stew_effects, encode_stew_effects,
};
use mc_nbt::Tag;

use super::{PlayerPersistenceError, field, field_mut, set_field};

pub(super) fn load(fields: &[(String, Tag)]) -> Result<Vec<StewEffect>, PlayerPersistenceError> {
    let Some(Tag::Compound(components)) = field(fields, "components") else {
        return Ok(Vec::new());
    };
    field(components, STEW_EFFECTS_COMPONENT)
        .map(|tag| {
            decode_stew_effects(tag)
                .map_err(|reason| PlayerPersistenceError::InvalidItemId(reason.into()))
        })
        .transpose()
        .map(Option::unwrap_or_default)
}

pub(super) fn save(fields: &mut Vec<(String, Tag)>, effects: &[StewEffect]) {
    if let Some(Tag::Compound(components)) = field_mut(fields, "components") {
        if effects.is_empty() {
            components.retain(|(name, _)| name != STEW_EFFECTS_COMPONENT);
        } else {
            set_field(
                components,
                STEW_EFFECTS_COMPONENT,
                encode_stew_effects(effects),
            );
        }
    } else if !effects.is_empty() {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(
                STEW_EFFECTS_COMPONENT.into(),
                encode_stew_effects(effects),
            )]),
        );
    }
}
