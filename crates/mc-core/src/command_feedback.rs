//! Neutral command text-component construction, without styling or delivery.

use mc_nbt::{ListTag, Tag, tag_type};

pub(crate) fn text(value: impl ToString) -> Tag {
    Tag::Compound(vec![("text".into(), Tag::String(value.to_string()))])
}

pub(crate) fn translated(key: impl Into<String>, arguments: Vec<Tag>) -> Tag {
    let mut fields = vec![("translate".into(), Tag::String(key.into()))];
    if !arguments.is_empty() {
        fields.push((
            "with".into(),
            Tag::List(ListTag {
                element_type: tag_type::COMPOUND,
                elements: arguments,
            }),
        ));
    }
    Tag::Compound(fields)
}

#[cfg(test)]
#[path = "command_feedback_tests.rs"]
mod tests;
