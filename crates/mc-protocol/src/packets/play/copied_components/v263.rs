//! 26.3 additions, transcribed from the local official component STREAM_CODECs.
use super::*;

pub(super) fn encode<B: BufMut>(
    buf: &mut B,
    id: i32,
    value: &Tag,
    depth: usize,
) -> Result<(), CodecError> {
    match id {
        ids::ATTACK_ANIMATION | ids::INTERACT_ANIMATION => {
            let fields = compound(value)?;
            let animation = field(fields, "type")
                .map(string)
                .transpose()?
                .unwrap_or("whack");
            buf.write_varint(match animation {
                "none" => 0,
                "whack" => 1,
                "stab" => 2,
                _ => return Err(invalid()),
            });
            buf.write_varint(
                field(fields, "duration")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(6),
            );
        }
        ids::BLOCK_TRANSFORMER => {
            buf.write_varint(registry_id("block_transformer", string(value)?)?)
        }
        ids::PROVIDES_POTTERY_PATTERN => {
            buf.write_varint(registry_id("decorated_pot_pattern", string(value)?)?)
        }
        ids::VILLAGER_FOOD => buf.write_varint(integer(required(compound(value)?, "nutrition")?)?),
        ids::SULFUR_CUBE_CONTENT => templates::encode(buf, value, depth)?,
        ids::COMPOSTABLE | ids::COOKING_FUEL | ids::BREWING_FUEL => {
            let fields = compound(value)?;
            let name = match id {
                ids::COMPOSTABLE => "layers",
                ids::COOKING_FUEL => "burn_time",
                _ => "uses",
            };
            write_resolvable(buf, required(fields, name)?, false)?;
            if id != ids::COMPOSTABLE {
                write_resolvable(buf, required(fields, "speed_multiplier")?, true)?;
            }
        }
        ids::MOB_VISIBILITY => {
            let fields = compound(value)?;
            let targets = required(fields, "targeting_entity_types")?;
            if let Tag::String(name) = targets {
                if let Some(tag) = name.strip_prefix('#') {
                    buf.write_varint(0);
                    buf.write_identifier(&Identifier::parse(tag).map_err(|_| invalid())?)?;
                } else {
                    buf.write_varint(2);
                    buf.write_varint(registry_id("entity_type", name)?);
                }
            } else {
                let targets = list(targets)?;
                write_count(buf, targets.len().checked_add(1).ok_or_else(invalid)?)?;
                for target in targets {
                    buf.write_varint(registry_id("entity_type", string(target)?)?);
                }
            }
            buf.write_f32(number(required(fields, "visibility")?)?);
        }
        ids::SIGN_TEXT_FRONT | ids::SIGN_TEXT_BACK => {
            let fields = compound(value)?;
            write_lines(buf, required(fields, "messages")?)?;
            let filtered = field(fields, "filtered_messages");
            buf.write_bool(filtered.is_some());
            if let Some(filtered) = filtered {
                write_lines(buf, filtered)?;
            }
            buf.write_varint(color(
                field(fields, "color")
                    .map(string)
                    .transpose()?
                    .unwrap_or("black"),
            )?);
            buf.write_bool(
                field(fields, "has_glowing_text")
                    .map(integer)
                    .transpose()?
                    .unwrap_or(0)
                    != 0,
            );
        }
        ids::WAXED | ids::UNBREAKABLE => {
            if !compound(value)?.is_empty() {
                return Err(invalid());
            }
        }
        ids::CUSHION_COLOR => buf.write_varint(color(string(value)?)?),
        _ => return Err(invalid()),
    }
    Ok(())
}

