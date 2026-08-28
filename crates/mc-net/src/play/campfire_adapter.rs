use std::collections::HashSet;

use mc_data::{ItemStack, items::ItemRegistry};
use mc_domain::{GameMode, InteractionHand};
use mc_protocol::packets::play::{ClientboundBlockEntityData, pack_block_pos};
use mc_world::SECTION_DIM;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

use crate::connection::write_packet;
use crate::error::ConnectionError;
use crate::server::ServerConfig;

#[cfg(test)]
use super::block_edit_commit::apply_opaque_block_entity_to_storage_conditionally;
use super::block_edit_commit::send_loaded_block_edit_resyncs;
#[cfg(test)]
use super::campfire::campfire_cooking_states_from_chunk;
use super::campfire::{
    CampfireCookingState, campfire_block_entity_id, campfire_block_entity_persistent_bytes,
    campfire_block_entity_update_nbt, campfire_cooking_states_from_chunk_strict,
    campfire_recipe_result_stack, is_campfire_block, is_lit_campfire_block,
};
use super::containers::find_campfire_recipe_for_item;
use super::session::{SessionId, SessionRegistry, dispatch_visibility_commands};
use super::simulation::{self, CampfireUsePlan};
use super::{
    BlockEdit, InteractionState, ResidentWorldJournalWave, hand_inventory_slot,
    item_entity_type_id, published_block_precondition, write_block_ack,
    write_inventory_slot_updates,
};

pub(in crate::play) const CAMPFIRE_BLOCK_ENTITY_TYPE_ID: i32 = 33;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::play) struct CommittedCampfireCookingTick {
    pub(in crate::play) cooking: CampfireCookingState,
    pub(in crate::play) completed: Vec<ItemStack>,
    pub(in crate::play) changed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CampfireCookingTickReport {
    pub(crate) persisted: usize,
    pub(crate) completed: usize,
    pub(crate) dropped: usize,
}

async fn send_campfire_block_entity_update<W>(
    state: &InteractionState,
    writer: &mut W,
    position: mc_world::BlockPos,
    cooking: &CampfireCookingState,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(nbt) = campfire_block_entity_update_nbt(&state.items, cooking) else {
        warn!(
            ?position,
            "campfire block entity update skipped for unknown item id"
        );
        return Ok(());
    };
    write_packet(
        writer,
        &ClientboundBlockEntityData {
            position: pack_block_pos(position.x, position.y, position.z),
            block_entity_type: CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
            nbt,
        },
        state.compression,
    )
    .await?;
    Ok(())
}

pub(in crate::play) fn dispatch_campfire_block_entity_update(
    items: &ItemRegistry,
    sessions: &SessionRegistry,
    except: Option<SessionId>,
    position: mc_world::BlockPos,
    cooking: &CampfireCookingState,
) {
    let Some(nbt) = campfire_block_entity_update_nbt(items, cooking) else {
        warn!(
            ?position,
            "campfire block entity update skipped for unknown item id"
        );
        return;
    };
    dispatch_visibility_commands(sessions.block_entity_data_dispatches(
        position,
        except,
        CAMPFIRE_BLOCK_ENTITY_TYPE_ID,
        nbt,
    ));
}

pub(in crate::play) async fn handle_campfire_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    _game_mode: GameMode,
    sequence: i32,
    position: mc_world::BlockPos,
    hand: InteractionHand,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(precondition) = published_block_precondition(state, position) else {
        return Ok(false);
    };
    let campfire_state = precondition.expected_state;
    let campfire_token = precondition.expected_token;
    let Some(block_entity_id) = campfire_block_entity_id(&state.blocks, campfire_state) else {
        return Ok(false);
    };

    let slot = hand_inventory_slot(state, hand);
    let held = state.inventory.slots[slot].clone();
    if held.is_empty() {
        return Ok(false);
    }
    let Some(recipe) = find_campfire_recipe_for_item(state, held.item_id) else {
        return Ok(false);
    };
    let Some(result) = campfire_recipe_result_stack(&state.items, &recipe) else {
        return Ok(false);
    };
    let cooking_time = match &recipe.kind {
        mc_data::recipes::RecipeKind::CampfireCooking(smelting) => smelting.cooking_time,
        _ => return Ok(false),
    };
    let input = ItemStack {
        count: 1,
        item_id: held.item_id,
        damage: held.damage,
        enchantments: held.enchantments.clone(),
        custom_name: None,
        item_model: None,
    };
    let expected = state.sessions.campfire_cooking_state(position);
    let mut cooking = expected.clone();
    if !cooking.insert(input.clone(), result.clone(), cooking_time) {
        write_block_ack(writer, state.compression, sequence).await?;
        return Ok(true);
    }
    let Some(persistent_bytes) =
        campfire_block_entity_persistent_bytes(block_entity_id, position, &state.items, &cooking)
    else {
        return Ok(false);
    };
    let Some(client_nbt) = campfire_block_entity_update_nbt(&state.items, &cooking) else {
        return Ok(false);
    };
    let committed = match commit_campfire_use(
        state,
        CampfireUsePlan {
            position,
            expected_state: campfire_state,
            expected_token: campfire_token,
            expected_cooking: expected,
            updated_cooking: cooking.clone(),
            persistent_bytes,
            client_nbt,
            held_slot: slot,
            expected_held: held,
        },
    )
    .await
    {
        Ok(Some(committed)) => committed,
        Ok(None) => {
            send_loaded_block_edit_resyncs(
                state,
                writer,
                &[BlockEdit {
                    pos: position,
                    new_state: campfire_state,
                }],
            )
            .await?;
            write_inventory_slot_updates(
                state,
                writer,
                vec![(slot, state.inventory.slots[slot].clone())],
            )
            .await?;
            write_block_ack(writer, state.compression, sequence).await?;
            return Ok(true);
        }
        Err(error) => {
            debug!(?error, ?position, "simulation campfire use rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "committing campfire use",
            });
        }
    };

    state.inventory = committed.inventory;
    write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
    send_campfire_block_entity_update(state, writer, position, &cooking).await?;
    write_block_ack(writer, state.compression, sequence).await?;
    Ok(true)
}

