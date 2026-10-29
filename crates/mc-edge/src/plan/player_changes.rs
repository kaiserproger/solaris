//! One connection's reliable core outbox -> packets.
//!
//! The core addresses every outbox queue by connection, so each change
//! planned here belongs to the viewer it is planned for. Changes are planned
//! in queue order: `Joined` adopts the core's entity id and a respawn or
//! dimension change resets the cached world view before later changes.

use mc_core::boundary::{BossBarChange, PlayerAttribute, PlayerChange, TitleChange};
use mc_data::item_stack::ItemStack;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{
    AttributeId, AttributeModifierOperation, BlockChangedAck, BlockUpdate, BossOperation,
    ClientboundAwardStats, ClientboundBlockEntityData, ClientboundBlockEvent, ClientboundBossEvent,
    ClientboundChangeDifficulty, ClientboundClearDialog, ClientboundClearTitles,
    ClientboundContainerClose, ClientboundContainerSetContent, ClientboundContainerSetData,
    ClientboundContainerSetSlot, ClientboundCooldown, ClientboundCustomPayload,
    ClientboundMerchantOffers, ClientboundMoveVehicle, ClientboundOpenScreen,
    ClientboundOpenSignEditor, ClientboundPlayerAbilities, ClientboundPlayerCombatKill,
    ClientboundRecipeBookAdd, ClientboundRemoveEntityEffect, ClientboundRespawn,
    ClientboundSelectAdvancementsTab, ClientboundSetActionBarText, ClientboundSetCursorItem,
    ClientboundSetEntityData, ClientboundSetExperience, ClientboundSetHealth,
    ClientboundSetSubtitleText, ClientboundSetTitleText, ClientboundSetTitlesAnimation,
    ClientboundShowDialog, ClientboundSound, ClientboundSystemChat, ClientboundTickingState,
    ClientboundTickingStep, ClientboundUpdateEntityAttributes, ClientboundUpdateEntityEffect,
    ENTITY_DATA_AIR_SUPPLY_INDEX, EntityAttributeModifier, EntityAttributeSnapshot,
    EntityDataValue as WireEntityDataValue, EntityEffectFlags, EntityEvent as WireEntityEvent,
    GameEvent, LevelEvent, MerchantItemCost, MerchantOffer, MobEffectId,
    PLAYER_DATA_ABSORPTION_INDEX, PlayDisconnect, PlayerInfoActions, PlayerInfoEntry,
    PlayerInfoRemove, PlayerInfoUpdate, RemoveEntities, SetEntityMotion, SynchronizePlayerPosition,
    pack_block_pos,
};

use super::{PlannedPacket, ViewerState, per_tick, push_reliable};

/// Plan one connection's drained outbox. Teleport ids come from the core's
/// connection-local movement fence, so reliable replay and independently
/// planned batches cannot reuse an id.
pub fn plan_player_changes(
    changes: &[PlayerChange],
    viewer: &mut ViewerState,
) -> Vec<PlannedPacket> {
    let mut out = Vec::new();
    for change in changes {
        plan_change(change, viewer, &mut out);
    }
    out
}

