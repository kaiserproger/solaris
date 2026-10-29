//! Vanilla `/locate structure|biome|poi` (LocateCommand.java).
//!
//! Searches the generator's actual village and non-village starts; registry
//! entries without a generator still report `commands.locate.structure.not_found`.
//! Biome search samples the generator's
//! 3D noise biome on expanding square rings (vanilla `findClosestBiome3d`,
//! 6400/32/64). POI search scans loaded chunks for the vanilla
//! [`PoiTypes`](https://minecraft.wiki/w/Point_of_interest) block lists.
//!
//! Structure and biome searches run on one lazily spawned worker thread so the
//! tick never blocks; completions drain once per tick ([`drain`]). POI scans
//! only loaded chunks and runs inline on the tick. A player with two searches
//! already in flight (or a full queue of eight) gets a plain busy refusal;
//! queued searches are never reordered or dropped.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};

use crate::boundary::{ConnectionId, EntityId, PlayerChange};
use crate::command_feedback::{text, translated};
use crate::commands::CommandError;
use crate::tick::TickState;
use mc_data::Identifier;
use mc_nbt::{ListTag, Tag};
use mc_world::{BlockRegistry, BlockStateId, ChunkGenerator};
use mc_worldgen::vanilla_features::BiomeTagIndex;

const MAX_STRUCTURE_SEARCH_RADIUS_CHUNKS: i32 = 100;
const MAX_BIOME_SEARCH_RADIUS: i32 = 6400;
const BIOME_SAMPLE_STEP: i32 = 32;
const BIOME_SAMPLE_STEP_Y: i32 = 64;
const POI_SEARCH_RADIUS: i32 = 256;
/// In-flight searches per player, and total queued jobs.
const MAX_JOBS_PER_PLAYER: usize = 2;
const MAX_QUEUED_JOBS: usize = 8;
/// Every structure id vanilla 26.1.2 knows
/// (`data/minecraft/worldgen/structure/*.json`): anything else is `invalid`,
/// anything here but not generated is `not_found`.
const KNOWN_STRUCTURES: &[&str] = &[
    "minecraft:ancient_city",
    "minecraft:bastion_remnant",
    "minecraft:buried_treasure",
    "minecraft:desert_pyramid",
    "minecraft:end_city",
    "minecraft:fortress",
    "minecraft:igloo",
    "minecraft:jungle_pyramid",
    "minecraft:mansion",
    "minecraft:mineshaft",
    "minecraft:mineshaft_mesa",
    "minecraft:monument",
    "minecraft:nether_fossil",
    "minecraft:ocean_ruin_cold",
    "minecraft:ocean_ruin_warm",
    "minecraft:pillager_outpost",
    "minecraft:ruined_portal",
    "minecraft:ruined_portal_desert",
    "minecraft:ruined_portal_jungle",
    "minecraft:ruined_portal_mountain",
    "minecraft:ruined_portal_nether",
    "minecraft:ruined_portal_ocean",
    "minecraft:ruined_portal_swamp",
    "minecraft:shipwreck",
    "minecraft:shipwreck_beached",
    "minecraft:stronghold",
    "minecraft:swamp_hut",
    "minecraft:trail_ruins",
    "minecraft:trial_chambers",
    "minecraft:village_desert",
    "minecraft:village_plains",
    "minecraft:village_savanna",
    "minecraft:village_snowy",
    "minecraft:village_taiga",
];

/// Vanilla structure tags (`data/minecraft/tags/worldgen/structure/*.json`).
/// Entries name structures or nested `#tags`, exactly as the JSON does.
const STRUCTURE_TAGS: &[(&str, &[&str])] = &[
    (
        "minecraft:village",
        &[
            "minecraft:village_plains",
            "minecraft:village_desert",
            "minecraft:village_savanna",
            "minecraft:village_snowy",
            "minecraft:village_taiga",
        ],
    ),
    (
        "minecraft:mineshaft",
        &["minecraft:mineshaft", "minecraft:mineshaft_mesa"],
    ),
    (
        "minecraft:ocean_ruin",
        &["minecraft:ocean_ruin_cold", "minecraft:ocean_ruin_warm"],
    ),
    (
        "minecraft:shipwreck",
        &["minecraft:shipwreck", "minecraft:shipwreck_beached"],
    ),
    (
        "minecraft:ruined_portal",
        &[
            "minecraft:ruined_portal",
            "minecraft:ruined_portal_desert",
            "minecraft:ruined_portal_jungle",
            "minecraft:ruined_portal_mountain",
            "minecraft:ruined_portal_nether",
            "minecraft:ruined_portal_ocean",
            "minecraft:ruined_portal_swamp",
        ],
    ),
    ("minecraft:cats_spawn_in", &["minecraft:swamp_hut"]),
    ("minecraft:cats_spawn_as_black", &["minecraft:swamp_hut"]),
    (
        "minecraft:dolphin_located",
        &[
            "minecraft:ocean_ruin_cold",
            "minecraft:ocean_ruin_warm",
            "minecraft:shipwreck",
            "minecraft:shipwreck_beached",
            "minecraft:monument",
            "minecraft:ocean_monument",
        ],
    ),
    ("minecraft:eye_of_ender_located", &["minecraft:stronghold"]),
    ("minecraft:on_ocean_explorer_maps", &["minecraft:monument"]),
    (
        "minecraft:on_woodland_explorer_maps",
        &["minecraft:mansion"],
    ),
    (
        "minecraft:on_jungle_explorer_maps",
        &["minecraft:jungle_pyramid"],
    ),
    ("minecraft:on_swamp_explorer_maps", &["minecraft:swamp_hut"]),
    ("minecraft:on_treasure_maps", &["minecraft:buried_treasure"]),
    (
        "minecraft:on_trial_chambers_maps",
        &["minecraft:trial_chambers"],
    ),
    (
        "minecraft:on_plains_village_maps",
        &["minecraft:village_plains"],
    ),
    (
        "minecraft:on_desert_village_maps",
        &["minecraft:village_desert"],
    ),
    (
        "minecraft:on_savanna_village_maps",
        &["minecraft:village_savanna"],
    ),
    (
        "minecraft:on_snowy_village_maps",
        &["minecraft:village_snowy"],
    ),
    (
        "minecraft:on_taiga_village_maps",
        &["minecraft:village_taiga"],
    ),
];

