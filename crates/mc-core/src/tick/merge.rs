//! Intent validation, deterministic merge and post-merge effects
//! (drops, loot, tool wear). Moved verbatim from `tick.rs`.

use super::*;

impl TickState {
    /// Kind-specific preconditions against merge state: a second feed of
    /// the same animal, or a second breed with the same partner, in one tick
    /// is rejected whole (pays nothing).
    pub(super) fn kind_valid(&self, intent: &Intent) -> bool {
        let kind = |id: EntityId| {
            self.entities
                .slot_of(id)
                .map(|slot| self.entities.body(slot).kind)
        };
        if !intent.footprint.block_write.is_empty()
            && kind(intent.actor).is_some_and(|kind| matches!(kind, EntityKind::Mob { .. }))
            && !crate::command_rules::boolean(self, crate::command_rules::BoolRule::MOB_GRIEFING)
        {
            return false;
        }
        match &intent.kind {
            IntentKind::FeedAnimal { target } => {
                self.entities.slot_of(*target).is_some_and(|slot| {
                    crate::entity_life::can_feed(self.entities.body(slot), self.entities.row(slot))
                        && crate::entity_life::can_breed(
                            self,
                            self.entities.body(slot),
                            self.entities.row(slot),
                        )
                })
            }
            IntentKind::DolphinFeed { plan } => crate::dolphin::valid(self, intent.actor, plan),
            IntentKind::HarvestAnimal { plan } => {
                crate::entity_life::valid(self, intent.actor, plan)
            }
            IntentKind::MountInteract { plan } => crate::mounts::valid(self, intent.actor, plan),
            IntentKind::Breed {
                partner,
                child_type,
                ..
            } => crate::entity_life::breeding::valid(self, intent.actor, *partner, *child_type),
            _ => true,
        }
    }

    /// Validate one footprint against current merge state.
    pub(crate) fn footprint_valid(&self, footprint: &Footprint) -> bool {
        for id in &footprint.entity_reads {
            if self
                .entities
                .slot_of(crate::end_fight::attack_target(self, *id).0)
                .is_none()
            {
                return false;
            }
        }
        for id in &footprint.entity_writes {
            if self
                .entities
                .slot_of(crate::end_fight::attack_target(self, *id).0)
                .is_none()
            {
                return false;
            }
        }
        if let Some(item) = footprint.item_take {
            let Some(slot) = self.entities.slot_of(item) else {
                return false;
            };
            // One winner per item per tick: a staged pickup for the same
            // slot rejects later claimants (loser rejects whole).
            if self
                .merge_pickups
                .iter()
                .any(|(staged, _, _)| *staged == slot)
            {
                return false;
            }
        }
        for (pos, expect) in &footprint.block_expect {
            if self.world.block_at(pos.x, pos.y, pos.z) != Some(*expect) {
                return false;
            }
            // A previously accepted intent already rewrote this position.
            if self
                .merge_block_writes
                .iter()
                .any(|(other, _)| other == pos)
            {
                return false;
            }
        }
        for claim in &footprint.inv {
            let Some(player) = self.players.get(&claim.owner) else {
                return false;
            };
            let slot = player.inventory.slots.get(claim.slot as usize);
            if slot.is_none_or(mc_data::ItemStack::is_empty) {
                if claim.take > 0 || claim.expect_count != 0 {
                    return false;
                }
            } else if let Some(slot) = slot {
                if slot.item_id != claim.expect_item || slot.count != claim.expect_count {
                    return false;
                }
                if slot.count < claim.take {
                    return false;
                }
            }
        }
        !crate::plugin_inventories::below_intent_floors(self, &footprint.inv)
    }

