//! Byte bound for the journal work queue.
//!
//! The queue's count cap alone does not bound memory: a TNT tick's `Append` is
//! 12-28 MB, so 128 queued batches could pin gigabytes behind a stalled disk.
//! The tick thread reserves each batch's estimated bytes before `send` and
//! blocks while the in-flight total would exceed [`JOURNAL_QUEUE_BYTES`]; the
//! journal thread releases exactly that number once `append` returned. A
//! single batch larger than the whole budget is admitted alone. Nothing is
//! dropped or reordered; the budget only parks the sender.
//!
//! The estimate is a conservative overestimate, not an allocator-exact bound:
//! growing payloads are walked by length/capacity (no serialization on the
//! tick thread) and only fixed-size records get a flat allowance; `Identifier`
//! spare capacity is not exposed, so it is priced by text length plus an
//! allowance. Scope: queued `Append` payload only. `Checkpoint` jobs are
//! bounded by the single-in-flight checkpoint rule and `SaveGenerated` by
//! `generated_busy` (one outstanding), not by this budget.

use std::sync::{Condvar, Mutex};

use crate::journal::{EntityAppearanceImage, EntityDataValueImage, EntityStateImage, JournalEntry};

/// In-flight `Append` payload cap (queued plus the batch being written).
pub(crate) const JOURNAL_QUEUE_BYTES: usize = 256 << 20;

/// Flat allowance for the heap behind one fixed-size nested record (a
/// transient cache, timers, a furnace image, ...). Growing payloads are never
/// priced by this: they are walked by length/capacity.
pub(crate) const NESTED_STATE_ALLOWANCE: usize = 1024;

/// Fixed per-element bookkeeping allowance for ordered maps and sets.
const NODE: usize = 64;

/// Worst-case slot slack of a `BTreeMap`/`BTreeSet`: nodes hold 11 slots and
/// stay at least ~5/11 full, so inline key+value bytes can occupy ~2.2x per
/// logical element; priced at 3x.
const TREE_SLACK: usize = 3;

/// Estimated bytes one tree element occupies for its inline `(key, value)`.
pub(crate) fn tree_slot(inline: usize) -> usize {
    inline * TREE_SLACK + NODE
}

/// A tree's root node is a full 11-slot allocation however few elements it
/// holds (exempt from the min-fill rule): added once per non-empty tree.
pub(crate) fn tree_root(len: usize, inline: usize) -> usize {
    if len == 0 { 0 } else { 11 * inline }
}

/// Allowance for the heap behind one `Identifier`.
const IDENTIFIER_HEAP: usize = 64;

/// Estimated resident bytes of a batch: the entry slots plus the heap each
/// entry owns (see [`entry_heap`]). Sized to over- rather than under-count.
pub(crate) fn entries_bytes(entries: &[JournalEntry]) -> usize {
    std::mem::size_of_val(entries) + entries.iter().map(entry_heap).sum::<usize>()
}

/// Heap behind a JSON value (plugin domain state, advancement progress),
/// walked by capacities and lengths.
pub(crate) fn value_heap(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
        Value::String(text) => text.capacity(),
        Value::Array(items) => {
            items.capacity() * std::mem::size_of::<Value>()
                + items.iter().map(value_heap).sum::<usize>()
        }
        Value::Object(map) => {
            let inline = std::mem::size_of::<(String, Value)>();
            tree_root(map.len(), inline)
                + map
                    .iter()
                    .map(|(key, value)| key.capacity() + tree_slot(inline) + value_heap(value))
                    .sum::<usize>()
        }
    }
}

/// Heap behind an identifier: its text plus a node allowance (spare capacity
/// is not exposed by `Identifier`).
pub(crate) fn ident_heap(id: &mc_data::Identifier) -> usize {
    id.as_str().len() + IDENTIFIER_HEAP
}

