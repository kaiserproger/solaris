use super::*;
use mc_protocol::Packet;

#[test]
fn admin_inventory_trees_and_slot_suggestions() {
    let packet = tree(2, &[]);
    for root in ["item", "loot", "attribute"] {
        assert!(!permitted(root, 1, &[]));
        assert!(permitted(root, 2, &[]));
    }
    assert!(
        node(
            &packet,
            &[
                "item", "replace", "entity", "target", "slot", "with", "item"
            ]
        )
        .executable
    );
    assert!(node(&packet, &["loot", "give", "players", "loot", "loot_table"]).executable);
    assert!(
        node(
            &packet,
            &["attribute", "target", "attribute", "base", "reset"]
        )
        .executable
    );
    let response = suggestions(
        1,
        "/item replace entity @s hot",
        2,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
    );
    assert!(
        response
            .suggestions
            .iter()
            .any(|entry| entry.text == "hotbar.0")
    );
}

#[test]
fn map_command_trees_and_suggestions_match_implemented_paths() {
    let packet = tree(2, &[]);
    for root in ["tag", "playsound", "particle", "data"] {
        assert!(permitted(root, 2, &[]));
        assert!(!permitted(root, 1, &[]));
    }
    assert!(node(&packet, &["tag", "targets", "add", "name"]).executable);
    assert!(node(&packet, &["particle", "name"]).executable);
    assert!(
        node(
            &packet,
            &[
                "playsound",
                "sound",
                "master",
                "targets",
                "pos",
                "volume",
                "pitch",
                "minVolume"
            ]
        )
        .executable
    );
    assert!(
        node(
            &packet,
            &[
                "data", "modify", "storage", "target", "path", "set", "value", "value"
            ]
        )
        .executable
    );
    let response = suggestions(1, "/particle minecraft:fl", 2, &[], &[], &[], &[], &[], &[]);
    assert!(
        response
            .suggestions
            .iter()
            .any(|entry| entry.text == "minecraft:flame")
    );
}

#[test]
fn damage_tree_and_permission_match_native_grammar() {
    let packet = tree(2, &[]);
    assert!(!permitted("damage @s 1", 1, &[]));
    assert!(permitted("damage @s 1", 2, &[]));
    assert_eq!(parser(&packet, &["damage", "target"]), entities(true));
    assert_eq!(
        parser(&packet, &["damage", "target", "amount"]),
        Parser::Float {
            min: Some(0.0_f32.to_bits()),
            max: None
        }
    );
    let kind = ["damage", "target", "amount", "damageType"];
    assert_eq!(
        parser(&packet, &kind),
        Parser::Resource {
            registry: identifier("damage_type")
        }
    );
    assert!(node(&packet, &["damage", "target", "amount"]).executable);
    assert!(node(&packet, &kind).executable);
    assert_eq!(
        parser(
            &packet,
            &["damage", "target", "amount", "damageType", "at", "location"]
        ),
        Parser::Vec3
    );
    assert_eq!(
        parser(
            &packet,
            &[
                "damage",
                "target",
                "amount",
                "damageType",
                "by",
                "entity",
                "from",
                "cause"
            ]
        ),
        entities(true)
    );
}

#[test]
fn advancement_tree_covers_actions_scopes_and_greedy_criterion() {
    let packet = tree(2, &[]);
    assert!(!permitted("advancement grant @s everything", 1, &[]));
    for action in ["grant", "revoke"] {
        assert_eq!(
            parser(&packet, &["advancement", action, "targets"]),
            players(false)
        );
        assert!(node(&packet, &["advancement", action, "targets", "everything"]).executable);
        for mode in ["only", "from", "until", "through"] {
            let path = ["advancement", action, "targets", mode, "advancement"];
            assert!(node(&packet, &path).executable);
            assert_eq!(
                parser(&packet, &path),
                Parser::ResourceKey {
                    registry: identifier("advancement")
                },
            );
        }
        assert_eq!(
            parser(
                &packet,
                &[
                    "advancement",
                    action,
                    "targets",
                    "only",
                    "advancement",
                    "criterion"
                ]
            ),
            Parser::String(CommandStringKind::GreedyPhrase),
        );
    }
}

