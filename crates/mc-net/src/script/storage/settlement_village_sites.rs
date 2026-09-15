//! Core-generated vanilla villages, as settlement sites.
//!
//! A vanilla village is a structure the world's own generator decided to place,
//! and this module is the settlement side's description of one. Three
//! properties are load-bearing and deliberate:
//!
//! - **Identity comes from the generator, not from a position.** The village a
//!   plan assembles is a pure function of the world seed and its start chunk
//!   ([`VillagePlanSource::plan_for_start_chunk`]), so the site id is minted from
//!   the world's identity, the dimension and that start chunk and can be
//!   reversed without trusting the caller. Nothing here reads where a villager
//!   happens to stand.
//! - **The descriptor is derived on read; nothing is registered.** There is no
//!   registry of "known villages": an entry would be a second authority that
//!   drifts from generation the moment a chunk is unloaded, a world is restored
//!   or a region regenerates. Listing a page asks the generator's placement
//!   formula about the cells the page names, exactly as vanilla's `/locate`
//!   enumerates a structure without generating the world. Only adoption state —
//!   the resident ledger and its reservations — is durable.
//! - **What the village contains comes from what was materialized.** The points
//!   of interest and the inhabitant identities a site reports are read from the
//!   chunks the world actually generated (its stored blocks and its stored
//!   settlement inhabitant markers), never re-derived from the templates: a
//!   processor may replace a block, and a re-derivation would then report a bed
//!   the world does not have. A page whose chunks are not generated reports the
//!   site's identity, region and pieces, and says so for the rest.
//!
//! [`VillagePlanSource::plan_for_start_chunk`]: mc_worldgen::village::plan_source::VillagePlanSource

use std::sync::Arc;

use sha2::{Digest, Sha256};

use mc_world::{ChunkGenerator, GeneratedVillageSite};
use mc_worldgen::settlement_sites::SITE_CELL_BLOCKS;

/// The prefix every generated-village site id carries.
const VILLAGE_SITE_PREFIX: &str = "village_";

/// The identity domain, so a village id can never collide with another id this
/// module family mints.
const VILLAGE_SITE_DOMAIN: &[u8] = b"solaris.village.site.v1";

/// The blocks vanilla 26.1.2 registers as village job-site points of interest.
///
/// Pinned from the server's own registration rather than from memory:
/// `PoiTypes.bootstrap` (`net/minecraft/world/entity/ai/village/poi/PoiTypes`),
/// decompiled locally from `.analysis/server.jar`, registers ARMORER on
/// `blast_furnace`, BUTCHER on `smoker`, CARTOGRAPHER on `cartography_table`,
/// CLERIC on `brewing_stand`, FARMER on `composter`, FISHERMAN on `barrel`,
/// FLETCHER on `fletching_table`, LEATHERWORKER on the four cauldrons,
/// LIBRARIAN on `lectern`, MASON on `stonecutter`, SHEPHERD on `loom`, TOOLSMITH
/// on `smithing_table` and WEAPONSMITH on `grindstone`. Those thirteen job-site
/// types are exactly `#minecraft:acquirable_job_site`
/// (`data/minecraft/tags/point_of_interest_type/acquirable_job_site.json`), and
/// the village tag adds `home` (the bed tag) and `meeting` (the bell). The
/// decompiled registration is kept as a receipt at
/// `.analysis/codex-logs/settlement-village-bridge/poi-types-registration.txt`.
///
/// This is the *point-of-interest* classification, which is wider than the
/// professions the resident lane can model today (none, nitwit, toolsmith): a
/// village whose only workstation is a composter really does have a job site,
/// and under-reporting it would misdescribe the village. The two limits are
/// separate — the profession slice governs which job a resident can hold, this
/// table governs which blocks a village's points of interest are.
const JOB_SITE_BLOCKS: &[&str] = &[
    "blast_furnace",
    "smoker",
    "cartography_table",
    "brewing_stand",
    "composter",
    "barrel",
    "fletching_table",
    "cauldron",
    "lava_cauldron",
    "water_cauldron",
    "powder_snow_cauldron",
    "lectern",
    "stonecutter",
    "loom",
    "smithing_table",
    "grindstone",
];

