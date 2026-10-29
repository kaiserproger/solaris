use super::*;
use mc_protocol::Packet;

#[test]
fn nested_tick_root_keeps_current_level_two_edge_admission() {
    // Characterize the known gap together with the native interpreter test:
    // the edge checks the outer root, not the nested `tick` permission.
    assert!(!permitted("tick freeze", 2, &[]));
    assert!(permitted("execute run tick freeze", 2, &[]));
    assert!(permitted("tick freeze", 3, &[]));
    assert!(!permitted("execute run tick freeze", 1, &[]));
}

fn advertised<'a>(packet: &'a ClientboundCommands, path: &[&str]) -> &'a CommandNode {
    let mut node = &packet.nodes[packet.root_index as usize];
    for name in path {
        node = node
            .children
            .iter()
            .map(|i| &packet.nodes[*i as usize])
            .find(|node| match &node.kind {
                CommandNodeKind::Literal(value) => value == name,
                CommandNodeKind::Argument { name: value, .. } => value == name,
                CommandNodeKind::Root => false,
            })
            .unwrap();
    }
    node
}

#[test]
fn rc2_roots_are_advertised_and_admitted_at_level_two() {
    let packet = tree(2, &[]);
    for root in [
        "item",
        "loot",
        "attribute",
        "clone",
        "tag",
        "playsound",
        "particle",
    ] {
        advertised(&packet, &[root]);
        assert!(mc_core::commands::root_permitted(root, 2), "{root}");
        assert!(!mc_core::commands::root_permitted(root, 1), "{root}");
        assert!(permitted(root, 2, &[]), "{root}");
        assert!(!permitted(root, 1, &[]), "{root}");
    }
}

#[test]
fn border_summon_nbt_and_distinct_time_queries_use_exact_packet_parsers() {
    let packet = tree(2, &[]);
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut input = bytes.as_slice();
    let decoded = ClientboundCommands::decode(&mut input).unwrap();
    assert!(input.is_empty());
    for (path, expected) in [
        (&["worldborder", "center", "pos"][..], Parser::Vec2),
        (
            &["summon", "entity", "pos", "nbt"][..],
            Parser::NbtCompoundTag,
        ),
        (
            &["worldborder", "set", "distance"][..],
            Parser::Double {
                min: Some(1.0_f64.to_bits()),
                max: Some(59_999_968.0_f64.to_bits()),
            },
        ),
    ] {
        let node = advertised(&decoded, path);
        assert!(node.executable);
        let CommandNodeKind::Argument { parser, .. } = &node.kind else {
            panic!("argument");
        };
        assert_eq!(*parser, expected);
    }
    for query in ["time", "daytime", "day", "gametime"] {
        assert!(advertised(&decoded, &["time", "query", query]).executable);
    }
    assert!(advertised(&decoded, &["worldborder", "get"]).executable);
    assert!(permitted("worldborder set 50", 2, &[]));
    assert!(!permitted("worldborder set 50", 1, &[]));
}

#[test]
fn execute_and_datapack_roots_admit_namespaced_arguments_at_operator_level_two() {
    let packet = tree(2, &[]);
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut input = bytes.as_slice();
    let decoded = ClientboundCommands::decode(&mut input).unwrap();
    assert!(input.is_empty());
    for path in [
        &["execute", "arguments"][..],
        &["function", "name"][..],
        &["schedule", "function", "arguments"][..],
        &["schedule", "clear", "name"][..],
    ] {
        let node = advertised(&decoded, path);
        assert!(node.executable);
        assert!(matches!(
            &node.kind,
            CommandNodeKind::Argument {
                parser: Parser::String(CommandStringKind::GreedyPhrase),
                ..
            }
        ));
    }
    for command in [
        "execute as @a at @s positioned ~ ~1 ~ run function test:tick",
        "execute in minecraft:the_nether run setblock 0 80 0 minecraft:stone",
        "function #minecraft:tick",
        "schedule function test:entry 2s append",
        "schedule clear #test:tag",
        "data merge block 0 80 0 {Command:\"time add 1\",auto:1b}",
    ] {
        assert!(permitted(command, 2, &[]), "{command}");
        assert!(!permitted(command, 1, &[]), "{command}");
    }
}

