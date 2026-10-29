//! Supported 26.1.2 command syntax, filtered before it reaches the client.
//!
//! Source: ../solaris-loader/loader-neoforge/build/moddev/artifacts/
//! minecraft-patched-26.1.2.76-sources.jar, net/minecraft/server/commands/.
//! Registration evidence: GameModeCommand.java:22-34, TeleportCommand.java:38-143,
//! GiveCommand.java:26-48, ClearInventoryCommands.java:29-58,
//! ExperienceCommand.java:27-130, TimeCommand.java:48-113,
//! WeatherCommand.java:20-50, DifficultyCommand.java:22-33,
//! GameRuleCommand.java:15-33, SummonCommand.java:36-75,
//! SetBlockCommand.java:32-102, FillCommand.java:40-155,
//! SetSpawnCommand.java:24-69, SetWorldSpawnCommand.java:18-31,
//! EnchantCommand.java:40-67, TeamCommand.java:64-258,
//! ScoreboardCommand.java:70-380, MsgCommand.java:19-33,
//! HelpCommand.java:18-48, ListPlayersCommand.java:16-21,
//! EmoteCommands.java:15-25, BanPlayerCommands.java:22-37,
//! KickCommand.java:22-36, SaveAllCommand.java:15-21.
//! BanIpCommands.java:26-38, PardonIpCommand.java:22-31.
//! KillCommand.java:17-23, SayCommand.java:15-28, SeedCommand.java:12-21,
//! OpCommand.java:23-38, DeOpCommands.java:21-31, PardonCommand.java:21-31,
//! StopCommand.java:11-18. Debug give is a retained Solaris fixture.
//! EffectCommands.java:32-151.
//! AdvancementCommands.java:33-229 (resource-key advancement and greedy criterion).
//! LocateCommand.java:56-87, TickCommand.java:20-55,
//! TellRawCommand.java:12-30, TitleCommand.java:26-102,
//! BossBarCommands.java:65-176.
//! Suggestion provider names: net/minecraft/commands/synchronization/
//! SuggestionProviders.java:19-40. Unsupported vanilla branches (such as
//! teleport facing and fill strict) are intentionally not advertised.

use mc_core::command_rules::{RuleKind, rules};
use mc_data::Identifier;
use mc_protocol::packets::play::{
    ClientboundCommandSuggestions, ClientboundCommands, CommandArgumentParser as Parser,
    CommandNode, CommandNodeKind, CommandStringKind, CommandSuggestionEntry,
};
mod subtrees;
use subtrees::*;
mod suggest;
pub(crate) use suggest::*;

const ROOTS: &[(&str, u8)] = &[
    ("help", 0),
    ("list", 0),
    ("me", 0),
    ("msg", 0),
    ("tell", 0),
    ("w", 0),
    ("teammsg", 0),
    ("tm", 0),
    ("settlement", 0),
    ("trigger", 0),
    ("gamemode", 2),
    ("teleport", 2),
    ("tp", 2),
    ("execute", 2),
    ("give", 2),
    ("item", 2),
    ("loot", 2),
    ("attribute", 2),
    ("clear", 2),
    ("kill", 2),
    ("damage", 2),
    ("time", 2),
    ("weather", 2),
    ("difficulty", 2),
    ("gamerule", 2),
    ("summon", 2),
    ("worldborder", 2),
    ("setblock", 2),
    ("fill", 2),
    ("clone", 2),
    ("experience", 2),
    ("xp", 2),
    ("spawnpoint", 2),
    ("setworldspawn", 2),
    ("forceload", 2),
    ("say", 2),
    ("seed", 2),
    ("enchant", 2),
    ("effect", 2),
    ("advancement", 2),
    ("debug", 2),
    ("team", 2),
    ("tps", 2),
    ("mspt", 2),
    ("scoreboard", 2),
    ("locate", 2),
    ("tellraw", 2),
    ("title", 2),
    ("bossbar", 2),
    ("function", 2),
    ("schedule", 2),
    ("datapack", 2),
    ("reload", 2),
    ("data", 2),
    ("tag", 2),
    ("playsound", 2),
    ("particle", 2),
    ("tick", 3),
    ("kick", 3),
    ("mute", 3),
    ("unmute", 3),
    ("warn", 3),
    ("transfer", 3),
    ("ban", 3),
    ("ban-ip", 3),
    ("pardon", 3),
    ("pardon-ip", 3),
    ("op", 3),
    ("deop", 3),
    ("whitelist", 3),
    ("banlist", 3),
    ("co", 3),
    ("stop", 4),
    ("save-all", 4),
];

const SETTLEMENT_HELP: &[(&str, &str)] = &[
    ("guide", "Step-by-step in-game settlement tutorial."),
    (
        "menu",
        "Open the settlement dialogs (also: /settlement alone). Pages: main, buildings, certify, residents, production, army, projects, settings.",
    ),
    (
        "building",
        "Certify or upgrade player-built buildings: list | certify <kind> <x1 y1 z1 x2 y2 z2> | upgrade <id>.",
    ),
    (
        "build",
        "Start a fully funded blueprint building or native-built upgrade: <blueprint> | upgrade <buildingId>. Owner/operator of the claimed settlement here; one active project, real materials from storage.",
    ),
    (
        "create",
        "Found a settlement here; optional radius 1-256 (default 48).",
    ),
    (
        "claim",
        "Claim the autonomous settlement you stand in (ownerless villages refuse other mutations).",
    ),
    ("list", "List all settlements."),
    ("info", "Show owner, centre, radius and resident count."),
    ("rename", "Rename a settlement (owner/operator)."),
    (
        "remove",
        "Dissolve a settlement and release membership (owner/operator).",
    ),
    (
        "bind",
        "Bind the nearest villager within 8 blocks, or supply a UUID (owner/operator).",
    ),
    (
        "unbind",
        "Release the nearest villager within 8 blocks, or supply a UUID (owner/operator).",
    ),
    (
        "relocate",
        "Move a household; requires ownership of both towns or operator.",
    ),
    (
        "production",
        "Run real worker recipes: status/start/pause/resume/cancel.",
    ),
    (
        "commerce",
        "Trade and escrow: status/quote/accept/cancel/reserve/payout.",
    ),
    (
        "payroll",
        "Fund worker contracts: status/employ/stop/dismiss/pay.",
    ),
    (
        "caravan",
        "Physical transport: status/open/pickup/unload/cancel/block.",
    ),
    (
        "army",
        "Native soldiers: list | recruit | dismiss <uuid> | order <all|uuid> follow|guard|patrol|return (owner/operator for mutations).",
    ),
];

