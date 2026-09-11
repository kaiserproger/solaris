use mc_data::recipes::{RecipeDataError, load_recipes};

#[test]
fn stew_recipe_rejects_an_effect_that_cannot_be_sent_to_the_client() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("stew.json"),
        r#"{
        "type": "minecraft:crafting_shapeless",
        "ingredients": ["minecraft:bowl"],
        "result": {
            "id": "minecraft:suspicious_stew",
            "components": {
                "minecraft:suspicious_stew_effects": [{"id": "minecraft:missing_effect"}]
            }
        }
    }"#,
    )
    .unwrap();
    assert!(
        matches!(load_recipes(root.path()), Err(RecipeDataError::InvalidIdentifier { value, .. }) if value == "minecraft:missing_effect")
    );
}

#[test]
fn vanilla_stew_nbt_without_duration_uses_the_vanilla_default() {
    use mc_nbt::{ListTag, Tag, tag_type};
    let tag = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![Tag::Compound(vec![(
            "id".into(),
            Tag::String("minecraft:poison".into()),
        )])],
    });
    let effects = mc_data::item_stack::decode_stew_effects(&tag).unwrap();
    assert_eq!(effects[0].duration, 160);
}