/// Vanilla POI block lists (PoiTypes.java `bootstrap` + bed/lightning-rod
/// variants). Beds match head-part states only (filtered at resolve time).
const POI_BLOCKS: &[(&str, &[&str])] = &[
    ("minecraft:armorer", &["minecraft:blast_furnace"]),
    ("minecraft:butcher", &["minecraft:smoker"]),
    ("minecraft:cartographer", &["minecraft:cartography_table"]),
    ("minecraft:cleric", &["minecraft:brewing_stand"]),
    ("minecraft:farmer", &["minecraft:composter"]),
    ("minecraft:fisherman", &["minecraft:barrel"]),
    ("minecraft:fletcher", &["minecraft:fletching_table"]),
    (
        "minecraft:leatherworker",
        &[
            "minecraft:cauldron",
            "minecraft:lava_cauldron",
            "minecraft:water_cauldron",
            "minecraft:powder_snow_cauldron",
        ],
    ),
    ("minecraft:librarian", &["minecraft:lectern"]),
    ("minecraft:mason", &["minecraft:stonecutter"]),
    ("minecraft:shepherd", &["minecraft:loom"]),
    ("minecraft:toolsmith", &["minecraft:smithing_table"]),
    ("minecraft:weaponsmith", &["minecraft:grindstone"]),
    (
        "minecraft:home",
        &[
            "minecraft:red_bed",
            "minecraft:black_bed",
            "minecraft:blue_bed",
            "minecraft:brown_bed",
            "minecraft:cyan_bed",
            "minecraft:gray_bed",
            "minecraft:green_bed",
            "minecraft:light_blue_bed",
            "minecraft:light_gray_bed",
            "minecraft:lime_bed",
            "minecraft:magenta_bed",
            "minecraft:orange_bed",
            "minecraft:pink_bed",
            "minecraft:purple_bed",
            "minecraft:white_bed",
            "minecraft:yellow_bed",
        ],
    ),
    ("minecraft:meeting", &["minecraft:bell"]),
    ("minecraft:beehive", &["minecraft:beehive"]),
    ("minecraft:bee_nest", &["minecraft:bee_nest"]),
    ("minecraft:nether_portal", &["minecraft:nether_portal"]),
    ("minecraft:lodestone", &["minecraft:lodestone"]),
    (
        "minecraft:lightning_rod",
        &[
            "minecraft:lightning_rod",
            "minecraft:exposed_lightning_rod",
            "minecraft:weathered_lightning_rod",
            "minecraft:oxidized_lightning_rod",
            "minecraft:waxed_lightning_rod",
            "minecraft:waxed_exposed_lightning_rod",
            "minecraft:waxed_weathered_lightning_rod",
            "minecraft:waxed_oxidized_lightning_rod",
        ],
    ),
    (
        "minecraft:test_instance",
        &["minecraft:test_instance_block"],
    ),
];

/// Vanilla POI tags (`data/minecraft/tags/point_of_interest_type/*.json`).
const POI_TAGS: &[(&str, &[&str])] = &[
    (
        "minecraft:village",
        &[
            "#minecraft:acquirable_job_site",
            "minecraft:home",
            "minecraft:meeting",
        ],
    ),
    (
        "minecraft:acquirable_job_site",
        &[
            "minecraft:armorer",
            "minecraft:butcher",
            "minecraft:cartographer",
            "minecraft:cleric",
            "minecraft:farmer",
            "minecraft:fisherman",
            "minecraft:fletcher",
            "minecraft:leatherworker",
            "minecraft:librarian",
            "minecraft:mason",
            "minecraft:shepherd",
            "minecraft:toolsmith",
            "minecraft:weaponsmith",
        ],
    ),
    (
        "minecraft:bee_home",
        &["minecraft:beehive", "minecraft:bee_nest"],
    ),
];

/// Parsed `/locate <kind> <id|#tag>`.
#[derive(Debug, Clone, PartialEq)]
pub enum LocateCommand {
    Structure(ResourceOrTag),
    Biome(ResourceOrTag),
    Poi(ResourceOrTag),
}

/// One resource-or-tag key: `#tag` (`tag: true`) or a bare id defaulting to
/// the `minecraft` namespace, exactly like vanilla `ResourceOrTagKeyArgument`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceOrTag {
    pub tag: bool,
    pub id: Identifier,
}

/// Parse `structure <ref>`, `biome <ref>` or `poi <ref>`.
pub(crate) fn parse(args: &[&str]) -> Result<LocateCommand, CommandError> {
    const USAGE: &str = "locate <structure|biome|poi> <id|#tag>";
    let [kind, reference] = args else {
        return Err(CommandError::Usage(USAGE));
    };
    let target = ResourceOrTag::parse(reference).ok_or(CommandError::Usage(USAGE))?;
    match *kind {
        "structure" => Ok(LocateCommand::Structure(target)),
        "biome" => Ok(LocateCommand::Biome(target)),
        "poi" => Ok(LocateCommand::Poi(target)),
        _ => Err(CommandError::Usage(USAGE)),
    }
}

impl ResourceOrTag {
    fn parse(word: &str) -> Option<Self> {
        let (tag, raw) = match word.strip_prefix('#') {
            Some(rest) => (true, rest),
            None => (false, word),
        };
        if raw.is_empty() {
            return None;
        }
        Identifier::parse(raw).ok().map(|id| Self { tag, id })
    }

    /// Printable form for error keys: `#tag` keeps its prefix.
    fn printable(&self) -> String {
        if self.tag {
            format!("#{}", self.id.as_str())
        } else {
            self.id.as_str().to_owned()
        }
    }
}

