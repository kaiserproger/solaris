use super::{DamagePiece, TickState, spawn_item_entity};
use crate::boundary::{EntityEvent, EntityId, PlayerChange};
use crate::entities::{EntityKind, EntitySlot};
use mc_data::statistics::{CustomStat, StatType};
use mc_physics::mth;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[path = "totem.rs"]
mod totem;

impl TickState {
    pub(super) fn commit_damage(&mut self) -> (Vec<EntityId>, Vec<EntityEvent>, Vec<EntityId>) {
        let mut deaths = Vec::new();
        let mut events = Vec::new();
        let mut spawned = Vec::new();
        // Damage per slot, applied piece by piece (two attackers both
        // apply); players reduce each piece by its own kind (armor,
        // feather falling, none for void).
        // Keep each admitted source's original amount before land_hit mutates
        // it. Advancement dealt/taken evidence must not merge other hits.
        type AdmittedDamage = (DamagePiece, (f32, f32));
        let mut damage_by_slot: HashMap<EntitySlot, Vec<AdmittedDamage>> = HashMap::default();
        for (slot, piece) in std::mem::take(&mut self.merge_damage) {
            if crate::plugin_player::shields_damage(self, slot, piece.source) {
                crate::command_damage::report(self, None, &piece, piece.amount, false);
                continue;
            }
            damage_by_slot
                .entry(slot)
                .or_default()
                .push((piece, (piece.amount, 0.0)));
        }
        let mut damaged: Vec<_> = damage_by_slot.into_iter().collect();
        damaged.sort_by_key(|(slot, _)| *slot);
        // Resolve ordinary hits first: sweep damage and visuals exist only
        // when their primary hit lands, regardless of dense target-slot order.
        let mut successful_sweeps = HashSet::default();
        let mut projectile_followups = Vec::new();
        for (slot, pieces) in &mut damaged {
            if (slot.0 as usize) >= self.entities.len() {
                for (piece, (raw, _)) in pieces.iter() {
                    crate::command_damage::report(self, None, piece, *raw, false);
                }
                pieces.clear();
                continue;
            }
            pieces.retain_mut(|(piece, (raw, _))| {
                if matches!(
                    piece.source,
                    Some(crate::intent::DamageSource::Sweep { .. })
                ) {
                    return true;
                }
                let accepted = self.commit_damage_piece(*slot, piece, &mut events, &mut spawned);
                let target = self.entities.body(*slot).id;
                crate::command_damage::report(self, Some(target), piece, *raw, accepted);
                if !accepted {
                    return false;
                }
                if let Some(source) = piece.source {
                    let target = self.entities.body(*slot).id;
                    projectile_followups.extend(crate::projectiles::payloads::accepted_hit(
                        self,
                        target,
                        source,
                        &mut piece.amount,
                    ));
                }
                if let Some(crate::intent::DamageSource::Melee {
                    attacker,
                    position,
                    yaw,
                    sweep: true,
                    ..
                }) = piece.source
                {
                    successful_sweeps.insert((attacker, self.entities.body(*slot).id));
                    let height = self
                        .entities
                        .slot_of(attacker)
                        .map_or(1.8, |actor| self.entities.body(actor).height);
                    let angle = f64::from(yaw).to_radians();
                    events.push(EntityEvent::SweepAttack {
                        entity: attacker,
                        position: [
                            position[0] - f64::from(mth::sin(angle)),
                            position[1] + height * 0.5,
                            position[2] + f64::from(mth::cos(angle)),
                        ],
                        yaw,
                    });
                }
                true
            });
            for mut followup in projectile_followups.drain(..) {
                let raw = followup.amount;
                if self.commit_damage_piece(*slot, &mut followup, &mut events, &mut spawned) {
                    pieces.push((followup, (raw, 0.0)));
                }
            }
        }
        for (slot, pieces) in &mut damaged {
            pieces.retain_mut(|(piece, _)| {
                if let Some(crate::intent::DamageSource::Sweep {
                    attacker, primary, ..
                }) = piece.source
                {
                    successful_sweeps.contains(&(attacker, primary))
                        && self.commit_damage_piece(*slot, piece, &mut events, &mut spawned)
                } else {
                    true
                }
            });
        }
        for (slot, mut piece) in std::mem::take(&mut self.merge_damage) {
            if crate::plugin_player::shields_damage(self, slot, piece.source) {
                crate::command_damage::report(self, None, &piece, piece.amount, false);
                continue;
            }
            let raw = piece.amount;
            let accepted = (slot.0 as usize) < self.entities.len()
                && self.commit_damage_piece(slot, &mut piece, &mut events, &mut spawned);
            let target =
                ((slot.0 as usize) < self.entities.len()).then(|| self.entities.body(slot).id);
            crate::command_damage::report(self, target, &piece, raw, accepted);
            if !accepted {
                continue;
            }
            if let Some((_, pieces)) = damaged.iter_mut().find(|(target, _)| *target == slot) {
                pieces.push((piece, (raw, 0.0)));
            } else {
                damaged.push((slot, vec![(piece, (raw, 0.0))]));
            }
        }
        for (slot, mut pieces) in damaged {
            if (slot.0 as usize) >= self.entities.len() {
                continue;
            }
            let total: f32 = pieces.iter().map(|(piece, _)| piece.amount).sum();
            if total <= 0.0 {
                continue;
            }
            let id = self.entities.body(slot).id;
            let is_player = matches!(self.entities.body(slot).kind, EntityKind::Player { .. });
            if is_player {
                let statistics_killer = pieces
                    .iter()
                    .rev()
                    .find_map(|(piece, _)| crate::statistics::kill_credit(self, piece.source));
                let rules = self.registry.item_rules();
                let mut died = false;
                let mut health_changed = false;
                let mut armor_changed = false;
                let mut absorption_changed = false;
                let victim_uuid = self.entities.row(slot).uuid;
                let mut combat_hits = Vec::new();
                let mut totems = Vec::new();
                for (piece, amounts) in &mut pieces {
                    let DamagePiece {
                        amount,
                        kind,
                        source,
                        physical,
                        custom_armor,
                    } = *piece;
                    let mut credit = None;
                    if let Some(player) = self.players.get_mut(&id) {
                        let before = player.vitals.health;
                        let absorption_before = player.active_effects.absorption;
                        let reduced = if custom_armor {
                            amount
                        } else if let Some((armor, toughness)) = player
                            .effective_attribute("armor")
                            .zip(player.effective_attribute("armor_toughness"))
                        {
                            crate::survival::reduce_damage_with_player_attributes(
                                &rules,
                                &player.inventory.slots[36..40],
                                amount,
                                kind,
                                crate::mace::breach(source),
                                mc_data::armor::ArmorStats {
                                    armor: armor.min(30.0) as f32,
                                    toughness: toughness.min(20.0) as f32,
                                },
                            )
                        } else {
                            crate::survival::reduce_damage_with_breach(
                                &rules,
                                &player.inventory.slots[36..40],
                                amount,
                                kind,
                                crate::mace::breach(source),
                            )
                        };
                        let after_armor = reduced;
                        let absorption = player.active_effects.absorption;
                        let reduced = player.active_effects.reduce_damage(kind, reduced);
                        let absorbed = (absorption - player.active_effects.absorption).max(0.0);
                        let resisted = if !matches!(
                            kind,
                            crate::survival::DamageKind::Void
                                | crate::survival::DamageKind::GenericKill
                                | crate::survival::DamageKind::Starvation
                        ) && player
                            .active_effects
                            .has(crate::effects::EffectId::Resistance)
                        {
                            (after_armor - reduced - absorbed).max(0.0)
                        } else {
                            0.0
                        };
                        let reduced = if kind == crate::survival::DamageKind::Poison {
                            reduced.min((player.vitals.health - 1.0).max(0.0))
                        } else {
                            reduced
                        };
                        let wear_amount = if custom_armor {
                            physical.map_or(amount, |impact| impact.damage_hp)
                        } else {
                            amount
                        };
                        let wear = crate::survival::armor_wear(wear_amount, kind);
                        if wear > 0
                            && !player.creative()
                            && !player.spectator()
                            && !player.vitals.is_dead()
                        {
                            for (stack, armor_slot) in player.inventory.slots[36..40]
                                .iter_mut()
                                .zip(["feet", "legs", "chest", "head"])
                            {
                                if stack.is_empty() {
                                    continue;
                                }
                                let Some(max_damage) = rules
                                    .facts
                                    .facts_for_stack(stack, rules.items)
                                    .filter(|facts| {
                                        !custom_armor
                                            || facts
                                                .physical_profile
                                                .as_ref()
                                                .is_none_or(|profile| profile.armor.is_none())
                                    })
                                    .filter(|facts| {
                                        facts
                                            .armor
                                            .as_ref()
                                            .is_some_and(|armor| armor.slot == armor_slot)
                                    })
                                    .and_then(|facts| facts.max_damage)
                                else {
                                    continue;
                                };
                                let wear = crate::enchantments::durability_damage(
                                    stack,
                                    wear,
                                    true,
                                    &mut player.enchantment_seed,
                                );
                                if wear == 0 {
                                    continue;
                                }
                                let damage = stack.damage.unwrap_or(0).saturating_add(wear);
                                if u32::try_from(damage).is_ok_and(|damage| damage >= max_damage) {
                                    player
                                        .statistics
                                        .increment(StatType::Broken, stack.item_id, 1);
                                    *stack = mc_data::ItemStack::EMPTY;
                                } else {
                                    stack.damage = Some(damage);
                                }
                                armor_changed = true;
                            }
                        }
                        let before_hit = player.vitals.health;
                        let lethal = player.vitals.hurt(kind, reduced);
                        let health_after_hit = player.vitals.health;
                        player.statistics.increment_custom(
                            CustomStat::DamageTaken,
                            (reduced * 10.0).round() as i32,
                        );
                        player.statistics.increment_custom(
                            CustomStat::DamageAbsorbed,
                            (absorbed * 10.0).round() as i32,
                        );
                        player.statistics.increment_custom(
                            CustomStat::DamageResisted,
                            (resisted * 10.0).round() as i32,
                        );
                        amounts.1 = (before_hit - player.vitals.health).max(0.0);
                        credit =
                            Some(((before - player.vitals.health).max(0.0), absorbed, resisted));
                        if lethal {
                            if let Some(rescue) = totem::rescue(player, &self.registry, kind) {
                                totems.push(rescue);
                            } else {
                                died = true;
                            }
                        }
                        health_changed |= lethal || player.vitals.health != before;
                        absorption_changed |= player.active_effects.absorption != absorption_before;
                        let damage = (before_hit.max(0.0) - health_after_hit.max(0.0)).max(0.0);
                        if damage > 0.0
                            && let Some(attacker) = crate::plugin::damage_attacker(source)
                        {
                            combat_hits.push((
                                attacker,
                                damage,
                                before_hit > 0.0 && player.vitals.is_dead(),
                            ));
                        }
                    }
                    if let Some((dealt, absorbed, resisted)) = credit {
                        crate::statistics::damage_dealt(
                            &mut self.players,
                            source,
                            dealt,
                            absorbed,
                            resisted,
                        );
                    }
                }
                for (used, changes) in totems {
                    if let Some(connection) = self.connections.entity_connection(id) {
                        for change in changes {
                            self.push_player(connection, change);
                        }
                    }
                    let evidence = crate::advancements::item_snapshot(self, &used);
                    crate::advancements::trigger(
                        self,
                        id,
                        "minecraft:used_totem",
                        serde_json::json!({"item": evidence}),
                    );
                    crate::vibration::emit(
                        self,
                        self.entities.pos(slot),
                        crate::vibration::GameEvent::ItemInteractFinish,
                        Some(id),
                    );
                    // LivingEntity.checkTotemDeathProtection, pinned 26.1.2.
                    events.push(EntityEvent::Status {
                        entity: id,
                        event: 35,
                    });
                    crate::effects::sync_appearance(self, id);
                }
                for (attacker, damage, killed) in combat_hits {
                    crate::plugin_npcs::record_combat(self, attacker, victim_uuid, damage, killed);
                }
                let health = self
                    .players
                    .get(&id)
                    .map(|player| player.vitals.health)
                    .unwrap_or(0.0);
                self.entities.row_mut(slot).health = health;
                let last = pieces.len() - 1;
                let last_damage_kind = pieces.last().map(|(piece, _)| piece.kind);
                let last_damage_source = pieces.last().and_then(|(piece, _)| piece.source);
                for (index, (piece, amounts)) in pieces.into_iter().enumerate() {
                    crate::advancements::damage(
                        self,
                        id,
                        piece.source,
                        piece.kind,
                        amounts,
                        died && index == last,
                        false,
                    );
                }
                if absorption_changed && let Some(player) = self.players.get(&id) {
                    let connection = player.connection;
                    let amount = player.active_effects.absorption;
                    self.push_player(connection, PlayerChange::Absorption { amount });
                    crate::effects::sync_appearance(self, id);
                }
                if health_changed {
                    let appearance =
                        std::sync::Arc::make_mut(&mut self.entities.row_mut(slot).appearance);
                    appearance.health = health;
                    self.appearance_changed.push(id);
                }
                if died && self.entities.body(slot).alive {
                    self.entities.body_mut(slot).alive = false;
                    crate::statistics::kill(self, statistics_killer, id);
                    deaths.push(id);
                    crate::plugin_inventories::invalidate_player(self, id);
                    crate::plugin::player_died(self, id);
                    events.push(EntityEvent::Death { entity: id });
                    crate::effects_trigger::death(self, id, &mut spawned);
                    crate::scoreboard::automatic(self, &[id], &[]);
                    let keep_inventory = crate::command_rules::boolean(
                        self,
                        crate::command_rules::BoolRule::KEEP_INVENTORY,
                    );
                    // A deployed `death_graves` package takes custody of the drops
                    // this death would have scattered. `keepInventory` wins, as in
                    // vanilla: a retained inventory never reaches a grave. A death
                    // with no live connection keeps the vanilla drop too, because
                    // nobody could be told where the grave is.
                    let grave_owner =
                        if keep_inventory || self.connections.entity_connection(id).is_none() {
                            None
                        } else {
                            self.plugin_runtime
                                .as_ref()
                                .and_then(|runtime| runtime.death_graves.clone())
                        };
                    let keep_experience = grave_owner.is_some();
                    let death_xp = if keep_inventory || keep_experience {
                        0
                    } else {
                        self.players
                            .get(&id)
                            .map_or(0, |player| player.vitals.experience.death_drop())
                    };
                    let rules = self.registry.item_rules();
                    let effects = self
                        .players
                        .get_mut(&id)
                        .map(|player| player.die(&rules, keep_inventory, keep_experience))
                        .unwrap_or_default();
                    let pos = self.entities.pos(slot);
                    crate::experience_orbs::award(self, pos, death_xp, &mut spawned);
                    let mut dropped = effects.dropped;
                    let grave = grave_owner.and_then(|owner| {
                        crate::plugin_graves::bury(self, &owner, pos, &mut dropped)
                    });
                    if let Some(connection) = self.connections.entity_connection(id) {
                        for change in effects.changes {
                            self.push_player(connection, change);
                        }
                        for stack in dropped {
                            let drop = spawn_item_entity(self, pos, stack);
                            spawned.push(drop);
                        }
                        // Death-screen order (commit drain = same tick as
                        // the drops): Health(0) then Died. Appearance was
                        // updated with the committed health above.
                        if let Some(player) = self.players.get(&id) {
                            self.push_player(connection, player.health_change());
                        }
                        let show = crate::command_rules::boolean(
                            self,
                            crate::command_rules::BoolRule::SHOW_DEATH_MESSAGES,
                        );
                        let message = if show {
                            super::death_message::component(
                                self,
                                id,
                                last_damage_kind.unwrap_or(crate::survival::DamageKind::Generic),
                                last_damage_source,
                            )
                        } else {
                            mc_nbt::Tag::String(String::new())
                        };
                        self.push_player(
                            connection,
                            PlayerChange::Died {
                                message: message.clone(),
                            },
                        );
                        if show {
                            super::death_message::broadcast(self, id, message);
                        }
                    } else {
                        // Offline/dead-connection victim: drops still spawn
                        // (journaled below); nobody to sync to.
                        for stack in dropped {
                            let drop = spawn_item_entity(self, pos, stack);
                            spawned.push(drop);
                        }
                    }
                    if let Some(grave) = grave {
                        crate::plugin::player_grave_created(self, id, &grave);
                    }
                } else if health_changed
                    && let Some(connection) = self.connections.entity_connection(id)
                    && let Some(player) = self.players.get(&id)
                {
                    self.push_player(connection, player.health_change());
                }
                if armor_changed
                    && !died
                    && let Some(connection) = self.connections.entity_connection(id)
                    && let Some(player) = self.players.get(&id)
                {
                    self.push_player(connection, player.inventory_contents());
                }
            } else {
                let statistics_killer = pieces
                    .iter()
                    .rev()
                    .find_map(|(piece, _)| crate::statistics::kill_credit(self, piece.source));
                let player_hit = pieces
                    .iter()
                    .filter_map(|(piece, _)| {
                        let attacker = match piece.source? {
                            crate::intent::DamageSource::Melee { attacker, .. }
                            | crate::intent::DamageSource::Mace { attacker, .. }
                            | crate::intent::DamageSource::Sweep { attacker, .. } => attacker,
                            crate::intent::DamageSource::Projectile { owner, .. } => owner,
                            crate::intent::DamageSource::Command { cause, .. } => cause?.0,
                            crate::intent::DamageSource::Explosion { .. } => return None,
                        };
                        self.players.contains_key(&attacker).then_some(attacker)
                    })
                    .next();
                let killer = pieces
                    .iter()
                    .rev()
                    .find_map(|(piece, _)| match piece.source {
                        Some(
                            crate::intent::DamageSource::Melee { attacker, .. }
                            | crate::intent::DamageSource::Mace { attacker, .. },
                        ) => Some(attacker),
                        _ => None,
                    });
                let advancement_source = pieces.last().and_then(|(piece, _)| piece.source);
                let cause = crate::entity_loot::death_cause(
                    self,
                    slot,
                    pieces.last().and_then(|(piece, _)| piece.source),
                    pieces
                        .last()
                        .map_or(crate::survival::DamageKind::Generic, |(piece, _)| {
                            piece.kind
                        }),
                );
                let armor_stand = crate::armor_stand::is_stand(self, slot);
                let rules = self.registry.item_rules();
                let victim_uuid = self.entities.row(slot).uuid;
                let mut combat_hits = Vec::new();
                let was_alive = self.entities.body(slot).alive;
                let resists_magic = match self.entities.body(slot).kind {
                    EntityKind::Mob { type_id } => self
                        .registry
                        .blueprint(type_id)
                        .is_some_and(|bp| bp.capabilities.resists_magic),
                    _ => false,
                };
                let row = self.entities.row_mut(slot);
                let previous_health = row.health;
                if let Some(attacker) = player_hit {
                    row.last_player_hit = Some((attacker, self.tick));
                }
                for (piece, amounts) in &mut pieces {
                    let reduced = if armor_stand || piece.custom_armor {
                        piece.amount
                    } else {
                        let reduced = crate::entity_loot::reduce_damage_with_breach(
                            &rules,
                            row,
                            piece.amount,
                            piece.kind,
                            crate::mace::breach(piece.source),
                        );
                        crate::entity_loot::wear_equipment(&rules, row, piece.amount, piece.kind);
                        reduced
                    };
                    if piece.custom_armor && !armor_stand {
                        let incoming = piece
                            .physical
                            .map_or(piece.amount, |impact| impact.damage_hp);
                        crate::entity_loot::wear_equipment_after_physical(
                            &rules, row, incoming, piece.kind,
                        );
                    }
                    let reduced = if resists_magic
                        && matches!(
                            piece.kind,
                            crate::survival::DamageKind::Magic
                                | crate::survival::DamageKind::Poison
                                | crate::survival::DamageKind::Thorns
                        ) {
                        reduced * 0.15
                    } else {
                        reduced
                    };
                    let before = row.health.max(0.0);
                    let absorption = row.active_effects.absorption;
                    let amount = row.active_effects.reduce_damage(piece.kind, reduced);
                    let absorbed = (absorption - row.active_effects.absorption).max(0.0);
                    let resisted =
                        if !matches!(
                            piece.kind,
                            crate::survival::DamageKind::Void
                                | crate::survival::DamageKind::GenericKill
                                | crate::survival::DamageKind::Starvation
                        ) && row.active_effects.has(crate::effects::EffectId::Resistance)
                        {
                            (reduced - amount - absorbed).max(0.0)
                        } else {
                            0.0
                        };
                    let before_hit = row.health.max(0.0);
                    row.health -= if piece.kind == crate::survival::DamageKind::Poison {
                        amount.min((row.health - 1.0).max(0.0))
                    } else {
                        amount
                    };
                    crate::statistics::damage_dealt(
                        &mut self.players,
                        piece.source,
                        (before - row.health.max(0.0)).max(0.0),
                        absorbed,
                        resisted,
                    );
                    amounts.1 = (before_hit - row.health.max(0.0)).max(0.0);
                    let damage = (before_hit.max(0.0) - row.health.max(0.0)).max(0.0);
                    if damage > 0.0
                        && let Some(attacker) = crate::plugin::damage_attacker(piece.source)
                    {
                        combat_hits.push((attacker, damage, before_hit > 0.0 && row.health <= 0.0));
                    }
                }
                let health = row.health;
                let appearance = std::sync::Arc::make_mut(&mut row.appearance);
                appearance.health = health;
                self.appearance_changed.push(id);
                let killed = row.health <= 0.0 && was_alive;
                let villager = row
                    .villager
                    .as_ref()
                    .is_some_and(|v| v.conversion_time.is_none());
                for (attacker, damage, killed) in combat_hits {
                    crate::plugin_npcs::record_combat(self, attacker, victim_uuid, damage, killed);
                }
                crate::end_fight::damage_committed(self, slot, (previous_health - health).max(0.0));
                let last = pieces.len() - 1;
                for (index, (piece, amounts)) in pieces.into_iter().enumerate() {
                    crate::advancements::damage(
                        self,
                        id,
                        piece.source,
                        piece.kind,
                        amounts,
                        killed && index == last,
                        false,
                    );
                }
                if killed {
                    crate::statistics::kill(self, statistics_killer, id);
                    crate::raids::killed(self, id, player_hit);
                }
                if !killed {
                    crate::entity_loot::restore(self, slot);
                }
                if villager {
                    crate::villager::brain::harmed(self, id, player_hit, killed);
                }
                if killed && crate::villager::conversion::infect(self, slot, killer) {
                    continue;
                }
                if killed && crate::end_fight::begin_death(self, slot) {
                    continue;
                }
                let (body, row) = self.entities.parts_mut(slot);
                if row.health <= 0.0 && body.alive {
                    body.alive = false;
                    let kind = body.kind;
                    deaths.push(id);
                    events.push(EntityEvent::Death { entity: id });
                    let victim_uuid = self.entities.row(slot).uuid;
                    crate::plugin_npcs::record_death(self, victim_uuid);
                    if let Some(attacker) = killer {
                        crate::plugin::kill_event(self, attacker, slot);
                    }
                    crate::effects_trigger::death(self, id, &mut spawned);
                    crate::leash::detach(self, slot);
                    crate::mounts::death(self, slot);
                    crate::entity_loot::commit_death(self, slot, cause, &mut spawned);
                    let sculk_absorbed = crate::sculk::death(self, slot);
                    if sculk_absorbed.is_some_and(|xp| xp > 0)
                        && let Some(attacker) = advancement_source.and_then(|source| match source {
                            crate::intent::DamageSource::Melee { attacker, .. }
                            | crate::intent::DamageSource::Mace { attacker, .. }
                            | crate::intent::DamageSource::Sweep { attacker, .. } => Some(attacker),
                            crate::intent::DamageSource::Projectile { owner, .. } => Some(owner),
                            crate::intent::DamageSource::Explosion { kill_credit, .. } => {
                                kill_credit.map(|(id, _)| id)
                            }
                            crate::intent::DamageSource::Command { cause, .. } => {
                                cause.map(|(id, _)| id)
                            }
                        })
                    {
                        let evidence = crate::advancements::entity_snapshot(self, id);
                        crate::advancements::trigger(
                            self,
                            attacker,
                            "minecraft:kill_mob_near_sculk_catalyst",
                            serde_json::json!({"entity":evidence}),
                        );
                    }
                    let row = self.entities.row(slot);
                    if row
                        .last_player_hit
                        .is_some_and(|(_, tick)| self.tick.saturating_sub(tick) < 100)
                        && crate::command_rules::boolean(
                            self,
                            crate::command_rules::BoolRule::MOB_DROPS,
                        )
                        && sculk_absorbed.is_none()
                        && let EntityKind::Mob { type_id } = kind
                        && let Some(blueprint) = self.registry.blueprint(type_id)
                    {
                        let xp = crate::entity_loot::death_experience(
                            &blueprint.name,
                            row,
                            self.tick ^ id.0 as u64,
                        );
                        let pos = self.entities.pos(slot);
                        crate::experience_orbs::award(self, pos, xp, &mut spawned);
                    }
                }
            }
        }
        (deaths, events, spawned)
    }

