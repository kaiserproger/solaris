use super::*;

pub(super) const MAX_ACCESS_CONTROL_FILE_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_ACCESS_CONTROL_FILE_ENTRIES: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessControlLoadReport {
    pub files_loaded: usize,
    pub operator_identities: usize,
    pub whitelist_identities: usize,
    pub banned_identities: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorFileOperation {
    Add(String),
    Remove(String),
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorFileResult {
    pub changed: bool,
    pub identities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AccessControlProfileEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
}

impl ServerConfig {
    /// Merge optional file-backed access-control profiles into the inline TOML policy.
    ///
    /// Relative paths are resolved from the directory containing `config_path`.
    /// When `admin.operators_file` or `auth.whitelist_file` is omitted, an
    /// existing `ops.json` or `whitelist.json` beside the config is used, so
    /// entries written by the console survive a restart. File entries use
    /// vanilla-style JSON objects with `name` and/or `uuid`; extra fields such as
    /// operator level or ban metadata are ignored deliberately.
    pub fn load_access_control_files(
        &mut self,
        config_path: &Path,
    ) -> anyhow::Result<AccessControlLoadReport> {
        let mut report = AccessControlLoadReport::default();

        let operators_file = self.admin.operators_file.clone().or_else(|| {
            let default = Path::new("ops.json");
            resolve_config_relative_path(config_path, default)
                .is_file()
                .then(|| default.to_path_buf())
        });
        if let Some(path) = operators_file {
            let entries = load_access_control_file(config_path, &path, "admin.operators_file")?;
            report.files_loaded += 1;
            report.operator_identities = entries.len();
            self.admin.operators.extend(entries);
        }
        let whitelist_file = self.auth.whitelist_file.clone().or_else(|| {
            let default = Path::new("whitelist.json");
            resolve_config_relative_path(config_path, default)
                .is_file()
                .then(|| default.to_path_buf())
        });
        if let Some(path) = whitelist_file {
            let entries = load_access_control_file(config_path, &path, "auth.whitelist_file")?;
            report.files_loaded += 1;
            report.whitelist_identities = entries.len();
            self.auth.whitelist.extend(entries);
        }
        if let Some(path) = self.auth.banned_players_file.clone() {
            let entries = load_access_control_file(config_path, &path, "auth.banned_players_file")?;
            report.files_loaded += 1;
            report.banned_identities = entries.len();
            self.auth.banned_players.extend(entries);
        }

        Ok(report)
    }

    /// Add, remove, or list identities in the configured vanilla-style operator file.
    ///
    /// When `admin.operators_file` is absent, the management caller may supply
    /// the default `ops.json` path in memory; startup auto-loads that file when
    /// it exists beside the selected config.
    pub fn manage_operator_file(
        &self,
        config_path: &Path,
        operation: OperatorFileOperation,
    ) -> anyhow::Result<OperatorFileResult> {
        self.manage_access_file(config_path, AccessControlTarget::Operators, operation)
    }

    /// Add an identity to the configured whitelist file.
    ///
    /// The console supplies the default `whitelist.json` path in memory when
    /// `auth.whitelist_file` is unset; startup auto-loads that file when it
    /// exists beside the selected config.
    pub fn add_whitelist_identity(&self, config_path: &Path, identity: &str) -> anyhow::Result<()> {
        self.manage_access_file(
            config_path,
            AccessControlTarget::Whitelist,
            OperatorFileOperation::Add(identity.to_owned()),
        )?;
        Ok(())
    }

    /// Remove an identity from the configured whitelist file.
    pub fn remove_whitelist_identity(
        &self,
        config_path: &Path,
        identity: &str,
    ) -> anyhow::Result<()> {
        self.manage_access_file(
            config_path,
            AccessControlTarget::Whitelist,
            OperatorFileOperation::Remove(identity.to_owned()),
        )?;
        Ok(())
    }

    /// Add, remove, or list identities in one vanilla-style access-control file.
    ///
    /// Add/remove preserve unknown profile metadata and normalize identities
    /// while writing deterministic JSON. Removal revokes the complete profile
    /// and any overlapping aliases. Mutations use a persistent `.lock` sidecar
    /// and durable same-directory replacement. Management refuses symlink and
    /// multiply linked targets rather than silently changing which file an
    /// existing link updates.
    fn manage_access_file(
        &self,
        config_path: &Path,
        target: AccessControlTarget,
        operation: OperatorFileOperation,
    ) -> anyhow::Result<OperatorFileResult> {
        let configured_path = target.configured_path(self).ok_or_else(|| {
            anyhow::anyhow!(
                "{} management requires {} in {}",
                target.subject(),
                target.field(),
                config_path.display()
            )
        })?;
        let configured = resolve_config_relative_path(config_path, configured_path);
        let parent = configured
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .with_context(|| {
                format!(
                    "resolving {} file directory {}",
                    target.subject(),
                    configured.display()
                )
            })?;
        let path = parent.join(
            configured
                .file_name()
                .context("access-control file path must have a file name")?,
        );
        let requested_identity = match &operation {
            OperatorFileOperation::Add(raw) | OperatorFileOperation::Remove(raw) => {
                Some(normalize_access_identity(raw)?)
            }
            OperatorFileOperation::List => None,
        };
        // The sidecar survives replacement. Locking ops.json itself would protect
        // only the old inode and permit concurrent read-modify-write lost updates.
        let _lock = if matches!(&operation, OperatorFileOperation::List) {
            None
        } else {
            Some(lock_access_file(&path)?)
        };
        let metadata = access_file_metadata(&path)?;
        let values = if metadata.is_none() && matches!(&operation, OperatorFileOperation::Add(_)) {
            Vec::new()
        } else {
            read_access_control_values(config_path, &path, target.field())?.1
        };
        let (mut values, mut identities) =
            canonicalize_operator_profiles(values, target.field(), &path)?;

        match operation {
            OperatorFileOperation::List => {
                return Ok(OperatorFileResult {
                    changed: false,
                    identities: identities.into_iter().collect(),
                });
            }
            OperatorFileOperation::Add(_) => {
                let identity = requested_identity
                    .as_deref()
                    .expect("add operation has a normalized identity");
                if identities.insert(identity.to_owned()) {
                    let mut profile = serde_json::Map::new();
                    if uuid::Uuid::parse_str(identity).is_ok() {
                        profile.insert(
                            "uuid".to_owned(),
                            serde_json::Value::String(identity.to_owned()),
                        );
                    } else {
                        profile.insert(
                            "name".to_owned(),
                            serde_json::Value::String(identity.to_owned()),
                        );
                    }
                    values.push(serde_json::Value::Object(profile));
                } else {
                    return Ok(OperatorFileResult {
                        changed: false,
                        identities: identities.into_iter().collect(),
                    });
                }
            }
            OperatorFileOperation::Remove(_) => {
                let identity = requested_identity
                    .as_deref()
                    .expect("remove operation has a normalized identity");
                // Keep paired identities together, including duplicate/overlapping
                // profiles. Removing only a field can leave an alias authorized.
                let mut removed_identities = BTreeSet::from([identity.to_owned()]);
                let mut removed = false;
                loop {
                    let previous_len = values.len();
                    values.retain(|value| {
                        let profile = value.as_object().expect("validated access-control profile");
                        let matches = ["name", "uuid"].iter().any(|key| {
                            profile
                                .get(*key)
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|identity| removed_identities.contains(identity))
                        });
                        if matches {
                            for key in ["name", "uuid"] {
                                if let Some(identity) =
                                    profile.get(key).and_then(serde_json::Value::as_str)
                                {
                                    removed_identities.insert(identity.to_owned());
                                }
                            }
                        }
                        !matches
                    });
                    if values.len() == previous_len {
                        break;
                    }
                    removed = true;
                }
                identities.retain(|identity| !removed_identities.contains(identity));
                if !removed {
                    return Ok(OperatorFileResult {
                        changed: false,
                        identities: identities.into_iter().collect(),
                    });
                }
            }
        }

        write_access_profiles(&path, &values, metadata.as_ref())?;
        Ok(OperatorFileResult {
            changed: true,
            identities: identities.into_iter().collect(),
        })
    }
}

/// File-backed access-control target for [`ServerConfig::manage_access_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessControlTarget {
    Operators,
    Whitelist,
}