/// A finished search: position, resolved display name and distance.
#[derive(Debug, Clone, PartialEq)]
pub struct LocateHit {
    pub pos: (i32, i32, i32),
    /// Resolved structure/biome/POI id at the hit (for `#tag (id)` names).
    pub found_id: Identifier,
    pub distance: i32,
}

/// Resolve a `#tag` to its member ids; `None` when the tag is unknown.
fn resolve_structure_tag(tag: &Identifier) -> Option<Vec<Identifier>> {
    let members = STRUCTURE_TAGS
        .iter()
        .find(|(name, _)| *name == tag.as_str())?
        .1;
    let mut out = Vec::new();
    for member in members {
        if let Some(nested) = member.strip_prefix('#') {
            let nested = Identifier::parse((*nested).to_owned()).ok()?;
            out.extend(resolve_structure_tag(&nested)?);
        } else {
            out.push(Identifier::parse((*member).to_owned()).ok()?);
        }
    }
    Some(out)
}

/// Resolve a POI `#tag` to member POI ids; `None` when unknown.
fn resolve_poi_tag(tag: &Identifier) -> Option<Vec<Identifier>> {
    let members = POI_TAGS.iter().find(|(name, _)| *name == tag.as_str())?.1;
    let mut out = Vec::new();
    for member in members {
        if let Some(nested) = member.strip_prefix('#') {
            let nested = Identifier::parse((*nested).to_owned()).ok()?;
            out.extend(resolve_poi_tag(&nested)?);
        } else {
            out.push(Identifier::parse((*member).to_owned()).ok()?);
        }
    }
    Some(out)
}

type SavedStarts = HashMap<mc_world::ChunkPos, Vec<mc_world::chunk::GeneratedStructureSite>>;

/// Saved coverage is authoritative, including covered chunks without starts.
/// Only uncovered chunks may contribute deterministic generator plans.
fn search_structure(
    generator: &dyn ChunkGenerator,
    origin: (i32, i32, i32),
    targets: &[Identifier],
    saved: &SavedStarts,
    options: &StructureSearchOptions,
) -> Option<LocateHit> {
    let centre = (origin.0.div_euclid(16), origin.2.div_euclid(16));
    let min = (
        centre.0.saturating_sub(options.radius),
        centre.1.saturating_sub(options.radius),
    );
    let max = (
        centre.0.saturating_add(options.radius),
        centre.1.saturating_add(options.radius),
    );
    let mut sites = if targets.iter().any(|id| !id.path().starts_with("village_")) {
        generator.structure_sites_in_region(min, max)
    } else {
        Vec::new()
    };
    if targets.iter().any(|id| id.path().starts_with("village_")) {
        let radius = MAX_STRUCTURE_SEARCH_RADIUS_CHUNKS.min(options.radius);
        for site in generator.village_sites_in_region(
            (
                centre.0.saturating_sub(radius),
                centre.1.saturating_sub(radius),
            ),
            (
                centre.0.saturating_add(radius),
                centre.1.saturating_add(radius),
            ),
        ) {
            let x = site.start_chunk.0 * 16 + 8;
            let z = site.start_chunk.1 * 16 + 8;
            sites.push(mc_world::chunk::GeneratedStructureSite {
                structure: site.structure,
                start_chunk: site.start_chunk,
                position: mc_world::BlockPos {
                    x,
                    y: generator.surface_height(x, z).unwrap_or(64),
                    z,
                },
            });
        }
    }
    sites.retain(|site| {
        !saved.contains_key(&mc_world::ChunkPos {
            x: site.start_chunk.0,
            z: site.start_chunk.1,
        })
    });
    sites.extend(saved.values().flatten().cloned());
    sites
        .into_iter()
        .filter(|site| targets.contains(&site.structure))
        .filter(|site| !options.skip_referenced || !options.referenced.contains(&site.position))
        .filter(|site| {
            site.start_chunk.0 >= min.0
                && site.start_chunk.0 <= max.0
                && site.start_chunk.1 >= min.1
                && site.start_chunk.1 <= max.1
        })
        .map(|site| {
            let p = site.position;
            let dx = i64::from(p.x) - i64::from(origin.0);
            let dz = i64::from(p.z) - i64::from(origin.2);
            (dx * dx + dz * dz, (p.x, p.y, p.z), site.structure)
        })
        .min_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)))
        .map(|(distance, pos, found_id)| LocateHit {
            pos,
            found_id,
            distance: distance.isqrt() as i32,
        })
}

struct StructureSnapshot {
    source: Arc<crate::chunk_loader::ChunkSource>,
    resident: Result<SavedStarts, String>,
    radius: i32,
    skip_referenced: bool,
}

impl StructureSnapshot {
    fn capture(
        state: &TickState,
        origin: (i32, i32, i32),
        options: &StructureSearchOptions,
    ) -> Self {
        let centre = (origin.0.div_euclid(16), origin.2.div_euclid(16));
        let resident = state
            .world
            .chunks()
            .filter(|chunk| {
                (i64::from(chunk.pos.x) - i64::from(centre.0)).abs() <= i64::from(options.radius)
                    && (i64::from(chunk.pos.z) - i64::from(centre.1)).abs()
                        <= i64::from(options.radius)
            })
            .map(|chunk| {
                mc_world::structure_starts::read(&chunk.extras, chunk.pos, options.skip_referenced)
                    .map(|sites| (chunk.pos, sites))
            })
            .collect();
        Self {
            source: state.world.chunk_source(),
            resident,
            radius: options.radius,
            skip_referenced: options.skip_referenced,
        }
    }