async fn commit_campfire_use(
    state: &InteractionState,
    plan: CampfireUsePlan,
) -> Result<Option<simulation::CommittedCampfireUse>, simulation::SimulationRequestError> {
    #[cfg(test)]
    {
        if state.inventory.slots[plan.held_slot] != plan.expected_held {
            return Ok(None);
        }
        let mut inventory = state.inventory.clone();
        let held = &mut inventory.slots[plan.held_slot];
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
        let changed_slots = vec![(plan.held_slot, held.clone())];
        let mut storage = state.world.lock().await;
        let committed = state
            .sessions
            .commit_campfire_cooking_legacy_for_test(
                plan.position,
                &plan.expected_cooking,
                plan.updated_cooking,
                || match apply_opaque_block_entity_to_storage_conditionally(
                    &mut storage,
                    plan.position,
                    plan.expected_state,
                    plan.expected_token,
                    plan.persistent_bytes,
                ) {
                    Ok(true) => Ok(()),
                    Ok(false) | Err(_) => Err(()),
                },
            )
            .unwrap_or(false);
        Ok(committed.then_some(simulation::CommittedCampfireUse {
            inventory,
            changed_slots,
        }))
    }

    #[cfg(not(test))]
    {
        state.simulation.commit_campfire_use(plan).await
    }
}

