//! Vanilla crafting recipe reader.
//!
//! This loader keeps the data sidecar boundary in `mc-data`: it parses
//! shaped, shapeless, cooking, and bounded stonecutting recipe JSON into
//! small Solaris data types without executing crafting or depending on Play
//! inventory packets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{Identifier, read_json_file, visit_json_files};

const REQUIRED_RECIPES: &str = include_str!("../data/required_recipes.json");

#[derive(Debug, Error)]
pub enum RecipeDataError {
    #[error("recipe data io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("recipe data parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error("invalid shaped recipe key {key:?} in {path}")]
    InvalidKey { path: PathBuf, key: String },
    #[error("shaped recipe {path} references missing key {key:?}")]
    MissingKey { path: PathBuf, key: char },
    #[error("invalid ingredient in {path}: alternatives must not be empty")]
    EmptyIngredient { path: PathBuf },
    #[error("invalid shaped recipe pattern in {path}: rows must be non-empty and rectangular")]
    InvalidPattern { path: PathBuf },
    #[error("invalid cooking experience {value} in {path}")]
    InvalidCookingExperience { path: PathBuf, value: f64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    pub id: Identifier,
    pub kind: RecipeKind,
    pub result: RecipeResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeKind {
    Shaped(ShapedRecipe),
    Shapeless(ShapelessRecipe),
    Smelting(SmeltingRecipe),
    Blasting(SmeltingRecipe),
    Smoking(SmeltingRecipe),
    CampfireCooking(SmeltingRecipe),
    Stonecutting(StonecuttingRecipe),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapedRecipe {
    pub pattern: Vec<String>,
    pub key: BTreeMap<char, Ingredient>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapelessRecipe {
    pub ingredients: Vec<Ingredient>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmeltingRecipe {
    pub ingredient: Ingredient,
    pub cooking_time: u32,
    pub experience_milli: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StonecuttingRecipe {
    pub ingredient: Ingredient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingredient {
    pub alternatives: Vec<IngredientAlternative>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngredientAlternative {
    Item(Identifier),
    Tag(Identifier),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeResult {
    pub item: Identifier,
    pub count: u32,
}

/// Load shaped, shapeless, cooking, and simple stonecutting recipes from
/// `<data>/minecraft/recipe`.
///
/// Stonecutting is deliberately bounded to one ingredient plus an id/count
/// result. Component-bearing outputs and unsupported recipe types are skipped,
/// so Solaris never advertises an output it cannot reproduce. A missing
/// directory is not an error: startup keeps working without sidecar recipe data.
pub fn load_recipes(recipe_dir: impl AsRef<Path>) -> Result<Vec<Recipe>, RecipeDataError> {
    let recipe_dir = recipe_dir.as_ref();
    if !recipe_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    visit_json_files(
        recipe_dir,
        &mut |path| {
            paths.push(path);
            Ok(())
        },
        &|path, source| RecipeDataError::Io { path, source },
    )?;
    paths.sort();

    let mut recipes = Vec::new();
    for path in paths {
        if let Some(recipe) = load_one_recipe(recipe_dir, &path)? {
            recipes.push(recipe);
        }
    }
    Ok(recipes)
}

/// Repo-owned crafting baseline for running Solaris without a local recipe sidecar.
#[must_use]
pub fn solaris_required_recipes() -> Vec<Recipe> {
    let path = Path::new("crates/mc-data/data/required_recipes.json");
    let mut raw: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(REQUIRED_RECIPES).expect("embedded required recipe JSON is valid");
    // Keep legacy fallback display ids stable while adding the vanilla-id recipe.
    let bone_meal = raw.remove("minecraft:bone_meal");
    let mut recipes = raw
        .into_iter()
        .map(|(id, value)| {
            let id = Identifier::parse(id).expect("embedded required recipe id is valid");
            parse_recipe_value(path, id, value)
                .expect("embedded required recipe JSON uses supported recipe shapes")
                .expect("embedded required recipe JSON contains supported recipe types")
        })
        .collect::<Vec<_>>();
    if let Some(value) = bone_meal {
        let id = Identifier::parse("minecraft:bone_meal").expect("static identifier");
        recipes.push(
            parse_recipe_value(path, id, value)
                .expect("embedded bone meal recipe uses a supported shape")
                .expect("embedded bone meal recipe is supported"),
        );
    }
    recipes
}

fn load_one_recipe(root: &Path, path: &Path) -> Result<Option<Recipe>, RecipeDataError> {
    let id = id_from_file(root, path)?;
    let value: serde_json::Value = read_json_file(
        path,
        &|path, source| RecipeDataError::Io { path, source },
        &|path, source| RecipeDataError::Parse { path, source },
    )?;
    parse_recipe_value(path, id, value)
}

fn parse_recipe_value(
    path: &Path,
    id: Identifier,
    value: serde_json::Value,
) -> Result<Option<Recipe>, RecipeDataError> {
    let header: RawRecipeHeader = from_value(path, value.clone())?;
    match header.kind.as_str() {
        "minecraft:crafting_shaped" => {
            let raw: RawShapedRecipe = from_value(path, value)?;
            Ok(Some(parse_shaped_recipe(path, id, raw)?))
        }
        "minecraft:crafting_shapeless" => {
            let raw: RawShapelessRecipe = from_value(path, value)?;
            Ok(Some(parse_shapeless_recipe(path, id, raw)?))
        }
        "minecraft:smelting"
        | "minecraft:blasting"
        | "minecraft:smoking"
        | "minecraft:campfire_cooking" => {
            let raw: RawSmeltingRecipe = from_value(path, value)?;
            Ok(Some(parse_smelting_recipe(path, id, raw, header.kind)?))
        }
        "minecraft:stonecutting" if stonecutting_result_is_supported(&value) => {
            let raw: RawStonecuttingRecipe = from_value(path, value)?;
            Ok(Some(Recipe {
                id,
                kind: RecipeKind::Stonecutting(StonecuttingRecipe {
                    ingredient: parse_ingredient(path, raw.ingredient)?,
                }),
                result: parse_result(path, raw.result)?,
            }))
        }
        _ => Ok(None),
    }
}

fn stonecutting_result_is_supported(value: &serde_json::Value) -> bool {
    let Some(result) = value.get("result") else {
        return false;
    };
    match result {
        serde_json::Value::String(_) => true,
        serde_json::Value::Object(fields) => fields
            .keys()
            .all(|key| matches!(key.as_str(), "id" | "count")),
        _ => false,
    }
}

fn parse_shaped_recipe(
    path: &Path,
    id: Identifier,
    raw: RawShapedRecipe,
) -> Result<Recipe, RecipeDataError> {
    validate_pattern(path, &raw.pattern)?;

    let mut key = BTreeMap::new();
    for (raw_key, raw_ingredient) in raw.key {
        let mut chars = raw_key.chars();
        let Some(ch) = chars.next() else {
            return Err(RecipeDataError::InvalidKey {
                path: path.to_path_buf(),
                key: raw_key,
            });
        };
        if chars.next().is_some() || ch == ' ' {
            return Err(RecipeDataError::InvalidKey {
                path: path.to_path_buf(),
                key: raw_key,
            });
        }
        key.insert(ch, parse_ingredient(path, raw_ingredient)?);
    }

    for row in &raw.pattern {
        for ch in row.chars().filter(|ch| *ch != ' ') {
            if !key.contains_key(&ch) {
                return Err(RecipeDataError::MissingKey {
                    path: path.to_path_buf(),
                    key: ch,
                });
            }
        }
    }

    Ok(Recipe {
        id,
        kind: RecipeKind::Shaped(ShapedRecipe {
            pattern: raw.pattern,
            key,
        }),
        result: parse_result(path, raw.result)?,
    })
}

fn parse_shapeless_recipe(
    path: &Path,
    id: Identifier,
    raw: RawShapelessRecipe,
) -> Result<Recipe, RecipeDataError> {
    let ingredients = raw
        .ingredients
        .into_iter()
        .map(|ingredient| parse_ingredient(path, ingredient))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Recipe {
        id,
        kind: RecipeKind::Shapeless(ShapelessRecipe { ingredients }),
        result: parse_result(path, raw.result)?,
    })
}

fn parse_smelting_recipe(
    path: &Path,
    id: Identifier,
    raw: RawSmeltingRecipe,
    raw_kind: String,
) -> Result<Recipe, RecipeDataError> {
    let smelting = SmeltingRecipe {
        ingredient: parse_ingredient(path, raw.ingredient)?,
        cooking_time: raw.cooking_time,
        experience_milli: cooking_experience_milli(path, raw.experience)?,
    };
    let kind = match raw_kind.as_str() {
        "minecraft:smelting" => RecipeKind::Smelting(smelting),
        "minecraft:blasting" => RecipeKind::Blasting(smelting),
        "minecraft:smoking" => RecipeKind::Smoking(smelting),
        "minecraft:campfire_cooking" => RecipeKind::CampfireCooking(smelting),
        _ => unreachable!("caller filters supported cooking recipe types"),
    };
    Ok(Recipe {
        id,
        kind,
        result: parse_result(path, raw.result)?,
    })
}

fn validate_pattern(path: &Path, pattern: &[String]) -> Result<(), RecipeDataError> {
    let Some(width) = pattern.first().map(String::len) else {
        return Err(RecipeDataError::InvalidPattern {
            path: path.to_path_buf(),
        });
    };
    if width == 0 || pattern.iter().any(|row| row.len() != width) {
        return Err(RecipeDataError::InvalidPattern {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn parse_ingredient(path: &Path, raw: RawIngredient) -> Result<Ingredient, RecipeDataError> {
    let alternatives = match raw {
        RawIngredient::Single(value) => vec![parse_ingredient_alternative(path, value)?],
        RawIngredient::Alternatives(values) => values
            .into_iter()
            .map(|value| parse_ingredient_alternative(path, value))
            .collect::<Result<Vec<_>, _>>()?,
    };
    if alternatives.is_empty() {
        return Err(RecipeDataError::EmptyIngredient {
            path: path.to_path_buf(),
        });
    }
    Ok(Ingredient { alternatives })
}

fn parse_ingredient_alternative(
    path: &Path,
    value: RawIngredientValue,
) -> Result<IngredientAlternative, RecipeDataError> {
    match value {
        RawIngredientValue::String(value) => {
            if let Some(tag) = value.strip_prefix('#') {
                parse_id(path, tag.to_string()).map(IngredientAlternative::Tag)
            } else {
                parse_id(path, value).map(IngredientAlternative::Item)
            }
        }
        RawIngredientValue::Object {
            item: Some(item),
            tag: None,
        } => parse_id(path, item).map(IngredientAlternative::Item),
        RawIngredientValue::Object {
            item: None,
            tag: Some(tag),
        } => parse_id(path, tag).map(IngredientAlternative::Tag),
        RawIngredientValue::Object { item, tag } => Err(RecipeDataError::InvalidIdentifier {
            path: path.to_path_buf(),
            value: format!("ingredient object item={item:?} tag={tag:?}"),
        }),
    }
}

fn parse_result(path: &Path, raw: RawResult) -> Result<RecipeResult, RecipeDataError> {
    let (item, count) = match raw {
        RawResult::Object { id, count } => (id, count),
        RawResult::Id(id) => (id, default_count()),
    };
    Ok(RecipeResult {
        item: parse_id(path, item)?,
        count,
    })
}

fn id_from_file(root: &Path, path: &Path) -> Result<Identifier, RecipeDataError> {
    let rel = path
        .strip_prefix(root)
        .expect("walk yields paths under recipe root")
        .with_extension("");
    let mut joined = String::from("minecraft:");
    for component in rel.components() {
        if !joined.ends_with(':') {
            joined.push('/');
        }
        joined.push_str(component.as_os_str().to_string_lossy().as_ref());
    }
    parse_id(path, joined)
}

fn parse_id(path: &Path, value: String) -> Result<Identifier, RecipeDataError> {
    Identifier::parse(value.clone()).map_err(|_| RecipeDataError::InvalidIdentifier {
        path: path.to_path_buf(),
        value,
    })
}

fn from_value<T: for<'de> Deserialize<'de>>(
    path: &Path,
    value: serde_json::Value,
) -> Result<T, RecipeDataError> {
    serde_json::from_value(value).map_err(|source| RecipeDataError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn default_count() -> u32 {
    1
}

fn default_cooking_time() -> u32 {
    200
}

fn cooking_experience_milli(path: &Path, experience: f64) -> Result<u32, RecipeDataError> {
    let scaled = experience * 1000.0;
    if !scaled.is_finite() || scaled < 0.0 || scaled > f64::from(u32::MAX) {
        return Err(RecipeDataError::InvalidCookingExperience {
            path: path.to_path_buf(),
            value: experience,
        });
    }
    Ok(scaled.round() as u32)
}

#[derive(Deserialize)]
struct RawRecipeHeader {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct RawShapedRecipe {
    #[serde(rename = "type")]
    _kind: String,
    pattern: Vec<String>,
    key: BTreeMap<String, RawIngredient>,
    result: RawResult,
}

#[derive(Deserialize)]
struct RawShapelessRecipe {
    #[serde(rename = "type")]
    _kind: String,
    ingredients: Vec<RawIngredient>,
    result: RawResult,
}

#[derive(Deserialize)]
struct RawSmeltingRecipe {
    #[serde(rename = "type")]
    _kind: String,
    ingredient: RawIngredient,
    result: RawResult,
    #[serde(
        default = "default_cooking_time",
        rename = "cookingtime",
        alias = "cooking_time"
    )]
    cooking_time: u32,
    #[serde(default)]
    experience: f64,
}

#[derive(Deserialize)]
struct RawStonecuttingRecipe {
    #[serde(rename = "type")]
    _kind: String,
    ingredient: RawIngredient,
    result: RawResult,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIngredient {
    Single(RawIngredientValue),
    Alternatives(Vec<RawIngredientValue>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIngredientValue {
    String(String),
    Object {
        item: Option<String>,
        tag: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawResult {
    Object {
        id: String,
        #[serde(default = "default_count")]
        count: u32,
    },
    Id(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn loads_synthetic_shaped_and_shapeless_recipes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("building_blocks/test_planks.json"),
            r###"{
              "type": "minecraft:crafting_shaped",
              "pattern": ["##", "##"],
              "key": { "#": ["minecraft:oak_log", "minecraft:birch_log"] },
              "result": { "id": "minecraft:test_planks", "count": 4 }
            }"###,
        );
        write(
            &root.join("misc/test_stick.json"),
            r#"{
              "type": "minecraft:crafting_shapeless",
              "ingredients": ["minecraft:test_planks", ["minecraft:oak_planks", "minecraft:birch_planks"]],
              "result": { "id": "minecraft:stick", "count": 4 }
            }"#,
        );
        write(
            &root.join("ignored/test_smelting.json"),
            r#"{
              "type": "minecraft:smelting",
              "ingredient": "minecraft:iron_ore",
              "result": { "id": "minecraft:iron_ingot" }
            }"#,
        );
        write(
            &root.join("ignored/test_blasting.json"),
            r#"{
              "type": "minecraft:blasting",
              "ingredient": "minecraft:raw_iron",
              "result": { "id": "minecraft:iron_ingot" },
              "cooking_time": 100
            }"#,
        );
        write(
            &root.join("ignored/test_smoking.json"),
            r#"{
              "type": "minecraft:smoking",
              "ingredient": "minecraft:beef",
              "result": { "id": "minecraft:cooked_beef" },
              "cooking_time": 100
            }"#,
        );
        write(
            &root.join("ignored/test_campfire.json"),
            r#"{
              "type": "minecraft:campfire_cooking",
              "ingredient": "minecraft:porkchop",
              "result": { "id": "minecraft:cooked_porkchop" },
              "cooking_time": 600
            }"#,
        );

        let recipes = load_recipes(root).unwrap();

        assert_eq!(recipes.len(), 6);
        assert_eq!(
            recipes[0].id.as_str(),
            "minecraft:building_blocks/test_planks"
        );
        assert_eq!(recipes[0].result.item.as_str(), "minecraft:test_planks");
        assert_eq!(recipes[0].result.count, 4);
        let RecipeKind::Shaped(shaped) = &recipes[0].kind else {
            panic!("expected shaped recipe");
        };
        assert_eq!(shaped.pattern, ["##", "##"]);
        assert_eq!(
            shaped.key[&'#']
                .alternatives
                .iter()
                .map(|alternative| match alternative {
                    IngredientAlternative::Item(id) => id.as_str(),
                    IngredientAlternative::Tag(id) => id.as_str(),
                })
                .collect::<Vec<_>>(),
            ["minecraft:oak_log", "minecraft:birch_log"]
        );

        let stick = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == "minecraft:misc/test_stick")
            .unwrap();
        let RecipeKind::Shapeless(shapeless) = &stick.kind else {
            panic!("expected shapeless recipe");
        };
        assert_eq!(shapeless.ingredients.len(), 2);
        assert_eq!(
            shapeless.ingredients[0].alternatives[0],
            IngredientAlternative::Item(Identifier::parse("minecraft:test_planks").unwrap())
        );
        assert_eq!(
            shapeless.ingredients[1].alternatives[0],
            IngredientAlternative::Item(Identifier::parse("minecraft:oak_planks").unwrap())
        );
        assert_eq!(stick.result.item.as_str(), "minecraft:stick");

        let smelting_recipe = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == "minecraft:ignored/test_smelting")
            .unwrap();
        let RecipeKind::Smelting(smelting) = &smelting_recipe.kind else {
            panic!("expected smelting recipe");
        };
        assert_eq!(smelting.cooking_time, 200);
        assert_eq!(
            smelting.ingredient.alternatives[0],
            IngredientAlternative::Item(Identifier::parse("minecraft:iron_ore").unwrap())
        );
        assert_eq!(smelting_recipe.result.item.as_str(), "minecraft:iron_ingot");

        assert!(recipes.iter().any(|recipe| matches!(
            (&recipe.kind, recipe.id.as_str()),
            (RecipeKind::Blasting(smelting), "minecraft:ignored/test_blasting")
                if smelting.cooking_time == 100
        )));
        assert!(recipes.iter().any(|recipe| matches!(
            (&recipe.kind, recipe.id.as_str()),
            (RecipeKind::Smoking(smelting), "minecraft:ignored/test_smoking")
                if smelting.cooking_time == 100
        )));
        assert!(recipes.iter().any(|recipe| matches!(
            (&recipe.kind, recipe.id.as_str()),
            (RecipeKind::CampfireCooking(smelting), "minecraft:ignored/test_campfire")
                if smelting.cooking_time == 600
        )));
    }

    #[test]
    fn loads_simple_stonecutting_recipe_and_skips_component_results() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("building_blocks/test_slab.json"),
            r#"{
              "type": "minecraft:stonecutting",
              "ingredient": "minecraft:cobblestone",
              "result": { "id": "minecraft:cobblestone_slab", "count": 2 }
            }"#,
        );
        write(
            &root.join("building_blocks/unsupported_components.json"),
            r#"{
              "type": "minecraft:stonecutting",
              "ingredient": "minecraft:stone",
              "result": {
                "id": "minecraft:stone_slab",
                "components": { "minecraft:custom_name": "unsupported" }
              }
            }"#,
        );

        let recipes = load_recipes(root).unwrap();

        assert_eq!(recipes.len(), 1);
        assert_eq!(
            recipes[0].id.as_str(),
            "minecraft:building_blocks/test_slab"
        );
        assert_eq!(
            recipes[0].kind,
            RecipeKind::Stonecutting(StonecuttingRecipe {
                ingredient: Ingredient {
                    alternatives: vec![IngredientAlternative::Item(
                        Identifier::parse("minecraft:cobblestone").unwrap(),
                    )],
                },
            })
        );
        assert_eq!(
            recipes[0].result,
            RecipeResult {
                item: Identifier::parse("minecraft:cobblestone_slab").unwrap(),
                count: 2,
            }
        );
    }

    #[test]
    fn loads_canonical_cookingtime_and_experience() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("raw_iron_blasting.json"),
            r#"{
              "type": "minecraft:blasting",
              "ingredient": "minecraft:raw_iron",
              "result": { "id": "minecraft:iron_ingot" },
              "cookingtime": 100,
              "experience": 0.7
            }"#,
        );

        let recipes = load_recipes(root).unwrap();
        let RecipeKind::Blasting(recipe) = &recipes[0].kind else {
            panic!("expected blasting recipe");
        };
        assert_eq!(recipe.cooking_time, 100);
        assert_eq!(recipe.experience_milli, 700);
    }

    #[test]
    fn parses_tag_ingredients() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("oak_planks.json"),
            r##"{
              "type": "minecraft:crafting_shapeless",
              "ingredients": ["#minecraft:oak_logs"],
              "result": { "id": "minecraft:oak_planks", "count": 4 }
            }"##,
        );

