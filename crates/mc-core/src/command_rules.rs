//! Enabled vanilla 26.1.2 game rules, with canonical snake_case storage.
//!
//! Source: minecraft-patched-26.1.2.76-sources.jar,
//! net/minecraft/world/level/gamerules/GameRules.java (registrations and bounds),
//! net/minecraft/util/datafix/fixes/GameRuleRegistryFix.java (legacy aliases).
//! The feature-gated max_minecart_speed is deliberately not registered.

use crate::commands::CommandError;
use crate::tick::TickState;

/// Operational limit: random ticks scale with each active section every tick.
pub(crate) const MAX_RANDOM_TICK_SPEED: i32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Boolean,
    Integer { min: i32, max: i32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub name: &'static str,
    pub default: &'static str,
    pub kind: RuleKind,
    /// Legacy names; disable* aliases invert boolean values on input.
    pub aliases: &'static [&'static str],
    default_integer: i32,
}

macro_rules! boolean_rule {
    ($name:literal, $default:literal, $($alias:literal),* $(,)?) => {
        Rule { name: $name, default: $default, kind: RuleKind::Boolean, aliases: &[$($alias),*], default_integer: 0 }
    };
}
macro_rules! integer_rule {
    ($name:literal, $default:literal, $min:expr, $max:expr, $($alias:literal),* $(,)?) => {
        Rule { name: $name, default: stringify!($default), kind: RuleKind::Integer { min: $min, max: $max }, aliases: &[$($alias),*], default_integer: $default }
    };
}

