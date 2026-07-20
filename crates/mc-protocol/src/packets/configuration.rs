//! Configuration state — between Login and Play.
//!
//! At M3.i the choreography is:
//!
//! ```text
//! S → C  Known Packs               (0x0E, our list of known data packs)
//! C → S  Known Packs               (0x07, the subset the client also has)
//! S → C  Registry Data × N         (0x07, one per built-in registry)
//! S → C  Update Tags               (0x0D, the full tag set in one packet)
//! S → C  Finish Configuration      (0x03, empty)
//! C → S  Acknowledge Finish Conf.  (0x03, empty)
//!        → state transitions to Play
//! ```
//!
//! Resource Pack push/pop, Client Information, Plugin Messages, and
//! Cookie storage are still deferred — see `docs/milestones/M1.md` for
//! the full deferral list. The handler in `mc-net` reads but does not
//! act on any other clientbound-in-Conf packets the client may send.

use std::sync::Arc;

use bytes::{Buf, BufMut};

use super::{ClientInformation, CustomPayload, Packet, ResourcePackStatus};
use crate::codec::{Identifier, ReadMc, WriteMc};
use crate::error::CodecError;

const MAX_CLIENTBOUND_KNOWN_PACKS: usize = 1_024;
const MAX_SERVERBOUND_KNOWN_PACKS: usize = 64;
const MAX_KNOWN_PACK_STRING: usize = 32_767;
const MIN_KNOWN_PACK_ENTRY_BYTES: usize = 3;
const MAX_REGISTRY_ENTRIES: usize = 65_536;
const MAX_TAG_REGISTRIES: usize = 1_024;
const MAX_TAGS_PER_REGISTRY: usize = 65_536;
const MAX_TAG_ENTRIES: usize = 1_048_576;

fn read_count<B: Buf>(buf: &mut B, max: usize) -> Result<usize, CodecError> {
    let count = buf.read_varint()?;
    if count < 0 {
        return Err(CodecError::NegativeLength(count));
    }
    let count = count as usize;
    if count > max {
        return Err(CodecError::StringTooLong { len: count, max });
    }
    Ok(count)
}

fn write_count<B: BufMut>(buf: &mut B, count: usize, max: usize) -> Result<(), CodecError> {
    if count > max {
        return Err(CodecError::StringTooLong { len: count, max });
    }
    buf.write_varint(count as i32);
    Ok(())
}

// -----------------------------------------------------------------------
// Known Packs — both directions share the same struct.
// -----------------------------------------------------------------------

/// One entry in a Known Packs list. Vanilla uses `{ "minecraft", "core",
/// <game version> }` to refer to the built-in resource/data pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPackEntry {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

fn write_known_pack_array<B: BufMut>(
    buf: &mut B,
    entries: &[KnownPackEntry],
    max: usize,
) -> Result<(), CodecError> {
    if entries.len() > max {
        return Err(CodecError::StringTooLong {
            len: entries.len(),
            max,
        });
    }
    for entry in entries {
        validate_known_pack_string(&entry.namespace)?;
        validate_known_pack_string(&entry.id)?;
        validate_known_pack_string(&entry.version)?;
    }

    write_count(buf, entries.len(), max)?;
    for entry in entries {
        buf.write_string(&entry.namespace, MAX_KNOWN_PACK_STRING)?;
        buf.write_string(&entry.id, MAX_KNOWN_PACK_STRING)?;
        buf.write_string(&entry.version, MAX_KNOWN_PACK_STRING)?;
    }
    Ok(())
}

fn validate_known_pack_string(value: &str) -> Result<(), CodecError> {
    let len = value.encode_utf16().count();
    if len > MAX_KNOWN_PACK_STRING {
        return Err(CodecError::StringTooLong {
            len,
            max: MAX_KNOWN_PACK_STRING,
        });
    }
    Ok(())
}

