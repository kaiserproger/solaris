use super::*;

fn player_info_entry(player: &PlayerEntitySnapshot) -> PlayerInfoEntry {
    PlayerInfoEntry {
        profile_id: player.uuid,
        name: player.name.clone(),
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
        delta_movement: EntityVec3 {
            x: entity.velocity.x,
            y: entity.velocity.y,
            z: entity.velocity.z,
        },
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
            movement: EntityVec3 {
                x: entity.velocity.x,
                y: entity.velocity.y,
                z: entity.velocity.z,
            },
            pitch: entity.rotation.pitch,
            yaw: entity.rotation.yaw,
            head_yaw: entity.rotation.head_yaw,
            data: entity.experience_value.unwrap_or(0),
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
    let Some(stack) = entity.item_stack else {
        return Ok(());
    };
    write_packet(
        writer,
        &ClientboundSetEntityData {
            entity_id: entity.id.0,
            values: vec![EntityDataValue::ItemStack {
                index: ITEM_ENTITY_DATA_ITEM_INDEX,
                stack: ItemStack::new(stack.item_id, stack.count),
            }],
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
            movement: EntityVec3 {
                x: entity.velocity.x,
                y: entity.velocity.y,
                z: entity.velocity.z,
            },
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
    if movement.send_velocity {
        write_packet(
            writer,
            &SetEntityMotion {
                entity_id: movement.id.0,
                movement: EntityVec3 {
                    x: movement.velocity.x,
                    y: movement.velocity.y,
                    z: movement.velocity.z,
                },
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