impl AccessControlTarget {
    fn configured_path(self, config: &ServerConfig) -> Option<&Path> {
        match self {
            Self::Operators => config.admin.operators_file.as_deref(),
            Self::Whitelist => config.auth.whitelist_file.as_deref(),
        }
    }

    /// Config field named in diagnostics.
    fn field(self) -> &'static str {
        match self {
            Self::Operators => "admin.operators_file",
            Self::Whitelist => "auth.whitelist_file",
        }
    }

    /// Human-readable subject used in diagnostics.
    fn subject(self) -> &'static str {
        match self {
            Self::Operators => "operator",
            Self::Whitelist => "whitelist",
        }
    }
}

fn load_access_control_file(
    config_path: &Path,
    configured_path: &Path,
    field: &'static str,
) -> anyhow::Result<Vec<String>> {
    let path = resolve_config_relative_path(config_path, configured_path);
    let values = read_access_control_values(config_path, configured_path, field)?.1;
    let mut entries = BTreeSet::new();
    for (index, value) in values.into_iter().enumerate() {
        let profile = parse_access_control_profile(value, field, &path, index)?;
        let mut populated = false;
        if let Some(name) = profile.name {
            entries.insert(validate_access_control_name(&name, field, &path, index)?);
            populated = true;
        }
        if let Some(raw_uuid) = profile.uuid {
            let raw_uuid = raw_uuid.trim();
            if raw_uuid.is_empty() {
                bail!(
                    "{field} entry {index} from {} contains an empty uuid",
                    path.display()
                );
            }
            let uuid = uuid::Uuid::parse_str(raw_uuid).map_err(|_| {
                anyhow::anyhow!(
                    "{field} entry {index} from {} contains an invalid uuid",
                    path.display()
                )
            })?;
            entries.insert(uuid.to_string());
            populated = true;
        }
        if !populated {
            bail!(
                "{field} entry {index} from {} must contain name and/or uuid",
                path.display()
            );
        }
    }
    Ok(entries.into_iter().collect())
}