/// `PoiTypes.MEETING`'s only block: the village bell.
const MEETING_BLOCK: &str = "bell";

/// The capacity the meeting point has, as vanilla registers it.
///
/// `PoiTypes.register` takes the type's ticket count: 1 for every job site and
/// for a bed, and 32 for the meeting point — the bell is one block many
/// villagers may treat as their gathering place.
const MEETING_CAPACITY: u16 = 32;
/// Every other village point of interest is claimed by one villager.
const POI_CAPACITY: u16 = 1;

/// The point-of-interest role one generated block provides, or `None` for a
/// block no village point-of-interest type matches.
///
/// `is_bed` is the caller's reading of the `minecraft:beds` block tag, which is
/// what `PoiTypes.HOME` registers against; the other two roles are single block
/// identities and need no tag.
pub(crate) fn village_poi_kind(
    block: &mc_data::Identifier,
    is_bed: bool,
) -> Option<mc_script::ScriptSitePoiKind> {
    if !matches!(block.namespace(), "minecraft") {
        return None;
    }
    if is_bed {
        return Some(mc_script::ScriptSitePoiKind::Home);
    }
    let path = block.path();
    if path == MEETING_BLOCK {
        return Some(mc_script::ScriptSitePoiKind::Meeting);
    }
    JOB_SITE_BLOCKS
        .contains(&path)
        .then_some(mc_script::ScriptSitePoiKind::Work)
}

/// The capacity one point of interest of `kind` has.
pub(crate) const fn village_poi_capacity(kind: mc_script::ScriptSitePoiKind) -> u16 {
    match kind {
        mc_script::ScriptSitePoiKind::Meeting => MEETING_CAPACITY,
        // Every other village point of interest is claimed by one villager; a
        // role added later decides its own capacity here rather than inheriting
        // this one silently.
        _ => POI_CAPACITY,
    }
}

/// How many digest bytes a site id carries: its whole job is to make a forged
/// start chunk fail closed, not to be a security boundary.
const VILLAGE_SITE_DIGEST_BYTES: usize = 4;

/// The generator's own village enumeration, as the settlement runtime sees it.
///
/// Core installs one adapter over the world's terrain generator; the settlement
/// tests install an in-memory one. Nothing else in the settlement lane reaches
/// generation.
pub(crate) trait VillageSiteGround: Send + Sync {
    /// Every vanilla village whose start chunk lies in the inclusive chunk
    /// rectangle, in ascending start-chunk order.
    ///
    /// This is the placement formula and nothing more: it generates no chunk and
    /// reads no world state, so a caller can enumerate the villages a bounded
    /// region holds before any of them exists.
    fn village_sites_in_region(
        &self,
        min_chunk: (i32, i32),
        max_chunk: (i32, i32),
    ) -> Vec<GeneratedVillageSite>;
}

/// The one production village source: the world's own chunk generator.
///
/// A generator that places no villages answers with an empty list, so a world
/// without the `minecraft:villages` set needs no special case here.
pub(crate) struct GeneratorVillageSites {
    generator: Arc<dyn ChunkGenerator>,
}

impl GeneratorVillageSites {
    pub(crate) fn new(generator: Arc<dyn ChunkGenerator>) -> Self {
        Self { generator }
    }
}

impl VillageSiteGround for GeneratorVillageSites {
    fn village_sites_in_region(
        &self,
        min_chunk: (i32, i32),
        max_chunk: (i32, i32),
    ) -> Vec<GeneratedVillageSite> {
        self.generator.village_sites_in_region(min_chunk, max_chunk)
    }
}