    /// Called only by the locator worker; never opens native storage on tick.
    fn load(self, origin: (i32, i32, i32)) -> Result<SavedStarts, String> {
        let centre = (origin.0.div_euclid(16), origin.2.div_euclid(16));
        let mut saved: SavedStarts = if let Some(native) = &self.source.native {
            native
                .checkpoint_reader()
                .and_then(|reader| {
                    reader.structure_starts_in_region(
                        self.source.dimension,
                        (
                            centre.0.saturating_sub(self.radius),
                            centre.1.saturating_sub(self.radius),
                        ),
                        (
                            centre.0.saturating_add(self.radius),
                            centre.1.saturating_add(self.radius),
                        ),
                        self.skip_referenced,
                    )
                })
                .map_err(|error| error.to_string())?
                .into_iter()
                .collect()
        } else {
            HashMap::new()
        };
        saved.extend(self.resident?);
        Ok(saved)
    }
}

/// Square perimeter at Chebyshev distance `radius` around `centre`, in
/// ascending order (vanilla `spiralAround` visits rings in the same order).
pub fn square_ring(centre: (i32, i32), radius: i32) -> Vec<(i32, i32)> {
    if radius == 0 {
        return vec![centre];
    }
    let mut out = Vec::with_capacity((radius * 8) as usize);
    for offset in -radius..=radius {
        out.push((centre.0 + offset, centre.1 - radius));
        out.push((centre.0 + offset, centre.1 + radius));
    }
    for offset in (-radius + 1)..radius {
        out.push((centre.0 - radius, centre.1 + offset));
        out.push((centre.0 + radius, centre.1 + offset));
    }
    out
}

/// Synchronous biome search: vanilla `findClosestBiome3d` with 32-block
/// horizontal and 64-block vertical steps. `noise` is the generator's 3D
/// noise biome; `min_y`/`max_y` bound the vertical samples (block coords).
/// First hit in spiral order wins, exactly like vanilla.
pub fn search_biome(
    noise: &dyn Fn(i32, i32, i32) -> Option<Identifier>,
    origin: (i32, i32, i32),
    targets: &[Identifier],
    min_y: i32,
    max_y: i32,
) -> Option<LocateHit> {
    if targets.is_empty() {
        return None;
    }
    let sample_radius = MAX_BIOME_SEARCH_RADIUS / BIOME_SAMPLE_STEP;
    let sample_ys = sample_heights(origin.1, min_y + 1, max_y + 1);
    for ring in 0..=sample_radius {
        for (dx, dz) in square_ring((0, 0), ring) {
            let block_x = origin.0 + dx * BIOME_SAMPLE_STEP;
            let block_z = origin.2 + dz * BIOME_SAMPLE_STEP;
            for block_y in &sample_ys {
                let Some(biome) = noise(block_x, *block_y, block_z) else {
                    continue;
                };
                if targets.iter().any(|target| target == &biome) {
                    let distance = (i64::from(block_x - origin.0) * i64::from(block_x - origin.0)
                        + i64::from(block_y - origin.1) * i64::from(block_y - origin.1)
                        + i64::from(block_z - origin.2) * i64::from(block_z - origin.2))
                    .isqrt() as i32;
                    return Some(LocateHit {
                        pos: (block_x, *block_y, block_z),
                        found_id: biome,
                        distance,
                    });
                }
            }
        }
    }
    None
}

/// Vanilla `Mth.outFromOrigin`: the clamped origin first, then alternating
/// outward steps of 64 while either side stays inside the bounds.
pub fn sample_heights(origin: i32, min: i32, max: i32) -> Vec<i32> {
    if min > max {
        return Vec::new();
    }
    let mut out = vec![origin.clamp(min, max)];
    let mut low = out[0] - BIOME_SAMPLE_STEP_Y;
    let mut high = out[0] + BIOME_SAMPLE_STEP_Y;
    while low >= min || high <= max {
        if high <= max {
            out.push(high);
        }
        if low >= min {
            out.push(low);
        }
        low -= BIOME_SAMPLE_STEP_Y;
        high += BIOME_SAMPLE_STEP_Y;
    }
    out
}

/// Synchronous POI search over one candidate column set: nearest POI state
/// within 256 blocks by horizontal distance. `candidates` yields the loaded
/// columns to scan (x, z, y-range) so callers bound the work to resident
/// chunks; `block_at` reads the folded world image (overlay included) and
/// `is_poi` tests a global block state id.
pub fn search_poi(
    candidates: &dyn Fn() -> Vec<(i32, i32, i32, i32)>,
    block_at: &dyn Fn(i32, i32, i32) -> Option<u32>,
    is_poi: &dyn Fn(u32) -> bool,
    origin: (i32, i32, i32),
) -> Option<LocateHit> {
    let mut best: Option<(i64, (i32, i32, i32))> = None;
    for (x, z, min_y, max_y) in candidates() {
        if (x - origin.0).abs() > POI_SEARCH_RADIUS || (z - origin.2).abs() > POI_SEARCH_RADIUS {
            continue;
        }
        for y in min_y..=max_y {
            let Some(state) = block_at(x, y, z) else {
                continue;
            };
            if !is_poi(state) {
                continue;
            }
            let distance_sqr = i64::from(x - origin.0) * i64::from(x - origin.0)
                + i64::from(z - origin.2) * i64::from(z - origin.2);
            if best.as_ref().is_none_or(|(known, _)| distance_sqr < *known) {
                best = Some((distance_sqr, (x, y, z)));
            }
        }
    }
    // The caller resolves the hit block back to its POI type; the placeholder
    // below only feeds tag-display tests and never reaches a player.
    best.map(|(distance_sqr, pos)| LocateHit {
        pos,
        found_id: Identifier::parse("minecraft:meeting").expect("static POI id"),
        distance: distance_sqr.isqrt() as i32,
    })
}

/// Resolve POI member ids to the global block states that carry them, using
/// the vanilla [`PoiTypes`](https://minecraft.wiki/w/Point_of_interest) block
/// lists. Bed blocks match head-part states only, like vanilla `BEDS`.
pub fn poi_states(blocks: &BlockRegistry, members: &[Identifier]) -> HashSet<u32> {
    let mut wanted: HashSet<&str> = HashSet::new();
    for member in members {
        let Some((_, block_names)) = POI_BLOCKS.iter().find(|(name, _)| *name == member.as_str())
        else {
            continue;
        };
        wanted.extend(block_names.iter().copied());
    }
    let mut out = HashSet::new();
    for name in wanted {
        let Ok(id) = Identifier::parse(name) else {
            continue;
        };
        let Some(block) = blocks.block(&id) else {
            continue;
        };
        let head_only = name.ends_with("_bed");
        for state in &block.states {
            if head_only && !is_bed_head(blocks, *state) {
                continue;
            }
            out.insert(state.0);
        }
    }
    out
}

