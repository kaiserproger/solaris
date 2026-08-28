use std::time::Duration;

use anyhow::{Context, Result, ensure};
use mc_protocol::RawFrame;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundSetPassengers, EntityEvent, MovePlayerFlags, RemoveEntities,
    SHEEP_ENTITY_DATA_WOOL_INDEX, SynchronizePlayerPosition,
};
use mc_test_harness::parity::ServerKind;

use super::model::{
    EntityAliases, EntityFact, EntityStatePacket, EvidenceState, MilliblockPosition, ScenarioId,
    ScenarioObservation,
};
use super::protocol::{
    CLIENTBOUND_DAMAGE_EVENT_ID, CLIENTBOUND_REMOVE_MOB_EFFECT_ID, CLIENTBOUND_SET_EQUIPMENT_ID,
    CLIENTBOUND_UPDATE_ATTRIBUTES_ID, CLIENTBOUND_UPDATE_MOB_EFFECT_ID,
};
use super::support::{EntityEndpoint, EntityProtocolHarness, server_label};

const PASSENGER_CONTROL_BLOCKER: &str = "Solaris exposes no wire command or interaction path that constructs a passenger graph; summon accepts only entity type and coordinates";
const PASSIVE_SCHEDULE_BLOCKER: &str = "the side-by-side parity harness cannot yet construct equivalent persisted villager brain/POI schedule fixtures on Solaris and vanilla; Solaris production schedule movement is covered by villager_schedule_presence";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScenarioSpec {
    pub(crate) id: ScenarioId,
    pub(crate) slug: &'static str,
    pub(crate) failure_timeout: Duration,
}

pub(crate) fn scenario_catalog() -> Vec<ScenarioSpec> {
    [
        (
            ScenarioId::MetadataDirtyDefault,
            "metadata-dirty-default",
            8,
        ),
        (ScenarioId::DamageDeath, "damage-death", 8),
        (
            ScenarioId::AttributesEquipmentEffects,
            "attributes-equipment-effects",
            8,
        ),
        (ScenarioId::PassiveAiSchedule, "passive-ai-schedule", 8),
        (ScenarioId::CollisionStep, "collision-step", 8),
        (
            ScenarioId::LifecyclePassengerCleanup,
            "lifecycle-passenger-cleanup",
            8,
        ),
    ]
    .into_iter()
    .map(|(id, slug, seconds)| ScenarioSpec {
        id,
        slug,
        failure_timeout: Duration::from_secs(seconds),
    })
    .collect()
}

pub(crate) async fn run_scenario_catalog(
    harness: &mut EntityProtocolHarness,
) -> Result<Vec<ScenarioObservation>> {
    let mut observations = Vec::new();
    for spec in scenario_catalog() {
        observations.push(run_scenario_spec(harness, spec).await?);
    }
    Ok(observations)
}

pub(crate) async fn run_isolated_scenario_catalog(
    endpoint: EntityEndpoint,
    client_prefix: &str,
    connect_timeout: Duration,
) -> Result<Vec<ScenarioObservation>> {
    let mut observations = Vec::new();
    for (index, spec) in scenario_catalog().into_iter().enumerate() {
        let client_name = format!("{client_prefix}{index}");
        let mut harness = EntityProtocolHarness::connect(endpoint, &client_name, connect_timeout)
            .await
            .with_context(|| format!("connect isolated scenario {}", spec.slug))?;
        observations.push(run_scenario_spec(&mut harness, spec).await?);
    }
    Ok(observations)
}

async fn run_scenario_spec(
    harness: &mut EntityProtocolHarness,
    spec: ScenarioSpec,
) -> Result<ScenarioObservation> {
    tokio::time::timeout(spec.failure_timeout, run_scenario(harness, spec.id))
        .await
        .with_context(|| {
            format!(
                "{} scenario {} timed out after {:?}",
                server_label(harness.kind()),
                spec.slug,
                spec.failure_timeout
            )
        })?
        .with_context(|| {
            format!(
                "{} scenario {} failed",
                server_label(harness.kind()),
                spec.slug
            )
        })
}

