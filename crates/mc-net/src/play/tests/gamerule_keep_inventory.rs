#[test]
fn keep_inventory_gamerule_parses_queries_updates_and_rejections() {
    let op = CommandPermissions { op: true };
    assert_eq!(
        parse_admin_command("/gamerule keep_inventory", op),
        Ok(AdminCommand::KeepInventory(None))
    );
    assert_eq!(
        parse_admin_command(&["/gamerule keep_inventory", "true"].join(" "), op),
        Ok(AdminCommand::KeepInventory(Some(true)))
    );
    assert_eq!(
        parse_admin_command(&["/gamerule keep_inventory", "false"].join(" "), op),
        Ok(AdminCommand::KeepInventory(Some(false)))
    );
    assert_eq!(
        parse_admin_command("/gamerule keep_inventory yes", op),
        Err(CommandError::Usage(
            "Usage: /gamerule <do_daylight_cycle|keep_inventory|players_sleeping_percentage> [value]"
        ))
    );

    let suggestions = command_suggestions("/gamerule k", op);
    assert_eq!(suggestions.start, 10);
    assert_eq!(suggestions.length, 1);
    assert_eq!(suggestions.suggestions, vec!["keep_inventory".to_string()]);
}
