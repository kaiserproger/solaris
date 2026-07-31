//! Filesystem-safe projection of logical resource identifiers.
//!
//! [`Identifier`] intentionally models Minecraft's logical resource syntax. A
//! logical identifier is not itself authority to traverse the host filesystem:
//! callers must first build a [`ResourcePath`], which rejects ambiguous path
//! segments and opens existing targets only when the opened file identity stays
//! below the trusted root.

use std::fs::File;
use std::path::{Component, Path, PathBuf};

use same_file::Handle;
use thiserror::Error;

use crate::Identifier;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourcePath {
    relative: PathBuf,
}

#[derive(Debug)]
pub struct OpenedResource {
    path: PathBuf,
    file: File,
}

impl OpenedResource {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn into_parts(self) -> (PathBuf, File) {
        (self.path, self.file)
    }
}

#[derive(Debug, Error)]
pub enum ResourcePathError {
    #[error("unsafe resource path {value:?}: {reason}")]
    Unsafe { value: String, reason: &'static str },
    #[error("failed to resolve resource target {path}: {source}")]
    TargetIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to resolve trusted resource root {path}: {source}")]
    RootIo {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("resource target {target} escapes trusted root {root}")]
    EscapesRoot { root: PathBuf, target: PathBuf },
    #[error("resource target changed while it was being opened at {path}")]
    ChangedDuringOpen { path: PathBuf },
}

impl ResourcePath {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_identifier_path(identifier: &Identifier) -> Result<Self, ResourcePathError> {
        let mut resource = Self::new();
        resource.push_identifier_path(identifier)?;
        Ok(resource)
    }

    pub fn push_namespace(&mut self, identifier: &Identifier) -> Result<(), ResourcePathError> {
        self.push_component(identifier.namespace(), identifier.as_str())
    }

    pub fn push_identifier_path(
        &mut self,
        identifier: &Identifier,
    ) -> Result<(), ResourcePathError> {
        self.push_relative(identifier.path(), identifier.as_str())
    }

    pub fn push_static(&mut self, relative: &'static str) -> Result<(), ResourcePathError> {
        self.push_relative(relative, relative)
    }