fn potion_effect_heap(effect: &mc_data::item_stack::PotionEffect) -> usize {
    // Walk the hidden-effect chain (each link is a boxed record).
    let mut total = ident_heap(&effect.id);
    let mut hidden = effect.hidden.as_deref();
    while let Some(link) = hidden {
        total += std::mem::size_of_val(link) + ident_heap(&link.id);
        hidden = link.hidden.as_deref();
    }
    total
}

fn components_heap(components: &mc_data::item_stack::ExtendedItemComponents) -> usize {
    std::mem::size_of_val(components)
        + components.potion.as_ref().map_or(0, ident_heap)
        + components.potion_custom_effects.capacity()
            * std::mem::size_of::<mc_data::item_stack::PotionEffect>()
        + components
            .potion_custom_effects
            .iter()
            .map(potion_effect_heap)
            .sum::<usize>()
        + components
            .potion_custom_name
            .as_ref()
            .map_or(0, String::capacity)
        + components.trim.as_ref().map_or(0, |trim| {
            ident_heap(&trim.material) + ident_heap(&trim.pattern)
        })
        + components.banner_patterns.capacity()
            * std::mem::size_of::<mc_data::item_stack::BannerPattern>()
        + components
            .banner_patterns
            .iter()
            .map(|banner| ident_heap(&banner.pattern))
            .sum::<usize>()
        + components.stored_enchantments.capacity()
            * std::mem::size_of::<mc_data::ItemEnchantment>()
        + components
            .stored_enchantments
            .iter()
            .map(|enchantment| ident_heap(&enchantment.id))
            .sum::<usize>()
        + stacks_heap(&components.charged_projectiles)
        + components.fireworks.as_ref().map_or(0, |fireworks| {
            fireworks.explosions.capacity()
                * std::mem::size_of::<mc_data::item_stack::FireworkExplosion>()
                + fireworks
                    .explosions
                    .iter()
                    .map(|explosion| {
                        (explosion.colors.capacity() + explosion.fade_colors.capacity())
                            * std::mem::size_of::<i32>()
                    })
                    .sum::<usize>()
        })
        + components.custom_data.as_ref().map_or(0, Vec::capacity)
        + components.preserved_components.capacity()
            * std::mem::size_of::<mc_data::item_stack::PreservedItemComponent>()
        + components
            .preserved_components
            .iter()
            .map(|component| {
                component.name.capacity()
                    + component.value.capacity()
                    + component.baseline.as_ref().map_or(0, Vec::capacity)
            })
            .sum::<usize>()
}

/// Heap behind one stack: nothing for a plain stack (the hot case).
pub(crate) fn item_heap(stack: &mc_data::ItemStack) -> usize {
    stack.enchantments.capacity() * std::mem::size_of::<mc_data::ItemEnchantment>()
        + stack
            .enchantments
            .iter()
            .map(|enchantment| ident_heap(&enchantment.id))
            .sum::<usize>()
        + stack.stew_effects.capacity() * std::mem::size_of::<mc_data::item_stack::StewEffect>()
        + stack
            .stew_effects
            .iter()
            .map(|effect| ident_heap(&effect.id))
            .sum::<usize>()
        + stack.custom_name.as_deref().map_or(0, str::len)
        + stack.item_model.as_deref().map_or(0, ident_heap)
        + stack.components.as_deref().map_or(0, components_heap)
}

/// A boxed stack's allocation plus what the stack owns.
fn boxed_item_heap(stack: &Option<Box<mc_data::ItemStack>>) -> usize {
    stack.as_deref().map_or(0, |stack| {
        std::mem::size_of::<mc_data::ItemStack>() + item_heap(stack)
    })
}

pub(crate) fn opt_item_heap(stack: &Option<mc_data::ItemStack>) -> usize {
    stack.as_ref().map_or(0, item_heap)
}

/// A stack vector's allocation plus what each stack owns.
pub(crate) fn stacks_heap(stacks: &Vec<mc_data::ItemStack>) -> usize {
    stacks.capacity() * std::mem::size_of::<mc_data::ItemStack>()
        + stacks.iter().map(item_heap).sum::<usize>()
}

