//! Inventory of local vanilla worldgen sidecar data.
//!
//! This module records which declarative data files are present under the
//! ADR 0001 sidecar. It intentionally does not interpret vanilla worldgen
//! algorithms; concrete loaders decide which facts Solaris can consume.

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorldgenInventoryError {
    #[error("vanilla data sidecar directory not found at {0}")]
    MissingRoot(PathBuf),
    #[error("worldgen inventory io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldgenSidecarInventory {
    root: PathBuf,
    areas: Vec<WorldgenInventoryArea>,
}

impl WorldgenSidecarInventory {
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn areas(&self) -> &[WorldgenInventoryArea] {
        &self.areas
    }

    #[must_use]
    pub fn area(&self, name: &str) -> Option<&WorldgenInventoryArea> {
        self.areas.iter().find(|area| area.name == name)
    }

    #[must_use]
    pub fn total_files(&self) -> usize {
        self.areas.iter().map(|area| area.file_count).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldgenInventoryArea {
    pub name: &'static str,
    pub relative_path: &'static str,
    pub extension: &'static str,
    pub present: bool,
    pub file_count: usize,
    pub sample_files: Vec<String>,
}

struct AreaSpec {
    name: &'static str,
    relative_path: &'static str,
    extension: &'static str,
    recursive: bool,
}

const AREA_SPECS: &[AreaSpec] = &[
    AreaSpec {
        name: "biome_json",
        relative_path: "data/minecraft/worldgen/biome",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "biome_tags",
        relative_path: "data/minecraft/tags/worldgen/biome",
        extension: "json",
        recursive: true,
    },
    AreaSpec {
        name: "configured_features",
        relative_path: "data/minecraft/worldgen/configured_feature",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "placed_features",
        relative_path: "data/minecraft/worldgen/placed_feature",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "noise_settings",
        relative_path: "data/minecraft/worldgen/noise_settings",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "multi_noise_biome_source_parameter_lists",
        relative_path: "data/minecraft/worldgen/multi_noise_biome_source_parameter_list",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "structures",
        relative_path: "data/minecraft/worldgen/structure",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "structure_sets",
        relative_path: "data/minecraft/worldgen/structure_set",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "template_pools",
        relative_path: "data/minecraft/worldgen/template_pool",
        extension: "json",
        recursive: false,
    },
    AreaSpec {
        name: "structure_templates",
        relative_path: "data/minecraft/structure",
        extension: "nbt",
        recursive: true,
    },
    AreaSpec {
        name: "reports",
        relative_path: "reports",
        extension: "json",
        recursive: true,
    },
];

pub fn load_worldgen_inventory(
    vanilla_root: impl AsRef<Path>,
) -> Result<WorldgenSidecarInventory, WorldgenInventoryError> {
    let root = vanilla_root.as_ref();
    if !root.is_dir() {
        return Err(WorldgenInventoryError::MissingRoot(root.to_path_buf()));
    }

    let mut areas = Vec::with_capacity(AREA_SPECS.len());
    for spec in AREA_SPECS {
        let path = root.join(spec.relative_path);
        let mut files = Vec::new();
        let present = path.is_dir();
        if present {
            collect_files(&path, spec.extension, spec.recursive, &mut files)?;
        }
        files.sort();
        let sample_files = files
            .iter()
            .take(8)
            .map(|path| relative_slash_path(root, path))
            .collect();
        areas.push(WorldgenInventoryArea {
            name: spec.name,
            relative_path: spec.relative_path,
            extension: spec.extension,
            present,
            file_count: files.len(),
            sample_files,
        });
    }

    Ok(WorldgenSidecarInventory {
        root: root.to_path_buf(),
        areas,
    })
}

fn collect_files(
    dir: &Path,
    extension: &str,
    recursive: bool,
    out: &mut Vec<PathBuf>,
) -> Result<(), WorldgenInventoryError> {
    for entry in std::fs::read_dir(dir).map_err(|source| WorldgenInventoryError::Io {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| WorldgenInventoryError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let ty = entry
            .file_type()
            .map_err(|source| WorldgenInventoryError::Io {
                path: path.clone(),
                source,
            })?;
        if ty.is_dir() && recursive {
            collect_files(&path, extension, recursive, out)?;
        } else if ty.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
            out.push(path);
        }
    }
    Ok(())
}

fn relative_slash_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"{}").unwrap();
    }

    #[test]
    fn inventories_synthetic_worldgen_sidecar_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("data/minecraft/worldgen/biome/plains.json"));
        write(&root.join("data/minecraft/tags/worldgen/biome/is_overworld.json"));
        write(&root.join("data/minecraft/worldgen/placed_feature/patch_grass.json"));
        write(&root.join("data/minecraft/structure/village/plains/fountain.nbt"));
        write(&root.join("reports/registries.json"));

        let inventory = load_worldgen_inventory(root).unwrap();

        assert_eq!(inventory.root(), root);
        assert_eq!(inventory.total_files(), 5);
        let biomes = inventory.area("biome_json").unwrap();
        assert!(biomes.present);
        assert_eq!(biomes.file_count, 1);
        assert_eq!(
            biomes.sample_files,
            vec![String::from("data/minecraft/worldgen/biome/plains.json")]
        );
        let templates = inventory.area("structure_templates").unwrap();
        assert_eq!(templates.file_count, 1);
        assert_eq!(templates.extension, "nbt");
        assert!(!inventory.area("noise_settings").unwrap().present);
    }

    #[test]
    fn inventories_real_worldgen_sidecar_when_present() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("data/vanilla");
        if !root.join("data/minecraft/worldgen").is_dir() {
            eprintln!(
                "skipping: worldgen sidecar missing under {}",
                root.display()
            );
            return;
        }

        let inventory = load_worldgen_inventory(root).unwrap();

        assert!(inventory.area("biome_json").unwrap().file_count >= 60);
        assert!(inventory.area("configured_features").unwrap().file_count >= 150);
        assert!(inventory.area("placed_features").unwrap().file_count >= 200);
        assert!(inventory.area("structure_sets").unwrap().file_count >= 15);
        assert!(inventory.area("structure_templates").unwrap().file_count >= 1);
        assert!(inventory.area("reports").unwrap().file_count >= 3);
    }
}