    pub fn set_extension(&mut self, extension: &'static str) -> Result<(), ResourcePathError> {
        if extension.is_empty()
            || extension.contains(['/', '\\', ':'])
            || matches!(extension, "." | "..")
        {
            return Err(ResourcePathError::Unsafe {
                value: extension.to_string(),
                reason: "invalid file extension",
            });
        }
        if !self.relative.set_extension(extension) {
            return Err(ResourcePathError::Unsafe {
                value: self.relative.display().to_string(),
                reason: "resource path has no final component",
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn lexical_under(&self, root: &Path) -> PathBuf {
        root.join(&self.relative)
    }

    /// Open one existing resource below `root` and return the already-opened file.
    ///
    /// The trusted root is canonicalized before constructing the candidate. After
    /// opening, the candidate is canonicalized and compared with the opened file's
    /// OS identity. This closes the canonicalize-then-reopen symlink-swap race: a
    /// caller never reopens the validated pathname. Missing targets return
    /// `Ok(None)` so domain loaders can keep their own missing-resource errors.
    pub fn open_existing_under(
        &self,
        root: &Path,
    ) -> Result<Option<OpenedResource>, ResourcePathError> {
        let canonical_root =
            std::fs::canonicalize(root).map_err(|source| ResourcePathError::RootIo {
                path: root.to_path_buf(),
                source,
            })?;
        let candidate = self.lexical_under(&canonical_root);
        let file = match File::open(&candidate) {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ResourcePathError::TargetIo {
                    path: candidate,
                    source,
                });
            }
        };
        validate_opened_resource(file, candidate, canonical_root).map(Some)
    }

    fn push_relative(&mut self, value: &str, source: &str) -> Result<(), ResourcePathError> {
        if value.is_empty() {
            return Err(ResourcePathError::Unsafe {
                value: source.to_string(),
                reason: "empty relative path",
            });
        }
        if value.starts_with('/') || value.ends_with('/') {
            return Err(ResourcePathError::Unsafe {
                value: source.to_string(),
                reason: "leading or trailing separator",
            });
        }
        if value.contains('\\') || value.contains(':') {
            return Err(ResourcePathError::Unsafe {
                value: source.to_string(),
                reason: "platform-ambiguous separator or prefix",
            });
        }
        for component in value.split('/') {
            self.push_component(component, source)?;
        }
        Ok(())
    }

    fn push_component(&mut self, component: &str, source: &str) -> Result<(), ResourcePathError> {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(ResourcePathError::Unsafe {
                value: source.to_string(),
                reason: "empty or dot path segment",
            });
        }
        if component.contains(['/', '\\', ':']) {
            return Err(ResourcePathError::Unsafe {
                value: source.to_string(),
                reason: "component contains a separator or platform prefix",
            });
        }
        let mut components = Path::new(component).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(ResourcePathError::Unsafe {
                value: source.to_string(),
                reason: "component is not one normal relative path segment",
            });
        }
        self.relative.push(component);
        Ok(())
    }
}

fn validate_opened_resource(
    file: File,
    candidate: PathBuf,
    canonical_root: PathBuf,
) -> Result<OpenedResource, ResourcePathError> {
    let opened_handle =
        Handle::from_file(
            file.try_clone()
                .map_err(|source| ResourcePathError::TargetIo {
                    path: candidate.clone(),
                    source,
                })?,
        )
        .map_err(|source| ResourcePathError::TargetIo {
            path: candidate.clone(),
            source,
        })?;
    let target =
        std::fs::canonicalize(&candidate).map_err(|source| ResourcePathError::TargetIo {
            path: candidate.clone(),
            source,
        })?;
    if !target.starts_with(&canonical_root) {
        return Err(ResourcePathError::EscapesRoot {
            root: canonical_root,
            target,
        });
    }
    let current_handle =
        Handle::from_path(&target).map_err(|source| ResourcePathError::TargetIo {
            path: target.clone(),
            source,
        })?;
    if opened_handle != current_handle {
        return Err(ResourcePathError::ChangedDuringOpen { path: candidate });
    }
    Ok(OpenedResource { path: target, file })
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn accepts_nested_identifier_paths() {
        let identifier = Identifier::parse("minecraft:loot/entities/cow").unwrap();
        let resource = ResourcePath::from_identifier_path(&identifier).unwrap();
        assert_eq!(
            resource.lexical_under(Path::new("root")),
            Path::new("root/loot/entities/cow")
        );
    }

    #[test]
    fn rejects_traversal_absolute_and_empty_segments() {
        for raw in [
            "minecraft:../secret",
            "minecraft:loot/../../secret",
            "minecraft:/etc/passwd",
            "minecraft:loot//cow",
            "minecraft:loot/./cow",
            "minecraft:loot/cow/",
        ] {
            let identifier = Identifier::parse(raw).unwrap();
            assert!(
                ResourcePath::from_identifier_path(&identifier).is_err(),
                "accepted unsafe identifier {raw}"
            );
        }
    }

    #[test]
    fn opens_existing_nested_target_and_reports_missing() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir_all(root.path().join("loot/entities")).unwrap();
        std::fs::write(root.path().join("loot/entities/cow.json"), b"cow").unwrap();

        let mut existing = ResourcePath::from_identifier_path(
            &Identifier::parse("minecraft:loot/entities/cow").unwrap(),
        )
        .unwrap();
        existing.set_extension("json").unwrap();
        let opened = existing.open_existing_under(root.path()).unwrap().unwrap();
        assert!(
            opened
                .path()
                .starts_with(std::fs::canonicalize(root.path()).unwrap())
        );
        let (_, mut file) = opened.into_parts();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "cow");

        let missing = ResourcePath::from_identifier_path(
            &Identifier::parse("minecraft:loot/entities/pig").unwrap(),
        )
        .unwrap();
        assert!(missing.open_existing_under(root.path()).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.json"), b"{}").unwrap();
        symlink(outside.path(), root.path().join("linked")).unwrap();

        let mut resource = ResourcePath::from_identifier_path(
            &Identifier::parse("minecraft:linked/secret").unwrap(),
        )
        .unwrap();
        resource.set_extension("json").unwrap();
        assert!(matches!(
            resource.open_existing_under(root.path()),
            Err(ResourcePathError::EscapesRoot { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_target_swapped_between_open_and_validation() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let candidate = root.path().join("resource.json");
        let outside_file = outside.path().join("outside.json");
        std::fs::write(&outside_file, b"outside").unwrap();
        symlink(&outside_file, &candidate).unwrap();

        let opened_outside = File::open(&candidate).unwrap();
        std::fs::remove_file(&candidate).unwrap();
        std::fs::write(&candidate, b"inside").unwrap();

        let error = validate_opened_resource(
            opened_outside,
            candidate.clone(),
            std::fs::canonicalize(root.path()).unwrap(),
        )
        .expect_err("opened file identity must match the post-open target");
        assert!(matches!(error, ResourcePathError::ChangedDuringOpen { .. }));
    }
}