/// A string key/element's heap plus its tree slot (inline key and value).
fn string_node(text: &String, value_size: usize) -> usize {
    text.capacity() + tree_slot(std::mem::size_of::<String>() + value_size)
}

fn scoreboard_heap(board: &crate::scoreboard::Scoreboard) -> usize {
    use std::mem::size_of;
    let string = size_of::<String>();
    let teams = board.teams.iter().map(|(name, team)| {
        string_node(name, size_of::<crate::scoreboard::Team>())
            + team.display_name.capacity()
            + team.prefix.capacity()
            + team.suffix.capacity()
            + tree_root(team.members.len(), string)
            + team
                .members
                .iter()
                .map(|member| string_node(member, 0))
                .sum::<usize>()
    });
    let objectives = board.objectives.iter().map(|(name, objective)| {
        string_node(name, size_of::<crate::scoreboard::Objective>())
            + objective.criteria.capacity()
            + objective.display_name.capacity()
    });
    let scores = board.scores.iter().map(|(holder, scores)| {
        string_node(
            holder,
            size_of::<std::collections::BTreeMap<String, crate::scoreboard::Score>>(),
        ) + tree_root(scores.len(), string + size_of::<crate::scoreboard::Score>())
            + scores
                .keys()
                .map(|objective| string_node(objective, size_of::<crate::scoreboard::Score>()))
                .sum::<usize>()
    });
    let slots = board
        .display_slots
        .values()
        .map(|name| string_node(name, size_of::<i32>()));
    let roots = tree_root(
        board.teams.len(),
        string + size_of::<crate::scoreboard::Team>(),
    ) + tree_root(
        board.objectives.len(),
        string + size_of::<crate::scoreboard::Objective>(),
    ) + tree_root(
        board.scores.len(),
        string + size_of::<std::collections::BTreeMap<String, crate::scoreboard::Score>>(),
    ) + tree_root(board.display_slots.len(), string + size_of::<i32>());
    roots
        + teams
            .chain(objectives)
            .chain(scores)
            .chain(slots)
            .sum::<usize>()
}
fn bossbars_heap(bytes: &Vec<u8>) -> usize {
    bytes.capacity()
}

fn raids_heap(raids: &crate::raids::Raids) -> usize {
    let uuid_inline = std::mem::size_of::<uuid::Uuid>();
    let uuid_slot = tree_slot(uuid_inline);
    tree_root(
        raids.raids.len(),
        std::mem::size_of::<(i32, crate::raids::Raid)>(),
    ) + raids
        .raids
        .values()
        .map(|raid| {
            tree_slot(std::mem::size_of::<(i32, crate::raids::Raid)>())
                + raid.status.capacity()
                + (raid.heroes.len() + raid.members.len()) * uuid_slot
                + tree_root(raid.heroes.len(), uuid_inline)
                + tree_root(raid.members.len(), uuid_inline)
        })
        .sum::<usize>()
        + tree_root(
            raids.pending.len(),
            std::mem::size_of::<(uuid::Uuid, crate::raids::PendingOmen)>(),
        )
        + raids.pending.len()
            * tree_slot(std::mem::size_of::<(uuid::Uuid, crate::raids::PendingOmen)>())
}