/// The chunk rectangle one settlement grid cell covers, inclusive.
///
/// The village enumeration and the authored selector walk the same cell grid, so
/// one page of cells is one region for both.
pub(crate) fn cell_chunk_bounds(cell: [i32; 2]) -> ((i32, i32), (i32, i32)) {
    const CHUNK_AXIS: i32 = 16;
    let min_chunk_x = cell[0]
        .saturating_mul(SITE_CELL_BLOCKS)
        .div_euclid(CHUNK_AXIS);
    let min_chunk_z = cell[1]
        .saturating_mul(SITE_CELL_BLOCKS)
        .div_euclid(CHUNK_AXIS);
    // A cell is `SITE_CELL_BLOCKS / CHUNK_AXIS` chunks wide; the last one is
    // inclusive, so the rectangle is `min .. min + width - 1`.
    let width = (SITE_CELL_BLOCKS / CHUNK_AXIS).max(1);
    (
        (min_chunk_x, min_chunk_z),
        (
            min_chunk_x.saturating_add(width - 1),
            min_chunk_z.saturating_add(width - 1),
        ),
    )
}

/// The stable site id of the village the generator started in `start_chunk`.
///
/// Stable across restarts and independent of who asks or from where, because it
/// is minted from what identifies the village: the world, the dimension and the
/// start chunk the placement formula named.
pub(crate) fn village_site_id(
    world_identity: &str,
    dimension: &str,
    start_chunk: (i32, i32),
) -> String {
    let digest = village_site_digest(world_identity, dimension, start_chunk);
    format!(
        "{VILLAGE_SITE_PREFIX}{}_{}_{digest}",
        start_chunk.0, start_chunk.1
    )
}

/// The start chunk a village site id names, or `None` when the id names no
/// village of this world.
///
/// The digest is re-derived, so an id for another world, another dimension or a
/// chunk no placement formula ever started is refused rather than scanned.
pub(crate) fn village_site_start_chunk(
    world_identity: &str,
    dimension: &str,
    site_id: &str,
) -> Option<(i32, i32)> {
    let rest = site_id.strip_prefix(VILLAGE_SITE_PREFIX)?;
    let mut parts = rest.split('_');
    let chunk_x: i32 = parts.next()?.parse().ok()?;
    let chunk_z: i32 = parts.next()?.parse().ok()?;
    let digest = parts.next()?;
    if parts.next().is_some() || digest.len() != VILLAGE_SITE_DIGEST_BYTES * 2 {
        return None;
    }
    let expected = village_site_digest(world_identity, dimension, (chunk_x, chunk_z));
    (digest == expected).then_some((chunk_x, chunk_z))
}