fn plan_change(change: &PlayerChange, viewer: &mut ViewerState, out: &mut Vec<PlannedPacket>) {
    let conn = viewer.connection;
    match change {
        PlayerChange::Map { update } => push_reliable(
            out,
            conn,
            &mc_protocol::packets::play::ClientboundMapData {
                id: update.id,
                scale: update.scale,
                locked: update.locked,
                decorations: Some(
                    update
                        .markers
                        .iter()
                        .map(|d| mc_protocol::packets::play::MapDecoration {
                            kind: d.kind,
                            x: d.x,
                            z: d.z,
                            rotation: d.rotation,
                            name: d
                                .name
                                .as_ref()
                                .map(|name| mc_core::scoreboard::component(name)),
                        })
                        .collect(),
                ),
                rectangle: update.rectangle,
                colors: update.colors.clone(),
            },
        ),
        PlayerChange::Statistics { stats, .. } => push_reliable(
            out,
            conn,
            &ClientboundAwardStats {
                stats: stats.clone(),
            },
        ),
        PlayerChange::Advancements {
            reset,
            definitions,
            progress,
            ..
        } => push_reliable(
            out,
            conn,
            &viewer.advancements.update(*reset, definitions, progress),
        ),
        PlayerChange::AdvancementsTab { tab, .. } => push_reliable(
            out,
            conn,
            &ClientboundSelectAdvancementsTab {
                tab: tab.as_ref().and_then(|id| Identifier::parse(id).ok()),
            },
        ),
        PlayerChange::RecipesUnlocked {
            recipes, replace, ..
        } => {
            let entries = recipes
                .iter()
                .filter_map(|id| {
                    let mut entry = viewer.advancements.recipes.get(id)?.clone();
                    if *replace {
                        // Joining never replays the unlock toast.
                        entry.flags = 0;
                    }
                    Some(entry)
                })
                .collect();
            push_reliable(
                out,
                conn,
                &ClientboundRecipeBookAdd {
                    entries,
                    replace: *replace,
                },
            );
        }
        PlayerChange::VehicleCorrected {
            position,
            yaw,
            pitch,
            ..
        } => push_reliable(
            out,
            conn,
            &ClientboundMoveVehicle {
                x: position[0],
                y: position[1],
                z: position[2],
                yaw: *yaw,
                pitch: *pitch,
            },
        ),
        PlayerChange::ProjectileStatus { entity, event, .. } => push_reliable(
            out,
            conn,
            &WireEntityEvent {
                entity_id: entity.0,
                event_id: *event,
            },
        ),
        PlayerChange::ProjectileLevelEvent {
            position,
            event,
            data,
            ..
        } => push_reliable(
            out,
            conn,
            &LevelEvent {
                event_id: *event,
                position: pack_block_pos(position.x, position.y, position.z),
                data: *data,
                global: false,
            },
        ),
        PlayerChange::MobEffect { effect, blend, .. } => push_reliable(
            out,
            conn,
            &ClientboundUpdateEntityEffect {
                entity_id: viewer.entity_id.0,
                effect_id: MobEffectId::new(effect.id as u32)
                    .expect("built-in mob-effect registry id"),
                amplifier: i32::from(effect.amplifier),
                duration_ticks: effect.duration,
                flags: EntityEffectFlags {
                    ambient: effect.ambient,
                    visible: effect.show_particles,
                    show_icon: effect.show_icon,
                    blend: *blend,
                },
            },
        ),
        PlayerChange::RemoveMobEffect { id, .. } => push_reliable(
            out,
            conn,
            &ClientboundRemoveEntityEffect {
                entity_id: viewer.entity_id.0,
                effect_id: MobEffectId::new(*id as u32).expect("built-in mob-effect registry id"),
            },
        ),
        PlayerChange::Absorption { amount, .. } => push_reliable(
            out,
            conn,
            &ClientboundSetEntityData {
                entity_id: viewer.entity_id.0,
                values: vec![WireEntityDataValue::Float {
                    index: PLAYER_DATA_ABSORPTION_INDEX,
                    value: *amount,
                }],
            },
        ),
        PlayerChange::AirSupply { air } => push_reliable(
            out,
            conn,
            &ClientboundSetEntityData {
                entity_id: viewer.entity_id.0,
                values: vec![WireEntityDataValue::Int {
                    index: ENTITY_DATA_AIR_SUPPLY_INDEX,
                    value: *air,
                }],
            },
        ),
        PlayerChange::Scoreboard { event, .. } => push_scoreboard_event(out, conn, event.clone()),
        PlayerChange::Roster {
            players, removed, ..
        } => {
            if !removed.is_empty() {
                viewer
                    .player_profiles
                    .retain(|_, uuid| !removed.contains(uuid));
            }
            for player in players.iter() {
                let previous = viewer.player_profiles.insert(player.entity, player.uuid);
                if previous != Some(player.uuid)
                    && player.entity != viewer.entity_id
                    && viewer.tracked_entities.remove(&player.entity).is_some()
                {
                    // A provisional auth body was not a trusted player profile.
                    // Remove its old client object before respawning with the
                    // verified UUID on the next world publication.
                    push_reliable(
                        out,
                        conn,
                        &RemoveEntities {
                            entity_ids: vec![player.entity.0],
                        },
                    );
                    viewer.entity_interest.removed(player.entity);
                }
            }
            if !removed.is_empty() {
                push_reliable(
                    out,
                    conn,
                    &PlayerInfoRemove {
                        profile_ids: removed.to_vec(),
                    },
                );
            }
            if !players.is_empty() {
                push_reliable(
                    out,
                    conn,
                    &PlayerInfoUpdate {
                        actions: PlayerInfoActions::minimal_add_player(),
                        entries: players
                            .iter()
                            .map(|player| PlayerInfoEntry {
                                profile_id: player.uuid,
                                name: player.name.clone(),
                                properties: Vec::new(),
                                listed: true,
                                latency: 0,
                                game_mode: i32::from(player.game_mode),
                                list_order: 0,
                                show_hat: true,
                            })
                            .collect(),
                    },
                );
            }
        }
        PlayerChange::StyledChat { content, .. } => {
            let mut content_nbt = Vec::new();
            if mc_nbt::write_network(&mut content_nbt, content).is_ok() {
                push_reliable(
                    out,
                    conn,
                    &ClientboundSystemChat {
                        content_nbt,
                        overlay: false,
                    },
                );
            }
        }
        PlayerChange::PluginClientSound { sound, .. } => push_reliable(
            out,
            conn,
            &ClientboundCustomPayload {
                payload: mc_protocol::packets::CustomPayload::Unknown {
                    channel: mc_loader::loader_sound_channel().clone(),
                    payload: mc_loader::sound_payload(sound),
                },
            },
        ),
        PlayerChange::PluginClientView { payload } => push_reliable(
            out,
            conn,
            &ClientboundCustomPayload {
                payload: mc_protocol::packets::CustomPayload::Unknown {
                    channel: mc_loader::loader_view_channel().clone(),
                    payload: payload.clone(),
                },
            },
        ),
        PlayerChange::ShieldCooldown { ticks, .. } => push_reliable(
            out,
            conn,
            &ClientboundCooldown {
                cooldown_group: Identifier::parse("minecraft:shield").expect("shield identifier"),
                duration: *ticks,
            },
        ),
        PlayerChange::ItemCooldown { group, ticks, .. } => {
            if let Ok(cooldown_group) = Identifier::parse(group) {
                push_reliable(
                    out,
                    conn,
                    &ClientboundCooldown {
                        cooldown_group,
                        duration: *ticks,
                    },
                );
            }
        }
        PlayerChange::Attributes { attributes, .. } => push_reliable(
            out,
            conn,
            &ClientboundUpdateEntityAttributes {
                entity_id: viewer.entity_id.0,
                attributes: attributes.iter().filter_map(attribute_snapshot).collect(),
            },
        ),
        PlayerChange::Joined {
            entity,
            dimension,
            position,
            yaw,
            pitch,
            ..
        } => {
            // The pump created this viewer with an id invented from the
            // connection; the core's entity id is authoritative.
            viewer.entity_id = *entity;
            viewer.dimension = *dimension;
            // The initial join burst uses the connection's first teleport id.
            push_reliable(out, conn, &position_sync(1, *position, *yaw, *pitch));
            // Seed inventory/health surfaces; authoritative values arrive
            // via MenuContents/Health changes. Window 0 (player inventory)
            // is always 46 slots on the wire.
            push_reliable(
                out,
                conn,
                &ClientboundContainerSetContent {
                    container_id: 0,
                    state_id: 1,
                    items: vec![ItemStack::EMPTY; 46],
                    carried_item: ItemStack::EMPTY,
                },
            );
            push_reliable(
                out,
                conn,
                &ClientboundSetHealth {
                    health: 20.0,
                    food: 20,
                    saturation: 5.0,
                },
            );
            push_reliable(
                out,
                conn,
                &ClientboundSetExperience {
                    experience_progress: 0.0,
                    total_experience: 0,
                    experience_level: 0,
                },
            );
        }
        PlayerChange::PositionCorrected {
            teleport_id,
            position,
            yaw,
            pitch,
            ..
        } => push_reliable(
            out,
            conn,
            &position_sync(*teleport_id, *position, *yaw, *pitch),
        ),
        PlayerChange::Motion {
            entity, velocity, ..
        } => push_reliable(
            out,
            conn,
            &SetEntityMotion {
                entity_id: entity.0,
                movement: per_tick(*velocity),
            },
        ),
        PlayerChange::BlockResynced {
            position, block, ..
        } => push_reliable(
            out,
            conn,
            &BlockUpdate {
                position: pack_block_pos(position.x, position.y, position.z),
                state_id: *block as i32,
            },
        ),
        PlayerChange::BlockActionsAcked { sequence, .. } => push_reliable(
            out,
            conn,
            &BlockChangedAck {
                sequence: *sequence,
            },
        ),
        PlayerChange::BookOpened { hand } => push_reliable(
            out,
            conn,
            &mc_protocol::packets::play::ClientboundOpenBook {
                hand: match hand {
                    mc_core::boundary::Hand::Main => {
                        mc_protocol::packets::play::InteractionHand::MainHand
                    }
                    mc_core::boundary::Hand::Off => {
                        mc_protocol::packets::play::InteractionHand::OffHand
                    }
                },
            },
        ),
        PlayerChange::SignEditorOpened {
            position,
            front,
            block,
            ..
        } => {
            let position = pack_block_pos(position.x, position.y, position.z);
            // The predicted sign may have been rolled back by an ack:
            // install its block before vanilla looks up its entity.
            push_reliable(
                out,
                conn,
                &BlockUpdate {
                    position,
                    state_id: *block as i32,
                },
            );
            push_reliable(
                out,
                conn,
                &ClientboundOpenSignEditor {
                    position,
                    is_front_text: *front,
                },
            );
        }
        PlayerChange::BlockEntityData {
            position,
            block_entity_type,
            nbt,
            ..
        } => {
            push_reliable(
                out,
                conn,
                &ClientboundBlockEntityData {
                    position: pack_block_pos(position.x, position.y, position.z),
                    block_entity_type: *block_entity_type as i32,
                    nbt: nbt.clone(),
                },
            );
            let chunk = (position.x.div_euclid(16), position.z.div_euclid(16));
            if viewer.loaded_chunks.contains_key(&chunk) {
                viewer.block_entity_updates.insert(chunk);
            }
        }
        PlayerChange::BlockEvent {
            position,
            block,
            state,
            action,
            parameter,
            ..
        } => {
            let position = pack_block_pos(position.x, position.y, position.z);
            push_reliable(
                out,
                conn,
                &BlockUpdate {
                    position,
                    state_id: *state as i32,
                },
            );
            push_reliable(
                out,
                conn,
                &ClientboundBlockEvent {
                    position,
                    action: *action,
                    parameter: *parameter,
                    block_type: *block as i32,
                },
            );
        }
        PlayerChange::BellRung { position } => push_reliable(
            out,
            conn,
            &ClientboundSound {
                sound: Identifier::parse("minecraft:block.bell.use")
                    .expect("static bell sound identifier"),
                fixed_range: None,
                source: 4,
                position: [position.x * 8 + 4, position.y * 8 + 4, position.z * 8 + 4],
                volume: 2.0,
                pitch: 1.0,
                seed: 0,
            },
        ),
        PlayerChange::MenuOpened {
            window,
            menu_type,
            title,
            ..
        } => push_reliable(
            out,
            conn,
            &ClientboundOpenScreen {
                container_id: i32::from(*window),
                menu_type: *menu_type as i32,
                title_nbt: menu_title_nbt(title).unwrap_or_default(),
            },
        ),
        PlayerChange::MountMenuOpened {
            window,
            columns,
            entity,
        } => push_reliable(
            out,
            conn,
            &mc_protocol::packets::play::ClientboundMountScreenOpen {
                container_id: i32::from(*window),
                columns: i32::from(*columns),
                entity_id: entity.0,
            },
        ),
        PlayerChange::MerchantOffers {
            window,
            offers,
            level,
            xp,
            can_restock,
            ..
        } => {
            let cost = |stack: &ItemStack| MerchantItemCost {
                item_id: stack.item_id,
                count: stack.count,
            };
            push_reliable(
                out,
                conn,
                &ClientboundMerchantOffers {
                    container_id: i32::from(*window),
                    offers: offers
                        .iter()
                        .map(|offer| MerchantOffer {
                            cost_a: cost(&offer.cost_a),
                            result: offer.result.clone(),
                            cost_b: offer.cost_b.as_ref().map(cost),
                            out_of_stock: offer.uses >= offer.max_uses,
                            uses: offer.uses,
                            max_uses: offer.max_uses,
                            xp: offer.xp,
                            special_price: offer.special_price,
                            price_multiplier: offer.price_multiplier,
                            demand: offer.demand,
                        })
                        .collect(),
                    villager_level: i32::from(*level),
                    villager_xp: *xp,
                    show_progress: true,
                    can_restock: *can_restock,
                },
            );
        }
        PlayerChange::ShowDialog { dialog } => push_reliable(
            out,
            conn,
            &ClientboundShowDialog {
                dialog: dialog.clone(),
            },
        ),
        PlayerChange::ClearDialog => push_reliable(out, conn, &ClientboundClearDialog),
        PlayerChange::MenuClosed { window, .. } => push_reliable(
            out,
            conn,
            &ClientboundContainerClose {
                container_id: i32::from(*window),
            },
        ),
        PlayerChange::MenuContents {
            window,
            state_id,
            slots,
            cursor,
            ..
        } => {
            let mut items: Vec<ItemStack> = slots.iter().map(stack_or_empty).collect();
            // Window 0 is always 46 slots on the wire; pad short core
            // inventories rather than sending a rejected packet.
            if *window == 0 {
                items.resize(46, ItemStack::EMPTY);
            }
            push_reliable(
                out,
                conn,
                &ClientboundContainerSetContent {
                    container_id: i32::from(*window),
                    state_id: *state_id,
                    items,
                    carried_item: stack_or_empty(cursor),
                },
            );
        }
        PlayerChange::MenuSlot {
            window,
            state_id,
            slot,
            item,
            ..
        } => push_reliable(
            out,
            conn,
            &ClientboundContainerSetSlot {
                container_id: i32::from(*window),
                state_id: *state_id,
                slot: *slot,
                item_stack: stack_or_empty(item),
            },
        ),
        PlayerChange::Cursor { item, .. } => push_reliable(
            out,
            conn,
            &ClientboundSetCursorItem {
                item_stack: stack_or_empty(item),
            },
        ),
        PlayerChange::HeldSlot { slot } => push_reliable(
            out,
            conn,
            &mc_protocol::packets::play::ClientboundSetHeldSlot {
                slot: i32::from(*slot),
            },
        ),
        PlayerChange::MenuProperty {
            window,
            property,
            value,
            ..
        } => push_reliable(
            out,
            conn,
            &ClientboundContainerSetData {
                container_id: i32::from(*window),
                id: *property,
                value: *value,
            },
        ),
        PlayerChange::Experience {
            progress,
            level,
            total,
            ..
        } => push_reliable(
            out,
            conn,
            &ClientboundSetExperience {
                experience_progress: *progress,
                total_experience: *total,
                experience_level: *level,
            },
        ),
        PlayerChange::Died { message } => {
            let mut message_nbt = Vec::new();
            if mc_nbt::write_network(&mut message_nbt, message).is_ok() {
                push_reliable(
                    out,
                    conn,
                    &ClientboundPlayerCombatKill {
                        player_id: viewer.entity_id.0,
                        message_nbt,
                    },
                );
            }
        }
        PlayerChange::WeatherChanged {
            raining,
            rain_level,
            thunder_level,
            ..
        } => viewer
            .weather_baseline
            .update(conn, *raining, *rain_level, *thunder_level, out),
        PlayerChange::Respawned {
            teleport_id,
            dimension,
            position,
            yaw,
            pitch,
            game_mode,
            ..
        } => {
            let respawn = Respawn {
                teleport_id: *teleport_id,
                dimension: *dimension,
                position: *position,
                yaw: *yaw,
                pitch: *pitch,
                game_mode: *game_mode,
                portal_cooldown: 0,
                travel: false,
            };
            plan_respawn(&respawn, viewer, out);
        }
        PlayerChange::DimensionChanged {
            teleport_id,
            dimension,
            position,
            yaw,
            pitch,
            game_mode,
            portal_cooldown,
            ..
        } => {
            let respawn = Respawn {
                teleport_id: *teleport_id,
                dimension: *dimension,
                position: *position,
                yaw: *yaw,
                pitch: *pitch,
                game_mode: *game_mode,
                portal_cooldown: i32::from(*portal_cooldown),
                travel: true,
            };
            plan_respawn(&respawn, viewer, out);
        }
        PlayerChange::GameModeChanged { game_mode, .. } => {
            push_reliable(
                out,
                conn,
                &GameEvent {
                    event: GameEvent::EVENT_CHANGE_GAME_MODE,
                    value: f32::from(*game_mode),
                },
            );
            push_reliable(out, conn, &abilities_for_mode(*game_mode));
        }
        PlayerChange::DifficultyChanged { difficulty, .. } => push_reliable(
            out,
            conn,
            &ClientboundChangeDifficulty {
                difficulty: *difficulty,
                locked: false,
            },
        ),
        PlayerChange::Health {
            health,
            food,
            saturation,
            ..
        } => push_reliable(
            out,
            conn,
            &ClientboundSetHealth {
                health: *health,
                food: i32::from(*food),
                saturation: *saturation,
            },
        ),
        PlayerChange::Chat { message, .. } => push_chat(out, conn, message),
        // Core-overflow kick: the pump sends this packet, flushes, and closes
        // the connection (see `route_outbox`). Reliable, so it jumps the
        // queue ahead of world traffic.
        PlayerChange::Disconnect { reason, .. } => push_reliable(
            out,
            conn,
            &PlayDisconnect {
                reason_nbt: text_component_nbt(reason).unwrap_or_default(),
            },
        ),
        PlayerChange::Title(change) => match change {
            TitleChange::Title(content) => push_reliable(
                out,
                conn,
                &ClientboundSetTitleText {
                    content: content.clone(),
                },
            ),
            TitleChange::Subtitle(content) => push_reliable(
                out,
                conn,
                &ClientboundSetSubtitleText {
                    content: content.clone(),
                },
            ),
            TitleChange::ActionBar(content) => push_reliable(
                out,
                conn,
                &ClientboundSetActionBarText {
                    content: content.clone(),
                },
            ),
            TitleChange::Times {
                fade_in,
                stay,
                fade_out,
            } => push_reliable(
                out,
                conn,
                &ClientboundSetTitlesAnimation {
                    fade_in: *fade_in,
                    stay: *stay,
                    fade_out: *fade_out,
                },
            ),
            TitleChange::Clear { reset_times } => push_reliable(
                out,
                conn,
                &ClientboundClearTitles {
                    reset_times: *reset_times,
                },
            ),
        },
        PlayerChange::TickingState { rate, frozen } => push_reliable(
            out,
            conn,
            &ClientboundTickingState {
                tick_rate: *rate,
                frozen: *frozen,
            },
        ),
        PlayerChange::TickingStep { steps } => {
            push_reliable(out, conn, &ClientboundTickingStep { tick_steps: *steps })
        }
        PlayerChange::BossBar { id, change } => push_reliable(
            out,
            conn,
            &ClientboundBossEvent {
                uuid: *id,
                operation: match change {
                    BossBarChange::Add {
                        name,
                        progress,
                        color,
                        overlay,
                        flags,
                    } => BossOperation::Add {
                        name: name.clone(),
                        progress: *progress,
                        color: *color,
                        overlay: *overlay,
                        flags: *flags,
                    },
                    BossBarChange::Remove => BossOperation::Remove,
                    BossBarChange::Progress(value) => BossOperation::Progress(*value),
                    BossBarChange::Name(name) => BossOperation::Name(name.clone()),
                    BossBarChange::Style { color, overlay } => BossOperation::Style {
                        color: *color,
                        overlay: *overlay,
                    },
                    BossBarChange::Properties(flags) => BossOperation::Properties(*flags),
                },
            },
        ),
    }
}