fn villager_heap(villager: &crate::villager::VillagerState) -> usize {
    std::mem::size_of_val(villager)
        + villager.offers.capacity() * std::mem::size_of::<crate::villager::VillagerTradeOffer>()
        + villager
            .offers
            .iter()
            .map(|offer| {
                item_heap(&offer.cost_a) + opt_item_heap(&offer.cost_b) + item_heap(&offer.result)
            })
            .sum::<usize>()
        + villager.gossips.capacity() * std::mem::size_of::<crate::villager::Gossip>()
        + villager
            .gossips
            .iter()
            .map(|gossip| gossip.kind.capacity())
            .sum::<usize>()
        + villager.household.as_ref().map_or(0, String::capacity)
        + villager.birth_origin.as_ref().map_or(0, |origin| {
            origin.parents.iter().map(String::capacity).sum::<usize>()
        })
        + villager.job_site_dimension.capacity()
        + villager.home_dimension.capacity()
        + villager.meeting_point_dimension.capacity()
        + villager.inventory.iter().map(opt_item_heap).sum::<usize>()
        + villager.navigation_path.capacity() * std::mem::size_of::<crate::boundary::BlockPos>()
        + villager.navigation_chunks.capacity()
            * std::mem::size_of::<(crate::boundary::ChunkPos, u64)>()
        // Transient caches (motion geometry, POI validation, trade work) are
        // fixed-size records: flat allowance.
        + NESTED_STATE_ALLOWANCE
}

fn projectile_heap(projectile: &crate::projectiles::ProjectileState) -> usize {
    std::mem::size_of_val(projectile)
        + item_heap(&projectile.stack)
        + projectile.hit.capacity() * std::mem::size_of::<uuid::Uuid>()
        + match &projectile.data {
            crate::projectiles::ProjectileData::Physical { definition } => ident_heap(definition),
            crate::projectiles::ProjectileData::Cloud(cloud) => {
                let inline = std::mem::size_of::<(uuid::Uuid, u64)>();
                tree_root(cloud.affected_until.len(), inline)
                    + cloud.affected_until.len() * tree_slot(inline)
            }
            _ => 0,
        }
        + projectile
            .fired_from_weapon
            .as_deref()
            .map_or(0, |stack| std::mem::size_of_val(stack) + item_heap(stack))
        + projectile.crossbow_kills.capacity() * std::mem::size_of::<serde_json::Value>()
        + projectile
            .crossbow_kills
            .iter()
            .map(value_heap)
            .sum::<usize>()
}

/// Retained statistics: sparse counters plus the preserved vanilla JSON.
fn player_statistics_heap(statistics: &mc_data::statistics::PlayerStatistics) -> usize {
    let (sparse_entries, retained) = statistics.heap_parts();
    std::mem::size_of::<mc_data::statistics::PlayerStatistics>()
        // Outer map: one inner map per stat category (bounded, fixed set).
        + NESTED_STATE_ALLOWANCE
        + sparse_entries * tree_slot(std::mem::size_of::<(u32, i32)>())
        + value_heap(retained)
}

fn effects_heap(effects: &crate::effects::ActiveEffects) -> usize {
    effects.entries.capacity() * std::mem::size_of::<crate::effects::ActiveEffect>()
        + effects
            .entries
            .iter()
            .map(|effect| {
                // Each hidden link is a boxed record.
                let mut total = 0;
                let mut hidden = effect.hidden.as_deref();
                while let Some(link) = hidden {
                    total += std::mem::size_of_val(link);
                    hidden = link.hidden.as_deref();
                }
                total
            })
            .sum::<usize>()
}

fn appearance_heap(appearance: &EntityAppearanceImage) -> usize {
    opt_item_heap(&appearance.item)
        + appearance.equipment.capacity() * std::mem::size_of::<Option<mc_data::ItemStack>>()
        + appearance
            .equipment
            .iter()
            .map(opt_item_heap)
            .sum::<usize>()
        + appearance.custom_name.as_ref().map_or(0, String::capacity)
        + appearance.extra.capacity() * std::mem::size_of::<(u8, EntityDataValueImage)>()
        + appearance
            .extra
            .iter()
            .map(|(_, value)| match value {
                EntityDataValueImage::String(text) => text.capacity(),
                EntityDataValueImage::Item(stack) => opt_item_heap(stack),
                _ => 0,
            })
            .sum::<usize>()
}

/// Heap of a retained special-mob record: only the growing payloads are walked.
fn retained_heap(retained: &crate::special_mobs::Retained) -> usize {
    match retained {
        crate::special_mobs::Retained::Warden(warden) => {
            let inline = std::mem::size_of::<(uuid::Uuid, u32)>();
            tree_root(warden.anger.len(), inline) + warden.anger.len() * tree_slot(inline)
        }
        _ => 0,
    }
}

