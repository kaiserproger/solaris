use super::super::*;

#[test]
fn parses_operator_management_commands() {
    let cli = Cli::try_parse_from([
        "mc-server",
        "--config",
        "server.toml",
        "operator",
        "add",
        "Alice",
    ])
    .unwrap();
    assert_eq!(cli.config, PathBuf::from("server.toml"));
    assert!(!cli.check);
    assert!(matches!(
        cli.command,
        Some(Command::Operator {
            command: OperatorCommand::Add { identity }
        }) if identity == "Alice"
    ));
}
