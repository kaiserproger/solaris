use super::*;
use mc_protocol::packets::play::SetEntityMotion;

fn wire_velocity(velocity: Vec3) -> EntityVec3 {
    EntityVec3 {
        x: velocity.x * mc_physics::TICK_SECONDS,
        y: velocity.y * mc_physics::TICK_SECONDS,
        z: velocity.z * mc_physics::TICK_SECONDS,
    }
}

fn player_info_entry(player: &PlayerEntitySnapshot) -> PlayerInfoEntry {
    PlayerInfoEntry {
        profile_id: player.uuid,
        name: player.name.clone(),
        properties: player.properties.clone(),
        listed: true,
        latency: 0,
        game_mode: 0,
        list_order: player.entity_id,
        show_hat: true,
    }
}

fn player_position(player: &PlayerEntitySnapshot) -> PositionMoveRotation {
    PositionMoveRotation {
        position: EntityVec3 {
            x: player.pose.x,
            y: player.pose.y,
            z: player.pose.z,
        },
        delta_movement: EntityVec3::ZERO,
        yaw: player.pose.yaw,
        pitch: player.pose.pitch,
    }
}

fn entity_position(entity: &ServerEntitySnapshot) -> PositionMoveRotation {
    PositionMoveRotation {
        position: EntityVec3 {
            x: entity.position.x,
            y: entity.position.y,
            z: entity.position.z,
        },
        delta_movement: wire_velocity(entity.velocity),
        yaw: entity.rotation.yaw,
        pitch: entity.rotation.pitch,
    }
}

pub(super) async fn send_player_spawn<W>(
    writer: &mut W,
    compression: Compression,
    player: &PlayerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        target_session = player.session_id,
        entity_id = player.entity_id,
        player = %player.name,
        "spawning visible player"
    );
    write_packet(
        writer,
        &PlayerInfoUpdate {
            actions: PlayerInfoActions::minimal_add_player(),
            entries: vec![player_info_entry(player)],
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &AddEntity {
            entity_id: player.entity_id,
            uuid: player.uuid,
            entity_type_id: PLAYER_ENTITY_TYPE_ID,
            x: player.pose.x,
            y: player.pose.y,
            z: player.pose.z,
            movement: EntityVec3::ZERO,
            pitch: player.pose.pitch,
            yaw: player.pose.yaw,
            head_yaw: player.pose.yaw,
            data: 0,
        },
        compression,
    )
    .await?;
    send_player_data(writer, compression, player).await?;
    send_player_move(writer, compression, player).await
}

pub(super) async fn send_player_move<W>(
    writer: &mut W,
    compression: Compression,
    player: &PlayerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &EntityPositionSync {
            entity_id: player.entity_id,
            values: player_position(player),
            on_ground: player.pose.flags.on_ground,
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &RotateHead {
            entity_id: player.entity_id,
            head_yaw: player.pose.yaw,
        },
        compression,
    )
    .await?;
    send_player_data(writer, compression, player).await?;
    Ok(())
}

async fn send_player_data<W>(
    writer: &mut W,
    compression: Compression,
    player: &PlayerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundSetEntityData {
            entity_id: player.entity_id,
            values: player.pose.entity_data_values(),
        },
        compression,
    )
    .await?;
    Ok(())
}

pub(super) async fn send_player_despawn<W>(
    writer: &mut W,
    compression: Compression,
    player: &PlayerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        target_session = player.session_id,
        entity_id = player.entity_id,
        player = %player.name,
        "despawning visible player"
    );
    write_packet(
        writer,
        &RemoveEntities {
            entity_ids: vec![player.entity_id],
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &PlayerInfoRemove {
            profile_ids: vec![player.uuid],
        },
        compression,
    )
    .await?;
    Ok(())
}

pub(super) async fn send_entity_spawn<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        entity_id = entity.id.0,
        entity_type = %entity.type_name,
        "spawning visible server entity"
    );
    write_packet(
        writer,
        &AddEntity {
            entity_id: entity.id.0,
            uuid: entity.uuid,
            entity_type_id: entity.type_id,
            x: entity.position.x,
            y: entity.position.y,
            z: entity.position.z,
            movement: wire_velocity(entity.velocity),
            pitch: entity.rotation.pitch,
            yaw: entity.rotation.yaw,
            head_yaw: entity.rotation.head_yaw,
            data: entity
                .block_state
                .and_then(|state| i32::try_from(state).ok())
                .or(entity.experience_value)
                .unwrap_or(0),
        },
        compression,
    )
    .await?;
    send_entity_data(writer, compression, entity).await?;
    send_entity_move(writer, compression, entity).await
}

pub(super) async fn send_entity_data<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut values = Vec::new();
    if let Some(ref stack) = entity.item_stack {
        values.push(EntityDataValue::ItemStack {
            index: ITEM_ENTITY_DATA_ITEM_INDEX,
            stack: ItemStack {
                item_id: stack.item_id,
                count: stack.count,
                damage: stack.damage,
                enchantments: stack.enchantments.clone(),
            },
        });
    }
    if let Some(animal) = entity.animal {
        values.push(EntityDataValue::Boolean {
            index: AGEABLE_ENTITY_DATA_BABY_INDEX,
            value: animal.is_baby(),
        });
        if let Some(wool) = animal.sheep_wool {
            values.push(EntityDataValue::Byte {
                index: SHEEP_ENTITY_DATA_WOOL_INDEX,
                value: wool.packed_metadata(),
            });
        }
    }
    if values.is_empty() {
        return Ok(());
    }
    write_packet(
        writer,
        &ClientboundSetEntityData {
            entity_id: entity.id.0,
            values,
        },
        compression,
    )
    .await
}