    /// Shared hit admission, shield response, hurt window and motion pipeline.
    fn commit_damage_piece(
        &mut self,
        slot: EntitySlot,
        piece: &mut DamagePiece,
        events: &mut Vec<EntityEvent>,
        spawned: &mut Vec<EntityId>,
    ) -> bool {
        let victim = self.entities.body(slot).id;
        if !piece.kind.bypasses_invulnerability()
            && self
                .players
                .get(&victim)
                .is_some_and(|player| player.creative() || player.spectator())
        {
            return false;
        }
        // Recheck current geometry at merge, not just the earlier target search.
        if let Some(crate::intent::DamageSource::Melee { attacker, .. }) = piece.source
            && !self.players.contains_key(&attacker)
            && let Some(actor) = self.entities.slot_of(attacker)
        {
            let mut from = self.entities.pos(actor);
            let mut to = self.entities.pos(slot);
            from[1] += self.entities.body(actor).height * 0.85;
            to[1] += self.entities.body(slot).height * 0.85;
            if !crate::hostile::sight_clear(self, from, to) {
                return false;
            }
        }
        if self.entity_session_locked(self.entities.body(slot).id) {
            return false;
        }
        if crate::physical_defense::impact_from_piece(self, slot, piece)
            .is_some_and(|impact| crate::plugin_npcs::absorb_deployed_impact(self, slot, impact))
        {
            return false;
        }
        if !crate::enderman::damage_allowed(self, slot, piece) {
            return false;
        }
        if !crate::end_fight::damage_allowed(self, slot, piece) {
            return false;
        }
        let row = self.entities.row(slot);
        if !crate::special_mobs::damage_allowed(row, piece) {
            return false;
        }
        if row.tnt.is_some() || row.falling.is_some() {
            return false;
        }
        if matches!(self.entities.body(slot).kind, EntityKind::Player { .. }) {
            let rule = match piece.kind {
                crate::survival::DamageKind::Fall => {
                    Some(crate::command_rules::BoolRule::FALL_DAMAGE)
                }
                crate::survival::DamageKind::Drowning => {
                    Some(crate::command_rules::BoolRule::DROWNING_DAMAGE)
                }
                crate::survival::DamageKind::InFire
                | crate::survival::DamageKind::OnFire
                | crate::survival::DamageKind::Lava
                | crate::survival::DamageKind::HotFloor
                | crate::survival::DamageKind::Fireball
                | crate::survival::DamageKind::Lightning
                | crate::survival::DamageKind::Campfire => {
                    Some(crate::command_rules::BoolRule::FIRE_DAMAGE)
                }
                _ => None,
            };
            if rule.is_some_and(|rule| !crate::command_rules::boolean(self, rule)) {
                return false;
            }
        }
        if matches!(
            piece.kind,
            crate::survival::DamageKind::InFire
                | crate::survival::DamageKind::OnFire
                | crate::survival::DamageKind::Lava
                | crate::survival::DamageKind::HotFloor
                | crate::survival::DamageKind::Fireball
                | crate::survival::DamageKind::Lightning
                | crate::survival::DamageKind::Campfire
        ) {
            let body = self.entities.body(slot);
            let row = self.entities.row(slot);
            let effects = self
                .players
                .get(&body.id)
                .map(|player| &player.active_effects)
                .unwrap_or(&row.active_effects);
            if effects.has(crate::effects::EffectId::FireResistance) {
                return false;
            }
        }
        if let Some(
            crate::intent::DamageSource::Melee { attacker, .. }
            | crate::intent::DamageSource::Mace { attacker, .. }
            | crate::intent::DamageSource::Sweep { attacker, .. }
            | crate::intent::DamageSource::Projectile {
                owner: attacker, ..
            }
            | crate::intent::DamageSource::Command {
                cause: Some((attacker, _)),
                ..
            },
        ) = piece.source
        {
            let target = self.entities.body(slot).id;
            if !crate::scoreboard::permits_player_damage(self, attacker, target)
                || (attacker != target
                    && self.players.contains_key(&attacker)
                    && self.players.contains_key(&target)
                    && !crate::command_rules::boolean(self, crate::command_rules::BoolRule::PVP))
            {
                return false;
            }
        }
        if self.commit_shield_block(slot, piece, events) {
            let victim = self.entities.body(slot).id;
            crate::advancements::damage(
                self,
                victim,
                piece.source,
                piece.kind,
                (piece.amount, 0.0),
                false,
                true,
            );
            return false;
        }
        if crate::physical_defense::block_damage(self, slot, piece) {
            return false;
        }
        let fresh = self.entities.body(slot).hurt_at.is_none_or(|at| {
            self.tick.saturating_sub(at) >= crate::entities::HURT_INVULNERABLE_TICKS
        });
        piece.amount = self
            .entities
            .land_hit(slot, self.tick, piece.amount, piece.kind);
        if piece.amount <= 0.0 {
            return false;
        }
        let id = self.entities.body(slot).id;
        if !crate::physical_defense::reduce_damage(self, slot, piece) {
            return false;
        }
        if fresh && !piece.kind.bypasses_invulnerability() {
            if let Some(source) = piece.source
                && piece.kind != crate::survival::DamageKind::Thorns
            {
                self.commit_hit_knockback(slot, source, crate::combat::HIT_KNOCKBACK);
            }
            events.push(EntityEvent::Hurt { entity: id });
        }
        let (cause, direct) = match piece.source {
            Some(
                crate::intent::DamageSource::Melee { attacker, .. }
                | crate::intent::DamageSource::Mace { attacker, .. }
                | crate::intent::DamageSource::Sweep { attacker, .. },
            ) => (Some(attacker), Some(attacker)),
            Some(crate::intent::DamageSource::Projectile {
                owner, projectile, ..
            }) => (
                self.entities.slot_of(owner).map(|_| owner),
                Some(projectile),
            ),
            Some(crate::intent::DamageSource::Explosion {
                source,
                kill_credit,
                ..
            }) => (kill_credit.map(|(owner, _)| owner), source),
            Some(crate::intent::DamageSource::Command { cause, direct, .. }) => (
                cause.map(|(entity, _)| entity),
                direct.map(|(entity, _)| entity),
            ),
            None => (None, None),
        };
        // Vanilla broadcasts the damage event only for a fresh hit; a stronger
        // hit inside the hurt window changes health (journaled through the
        // appearance post-image) without a new client hurt animation.
        if fresh {
            events.push(EntityEvent::Damage {
                entity: self.entities.body(slot).id,
                source_type_id: piece.kind.type_id(
                    piece.source,
                    cause.is_some_and(|id| self.players.contains_key(&id)),
                ),
                cause,
                direct,
                source_position: match piece.source {
                    Some(crate::intent::DamageSource::Command {
                        direct: None,
                        position,
                        ..
                    }) => position,
                    _ => None,
                },
            });
        }
        crate::enderman::hurt(self, slot, piece);
        if matches!(
            piece.source,
            Some(crate::intent::DamageSource::Projectile { flame: true, .. })
        ) {
            let target = self.entities.body(slot).id;
            crate::contact_hazards::ignite(self, target, 100);
        }
        if let Some(
            crate::intent::DamageSource::Melee { fire_ticks, .. }
            | crate::intent::DamageSource::Mace { fire_ticks, .. }
            | crate::intent::DamageSource::Sweep { fire_ticks, .. },
        ) = piece.source
            && fire_ticks > 0
        {
            let target = self.entities.body(slot).id;
            crate::contact_hazards::ignite(self, target, fire_ticks);
        }
        if let Some(
            crate::intent::DamageSource::Melee { bane_ticks, .. }
            | crate::intent::DamageSource::Mace { bane_ticks, .. }
            | crate::intent::DamageSource::Sweep { bane_ticks, .. },
        ) = piece.source
            && bane_ticks > 0
        {
            let target = self.entities.body(slot).id;
            crate::projectiles::payloads::give_effect(
                self,
                target,
                crate::effects::ActiveEffect::new(
                    crate::effects::EffectId::Slowness,
                    bane_ticks.min(i32::MAX as u32) as i32,
                    3,
                ),
            );
        }
        crate::entity_loot::accepted_hit(self, slot, piece.source);
        if let Some(attacker) = crate::plugin::damage_attacker(piece.source) {
            crate::hostile::retaliate(self, slot, attacker);
        }
        crate::projectiles::embedded_projectiles::accepted_hit(self, slot, piece.source);
        crate::bees::accepted_hit(self, slot, piece.source);
        crate::enchantment_thorns::accepted_hit(self, slot, piece);
        crate::mace::accepted_hit(self, slot, piece.source);
        crate::effects_trigger::accepted_hit(self, slot, spawned);
        true
    }