pub(in crate::play) async fn run_campfire_cooking_ticks_owned(
    simulation_owner: &simulation::SimulationOwner,
    config: &ServerConfig,
    sessions: &SessionRegistry,
    world_read: Option<&mc_world::WorldReadView>,
    world_mutation: Option<&mc_world::WorldMutationView>,
) -> CampfireCookingTickReport {
    let Some(world) = config.world.as_ref() else {
        return CampfireCookingTickReport::default();
    };
    let positions = sessions.campfire_cooking_positions();
    if positions.is_empty() {
        return CampfireCookingTickReport::default();
    }
    let Some(entity_type_id) = item_entity_type_id(&config.entity_types) else {
        warn!(
            count = positions.len(),
            "campfire cooking paused: item entity type unavailable"
        );
        return CampfireCookingTickReport::default();
    };
    let owned_access = if world_read.is_none() || world_mutation.is_none() {
        let storage = world.lock().await;
        Some((storage.read_view(), storage.mutation_view()))
    } else {
        None
    };
    let world_read = world_read
        .or_else(|| owned_access.as_ref().map(|(read, _)| read))
        .expect("campfire tick read view");
    let world_mutation = world_mutation
        .or_else(|| owned_access.as_ref().map(|(_, mutation)| mutation))
        .expect("campfire tick mutation view");

    let mut candidates = Vec::new();
    for position in positions {
        let Some((block_state, token)) = world_read.block_mutation_snapshot(position) else {
            continue;
        };
        if !is_campfire_block(&config.blocks, block_state) {
            continue;
        }
        let Some(block_entity_id) = campfire_block_entity_id(&config.blocks, block_state) else {
            continue;
        };
        candidates.push((
            position,
            block_state,
            token,
            block_entity_id,
            is_lit_campfire_block(&config.blocks, block_state),
        ));
    }
    if candidates.is_empty() {
        return CampfireCookingTickReport::default();
    }
    let mut wave = match ResidentWorldJournalWave::begin(sessions).await {
        Ok(wave) => wave,
        Err(()) => return CampfireCookingTickReport::default(),
    };
    let mut cooled = Vec::new();
    let mut ticks = Vec::new();
    let world_decision_id = wave.decision_id_or(sessions.simulation_tick());
    for &(position, block_state, token, block_entity_id, lit) in &candidates {
        if !lit {
            if sessions.cool_down_campfire_cooking_conditionally(position, |cooking| {
                let Some(bytes) = campfire_block_entity_persistent_bytes(
                    block_entity_id,
                    position,
                    &config.items,
                    cooking,
                ) else {
                    return false;
                };
                matches!(
                    wave.commit_opaque_block_entity(
                        world_mutation,
                        position,
                        block_state,
                        token,
                        bytes,
                    ),
                    mc_world::ResidentOpaqueBlockEntityCommitResult::Applied
                )
            }) {
                cooled.push(position);
            }
            continue;
        }
        let committed =
            sessions.tick_campfire_cooking_conditionally(position, world_decision_id, |cooking| {
                let Some(bytes) = campfire_block_entity_persistent_bytes(
                    block_entity_id,
                    position,
                    &config.items,
                    cooking,
                ) else {
                    return false;
                };
                matches!(
                    wave.commit_opaque_block_entity(
                        world_mutation,
                        position,
                        block_state,
                        token,
                        bytes,
                    ),
                    mc_world::ResidentOpaqueBlockEntityCommitResult::Applied
                )
            });
        let Some(committed) = committed else {
            continue;
        };
        ticks.push((position, committed));
    }
    if wave
        .finish(
            sessions,
            world_read,
            world_mutation,
            sessions.simulation_tick(),
        )
        .await
        .is_err()
    {
        return CampfireCookingTickReport::default();
    }
    #[cfg(test)]
    sessions.pause_after_campfire_d1_for_test().await;

    let mut report = CampfireCookingTickReport {
        persisted: cooled.len() + ticks.len(),
        ..CampfireCookingTickReport::default()
    };
    let mut invalidated_chunks = HashSet::new();
    for position in cooled {
        invalidated_chunks.insert((
            position.x.div_euclid(SECTION_DIM as i32),
            position.z.div_euclid(SECTION_DIM as i32),
        ));
    }
    for (position, committed) in &ticks {
        report.completed += committed.completed.len();
        invalidated_chunks.insert((
            position.x.div_euclid(SECTION_DIM as i32),
            position.z.div_euclid(SECTION_DIM as i32),
        ));
    }

    let mut materialized = Vec::new();
    for &(position, _, _, block_entity_id, _) in &candidates {
        let cooking = sessions.campfire_cooking_state(position);
        if cooking.pending_outputs.is_empty() {
            continue;
        }
        let snapshots = simulation_owner.materialize_pending_campfire_outputs(
            sessions,
            entity_type_id,
            position,
            &cooking.pending_outputs,
        );
        if snapshots.len() != cooking.pending_outputs.len() {
            warn!(
                ?position,
                expected = cooking.pending_outputs.len(),
                materialized = snapshots.len(),
                "campfire output materialization was incomplete"
            );
            continue;
        }
        materialized.push((
            position,
            block_entity_id,
            cooking.pending_outputs,
            snapshots,
        ));
    }
    if materialized.is_empty() {
        if !invalidated_chunks.is_empty() {
            sessions.invalidate_prepared_chunks(&invalidated_chunks);
        }
        return report;
    }
    #[cfg(test)]
    sessions.pause_after_campfire_entity_commit_for_test().await;

    let mut ack_wave = match ResidentWorldJournalWave::begin(sessions).await {
        Ok(wave) => wave,
        Err(()) => return report,
    };
    let mut acknowledged = Vec::new();
    for (position, block_entity_id, pending, snapshots) in materialized {
        let Some((block_state, token)) = world_read.block_mutation_snapshot(position) else {
            continue;
        };
        if campfire_block_entity_id(&config.blocks, block_state) != Some(block_entity_id) {
            continue;
        }
        let Some(cooking) = sessions.acknowledge_pending_campfire_outputs_conditionally(
            position,
            &pending,
            |cooking| {
                let Some(bytes) = campfire_block_entity_persistent_bytes(
                    block_entity_id,
                    position,
                    &config.items,
                    cooking,
                ) else {
                    return false;
                };
                matches!(
                    ack_wave.commit_opaque_block_entity(
                        world_mutation,
                        position,
                        block_state,
                        token,
                        bytes,
                    ),
                    mc_world::ResidentOpaqueBlockEntityCommitResult::Applied
                )
            },
        ) else {
            continue;
        };
        acknowledged.push((position, cooking, pending.len(), snapshots));
    }
    if ack_wave
        .finish(
            sessions,
            world_read,
            world_mutation,
            sessions.simulation_tick(),
        )
        .await
        .is_err()
    {
        return report;
    }

    for (position, cooking, output_count, snapshots) in acknowledged {
        report.dropped += output_count;
        dispatch_campfire_block_entity_update(&config.items, sessions, None, position, &cooking);
        dispatch_visibility_commands(
            simulation_owner.publish_materialized_campfire_outputs(sessions, &snapshots),
        );
    }
    if !invalidated_chunks.is_empty() {
        sessions.invalidate_prepared_chunks(&invalidated_chunks);
    }
    report
}