fn vehicle_heap(vehicle: &crate::vehicles::VehicleState) -> usize {
    stacks_heap(&vehicle.slots) + vehicle.loot_table.as_ref().map_or(0, ident_heap)
}

/// An `EntityStateImage` is shared with the journal baseline; it is counted in
/// full per entry, an upper bound. Growing payloads (item stacks, strings,
/// vectors, sets) are walked by capacity or length; only genuinely fixed-size
/// records get a flat allowance. Never serialized on the tick thread.
fn state_heap(state: &EntityStateImage) -> usize {
    let life = &state.entity_life;
    std::mem::size_of::<EntityStateImage>()
        + state.opaque_entity_fields.capacity()
        + life.stew_effects.capacity() * std::mem::size_of::<mc_data::item_stack::StewEffect>()
        + life
            .stew_effects
            .iter()
            .map(|effect| ident_heap(&effect.id))
            .sum::<usize>()
        + life.vehicle.as_deref().map_or(0, |vehicle| {
            std::mem::size_of::<crate::vehicles::VehicleState>() + vehicle_heap(vehicle)
        })
        + if *life == crate::entity_life::EntityLifeState::default() {
            0
        } else {
            NESTED_STATE_ALLOWANCE
        }
        + state.loot.attributes.capacity()
            * std::mem::size_of::<crate::entity_loot::EntityAttribute>()
        + state
            .loot
            .attributes
            .iter()
            .map(|attribute| {
                attribute.id.capacity()
                    + attribute.modifiers.capacity()
                        * std::mem::size_of::<crate::entity_loot::EntityAttributeModifier>()
                    + attribute
                        .modifiers
                        .iter()
                        .map(|modifier| modifier.id.capacity() + modifier.operation.capacity())
                        .sum::<usize>()
            })
            .sum::<usize>()
        + state.loot.player_credit.as_deref().map_or(0, |credit| {
            std::mem::size_of::<crate::entity_loot::PlayerLootCredit>() + item_heap(&credit.held)
        })
        + boxed_item_heap(&state.loot.body_armor)
        + boxed_item_heap(&state.loot.saddle)
        + state.special_mob.as_ref().map_or(0, retained_heap)
        + state.allay.as_ref().map_or(0, |allay| {
            item_heap(&allay.held) + item_heap(&allay.inventory) + allay.note_dimension.capacity()
        })
}