const RULES: &[Rule] = &[
    boolean_rule!("advance_time", "true", "doDaylightCycle"),
    boolean_rule!("advance_weather", "true", "doWeatherCycle"),
    boolean_rule!(
        "allow_entering_nether_using_portals",
        "true",
        "allowEnteringNetherUsingPortals"
    ),
    boolean_rule!("block_drops", "true", "doTileDrops"),
    boolean_rule!(
        "block_explosion_drop_decay",
        "true",
        "blockExplosionDropDecay"
    ),
    boolean_rule!(
        "command_blocks_work",
        "true",
        "enableCommandBlocks",
        "commandBlocksEnabled"
    ),
    boolean_rule!("command_block_output", "true", "commandBlockOutput"),
    boolean_rule!("drowning_damage", "true", "drowningDamage"),
    boolean_rule!(
        "elytra_movement_check",
        "true",
        "disableElytraMovementCheck"
    ),
    boolean_rule!(
        "ender_pearls_vanish_on_death",
        "true",
        "enderPearlsVanishOnDeath"
    ),
    boolean_rule!("entity_drops", "true", "doEntityDrops"),
    boolean_rule!("fall_damage", "true", "fallDamage"),
    boolean_rule!("fire_damage", "true", "fireDamage"),
    integer_rule!("fire_spread_radius_around_player", 128, -1, i32::MAX,),
    boolean_rule!("forgive_dead_players", "true", "forgiveDeadPlayers"),
    boolean_rule!("freeze_damage", "true", "freezeDamage"),
    boolean_rule!("global_sound_events", "true", "globalSoundEvents"),
    boolean_rule!("immediate_respawn", "false", "doImmediateRespawn"),
    boolean_rule!("keep_inventory", "false", "keepInventory"),
    boolean_rule!("lava_source_conversion", "false", "lavaSourceConversion"),
    boolean_rule!("limited_crafting", "false", "doLimitedCrafting"),
    boolean_rule!("locator_bar", "true", "locatorBar"),
    boolean_rule!("log_admin_commands", "true", "logAdminCommands"),
    integer_rule!(
        "max_block_modifications",
        32768,
        1,
        i32::MAX,
        "commandModificationBlockLimit"
    ),
    integer_rule!(
        "max_command_forks",
        65536,
        0,
        i32::MAX,
        "maxCommandForkCount"
    ),
    integer_rule!(
        "max_command_sequence_length",
        65536,
        0,
        i32::MAX,
        "maxCommandChainLength"
    ),
    integer_rule!("max_entity_cramming", 24, 0, i32::MAX, "maxEntityCramming"),
    integer_rule!(
        "max_snow_accumulation_height",
        1,
        0,
        8,
        "snowAccumulationHeight"
    ),
    boolean_rule!("mob_drops", "true", "doMobLoot"),
    boolean_rule!("mob_explosion_drop_decay", "true", "mobExplosionDropDecay"),
    boolean_rule!("mob_griefing", "true", "mobGriefing"),
    boolean_rule!("natural_health_regeneration", "true", "naturalRegeneration"),
    boolean_rule!(
        "player_movement_check",
        "true",
        "disablePlayerMovementCheck"
    ),
    integer_rule!(
        "players_nether_portal_creative_delay",
        0,
        0,
        i32::MAX,
        "playersNetherPortalCreativeDelay"
    ),
    integer_rule!(
        "players_nether_portal_default_delay",
        80,
        0,
        i32::MAX,
        "playersNetherPortalDefaultDelay"
    ),
    integer_rule!(
        "players_sleeping_percentage",
        100,
        0,
        i32::MAX,
        "playersSleepingPercentage"
    ),
    boolean_rule!(
        "projectiles_can_break_blocks",
        "true",
        "projectilesCanBreakBlocks"
    ),
    boolean_rule!("pvp", "true",),
    boolean_rule!("raids", "true", "disableRaids"),
    integer_rule!("random_tick_speed", 3, 0, i32::MAX, "randomTickSpeed"),
    boolean_rule!("reduced_debug_info", "false", "reducedDebugInfo"),
    integer_rule!("respawn_radius", 10, 0, i32::MAX, "spawnRadius"),
    boolean_rule!("send_command_feedback", "true", "sendCommandFeedback"),
    boolean_rule!("show_advancement_messages", "true", "announceAdvancements"),
    boolean_rule!("show_death_messages", "true", "showDeathMessages"),
    boolean_rule!("spawner_blocks_work", "true", "spawnerBlocksEnabled"),
    boolean_rule!("spawn_mobs", "true", "doMobSpawning"),
    boolean_rule!("spawn_monsters", "true", "spawnMonsters"),
    boolean_rule!("spawn_patrols", "true", "doPatrolSpawning"),
    boolean_rule!("spawn_phantoms", "true", "doInsomnia"),
    boolean_rule!("spawn_wandering_traders", "true", "doTraderSpawning"),
    boolean_rule!("spawn_wardens", "true", "doWardenSpawning"),
    boolean_rule!(
        "spectators_generate_chunks",
        "true",
        "spectatorsGenerateChunks"
    ),
    boolean_rule!("spread_vines", "true", "doVinesSpread"),
    boolean_rule!("tnt_explodes", "true", "tntExplodes"),
    boolean_rule!("tnt_explosion_drop_decay", "false", "tntExplosionDropDecay"),
    boolean_rule!("universal_anger", "false", "universalAnger"),
    boolean_rule!("water_source_conversion", "true", "waterSourceConversion"),
];

/// All rules enabled by vanilla's default feature set, in registration order.
#[must_use]
pub fn rules() -> &'static [Rule] {
    RULES
}

fn find(name: &str) -> Option<(&'static Rule, bool)> {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    RULES.iter().find_map(|rule| {
        if rule.name == name {
            Some((rule, false))
        } else if rule.aliases.contains(&name) {
            Some((
                rule,
                matches!(
                    name,
                    "disableElytraMovementCheck" | "disablePlayerMovementCheck" | "disableRaids"
                ),
            ))
        } else {
            None
        }
    })
}

/// Parse arguments after `gamerule`, returning only canonical storage keys.
/// Legacy inverted aliases translate writes; queries address the canonical rule.
pub fn parse(args: &[&str]) -> Result<(String, Option<String>), CommandError> {
    const USAGE: CommandError = CommandError::Usage("gamerule <rule> [value]");
    if !(1..=2).contains(&args.len()) {
        return Err(USAGE);
    }
    let (rule, inverted) = find(args[0]).ok_or(CommandError::Unknown)?;
    let value = args
        .get(1)
        .map(|value| match rule.kind {
            RuleKind::Boolean => match *value {
                "true" => Ok(if inverted { "false" } else { "true" }.to_owned()),
                "false" => Ok(if inverted { "true" } else { "false" }.to_owned()),
                _ => Err(USAGE),
            },
            RuleKind::Integer { min, max } => {
                // Brigadier accepts digits and a leading minus, not a leading plus.
                if !value
                    .strip_prefix('-')
                    .unwrap_or(value)
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
                {
                    return Err(USAGE);
                }
                let number = value.parse::<i32>().map_err(|_| USAGE)?;
                if rule.name == "random_tick_speed" && number > MAX_RANDOM_TICK_SPEED {
                    return Err(CommandError::Message(format!(
                        "randomTickSpeed must not exceed {MAX_RANDOM_TICK_SPEED}"
                    )));
                }
                if !(min..=max).contains(&number) {
                    return Err(USAGE);
                }
                Ok(number.to_string())
            }
        })
        .transpose()?;
    Ok((rule.name.to_owned(), value))
}

