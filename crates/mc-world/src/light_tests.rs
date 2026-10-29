use super::*;

#[test]
fn packed_light_population_matches_cell_reads_at_every_palette_width() {
    use crate::section::{ChunkSection, PackedBitArray};
    let table = tiny_table();
    for count in [2_usize, 3, 5, 9, 17, 33, 65, 129, 257, 513] {
        let bits = (usize::BITS - (count - 1).leading_zeros()) as u8;
        let palette: Vec<_> = (0..count).map(|id| BlockStateId(id as u32)).collect();
        let mut indices = PackedBitArray::zeroed(bits, SECTION_VOLUME);
        for cell in 0..SECTION_VOLUME {
            indices.set(cell, ((cell * 31 + cell / 7) % count) as u32);
        }
        let section = ChunkSection::from_indirect(palette, indices, BlockStateId(0));
        let mut actual_opacity = vec![0; N_VOL];
        let mut actual_sky = vec![true; N_VOL];
        let mut actual_block = vec![0; N_VOL];
        let mut actual_emitters = Vec::new();
        populate_indirect_section(
            2,
            1,
            368,
            &section,
            &table,
            &mut actual_opacity,
            &mut actual_sky,
            &mut actual_block,
            &mut actual_emitters,
        );
        let mut expected_emitters = Vec::new();
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let state = section.get(x, y, z);
                    let idx = grid_idx(32 + x as usize, 368 + y as usize, 16 + z as usize);
                    assert_eq!(actual_opacity[idx], table.opacity(state.0).unwrap_or(0));
                    assert_eq!(
                        actual_sky[idx],
                        table.propagates_sky(state.0).unwrap_or(true)
                    );
                    let emission = table.emission(state.0).unwrap_or(0);
                    assert_eq!(actual_block[idx], emission);
                    if emission > 0 {
                        expected_emitters.push(idx as u32);
                    }
                }
            }
        }
        assert_eq!(actual_emitters, expected_emitters, "palette length={count}");
    }
}

fn extract_centre_reference(sky: &[u8], block: &[u8]) -> ChunkLight {
    let mut out = ChunkLight::zeroed();
    for local_y in 0..WORLD_HEIGHT {
        for local_z in 0..SECTION_DIM {
            for local_x in 0..SECTION_DIM {
                let source = grid_idx(SECTION_DIM + local_x, local_y, SECTION_DIM + local_z);
                out.set_sky_local(local_x, local_y, local_z, sky[source]);
                out.set_block_local(local_x, local_y, local_z, block[source]);
            }
        }
    }
    out
}
use mc_data::Identifier;

use crate::block::BlockRegistry;
use crate::chunk::{ChunkGeometry, ChunkPos, MAX_Y};

fn tiny_table() -> BlockLightTable {
    // State ids:
    //   0 = air (transparent, no emission, sky passes)
    //   1 = stone (opaque, no emission, sky blocked)
    //   2 = glass (transparent, sky passes — same shape as air
    //              for the purposes of this fixture)
    //   3 = glowstone (transparent emitter, emission=15)
    //   4 = water (opacity 1 soft attenuator, no emission, sky
    //              blocked at the source per vanilla's predicate)
    BlockLightTable::from_arrays(
        "test",
        vec![0, 0, 0, 15, 0],
        vec![0, 15, 0, 0, 1],
        vec![true, false, true, true, false],
    )
}

fn air_chunk() -> Chunk {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains)
}

fn custom_air_chunk() -> Chunk {
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let geometry = ChunkGeometry::new(0, 256).unwrap();
    Chunk::empty_with_geometry(ChunkPos { x: 0, z: 0 }, BlockStateId(0), plains, geometry)
}

fn solo(chunk: Chunk) -> [[Option<Chunk>; 3]; 3] {
    // Centre-only neighbourhood. Wrapped in Option so the borrow
    // pattern in `compute_chunk_light` matches.
    [
        [None, None, None],
        [None, Some(chunk), None],
        [None, None, None],
    ]
}

fn borrow(input: &[[Option<Chunk>; 3]; 3]) -> [[Option<&Chunk>; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| input[i][j].as_ref()))
}