/// A death respawn (`travel == false`) or a portal/command dimension change.
struct Respawn {
    teleport_id: i32,
    dimension: mc_core::dimensions::Dimension,
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
    game_mode: u8,
    portal_cooldown: i32,
    travel: bool,
}

fn plan_respawn(respawn: &Respawn, viewer: &mut ViewerState, out: &mut Vec<PlannedPacket>) {
    let conn = viewer.connection;
    if respawn.travel || viewer.dimension != respawn.dimension {
        // The client builds a new level: drop the whole cached view.
        viewer.dimension = respawn.dimension;
        viewer.loaded_chunks.clear();
        viewer.chunk_sync = None;
        viewer.chunk_repairs.clear();
        viewer.tracked_entities.clear();
        viewer.entity_tick = None;
        viewer.pending_blocks.clear();
        viewer.block_entity_updates.clear();
        viewer.light_baseline = Default::default();
        viewer.weather_baseline = Default::default();
        viewer.boss_baseline.clear(conn, out);
        viewer.border_baseline = None;
        viewer.center_sent = None;
        viewer.forget_all = false;
        crate::settlement_labels::forget_labels(viewer, out);
    } else {
        // Same-dimension death respawn: the client keeps its level, so the
        // next plan forgets everything held (see `ViewerState::forget_all`).
        viewer.forget_all = true;
        viewer.weather_baseline.reset();
    }
    viewer.clock_baseline = None;
    viewer.position = respawn.position;
    // Travel preserves player data; death never does, even when the
    // selected respawn point is in another dimension.
    push_reliable(
        out,
        conn,
        &ClientboundRespawn {
            dimension_type_id: viewer.dimension_type_ids[viewer.dimension as usize],
            dimension_name: Identifier::parse(viewer.dimension.name()).expect("static dimension"),
            hashed_seed: 0,
            game_mode: respawn.game_mode,
            previous_game_mode: -1,
            is_debug: false,
            is_flat: false,
            death_location: None,
            portal_cooldown: respawn.portal_cooldown,
            sea_level: respawn.dimension.sea_level(),
            data_to_keep: if respawn.travel { 3 } else { 0 },
        },
    );
    push_reliable(
        out,
        conn,
        &position_sync(
            respawn.teleport_id,
            respawn.position,
            respawn.yaw,
            respawn.pitch,
        ),
    );
    push_reliable(out, conn, &abilities_for_mode(respawn.game_mode));
    // The 26.1.2 client's Respawn opens LevelLoadingScreen waiting for the
    // server; only this event (vanilla `sendLevelInfo`) moves it on, with no
    // timeout.
    push_reliable(
        out,
        conn,
        &GameEvent {
            event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
            value: 0.0,
        },
    );
}