/// Canonical default, including when addressed by a qualified or legacy name.
#[must_use]
pub fn default_value(name: &str) -> Option<&'static str> {
    find(name).map(|(rule, _)| rule.default)
}

fn value<'a>(values: &'a rustc_hash::FxHashMap<String, String>, rule: &Rule) -> &'a str {
    values.get(rule.name).map_or(rule.default, String::as_str)
}

/// Typed handle of a boolean rule. Handles exist only as the associated
/// constants below; an unknown name or wrong kind fails const evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoolRule(usize);
/// Typed handle of an integer rule; see [`BoolRule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntRule(usize);

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn index(name: &str, integer: bool) -> usize {
    let mut i = 0;
    while i < RULES.len() {
        if same(RULES[i].name, name) {
            assert!(
                matches!(RULES[i].kind, RuleKind::Integer { .. }) == integer,
                "game rule kind mismatch"
            );
            return i;
        }
        i += 1;
    }
    panic!("unknown game rule")
}

macro_rules! handles {
    ($ty:ident, $integer:literal: $($constant:ident = $name:literal),* $(,)?) => {
        impl $ty {
            $(pub const $constant: Self = Self(index($name, $integer));)*
        }
    };
}

handles!(BoolRule, false:
    ADVANCE_TIME = "advance_time",
    ADVANCE_WEATHER = "advance_weather",
    ALLOW_ENTERING_NETHER_USING_PORTALS = "allow_entering_nether_using_portals",
    BLOCK_DROPS = "block_drops",
    DROWNING_DAMAGE = "drowning_damage",
    ENDER_PEARLS_VANISH_ON_DEATH = "ender_pearls_vanish_on_death",
    ENTITY_DROPS = "entity_drops",
    FALL_DAMAGE = "fall_damage",
    FIRE_DAMAGE = "fire_damage",
    IMMEDIATE_RESPAWN = "immediate_respawn",
    KEEP_INVENTORY = "keep_inventory",
    LAVA_SOURCE_CONVERSION = "lava_source_conversion",
    MOB_DROPS = "mob_drops",
    MOB_EXPLOSION_DROP_DECAY = "mob_explosion_drop_decay",
    MOB_GRIEFING = "mob_griefing",
    NATURAL_HEALTH_REGENERATION = "natural_health_regeneration",
    PVP = "pvp",
    RAIDS = "raids",
    SEND_COMMAND_FEEDBACK = "send_command_feedback",
    SHOW_ADVANCEMENT_MESSAGES = "show_advancement_messages",
    SHOW_DEATH_MESSAGES = "show_death_messages",
    SPAWN_MOBS = "spawn_mobs",
    SPAWN_MONSTERS = "spawn_monsters",
    SPAWN_WANDERING_TRADERS = "spawn_wandering_traders",
    SPAWN_WARDENS = "spawn_wardens",
    TNT_EXPLODES = "tnt_explodes",
    TNT_EXPLOSION_DROP_DECAY = "tnt_explosion_drop_decay",
    WATER_SOURCE_CONVERSION = "water_source_conversion",
);
handles!(IntRule, true:
    FIRE_SPREAD_RADIUS_AROUND_PLAYER = "fire_spread_radius_around_player",
    MAX_BLOCK_MODIFICATIONS = "max_block_modifications",
    MAX_COMMAND_FORKS = "max_command_forks",
    MAX_COMMAND_SEQUENCE_LENGTH = "max_command_sequence_length",
    MAX_SNOW_ACCUMULATION_HEIGHT = "max_snow_accumulation_height",
    PLAYERS_NETHER_PORTAL_CREATIVE_DELAY = "players_nether_portal_creative_delay",
    PLAYERS_NETHER_PORTAL_DEFAULT_DELAY = "players_nether_portal_default_delay",
    PLAYERS_SLEEPING_PERCENTAGE = "players_sleeping_percentage",
    RANDOM_TICK_SPEED = "random_tick_speed",
    RESPAWN_RADIUS = "respawn_radius",
);