#[test]
fn scalar_and_portable_extraction_are_bit_identical() {
    let mut sky = vec![0; N_VOL];
    let mut block = vec![0; N_VOL];
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    for value in sky.iter_mut().chain(&mut block) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *value = (state & 0x0f) as u8;
    }

    let scalar =
        extract_centre_with_backend(&sky, &block, LightKernelBackend::Scalar, OVERWORLD_GEOMETRY);
    let portable = extract_centre_with_backend(
        &sky,
        &block,
        LightKernelBackend::PortableSimd,
        OVERWORLD_GEOMETRY,
    );
    let reference = extract_centre_reference(&sky, &block);

    assert_eq!(scalar, reference);
    assert_eq!(portable, reference);
}

#[test]
fn open_air_chunk_is_sky_15_everywhere_and_block_0() {
    let table = tiny_table();
    let input = solo(air_chunk());
    let out = compute_chunk_light(borrow(&input), &table);
    for ly in 0..WORLD_HEIGHT {
        for lz in 0..SECTION_DIM {
            for lx in 0..SECTION_DIM {
                assert_eq!(out.sky.get(lx, ly, lz), 15, "sky at ({lx},{ly},{lz})");
                assert_eq!(out.block.get(lx, ly, lz), 0, "block at ({lx},{ly},{lz})");
            }
        }
    }
}

#[test]
fn custom_geometry_computes_open_sky_and_block_light() {
    let table = tiny_table();
    let mut chunk = custom_air_chunk();
    chunk.set_block(8, 64, 8, BlockStateId(3));
    let input = solo(chunk);

    let out = compute_chunk_light(borrow(&input), &table);

    assert_eq!(out.section_count(), 16);
    assert_eq!(out.sky_at(8, 0, 8), 15);
    assert_eq!(out.sky_at(8, 255, 8), 15);
    assert_eq!(out.block_at(8, 64, 8), 15);
    assert_eq!(out.block_at(9, 64, 8), 14);
}

#[test]
fn chunk_light_can_be_rebuilt_from_baked_section_layers() {
    let mut chunk = air_chunk();
    let mut sky = vec![0; LIGHT_LAYER_BYTES];
    let mut block = vec![0; LIGHT_LAYER_BYTES];
    sky[0] = 0x21;
    block[7] = 0xF0;
    chunk.section_lights[0].sky = Some(LightSection::from_bytes(sky.clone().try_into().unwrap()));
    chunk.section_lights[2].block =
        Some(LightSection::from_bytes(block.clone().try_into().unwrap()));

    let light = ChunkLight::from_section_lights(&chunk.section_lights)
        .expect("present baked layers should rebuild chunk light");

    assert_eq!(light.sky.section(0).unwrap().byte(0), 0x21);
    assert_eq!(light.block.section(2).unwrap().byte(7), 0xF0);
    assert_eq!(light.sky.get(0, 0, 0), 1);
    assert_eq!(light.sky.get(1, 0, 0), 2);
    assert!(light.block.section(0).unwrap().is_zero());
}

#[test]
fn custom_geometry_baked_light_uses_chunk_world_y() {
    let mut chunk = custom_air_chunk();
    let mut block = vec![0; LIGHT_LAYER_BYTES];
    block[0] = 0x0F;
    chunk.section_lights[0].block = Some(LightSection::from_bytes(block.try_into().unwrap()));

    let light =
        ChunkLight::from_chunk(&chunk).expect("present baked layer should rebuild chunk light");

    assert_eq!(light.block_at(0, 0, 0), 15);
}

#[test]
fn glowstone_emits_15_radius() {
    let table = tiny_table();
    let mut chunk = air_chunk();
    // Place glowstone (state 3) at (8, 64, 8).
    chunk.set_block(8, 64, 8, BlockStateId(3));
    let input = solo(chunk);
    let out = compute_chunk_light(borrow(&input), &table);
    // Glowstone cell is at emission 15.
    assert_eq!(out.block_at(8, 64, 8), 15);
    // 1 cell away: 14.
    assert_eq!(out.block_at(9, 64, 8), 14);
    assert_eq!(out.block_at(8, 65, 8), 14);
    // 15 cells away laterally: 0 (15 - 15 steps).
    assert_eq!(out.block_at(15, 64, 0), 0);
    // 14 cells away (Chebyshev sum is 14 — 7+7): 1 via shortest
    // Manhattan path. Pin via Manhattan distance.
    // (8,64,8) → (8,64,1): 7 z-steps, value 8.
    assert_eq!(out.block_at(8, 64, 1), 8);
}