pub(super) async fn send_entity_move<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &EntityPositionSync {
            entity_id: entity.id.0,
            values: entity_position(entity),
            on_ground: entity.on_ground,
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &SetEntityMotion {
            entity_id: entity.id.0,
            movement: wire_velocity(entity.velocity),
        },
        compression,
    )
    .await?;
    write_packet(
        writer,
        &RotateHead {
            entity_id: entity.id.0,
            head_yaw: entity.rotation.head_yaw,
        },
        compression,
    )
    .await?;
    Ok(())
}

pub(super) async fn send_entity_relative_move<W>(
    writer: &mut W,
    compression: Compression,
    movement: &ServerEntityMove,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug_assert!(movement.send_position_rotation || movement.send_velocity);
    if movement.send_position_rotation {
        write_packet(
            writer,
            &MoveEntityPosRot {
                entity_id: movement.id.0,
                delta_x: MoveEntityPosRot::delta_to_short(movement.delta.x),
                delta_y: MoveEntityPosRot::delta_to_short(movement.delta.y),
                delta_z: MoveEntityPosRot::delta_to_short(movement.delta.z),
                yaw: MoveEntityPosRot::pack_degrees(movement.rotation.yaw),
                pitch: MoveEntityPosRot::pack_degrees(movement.rotation.pitch),
                on_ground: movement.on_ground,
            },
            compression,
        )
        .await?;
        write_packet(
            writer,
            &RotateHead {
                entity_id: movement.id.0,
                head_yaw: movement.rotation.head_yaw,
            },
            compression,
        )
        .await?;
    }
    if movement.send_velocity {
        write_packet(
            writer,
            &SetEntityMotion {
                entity_id: movement.id.0,
                movement: wire_velocity(movement.velocity),
            },
            compression,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn send_entity_despawn<W>(
    writer: &mut W,
    compression: Compression,
    entity: &ServerEntitySnapshot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(
        entity_id = entity.id.0,
        entity_type = %entity.type_name,
        "despawning visible server entity"
    );
    write_packet(
        writer,
        &RemoveEntities {
            entity_ids: vec![entity.id.0],
        },
        compression,
    )
    .await
}

pub(super) async fn send_take_item_entity<W>(
    writer: &mut W,
    compression: Compression,
    item_entity_id: i32,
    player_entity_id: i32,
    amount: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &ClientboundTakeItemEntity {
            item_entity_id,
            player_entity_id,
            amount,
        },
        compression,
    )
    .await
}

pub(super) async fn send_player_animation<W>(
    writer: &mut W,
    compression: Compression,
    entity_id: i32,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    write_packet(
        writer,
        &EntityAnimation {
            entity_id,
            action: EntityAnimationAction::SwingMainHand,
        },
        compression,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physics_velocity_is_converted_from_blocks_per_second_to_blocks_per_tick() {
        let movement = wire_velocity(Vec3::new(
            2.0,
            mc_physics::LIVING_JUMP_SPEED_BLOCKS_PER_SECOND,
            -2.5,
        ));

        assert!((movement.x - 0.1).abs() < 1.0e-12);
        assert!((movement.y - 0.419_999_986_886_978_15).abs() < 1.0e-12);
        assert!((movement.z + 0.125).abs() < 1.0e-12);
    }

    #[tokio::test]
    async fn sheep_spawn_metadata_encodes_authoritative_color() {
        let entity = ServerEntitySnapshot {
            id: EntityId(42),
            uuid: uuid::Uuid::from_u128(42),
            type_id: 2,
            type_name: "minecraft:sheep".to_owned(),
            position: Vec3::new(1.5, 64.0, 1.5),
            rotation: Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            animal: Some(mc_entity::AnimalBreedingState::adult_sheep(
                mc_entity::SheepColor::Brown,
            )),
        };
        let mut writer = Vec::new();

        send_entity_data(&mut writer, Compression::Disabled, &entity)
            .await
            .unwrap();

        let mut bytes = BytesMut::from(writer.as_slice());
        let mut frame = mc_protocol::frame::try_decode_frame(&mut bytes, Compression::Disabled)
            .unwrap()
            .expect("set entity data frame");
        assert_eq!(frame.id, ClientboundSetEntityData::ID);
        let packet = ClientboundSetEntityData::decode(&mut frame.body).unwrap();
        assert_eq!(packet.entity_id, entity.id.0);
        assert!(packet.values.iter().any(|value| {
            matches!(
                value,
                EntityDataValue::Byte { index, value }
                    if *index == SHEEP_ENTITY_DATA_WOOL_INDEX
                        && *value == i8::try_from(mc_entity::SheepColor::Brown.id()).unwrap()
            )
        }));
        assert!(bytes.is_empty());
    }
}