/// Allocation-free typed lookup; invalid persisted values use the default.
#[must_use]
pub fn boolean(state: &TickState, rule: BoolRule) -> bool {
    rule_boolean(&state.gamerules, &RULES[rule.0])
}

/// Which finer block-damage switch a call site belongs to. Vanilla 26.1.2 has
/// only the master `mobGriefing` gamerule; these finer switches live in server
/// configuration (all default to true, so vanilla behaviour is unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobGrief {
    /// A blast no specific mob owns; only the master gamerule applies.
    Other,
    Creeper,
    Ghast,
    Wither,
    VillagerFarming,
    SheepGrazing,
    /// A mob blast whose blueprint never breaks blocks (data-defined
    /// [`Explosion::griefing`](crate::blueprint::Explosion::griefing) off).
    Never,
}

/// `mobGriefing` combined with the per-mob switch for `grief`.
#[must_use]
pub fn mob_griefing(state: &TickState, grief: MobGrief) -> bool {
    if !boolean(state, BoolRule::MOB_GRIEFING) {
        return false;
    }
    let rules = &state.mob_griefing;
    match grief {
        MobGrief::Other => true,
        MobGrief::Creeper => rules.creeper_block_damage,
        MobGrief::Ghast => rules.ghast_block_damage,
        MobGrief::Wither => rules.wither_block_damage,
        MobGrief::VillagerFarming => rules.villager_farming,
        MobGrief::SheepGrazing => rules.sheep_grazing,
        MobGrief::Never => false,
    }
}

fn rule_boolean(values: &rustc_hash::FxHashMap<String, String>, rule: &Rule) -> bool {
    match value(values, rule) {
        "true" => true,
        "false" => false,
        _ => rule.default == "true",
    }
}

/// Name-addressed lookup (legacy aliases included); unknown or non-boolean
/// names return false.
#[cfg(test)]
fn boolean_value(values: &rustc_hash::FxHashMap<String, String>, name: &str) -> bool {
    find(name)
        .filter(|(rule, _)| rule.kind == RuleKind::Boolean)
        .is_some_and(|(rule, _)| rule_boolean(values, rule))
}

/// Allocation-free typed lookup; invalid persisted values use the default.
#[must_use]
pub fn integer(state: &TickState, rule: IntRule) -> i32 {
    rule_integer(&state.gamerules, &RULES[rule.0])
}

fn rule_integer(values: &rustc_hash::FxHashMap<String, String>, rule: &Rule) -> i32 {
    let RuleKind::Integer { min, max } = rule.kind else {
        unreachable!("IntRule handles only address integer rules")
    };
    values
        .get(rule.name)
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|value| (min..=max).contains(value))
        .unwrap_or(rule.default_integer)
}

