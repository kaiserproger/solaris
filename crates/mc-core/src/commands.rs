//! Slash commands (ADR 0013): a typed parse of the vanilla subset Solaris
//! supports, and the player-local effects. World-level commands (teleport,
//! time, weather, summon, difficulty, game rules) are typed here and executed
//! by the tick against world state.

use mc_data::{Identifier, ItemStack};
use mc_physics::mth;

use crate::inventory::{ItemRules, PlayerInventory};
use crate::survival::{Difficulty, Experience};

/// Shared root permission policy for native execution and edge advertisement.
/// Edge-only administration is listed too; this does not add native handlers.
pub const ROOTS: &[(&str, u8)] = &[
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

/// Check a bare root; plugin-owned roots retain their separate first routing.
#[must_use]
pub fn root_permitted(root: &str, permission: u8) -> bool {
    ROOTS
        .iter()
        .any(|&(name, required)| name == root && permission >= required)
}

/// Who a command applies to.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Executor,
    Nearest,
    AllPlayers,
    Player(String),
    Selector(Box<Selector>),
    /// Already resolved by the tick owner's global command dispatcher.
    Entity(crate::boundary::EntityId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    Nearest,
    AllPlayers,
    Random,
    Executor,
    Entities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorSort {
    Nearest,
    Furthest,
    Random,
    Arbitrary,
}

/// Inclusive integer range with open ends, Java `..` grammar (`5`, `..5`,
/// `5..`, `3..9`).
pub type ScoreRange = (Option<i32>, Option<i32>);

#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    pub kind: SelectorKind,
    /// Repeated `type=` clauses as `(id, is_entity_tag, negated)`; at most
    /// one positive clause (Java rejects duplicate `type=` admission).
    pub entity_types: Vec<(Identifier, bool, bool)>,
    /// Repeated `name=` clauses with the same duplicate rule.
    pub names: Vec<(String, bool)>,
    /// Score filters; repeated objectives are rejected, ranges use Java
    /// `..` grammar.
    pub scores: Vec<(String, ScoreRange)>,
    /// Scoreboard-tag membership filters (`tag=<id>` / `tag=!<id>`).
    pub tags: Vec<(String, bool)>,
    /// Team membership filter (`team=<id>` / `team=` empty / `team=!<id>`).
    pub team: Option<(String, bool)>,
    /// Experience-level range (`level=..5`, `level=10..`).
    pub level: Option<ScoreRange>,
    /// Gamemode filter (`gamemode=survival`, negatable).
    pub gamemode: Option<(GameMode, bool)>,
    /// Pitch range in degrees (`x_rotation=-90..90`).
    pub pitch: Option<(Option<f32>, Option<f32>)>,
    /// Yaw range in degrees, wrapped (`y_rotation=0..90`).
    pub yaw: Option<(Option<f32>, Option<f32>)>,
    /// Repeated, optionally negated NBT compound predicates.
    pub nbt: Vec<(mc_nbt::Tag, bool)>,
    /// Advancement filters (`advancements={id=true}`).
    pub advancements: Vec<(String, bool)>,
    /// Loot-predicate filters (`predicate=namespace:id`, negatable).
    pub predicates: Vec<(Identifier, bool)>,
    pub distance: Option<(Option<f64>, Option<f64>)>,
    /// Absolute selector-origin overrides (`x`, `y`, `z`).
    pub position: [Option<f64>; 3],
    /// Signed volume extents; omitted axes are zero when any extent is present.
    pub extent: [Option<f64>; 3],
    pub limit: Option<usize>,
    pub sort: Option<SelectorSort>,
}

/// Absolute, `~`-relative, or `^`-local command coordinate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coord {
    pub relative: bool,
    pub local: bool,
    pub value: f64,
}

impl Coord {
    #[must_use]
    pub fn resolve(self, base: f64) -> f64 {
        if self.relative {
            base + self.value
        } else {
            self.value
        }
    }
}