fn read_access_control_values(
    config_path: &Path,
    configured_path: &Path,
    field: &'static str,
) -> anyhow::Result<(PathBuf, Vec<serde_json::Value>)> {
    let path = resolve_config_relative_path(config_path, configured_path);
    let file = std::fs::File::open(&path)
        .with_context(|| format!("opening {field} from {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {field} metadata from {}", path.display()))?;
    if !metadata.is_file() {
        bail!("{field} must point to a regular file: {}", path.display());
    }
    if metadata.len() > MAX_ACCESS_CONTROL_FILE_BYTES {
        bail!(
            "{field} exceeds the {} byte limit: {}",
            MAX_ACCESS_CONTROL_FILE_BYTES,
            path.display()
        );
    }
    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len().min(MAX_ACCESS_CONTROL_FILE_BYTES)).unwrap_or(0),
    );
    file.take(MAX_ACCESS_CONTROL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading bounded {field} from {}", path.display()))?;
    if bytes.len() as u64 > MAX_ACCESS_CONTROL_FILE_BYTES {
        bail!(
            "{field} grew beyond the {} byte limit while reading: {}",
            MAX_ACCESS_CONTROL_FILE_BYTES,
            path.display()
        );
    }
    let values: Vec<serde_json::Value> = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing {field} JSON from {}", path.display()))?;
    if values.len() > MAX_ACCESS_CONTROL_FILE_ENTRIES {
        bail!(
            "{field} from {} contains {} entries; maximum is {}",
            path.display(),
            values.len(),
            MAX_ACCESS_CONTROL_FILE_ENTRIES
        );
    }
    Ok((path, values))
}

fn parse_access_control_profile(
    value: serde_json::Value,
    field: &'static str,
    path: &Path,
    index: usize,
) -> anyhow::Result<AccessControlProfileEntry> {
    serde_json::from_value(value)
        .with_context(|| format!("parsing {field} JSON entry {index} from {}", path.display()))
}

fn canonicalize_operator_profiles(
    values: Vec<serde_json::Value>,
    field: &'static str,
    path: &Path,
) -> anyhow::Result<(Vec<serde_json::Value>, BTreeSet<String>)> {
    let mut identities = BTreeSet::new();
    let mut canonical = Vec::with_capacity(values.len());
    for (index, mut value) in values.into_iter().enumerate() {
        let profile = parse_access_control_profile(value.clone(), field, path, index)?;
        let mut populated = false;
        if let Some(name) = profile.name.as_deref() {
            let identity = validate_access_control_name(name, field, path, index)?;
            populated = true;
            identities.insert(identity.clone());
            value
                .as_object_mut()
                .expect("validated operator profile")
                .insert("name".to_owned(), serde_json::Value::String(identity));
        }
        if let Some(raw_uuid) = profile.uuid.as_deref() {
            let identity = normalize_profile_identity("uuid", raw_uuid).map_err(|message| {
                anyhow::anyhow!("{field} entry {index} from {} {message}", path.display())
            })?;
            populated = true;
            identities.insert(identity.clone());
            value
                .as_object_mut()
                .expect("validated operator profile")
                .insert("uuid".to_owned(), serde_json::Value::String(identity));
        }
        if !populated {
            bail!(
                "{field} entry {index} from {} must contain name and/or uuid",
                path.display()
            );
        }
        // Do not deduplicate individual fields: that discards the relationship
        // needed to revoke all aliases in overlapping profiles.
        canonical.push(value);
    }
    Ok((canonical, identities))
}