/// Name-addressed lookup (legacy aliases included); unknown or non-integer
/// names return zero.
#[cfg(test)]
fn integer_value(values: &rustc_hash::FxHashMap<String, String>, name: &str) -> i32 {
    find(name)
        .filter(|(rule, _)| matches!(rule.kind, RuleKind::Integer { .. }))
        .map_or(0, |(rule, _)| rule_integer(values, rule))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_has_a_valid_default_and_unique_names() {
        let mut names = std::collections::HashSet::new();
        for rule in rules() {
            assert!(names.insert(rule.name));
            assert_eq!(parse(&[rule.name]), Ok((rule.name.to_owned(), None)));
            assert_eq!(
                parse(&[rule.name, rule.default]),
                Ok((rule.name.to_owned(), Some(rule.default.to_owned())))
            );
            assert_eq!(default_value(rule.name), Some(rule.default));
            for alias in rule.aliases {
                assert!(names.insert(alias));
                assert_eq!(parse(&[alias]), Ok((rule.name.to_owned(), None)));
            }
        }
    }

    #[test]
    fn integer_bounds_match_vanilla() {
        for rule in rules() {
            let RuleKind::Integer { min, max } = rule.kind else {
                continue;
            };
            let effective_max = if rule.name == "random_tick_speed" {
                MAX_RANDOM_TICK_SPEED
            } else {
                max
            };
            for bound in [min, effective_max] {
                assert!(parse(&[rule.name, &bound.to_string()]).is_ok());
            }
            assert!(parse(&[rule.name, &(i64::from(min) - 1).to_string()]).is_err());
            assert!(parse(&[rule.name, &(i64::from(effective_max) + 1).to_string()]).is_err());
        }
        assert_eq!(
            default_value("fire_spread_radius_around_player"),
            Some("128")
        );
        assert_eq!(
            parse(&["fire_spread_radius_around_player", "-1"]),
            Ok((
                "fire_spread_radius_around_player".to_owned(),
                Some("-1".to_owned())
            ))
        );
        assert!(parse(&["players_sleeping_percentage", "101"]).is_ok());
        assert!(parse(&["max_block_modifications", "0"]).is_err());
        assert!(parse(&["max_snow_accumulation_height", "9"]).is_err());
    }

    #[test]
    fn qualified_names_and_legacy_aliases_share_storage() {
        for rule in rules() {
            assert_eq!(
                parse(&[&format!("minecraft:{}", rule.name), rule.default]),
                parse(&[rule.name, rule.default])
            );
            for alias in rule.aliases {
                let inverted = find(alias).unwrap().1;
                let input = if inverted { "false" } else { rule.default };
                assert_eq!(
                    parse(&[alias, input]),
                    Ok((
                        rule.name.to_owned(),
                        Some(if inverted { "true" } else { rule.default }.to_owned())
                    ))
                );
            }
        }
        assert_eq!(
            parse(&["keepInventory", "true"]),
            parse(&["keep_inventory", "true"])
        );
        assert_eq!(parse(&["disableRaids", "true"]), parse(&["raids", "false"]));
        assert_eq!(
            parse(&["randomTickSpeed", "0003"]),
            parse(&["random_tick_speed", "3"])
        );
    }

    #[test]
    fn malformed_values_and_unknown_rules_are_rejected() {
        for args in [
            vec![],
            vec!["pvp", "true", "extra"],
            vec!["pvp", "1"],
            vec!["pvp", "True"],
            vec!["random_tick_speed", "true"],
            vec!["random_tick_speed", "+1"],
            vec!["random_tick_speed", "1.0"],
            vec!["random_tick_speed", ""],
        ] {
            assert!(parse(&args).is_err());
        }
        for name in [
            "unknown",
            "other:pvp",
            "max_minecart_speed",
            "minecartMaxSpeed",
            "spawnChunkRadius",
            "doFireTick",
        ] {
            assert_eq!(parse(&[name]), Err(CommandError::Unknown));
            assert_eq!(default_value(name), None);
        }
    }

    #[test]
    fn typed_lookup_uses_defaults_and_canonical_overrides() {
        let mut values = rustc_hash::FxHashMap::default();
        for rule in rules() {
            match rule.kind {
                RuleKind::Boolean => {
                    assert_eq!(boolean_value(&values, rule.name), rule.default == "true")
                }
                RuleKind::Integer { .. } => assert_eq!(
                    integer_value(&values, rule.name),
                    rule.default.parse::<i32>().unwrap()
                ),
            }
        }
        values.insert("keep_inventory".to_owned(), "true".to_owned());
        values.insert("random_tick_speed".to_owned(), "42".to_owned());
        assert!(boolean_value(&values, "minecraft:keep_inventory"));
        assert!(boolean_value(&values, "keepInventory"));
        assert_eq!(integer_value(&values, "randomTickSpeed"), 42);
        values.insert("random_tick_speed".to_owned(), "-1".to_owned());
        values.insert("pvp".to_owned(), "invalid".to_owned());
        assert_eq!(integer_value(&values, "random_tick_speed"), 3);
        assert!(boolean_value(&values, "pvp"));
        assert!(!boolean_value(&values, "unknown"));
        assert_eq!(integer_value(&values, "unknown"), 0);
    }
}
