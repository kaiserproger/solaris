use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_world::block::BlockStateId;
use mc_world::chunk::{Chunk, ChunkPos, MAX_Y, MIN_Y};
use mc_world::light::{
    LightCache, LightWorkspace, apply_block_change_to_light, compute_chunk_light_in,
};
use mc_world::section::SECTION_DIM;

const AIR: BlockStateId = BlockStateId(0);
const STONE: BlockStateId = BlockStateId(1);
const DIRT: BlockStateId = BlockStateId(2);
const GRASS: BlockStateId = BlockStateId(3);
const GLOWSTONE: BlockStateId = BlockStateId(4);

fn light_table() -> BlockLightTable {
    BlockLightTable::from_arrays(
        "bench",
        vec![0, 0, 0, 0, 15],
        vec![0, 15, 15, 15, 0],
        vec![true, false, false, false, true],
    )
}

fn plains() -> Identifier {
    Identifier::parse("minecraft:plains").expect("static identifier")
}

fn chunk_with_height(pos: ChunkPos, height_at: impl Fn(i32, i32) -> i32) -> Chunk {
    let mut chunk = Chunk::empty(pos, AIR, plains());
    for lz in 0..SECTION_DIM as u8 {
        for lx in 0..SECTION_DIM as u8 {
            let wx = pos.x * SECTION_DIM as i32 + lx as i32;
            let wz = pos.z * SECTION_DIM as i32 + lz as i32;
            let height = height_at(wx, wz).clamp(MIN_Y + 1, MAX_Y - 2);
            for y in MIN_Y..height - 3 {
                let _ = chunk.set_block(lx, y, lz, STONE);
            }
            for y in height - 3..height {
                let _ = chunk.set_block(lx, y, lz, DIRT);
            }
            let _ = chunk.set_block(lx, height, lz, GRASS);
        }
    }
    chunk
}

fn flat_chunk(pos: ChunkPos) -> Chunk {
    chunk_with_height(pos, |_wx, _wz| -61)
}

fn noisy_chunk(pos: ChunkPos) -> Chunk {
    chunk_with_height(pos, |wx, wz| {
        let n = ((wx * 73 + wz * 151) ^ (wx * wz * 17)).rem_euclid(17);
        64 + n - 8
    })
}

fn neighbourhood(build: impl Fn(ChunkPos) -> Chunk) -> [[Option<Chunk>; 3]; 3] {
    std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            let pos = ChunkPos {
                x: dx as i32 - 1,
                z: dz as i32 - 1,
            };
            Some(build(pos))
        })
    })
}

fn borrow(chunks: &[[Option<Chunk>; 3]; 3]) -> [[Option<&Chunk>; 3]; 3] {
    std::array::from_fn(|dz| std::array::from_fn(|dx| chunks[dz][dx].as_ref()))
}

fn borrow_around(
    chunks: &[[Option<Chunk>; 3]; 3],
    centre_dx: usize,
    centre_dz: usize,
) -> [[Option<&Chunk>; 3]; 3] {
    std::array::from_fn(|dz| {
        std::array::from_fn(|dx| {
            let sx = centre_dx as isize + dx as isize - 1;
            let sz = centre_dz as isize + dz as isize - 1;
            if (0..3).contains(&sx) && (0..3).contains(&sz) {
                chunks[sz as usize][sx as usize].as_ref()
            } else {
                None
            }
        })
    })
}

fn seed_cache(chunks: &[[Option<Chunk>; 3]; 3], table: &BlockLightTable) -> LightCache {
    let mut cache = LightCache::new();
    let mut ws = LightWorkspace::new();
    for dz in 0..3 {
        for dx in 0..3 {
            let Some(chunk) = chunks[dz][dx].as_ref() else {
                continue;
            };
            let refs = borrow_around(chunks, dx, dz);
            let light = compute_chunk_light_in(&mut ws, refs, table);
            cache.insert(chunk.pos, light);
        }
    }
    cache
}

fn edit_storm(mut chunks: [[Option<Chunk>; 3]; 3], mut cache: LightCache, table: &BlockLightTable) {
    const EDITS: [(u8, i32, u8, BlockStateId); 12] = [
        (8, 68, 8, GLOWSTONE),
        (8, 68, 8, AIR),
        (1, 66, 1, GLOWSTONE),
        (1, 66, 1, AIR),
        (15, 67, 8, GLOWSTONE),
        (15, 67, 8, AIR),
        (8, 72, 15, STONE),
        (8, 72, 15, AIR),
        (3, 60, 12, GLOWSTONE),
        (3, 60, 12, AIR),
        (12, 74, 3, STONE),
        (12, 74, 3, AIR),
    ];

    let centre_pos = ChunkPos { x: 0, z: 0 };
    for (lx, y, lz, new_state) in EDITS {
        let centre = chunks[1][1].as_mut().expect("centre chunk present");
        let prev = centre
            .set_block(lx, y, lz, new_state)
            .expect("edit y inside world");
        let refs = borrow(&chunks);
        let touched = apply_block_change_to_light(
            &mut cache, &refs, table, centre_pos, lx, y, lz, prev, new_state,
        );
        black_box(touched);
    }
}

fn bench_full_recompute(c: &mut Criterion) {
    let table = light_table();
    let flat = neighbourhood(flat_chunk);
    let noisy = neighbourhood(noisy_chunk);

    c.bench_function("light/full_recompute/flat", |b| {
        let mut ws = LightWorkspace::new();
        b.iter(|| {
            let light =
                compute_chunk_light_in(&mut ws, borrow(black_box(&flat)), black_box(&table));
            black_box(light);
        });
    });

    c.bench_function("light/full_recompute/noisy", |b| {
        let mut ws = LightWorkspace::new();
        b.iter(|| {
            let light =
                compute_chunk_light_in(&mut ws, borrow(black_box(&noisy)), black_box(&table));
            black_box(light);
        });
    });
}

fn bench_incremental(c: &mut Criterion) {
    let table = light_table();
    let flat = neighbourhood(flat_chunk);
    let noisy = neighbourhood(noisy_chunk);
    let flat_cache = seed_cache(&flat, &table);
    let noisy_cache = seed_cache(&noisy, &table);

    c.bench_function("light/incremental_edit_storm/flat", |b| {
        b.iter_batched(
            || (flat.clone(), flat_cache.clone()),
            |(chunks, cache)| edit_storm(chunks, cache, black_box(&table)),
            BatchSize::SmallInput,
        );
    });

    c.bench_function("light/incremental_edit_storm/noisy", |b| {
        b.iter_batched(
            || (noisy.clone(), noisy_cache.clone()),
            |(chunks, cache)| edit_storm(chunks, cache, black_box(&table)),
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_full_recompute, bench_incremental);
criterion_main!(benches);
