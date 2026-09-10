use super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;

#[test]
fn compact_block_palettes_round_trip_through_vanilla_anvil_words() {
    let report = (0..257)
        .map(|state| BlockReport {
            id: Identifier::parse(format!("minecraft:state_{state}")).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state,
                default: true,
                properties: BTreeMap::new(),
            }],
        })
        .collect::<Vec<_>>();
    let registry = BlockRegistry::from_report(&report).unwrap();
    let air = BlockStateId(0);
    for palette_len in [1usize, 2, 3, 5, 9, 17, 257] {
        let mut section = ChunkSection::filled(air, air);
        for cell in 0..SECTION_VOLUME {
            section.set(
                (cell % 16) as u8,
                (cell / 256) as u8,
                ((cell / 16) % 16) as u8,
                BlockStateId((cell % palette_len) as u32),
            );
        }
        let encoded = encode_block_section(&section, &registry).unwrap();
        let fields = expect_compound(&encoded, "block_states").unwrap();
        let palette = get_list(fields, "palette").unwrap();
        assert_eq!(palette.elements.len(), palette_len);
        if palette_len == 1 {
            assert!(get_optional_long_array(fields, "data").unwrap().is_none());
        } else {
            let bits = ((palette_len - 1).ilog2() as usize + 1).max(4);
            let entries_per_word = 64 / bits;
            // Independent vanilla packing oracle: no entry crosses a long.
            let mut expected = vec![0i64; SECTION_VOLUME.div_ceil(entries_per_word)];
            for cell in 0..SECTION_VOLUME {
                expected[cell / entries_per_word] |=
                    ((cell % palette_len) as i64) << ((cell % entries_per_word) * bits);
            }
            assert_eq!(
                get_optional_long_array(fields, "data").unwrap().unwrap(),
                expected
            );
        }
        let decoded = decode_block_section(fields, &registry, air).unwrap();
        assert_eq!(decoded.non_air_count(), section.non_air_count());
        let expected_ram_bits = if palette_len == 1 {
            0
        } else {
            (palette_len - 1).ilog2() as u8 + 1
        };
        assert_eq!(
            decoded.indices().map_or(0, PackedBitArray::bits_per_entry),
            expected_ram_bits
        );
        for cell in 0..SECTION_VOLUME {
            assert_eq!(
                decoded.get(
                    (cell % 16) as u8,
                    (cell / 256) as u8,
                    ((cell / 16) % 16) as u8
                ),
                BlockStateId((cell % palette_len) as u32)
            );
        }
        assert_eq!(encode_block_section(&decoded, &registry).unwrap(), encoded);
    }
}
