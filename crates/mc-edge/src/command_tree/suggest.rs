//! Tab-completion suggestions.
//! Moved verbatim from `command_tree.rs`.

use super::*;

/// Cheap completion for roots, player selectors/names and registry arguments.
/// Brigadier string ranges are UTF-16 code-unit offsets, including any slash.
#[allow(clippy::too_many_arguments)]
pub(crate) fn suggestions(
    request_id: i32,
    command: &str,
    op_level: u8,
    player_names: &[String],
    item_ids: &[String],
    block_ids: &[String],
    entity_ids: &[String],
    structure_ids: &[String],
    plugins: &[(String, u8)],
) -> ClientboundCommandSuggestions {
    let start = command
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(usize::from(command.starts_with('/')), |(index, c)| {
            index + c.len_utf8()
        });
    let prefix = &command[start..];
    let preceding = command[..start].trim_start_matches('/');
    let mut words = preceding.split_whitespace();
    let root = words.next();
    let second = words.next();
    let argument_index = preceding.split_whitespace().count();
    let mut result = ClientboundCommandSuggestions {
        id: request_id,
        start: command[..start].encode_utf16().count() as i32,
        length: prefix.encode_utf16().count() as i32,
        suggestions: Vec::new(),
    };
    let mut add = |candidate: &str| {
        if result.suggestions.len() < 256
            && (candidate.starts_with(prefix)
                || (!prefix.contains(':')
                    && candidate
                        .strip_prefix("minecraft:")
                        .is_some_and(|path| path.starts_with(prefix))))
        {
            result.suggestions.push(CommandSuggestionEntry {
                text: candidate.to_owned(),
                tooltip_nbt: None,
            });
        }
    };
    let Some(root) = root else {
        for &(name, level) in ROOTS {
            if op_level >= level && !plugins.iter().any(|(root, _)| root == name) {
                add(name);
            }
        }
        for (name, level) in plugins {
            if op_level >= *level {
                add(name);
            }
        }
        return result;
    };
    if plugins.iter().any(|(name, _)| name == root) {
        return result;
    }
    if !ROOTS
        .iter()
        .any(|&(name, level)| name == root && op_level >= level)
    {
        return result;
    }
    if matches!(root, "item" | "loot") && second == Some("replace") {
        let kind = preceding.split_whitespace().nth(2);
        let destination_slot = (kind == Some("entity") && argument_index == 4)
            || (kind == Some("block") && argument_index == 6);
        let source_kind = preceding
            .split_whitespace()
            .find(|word| *word == "from")
            .and_then(|_| {
                let mut parts = preceding
                    .split_whitespace()
                    .skip_while(|word| *word != "from");
                parts.next();
                parts.next()
            });
        let source_slot = source_kind.is_some_and(|source| {
            let source_len = if source == "entity" { 2 } else { 4 };
            preceding
                .split_whitespace()
                .position(|word| word == "from")
                .is_some_and(|at| argument_index == at + 1 + source_len)
        });
        if destination_slot || source_slot {
            let kind = if source_slot { source_kind } else { kind };
            if kind == Some("block") {
                for index in 0..54 {
                    add(&format!("container.{index}"));
                }
            } else {
                for name in [
                    "weapon.mainhand",
                    "weapon.offhand",
                    "armor.head",
                    "armor.chest",
                    "armor.legs",
                    "armor.feet",
                ] {
                    add(name);
                }
                for index in 0..9 {
                    add(&format!("hotbar.{index}"));
                }
                for index in 0..27 {
                    add(&format!("inventory.{index}"));
                }
            }
            return result;
        }
    }
    if root == "settlement" {
        if argument_index == 1 {
            for &(action, _) in SETTLEMENT_HELP {
                add(action);
            }
        } else if argument_index == 3
            && let Some(domain @ ("production" | "commerce" | "payroll" | "caravan")) = second
        {
            add("status");
            for &(_, action, _) in SETTLEMENT_ECONOMY.iter().filter(|entry| entry.0 == domain) {
                add(action);
            }
        }
        return result;
    }
    if root == "effect" {
        match (second, argument_index) {
            (_, 1) => {
                add("give");
                add("clear");
                return result;
            }
            (Some("give" | "clear"), 3) => {
                for id in mc_core::effects::EffectId::ALL {
                    add(mc_data::mob_effects_26_1_2::name_for_id(id as u32)
                        .expect("built-in mob-effect identifier"));
                }
                return result;
            }
            (Some("give"), 4) => {
                add("infinite");
                return result;
            }
            (Some("give"), 6) => {
                add("false");
                add("true");
                return result;
            }
            _ => {}
        }
    }
    if root == "tick" {
        match (second, argument_index) {
            (_, 1) => {
                for literal in ["query", "rate", "step", "sprint", "unfreeze", "freeze"] {
                    add(literal);
                }
                return result;
            }
            (Some("rate"), 2) => {
                add("20");
                return result;
            }
            (Some("step"), 2) => {
                // Vanilla merges the `stop` literal with the time argument's
                // `1t`/`1s` suggestions (TickCommand.java:35-44); `add`
                // filters by the typed prefix.
                add("stop");
                add("1t");
                add("1s");
                return result;
            }
            (Some("sprint"), 2) => {
                // TickCommand.java:45-53: `stop` plus `60s`/`1d`/`3d`.
                add("stop");
                add("60s");
                add("1d");
                add("3d");
                return result;
            }
            _ => return result,
        }
    }
    if root == "bossbar" {
        match (second, argument_index) {
            (_, 1) => {
                for literal in ["add", "remove", "list", "set", "get"] {
                    add(literal);
                }
                return result;
            }
            (Some("set"), 3) => {
                for literal in [
                    "name", "color", "style", "value", "max", "visible", "players",
                ] {
                    add(literal);
                }
                return result;
            }
            (Some("set"), 4) => {
                // Words are root,set,id,field: the field is the fourth word.
                let field = preceding.split_whitespace().nth(3);
                match field {
                    Some("color") => {
                        for color in ["pink", "blue", "red", "green", "yellow", "purple", "white"] {
                            add(color);
                        }
                        return result;
                    }
                    Some("style") => {
                        for style in [
                            "progress",
                            "notched_6",
                            "notched_10",
                            "notched_12",
                            "notched_20",
                        ] {
                            add(style);
                        }
                        return result;
                    }
                    Some("visible") => {
                        add("false");
                        add("true");
                        return result;
                    }
                    _ => {}
                }
            }
            (Some("get"), 3) => {
                for literal in ["value", "max", "visible", "players"] {
                    add(literal);
                }
                return result;
            }
            _ => {}
        }
    }
    if root == "locate" {
        match (second, argument_index) {
            (_, 1) => {
                for literal in ["structure", "biome", "poi"] {
                    add(literal);
                }
                return result;
            }
            (Some("structure"), 2) => {
                for id in structure_ids {
                    add(id);
                }
                return result;
            }
            _ => return result,
        }
    }
    if root == "title" {
        match (second, argument_index) {
            (_, 1) => {
                for selector in ["@a", "@p", "@r", "@s"] {
                    add(selector);
                }
                for name in player_names {
                    add(name);
                }
                return result;
            }
            (_, 2) => {
                for literal in ["clear", "reset", "title", "subtitle", "actionbar", "times"] {
                    add(literal);
                }
                return result;
            }
            _ => return result,
        }
    }
    if root == "tellraw" {
        if argument_index == 1 {
            for selector in ["@a", "@p", "@r", "@s"] {
                add(selector);
            }
            for name in player_names {
                add(name);
            }
        }
        return result;
    }
    if root == "damage" {
        match argument_index {
            1 => {
                for selector in ["@p", "@r", "@s", "@e[limit=1]"] {
                    add(selector);
                }
                for name in player_names {
                    add(name);
                }
            }
            3 => {
                for id in 0.. {
                    let Some(name) = mc_data::required_registry_entry("damage_type", id) else {
                        break;
                    };
                    if mc_core::survival::DamageKind::command_type(name.as_str()).is_some() {
                        add(name.as_str());
                    }
                }
            }
            4 => {
                add("at");
                add("by");
            }
            5 | 7 if preceding.split_whitespace().nth(4) == Some("by") => {
                for selector in ["@p", "@r", "@s", "@e[limit=1]"] {
                    add(selector);
                }
                for name in player_names {
                    add(name);
                }
            }
            6 if preceding.split_whitespace().nth(4) == Some("by") => add("from"),
            _ => {}
        }
        return result;
    }
    let registry = match (root, second, argument_index) {
        ("give" | "clear", _, 2) | ("debug", Some("give"), 2) => Some(item_ids),
        ("summon", _, 1) => Some(entity_ids),
        ("setblock", _, 4) | ("fill", _, 7) | ("fill", _, 9) => Some(block_ids),
        _ => None,
    };
    if let Some(ids) = registry {
        for id in ids {
            add(id);
        }
    } else if (matches!(
        root,
        "msg"
            | "tell"
            | "w"
            | "give"
            | "clear"
            | "kill"
            | "kick"
            | "ban"
            | "mute"
            | "unmute"
            | "warn"
            | "pardon"
            | "op"
            | "deop"
            | "spawnpoint"
            | "enchant"
    ) && argument_index == 1)
        || (matches!(root, "tp" | "teleport") && (1..=2).contains(&argument_index))
        || (root == "gamemode" && argument_index == 2)
        || (matches!(root, "xp" | "experience") && argument_index == 2)
        || (root == "advancement" && argument_index == 2)
        || (root == "team" && second == Some("leave") && argument_index == 2)
        || (root == "team" && second == Some("join") && argument_index == 3)
        || (root == "scoreboard" && second == Some("players") && argument_index == 3)
        || (root == "effect" && matches!(second, Some("give" | "clear")) && argument_index == 2)
    {
        for selector in ["@a", "@p", "@r", "@s"] {
            add(selector);
        }
        if matches!(
            root,
            "kill" | "tp" | "teleport" | "enchant" | "team" | "scoreboard" | "effect"
        ) {
            add("@e");
            add("@n");
        }
        for name in player_names {
            add(name);
        }
    } else if root == "time" && second == Some("set") && argument_index == 2 {
        for marker in [
            "minecraft:day",
            "minecraft:noon",
            "minecraft:night",
            "minecraft:midnight",
        ] {
            add(marker);
        }
    }
    result
}