fn normalize_profile_identity(key: &str, raw: &str) -> Result<String, String> {
    if key == "uuid" {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("contains an empty uuid".to_owned());
        }
        return uuid::Uuid::parse_str(raw)
            .map(|uuid| uuid.to_string())
            .map_err(|_| "contains an invalid uuid".to_owned());
    }
    let name = raw.trim();
    if !(3..=16).contains(&name.len())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(
            "contains an invalid Minecraft username; expected 3..=16 ASCII letters, digits, or `_`"
                .to_owned(),
        );
    }
    Ok(name.to_ascii_lowercase())
}

fn normalize_access_identity(raw: &str) -> anyhow::Result<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("operator identity cannot be empty; provide a Minecraft username or UUID");
    }
    if let Ok(uuid) = uuid::Uuid::parse_str(raw) {
        return Ok(uuid.to_string());
    }
    normalize_profile_identity("name", raw)
        .map_err(|message| anyhow::anyhow!("invalid operator identity `{raw}`: {message}"))
}

fn write_access_profiles(
    path: &Path,
    values: &[serde_json::Value],
    metadata: Option<&std::fs::Metadata>,
) -> anyhow::Result<()> {
    let mut rendered = serde_json::to_vec_pretty(values).context("rendering operator file JSON")?;
    rendered.push(b'\n');
    if values.len() > MAX_ACCESS_CONTROL_FILE_ENTRIES
        || rendered.len() as u64 > MAX_ACCESS_CONTROL_FILE_BYTES
    {
        bail!("updated operator file exceeds access-control file limits");
    }
    if let Some(metadata) = metadata {
        if metadata.permissions().readonly() {
            bail!(
                "operator management refuses to replace a read-only file: {}",
                path.display()
            );
        }
        // Rename permission comes from the directory, not the target. Do not
        // bypass the target's existing write permissions during replacement.
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .with_context(|| format!("checking operator file write access {}", path.display()))?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(
        path.parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )
    .with_context(|| format!("creating temporary operator file for {}", path.display()))?;
    temporary
        .write_all(&rendered)
        .with_context(|| format!("writing temporary operator file for {}", path.display()))?;
    if let Some(metadata) = metadata {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            std::os::unix::fs::chown(temporary.path(), Some(metadata.uid()), Some(metadata.gid()))
                .context("preserving operator file ownership")?;
        }
        temporary
            .as_file()
            .set_permissions(metadata.permissions())
            .context("preserving operator file permissions")?;
    }
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("syncing temporary operator file for {}", path.display()))?;
    mc_world::atomic_file::replace_file_durable(temporary.path(), path)
        .with_context(|| format!("replacing and syncing operator file {}", path.display()))
}

fn access_file_metadata(path: &Path) -> anyhow::Result<Option<std::fs::Metadata>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("reading metadata for {}", path.display()));
        }
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!(
            "operator management requires a regular, non-symlink file: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            bail!(
                "operator management refuses multiply linked files: {}",
                path.display()
            );
        }
    }
    Ok(Some(metadata))
}

fn lock_access_file(path: &Path) -> anyhow::Result<std::fs::File> {
    let mut name = path
        .file_name()
        .context("operator file path must have a file name")?
        .to_os_string();
    name.push(".lock");
    let lock_path = path.with_file_name(name);
    access_file_metadata(&lock_path)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening operator lock {}", lock_path.display()))?;
    lock.lock()
        .with_context(|| format!("locking operator file {}", path.display()))?;
    Ok(lock)
}

fn resolve_config_relative_path(config_path: &Path, configured_path: &Path) -> PathBuf {
    if configured_path.is_absolute() {
        return configured_path.to_path_buf();
    }
    config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(configured_path)
}

fn validate_access_control_name(
    raw: &str,
    field: &'static str,
    path: &Path,
    index: usize,
) -> anyhow::Result<String> {
    normalize_profile_identity("name", raw).map_err(|message| {
        anyhow::anyhow!("{field} entry {index} from {} {message}", path.display())
    })
}