#[cfg(test)]
pub(crate) async fn hydrate_persisted_campfire_cooking(
    config: &ServerConfig,
    sessions: &SessionRegistry,
) -> usize {
    let Some(world) = config.world.as_ref() else {
        return 0;
    };
    let chunks = world.lock().await.resident_chunk_snapshots();
    let resident = chunks.len();
    let mut restored = 0usize;
    for (_, chunk) in chunks {
        for (position, cooking) in
            campfire_cooking_states_from_chunk(&chunk, &config.recipes, &config.items, &config.tags)
        {
            if sessions.restore_campfire_cooking(position, cooking) {
                restored += 1;
            }
        }
    }
    if restored > 0 {
        info!(resident, restored, "hydrated resident campfire cooking");
    }
    restored
}

pub(crate) async fn hydrate_persisted_campfire_cooking_strict(
    config: &ServerConfig,
    sessions: &SessionRegistry,
) -> Result<usize, String> {
    let Some(world) = config.world.as_ref() else {
        return Ok(0);
    };
    let chunks = world.lock().await.resident_chunk_snapshots();
    let resident = chunks.len();
    let mut restored = 0usize;
    for (_, chunk) in chunks {
        for (position, cooking) in campfire_cooking_states_from_chunk_strict(
            &chunk,
            &config.recipes,
            &config.items,
            &config.tags,
        )? {
            if sessions.restore_campfire_cooking(position, cooking) {
                restored += 1;
            }
        }
    }
    if restored > 0 {
        info!(resident, restored, "hydrated resident campfire cooking");
    }
    Ok(restored)
}

