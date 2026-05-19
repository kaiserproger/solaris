use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::Compression as GzipCompression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use mc_nbt::{ListTag, Tag, tag_type};
use thiserror::Error;

use super::*;

const PLAYERDATA_DIR: &str = "playerdata";
const SOLARIS_DIR: &str = "solaris";
const ENTITIES_FILE: &str = "entities.dat";
const WORLD_FILE: &str = "world.dat";
const DAMAGE_COMPONENT: &str = "minecraft:damage";
static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorldPersistedMetadata {
    pub(crate) world_time: u64,
    pub(crate) world_identity: String,
}

#[derive(Debug, Clone)]
pub(super) struct XpState {
    pub(super) level: i32,
    pub(super) progress: f32,
    pub(super) total: i32,
    pub(super) seed: i32,
}

impl Default for XpState {
    fn default() -> Self {
        Self {
            level: 0,
            progress: 0.0,
            total: 0,
            seed: 0,
        }
    }
}

impl XpState {
    pub(super) fn add_points(&mut self, points: i32) -> bool {
        if points <= 0 {
            return false;
        }
        self.total = self.total.saturating_add(points).max(0);
        self.level = self.total / 7;
        self.progress = (self.total % 7) as f32 / 7.0;
        true
    }

    pub(super) const fn as_packet(&self) -> ClientboundSetExperience {
        ClientboundSetExperience {
            experience_progress: self.progress,
            total_experience: self.total,
            experience_level: self.level,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SpawnState {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) z: i32,
    pub(super) angle: f32,
}

impl SpawnState {
    pub(super) fn from_pose(pose: PlayerPose) -> Self {
        Self {
            x: pose.x.floor() as i32,
            y: pose.y.floor() as i32,
            z: pose.z.floor() as i32,
            angle: pose.yaw,
        }
    }

    pub(super) fn pose(&self) -> PlayerPose {
        let mut pose = PlayerPose::new(
            f64::from(self.x) + 0.5,
            f64::from(self.y),
            f64::from(self.z) + 0.5,
        );
        pose.yaw = self.angle;
        pose
    }
}

#[derive(Debug, Clone)]
pub(super) struct InventorySlotExtras {
    item_id: u32,
    damage: Option<i32>,
    fields: Vec<(String, Tag)>,
}

#[derive(Debug, Clone)]
pub(crate) struct PlayerPersistedState {
    pub(super) pose: PlayerPose,
    pub(super) game_mode: GameMode,
    pub(super) survival: SurvivalState,
    pub(super) inventory: PlayerInventory,
    pub(super) selected_hotbar_slot: u8,
    pub(super) spawn: SpawnState,
    pub(super) xp: XpState,
    inventory_extras: [Option<InventorySlotExtras>; 46],
}

impl PlayerPersistedState {
    pub(super) fn new_default(spawn: PlayerPose) -> Self {
        Self {
            pose: spawn,
            game_mode: GameMode::Survival,
            survival: SurvivalState::FULL,
            inventory: PlayerInventory::empty(),
            selected_hotbar_slot: 0,
            spawn: SpawnState::from_pose(spawn),
            xp: XpState::default(),
            inventory_extras: std::array::from_fn(|_| None),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum PlayerPersistenceError {
    #[error("persistence I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("persistence NBT at {path}: {source}")]
    Nbt {
        path: PathBuf,
        source: mc_nbt::NbtError,
    },
    #[error("persistence root at {path} is not a compound")]
    RootNotCompound { path: PathBuf },
    #[error("playerdata item id is invalid: {0}")]
    InvalidItemId(String),
    #[error("playerdata item is not in registry: {0}")]
    UnknownItem(String),
}

pub(super) fn load_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    default: PlayerPersistedState,
) -> Result<Option<PlayerPersistedState>, PlayerPersistenceError> {
    let path = playerdata_path(world_root, uuid);
    if !path.is_file() {
        return Ok(None);
    }

    let (root_name, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let _ = root_name;

    let mut state = default;
    if let Some(pose) = read_pose(&fields) {
        state.pose = pose;
    }
    if let Some(game_mode) = int_field(&fields, "playerGameType") {
        state.game_mode = GameMode::from_id(game_mode);
    }
    if let Some(health) = float_field(&fields, "Health") {
        state.survival.health = health.clamp(0.0, SurvivalState::MAX_HEALTH);
    }
    if let Some(food) = int_field(&fields, "foodLevel") {
        state.survival.food = food.clamp(0, SurvivalState::MAX_FOOD);
    }
    if let Some(saturation) = float_field(&fields, "foodSaturationLevel") {
        state.survival.saturation = saturation.max(0.0);
    }
    if let Some(exhaustion) = float_field(&fields, "foodExhaustionLevel") {
        state.survival.exhaustion = exhaustion.max(0.0);
    }
    if let Some(slot) = int_field(&fields, "SelectedItemSlot") {
        state.selected_hotbar_slot = slot.clamp(0, 8) as u8;
    }
    if let Some(spawn) = read_spawn(&fields) {
        state.spawn = spawn;
    }
    state.xp.level = int_field(&fields, "XpLevel")
        .unwrap_or(state.xp.level)
        .max(0);
    state.xp.progress = float_field(&fields, "XpP")
        .unwrap_or(state.xp.progress)
        .clamp(0.0, 1.0);
    state.xp.total = int_field(&fields, "XpTotal")
        .unwrap_or(state.xp.total)
        .max(0);
    state.xp.seed = int_field(&fields, "XpSeed").unwrap_or(state.xp.seed);

    if let Some(Tag::List(list)) = field(&fields, "Inventory") {
        for element in &list.elements {
            let Tag::Compound(item_fields) = element else {
                continue;
            };
            let Some(slot) = slot_field(item_fields) else {
                continue;
            };
            if slot >= state.inventory.slots.len() {
                continue;
            }
            let Some(item_name) = string_field(item_fields, "id") else {
                continue;
            };
            let parsed = mc_data::Identifier::parse(item_name.to_string())
                .map_err(|_| PlayerPersistenceError::InvalidItemId(item_name.to_string()))?;
            let item_id = items
                .id_of(&parsed)
                .ok_or_else(|| PlayerPersistenceError::UnknownItem(item_name.to_string()))?;
            let count = int_field(item_fields, "count").unwrap_or(1).max(0);
            let damage = damage_component(item_fields);
            state.inventory.slots[slot] = ItemStack {
                count,
                item_id,
                damage,
            };
            let extras = item_fields
                .iter()
                .filter(|(name, _)| !is_modelled_item_key(name))
                .cloned()
                .collect::<Vec<_>>();
            if !extras.is_empty() {
                state.inventory_extras[slot] = Some(InventorySlotExtras {
                    item_id,
                    damage,
                    fields: extras,
                });
            }
        }
    }

    Ok(Some(state))
}

pub(crate) fn save_player_state(
    world_root: &Path,
    uuid: uuid::Uuid,
    items: &ItemRegistry,
    state: &PlayerPersistedState,
) -> Result<(), PlayerPersistenceError> {
    let path = playerdata_path(world_root, uuid);
    let (root_name, root) = if path.is_file() {
        read_player_root(&path)?
    } else {
        (String::new(), Tag::Compound(Vec::new()))
    };
    let Tag::Compound(mut fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };

    set_field(&mut fields, "Pos", pose_position_tag(state.pose));
    set_field(&mut fields, "Rotation", pose_rotation_tag(state.pose));
    set_field(
        &mut fields,
        "OnGround",
        Tag::Byte(i8::from(state.pose.flags.on_ground)),
    );
    set_field(
        &mut fields,
        "playerGameType",
        Tag::Int(state.game_mode.id()),
    );
    set_field(&mut fields, "Health", Tag::Float(state.survival.health));
    set_field(&mut fields, "foodLevel", Tag::Int(state.survival.food));
    set_field(
        &mut fields,
        "foodSaturationLevel",
        Tag::Float(state.survival.saturation),
    );
    set_field(
        &mut fields,
        "foodExhaustionLevel",
        Tag::Float(state.survival.exhaustion),
    );
    set_field(
        &mut fields,
        "SelectedItemSlot",
        Tag::Int(i32::from(state.selected_hotbar_slot)),
    );
    set_field(&mut fields, "SpawnX", Tag::Int(state.spawn.x));
    set_field(&mut fields, "SpawnY", Tag::Int(state.spawn.y));
    set_field(&mut fields, "SpawnZ", Tag::Int(state.spawn.z));
    set_field(&mut fields, "SpawnAngle", Tag::Float(state.spawn.angle));
    set_field(&mut fields, "XpLevel", Tag::Int(state.xp.level));
    set_field(&mut fields, "XpP", Tag::Float(state.xp.progress));
    set_field(&mut fields, "XpTotal", Tag::Int(state.xp.total));
    set_field(&mut fields, "XpSeed", Tag::Int(state.xp.seed));
    set_field(&mut fields, "Inventory", inventory_tag(items, state)?);

    write_player_root(&path, &root_name, &Tag::Compound(fields))
}

pub(crate) fn load_persisted_entities(
    world_root: &Path,
    items: &ItemRegistry,
    entity_types: &EntityTypeRegistry,
) -> Result<Vec<EntitySnapshot>, PlayerPersistenceError> {
    let path = entities_path(world_root);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let (_, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    let Some(Tag::List(list)) = field(&fields, "Entities") else {
        return Ok(Vec::new());
    };
    let mut entities = Vec::new();
    for element in &list.elements {
        let Tag::Compound(fields) = element else {
            continue;
        };
        let Some(type_name) = string_field(fields, "id") else {
            continue;
        };
        let parsed = mc_data::Identifier::parse(type_name.to_string())
            .map_err(|_| PlayerPersistenceError::InvalidItemId(type_name.to_string()))?;
        let type_id = entity_types
            .id_of(&parsed)
            .ok_or_else(|| PlayerPersistenceError::UnknownItem(type_name.to_string()))?
            as i32;
        let pos = double_list::<3>(
            field(fields, "Pos").unwrap_or(&Tag::List(ListTag::empty())),
            3,
        )
        .unwrap_or([0.0, 0.0, 0.0]);
        let motion = double_list::<3>(
            field(fields, "Motion").unwrap_or(&Tag::List(ListTag::empty())),
            3,
        )
        .unwrap_or([0.0, 0.0, 0.0]);
        let rotation = float_list::<2>(
            field(fields, "Rotation").unwrap_or(&Tag::List(ListTag::empty())),
            2,
        )
        .unwrap_or([0.0, 0.0]);
        let item_stack = if let Some(Tag::Compound(item)) = field(fields, "Item") {
            read_entity_item_stack(item, items)?
        } else {
            None
        };
        let experience_value = int_field(fields, "Value").filter(|value| *value > 0);
        let block_state =
            int_field(fields, "BlockState").and_then(|value| u32::try_from(value).ok());
        let mut attributes = attributes_from_entity_facts(&parsed, type_id as u32);
        let health = float_field(fields, "Health").unwrap_or(20.0).max(0.0);
        attributes.set_base(AttributeKind::MaxHealth, health.max(1.0) as f64);
        let id = EntityId(int_field(fields, "SolarisEntityId").unwrap_or(0).max(0));
        let uuid = uuid_field(fields).unwrap_or_else(|| {
            let id = int_field(fields, "SolarisEntityId").unwrap_or(0) as u32 as u128;
            uuid::Uuid::from_u128(0x5f1a_0000_0000_0000_0000_0000_0000_0000 | id)
        });
        let aquatic = persisted_entity_type_is_aquatic(type_name);
        entities.push(EntitySnapshot {
            id,
            uuid,
            type_id,
            type_name: type_name.to_string(),
            position: Vec3::new(pos[0], pos[1], pos[2]),
            rotation: mc_entity::Rotation {
                yaw: rotation[0],
                pitch: rotation[1],
                head_yaw: rotation[0],
            },
            velocity: Vec3::new(motion[0], motion[1], motion[2]),
            on_ground: byte_field(fields, "OnGround").unwrap_or(0) != 0 && !aquatic,
            item_stack,
            experience_value,
            block_state,
            lifecycle: EntityLifecycle::Alive,
            health,
            attributes,
            goal: if type_name == "minecraft:item"
                || type_name == "minecraft:falling_block"
                || experience_value.is_some()
            {
                GoalState::Idle
            } else if aquatic {
                GoalState::AquaticWander {
                    speed: 0.72,
                    vertical_speed: 0.18,
                    period_ticks: 45,
                }
            } else {
                GoalState::Wander {
                    speed: 0.8,
                    period_ticks: 80,
                }
            },
        });
    }
    Ok(entities)
}

fn persisted_entity_type_is_aquatic(type_name: &str) -> bool {
    matches!(
        type_name,
        "minecraft:cod"
            | "minecraft:salmon"
            | "minecraft:tropical_fish"
            | "minecraft:pufferfish"
            | "minecraft:squid"
            | "minecraft:glow_squid"
            | "minecraft:dolphin"
            | "minecraft:axolotl"
            | "minecraft:turtle"
    )
}

fn attributes_from_entity_facts(
    id: &mc_data::Identifier,
    protocol_id: u32,
) -> mc_entity::AttributeSet {
    let facts = mc_data::entity_types::fallback_entity_type_facts(id.clone(), protocol_id);
    let mut attributes = mc_entity::AttributeSet::vanilla_mob_defaults();
    if let Some(value) = facts.attributes.max_health {
        attributes.set_base(AttributeKind::MaxHealth, value);
    }
    if let Some(value) = facts.attributes.movement_speed {
        attributes.set_base(AttributeKind::MovementSpeed, value);
    }
    if let Some(value) = facts.attributes.follow_range {
        attributes.set_base(AttributeKind::FollowRange, value);
    }
    if let Some(value) = facts.attributes.attack_damage {
        attributes.set_base(AttributeKind::AttackDamage, value);
    }
    attributes
}

pub(crate) fn save_persisted_entities(
    world_root: &Path,
    items: &ItemRegistry,
    entities: &[EntitySnapshot],
) -> Result<(), PlayerPersistenceError> {
    let path = entities_path(world_root);
    let mut elements = Vec::new();
    for entity in entities
        .iter()
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
    {
        elements.push(entity_tag(items, entity)?);
    }
    let root = Tag::Compound(vec![(
        "Entities".into(),
        Tag::List(ListTag {
            element_type: if elements.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements,
        }),
    )]);
    write_player_root(&path, "", &root)
}

pub(crate) fn load_world_metadata(
    world_root: &Path,
) -> Result<Option<WorldPersistedMetadata>, PlayerPersistenceError> {
    let path = world_metadata_path(world_root);
    if !path.is_file() {
        return Ok(None);
    }
    let (_, root) = read_player_root(&path)?;
    let Tag::Compound(fields) = root else {
        return Err(PlayerPersistenceError::RootNotCompound { path });
    };
    Ok(Some(WorldPersistedMetadata {
        world_time: long_field(&fields, "SolarisWorldTime").unwrap_or(0) as u64,
        world_identity: string_field(&fields, "SolarisWorldIdentity")
            .unwrap_or_default()
            .to_string(),
    }))
}

pub(crate) fn save_world_metadata(
    world_root: &Path,
    metadata: &WorldPersistedMetadata,
) -> Result<(), PlayerPersistenceError> {
    let path = world_metadata_path(world_root);
    let root = Tag::Compound(vec![
        (
            "SolarisWorldTime".into(),
            Tag::Long(metadata.world_time as i64),
        ),
        (
            "SolarisWorldIdentity".into(),
            Tag::String(metadata.world_identity.clone()),
        ),
    ]);
    write_player_root(&path, "", &root)
}

pub(crate) fn world_identity(world_root: &Path) -> String {
    world_root.to_string_lossy().into_owned()
}

fn world_metadata_path(world_root: &Path) -> PathBuf {
    world_root.join(SOLARIS_DIR).join(WORLD_FILE)
}

fn entities_path(world_root: &Path) -> PathBuf {
    world_root.join(SOLARIS_DIR).join(ENTITIES_FILE)
}

fn entity_tag(
    items: &ItemRegistry,
    entity: &EntitySnapshot,
) -> Result<Tag, PlayerPersistenceError> {
    let mut fields = vec![
        ("id".into(), Tag::String(entity.type_name.clone())),
        ("SolarisEntityId".into(), Tag::Int(entity.id.0)),
        ("UUID".into(), Tag::IntArray(uuid_to_int_array(entity.uuid))),
        ("Pos".into(), vec3_double_list(entity.position)),
        ("Motion".into(), vec3_double_list(entity.velocity)),
        (
            "Rotation".into(),
            Tag::List(ListTag {
                element_type: tag_type::FLOAT,
                elements: vec![
                    Tag::Float(entity.rotation.yaw),
                    Tag::Float(entity.rotation.pitch),
                ],
            }),
        ),
        ("OnGround".into(), Tag::Byte(i8::from(entity.on_ground))),
        ("Health".into(), Tag::Float(entity.health)),
        ("Age".into(), Tag::Int(0)),
    ];
    if let Some(stack) = entity.item_stack {
        let item = entity_item_stack_tag(items, stack)?;
        fields.push(("Item".into(), item));
        fields.push(("PickupDelay".into(), Tag::Short(0)));
    }
    if let Some(value) = entity.experience_value {
        fields.push(("Value".into(), Tag::Int(value.max(0))));
    }
    if let Some(block_state) = entity.block_state {
        fields.push(("BlockState".into(), Tag::Int(block_state as i32)));
    }
    Ok(Tag::Compound(fields))
}

fn entity_item_stack_tag(
    items: &ItemRegistry,
    stack: EntityItemStack,
) -> Result<Tag, PlayerPersistenceError> {
    let name = items
        .name_of(stack.item_id)
        .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
    Ok(Tag::Compound(vec![
        ("id".into(), Tag::String(name.as_str().to_string())),
        ("count".into(), Tag::Int(stack.count)),
    ]))
}

fn read_entity_item_stack(
    fields: &[(String, Tag)],
    items: &ItemRegistry,
) -> Result<Option<EntityItemStack>, PlayerPersistenceError> {
    let Some(item_name) = string_field(fields, "id") else {
        return Ok(None);
    };
    let parsed = mc_data::Identifier::parse(item_name.to_string())
        .map_err(|_| PlayerPersistenceError::InvalidItemId(item_name.to_string()))?;
    let Some(item_id) = items.id_of(&parsed) else {
        return Err(PlayerPersistenceError::UnknownItem(item_name.to_string()));
    };
    let count = int_field(fields, "count").unwrap_or(1).max(0);
    Ok((count > 0).then_some(EntityItemStack::new(item_id, count)))
}

fn vec3_double_list(vec: Vec3) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::DOUBLE,
        elements: vec![Tag::Double(vec.x), Tag::Double(vec.y), Tag::Double(vec.z)],
    })
}

fn uuid_to_int_array(uuid: uuid::Uuid) -> Vec<i32> {
    let bytes = uuid.as_u128().to_be_bytes();
    bytes
        .chunks_exact(4)
        .map(|chunk| i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn uuid_field(fields: &[(String, Tag)]) -> Option<uuid::Uuid> {
    let Tag::IntArray(values) = field(fields, "UUID")? else {
        return None;
    };
    if values.len() != 4 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (idx, value) in values.iter().enumerate() {
        bytes[idx * 4..idx * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    Some(uuid::Uuid::from_u128(u128::from_be_bytes(bytes)))
}

fn playerdata_path(world_root: &Path, uuid: uuid::Uuid) -> PathBuf {
    world_root
        .join(PLAYERDATA_DIR)
        .join(format!("{}.dat", uuid.hyphenated()))
}

fn read_player_root(path: &Path) -> Result<(String, Tag), PlayerPersistenceError> {
    let file = File::open(path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|source| PlayerPersistenceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut slice = bytes.as_slice();
    mc_nbt::read_named(&mut slice).map_err(|source| PlayerPersistenceError::Nbt {
        path: path.to_path_buf(),
        source,
    })
}

fn write_player_root(
    path: &Path,
    root_name: &str,
    root: &Tag,
) -> Result<(), PlayerPersistenceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| PlayerPersistenceError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let tmp_path = temporary_write_path(path);
    let file = File::create(&tmp_path).map_err(|source| PlayerPersistenceError::Io {
        path: tmp_path.clone(),
        source,
    })?;
    let mut encoder = GzEncoder::new(file, GzipCompression::default());
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, root_name, root).map_err(|source| {
        PlayerPersistenceError::Nbt {
            path: path.to_path_buf(),
            source,
        }
    })?;
    encoder
        .write_all(&bytes)
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    encoder
        .finish()
        .map_err(|source| PlayerPersistenceError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    std::fs::rename(&tmp_path, path).map_err(|source| PlayerPersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn temporary_write_path(path: &Path) -> PathBuf {
    let counter = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("persisted.dat");
    path.with_file_name(format!("{file_name}.{pid}.{counter}.tmp"))
}

fn read_pose(fields: &[(String, Tag)]) -> Option<PlayerPose> {
    let pos = double_list::<3>(field(fields, "Pos")?, 3)?;
    let rotation = float_list::<2>(field(fields, "Rotation")?, 2)?;
    let mut pose = PlayerPose::new(pos[0], pos[1], pos[2]);
    pose.yaw = rotation[0];
    pose.pitch = rotation[1];
    pose.flags = MovePlayerFlags::new(byte_field(fields, "OnGround").unwrap_or(0) != 0, false);
    Some(pose)
}

fn read_spawn(fields: &[(String, Tag)]) -> Option<SpawnState> {
    Some(SpawnState {
        x: int_field(fields, "SpawnX")?,
        y: int_field(fields, "SpawnY")?,
        z: int_field(fields, "SpawnZ")?,
        angle: float_field(fields, "SpawnAngle").unwrap_or(0.0),
    })
}

fn pose_position_tag(pose: PlayerPose) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::DOUBLE,
        elements: vec![
            Tag::Double(pose.x),
            Tag::Double(pose.y),
            Tag::Double(pose.z),
        ],
    })
}

fn pose_rotation_tag(pose: PlayerPose) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::FLOAT,
        elements: vec![Tag::Float(pose.yaw), Tag::Float(pose.pitch)],
    })
}

fn inventory_tag(
    items: &ItemRegistry,
    state: &PlayerPersistedState,
) -> Result<Tag, PlayerPersistenceError> {
    let mut elements = Vec::new();
    for (slot, stack) in state.inventory.slots.iter().enumerate() {
        if stack.is_empty() {
            continue;
        }
        let name = items
            .name_of(stack.item_id)
            .ok_or_else(|| PlayerPersistenceError::UnknownItem(stack.item_id.to_string()))?;
        let mut fields = state.inventory_extras[slot]
            .as_ref()
            .filter(|extras| extras.item_id == stack.item_id && extras.damage == stack.damage)
            .map(|extras| extras.fields.clone())
            .unwrap_or_default();
        set_field(&mut fields, "Slot", Tag::Byte(slot as i8));
        set_field(&mut fields, "id", Tag::String(name.as_str().to_string()));
        set_field(&mut fields, "count", Tag::Int(stack.count));
        if let Some(damage) = stack.damage {
            set_damage_component(&mut fields, damage);
        }
        elements.push(Tag::Compound(fields));
    }
    Ok(Tag::List(ListTag {
        element_type: if elements.is_empty() {
            tag_type::END
        } else {
            tag_type::COMPOUND
        },
        elements,
    }))
}

fn damage_component(fields: &[(String, Tag)]) -> Option<i32> {
    let Tag::Compound(components) = field(fields, "components")? else {
        return None;
    };
    int_field(components, DAMAGE_COMPONENT)
}

fn set_damage_component(fields: &mut Vec<(String, Tag)>, damage: i32) {
    let components = field_mut(fields, "components").and_then(|tag| match tag {
        Tag::Compound(fields) => Some(fields),
        _ => None,
    });
    if let Some(components) = components {
        set_field(components, DAMAGE_COMPONENT, Tag::Int(damage));
    } else {
        set_field(
            fields,
            "components",
            Tag::Compound(vec![(DAMAGE_COMPONENT.into(), Tag::Int(damage))]),
        );
    }
}

fn is_modelled_item_key(name: &str) -> bool {
    matches!(name, "Slot" | "id" | "count")
}

fn slot_field(fields: &[(String, Tag)]) -> Option<usize> {
    match field(fields, "Slot")? {
        Tag::Byte(value) => Some(*value as u8 as usize),
        Tag::Short(value) => usize::try_from(*value).ok(),
        Tag::Int(value) => usize::try_from(*value).ok(),
        _ => None,
    }
}

fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

fn field_mut<'a>(fields: &'a mut [(String, Tag)], name: &str) -> Option<&'a mut Tag> {
    fields
        .iter_mut()
        .find(|(key, _)| key == name)
        .map(|(_, tag)| tag)
}

fn set_field(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    if let Some((_, existing)) = fields.iter_mut().find(|(key, _)| key == name) {
        *existing = value;
    } else {
        fields.push((name.into(), value));
    }
}

fn int_field(fields: &[(String, Tag)], name: &str) -> Option<i32> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i32::from(*value)),
        Tag::Short(value) => Some(i32::from(*value)),
        Tag::Int(value) => Some(*value),
        _ => None,
    }
}