pub(crate) const SETTLEMENT_GUIDE: &str = concat!(
    "Settlements / Поселения — quick start:\n",
    "1. Stand at your town centre: /settlement create Oak (optional radius 1-256; default 48).\n",
    "   Then /settlement (or /settlement menu) opens the dialogs: buildings (certify/upgrade), residents & jobs, production orders, settings.\n",
    "2. Place beds and workstations: composter=Farmer, lectern=Librarian, barrel=Fisherman; /help settlement lists all jobs.\n",
    "3. Keep the village loaded: adults take free stations during day ticks 2000-8999, within 48 blocks via a walkable path. Nitwits never work; traded villagers keep their profession.\n",
    "4. Stand next to a villager: /settlement bind Oak (nearest within 8 blocks). /settlement unbind releases the nearest; optional native UUID targets one explicitly. Villagers work without binding; bound villagers search inside the town.\n",
    "5. Read villagers' nameplates/status labels; /settlement info Oak shows population and jobs, /settlement list shows towns. Changes require owner/operator; reads and founding are public.\n",
    "6. /settlement production Oak status — real worker recipes (start/pause/resume/cancel).\n",
    "7. /settlement commerce Oak status — emerald trades and escrow (quote/accept/cancel/reserve/payout).\n",
    "8. /settlement payroll Oak status — funded worker contracts (employ/stop/dismiss/pay).\n",
    "9. /settlement caravan Oak status — physical deliveries (open/pickup/unload/cancel/block).\n",
    "Action syntax: /help settlement <domain> Oak. Short bilingual guide: docs/SETTLEMENTS_GUIDE.md",
);

// Native economy uses compact endpoint/coordinate/route tokens (":" / "," /
// ";") that Brigadier's unquoted word parser rejects. Advertise each action's
// native tail as a greedy string, leaving exact token validation to the tick.
const SETTLEMENT_ECONOMY: &[(&str, &str, &str)] = &[
    (
        "production",
        "start",
        "job worker-UUID recipe count building station-x,y,z stand-at-x,y,z",
    ),
    ("production", "pause", "job"),
    ("production", "resume", "job"),
    ("production", "cancel", "job"),
    (
        "commerce",
        "quote",
        "quote seller-endpoint buyer-endpoint slot quantity price",
    ),
    ("commerce", "accept", "quote"),
    ("commerce", "cancel", "quote"),
    (
        "commerce",
        "reserve",
        "mandate shipment payer-endpoint escrow-endpoint payee-endpoint price",
    ),
    ("commerce", "payout", "mandate"),
    (
        "payroll",
        "employ",
        "contract worker-UUID treasury-endpoint wage period budget",
    ),
    ("payroll", "stop", "contract"),
    ("payroll", "dismiss", "contract"),
    ("payroll", "pay", "contract"),
    (
        "caravan",
        "open",
        "shipment carrier-UUID src-building src-x,y,z dst-building dst-x,y,z slot quantity ticks waypoints [paid-mandate]",
    ),
    ("caravan", "pickup", "shipment"),
    ("caravan", "unload", "shipment"),
    ("caravan", "cancel", "shipment"),
    ("caravan", "block", "shipment reason"),
];

pub(crate) fn permitted(command: &str, op_level: u8, plugins: &[(String, u8)]) -> bool {
    command
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .is_some_and(|root| {
            if let Some((_, required)) = plugins.iter().find(|(name, _)| root == name) {
                op_level >= *required
            } else {
                ROOTS
                    .iter()
                    .any(|&(name, required)| root == name && op_level >= required)
            }
        })
}

/// Permission-filtered, tree-derived usage for actual command arguments and
/// optional named argument placeholders, including alias redirects.
///
/// Rendering follows `CommandDispatcher.getSmartUsage`:
/// https://github.com/Mojang/brigadier/blob/master/src/main/java/com/mojang/brigadier/CommandDispatcher.java
/// The full path prefix follows vanilla `HelpCommand.java:18-50`.
pub(crate) fn help(
    command: Option<&str>,
    op_level: u8,
    plugins: &[(String, u8)],
) -> Result<Vec<String>, String> {
    let packet = tree(op_level, plugins);
    let path = command.map(|path| path.trim().trim_start_matches('/'));
    let mut index = packet.root_index;
    if let Some(path) = path {
        if path.is_empty() {
            return Err("Unknown or unavailable command path.".to_owned());
        }
        let (parsed, consumed) = help_path(&packet, index, path);
        if consumed == 0 {
            return Err(format!("Unknown or unavailable command path: {path}"));
        }
        index = parsed;
    }
    if let Some(redirect) = packet.nodes[index as usize].redirect {
        index = redirect;
    }
    let parent = &packet.nodes[index as usize];
    let settlement_help =
        path == Some("settlement") && !plugins.iter().any(|(root, _)| root == "settlement");
    let mut usages =
        Vec::with_capacity(parent.children.len() + if settlement_help { 4 } else { 0 });
    for &child in &parent.children {
        let mut line = String::from("/");
        if let Some(path) = path {
            line.push_str(path);
            line.push(' ');
        }
        smart_usage(&packet, child, parent.executable, false, &mut line);
        if settlement_help
            && let CommandNodeKind::Literal(name) = &packet.nodes[child as usize].kind
            && let Some((_, description)) = SETTLEMENT_HELP.iter().find(|entry| entry.0 == name)
        {
            line.push_str(" — ");
            line.push_str(description);
        }
        usages.push(line);
    }
    if settlement_help {
        usages.push("Jobs: adults use free workstations during day ticks 2000-8999, within 48 blocks with a walkable path; keep the village loaded.".into());
        usages.push("composter=Farmer, lectern=Librarian, barrel=Fisherman, smoker=Butcher, blast_furnace=Armorer, brewing_stand=Cleric.".into());
        usages.push("cartography_table=Cartographer, fletching_table=Fletcher, cauldron=Leatherworker, stonecutter=Mason, loom=Shepherd, smithing_table=Toolsmith, grindstone=Weaponsmith.".into());
        usages.push("Unbound villagers need no command; bound villagers search inside their town. Nitwits never work; traded villagers keep their profession. Guide: docs/SETTLEMENTS_GUIDE.md".into());
    }
    Ok(usages)
}

