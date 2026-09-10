#[tokio::test]
async fn common_contact_damage_sources_are_detected_from_world_state() {
    let reports = solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&reports).expect("embedded vanilla registry builds"),
    );
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &reports,
    ));
    state.block_light = Some(Arc::new(
        mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&reports),
    ));
    insert_fluid_test_chunk(&state).await;
    let pose = PlayerPose::new(0.5, 64.0, 0.5);

    for (block_name, expected) in [
        ("minecraft:air", None),
        ("minecraft:fire", Some((1.0, PlayerDamageKind::Fire))),
        ("minecraft:lava", Some((4.0, PlayerDamageKind::Lava))),
        ("minecraft:stone", None),
    ] {
        let state_id = blocks
            .block(&Identifier::parse(block_name).expect("valid contact block name"))
            .unwrap_or_else(|| panic!("missing contact block {block_name}"))
            .default;
        state
            .world
            .lock()
            .await
            .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, state_id)
            .expect("contact fixture block is inside the loaded chunk");

        assert_eq!(
            player_damage_adapter::contact_block_damage(&state, pose).await,
            expected,
            "{block_name}"
        );
    }
    state
        .world
        .lock()
        .await
        .set_block_at(
            mc_world::BlockPos { x: 0, y: 65, z: 0 },
            blocks
                .block(&Identifier::parse("minecraft:stone").unwrap())
                .unwrap()
                .default,
        )
        .unwrap();
    assert_eq!(
        player_damage_adapter::contact_block_damage(&state, pose).await,
        Some((1.0, PlayerDamageKind::Suffocation)),
    );
}