    /// Resolve shielding before hurt invulnerability, armor wear or victim motion.
    fn commit_shield_block(
        &mut self,
        slot: EntitySlot,
        piece: &DamagePiece,
        events: &mut Vec<EntityEvent>,
    ) -> bool {
        use crate::intent::DamageSource;
        use crate::survival::DamageKind;
        if piece.amount <= 0.0
            || !matches!(
                piece.kind,
                DamageKind::Melee
                    | DamageKind::Projectile
                    | DamageKind::Fireball
                    | DamageKind::Explosion
            )
            || self.tick < self.entities.row(slot).protected_until
        {
            return false;
        }
        if let Some(DamageSource::Projectile { projectile, .. }) = piece.source {
            let record = self
                .entities
                .slot_of(projectile)
                .and_then(|slot| self.entities.row(slot).projectile.as_deref())
                .or_else(|| self.projectiles.retired_crossbows.get(&projectile));
            if record.is_some_and(|record| record.piercing > 0) {
                return false;
            }
        }
        let id = self.entities.body(slot).id;
        let rules = self.registry.item_rules();
        let Some(hand) = self
            .players
            .get(&id)
            .and_then(|player| player.raised_shield(&rules))
        else {
            return false;
        };
        let pos = self.entities.pos(slot);
        let (direction, attacker) = match piece.source {
            Some(
                DamageSource::Melee {
                    attacker, position, ..
                }
                | DamageSource::Mace {
                    attacker, position, ..
                }
                | DamageSource::Sweep {
                    attacker, position, ..
                },
            ) => ([position[0] - pos[0], position[2] - pos[2]], Some(attacker)),
            Some(DamageSource::Projectile { velocity, .. }) => ([-velocity[0], -velocity[1]], None),
            Some(DamageSource::Explosion { position, .. }) => {
                ([position[0] - pos[0], position[2] - pos[2]], None)
            }
            Some(DamageSource::Command {
                position: Some(position),
                ..
            }) => ([position[0] - pos[0], position[2] - pos[2]], None),
            Some(DamageSource::Command { position: None, .. }) => return false,
            None => return false,
        };
        let yaw = f64::from(self.entities.rot(slot)[0]).to_radians();
        if direction[0] * direction[0] + direction[1] * direction[1] < 1.0e-12
            || direction[0] * -f64::from(mth::sin(yaw)) + direction[1] * f64::from(mth::cos(yaw))
                < 0.0
        {
            return false;
        }
        let disables = attacker
            .and_then(|attacker| self.players.get(&attacker))
            .and_then(|player| {
                rules.facts.facts_for_stack(
                    &player.inventory.slots[usize::from(player.inventory.selected.min(8))],
                    rules.items,
                )
            })
            .and_then(|facts| facts.weapon_disable_blocking_seconds)
            .is_some_and(|seconds| seconds > 0.0);
        let plan = self.players[&id].plan_shield_wear(&rules, hand, piece.amount);
        if crate::plugin_inventories::below_player_debit_floors(self, id, &[], plan.broken_slot()) {
            return false;
        }
        let player = self.players.get_mut(&id).expect("blocking player");
        let connection = player.connection;
        player.statistics.increment_custom(
            CustomStat::DamageBlockedByShield,
            (piece.amount * 10.0).round() as i32,
        );
        let effects = player.apply_tool_wear(plan);
        let broke = player.raised_shield(&rules).is_none();
        // Vanilla rechecks the blocking item after durability; a broken
        // shield has already stopped use and cannot receive a cooldown.
        let cooldown = (disables && !broke).then(|| player.disable_shield());
        for change in effects.changes {
            self.push_player(connection, change);
        }
        if let Some(change) = cooldown {
            self.push_player(connection, change);
        }
        events.push(EntityEvent::ShieldBlock {
            entity: id,
            disabled: false,
        });
        if disables || broke {
            events.push(EntityEvent::ShieldBlock {
                entity: id,
                disabled: true,
            });
        }
        if let Some(DamageSource::Projectile { projectile, .. }) = piece.source
            && let Some(arrow) = self.entities.slot_of(projectile)
            && self
                .entities
                .row(arrow)
                .projectile
                .as_ref()
                .is_some_and(|record| record.kind.arrow())
        {
            self.projectiles.pending_arrow_removals.remove(&projectile);
            let velocity = self.entities.vel(arrow).map(|v| v / -200.0);
            crate::projectiles::set_flight(self, arrow, velocity);
            if let Some(record) = self.entities.row_mut(arrow).projectile.as_mut() {
                record.critical = false;
            }
            self.appearance_changed.push(projectile);
        }
        if let Some(attacker) = attacker
            && let Some(attacker_slot) = self.entities.slot_of(attacker)
        {
            self.commit_hit_knockback(
                attacker_slot,
                DamageSource::Melee {
                    attacker: id,
                    position: pos,
                    yaw: 0.0,
                    bonus_knockback: 0.0,
                    sweep: false,
                    fire_ticks: 0,
                    bane_ticks: 0,
                },
                0.5,
            );
        }
        true
    }