pub(crate) async fn recover_pending_campfire_outputs(
    config: &ServerConfig,
    sessions: &SessionRegistry,
    simulation_owner: &simulation::SimulationOwner,
) -> Result<usize, String> {
    let Some(world) = config.world.as_ref() else {
        return Ok(0);
    };
    let mut positions = sessions.campfire_cooking_positions();
    positions.sort_unstable_by_key(|position| (position.x, position.y, position.z));
    let pending = positions
        .into_iter()
        .filter_map(|position| {
            let cooking = sessions.campfire_cooking_state(position);
            (!cooking.pending_outputs.is_empty()).then_some((position, cooking))
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(0);
    }
    let Some(entity_type_id) = item_entity_type_id(&config.entity_types) else {
        return Err("campfire output recovery requires minecraft:item entity type".into());
    };
    let (world_read, world_mutation) = {
        let storage = world.lock().await;
        (storage.read_view(), storage.mutation_view())
    };
    let mut materialized = Vec::new();
    for (position, cooking) in pending {
        let Some((block_state, _)) = world_read.block_mutation_snapshot(position) else {
            return Err(format!(
                "pending campfire output at {position:?} has no resident block"
            ));
        };
        let Some(block_entity_id) = campfire_block_entity_id(&config.blocks, block_state) else {
            return Err(format!(
                "pending campfire output at {position:?} is not attached to a campfire"
            ));
        };
        let snapshots = simulation_owner.materialize_pending_campfire_outputs(
            sessions,
            entity_type_id,
            position,
            &cooking.pending_outputs,
        );
        if snapshots.len() != cooking.pending_outputs.len() {
            return Err(format!(
                "campfire output recovery at {position:?} materialized {} of {} entities",
                snapshots.len(),
                cooking.pending_outputs.len()
            ));
        }
        materialized.push((
            position,
            block_entity_id,
            cooking.pending_outputs,
            snapshots,
        ));
    }
    let mut ack_wave = ResidentWorldJournalWave::begin(sessions)
        .await
        .map_err(|()| "campfire output recovery could not reserve world ack".to_string())?;
    let mut acknowledged = Vec::new();
    for (position, block_entity_id, pending, snapshots) in materialized {
        let Some((block_state, token)) = world_read.block_mutation_snapshot(position) else {
            return Err(format!(
                "pending campfire output at {position:?} disappeared before ack"
            ));
        };
        let Some(cooking) = sessions.acknowledge_pending_campfire_outputs_conditionally(
            position,
            &pending,
            |cooking| {
                let Some(bytes) = campfire_block_entity_persistent_bytes(
                    block_entity_id,
                    position,
                    &config.items,
                    cooking,
                ) else {
                    return false;
                };
                matches!(
                    ack_wave.commit_opaque_block_entity(
                        &world_mutation,
                        position,
                        block_state,
                        token,
                        bytes,
                    ),
                    mc_world::ResidentOpaqueBlockEntityCommitResult::Applied
                )
            },
        ) else {
            return Err(format!(
                "pending campfire output at {position:?} changed before ack"
            ));
        };
        acknowledged.push((cooking, snapshots));
    }
    ack_wave
        .finish(
            sessions,
            &world_read,
            &world_mutation,
            sessions.simulation_tick(),
        )
        .await
        .map_err(|()| "campfire output recovery world ack failed".to_string())?;

    let recovered = acknowledged
        .iter()
        .map(|(_, snapshots)| snapshots.len())
        .sum();
    for (_, snapshots) in acknowledged {
        dispatch_visibility_commands(
            simulation_owner.publish_materialized_campfire_outputs(sessions, &snapshots),
        );
    }
    Ok(recovered)
}
