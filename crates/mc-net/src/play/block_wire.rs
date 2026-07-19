use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BlockDelta {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) z: i32,
    pub(super) state_id: mc_world::BlockStateId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BlockDeltaPacket {
    Single(BlockDelta),
    Section {
        section_x: i32,
        section_y: i32,
        section_z: i32,
        changes: Vec<BlockDelta>,
    },
}

pub(super) async fn send_block_deltas<W>(
    writer: &mut W,
    compression: Compression,
    deltas: &[BlockDelta],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for packet in plan_block_delta_packets(deltas) {
        match packet {
            BlockDeltaPacket::Single(delta) => {
                write_packet(
                    writer,
                    &BlockUpdate {
                        position: mc_protocol::packets::play::pack_block_pos(
                            delta.x, delta.y, delta.z,
                        ),
                        state_id: delta.state_id.0 as i32,
                    },
                    compression,
                )
                .await?;
            }
            BlockDeltaPacket::Section {
                section_x,
                section_y,
                section_z,
                changes,
            } => {
                write_packet(
                    writer,
                    &SectionBlocksUpdate {
                        section_pos: pack_section_pos(section_x, section_y, section_z),
                        changes: changes
                            .into_iter()
                            .map(|delta| SectionBlockChange {
                                relative_pos: pack_section_relative_pos(delta.x, delta.y, delta.z),
                                state_id: delta.state_id.0 as i32,
                            })
                            .collect(),
                    },
                    compression,
                )
                .await?;
            }
        }
    }
    Ok(())
}

pub(super) async fn send_light_updates<W>(
    state: &mut InteractionState,
    writer: &mut W,
    updates: &[OutboundLightUpdate],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    for update in updates {
        state.light_cache.insert(update.pos, update.light.clone());
        write_packet(
            writer,
            &LightUpdate {
                chunk_x: update.pos.x,
                chunk_z: update.pos.z,
                light: update.wire.clone(),
            },
            state.compression,
        )
        .await?;
    }
    Ok(())
}

pub(super) fn broadcast_block_deltas(
    state: &InteractionState,
    chunks: &HashSet<(i32, i32)>,
    deltas: &[BlockDelta],
    except: Option<SessionId>,
) {
    broadcast_block_deltas_to_sessions(&state.sessions, chunks, deltas, except);
}

pub(super) fn broadcast_block_deltas_to_sessions(
    sessions: &SessionRegistry,
    chunks: &HashSet<(i32, i32)>,
    deltas: &[BlockDelta],
    except: Option<SessionId>,
) {
    if deltas.is_empty() || chunks.is_empty() {
        return;
    }
    dispatch_visibility_commands(
        sessions
            .loaded_recipients_for_chunks(chunks, except)
            .into_iter()
            .map(|recipient| VisibilityDispatch {
                recipient,
                command: OutboundCommand::BlockDeltas(deltas.to_vec()),
            })
            .collect(),
    );
}

pub(super) fn broadcast_level_event(
    state: &InteractionState,
    position: mc_world::BlockPos,
    event_id: i32,
    data: i32,
    except: Option<SessionId>,
) {
    let chunks = HashSet::from([(position.x.div_euclid(16), position.z.div_euclid(16))]);
    dispatch_visibility_commands(
        state
            .sessions
            .loaded_recipients_for_chunks(&chunks, except)
            .into_iter()
            .map(|recipient| VisibilityDispatch {
                recipient,
                command: OutboundCommand::LevelEvent(LevelEvent {
                    event_id,
                    position: mc_protocol::packets::play::pack_block_pos(
                        position.x, position.y, position.z,
                    ),
                    data,
                    global: false,
                }),
            })
            .collect(),
    );
}

pub(super) fn broadcast_light_updates(
    state: &InteractionState,
    updates: &[OutboundLightUpdate],
    except: Option<SessionId>,
) {
    broadcast_light_updates_to_sessions(&state.sessions, updates, except);
}

pub(super) fn broadcast_light_updates_to_sessions(
    sessions: &SessionRegistry,
    updates: &[OutboundLightUpdate],
    except: Option<SessionId>,
) {
    if updates.is_empty() {
        return;
    }
    let chunks: HashSet<_> = updates
        .iter()
        .map(|update| (update.pos.x, update.pos.z))
        .collect();
    dispatch_visibility_commands(
        sessions
            .loaded_recipients_for_chunks(&chunks, except)
            .into_iter()
            .map(|recipient| VisibilityDispatch {
                recipient,
                command: OutboundCommand::LightUpdates(updates.to_vec()),
            })
            .collect(),
    );
}

pub(super) fn plan_block_delta_packets(deltas: &[BlockDelta]) -> Vec<BlockDeltaPacket> {
    if deltas.len() <= 1 {
        return deltas
            .iter()
            .copied()
            .map(BlockDeltaPacket::Single)
            .collect();
    }

    let mut by_section: BTreeMap<(i32, i32, i32), Vec<BlockDelta>> = BTreeMap::new();
    for &delta in deltas {
        by_section
            .entry((
                delta.x.div_euclid(16),
                delta.y.div_euclid(16),
                delta.z.div_euclid(16),
            ))
            .or_default()
            .push(delta);
    }

    let mut packets = Vec::new();
    for ((section_x, section_y, section_z), changes) in by_section {
        if changes.len() == 1 {
            packets.push(BlockDeltaPacket::Single(changes[0]));
        } else {
            packets.push(BlockDeltaPacket::Section {
                section_x,
                section_y,
                section_z,
                changes,
            });
        }
    }
    packets
}
