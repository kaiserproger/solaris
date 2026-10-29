//! 26.1.2 (protocol 775) clients on the 26.3 server (ADR 0015).
//!
//! A per-session translator at the edge's encode/decode boundary: the server
//! encodes and decodes only its canonical protocol; for a legacy session every
//! clientbound frame is rewritten into the 26.1.2 ids/layouts just before
//! framing, and every serverbound frame into the canonical ones right after
//! reading. Ids come from both content caches' datagen reports
//! ([`tables`]); blocks, items and entities that 26.1.2 lacks are shown as the
//! nearest existing ones. Configuration sends the 26.1.2 registries and tags
//! ([`Profile::data`], [`Profile::tags`]) under the `minecraft:core 26.1.2`
//! known pack, so the client uses its built-in definitions.

mod chunk;
mod displays;
mod items;
mod play;
mod tables;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use mc_protocol::{Compression, RawFrame};
use parking_lot::Mutex;

pub use tables::Metadata;
use tables::{PacketNames, Reports, Tables};

/// The only legacy protocol served.
pub const PROTOCOL: i32 = 775;
/// Its release name (known pack version, status name).
pub const RELEASE: &str = "26.1.2";
/// Overrides where the 26.1.2 content cache is found.
pub const CACHE_ENV: &str = "SOLARIS_LEGACY_CONTENT_CACHE";

/// Where the 26.1.2 cache is expected next to the canonical one: the sibling
/// release directory of a per-release cache (`.../content/26.3` ->
/// `.../content/26.1.2`), else `<cache>-26.1.2` (`data/vanilla` ->
/// `data/vanilla-26.1.2`). [`CACHE_ENV`] overrides both.
#[must_use]
pub fn cache_root(canonical: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os(CACHE_ENV) {
        return PathBuf::from(path);
    }
    match (canonical.file_name(), canonical.parent()) {
        (Some(name), Some(parent)) if name == mc_protocol::TARGET_RELEASE => parent.join(RELEASE),
        _ => {
            let mut name = canonical.as_os_str().to_owned();
            name.push(format!("-{RELEASE}"));
            PathBuf::from(name)
        }
    }
}

/// Everything a 775 session needs, loaded once at startup.
pub struct Profile {
    /// 26.1.2 synced registries (Configuration `RegistryData` entry names).
    pub data: mc_data::VanillaData,
    /// 26.1.2 tags with 26.1.2 numeric ids (Configuration `UpdateTags`).
    pub tags: mc_data::tags::TagsData,
    tables: Tables,
}

impl Profile {
    /// Load the 26.1.2 cache at `legacy_root` and pair it with the canonical
    /// cache the server runs on.
    pub fn load(
        canonical_root: &Path,
        legacy_root: &Path,
        canonical: &mc_data::VanillaData,
    ) -> Result<Self> {
        let data = mc_data::load(legacy_root)
            .with_context(|| format!("loading 26.1.2 registries from {}", legacy_root.display()))?;
        let tags = mc_data::tags::load(legacy_root, &data)
            .with_context(|| format!("loading 26.1.2 tags from {}", legacy_root.display()))?;
        let tables = Tables::new(
            &Reports::load(canonical_root)?,
            &Reports::load(legacy_root)?,
            canonical,
            &data,
        )?;
        Ok(Self { data, tags, tables })
    }

    /// Optional startup: a missing or unusable 26.1.2 cache only disables
    /// legacy logins (they keep the vanilla incompatible-version refusal).
    #[must_use]
    pub fn discover(canonical: &mc_data::VanillaData) -> Option<Arc<Self>> {
        let canonical_root = canonical.sidecar_root()?;
        let legacy_root = cache_root(canonical_root);
        if !legacy_root.is_dir() {
            tracing::info!(
                cache = %legacy_root.display(),
                "no 26.1.2 content cache; protocol 775 clients are refused"
            );
            return None;
        }
        match Self::load(canonical_root, &legacy_root, canonical) {
            Ok(profile) => {
                tracing::info!(cache = %legacy_root.display(), "26.1.2 (protocol 775) clients enabled");
                Some(Arc::new(profile))
            }
            Err(error) => {
                tracing::warn!(cache = %legacy_root.display(), error = %format!("{error:#}"), "26.1.2 content cache unusable; protocol 775 clients are refused");
                None
            }
        }
    }
}

/// Protocol phase of a legacy session (Status never needs translation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Login,
    Configuration,
    Play,
}

/// The last absolute teleport sent and the client's last reported pose: a
/// 26.1.2 accept-teleport carries only the id, 26.3's echoes the pose.
#[derive(Debug, Default)]
struct Teleports {
    sent: HashMap<i32, Teleport>,
    pose: Option<([f64; 3], [f32; 2])>,
}

#[derive(Debug, Clone, Copy)]
struct Teleport {
    position: [f64; 3],
    rotation: [f32; 2],
    relative: i32,
}

/// One connection direction's translator. Pre-play one instance does both
/// directions; Play splits it into reader and writer halves.
pub struct Session {
    profile: Arc<Profile>,
    phase: Phase,
    teleports: Arc<Mutex<Teleports>>,
    /// Last lossy player-inventory images sent on the wire, keyed by menu slot.
    inventory_polyfills: Arc<Mutex<HashMap<i16, Vec<u8>>>>,
    /// Entities whose metadata layout differs (canonical type policy).
    entities: HashMap<i32, Metadata>,
}