/// Whether a bed state is a head part (vanilla `BEDS` only matches heads).
/// Falls back to matching every state when the schema has no `part` property.
fn is_bed_head(blocks: &BlockRegistry, state: BlockStateId) -> bool {
    let Some(resolved) = blocks.by_id(state) else {
        return false;
    };
    resolved
        .properties
        .iter()
        .find(|(name, _)| name == "part")
        .is_none_or(|(_, value)| value == "head")
}

/// Async job state owned by one dimension's [`TickState`][crate::tick::TickState].
/// The worker thread spawns lazily on the first `/locate`; both channels stay
/// `None` until then so idle worlds pay nothing.
#[derive(Default)]
pub struct LocateState {
    jobs: Option<SyncSender<LocateJob>>,
    done: Option<Receiver<LocateDone>>,
    in_flight: HashMap<ConnectionId, usize>,
}

impl LocateState {
    /// In-flight search count for one connection.
    fn busy(&self, connection: ConnectionId) -> usize {
        self.in_flight.get(&connection).copied().unwrap_or(0)
    }

    fn worker(&mut self) -> &SyncSender<LocateJob> {
        if self.jobs.is_none() {
            let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<LocateJob>(MAX_QUEUED_JOBS);
            let (done_tx, done_rx) = std::sync::mpsc::channel::<LocateDone>();
            std::thread::Builder::new()
                .name("locate".into())
                .spawn(move || locate_worker(job_rx, done_tx))
                .expect("locate worker thread spawns");
            self.jobs = Some(job_tx);
            self.done = Some(done_rx);
        }
        self.jobs.as_ref().expect("locate worker just spawned")
    }
}

/// One async search with owned inputs and a pinned metadata source.
struct LocateJob {
    connection: ConnectionId,
    kind: LocateKind,
    origin: (i32, i32, i32),
    generator: Arc<dyn ChunkGenerator>,
    targets: Vec<Identifier>,
    snapshot: Option<StructureSnapshot>,
    completion: LocateCompletion,
    options: StructureSearchOptions,
    min_y: i32,
    max_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocateKind {
    Structure,
    Biome,
}

/// A finished worker search, delivered to the tick thread.
struct LocateDone {
    completion: LocateCompletion,
    connection: ConnectionId,
    kind: LocateKind,
    origin: (i32, i32, i32),
    hit: Option<LocateHit>,
    elapsed_ms: u128,
}

fn locate_worker(jobs: Receiver<LocateJob>, done: Sender<LocateDone>) {
    for job in jobs {
        let start = std::time::Instant::now();
        let hit = match job.kind {
            LocateKind::Structure => {
                match job.snapshot.expect("structure snapshot").load(job.origin) {
                    Ok(saved) => search_structure(
                        job.generator.as_ref(),
                        job.origin,
                        &job.targets,
                        &saved,
                        &job.options,
                    ),
                    Err(error) => {
                        tracing::error!(%error, "Cannot read authoritative structure starts");
                        None
                    }
                }
            }
            LocateKind::Biome => {
                let generator = Arc::clone(&job.generator);
                search_biome(
                    &move |x, y, z| generator.noise_biome(x, y, z),
                    job.origin,
                    &job.targets,
                    job.min_y,
                    job.max_y,
                )
            }
        };
        let elapsed_ms = start.elapsed().as_millis();
        let result = LocateDone {
            connection: job.connection,
            kind: job.kind,
            origin: job.origin,
            hit,
            elapsed_ms,
            completion: job.completion,
        };
        if done.send(result).is_err() {
            return;
        }
    }
}

/// Execute a parsed `/locate` on the tick thread: immediate errors answer
/// inline, real searches queue on the worker and reply through [`drain`].
pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &LocateCommand,
) {
    let origin_f = crate::command_console::pose(state, executor).map_or_else(
        || state.world.world_spawn_position(),
        |(position, _)| position,
    );
    let origin = (
        origin_f[0].floor() as i32,
        origin_f[1].floor() as i32,
        origin_f[2].floor() as i32,
    );
    match command {
        LocateCommand::Structure(target) => execute_structure(state, connection, origin, target),
        LocateCommand::Biome(target) => execute_biome(state, connection, origin, target),
        LocateCommand::Poi(target) => execute_poi(state, connection, origin, target),
    }
}

fn execute_structure(
    state: &mut TickState,
    connection: ConnectionId,
    origin: (i32, i32, i32),
    target: &ResourceOrTag,
) {
    let (targets, display) = match resolve_structure_target(target) {
        Ok(resolved) => resolved,
        Err(key) => {
            failure(state, connection, key, vec![text(target.printable())]);
            return;
        }
    };
    if submit(
        state,
        connection,
        LocateKind::Structure,
        origin,
        targets,
        LocateCompletion::Command(display),
        StructureSearchOptions::default(),
    ) {
        feedback(
            state,
            connection,
            "commands.locate.structure.searching",
            Vec::new(),
        );
    }
}

fn execute_biome(
    state: &mut TickState,
    connection: ConnectionId,
    origin: (i32, i32, i32),
    target: &ResourceOrTag,
) {
    let (targets, display) = match resolve_biome_target(state, target) {
        Ok(resolved) => resolved,
        Err(key) => {
            failure(state, connection, key, vec![text(target.printable())]);
            return;
        }
    };
    if submit(
        state,
        connection,
        LocateKind::Biome,
        origin,
        targets,
        LocateCompletion::Command(display),
        StructureSearchOptions::default(),
    ) {
        feedback(
            state,
            connection,
            "commands.locate.biome.searching",
            Vec::new(),
        );
    }
}

