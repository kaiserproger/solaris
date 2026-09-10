use super::*;

const AIR: BlockStateId = BlockStateId(0);
const STONE: BlockStateId = BlockStateId(1);

// ----- PackedBitArray -----

#[test]
fn packed_round_trip_4bit() {
    let mut a = PackedBitArray::zeroed(4, 100);
    for i in 0..100 {
        a.set(i, (i % 16) as u32);
    }
    for i in 0..100 {
        assert_eq!(a.get(i), (i % 16) as u32);
    }
}

#[test]
fn packed_word_boundary() {
    // 5 bits per entry, 12 entries per word — entry 12 starts a
    // fresh word. Confirm the boundary is respected.
    let mut a = PackedBitArray::zeroed(5, 30);
    for i in 0..30 {
        a.set(i, (i as u32) & 0x1F);
    }
    for i in 0..30 {
        assert_eq!(a.get(i), (i as u32) & 0x1F);
    }
    // Top 4 bits of each word are padding for 5-bit packing.
    for w in a.words() {
        assert_eq!(w >> 60, 0);
    }
}

#[test]
fn packed_rebit_preserves_values() {
    let mut a = PackedBitArray::zeroed(4, 50);
    for i in 0..50 {
        a.set(i, (i % 16) as u32);
    }
    a.rebit(8);
    assert_eq!(a.bits_per_entry(), 8);
    for i in 0..50 {
        assert_eq!(a.get(i), (i % 16) as u32);
    }
}

// ----- ChunkSection -----

#[test]
fn filled_air_is_empty() {
    let s = ChunkSection::filled(AIR, AIR);
    assert_eq!(s.non_air_count(), 0);
    assert!(s.is_empty());
    assert_eq!(s.get(0, 0, 0), AIR);
    assert_eq!(s.get(15, 15, 15), AIR);
    assert!(s.palette().is_none());
}

#[test]
fn filled_stone_counts_all_cells() {
    let s = ChunkSection::filled(STONE, AIR);
    assert_eq!(s.non_air_count(), SECTION_VOLUME as u16);
    assert_eq!(s.get(7, 3, 12), STONE);
}

#[test]
fn first_set_promotes_to_indirect_and_tracks_count() {
    let mut s = ChunkSection::filled(AIR, AIR);
    assert_eq!(s.set(1, 2, 3, STONE), AIR);
    assert_eq!(s.get(1, 2, 3), STONE);
    assert_eq!(s.get(0, 0, 0), AIR);
    assert_eq!(s.non_air_count(), 1);
    let palette = s.palette().unwrap();
    assert_eq!(palette, &[AIR, STONE]);
}

#[test]
fn set_back_to_air_decrements_count() {
    let mut s = ChunkSection::filled(AIR, AIR);
    s.set(1, 2, 3, STONE);
    s.set(1, 2, 3, AIR);
    assert_eq!(s.non_air_count(), 0);
    assert_eq!(s.get(1, 2, 3), AIR);
}

#[test]
fn growing_palette_preserves_cells_and_snapshots_across_compact_widths() {
    let mut section = ChunkSection::filled(AIR, AIR);
    let mut expected = vec![AIR; SECTION_VOLUME];
    for state in 1..=17u32 {
        let snapshot = section.clone();
        let previous = expected.clone();
        // Exercise both packed-word and coordinate boundaries, including
        // the final, partially occupied word of the three-bit array.
        let cell = if state == 1 {
            SECTION_VOLUME - 1
        } else {
            (state as usize * 251) % SECTION_VOLUME
        };
        let x = (cell % 16) as u8;
        let y = (cell / 256) as u8;
        let z = ((cell / 16) % 16) as u8;
        assert_eq!(section.set(x, y, z, BlockStateId(state)), expected[cell]);
        expected[cell] = BlockStateId(state);
        let bits = match state {
            1 => 1,
            2..=3 => 2,
            4..=7 => 3,
            8..=15 => 4,
            _ => 5,
        };
        assert_eq!(section.indices().unwrap().bits_per_entry(), bits);
        assert_eq!(
            section.indices().unwrap().words().len(),
            SECTION_VOLUME.div_ceil(64 / bits as usize)
        );
        assert_eq!(section.non_air_count(), state as u16);
        assert_eq!(snapshot.non_air_count(), state as u16 - 1);
        for cell in 0..SECTION_VOLUME {
            let x = (cell % 16) as u8;
            let y = (cell / 256) as u8;
            let z = ((cell / 16) % 16) as u8;
            assert_eq!(section.get(x, y, z), expected[cell]);
            assert_eq!(snapshot.get(x, y, z), previous[cell]);
        }
    }
}

