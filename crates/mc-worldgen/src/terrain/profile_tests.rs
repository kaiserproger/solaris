//! Release-only stage measurements; timers are absent from production builds.
use super::*;
use std::cell::RefCell;
use std::time::{Duration, Instant};

thread_local! {
    static TOTALS: RefCell<[Duration; 9]> = const { RefCell::new([Duration::ZERO; 9]) };
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
#[ignore = "requires the local licensed vanilla content cache"]
fn vanilla_fixed_seed_region_block_fingerprint() {
    let generator = generator(712816);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for z in -2..2 {
        for x in -2..2 {
            let chunk = generator.generate(ChunkPos { x, z });
            for y in chunk.geometry().min_y()..chunk.geometry().max_y() {
                for lz in 0..16 {
                    for lx in 0..16 {
                        for byte in chunk.get_block(lx, y, lz).unwrap().0.to_le_bytes() {
                            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
                        }
                    }
                }
            }
        }
    }
    assert_eq!(hash, 7_212_625_076_892_161_529);
}

#[test]
#[ignore = "release stage profile; approved measurement window only"]
fn vanilla_stages() {
    stage_profile(&[712816, 5617830, 42], 3, Solids::Alone);
}

/// `vanilla_stages` with densities from the GPU: the device and program are
/// built outside the timer; the GPU fill of the profiled chunks plus one ring
/// (64 chunks), the grid and the 36 generations are timed together.
#[test]
#[ignore = "release stage profile with GPU density; needs --features gpu and a GPU"]
fn vanilla_stages_gpu_density() {
    stage_profile(&[712816, 5617830, 42], 3, Solids::Gpu);
}

/// `vanilla_stages` as chunk workers generate with the shared solids cache in
/// steady state: each chunk's density is filled once on the CPU and read by
/// the generations around it. The 64 fills (profiled chunks plus one ring) are
/// timed apart and one fill per chunk is added to `ms_per_chunk`.
#[test]
#[ignore = "release stage profile; approved measurement window only"]
fn vanilla_stages_shared_solids() {
    stage_profile(&[712816, 5617830, 42], 3, Solids::Shared);
}

#[test]
#[ignore = "release single-threaded parity-seed profile; approved measurement window only"]
fn parity_stages() {
    stage_profile(&[0, 81, 497625], 1, Solids::Alone);
}

#[derive(Clone, Copy, PartialEq)]
enum Solids {
    /// Each generation evaluates its own density and neighbour columns.
    Alone,
    Gpu,
    Shared,
}

#[cfg(feature = "gpu")]
type Gpu = crate::vanilla_density::gpu::GpuDensity;
#[cfg(not(feature = "gpu"))]
type Gpu = ();

#[cfg(feature = "gpu")]
fn gpu_device(generator: &TerrainGenerator) -> Gpu {
    let (_, density) = generator.vanilla().expect("vanilla worldgen");
    Gpu::new(density).unwrap()
}

#[cfg(not(feature = "gpu"))]
fn gpu_device(_: &TerrainGenerator) -> Gpu {
    panic!("GPU density profile needs --features gpu")
}

/// GPU solids of the profiled chunks and a one-chunk ring around them (what
/// an ore halo reaches), in one batch.
#[cfg(feature = "gpu")]
fn gpu_densities(gpu: &Gpu) -> SolidGrid {
    let ring: Vec<ChunkPos> = (-4..4)
        .flat_map(|z| (-4..4).map(move |x| ChunkPos { x, z }))
        .collect();
    let origins: Vec<(i32, i32)> = ring.iter().map(|p| (p.x * 16, p.z * 16)).collect();
    let solids = gpu.fill_chunks(&origins).unwrap().chunks;
    let mut grid = SolidGrid::default();
    for (pos, solids) in ring.into_iter().zip(solids) {
        grid.insert(pos, std::sync::Arc::new(solids));
    }
    grid
}

#[cfg(not(feature = "gpu"))]
fn gpu_densities(_: &Gpu) -> SolidGrid {
    unreachable!()
}

/// [`gpu_densities`]'s chunks filled on the CPU.
fn cpu_densities(generator: &TerrainGenerator) -> SolidGrid {
    let (_, density) = generator.vanilla().expect("vanilla worldgen");
    let mut grid = SolidGrid::default();
    for z in -4..4 {
        for x in -4..4 {
            let pos = ChunkPos { x, z };
            grid.insert(pos, std::sync::Arc::new(chunk_solids(density, pos)));
        }
    }
    grid
}

/// Blocks and biomes of chunks generated through the GPU service against the
/// CPU path, on separate generators so no cached column hides a difference.
/// Four threads generate at once, as chunk workers do, so batches mix their
/// requests.
#[test]
#[ignore = "GPU worldgen experiment; needs --features gpu and a GPU"]
fn gpu_density_chunks_match_cpu_chunks() {
    for seed in [712816, 42] {
        let cpu = generator(seed);
        let mut gpu = generator(seed);
        gpu.enable_gpu().unwrap();
        let positions: Vec<ChunkPos> = (-3..3)
            .flat_map(|z| (-3..3).map(move |x| ChunkPos { x, z }))
            .collect();
        let generated: Vec<Chunk> = std::thread::scope(|scope| {
            let gpu = &gpu;
            let workers: Vec<_> = positions
                .chunks(9)
                .map(|part| {
                    scope.spawn(move || {
                        part.iter()
                            .map(|&pos| gpu.generate(pos))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect()
        });
        let (mut blocks, mut biomes) = (0, 0);
        for (&pos, actual) in positions.iter().zip(&generated) {
            let expected = cpu.generate_with_solids(pos, None);
            let min_y = expected.geometry().min_y();
            for y in min_y..expected.geometry().max_y() {
                for column in 0..256u16 {
                    let (lx, lz) = ((column % 16) as u8, (column / 16) as u8);
                    blocks +=
                        usize::from(expected.get_block(lx, y, lz) != actual.get_block(lx, y, lz));
                }
            }
            for (e, a) in expected.biomes.iter().zip(&actual.biomes) {
                for cell in 0..64u8 {
                    let (cx, cy, cz) = (cell % 4, cell / 16, cell / 4 % 4);
                    biomes += usize::from(e.get(cx, cy, cz) != a.get(cx, cy, cz));
                }
            }
        }
        println!(
            "EQUIVALENCE seed={seed} differing_blocks={blocks} differing_biome_cells={biomes} chunks=36"
        );
        assert_eq!(biomes, 0, "seed {seed}");
        assert!(blocks <= 36, "seed {seed}: {blocks} blocks differ");
    }
}

fn stage_profile(seeds: &[i64], runs: usize, solids: Solids) {
    let report = mc_data::blocks::solaris_required_blocks_report();
    let light_table =
        mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&report);
    let positions: Vec<ChunkPos> = (-3..3)
        .flat_map(|z| (-3..3).map(move |x| ChunkPos { x, z }))
        .collect();
    for &seed in seeds {
        for run in 0..runs {
            let generator = generator(seed);
            let device = (solids == Solids::Gpu).then(|| gpu_device(&generator));
            let shared = (solids == Solids::Shared).then(|| {
                let start = Instant::now();
                let grid = cpu_densities(&generator);
                (grid, start.elapsed().as_secs_f64() * 1000.0 / 64.0)
            });
            TOTALS.with_borrow_mut(|totals| *totals = [Duration::ZERO; 9]);
            let start = Instant::now();
            let densities = device.as_ref().map(gpu_densities);
            let fill = match &shared {
                Some((_, per_chunk)) => *per_chunk,
                None => start.elapsed().as_secs_f64() * 1000.0 / 36.0,
            };
            let grid = densities.as_ref().or(shared.as_ref().map(|(grid, _)| grid));
            let chunks: Vec<_> = positions
                .iter()
                .map(|&pos| generator.generate_with_solids(pos, grid))
                .collect();
            let mut elapsed = start.elapsed().as_secs_f64() * 1000.0 / 36.0;
            if shared.is_some() {
                elapsed += fill;
            }
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
            // What chunk workers pay: light baked with no neighbours loaded.
            let start = Instant::now();
            for chunk in &chunks {
                std::hint::black_box(mc_world::light::compute_chunk_light_in(
                    &mut workspace,
                    [
                        [None, None, None],
                        [None, Some(chunk), None],
                        [None, None, None],
                    ],
                    &light_table,
                ));
            }
            let bake = start.elapsed().as_secs_f64() * 1000.0 / 36.0;
            println!(
                "STAGES seed={seed} run={run} chunks=36 ms_per_chunk={elapsed} fill={fill} density={} climate_biome={} surface_fill={} carvers={} ores={} decorations={} structure_paste={} villages_jigsaw={} other={} lighting={lighting} bake={bake}",
                stages[0],
                stages[6],
                stages[1],
                stages[2],
                stages[3],
                stages[7],
                stages[8],
                stages[4],
                stages[5]
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