/// Follow the branch that parses the most input. Literal matches take priority,
/// and an incomplete suffix retains its last parsed node, as vanilla help does.
fn help_path(packet: &ClientboundCommands, mut index: i32, input: &str) -> (i32, usize) {
    if let Some(redirect) = packet.nodes[index as usize].redirect {
        index = redirect;
    }
    let tail = input.trim_start();
    let leading = input.len() - tail.len();
    if tail.is_empty() {
        return (index, leading);
    }
    let word_end = tail.find(char::is_whitespace).unwrap_or(tail.len());
    let word = &tail[..word_end];
    let node = &packet.nodes[index as usize];
    if let Some(&literal) = node.children.iter().find(|&&child| {
        matches!(&packet.nodes[child as usize].kind, CommandNodeKind::Literal(name) if name == word)
    }) {
        let (parsed, consumed) = help_path(packet, literal, &tail[word_end..]);
        return (parsed, leading + word_end + consumed);
    }
    let mut best = (index, 0);
    for &child in &node.children {
        let CommandNodeKind::Argument { name, parser, .. } = &packet.nodes[child as usize].kind
        else {
            continue;
        };
        let placeholder = name == word
            || word
                .strip_prefix('<')
                .and_then(|word| word.strip_suffix('>'))
                == Some(name.as_str());
        let Some(length) = (if placeholder {
            Some(word_end)
        } else {
            help_argument_length(parser, tail)
        }) else {
            continue;
        };
        let (parsed, consumed) = help_path(packet, child, &tail[length..]);
        let consumed = leading + length + consumed;
        if consumed > best.1 {
            best = (parsed, consumed);
        }
    }
    best
}

/// Lexical argument traversal, not execution or registry/target resolution.
/// Balanced component/item/block/selector text stays a single argument.
fn help_token_length(input: &str) -> Option<usize> {
    let mut square = 0usize;
    let mut curly = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, c) in input.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == delimiter {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '[' => square += 1,
            ']' => square = square.checked_sub(1)?,
            '{' => curly += 1,
            '}' => curly = curly.checked_sub(1)?,
            _ if c.is_whitespace() && square == 0 && curly == 0 => return Some(index),
            _ => {}
        }
    }
    (quote.is_none() && square == 0 && curly == 0 && !input.is_empty()).then_some(input.len())
}

fn help_coordinates(input: &str, count: usize, integer: bool) -> Option<usize> {
    let mut consumed = 0;
    for _ in 0..count {
        let remaining = &input[consumed..];
        let tail = remaining.trim_start();
        consumed += remaining.len() - tail.len();
        let length = tail.find(char::is_whitespace).unwrap_or(tail.len());
        let coordinate = &tail[..length];
        let relative = coordinate.starts_with(['~', '^']);
        let number = if relative {
            &coordinate[1..]
        } else {
            coordinate
        };
        if !(relative && number.is_empty()) {
            if integer && !relative {
                number.parse::<i32>().ok()?;
            } else if !number.parse::<f64>().ok()?.is_finite() {
                return None;
            }
        }
        consumed += length;
    }
    Some(consumed)
}

fn help_argument_length(parser: &Parser, input: &str) -> Option<usize> {
    match parser {
        Parser::Message | Parser::String(CommandStringKind::GreedyPhrase) => {
            return Some(input.len());
        }
        Parser::Vec3 => return help_coordinates(input, 3, false),
        Parser::BlockPos => return help_coordinates(input, 3, true),
        Parser::Vec2 | Parser::Rotation => return help_coordinates(input, 2, false),
        Parser::ColumnPos => return help_coordinates(input, 2, true),
        Parser::Angle => return help_coordinates(input, 1, false),
        _ => {}
    }
    let length = help_token_length(input)?;
    let word = &input[..length];
    let valid = match parser {
        Parser::Bool => matches!(word, "true" | "false"),
        Parser::Integer { min, max } => word.parse::<i32>().ok().is_some_and(|value| {
            min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
        }),
        Parser::Long { min, max } => word.parse::<i64>().ok().is_some_and(|value| {
            min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
        }),
        Parser::Float { min, max } => word.parse::<f32>().ok().is_some_and(|value| {
            value.is_finite()
                && min.is_none_or(|min| value >= f32::from_bits(min))
                && max.is_none_or(|max| value <= f32::from_bits(max))
        }),
        Parser::Double { min, max } => word.parse::<f64>().ok().is_some_and(|value| {
            value.is_finite()
                && min.is_none_or(|min| value >= f64::from_bits(min))
                && max.is_none_or(|max| value <= f64::from_bits(max))
        }),
        Parser::Time { min } => {
            let (number, multiplier) = match word.as_bytes().last() {
                Some(b'd') => (&word[..word.len() - 1], 24_000.0),
                Some(b's') => (&word[..word.len() - 1], 20.0),
                Some(b't') => (&word[..word.len() - 1], 1.0),
                _ => (word, 1.0),
            };
            number.parse::<f64>().ok().is_some_and(|number| {
                let ticks = (number * multiplier).round();
                ticks.is_finite() && ticks >= f64::from(*min) && ticks <= f64::from(i32::MAX)
            })
        }
        Parser::GameMode => matches!(word, "survival" | "creative" | "adventure" | "spectator"),
        Parser::String(CommandStringKind::SingleWord) => word
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+')),
        Parser::Entity {
            single,
            players_only,
        } => {
            if word.starts_with('@') {
                let selector = word.as_bytes().get(1).copied();
                matches!(selector, Some(b'a' | b'e' | b'n' | b'p' | b'r' | b's'))
                    && (!single
                        || !matches!(selector, Some(b'a' | b'e'))
                        || word.contains("limit=1"))
                    && (!players_only
                        || !matches!(selector, Some(b'e' | b'n'))
                        || word.contains("type=player")
                        || word.contains("type=minecraft:player"))
            } else {
                word.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            }
        }
        Parser::ResourceLocation | Parser::Resource { .. } | Parser::ResourceKey { .. } => {
            word.bytes().all(|c| {
                c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, b'_' | b'-' | b'.' | b'/' | b':')
            })
        }
        Parser::Operation => matches!(
            word,
            "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "<" | ">" | "><"
        ),
        _ => true,
    };
    valid.then_some(length)
}

