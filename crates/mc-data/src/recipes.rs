//! Vanilla crafting recipe reader.
//!
//! This loader keeps the data sidecar boundary in `mc-data`: it parses
//! shaped and shapeless recipe JSON into small Solaris data types without
//! executing crafting or depending on Play inventory packets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::Identifier;

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

/// Load shaped and shapeless recipes from `<data>/minecraft/recipe`.
///
/// Unsupported recipe types are skipped because this foundation slice only
/// prepares normal crafting data. A missing directory is not an error: the
/// default sidecar extraction excludes recipe JSON, and startup should keep
/// working without crafting data.
pub fn load_recipes(recipe_dir: impl AsRef<Path>) -> Result<Vec<Recipe>, RecipeDataError> {
    let recipe_dir = recipe_dir.as_ref();
    if !recipe_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    collect_recipe_files(recipe_dir, &mut paths)?;
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
    let raw: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(REQUIRED_RECIPES).expect("embedded required recipe JSON is valid");
    raw.into_iter()
        .map(|(id, value)| {
            let id = Identifier::parse(id).expect("embedded required recipe id is valid");
            parse_recipe_value(path, id, value)
                .expect("embedded required recipe JSON uses supported recipe shapes")
                .expect("embedded required recipe JSON contains supported recipe types")
        })
        .collect()
}

fn collect_recipe_files(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), RecipeDataError> {
    let entries = std::fs::read_dir(dir).map_err(|source| RecipeDataError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| RecipeDataError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let ty = entry.file_type().map_err(|source| RecipeDataError::Io {
            path: path.clone(),
            source,
        })?;
        if ty.is_dir() {
            collect_recipe_files(&path, paths)?;
        } else if ty.is_file() && path.extension().is_some_and(|ext| ext == "json") {
            paths.push(path);
        }
    }
    Ok(())
}

fn load_one_recipe(root: &Path, path: &Path) -> Result<Option<Recipe>, RecipeDataError> {
    let id = id_from_file(root, path)?;
    let value: serde_json::Value = read_json(path)?;
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
        "minecraft:smelting" => {
            let raw: RawSmeltingRecipe = from_value(path, value)?;
            Ok(Some(parse_smelting_recipe(path, id, raw)?))
        }
        _ => Ok(None),
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
) -> Result<Recipe, RecipeDataError> {
    Ok(Recipe {
        id,
        kind: RecipeKind::Smelting(SmeltingRecipe {
            ingredient: parse_ingredient(path, raw.ingredient)?,
            cooking_time: raw.cooking_time,
        }),
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

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RecipeDataError> {
    let bytes = std::fs::read(path).map_err(|source| RecipeDataError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| RecipeDataError::Parse {
        path: path.to_path_buf(),
        source,
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
    #[serde(default = "default_cooking_time")]
    cooking_time: u32,
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

        let recipes = load_recipes(root).unwrap();

        assert_eq!(recipes.len(), 3);
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
    fn loads_real_recipe_sidecar_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("data/vanilla/data/minecraft/recipe");
        if !root.is_dir() {
            eprintln!("skipping: {} missing", root.display());
            return;
        }

        let recipes = load_recipes(root).unwrap();
        assert!(recipes.len() > 100, "vanilla has many crafting recipes");
        assert!(recipes.iter().any(|recipe| {
            recipe.id.as_str() == "minecraft:stick"
                && recipe.result.item.as_str() == "minecraft:stick"
        }));
    }
}