fn execute_poi(
    state: &mut TickState,
    connection: ConnectionId,
    origin: (i32, i32, i32),
    target: &ResourceOrTag,
) {
    let (members, display) = match resolve_poi_target(target) {
        Ok(resolved) => resolved,
        Err(key) => {
            failure(state, connection, key, vec![text(target.printable())]);
            return;
        }
    };
    let states = poi_states(&state.registry.blocks, &members);
    if states.is_empty() {
        failure(
            state,
            connection,
            "commands.locate.poi.not_found",
            vec![text(&display)],
        );
        return;
    }
    let geometry = state.world.geometry();
    let start = std::time::Instant::now();
    let hit = search_poi_loaded(state, origin, &states);
    let elapsed_ms = start.elapsed().as_millis();
    match hit {
        Some((pos, found_id)) => {
            let found_name = poi_display(&display, target, &found_id);
            show_result(
                state,
                connection,
                "commands.locate.poi.success",
                &found_name,
                origin,
                pos,
                false,
            );
            tracing::info!("Locating element {found_name} took {elapsed_ms} ms");
        }
        None => failure(
            state,
            connection,
            "commands.locate.poi.not_found",
            vec![text(&display)],
        ),
    }
    let _ = geometry;
}

/// Scan resident chunks (plus the tick overlay) for POI states, cheapest
/// first: section palettes gate the exact cell scan.
fn search_poi_loaded(
    state: &TickState,
    origin: (i32, i32, i32),
    states: &HashSet<u32>,
) -> Option<((i32, i32, i32), Identifier)> {
    let is_poi = |id: u32| states.contains(&id);
    let (min_cx, max_cx) = (
        (origin.0 - POI_SEARCH_RADIUS).div_euclid(16),
        (origin.0 + POI_SEARCH_RADIUS).div_euclid(16),
    );
    let (min_cz, max_cz) = (
        (origin.2 - POI_SEARCH_RADIUS).div_euclid(16),
        (origin.2 + POI_SEARCH_RADIUS).div_euclid(16),
    );
    let mut best: Option<(i64, (i32, i32, i32), u32)> = None;
    let mut consider = |x: i32, y: i32, z: i32, id: u32| {
        let distance_sqr = i64::from(x - origin.0) * i64::from(x - origin.0)
            + i64::from(z - origin.2) * i64::from(z - origin.2);
        if best
            .as_ref()
            .is_none_or(|(known, _, _)| distance_sqr < *known)
        {
            best = Some((distance_sqr, (x, y, z), id));
        }
    };
    for cz in min_cz..=max_cz {
        for cx in min_cx..=max_cx {
            let Some(chunk) = state.world.chunk(mc_world::ChunkPos { x: cx, z: cz }) else {
                continue;
            };
            let geometry = chunk.geometry();
            for (section_idx, section) in chunk.sections.iter().enumerate() {
                let section_min_y = geometry.min_y() + section_idx as i32 * 16;
                let palette_hit = match section.palette() {
                    Some(palette) => palette.iter().any(|id| is_poi(id.0)),
                    None => is_poi(section.get(0, 0, 0).0),
                };
                if !palette_hit {
                    continue;
                }
                for ly in 0..16u8 {
                    let y = section_min_y + i32::from(ly);
                    for lz in 0..16u8 {
                        for lx in 0..16u8 {
                            let id = section.get(lx, ly, lz);
                            if !is_poi(id.0) {
                                continue;
                            }
                            consider(cx * 16 + i32::from(lx), y, cz * 16 + i32::from(lz), id.0);
                        }
                    }
                }
            }
        }
    }
    for ((x, y, z), id) in state.world.overlay_entries() {
        if (x - origin.0).abs() > POI_SEARCH_RADIUS || (z - origin.2).abs() > POI_SEARCH_RADIUS {
            continue;
        }
        if is_poi(id) {
            consider(x, y, z, id);
        }
    }
    best.map(|(_, pos, id)| {
        let found_id = state
            .registry
            .blocks
            .by_id(BlockStateId(id))
            .map(|resolved| poi_type_of(&resolved.block.id))
            .unwrap_or_else(|| Identifier::parse("minecraft:meeting").expect("static POI id"));
        (pos, found_id)
    })
}

/// Map a block back to its POI type (first table entry wins; beds map home).
fn poi_type_of(block: &Identifier) -> Identifier {
    for (poi, blocks) in POI_BLOCKS {
        if blocks.contains(&block.as_str()) {
            return Identifier::parse((*poi).to_owned()).expect("static POI id");
        }
    }
    Identifier::parse("minecraft:meeting").expect("static POI id")
}

/// Resolve a structure target: id must be a known vanilla structure, tags
/// expand to member structures. Returns member ids plus the display name.
fn resolve_structure_target(
    target: &ResourceOrTag,
) -> Result<(Vec<Identifier>, String), &'static str> {
    if target.tag {
        let members =
            resolve_structure_tag(&target.id).ok_or("commands.locate.structure.invalid")?;
        let display = format!("#{}", target.id.as_str());
        Ok((members, display))
    } else if KNOWN_STRUCTURES.contains(&target.id.as_str()) {
        let display = target.id.as_str().to_owned();
        Ok((vec![target.id.clone()], display))
    } else {
        Err("commands.locate.structure.invalid")
    }
}

/// Resolve a biome target against the biome registry: ids must name a real
/// biome, tags expand through the world's biome-tag index.
fn resolve_biome_target(
    state: &TickState,
    target: &ResourceOrTag,
) -> Result<(Vec<Identifier>, String), &'static str> {
    let known: Vec<Identifier> = state.registry.biome_spawns.biomes().cloned().collect();
    if target.tag {
        let Some(worldgen_tags) = state.registry.worldgen_tags.as_ref() else {
            return Err("argument.resource_or_id.invalid");
        };
        if state
            .registry
            .tags
            .raw_ids("minecraft:worldgen/biome", target.id.as_str())
            .is_none()
        {
            return Err("argument.resource_or_id.invalid");
        }
        let members: Vec<Identifier> = known
            .into_iter()
            .filter(|biome| worldgen_tags.biome_in_tag(&target.id, biome))
            .collect();
        Ok((members, format!("#{}", target.id.as_str())))
    } else if known.iter().any(|biome| biome == &target.id) {
        let display = target.id.as_str().to_owned();
        Ok((vec![target.id.clone()], display))
    } else {
        Err("argument.resource_or_id.invalid")
    }
}