/// The digest a site id carries: SHA-256 over the identity domain, the world,
/// the dimension and the start chunk, each length-framed.
fn village_site_digest(world_identity: &str, dimension: &str, start_chunk: (i32, i32)) -> String {
    let mut hasher = Sha256::new();
    hasher.update(VILLAGE_SITE_DOMAIN);
    for part in [world_identity, dimension] {
        hasher.update((part.len() as u32).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.update(start_chunk.0.to_le_bytes());
    hasher.update(start_chunk.1.to_le_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(VILLAGE_SITE_DIGEST_BYTES * 2);
    for byte in &digest[..VILLAGE_SITE_DIGEST_BYTES] {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_village_site_id_reverses_to_the_chunk_it_names() {
        let start = (1234, -7);
        let site_id = village_site_id("world-a", "minecraft:overworld", start);
        assert_eq!(
            village_site_start_chunk("world-a", "minecraft:overworld", &site_id),
            Some(start)
        );
        // The id is a village id, not another family's, and reversing one is
        // scoped to the world and dimension that minted it.
        assert!(site_id.starts_with("village_"));
        assert_eq!(
            village_site_start_chunk("world-b", "minecraft:overworld", &site_id),
            None
        );
        assert_eq!(
            village_site_start_chunk("world-a", "minecraft:the_nether", &site_id),
            None
        );
    }

    #[test]
    fn a_village_site_id_is_stable_and_refuses_a_forged_chunk() {
        let start = (12, 34);
        let first = village_site_id("world-a", "minecraft:overworld", start);
        let second = village_site_id("world-a", "minecraft:overworld", start);
        assert_eq!(first, second, "the same village has one id");

        // A different chunk is a different id, and swapping the digest for a
        // chunk the id does not name is refused instead of reversing.
        let other = village_site_id("world-a", "minecraft:overworld", (12, 35));
        assert_ne!(first, other);
        let forged = first.replace("12_34_", "12_35_");
        assert_eq!(
            village_site_start_chunk("world-a", "minecraft:overworld", &forged),
            None
        );
        for malformed in [
            "village_",
            "village_12",
            "village_12_34",
            "village_12_34_00",
            "site_12_34_00000000",
            "village_12_34_00000000_extra",
            "village_x_34_00000000",
        ] {
            assert_eq!(
                village_site_start_chunk("world-a", "minecraft:overworld", malformed),
                None,
                "{malformed:?} names no village"
            );
        }
    }

    /// The classification covers exactly the village's own registration: the
    /// thirteen `#minecraft:acquirable_job_site` blocks, the bell and the bed
    /// tag. Pinned against the decompiled `PoiTypes.bootstrap` kept at
    /// `.analysis/codex-logs/settlement-village-bridge/poi-types-registration.txt`.
    #[test]
    fn village_points_of_interest_follow_the_vanilla_registration() {
        let bed = mc_data::Identifier::parse("minecraft:red_bed".to_owned()).unwrap();
        assert_eq!(
            village_poi_kind(&bed, true),
            Some(mc_script::ScriptSitePoiKind::Home)
        );
        // A bed that the tag does not name is not a home: the tag is the
        // authority, not the block's name.
        assert_eq!(village_poi_kind(&bed, false), None);

        let bell = mc_data::Identifier::parse("minecraft:bell".to_owned()).unwrap();
        assert_eq!(
            village_poi_kind(&bell, false),
            Some(mc_script::ScriptSitePoiKind::Meeting)
        );
        for job_site in JOB_SITE_BLOCKS {
            let block = mc_data::Identifier::parse(format!("minecraft:{job_site}")).unwrap();
            assert_eq!(
                village_poi_kind(&block, false),
                Some(mc_script::ScriptSitePoiKind::Work),
                "{job_site} is one of the thirteen job-site blocks"
            );
        }
        // The village-relevant remainder of the registration is not a village
        // point of interest, and neither is an unrelated or foreign block.
        for other in [
            "minecraft:beehive",
            "minecraft:bee_nest",
            "minecraft:nether_portal",
            "minecraft:lodestone",
            "minecraft:lightning_rod",
            "minecraft:stone",
            "other:composter",
        ] {
            let block = mc_data::Identifier::parse(other.to_owned()).unwrap();
            assert_eq!(village_poi_kind(&block, false), None, "{other}");
        }

        // Capacity comes from the same registration: the bell takes 32
        // villagers, every other village point of interest one.
        assert_eq!(
            village_poi_capacity(mc_script::ScriptSitePoiKind::Meeting),
            MEETING_CAPACITY
        );
        assert_eq!(
            village_poi_capacity(mc_script::ScriptSitePoiKind::Home),
            POI_CAPACITY
        );
        assert_eq!(
            village_poi_capacity(mc_script::ScriptSitePoiKind::Work),
            POI_CAPACITY
        );
        assert_eq!(
            JOB_SITE_BLOCKS.len(),
            16,
            "the thirteen job-site types are sixteen blocks: LEATHERWORKER's cauldron is four of them"
        );
    }

    #[test]
    fn a_cell_covers_the_chunk_rectangle_its_page_scan_names() {
        // The cell grid is 512 blocks wide, so one cell is 32 chunks. Cell zero
        // starts at the origin; a negative cell starts before it and never
        // claims the chunk on the positive side of the boundary.
        assert_eq!(cell_chunk_bounds([0, 0]), ((0, 0), (31, 31)));
        assert_eq!(cell_chunk_bounds([1, 0]), ((32, 0), (63, 31)));
        assert_eq!(cell_chunk_bounds([-1, -1]), ((-32, -32), (-1, -1)));
        let ((min_x, min_z), (max_x, max_z)) = cell_chunk_bounds([3, -2]);
        assert_eq!(max_x - min_x, 31);
        assert_eq!(max_z - min_z, 31);
        // Adjacent cells tile without gap or overlap.
        let right = cell_chunk_bounds([4, -2]);
        assert_eq!(right.0, (max_x + 1, min_z));
    }
}
