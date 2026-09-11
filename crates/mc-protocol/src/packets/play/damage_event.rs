use bytes::{Buf, BufMut};

use crate::codec::{ReadMc, WriteMc};
use crate::error::CodecError;
use crate::packets::Packet;

/// 26.1.2 ClientboundDamageEventPacket, not the obsolete hurt entity event 2.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientboundDamageEvent {
    pub entity_id: i32,
    pub source_type_id: i32,
    /// Vanilla uses -1 for an absent entity, encoded as VarInt zero.
    pub source_cause_id: i32,
    pub source_direct_id: i32,
    pub source_position: Option<[f64; 3]>,
}

impl Packet for ClientboundDamageEvent {
    // 26.1.2 GameProtocols clientbound registration 25 (including bundle delimiter).
    const ID: i32 = 0x19;

    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError> {
        buf.write_varint(self.entity_id);
        buf.write_varint(self.source_type_id);
        buf.write_varint(self.source_cause_id.wrapping_add(1));
        buf.write_varint(self.source_direct_id.wrapping_add(1));
        buf.write_bool(self.source_position.is_some());
        if let Some(position) = self.source_position {
            for coordinate in position {
                buf.write_f64(coordinate);
            }
        }
        Ok(())
    }

    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError> {
        let entity_id = buf.read_varint()?;
        let source_type_id = buf.read_varint()?;
        let source_cause_id = buf.read_varint()?.wrapping_sub(1);
        let source_direct_id = buf.read_varint()?.wrapping_sub(1);
        let source_position = if buf.read_bool()? {
            Some([buf.read_f64()?, buf.read_f64()?, buf.read_f64()?])
        } else {
            None
        };
        Ok(Self {
            entity_id,
            source_type_id,
            source_cause_id,
            source_direct_id,
            source_position,
        })
    }
}