fn position_sync(
    teleport_id: i32,
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
) -> SynchronizePlayerPosition {
    SynchronizePlayerPosition {
        teleport_id,
        x: position[0],
        y: position[1],
        z: position[2],
        dx: 0.0,
        dy: 0.0,
        dz: 0.0,
        yaw,
        pitch,
        relative_flags: 0,
    }
}

fn stack_or_empty(stack: &Option<ItemStack>) -> ItemStack {
    stack.clone().unwrap_or(ItemStack::EMPTY)
}

/// Client-tracked attributes by `minecraft:attribute` registry order in 26.3.
fn attribute_snapshot(attribute: &PlayerAttribute) -> Option<EntityAttributeSnapshot> {
    let id = match attribute.kind {
        "armor" => 1,
        "armor_toughness" => 2,
        "attack_damage" => 3,
        "attack_speed" => 5,
        "block_interaction_range" => 8,
        "entity_interaction_range" => 13,
        "knockback_resistance" => 20,
        "max_health" => 23,
        "movement_efficiency" => 25,
        "movement_speed" => 26,
        "oxygen_bonus" => 28,
        "scale" => 30,
        "sneaking_speed" => 31,
        "step_height" => 33,
        "submerged_mining_speed" => 34,
        "water_movement_efficiency" => 37,
        _ => return None,
    };
    Some(EntityAttributeSnapshot {
        attribute_id: AttributeId::new(id).expect("vanilla attribute id"),
        base: attribute.base,
        modifiers: attribute
            .modifiers
            .iter()
            .filter_map(|modifier| {
                let operation = match modifier.operation.trim_start_matches("minecraft:") {
                    "add_value" => AttributeModifierOperation::AddValue,
                    "add_multiplied_base" => AttributeModifierOperation::AddMultipliedBase,
                    "add_multiplied_total" => AttributeModifierOperation::AddMultipliedTotal,
                    _ => return None,
                };
                Some(EntityAttributeModifier {
                    id: mc_data::Identifier::parse(&modifier.id).ok()?,
                    amount: modifier.amount,
                    operation,
                })
            })
            .collect(),
    })
}

