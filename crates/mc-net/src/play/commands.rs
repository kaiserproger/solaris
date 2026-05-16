use mc_protocol::packets::play::{ClientboundPlayerAbilities, GameMode};

use crate::login::LoggedInProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CommandPermissions {
    pub(super) op: bool,
}

impl CommandPermissions {
    pub(super) fn for_local_dev_profile(_profile: &LoggedInProfile) -> Self {
        Self { op: true }
    }

    pub(super) const fn can_change_game_mode(self) -> bool {
        self.op
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum SurvivalCommand {
    Damage(f32),
    Heal(f32),
    Feed { food: i32, saturation: f32 },
    Exhaust(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum DebugCommand {
    Survival(SurvivalCommand),
    Give {
        item: mc_data::Identifier,
        count: i32,
        hotbar_slot: u8,
    },
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
    let count = parts.next().unwrap_or("1").parse().ok()?;
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
