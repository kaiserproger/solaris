use crate::login::LoggedInProfile;
use crate::play::command_execution::runtime_control_status_message;
use crate::play::commands::{
    AdminCommand, CommandError, CommandPermissions, command_suggestions, command_tree_packet,
    parse_admin_command,
};
use mc_protocol::packets::play::GameMode;

#[test]
fn admin_dispatcher_parses_slash_commands_and_permissions() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    assert_eq!(
        parse_admin_command("/gamemode creative", op),
        Ok(AdminCommand::GameMode(GameMode::Creative))
    );
    assert_eq!(
        parse_admin_command("give minecraft:dirt 12", op),
        Ok(AdminCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 12,
        })
    );
    assert_eq!(
        parse_admin_command("/tp 1.5 70 -2", op),
        Ok(AdminCommand::Teleport {
            x: 1.5,
            y: 70.0,
            z: -2.0,
        })
    );
    assert_eq!(
        parse_admin_command("/summon minecraft:zombie", op),
        Ok(AdminCommand::Summon {
            entity: mc_data::Identifier::parse("minecraft:zombie").unwrap(),
            x: None,
            y: None,
            z: None,
        })
    );
    assert_eq!(parse_admin_command("/kill", op), Ok(AdminCommand::Kill));
    assert_eq!(parse_admin_command("/status", op), Ok(AdminCommand::Status));
    assert_eq!(
        parse_admin_command("/gamerule players_sleeping_percentage", op),
        Ok(AdminCommand::PlayersSleepingPercentage(None))
    );
    assert_eq!(
        parse_admin_command("/gamerule players_sleeping_percentage 50", op),
        Ok(AdminCommand::PlayersSleepingPercentage(Some(50)))
    );
    assert_eq!(
        parse_admin_command("/gamerule do_daylight_cycle", op),
        Ok(AdminCommand::DaylightCycle(None))
    );
    assert_eq!(
        parse_admin_command("/gamerule do_daylight_cycle false", op),
        Ok(AdminCommand::DaylightCycle(Some(false)))
    );
    assert_eq!(
        parse_admin_command("/gamemode creative", not_op),
        Err(CommandError::PermissionDenied)
    );
    assert_eq!(
        parse_admin_command("/status extra", op),
        Err(CommandError::Usage("Usage: /status"))
    );
    assert_eq!(
        parse_admin_command("/gamemode", op),
        Err(CommandError::Usage(
            "Usage: /gamemode <survival|creative|adventure|spectator>"
        ))
    );
    assert_eq!(
        parse_admin_command("/doesnotexist", op),
        Err(CommandError::Unknown)
    );
    assert_eq!(
        parse_admin_command("/tp NaN 70 0", op),
        Err(CommandError::Usage("Usage: /tp <x> <y> <z>"))
    );
    assert_eq!(
        parse_admin_command("/tp 0 inf 0", op),
        Err(CommandError::Usage("Usage: /tp <x> <y> <z>"))
    );
    assert_eq!(
        parse_admin_command("/summon minecraft:zombie 0 70 -inf", op),
        Err(CommandError::Usage("Usage: /summon <entity> [x y z]"))
    );
    assert_eq!(
        parse_admin_command("/gamerule players_sleeping_percentage -1", op),
        Err(CommandError::Usage(
            "Usage: /gamerule <do_daylight_cycle|keep_inventory|players_sleeping_percentage> [value]"
        ))
    );
}

#[test]
fn command_tree_and_suggestions_are_permission_aware() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    let tree = command_tree_packet(op);
    assert_eq!(tree.root_index, 0);
    assert_eq!(
        tree.nodes[0].children,
        vec![1, 6, 8, 10, 11, 12, 13, 15, 17, 19, 20, 27]
    );
    assert_eq!(tree.nodes[20].children, vec![21, 23, 25]);
    assert_eq!(
        tree.nodes[23],
        mc_protocol::packets::play::CommandNode::literal("do_daylight_cycle", vec![24], true,)
            .restricted(true)
    );
    assert_eq!(
        command_tree_packet(not_op).nodes[0].children,
        Vec::<i32>::new()
    );

    let root = command_suggestions("/g", op);
    assert_eq!(root.start, 1);
    assert_eq!(root.length, 1);
    assert_eq!(
        root.suggestions,
        vec![
            "gamemode".to_string(),
            "gamerule".to_string(),
            "give".to_string()
        ]
    );

    let modes = command_suggestions("/gamemode c", op);
    assert_eq!(modes.start, 10);
    assert_eq!(modes.length, 1);
    assert_eq!(modes.suggestions, vec!["creative".to_string()]);

    let gamerules = command_suggestions("/gamerule p", op);
    assert_eq!(gamerules.start, 10);
    assert_eq!(gamerules.length, 1);
    assert_eq!(
        gamerules.suggestions,
        vec!["players_sleeping_percentage".to_string()]
    );
    assert_eq!(
        command_suggestions("/gamerule d", op).suggestions,
        vec!["do_daylight_cycle".to_string()]
    );

    let status = command_suggestions("/st", op);
    assert_eq!(status.start, 1);
    assert_eq!(status.length, 2);
    assert_eq!(
        status.suggestions,
        vec!["status".to_string(), "stop".to_string()]
    );

    assert!(command_suggestions("/g", not_op).suggestions.is_empty());
}

#[test]
fn runtime_control_status_message_reports_disabled_and_drain_snapshot() {
    assert_eq!(
        runtime_control_status_message(None),
        "Runtime control: disabled"
    );

    let control = crate::RuntimeControlHandle::new(crate::RuntimeControlConfig {
        policy: crate::AutoscalePolicy {
            min_view_distance: 2,
            max_view_distance: 8,
            min_chunk_send_rate: 1,
            max_chunk_send_rate: 16,
            min_chunk_load_rate: 2,
            max_chunk_load_rate: 64,
            min_chunk_generate_rate: 3,
            max_chunk_generate_rate: 32,
            ..crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced)
        },
        initial_limits: crate::RuntimeControlLimits {
            view_distance: 8,
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
        },
    });
    control.request_drain();

    assert_eq!(
        runtime_control_status_message(Some(&control)),
        "Runtime control: draining=true action=scale_down pressure=none limits=view_distance:2,send:1,load:2,generate:3 pressure_ticks=0 healthy_ticks=0 reason=drain requested; clamped to minimum chunk throughput"
    );
}

#[test]
fn local_dev_profiles_are_op_capable_for_now() {
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "op_probe".to_string(),
    };

    let permissions = crate::server::CommandPermissionConfig::new(Vec::<String>::new(), true)
        .permissions_for(&profile, "127.0.0.1:40000".parse().unwrap());

    assert!(permissions.can_change_game_mode());
    assert!(permissions.can_use_admin_commands());
}