#[test]
fn solid_column_blocks_sky_underneath() {
    let table = tiny_table();
    let mut chunk = air_chunk();
    // Stone column at x=8, z=8 from world Y=MIN_Y+1 up to Y=64.
    // The cell at world Y=MIN_Y is left as air, forming a 1×1
    // "well" at the very bottom of the world that lateral
    // propagation can reach.
    for y in (MIN_Y + 1)..=64 {
        chunk.set_block(8, y, 8, BlockStateId(1));
    }
    let input = solo(chunk);
    let out = compute_chunk_light(borrow(&input), &table);

    // Sky above the column: 15 (open air well above the stone top).
    assert_eq!(out.sky_at(8, 100, 8), 15);
    // The cell *inside* the column is 0 (opaque).
    assert_eq!(out.sky_at(8, 0, 8), 0);
    // Bottom-of-the-world air cell directly below the column:
    // lateral BFS brings sky-15 from (7, MIN_Y, 8) and (9, MIN_Y, 8)
    // (both fully open columns) with one step of cost-1 attenuation.
    assert_eq!(
        out.sky_at(8, MIN_Y, 8),
        14,
        "sky leaks into the well floor from adjacent open columns",
    );
}

#[test]
fn columnar_fast_path_matches_slow_bfs() {
    let table = tiny_table();
    let mut chunk = air_chunk();
    for lz in 0..SECTION_DIM {
        for lx in 0..SECTION_DIM {
            let top = 48 + ((lx + lz) % 5) as i32;
            for y in MIN_Y..=top {
                chunk.set_block(lx as u8, y, lz as u8, BlockStateId(1));
            }
        }
    }
    let input = solo(chunk);
    let borrowed = borrow(&input);
    let fast = compute_columnar_no_emitter_light(&borrowed, &table, OVERWORLD_GEOMETRY)
        .expect("columnar terrain should use fast path");
    let mut ws = LightWorkspace::new();
    let slow = compute_chunk_light_slow_in(&mut ws, borrowed, &table, OVERWORLD_GEOMETRY);
    assert_eq!(fast, slow);
}

#[test]
fn columnar_fast_path_rejects_emitters_and_caves() {
    let table = tiny_table();

    let mut emitter = air_chunk();
    emitter.set_block(8, 64, 8, BlockStateId(3));
    let input = solo(emitter);
    assert!(
        compute_columnar_no_emitter_light(&borrow(&input), &table, OVERWORLD_GEOMETRY).is_none()
    );

    let mut cave = air_chunk();
    for y in MIN_Y..=64 {
        cave.set_block(8, y, 8, BlockStateId(1));
    }
    cave.set_block(8, 0, 8, BlockStateId(0));
    let input = solo(cave);
    assert!(
        compute_columnar_no_emitter_light(&borrow(&input), &table, OVERWORLD_GEOMETRY).is_none()
    );

    let mut soft_blocker = air_chunk();
    for y in MIN_Y..64 {
        soft_blocker.set_block(8, y, 8, BlockStateId(1));
    }
    soft_blocker.set_block(8, 64, 8, BlockStateId(4));
    let input = solo(soft_blocker);
    assert!(
        compute_columnar_no_emitter_light(&borrow(&input), &table, OVERWORLD_GEOMETRY).is_none()
    );
}