fn usage_text(node: &CommandNode, output: &mut String) {
    match &node.kind {
        CommandNodeKind::Root => {}
        CommandNodeKind::Literal(name) => output.push_str(name),
        CommandNodeKind::Argument { name, .. } => {
            output.push('<');
            output.push_str(name);
            output.push('>');
        }
    }
}

fn smart_usage(
    packet: &ClientboundCommands,
    index: i32,
    optional: bool,
    deep: bool,
    output: &mut String,
) {
    let node = &packet.nodes[index as usize];
    if optional {
        output.push('[');
    }
    usage_text(node, output);
    if optional {
        output.push(']');
    }
    if deep {
        return;
    }
    if let Some(redirect) = node.redirect {
        if redirect == packet.root_index {
            output.push_str(" ...");
        } else {
            output.push_str(" -> ");
            usage_text(&packet.nodes[redirect as usize], output);
        }
        return;
    }
    match node.children.as_slice() {
        [] => {}
        [child] => {
            output.push(' ');
            smart_usage(packet, *child, node.executable, node.executable, output);
        }
        children => {
            // This tree has distinct sibling usage texts, so Brigadier's
            // alternative deduplication never needs an intermediate set.
            output.push(' ');
            output.push(if node.executable { '[' } else { '(' });
            for (position, &child) in children.iter().enumerate() {
                if position > 0 {
                    output.push('|');
                }
                usage_text(&packet.nodes[child as usize], output);
            }
            output.push(if node.executable { ']' } else { ')' });
        }
    }
}

fn identifier(path: &str) -> Identifier {
    Identifier::parse(format!("minecraft:{path}")).expect("static vanilla identifier")
}

/// Vanilla data-pack structure ids (plus every `#tag`) backing
/// `locate structure` server suggestions: the client has no structure
/// registry, so the tree advertises `ask_server` for it (LocateCommand
/// suggests through the command context, not a synced registry).
/// Filesystem layout is `data/minecraft/worldgen/structure/*.json` and
/// `data/minecraft/tags/worldgen/structure/*.json`; entries load the same
/// way startup suggestion lists do. Missing/unreadable sidecar data yields
/// an empty list rather than failing startup.
pub(crate) fn structure_suggestion_ids(vanilla_data_dir: &std::path::Path) -> Vec<String> {
    use std::collections::BTreeSet;
    let root = vanilla_data_dir.join("data").join("minecraft");
    let mut ids = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(root.join("worldgen").join("structure")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                ids.insert(format!("minecraft:{stem}"));
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(root.join("tags").join("worldgen").join("structure")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            {
                ids.insert(format!("#minecraft:{stem}"));
            }
        }
    }
    ids.into_iter().collect()
}

fn integer(min: Option<i32>, max: Option<i32>) -> Parser {
    Parser::Integer {
        min: min.filter(|&value| value != i32::MIN),
        max: max.filter(|&value| value != i32::MAX),
    }
}

fn players(single: bool) -> Parser {
    Parser::Entity {
        single,
        players_only: true,
    }
}

fn entities(single: bool) -> Parser {
    Parser::Entity {
        single,
        players_only: false,
    }
}

struct Tree<'a> {
    nodes: Vec<CommandNode>,
    op_level: u8,
    plugins: &'a [(String, u8)],
}

impl<'a> Tree<'a> {
    fn new(op_level: u8, plugins: &'a [(String, u8)]) -> Self {
        let capacity = if op_level >= 2 {
            rules().len() * 4 + 256
        } else {
            256
        };
        let mut nodes = Vec::with_capacity(capacity);
        nodes.push(CommandNode::root(Vec::with_capacity(ROOTS.len())));
        Self {
            nodes,
            op_level,
            plugins,
        }
    }

    fn root(&mut self, name: &str, executable: bool) -> Option<i32> {
        if self.plugins.iter().any(|(root, _)| root == name) {
            return None;
        }
        let level = ROOTS.iter().find(|(root, _)| *root == name)?.1;
        if self.op_level < level {
            return None;
        }
        let node = self.literal(0, name, executable);
        self.nodes[node as usize].restricted = level > 0;
        Some(node)
    }

    /// Keep the native schema reachable for its independently owned aliases,
    /// without advertising a second root when a plugin owns the literal.
    fn aliased_root(&mut self, name: &str, executable: bool) -> Option<i32> {
        if !self.plugins.iter().any(|(root, _)| root == name) {
            return self.root(name, executable);
        }
        let level = ROOTS.iter().find(|(root, _)| *root == name)?.1;
        if self.op_level < level {
            return None;
        }
        let index = self.nodes.len() as i32;
        let mut node = CommandNode::literal(name, Vec::new(), executable);
        node.restricted = level > 0;
        self.nodes.push(node);
        Some(index)
    }

    fn push(&mut self, parent: i32, node: CommandNode) -> i32 {
        let index = self.nodes.len() as i32;
        self.nodes.push(node);
        self.nodes[parent as usize].children.push(index);
        index
    }

    fn literal(&mut self, parent: i32, name: &str, executable: bool) -> i32 {
        self.push(parent, CommandNode::literal(name, Vec::new(), executable))
    }

    fn argument(&mut self, parent: i32, name: &str, parser: Parser, executable: bool) -> i32 {
        self.push(
            parent,
            CommandNode::argument(name, parser, Vec::new(), executable),
        )
    }

    fn suggested(
        &mut self,
        parent: i32,
        name: &str,
        parser: Parser,
        executable: bool,
        provider: &str,
    ) -> i32 {
        let index = self.argument(parent, name, parser, executable);
        if let CommandNodeKind::Argument { suggestions, .. } = &mut self.nodes[index as usize].kind
        {
            *suggestions = Some(identifier(provider));
        }
        index
    }

    fn literals(&mut self, parent: i32, names: &[&str]) {
        for name in names {
            self.literal(parent, name, true);
        }
    }