#[test]
fn forceload_tree_matches_vanilla_column_syntax_and_is_operator_gated() {
    let packet = tree(2, &[]);
    for path in [
        &["forceload", "add", "from"][..],
        &["forceload", "add", "from", "to"][..],
        &["forceload", "remove", "from"][..],
        &["forceload", "remove", "from", "to"][..],
        &["forceload", "remove", "all"][..],
        &["forceload", "query"][..],
        &["forceload", "query", "pos"][..],
    ] {
        assert!(node(&packet, path).executable, "{path:?}");
    }
    assert!(!node(&packet, &["forceload"]).executable);
    assert!(!node(&packet, &["forceload", "add"]).executable);
    for path in [
        &["forceload", "add", "from"][..],
        &["forceload", "add", "from", "to"][..],
        &["forceload", "remove", "from", "to"][..],
        &["forceload", "query", "pos"][..],
    ] {
        assert_eq!(parser(&packet, path), Parser::ColumnPos, "{path:?}");
    }
    let roots = |level| {
        let packet = tree(level, &[]);
        packet.nodes[0].children.iter().any(|child| {
            matches!(&packet.nodes[*child as usize].kind,
                    CommandNodeKind::Literal(name) if name == "forceload")
        })
    };
    assert!(!roots(1) && roots(2));
}

#[test]
fn effect_tree_matches_vanilla_optional_arguments_and_resource_registry() {
    let packet = tree(2, &[]);
    assert!(node(&packet, &["effect", "clear"]).executable);
    assert!(node(&packet, &["effect", "clear", "targets"]).executable);
    for operation in ["give", "clear"] {
        assert_eq!(
            parser(&packet, &["effect", operation, "targets"]),
            entities(false)
        );
        assert_eq!(
            parser(&packet, &["effect", operation, "targets", "effect"]),
            Parser::Resource {
                registry: identifier("mob_effect")
            }
        );
        assert!(node(&packet, &["effect", operation, "targets", "effect"]).executable);
    }
    assert_eq!(
        parser(&packet, &["effect", "give", "targets", "effect", "seconds"]),
        integer(Some(1), Some(1_000_000))
    );
    for duration in ["seconds", "infinite"] {
        assert!(node(&packet, &["effect", "give", "targets", "effect", duration]).executable);
        assert_eq!(
            parser(
                &packet,
                &["effect", "give", "targets", "effect", duration, "amplifier"]
            ),
            integer(Some(0), Some(255))
        );
        assert_eq!(
            parser(
                &packet,
                &[
                    "effect",
                    "give",
                    "targets",
                    "effect",
                    duration,
                    "amplifier",
                    "hideParticles"
                ]
            ),
            Parser::Bool
        );
    }
}

#[test]
fn effect_suggestions_cover_actions_targets_ids_infinite_and_visibility() {
    let names = ["Alice".to_owned()];
    let texts = |command: &str, level| {
        suggestions(1, command, level, &names, &[], &[], &[], &[], &[])
            .suggestions
            .into_iter()
            .map(|entry| entry.text)
            .collect::<Vec<_>>()
    };
    assert_eq!(texts("effect ", 2), ["give", "clear"]);
    assert!(texts("effect ", 1).is_empty());
    assert!(texts("effect give @", 2).contains(&"@e".to_owned()));
    assert_eq!(texts("effect clear A", 2), ["Alice"]);
    for operation in ["give", "clear"] {
        assert_eq!(
            texts(&format!("effect {operation} @s dar"), 2),
            ["minecraft:darkness"]
        );
    }
    assert_eq!(texts("effect give @s speed inf", 2), ["infinite"]);
    assert_eq!(
        texts("effect give @s speed infinite 255 ", 2),
        ["false", "true"]
    );
}

fn node<'a>(packet: &'a ClientboundCommands, path: &[&str]) -> &'a CommandNode {
    let mut current = &packet.nodes[packet.root_index as usize];
    for name in path {
        if let Some(redirect) = current.redirect {
            current = &packet.nodes[redirect as usize];
        }
        current = current
            .children
            .iter()
            .map(|index| &packet.nodes[*index as usize])
            .find(|node| match &node.kind {
                CommandNodeKind::Literal(literal) => literal == name,
                CommandNodeKind::Argument { name: argument, .. } => argument == name,
                CommandNodeKind::Root => false,
            })
            .unwrap_or_else(|| panic!("advertised command path {path:?} missing {name:?}"));
    }
    current
}

fn parser(packet: &ClientboundCommands, path: &[&str]) -> Parser {
    match &node(packet, path).kind {
        CommandNodeKind::Argument { parser, .. } => parser.clone(),
        _ => panic!("expected argument"),
    }
}