fn long_field(fields: &[(String, Tag)], name: &str) -> Option<i64> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(i64::from(*value)),
        Tag::Short(value) => Some(i64::from(*value)),
        Tag::Int(value) => Some(i64::from(*value)),
        Tag::Long(value) => Some(*value),
        _ => None,
    }
}

fn byte_field(fields: &[(String, Tag)], name: &str) -> Option<i8> {
    match field(fields, name)? {
        Tag::Byte(value) => Some(*value),
        _ => None,
    }
}

fn float_field(fields: &[(String, Tag)], name: &str) -> Option<f32> {
    match field(fields, name)? {
        Tag::Float(value) => Some(*value),
        Tag::Double(value) => Some(*value as f32),
        _ => None,
    }
}

fn string_field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a str> {
    match field(fields, name)? {
        Tag::String(value) => Some(value),
        _ => None,
    }
}

fn double_list<const N: usize>(tag: &Tag, len: usize) -> Option<[f64; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != len {
        return None;
    }
    let mut values = [0.0; N];
    for (idx, element) in list.elements.iter().enumerate() {
        values[idx] = match element {
            Tag::Double(value) => *value,
            Tag::Float(value) => f64::from(*value),
            _ => return None,
        };
    }
    Some(values)
}

fn float_list<const N: usize>(tag: &Tag, len: usize) -> Option<[f32; N]> {
    let Tag::List(list) = tag else {
        return None;
    };
    if list.elements.len() != len {
        return None;
    }
    let mut values = [0.0; N];
    for (idx, element) in list.elements.iter().enumerate() {
        values[idx] = match element {
            Tag::Float(value) => *value,
            Tag::Double(value) => *value as f32,
            _ => return None,
        };
    }
    Some(values)
}