/// LocateCommand.java:56-87, TickCommand.java:20-55,
/// TellRawCommand.java:12-30, TitleCommand.java:26-102,
/// BossBarCommands.java:65-176. `tick` needs op 3 (LEVEL_ADMINS);
/// the other four need op 2 (LEVEL_GAMEMASTERS). Parser ids 44/45
/// (`ResourceOrTag` / `ResourceOrTagKey`) round-trip through the packet.
#[test]
fn admin_commands_advertise_vanilla_roots_parsers_and_permissions() {
    for level in [2, 3, 4] {
        let packet = tree(level, &[]);
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        let mut input = bytes.as_slice();
        let decoded = ClientboundCommands::decode(&mut input).unwrap();
        assert!(input.is_empty());
        assert_eq!(decoded, packet);
        assert!(advertised(&decoded, &["locate", "structure", "structure"]).executable);
        assert!(advertised(&decoded, &["locate", "biome", "biome"]).executable);
        assert!(advertised(&decoded, &["locate", "poi", "poi"]).executable);
        for (path, expected) in [
            (
                &["locate", "structure", "structure"][..],
                Parser::ResourceOrTagKey {
                    registry: identifier("worldgen/structure"),
                },
            ),
            (
                &["locate", "biome", "biome"][..],
                Parser::ResourceOrTag {
                    registry: identifier("worldgen/biome"),
                },
            ),
            (
                &["locate", "poi", "poi"][..],
                Parser::ResourceOrTag {
                    registry: identifier("point_of_interest_type"),
                },
            ),
        ] {
            let node = advertised(&decoded, path);
            let CommandNodeKind::Argument { parser, .. } = &node.kind else {
                panic!("argument");
            };
            assert_eq!(*parser, expected, "{path:?}");
        }
        if level < 3 {
            let hidden_tick = tree(level, &[]);
            assert!(
                !hidden_tick.nodes[hidden_tick.root_index as usize]
                    .children
                    .iter()
                    .any(|index| matches!(
                        &hidden_tick.nodes[*index as usize].kind,
                        CommandNodeKind::Literal(name) if name == "tick"
                    ))
            );
        } else {
            for path in [
                &["tick", "query"][..],
                &["tick", "rate", "rate"][..],
                &["tick", "step"][..],
                &["tick", "step", "stop"][..],
                &["tick", "step", "time"][..],
                &["tick", "sprint", "stop"][..],
                &["tick", "sprint", "time"][..],
                &["tick", "unfreeze"][..],
                &["tick", "freeze"][..],
            ] {
                assert!(advertised(&decoded, path).executable, "{path:?}");
            }
            assert_eq!(
                advertised(&decoded, &["tick", "rate", "rate"]),
                advertised(&tree(level, &[]), &["tick", "rate", "rate"])
            );
            let rate = advertised(&decoded, &["tick", "rate", "rate"]);
            let CommandNodeKind::Argument { parser, .. } = &rate.kind else {
                panic!("argument");
            };
            assert_eq!(
                *parser,
                Parser::Float {
                    min: Some(1.0f32.to_bits()),
                    max: Some(10000.0f32.to_bits()),
                }
            );
            for path in [
                &["tick", "step", "time"][..],
                &["tick", "sprint", "time"][..],
            ] {
                let node = advertised(&decoded, path);
                let CommandNodeKind::Argument { parser, .. } = &node.kind else {
                    panic!("argument");
                };
                assert_eq!(*parser, Parser::Time { min: 1 }, "{path:?}");
            }
        }
        assert!(advertised(&decoded, &["tellraw", "targets", "message"]).executable);
        let targets = advertised(&decoded, &["tellraw", "targets"]);
        let CommandNodeKind::Argument { parser, .. } = &targets.kind else {
            panic!("argument");
        };
        assert_eq!(*parser, players(false));
        let message = advertised(&decoded, &["tellraw", "targets", "message"]);
        let CommandNodeKind::Argument { parser, .. } = &message.kind else {
            panic!("argument");
        };
        assert_eq!(*parser, Parser::Component);
        for section in ["title", "subtitle", "actionbar"] {
            assert!(
                advertised(&decoded, &["title", "targets", section, "title"]).executable,
                "{section}"
            );
        }
        assert!(advertised(&decoded, &["title", "targets", "clear"]).executable);
        assert!(advertised(&decoded, &["title", "targets", "reset"]).executable);
        assert!(
            advertised(
                &decoded,
                &["title", "targets", "times", "fadeIn", "stay", "fadeOut"]
            )
            .executable
        );
        assert!(advertised(&decoded, &["bossbar", "add", "id", "name"]).executable);
        assert!(advertised(&decoded, &["bossbar", "remove", "id"]).executable);
        assert!(advertised(&decoded, &["bossbar", "list"]).executable);
        for field in [
            "name", "color", "style", "value", "max", "visible", "players",
        ] {
            assert!(
                !advertised(&decoded, &["bossbar", "set", "id", field])
                    .children
                    .is_empty()
                    || advertised(&decoded, &["bossbar", "set", "id", field]).executable,
                "{field}"
            );
        }
        for query in ["value", "max", "visible", "players"] {
            assert!(
                advertised(&decoded, &["bossbar", "get", "id", query]).executable,
                "{query}"
            );
        }
    }
    for root in ["locate", "tellraw", "title", "bossbar"] {
        assert!(permitted(root, 2, &[]), "{root}");
        assert!(!permitted(root, 1, &[]), "{root}");
    }
    assert!(!permitted("tick", 2, &[]));
    assert!(permitted("tick", 3, &[]));
    assert!(!permitted("tick query", 2, &[]));
    assert!(permitted("tick query", 3, &[]));
    let hidden = tree(1, &[]);
    for root in ["locate", "tellraw", "title", "bossbar", "tick"] {
        assert!(
            !hidden.nodes[hidden.root_index as usize].children.iter().any(|index| {
                matches!(&hidden.nodes[*index as usize].kind, CommandNodeKind::Literal(name) if name == root)
            }),
            "{root}"
        );
    }
    let ops = tree(2, &[]);
    assert!(
        !ops.nodes[ops.root_index as usize].children.iter().any(|index| {
            matches!(&ops.nodes[*index as usize].kind, CommandNodeKind::Literal(name) if name == "tick")
        })
    );
    for root in ["locate", "tellraw", "title", "bossbar"] {
        assert!(
            ops.nodes[ops.root_index as usize].children.iter().any(|index| {
                matches!(&ops.nodes[*index as usize].kind, CommandNodeKind::Literal(name) if name == root)
            }),
            "{root}"
        );
    }
}