    fn alias(&mut self, name: &str, target: i32) {
        if let Some(index) = self.root(name, false) {
            self.nodes[index as usize].redirect = Some(target);
        }
    }
}

pub(crate) fn tree(op_level: u8, plugins: &[(String, u8)]) -> ClientboundCommands {
    let mut t = Tree::new(op_level, plugins);
    t.root("reload", true);
    if let Some(root) = t.root("data", false) {
        let get = t.literal(root, "get", false);
        for kind in ["entity", "block", "storage"] {
            let branch = t.literal(get, kind, false);
            let parser = match kind {
                "entity" => entities(true),
                "block" => Parser::BlockPos,
                _ => Parser::ResourceLocation,
            };
            let target = t.argument(branch, "target", parser, true);
            let path = t.argument(target, "path", Parser::NbtPath, true);
            t.argument(
                path,
                "scale",
                Parser::Double {
                    min: None,
                    max: None,
                },
                true,
            );
        }
        let merge = t.literal(root, "merge", false);
        for kind in ["block", "storage"] {
            let branch = t.literal(merge, kind, false);
            let parser = if kind == "block" {
                Parser::BlockPos
            } else {
                Parser::ResourceLocation
            };
            let target = t.argument(branch, "target", parser, false);
            t.argument(target, "nbt", Parser::NbtCompoundTag, true);
        }
        let remove = t.literal(root, "remove", false);
        let storage = t.literal(remove, "storage", false);
        let target = t.argument(storage, "target", Parser::ResourceLocation, false);
        t.argument(target, "path", Parser::NbtPath, true);
        let modify = t.literal(root, "modify", false);
        let storage = t.literal(modify, "storage", false);
        let target = t.argument(storage, "target", Parser::ResourceLocation, false);
        let path = t.argument(target, "path", Parser::NbtPath, false);
        for operation in ["set", "merge", "append", "prepend", "insert"] {
            let action = t.literal(path, operation, false);
            let action = if operation == "insert" {
                t.argument(action, "index", integer(None, None), false)
            } else {
                action
            };
            let value = t.literal(action, "value", false);
            t.argument(value, "value", Parser::NbtTag, true);
            let from = t.literal(action, "from", false);
            let storage = t.literal(from, "storage", false);
            let source = t.argument(storage, "source", Parser::ResourceLocation, true);
            t.argument(source, "path", Parser::NbtPath, true);
        }
    }
    if let Some(root) = t.root("tag", false) {
        let targets = t.argument(root, "targets", entities(false), false);
        for action in ["add", "remove"] {
            let action = t.literal(targets, action, false);
            t.argument(
                action,
                "name",
                Parser::String(CommandStringKind::SingleWord),
                true,
            );
        }
        t.literal(targets, "list", true);
    }
    if let Some(root) = t.root("playsound", false) {
        let sound = t.suggested(
            root,
            "sound",
            Parser::String(CommandStringKind::SingleWord),
            true,
            "ask_server",
        );
        for source in [
            "master", "music", "record", "weather", "block", "hostile", "neutral", "player",
            "ambient", "voice",
        ] {
            let source = t.literal(sound, source, true);
            let targets = t.argument(source, "targets", players(false), true);
            let pos = t.argument(targets, "pos", Parser::Vec3, true);
            let volume = t.argument(
                pos,
                "volume",
                Parser::Float {
                    min: Some(0.0_f32.to_bits()),
                    max: None,
                },
                true,
            );
            let pitch = t.argument(
                volume,
                "pitch",
                Parser::Float {
                    min: Some(0.0_f32.to_bits()),
                    max: Some(2.0_f32.to_bits()),
                },
                true,
            );
            t.argument(
                pitch,
                "minVolume",
                Parser::Float {
                    min: Some(0.0_f32.to_bits()),
                    max: Some(1.0_f32.to_bits()),
                },
                true,
            );
        }
    }
    if let Some(root) = t.root("particle", false) {
        let name = t.suggested(
            root,
            "name",
            Parser::String(CommandStringKind::SingleWord),
            true,
            "ask_server",
        );
        let pos = t.argument(name, "pos", Parser::Vec3, true);
        let delta = t.argument(pos, "delta", Parser::Vec3, false);
        let speed = t.argument(
            delta,
            "speed",
            Parser::Float {
                min: None,
                max: None,
            },
            false,
        );
        let count = t.argument(speed, "count", integer(Some(0), None), true);
        for mode in ["normal", "force"] {
            let mode = t.literal(count, mode, true);
            t.argument(mode, "viewers", players(false), true);
        }
    }
    if let Some(root) = t.root("function", false) {
        t.argument(
            root,
            "name",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("schedule", false) {
        let function = t.literal(root, "function", false);
        t.argument(
            function,
            "arguments",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
        let clear = t.literal(root, "clear", false);
        t.argument(
            clear,
            "name",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("datapack", false) {
        let list = t.literal(root, "list", true);
        t.literal(list, "enabled", true);
    }
    if let Some(root) = t.root("help", true) {
        t.argument(
            root,
            "command",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("list", true) {
        t.literal(root, "uuids", true);
    }
    if let Some(root) = t.root("me", false) {
        t.argument(root, "action", Parser::Message, true);
    }
    if let Some(root) = t.aliased_root("msg", false) {
        let targets = t.argument(root, "targets", players(false), false);
        t.argument(targets, "message", Parser::Message, true);
        t.alias("tell", root);
        t.alias("w", root);
    }
    if let Some(root) = t.aliased_root("teammsg", false) {
        t.argument(root, "message", Parser::Message, true);
        t.alias("tm", root);
    }
    if let Some(root) = t.root("settlement", false) {
        settlement_tree(&mut t, root);
    }
    if let Some(root) = t.root("gamemode", false) {
        let mode = t.argument(root, "gamemode", Parser::GameMode, true);
        t.argument(mode, "target", players(false), true);
    }
    if let Some(root) = t.aliased_root("teleport", false) {
        let location = t.argument(root, "location", Parser::Vec3, true);
        t.argument(location, "rotation", Parser::Rotation, true);
        t.argument(root, "destination", entities(true), true);
        let targets = t.argument(root, "targets", entities(false), false);
        let location = t.argument(targets, "location", Parser::Vec3, true);
        t.argument(location, "rotation", Parser::Rotation, true);
        t.argument(targets, "destination", entities(true), true);
        t.alias("tp", root);
    }
    if let Some(root) = t.root("execute", false) {
        t.argument(
            root,
            "arguments",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
    if let Some(root) = t.root("item", false) {
        admin_item_tree(&mut t, root);
    }
    if let Some(root) = t.root("loot", false) {
        admin_loot_tree(&mut t, root);
    }
    if let Some(root) = t.root("attribute", false) {
        admin_attribute_tree(&mut t, root);
    }
    if let Some(root) = t.root("give", false) {
        let targets = t.argument(root, "targets", players(false), false);
        let item = t.argument(targets, "item", Parser::ItemStack, true);
        t.argument(item, "count", integer(Some(1), None), true);
    }
    if let Some(root) = t.root("clear", true) {
        let targets = t.argument(root, "targets", players(false), true);
        let item = t.argument(targets, "item", Parser::ItemPredicate, true);
        t.argument(item, "maxCount", integer(Some(0), None), true);
    }
    if let Some(root) = t.root("kill", true) {
        t.argument(root, "targets", entities(false), true);
    }
    if let Some(root) = t.root("damage", false) {
        let target = t.argument(root, "target", entities(true), false);
        let amount = t.argument(
            target,
            "amount",
            Parser::Float {
                min: Some(0.0_f32.to_bits()),
                max: None,
            },
            true,
        );
        let kind = t.argument(
            amount,
            "damageType",
            Parser::Resource {
                registry: identifier("damage_type"),
            },
            true,
        );
        let at = t.literal(kind, "at", false);
        t.argument(at, "location", Parser::Vec3, true);
        let by = t.literal(kind, "by", false);
        let direct = t.argument(by, "entity", entities(true), true);
        let from = t.literal(direct, "from", false);
        t.argument(from, "cause", entities(true), true);
    }
    if let Some(root) = t.root("time", false) {
        let set = t.literal(root, "set", false);
        t.argument(set, "time", Parser::Time { min: 0 }, true);
        t.suggested(
            set,
            "timemarker",
            Parser::ResourceLocation,
            true,
            "ask_server",
        );
        let add = t.literal(root, "add", false);
        t.argument(add, "time", Parser::Time { min: i32::MIN }, true);
        let query = t.literal(root, "query", false);
        t.literals(query, &["time", "daytime", "day", "gametime"]);
    }
    if let Some(root) = t.root("weather", false) {
        for name in ["clear", "rain", "thunder"] {
            let kind = t.literal(root, name, true);
            t.argument(kind, "duration", Parser::Time { min: 1 }, true);
        }
    }
    if let Some(root) = t.root("difficulty", true) {
        t.literals(root, &["peaceful", "easy", "normal", "hard"]);
    }
    if let Some(root) = t.root("gamerule", false) {
        for rule in rules() {
            let parser = match rule.kind {
                RuleKind::Boolean => Parser::Bool,
                RuleKind::Integer { min, max } => integer(Some(min), Some(max)),
            };
            let short = t.literal(root, rule.name, true);
            t.argument(short, "value", parser.clone(), true);
            let qualified = t.literal(root, &format!("minecraft:{}", rule.name), true);
            t.argument(qualified, "value", parser, true);
        }
    }
    if let Some(root) = t.root("summon", false) {
        let entity = t.suggested(
            root,
            "entity",
            Parser::Resource {
                registry: identifier("entity_type"),
            },
            true,
            "summonable_entities",
        );
        let pos = t.argument(entity, "pos", Parser::Vec3, true);
        t.argument(pos, "nbt", Parser::NbtCompoundTag, true);
    }
    if let Some(root) = t.root("worldborder", false) {
        t.literal(root, "get", true);
        let center = t.literal(root, "center", false);
        t.argument(center, "pos", Parser::Vec2, true);
        let set = t.literal(root, "set", false);
        t.argument(
            set,
            "distance",
            Parser::Double {
                min: Some(1.0_f64.to_bits()),
                max: Some(59_999_968.0_f64.to_bits()),
            },
            true,
        );
        let add = t.literal(root, "add", false);
        t.argument(
            add,
            "distance",
            Parser::Double {
                min: None,
                max: None,
            },
            true,
        );
    }
    if let Some(root) = t.root("setblock", false) {
        let pos = t.argument(root, "pos", Parser::BlockPos, false);
        let block = t.argument(pos, "block", Parser::BlockState, true);
        t.literals(block, &["replace", "destroy", "keep"]);
    }
    if let Some(root) = t.root("fill", false) {
        let from = t.argument(root, "from", Parser::BlockPos, false);
        let to = t.argument(from, "to", Parser::BlockPos, false);
        let block = t.argument(to, "block", Parser::BlockState, true);
        t.literals(block, &["destroy", "keep", "hollow", "outline"]);
        let replace = t.literal(block, "replace", true);
        t.argument(replace, "filter", Parser::BlockPredicate, true);
    }
    if let Some(root) = t.root("clone", false) {
        let from = t.argument(root, "begin", Parser::BlockPos, false);
        let to = t.argument(from, "end", Parser::BlockPos, false);
        let destination = t.argument(to, "destination", Parser::BlockPos, true);
        for filter in ["replace", "masked"] {
            let node = t.literal(destination, filter, true);
            t.literals(node, &["normal", "force", "move"]);
        }
        let filtered = t.literal(destination, "filtered", false);
        let filter = t.argument(filtered, "filter", Parser::BlockPredicate, true);
        t.literals(filter, &["normal", "force", "move"]);
    }
    if let Some(root) = t.root("advancement", false) {
        for action in ["grant", "revoke"] {
            let action = t.literal(root, action, false);
            let targets = t.argument(action, "targets", players(false), false);
            for mode in ["only", "from", "until", "through"] {
                let mode_node = t.literal(targets, mode, false);
                let advancement = t.argument(
                    mode_node,
                    "advancement",
                    Parser::ResourceKey {
                        registry: identifier("advancement"),
                    },
                    true,
                );
                if mode == "only" {
                    t.suggested(
                        advancement,
                        "criterion",
                        Parser::String(CommandStringKind::GreedyPhrase),
                        true,
                        "ask_server",
                    );
                }
            }
            t.literal(targets, "everything", true);
        }
    }
    if let Some(root) = t.aliased_root("experience", false) {
        for (name, min) in [("add", None), ("set", Some(0))] {
            let operation = t.literal(root, name, false);
            let target = t.argument(operation, "target", players(false), false);
            let amount = t.argument(target, "amount", integer(min, None), true);
            t.literals(amount, &["points", "levels"]);
        }
        let query = t.literal(root, "query", false);
        let target = t.argument(query, "target", players(true), false);
        t.literals(target, &["points", "levels"]);
        t.alias("xp", root);
    }
    if let Some(root) = t.root("spawnpoint", true) {
        let targets = t.argument(root, "targets", players(false), true);
        let pos = t.argument(targets, "pos", Parser::BlockPos, true);
        t.argument(pos, "rotation", Parser::Rotation, true);
    }
    if let Some(root) = t.root("setworldspawn", true) {
        let pos = t.argument(root, "pos", Parser::BlockPos, true);
        t.argument(pos, "rotation", Parser::Rotation, true);
    }
    if let Some(root) = t.root("forceload", false) {
        let add = t.literal(root, "add", false);
        let from = t.argument(add, "from", Parser::ColumnPos, true);
        t.argument(from, "to", Parser::ColumnPos, true);
        let remove = t.literal(root, "remove", false);
        let from = t.argument(remove, "from", Parser::ColumnPos, true);
        t.argument(from, "to", Parser::ColumnPos, true);
        t.literal(remove, "all", true);
        let query = t.literal(root, "query", true);
        t.argument(query, "pos", Parser::ColumnPos, true);
    }
    if let Some(root) = t.root("say", false) {
        t.argument(root, "message", Parser::Message, true);
    }
    let _ = t.root("seed", true);
    // MetricsMgmt: argument-less server pace reports (handled in server.rs).
    let _ = t.root("tps", true);
    let _ = t.root("mspt", true);
    if let Some(root) = t.root("enchant", false) {
        let targets = t.argument(root, "targets", entities(false), false);
        let enchantment = t.argument(
            targets,
            "enchantment",
            Parser::Resource {
                registry: identifier("enchantment"),
            },
            true,
        );
        t.argument(enchantment, "level", integer(Some(0), None), true);
    }
    if let Some(root) = t.root("effect", false) {
        let clear = t.literal(root, "clear", true);
        let targets = t.argument(clear, "targets", entities(false), true);
        t.argument(
            targets,
            "effect",
            Parser::Resource {
                registry: identifier("mob_effect"),
            },
            true,
        );
        let give = t.literal(root, "give", false);
        let targets = t.argument(give, "targets", entities(false), false);
        let effect = t.argument(
            targets,
            "effect",
            Parser::Resource {
                registry: identifier("mob_effect"),
            },
            true,
        );
        let seconds = t.argument(effect, "seconds", integer(Some(1), Some(1_000_000)), true);
        let infinite = t.literal(effect, "infinite", true);
        for duration in [seconds, infinite] {
            let amplifier = t.argument(duration, "amplifier", integer(Some(0), Some(255)), true);
            t.argument(amplifier, "hideParticles", Parser::Bool, true);
        }
    }
    if let Some(root) = t.root("debug", false) {
        // Solaris's retained fixture, not vanilla DebugCommand's profiling tree.
        let give = t.literal(root, "give", false);
        let item = t.argument(give, "item", Parser::ItemStack, true);
        let count = t.argument(item, "count", integer(None, None), true);
        t.argument(count, "hotbar_slot", integer(Some(0), Some(8)), true);
    }
    if let Some(root) = t.root("team", false) {
        team_tree(&mut t, root);
    }
    if let Some(root) = t.root("scoreboard", false) {
        scoreboard_tree(&mut t, root);
    }
    if let Some(root) = t.root("trigger", false) {
        let objective = t.argument(root, "objective", Parser::Objective, true);
        for name in ["add", "set"] {
            let operation = t.literal(objective, name, false);
            t.argument(operation, "value", integer(None, None), true);
        }
    }
    if let Some(root) = t.root("kick", false) {
        let targets = t.argument(root, "targets", players(false), true);
        t.argument(targets, "reason", Parser::Message, true);
    }
    if let Some(root) = t.root("transfer", false) {
        // Vanilla TransferCommand: /transfer <hostname> [port] [players].
        let hostname = t.argument(
            root,
            "hostname",
            Parser::String(CommandStringKind::QuotablePhrase),
            true,
        );
        let port = t.argument(hostname, "port", integer(Some(1), Some(65_535)), true);
        t.argument(port, "players", players(false), true);
    }
    for name in ["ban", "pardon", "op", "deop", "mute", "unmute", "warn"] {
        if let Some(root) = t.root(name, false) {
            let targets = match name {
                "ban" | "mute" | "warn" => t.argument(root, "targets", Parser::GameProfile, true),
                _ => t.suggested(root, "targets", Parser::GameProfile, true, "ask_server"),
            };
            match name {
                // Solaris extends vanilla `/ban` with an optional duration, so
                // the reason starts one argument later when one is present.
                "ban" | "mute" => {
                    let duration = t.argument(
                        targets,
                        "duration",
                        Parser::String(CommandStringKind::SingleWord),
                        true,
                    );
                    t.argument(duration, "reason", Parser::Message, true);
                }
                "warn" => {
                    t.argument(targets, "reason", Parser::Message, true);
                }
                _ => {}
            }
        }
    }
    for name in ["ban-ip", "pardon-ip"] {
        if let Some(root) = t.root(name, false) {
            let target = if name == "pardon-ip" {
                t.suggested(
                    root,
                    "target",
                    Parser::String(CommandStringKind::SingleWord),
                    true,
                    "ask_server",
                )
            } else {
                t.argument(
                    root,
                    "target",
                    Parser::String(CommandStringKind::SingleWord),
                    true,
                )
            };
            if name == "ban-ip" {
                t.argument(target, "reason", Parser::Message, true);
            }
        }
    }
    if let Some(root) = t.root("whitelist", false) {
        // WhitelistCommand.java: the on/off/list/reload literals run without
        // arguments, add/remove take a resolved game profile.
        t.literals(root, &["on", "off", "list", "reload"]);
        for name in ["add", "remove"] {
            let literal = t.literal(root, name, false);
            t.argument(literal, "targets", Parser::GameProfile, true);
        }
    }
    if let Some(root) = t.root("banlist", false) {
        // BanListCommands.java: `/banlist players|ips`, never bare.
        t.literals(root, &["players", "ips"]);
    }
    if let Some(root) = t.root("co", true) {
        // Solaris block log (CoreProtect-style `u:` `t:` `r:` parameters).
        t.literals(root, &["inspect", "i", "help"]);
        for name in ["lookup", "l", "rollback", "rb", "restore", "rs"] {
            let literal = t.literal(root, name, true);
            t.argument(
                literal,
                "parameters",
                Parser::String(CommandStringKind::GreedyPhrase),
                true,
            );
        }
    }
    locate_tree(&mut t);
    tick_tree(&mut t);
    tellraw_title_bossbar_trees(&mut t);
    let _ = t.root("stop", true);
    if let Some(root) = t.root("save-all", true) {
        t.literal(root, "flush", true);
    }
    for (name, level) in plugins {
        if op_level >= *level {
            let root = t.literal(0, name, true);
            t.nodes[root as usize].restricted = *level > 0;
            t.argument(
                root,
                "arguments",
                Parser::String(CommandStringKind::GreedyPhrase),
                true,
            );
        }
    }
    ClientboundCommands {
        nodes: t.nodes,
        root_index: 0,
    }
}

fn admin_item_tree(t: &mut Tree<'_>, root: i32) {
    let replace = t.literal(root, "replace", false);
    for kind in ["entity", "block"] {
        let branch = t.literal(replace, kind, false);
        let target = t.argument(
            branch,
            "target",
            if kind == "entity" {
                entities(false)
            } else {
                Parser::BlockPos
            },
            false,
        );
        let slot = t.suggested(
            target,
            "slot",
            Parser::String(CommandStringKind::SingleWord),
            false,
            "ask_server",
        );
        let with = t.literal(slot, "with", false);
        let item = t.argument(with, "item", Parser::ItemStack, true);
        t.argument(item, "count", integer(Some(1), Some(99)), true);
        let from = t.literal(slot, "from", false);
        for source_kind in ["entity", "block"] {
            let branch = t.literal(from, source_kind, false);
            let source = t.argument(
                branch,
                "source",
                if source_kind == "entity" {
                    entities(false)
                } else {
                    Parser::BlockPos
                },
                false,
            );
            t.suggested(
                source,
                "sourceSlot",
                Parser::String(CommandStringKind::SingleWord),
                true,
                "ask_server",
            );
        }
    }
    let modify = t.literal(root, "modify", false);
    t.argument(
        modify,
        "arguments",
        Parser::String(CommandStringKind::GreedyPhrase),
        true,
    );
}

fn admin_loot_sources(t: &mut Tree<'_>, parent: i32) {
    let loot = t.literal(parent, "loot", false);
    t.argument(loot, "loot_table", Parser::ResourceLocation, true);
    let kill = t.literal(parent, "kill", false);
    t.argument(kill, "target", entities(true), true);
    for name in ["fish", "mine"] {
        let node = t.literal(parent, name, false);
        t.argument(
            node,
            "arguments",
            Parser::String(CommandStringKind::GreedyPhrase),
            true,
        );
    }
}

fn admin_loot_tree(t: &mut Tree<'_>, root: i32) {
    let give = t.literal(root, "give", false);
    let players = t.argument(give, "players", players(false), false);
    admin_loot_sources(t, players);
    for name in ["insert", "spawn"] {
        let node = t.literal(root, name, false);
        let position = t.argument(
            node,
            "position",
            if name == "spawn" {
                Parser::Vec3
            } else {
                Parser::BlockPos
            },
            false,
        );
        admin_loot_sources(t, position);
    }
    let replace = t.literal(root, "replace", false);
    for kind in ["entity", "block"] {
        let branch = t.literal(replace, kind, false);
        let target = t.argument(
            branch,
            "target",
            if kind == "entity" {
                entities(false)
            } else {
                Parser::BlockPos
            },
            false,
        );
        let slot = t.suggested(
            target,
            "slot",
            Parser::String(CommandStringKind::SingleWord),
            false,
            "ask_server",
        );
        admin_loot_sources(t, slot);
        let count = t.argument(slot, "count", integer(Some(0), None), false);
        admin_loot_sources(t, count);
    }
}

fn admin_attribute_tree(t: &mut Tree<'_>, root: i32) {
    let target = t.argument(root, "target", entities(true), false);
    let attribute = t.argument(
        target,
        "attribute",
        Parser::Resource {
            registry: identifier("attribute"),
        },
        false,
    );
    let get = t.literal(attribute, "get", true);
    t.argument(
        get,
        "scale",
        Parser::Double {
            min: None,
            max: None,
        },
        true,
    );
    let base = t.literal(attribute, "base", false);
    let get = t.literal(base, "get", true);
    t.argument(
        get,
        "scale",
        Parser::Double {
            min: None,
            max: None,
        },
        true,
    );
    let set = t.literal(base, "set", false);
    t.argument(
        set,
        "value",
        Parser::Double {
            min: None,
            max: None,
        },
        true,
    );
    t.literal(base, "reset", true);
    let modifier = t.literal(attribute, "modifier", false);
    let add = t.literal(modifier, "add", false);
    let id = t.argument(add, "id", Parser::ResourceLocation, false);
    let value = t.argument(
        id,
        "value",
        Parser::Double {
            min: None,
            max: None,
        },
        false,
    );
    t.literals(
        value,
        &["add_value", "add_multiplied_base", "add_multiplied_total"],
    );
    let remove = t.literal(modifier, "remove", false);
    t.argument(remove, "id", Parser::ResourceLocation, true);
    let value = t.literal(modifier, "value", false);
    let get = t.literal(value, "get", false);
    let id = t.argument(get, "id", Parser::ResourceLocation, true);
    t.argument(
        id,
        "scale",
        Parser::Double {
            min: None,
            max: None,
        },
        true,
    );
}

#[cfg(test)]
#[path = "command_tree/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "command_tree_parity_tests.rs"]
mod parity;