/// Owned heap of one entry. Every variant is listed so a new one must choose
/// its accounting here.
fn entry_heap(entry: &JournalEntry) -> usize {
    use JournalEntry as E;
    match entry {
        E::Block { .. }
        | E::InhabitedTime { .. }
        | E::SimDebt { .. }
        | E::Climate { .. }
        | E::Border { .. }
        | E::Difficulty { .. }
        | E::RemoveBlockEntity { .. }
        | E::EntityVitals { .. }
        | E::RemoveEntity { .. } => 0,
        // Rare per tick: a fixed allowance for small nested state, plus direct
        // lengths where the payload can grow without bound.
        E::Plugins { value } => value.heap_estimate(),
        E::EndFight { value } => {
            value.crystals.capacity() * std::mem::size_of::<crate::end_fight::Crystal>()
                + value.gateways.capacity()
        }
        E::Raids { value } => raids_heap(value),
        E::WanderingTrader { .. } => 0,
        E::ScheduledFunctions { value } => {
            value.capacity() * std::mem::size_of::<(u64, String)>()
                + value.iter().map(|(_, name)| name.capacity()).sum::<usize>()
        }
        E::DatapackSelection { value } => {
            value.enabled.iter().map(String::len).sum::<usize>()
                + value.disabled.iter().map(String::len).sum::<usize>()
        }
        E::Settlement { image, .. } => {
            std::mem::size_of::<crate::settlements::RecordImage>()
                + image
                    .value
                    .as_ref()
                    .map_or(0, crate::settlements::SettlementRecord::heap_estimate)
        }
        E::Scoreboard { value } => scoreboard_heap(value),
        E::CommandStorage { value } => value.capacity(),
        E::SavedMap { value, .. } => {
            value.colors.capacity()
                + value.original_nbt.capacity()
                + value.banners.capacity() * std::mem::size_of::<crate::maps::Banner>()
                + value
                    .banners
                    .iter()
                    .map(|banner| {
                        banner.color.capacity() + banner.name.as_ref().map_or(0, String::capacity)
                    })
                    .sum::<usize>()
                + value
                    .decorations
                    .iter()
                    .map(|(key, decoration)| {
                        key.capacity()
                            + std::mem::size_of::<crate::maps::Decoration>()
                            + decoration.kind.capacity()
                            + decoration.name.as_ref().map_or(0, String::capacity)
                    })
                    .sum::<usize>()
        }
        E::Bossbars { value } => bossbars_heap(value),
        E::Dimension { entries, .. } => {
            entries.capacity() * std::mem::size_of::<JournalEntry>()
                + entries.iter().map(entry_heap).sum::<usize>()
        }
        E::BlockTicks { image, .. } => {
            image.ticks.capacity() * std::mem::size_of::<crate::block_ticks::BlockTickImage>()
                + image
                    .ticks
                    .iter()
                    .map(|tick| ident_heap(&tick.block))
                    .sum::<usize>()
        }
        E::FluidTicks { image } => {
            image.due.capacity() * std::mem::size_of::<crate::fluids::FluidTickImage>()
                + image
                    .due
                    .iter()
                    .map(|tick| ident_heap(&tick.fluid))
                    .sum::<usize>()
                + image.covered.capacity() * std::mem::size_of::<[i32; 2]>()
        }
        E::GameRules { values } => {
            tree_root(values.len(), 2 * std::mem::size_of::<String>())
                + values
                    .iter()
                    .map(|(name, value)| {
                        string_node(name, std::mem::size_of::<String>()) + value.capacity()
                    })
                    .sum::<usize>()
        }
        E::GeneratedEntityClaim { claim } => claim.capacity(),
        E::Player {
            uuid,
            opaque_player_fields,
            advancement_progress,
            unlocked_recipes,
            statistics,
            inventory,
            carried,
            grid,
            active_effects,
            ..
        } => {
            uuid.capacity()
                + opaque_player_fields.capacity()
                + advancement_progress.as_deref().map_or(0, value_heap)
                + unlocked_recipes.as_deref().map_or(0, |recipes| {
                    tree_root(recipes.len(), std::mem::size_of::<String>())
                        + recipes
                            .iter()
                            .map(|name| string_node(name, 0))
                            .sum::<usize>()
                })
                + player_statistics_heap(statistics)
                + stacks_heap(inventory)
                + item_heap(carried)
                + stacks_heap(grid)
                + effects_heap(active_effects)
        }
        E::Statistics {
            uuid,
            changes,
            timers,
            ..
        } => {
            uuid.capacity()
                + changes.capacity() * std::mem::size_of::<(u32, u32, i32)>()
                + if timers.is_some() {
                    NESTED_STATE_ALLOWANCE
                } else {
                    0
                }
        }
        E::BlockEntity { slots, furnace, .. } => {
            stacks_heap(slots)
                + if furnace.is_some() {
                    NESTED_STATE_ALLOWANCE
                } else {
                    0
                }
        }
        E::OpaqueBlockEntity { nbt, .. } => nbt.as_ref().map_or(0, Vec::capacity),
        E::Entity {
            uuid,
            item,
            appearance,
            active_effects,
            villager,
            state,
            projectile,
            ..
        } => {
            uuid.capacity()
                + opt_item_heap(item)
                + appearance_heap(appearance)
                + effects_heap(active_effects)
                + villager.as_deref().map_or(0, villager_heap)
                + state_heap(state)
                + projectile.as_deref().map_or(0, projectile_heap)
        }
        E::EntityState { state, .. } => state_heap(state),
        E::Ticket { owner, .. } | E::TicketRemoved { owner, .. } => owner.capacity(),
        E::BlockLog { value } => std::mem::size_of::<crate::block_log::Delta>()
            + value.records.capacity() * std::mem::size_of::<crate::block_log::Record>()
            + value.marks.capacity() * std::mem::size_of::<(u64, bool)>()
            + value
                .records
                .iter()
                .map(|record| match &record.change {
                    crate::block_log::Change::Block { entity, .. } => {
                        entity.as_ref().map_or(0, |entity| {
                            entity.slots.as_ref().map_or(0, stacks_heap)
                                + entity.nbt.as_ref().map_or(0, Vec::capacity)
                        })
                    }
                    crate::block_log::Change::Container { slots } => {
                        slots.capacity()
                            * std::mem::size_of::<(u16, mc_data::ItemStack, mc_data::ItemStack)>()
                            + slots
                                .iter()
                                .map(|(_, before, after)| item_heap(before) + item_heap(after))
                                .sum::<usize>()
                    }
                })
                .sum::<usize>(),
    }
}

