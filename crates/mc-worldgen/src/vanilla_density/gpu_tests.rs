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

/// Accuracy against `fill_density_chunk` near spawn and at far coordinates,
/// then GPU wall time per chunk by batch size next to the CPU fill.
#[test]
#[ignore = "GPU worldgen experiment; needs a GPU, run in the serialized perf lane"]
fn gpu_chunk_density_matches_cpu_and_reports_time() {
    for seed in [712816, 42] {
        let density = VanillaDensity::new(seed, root()).unwrap();
        let gpu = GpuDensity::new(&density).unwrap();
        let blocks = density.height() as usize * 256;
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
            let gpu = &result.values[index * blocks..][..blocks];
            let (mut max, mut flips, mut exact, mut over_1e5, mut over_1e3) = (0.0_f32, 0, 0, 0, 0);
            for (c, g) in cpu.iter().zip(gpu) {
                let diff = (c - g).abs();
                max = max.max(diff);
                flips += usize::from((*c > 0.0) != (*g > 0.0));
                exact += usize::from(c.to_bits() == g.to_bits());
                over_1e5 += usize::from(diff > 1e-5);
                over_1e3 += usize::from(diff > 1e-3);
            }
            println!(
                "ACCURACY seed={seed} chunk=({x},{z}) max_abs_diff={max:e} sign_flips={flips} exact={exact} over_1e-5={over_1e5} over_1e-3={over_1e3} of {blocks}"
            );
            assert!(max.is_finite(), "non-finite GPU density at ({x},{z})");
            if index < 2 {
                let layers: Vec<i32> = (0..density.height())
                    .filter(|y| {
                        let at = *y as usize * 256;
                        cpu[at..at + 256]
                            .iter()
                            .zip(&gpu[at..at + 256])
                            .any(|(c, g)| (c - g).abs() > 1e-5)
                    })
                    .map(|y| y + density.min_y())
                    .collect();
                println!("LAYERS seed={seed} chunk=({x},{z}) differing_y={layers:?}");
            }
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
            println!("GPU seed={seed} batch={batch} wall_ms_per_chunk={walls:.3?} pass_ms_per_chunk(flat,corner,block)={passes:.3?}");
        }
    }
}