#[test]
fn decoded_trees_round_trip_and_every_node_is_reachable() {
    for level in 0..=4 {
        let packet = tree(level, &[]);
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        let mut input = bytes.as_slice();
        let decoded = ClientboundCommands::decode(&mut input).unwrap();
        assert_eq!(decoded, packet);
        assert!(input.is_empty());
        let mut visited = vec![false; decoded.nodes.len()];
        let mut pending = vec![decoded.root_index];
        while let Some(index) = pending.pop() {
            assert!(index >= 0 && (index as usize) < decoded.nodes.len());
            if std::mem::replace(&mut visited[index as usize], true) {
                continue;
            }
            let node = &decoded.nodes[index as usize];
            pending.extend_from_slice(&node.children);
            pending.extend(node.redirect);
            if node.children.is_empty() && node.redirect.is_none() {
                assert!(node.executable, "unusable leaf: {:?}", node.kind);
            }
        }
        assert!(visited.into_iter().all(|reachable| reachable));
    }
}

#[test]
fn nonoperators_only_receive_public_roots_and_thresholds_match_vanilla() {
    for level in 0..=4 {
        let packet = tree(level, &[]);
        let roots: Vec<&str> = packet.nodes[0]
            .children
            .iter()
            .map(|index| match &packet.nodes[*index as usize].kind {
                CommandNodeKind::Literal(name) => name.as_str(),
                _ => panic!("root child must be a literal"),
            })
            .collect();
        let expected: Vec<&str> = ROOTS
            .iter()
            .filter(|(_, required)| level >= *required)
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(roots.len(), expected.len());
        for root in expected {
            assert!(roots.contains(&root));
        }
        if level < 2 {
            assert_eq!(
                roots,
                [
                    "help",
                    "list",
                    "me",
                    "msg",
                    "tell",
                    "w",
                    "teammsg",
                    "tm",
                    "settlement",
                    "trigger"
                ]
            );
        }
        for &index in &packet.nodes[0].children {
            let root = &packet.nodes[index as usize];
            assert_eq!(
                root.restricted,
                level >= 2
                    && !matches!(&root.kind, CommandNodeKind::Literal(name) if ["help", "list", "me", "msg", "tell", "w", "teammsg", "tm", "settlement", "trigger"].contains(&name.as_str()))
            );
        }
    }
}

#[test]
fn aliases_redirect_to_canonical_nodes_without_duplicate_subtrees() {
    let packet = tree(4, &[]);
    for (alias, canonical) in [
        ("tp", "teleport"),
        ("xp", "experience"),
        ("tell", "msg"),
        ("w", "msg"),
    ] {
        let alias = node(&packet, &[alias]);
        let canonical = node(&packet, &[canonical]);
        assert!(alias.children.is_empty());
        assert!(!alias.executable);
        assert_eq!(&packet.nodes[alias.redirect.unwrap() as usize], canonical);
    }
    assert!(node(&packet, &["tp", "location"]).executable);
    assert!(node(&packet, &["tp", "location", "rotation"]).executable);
    assert!(node(&packet, &["tp", "targets", "location", "rotation"]).executable);
    assert!(node(&packet, &["w", "targets", "message"]).executable);
}

#[test]
fn parser_properties_and_executable_optional_arguments_are_exact() {
    let packet = tree(4, &[]);
    for (path, expected) in [
        (&["teleport", "location", "rotation"][..], Parser::Rotation),
        (
            &["teleport", "targets", "location", "rotation"][..],
            Parser::Rotation,
        ),
        (&["give", "targets"][..], players(false)),
        (&["give", "targets", "item"][..], Parser::ItemStack),
        (
            &["give", "targets", "item", "count"][..],
            integer(Some(1), None),
        ),
        (&["clear", "targets", "item"][..], Parser::ItemPredicate),
        (
            &["clear", "targets", "item", "maxCount"][..],
            integer(Some(0), None),
        ),
        (&["gamemode", "gamemode"][..], Parser::GameMode),
        (&["teleport", "destination"][..], entities(true)),
        (&["teleport", "targets"][..], entities(false)),
        (&["teleport", "location"][..], Parser::Vec3),
        (&["time", "set", "time"][..], Parser::Time { min: 0 }),
        (&["time", "add", "time"][..], Parser::Time { min: i32::MIN }),
        (
            &["weather", "rain", "duration"][..],
            Parser::Time { min: 1 },
        ),
        (
            &["spawnpoint", "targets", "pos", "rotation"][..],
            Parser::Rotation,
        ),
        (
            &["enchant", "targets", "enchantment", "level"][..],
            integer(Some(0), None),
        ),
        (&["experience", "query", "target"][..], players(true)),
        (
            &["scoreboard", "players", "get", "target"][..],
            Parser::ScoreHolder { multiple: false },
        ),
        (
            &[
                "scoreboard",
                "players",
                "add",
                "targets",
                "objective",
                "score",
            ][..],
            integer(Some(0), None),
        ),
    ] {
        assert_eq!(parser(&packet, path), expected, "{path:?}");
    }
    assert_eq!(
        parser(&packet, &["summon", "entity"]),
        Parser::Resource {
            registry: identifier("entity_type")
        }
    );
    assert_eq!(
        parser(&packet, &["enchant", "targets", "enchantment"]),
        Parser::Resource {
            registry: identifier("enchantment")
        }
    );
    assert_eq!(
        integer(Some(i32::MIN), Some(i32::MAX)),
        Parser::Integer {
            min: None,
            max: None
        }
    );
    for path in [
        &["give", "targets", "item"][..],
        &["clear"][..],
        &["difficulty"][..],
        &["summon", "entity"][..],
        &["spawnpoint"][..],
        &["scoreboard", "objectives", "setdisplay", "slot"][..],
    ] {
        assert!(node(&packet, path).executable);
    }
    for rule in rules() {
        let expected = match rule.kind {
            RuleKind::Boolean => Parser::Bool,
            RuleKind::Integer { min, max } => integer(Some(min), Some(max)),
        };
        assert_eq!(parser(&packet, &["gamerule", rule.name, "value"]), expected);
        assert_eq!(
            parser(
                &packet,
                &["gamerule", &format!("minecraft:{}", rule.name), "value"]
            ),
            expected
        );
    }
    for (path, expected) in [
        (&["summon", "entity"][..], "summonable_entities"),
        (&["time", "set", "timemarker"][..], "ask_server"),
        (&["team", "leave", "members"][..], "ask_server"),
    ] {
        match &node(&packet, path).kind {
            CommandNodeKind::Argument { suggestions, .. } => {
                assert_eq!(*suggestions, Some(identifier(expected)))
            }
            _ => panic!("expected suggested argument"),
        }
    }
}

