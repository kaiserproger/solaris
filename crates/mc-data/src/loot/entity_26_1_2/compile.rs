use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::model::*;
use crate::Identifier;

#[derive(Clone, Copy)]
pub(super) struct JsonBudgetLimits {
    pub(super) nodes: usize,
    pub(super) collection_elements: usize,
    pub(super) string_bytes: usize,
    pub(super) array_length: usize,
}

impl Default for JsonBudgetLimits {
    fn default() -> Self {
        Self {
            nodes: MAX_JSON_NODES,
            collection_elements: MAX_JSON_COLLECTION_ELEMENTS,
            string_bytes: MAX_JSON_STRING_BYTES,
            array_length: MAX_JSON_ARRAY_LENGTH,
        }
    }
}

impl JsonBudgetLimits {
    #[cfg(test)]
    pub(super) fn unbounded() -> Self {
        Self {
            nodes: usize::MAX,
            collection_elements: usize::MAX,
            string_bytes: usize::MAX,
            array_length: usize::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JsonBudgetViolation {
    pub(super) limit: EntityLootLimit,
    pub(super) actual: u64,
}

pub(super) fn validate_json_budget(
    value: &Value,
    limits: JsonBudgetLimits,
) -> Result<(), JsonBudgetViolation> {
    let mut nodes = 0_usize;
    let mut collection_elements = 0_usize;
    let mut string_bytes = 0_usize;
    let mut pending = vec![value];

    while let Some(value) = pending.pop() {
        nodes = checked_json_budget_add(nodes, 1, EntityLootLimit::JsonNodes)?;
        check_json_budget(EntityLootLimit::JsonNodes, nodes, limits.nodes)?;
        match value {
            Value::String(value) => {
                string_bytes = checked_json_budget_add(
                    string_bytes,
                    value.len(),
                    EntityLootLimit::JsonStringBytes,
                )?;
                check_json_budget(
                    EntityLootLimit::JsonStringBytes,
                    string_bytes,
                    limits.string_bytes,
                )?;
            }
            Value::Array(values) => {
                check_json_budget(
                    EntityLootLimit::JsonArrayLength,
                    values.len(),
                    limits.array_length,
                )?;
                collection_elements = checked_json_budget_add(
                    collection_elements,
                    values.len(),
                    EntityLootLimit::JsonCollectionElements,
                )?;
                check_json_budget(
                    EntityLootLimit::JsonCollectionElements,
                    collection_elements,
                    limits.collection_elements,
                )?;
                pending.extend(values);
            }
            Value::Object(fields) => {
                collection_elements = checked_json_budget_add(
                    collection_elements,
                    fields.len(),
                    EntityLootLimit::JsonCollectionElements,
                )?;
                check_json_budget(
                    EntityLootLimit::JsonCollectionElements,
                    collection_elements,
                    limits.collection_elements,
                )?;
                for (field, value) in fields {
                    string_bytes = checked_json_budget_add(
                        string_bytes,
                        field.len(),
                        EntityLootLimit::JsonStringBytes,
                    )?;
                    check_json_budget(
                        EntityLootLimit::JsonStringBytes,
                        string_bytes,
                        limits.string_bytes,
                    )?;
                    pending.push(value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    Ok(())
}

fn checked_json_budget_add(
    current: usize,
    increment: usize,
    limit: EntityLootLimit,
) -> Result<usize, JsonBudgetViolation> {
    current.checked_add(increment).ok_or(JsonBudgetViolation {
        limit,
        actual: u64::MAX,
    })
}

fn check_json_budget(
    limit: EntityLootLimit,
    actual: usize,
    maximum: usize,
) -> Result<(), JsonBudgetViolation> {
    if actual > maximum {
        return Err(JsonBudgetViolation {
            limit,
            actual: actual as u64,
        });
    }
    Ok(())
}

impl CompiledEntityLootTable {
    pub fn compile(id: Identifier, raw: &str) -> Result<Self, EntityLootCompileError> {
        if raw.len() > MAX_SOURCE_BYTES {
            return Err(EntityLootCompileError {
                table: id,
                path: "$".to_string(),
                kind: EntityLootCompileErrorKind::LimitExceeded {
                    limit: EntityLootLimit::SourceBytes,
                    actual: raw.len() as u64,
                },
            });
        }
        let value: Value = serde_json::from_str(raw).map_err(|source| EntityLootCompileError {
            table: id.clone(),
            path: "$".to_string(),
            kind: EntityLootCompileErrorKind::MalformedJson {
                message: source.to_string(),
            },
        })?;
        validate_json_budget(&value, JsonBudgetLimits::default()).map_err(|violation| {
            EntityLootCompileError {
                table: id.clone(),
                path: "$".to_string(),
                kind: EntityLootCompileErrorKind::LimitExceeded {
                    limit: violation.limit,
                    actual: violation.actual,
                },
            }
        })?;
        Compiler::new(id).compile_table(&value)
    }
}

impl EntityLootCatalog {
    pub fn from_tables(
        roots: impl IntoIterator<Item = Identifier>,
        tables: impl IntoIterator<Item = CompiledEntityLootTable>,
    ) -> Result<Self, EntityLootLoadError> {
        let roots = collect_roots(roots)?;

        let mut compiled = BTreeMap::new();
        for table in tables {
            let table_id = table.id.clone();
            if compiled.contains_key(&table_id) {
                return Err(EntityLootLoadError::DuplicateTable { table: table_id });
            }
            check_load_limit(
                EntityLootLimit::CatalogResources,
                checked_load_count(EntityLootLimit::CatalogResources, compiled.len())?,
            )?;
            compiled.insert(table_id, table);
        }

        for root in &roots {
            let table = compiled
                .get(root)
                .ok_or_else(|| EntityLootLoadError::MissingRoot { root: root.clone() })?;
            if table.table_type != LootTableType::Entity {
                return Err(EntityLootLoadError::InvalidRootType {
                    root: root.clone(),
                    table_type: table.table_type.as_str().to_string(),
                });
            }
        }
        for table in compiled.values() {
            for referenced in &table.references {
                if !compiled.contains_key(referenced) {
                    return Err(EntityLootLoadError::MissingReference {
                        table: table.id.clone(),
                        referenced: referenced.clone(),
                    });
                }
            }
        }

        validate_reference_graph(&roots, &compiled)?;
        Ok(Self {
            roots,
            tables: compiled,
        })
    }

    /// Compiles the complete resource-ID closure rooted at entity loot tables.
    ///
    /// `data_root` is the data-pack `data` directory. A table
    /// `namespace:path` is read from
    /// `<data_root>/<namespace>/loot_table/<path>.json`. Only configured roots
    /// and their transitive references are loaded, so entity references to
    /// `minecraft:fishing` tables are included without compiling unrelated
    /// loot-table families.
    pub fn compile_resources(
        data_root: impl AsRef<Path>,
        roots: impl IntoIterator<Item = Identifier>,
    ) -> Result<Self, EntityLootLoadError> {
        let data_root = data_root.as_ref();
        let roots = collect_roots(roots)?;

        let mut pending = roots
            .iter()
            .cloned()
            .map(|root| (root, None))
            .collect::<VecDeque<_>>();
        let mut scheduled = roots.clone();
        let mut tables = BTreeMap::new();
        while let Some((table_id, referenced_from)) = pending.pop_front() {
            if tables.contains_key(&table_id) {
                continue;
            }
            let path = resource_path(data_root, &table_id);
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    return Err(referenced_from.map_or_else(
                        || EntityLootLoadError::MissingRoot {
                            root: table_id.clone(),
                        },
                        |table| EntityLootLoadError::MissingReference {
                            table,
                            referenced: table_id.clone(),
                        },
                    ));
                }
                Err(source) => {
                    return Err(EntityLootLoadError::Io { path, source });
                }
            };
            let mut raw = Vec::new();
            file.take(MAX_SOURCE_BYTES as u64 + 1)
                .read_to_end(&mut raw)
                .map_err(|source| EntityLootLoadError::Io {
                    path: path.clone(),
                    source,
                })?;
            if raw.len() > MAX_SOURCE_BYTES {
                return Err(EntityLootLoadError::Compile {
                    path,
                    source: EntityLootCompileError {
                        table: table_id,
                        path: "$".to_string(),
                        kind: EntityLootCompileErrorKind::LimitExceeded {
                            limit: EntityLootLimit::SourceBytes,
                            actual: raw.len() as u64,
                        },
                    },
                });
            }
            let raw = String::from_utf8(raw).map_err(|source| EntityLootLoadError::Io {
                path: path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
            })?;
            let table =
                CompiledEntityLootTable::compile(table_id.clone(), &raw).map_err(|source| {
                    EntityLootLoadError::Compile {
                        path: path.clone(),
                        source,
                    }
                })?;
            for referenced in &table.references {
                if scheduled.insert(referenced.clone()) {
                    check_load_limit(EntityLootLimit::CatalogResources, scheduled.len())?;
                    check_load_limit(
                        EntityLootLimit::ClosureWidth,
                        checked_load_count(EntityLootLimit::ClosureWidth, pending.len())?,
                    )?;
                    pending.push_back((referenced.clone(), Some(table_id.clone())));
                }
            }
            tables.insert(table_id, table);
        }

        Self::from_tables(roots, tables.into_values())
    }

    #[must_use]
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    #[must_use]
    pub fn contains(&self, table: &Identifier) -> bool {
        self.tables.contains_key(table)
    }

    #[must_use]
    pub fn inventory(&self) -> EntityLootInventory {
        let mut table_types = BTreeSet::new();
        let mut condition_families = BTreeSet::new();
        let mut function_families = BTreeSet::new();
        let mut entry_families = BTreeSet::new();
        let mut number_provider_families = BTreeSet::new();
        let mut reference_count = 0;

        for table in self.tables.values() {
            table_types.insert(table.table_type.as_str().to_string());
            condition_families.extend(table.condition_families.iter().cloned());
            function_families.extend(table.function_families.iter().cloned());
            entry_families.extend(table.entry_families.iter().cloned());
            number_provider_families.extend(table.number_provider_families.iter().cloned());
            reference_count += table.references.len();
        }

        EntityLootInventory {
            root_count: self.roots.len(),
            table_count: self.tables.len(),
            reference_count,
            table_types,
            condition_families,
            function_families,
            entry_families,
            number_provider_families,
        }
    }
}

fn collect_roots(
    roots: impl IntoIterator<Item = Identifier>,
) -> Result<BTreeSet<Identifier>, EntityLootLoadError> {
    let mut collected = BTreeSet::new();
    let mut processed = 0_usize;
    for root in roots {
        processed = checked_load_count(EntityLootLimit::CatalogRoots, processed)?;
        check_load_limit(EntityLootLimit::CatalogRoots, processed)?;
        collected.insert(root);
    }
    if collected.is_empty() {
        return Err(EntityLootLoadError::EmptyRoots);
    }
    Ok(collected)
}

fn check_load_limit(limit: EntityLootLimit, actual: usize) -> Result<(), EntityLootLoadError> {
    let actual = u64::try_from(actual).map_err(|_| EntityLootLoadError::LimitExceeded {
        limit,
        actual: u64::MAX,
        maximum: limit.maximum(),
    })?;
    let maximum = limit.maximum();
    if actual > maximum {
        return Err(EntityLootLoadError::LimitExceeded {
            limit,
            actual,
            maximum,
        });
    }
    Ok(())
}

fn checked_load_count(
    limit: EntityLootLimit,
    current: usize,
) -> Result<usize, EntityLootLoadError> {
    current
        .checked_add(1)
        .ok_or(EntityLootLoadError::LimitExceeded {
            limit,
            actual: u64::MAX,
            maximum: limit.maximum(),
        })
}

fn resource_path(data_root: &Path, table: &Identifier) -> PathBuf {
    data_root
        .join(table.namespace())
        .join("loot_table")
        .join(table.path())
        .with_extension("json")
}

fn validate_reference_graph(
    roots: &BTreeSet<Identifier>,
    tables: &BTreeMap<Identifier, CompiledEntityLootTable>,
) -> Result<(), EntityLootLoadError> {
    let mut starts = roots.iter().cloned().collect::<Vec<_>>();
    starts.extend(
        tables
            .keys()
            .filter(|table| !roots.contains(*table))
            .cloned(),
    );
    let mut checked_depths = BTreeMap::new();
    for root in starts {
        let mut path = Vec::new();
        validate_reference_node(&root, &root, tables, &mut path, &mut checked_depths)?;
    }
    Ok(())
}

fn validate_reference_node(
    root: &Identifier,
    table_id: &Identifier,
    tables: &BTreeMap<Identifier, CompiledEntityLootTable>,
    path: &mut Vec<Identifier>,
    checked_depths: &mut BTreeMap<Identifier, usize>,
) -> Result<(), EntityLootLoadError> {
    if let Some(cycle_start) = path.iter().position(|visited| visited == table_id) {
        let mut tables = path[cycle_start..].to_vec();
        tables.push(table_id.clone());
        return Err(EntityLootLoadError::ReferenceCycle { tables });
    }
    if path.len() >= MAX_REFERENCE_DEPTH {
        return Err(EntityLootLoadError::ReferenceDepthExceeded {
            root: root.clone(),
            table: table_id.clone(),
            maximum: MAX_REFERENCE_DEPTH,
        });
    }
    let current_depth =
        path.len()
            .checked_add(1)
            .ok_or_else(|| EntityLootLoadError::ReferenceDepthExceeded {
                root: root.clone(),
                table: table_id.clone(),
                maximum: MAX_REFERENCE_DEPTH,
            })?;
    if checked_depths
        .get(table_id)
        .is_some_and(|checked_depth| *checked_depth >= current_depth)
    {
        return Ok(());
    }

    let table = tables
        .get(table_id)
        .ok_or_else(|| EntityLootLoadError::MissingReference {
            table: path.last().cloned().unwrap_or_else(|| root.clone()),
            referenced: table_id.clone(),
        })?;
    path.push(table_id.clone());
    for referenced in &table.references {
        validate_reference_node(root, referenced, tables, path, checked_depths)?;
    }
    path.pop();
    checked_depths
        .entry(table_id.clone())
        .and_modify(|checked_depth| *checked_depth = (*checked_depth).max(current_depth))
        .or_insert(current_depth);
    Ok(())
}

struct Compiler {
    table: Identifier,
    condition_families: BTreeSet<String>,
    function_families: BTreeSet<String>,
    entry_families: BTreeSet<String>,
    number_provider_families: BTreeSet<String>,
    references: BTreeSet<Identifier>,
}

impl Compiler {
    fn new(table: Identifier) -> Self {
        Self {
            table,
            condition_families: BTreeSet::new(),
            function_families: BTreeSet::new(),
            entry_families: BTreeSet::new(),
            number_provider_families: BTreeSet::new(),
            references: BTreeSet::new(),
        }
    }

    fn compile_table(
        mut self,
        value: &Value,
    ) -> Result<CompiledEntityLootTable, EntityLootCompileError> {
        let path = "$";
        let fields = self.object(value, path)?;
        self.check_fields(
            fields,
            &["type", "random_sequence", "pools", "functions"],
            path,
        )?;
        let table_type = match self.required_string(fields, "type", path)? {
            "minecraft:entity" => LootTableType::Entity,
            "minecraft:fishing" => LootTableType::Fishing,
            unsupported => {
                return Err(self.error(
                    "$.type",
                    EntityLootCompileErrorKind::InvalidValue {
                        message: format!(
                            "entity loot closure supports minecraft:entity and referenced minecraft:fishing tables, got {unsupported:?}"
                        ),
                    },
                ));
            }
        };
        let random_sequence = fields
            .get("random_sequence")
            .map(|value| self.identifier(value, "$.random_sequence"))
            .transpose()?;
        let pools = match fields.get("pools") {
            Some(value) => {
                let pools = self.array(value, "$.pools")?;
                if pools.len() > MAX_POOLS_PER_TABLE {
                    return Err(self.error(
                        "$.pools",
                        EntityLootCompileErrorKind::LimitExceeded {
                            limit: EntityLootLimit::PoolsPerTable,
                            actual: pools.len() as u64,
                        },
                    ));
                }
                pools
                    .iter()
                    .enumerate()
                    .map(|(index, pool)| self.pool(pool, &format!("$.pools[{index}]"), 1))
                    .collect::<Result<Vec<_>, _>>()?
            }
            None => Vec::new(),
        };
        let functions = self.function_list(fields.get("functions"), "$.functions", 1)?;

        Ok(CompiledEntityLootTable {
            id: self.table,
            table_type,
            random_sequence,
            pools,
            functions,
            condition_families: self.condition_families,
            function_families: self.function_families,
            entry_families: self.entry_families,
            number_provider_families: self.number_provider_families,
            references: self.references,
        })
    }

    fn pool(
        &mut self,
        value: &Value,
        path: &str,
        depth: usize,
    ) -> Result<LootPool, EntityLootCompileError> {
        self.check_depth(path, depth)?;
        let fields = self.object(value, path)?;
        self.check_fields(
            fields,
            &["conditions", "functions", "rolls", "bonus_rolls", "entries"],
            path,
        )?;
        let conditions = self.condition_list(
            fields.get("conditions"),
            &format!("{path}.conditions"),
            depth + 1,
        )?;
        let functions = self.function_list(
            fields.get("functions"),
            &format!("{path}.functions"),
            depth + 1,
        )?;
        let rolls = self.number_provider(
            self.required(fields, "rolls", path)?,
            &format!("{path}.rolls"),
        )?;
        let bonus_rolls = fields.get("bonus_rolls").map_or_else(
            || Ok(NumberProvider::Constant(0.0)),
            |value| self.number_provider(value, &format!("{path}.bonus_rolls")),
        )?;
        if rolls.integer_upper_bound() > MAX_POOL_ROLLS {
            return Err(self.error(
                &format!("{path}.rolls"),
                EntityLootCompileErrorKind::LimitExceeded {
                    limit: EntityLootLimit::PoolRolls,
                    actual: rolls.integer_upper_bound() as u64,
                },
            ));
        }

        let entries = self
            .array(
                self.required(fields, "entries", path)?,
                &format!("{path}.entries"),
            )?
            .iter()
            .enumerate()
            .map(|(index, entry)| self.entry(entry, &format!("{path}.entries[{index}]"), depth + 1))
            .collect::<Result<Vec<_>, _>>()?;

        if let Some(static_count) = entries.iter().try_fold(0_usize, |total, entry| {
            entry
                .static_candidate_count()
                .and_then(|count| total.checked_add(count))
        }) && static_count > MAX_CANDIDATES_PER_ROLL
        {
            return Err(self.error(
                &format!("{path}.entries"),
                EntityLootCompileErrorKind::LimitExceeded {
                    limit: EntityLootLimit::CandidatesPerRoll,
                    actual: static_count as u64,
                },
            ));
        }

        let mut static_weight = 0_i32;
        for entry in &entries {
            let Some(weight) = entry.static_expanded_weight() else {
                continue;
            };
            static_weight = static_weight.checked_add(weight).ok_or_else(|| {
                self.error(
                    &format!("{path}.entries"),
                    EntityLootCompileErrorKind::NumericOverflow {
                        value: "static pool weight sum".to_string(),
                    },
                )
            })?;
        }

        Ok(LootPool {
            conditions,
            functions,
            rolls,
            bonus_rolls,
            entries,
        })
    }

    fn entry(
        &mut self,
        value: &Value,
        path: &str,
        depth: usize,
    ) -> Result<LootEntry, EntityLootCompileError> {
        self.check_depth(path, depth)?;
        let fields = self.object(value, path)?;
        let entry_type = self.required_string(fields, "type", path)?.to_string();
        self.entry_families.insert(entry_type.clone());
        match entry_type.as_str() {
            "minecraft:item" => {
                self.check_fields(
                    fields,
                    &[
                        "type",
                        "name",
                        "weight",
                        "quality",
                        "conditions",
                        "functions",
                    ],
                    path,
                )?;
                let item = self.identifier(
                    self.required(fields, "name", path)?,
                    &format!("{path}.name"),
                )?;
                Ok(LootEntry::Item {
                    singleton: self.singleton(fields, path, depth + 1)?,
                    item,
                })
            }
            "minecraft:loot_table" => {
                self.check_fields(
                    fields,
                    &[
                        "type",
                        "value",
                        "weight",
                        "quality",
                        "conditions",
                        "functions",
                    ],
                    path,
                )?;
                let table = self.identifier(
                    self.required(fields, "value", path)?,
                    &format!("{path}.value"),
                )?;
                if !self.references.contains(&table)
                    && self.references.len() >= MAX_REFERENCES_PER_TABLE
                {
                    let actual = self.references.len().checked_add(1).ok_or_else(|| {
                        self.error(
                            &format!("{path}.value"),
                            EntityLootCompileErrorKind::NumericOverflow {
                                value: "reference count".to_string(),
                            },
                        )
                    })?;
                    let actual = u64::try_from(actual).map_err(|_| {
                        self.error(
                            &format!("{path}.value"),
                            EntityLootCompileErrorKind::NumericOverflow {
                                value: "reference count".to_string(),
                            },
                        )
                    })?;
                    return Err(self.error(
                        &format!("{path}.value"),
                        EntityLootCompileErrorKind::LimitExceeded {
                            limit: EntityLootLimit::ReferencesPerTable,
                            actual,
                        },
                    ));
                }
                self.references.insert(table.clone());
                Ok(LootEntry::Table {
                    singleton: self.singleton(fields, path, depth + 1)?,
                    table,
                })
            }
            "minecraft:empty" => {
                self.check_fields(
                    fields,
                    &["type", "weight", "quality", "conditions", "functions"],
                    path,
                )?;
                Ok(LootEntry::Empty {
                    singleton: self.singleton(fields, path, depth + 1)?,
                })
            }
            "minecraft:tag" => {
                self.check_fields(
                    fields,
                    &[
                        "type",
                        "name",
                        "expand",
                        "weight",
                        "quality",
                        "conditions",
                        "functions",
                    ],
                    path,
                )?;
                let tag = self.identifier(
                    self.required(fields, "name", path)?,
                    &format!("{path}.name"),
                )?;
                let expand = self.required_bool(fields, "expand", path)?;
                Ok(LootEntry::Tag {
                    singleton: self.singleton(fields, path, depth + 1)?,
                    tag,
                    expand,
                })
            }
            "minecraft:alternatives" => {
                self.check_fields(fields, &["type", "conditions", "children"], path)?;
                let conditions = self.condition_list(
                    fields.get("conditions"),
                    &format!("{path}.conditions"),
                    depth + 1,
                )?;
                let children = self
                    .array(
                        self.required(fields, "children", path)?,
                        &format!("{path}.children"),
                    )?
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        self.entry(child, &format!("{path}.children[{index}]"), depth + 1)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(LootEntry::Alternatives {
                    conditions,
                    children,
                })
            }
            unsupported => Err(self.error(
                path,
                EntityLootCompileErrorKind::UnsupportedEntry {
                    entry: unsupported.to_string(),
                },
            )),
        }
    }

    fn singleton(
        &mut self,
        fields: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Result<SingletonEntry, EntityLootCompileError> {
        let weight = fields
            .get("weight")
            .map_or(Ok(1), |value| self.i32(value, &format!("{path}.weight")))?;
        let quality = fields
            .get("quality")
            .map_or(Ok(0), |value| self.i32(value, &format!("{path}.quality")))?;
        let conditions = self.condition_list(
            fields.get("conditions"),
            &format!("{path}.conditions"),
            depth,
        )?;
        let functions =
            self.function_list(fields.get("functions"), &format!("{path}.functions"), depth)?;
        Ok(SingletonEntry {
            weight,
            quality,
            conditions,
            functions,
        })
    }

    fn condition_list(
        &mut self,
        value: Option<&Value>,
        path: &str,
        depth: usize,
    ) -> Result<Vec<LootCondition>, EntityLootCompileError> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        self.array(value, path)?
            .iter()
            .enumerate()
            .map(|(index, condition)| self.condition(condition, &format!("{path}[{index}]"), depth))
            .collect()
    }

    fn condition(
        &mut self,
        value: &Value,
        path: &str,
        depth: usize,
    ) -> Result<LootCondition, EntityLootCompileError> {
        self.check_depth(path, depth)?;
        let fields = self.object(value, path)?;
        let condition = self.required_string(fields, "condition", path)?.to_string();
        self.condition_families.insert(condition.clone());
        match condition.as_str() {
            "minecraft:any_of" => {
                self.check_fields(fields, &["condition", "terms"], path)?;
                let terms = self
                    .array(
                        self.required(fields, "terms", path)?,
                        &format!("{path}.terms"),
                    )?
                    .iter()
                    .enumerate()
                    .map(|(index, term)| {
                        self.condition(term, &format!("{path}.terms[{index}]"), depth + 1)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(LootCondition::AnyOf(terms))
            }
            "minecraft:damage_source_properties" => {
                self.check_fields(fields, &["condition", "predicate"], path)?;
                let predicate = fields.get("predicate").map_or_else(
                    || Ok(DamageSourcePredicate::default()),
                    |value| {
                        self.damage_source_predicate(value, &format!("{path}.predicate"), depth + 1)
                    },
                )?;
                Ok(LootCondition::DamageSource(predicate))
            }
            "minecraft:entity_properties" => {
                self.check_fields(fields, &["condition", "entity", "predicate"], path)?;
                let target = match self.required_string(fields, "entity", path)? {
                    "this" => EntityTarget::This,
                    "attacker" => EntityTarget::Attacker,
                    "direct_attacker" => EntityTarget::DirectAttacker,
                    "attacking_player" => EntityTarget::AttackingPlayer,
                    unsupported => {
                        return Err(self.error(
                            &format!("{path}.entity"),
                            EntityLootCompileErrorKind::InvalidValue {
                                message: format!("unsupported entity target {unsupported:?}"),
                            },
                        ));
                    }
                };
                let predicate = fields.get("predicate").map_or_else(
                    || Ok(EntityPredicate::default()),
                    |value| self.entity_predicate(value, &format!("{path}.predicate"), depth + 1),
                )?;
                Ok(LootCondition::EntityProperties { target, predicate })
            }
            "minecraft:inverted" => {
                self.check_fields(fields, &["condition", "term"], path)?;
                let term = self.condition(
                    self.required(fields, "term", path)?,
                    &format!("{path}.term"),
                    depth + 1,
                )?;
                Ok(LootCondition::Inverted(Box::new(term)))
            }
            "minecraft:killed_by_player" => {
                self.check_fields(fields, &["condition"], path)?;
                Ok(LootCondition::KilledByPlayer)
            }
            "minecraft:random_chance" => {
                self.check_fields(fields, &["condition", "chance"], path)?;
                let chance = self.number_provider(
                    self.required(fields, "chance", path)?,
                    &format!("{path}.chance"),
                )?;
                Ok(LootCondition::RandomChance(chance))
            }
            "minecraft:random_chance_with_enchanted_bonus" => {
                self.check_fields(
                    fields,
                    &[
                        "condition",
                        "unenchanted_chance",
                        "enchanted_chance",
                        "enchantment",
                    ],
                    path,
                )?;
                let unenchanted_chance = self.probability(
                    self.required(fields, "unenchanted_chance", path)?,
                    &format!("{path}.unenchanted_chance"),
                )?;
                let enchanted_path = format!("{path}.enchanted_chance");
                let enchanted = self.object(
                    self.required(fields, "enchanted_chance", path)?,
                    &enchanted_path,
                )?;
                self.check_fields(
                    enchanted,
                    &["type", "base", "per_level_above_first"],
                    &enchanted_path,
                )?;
                let provider_type = self
                    .required_string(enchanted, "type", &enchanted_path)?
                    .to_string();
                self.number_provider_families.insert(provider_type.clone());
                if provider_type != "minecraft:linear" {
                    return Err(self.error(
                        &enchanted_path,
                        EntityLootCompileErrorKind::UnsupportedNumberProvider {
                            provider: provider_type,
                        },
                    ));
                }
                let base = self.probability(
                    self.required(enchanted, "base", &enchanted_path)?,
                    &format!("{enchanted_path}.base"),
                )?;
                let per_level_above_first = self.finite_f64(
                    self.required(enchanted, "per_level_above_first", &enchanted_path)?,
                    &format!("{enchanted_path}.per_level_above_first"),
                )?;
                if per_level_above_first < 0.0 {
                    return Err(self.error(
                        &format!("{enchanted_path}.per_level_above_first"),
                        EntityLootCompileErrorKind::InvalidValue {
                            message: "enchanted chance increment must be non-negative".to_string(),
                        },
                    ));
                }
                let enchantment = self.identifier(
                    self.required(fields, "enchantment", path)?,
                    &format!("{path}.enchantment"),
                )?;
                Ok(LootCondition::RandomChanceWithEnchantedBonus {
                    unenchanted_chance,
                    base,
                    per_level_above_first,
                    enchantment,
                })
            }
            unsupported => Err(self.error(
                path,
                EntityLootCompileErrorKind::UnsupportedCondition {
                    condition: unsupported.to_string(),
                },
            )),
        }
    }

    fn damage_source_predicate(
        &mut self,
        value: &Value,
        path: &str,
        depth: usize,
    ) -> Result<DamageSourcePredicate, EntityLootCompileError> {
        self.check_depth(path, depth)?;
        let fields = self.object(value, path)?;
        self.check_fields(
            fields,
            &["tags", "direct_entity", "source_entity", "is_direct"],
            path,
        )?;
        let tags = match fields.get("tags") {
            Some(value) => self
                .array(value, &format!("{path}.tags"))?
                .iter()
                .enumerate()
                .map(|(index, tag)| {
                    let tag_path = format!("{path}.tags[{index}]");
                    let tag_fields = self.object(tag, &tag_path)?;
                    self.check_fields(tag_fields, &["id", "expected"], &tag_path)?;
                    Ok((
                        self.identifier(
                            self.required(tag_fields, "id", &tag_path)?,
                            &format!("{tag_path}.id"),
                        )?,
                        self.required_bool(tag_fields, "expected", &tag_path)?,
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        let direct_entity = fields
            .get("direct_entity")
            .map(|value| self.entity_predicate(value, &format!("{path}.direct_entity"), depth + 1))
            .transpose()?;
        let source_entity = fields
            .get("source_entity")
            .map(|value| self.entity_predicate(value, &format!("{path}.source_entity"), depth + 1))
            .transpose()?;
        let is_direct = fields
            .get("is_direct")
            .map(|value| self.bool(value, &format!("{path}.is_direct")))
            .transpose()?;
        Ok(DamageSourcePredicate {
            tags,
            direct_entity,
            source_entity,
            is_direct,
        })
    }

    fn entity_predicate(
        &mut self,
        value: &Value,
        path: &str,
        depth: usize,
    ) -> Result<EntityPredicate, EntityLootCompileError> {
        self.check_depth(path, depth)?;
        let fields = self.object(value, path)?;
        self.check_fields(
            fields,
            &[
                "type",
                "components",
                "flags",
                "equipment",
                "vehicle",
                "type_specific",
            ],
            path,
        )?;
        let entity_type = fields
            .get("type")
            .map(|value| self.tagged_identifier(value, &format!("{path}.type")))
            .transpose()?;
        let components = match fields.get("components") {
            Some(value) => {
                let component_fields = self.object(value, &format!("{path}.components"))?;
                let mut components = BTreeMap::new();
                for (name, value) in component_fields {
                    let component = Identifier::parse(name.clone()).map_err(|_| {
                        self.error(
                            &format!("{path}.components.{name}"),
                            EntityLootCompileErrorKind::InvalidIdentifier {
                                value: name.clone(),
                            },
                        )
                    })?;
                    let expected = value.as_str().ok_or_else(|| {
                        self.error(
                            &format!("{path}.components.{name}"),
                            EntityLootCompileErrorKind::Expected {
                                expected: "a string component value",
                            },
                        )
                    })?;
                    components.insert(component, expected.to_string());
                }
                components
            }
            None => BTreeMap::new(),
        };
        let (is_baby, is_on_fire) = match fields.get("flags") {
            Some(value) => {
                let flag_path = format!("{path}.flags");
                let flags = self.object(value, &flag_path)?;
                self.check_fields(flags, &["is_baby", "is_on_fire"], &flag_path)?;
                (
                    flags
                        .get("is_baby")
                        .map(|value| self.bool(value, &format!("{flag_path}.is_baby")))
                        .transpose()?,
                    flags
                        .get("is_on_fire")
                        .map(|value| self.bool(value, &format!("{flag_path}.is_on_fire")))
                        .transpose()?,
                )
            }
            None => (None, None),
        };
        let mainhand_enchantments = fields
            .get("equipment")
            .map(|value| self.mainhand_enchantment_predicate(value, &format!("{path}.equipment")))
            .transpose()?;
        let vehicle = fields
            .get("vehicle")
            .map(|value| {
                self.entity_predicate(value, &format!("{path}.vehicle"), depth + 1)
                    .map(Box::new)
            })
            .transpose()?;
        let type_specific = fields
            .get("type_specific")
            .map(|value| self.type_specific_predicate(value, &format!("{path}.type_specific")))
            .transpose()?;
        Ok(EntityPredicate {
            entity_type,
            components,
            is_baby,
            is_on_fire,
            mainhand_enchantments,
            vehicle,
            type_specific,
        })
    }

    fn mainhand_enchantment_predicate(
        &self,
        value: &Value,
        path: &str,
    ) -> Result<Vec<TaggedIdentifier>, EntityLootCompileError> {
        let equipment = self.object(value, path)?;
        self.check_fields(equipment, &["mainhand"], path)?;
        let mainhand_path = format!("{path}.mainhand");
        let mainhand = self.object(self.required(equipment, "mainhand", path)?, &mainhand_path)?;
        self.check_fields(mainhand, &["predicates"], &mainhand_path)?;
        let predicates_path = format!("{mainhand_path}.predicates");
        let predicates = self.object(
            self.required(mainhand, "predicates", &mainhand_path)?,
            &predicates_path,
        )?;
        self.check_fields(predicates, &["minecraft:enchantments"], &predicates_path)?;
        let enchantments_path = format!("{predicates_path}.minecraft:enchantments");
        self.array(
            self.required(predicates, "minecraft:enchantments", &predicates_path)?,
            &enchantments_path,
        )?
        .iter()
        .enumerate()
        .map(|(index, predicate)| {
            let predicate_path = format!("{enchantments_path}[{index}]");
            let fields = self.object(predicate, &predicate_path)?;
            self.check_fields(fields, &["enchantments"], &predicate_path)?;
            self.tagged_identifier(
                self.required(fields, "enchantments", &predicate_path)?,
                &format!("{predicate_path}.enchantments"),
            )
        })
        .collect()
    }

    fn type_specific_predicate(
        &self,
        value: &Value,
        path: &str,
    ) -> Result<TypeSpecificPredicate, EntityLootCompileError> {
        let fields = self.object(value, path)?;
        match self.required_string(fields, "type", path)? {
            "minecraft:sheep" => {
                self.check_fields(fields, &["type", "sheared"], path)?;
                Ok(TypeSpecificPredicate::Sheep {
                    sheared: self.required_bool(fields, "sheared", path)?,
                })
            }
            "minecraft:slime" => {
                self.check_fields(fields, &["type", "size"], path)?;
                Ok(TypeSpecificPredicate::Slime {
                    size: self.int_range(
                        self.required(fields, "size", path)?,
                        &format!("{path}.size"),
                    )?,
                })
            }
            "minecraft:raider" => {
                self.check_fields(fields, &["type", "is_captain"], path)?;
                Ok(TypeSpecificPredicate::Raider {
                    is_captain: self.required_bool(fields, "is_captain", path)?,
                })
            }
            unsupported => Err(self.error(
                path,
                EntityLootCompileErrorKind::UnsupportedEntityPredicate {
                    predicate: unsupported.to_string(),
                },
            )),
        }
    }

    fn int_range(&self, value: &Value, path: &str) -> Result<IntRange, EntityLootCompileError> {
        if value.is_number() {
            let exact = self.i32(value, path)?;
            return Ok(IntRange {
                min: exact,
                max: exact,
            });
        }
        let fields = self.object(value, path)?;
        self.check_fields(fields, &["min", "max"], path)?;
        let min = fields.get("min").map_or(Ok(i32::MIN), |value| {
            self.i32(value, &format!("{path}.min"))
        })?;
        let max = fields.get("max").map_or(Ok(i32::MAX), |value| {
            self.i32(value, &format!("{path}.max"))
        })?;
        if min > max {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::InvalidValue {
                    message: format!("integer range minimum {min} exceeds maximum {max}"),
                },
            ));
        }
        Ok(IntRange { min, max })
    }

    fn function_list(
        &mut self,
        value: Option<&Value>,
        path: &str,
        depth: usize,
    ) -> Result<Vec<LootFunction>, EntityLootCompileError> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        self.array(value, path)?
            .iter()
            .enumerate()
            .map(|(index, function)| self.function(function, &format!("{path}[{index}]"), depth))
            .collect()
    }

    fn function(
        &mut self,
        value: &Value,
        path: &str,
        depth: usize,
    ) -> Result<LootFunction, EntityLootCompileError> {
        self.check_depth(path, depth)?;
        let fields = self.object(value, path)?;
        let function = self.required_string(fields, "function", path)?.to_string();
        self.function_families.insert(function.clone());
        let conditions = |compiler: &mut Self| {
            compiler.condition_list(
                fields.get("conditions"),
                &format!("{path}.conditions"),
                depth + 1,
            )
        };
        match function.as_str() {
            "minecraft:set_count" => {
                self.check_fields(fields, &["function", "conditions", "count", "add"], path)?;
                let conditions = conditions(self)?;
                let count = self.number_provider(
                    self.required(fields, "count", path)?,
                    &format!("{path}.count"),
                )?;
                let add = fields
                    .get("add")
                    .map_or(Ok(false), |value| self.bool(value, &format!("{path}.add")))?;
                Ok(LootFunction::SetCount {
                    conditions,
                    count,
                    add,
                })
            }
            "minecraft:enchanted_count_increase" => {
                self.check_fields(
                    fields,
                    &["function", "conditions", "enchantment", "count", "limit"],
                    path,
                )?;
                let conditions = conditions(self)?;
                let enchantment = self.identifier(
                    self.required(fields, "enchantment", path)?,
                    &format!("{path}.enchantment"),
                )?;
                let count = self.number_provider(
                    self.required(fields, "count", path)?,
                    &format!("{path}.count"),
                )?;
                let limit = fields
                    .get("limit")
                    .map(|value| self.i32(value, &format!("{path}.limit")))
                    .transpose()?;
                if limit.is_some_and(|limit| limit < 0) {
                    return Err(self.error(
                        &format!("{path}.limit"),
                        EntityLootCompileErrorKind::InvalidValue {
                            message: "enchanted count limit must be non-negative".to_string(),
                        },
                    ));
                }
                Ok(LootFunction::EnchantedCountIncrease {
                    conditions,
                    enchantment,
                    count,
                    limit,
                })
            }
            "minecraft:furnace_smelt" => {
                self.check_fields(fields, &["function", "conditions", "use_input_count"], path)?;
                let conditions = conditions(self)?;
                let use_input_count = fields.get("use_input_count").map_or(Ok(true), |value| {
                    self.bool(value, &format!("{path}.use_input_count"))
                })?;
                Ok(LootFunction::FurnaceSmelt {
                    conditions,
                    use_input_count,
                })
            }
            "minecraft:set_potion" => {
                self.check_fields(fields, &["function", "conditions", "id"], path)?;
                let conditions = conditions(self)?;
                let potion =
                    self.identifier(self.required(fields, "id", path)?, &format!("{path}.id"))?;
                Ok(LootFunction::SetPotion { conditions, potion })
            }
            "minecraft:set_ominous_bottle_amplifier" => {
                self.check_fields(fields, &["function", "conditions", "amplifier"], path)?;
                let conditions = conditions(self)?;
                let amplifier = self.number_provider(
                    self.required(fields, "amplifier", path)?,
                    &format!("{path}.amplifier"),
                )?;
                Ok(LootFunction::SetOminousBottleAmplifier {
                    conditions,
                    amplifier,
                })
            }
            unsupported => Err(self.error(
                path,
                EntityLootCompileErrorKind::UnsupportedFunction {
                    function: unsupported.to_string(),
                },
            )),
        }
    }

    fn number_provider(
        &mut self,
        value: &Value,
        path: &str,
    ) -> Result<NumberProvider, EntityLootCompileError> {
        if value.is_number() {
            self.number_provider_families
                .insert("minecraft:constant".to_string());
            return self
                .provider_bound(value, path)
                .map(NumberProvider::Constant);
        }
        let fields = self.object(value, path)?;
        let provider = self.required_string(fields, "type", path)?.to_string();
        self.number_provider_families.insert(provider.clone());
        match provider.as_str() {
            "minecraft:constant" => {
                self.check_fields(fields, &["type", "value"], path)?;
                Ok(NumberProvider::Constant(self.provider_bound(
                    self.required(fields, "value", path)?,
                    &format!("{path}.value"),
                )?))
            }
            "minecraft:uniform" => {
                self.check_fields(fields, &["type", "min", "max"], path)?;
                let min = self
                    .provider_bound(self.required(fields, "min", path)?, &format!("{path}.min"))?;
                let max = self
                    .provider_bound(self.required(fields, "max", path)?, &format!("{path}.max"))?;
                if min > max {
                    return Err(self.error(
                        path,
                        EntityLootCompileErrorKind::InvalidValue {
                            message: format!("uniform minimum {min} exceeds maximum {max}"),
                        },
                    ));
                }
                Ok(NumberProvider::Uniform { min, max })
            }
            unsupported => Err(self.error(
                path,
                EntityLootCompileErrorKind::UnsupportedNumberProvider {
                    provider: unsupported.to_string(),
                },
            )),
        }
    }

    fn check_depth(&self, path: &str, depth: usize) -> Result<(), EntityLootCompileError> {
        if depth > MAX_COMPILE_NESTING_DEPTH {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::LimitExceeded {
                    limit: EntityLootLimit::CompileNesting,
                    actual: depth as u64,
                },
            ));
        }
        Ok(())
    }

    fn provider_bound(&self, value: &Value, path: &str) -> Result<f64, EntityLootCompileError> {
        let number = self.finite_f64(value, path)?;
        if number < f64::from(i32::MIN) || number > f64::from(i32::MAX) {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::NumericOverflow {
                    value: number.to_string(),
                },
            ));
        }
        Ok(number)
    }

    fn probability(&self, value: &Value, path: &str) -> Result<f64, EntityLootCompileError> {
        let probability = self.finite_f64(value, path)?;
        if !(0.0..=1.0).contains(&probability) {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::InvalidValue {
                    message: format!("probability {probability} is outside 0..=1"),
                },
            ));
        }
        Ok(probability)
    }

    fn tagged_identifier(
        &self,
        value: &Value,
        path: &str,
    ) -> Result<TaggedIdentifier, EntityLootCompileError> {
        let raw = value.as_str().ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "an identifier string",
                },
            )
        })?;
        let (is_tag, raw_id) = raw
            .strip_prefix('#')
            .map_or((false, raw), |identifier| (true, identifier));
        let identifier = Identifier::parse(raw_id.to_string()).map_err(|_| {
            self.error(
                path,
                EntityLootCompileErrorKind::InvalidIdentifier {
                    value: raw.to_string(),
                },
            )
        })?;
        Ok(if is_tag {
            TaggedIdentifier::Tag(identifier)
        } else {
            TaggedIdentifier::Exact(identifier)
        })
    }