impl Session {
    #[must_use]
    pub fn new(profile: Arc<Profile>) -> Self {
        Self {
            profile,
            phase: Phase::Login,
            teleports: Arc::default(),
            inventory_polyfills: Arc::default(),
            entities: HashMap::new(),
        }
    }

    #[must_use]
    pub fn profile(&self) -> &Arc<Profile> {
        &self.profile
    }

    /// A second translator for the other Play half, sharing teleport state.
    #[must_use]
    pub fn split(&self) -> Self {
        Self {
            profile: Arc::clone(&self.profile),
            phase: self.phase,
            teleports: Arc::clone(&self.teleports),
            inventory_polyfills: Arc::clone(&self.inventory_polyfills),
            entities: HashMap::new(),
        }
    }

    fn names(profile: &Profile, phase: Phase, clientbound: bool) -> &PacketNames {
        let packets = &profile.tables.packets;
        match (phase, clientbound) {
            (Phase::Login, true) => &packets.login_cb,
            (Phase::Login, false) => &packets.login_sb,
            (Phase::Configuration, true) => &packets.config_cb,
            (Phase::Configuration, false) => &packets.config_sb,
            (Phase::Play, true) => &packets.play_cb,
            (Phase::Play, false) => &packets.play_sb,
        }
    }

    /// Canonical clientbound packet -> legacy id and body; `None` drops it.
    pub fn clientbound(&mut self, id: i32, body: &[u8]) -> Result<Option<(i32, Vec<u8>)>> {
        let profile = Arc::clone(&self.profile);
        let names = Self::names(&profile, self.phase, true);
        let name = names.canonical_name(id).with_context(|| {
            format!(
                "no canonical clientbound packet {id:#04x} in {:?}",
                self.phase
            )
        })?;
        let Some(legacy_id) = names.legacy_id(play::legacy_name(name)) else {
            return Ok(None);
        };
        let body = match self.phase {
            Phase::Login if name == "minecraft:login_finished" => {
                // 26.3 appends the session UUID to the game profile.
                let len = body.len().checked_sub(16).context("short login_finished")?;
                Some(body[..len].to_vec())
            }
            Phase::Login | Phase::Configuration => Some(body.to_vec()),
            Phase::Play => play::clientbound(self, name, body)
                .with_context(|| format!("translating canonical {name}"))?,
        };
        if self.phase == Phase::Play && name == "minecraft:start_configuration" {
            self.phase = Phase::Configuration;
        }
        Ok(body.map(|body| (legacy_id, body)))
    }

    /// Legacy serverbound frame -> canonical frame; `None` drops packets
    /// without a corresponding canonical operation.
    pub fn serverbound(&mut self, frame: RawFrame) -> Result<Option<RawFrame>> {
        let profile = Arc::clone(&self.profile);
        let names = Self::names(&profile, self.phase, false);
        let name = names.legacy_name(frame.id).with_context(|| {
            format!(
                "unknown 26.1.2 serverbound packet {:#04x} in {:?}",
                frame.id, self.phase
            )
        })?;
        let canonical_name = match name {
            "minecraft:swing" => "minecraft:punch",
            "minecraft:spectate_entity" => "minecraft:spectator_action",
            other => other,
        };
        let canonical = names.canonical_id(canonical_name);
        let next = match (self.phase, name) {
            (Phase::Login, "minecraft:login_acknowledged") => Some(Phase::Configuration),
            (Phase::Configuration, "minecraft:finish_configuration") => Some(Phase::Play),
            (Phase::Play, "minecraft:configuration_acknowledged") => Some(Phase::Configuration),
            _ => None,
        };
        let body = match canonical {
            None => None,
            Some(_) if self.phase == Phase::Play => play::serverbound(self, name, &frame.body)
                .with_context(|| format!("translating 26.1.2 {name}"))?,
            Some(_) => Some(frame.body.to_vec()),
        };
        if let Some(next) = next {
            self.phase = next;
        }
        Ok(canonical.zip(body).map(|(id, body)| RawFrame {
            id,
            body: Bytes::from(body),
        }))
    }

    /// Translate whole pre-framed Play frames (`compression` as on the wire).
    pub fn clientbound_frames(
        &mut self,
        framed: &[u8],
        compression: Compression,
    ) -> Result<BytesMut> {
        let mut input = BytesMut::from(framed);
        let mut out = BytesMut::with_capacity(framed.len());
        while let Some(frame) = mc_protocol::frame::try_decode_frame(&mut input, compression)? {
            if let Some((id, body)) = self.clientbound(frame.id, &frame.body)? {
                mc_protocol::frame::append_frame(&mut out, id, &body, compression)?;
            }
        }
        anyhow::ensure!(input.is_empty(), "partial frame in a pre-framed batch");
        Ok(out)
    }
}

#[cfg(test)]
fn test_session(tables: Tables) -> Session {
    Session::new(Arc::new(Profile {
        data: mc_data::VanillaData::from_registries("", Vec::new()),
        tags: mc_data::tags::TagsData::default(),
        tables,
    }))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "join_tests.rs"]
mod join_tests;
