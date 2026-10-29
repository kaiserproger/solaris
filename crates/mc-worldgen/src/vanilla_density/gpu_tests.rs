use std::time::Instant;

use super::*;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
}

/// The density shader parses and validates (naga, no GPU or game data).
#[test]
fn density_shader_validates() {
    use wgpu::naga::valid::{Capabilities, ValidationFlags, Validator};
    let source = shader_source(1024);
    let module = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
}

/// Solid bits against `fill_density_chunk` near spawn and at far coordinates
/// (up to the world border), then GPU wall time per chunk by batch size.
#[test]
#[ignore = "GPU worldgen experiment; needs a GPU, run in the serialized perf lane"]
fn gpu_chunk_density_matches_cpu_and_reports_time() {
    for seed in [712816, 42] {
        let density = VanillaDensity::new(seed, root()).unwrap();
        let gpu = GpuDensity::new(&density).unwrap();
        let height = density.height() as usize;
        let blocks = height * 256;
        let mut chunks: Vec<(i32, i32)> = (-3..3)
            .flat_map(|z| (-3..3).map(move |x| (x * 16, z * 16)))
            .collect();
        let near = chunks.len();
        chunks.extend([
            (1_000_000, -2_000_000),
            (-12_345_600, 7_654_320),
            (29_999_984, 16),
            (-29_999_984, -29_999_984),
        ]);
        let result = gpu.fill_chunks(&chunks).unwrap();
        let mut cpu = vec![0.0; blocks];
        let mut cpu_time = 0.0;
        for (index, &(x, z)) in chunks.iter().enumerate() {
            let start = Instant::now();
            density.fill_density_chunk(x, z, &mut cpu);
            if index < near {
                cpu_time += start.elapsed().as_secs_f64() * 1000.0;
            }
            let expected = ChunkSolids::from_densities(&cpu, height);
            let actual = &result.chunks[index];
            let flips = (0..256)
                .flat_map(|column| (0..height).map(move |offset| (column, offset)))
                .filter(|&(column, offset)| {
                    expected.solid(column, offset) != actual.solid(column, offset)
                })
                .count();
            println!("ACCURACY seed={seed} chunk=({x},{z}) sign_flips={flips} of {blocks}");
            // Earlier f32 comparisons: |gpu - cpu| <= 8e-6, flips only where
            // |density| < 1e-6.
            assert!(flips <= 2, "seed {seed} chunk ({x},{z}): {flips} flips");
        }
        println!(
            "CPU seed={seed} fill_density_chunk ms_per_chunk={:.3}",
            cpu_time / near as f64
        );
        for batch in [1usize, 8, 32, 64, 128] {
            let set: Vec<(i32, i32)> = (0..batch as i32).map(|i| (i * 16, 4096)).collect();
            gpu.fill_chunks(&set).unwrap();
            let mut walls = Vec::new();
            let mut passes = Vec::new();
            for _ in 0..3 {
                let start = Instant::now();
                let result = gpu.fill_chunks(&set).unwrap();
                walls.push(start.elapsed().as_secs_f64() * 1000.0 / batch as f64);
                passes.push(result.pass_ms.map(|ms| ms.map(|m| m / batch as f64)));
            }
            println!(
                "GPU seed={seed} batch={batch} wall_ms_per_chunk={walls:.3?} pass_ms_per_chunk(flat,corner,block)={passes:.3?}"
            );
        }
    }
}