    fn identifier(&self, value: &Value, path: &str) -> Result<Identifier, EntityLootCompileError> {
        let raw = value.as_str().ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "an identifier string",
                },
            )
        })?;
        Identifier::parse(raw.to_string()).map_err(|_| {
            self.error(
                path,
                EntityLootCompileErrorKind::InvalidIdentifier {
                    value: raw.to_string(),
                },
            )
        })
    }

    fn i32(&self, value: &Value, path: &str) -> Result<i32, EntityLootCompileError> {
        let Some(number) = value.as_number() else {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "an integer",
                },
            ));
        };
        if let Some(number) = number.as_i64() {
            return i32::try_from(number).map_err(|_| {
                self.error(
                    path,
                    EntityLootCompileErrorKind::NumericOverflow {
                        value: number.to_string(),
                    },
                )
            });
        }
        if let Some(number) = number.as_u64() {
            return i32::try_from(number).map_err(|_| {
                self.error(
                    path,
                    EntityLootCompileErrorKind::NumericOverflow {
                        value: number.to_string(),
                    },
                )
            });
        }
        let number = number.as_f64().ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "a finite integer",
                },
            )
        })?;
        if !number.is_finite() || number.fract() != 0.0 {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "a finite integer",
                },
            ));
        }
        if number < f64::from(i32::MIN) || number > f64::from(i32::MAX) {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::NumericOverflow {
                    value: number.to_string(),
                },
            ));
        }
        Ok(number as i32)
    }

    fn finite_f64(&self, value: &Value, path: &str) -> Result<f64, EntityLootCompileError> {
        let number = value.as_f64().ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "a finite number",
                },
            )
        })?;
        if !number.is_finite() {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "a finite number",
                },
            ));
        }
        Ok(number)
    }

    fn required<'a>(
        &self,
        fields: &'a Map<String, Value>,
        field: &str,
        path: &str,
    ) -> Result<&'a Value, EntityLootCompileError> {
        fields.get(field).ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::MissingField {
                    field: field.to_string(),
                },
            )
        })
    }

    fn required_string<'a>(
        &self,
        fields: &'a Map<String, Value>,
        field: &str,
        path: &str,
    ) -> Result<&'a str, EntityLootCompileError> {
        self.required(fields, field, path)?.as_str().ok_or_else(|| {
            self.error(
                &format!("{path}.{field}"),
                EntityLootCompileErrorKind::Expected {
                    expected: "a string",
                },
            )
        })
    }

    fn required_bool(
        &self,
        fields: &Map<String, Value>,
        field: &str,
        path: &str,
    ) -> Result<bool, EntityLootCompileError> {
        self.bool(
            self.required(fields, field, path)?,
            &format!("{path}.{field}"),
        )
    }

    fn bool(&self, value: &Value, path: &str) -> Result<bool, EntityLootCompileError> {
        value.as_bool().ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "a boolean",
                },
            )
        })
    }

    fn object<'a>(
        &self,
        value: &'a Value,
        path: &str,
    ) -> Result<&'a Map<String, Value>, EntityLootCompileError> {
        value.as_object().ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "an object",
                },
            )
        })
    }

    fn array<'a>(
        &self,
        value: &'a Value,
        path: &str,
    ) -> Result<&'a [Value], EntityLootCompileError> {
        value.as_array().map(Vec::as_slice).ok_or_else(|| {
            self.error(
                path,
                EntityLootCompileErrorKind::Expected {
                    expected: "an array",
                },
            )
        })
    }

    fn check_fields(
        &self,
        fields: &Map<String, Value>,
        allowed: &[&str],
        path: &str,
    ) -> Result<(), EntityLootCompileError> {
        if let Some(field) = fields
            .keys()
            .filter(|field| !allowed.contains(&field.as_str()))
            .min()
        {
            return Err(self.error(
                path,
                EntityLootCompileErrorKind::UnsupportedField {
                    field: field.clone(),
                },
            ));
        }
        Ok(())
    }

    fn error(&self, path: &str, kind: EntityLootCompileErrorKind) -> EntityLootCompileError {
        EntityLootCompileError {
            table: self.table.clone(),
            path: path.to_string(),
            kind,
        }
    }
}