#[test]
fn three_by_three_seam_propagates_block_light() {
    // Glowstone in the centre chunk at (15, 64, 15) (NE corner).
    // The NE neighbour should receive light at (0, 64, 0) with a
    // 2-Manhattan-step penalty (one step to leave the centre
    // chunk, one to enter the NE neighbour — but we pass through
    // a single shared "diagonal" face, which is actually two
    // steps).
    let table = tiny_table();
    let mut centre = air_chunk();
    centre.set_block(15, 64, 15, BlockStateId(3));
    let ne = air_chunk();
    let input: [[Option<Chunk>; 3]; 3] = [
        [None, None, None],
        [None, Some(centre), None],
        [None, None, Some(ne)],
    ];
    let out = compute_chunk_light(borrow(&input), &table);
    // Centre chunk's emitter cell: 15.
    assert_eq!(out.block_at(15, 64, 15), 15);
    // We can only extract the centre chunk's light from the
    // engine, but the BFS *did* visit the NE neighbour during
    // compute. The verification is indirect: the centre's own
    // cells near the corner should be 15 (emitter) and 14
    // (one step away) — which is the same as a fully-isolated
    // chunk.
    assert_eq!(out.block_at(14, 64, 15), 14);
    assert_eq!(out.block_at(15, 64, 14), 14);
}

#[test]
fn missing_neighbours_light_like_air_neighbours() {
    let table = tiny_table();
    // Stone up to y=59 with tunnels open at the z=0 and x=15 edges and a
    // glowstone in a side pocket of the first one.
    let mut chunk = air_chunk();
    for x in 0..16 {
        for z in 0..16 {
            for y in MIN_Y..60 {
                chunk.set_block(x, y, z, BlockStateId(1));
            }
        }
    }
    for x in 0..16 {
        for y in 40..43 {
            chunk.set_block(x, y, 0, BlockStateId(0));
        }
    }
    for z in 0..6 {
        for y in 30..33 {
            chunk.set_block(15, y, z, BlockStateId(0));
        }
    }
    chunk.set_block(3, 41, 1, BlockStateId(0));
    chunk.set_block(3, 41, 2, BlockStateId(3));
    let missing = solo(chunk.clone());
    let air = std::array::from_fn(|z| {
        std::array::from_fn(|x| {
            Some(if (z, x) == (1, 1) {
                chunk.clone()
            } else {
                air_chunk()
            })
        })
    });
    let missing = compute_chunk_light(borrow(&missing), &table);
    let air = compute_chunk_light(borrow(&air), &table);
    assert_light_eq(&missing, &air, "missing vs air neighbours");
    assert_eq!(missing.sky_at(5, 41, 0), 14, "side sky enters the tunnel");
    assert_eq!(missing.block_at(3, 41, 2), 15);
}

/// When the real .analysis/test-world block_light.json + blocks
/// report are present, run the engine on the spawn chunk and
/// sanity-check the output. The exact byte-for-byte values
/// aren't pinned (vanilla's algorithm differs in iteration
/// quirks, and the M3 status appendix already documented that
/// our world has almost no baked light to compare against), but
/// invariants are checked: sky=15 on the very top, and block-light
/// stays dark only when the sampled chunk has no emitters.
#[test]
#[ignore = "requires local .analysis/test-world and 26.1.2 light sidecars"]
fn engine_runs_on_real_spawn_chunk_when_data_present() {
    let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .to_path_buf();
    let blocks_path = workspace.join("data/vanilla/reports/blocks.json");
    let light_path = workspace.join("data/vanilla/reports/block_light.json");
    let region_path = workspace.join(".analysis/test-world/region/r.0.0.mca");
    assert!(
        blocks_path.is_file() && light_path.is_file() && region_path.is_file(),
        "need {}, {}, and {}",
        blocks_path.display(),
        light_path.display(),
        region_path.display()
    );

    let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
    let registry = BlockRegistry::from_report(&report).unwrap();
    let table = mc_data::block_light::load(&light_path).unwrap();

    let payloads = crate::anvil::region::read_region(&region_path).unwrap();
    // Find the first chunk that's Status:full.
    let mut full_chunk: Option<Chunk> = None;
    for payload in &payloads {
        let mut cur = std::io::Cursor::new(&payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cur).unwrap();
        let chunk =
            crate::anvil::chunk_nbt::chunk_from_nbt(&root, &registry, crate::OVERWORLD_GEOMETRY)
                .unwrap();
        if chunk.status.contains("full") {
            full_chunk = Some(chunk);
            break;
        }
    }
    let chunk = full_chunk.expect("test world must contain a Status:full chunk");

    let has_emitter = (0..WORLD_HEIGHT).any(|ly| {
        let world_y = MIN_Y + ly as i32;
        (0..16u8).any(|lz| {
            (0..16u8).any(|lx| {
                chunk
                    .get_block(lx, world_y, lz)
                    .is_some_and(|state| table.emission(state.0).unwrap_or(0) > 0)
            })
        })
    });

    let input = [
        [None, None, None],
        [None, Some(chunk), None],
        [None, None, None],
    ];
    let out = compute_chunk_light(borrow(&input), &table);

    // The top of the world should always be open sky.
    assert_eq!(out.sky_at(0, MAX_Y - 1, 0), 15);
    assert_eq!(out.sky_at(15, MAX_Y - 1, 15), 15);
    if has_emitter {
        let has_block_light = (0..WORLD_HEIGHT).any(|ly| {
            let world_y = MIN_Y + ly as i32;
            (0..16u8).any(|lz| (0..16u8).any(|lx| out.block_at(lx, world_y, lz) > 0))
        });
        assert!(has_block_light, "emitting chunk produced no block-light");
    } else {
        for ly in 0..WORLD_HEIGHT {
            for lz in 0..16u8 {
                for lx in 0..16u8 {
                    let world_y = MIN_Y + ly as i32;
                    assert_eq!(
                        out.block_at(lx, world_y, lz),
                        0,
                        "expected no block-light in the spawn chunk; got non-zero at \
                         ({lx}, {world_y}, {lz})",
                    );
                }
            }
        }
    }
}