fn read_known_pack_array<B: Buf>(
    buf: &mut B,
    max: usize,
) -> Result<Vec<KnownPackEntry>, CodecError> {
    let count = read_count(buf, max)?;
    let minimum_body_bytes = count * MIN_KNOWN_PACK_ENTRY_BYTES;
    let available = buf.remaining();
    if available < minimum_body_bytes {
        return Err(CodecError::Underflow {
            needed: minimum_body_bytes - available,
            available,
        });
    }

    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let namespace = buf.read_string(MAX_KNOWN_PACK_STRING)?;
        let id = buf.read_string(MAX_KNOWN_PACK_STRING)?;
        let version = buf.read_string(MAX_KNOWN_PACK_STRING)?;
        entries.push(KnownPackEntry {
            namespace,
            id,
            version,
        });
    }
    Ok(entries)
}

/// Clientbound 0x0E — server advertises the data packs it knows about so
/// the client can match them against its built-ins and decide which
/// registry entries it can read from its own bundle vs. needs the server
/// to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundKnownPacks {
    pub packs: Vec<KnownPackEntry>,
}

impl Packet for ClientboundKnownPacks {
    const ID: i32 = 0x0E;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_known_pack_array(buf, &self.packs, MAX_CLIENTBOUND_KNOWN_PACKS)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            packs: read_known_pack_array(buf, MAX_CLIENTBOUND_KNOWN_PACKS)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundCustomPayload {
    pub payload: CustomPayload,
}

impl Packet for ClientboundCustomPayload {
    // `.analysis/protocol-dump.txt`: configuration CLIENTBOUND_CUSTOM_PAYLOAD
    // is clientbound registration index 1, wire id 0x01. Body is the common
    // custom-payload codec from local decompiled sources.
    const ID: i32 = 0x01;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.payload.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: CustomPayload::decode(buf)?,
        })
    }
}

/// Serverbound 0x07 — client tells the server which of the advertised
/// known packs it also has bundled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundKnownPacks {
    pub packs: Vec<KnownPackEntry>,
}

impl Packet for ServerboundKnownPacks {
    const ID: i32 = 0x07;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_known_pack_array(buf, &self.packs, MAX_SERVERBOUND_KNOWN_PACKS)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            packs: read_known_pack_array(buf, MAX_SERVERBOUND_KNOWN_PACKS)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundClientInformation {
    pub information: ClientInformation,
}

impl Packet for ServerboundClientInformation {
    // `.analysis/protocol-dump.txt`: configuration SERVERBOUND_CLIENT_INFORMATION
    // is the first serverbound registration, wire id 0x00. Body delegates to
    // decompiled `ServerboundClientInformationPacket(ClientInformation)`.
    const ID: i32 = 0x00;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.information.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            information: ClientInformation::decode(buf)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundCustomPayload {
    pub payload: CustomPayload,
}

impl Packet for ServerboundCustomPayload {
    // `.analysis/protocol-dump.txt`: configuration SERVERBOUND_CUSTOM_PAYLOAD
    // is serverbound registration index 2, wire id 0x02. Body is the common
    // custom-payload codec from local decompiled sources.
    const ID: i32 = 0x02;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.payload.encode_serverbound(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: CustomPayload::decode_serverbound(buf)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerboundResourcePack {
    pub status: ResourcePackStatus,
}

impl Packet for ServerboundResourcePack {
    // `.analysis/protocol-dump.txt`: configuration SERVERBOUND_RESOURCE_PACK is
    // serverbound registration index 6, wire id 0x06. Body is local decompiled
    // `ServerboundResourcePackPacket(UUID id, Action action)`.
    const ID: i32 = 0x06;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        self.status.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            status: ResourcePackStatus::decode(buf)?,
        })
    }
}

// -----------------------------------------------------------------------
// Finish Configuration — both directions, both empty bodies.
// -----------------------------------------------------------------------

/// Clientbound 0x03 — server tells client "I am done sending
/// Configuration packets, please move to Play state".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FinishConfiguration;

impl Packet for FinishConfiguration {
    const ID: i32 = 0x03;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

/// Serverbound 0x03 — client acks the transition to Play.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AcknowledgeFinishConfiguration;

impl Packet for AcknowledgeFinishConfiguration {
    const ID: i32 = 0x03;