fn abilities_for_mode(game_mode: u8) -> ClientboundPlayerAbilities {
    let (invulnerable, flying, can_fly, instabuild) = match game_mode {
        1 => (true, false, true, true),
        3 => (true, true, true, false),
        _ => (false, false, false, false),
    };
    ClientboundPlayerAbilities {
        invulnerable,
        flying,
        can_fly,
        instabuild,
        flying_speed: 0.05,
        walking_speed: 0.1,
    }
}

fn menu_title_nbt(title: &str) -> Result<Vec<u8>, mc_nbt::NbtError> {
    let mut out = Vec::new();
    mc_nbt::write_network_value(&mut out, &mc_core::scoreboard::component(title))?;
    Ok(out)
}

fn text_component_nbt(text: &str) -> Result<Vec<u8>, mc_nbt::NbtError> {
    let mut out = Vec::new();
    mc_nbt::write_network(
        &mut out,
        &mc_nbt::Tag::Compound(vec![(
            "text".to_owned(),
            mc_nbt::Tag::String(text.to_owned()),
        )]),
    )?;
    Ok(out)
}

fn push_chat(
    out: &mut Vec<PlannedPacket>,
    connection: mc_core::boundary::ConnectionId,
    message: &str,
) {
    let content_nbt =
        text_component_nbt(message).unwrap_or_else(|_| text_component_nbt("").unwrap_or_default());
    push_reliable(
        out,
        connection,
        &ClientboundSystemChat {
            content_nbt,
            overlay: false,
        },
    );
}