// ===== M9.a-c: incremental relight tests =====

fn full_recompute(neighbourhood: [[Option<&Chunk>; 3]; 3], table: &BlockLightTable) -> ChunkLight {
    compute_chunk_light(neighbourhood, table)
}

fn seed_cache_from_full(
    cache: &mut LightCache,
    chunks: &[(ChunkPos, &Chunk)],
    table: &BlockLightTable,
) {
    // Seed each chunk's light by running a full compute with its
    // 3×3 neighbourhood drawn from the provided chunk list.
    let by_pos: FxHashMap<ChunkPos, &Chunk> = chunks.iter().map(|&(p, c)| (p, c)).collect();
    for &(pos, _) in chunks {
        let mut nbh: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let np = ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                };
                nbh[(dz + 1) as usize][(dx + 1) as usize] = by_pos.get(&np).copied();
            }
        }
        let light = full_recompute(nbh, table);
        cache.insert(pos, light);
    }
}

fn assert_light_eq(actual: &ChunkLight, expected: &ChunkLight, tag: &str) {
    for ly in 0..WORLD_HEIGHT {
        for lz in 0..SECTION_DIM {
            for lx in 0..SECTION_DIM {
                let actual_sky = actual.sky.get(lx, ly, lz);
                let expected_sky = expected.sky.get(lx, ly, lz);
                assert_eq!(
                    actual_sky, expected_sky,
                    "{tag}: sky mismatch at ({lx},{ly},{lz}): inc={} full={}",
                    actual_sky, expected_sky,
                );
                let actual_block = actual.block.get(lx, ly, lz);
                let expected_block = expected.block.get(lx, ly, lz);
                assert_eq!(
                    actual_block, expected_block,
                    "{tag}: block mismatch at ({lx},{ly},{lz}): inc={} full={}",
                    actual_block, expected_block,
                );
            }
        }
    }
}

#[test]
fn incremental_no_op_for_like_for_like_swap() {
    let table = tiny_table();
    let chunk = air_chunk();
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &chunk)], &table);

    // Place air on air — opacity, emission, propagates_sky all
    // unchanged. Should return empty touched list and not mutate
    // the cache.
    let touched = apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&chunk), None],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        64,
        8,
        BlockStateId(0),
        BlockStateId(0),
    );
    assert!(touched.is_empty());
}