    fn encode<B: BufMut>(&self, _buf: &mut B) -> Result<(), CodecError> {
        Ok(())
    }

    fn decode<B: Buf>(_buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self)
    }
}

// -----------------------------------------------------------------------
// Registry Data (0x07 CB).
// -----------------------------------------------------------------------

/// One registry entry. If `has_data` is `false` the client uses its
/// built-in data for this entry (matched via the Known Packs handshake).
///
/// The `nbt_payload` field contains a pre-serialised, root-less Network-NBT
/// compound. Keeping the encoded bytes avoids rebuilding immutable registry
/// payloads for every connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub name: Identifier,
    pub nbt_payload: Option<Arc<[u8]>>,
}

/// Clientbound 0x07 — one packet per registry, sent between Known Packs
/// and Finish Configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryData {
    pub registry_id: Identifier,
    pub entries: Vec<RegistryEntry>,
}

impl Packet for RegistryData {
    const ID: i32 = 0x07;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_identifier(&self.registry_id)?;
        write_count(buf, self.entries.len(), MAX_REGISTRY_ENTRIES)?;
        for entry in &self.entries {
            buf.write_identifier(&entry.name)?;
            match &entry.nbt_payload {
                Some(payload) => {
                    buf.write_bool(true);
                    buf.put_slice(payload);
                }
                None => buf.write_bool(false),
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let registry_id = buf.read_identifier()?;
        let count = read_count(buf, MAX_REGISTRY_ENTRIES)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let name = buf.read_identifier()?;
            let has_data = buf.read_bool()?;
            let nbt_payload = if has_data {
                let root = mc_nbt::read_network(buf)?;
                let mut payload = Vec::new();
                mc_nbt::write_network(&mut payload, &root)?;
                Some(payload.into())
            } else {
                None
            };
            entries.push(RegistryEntry { name, nbt_payload });
        }
        Ok(Self {
            registry_id,
            entries,
        })
    }
}

// -----------------------------------------------------------------------
// Update Tags (0x0D CB).
//
// Wire shape per `ClientboundUpdateTagsPacket` +
// `TagNetworkSerialization$NetworkPayload.write()` in the unobfuscated
// vanilla 26.1.2 jar (ADR 0002 javap): a `Map<ResourceKey<Registry>,
// NetworkPayload>` written as
//
// ```text
// VarInt num_registries
// per registry:
//     Identifier registry_id     (e.g. "minecraft:item")
//     VarInt num_tags
//     per tag:
//         Identifier tag_name     (e.g. "minecraft:enchantable/melee_weapon")
//         VarInt num_entries
//         VarInt[num_entries] entry_ids
// ```
//
// `entry_ids` are the *client-side* indices into the matching registry:
// for built-in registries (item, block, entity_type, fluid, …) those
// are the `protocol_id`s from Mojang's `reports/registries.json`; for
// data-driven registries we ship via `RegistryData` (enchantment,
// damage_type, …) they are the *position in the entries vec we sent*.
// `mc_data::tags` produces the right numbers for both flavours.
// -----------------------------------------------------------------------

/// One tag mapping inside [`UpdateTags`]: the tag identifier plus the
/// numeric ids of the registry entries it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTagsEntry {
    pub tag: Identifier,
    pub entries: Vec<i32>,
}

/// One registry's tag block inside [`UpdateTags`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTagsRegistry {
    pub registry: Identifier,
    pub tags: Vec<UpdateTagsEntry>,
}

/// Clientbound 0x0D — the full tag set for every registry the client
/// is about to load. Sent once between the last `RegistryData` and
/// `FinishConfiguration`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateTags {
    pub registries: Vec<UpdateTagsRegistry>,
}

