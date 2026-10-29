use super::{text, translated};
use mc_nbt::{Tag, tag_type};

#[test]
fn literal_text_has_one_string_field() {
    assert_eq!(
        text("name 😀"),
        Tag::Compound(vec![("text".into(), Tag::String("name 😀".into()))])
    );
}

#[test]
fn translation_without_arguments_omits_with() {
    assert_eq!(
        translated("commands.tick.status.frozen", Vec::new()),
        Tag::Compound(vec![(
            "translate".into(),
            Tag::String("commands.tick.status.frozen".into()),
        )])
    );
}

#[test]
fn translation_keeps_argument_order_and_compound_list_type() {
    let first = Tag::Compound(vec![("text".into(), Tag::String("first".into()))]);
    let second = Tag::Compound(vec![("translate".into(), Tag::String("second".into()))]);
    let Tag::Compound(fields) = translated(
        String::from("commands.example"),
        vec![first.clone(), second.clone()],
    ) else {
        panic!("expected compound");
    };
    assert!(
        fields
            .iter()
            .any(|(key, value)| key == "translate"
                && *value == Tag::String("commands.example".into()))
    );
    let Some(Tag::List(list)) = fields
        .iter()
        .find_map(|(key, value)| (key == "with").then_some(value))
    else {
        panic!("expected translation arguments");
    };
    assert_eq!(list.element_type, tag_type::COMPOUND);
    assert_eq!(list.elements, vec![first, second]);
}
