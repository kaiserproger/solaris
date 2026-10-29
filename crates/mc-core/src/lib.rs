//! Core model v2 (ADR 0013).
//!
//! The only way in or out of the simulation is the tick boundary in
//! [`boundary`]: inputs accepted since the previous tick go in, an immutable
//! snapshot plus the tick's change set come out.

pub(crate) mod abstract_sim;
#[cfg(test)]
mod abstract_sim_tests;
pub mod advancements;
pub mod allay;
pub mod armadillo;
mod armor_stand;
mod beacon;
#[cfg(test)]
mod beacon_tests;
pub mod bees;
mod block_drop_context;
pub mod block_drops;
mod block_entity_defaults;
pub mod block_log;
pub(crate) mod block_ticks;
pub(crate) mod block_tools;
#[cfg(test)]
mod block_tools_tests;
pub mod blocks;
pub mod blueprint;
#[cfg(test)]
mod blueprint_family_tests;
mod books;
pub mod bossbar;
mod bottles;
pub mod boundary;
mod brewing;
pub mod build_precommit;
mod campfire;
pub(crate) mod carrier_blocks;
#[cfg(test)]
mod carrier_blocks_tests;
mod cauldron_wash;
#[cfg(test)]
#[path = "cauldron_wash_tests.rs"]
mod cauldron_wash_tests;
mod chests;
pub mod chiseled_shelf;
mod chorus;
mod chunk_loader;
pub(crate) mod chunk_tickets;
pub mod combat;
pub mod command_attribute;
mod command_block_entity;
mod command_blocks;
pub mod command_border;
pub mod command_bossbar;
pub mod command_console;
mod command_context;
pub mod command_damage;
mod command_data;
pub(crate) mod command_debug;
mod command_particle;
mod command_sound;
mod command_storage;
mod command_tag;
pub(crate) use command_debug::{Profiler as DebugProfiler, TraceArtifact as DebugTraceArtifact};
pub mod command_enchant;
mod command_execution;
mod command_item_replace;
pub mod command_items;
pub mod command_locate;
pub mod command_loot;
mod command_nbt_path;
pub mod command_rules;
pub(crate) mod command_snbt;
mod command_store;
mod command_summon;
pub(crate) mod command_text;
pub mod command_tick;
pub mod command_world;
pub mod commands;
mod composter;
mod conduit;
pub mod config;
pub(crate) mod consumables;
pub(crate) mod contact_hazards;
mod copper;
mod copper_golem;
pub mod core;
mod crafter;
mod creaking;
pub mod datapacks;
pub(crate) mod debt;
pub mod dialog;
pub mod dimensions;
pub(crate) mod dolphin;
pub mod effects;
pub(crate) mod effects_trigger;
pub(crate) mod eggs;
pub(crate) mod enchantment_locations;
pub(crate) mod enchantment_thorns;
pub(crate) mod enchantments;
pub mod end_fight;
pub(crate) mod end_portals;
mod ender_chest;
mod enderman;
pub mod entities;
pub mod entity_controls;
pub mod entity_life;
pub mod entity_loot;
pub mod entity_motion;
mod entity_uuid;
mod entity_views;
pub mod experience_orbs;
pub mod explosion;
pub mod falling;
pub mod farming;
pub(crate) mod fire;
pub mod fluids;
pub mod forceload;
pub mod golden_dandelion;
mod golems;
mod horde_bench;
pub mod hostile;
pub mod hot_backup;
#[cfg(test)]
mod hot_backup_tests;
pub(crate) mod input_log;
#[cfg(test)]
mod input_log_tests;
pub mod intent;
pub mod inventory;
pub(crate) mod item_frame;
pub(crate) mod item_hazards;
pub(crate) mod item_motion;
pub mod journal;
pub(crate) mod journal_budget;
pub(crate) mod journal_touched;
mod jukebox;
pub mod leash;
pub(crate) mod leaves;
mod lectern;
mod level_metadata;
mod light;
pub mod lod;
pub(crate) mod mace;
pub mod maps;
mod mob_equipment;
mod mob_equipment_pickup;
pub(crate) mod mob_fall;
pub(crate) mod mob_lanes;
pub mod mob_loot;
mod monster_spawner;
pub mod mounts;
pub(crate) mod native_checkpoint;
pub mod natural_spawn;
mod omens;
mod physical_attacks;
mod physical_defense;
mod piglin;
pub mod placement;
pub(crate) mod plants;
pub mod player;
mod plugin;
#[cfg(test)]
mod plugin_blueprint_tests;
mod plugin_graves;
mod plugin_inventories;
mod plugin_npcs;
mod plugin_player;
mod plugin_settlements;
#[cfg(test)]
mod plugin_settlements_tests;
mod plugin_startup;
mod plugin_structures;
#[cfg(test)]
mod plugin_tests;
pub mod portals;
mod pov_ray;
pub mod pregenerate;
pub mod projectiles;
pub mod raids;
pub(crate) mod redstone;
pub mod registry;
pub mod scoreboard;
mod scoreboard_persistence;
pub(crate) mod sculk;
pub mod settlements;
mod shelf;
pub mod shrieker;
mod shulker;
mod signs;
pub mod sleep;
mod spawn_egg;
pub mod special_mobs;
pub mod species;
mod statistics;
pub mod survival;
pub mod taming;
mod target_grid;
pub mod terrain;
pub mod tick;
pub(crate) mod tick_budget;
pub(crate) mod tick_control;
pub mod tnt;
mod trial_spawner;
mod vault;
pub mod vehicles;
pub(crate) mod vibration;
pub mod villager;
pub mod warden;
pub mod weather;
pub mod world;

