fn merchant_offer_fixture() -> MerchantOffer {
    MerchantOffer {
        cost_a: MerchantItemCost {
            item_id: 17,
            count: 4,
        },
        result: ItemStack::new(23, 1),
        cost_b: Some(MerchantItemCost {
            item_id: 41,
            count: 2,
        }),
        out_of_stock: false,
        uses: 1,
        max_uses: 12,
        xp: 2,
        special_price: -1,
        price_multiplier: 0.05,
        demand: 3,
    }
}

#[test]
fn merchant_packet_ids_match_the_local_26_1_2_registration_order() {
    assert_eq!(ClientboundMerchantOffers::ID, 0x34);
    assert_eq!(ServerboundSelectTrade::ID, 0x33);
}

#[test]
fn merchant_offers_round_trip_exact_vanilla_field_order() {
    let packet = ClientboundMerchantOffers {
        container_id: 5,
        offers: vec![merchant_offer_fixture()],
        villager_level: 2,
        villager_xp: 10,
        show_progress: true,
        can_restock: true,
    };
    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();

    let mut cursor: &[u8] = &encoded;
    assert_eq!(cursor.read_varint().unwrap(), 5);
    assert_eq!(cursor.read_varint().unwrap(), 1);
    assert_eq!(cursor.read_varint().unwrap(), 17);
    assert_eq!(cursor.read_varint().unwrap(), 4);
    assert_eq!(cursor.read_varint().unwrap(), 0);
    assert_eq!(ItemStack::decode(&mut cursor).unwrap(), ItemStack::new(23, 1));
    assert!(cursor.read_bool().unwrap());
    assert_eq!(cursor.read_varint().unwrap(), 41);
    assert_eq!(cursor.read_varint().unwrap(), 2);
    assert_eq!(cursor.read_varint().unwrap(), 0);
    assert!(!cursor.read_bool().unwrap());
    assert_eq!(cursor.read_i32().unwrap(), 1);
    assert_eq!(cursor.read_i32().unwrap(), 12);
    assert_eq!(cursor.read_i32().unwrap(), 2);
    assert_eq!(cursor.read_i32().unwrap(), -1);
    assert_eq!(cursor.read_f32().unwrap(), 0.05);
    assert_eq!(cursor.read_i32().unwrap(), 3);
    assert_eq!(cursor.read_varint().unwrap(), 2);
    assert_eq!(cursor.read_varint().unwrap(), 10);
    assert!(cursor.read_bool().unwrap());
    assert!(cursor.read_bool().unwrap());
    assert!(cursor.is_empty());

    round_trip(packet);
    round_trip(ServerboundSelectTrade { offer_index: 300 });
}

#[test]
fn merchant_decode_rejects_unimplemented_component_predicates_before_allocation() {
    let mut encoded = Vec::new();
    encoded.write_varint(1);
    encoded.write_varint(1);
    encoded.write_varint(2);
    encoded.write_varint(1);
    encoded.write_varint(1);
    let mut cursor: &[u8] = &encoded;
    assert!(matches!(
        ClientboundMerchantOffers::decode(&mut cursor),
        Err(CodecError::NotSupported(
            "merchant exact component predicates are unsupported"
        ))
    ));
}

#[test]
fn merchant_offer_count_and_select_trade_are_bounded() {
    let mut oversized = Vec::new();
    oversized.write_varint(1);
    oversized.write_varint(257);
    let mut cursor: &[u8] = &oversized;
    assert!(matches!(
        ClientboundMerchantOffers::decode(&mut cursor),
        Err(CodecError::StringTooLong { max: 256, .. })
    ));

    assert!(ServerboundSelectTrade { offer_index: -1 }
        .encode(&mut Vec::new())
        .is_err());
}
