use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundPlayerAbilities, CommandArgumentParser, CommandNode,
    CommandStringKind, GameMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandPermissions {
    pub(super) op: bool,
}

impl CommandPermissions {
    pub(crate) const CONSOLE: Self = Self { op: true };

    pub(crate) const fn from_op(op: bool) -> Self {
        Self { op }
    }

    pub(super) const fn can_change_game_mode(self) -> bool {
        self.op
    }

    pub(super) const fn can_use_admin_commands(self) -> bool {
        self.op
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SurvivalCommand {
    Damage(f32),
    Heal(f32),
    Feed { food: i32, saturation: f32 },
    Exhaust(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DebugCommand {
    Survival(SurvivalCommand),
    Give {
        item: mc_data::Identifier,
        count: i32,
        hotbar_slot: u8,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AdminCommand {
    GameMode(GameMode),
    Give {
        item: mc_data::Identifier,
        count: i32,
    },
    Kill,
    SaveAll,
    Status,
    Stop,
    Summon {
        entity: mc_data::Identifier,
        x: Option<f64>,
        y: Option<f64>,
        z: Option<f64>,
    },
    Teleport {
        x: f64,
        y: f64,
        z: f64,
    },
    TimeSet(u64),
    Debug(DebugCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandError {
    Unknown,
    PermissionDenied,
    Usage(&'static str),
}

pub(crate) struct CommandSuggestionSet {
    pub(super) start: i32,
    pub(super) length: i32,
    pub(super) suggestions: Vec<String>,
}

const ROOT_COMMANDS: &[&str] = &[
    "debug", "gamemode", "give", "kill", "save-all", "status", "stop", "summon", "time", "tp",
];
const GAME_MODES: &[&str] = &["survival", "creative", "adventure", "spectator"];

pub(crate) fn command_tree_packet(permissions: CommandPermissions) -> ClientboundCommands {
    if !permissions.can_use_admin_commands() {
        return ClientboundCommands {
            nodes: vec![CommandNode::root(Vec::new())],
            root_index: 0,
        };
    }

    ClientboundCommands {
        nodes: vec![
            CommandNode::root(vec![1, 6, 8, 10, 11, 12, 13, 15, 17, 19]),
            CommandNode::literal("gamemode", vec![2, 3, 4, 5], false).restricted(true),
            CommandNode::literal("survival", Vec::new(), true).restricted(true),
            CommandNode::literal("creative", Vec::new(), true).restricted(true),
            CommandNode::literal("adventure", Vec::new(), true).restricted(true),
            CommandNode::literal("spectator", Vec::new(), true).restricted(true),
            CommandNode::literal("give", vec![7], false).restricted(true),
            CommandNode::argument(
                "args",
                CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
                Vec::new(),
                true,
            )
            .restricted(true),
            CommandNode::literal("debug", vec![9], false).restricted(true),
            CommandNode::argument(
                "args",
                CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
                Vec::new(),
                true,
            )
            .restricted(true),
            CommandNode::literal("kill", Vec::new(), true).restricted(true),
            CommandNode::literal("save-all", Vec::new(), true).restricted(true),
            CommandNode::literal("stop", Vec::new(), true).restricted(true),
            CommandNode::literal("summon", vec![14], false).restricted(true),
            CommandNode::argument(
                "args",
                CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
                Vec::new(),
                true,
            )
            .restricted(true),
            CommandNode::literal("time", vec![16], false).restricted(true),
            CommandNode::argument(
                "args",
                CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
                Vec::new(),
                true,
            )
            .restricted(true),
            CommandNode::literal("tp", vec![18], false).restricted(true),
            CommandNode::argument(
                "args",
                CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
                Vec::new(),
                true,
            )
            .restricted(true),
            CommandNode::literal("status", Vec::new(), true).restricted(true),
        ],
        root_index: 0,
    }
}

pub(crate) fn parse_admin_command(
    input: &str,
    permissions: CommandPermissions,
) -> Result<AdminCommand, CommandError> {
    let command = normalize_command_input(input);
    let parsed = parse_admin_command_inner(command)?;
    if !permissions.can_use_admin_commands() {
        return Err(CommandError::PermissionDenied);
    }
    Ok(parsed)
}

fn parse_admin_command_inner(command: &str) -> Result<AdminCommand, CommandError> {
    if let Some(mode) = parse_gamemode_command(command) {
        return Ok(AdminCommand::GameMode(mode));
    }
    if command.starts_with("gamemode") || command.starts_with("defaultgamemode") {
        return Err(CommandError::Usage(
            "Usage: /gamemode <survival|creative|adventure|spectator>",
        ));
    }
    if let Some(give) = parse_give_command(command) {
        return Ok(give);
    }
    if command.starts_with("give") {
        return Err(CommandError::Usage("Usage: /give <item> [count]"));
    }
    if let Some(tp) = parse_tp_command(command) {
        return Ok(tp);
    }
    if command.starts_with("tp") || command.starts_with("teleport") {
        return Err(CommandError::Usage("Usage: /tp <x> <y> <z>"));
    }
    if let Some(summon) = parse_summon_command(command) {
        return Ok(summon);
    }
    if command.starts_with("summon") {
        return Err(CommandError::Usage("Usage: /summon <entity> [x y z]"));
    }
    if command == "kill" {
        return Ok(AdminCommand::Kill);
    }
    if command.starts_with("kill") {
        return Err(CommandError::Usage("Usage: /kill"));
    }
    if command == "save-all" {
        return Ok(AdminCommand::SaveAll);
    }
    if command.starts_with("save-all") {
        return Err(CommandError::Usage("Usage: /save-all"));
    }
    if command == "status" {
        return Ok(AdminCommand::Status);
    }
    if command.starts_with("status") {
        return Err(CommandError::Usage("Usage: /status"));
    }
    if command == "stop" {
        return Ok(AdminCommand::Stop);
    }
    if command.starts_with("stop") {
        return Err(CommandError::Usage("Usage: /stop"));
    }
    if let Some(time) = parse_time_command(command) {
        return Ok(time);
    }
    if command.starts_with("time") {
        return Err(CommandError::Usage(
            "Usage: /time set <ticks|day|noon|night|midnight>",
        ));
    }
    if let Some(debug) = parse_debug_command(command) {
        return Ok(AdminCommand::Debug(debug));
    }
    if command.starts_with("debug") {
        return Err(CommandError::Usage(
            "Usage: /debug survival <damage|heal|feed|exhaust> ...",
        ));
    }
    Err(CommandError::Unknown)
}

pub(crate) fn command_suggestions(
    input: &str,
    permissions: CommandPermissions,
) -> CommandSuggestionSet {
    if !permissions.can_use_admin_commands() {
        return empty_suggestions(input);
    }

    let slash_len = i32::from(input.starts_with('/'));
    let command = normalize_command_input(input);
    if let Some(rest) = command.strip_prefix("gamemode ") {
        return suggestions_for_prefix(
            input,
            slash_len + "gamemode ".len() as i32,
            rest,
            GAME_MODES,
        );
    }
    if command.contains(char::is_whitespace) {
        return empty_suggestions(input);
    }
    suggestions_for_prefix(input, slash_len, command, ROOT_COMMANDS)
}

fn suggestions_for_prefix(
    _input: &str,
    start: i32,
    prefix: &str,
    candidates: &[&str],
) -> CommandSuggestionSet {
    let suggestions = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.starts_with(prefix))
        .map(str::to_string)
        .collect();
    CommandSuggestionSet {
        start,
        length: prefix.chars().count().min(i32::MAX as usize) as i32,
        suggestions,
    }
}

fn empty_suggestions(input: &str) -> CommandSuggestionSet {
    CommandSuggestionSet {
        start: input.chars().count().min(i32::MAX as usize) as i32,
        length: 0,
        suggestions: Vec::new(),
    }
}

fn normalize_command_input(input: &str) -> &str {
    input.trim().strip_prefix('/').unwrap_or(input.trim())
}

pub(super) fn parse_gamemode_command(command: &str) -> Option<GameMode> {
    let mut parts = command.split_whitespace();
    let name = parts.next()?;
    if name != "gamemode" && name != "defaultgamemode" {
        return None;
    }
    let mode = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    parse_game_mode(mode)
}

fn parse_game_mode(mode: &str) -> Option<GameMode> {
    match mode {
        "0" | "survival" | "s" => Some(GameMode::Survival),
        "1" | "creative" | "c" => Some(GameMode::Creative),
        "2" | "adventure" | "a" => Some(GameMode::Adventure),
        "3" | "spectator" | "sp" => Some(GameMode::Spectator),
        _ => None,
    }
}

pub(super) fn parse_debug_command(command: &str) -> Option<DebugCommand> {
    let rest = command.strip_prefix("debug ")?;
    if let Some(survival) = rest.strip_prefix("survival ") {
        return parse_survival_command(survival).map(DebugCommand::Survival);
    }

    let mut parts = rest.split_whitespace();
    let name = parts.next()?;
    if name != "give" {
        return None;
    }
    let item = mc_data::Identifier::parse(parts.next()?.to_string()).ok()?;
    let count: i32 = parts.next().unwrap_or("1").parse().ok()?;
    let hotbar_slot = parts.next().unwrap_or("0").parse::<i32>().ok()?;
    if parts.next().is_some() || !(0..=8).contains(&hotbar_slot) {
        return None;
    }
    Some(DebugCommand::Give {
        item,
        count,
        hotbar_slot: hotbar_slot as u8,
    })
}

fn parse_give_command(command: &str) -> Option<AdminCommand> {
    let mut parts = command.split_whitespace();
    if parts.next()? != "give" {
        return None;
    }
    let item = mc_data::Identifier::parse(parts.next()?.to_string()).ok()?;
    let count: i32 = parts.next().unwrap_or("1").parse().ok()?;
    if parts.next().is_some() || count <= 0 {
        return None;
    }
    Some(AdminCommand::Give {
        item,
        count: count.min(i32::from(u8::MAX)),
    })
}

fn parse_tp_command(command: &str) -> Option<AdminCommand> {
    let mut parts = command.split_whitespace();
    let name = parts.next()?;
    if name != "tp" && name != "teleport" {
        return None;
    }
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(AdminCommand::Teleport { x, y, z })
}

fn parse_summon_command(command: &str) -> Option<AdminCommand> {
    let mut parts = command.split_whitespace();
    if parts.next()? != "summon" {
        return None;
    }
    let entity = mc_data::Identifier::parse(parts.next()?.to_string()).ok()?;
    let x = parts.next().map(str::parse).transpose().ok()?;
    let y = parts.next().map(str::parse).transpose().ok()?;
    let z = parts.next().map(str::parse).transpose().ok()?;
    if parts.next().is_some() || x.is_some() != y.is_some() || y.is_some() != z.is_some() {
        return None;
    }
    Some(AdminCommand::Summon { entity, x, y, z })
}

fn parse_time_command(command: &str) -> Option<AdminCommand> {
    let mut parts = command.split_whitespace();
    if parts.next()? != "time" || parts.next()? != "set" {
        return None;
    }
    let value = match parts.next()? {
        "day" => 1000,
        "noon" => 6000,
        "night" => 13000,
        "midnight" => 18000,
        raw => raw.parse().ok()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(AdminCommand::TimeSet(value))
}

fn parse_survival_command(command: &str) -> Option<SurvivalCommand> {
    let mut parts = command.split_whitespace();
    let name = parts.next()?;
    match name {
        "damage" => {
            let amount = parts.next()?.parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Damage(amount))
        }
        "heal" => {
            let amount = parts.next().unwrap_or("20").parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Heal(amount))
        }
        "feed" => {
            let food = parts.next().unwrap_or("20").parse().ok()?;
            let saturation = parts.next().unwrap_or("5").parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Feed { food, saturation })
        }
        "exhaust" => {
            let amount = parts.next()?.parse().ok()?;
            parts
                .next()
                .is_none()
                .then_some(SurvivalCommand::Exhaust(amount))
        }
        _ => None,
    }
}

pub(super) fn player_abilities_for_mode(mode: GameMode) -> ClientboundPlayerAbilities {
    match mode {
        GameMode::Creative => ClientboundPlayerAbilities {
            invulnerable: true,
            flying: false,
            can_fly: true,
            instabuild: true,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
        GameMode::Spectator => ClientboundPlayerAbilities {
            invulnerable: true,
            flying: true,
            can_fly: true,
            instabuild: false,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
        GameMode::Survival | GameMode::Adventure => ClientboundPlayerAbilities {
            invulnerable: false,
            flying: false,
            can_fly: false,
            instabuild: false,
            flying_speed: 0.05,
            walking_speed: 0.1,
        },
    }
}
