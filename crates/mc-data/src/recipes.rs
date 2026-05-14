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
pub struct Ingredient {
    pub alternatives: Vec<Identifier>,
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
        RawIngredient::Single(value) => vec![parse_id(path, value)?],
        RawIngredient::Alternatives(values) => values
            .into_iter()
            .map(|value| parse_id(path, value))
            .collect::<Result<Vec<_>, _>>()?,
    };
    if alternatives.is_empty() {
        return Err(RecipeDataError::EmptyIngredient {
            path: path.to_path_buf(),
        });
    }
    Ok(Ingredient { alternatives })
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
#[serde(untagged)]
enum RawIngredient {
    Single(String),
    Alternatives(Vec<String>),
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

        assert_eq!(recipes.len(), 2);
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
                .map(Identifier::as_str)
                .collect::<Vec<_>>(),
            ["minecraft:oak_log", "minecraft:birch_log"]
        );

        assert_eq!(recipes[1].id.as_str(), "minecraft:misc/test_stick");
        let RecipeKind::Shapeless(shapeless) = &recipes[1].kind else {
            panic!("expected shapeless recipe");
        };
        assert_eq!(shapeless.ingredients.len(), 2);
        assert_eq!(
            shapeless.ingredients[0].alternatives[0].as_str(),
            "minecraft:test_planks"
        );
        assert_eq!(recipes[1].result.item.as_str(), "minecraft:stick");
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
