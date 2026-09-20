use super::super::*;

pub(crate) struct PlayerUseIngressContext<'a, W> {
    pub(in crate::play) writer: &'a mut W,
    pub(in crate::play) interaction: Option<&'a mut InteractionState>,
    pub(in crate::play) script_gameplay_events: Option<&'a ScriptGameplayEventPublisher>,
    pub(in crate::play) game_mode: GameMode,
    pub(in crate::play) survival_state: &'a mut SurvivalState,
    pub(in crate::play) xp_state: &'a mut XpState,
    pub(in crate::play) player_pose: PlayerPose,
    pub(in crate::play) respawn_pose: &'a mut PlayerPose,
    pub(in crate::play) client_loaded: bool,
}

pub(crate) fn is_serverbound_use_interaction_packet(id: i32) -> bool {
    matches!(
        id,
        ServerboundUseItemOn::ID
            | ServerboundUseItem::ID
            | ServerboundSignUpdate::ID
            | ServerboundAttack::ID
            | ServerboundInteract::ID
            | ServerboundSwing::ID
    )
}

pub(crate) async fn handle_serverbound_use_interaction<W>(
    context: PlayerUseIngressContext<'_, W>,
    frame: mc_protocol::RawFrame,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let PlayerUseIngressContext {
        writer,
        mut interaction,
        script_gameplay_events,
        game_mode,
        survival_state,
        xp_state,
        player_pose,
        respawn_pose,
        client_loaded,
    } = context;
    let mut body = frame.body;
    match frame.id {
        ServerboundUseItemOn::ID => {
            let use_on = ServerboundUseItemOn::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                Box::pin(handle_use_item_on(
                    state,
                    writer,
                    script_gameplay_events,
                    game_mode,
                    *survival_state,
                    xp_state,
                    player_pose,
                    respawn_pose,
                    use_on,
                ))
                .await?;
            } else {
                debug!(
                    sequence = use_on.sequence,
                    "UseItemOn ignored — no world configured"
                );
            }
        }
        ServerboundUseItem::ID => {
            let use_item = ServerboundUseItem::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                handle_use_item(
                    state,
                    writer,
                    script_gameplay_events,
                    game_mode,
                    survival_state,
                    player_pose,
                    use_item,
                )
                .await?;
            } else {
                debug!(
                    sequence = use_item.sequence,
                    "UseItem ignored — no world configured"
                );
            }
        }
        ServerboundSignUpdate::ID => {
            let sign_update = ServerboundSignUpdate::decode(&mut body)?;
            if let Some(state) = interaction.as_deref_mut() {
                handle_sign_update(state, writer, sign_update).await?;
            } else {
                debug!("SignUpdate ignored — no world configured");
            }
        }
        ServerboundAttack::ID => {
            let attack = ServerboundAttack::decode(&mut body)?;
            if !client_loaded {
                debug!(
                    entity_id = attack.entity_id,
                    "Attack ignored while client is loading"
                );
            } else if let Some(state) = interaction.as_deref_mut() {
                handle_attack(
                    state,
                    writer,
                    game_mode,
                    survival_state,
                    xp_state,
                    player_pose,
                    attack,
                )
                .await?;
            } else {
                debug!(
                    entity_id = attack.entity_id,
                    "Attack ignored — no world configured"
                );
            }
        }
        ServerboundInteract::ID => {
            let interact = ServerboundInteract::decode(&mut body)?;
            if !client_loaded {
                debug!(
                    entity_id = interact.entity_id,
                    "Interact ignored while client is loading"
                );
            } else if let Some(state) = interaction {
                handle_interact(state, writer, script_gameplay_events, interact).await?;
            } else {
                debug!(
                    entity_id = interact.entity_id,
                    "Interact ignored — no world configured"
                );
            }
        }
        ServerboundSwing::ID => {
            let _ = ServerboundSwing::decode(&mut body)?;
        }
        _ => unreachable!("use/interact helper only accepts use/interact packet ids"),
    }
    Ok(())
}
