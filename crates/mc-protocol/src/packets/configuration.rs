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

use bytes::{Buf, BufMut};

use super::{ClientInformation, CustomPayload, Packet, ResourcePackStatus};
use crate::codec::{Identifier, ReadMc, WriteMc};
use crate::error::CodecError;

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
) -> Result<(), CodecError> {
    buf.write_varint(
        i32::try_from(entries.len()).map_err(|_| CodecError::StringTooLong {
            len: entries.len(),
            max: i32::MAX as usize,
        })?,
    );
    for entry in entries {
        buf.write_string(&entry.namespace, 32_767)?;
        buf.write_string(&entry.id, 32_767)?;
        buf.write_string(&entry.version, 32_767)?;
    }
    Ok(())
}

fn read_known_pack_array<B: Buf>(buf: &mut B) -> Result<Vec<KnownPackEntry>, CodecError> {
    let count = buf.read_varint()?;
    if count < 0 {
        return Err(CodecError::NegativeLength(count));
    }
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let namespace = buf.read_string(32_767)?;
        let id = buf.read_string(32_767)?;
        let version = buf.read_string(32_767)?;
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
        write_known_pack_array(buf, &self.packs)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            packs: read_known_pack_array(buf)?,
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
        write_known_pack_array(buf, &self.packs)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            packs: read_known_pack_array(buf)?,
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
        self.payload.encode(buf)
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        Ok(Self {
            payload: CustomPayload::decode(buf)?,
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
// Registry Data (0x07 CB) — defined for completeness but the M1.e handler
// does not send any. Encoding entries with NBT payloads is deferred to
// the milestone that actually needs them.
// -----------------------------------------------------------------------

/// One registry entry. If `has_data` is `false` the client uses its
/// built-in data for this entry (matched via the Known Packs handshake).
///
/// The `nbt_payload` field is intentionally `Vec<u8>`-typed and assumed
/// to be a pre-serialised, root-less Network-NBT blob. We do not have a
/// typed NBT codec yet (M1.f); for M1.e all entries we emit have
/// `has_data = false` and the field is never inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub name: Identifier,
    pub nbt_payload: Option<Vec<u8>>,
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
        buf.write_identifier(&self.registry_id);
        buf.write_varint(i32::try_from(self.entries.len()).map_err(|_| {
            CodecError::StringTooLong {
                len: self.entries.len(),
                max: i32::MAX as usize,
            }
        })?);
        for entry in &self.entries {
            buf.write_identifier(&entry.name);
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

    /// Decoding `RegistryData` with payload entries requires a real NBT
    /// parser (the payload is self-delimiting, not length-prefixed). We
    /// only need decoding for tests that confirm we can round-trip the
    /// "no payloads" shape we actually send.
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let registry_id = buf.read_identifier()?;
        let count = buf.read_varint()?;
        if count < 0 {
            return Err(CodecError::NegativeLength(count));
        }
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = buf.read_identifier()?;
            let has_data = buf.read_bool()?;
            if has_data {
                return Err(CodecError::InvalidIdentifier(
                    "RegistryData entries with NBT payloads cannot be decoded \
                     without a Network-NBT parser (deferred to M1.f)"
                        .to_string(),
                ));
            }
            entries.push(RegistryEntry {
                name,
                nbt_payload: None,
            });
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
        buf.write_varint(i32::try_from(self.registries.len()).map_err(|_| {
            CodecError::StringTooLong {
                len: self.registries.len(),
                max: i32::MAX as usize,
            }
        })?);
        for reg in &self.registries {
            buf.write_identifier(&reg.registry);
            buf.write_varint(i32::try_from(reg.tags.len()).map_err(|_| {
                CodecError::StringTooLong {
                    len: reg.tags.len(),
                    max: i32::MAX as usize,
                }
            })?);
            for tag in &reg.tags {
                buf.write_identifier(&tag.tag);
                buf.write_varint(i32::try_from(tag.entries.len()).map_err(|_| {
                    CodecError::StringTooLong {
                        len: tag.entries.len(),
                        max: i32::MAX as usize,
                    }
                })?);
                for &id in &tag.entries {
                    buf.write_varint(id);
                }
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let reg_count = buf.read_varint()?;
        if reg_count < 0 {
            return Err(CodecError::NegativeLength(reg_count));
        }
        let mut registries = Vec::with_capacity(reg_count as usize);
        for _ in 0..reg_count {
            let registry = buf.read_identifier()?;
            let tag_count = buf.read_varint()?;
            if tag_count < 0 {
                return Err(CodecError::NegativeLength(tag_count));
            }
            let mut tags = Vec::with_capacity(tag_count as usize);
            for _ in 0..tag_count {
                let tag = buf.read_identifier()?;
                let entry_count = buf.read_varint()?;
                if entry_count < 0 {
                    return Err(CodecError::NegativeLength(entry_count));
                }
                let mut entries = Vec::with_capacity(entry_count as usize);
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
    }

    #[test]
    fn serverbound_known_packs_round_trip() {
        round_trip(ServerboundKnownPacks {
            packs: vec![sample_pack(), sample_pack()],
        });
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
    fn registry_data_decode_refuses_inline_nbt() {
        // Manually build a wire layout with has_data=true to confirm we
        // surface the limitation rather than silently mis-parsing.
        let mut buf = Vec::new();
        buf.write_identifier(&Identifier::parse("minecraft:foo").unwrap());
        buf.write_varint(1);
        buf.write_identifier(&Identifier::parse("minecraft:bar").unwrap());
        buf.write_bool(true);
        let mut cursor: &[u8] = &buf;
        let err = RegistryData::decode(&mut cursor).unwrap_err();
        assert!(matches!(err, CodecError::InvalidIdentifier(_)));
    }
}
