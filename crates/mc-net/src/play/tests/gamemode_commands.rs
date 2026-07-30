use super::{GameMode, parse_gamemode_command};

#[test]
fn gamemode_command_parses_names_and_numeric_modes() {
    assert_eq!(
        parse_gamemode_command("gamemode survival"),
        Some(GameMode::Survival)
    );
    assert_eq!(
        parse_gamemode_command("gamemode creative"),
        Some(GameMode::Creative)
    );
    assert_eq!(
        parse_gamemode_command("gamemode adventure"),
        Some(GameMode::Adventure)
    );
    assert_eq!(
        parse_gamemode_command("gamemode spectator"),
        Some(GameMode::Spectator)
    );
    assert_eq!(
        parse_gamemode_command("gamemode 1"),
        Some(GameMode::Creative)
    );
}

#[test]
fn gamemode_command_rejects_unknown_or_extra_args() {
    assert_eq!(parse_gamemode_command("time set day"), None);
    assert_eq!(parse_gamemode_command("gamemode nope"), None);
    assert_eq!(parse_gamemode_command("gamemode creative other"), None);
}
