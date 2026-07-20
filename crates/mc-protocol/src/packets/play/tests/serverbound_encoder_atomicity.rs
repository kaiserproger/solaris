#[test]
fn command_suggestion_accepts_empty_and_vanilla_maximum() {
    for command in [String::new(), "x".repeat(MAX_COMMAND_SUGGESTION_LEN)] {
        let packet = ServerboundCommandSuggestion { id: 42, command };
        let mut encoded = Vec::new();
        packet.encode(&mut encoded).unwrap();

        let mut cursor: &[u8] = &encoded;
        assert_eq!(
            ServerboundCommandSuggestion::decode(&mut cursor).unwrap(),
            packet
        );
        assert!(cursor.is_empty());
    }
}

#[test]
fn overlong_command_suggestion_does_not_modify_output() {
    let packet = ServerboundCommandSuggestion {
        id: 42,
        command: "x".repeat(MAX_COMMAND_SUGGESTION_LEN + 1),
    };
    let mut encoded = vec![0xA5, 0x5A];

    assert!(matches!(
        packet.encode(&mut encoded),
        Err(CodecError::StringTooLong {
            len,
            max: MAX_COMMAND_SUGGESTION_LEN
        }) if len == MAX_COMMAND_SUGGESTION_LEN + 1
    ));
    assert_eq!(encoded, [0xA5, 0x5A]);
}

#[test]
fn malformed_command_suggestion_length_is_rejected_on_decode() {
    let mut encoded = Vec::new();
    encoded.write_varint(42);
    encoded.write_varint((MAX_COMMAND_SUGGESTION_LEN * 3 + 1) as i32);

    let mut cursor: &[u8] = &encoded;
    assert!(matches!(
        ServerboundCommandSuggestion::decode(&mut cursor),
        Err(CodecError::StringTooLong { .. })
    ));
}

#[test]
fn sign_lines_accept_empty_and_vanilla_maximum_in_every_field() {
    let empty = ServerboundSignUpdate {
        position: 17,
        lines: vec![String::new(); SIGN_LINE_COUNT],
        is_front_text: true,
    };
    let mut encoded = Vec::new();
    empty.encode(&mut encoded).unwrap();
    let mut cursor: &[u8] = &encoded;
    assert_eq!(ServerboundSignUpdate::decode(&mut cursor).unwrap(), empty);
    assert!(cursor.is_empty());

    for line_index in 0..SIGN_LINE_COUNT {
        let mut lines = vec![String::new(); SIGN_LINE_COUNT];
        lines[line_index] = "x".repeat(MAX_SIGN_LINE_LEN);
        let packet = ServerboundSignUpdate {
            position: 17,
            lines,
            is_front_text: false,
        };
        let mut encoded = Vec::new();
        packet.encode(&mut encoded).unwrap();

        let mut cursor: &[u8] = &encoded;
        assert_eq!(ServerboundSignUpdate::decode(&mut cursor).unwrap(), packet);
        assert!(cursor.is_empty());
    }
}

#[test]
fn overlong_sign_line_does_not_modify_output_in_any_field() {
    for line_index in 0..SIGN_LINE_COUNT {
        let mut lines = vec![String::new(); SIGN_LINE_COUNT];
        lines[line_index] = "x".repeat(MAX_SIGN_LINE_LEN + 1);
        let packet = ServerboundSignUpdate {
            position: 17,
            lines,
            is_front_text: true,
        };
        let mut encoded = vec![0xA5, 0x5A];

        assert!(matches!(
            packet.encode(&mut encoded),
            Err(CodecError::StringTooLong {
                len,
                max: MAX_SIGN_LINE_LEN
            }) if len == MAX_SIGN_LINE_LEN + 1
        ));
        assert_eq!(encoded, [0xA5, 0x5A], "line index {line_index}");
    }
}

#[test]
fn invalid_sign_line_count_does_not_modify_output() {
    for lines in [
        vec![String::new(); SIGN_LINE_COUNT - 1],
        vec![String::new(); SIGN_LINE_COUNT + 1],
    ] {
        let packet = ServerboundSignUpdate {
            position: 17,
            lines,
            is_front_text: true,
        };
        let mut encoded = vec![0xA5, 0x5A];

        assert!(matches!(
            packet.encode(&mut encoded),
            Err(CodecError::NotSupported(
                "sign update must contain four lines"
            ))
        ));
        assert_eq!(encoded, [0xA5, 0x5A]);
    }
}

#[test]
fn malformed_sign_line_length_is_rejected_in_every_field() {
    for line_index in 0..SIGN_LINE_COUNT {
        let mut encoded = Vec::new();
        encoded.write_i64(17);
        encoded.write_bool(true);
        for _ in 0..line_index {
            encoded.write_string("", MAX_SIGN_LINE_LEN).unwrap();
        }
        encoded.write_varint((MAX_SIGN_LINE_LEN * 3 + 1) as i32);

        let mut cursor: &[u8] = &encoded;
        assert!(matches!(
            ServerboundSignUpdate::decode(&mut cursor),
            Err(CodecError::StringTooLong { .. })
        ));
    }
}

#[test]
fn serverbound_brand_payload_preflights_string_before_play_output() {
    let maximum = ServerboundCustomPayload {
        payload: CustomPayload::Brand("x".repeat(DEFAULT_MAX_STRING_LEN)),
    };
    let mut encoded = Vec::new();
    maximum.encode(&mut encoded).unwrap();
    let mut cursor: &[u8] = &encoded;
    assert_eq!(
        ServerboundCustomPayload::decode(&mut cursor).unwrap(),
        maximum
    );
    assert!(cursor.is_empty());

    let oversized = ServerboundCustomPayload {
        payload: CustomPayload::Brand("x".repeat(DEFAULT_MAX_STRING_LEN + 1)),
    };
    let mut encoded = vec![0xA5, 0x5A];
    assert_eq!(
        oversized.encode(&mut encoded).unwrap_err(),
        CodecError::StringTooLong {
            len: DEFAULT_MAX_STRING_LEN + 1,
            max: DEFAULT_MAX_STRING_LEN,
        }
    );
    assert_eq!(encoded, [0xA5, 0x5A]);
}
