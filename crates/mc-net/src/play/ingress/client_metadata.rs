use super::super::*;

pub(crate) struct ClientMetadataIngressContext<'a, W> {
    pub(in crate::play) writer: &'a mut W,
    pub(in crate::play) compression: Compression,
    pub(in crate::play) chunk_stream: &'a mut Option<ChunkStreamState>,
    pub(in crate::play) sessions: &'a SessionRegistry,
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) config: &'a ServerConfig,
    pub(in crate::play) server_view_distance: i32,
    pub(in crate::play) player_pose: PlayerPose,
    pub(in crate::play) effective_client_view_distance: &'a mut i32,
    pub(in crate::play) client_brand: &'a mut Option<String>,
    pub(in crate::play) client_preferences: &'a mut Option<ClientPreferences>,
    pub(in crate::play) scripts: Option<&'a ScriptEventSink>,
    pub(in crate::play) loader_eligible: bool,
    pub(in crate::play) client_load: &'a mut ClientLoadGate,
}

pub(crate) fn is_serverbound_client_metadata_packet(id: i32) -> bool {
    matches!(
        id,
        ServerboundRecipeBookChangeSettings::ID
            | ServerboundRecipeBookSeenRecipe::ID
            | ServerboundClientInformation::ID
            | ServerboundCustomPayload::ID
            | ServerboundResourcePack::ID
            | ServerboundChatAck::ID
            | ServerboundChunkBatchReceived::ID
            | ServerboundClientTickEnd::ID
            | ServerboundPlayerLoaded::ID
    )
}

pub(crate) async fn handle_serverbound_client_metadata<W>(
    context: ClientMetadataIngressContext<'_, W>,
    frame: mc_protocol::RawFrame,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let ClientMetadataIngressContext {
        writer,
        compression,
        chunk_stream,
        sessions,
        session_id,
        config,
        server_view_distance,
        player_pose,
        effective_client_view_distance,
        client_brand,
        client_preferences,
        scripts,
        loader_eligible,
        client_load,
    } = context;
    let mut body = frame.body;
    match frame.id {
        ServerboundRecipeBookChangeSettings::ID => {
            let settings = ServerboundRecipeBookChangeSettings::decode(&mut body)?;
            debug!(
                book_type = ?settings.book_type,
                open = settings.is_open,
                filtering = settings.is_filtering,
                "recipe book settings noted"
            );
        }
        ServerboundRecipeBookSeenRecipe::ID => {
            let seen = ServerboundRecipeBookSeenRecipe::decode(&mut body)?;
            debug!(
                recipe = seen.recipe_display_id,
                "recipe book seen recipe noted"
            );
        }
        ServerboundClientInformation::ID => {
            let information = ServerboundClientInformation::decode(&mut body)?.information;
            let preferences = ClientPreferences::from_packet(
                information,
                server_view_distance,
                client_brand.clone(),
            );
            debug!(
                language = %preferences.language,
                requested_view_distance = preferences.requested_view_distance,
                clamped_view_distance = preferences.clamped_view_distance,
                chat_visibility = ?preferences.chat_visibility,
                chat_colors = preferences.chat_colors,
                model_customisation = preferences.model_customisation,
                main_hand = ?preferences.main_hand,
                text_filtering_enabled = preferences.text_filtering_enabled,
                allows_listing = preferences.allows_listing,
                particle_status = ?preferences.particle_status,
                brand = ?preferences.brand,
                "client information updated"
            );
            if preferences.clamped_view_distance != *effective_client_view_distance {
                *effective_client_view_distance = preferences.clamped_view_distance;
                if let Some(stream) = chunk_stream.as_mut() {
                    let unloads = stream
                        .replan_view_distance(*effective_client_view_distance, player_pose.yaw);
                    for (chunk_x, chunk_z) in unloads {
                        write_packet(writer, &ForgetLevelChunk { chunk_x, chunk_z }, compression)
                            .await?;
                        stream.log_chunk_unload(chunk_x, chunk_z, "client_information");
                    }
                }
            }
            *client_preferences = Some(preferences);
        }
        ServerboundCustomPayload::ID => match classify_play_custom_payload(body)? {
            PlayCustomPayloadAction::Brand(brand) => {
                debug!(brand = %brand, "client brand noted");
                if let Some(preferences) = client_preferences.as_mut() {
                    preferences.brand = Some(brand.clone());
                }
                if let Some(scripts) = scripts {
                    match ScriptEvent::client_brand(ScriptPlayerId::new(session_id), &brand) {
                        Ok(event) => scripts.enqueue_event(event),
                        Err(error) => debug!(?error, "client brand event rejected"),
                    }
                }
                *client_brand = Some(brand);
            }
            PlayCustomPayloadAction::LoaderView(payload) => {
                if let Err(error) = session::route_client_loader_view_request(
                    scripts,
                    sessions,
                    session_id,
                    loader_eligible,
                    config.loader_manifest.as_deref(),
                    payload.as_ref(),
                )
                .await
                {
                    debug!(
                        ?error,
                        player_id = session_id,
                        "Loader view request rejected"
                    );
                }
            }
            PlayCustomPayloadAction::Unknown { channel, payload } => {
                if let Some(scripts) = scripts
                    && scripts.boundary().allows_custom_payload(&channel)
                {
                    scripts.enqueue_custom_payload(
                        ScriptPlayerId::new(session_id),
                        ScriptProtocolPhase::Play,
                        &channel,
                        payload.to_vec(),
                    );
                } else {
                    debug!(channel = %channel, len = payload.len(), "custom payload ignored");
                }
            }
            PlayCustomPayloadAction::Oversized { len } => {
                warn!(
                    len,
                    max = MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES,
                    "oversized custom payload rejected before decode"
                );
            }
        },
        ServerboundResourcePack::ID => {
            let status = ServerboundResourcePack::decode(&mut body)?.status;
            debug!(
                id = %status.id,
                action = ?status.action,
                terminal = status.action.is_terminal(),
                "resource-pack status noted"
            );
        }
        ServerboundChatAck::ID => {
            let ack = ServerboundChatAck::decode(&mut body)?;
            debug!(offset = ack.offset, "chat acknowledgement ignored");
        }
        ServerboundChunkBatchReceived::ID => {
            let packet = ServerboundChunkBatchReceived::decode(&mut body)?;
            debug!(
                desired_chunks_per_tick = packet.desired_chunks_per_tick,
                "client chunk-batch preference noted"
            );
        }
        ServerboundClientTickEnd::ID => {
            let _ = ServerboundClientTickEnd::decode(&mut body)?;
        }
        ServerboundPlayerLoaded::ID => {
            let _ = ServerboundPlayerLoaded::decode(&mut body)?;
            client_load.acknowledge();
            let completed_respawn_load = sessions.mark_client_loaded(session_id);
            debug!(completed_respawn_load, "client reported player loaded");
        }
        _ => unreachable!("client metadata helper only accepts client metadata packet ids"),
    }
    Ok(())
}