impl Packet for UpdateTags {
    // Verified via `javap` of vanilla 26.1.2's ConfigurationProtocols:
    // CLIENTBOUND_UPDATE_TAGS at configuration-CB index 13 = wire id 0x0D.
    const ID: i32 = 0x0D;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        write_count(buf, self.registries.len(), MAX_TAG_REGISTRIES)?;
        for reg in &self.registries {
            buf.write_identifier(&reg.registry)?;
            write_count(buf, reg.tags.len(), MAX_TAGS_PER_REGISTRY)?;
            for tag in &reg.tags {
                buf.write_identifier(&tag.tag)?;
                write_count(buf, tag.entries.len(), MAX_TAG_ENTRIES)?;
                for &id in &tag.entries {
                    buf.write_varint(id);
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let reg_count = read_count(buf, MAX_TAG_REGISTRIES)?;
        let mut registries = Vec::with_capacity(reg_count);
        for _ in 0..reg_count {
            let registry = buf.read_identifier()?;
            let tag_count = read_count(buf, MAX_TAGS_PER_REGISTRY)?;
            let mut tags = Vec::with_capacity(tag_count);
            for _ in 0..tag_count {
                let tag = buf.read_identifier()?;
                let entry_count = read_count(buf, MAX_TAG_ENTRIES)?;
                let mut entries = Vec::with_capacity(entry_count);
                for _ in 0..entry_count {
                    entries.push(buf.read_varint()?);
                }
                tags.push(UpdateTagsEntry { tag, entries });
            }
            registries.push(UpdateTagsRegistry { registry, tags });
        }
        Ok(Self { registries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<P: Packet + PartialEq + std::fmt::Debug>(p: P) {
        let mut buf = Vec::new();
        p.encode(&mut buf).unwrap();
        let mut cursor: &[u8] = &buf;
        let decoded: P = P::decode(&mut cursor).unwrap();
        assert_eq!(decoded, p);
        assert!(cursor.is_empty());
    }

    fn sample_pack() -> KnownPackEntry {
        KnownPackEntry {
            namespace: "minecraft".into(),
            id: "core".into(),
            version: "26.1.2".into(),
        }
    }

    #[test]
    fn clientbound_known_packs_round_trip() {
        round_trip(ClientboundKnownPacks {
            packs: vec![sample_pack()],
        });
        round_trip(ClientboundKnownPacks { packs: vec![] });
        round_trip(ClientboundKnownPacks {
            packs: vec![sample_pack(); 65],
        });
    }

    #[test]
    fn clientbound_custom_payload_id_and_layout_match_local_decompiled_sources() {
        assert_eq!(ClientboundCustomPayload::ID, 0x01);
        let packet = ClientboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse("solaris:test").unwrap(),
                payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(
            buf,
            vec![
                0x0c, b's', b'o', b'l', b'a', b'r', b'i', b's', b':', b't', b'e', b's', b't', 0xDE,
                0xAD, 0xBE, 0xEF,
            ]
        );
        let mut cursor: &[u8] = &buf;
        assert_eq!(
            ClientboundCustomPayload::decode(&mut cursor).unwrap(),
            packet
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn serverbound_known_packs_round_trip() {
        round_trip(ServerboundKnownPacks { packs: Vec::new() });
        round_trip(ServerboundKnownPacks {
            packs: vec![sample_pack(), sample_pack()],
        });
    }

    #[test]
    fn serverbound_known_packs_accepts_exact_vanilla_maximum() {
        round_trip(ServerboundKnownPacks {
            packs: vec![sample_pack(); 64],
        });
    }

    #[test]
    fn serverbound_known_packs_rejects_maximum_plus_one_on_encode_and_decode() {
        let mut encoded = vec![0xAA, 0x55];
        let error = ServerboundKnownPacks {
            packs: vec![sample_pack(); 65],
        }
        .encode(&mut encoded)
        .unwrap_err();
        assert_eq!(error, CodecError::StringTooLong { len: 65, max: 64 });
        assert_eq!(encoded, [0xAA, 0x55]);

        let mut buf = Vec::new();
        buf.write_varint(65);
        let error = ServerboundKnownPacks::decode(&mut buf.as_slice()).unwrap_err();
        assert_eq!(error, CodecError::StringTooLong { len: 65, max: 64 });
    }

    #[test]
    fn serverbound_known_packs_rejects_absurd_count_before_allocation() {
        let mut buf = Vec::new();
        buf.write_varint(i32::MAX);
        let error = ServerboundKnownPacks::decode(&mut buf.as_slice()).unwrap_err();
        assert_eq!(
            error,
            CodecError::StringTooLong {
                len: i32::MAX as usize,
                max: 64,
            }
        );
    }

    #[test]
    fn serverbound_known_packs_rejects_truncated_count() {
        let error = ServerboundKnownPacks::decode(&mut [0x80].as_slice()).unwrap_err();
        assert_eq!(
            error,
            CodecError::Underflow {
                needed: 1,
                available: 0,
            }
        );
    }

    #[test]
    fn serverbound_known_packs_rejects_negative_and_overlong_counts() {
        let mut negative = Vec::new();
        negative.write_varint(-1);
        assert_eq!(
            ServerboundKnownPacks::decode(&mut negative.as_slice()).unwrap_err(),
            CodecError::NegativeLength(-1)
        );

        let mut overlong = [0x80, 0x80, 0x80, 0x80, 0x80, 0x00].as_slice();
        assert_eq!(
            ServerboundKnownPacks::decode(&mut overlong).unwrap_err(),
            CodecError::VarIntTooLong
        );
    }

    #[test]
    fn serverbound_known_packs_checks_minimum_entry_bytes_before_reserving() {
        let mut encoded = Vec::new();
        encoded.write_varint(64);

        assert_eq!(
            ServerboundKnownPacks::decode(&mut encoded.as_slice()).unwrap_err(),
            CodecError::Underflow {
                needed: 64 * 3,
                available: 0,
            }
        );
    }

    #[test]
    fn serverbound_known_packs_accepts_exact_nested_string_maximum() {
        let maximum = "x".repeat(32_767);
        round_trip(ServerboundKnownPacks {
            packs: vec![KnownPackEntry {
                namespace: maximum.clone(),
                id: maximum.clone(),
                version: maximum,
            }],
        });
    }

    #[test]
    fn known_pack_encode_preflights_every_nested_string() {
        let oversized = "x".repeat(32_768);

        for field in 0..3 {
            let mut invalid = sample_pack();
            match field {
                0 => invalid.namespace = oversized.clone(),
                1 => invalid.id = oversized.clone(),
                2 => invalid.version = oversized.clone(),
                _ => unreachable!(),
            }

            let packet = ServerboundKnownPacks {
                packs: vec![sample_pack(), invalid],
            };
            let mut encoded = vec![0xAA, 0x55];
            assert_eq!(
                packet.encode(&mut encoded).unwrap_err(),
                CodecError::StringTooLong {
                    len: 32_768,
                    max: 32_767,
                }
            );
            assert_eq!(encoded, [0xAA, 0x55], "field {field} wrote a packet prefix");
        }
    }

    fn sample_client_information() -> ClientInformation {
        ClientInformation {
            language: "en_us".to_string(),
            view_distance: 12,
            chat_visibility: super::super::ChatVisibility::System,
            chat_colors: true,
            model_customisation: 0x7f,
            main_hand: super::super::MainHand::Right,
            text_filtering_enabled: false,
            allows_listing: true,
            particle_status: super::super::ParticleStatus::Decreased,
        }
    }

    #[test]
    fn serverbound_client_information_id_and_layout_match_local_decompiled_sources() {
        assert_eq!(ServerboundClientInformation::ID, 0x00);
        let packet = ServerboundClientInformation {
            information: sample_client_information(),
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(
            buf,
            vec![
                0x05, b'e', b'n', b'_', b'u', b's', // readUtf(16)
                12,   // readByte viewDistance
                1,    // readEnum(ChatVisiblity.SYSTEM)
                1,    // chatColors
                0x7f, // readUnsignedByte modelCustomisation
                1,    // readEnum(HumanoidArm.RIGHT)
                0,    // textFilteringEnabled
                1,    // allowsListing
                1,    // readEnum(ParticleStatus.DECREASED)
            ]
        );
        let mut cursor: &[u8] = &buf;
        assert_eq!(
            ServerboundClientInformation::decode(&mut cursor).unwrap(),
            packet
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn serverbound_custom_payload_brand_id_and_layout_match_local_decompiled_sources() {
        assert_eq!(ServerboundCustomPayload::ID, 0x02);
        let packet = ServerboundCustomPayload {
            payload: CustomPayload::Brand("vanilla".to_string()),
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(
            buf,
            vec![
                0x0f, b'm', b'i', b'n', b'e', b'c', b'r', b'a', b'f', b't', b':', b'b', b'r', b'a',
                b'n', b'd', 0x07, b'v', b'a', b'n', b'i', b'l', b'l', b'a',
            ]
        );
        let mut cursor: &[u8] = &buf;
        assert_eq!(
            ServerboundCustomPayload::decode(&mut cursor).unwrap(),
            packet
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn serverbound_brand_payload_preflights_string_before_configuration_output() {
        round_trip(ServerboundCustomPayload {
            payload: CustomPayload::Brand("x".repeat(32_767)),
        });

        let oversized = ServerboundCustomPayload {
            payload: CustomPayload::Brand("x".repeat(32_768)),
        };
        let mut encoded = vec![0xA5, 0x5A];
        assert_eq!(
            oversized.encode(&mut encoded).unwrap_err(),
            CodecError::StringTooLong {
                len: 32_768,
                max: 32_767,
            }
        );
        assert_eq!(encoded, [0xA5, 0x5A]);
    }

    #[test]
    fn serverbound_custom_payload_enforces_unknown_body_limit_symmetrically() {
        let channel = Identifier::parse("solaris:test").unwrap();
        let packet = ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: channel.clone(),
                payload: vec![0xAB; 32_767],
            },
        };
        round_trip(packet);

        let oversized = ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel,
                payload: vec![0xAB; 32_768],
            },
        };
        assert_eq!(
            oversized.encode(&mut Vec::new()).unwrap_err(),
            CodecError::StringTooLong {
                len: 32_768,
                max: 32_767,
            }
        );

        let mut buf = Vec::new();
        buf.write_identifier(oversized.payload.channel()).unwrap();
        buf.resize(buf.len() + 32_768, 0xAB);
        assert_eq!(
            ServerboundCustomPayload::decode(&mut buf.as_slice()).unwrap_err(),
            CodecError::StringTooLong {
                len: 32_768,
                max: 32_767,
            }
        );
    }

    #[test]
    fn clientbound_custom_payload_does_not_use_serverbound_body_limit() {
        round_trip(ClientboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse("solaris:test").unwrap(),
                payload: vec![0xCD; 32_768],
            },
        });
    }

    #[test]
    fn serverbound_resource_pack_id_and_layout_match_local_decompiled_sources() {
        assert_eq!(ServerboundResourcePack::ID, 0x06);
        let packet = ServerboundResourcePack {
            status: ResourcePackStatus {
                id: uuid::Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff),
                action: super::super::ResourcePackAction::Downloaded,
            },
        };
        let mut buf = Vec::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(
            buf,
            vec![
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff, 0x04,
            ]
        );
        let mut cursor: &[u8] = &buf;
        assert_eq!(
            ServerboundResourcePack::decode(&mut cursor).unwrap(),
            packet
        );
        assert!(cursor.is_empty());
    }

    #[test]
    fn finish_configuration_round_trip() {
        round_trip(FinishConfiguration);
    }

    #[test]
    fn acknowledge_finish_configuration_round_trip() {
        round_trip(AcknowledgeFinishConfiguration);
    }

    #[test]
    fn registry_data_empty_entries_round_trip() {
        round_trip(RegistryData {
            registry_id: Identifier::parse("minecraft:dimension_type").unwrap(),
            entries: vec![],
        });
    }

    #[test]
    fn registry_data_entries_without_payload_round_trip() {
        round_trip(RegistryData {
            registry_id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
            entries: vec![
                RegistryEntry {
                    name: Identifier::parse("minecraft:plains").unwrap(),
                    nbt_payload: None,
                },
                RegistryEntry {
                    name: Identifier::parse("minecraft:desert").unwrap(),
                    nbt_payload: None,
                },
            ],
        });
    }

    #[test]
    fn registry_data_rejects_oversized_entry_count_before_allocation() {
        let mut buf = Vec::new();
        buf.write_identifier(&Identifier::parse("minecraft:test").unwrap())
            .unwrap();
        buf.write_varint(65_537);
        let error = RegistryData::decode(&mut buf.as_slice()).unwrap_err();
        assert_eq!(
            error,
            CodecError::StringTooLong {
                len: 65_537,
                max: 65_536,
            }
        );
    }

    #[test]
    fn update_tags_id_matches_javap() {
        assert_eq!(UpdateTags::ID, 0x0D);
    }

    #[test]
    fn update_tags_empty_round_trips() {
        round_trip(UpdateTags::default());
    }

    #[test]
    fn update_tags_with_entries_round_trips() {
        round_trip(UpdateTags {
            registries: vec![
                UpdateTagsRegistry {
                    registry: Identifier::parse("minecraft:item").unwrap(),
                    tags: vec![
                        UpdateTagsEntry {
                            tag: Identifier::parse("minecraft:enchantable/melee_weapon").unwrap(),
                            entries: vec![7, 42, 1024],
                        },
                        UpdateTagsEntry {
                            tag: Identifier::parse("minecraft:arrows").unwrap(),
                            entries: vec![],
                        },
                    ],
                },
                UpdateTagsRegistry {
                    registry: Identifier::parse("minecraft:enchantment").unwrap(),
                    tags: vec![UpdateTagsEntry {
                        tag: Identifier::parse("minecraft:exclusive_set/armor").unwrap(),
                        entries: vec![0, 1, 2, 3],
                    }],
                },
            ],
        });
    }

    #[test]
    fn update_tags_byte_layout_minimum() {
        // Empty payload = a single VarInt(0).
        let mut buf = Vec::new();
        UpdateTags::default().encode(&mut buf).unwrap();
        assert_eq!(buf, vec![0]);
    }

    #[test]
    fn update_tags_rejects_oversized_nested_counts_before_allocation() {
        let mut registries = Vec::new();
        registries.write_varint(1_025);
        assert_eq!(
            UpdateTags::decode(&mut registries.as_slice()).unwrap_err(),
            CodecError::StringTooLong {
                len: 1_025,
                max: 1_024,
            }
        );

        let mut tags = Vec::new();
        tags.write_varint(1);
        tags.write_identifier(&Identifier::parse("minecraft:item").unwrap())
            .unwrap();
        tags.write_varint(65_537);
        assert_eq!(
            UpdateTags::decode(&mut tags.as_slice()).unwrap_err(),
            CodecError::StringTooLong {
                len: 65_537,
                max: 65_536,
            }
        );

        let mut entries = Vec::new();
        entries.write_varint(1);
        entries
            .write_identifier(&Identifier::parse("minecraft:item").unwrap())
            .unwrap();
        entries.write_varint(1);
        entries
            .write_identifier(&Identifier::parse("minecraft:test").unwrap())
            .unwrap();
        entries.write_varint(1_048_577);
        assert_eq!(
            UpdateTags::decode(&mut entries.as_slice()).unwrap_err(),
            CodecError::StringTooLong {
                len: 1_048_577,
                max: 1_048_576,
            }
        );
    }

    #[test]
    fn registry_data_entries_with_network_nbt_round_trip() {
        let mut payload = Vec::new();
        mc_nbt::write_network(
            &mut payload,
            &mc_nbt::Tag::Compound(vec![
                ("message_id".into(), mc_nbt::Tag::String("test.foo".into())),
                ("exhaustion".into(), mc_nbt::Tag::Float(0.1)),
            ]),
        )
        .unwrap();
        round_trip(RegistryData {
            registry_id: Identifier::parse("minecraft:damage_type").unwrap(),
            entries: vec![RegistryEntry {
                name: Identifier::parse("minecraft:test").unwrap(),
                nbt_payload: Some(payload.into()),
            }],
        });
    }
}