pub(crate) fn resolve_coords(coords: [Coord; 3], origin: [f64; 3], rotation: [f32; 2]) -> [f64; 3] {
    if !coords[0].local {
        return std::array::from_fn(|axis| coords[axis].resolve(origin[axis]));
    }
    let yaw = (f64::from(rotation[0]) + 90.0).to_radians();
    let pitch = -f64::from(rotation[1]).to_radians();
    let up_pitch = pitch + std::f64::consts::FRAC_PI_2;
    let forward = [
        f64::from(mth::cos(yaw)) * f64::from(mth::cos(pitch)),
        f64::from(mth::sin(pitch)),
        f64::from(mth::sin(yaw)) * f64::from(mth::cos(pitch)),
    ];
    let up = [
        f64::from(mth::cos(yaw)) * f64::from(mth::cos(up_pitch)),
        f64::from(mth::sin(up_pitch)),
        f64::from(mth::sin(yaw)) * f64::from(mth::cos(up_pitch)),
    ];
    let left = [
        forward[2] * up[1] - forward[1] * up[2],
        forward[0] * up[2] - forward[2] * up[0],
        forward[1] * up[0] - forward[0] * up[1],
    ];
    std::array::from_fn(|axis| {
        origin[axis]
            + left[axis] * coords[0].value
            + up[axis] * coords[1].value
            + forward[axis] * coords[2].value
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    /// Protocol game mode id.
    #[must_use]
    pub fn id(self) -> u8 {
        match self {
            Self::Survival => 0,
            Self::Creative => 1,
            Self::Adventure => 2,
            Self::Spectator => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Destination {
    Position([Coord; 3]),
    Entity(Target),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weather {
    Clear,
    Rain,
    Thunder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeQuery {
    Absolute,
    DayTime,
    Day,
    GameTime,
}

impl TimeQuery {
    pub fn value(self, tick: u64, day_time: i64) -> i64 {
        match self {
            Self::Absolute => day_time,
            Self::DayTime => day_time.rem_euclid(24_000),
            Self::Day => day_time.div_euclid(24_000),
            Self::GameTime => (tick % i32::MAX as u64) as i64,
        }
    }
}

/// Vanilla advancement tree traversal relative to the named advancement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvancementScope {
    Everything,
    Only,
    From,
    Through,
    Until,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Command {
    Attribute(crate::command_attribute::AttributeCommand),
    Item(crate::command_item_replace::ItemCommand),
    Loot(crate::command_loot::LootCommand),
    Advancement {
        target: Target,
        grant: bool,
        scope: AdvancementScope,
        advancement: Option<String>,
        criterion: Option<String>,
    },
    EffectGive {
        target: Target,
        effect: crate::effects::ActiveEffect,
    },
    EffectClear {
        target: Target,
        effect: Option<crate::effects::EffectId>,
    },
    TimeMarker {
        name: String,
        ticks: i64,
    },
    GameMode {
        mode: GameMode,
        target: Target,
    },
    Teleport {
        target: Target,
        destination: Destination,
        rotation: Option<[Coord; 2]>,
    },
    Give {
        target: Target,
        item: crate::command_items::ItemArgument,
        count: i32,
    },
    Clear {
        target: Target,
        item: Option<crate::command_items::ItemPredicate>,
        max: Option<i32>,
    },
    Kill {
        target: Target,
    },
    Damage(crate::command_damage::DamageCommand),
    /// `time set` (`add == false`) or `time add`.
    Time {
        ticks: i64,
        add: bool,
    },
    Weather {
        weather: Weather,
        duration_ticks: Option<i64>,
    },
    Difficulty(Difficulty),
    GameRule {
        name: String,
        value: Option<String>,
    },
    Experience {
        target: Target,
        amount: i32,
        levels: bool,
    },
    Summon {
        entity: Identifier,
        position: Option<[Coord; 3]>,
        nbt: mc_nbt::Tag,
    },
    TimeQuery(TimeQuery),
    DifficultyQuery,
    ExperienceSet {
        target: Target,
        amount: i32,
        levels: bool,
    },
    ExperienceQuery {
        target: Target,
        levels: bool,
    },
    WorldBorder(crate::command_border::BorderCommand),
    BossBar(crate::command_bossbar::BossBarCommand),
    Settlement(crate::settlements::commands::SettlementCommand),
    World(crate::command_world::WorldCommand),
    /// `/co`: block-log inspect, lookup, rollback and restore.
    BlockLog(crate::block_log::BlockLogCommand),
    ForceLoad(crate::forceload::ForceLoadCommand),
    Enchant(crate::command_enchant::EnchantCommand),
    Scoreboard(crate::scoreboard::ScoreboardCommand),
    Say(String),
    Me {
        message: String,
        selectors: bool,
    },
    TeamMessage {
        message: String,
        selectors: bool,
    },
    Message {
        target: Target,
        message: String,
        selectors: bool,
    },
    TellRaw {
        target: Target,
        message: mc_nbt::Tag,
    },
    Title {
        target: Target,
        action: crate::command_text::TitleAction,
    },
    Seed,
    Tick(crate::command_tick::TickCommand),
    Locate(crate::command_locate::LocateCommand),
    /// Solaris test fixture: put `count` of `item` into hotbar slot.
    DebugGive {
        item: crate::command_items::ItemArgument,
        count: i32,
        hotbar_slot: u8,
    },
    /// Vanilla `/debug start`: begin tick-time profiling.
    DebugStart,
    /// Vanilla `/debug stop`: end profiling, report elapsed ticks/time/TPS.
    DebugStop,
    /// Vanilla `/debug function <id|#tag>`: trace one function execution.
    DebugFunction {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    /// Unknown command, or one the sender may not see (vanilla hides
    /// operator commands from non-operators).
    Unknown,
    Usage(&'static str),
    Message(String),
}

impl CommandError {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Unknown => "Unknown or incomplete command, see below for error".to_owned(),
            Self::Usage(_) => "Unknown or incomplete command, see below for error".to_owned(),
            Self::Message(message) => message.clone(),
        }
    }
}

/// Parses the supported vanilla command syntax. Administration commands are
/// handled by the edge before submission to the tick.
pub fn parse(command: &str, operator: bool) -> Result<Command, CommandError> {
    let command = command.trim();
    let words = command_words(command)?;
    let (name, args) = words.split_first().ok_or(CommandError::Unknown)?;
    if !operator
        && !matches!(
            *name,
            "me" | "msg" | "tell" | "w" | "teammsg" | "tm" | "settlement" | "trigger"
        )
    {
        return Err(CommandError::Unknown);
    }
    match *name {
        "attribute" => crate::command_attribute::parse(args).map(Command::Attribute),
        "item" => crate::command_item_replace::parse(args).map(Command::Item),
        "loot" => crate::command_loot::parse(args).map(Command::Loot),
        "settlement" => crate::settlements::commands::parse(args).map(Command::Settlement),
        "gamemode" => parse_gamemode(args),
        "tp" | "teleport" => parse_teleport(args),
        "give" => parse_give(args),
        "clear" => parse_clear(args),
        "effect" => crate::effects::parse_command(args),
        "advancement" => parse_advancement(args),
        "damage" => crate::command_damage::parse(args).map(Command::Damage),
        "kill" => match args {
            [] => Ok(Command::Kill {
                target: Target::Executor,
            }),
            [target] => Ok(Command::Kill {
                target: target_of(target)?,
            }),
            _ => Err(CommandError::Usage("kill [<targets>]")),
        },
        "time" => parse_time(args),
        "weather" => parse_weather(args),
        "difficulty" => parse_difficulty(args),
        "gamerule" => {
            let (name, value) = crate::command_rules::parse(args)?;
            Ok(Command::GameRule { name, value })
        }
        "xp" | "experience" => parse_experience(args),
        "summon" => parse_summon(args),
        "worldborder" => crate::command_border::parse(args).map(Command::WorldBorder),
        "forceload" => crate::forceload::parse(args).map(Command::ForceLoad),
        "co" => crate::block_log::parse(args).map(Command::BlockLog),
        "setblock" | "fill" | "clone" | "spawnpoint" | "setworldspawn" => {
            crate::command_world::parse(name, args).map(Command::World)
        }
        "bossbar" => crate::command_bossbar::parse(args, command).map(Command::BossBar),
        "enchant" => crate::command_enchant::parse(args).map(Command::Enchant),
        "team" | "scoreboard" | "trigger" => {
            crate::scoreboard::ScoreboardCommand::parse(name, args).map(Command::Scoreboard)
        }
        "say" | "me" if !args.is_empty() => {
            let message = command[name.len()..].trim_start().to_owned();
            Ok(if *name == "say" {
                Command::Say(message)
            } else {
                Command::Me {
                    message,
                    selectors: operator,
                }
            })
        }
        "teammsg" | "tm" if !args.is_empty() => Ok(Command::TeamMessage {
            message: command[name.len()..].trim_start().to_owned(),
            selectors: operator,
        }),
        "msg" | "tell" | "w" if args.len() >= 2 => {
            if !operator && args[0].starts_with('@') {
                return Err(CommandError::Message("Selector not allowed".into()));
            }
            let target = target_of(args[0])?;
            let rest = command[name.len()..].trim_start();
            Ok(Command::Message {
                target,
                message: rest[args[0].len()..].trim_start().to_owned(),
                selectors: operator,
            })
        }
        "tellraw" if args.len() >= 2 => {
            let (target, message) = crate::command_text::parse_tellraw(args, command)?;
            Ok(Command::TellRaw { target, message })
        }
        "title" if args.len() >= 2 => {
            let (target, action) = crate::command_text::parse_title(args, command)?;
            Ok(Command::Title { target, action })
        }
        "seed" if args.is_empty() => Ok(Command::Seed),
        "tick" => crate::command_tick::parse(args).map(Command::Tick),
        "locate" => crate::command_locate::parse(args).map(Command::Locate),
        "debug" => parse_debug(args),
        _ => Err(CommandError::Unknown),
    }
}

pub(crate) fn command_words(command: &str) -> Result<Vec<&str>, CommandError> {
    let mut words = Vec::new();
    let mut start = None;
    let mut depth = 0_i32;
    let mut quoted = None;
    let mut escaped = false;
    for (offset, ch) in command.char_indices() {
        if ch.is_whitespace() && depth == 0 && quoted.is_none() {
            if let Some(start) = start.take() {
                words.push(&command[start..offset]);
                if matches!(words[0], "say" | "me")
                    || (words.len() == 2
                        && matches!(words[0], "msg" | "tell" | "w" | "kick" | "ban" | "pardon"))
                {
                    let rest = command[offset..].trim_start();
                    if !rest.is_empty() {
                        words.push(rest);
                    }
                    return Ok(words);
                }
            }
            continue;
        }
        start.get_or_insert(offset);
        if escaped {
            escaped = false;
        } else if ch == '\\' && quoted.is_some() {
            escaped = true;
        } else if quoted == Some(ch) {
            quoted = None;
        } else if quoted.is_none() && matches!(ch, '"' | '\'') {
            quoted = Some(ch);
        } else if quoted.is_none() {
            match ch {
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            }
            if depth < 0 {
                return Err(CommandError::Message("Expected end of options".into()));
            }
        }
    }
    if quoted.is_some() || depth != 0 {
        return Err(CommandError::Message("Expected end of options".into()));
    }
    if let Some(start) = start {
        words.push(&command[start..]);
    }
    Ok(words)
}

pub(crate) fn target_of(word: &str) -> Result<Target, CommandError> {
    if !word.starts_with('@') {
        return Ok(Target::Player(word.to_owned()));
    }
    match word {
        "@s" => return Ok(Target::Executor),
        "@p" | "@n" => return Ok(Target::Nearest),
        "@a" => return Ok(Target::AllPlayers),
        _ => {}
    }
    let kind = match word.as_bytes().get(1) {
        Some(b'p') | Some(b'n') => SelectorKind::Nearest,
        Some(b'a') => SelectorKind::AllPlayers,
        Some(b'r') => SelectorKind::Random,
        Some(b's') => SelectorKind::Executor,
        Some(b'e') => SelectorKind::Entities,
        _ => {
            return Err(CommandError::Message(format!(
                "Unknown selector type '{}'",
                word.chars().nth(1).unwrap_or('@')
            )));
        }
    };
    let mut selector = Selector {
        kind,
        entity_types: Vec::new(),
        names: Vec::new(),
        scores: Vec::new(),
        tags: Vec::new(),
        team: None,
        level: None,
        gamemode: None,
        pitch: None,
        yaw: None,
        nbt: Vec::new(),
        advancements: Vec::new(),
        predicates: Vec::new(),
        distance: None,
        position: [None; 3],
        extent: [None; 3],
        limit: None,
        sort: None,
    };
    if word.len() == 2 {
        return Ok(Target::Selector(Box::new(selector)));
    }
    let options = word[2..]
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| CommandError::Message("Expected end of options".into()))?;
    let mut quote = None;
    let mut escaped = false;
    let mut depth = 0_u32;
    let mut scores_seen = false;
    let mut offset = 0;
    let mut parts = Vec::new();
    for (index, ch) in options.char_indices() {
        if escaped {
            escaped = false;
        } else if quote.is_some() && ch == '\\' {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() {
            match ch {
                '"' | '\'' => quote = Some(ch),
                '{' | '[' => depth += 1,
                '}' | ']' => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| CommandError::Message("Expected end of options".into()))?;
                }
                ',' if depth == 0 => {
                    parts.push(&options[offset..index]);
                    offset = index + 1;
                }
                _ => {}
            }
        }
    }
    if quote.is_some() || depth != 0 {
        return Err(CommandError::Message("Expected end of options".into()));
    }
    parts.push(&options[offset..]);
    for option in parts.into_iter().filter(|s| !s.trim().is_empty()) {
        let (key, value) = option
            .split_once('=')
            .ok_or_else(|| CommandError::Message("Expected value for option".into()))?;
        let (key, value) = (key.trim(), value.trim());
        let invalid = || CommandError::Message(format!("Option '{key}' isn't applicable here"));
        match key {
            "x" | "y" | "z" | "dx" | "dy" | "dz" => {
                let axis = match key.as_bytes().last() {
                    Some(b'x') => 0,
                    Some(b'y') => 1,
                    _ => 2,
                };
                let target = if key.starts_with('d') {
                    &mut selector.extent[axis]
                } else {
                    &mut selector.position[axis]
                };
                if target.is_some() {
                    return Err(invalid());
                }
                let number: f64 = value.parse().map_err(|_| invalid())?;
                if !number.is_finite() {
                    return Err(invalid());
                }
                *target = Some(number);
            }
            "type" => {
                if !matches!(kind, SelectorKind::Entities | SelectorKind::Executor) {
                    return Err(invalid());
                }
                let negated = value.starts_with('!');
                let name = value.strip_prefix('!').unwrap_or(value);
                let (id, is_tag) = if let Some(tag) = name.strip_prefix('#') {
                    (item_id(tag).ok_or_else(invalid)?, true)
                } else {
                    (item_id(name).ok_or_else(invalid)?, false)
                };
                if !negated && selector.entity_types.iter().any(|(_, _, neg)| !neg) {
                    return Err(invalid());
                }
                selector.entity_types.push((id, is_tag, negated));
            }
            "name" => {
                let negated = value.starts_with('!');
                let name = value.strip_prefix('!').unwrap_or(value);
                let name = name.trim_matches('"').to_owned();
                if !negated && selector.names.iter().any(|(_, neg)| !neg) {
                    return Err(invalid());
                }
                selector.names.push((name, negated));
            }
            "distance" if selector.distance.is_none() => {
                let number = |s: &str| -> Result<Option<f64>, CommandError> {
                    if s.is_empty() {
                        return Ok(None);
                    }
                    let n: f64 = s
                        .parse()
                        .map_err(|_| CommandError::Message(format!("Invalid double '{s}'")))?;
                    if !n.is_finite() {
                        return Err(invalid());
                    }
                    if n < 0.0 {
                        return Err(CommandError::Message("Distance cannot be negative".into()));
                    }
                    Ok(Some(n))
                };
                let range = if let Some((min, max)) = value.split_once("..") {
                    (number(min)?, number(max)?)
                } else {
                    let n = number(value)?.ok_or_else(invalid)?;
                    (Some(n), Some(n))
                };
                if range == (None, None) || matches!(range, (Some(a),Some(b)) if a>b) {
                    return Err(invalid());
                }
                selector.distance = Some(range);
            }
            "limit" if selector.limit.is_none() => {
                let limit = integer(value, 1)?;
                selector.limit = Some(limit as usize);
            }
            "sort" if selector.sort.is_none() => {
                selector.sort = Some(match value {
                    "nearest" => SelectorSort::Nearest,
                    "furthest" => SelectorSort::Furthest,
                    "random" => SelectorSort::Random,
                    "arbitrary" => SelectorSort::Arbitrary,
                    _ => {
                        return Err(CommandError::Message(format!(
                            "Invalid or unknown sort type '{value}'"
                        )));
                    }
                });
            }
            "tag" => {
                let negated = value.starts_with('!');
                let tag = value.strip_prefix('!').unwrap_or(value).to_owned();
                selector.tags.push((tag, negated));
            }
            "team" => {
                if selector.team.is_some() {
                    return Err(invalid());
                }
                let negated = value.starts_with('!');
                let team = value.strip_prefix('!').unwrap_or(value).to_owned();
                selector.team = Some((team, negated));
            }
            "level" => {
                if selector.level.is_some() {
                    return Err(invalid());
                }
                selector.level = Some(int_range(value).ok_or_else(invalid)?);
            }
            "gamemode" => {
                if selector.gamemode.is_some() {
                    return Err(invalid());
                }
                let negated = value.starts_with('!');
                let mode = value.strip_prefix('!').unwrap_or(value);
                let mode = match mode {
                    "survival" => GameMode::Survival,
                    "creative" => GameMode::Creative,
                    "adventure" => GameMode::Adventure,
                    "spectator" => GameMode::Spectator,
                    _ => return Err(invalid()),
                };
                selector.gamemode = Some((mode, negated));
            }
            "x_rotation" => {
                if selector.pitch.is_some() {
                    return Err(invalid());
                }
                selector.pitch = Some(float_range(value).ok_or_else(invalid)?);
            }
            "y_rotation" => {
                if selector.yaw.is_some() {
                    return Err(invalid());
                }
                selector.yaw = Some(float_range(value).ok_or_else(invalid)?);
            }
            "nbt" => {
                let negated = value.starts_with('!');
                let value = value.strip_prefix('!').unwrap_or(value);
                let nbt = crate::command_snbt::parse(value).map_err(|_| invalid())?;
                if !matches!(nbt, mc_nbt::Tag::Compound(_)) {
                    return Err(invalid());
                }
                selector.nbt.push((nbt, negated));
            }
            "predicate" => {
                let negated = value.starts_with('!');
                let id = value.strip_prefix('!').unwrap_or(value);
                selector
                    .predicates
                    .push((item_id(id).ok_or_else(invalid)?, negated));
            }
            "advancements" => {
                let body = value
                    .strip_prefix('{')
                    .and_then(|body| body.strip_suffix('}'))
                    .ok_or_else(invalid)?;
                for entry in body
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                {
                    let (id, granted) = entry.split_once('=').ok_or_else(invalid)?;
                    let granted = match granted.trim() {
                        "true" => true,
                        "false" => false,
                        _ => return Err(invalid()),
                    };
                    selector.advancements.push((id.trim().to_owned(), granted));
                }
                if selector.advancements.is_empty() {
                    return Err(invalid());
                }
            }
            "scores" => {
                if scores_seen {
                    return Err(invalid());
                }
                scores_seen = true;
                let body = value
                    .strip_prefix('{')
                    .and_then(|body| body.strip_suffix('}'))
                    .ok_or_else(invalid)?;
                for entry in body
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                {
                    let (objective, range) = entry.split_once('=').ok_or_else(invalid)?;
                    let objective = objective.trim().to_owned();
                    if objective.is_empty()
                        || selector.scores.iter().any(|(known, _)| known == &objective)
                    {
                        return Err(invalid());
                    }
                    selector
                        .scores
                        .push((objective, int_range(range.trim()).ok_or_else(invalid)?));
                }
                if selector.scores.is_empty() {
                    return Err(invalid());
                }
            }
            _ => {
                return Err(
                    if matches!(key, "type" | "name" | "distance" | "limit" | "sort") {
                        invalid()
                    } else {
                        CommandError::Message(format!("Unknown option '{key}'"))
                    },
                );
            }
        }
    }
    Ok(Target::Selector(Box::new(selector)))
}

/// Java integer `..` range grammar: `5`, `..5`, `5..`, `3..9`.
fn int_range(value: &str) -> Option<ScoreRange> {
    let number = |part: &str| -> Option<Option<i32>> {
        if part.is_empty() {
            return Some(None);
        }
        part.parse::<i32>().ok().map(Some)
    };
    let (min, max) = if let Some((min, max)) = value.split_once("..") {
        (number(min.trim())?, number(max.trim())?)
    } else {
        let exact = value.parse::<i32>().ok()?;
        (Some(exact), Some(exact))
    };
    if min.is_none() && max.is_none() {
        return None;
    }
    if matches!((min, max), (Some(min), Some(max)) if min > max) {
        return None;
    }
    Some((min, max))
}

/// Java float `..` range grammar for rotation filters.
fn float_range(value: &str) -> Option<(Option<f32>, Option<f32>)> {
    let number = |part: &str| -> Option<Option<f32>> {
        if part.is_empty() {
            return Some(None);
        }
        let value: f32 = part.parse().ok()?;
        value.is_finite().then_some(Some(value))
    };
    let (min, max) = if let Some((min, max)) = value.split_once("..") {
        (number(min.trim())?, number(max.trim())?)
    } else {
        let exact: f32 = value.parse().ok()?;
        exact.is_finite().then_some((Some(exact), Some(exact)))?
    };
    if min.is_none() && max.is_none() {
        return None;
    }
    // Yaw wraps, so min > max is a wrapped window; pitch is checked at match time.
    Some((min, max))
}

fn item_id(word: &str) -> Option<Identifier> {
    let name = if word.contains(':') {
        word.to_owned()
    } else {
        format!("minecraft:{word}")
    };
    Identifier::parse(name).ok()
}

pub(crate) fn coord(word: &str) -> Option<Coord> {
    if let Some(rest) = word.strip_prefix('~').or_else(|| word.strip_prefix('^')) {
        let value = if rest.is_empty() {
            0.0
        } else {
            rest.parse().ok()?
        };
        return value_ok(value).then_some(Coord {
            relative: true,
            local: word.starts_with('^'),
            value,
        });
    }
    let value: f64 = word.parse().ok()?;
    value_ok(value).then_some(Coord {
        relative: false,
        local: false,
        value,
    })
}

fn value_ok(value: f64) -> bool {
    value.is_finite() && value.abs() <= 30_000_000.0
}

pub(crate) fn coords(words: &[&str]) -> Option<[Coord; 3]> {
    match words {
        [x, y, z] => {
            let coords = [coord(x)?, coord(y)?, coord(z)?];
            coords
                .iter()
                .all(|coord| coord.local == coords[0].local)
                .then_some(coords)
        }
        _ => None,
    }
}

fn parse_gamemode(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "gamemode <survival|creative|adventure|spectator> [<target>]";
    let mode = match args.first().copied() {
        Some("survival") => GameMode::Survival,
        Some("creative") => GameMode::Creative,
        Some("adventure") => GameMode::Adventure,
        Some("spectator") => GameMode::Spectator,
        _ => return Err(CommandError::Usage(USAGE)),
    };
    let target = match args {
        [_] => Target::Executor,
        [_, target] => target_of(target)?,
        _ => return Err(CommandError::Usage(USAGE)),
    };
    Ok(Command::GameMode { mode, target })
}

pub(crate) fn rotation(words: &[&str]) -> Option<[Coord; 2]> {
    let [yaw, pitch] = words else {
        return None;
    };
    let rotation = [coord(yaw)?, coord(pitch)?];
    rotation
        .iter()
        .all(|coord| !coord.local)
        .then_some(rotation)
}

fn parse_teleport(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "teleport <targets> <destination|location> [<rotation>]";
    let usage = || CommandError::Usage(USAGE);
    let (target, destination, facing) = match args {
        [entity] => (
            Target::Executor,
            Destination::Entity(target_of(entity)?),
            None,
        ),
        [target, entity] => (
            target_of(target)?,
            Destination::Entity(target_of(entity)?),
            None,
        ),
        [x, y, z] => (
            Target::Executor,
            Destination::Position(coords(&[x, y, z]).ok_or_else(usage)?),
            None,
        ),
        [target, x, y, z] => (
            target_of(target)?,
            Destination::Position(coords(&[x, y, z]).ok_or_else(usage)?),
            None,
        ),
        [x, y, z, yaw, pitch] => (
            Target::Executor,
            Destination::Position(coords(&[x, y, z]).ok_or_else(usage)?),
            Some(rotation(&[yaw, pitch]).ok_or_else(usage)?),
        ),
        [target, x, y, z, yaw, pitch] => (
            target_of(target)?,
            Destination::Position(coords(&[x, y, z]).ok_or_else(usage)?),
            Some(rotation(&[yaw, pitch]).ok_or_else(usage)?),
        ),
        _ => return Err(usage()),
    };
    Ok(Command::Teleport {
        target,
        destination,
        rotation: facing,
    })
}

fn integer(word: &str, minimum: i32) -> Result<i32, CommandError> {
    let value: i32 = word
        .parse()
        .map_err(|_| CommandError::Message(format!("Invalid integer '{word}'")))?;
    if value < minimum {
        return Err(CommandError::Message(format!(
            "Integer must not be less than {minimum}, found {value}"
        )));
    }
    Ok(value)
}

fn parse_give(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "give <targets> <item> [<count>]";
    let (target, item, count) = match args {
        [target, item] => (target, item, "1"),
        [target, item, count] => (target, item, *count),
        _ => return Err(CommandError::Usage(USAGE)),
    };
    let count = integer(count, 1)?;
    Ok(Command::Give {
        target: target_of(target)?,
        item: crate::command_items::ItemArgument::parse(item)?,
        count,
    })
}

fn parse_clear(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "clear [<targets>] [<item>] [<maxCount>]";
    let target = args
        .first()
        .map(|word| target_of(word))
        .transpose()?
        .unwrap_or(Target::Executor);
    let item = match args.get(1) {
        Some(word) => Some(crate::command_items::ItemPredicate::parse(word)?),
        None => None,
    };
    let max = match args.get(2) {
        Some(word) => Some(integer(word, 0)?),
        None => None,
    };
    if args.len() > 3 {
        return Err(CommandError::Usage(USAGE));
    }
    Ok(Command::Clear { target, item, max })
}

pub(crate) fn time_ticks(word: &str) -> Option<i64> {
    let (number, scale) = match word.as_bytes().last()? {
        b'd' => (&word[..word.len() - 1], 24_000.0),
        b's' => (&word[..word.len() - 1], 20.0),
        b't' => (&word[..word.len() - 1], 1.0),
        _ => (word, 1.0),
    };
    let value: f64 = number.parse().ok()?;
    let scaled = (value * scale).round();
    (value.is_finite() && scaled >= i32::MIN as f64 && scaled <= i32::MAX as f64)
        .then_some(scaled as i64)
}

fn parse_time(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "time <set|add|query> <time>";
    match args {
        ["query", "time"] => Ok(Command::TimeQuery(TimeQuery::Absolute)),
        ["query", "daytime"] => Ok(Command::TimeQuery(TimeQuery::DayTime)),
        ["query", "day"] => Ok(Command::TimeQuery(TimeQuery::Day)),
        ["query", "gametime"] => Ok(Command::TimeQuery(TimeQuery::GameTime)),
        ["set", value] => {
            let marker = value.strip_prefix("minecraft:").unwrap_or(value);
            if let Some(ticks) = match marker {
                "day" => Some(1000),
                "noon" => Some(6000),
                "night" => Some(13000),
                "midnight" => Some(18000),
                _ => None,
            } {
                return Ok(Command::TimeMarker {
                    name: format!("minecraft:{marker}"),
                    ticks,
                });
            }
            let ticks = time_ticks(value)
                .filter(|n| *n >= 0)
                .ok_or(CommandError::Usage(USAGE))?;
            Ok(Command::Time { ticks, add: false })
        }
        ["add", value] => Ok(Command::Time {
            ticks: time_ticks(value).ok_or(CommandError::Usage(USAGE))?,
            add: true,
        }),
        _ => Err(CommandError::Usage(USAGE)),
    }
}

fn parse_weather(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "weather <clear|rain|thunder> [<duration>]";
    let weather = match args.first().copied() {
        Some("clear") => Weather::Clear,
        Some("rain") => Weather::Rain,
        Some("thunder") => Weather::Thunder,
        _ => return Err(CommandError::Usage(USAGE)),
    };
    let duration_ticks = match args {
        [_] => None,
        [_, word] => Some(
            time_ticks(word)
                .filter(|n| *n >= 1)
                .ok_or(CommandError::Usage(USAGE))?,
        ),
        _ => return Err(CommandError::Usage(USAGE)),
    };
    Ok(Command::Weather {
        weather,
        duration_ticks,
    })
}

fn parse_difficulty(args: &[&str]) -> Result<Command, CommandError> {
    if args.is_empty() {
        return Ok(Command::DifficultyQuery);
    }
    let difficulty = match args {
        ["peaceful"] => Difficulty::Peaceful,
        ["easy"] => Difficulty::Easy,
        ["normal"] => Difficulty::Normal,
        ["hard"] => Difficulty::Hard,
        _ => {
            return Err(CommandError::Usage(
                "difficulty <peaceful|easy|normal|hard>",
            ));
        }
    };
    Ok(Command::Difficulty(difficulty))
}

fn parse_advancement(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "advancement <grant|revoke> <targets> <everything|only|from|through|until> [<advancement>] [<criterion>]";
    let [operation, target, mode, rest @ ..] = args else {
        return Err(CommandError::Usage(USAGE));
    };
    let grant = match *operation {
        "grant" => true,
        "revoke" => false,
        _ => return Err(CommandError::Usage(USAGE)),
    };
    let scope = match *mode {
        "everything" => AdvancementScope::Everything,
        "only" => AdvancementScope::Only,
        "from" => AdvancementScope::From,
        "through" => AdvancementScope::Through,
        "until" => AdvancementScope::Until,
        _ => return Err(CommandError::Usage(USAGE)),
    };
    let (advancement, criterion) = match (scope, rest) {
        (AdvancementScope::Everything, []) => (None, None),
        (AdvancementScope::Only, [id, criterion @ ..]) if !criterion.is_empty() => (
            Some(item_id(id).ok_or(CommandError::Usage(USAGE))?.to_string()),
            Some(criterion.join(" ")),
        ),
        (scope, [id]) if scope != AdvancementScope::Everything => (
            Some(item_id(id).ok_or(CommandError::Usage(USAGE))?.to_string()),
            None,
        ),
        _ => return Err(CommandError::Usage(USAGE)),
    };
    Ok(Command::Advancement {
        target: target_of(target)?,
        grant,
        scope,
        advancement,
        criterion,
    })
}

fn parse_experience(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "experience <add|set|query> <targets> [<amount>] [points|levels]";
    let units = |word: &str| match word {
        "points" => Ok(false),
        "levels" => Ok(true),
        _ => Err(CommandError::Usage(USAGE)),
    };
    if let ["query", target, unit] = args {
        return Ok(Command::ExperienceQuery {
            target: target_of(target)?,
            levels: units(unit)?,
        });
    }
    let (operation, target, amount, unit) = match args {
        [op @ ("add" | "set"), target, amount] => (*op, target, amount, "points"),
        [op @ ("add" | "set"), target, amount, unit] => (*op, target, amount, *unit),
        _ => return Err(CommandError::Usage(USAGE)),
    };
    let amount = integer(amount, if operation == "set" { 0 } else { i32::MIN })?;
    let target = target_of(target)?;
    let levels = units(unit)?;
    Ok(if operation == "set" {
        Command::ExperienceSet {
            target,
            amount,
            levels,
        }
    } else {
        Command::Experience {
            target,
            amount,
            levels,
        }
    })
}

fn parse_summon(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "summon <entity> [<pos>] [<nbt>]";
    let entity = args
        .first()
        .and_then(|word| item_id(word))
        .ok_or(CommandError::Usage(USAGE))?;
    let (position, nbt) = match &args[1..] {
        [] => (None, mc_nbt::Tag::Compound(Vec::new())),
        [x, y, z] => (
            Some(coords(&[x, y, z]).ok_or(CommandError::Usage(USAGE))?),
            mc_nbt::Tag::Compound(Vec::new()),
        ),
        [x, y, z, raw] => {
            let nbt =
                crate::command_snbt::parse(raw).map_err(|e| CommandError::Message(e.into()))?;
            if !matches!(nbt, mc_nbt::Tag::Compound(_)) {
                return Err(CommandError::Message(
                    "Summon NBT must be a compound".into(),
                ));
            }
            (
                Some(coords(&[x, y, z]).ok_or(CommandError::Usage(USAGE))?),
                nbt,
            )
        }
        _ => return Err(CommandError::Usage(USAGE)),
    };
    Ok(Command::Summon {
        entity,
        position,
        nbt,
    })
}

fn parse_debug(args: &[&str]) -> Result<Command, CommandError> {
    const USAGE: &str = "debug give <item> <count> <hotbar slot 0-8>";
    match args {
        ["give", item, count, slot] => Ok(Command::DebugGive {
            item: crate::command_items::ItemArgument::parse(item)?,
            count: count.parse().map_err(|_| CommandError::Usage(USAGE))?,
            hotbar_slot: slot
                .parse::<u8>()
                .ok()
                .filter(|slot| *slot <= 8)
                .ok_or(CommandError::Usage(USAGE))?,
        }),
        ["start"] => Ok(Command::DebugStart),
        ["stop"] => Ok(Command::DebugStop),
        ["function", name] => Ok(Command::DebugFunction {
            name: (*name).to_owned(),
        }),
        _ => Err(CommandError::Usage(USAGE)),
    }
}

/// `/give`: adds `count` in max-size stacks; what does not fit is returned to
/// be dropped at the player.
pub fn give(
    rules: &ItemRules<'_>,
    inventory: &mut PlayerInventory,
    template: &ItemStack,
    count: i32,
) -> Vec<ItemStack> {
    let max = rules.max_stack(template).max(1);
    let mut remaining = count.max(0);
    let mut dropped = Vec::new();
    while remaining > 0 {
        let mut stack = template.clone();
        stack.count = remaining.min(max);
        remaining -= stack.count;
        inventory.add(rules, &mut stack);
        if !stack.is_empty() {
            dropped.push(stack);
        }
    }
    dropped
}

/// Solaris `debug give`: sets a hotbar slot outright (count 0 clears it,
/// at most 255).
pub fn debug_give(
    inventory: &mut PlayerInventory,
    mut stack: ItemStack,
    count: i32,
    hotbar_slot: u8,
) {
    inventory.slots[usize::from(hotbar_slot.min(8))] = if count <= 0 {
        ItemStack::EMPTY
    } else {
        stack.count = count.min(255);
        stack
    };
}

/// `/clear`: removes matching items (all items when `item` is `None`), at
/// most `max` (`Some(0)` only counts). Returns the number found or removed.
pub fn clear(
    inventory: &mut PlayerInventory,
    mut matches: impl FnMut(&ItemStack) -> bool,
    max: Option<i32>,
) -> i32 {
    let mut budget = max.unwrap_or(i32::MAX);
    let mut found = 0;
    for slot in &mut inventory.slots {
        if slot.is_empty() || !matches(slot) {
            continue;
        }
        if max == Some(0) {
            found += slot.count;
            continue;
        }
        let taken = slot.count.min(budget);
        slot.count -= taken;
        if slot.count <= 0 {
            *slot = ItemStack::EMPTY;
        }
        found += taken;
        budget -= taken;
        if budget == 0 {
            break;
        }
    }
    found
}

/// `/xp add`: points through vanilla leveling, or whole levels.
pub fn add_experience(experience: &mut Experience, amount: i32, levels: bool) {
    if levels {
        experience.add_levels(amount);
    } else {
        experience.add_points(amount);
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
