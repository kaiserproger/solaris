//! Parameterless 26.3 particles. IDs come from the vanilla registries report.
use crate::command_context::Context;
use crate::command_execution::CommandResult;
use crate::tick::TickState;

const PARTICLES: &[&str] = &[
    "angry_villager",
    "block",
    "block_marker",
    "bubble",
    "sulfur_bubbles",
    "noxious_gas",
    "noxious_gas_cloud",
    "geyser",
    "geyser_base",
    "geyser_poof",
    "geyser_plume",
    "cloud",
    "copper_fire_flame",
    "crit",
    "damage_indicator",
    "dragon_breath",
    "dripping_lava",
    "falling_lava",
    "landing_lava",
    "dripping_water",
    "falling_water",
    "dust",
    "dust_color_transition",
    "effect",
    "elder_guardian",
    "enchanted_hit",
    "enchant",
    "end_rod",
    "entity_effect",
    "explosion_emitter",
    "explosion",
    "gust",
    "small_gust",
    "gust_emitter_large",
    "gust_emitter_small",
    "sonic_boom",
    "falling_dust",
    "firework",
    "fishing",
    "flame",
    "infested",
    "cherry_leaves",
    "pale_oak_leaves",
    "red_poplar_leaves",
    "orange_poplar_leaves",
    "yellow_poplar_leaves",
    "tinted_leaves",
    "sculk_soul",
    "sculk_charge",
    "sculk_charge_pop",
    "soul_fire_flame",
    "soul",
    "flash",
    "happy_villager",
    "composter",
    "heart",
    "instant_effect",
    "item",
    "vibration",
    "trail",
    "pause_mob_growth",
    "reset_mob_growth",
    "item_slime",
    "item_cobweb",
    "item_snowball",
    "large_smoke",
    "lava",
    "mycelium",
    "note",
    "poof",
    "portal",
    "rain",
    "smoke",
    "white_smoke",
    "sneeze",
    "spit",
    "squid_ink",
    "sweep_attack",
    "totem_of_undying",
    "underwater",
    "splash",
    "witch",
    "bubble_pop",
    "current_down",
    "bubble_column_up",
    "nautilus",
    "dolphin",
    "campfire_cosy_smoke",
    "campfire_signal_smoke",
    "dripping_honey",
    "falling_honey",
    "landing_honey",
    "falling_nectar",
    "falling_spore_blossom",
    "ash",
    "crimson_spore",
    "warped_spore",
    "spore_blossom_air",
    "dripping_obsidian_tear",
    "falling_obsidian_tear",
    "landing_obsidian_tear",
    "reverse_portal",
    "white_ash",
    "small_flame",
    "snowflake",
    "dripping_dripstone_lava",
    "falling_dripstone_lava",
    "dripping_dripstone_water",
    "falling_dripstone_water",
    "glow_squid_ink",
    "glow",
    "wax_on",
    "wax_off",
    "electric_spark",
    "scrape",
    "shriek",
    "egg_crack",
    "dust_plume",
    "trial_spawner_detection",
    "trial_spawner_detection_ominous",
    "vault_connection",
    "dust_pillar",
    "ominous_spawning",
    "raid_omen",
    "trial_omen",
    "block_crumble",
    "firefly",
    "sulfur_cube_goo",
];

pub(crate) fn execute(state: &mut TickState, context: Context, input: &str) -> CommandResult {
    match run(state, context, input) {
        Ok(count) => CommandResult::Success(count),
        Err(message) => {
            state.push_player(
                context.connection,
                crate::boundary::PlayerChange::Chat { message },
            );
            CommandResult::Failed
        }
    }
}
fn run(state: &mut TickState, context: Context, input: &str) -> Result<i32, String> {
    let words: Vec<_> = input.split_whitespace().collect();
    if words.len() < 2 || words.len() > 12 {
        return Err(
            "Expected particle <name> [pos] [delta speed count] [force|normal] [viewers]".into(),
        );
    }
    let name = words[1].strip_prefix("minecraft:").unwrap_or(words[1]);
    let id = PARTICLES
        .iter()
        .position(|value| *value == name)
        .ok_or("Unknown particle")?;
    if matches!(
        name,
        "block"
            | "block_marker"
            | "falling_dust"
            | "dust_pillar"
            | "block_crumble"
            | "dust"
            | "dust_color_transition"
            | "entity_effect"
            | "tinted_leaves"
            | "flash"
            | "sculk_charge"
            | "shriek"
            | "item"
            | "vibration"
            | "trail"
            | "dragon_breath"
            | "effect"
            | "instant_effect"
    ) {
        return Err("Particle options are not supported".into());
    }
    let position = if words.len() > 2 {
        if words.len() < 5 {
            return Err("Expected three particle coordinates".into());
        }
        let coords = crate::commands::coords(&words[2..5]).ok_or("Invalid coordinates")?;
        crate::commands::resolve_coords(
            coords,
            context.coordinate_origin(coords[0].local),
            context.rotation,
        )
    } else {
        context.position
    };
    let (delta, speed, count) = if words.len() > 5 {
        if words.len() < 10 {
            return Err("Expected delta, speed and count".into());
        }
        let parse = |index: usize| -> Result<f32, String> {
            let value = words[index]
                .parse::<f32>()
                .map_err(|_| "Invalid particle number")?;
            if !value.is_finite() {
                return Err("Invalid particle number".into());
            }
            Ok(value)
        };
        let count = words[9]
            .parse::<i32>()
            .map_err(|_| "Invalid particle count")?;
        if count < 0 {
            return Err("Invalid particle count".into());
        }
        ([parse(5)?, parse(6)?, parse(7)?], parse(8)?, count)
    } else {
        ([0.0; 3], 0.0, 0)
    };
    let force = match words.get(10).copied().unwrap_or("normal") {
        "normal" => false,
        "force" => true,
        _ => return Err("Expected force or normal".into()),
    };
    let target = crate::commands::target_of(words.get(11).copied().unwrap_or("@a"))
        .map_err(|e| e.message())?;
    let targets = crate::dimensions::travel::context_targets(state, context, &target)?;
    let mut sent = 0;
    for entity in targets {
        let Some((dimension, slot)) = crate::dimensions::travel::locate(state, entity) else {
            continue;
        };
        if dimension != context.dimension {
            continue;
        }
        let world = if dimension == state.dimension {
            &mut *state
        } else {
            state
                .other_dimensions
                .get_mut(&dimension)
                .ok_or("Unknown dimension")?
        };
        let Some(connection) = world.connections.entity_connection(entity) else {
            continue;
        };
        let listener = world.entities.pos(slot);
        let distance = (0..3)
            .map(|i| (listener[i] - position[i]).powi(2))
            .sum::<f64>();
        let range: f64 = if force { 512.0 } else { 32.0 };
        if distance > range * range {
            continue;
        }
        world.push_player(
            connection,
            crate::boundary::PlayerChange::CommandParticle {
                particle_id: id as i32,
                position,
                delta,
                speed,
                count,
                force,
            },
        );
        sent += 1;
    }
    if sent == 0 {
        return Err("No player can see the particle".into());
    }
    Ok(sent)
}