#[test]
fn admin_command_suggestions_cover_tick_locate_and_bossbar() {
    let names = ["Alice".to_owned()];
    let structures = vec![
        "minecraft:village_plains".to_owned(),
        "minecraft:stronghold".to_owned(),
        "#minecraft:village".to_owned(),
    ];
    let texts = |command: &str, level| {
        suggestions(1, command, level, &names, &[], &[], &[], &structures, &[])
            .suggestions
            .into_iter()
            .map(|entry| entry.text)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        texts("/tick ", 3),
        ["query", "rate", "step", "sprint", "unfreeze", "freeze"]
    );
    assert!(texts("/tick ", 2).is_empty());
    assert_eq!(texts("/tick rate ", 3), ["20"]);
    assert_eq!(texts("/tick step ", 3), ["stop", "1t", "1s"]);
    assert_eq!(texts("/tick sprint ", 3), ["stop", "60s", "1d", "3d"]);
    assert_eq!(texts("/tick step st", 3), ["stop"]);
    assert_eq!(texts("/tick sprint 6", 3), ["60s"]);
    assert_eq!(texts("/locate ", 2), ["structure", "biome", "poi"]);
    assert!(texts("/locate ", 1).is_empty());
    assert_eq!(
        texts("/locate structure minecraft:vil", 2),
        ["minecraft:village_plains"]
    );
    assert_eq!(texts("/locate structure #", 2), ["#minecraft:village"]);
    assert!(texts("/locate biome ", 2).is_empty());
    assert_eq!(
        texts("/bossbar ", 2),
        ["add", "remove", "list", "set", "get"]
    );
    assert_eq!(
        texts("/bossbar set x ", 2),
        [
            "name", "color", "style", "value", "max", "visible", "players"
        ]
    );
    assert_eq!(
        texts("/bossbar set x color ", 2),
        ["pink", "blue", "red", "green", "yellow", "purple", "white"]
    );
    assert_eq!(
        texts("/bossbar set x style ", 2),
        [
            "progress",
            "notched_6",
            "notched_10",
            "notched_12",
            "notched_20"
        ]
    );
    assert_eq!(texts("/bossbar set x visible ", 2), ["false", "true"]);
    assert_eq!(
        texts("/bossbar get x ", 2),
        ["value", "max", "visible", "players"]
    );
    assert_eq!(texts("/title ", 2), ["@a", "@p", "@r", "@s", "Alice"]);
    assert_eq!(
        texts("/title @s ", 2),
        ["clear", "reset", "title", "subtitle", "actionbar", "times"]
    );
    assert_eq!(texts("/tellraw ", 2), ["@a", "@p", "@r", "@s", "Alice"]);
}

#[test]
fn admin_command_help_matches_vanilla_usage_shapes() {
    assert_eq!(
        help(Some("tick"), 3, &[]).unwrap(),
        [
            "/tick query",
            "/tick rate <rate>",
            "/tick step [stop|<time>]",
            "/tick sprint (stop|<time>)",
            "/tick unfreeze",
            "/tick freeze",
        ]
    );
    assert!(help(Some("tick"), 2, &[]).is_err());
    assert_eq!(
        help(Some("locate"), 2, &[]).unwrap(),
        [
            "/locate structure <structure>",
            "/locate biome <biome>",
            "/locate poi <poi>",
        ]
    );
    assert_eq!(
        help(Some("tellraw"), 2, &[]).unwrap(),
        ["/tellraw <targets> <message>"]
    );
    assert_eq!(
        help(Some("title"), 2, &[]).unwrap(),
        ["/title <targets> (clear|reset|title|subtitle|actionbar|times)"]
    );
    assert_eq!(
        help(Some("bossbar"), 2, &[]).unwrap(),
        [
            "/bossbar add <id> <name>",
            "/bossbar remove <id>",
            "/bossbar list",
            "/bossbar set <id> (name|color|style|value|max|visible|players)",
            "/bossbar get <id> (value|max|visible|players)",
        ]
    );
    assert!(help(Some("locate"), 1, &[]).is_err());
    assert!(help(Some("bossbar"), 1, &[]).is_err());
}