    /// Apply one validated intent's effect to merge staging.
    pub(super) fn apply_intent(&mut self, intent: &Intent) -> bool {
        match &intent.kind {
            IntentKind::AuthorizedBuild { .. } => return false,
            IntentKind::PhysicalAttack { started_tick } => {
                if let Some((slot, impact, source)) =
                    crate::physical_attacks::commit_attack(self, intent.actor, *started_tick)
                {
                    self.merge_damage.push((
                        slot,
                        DamagePiece {
                            amount: impact.damage_hp,
                            kind: crate::survival::DamageKind::Melee,
                            source: Some(source),
                            physical: Some(impact),
                            custom_armor: false,
                        },
                    ));
                }
            }
            IntentKind::PhysicalDamageEntity {
                target,
                impact,
                source,
            } => {
                if let Some(slot) = self.entities.slot_of(*target) {
                    self.merge_damage.push((
                        slot,
                        DamagePiece {
                            amount: impact.damage_hp,
                            kind: crate::survival::DamageKind::Projectile,
                            source: *source,
                            physical: Some(*impact),
                            custom_armor: false,
                        },
                    ));
                }
            }
            IntentKind::DamageEntity {
                target,
                amount,
                kind,
                source,
            } => {
                let (resolved, non_head) = crate::end_fight::attack_target(self, *target);
                if let Some(slot) = self.entities.slot_of(resolved) {
                    self.merge_damage.push((
                        slot,
                        DamagePiece {
                            amount: if non_head {
                                *amount / 4.0 + (*amount).min(1.0)
                            } else {
                                *amount
                            },
                            kind: *kind,
                            source: *source,
                            physical: None,
                            custom_armor: false,
                        },
                    ));
                }
            }
            IntentKind::PickupItem { item, count } => {
                // Reservation-free: the pickup reserves nothing at intent
                // time (capacity is known only at commit). Staging the
                // intent is enough; commit removes exactly the slots whose
                // remainder is empty.
                let (Some(item_slot), Some(actor_slot)) = (
                    self.entities.slot_of(*item),
                    self.entities.slot_of(intent.actor),
                ) else {
                    return false;
                };
                self.merge_pickups.push((item_slot, actor_slot, *count));
            }
            IntentKind::BreakBlock { pos } => {
                // Drops read the broken state now (merge precedes writes).
                if let Some(broken) = self.world.block_at(pos.x, pos.y, pos.z) {
                    let wear = self.block_break_tool_wear(intent.actor, broken);
                    let Ok(wear) = self.player_tool_wear_plan(intent.actor, wear) else {
                        return false;
                    };
                    let xp = self.block_break_experience(intent.actor, broken);
                    let rolled =
                        crate::block_drop_context::player_drops(self, intent.actor, *pos, broken);
                    let Some(drops) = crate::block_drop_context::admit(self, *pos, rolled) else {
                        return false;
                    };
                    if let Ok(Some(bees)) = crate::bees::inhabited_bees(self, *pos) {
                        let block = self
                            .registry
                            .blocks
                            .by_id(mc_world::BlockStateId(broken))
                            .map(|block| block.block.id.as_str().to_owned());
                        let tool = self
                            .players
                            .get(&intent.actor)
                            .and_then(|player| {
                                player
                                    .inventory
                                    .slots
                                    .get(usize::from(player.inventory.selected.min(8)))
                            })
                            .map(|tool| crate::advancements::item_snapshot(self, tool));
                        crate::advancements::trigger(
                            self,
                            intent.actor,
                            "minecraft:bee_nest_destroyed",
                            serde_json::json!({"block":block,"item":tool,"num_bees_inside":bees}),
                        );
                    }
                    self.record_block_mined(intent.actor, broken);
                    self.apply_player_tool_wear(intent.actor, wear);
                    self.stage_container_removal(
                        *pos,
                        !crate::block_drop_context::retains_contents(&drops),
                    );
                    self.merge_broken
                        .push((intent.actor, *pos, broken, drops, xp));
                    mining::consume(self, intent.actor, *pos, broken);
                }
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
            }
            IntentKind::UseBlock { pos, hand } => {
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if let Some(hand) = hand
                    && let Some((_, after)) = intent
                        .footprint
                        .block_write
                        .iter()
                        .find(|(cell, _)| cell == pos)
                    && self
                        .registry
                        .blocks
                        .by_id(mc_world::BlockStateId(*after))
                        .is_some_and(|block| block.block.id.path() == "respawn_anchor")
                {
                    let item = self.players.get(&intent.actor).map(|player| {
                        player.inventory.slots[held_slot_index(player, *hand)].clone()
                    });
                    crate::advancements::accepted_block(
                        self,
                        intent.actor,
                        "minecraft:item_used_on_block",
                        *pos,
                        Some(*after),
                        item,
                    );
                }
            }
            IntentKind::Bucket { pos, give } => {
                // Emptying into a washable block (grass, flowers) drops it.
                if let Some(old) = self.world.block_at(pos.x, pos.y, pos.z)
                    && old != self.registry.states.air
                    && crate::fluids::fluid_at(&self.registry.states, old).is_none()
                    && intent.footprint.block_write.iter().any(|(cell, new)| {
                        *cell == *pos
                            && crate::fluids::fluid_at(&self.registry.states, *new).is_some()
                    })
                {
                    let wear = self.block_break_tool_wear(intent.actor, old);
                    let Ok(wear) = self.player_tool_wear_plan(intent.actor, wear) else {
                        return false;
                    };
                    let xp = self.block_break_experience(intent.actor, old);
                    let rolled =
                        crate::block_drop_context::player_drops(self, intent.actor, *pos, old);
                    let Some(drops) = crate::block_drop_context::admit(self, *pos, rolled) else {
                        return false;
                    };
                    self.record_block_mined(intent.actor, old);
                    self.apply_player_tool_wear(intent.actor, wear);
                    self.stage_container_removal(
                        *pos,
                        !crate::block_drop_context::retains_contents(&drops),
                    );
                    self.merge_broken.push((intent.actor, *pos, old, drops, xp));
                }
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if let Some(item) = give {
                    self.merge_gives.push((intent.actor, *item));
                }
                if let Some(item) =
                    give.or_else(|| intent.footprint.inv.iter().find_map(|claim| claim.put_item))
                    && self
                        .registry
                        .items
                        .name_of(item)
                        .is_some_and(|id| id.path() != "bucket")
                {
                    let evidence =
                        crate::advancements::item_snapshot(self, &mc_data::ItemStack::new(item, 1));
                    crate::advancements::trigger(
                        self,
                        intent.actor,
                        "minecraft:filled_bucket",
                        serde_json::json!({"item":evidence}),
                    );
                }
            }
            IntentKind::Cultivate { pos, wear } => {
                let Ok(plan) = self.player_tool_wear_plan(intent.actor, *wear) else {
                    return false;
                };
                crate::farming::accepted_use(self, intent, *pos);
                self.apply_player_tool_wear(intent.actor, plan);
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
            }
            IntentKind::TrampleFarmland => {
                self.merge_block_writes
                    .extend_from_slice(&intent.footprint.block_write);
            }
            IntentKind::BlockTool { wear, give } => {
                let Ok(plan) = self.player_tool_wear_plan(intent.actor, *wear) else {
                    return false;
                };
                self.apply_player_tool_wear(intent.actor, plan);
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if let Some(item) = give {
                    self.merge_gives.push((intent.actor, *item));
                }
            }
            IntentKind::ComposterFill { pos, maturing } => {
                self.merge_block_writes
                    .extend_from_slice(&intent.footprint.block_write);
                crate::composter::schedule_fill(self, *pos, *maturing);
            }
            IntentKind::PlaceBlock { pos, state } => {
                let staged = self.merge_block_writes.len();
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if crate::bees::place_from_item(self, intent).is_err()
                    || crate::shulker::place_from_item(self, intent).is_err()
                {
                    self.merge_block_writes.truncate(staged);
                    return false;
                }
                crate::block_entity_defaults::accepted_placement(self, *pos, *state);
                let held = intent
                    .footprint
                    .inv
                    .iter()
                    .find_map(|claim| {
                        self.players
                            .get(&claim.owner)
                            .and_then(|player| player.inventory.slots.get(claim.slot as usize))
                    })
                    .cloned();
                crate::advancements::accepted_block(
                    self,
                    intent.actor,
                    "minecraft:placed_block",
                    *pos,
                    Some(*state),
                    held.clone(),
                );
                crate::advancements::accepted_block(
                    self,
                    intent.actor,
                    "minecraft:item_used_on_block",
                    *pos,
                    Some(*state),
                    held,
                );
                crate::plugin::block_event(self, intent.actor, *pos, *state, true);
                if self
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(*state))
                    .is_some_and(crate::signs::editable)
                    && let Some(connection) = self.connections.entity_connection(intent.actor)
                {
                    self.sign_editors.insert(
                        connection,
                        crate::signs::Editor {
                            position: *pos,
                            block: *state,
                            opened_tick: self.tick,
                            front: true,
                        },
                    );
                    self.push_player(
                        connection,
                        PlayerChange::SignEditorOpened {
                            position: *pos,
                            front: true,
                            block: *state,
                        },
                    );
                }
            }
            IntentKind::SpawnChild {
                pos,
                type_id,
                health,
            } => {
                self.merge_spawns.push(MergeSpawn {
                    pos: *pos,
                    type_id: *type_id,
                    health: *health,
                    wool: None,
                    taming: None,
                    advancement_breeding: None,
                });
            }
            IntentKind::Breed {
                partner,
                child_pos,
                child_type,
            } => {
                crate::entity_life::breed(self, intent.actor, *partner, *child_pos, *child_type);
            }
            IntentKind::FeedAnimal { target } => {
                crate::entity_life::feed(self, *target);
                if let Some(actor) = self.entities.slot_of(intent.actor)
                    && let Some(target) = self.entities.slot_of(*target)
                {
                    self.entities.row_mut(target).entity_life.taming.love_cause =
                        Some(self.entities.row(actor).uuid);
                }
                // Stacked tropical-fish buckets fed to an adult axolotl: the
                // water-bucket remainder goes to the inventory (overflow drops
                // at the player), mirroring bucket fills.
                if let Some(claim) = intent.footprint.inv.first()
                    && claim.take > 0
                    && claim.expect_count > 1
                    && let Some(slot) = self.entities.slot_of(*target)
                    && crate::entity_life::adult(self.entities.row(slot))
                    && let crate::entities::EntityKind::Mob { type_id } =
                        self.entities.body(slot).kind
                    && self
                        .registry
                        .blueprint(type_id)
                        .is_some_and(|blueprint| blueprint.name == "minecraft:axolotl")
                    && self
                        .registry
                        .items
                        .name_of(claim.expect_item)
                        .is_some_and(|name| name.as_str() == "minecraft:tropical_fish_bucket")
                    && let Some(water) = self.registry.items.id_of(
                        &mc_data::Identifier::parse("minecraft:water_bucket")
                            .expect("vanilla item"),
                    )
                {
                    self.merge_gives.push((intent.actor, water));
                }
            }
            IntentKind::DolphinFeed { plan } => {
                crate::dolphin::feed(self, plan);
            }
            IntentKind::HarvestAnimal { plan } => {
                crate::entity_life::apply(self, intent.actor, plan);
                return true;
            }
            IntentKind::MountInteract { plan } => {
                crate::mounts::apply(self, intent.actor, plan);
                return true;
            }
            IntentKind::Explode {
                center,
                power,
                mob_grief,
                kill_credit,
            } => {
                crate::explosion::apply(
                    self,
                    Some(intent.actor),
                    *center,
                    *power,
                    *mob_grief,
                    *kill_credit,
                );
            }
            IntentKind::EnvironmentalExplosion { center, power } => {
                for (pos, block) in &intent.footprint.block_write {
                    self.write_block(*pos, *block);
                }
                crate::explosion::apply(self, None, *center, *power, None, None);
            }
            IntentKind::SetRespawn { pos } => {
                let rotation = self
                    .entities
                    .slot_of(intent.actor)
                    .map(|slot| self.entities.rot(slot));
                if let Some(player) = self.players.get_mut(&intent.actor) {
                    player.respawn_point = Some([pos.x, pos.y, pos.z]);
                    player.respawn_dimension = self.dimension;
                    player.respawn_forced = false;
                    player.respawn_yaw = rotation.map_or(0.0, |rotation| rotation[0]);
                    player.respawn_pitch = 0.0;
                }
            }
        }
        crate::vibration::intent(self, intent);
        // Inventory claims apply with the intent, whole.
        for claim in &intent.footprint.inv {
            if let Some(player) = self.players.get_mut(&claim.owner)
                && let Some(slot) = player.inventory.slots.get_mut(claim.slot as usize)
            {
                if matches!(
                    intent.kind,
                    IntentKind::PlaceBlock { .. }
                        | IntentKind::Bucket { .. }
                        | IntentKind::Cultivate { .. }
                        | IntentKind::BlockTool { .. }
                        | IntentKind::UseBlock { .. }
                        | IntentKind::ComposterFill { .. }
                ) && claim.owner == intent.actor
                {
                    player
                        .statistics
                        .increment(StatType::Used, claim.expect_item, 1);
                }
                if slot.is_empty() {
                    if let Some(put) = claim.put_item {
                        *slot = mc_data::ItemStack::new(put, claim.put_count);
                    }
                } else {
                    let rest = slot.count - claim.take;
                    if rest <= 0 && claim.put_item.is_none() {
                        *slot = mc_data::ItemStack::EMPTY;
                    } else if let Some(put) = claim.put_item {
                        *slot = mc_data::ItemStack::new(put, claim.put_count);
                    } else if rest <= 0 {
                        *slot = mc_data::ItemStack::EMPTY;
                    } else {
                        slot.count = rest;
                    }
                }
            }
        }
        // Winner pays visibly: sync the inventory after the debit above.
        if matches!(
            intent.kind,
            IntentKind::PlaceBlock { .. }
                | IntentKind::FeedAnimal { .. }
                | IntentKind::Bucket { .. }
                | IntentKind::Cultivate { .. }
                | IntentKind::ComposterFill { .. }
        ) {
            let sync = self
                .players
                .get(&intent.actor)
                .map(|player| player.inventory_contents());
            if let (Some(contents), Some(connection)) =
                (sync, self.connections.by_entity.get(&intent.actor).copied())
            {
                self.push_player(connection, contents);
            }
        }
        true
    }

    /// Merge: sort intents, validate each footprint whole, apply or reject.
    /// Returns accepted count.
    pub fn merge(&mut self, intents: &mut Vec<Intent>) -> usize {
        #[cfg(test)]
        let sort_start = crate::horde_bench::merge_cost_tests::start();
        sort_intents(intents);
        #[cfg(test)]
        crate::horde_bench::merge_cost_tests::record(sort_start, |probe, ns| {
            probe.sort_ns += ns;
            probe.intents += intents.len();
        });
        let staged = std::mem::take(intents);
        let mut accepted = 0;
        let mut interactions = std::mem::take(&mut self.plugin_interactions);
        for intent in &staged {
            let writes = self.merge_block_writes.len();
            if let IntentKind::AuthorizedBuild { authorization } = &intent.kind {
                if crate::build_precommit::commit_authorized(
                    self,
                    authorization,
                    intent.seq,
                    |state, native| {
                        state.footprint_valid(&native.footprint) && state.kind_valid(native)
                    },
                    Self::apply_intent,
                ) {
                    accepted += 1;
                    crate::block_log::note_merge(self, intent.actor, writes);
                    crate::plugin::enqueue_result(
                        self,
                        interactions.remove(&(intent.actor, intent.seq)),
                    );
                }
                continue;
            }
            #[cfg(test)]
            let validation_start = crate::horde_bench::merge_cost_tests::start();
            let valid = self.footprint_valid(&intent.footprint) && self.kind_valid(intent);
            #[cfg(test)]
            crate::horde_bench::merge_cost_tests::record(validation_start, |probe, ns| {
                probe.validation_ns += ns;
            });
            if valid && self.apply_intent(intent) {
                accepted += 1;
                crate::block_log::note_merge(self, intent.actor, writes);
                crate::plugin::enqueue_result(
                    self,
                    interactions.remove(&(intent.actor, intent.seq)),
                );
            } else if matches!(
                intent.kind,
                IntentKind::PlaceBlock { .. }
                    | IntentKind::FeedAnimal { .. }
                    | IntentKind::Cultivate { .. }
                    | IntentKind::ComposterFill { .. }
            ) {
                // Rejected placement/feed paid nothing; resync the held slot
                // so client-side prediction snaps back.
                let sync = self
                    .players
                    .get(&intent.actor)
                    .map(|player| player.inventory_contents());
                if let (Some(contents), Some(connection)) =
                    (sync, self.connections.by_entity.get(&intent.actor).copied())
                {
                    self.push_player(connection, contents);
                }
            }
        }
        #[cfg(test)]
        crate::horde_bench::merge_cost_tests::record(
            crate::horde_bench::merge_cost_tests::start(),
            |probe, _| probe.accepted += accepted,
        );
        accepted
    }

    /// Commit staged merge effects: damage, pickups, spawns, animal harvesting,
    /// block drops. Returns `(deaths, events, spawned_item_entities)`.
    /// Death drops (player inventory + death containers) ride the returned
    /// spawn list so `run_tick` journals them in the same tick; committed
    /// full-pickup removals ride the fourth element so `run_tick` journals
    /// their tombstones in the same tick.
    pub fn commit_merge_effects(
        &mut self,
    ) -> (
        Vec<EntityId>,
        Vec<EntityEvent>,
        Vec<EntityId>,
        Vec<EntityId>,
    ) {
        super::refresh_player_attributes(self, false);
        let (deaths, mut events, mut spawned) = self.commit_damage();
        crate::entity_life::commit_drops(self, &mut spawned);
        // Pickups: move item counts into the collector's inventory. Only a
        // fully-consumed ground stack is queued for removal; a partial or
        // zero-capacity pickup keeps its reservation-free remainder on the
        // ground (the `merge_consumed_items` reservation is commit-scoped:
        // commit drains it into removals only for empty remainders).
        let pickups = std::mem::take(&mut self.merge_pickups);
        let mut committed_removals: Vec<EntityId> = Vec::new();
        for (item_slot, collector_slot, _count) in pickups {
            if (item_slot.0 as usize) >= self.entities.len()
                || (collector_slot.0 as usize) >= self.entities.len()
            {
                continue;
            }
            let item_id = self.entities.body(item_slot).id;
            let collector_id = self.entities.body(collector_slot).id;
            if !self.entities.body(collector_slot).alive || self.entity_session_locked(collector_id)
            {
                continue;
            }
            let item_row = self.entities.row(item_slot).clone();
            if let Some(stack) = item_row.item
                && let Some(player) = self.players.get_mut(&collector_id)
            {
                let rules = self.registry.item_rules();
                let mut moving = stack.full_stack();
                let before = moving.count;
                player.inventory.add(&rules, &mut moving);
                let taken = before - moving.count;
                if taken > 0 {
                    player
                        .statistics
                        .increment(StatType::PickedUp, stack.item_id, taken);
                }
                if moving.is_empty() {
                    committed_removals.push(item_id);
                } else {
                    // Partial pickup: the remainder stays on the ground.
                    let row = self.entities.row_mut(item_slot);
                    if let Some(slot) = row.item.as_mut() {
                        slot.count = moving.count;
                        slot.stack = Some(moving);
                    }
                    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
                    appearance.item = row.item.as_ref().map(|slot| slot.full_stack());
                    self.appearance_changed.push(item_id);
                }
                events.push(EntityEvent::Pickup {
                    item: item_id,
                    collector: collector_id,
                    count: taken.clamp(0, 255) as u8,
                });
                crate::plugin::pickup_event(self, collector_id, stack.item_id, taken);
                if taken > 0
                    && !self.registry.tags.contains_raw_id(
                        "minecraft:item",
                        "minecraft:dampens_vibrations",
                        stack.item_id as i32,
                    )
                {
                    let pos = self.entities.pos(collector_slot);
                    crate::vibration::emit(
                        self,
                        pos,
                        crate::vibration::GameEvent::EntityInteract,
                        Some(collector_id),
                    );
                }
                // Slot sync for the collector's inventory.
                if let Some(connection) = self.connections.by_entity.get(&collector_id).copied()
                    && let Some(player) = self.players.get(&collector_id)
                {
                    let burst = player.inventory_contents();
                    self.push_player(connection, burst);
                }
            }
            // No inventory (mob collector or despawned slot): the entity is
            // NOT removed — nothing took custody. It stays on the ground.
        }
        // Containers take custody before this commit. Their fully consumed
        // rows need the same removal/tombstone path as a player pickup;
        // reservations alone never delete a nonempty remainder.
        for slot in &self.merge_consumed_items {
            if (slot.0 as usize) < self.entities.len() {
                let body = self.entities.body(*slot);
                let row = self.entities.row(*slot);
                if matches!(body.kind, EntityKind::Item { .. })
                    && row.item.as_ref().is_some_and(|item| item.count <= 0)
                {
                    committed_removals.push(body.id);
                }
            }
        }
        self.merge_consumed_items.clear();
        committed_removals.extend(self.projectiles.pending_arrow_removals.drain());
        // Remove committed full pickups (by id: slots may have shifted under
        // partial-pickup writes) and the dead (by id, dead-inclusive: the
        // death flag above set alive=false before this lookup). Highest
        // slots first so swap-remove never invalidates a pending slot.
        let mut dead_slots: Vec<EntitySlot> = committed_removals
            .iter()
            .filter_map(|id| self.entities.slot_of_any(*id))
            .collect();
        crate::projectiles::queue_owned_pearls_on_deaths(self, &deaths, &mut dead_slots);
        crate::vehicles::queue_expired_removals(self, &mut dead_slots);
        // Dead mobs/items leave the store (tombstoned via `removed`); dead
        // PLAYERS stay addressable (alive=false row kept) so Respawn can
        // revive and persistence journals the emptied post-image. Their
        // drops still spawned above; only the row removal is skipped.
        for id in &deaths {
            let Some(slot) = self.entities.slot_of_any(*id) else {
                continue;
            };
            let is_player = matches!(
                self.entities.body(slot).kind,
                crate::entities::EntityKind::Player { .. }
            );
            if is_player {
                continue;
            }
            dead_slots.push(slot);
        }
        dead_slots.sort_by_key(|slot| std::cmp::Reverse(slot.0));
        dead_slots.dedup();
        let mut removed: Vec<EntityId> = Vec::with_capacity(dead_slots.len());
        for slot in dead_slots {
            if (slot.0 as usize) < self.entities.len() {
                removed.push(self.entities.body(slot).id);
                self.remove_entity(slot);
            }
        }
        // Spawns.
        let spawns = std::mem::take(&mut self.merge_spawns);
        for spawn in spawns {
            let id = self.spawn_mob(spawn.type_id, spawn.pos, spawn.health);
            crate::entity_life::make_child(self, id);
            if let Some(taming) = spawn.taming
                && let Some(slot) = self.entities.slot_of(id)
            {
                let (body, row) = self.entities.parts_mut(slot);
                row.entity_life.tamed = taming.owner.is_some();
                row.entity_life.taming = taming;
                crate::entity_life::restore_appearance(body, row);
            }
            if let Some(wool) = spawn.wool
                && let Some(slot) = self.entities.slot_of(id)
            {
                set_wool(
                    Arc::make_mut(&mut self.entities.row_mut(slot).appearance),
                    wool,
                );
            }
            if let Some((player, mut evidence)) = spawn.advancement_breeding {
                evidence["child"] = crate::advancements::entity_snapshot(self, id);
                crate::advancements::trigger(self, player, "minecraft:bred_animals", evidence);
            }
            spawned.push(id);
            let xp = 1 + ((self.tick ^ id.0 as u64) % 7) as i32;
            crate::experience_orbs::award(self, spawn.pos, xp, &mut spawned);
        }
        // Block drops: survival breaks spawn loot at the block centre;
        // Accepted breaks retain pre-wear loot and experience; wear was paid at admission.
        for (actor, pos, broken, drops, xp) in std::mem::take(&mut self.merge_broken) {
            self.commit_break_drops(actor, pos, broken, drops, xp, &mut spawned);
            crate::plugin::block_event(self, actor, pos, broken, false);
        }
        for (pos, _broken, drops) in std::mem::take(&mut self.merge_command_broken) {
            self.spawn_block_loot(pos, drops, pos.x as u64 ^ pos.z as u64, &mut spawned);
        }
        self.commit_gives(&mut spawned);
        // Container breaks: spawn each broken container's contents as drops
        // at the block centre (journaled via `spawned`), remove its storage,
        // mark the chunk dirty, stage the `RemoveBlockEntity` tombstone.
        for ((x, y, z), slots) in std::mem::take(&mut self.merge_container_contents) {
            let centre = [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5];
            for stack in slots {
                if stack.is_empty() {
                    continue;
                }
                let drop = spawn_item_entity(self, centre, stack);
                spawned.push(drop);
            }
            if let Some(mut furnace) = self
                .block_entities
                .remove(&(x, y, z))
                .and_then(|record| record.furnace)
            {
                let award = furnace.take_experience(&self.registry.item_rules(), self.tick);
                crate::experience_orbs::award(self, centre, award, &mut spawned);
            }
            self.brewing.remove(&(x, y, z));
            self.crafters.remove(&(x, y, z));
            self.lecterns.remove(&(x, y, z));
            let pos = BlockPos { x, y, z };
            if self
                .world
                .chunk(ChunkPos {
                    x: x.div_euclid(16),
                    z: z.div_euclid(16),
                })
                .is_some_and(|chunk| chunk.block_entities.contains_key(&pos))
            {
                self.world.set_opaque_block_entity(pos, None);
                self.opaque_changes.push((pos, None));
            }
            self.world.mark_block_entity_dirty(BlockPos { x, y, z });
            self.block_entity_removed.push((x, y, z));
        }
        for (pos, image) in std::mem::take(&mut self.merge_command_block_entities) {
            crate::command_block_entity::install(self, pos, image);
        }
        sync_chunk_block_entities(self);
        // Close every viewer of a broken container with a MenuClosed.
        for (viewer, (x, y, z)) in std::mem::take(&mut self.merge_closed_viewers) {
            if self.open_containers.remove(&viewer).is_none() {
                continue;
            }
            let centre = [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5];
            let connection = self.connections.entity_connection(viewer);
            // Capture the open window id BEFORE `close()` takes the menu
            // (the client needs the real window, not a constant).
            let window = self
                .players
                .get(&viewer)
                .and_then(|player| player.open.as_ref())
                .map(|menu| menu.window);
            if let Some(player) = self.players.get_mut(&viewer) {
                let rules = self.registry.item_rules();
                let effects = player.close(&rules);
                if let Some(connection) = connection {
                    for change in effects.changes {
                        self.push_player(connection, change);
                    }
                }
                for stack in effects.dropped {
                    let drop = spawn_item_entity(self, centre, stack);
                    spawned.push(drop);
                }
            }
            if let (Some(connection), Some(window)) = (connection, window) {
                self.push_player(connection, PlayerChange::MenuClosed { window });
            }
        }
        self.seq_counter.clear();
        // NOTE: `block_entity_removed` is NOT cleared here: `run_tick` drains
        // it after commit to emit `RemoveBlockEntity` tombstones in the same
        // tick's journal batch (see run_tick: drained via `std::mem::take`,
        // so no stale tombstones leak into later ticks).
        (deaths, events, spawned, removed)
    }

    pub(super) fn player_tool_wear_plan(
        &self,
        actor: EntityId,
        amount: i32,
    ) -> Result<Option<crate::player::NativeToolWearPlan>, ()> {
        let Some(player) = self.players.get(&actor) else {
            return Ok(None);
        };
        let plan = player.plan_held_tool_wear(&self.registry.item_rules(), amount);
        if crate::plugin_inventories::below_player_debit_floors(
            self,
            actor,
            &[],
            plan.broken_slot(),
        ) {
            return Err(());
        }
        Ok(Some(plan))
    }

    pub(super) fn apply_player_tool_wear(
        &mut self,
        actor: EntityId,
        plan: Option<crate::player::NativeToolWearPlan>,
    ) {
        if let Some(plan) = plan {
            let player = self
                .players
                .get_mut(&actor)
                .expect("prepared native tool owner");
            let connection = player.connection;
            let effects = player.apply_tool_wear(plan);
            for change in effects.changes {
                self.push_player(connection, change);
            }
        }
    }

    pub(super) fn record_block_mined(&mut self, actor: EntityId, broken: u32) {
        if self.connections.by_entity.contains_key(&actor)
            && let Some(player) = self.players.get_mut(&actor)
            && !player.creative()
            && let Some(block) = self.registry.blocks.by_id(mc_world::BlockStateId(broken))
        {
            let rules = self.registry.item_rules();
            let held = &player.inventory.slots[usize::from(player.inventory.selected.min(8))];
            if !self
                .registry
                .mining
                .requires_correct_tool_for_drops(broken)
                .unwrap_or(false)
                || crate::block_drops::correct_tool(&rules, &block.block, held)
            {
                player
                    .statistics
                    .increment(StatType::Mined, block.block.raw_id as u32, 1);
            }
        }
    }

    pub(super) fn block_break_tool_wear(&self, actor: EntityId, broken: u32) -> i32 {
        if self.registry.mining.destroy_speed(broken) == Some(0.0) {
            return 0;
        }
        let Some(player) = self.players.get(&actor) else {
            return 0;
        };
        let rules = self.registry.item_rules();
        let held = &player.inventory.slots[usize::from(player.inventory.selected.min(8))];
        rules
            .facts
            .facts_for_stack(held, rules.items)
            // The native tool component defaults omitted damage_per_block to
            // one; only a missing tool component or explicit zero skips wear.
            .and_then(|facts| facts.tool.as_ref())
            .map_or(0, |tool| {
                i32::try_from(tool.damage_per_block.unwrap_or(1)).unwrap_or(i32::MAX)
            })
    }

    pub(crate) fn block_break_experience(&self, actor: EntityId, broken: u32) -> i32 {
        let Some(player) = self.players.get(&actor) else {
            return 0;
        };
        if player.creative()
            || !crate::command_rules::boolean(self, crate::command_rules::BoolRule::BLOCK_DROPS)
        {
            return 0;
        }
        let rules = self.registry.item_rules();
        let held = &player.inventory.slots[usize::from(player.inventory.selected.min(8))];
        crate::block_drops::block_experience(
            &crate::block_drops::BlockDropRules {
                blocks: &self.registry.blocks,
                loot: &self.registry.loot,
                mining: &self.registry.mining,
            },
            &rules,
            broken,
            held,
            self.tick ^ actor.0 as u64,
        )
    }

    /// Drops and pre-wear experience for one accepted break. Spawns loot as item
    /// entities (vanilla pickup delay, small deterministic velocity).
    pub(super) fn commit_break_drops(
        &mut self,
        actor: EntityId,
        pos: BlockPos,
        broken: u32,
        drops: Vec<mc_data::ItemStack>,
        xp: i32,
        spawned: &mut Vec<EntityId>,
    ) {
        let creative_shulker = self
            .players
            .get(&actor)
            .is_some_and(|player| player.creative())
            && self
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(broken))
                .is_some_and(|block| crate::shulker::is_block(block.block.id.path()))
            && crate::block_drop_context::retains_contents(&drops);
        if creative_shulker {
            // Native playerWillDestroy spawns collected box components directly,
            // independent of the ordinary block-loot gamerule.
            let centre = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];
            for stack in drops {
                spawned.push(spawn_item_entity(self, centre, stack));
            }
        } else {
            self.spawn_block_loot(pos, drops, actor.0 as u64, spawned);
        }

        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        crate::experience_orbs::award(self, centre, xp, spawned);
    }

    /// Spawns block loot at the block centre as item entities (vanilla
    /// pickup delay, small deterministic scatter from `salt`).
    pub(crate) fn spawn_block_loot(
        &mut self,
        pos: BlockPos,
        drops: Vec<mc_data::ItemStack>,
        seed: u64,
        spawned: &mut Vec<EntityId>,
    ) {
        if !crate::command_rules::boolean(self, crate::command_rules::BoolRule::BLOCK_DROPS) {
            return;
        }
        let centre = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];
        for (index, stack) in drops.into_iter().enumerate() {
            let id = self.entities.alloc_id();
            let uuid = self.entity_uuid(id);
            let type_id = self.registry.item_type_id;
            // Deterministic scatter (no RNG on the tick path).
            let scatter = |salt: u64| {
                let hash = seed
                    .wrapping_mul(0xBF58_476D_1CE4_E5B9)
                    .wrapping_add(self.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15))
                    .wrapping_add(salt)
                    .wrapping_add(index as u64);
                (hash as f64 / u64::MAX as f64 - 0.5) * 0.4
            };
            let appearance = std::sync::Arc::new(EntityAppearance {
                item: Some(stack.clone()),
                health: 1.0,
                ..Default::default()
            });
            self.entities.spawn(
                id,
                uuid,
                EntityKind::Item { type_id },
                ColdFields {
                    pos: centre,
                    vel: [scatter(1), 0.1, scatter(2)],
                    yaw: 0.0,
                    pitch: 0.0,
                    ground: false,
                    health: 1.0,
                    breed_cooldown: 0,
                    pickup_delay: 10,
                    item: Some(crate::entities::StackRow {
                        item_id: stack.item_id,
                        count: stack.count,
                        stack: Some(stack),
                    }),
                    appearance,
                    movement: crate::blueprint::MovementClass::Item,
                    half_width: 0.125,
                    height: 0.25,
                    speed: 0.0,
                    attack_damage: 0.0,
                    sight_range: 0.0,
                    attack_reach: 0.0,
                },
            );
            spawned.push(id);
        }
    }

    /// New buckets from a stacked bucket: into the inventory (vanilla
    /// `ItemUtils.createFilledResult`), the overflow drops at the player.
    pub(super) fn commit_gives(&mut self, spawned: &mut Vec<EntityId>) {
        for (actor, item) in std::mem::take(&mut self.merge_gives) {
            let rules = self.registry.item_rules();
            let mut stack = mc_data::ItemStack::new(item, 1);
            let Some(player) = self.players.get_mut(&actor) else {
                continue;
            };
            player.inventory.add(&rules, &mut stack);
            let contents = player.inventory_contents();
            if let Some(connection) = self.connections.entity_connection(actor) {
                self.push_player(connection, contents);
            }
            if !stack.is_empty()
                && let Some(slot) = self.entities.slot_of(actor)
            {
                let pos = self.entities.pos(slot);
                let drop = spawn_item_entity(self, pos, stack);
                spawned.push(drop);
            }
        }
    }

    /// Loot of a block that popped or washed away (no tool).
    pub(crate) fn pop_block_loot(
        &mut self,
        pos: BlockPos,
        block: u32,
        spawned: &mut Vec<EntityId>,
    ) -> bool {
        let drops =
            if crate::command_rules::boolean(self, crate::command_rules::BoolRule::BLOCK_DROPS) {
                crate::block_drop_context::roll(
                    self,
                    pos,
                    block,
                    &mc_data::ItemStack::EMPTY,
                    self.tick ^ ((pos.x as u64) << 40) ^ ((pos.z as u64) << 16) ^ pos.y as u64,
                )
            } else {
                Ok(Vec::new())
            };
        let Some(drops) = crate::block_drop_context::admit(self, pos, drops) else {
            return false;
        };
        self.stage_container_removal(pos, !crate::block_drop_context::retains_contents(&drops));
        self.spawn_block_loot(pos, drops, pos.x as u64 ^ pos.z as u64, spawned);
        true
    }
}