#[test]
fn admitted_disk_indices_compact_without_changing_cells() {
    for (palette_len, expected_bits) in [(1, 0), (2, 1), (3, 2), (5, 3), (9, 4)] {
        let palette = (0..palette_len).map(BlockStateId).collect();
        let mut disk = PackedBitArray::zeroed(4, SECTION_VOLUME);
        for cell in 0..SECTION_VOLUME {
            disk.set(cell, cell as u32 % palette_len);
        }
        let section = ChunkSection::from_indirect(palette, disk, AIR);
        assert_eq!(
            section.indices().map_or(0, PackedBitArray::bits_per_entry),
            expected_bits
        );
        if palette_len == 1 {
            assert!(section.palette().is_none());
            assert_eq!(section.estimated_heap_bytes(), 0);
        }
        let expected_non_air = (0..SECTION_VOLUME)
            .filter(|&cell| !(cell as u32).is_multiple_of(palette_len))
            .count();
        assert_eq!(section.non_air_count() as usize, expected_non_air);
        for cell in 0..SECTION_VOLUME {
            assert_eq!(
                section.get(
                    (cell % 16) as u8,
                    (cell / 256) as u8,
                    ((cell / 16) % 16) as u8
                ),
                BlockStateId(cell as u32 % palette_len)
            );
        }
    }
}

#[test]
fn coordinate_corners_are_addressable() {
    let mut s = ChunkSection::filled(AIR, AIR);
    s.set(0, 0, 0, BlockStateId(10));
    s.set(15, 15, 15, BlockStateId(20));
    s.set(15, 0, 0, BlockStateId(30));
    s.set(0, 15, 0, BlockStateId(40));
    s.set(0, 0, 15, BlockStateId(50));
    assert_eq!(s.get(0, 0, 0), BlockStateId(10));
    assert_eq!(s.get(15, 15, 15), BlockStateId(20));
    assert_eq!(s.get(15, 0, 0), BlockStateId(30));
    assert_eq!(s.get(0, 15, 0), BlockStateId(40));
    assert_eq!(s.get(0, 0, 15), BlockStateId(50));
    assert_eq!(s.non_air_count(), 5);
}

#[test]
fn palette_grows_past_256_entries_without_panicking() {
    // M5.c.3: removed the MAX_INDIRECT_BITS = 8 cap. Inserting
    // 260 distinct synthetic state ids into one section must
    // produce a palette of length ≥ 260 and a packed bit-width
    // wide enough to address it (≥ 9 bits).
    let mut s = ChunkSection::filled(AIR, AIR);
    for i in 1..=260u32 {
        // Walk (x, y) so every set hits a distinct cell.
        let cell = i - 1;
        let x = (cell & 0x0F) as u8;
        let y = ((cell >> 4) & 0x0F) as u8;
        let z = ((cell >> 8) & 0x0F) as u8;
        s.set(x, y, z, BlockStateId(i));
    }
    let palette = s.palette().unwrap();
    assert!(
        palette.len() >= 260,
        "expected ≥ 260 palette entries, got {}",
        palette.len(),
    );
    let bits = s.indices().unwrap().bits_per_entry();
    assert!(
        bits >= 9,
        "expected ≥ 9 bits per entry past the 256-state threshold, got {bits}",
    );
    // Spot-check that all 260 distinct cells round-trip.
    for i in 1..=260u32 {
        let cell = i - 1;
        let x = (cell & 0x0F) as u8;
        let y = ((cell >> 4) & 0x0F) as u8;
        let z = ((cell >> 8) & 0x0F) as u8;
        assert_eq!(s.get(x, y, z), BlockStateId(i));
    }
}

/// Random sequence of sets — `non_air_count` must match a linear
/// scan. Catches off-by-one in the +/- bookkeeping in `set`.
#[test]
fn non_air_count_matches_linear_scan() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut s = ChunkSection::filled(AIR, AIR);
    // Deterministic pseudo-random walk: 200 ops.
    let mut h = DefaultHasher::new();
    for op in 0..200u64 {
        op.hash(&mut h);
        let r = h.finish();
        let x = (r & 0x0F) as u8;
        let y = ((r >> 4) & 0x0F) as u8;
        let z = ((r >> 8) & 0x0F) as u8;
        let state = if (r >> 12) & 0x07 == 0 {
            AIR
        } else {
            BlockStateId((r >> 16) as u32 & 0xFF)
        };
        s.set(x, y, z, state);
    }
    let mut linear = 0u16;
    for y in 0..16u8 {
        for z in 0..16u8 {
            for x in 0..16u8 {
                if s.get(x, y, z) != AIR {
                    linear += 1;
                }
            }
        }
    }
    assert_eq!(s.non_air_count(), linear);
}