/// Resolve a POI target: ids must be known POI types, tags expand statically.
fn resolve_poi_target(target: &ResourceOrTag) -> Result<(Vec<Identifier>, String), &'static str> {
    if target.tag {
        let members = resolve_poi_tag(&target.id).ok_or("commands.locate.poi.not_found")?;
        Ok((members, format!("#{}", target.id.as_str())))
    } else if POI_BLOCKS
        .iter()
        .any(|(name, _)| *name == target.id.as_str())
    {
        let display = target.id.as_str().to_owned();
        Ok((vec![target.id.clone()], display))
    } else {
        Err("commands.locate.poi.not_found")
    }
}

type StructureLookupReply = Box<dyn FnOnce(&mut TickState, Option<mc_world::BlockPos>) + Send>;

/// Deferred loot/trade lookup. Completion runs on the native tick, including
/// disconnected requesters; the callback validates its own container/token.
pub(crate) struct StructureLookupRequest {
    pub connection: ConnectionId,
    pub origin: (i32, i32, i32),
    pub destination: Identifier,
    pub search_radius: u32,
    pub skip_existing_chunks: bool,
    pub referenced: Vec<mc_world::BlockPos>,
    pub reply: StructureLookupReply,
}

enum LocateCompletion {
    Command(String),
    Eye(crate::projectiles::eye::EyeUse),
    Callback(StructureLookupReply),
}

struct StructureSearchOptions {
    radius: i32,
    skip_referenced: bool,
    referenced: HashSet<mc_world::BlockPos>,
}

impl Default for StructureSearchOptions {
    fn default() -> Self {
        Self {
            radius: 256,
            skip_referenced: false,
            referenced: HashSet::new(),
        }
    }
}

pub(crate) fn submit_structure_lookup(
    state: &mut TickState,
    request: StructureLookupRequest,
) -> bool {
    if request.search_radius > 256 {
        return false;
    }
    let Some(targets) = resolve_structure_tag(&request.destination) else {
        return false;
    };
    submit(
        state,
        request.connection,
        LocateKind::Structure,
        request.origin,
        targets,
        LocateCompletion::Callback(request.reply),
        StructureSearchOptions {
            radius: request.search_radius as i32,
            skip_referenced: request.skip_existing_chunks,
            referenced: request.referenced.into_iter().collect(),
        },
    )
}

pub(crate) fn submit_eye(state: &mut TickState, request: crate::projectiles::eye::EyeUse) -> bool {
    let origin = (
        request.origin[0].floor() as i32,
        request.origin[1].floor() as i32,
        request.origin[2].floor() as i32,
    );
    submit(
        state,
        request.connection,
        LocateKind::Structure,
        origin,
        vec![Identifier::parse("minecraft:stronghold").expect("static structure id")],
        LocateCompletion::Eye(request),
        StructureSearchOptions::default(),
    )
}

/// Queue a worker search; refuses (with a plain busy message) when the player
/// or the queue is full.
fn submit(
    state: &mut TickState,
    connection: ConnectionId,
    kind: LocateKind,
    origin: (i32, i32, i32),
    targets: Vec<Identifier>,
    completion: LocateCompletion,
    options: StructureSearchOptions,
) -> bool {
    let quiet = !matches!(completion, LocateCompletion::Command(_));
    if state.locate.busy(connection) >= MAX_JOBS_PER_PLAYER {
        if quiet {
            return false;
        }
        state.push_player(
            connection,
            PlayerChange::Chat {
                message: "A locate search is already running; try again in a moment".into(),
            },
        );
        return false;
    }
    let geometry = state.world.geometry();
    let job = LocateJob {
        connection,
        kind,
        origin,
        generator: state.world.generator(),
        targets,
        snapshot: (kind == LocateKind::Structure)
            .then(|| StructureSnapshot::capture(state, origin, &options)),
        completion,
        options,
        min_y: geometry.min_y(),
        max_y: geometry.max_y() - 1,
    };
    match state.locate.worker().try_send(job) {
        Ok(()) => {
            *state.locate.in_flight.entry(connection).or_default() += 1;
            true
        }
        Err(TrySendError::Full(_)) => {
            if quiet {
                return false;
            }
            state.push_player(
                connection,
                PlayerChange::Chat {
                    message: "Too many locate searches are running; try again in a moment".into(),
                },
            );
            false
        }
        Err(TrySendError::Disconnected(_)) => {
            if quiet {
                return false;
            }
            state.push_player(
                connection,
                PlayerChange::Chat {
                    message: "Locate is unavailable right now".into(),
                },
            );
            false
        }
    }
}