#[test]
fn incremental_place_glowstone_in_air() {
    let table = tiny_table();
    let pre = air_chunk();
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

    // Apply: place glowstone (3) at (8, 64, 8). Build the
    // post-edit chunk and feed both into the incremental update.
    let mut post = pre.clone();
    post.set_block(8, 64, 8, BlockStateId(3));
    let touched = apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        64,
        8,
        BlockStateId(0),
        BlockStateId(3),
    );
    assert_eq!(touched, vec![pos]);

    let inc = cache.get(pos).unwrap();
    let full = full_recompute(
        [
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
    );
    assert_light_eq(inc, &full, "place glowstone");
}

#[test]
fn incremental_custom_geometry_updates_the_edited_world_y() {
    let table = tiny_table();
    let pre = custom_air_chunk();
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

    let mut post = pre.clone();
    post.set_block(8, 0, 8, BlockStateId(3));
    let touched = apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        0,
        8,
        BlockStateId(0),
        BlockStateId(3),
    );

    let incremental = cache.get(pos).unwrap();
    let full = full_recompute(
        [
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
    );
    assert_eq!(touched, vec![pos]);
    assert_eq!(incremental.block_at(8, 0, 8), full.block_at(8, 0, 8));
    assert_eq!(incremental.block_at(9, 0, 8), full.block_at(9, 0, 8));
}

#[test]
fn incremental_custom_geometry_updates_sky_at_the_edited_world_y() {
    let table = tiny_table();
    let pre = custom_air_chunk();
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

    let mut post = pre.clone();
    post.set_block(8, 0, 8, BlockStateId(1));
    apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        0,
        8,
        BlockStateId(0),
        BlockStateId(1),
    );

    let incremental = cache.get(pos).unwrap();
    let full = full_recompute(
        [
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
    );
    assert_eq!(incremental.sky_at(8, 0, 8), full.sky_at(8, 0, 8));
    assert_eq!(incremental.sky_at(8, 1, 8), full.sky_at(8, 1, 8));
}

#[test]
#[should_panic(expected = "lighting neighbourhood chunks must share geometry")]
fn incremental_mixed_geometry_fails_closed() {
    let table = tiny_table();
    let centre = custom_air_chunk();
    let east = air_chunk();
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    cache.insert(
        pos,
        full_recompute(
            [
                [None, None, None],
                [None, Some(&centre), None],
                [None, None, None],
            ],
            &table,
        ),
    );

    apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&centre), Some(&east)],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        0,
        8,
        BlockStateId(0),
        BlockStateId(3),
    );
}

#[test]
fn incremental_remove_glowstone() {
    let table = tiny_table();
    let mut pre = air_chunk();
    pre.set_block(8, 64, 8, BlockStateId(3));
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

    let mut post = pre.clone();
    post.set_block(8, 64, 8, BlockStateId(0));
    let _ = apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        64,
        8,
        BlockStateId(3),
        BlockStateId(0),
    );

    let inc = cache.get(pos).unwrap();
    let full = full_recompute(
        [
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
    );
    assert_light_eq(inc, &full, "remove glowstone");
}

#[test]
fn incremental_place_stone_blocks_sky() {
    let table = tiny_table();
    let pre = air_chunk();
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

    // Place stone (1) at (8, 100, 8) — opaque, blocks sky.
    let mut post = pre.clone();
    post.set_block(8, 100, 8, BlockStateId(1));
    let _ = apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        100,
        8,
        BlockStateId(0),
        BlockStateId(1),
    );

    let inc = cache.get(pos).unwrap();
    let full = full_recompute(
        [
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
    );
    assert_light_eq(inc, &full, "place stone in air");
}

#[test]
fn incremental_break_stone_in_solid_column() {
    let table = tiny_table();
    // Build a stone column at (8, *, 8) from MIN_Y..=64.
    let mut pre = air_chunk();
    for y in (MIN_Y + 1)..=64 {
        pre.set_block(8, y, 8, BlockStateId(1));
    }
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &pre)], &table);

    // Break the stone at (8, 64, 8) — column top, so sky now
    // pours into a 1-cell pocket but doesn't reach further down
    // (cells below are still stone).
    let mut post = pre.clone();
    post.set_block(8, 64, 8, BlockStateId(0));
    let _ = apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
        pos,
        8,
        64,
        8,
        BlockStateId(1),
        BlockStateId(0),
    );

    let inc = cache.get(pos).unwrap();
    let full = full_recompute(
        [
            [None, None, None],
            [None, Some(&post), None],
            [None, None, None],
        ],
        &table,
    );
    assert_light_eq(inc, &full, "break stone column top");
}

