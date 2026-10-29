//! Release-only stage measurements; timers are absent from production builds.
use super::*;
use std::cell::RefCell;
use std::time::{Duration, Instant};

thread_local! {
    static TOTALS: RefCell<[Duration; 7]> = const { RefCell::new([Duration::ZERO; 7]) };
}
pub(super) struct StageClock(Instant);
impl StageClock {
    pub(super) fn new() -> Self {
        Self(Instant::now())
    }
    pub(super) fn end(&mut self, stage: usize) {
        let now = Instant::now();
        TOTALS.with_borrow_mut(|totals| totals[stage] += now - self.0);
        self.0 = now;
    }
}
fn generator(seed: i64) -> TerrainGenerator {
    use crate::vanilla_features::{BlockSemantics, CacheTags};
    use crate::village::plan_source::VillagePlanSource;
    use crate::village::{VillageDecor, load_village_closure};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let data = Arc::new(mc_data::load(&root).unwrap());
    let tags = Arc::new(mc_data::tags::load(&root, &data).unwrap());
    let tags = Arc::new(CacheTags::new(blocks.clone(), data, tags));
    let closure = Arc::new(
        load_village_closure(
            &root,
            &Identifier::parse("minecraft:villages").unwrap(),
            &blocks,
        )
        .unwrap(),
    );
    let decor = Arc::new(
        VillageDecor::compile(
            &root,
            &closure,
            &BlockSemantics::new(&blocks, tags.as_ref()),
        )
        .unwrap(),
    );
    let mut generator = TerrainGenerator::new(seed, blocks.clone());
    generator.load_vanilla_worldgen(&root).unwrap();
    generator.with_village_plans(Arc::new(
        VillagePlanSource::new(closure, decor, seed, blocks, tags.clone(), tags).unwrap(),
    ))
}

#[test]
#[ignore = "release stage profile; approved measurement window only"]
fn vanilla_stages() {
    stage_profile(&[712816, 5617830, 42], 3);
}

#[test]
#[ignore = "release single-threaded parity-seed profile; approved measurement window only"]
fn parity_stages() {
    stage_profile(&[0, 81, 497625], 1);
}

fn stage_profile(seeds: &[i64], runs: usize) {
    let report = mc_data::blocks::solaris_required_blocks_report();
    let light_table =
        mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&report);
    for &seed in seeds {
        for run in 0..runs {
            let generator = generator(seed);
            TOTALS.with_borrow_mut(|totals| *totals = [Duration::ZERO; 7]);
            let start = Instant::now();
            let mut chunks = Vec::with_capacity(36);
            for z in -3..3 {
                for x in -3..3 {
                    chunks.push(generator.generate(ChunkPos { x, z }));
                }
            }
            let elapsed = start.elapsed().as_secs_f64() * 1000.0 / 36.0;
            let stages =
                TOTALS.with_borrow(|totals| totals.map(|v| v.as_secs_f64() * 1000.0 / 36.0));
            let mut workspace = mc_world::light::LightWorkspace::new();
            let start = Instant::now();
            for z in 1..5 {
                for x in 1..5 {
                    let neighbours = std::array::from_fn(|dz| {
                        std::array::from_fn(|dx| Some(&chunks[(z + dz - 1) * 6 + x + dx - 1]))
                    });
                    std::hint::black_box(mc_world::light::compute_chunk_light_in(
                        &mut workspace,
                        neighbours,
                        &light_table,
                    ));
                }
            }
            let lighting = start.elapsed().as_secs_f64() * 1000.0 / 16.0;
            println!(
                "STAGES seed={seed} run={run} chunks=36 ms_per_chunk={elapsed} density={} climate_biome={} surface_fill={} carvers={} features={} structures={} other={} lighting={lighting}",
                stages[0], stages[6], stages[1], stages[2], stages[3], stages[4], stages[5]
            );
        }
    }
}

/// Binary fixture contains generated surface heights and biome strings per column,
/// independent of numeric representation. It can be compared across frozen builds.
#[test]
#[ignore = "writes similarity fixture only to an explicitly supplied path"]
fn similarity_snapshot() {
    use std::io::Write;
    let path = std::env::var_os("GEN32_SNAPSHOT").expect("GEN32_SNAPSHOT output path");
    let mut out = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for seed in [712816, 5617830, 42] {
        let generator = generator(seed);
        for z in -10..10 {
            for x in -10..10 {
                let chunk = generator.generate(ChunkPos { x, z });
                for lz in 0..16 {
                    for lx in 0..16 {
                        let height = chunk.heightmaps["WORLD_SURFACE"].get(lx, lz);
                        out.write_all(&height.to_le_bytes()).unwrap();
                        let biome = generator.noise_biome(
                            x * 16 + i32::from(lx),
                            height as i32 + generator.geometry.min_y() - 1,
                            z * 16 + i32::from(lz),
                        );
                        let name = biome.as_str().as_bytes();
                        out.write_all(&(name.len() as u16).to_le_bytes()).unwrap();
                        out.write_all(name).unwrap();
                    }
                }
            }
        }
    }
    out.flush().unwrap();
}

#[path = "work_profile_tests.rs"]
mod work;