/// Drain finished worker searches into player feedback. Skips connections
/// that left; always releases their in-flight slot.
pub(crate) fn drain(state: &mut TickState) {
    let ready: Vec<LocateDone> = state
        .locate
        .done
        .as_ref()
        .map(|done| done.try_iter().collect())
        .unwrap_or_default();
    if ready.is_empty() {
        // Also retire empty in-flight entries so a quiet map stays small.
        state.locate.in_flight.retain(|_, count| *count > 0);
        return;
    }
    // Console (connection 0) has no entity but always receives output.
    let live: HashSet<ConnectionId> = state.connections.all_connections().collect();
    let mut finished: Vec<LocateDone> = Vec::with_capacity(ready.len());
    let mut pending: VecDeque<LocateDone> = ready.into();
    while let Some(done) = pending.pop_front() {
        if matches!(done.completion, LocateCompletion::Callback(_))
            || live.contains(&done.connection)
            || done.connection == crate::command_console::CONNECTION
        {
            finished.push(done);
        } else {
            release(state, done.connection);
        }
    }
    for done in finished {
        release(state, done.connection);
        let display = match done.completion {
            LocateCompletion::Eye(request) => {
                if let Some(hit) = done.hit {
                    crate::projectiles::eye::complete(state, request, hit.pos);
                }
                continue;
            }
            LocateCompletion::Callback(reply) => {
                reply(
                    state,
                    done.hit.map(|hit| mc_world::BlockPos {
                        x: hit.pos.0,
                        y: hit.pos.1,
                        z: hit.pos.2,
                    }),
                );
                continue;
            }
            LocateCompletion::Command(display) => display,
        };
        match done.hit {
            Some(hit) => {
                let found_name = match done.kind {
                    LocateKind::Structure => structure_display(&display, &hit.found_id),
                    LocateKind::Biome => biome_display(&display, &hit.found_id),
                };
                let key = match done.kind {
                    LocateKind::Structure => "commands.locate.structure.success",
                    LocateKind::Biome => "commands.locate.biome.success",
                };
                show_result(
                    state,
                    done.connection,
                    key,
                    &found_name,
                    done.origin,
                    hit.pos,
                    matches!(done.kind, LocateKind::Biome),
                );
                let elapsed_ms = done.elapsed_ms;
                tracing::info!("Locating element {found_name} took {elapsed_ms} ms");
            }
            None => {
                let key = match done.kind {
                    LocateKind::Structure => "commands.locate.structure.not_found",
                    LocateKind::Biome => "commands.locate.biome.not_found",
                };
                failure(state, done.connection, key, vec![text(&display)]);
            }
        }
    }
}

fn release(state: &mut TickState, connection: ConnectionId) {
    if let Some(count) = state.locate.in_flight.get_mut(&connection) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            state.locate.in_flight.remove(&connection);
        }
    }
}

/// Vanilla `showLocateResult` display name: `#tag (resolved)` for tags.
fn structure_display(display: &str, found: &Identifier) -> String {
    tag_display(display, found)
}

fn biome_display(display: &str, found: &Identifier) -> String {
    tag_display(display, found)
}

fn poi_display(display: &str, target: &ResourceOrTag, found: &Identifier) -> String {
    if target.tag {
        format!("{display} ({})", found.as_str())
    } else {
        display.to_owned()
    }
}

fn tag_display(display: &str, found: &Identifier) -> String {
    if display.starts_with('#') {
        format!("{display} ({})", found.as_str())
    } else {
        display.to_owned()
    }
}

/// Vanilla `showLocateResult`: translatable success with a green
/// `[x, y|~, z]` component (suggest-command `/tp @s`, hover tooltip) plus the
/// integer distance (3D for biomes, horizontal otherwise).
fn show_result(
    state: &mut TickState,
    connection: ConnectionId,
    key: &str,
    found_name: &str,
    origin: (i32, i32, i32),
    found: (i32, i32, i32),
    include_y: bool,
) {
    let displayed_y = if include_y {
        found.1.to_string()
    } else {
        "~".to_owned()
    };
    let coordinates = Tag::Compound(vec![
        ("text".into(), Tag::String(String::new())),
        (
            "extra".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![
                    Tag::Compound(vec![("text".into(), Tag::String("[".into()))]),
                    Tag::Compound(vec![
                        ("translate".into(), Tag::String("chat.coordinates".into())),
                        (
                            "with".into(),
                            Tag::List(ListTag {
                                element_type: mc_nbt::tag_type::STRING,
                                elements: vec![
                                    Tag::String(found.0.to_string()),
                                    Tag::String(displayed_y.clone()),
                                    Tag::String(found.2.to_string()),
                                ],
                            }),
                        ),
                    ]),
                    Tag::Compound(vec![("text".into(), Tag::String("]".into()))]),
                ],
            }),
        ),
        ("color".into(), Tag::String("green".into())),
        (
            "click_event".into(),
            Tag::Compound(vec![
                ("action".into(), Tag::String("suggest_command".into())),
                (
                    "command".into(),
                    Tag::String(format!("/tp @s {} {displayed_y} {}", found.0, found.2)),
                ),
            ]),
        ),
        (
            "hover_event".into(),
            Tag::Compound(vec![
                ("action".into(), Tag::String("show_text".into())),
                (
                    "value".into(),
                    Tag::Compound(vec![(
                        "translate".into(),
                        Tag::String("chat.coordinates.tooltip".into()),
                    )]),
                ),
            ]),
        ),
    ]);
    let _ = origin;
    crate::command_execution::feedback(
        state,
        connection,
        key,
        vec![
            text(found_name),
            coordinates,
            text(found_distance(origin, found, include_y).to_string()),
        ],
    );
}

fn found_distance(origin: (i32, i32, i32), found: (i32, i32, i32), include_y: bool) -> i32 {
    let dx = f64::from(found.0 - origin.0);
    let dz = f64::from(found.2 - origin.2);
    if include_y {
        let dy = f64::from(found.1 - origin.1);
        (dx * dx + dy * dy + dz * dz).sqrt().floor() as i32
    } else {
        (dx * dx + dz * dz).sqrt().floor() as i32
    }
}

/// Vanilla `sendFailure`: red translatable component.
fn failure(state: &mut TickState, connection: ConnectionId, key: &str, arguments: Vec<Tag>) {
    let Tag::Compound(mut fields) = translated(key, arguments) else {
        unreachable!("translated builds a compound");
    };
    fields.push(("color".into(), Tag::String("red".into())));
    state.push_player(
        connection,
        PlayerChange::StyledChat {
            content: Tag::Compound(fields),
        },
    );
}

fn feedback(state: &mut TickState, connection: ConnectionId, key: &str, arguments: Vec<Tag>) {
    crate::command_execution::feedback(state, connection, key, arguments);
}

#[cfg(test)]
#[path = "command_locate_tests.rs"]
mod tests;