        let recipes = load_recipes(root).unwrap();
        let RecipeKind::Shapeless(shapeless) = &recipes[0].kind else {
            panic!("expected shapeless recipe");
        };
        assert_eq!(
            shapeless.ingredients[0].alternatives[0],
            IngredientAlternative::Tag(Identifier::parse("minecraft:oak_logs").unwrap())
        );
        assert_eq!(recipes[0].result.item.as_str(), "minecraft:oak_planks");
    }

    #[test]
    fn missing_recipe_directory_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();

        let recipes = load_recipes(tmp.path().join("missing")).unwrap();

        assert!(recipes.is_empty());
    }

    #[test]
    fn embedded_required_recipes_cover_basic_wood_and_stone_tools() {
        let recipes = solaris_required_recipes();
        for id in [
            "minecraft:wooden_pickaxe",
            "minecraft:wooden_axe",
            "minecraft:wooden_shovel",
            "minecraft:wooden_sword",
            "minecraft:wooden_hoe",
            "minecraft:stone_pickaxe",
            "minecraft:stone_axe",
            "minecraft:stone_shovel",
            "minecraft:stone_sword",
            "minecraft:stone_hoe",
        ] {
            assert!(
                recipes.iter().any(|recipe| recipe.id.as_str() == id
                    && recipe.result.item.as_str() == id
                    && recipe.result.count == 1),
                "missing fallback recipe {id}"
            );
        }
        assert_eq!(
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == "minecraft:wooden_hoe"),
            Some(30)
        );
    }

    #[test]
    fn embedded_required_recipes_turn_one_bone_into_three_bone_meal() {
        let recipes = solaris_required_recipes();
        let recipe = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == "minecraft:bone_meal")
            .expect("bone meal fallback recipe");

        assert_eq!(recipe.result.item.as_str(), "minecraft:bone_meal");
        assert_eq!(recipe.result.count, 3);
        let RecipeKind::Shapeless(shapeless) = &recipe.kind else {
            panic!("bone meal fallback recipe must be shapeless");
        };
        assert_eq!(shapeless.ingredients.len(), 1);
        assert_eq!(
            shapeless.ingredients[0].alternatives,
            vec![IngredientAlternative::Item(
                Identifier::parse("minecraft:bone").unwrap()
            )]
        );
    }

    #[test]
    fn embedded_required_recipes_cover_charcoal_from_logs() {
        let recipes = solaris_required_recipes();
        let recipe = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == "minecraft:charcoal")
            .expect("charcoal fallback recipe");
        assert_eq!(recipe.result.item.as_str(), "minecraft:charcoal");
        assert_eq!(recipe.result.count, 1);

        let RecipeKind::Smelting(smelting) = &recipe.kind else {
            panic!("charcoal fallback recipe must be a furnace smelting recipe");
        };
        assert_eq!(smelting.cooking_time, 200);
        assert_eq!(smelting.experience_milli, 150);
        assert!(
            smelting.ingredient.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:logs_that_burn")
            }),
            "charcoal fallback must accept the generated natural log family through minecraft:logs_that_burn"
        );
    }

    #[test]
    fn embedded_required_recipes_cover_playable_cooked_passive_food() {
        let recipes = solaris_required_recipes();
        for (raw_item, cooked_item) in [
            ("minecraft:beef", "minecraft:cooked_beef"),
            ("minecraft:porkchop", "minecraft:cooked_porkchop"),
            ("minecraft:chicken", "minecraft:cooked_chicken"),
        ] {
            let recipe = recipes
                .iter()
                .find(|recipe| recipe.id.as_str() == cooked_item)
                .unwrap_or_else(|| {
                    panic!("missing cooked passive food fallback recipe {cooked_item}")
                });
            assert_eq!(recipe.result.item.as_str(), cooked_item);
            assert_eq!(recipe.result.count, 1);

            let RecipeKind::Smelting(smelting) = &recipe.kind else {
                panic!("cooked passive food fallback recipe must be a furnace smelting recipe");
            };
            assert_eq!(smelting.cooking_time, 200);
            assert_eq!(smelting.experience_milli, 350);
            assert!(
                smelting.ingredient.alternatives.iter().any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == raw_item)
                }),
                "cooked passive food fallback recipe {cooked_item} must accept {raw_item}"
            );
        }
    }

    #[test]
    fn embedded_required_recipes_cover_playable_chest_from_planks() {
        let recipes = solaris_required_recipes();
        let recipe = recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == "minecraft:chest")
            .expect("chest fallback recipe");
        assert_eq!(recipe.result.item.as_str(), "minecraft:chest");
        assert_eq!(recipe.result.count, 1);

        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("chest fallback recipe must be a shaped crafting recipe");
        };
        assert_eq!(shaped.pattern, ["###", "# #", "###"]);
        let planks = shaped.key.get(&'#').expect("chest planks ingredient");
        assert!(
            planks.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:planks")
            }),
            "chest fallback must accept earned generated planks through minecraft:planks"
        );
    }

    #[test]
    fn embedded_required_recipes_cover_playable_white_bed_without_shifting_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let display_id = |id: &str| {
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
        };

        assert_eq!(display_id("minecraft:chest"), 5);
        assert_eq!(display_id("minecraft:crafting_table"), 10);
        assert_eq!(display_id("minecraft:furnace"), 13);
        assert_eq!(display_id("minecraft:torch"), 27);
        assert_eq!(display_id("minecraft:wooden_pickaxe"), 31);

        let white_bed_display_id = recipes
            .iter()
            .position(|recipe| recipe.result.item.as_str() == "minecraft:white_bed")
            .expect("white bed fallback recipe");
        assert_eq!(white_bed_display_id, 34);
        let recipe = &recipes[white_bed_display_id];
        assert_eq!(recipe.result.count, 1);

        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("white bed fallback recipe must be a shaped crafting recipe");
        };
        assert_eq!(shaped.pattern, ["###", "PPP"]);
        let wool = shaped.key.get(&'#').expect("bed wool ingredient");
        assert!(
            wool.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:white_wool")
            }),
            "playable white bed recipe must use naturally earned white wool"
        );
        let planks = shaped.key.get(&'P').expect("bed planks ingredient");
        assert!(
            planks.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:planks")
            }),
            "playable white bed recipe must accept generated planks through minecraft:planks"
        );
    }

    #[test]
    fn embedded_required_recipes_cover_playable_wooden_doors_without_shifting_existing_display_ids()
    {
        let recipes = solaris_required_recipes();
        let display_id = |id: &str| {
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
        };

        assert_eq!(display_id("minecraft:chest"), 5);
        assert_eq!(display_id("minecraft:crafting_table"), 10);
        assert_eq!(display_id("minecraft:furnace"), 13);
        assert_eq!(display_id("minecraft:torch"), 27);
        assert_eq!(display_id("minecraft:wooden_pickaxe"), 31);
        assert_eq!(display_id("minecraft:zz_playable_white_bed"), 34);

        for (wood, expected_display_id) in [
            ("acacia", 35),
            ("birch", 36),
            ("cherry", 37),
            ("dark_oak", 38),
            ("jungle", 39),
            ("mangrove", 40),
            ("oak", 41),
            ("pale_oak", 42),
            ("spruce", 43),
        ] {
            let recipe_id = format!("minecraft:zz_playable_wooden_{wood}_door");
            let recipe = &recipes[display_id(&recipe_id)];
            assert_eq!(display_id(&recipe_id), expected_display_id);
            assert_eq!(
                recipe.result.item.as_str(),
                format!("minecraft:{wood}_door")
            );
            assert_eq!(recipe.result.count, 3);

            let RecipeKind::Shaped(shaped) = &recipe.kind else {
                panic!("{recipe_id} fallback recipe must be a shaped crafting recipe");
            };
            assert_eq!(shaped.pattern, ["##", "##", "##"]);
            let planks = shaped.key.get(&'#').expect("door planks ingredient");
            assert!(
                planks.alternatives.iter().any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == format!("minecraft:{wood}_planks"))
                }),
                "playable door recipe {recipe_id} must use its matching generated planks item"
            );
        }
    }

    #[test]
    fn embedded_required_recipes_cover_playable_wooden_signs_without_shifting_existing_display_ids()
    {
        let recipes = solaris_required_recipes();
        let display_id = |id: &str| {
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
        };

        assert_eq!(display_id("minecraft:chest"), 5);
        assert_eq!(display_id("minecraft:crafting_table"), 10);
        assert_eq!(display_id("minecraft:furnace"), 13);
        assert_eq!(display_id("minecraft:torch"), 27);
        assert_eq!(display_id("minecraft:wooden_pickaxe"), 31);
        assert_eq!(display_id("minecraft:zz_playable_white_bed"), 34);
        assert_eq!(display_id("minecraft:zz_playable_wooden_spruce_door"), 43);

        for (wood, expected_display_id) in [
            ("acacia", 44),
            ("birch", 45),
            ("cherry", 46),
            ("dark_oak", 47),
            ("jungle", 48),
            ("mangrove", 49),
            ("oak", 50),
            ("pale_oak", 51),
            ("spruce", 52),
        ] {
            let recipe_id = format!("minecraft:zz_playable_wooden_zsign_{wood}");
            let recipe = &recipes[display_id(&recipe_id)];
            assert_eq!(display_id(&recipe_id), expected_display_id);
            assert_eq!(
                recipe.result.item.as_str(),
                format!("minecraft:{wood}_sign")
            );
            assert_eq!(recipe.result.count, 3);

            let RecipeKind::Shaped(shaped) = &recipe.kind else {
                panic!("{recipe_id} fallback recipe must be a shaped crafting recipe");
            };
            assert_eq!(shaped.pattern, ["###", "###", " X "]);
            let planks = shaped.key.get(&'#').expect("sign planks ingredient");
            assert!(
                planks.alternatives.iter().any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == format!("minecraft:{wood}_planks"))
                }),
                "playable sign recipe {recipe_id} must use its matching generated planks item"
            );
            let stick = shaped.key.get(&'X').expect("sign stick ingredient");
            assert!(
                stick.alternatives.iter().any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
                }),
                "playable sign recipe {recipe_id} must use an earned stick"
            );
        }
    }

    #[test]
    fn embedded_required_recipes_cover_playable_campfire_without_shifting_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let display_id = |id: &str| {
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
        };

        assert_eq!(display_id("minecraft:chest"), 5);
        assert_eq!(display_id("minecraft:crafting_table"), 10);
        assert_eq!(display_id("minecraft:furnace"), 13);
        assert_eq!(display_id("minecraft:torch"), 27);
        assert_eq!(display_id("minecraft:wooden_pickaxe"), 31);
        assert_eq!(display_id("minecraft:zz_playable_wooden_zsign_spruce"), 52);

        let campfire = &recipes[display_id("minecraft:zz_playable_zz_campfire")];
        assert_eq!(display_id("minecraft:zz_playable_zz_campfire"), 53);
        assert_eq!(campfire.result.item.as_str(), "minecraft:campfire");
        assert_eq!(campfire.result.count, 1);
        let RecipeKind::Shaped(shaped) = &campfire.kind else {
            panic!("campfire fallback recipe must be a shaped crafting recipe");
        };
        assert_eq!(shaped.pattern, [" S ", "SCS", "LLL"]);
        let stick = shaped.key.get(&'S').expect("campfire stick ingredient");
        assert!(
            stick.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
            }),
            "playable campfire recipe must use earned sticks"
        );
        let charcoal = shaped.key.get(&'C').expect("campfire charcoal ingredient");
        assert!(
            charcoal.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:charcoal")
            }),
            "playable campfire recipe must use earned charcoal"
        );
        let logs = shaped.key.get(&'L').expect("campfire log ingredient");
        assert!(
            logs.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:logs_that_burn")
            }),
            "playable campfire recipe must accept generated logs through minecraft:logs_that_burn"
        );

        for (raw_item, cooked_item, recipe_id, expected_display_id) in [
            (
                "minecraft:beef",
                "minecraft:cooked_beef",
                "minecraft:zz_playable_zz_campfire_cooked_beef",
                54,
            ),
            (
                "minecraft:chicken",
                "minecraft:cooked_chicken",
                "minecraft:zz_playable_zz_campfire_cooked_chicken",
                55,
            ),
            (
                "minecraft:porkchop",
                "minecraft:cooked_porkchop",
                "minecraft:zz_playable_zz_campfire_cooked_porkchop",
                56,
            ),
        ] {
            let recipe = &recipes[display_id(recipe_id)];
            assert_eq!(display_id(recipe_id), expected_display_id);
            assert_eq!(recipe.result.item.as_str(), cooked_item);
            assert_eq!(recipe.result.count, 1);
            let RecipeKind::CampfireCooking(cooking) = &recipe.kind else {
                panic!("{recipe_id} fallback recipe must be campfire cooking");
            };
            assert_eq!(cooking.cooking_time, 600);
            assert!(
                cooking.ingredient.alternatives.iter().any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == raw_item)
                }),
                "playable campfire cooking recipe {recipe_id} must accept {raw_item}"
            );
        }
    }

    #[test]
    fn embedded_required_recipes_cover_playable_iron_sword_without_shifting_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let display_id = |id: &str| {
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
        };

        assert_eq!(display_id("minecraft:chest"), 5);
        assert_eq!(display_id("minecraft:crafting_table"), 10);
        assert_eq!(display_id("minecraft:furnace"), 13);
        assert_eq!(display_id("minecraft:torch"), 27);
        assert_eq!(display_id("minecraft:wooden_pickaxe"), 31);
        assert_eq!(
            display_id("minecraft:zz_playable_zz_campfire_cooked_porkchop"),
            56
        );

        let recipe_id = "minecraft:zz_playable_zz_iron_sword";
        let recipe = &recipes[display_id(recipe_id)];
        assert_eq!(display_id(recipe_id), 57);
        assert_eq!(recipe.result.item.as_str(), "minecraft:iron_sword");
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("playable iron sword fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, ["#", "#", "X"]);
        let iron = shaped.key.get(&'#').expect("iron sword ingot ingredient");
        assert!(
            iron.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
            }),
            "playable iron sword recipe must use earned iron ingots"
        );
        let stick = shaped.key.get(&'X').expect("iron sword stick ingredient");
        assert!(
            stick.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
            }),
            "playable iron sword recipe must use earned sticks"
        );

        let shield_recipe_id = "minecraft:zz_playable_zz_shield";
        let shield = &recipes[display_id(shield_recipe_id)];
        assert_eq!(display_id(shield_recipe_id), 58);
        assert_eq!(shield.result.item.as_str(), "minecraft:shield");
        assert_eq!(shield.result.count, 1);
        let RecipeKind::Shaped(shaped) = &shield.kind else {
            panic!("playable shield fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, ["PIP", "PPP", " P "]);
        let planks = shaped.key.get(&'P').expect("shield planks ingredient");
        assert!(
            planks.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Tag(tag) if tag.as_str() == "minecraft:planks")
            }),
            "playable shield recipe must use earned planks"
        );
        let iron = shaped.key.get(&'I').expect("shield iron ingredient");
        assert!(
            iron.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
            }),
            "playable shield recipe must use an earned iron ingot"
        );

        let chestplate_recipe_id = "minecraft:zz_playable_zzz_iron_chestplate";
        let chestplate = &recipes[display_id(chestplate_recipe_id)];
        assert_eq!(display_id(chestplate_recipe_id), 59);
        assert_eq!(chestplate.result.item.as_str(), "minecraft:iron_chestplate");
        assert_eq!(chestplate.result.count, 1);
        let RecipeKind::Shaped(shaped) = &chestplate.kind else {
            panic!("playable iron chestplate fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, ["# #", "###", "###"]);
        let iron = shaped
            .key
            .get(&'#')
            .expect("iron chestplate ingot ingredient");
        assert!(
            iron.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
            }),
            "playable iron chestplate recipe must use earned iron ingots"
        );
    }

    #[test]
    fn embedded_required_recipes_cover_playable_bread_without_shifting_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let display_id = |id: &str| {
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
        };

        assert_eq!(display_id("minecraft:zz_playable_zzz_iron_chestplate"), 59);

        let recipe_id = "minecraft:zz_playable_zzzz_bread";
        let recipe = &recipes[display_id(recipe_id)];
        assert_eq!(display_id(recipe_id), 60);
        assert_eq!(recipe.result.item.as_str(), "minecraft:bread");
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("playable bread fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, ["###"]);
        let wheat = shaped.key.get(&'#').expect("bread wheat ingredient");
        assert!(
            wheat.alternatives.iter().any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:wheat")
            }),
            "playable bread recipe must use harvested wheat"
        );
    }

    #[test]
    fn embedded_required_recipes_cover_playable_iron_pickaxe_after_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let display_id = |id: &str| {
            recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"))
        };

        assert_eq!(display_id("minecraft:zz_playable_zzzz_bread"), 60);

        let recipe_id = "minecraft:zz_playable_zzzzz_iron_pickaxe";
        let recipe = &recipes[display_id(recipe_id)];
        assert_eq!(display_id(recipe_id), 61);
        assert_eq!(recipe.result.item.as_str(), "minecraft:iron_pickaxe");
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("playable iron pickaxe fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, ["###", " X ", " X "]);
        let iron = shaped.key.get(&'#').expect("iron pickaxe ingot ingredient");
        assert!(iron.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
        }));
        let stick = shaped.key.get(&'X').expect("iron pickaxe stick ingredient");
        assert!(stick.alternatives.iter().any(|alternative| {
            matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
        }));
    }

    #[test]
    fn embedded_required_recipes_cover_core_diamond_tools_after_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let recipe = |id: &str, expected_display_id: usize, output: &str, pattern: &[&str]| {
            let display_id = recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"));
            assert_eq!(display_id, expected_display_id);
            let recipe = &recipes[display_id];
            assert_eq!(recipe.result.item.as_str(), output);
            assert_eq!(recipe.result.count, 1);
            let RecipeKind::Shaped(shaped) = &recipe.kind else {
                panic!("{id} fallback recipe must be shaped");
            };
            assert_eq!(shaped.pattern, pattern);
            assert!(shaped
                .key
                .get(&'#')
                .expect("diamond ingredient")
                .alternatives
                .iter()
                .any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:diamond")
                }));
            assert!(shaped
                .key
                .get(&'X')
                .expect("stick ingredient")
                .alternatives
                .iter()
                .any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
                }));
        };

        recipe(
            "minecraft:zz_playable_zzzzzz_diamond_pickaxe",
            62,
            "minecraft:diamond_pickaxe",
            &["###", " X ", " X "],
        );
        recipe(
            "minecraft:zz_playable_zzzzzzz_diamond_sword",
            63,
            "minecraft:diamond_sword",
            &["#", "#", "X"],
        );
    }

    #[test]
    fn embedded_required_recipes_cover_playable_bucket_after_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let recipe_id = "minecraft:zz_playable_zzzzzzzz_bucket";
        let display_id = recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == recipe_id)
            .unwrap_or_else(|| panic!("missing fallback recipe {recipe_id}"));

        assert_eq!(display_id, 64);
        let recipe = &recipes[display_id];
        assert_eq!(recipe.result.item.as_str(), "minecraft:bucket");
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("playable bucket fallback recipe must be shaped");
        };
        assert_eq!(shaped.pattern, ["# #", " # "]);
        assert!(shaped
            .key
            .get(&'#')
            .expect("bucket iron ingredient")
            .alternatives
            .iter()
            .any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
            }));
    }

    #[test]
    fn embedded_required_recipes_complete_playable_iron_tier_after_bucket() {
        let recipes = solaris_required_recipes();
        assert_eq!(
            recipes.iter().position(|recipe| {
                recipe.id.as_str() == "minecraft:zz_playable_zzzzzzzz_bucket"
            }),
            Some(64)
        );

        for (id, display_id, output, pattern, uses_stick) in [
            (
                "minecraft:zz_playable_zzzzzzzzz_iron_axe",
                65,
                "minecraft:iron_axe",
                &["XX", "X#", " #"][..],
                true,
            ),
            (
                "minecraft:zz_playable_zzzzzzzzzz_iron_shovel",
                66,
                "minecraft:iron_shovel",
                &["X", "#", "#"][..],
                true,
            ),
            (
                "minecraft:zz_playable_zzzzzzzzzzz_iron_hoe",
                67,
                "minecraft:iron_hoe",
                &["XX", " #", " #"][..],
                true,
            ),
            (
                "minecraft:zz_playable_zzzzzzzzzzzz_iron_helmet",
                68,
                "minecraft:iron_helmet",
                &["XXX", "X X"][..],
                false,
            ),
            (
                "minecraft:zz_playable_zzzzzzzzzzzzz_iron_leggings",
                69,
                "minecraft:iron_leggings",
                &["XXX", "X X", "X X"][..],
                false,
            ),
            (
                "minecraft:zz_playable_zzzzzzzzzzzzzz_iron_boots",
                70,
                "minecraft:iron_boots",
                &["X X", "X X"][..],
                false,
            ),
        ] {
            let actual_display_id = recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"));
            assert_eq!(actual_display_id, display_id);
            let recipe = &recipes[actual_display_id];
            assert_eq!(recipe.result.item.as_str(), output);
            assert_eq!(recipe.result.count, 1);
            let RecipeKind::Shaped(shaped) = &recipe.kind else {
                panic!("{id} fallback recipe must be shaped");
            };
            assert_eq!(shaped.pattern, pattern);
            assert!(shaped
                .key
                .get(&'X')
                .expect("iron ingredient")
                .alternatives
                .iter()
                .any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
                }));
            assert_eq!(shaped.key.contains_key(&'#'), uses_stick);
            if uses_stick {
                assert!(shaped
                    .key
                    .get(&'#')
                    .expect("stick ingredient")
                    .alternatives
                    .iter()
                    .any(|alternative| {
                        matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
                    }));
            }
        }
    }

    #[test]
    fn embedded_required_recipes_complete_playable_diamond_tier_after_iron() {
        let recipes = solaris_required_recipes();
        assert_eq!(
            recipes.iter().position(|recipe| {
                recipe.id.as_str() == "minecraft:zz_playable_zzzzzzzzzzzzzz_iron_boots"
            }),
            Some(70)
        );

        for (id, display_id, output, pattern, uses_stick) in [
            (
                "minecraft:zzz_playable_diamond_axe",
                71,
                "minecraft:diamond_axe",
                &["XX", "X#", " #"][..],
                true,
            ),
            (
                "minecraft:zzz_playable_diamond_boots",
                72,
                "minecraft:diamond_boots",
                &["X X", "X X"][..],
                false,
            ),
            (
                "minecraft:zzz_playable_diamond_chestplate",
                73,
                "minecraft:diamond_chestplate",
                &["X X", "XXX", "XXX"][..],
                false,
            ),
            (
                "minecraft:zzz_playable_diamond_helmet",
                74,
                "minecraft:diamond_helmet",
                &["XXX", "X X"][..],
                false,
            ),
            (
                "minecraft:zzz_playable_diamond_hoe",
                75,
                "minecraft:diamond_hoe",
                &["XX", " #", " #"][..],
                true,
            ),
            (
                "minecraft:zzz_playable_diamond_leggings",
                76,
                "minecraft:diamond_leggings",
                &["XXX", "X X", "X X"][..],
                false,
            ),
            (
                "minecraft:zzz_playable_diamond_shovel",
                77,
                "minecraft:diamond_shovel",
                &["X", "#", "#"][..],
                true,
            ),
        ] {
            let actual_display_id = recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing fallback recipe {id}"));
            assert_eq!(actual_display_id, display_id);
            let recipe = &recipes[actual_display_id];
            assert_eq!(recipe.result.item.as_str(), output);
            assert_eq!(recipe.result.count, 1);
            let RecipeKind::Shaped(shaped) = &recipe.kind else {
                panic!("{id} fallback recipe must be shaped");
            };
            assert_eq!(shaped.pattern, pattern);
            assert!(shaped
                .key
                .get(&'X')
                .expect("diamond ingredient")
                .alternatives
                .iter()
                .any(|alternative| {
                    matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:diamond")
                }));
            assert_eq!(shaped.key.contains_key(&'#'), uses_stick);
            if uses_stick {
                assert!(shaped
                    .key
                    .get(&'#')
                    .expect("stick ingredient")
                    .alternatives
                    .iter()
                    .any(|alternative| {
                        matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
                    }));
            }
        }
    }

    #[test]
    fn embedded_required_recipes_turn_spider_string_into_a_bow() {
        let recipes = solaris_required_recipes();
        let display_id = recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == "minecraft:zzzz_playable_bow")
            .expect("playable bow recipe");
        assert_eq!(display_id, 78);
        let recipe = &recipes[display_id];
        assert_eq!(recipe.result.item.as_str(), "minecraft:bow");
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("playable bow recipe must be shaped");
        };
        assert_eq!(shaped.pattern, [" #X", "# X", " #X"]);
        assert!(shaped.key.get(&'#').unwrap().alternatives.iter().any(
            |alternative| matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:stick")
        ));
        assert!(shaped.key.get(&'X').unwrap().alternatives.iter().any(
            |alternative| matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:string")
        ));
    }

    #[test]
    fn embedded_required_recipes_add_shears_after_existing_display_ids() {
        let recipes = solaris_required_recipes();
        let display_id = recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == "minecraft:zzzzz_playable_shears")
            .expect("playable shears recipe");
        assert_eq!(display_id, 79);
        let recipe = &recipes[display_id];
        assert_eq!(recipe.result.item.as_str(), "minecraft:shears");
        assert_eq!(recipe.result.count, 1);
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            panic!("playable shears recipe must be shaped");
        };
        assert_eq!(shaped.pattern, [" #", "# "]);
        assert!(shaped
            .key
            .get(&'#')
            .expect("shears iron ingredient")
            .alternatives
            .iter()
            .any(|alternative| {
                matches!(alternative, IngredientAlternative::Item(item) if item.as_str() == "minecraft:iron_ingot")
            }));
    }

    #[test]
    fn rejects_missing_shaped_key() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(
            &root.join("bad.json"),
            r###"{
              "type": "minecraft:crafting_shaped",
              "pattern": ["#X"],
              "key": { "#": "minecraft:stone" },
              "result": { "id": "minecraft:stone" }
            }"###,
        );

        let err = load_recipes(root).unwrap_err();

        assert!(matches!(err, RecipeDataError::MissingKey { key: 'X', .. }));
    }

    #[test]
    #[ignore = "requires local 26.1.2 recipe sidecars"]
    fn loads_real_recipe_sidecar_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("data/vanilla/data/minecraft/recipe");
        assert!(
            root.is_dir(),
            "{} missing; run tools/extract-vanilla-data.sh",
            root.display()
        );

        let recipes = load_recipes(root).unwrap();
        assert!(recipes.len() > 100, "vanilla has many crafting recipes");
        assert!(recipes.iter().any(|recipe| {
            recipe.id.as_str() == "minecraft:stick"
                && recipe.result.item.as_str() == "minecraft:stick"
        }));
    }
}