/// Translate a tick-owned scoreboard event for its addressed viewer.
fn push_scoreboard_event(
    out: &mut Vec<PlannedPacket>,
    connection: mc_core::boundary::ConnectionId,
    event: mc_core::scoreboard::ScoreboardEvent,
) {
    use mc_core::scoreboard::{ScoreboardEvent, component};
    use mc_protocol::packets::play::{
        ClientboundResetScore, ClientboundSetDisplayObjective, ClientboundSetObjective,
        ClientboundSetPlayerTeam, ClientboundSetScore, TeamParameters,
    };
    match event {
        ScoreboardEvent::Team {
            name,
            method,
            team,
            members,
        } => {
            let parameters = team.map(|team| TeamParameters {
                display_name: component(&team.display_name),
                options: u8::from(team.friendly_fire)
                    | (u8::from(team.see_friendly_invisibles) << 1),
                nametag_visibility: team.nametag_visibility,
                collision_rule: team.collision_rule,
                color: team.color,
                prefix: component(&team.prefix),
                suffix: component(&team.suffix),
            });
            push_reliable(
                out,
                connection,
                &ClientboundSetPlayerTeam {
                    name,
                    method,
                    parameters,
                    players: members,
                },
            );
        }
        ScoreboardEvent::Objective {
            name,
            method,
            objective,
        } => {
            let render_type = i32::from(objective.as_ref().is_some_and(|o| o.criteria == "health"));
            let display_name = objective.map(|o| component(&o.display_name));
            push_reliable(
                out,
                connection,
                &ClientboundSetObjective {
                    name,
                    method,
                    display_name,
                    render_type,
                },
            );
        }
        ScoreboardEvent::Score {
            owner,
            objective,
            score,
        } => {
            push_reliable(
                out,
                connection,
                &ClientboundSetScore {
                    owner,
                    objective,
                    score,
                },
            );
        }
        ScoreboardEvent::Reset { owner, objective } => {
            push_reliable(out, connection, &ClientboundResetScore { owner, objective });
        }
        ScoreboardEvent::Display { slot, objective } => {
            push_reliable(
                out,
                connection,
                &ClientboundSetDisplayObjective { slot, objective },
            );
        }
    }
}