#[cfg(test)]
mod blueprint_tests;
#[cfg(test)]
mod core_tests;
#[cfg(test)]
mod debt_tests;
#[cfg(test)]
mod entity_life_tests;
#[cfg(test)]
mod entity_loot_tests;
#[cfg(test)]
mod entity_motion_tests;
#[cfg(test)]
mod entity_uuid_tests;
#[cfg(test)]
mod farming_tests;
#[cfg(test)]
mod fluids_tests;
#[cfg(test)]
mod hostile_tests;
#[cfg(test)]
mod journal_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod natural_spawn_tests;
#[cfg(test)]
mod pause_when_empty_tests;
#[cfg(test)]
mod quirk_parity_tests;
#[cfg(test)]
mod rare_mobs_tests;
#[cfg(test)]
mod replay_determinism_tests;
#[cfg(test)]
mod spawn_clearance_tests;
#[cfg(test)]
mod species_tests;
#[cfg(test)]
mod target_grid_tests;
#[cfg(test)]
mod tick_tests;
#[cfg(test)]
mod view_negotiation_tests;
pub mod world_conversion;
pub mod world_maintenance;
pub mod worldgen_upgrade;

pub use config::{CoreConfig, CoreError};
pub use core::{Core, RemoteTicket, SpawnRequest};
pub use physical_defense::PhysicalImpact;

#[doc(hidden)]
pub use core::ContainerBench;
#[doc(hidden)]
pub use horde_bench::HordeBench;

/// In-crate bench/test hook: spawn `count` mobs through the tick thread.
/// `spread` selects spread vs crowded placement.
#[doc(hidden)]
pub fn bench_seed(core: &Core, count: usize, spread: bool, zombies: usize) {
    // Chunked: the spawn queue is bounded (16); the tick thread drains it
    // every tick. Batches carry global spread offsets so the full count tiles
    // one grid. A full queue waits on the next TickOutput (event, never a
    // sleep): one tick always drains the queue.
    const BATCH: usize = 4000;
    let registry = core.registry();
    let mut rx = core.subscribe();
    if spread {
        // Keep the seeded grid simulated: ticket its chunks before spawning
        // (no players exist, so without this only the spawn area is resident
        // and surface probes fall back to y=80).
        let side = (count as f64).sqrt().ceil() as i32;
        let span = side * 2;
        let chunks = span.div_euclid(16) + 1;
        let radius = (chunks + 1) / 2 + 1;
        // The grid is centred on the world spawn (see `apply_spawn_request`).
        let [spawn_x, _, spawn_z] = core.world_spawn();
        let centre = crate::boundary::ChunkPos {
            x: spawn_x.div_euclid(16),
            z: spawn_z.div_euclid(16),
        };
        let want = crate::world::chunks_in_radius(centre, radius).len();
        core.add_ticket(RemoteTicket {
            owner: "bench".to_owned(),
            chunk: centre,
            radius: radius as u32,
            reason: "bench seeded grid".to_owned(),
            budget_chunks: want,
        })
        .expect("bench ticket fits its budget");
        // Chunks load off-tick: wait until every ticketed chunk is resident
        // (in a published snapshot) before the surface probes, or spawns
        // fall back to y=80 over terrain that has not arrived yet.
        let wanted: std::collections::HashSet<_> = crate::world::chunks_in_radius(centre, radius)
            .into_iter()
            .collect();
        loop {
            match rx.blocking_recv() {
                Ok(output) => {
                    let resident = output
                        .snapshot
                        .chunks
                        .iter()
                        .filter(|entry| wanted.contains(&entry.pos))
                        .count();
                    if resident == wanted.len() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("tick thread died while seeding")
                }
            }
        }
    }
    let mut wait_tick = || loop {
        match rx.blocking_recv() {
            Ok(_) => break,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("tick thread died while seeding")
            }
        }
    };
    let mut offset = 0usize;
    let mut remaining_zombie = zombies.min(count);
    let mut remaining_cow = count - remaining_zombie;
    while remaining_zombie > 0 || remaining_cow > 0 {
        let zombie_batch = remaining_zombie.min(BATCH / 2).max(remaining_zombie.min(1));
        let cow_batch = remaining_cow.min(BATCH / 2).max(remaining_cow.min(1));
        if zombie_batch > 0 {
            while !core.debug_spawn(SpawnRequest {
                type_id: registry.zombie_type_id,
                count: zombie_batch,
                spread,
                start: offset,
                total: count,
            }) {
                wait_tick();
            }
            offset += zombie_batch;
            remaining_zombie -= zombie_batch;
        }
        if cow_batch > 0 {
            while !core.debug_spawn(SpawnRequest {
                type_id: registry.cow_type_id,
                count: cow_batch,
                spread,
                start: offset,
                total: count,
            }) {
                wait_tick();
            }
            offset += cow_batch;
            remaining_cow -= cow_batch;
        }
    }
}

#[cfg(test)]
mod interact_failing_first_tests;
#[cfg(test)]
mod interact_parity_tests;

mod brushing;
mod painting;
mod spawn_protection;
mod special_storage;
mod sponge;
mod vanilla_interact;
