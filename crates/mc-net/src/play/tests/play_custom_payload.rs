use bytes::Bytes;
use mc_extension::DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES;

use super::{PlayCustomPayloadAction, classify_play_custom_payload};

#[test]
fn oversized_play_custom_payload_is_rejected_before_decode() {
    let body = Bytes::from(vec![0x80; DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES + 1]);

    let action = classify_play_custom_payload(body).unwrap();

    assert_eq!(
        action,
        PlayCustomPayloadAction::Oversized {
            len: DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES + 1
        }
    );
}

#[test]
fn loader_interaction_channel_is_claimed_before_extension_forwarding() {
    let channel = b"solaris:loader/interaction";
    let payload = b"action";
    let mut body = Vec::with_capacity(1 + channel.len() + payload.len());
    body.push(channel.len() as u8);
    body.extend_from_slice(channel);
    body.extend_from_slice(payload);

    assert_eq!(
        classify_play_custom_payload(Bytes::from(body)).unwrap(),
        PlayCustomPayloadAction::LoaderInteraction(Bytes::from_static(payload))
    );
}