    /// Apply hit knockback or shield recoil in vanilla blocks/tick, then
    /// store core blocks/second and reliably notify the affected player.
    pub(crate) fn commit_hit_knockback(
        &mut self,
        slot: EntitySlot,
        source: crate::intent::DamageSource,
        strength: f64,
    ) {
        let pos = self.entities.pos(slot);
        let ground = self.entities.ground(slot);
        let old = self.entities.vel(slot);
        let (mut direction, bonus, yaw) = match source {
            crate::intent::DamageSource::Melee {
                position,
                yaw,
                bonus_knockback,
                ..
            }
            | crate::intent::DamageSource::Mace {
                position,
                yaw,
                bonus_knockback,
                ..
            } => (
                [position[0] - pos[0], position[2] - pos[2]],
                bonus_knockback,
                yaw,
            ),
            crate::intent::DamageSource::Sweep { yaw, .. } => {
                let angle = f64::from(yaw).to_radians();
                (
                    [f64::from(mth::sin(angle)), -f64::from(mth::cos(angle))],
                    0.0,
                    yaw,
                )
            }
            crate::intent::DamageSource::Projectile { velocity, .. } => {
                ([-velocity[0], -velocity[1]], 0.0, 0.0)
            }
            crate::intent::DamageSource::Command { position, .. } => (
                position.map_or([0.0; 2], |position| {
                    [position[0] - pos[0], position[2] - pos[2]]
                }),
                0.0,
                0.0,
            ),
            crate::intent::DamageSource::Explosion { .. } => return,
        };
        // Vanilla randomizes a coincident source. A tick/id-derived angle
        // preserves a nonzero unbiased direction and deterministic merge.
        if direction[0] * direction[0] + direction[1] * direction[1] < 1.0e-5 {
            let seed = self.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ self.entities.body(slot).id.0 as u64;
            let angle = (seed as f64) * (std::f64::consts::TAU / u64::MAX as f64);
            direction = [f64::from(mth::sin(angle)), f64::from(mth::cos(angle))];
        }
        let resistance = crate::entity_loot::effective_attribute(
            &self.registry.item_rules(),
            self.entities.row(slot),
            "knockback_resistance",
            0.0,
        );
        let mut velocity = crate::combat::knockback_velocity(
            old.map(|v| v / 20.0),
            ground,
            strength,
            direction[0],
            direction[1],
            resistance,
        );
        if bonus > 0.0 {
            let yaw = f64::from(yaw).to_radians();
            velocity = crate::combat::knockback_velocity(
                velocity,
                ground,
                bonus,
                f64::from(mth::sin(yaw)),
                -f64::from(mth::cos(yaw)),
                resistance,
            );
        }
        if let crate::intent::DamageSource::Projectile {
            velocity: flight,
            punch,
            ..
        } = source
            && punch > 0
        {
            let length = mth::length2(flight[0], flight[1]);
            if length > 1.0e-9 {
                let force = f64::from(punch) * 0.6;
                velocity[0] += flight[0] / length * force;
                velocity[2] += flight[1] / length * force;
                velocity[1] += 0.1;
            }
        }
        let velocity = velocity.map(|v| v * 20.0);
        self.entities.set_vel(slot, velocity);
        let impulse = self.entities.horizontal_impulse(slot);
        self.entities.set_horizontal_impulse(
            slot,
            [
                impulse[0] + (velocity[0] - old[0]),
                impulse[1] + (velocity[2] - old[2]),
            ],
        );
        let id = self.entities.body(slot).id;
        if let Some(connection) = self.connections.entity_connection(id) {
            self.push_player(
                connection,
                PlayerChange::Motion {
                    entity: id,
                    velocity,
                },
            );
        }
    }
}