#[test]
fn permission_helper_matches_advertisement() {
    for level in 0..=4 {
        for &(root, required) in ROOTS {
            assert_eq!(permitted(root, level, &[]), level >= required);
            assert_eq!(
                permitted(&format!("/{root} arguments"), level, &[]),
                level >= required
            );
        }
    }
    assert!(permitted("effect", 2, &[]));
    assert!(!permitted("effect", 1, &[]));
    assert!(!permitted("", 4, &[]));
}

#[test]
fn suggestions_filter_permissions_registries_and_use_utf16_ranges() {
    let names = vec!["Alice".to_owned(), "😀Bob".to_owned()];
    let items = vec!["minecraft:stone".to_owned(), "minecraft:stick".to_owned()];
    let response = suggestions(7, "/s", 0, &names, &items, &[], &[], &[], &[]);
    assert_eq!((response.id, response.start, response.length), (7, 1, 1));
    assert_eq!(
        response
            .suggestions
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>(),
        ["settlement"]
    );
    let response = suggestions(8, "/give @s st", 2, &names, &items, &[], &[], &[], &[]);
    assert_eq!((response.start, response.length), (9, 2));
    assert_eq!(response.suggestions.len(), 2);
    let response = suggestions(9, "/msg 😀B", 0, &names, &items, &[], &[], &[], &[]);
    assert_eq!((response.start, response.length), (5, 3));
    assert_eq!(response.suggestions[0].text, "😀Bob");
    assert!(
        suggestions(10, "give @s ", 0, &names, &items, &[], &[], &[], &[])
            .suggestions
            .is_empty()
    );
    let response = suggestions(11, "msg @", 0, &names, &items, &[], &[], &[], &[]);
    assert!(
        !response
            .suggestions
            .iter()
            .any(|entry| entry.text == "@e" || entry.text == "@n")
    );
    let response = suggestions(12, "kill @", 2, &names, &items, &[], &[], &[], &[]);
    assert!(response.suggestions.iter().any(|entry| entry.text == "@e"));
}