async fn run_scenario(
    harness: &mut EntityProtocolHarness,
    scenario: ScenarioId,
) -> Result<ScenarioObservation> {
    match scenario {
        ScenarioId::LifecyclePassengerCleanup => lifecycle_passenger_cleanup(harness).await,
        ScenarioId::MetadataDirtyDefault => metadata_dirty_default(harness).await,
        ScenarioId::AttributesEquipmentEffects => attributes_equipment_effects(harness).await,
        ScenarioId::CollisionStep => collision_step(harness).await,
        ScenarioId::DamageDeath => damage_death(harness).await,
        ScenarioId::PassiveAiSchedule => passive_ai_schedule(harness).await,
    }
}

async fn lifecycle_passenger_cleanup(
    harness: &mut EntityProtocolHarness,
) -> Result<ScenarioObservation> {
    let mut observation = ScenarioObservation::new(ScenarioId::LifecyclePassengerCleanup);
    let mut aliases = harness.aliases()?;
    let anchor = harness.anchor();
    let subject_position = [anchor[0], anchor[1], anchor[2] + 1.0];
    let summon = harness
        .summon(
            &mut aliases,
            &mut observation,
            "subject",
            "minecraft:pig",
            subject_position,
        )
        .await?;

    let mut exit_frames = summon.intervening_frames;
    exit_frames.extend(
        harness
            .teleport([anchor[0] + 512.0, anchor[1], anchor[2]])
            .await?,
    );
    let lifecycle_frames = exit_frames
        .iter()
        .filter(|frame| matches!(frame.id, ClientboundSetPassengers::ID | RemoveEntities::ID))
        .cloned()
        .collect::<Vec<_>>();
    let mut exit_facts = harness.normalize_frames(&lifecycle_frames, &aliases, "tracking-exit")?;
    if !has_removed(&exit_facts, "subject") {
        exit_facts.extend(
            harness
                .observe_until_matching(
                    &aliases,
                    "tracking-exit",
                    "tracked entity removal",
                    |packet_id| {
                        matches!(packet_id, ClientboundSetPassengers::ID | RemoveEntities::ID)
                    },
                    |facts| has_removed(facts, "subject"),
                )
                .await?,
        );
    }
    observation.extend(relevant_lifecycle_facts(exit_facts));
    let _ = harness.teleport(anchor).await?;

    observation.degrade(PASSENGER_CONTROL_BLOCKER);
    Ok(observation)
}

async fn metadata_dirty_default(
    harness: &mut EntityProtocolHarness,
) -> Result<ScenarioObservation> {
    let mut observation = ScenarioObservation::new(ScenarioId::MetadataDirtyDefault);
    let mut aliases = harness.aliases()?;
    let anchor = harness.anchor();
    if harness.kind() == ServerKind::Solaris {
        harness.give_hotbar_zero("minecraft:shears").await?;
    }
    let default_summon = harness
        .summon(
            &mut aliases,
            &mut observation,
            "default-subject",
            "minecraft:sheep",
            [anchor[0], anchor[1], anchor[2] + 1.0],
        )
        .await?;

    let mut default_frames = default_summon.intervening_frames;
    default_frames.extend(
        harness
            .protocol_fence("default metadata publication fence")
            .await?,
    );
    let default_facts = normalize_metadata_frames(
        harness,
        &default_frames,
        &aliases,
        "default",
        &mut observation,
    )?;
    observation.extend(default_metadata_evidence(default_facts, "default-subject"));

    let dirty_position = [anchor[0] + 1.0, anchor[1], anchor[2] + 1.0];
    let mutation_frames = match harness.kind() {
        ServerKind::Solaris => {
            let dirty_summon = harness
                .summon(
                    &mut aliases,
                    &mut observation,
                    "subject",
                    "minecraft:sheep",
                    dirty_position,
                )
                .await?;
            let mut setup_frames = dirty_summon.intervening_frames;
            setup_frames.extend(
                harness
                    .protocol_fence("dirty sheep default metadata drain")
                    .await?,
            );
            let _ = normalize_metadata_frames(
                harness,
                &setup_frames,
                &aliases,
                "dirty-setup",
                &mut observation,
            )?;
            harness.interact(dirty_summon.runtime_entity_id).await?;
            harness
                .protocol_fence("sheep shear publication fence")
                .await?
        }
        ServerKind::Vanilla => {
            let dirty_summon = harness
                .summon_vanilla_nbt(
                    &mut aliases,
                    &mut observation,
                    "subject",
                    "minecraft:sheep",
                    dirty_position,
                    "{Sheared:1b}",
                )
                .await?;
            let mut frames = dirty_summon.intervening_frames;
            frames.extend(
                harness
                    .protocol_fence("pre-sheared sheep publication fence")
                    .await?,
            );
            frames
        }
    };
    let mut dirty_facts = normalize_metadata_frames(
        harness,
        &mutation_frames,
        &aliases,
        "dirty",
        &mut observation,
    )?;
    if !has_metadata_value(
        &dirty_facts,
        "dirty",
        "subject",
        SHEEP_ENTITY_DATA_WOOL_INDEX,
        "byte:16",
    ) {
        dirty_facts.extend(
            harness
                .observe_until(
                    &aliases,
                    "dirty",
                    "exact sheep shear metadata publication",
                    |facts| {
                        has_metadata_value(
                            facts,
                            "dirty",
                            "subject",
                            SHEEP_ENTITY_DATA_WOOL_INDEX,
                            "byte:16",
                        )
                    },
                )
                .await?,
        );
    }
    ensure!(
        has_metadata_value(
            &dirty_facts,
            "dirty",
            "subject",
            SHEEP_ENTITY_DATA_WOOL_INDEX,
            "byte:16",
        ),
        "exact sheep shear feedback window contained no wool metadata index {} value byte:16",
        SHEEP_ENTITY_DATA_WOOL_INDEX
    );
    observation.extend(metadata_facts(dirty_facts, "dirty", "subject"));
    Ok(observation)
}