impl fmt::Display for PlayerPersistedState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pos=({:.2},{:.2},{:.2}) mode={:?} health={:.1} food={} selected_slot={}",
            self.pose.x,
            self.pose.y,
            self.pose.z,
            self.game_mode,
            self.survival.health,
            self.survival.food,
            self.selected_hotbar_slot,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_nbt::Tag;

    fn items() -> ItemRegistry {
        let reports = vec![
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap(),
                protocol_id: 2,
            },
        ];
        ItemRegistry::from_report(&reports)
    }

    fn entity_types() -> EntityTypeRegistry {
        let reports = vec![
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:item").unwrap(),
                protocol_id: 1,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:cow").unwrap(),
                protocol_id: 2,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:cod").unwrap(),
                protocol_id: 3,
            },
            mc_data::entity_types::EntityTypeReport {
                id: mc_data::Identifier::parse("minecraft:falling_block").unwrap(),
                protocol_id: 4,
            },
        ];
        EntityTypeRegistry::from_report(&reports)
    }

    #[test]
    fn player_state_round_trips_through_real_playerdata_path() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let uuid = uuid::Uuid::from_u128(0x1234);
        let mut state = PlayerPersistedState::new_default(PlayerPose::new(1.5, 65.0, -2.5));
        state.pose.yaw = 90.0;
        state.pose.pitch = 12.0;
        state.game_mode = GameMode::Adventure;
        state.survival.health = 7.5;
        state.survival.food = 9;
        state.survival.saturation = 2.5;
        state.selected_hotbar_slot = 3;
        state.inventory.set_hotbar(3, ItemStack::new(1, 17));
        state.inventory.slots[9] = ItemStack::new(2, 1).with_damage(11);

        save_player_state(tmp.path(), uuid, &items, &state).unwrap();

        let loaded = load_player_state(
            tmp.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
        )
        .unwrap()
        .unwrap();

        assert_eq!(loaded.pose.x, 1.5);
        assert_eq!(loaded.pose.z, -2.5);
        assert_eq!(loaded.pose.yaw, 90.0);
        assert_eq!(loaded.game_mode, GameMode::Adventure);
        assert_eq!(loaded.survival.health, 7.5);
        assert_eq!(loaded.survival.food, 9);
        assert_eq!(loaded.selected_hotbar_slot, 3);
        assert_eq!(loaded.inventory.held(3), &ItemStack::new(1, 17));
        assert_eq!(
            loaded.inventory.slots[9],
            ItemStack::new(2, 1).with_damage(11)
        );
    }

    #[test]
    fn player_state_preserves_unknown_root_and_item_payloads() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let uuid = uuid::Uuid::from_u128(0x5678);
        let path = playerdata_path(tmp.path(), uuid);
        let root = Tag::Compound(vec![
            ("SolarisUnknownRoot".into(), Tag::String("keep".into())),
            (
                "Inventory".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(vec![
                        ("Slot".into(), Tag::Byte(36)),
                        ("id".into(), Tag::String("minecraft:stone".into())),
                        ("count".into(), Tag::Int(4)),
                        ("SolarisUnknownItem".into(), Tag::Long(99)),
                    ])],
                }),
            ),
        ]);
        write_player_root(&path, "", &root).unwrap();

        let mut loaded = load_player_state(
            tmp.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5)),
        )
        .unwrap()
        .unwrap();
        loaded.inventory.set_hotbar(0, ItemStack::new(1, 5));
        save_player_state(tmp.path(), uuid, &items, &loaded).unwrap();

        let (_, saved) = read_player_root(&path).unwrap();
        let Tag::Compound(fields) = saved else {
            panic!("root compound");
        };
        assert_eq!(string_field(&fields, "SolarisUnknownRoot"), Some("keep"));
        let Some(Tag::List(list)) = field(&fields, "Inventory") else {
            panic!("inventory list");
        };
        let Tag::Compound(slot) = &list.elements[0] else {
            panic!("inventory item");
        };
        assert_eq!(int_field(slot, "count"), Some(5));
        assert_eq!(field(slot, "SolarisUnknownItem"), Some(&Tag::Long(99)));
    }

    #[test]
    fn entities_round_trip_through_real_storage_path() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let entity_types = entity_types();
        let item = EntitySnapshot {
            id: EntityId(100),
            uuid: uuid::Uuid::from_u128(100),
            type_id: 1,
            type_name: "minecraft:item".into(),
            position: Vec3::new(1.0, 65.0, 2.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::new(0.1, 0.2, 0.3),
            on_ground: false,
            item_stack: Some(EntityItemStack::new(1, 3)),
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
        };
        let cow = EntitySnapshot {
            id: EntityId(101),
            uuid: uuid::Uuid::from_u128(101),
            type_id: 2,
            type_name: "minecraft:cow".into(),
            position: Vec3::new(-4.0, 64.0, 8.0),
            rotation: mc_entity::Rotation {
                yaw: 45.0,
                pitch: 0.0,
                head_yaw: 45.0,
            },
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 13.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Wander {
                speed: 0.8,
                period_ticks: 80,
            },
        };
        let falling_block = EntitySnapshot {
            id: EntityId(102),
            uuid: uuid::Uuid::from_u128(102),
            type_id: 4,
            type_name: "minecraft:falling_block".into(),
            position: Vec3::new(3.5, 70.0, 4.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: false,
            item_stack: None,
            experience_value: None,
            block_state: Some(1234),
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
        };

        save_persisted_entities(
            tmp.path(),
            &items,
            &[item.clone(), cow.clone(), falling_block.clone()],
        )
        .unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();

        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].id, item.id);
        assert_eq!(loaded[0].uuid, item.uuid);
        assert_eq!(loaded[0].position, item.position);
        assert_eq!(loaded[0].velocity, item.velocity);
        assert_eq!(loaded[0].item_stack, item.item_stack);
        assert_eq!(loaded[1].id, cow.id);
        assert_eq!(loaded[1].type_name, cow.type_name);
        assert_eq!(loaded[1].health, cow.health);
        assert_eq!(loaded[1].position, cow.position);
        assert_eq!(
            loaded[1].attributes.base(&AttributeKind::MovementSpeed),
            Some(0.2)
        );
        assert_eq!(loaded[2].type_name, falling_block.type_name);
        assert_eq!(loaded[2].block_state, falling_block.block_state);
        assert!(matches!(loaded[2].goal, GoalState::Idle));
    }

    #[test]
    fn restored_aquatic_entities_keep_aquatic_wander_goal() {
        let tmp = tempfile::tempdir().unwrap();
        let items = items();
        let entity_types = entity_types();
        let cod = EntitySnapshot {
            id: EntityId(102),
            uuid: uuid::Uuid::from_u128(102),
            type_id: 3,
            type_name: "minecraft:cod".into(),
            position: Vec3::new(1.0, 50.0, 2.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 3.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
        };

        save_persisted_entities(tmp.path(), &items, &[cod]).unwrap();
        let loaded = load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();

        assert!(matches!(loaded[0].goal, GoalState::AquaticWander { .. }));
        assert!(!loaded[0].on_ground);
    }

    #[test]
    fn concurrent_entity_saves_use_distinct_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let items = items();
        let entity_types = entity_types();
        let item = EntitySnapshot {
            id: EntityId(100),
            uuid: uuid::Uuid::from_u128(100),
            type_id: 1,
            type_name: "minecraft:item".into(),
            position: Vec3::new(1.0, 65.0, 2.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: false,
            item_stack: Some(EntityItemStack::new(1, 3)),
            experience_value: None,
            block_state: None,
            lifecycle: EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: GoalState::Idle,
        };

        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let root = &root;
                let items = &items;
                let item = &item;
                handles.push(scope.spawn(move || {
                    for _ in 0..25 {
                        save_persisted_entities(root, items, std::slice::from_ref(item)).unwrap();
                    }
                }));
            }
            for handle in handles {
                handle.join().unwrap();
            }
        });

        let loaded = load_persisted_entities(&root, &items, &entity_types).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, item.id);
    }

    #[test]
    fn xp_state_adds_points_and_maps_to_wire_packet() {
        let mut xp = XpState::default();

        assert!(!xp.add_points(0));
        assert!(xp.add_points(9));

        assert_eq!(xp.total, 9);
        assert_eq!(xp.level, 1);
        assert!((xp.progress - (2.0 / 7.0)).abs() < f32::EPSILON);
        assert_eq!(
            xp.as_packet(),
            ClientboundSetExperience {
                experience_progress: 2.0 / 7.0,
                total_experience: 9,
                experience_level: 1,
            }
        );
    }

    #[test]
    fn world_metadata_round_trips_through_real_storage_path() {
        let tmp = tempfile::tempdir().unwrap();
        let metadata = WorldPersistedMetadata {
            world_time: 12345,
            world_identity: world_identity(tmp.path()),
        };

        save_world_metadata(tmp.path(), &metadata).unwrap();
        let loaded = load_world_metadata(tmp.path()).unwrap().unwrap();

        assert_eq!(loaded, metadata);
    }
}
