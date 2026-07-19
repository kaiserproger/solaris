#[test]
fn clientbound_set_passengers_id_matches_protocol_dump() {
    assert_eq!(ClientboundSetPassengers::ID, 0x6B);
}

#[test]
fn clientbound_set_passengers_empty_list_round_trips() {
    round_trip(ClientboundSetPassengers {
        vehicle_id: 17,
        passenger_ids: Vec::new(),
    });
}

#[test]
fn clientbound_set_passengers_multi_passenger_layout_round_trips() {
    let packet = ClientboundSetPassengers {
        vehicle_id: 300,
        passenger_ids: vec![1, 127, 128, 16_384],
    };
    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();

    let mut expected = Vec::new();
    expected.write_varint(300);
    expected.write_varint(4);
    expected.write_varint(1);
    expected.write_varint(127);
    expected.write_varint(128);
    expected.write_varint(16_384);
    assert_eq!(encoded, expected);

    let mut cursor: &[u8] = &encoded;
    assert_eq!(
        ClientboundSetPassengers::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_set_passengers_rejects_negative_count() {
    let mut encoded = Vec::new();
    encoded.write_varint(1);
    encoded.write_varint(-1);

    let mut cursor: &[u8] = &encoded;
    assert_eq!(
        ClientboundSetPassengers::decode(&mut cursor).unwrap_err(),
        CodecError::NegativeLength(-1)
    );
}

#[test]
fn clientbound_set_passengers_rejects_oversized_count() {
    let packet = ClientboundSetPassengers {
        vehicle_id: 1,
        passenger_ids: vec![0; MAX_ENTITY_ID_LIST_LEN + 1],
    };
    let mut encoded = Vec::new();

    assert_eq!(
        packet.encode(&mut encoded).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ENTITY_ID_LIST_LEN + 1,
            max: MAX_ENTITY_ID_LIST_LEN,
        }
    );
}

#[test]
fn clientbound_set_passengers_decode_rejects_oversized_count_before_payload() {
    let mut encoded = Vec::new();
    encoded.write_varint(1);
    encoded.write_varint((MAX_ENTITY_ID_LIST_LEN + 1) as i32);

    let mut cursor: &[u8] = &encoded;
    assert_eq!(
        ClientboundSetPassengers::decode(&mut cursor).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_ENTITY_ID_LIST_LEN + 1,
            max: MAX_ENTITY_ID_LIST_LEN,
        }
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_set_passengers_exact_max_count_decodes_and_round_trips() {
    let packet = ClientboundSetPassengers {
        vehicle_id: 1,
        passenger_ids: (0..MAX_ENTITY_ID_LIST_LEN as i32).collect(),
    };
    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();

    let mut cursor: &[u8] = &encoded;
    assert_eq!(
        ClientboundSetPassengers::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}