#[test]
fn help_derives_smart_optional_and_alternative_usages_from_the_tree() {
    let public = help(None, 0, &[]).unwrap();
    assert_eq!(public.len(), 10);
    assert!(public.contains(&"/trigger <objective> [add|set]".to_owned()));
    assert!(public.contains(&"/help [<command>]".to_owned()));
    assert!(public.contains(&"/list [uuids]".to_owned()));
    assert!(public.contains(&"/msg <targets> <message>".to_owned()));
    assert!(public.contains(&"/tell -> msg".to_owned()));
    assert!(public.contains(&"/teammsg <message>".to_owned()));
    assert!(public.contains(&"/tm -> teammsg".to_owned()));
    assert!(public.iter().all(|line| !line.contains("gamemode")));
    assert_eq!(
        help(Some("give"), 2, &[]).unwrap(),
        ["/give <targets> <item> [<count>]"]
    );
    assert_eq!(
        help(Some("give <targets>"), 2, &[]).unwrap(),
        ["/give <targets> <item> [<count>]"]
    );
    assert_eq!(
        help(Some("time"), 2, &[]).unwrap(),
        [
            "/time set (<time>|<timemarker>)",
            "/time add <time>",
            "/time query (time|daytime|day|gametime)",
        ]
    );
    assert_eq!(
        help(Some("give targets item"), 2, &[]).unwrap(),
        ["/give targets item [<count>]"]
    );
    assert!(
        help(Some("give targets item count"), 2, &[])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn help_resolves_aliases_and_rejects_hidden_or_unknown_roots() {
    assert_eq!(help(Some("w"), 0, &[]).unwrap(), ["/w <targets> <message>"]);
    assert_eq!(
        help(Some("xp query"), 2, &[]).unwrap(),
        ["/xp query <target> (points|levels)"]
    );
    assert_eq!(
        help(Some("/scoreboard players get"), 2, &[]).unwrap(),
        ["/scoreboard players get <target> <objective>"]
    );
    for path in ["give", "giveaway", "op", "effect", ""] {
        assert!(help(Some(path), 0, &[]).is_err(), "{path}");
    }
    let partial = help(Some("scoreboard players missing"), 2, &[]).unwrap();
    assert!(!partial.is_empty());
    assert!(
        partial
            .iter()
            .all(|usage| usage.starts_with("/scoreboard players missing "))
    );
    assert_eq!(
        help(Some("give <missing>"), 2, &[]).unwrap(),
        ["/give <missing> <targets> <item> [<count>]"]
    );
    assert!(help(Some("stop"), 3, &[]).is_err());
    assert!(help(Some("stop"), 4, &[]).unwrap().is_empty());
}

#[test]
fn help_traverses_actual_values_and_prefers_the_longest_argument_branch() {
    assert_eq!(
        help(Some("gamemode creative"), 2, &[]).unwrap(),
        ["/gamemode creative [<target>]"]
    );
    assert_eq!(
        help(Some("give Bob"), 2, &[]).unwrap(),
        ["/give Bob <item> [<count>]"]
    );
    assert_eq!(
        help(Some("give Bob minecraft:stone"), 2, &[]).unwrap(),
        ["/give Bob minecraft:stone [<count>]"]
    );
    assert_eq!(
        help(Some("teleport Bob 0 80 0"), 2, &[]).unwrap(),
        ["/teleport Bob 0 80 0 [<rotation>]"]
    );
    assert_eq!(
        help(Some("tp Bob ~ ~1 ~"), 2, &[]).unwrap(),
        ["/tp Bob ~ ~1 ~ [<rotation>]"]
    );
    assert_eq!(
        help(Some("teleport 0 80 0"), 2, &[]).unwrap(),
        ["/teleport 0 80 0 [<rotation>]"]
    );
    assert!(
        help(Some("teleport Bob 0 80 0 90 0"), 2, &[])
            .unwrap()
            .is_empty()
    );
    assert!(help(Some("tp 0 80 0 ~90 ~"), 2, &[]).unwrap().is_empty());
    assert_eq!(
        help(Some("scoreboard players set Bob"), 2, &[]).unwrap(),
        ["/scoreboard players set Bob <objective> <score>"]
    );
    assert_eq!(
        help(Some("weather rain"), 2, &[]).unwrap(),
        ["/weather rain [<duration>]"]
    );
    assert!(help(Some("weather rain 10s"), 2, &[]).unwrap().is_empty());
    assert_eq!(
        help(Some("spawnpoint Bob 0 80 0"), 2, &[]).unwrap(),
        ["/spawnpoint Bob 0 80 0 [<rotation>]"]
    );
    assert!(
        help(Some("spawnpoint Bob 0 80 0 90 0"), 2, &[])
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        help(
            Some("give Bob minecraft:stone[custom_name='hello world']"),
            2,
            &[]
        )
        .unwrap(),
        ["/give Bob minecraft:stone[custom_name='hello world'] [<count>]"]
    );
    assert_eq!(
        help(Some("give Bob stone nope"), 2, &[]).unwrap(),
        ["/give Bob stone nope [<count>]"]
    );
    assert_eq!(
        help_argument_length(&Parser::Vec3, "0 80 0 remaining"),
        Some(6)
    );
    assert_eq!(help_argument_length(&Parser::Vec3, "0 80"), None);
    assert_eq!(help_argument_length(&integer(Some(1), None), "0"), None);
    assert_eq!(
        help_argument_length(&Parser::Time { min: 1 }, "2s"),
        Some(2)
    );
}