pub(super) fn decode<B: Buf>(buf: &mut B, id: i32, depth: usize) -> Result<Tag, CodecError> {
    Ok(match id {
        ids::ATTACK_ANIMATION | ids::INTERACT_ANIMATION => {
            let animation = match buf.read_varint()? {
                0 => "none",
                1 => "whack",
                2 => "stab",
                _ => return Err(invalid()),
            };
            Tag::Compound(vec![
                ("type".into(), Tag::String(animation.into())),
                ("duration".into(), Tag::Int(buf.read_varint()?)),
            ])
        }
        ids::BLOCK_TRANSFORMER => {
            Tag::String(registry_name("block_transformer", buf.read_varint()?)?)
        }
        ids::PROVIDES_POTTERY_PATTERN => {
            Tag::String(registry_name("decorated_pot_pattern", buf.read_varint()?)?)
        }
        ids::VILLAGER_FOOD => {
            Tag::Compound(vec![("nutrition".into(), Tag::Int(buf.read_varint()?))])
        }
        ids::SULFUR_CUBE_CONTENT => templates::decode(buf, depth)?,
        ids::COMPOSTABLE | ids::COOKING_FUEL | ids::BREWING_FUEL => {
            let name = match id {
                ids::COMPOSTABLE => "layers",
                ids::COOKING_FUEL => "burn_time",
                _ => "uses",
            };
            let mut fields = vec![(name.into(), read_resolvable(buf, false)?)];
            if id != ids::COMPOSTABLE {
                fields.push(("speed_multiplier".into(), read_resolvable(buf, true)?));
            }
            Tag::Compound(fields)
        }
        ids::MOB_VISIBILITY => {
            let count = buf.read_varint()?;
            let targets = if count == 0 {
                Tag::String(format!("#{}", buf.read_identifier()?.as_str()))
            } else {
                let count = count
                    .checked_sub(1)
                    .and_then(|count| usize::try_from(count).ok())
                    .ok_or_else(invalid)?;
                if count > 1024 || count > buf.remaining() {
                    return Err(invalid());
                }
                let mut targets = Vec::with_capacity(count);
                for _ in 0..count {
                    targets.push(Tag::String(registry_name(
                        "entity_type",
                        buf.read_varint()?,
                    )?));
                }
                make_list(targets)
            };
            Tag::Compound(vec![
                ("targeting_entity_types".into(), targets),
                ("visibility".into(), Tag::Float(buf.read_f32()?)),
            ])
        }
        ids::SIGN_TEXT_FRONT | ids::SIGN_TEXT_BACK => {
            let mut fields = vec![("messages".into(), read_lines(buf)?)];
            if buf.read_bool()? {
                fields.push(("filtered_messages".into(), read_lines(buf)?));
            }
            fields.push((
                "color".into(),
                Tag::String(color_name(buf.read_varint()?)?.into()),
            ));
            fields.push((
                "has_glowing_text".into(),
                Tag::Byte(i8::from(buf.read_bool()?)),
            ));
            Tag::Compound(fields)
        }
        ids::WAXED | ids::UNBREAKABLE => Tag::Compound(Vec::new()),
        ids::CUSHION_COLOR => Tag::String(color_name(buf.read_varint()?)?.into()),
        _ => return Err(invalid()),
    })
}

fn number(value: &Tag) -> Result<f32, CodecError> {
    match value {
        Tag::Float(value) => Ok(*value),
        Tag::Double(value) => Ok(*value as f32),
        _ => Ok(integer(value)? as f32),
    }
}
// ResolvableInt.Constant uses fixed INT, not VAR_INT. The Either selector
// is true for the constant and false for a resource-key identifier.
fn write_resolvable<B: BufMut>(buf: &mut B, value: &Tag, float: bool) -> Result<(), CodecError> {
    if let Tag::String(key) = value {
        buf.write_bool(false);
        buf.write_identifier(&Identifier::parse(key).map_err(|_| invalid())?)?;
    } else {
        buf.write_bool(true);
        if float {
            buf.write_f32(number(value)?);
        } else {
            buf.write_i32(integer(value)?);
        }
    }
    Ok(())
}
fn read_resolvable<B: Buf>(buf: &mut B, float: bool) -> Result<Tag, CodecError> {
    if buf.read_bool()? {
        if float {
            Ok(Tag::Float(buf.read_f32()?))
        } else {
            Ok(Tag::Int(buf.read_i32()?))
        }
    } else {
        Ok(Tag::String(buf.read_identifier()?.as_str().into()))
    }
}
fn write_lines<B: BufMut>(buf: &mut B, value: &Tag) -> Result<(), CodecError> {
    let lines = list(value)?;
    if lines.len() != 4 {
        return Err(invalid());
    }
    for line in lines {
        write_text(buf, line)?;
    }
    Ok(())
}
fn read_lines<B: Buf>(buf: &mut B) -> Result<Tag, CodecError> {
    let mut lines = Vec::with_capacity(4);
    for _ in 0..4 {
        lines.push(read_text(buf)?);
    }
    Ok(make_list(lines))
}