#[derive(Debug)]
struct State {
    in_flight: usize,
    closed: bool,
}

/// Shared between the tick thread (reserve) and the journal thread (release).
#[derive(Debug)]
pub(crate) struct JournalBudget {
    cap: usize,
    state: Mutex<State>,
    freed: Condvar,
}

/// The journal thread is gone: nothing will ever release the budget.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Closed;

impl JournalBudget {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            state: Mutex::new(State {
                in_flight: 0,
                closed: false,
            }),
            freed: Condvar::new(),
        }
    }

    fn admits(&self, state: &State, bytes: usize) -> bool {
        // A batch larger than the whole budget still goes through alone;
        // otherwise it could never be admitted.
        state.in_flight == 0 || state.in_flight.saturating_add(bytes) <= self.cap
    }

    /// Reserve `bytes`, blocking while the budget is full. Fails only after
    /// [`Self::close`].
    pub(crate) fn reserve(&self, bytes: usize) -> Result<(), Closed> {
        let mut state = self.state.lock().expect("journal budget");
        while !state.closed && !self.admits(&state, bytes) {
            state = self.freed.wait(state).expect("journal budget");
        }
        if state.closed {
            return Err(Closed);
        }
        state.in_flight += bytes;
        Ok(())
    }

    /// Non-blocking [`Self::reserve`]; `Ok(false)` while the budget is full.
    #[cfg(test)]
    pub(crate) fn try_reserve(&self, bytes: usize) -> Result<bool, Closed> {
        let mut state = self.state.lock().expect("journal budget");
        if state.closed {
            return Err(Closed);
        }
        if !self.admits(&state, bytes) {
            return Ok(false);
        }
        state.in_flight += bytes;
        Ok(true)
    }

    /// Return `bytes` once `append` returned (the WAL writer accepted the
    /// batch; its fsync may still be pending).
    pub(crate) fn release(&self, bytes: usize) {
        let mut state = self.state.lock().expect("journal budget");
        state.in_flight = state.in_flight.saturating_sub(bytes);
        self.freed.notify_all();
    }

    /// Wake and fail every blocked and future reserver (journal thread exit).
    pub(crate) fn close(&self) {
        self.state.lock().expect("journal budget").closed = true;
        self.freed.notify_all();
    }
}

/// Closes the budget when the journal thread returns by any path.
pub(crate) struct CloseOnDrop<'a>(pub(crate) &'a JournalBudget);

impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Test-only accessor for the Exp-VillagerBox footprint measurement: price one
/// `journal_image` baseline with the same model the journal budget uses.
#[cfg(test)]
pub(crate) fn villager_heap_for_test(villager: &crate::villager::VillagerState) -> usize {
    villager_heap(villager)
}