fn normalize_metadata_frames(
    harness: &EntityProtocolHarness,
    frames: &[RawFrame],
    aliases: &EntityAliases,
    phase: &str,
    observation: &mut ScenarioObservation,
) -> Result<Vec<EntityFact>> {
    match harness.normalize_frames(frames, aliases, phase) {
        Ok(facts) => Ok(facts),
        Err(error) if harness.kind() == ServerKind::Vanilla => {
            observation.degrade(format!(
                "vanilla {phase} metadata is outside the local typed serializer coverage: {error}"
            ));
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}

async fn attributes_equipment_effects(
    harness: &mut EntityProtocolHarness,
) -> Result<ScenarioObservation> {
    let mut observation = ScenarioObservation::new(ScenarioId::AttributesEquipmentEffects);
    let mut aliases = harness.aliases()?;
    let anchor = harness.anchor();
    let summon = harness
        .summon(
            &mut aliases,
            &mut observation,
            "subject",
            "minecraft:zombie",
            [anchor[0] + 1.0, anchor[1], anchor[2]],
        )
        .await?;
    let mut frames = summon.intervening_frames;
    frames.extend(
        harness
            .protocol_fence("entity state publication fence")
            .await?,
    );
    let state_frames = frames
        .iter()
        .filter(|frame| {
            matches!(
                frame.id,
                CLIENTBOUND_UPDATE_ATTRIBUTES_ID
                    | CLIENTBOUND_SET_EQUIPMENT_ID
                    | CLIENTBOUND_UPDATE_MOB_EFFECT_ID
                    | CLIENTBOUND_REMOVE_MOB_EFFECT_ID
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    observation.extend(harness.normalize_frames(&state_frames, &aliases, "spawn")?);

    for (kind, label) in [
        (EntityStatePacket::Attributes, "attributes"),
        (EntityStatePacket::Equipment, "equipment"),
        (EntityStatePacket::EffectUpdated, "effects"),
    ] {
        if !observation.facts().iter().any(|fact| {
            matches!(fact, EntityFact::PacketPayload { kind: actual, .. } if *actual == kind)
        }) {
            observation.degrade(format!(
                "{} did not publish {label} in the protocol-fenced spawn window; no shared mutation command is available",
                server_label(harness.kind())
            ));
        }
    }
    Ok(observation)
}

async fn collision_step(harness: &mut EntityProtocolHarness) -> Result<ScenarioObservation> {
    let mut observation = ScenarioObservation::new(ScenarioId::CollisionStep);
    if !prepare_collision_fixture(harness).await? {
        observation.degrade(format!(
            "{} collision fixture is unavailable",
            server_label(harness.kind())
        ));
        return Ok(observation);
    }

    let full_block_start = [3.5, 202.0, -2.5];
    let _ = harness.teleport(full_block_start).await?;
    let full_block_flags = MovePlayerFlags::new(false, true);
    let full_block_target = [4.5, 202.0, -2.5];
    let mut full_block_frames = harness
        .move_and_fence(full_block_target, full_block_flags)
        .await?;
    if !full_block_frames
        .iter()
        .any(|frame| frame.id == SynchronizePlayerPosition::ID)
    {
        full_block_frames.push(
            harness
                .wait_for_position_correction("full-block movement correction")
                .await?,
        );
    }
    let full_block = collision_fact(
        "full-block",
        full_block_target,
        full_block_flags,
        &full_block_frames,
    )?;
    ensure!(
        matches!(
            full_block,
            EntityFact::Collision {
                corrected: true,
                ..
            }
        ),
        "full-block collision produced no positive correction before the command feedback fence; captured packet ids: {:?}",
        full_block_frames
            .iter()
            .map(|frame| frame.id)
            .collect::<Vec<_>>()
    );
    observation.push(full_block);

    let half_step_start = [5.5, 200.0, -2.5];
    let _ = harness.teleport(half_step_start).await?;
    let _ = harness
        .move_and_fence(half_step_start, MovePlayerFlags::new(true, false))
        .await?;
    let half_step_flags = MovePlayerFlags::new(true, true);
    // Standing exactly at y=200.5 is valid on a bottom slab. Move slightly
    // into the shape so this remains a positive-correction oracle case.
    let half_step_target = [6.5, 200.49, -2.5];
    let half_step_frames = harness
        .move_and_fence(half_step_target, half_step_flags)
        .await?;
    observation.push(collision_fact(
        "half-step",
        half_step_target,
        half_step_flags,
        &half_step_frames,
    )?);
    let _ = harness.teleport(harness.anchor()).await?;
    Ok(observation)
}

async fn prepare_collision_fixture(harness: &mut EntityProtocolHarness) -> Result<bool> {
    if harness.collision_fixture_available() {
        return Ok(true);
    }
    if harness.kind() != ServerKind::Vanilla {
        return Ok(false);
    }
    let _ = harness
        .vanilla_command_fence(
            "gamerule player_movement_check true",
            "commands.gamerule.set",
            "vanilla collision movement-check control",
        )
        .await?;
    for (command, success_key) in [
        (
            "fill -8 199 -4 12 199 4 minecraft:stone",
            "commands.fill.success",
        ),
        (
            "setblock 4 202 -3 minecraft:stone",
            "commands.setblock.success",
        ),
        (
            "setblock 6 200 -3 minecraft:stone_slab[type=bottom,waterlogged=false]",
            "commands.setblock.success",
        ),
    ] {
        let _ = harness
            .vanilla_command_fence(command, success_key, "vanilla collision fixture command")
            .await?;
    }
    Ok(true)
}

fn collision_fact(
    case: &str,
    requested_position: [f64; 3],
    flags: MovePlayerFlags,
    frames: &[RawFrame],
) -> Result<EntityFact> {
    let mut correction = None;
    for frame in frames {
        if frame.id != SynchronizePlayerPosition::ID {
            continue;
        }
        ensure!(
            correction.is_none(),
            "collision case {case} produced multiple correction packets"
        );
        let mut body = frame.body.clone();
        let sync = SynchronizePlayerPosition::decode(&mut body)?;
        ensure!(
            body.is_empty(),
            "collision correction packet has trailing bytes"
        );
        correction = Some(sync);
    }
    Ok(collision_fact_from_correction(
        case,
        requested_position,
        flags,
        correction,
    ))
}

fn collision_fact_from_correction(
    case: &str,
    requested_position: [f64; 3],
    flags: MovePlayerFlags,
    correction: Option<SynchronizePlayerPosition>,
) -> EntityFact {
    let corrected = correction.is_some();
    let position = correction.map_or(requested_position, |sync| [sync.x, sync.y, sync.z]);
    EntityFact::Collision {
        case: case.to_owned(),
        position: MilliblockPosition::relative(position, [0.0, 200.0, -3.0]),
        corrected,
        on_ground: flags.on_ground,
        horizontal_collision: flags.horizontal_collision,
    }
}

async fn damage_death(harness: &mut EntityProtocolHarness) -> Result<ScenarioObservation> {
    let mut observation = ScenarioObservation::new(ScenarioId::DamageDeath);
    let mut aliases = harness.aliases()?;
    let anchor = harness.anchor();
    let _ = harness.teleport(anchor).await?;
    harness.give_hotbar_zero("minecraft:diamond_sword").await?;
    let summon = harness
        .summon(
            &mut aliases,
            &mut observation,
            "subject",
            "minecraft:chicken",
            [anchor[0], anchor[1], anchor[2] + 1.0],
        )
        .await?;
    let _ = harness
        .protocol_fence("chicken spawn publication fence")
        .await?;
    harness.attack(summon.runtime_entity_id).await?;
    let attack_frames = harness
        .protocol_fence("post-attack publication fence")
        .await?;
    let relevant_frames = attack_frames
        .iter()
        .filter(|frame| {
            matches!(
                frame.id,
                CLIENTBOUND_DAMAGE_EVENT_ID | EntityEvent::ID | RemoveEntities::ID
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let facts = harness
        .normalize_frames(&relevant_frames, &aliases, "attack")?
        .into_iter()
        .filter(|fact| {
            matches!(
                fact,
                EntityFact::Damage { .. }
                    | EntityFact::StatusEvent { .. }
                    | EntityFact::Removed { .. }
            )
        })
        .collect::<Vec<_>>();
    let mut missing = Vec::new();
    if !has_damage(&facts, "subject") {
        missing.push("normalized damage packet");
    }
    if !has_status(&facts, "subject", 3) {
        missing.push("death status event 3");
    }
    if !has_removed(&facts, "subject") {
        missing.push("subject removal");
    }
    if !missing.is_empty() {
        observation.degrade(format!(
            "post-attack command-feedback fence completed without {}",
            missing.join(", ")
        ));
    }
    observation.extend(facts);
    Ok(observation)
}

async fn passive_ai_schedule(harness: &mut EntityProtocolHarness) -> Result<ScenarioObservation> {
    let mut observation = ScenarioObservation::new(ScenarioId::PassiveAiSchedule);
    let mut aliases = harness.aliases()?;
    let anchor = harness.anchor();
    let summon = harness
        .summon(
            &mut aliases,
            &mut observation,
            "subject",
            "minecraft:villager",
            [anchor[0] - 1.0, anchor[1], anchor[2]],
        )
        .await?;
    let mut frames = summon.intervening_frames;
    frames.extend(harness.protocol_fence("passive entity spawn fence").await?);
    let event_frames = frames
        .iter()
        .filter(|frame| frame.id == EntityEvent::ID)
        .cloned()
        .collect::<Vec<_>>();
    observation.extend(harness.normalize_frames(&event_frames, &aliases, "passive-ai-schedule")?);
    observation.degrade(PASSIVE_SCHEDULE_BLOCKER);
    Ok(observation)
}

fn metadata_facts(
    facts: Vec<EntityFact>,
    phase: &str,
    entity: &str,
) -> impl Iterator<Item = EntityFact> {
    let phase = phase.to_owned();
    let entity = entity.to_owned();
    facts.into_iter().filter(move |fact| {
        matches!(
            fact,
            EntityFact::Metadata {
                phase: actual_phase,
                entity: actual_entity,
                ..
            } if actual_phase == &phase && actual_entity == &entity
        )
    })
}

fn default_metadata_evidence(facts: Vec<EntityFact>, entity: &str) -> Vec<EntityFact> {
    let metadata = metadata_facts(facts, "default", entity).collect::<Vec<_>>();
    if metadata.is_empty() {
        vec![EntityFact::MetadataOmitted {
            phase: "default".into(),
            entity: entity.into(),
        }]
    } else {
        metadata
    }
}

fn relevant_lifecycle_facts(facts: Vec<EntityFact>) -> impl Iterator<Item = EntityFact> {
    facts.into_iter().filter(|fact| {
        matches!(
            fact,
            EntityFact::Passengers { .. } | EntityFact::Removed { .. }
        )
    })
}

fn has_metadata_value(
    facts: &[EntityFact],
    phase: &str,
    entity: &str,
    index: u8,
    expected_value: &str,
) -> bool {
    facts.iter().any(|fact| {
        matches!(
            fact,
            EntityFact::Metadata {
                phase: actual_phase,
                entity: actual_entity,
                values,
            } if actual_phase == phase
                && actual_entity == entity
                && values
                    .iter()
                    .any(|value| value.index == index && value.value == expected_value)
        )
    })
}

fn has_removed(facts: &[EntityFact], entity: &str) -> bool {
    facts
        .iter()
        .any(|fact| matches!(fact, EntityFact::Removed { entity: actual } if actual == entity))
}

fn has_status(facts: &[EntityFact], entity: &str, event_id: i8) -> bool {
    facts.iter().any(|fact| {
        matches!(
            fact,
            EntityFact::StatusEvent {
                entity: actual,
                event_id: actual_event,
            } if actual == entity && *actual_event == event_id
        )
    })
}

fn has_damage(facts: &[EntityFact], entity: &str) -> bool {
    facts
        .iter()
        .any(|fact| matches!(fact, EntityFact::Damage { entity: actual, .. } if actual == entity))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn catalog_contains_the_six_short_w07_entity_scenarios() {
        let catalog = scenario_catalog();
        let slugs = catalog.iter().map(|spec| spec.slug).collect::<Vec<_>>();

        assert_eq!(
            slugs,
            vec![
                "metadata-dirty-default",
                "damage-death",
                "attributes-equipment-effects",
                "passive-ai-schedule",
                "collision-step",
                "lifecycle-passenger-cleanup",
            ]
        );
        assert_eq!(
            catalog
                .iter()
                .map(|spec| spec.id)
                .collect::<BTreeSet<_>>()
                .len(),
            catalog.len()
        );
        assert!(
            catalog
                .iter()
                .all(|spec| spec.failure_timeout <= Duration::from_secs(8))
        );
    }

    #[test]
    fn collision_fact_normalizes_correction_coordinates_and_movement_flags() {
        let fact = collision_fact_from_correction(
            "full-block",
            [4.5, 200.0, -2.5],
            MovePlayerFlags::new(true, true),
            Some(SynchronizePlayerPosition {
                teleport_id: 7,
                x: 3.25,
                y: 200.5,
                z: -2.0,
                dx: 0.0,
                dy: 0.0,
                dz: 0.0,
                yaw: 90.0,
                pitch: 0.0,
                relative_flags: 0,
            }),
        );

        assert_eq!(
            fact,
            EntityFact::Collision {
                case: "full-block".into(),
                position: MilliblockPosition {
                    x: 3_250,
                    y: 500,
                    z: 1_000,
                },
                corrected: true,
                on_ground: true,
                horizontal_collision: true,
            }
        );
    }

    #[test]
    fn dirty_shearing_requires_wool_byte_sixteen() {
        let metadata = |value: &str| {
            vec![EntityFact::Metadata {
                phase: "dirty".into(),
                entity: "subject".into(),
                values: vec![super::super::model::MetadataEntry {
                    index: SHEEP_ENTITY_DATA_WOOL_INDEX,
                    value: value.into(),
                }],
            }]
        };

        assert!(!has_metadata_value(
            &metadata("byte:0"),
            "dirty",
            "subject",
            SHEEP_ENTITY_DATA_WOOL_INDEX,
            "byte:16",
        ));
        assert!(has_metadata_value(
            &metadata("byte:16"),
            "dirty",
            "subject",
            SHEEP_ENTITY_DATA_WOOL_INDEX,
            "byte:16",
        ));
    }

    #[test]
    fn exact_summon_feedback_fence_proves_default_metadata_omission() {
        assert_eq!(
            default_metadata_evidence(Vec::new(), "subject"),
            vec![EntityFact::MetadataOmitted {
                phase: "default".into(),
                entity: "subject".into(),
            }]
        );
    }

    #[test]
    fn observed_default_metadata_is_not_replaced_with_an_omission_fact() {
        let metadata = EntityFact::Metadata {
            phase: "default".into(),
            entity: "subject".into(),
            values: vec![super::super::model::MetadataEntry {
                index: 0,
                value: "byte:0".into(),
            }],
        };

        assert_eq!(
            default_metadata_evidence(vec![metadata.clone()], "subject"),
            vec![metadata]
        );
    }

    #[test]
    fn unsupported_wire_controls_remain_explicit_production_blockers() {
        assert!(PASSENGER_CONTROL_BLOCKER.contains("no wire command or interaction path"));
        assert!(PASSIVE_SCHEDULE_BLOCKER.contains("equivalent persisted villager brain/POI"));
    }

    #[test]
    fn degraded_rows_retain_any_protocol_facts_the_runner_did_observe() {
        let mut observation = ScenarioObservation::new(ScenarioId::PassiveAiSchedule);
        observation.push(EntityFact::ScheduleEvent {
            entity: "subject".into(),
            event_id: 10,
        });
        observation.degrade("schedule control unavailable");

        assert!(matches!(
            observation.evidence(),
            EvidenceState::Degraded { .. }
        ));
        assert_eq!(observation.facts().len(), 1);
    }
}