#[test]
fn incremental_edit_at_chunk_seam_propagates() {
    let table = tiny_table();
    let pre_centre = air_chunk();
    let plains = Identifier::parse("minecraft:plains").unwrap();
    let pre_east = Chunk::empty(ChunkPos { x: 1, z: 0 }, BlockStateId(0), plains);

    let cpos = ChunkPos { x: 0, z: 0 };
    let epos = ChunkPos { x: 1, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(
        &mut cache,
        &[(cpos, &pre_centre), (epos, &pre_east)],
        &table,
    );

    // Place glowstone at the east edge of the centre chunk:
    // (15, 64, 8). Light should ripple into the east neighbour.
    let mut post_centre = pre_centre.clone();
    post_centre.set_block(15, 64, 8, BlockStateId(3));
    let post_east = pre_east.clone();
    let touched = apply_block_change_to_light(
        &mut cache,
        &[
            [None, None, None],
            [None, Some(&post_centre), Some(&post_east)],
            [None, None, None],
        ],
        &table,
        cpos,
        15,
        64,
        8,
        BlockStateId(0),
        BlockStateId(3),
    );
    assert!(touched.contains(&cpos), "centre chunk should be touched");
    assert!(touched.contains(&epos), "east neighbour should be touched");

    let inc_centre = cache.get(cpos).unwrap();
    let inc_east = cache.get(epos).unwrap();
    let full_centre = full_recompute(
        [
            [None, None, None],
            [None, Some(&post_centre), Some(&post_east)],
            [None, None, None],
        ],
        &table,
    );
    let full_east = full_recompute(
        [
            [None, None, None],
            [Some(&post_centre), Some(&post_east), None],
            [None, None, None],
        ],
        &table,
    );
    assert_light_eq(inc_centre, &full_centre, "centre after seam edit");
    assert_light_eq(inc_east, &full_east, "east after seam edit");
}

#[test]
fn incremental_random_edit_sequence_matches_full_recompute() {
    // Deterministic pseudo-random edit sequence inside one chunk;
    // after each edit, assert the cached light equals a full
    // recompute on the post-edit chunk. Block states limited to
    // {air=0, stone=1, glowstone=3}.
    let table = tiny_table();
    let mut chunk = air_chunk();
    let pos = ChunkPos { x: 0, z: 0 };
    let mut cache = LightCache::new();
    seed_cache_from_full(&mut cache, &[(pos, &chunk)], &table);

    // Simple LCG.
    let mut s: u32 = 0xdeadbeef;
    let mut next = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        s
    };

    for _ in 0..40 {
        let lx = (next() % 16) as u8;
        let lz = (next() % 16) as u8;
        // Restrict Y to a thin band so sky-light interactions stay
        // testable but the sequence converges.
        let world_y = 60 + (next() % 8) as i32;
        let new_state_choices = [BlockStateId(0), BlockStateId(1), BlockStateId(3)];
        let new_state = new_state_choices[(next() as usize) % 3];

        let prev = chunk.get_block(lx, world_y, lz).unwrap_or(BlockStateId(0));
        chunk.set_block(lx, world_y, lz, new_state);

        apply_block_change_to_light(
            &mut cache,
            &[
                [None, None, None],
                [None, Some(&chunk), None],
                [None, None, None],
            ],
            &table,
            pos,
            lx,
            world_y,
            lz,
            prev,
            new_state,
        );

        let inc = cache.get(pos).unwrap().clone();
        let full = full_recompute(
            [
                [None, None, None],
                [None, Some(&chunk), None],
                [None, None, None],
            ],
            &table,
        );
        assert_light_eq(
            &inc,
            &full,
            &format!("random edit ({lx},{world_y},{lz}) -> {}", new_state.0),
        );
    }
}
