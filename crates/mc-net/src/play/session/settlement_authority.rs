use mc_data::items::ItemRegistry;
use mc_entity::{EntitySnapshot, SpawnEntity, Vec3};

use crate::play::SettlementInhabitantSpawn;
use crate::play::simulation::SimulationAuthority;

use super::entity_lifecycle::track_entity_chunk_locked;
use super::interaction_geometry::entity_aabb;
use super::outbound::VisibilityDispatch;
use super::visibility::{
    initialize_entity_wire_state_from_snapshot_locked,
    install_committed_entity_publications_locked, server_entity_snapshot_from,
};
use super::{SessionRegistry, SessionRegistryInner, apply_entity_facts};

impl SessionRegistry {
    pub(in crate::play) fn ensure_settlement_inhabitants(
        &self,
        _authority: &SimulationAuthority,
        spawns: &[SettlementInhabitantSpawn],
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("ensure settlement inhabitants");
        let pending = spawns
            .iter()
            .filter(|spawn| !inner.settlement_spawn_claims.contains(&spawn.claim))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Vec::new();
        }

        let lifecycle_tick = inner.entity_lifecycle_tick;
        let day_time = i64::try_from(self.world_time()).unwrap_or(i64::MAX);
        let profile = mc_entity::villager_26_1_2::VillagerBrainProfile::vanilla_26_1_2();
        let candidates = pending
            .iter()
            .map(|spawn| settlement_candidate(spawn, lifecycle_tick, day_time, &profile))
            .collect::<Vec<_>>();
        let committed = inner.entities.spawn_unique_batch(candidates);
        for spawn in pending {
            inner.settlement_spawn_claims.insert(spawn.claim.clone());
        }
        install_settlement_inhabitants_locked(&mut inner, committed)
    }
}

/// Resolve one chunk inhabitant marker into the villager the runtime spawns.
///
/// The marker carries the data's own names (`villager_kind` is the
/// `VillagerData.type` path, `profession` the profession path); this is the one
/// place they become the core's enums, and the one place the entity type is
/// resolved against the entity registry. A marker naming something this build
/// does not know — an entity type outside the registry, or a kind or profession
/// the enums do not carry — is dropped rather than approximated.
pub(in crate::play) fn settlement_inhabitant_spawn(
    marker: mc_world::SettlementInhabitantMarker,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    items: &ItemRegistry,
) -> Option<SettlementInhabitantSpawn> {
    let entity_type = mc_data::Identifier::parse(marker.entity_type.clone()).ok()?;
    let entity_type_id = i32::try_from(entity_types.id_of(&entity_type)?).ok()?;
    let villager_kind = match marker.villager_kind.as_str() {
        "desert" => mc_entity::VillagerKind::Desert,
        "plains" => mc_entity::VillagerKind::Plains,
        "savanna" => mc_entity::VillagerKind::Savanna,
        "snow" => mc_entity::VillagerKind::Snow,
        "taiga" => mc_entity::VillagerKind::Taiga,
        _ => return None,
    };
    let profession = match marker.profession.as_str() {
        "none" => mc_entity::VillagerProfession::None,
        "nitwit" => mc_entity::VillagerProfession::Nitwit,
        "toolsmith" => mc_entity::VillagerProfession::Toolsmith,
        _ => return None,
    };
    let to_vec3 = |position: Option<[f64; 3]>| position.map(|[x, y, z]| Vec3::new(x, y, z));
    let pois = mc_entity::villager_26_1_2::VillagerPoiSet {
        home: to_vec3(marker.home),
        job_site: to_vec3(marker.job_site),
        meeting_point: to_vec3(marker.meeting_point),
    };
    // A negative `Age` is a baby, exactly as `Entity.isBaby` reads it, and it
    // selects the baby schedule for the brain as well.
    let villager_brain = if marker.age < 0 {
        mc_entity::villager_26_1_2::VillagerBrainState::baby(pois)
    } else {
        mc_entity::villager_26_1_2::VillagerBrainState::adult(pois)
    };
    Some(SettlementInhabitantSpawn {
        claim: marker.claim,
        entity_type_id,
        entity_type_name: entity_type.to_string(),
        position: Vec3::new(marker.position[0], marker.position[1], marker.position[2]),
        yaw: marker.yaw,
        pitch: marker.pitch,
        age: marker.age,
        villager: mc_entity::VillagerData::new(villager_kind, profession, marker.level),
        villager_brain,
        villager_merchant: (profession == mc_entity::VillagerProfession::Toolsmith)
            .then(|| toolsmith_merchant_state(items))
            .flatten(),
    })
}

fn settlement_candidate(
    spawn: &SettlementInhabitantSpawn,
    lifecycle_tick: u64,
    day_time: i64,
    profile: &mc_entity::villager_26_1_2::VillagerBrainProfile,
) -> SpawnEntity {
    let mut entity = SpawnEntity::new(
        spawn.entity_type_id,
        spawn.entity_type_name.clone(),
        spawn.position,
    );
    entity.uuid = Some(crate::settlement_identity::settlement_entity_uuid(
        &spawn.claim,
    ));
    // `StructureTemplate.placeEntities` snaps the entity to the yaw its
    // placement computed and to the pitch its own NBT carried
    // (`entity.getXRot()`, clamped the way `Entity.setXRot` clamps it). A marker
    // from the settlement plan lane carries neither, and lands on the spawn's
    // zero rotation — which is what vanilla's `StructurePlaceSettings.getRotation`
    // does for a plan that authors no rotation.
    entity.rotation = mc_entity::Rotation {
        yaw: spawn.yaw,
        pitch: entity_pitch(spawn.pitch),
        head_yaw: spawn.yaw,
    };
    entity.retained.spawn_tick = lifecycle_tick;
    entity.retained.villager = Some(spawn.villager);
    entity.retained.villager_population = Some(if spawn.age < 0 {
        mc_entity::villager_population_26_1_2::VillagerPopulationState::village_baby(
            spawn.age,
            spawn.claim.clone(),
        )
    } else {
        mc_entity::villager_population_26_1_2::VillagerPopulationState::adult()
    });
    entity.retained.villager_brain = Some(spawn.villager_brain.clone());
    entity.retained.villager_merchant = spawn.villager_merchant.clone();
    apply_entity_facts(&mut entity);
    let plan = mc_entity::villager_26_1_2::plan_villager_brain(
        &spawn.villager_brain,
        profile,
        lifecycle_tick,
        day_time,
    )
    .expect("settlement markers construct a validated villager brain");
    entity.retained.villager_brain = Some(plan.state);
    entity.goal = plan.goal;
    entity
}

pub(in crate::play) fn toolsmith_merchant_state(
    items: &ItemRegistry,
) -> Option<mc_entity::villager_merchant_26_1_2::VillagerMerchantState> {
    use mc_entity::villager_merchant_26_1_2::{
        VillagerMerchantState, VillagerTradeCost, VillagerTradeOffer,
    };

    let offers = mc_data::villager_trades_26_1_2::toolsmith_novice_offers_26_1_2()
        .into_iter()
        .map(|spec| {
            let cost_a = VillagerTradeCost::new(items.id_of(&spec.cost_a.item)?, spec.cost_a.count);
            let result =
                mc_entity::EntityItemStack::new(items.id_of(&spec.result_item)?, spec.result_count);
            let mut offer = VillagerTradeOffer::new(
                cost_a,
                result,
                spec.max_uses,
                spec.xp,
                spec.price_multiplier,
            );
            offer.cost_b = match spec.cost_b {
                Some(cost) => Some(VillagerTradeCost::new(items.id_of(&cost.item)?, cost.count)),
                None => None,
            };
            Some(offer)
        })
        .collect::<Option<Vec<_>>>()?;
    VillagerMerchantState::new(offers).ok()
}

fn install_settlement_inhabitants_locked(
    inner: &mut SessionRegistryInner,
    committed: Vec<EntitySnapshot>,
) -> Vec<VisibilityDispatch> {
    let mut snapshots = Vec::with_capacity(committed.len());
    for entity in committed {
        let aabb = entity_aabb(&entity.type_name);
        let snapshot = server_entity_snapshot_from(entity);
        inner
            .entity_type_aabbs
            .entry(snapshot.type_id)
            .or_insert(aabb);
        track_entity_chunk_locked(inner, snapshot.id, snapshot.position);
        initialize_entity_wire_state_from_snapshot_locked(inner, &snapshot);
        snapshots.push(snapshot);
    }
    install_committed_entity_publications_locked(inner, snapshots)
}

/// `Entity.setXRot`: `Math.clamp(xRot % 360, -90, 90)`, which is what the
/// authored template pitch becomes when the entity loads its own NBT. Java's
/// `%` is the truncating remainder, and Rust's `%` on `f32` is the same.
fn entity_pitch(pitch: f32) -> f32 {
    if !pitch.is_finite() {
        return 0.0;
    }
    (pitch % 360.0).clamp(-90.0, 90.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::persistence::{
        PersistedEntityCheckpoint, load_persisted_entities, save_persisted_entity_records,
    };
    use crate::play::simulation::SimulationAuthority;
    use mc_entity::{Vec3, VillagerData, VillagerKind, VillagerProfession};

    fn spawn() -> SettlementInhabitantSpawn {
        SettlementInhabitantSpawn {
            claim: "owner:smith@72,8".to_owned(),
            entity_type_id: 99,
            entity_type_name: "minecraft:villager".to_owned(),
            position: Vec3::new(72.5, 66.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            age: 0,
            villager: VillagerData::new(VillagerKind::Plains, VillagerProfession::Toolsmith, 1),
            villager_brain: mc_entity::villager_26_1_2::VillagerBrainState::adult(
                mc_entity::villager_26_1_2::VillagerPoiSet {
                    home: Some(Vec3::new(72.5, 66.0, 8.5)),
                    job_site: Some(Vec3::new(73.5, 66.0, 8.5)),
                    meeting_point: Some(Vec3::new(72.5, 65.0, 8.5)),
                },
            ),
            villager_merchant: toolsmith_merchant_state(&mc_data::items::solaris_required_items()),
        }
    }

    /// A generated villager is adopted through the identity its placement mints.
    ///
    /// This is the chain a settlement owner runs on a real village: the spawn
    /// lane turns a chunk's inhabitant marker into a villager whose UUID comes
    /// from the placement's claim, the site descriptor publishes that same UUID,
    /// and `claim_resident` adopts the live entity by it. Repeating the claim
    /// and reopening the storage must leave one resident, not two — the failure
    /// `ACC-03` and `REC-01` are about.
    #[tokio::test]
    async fn a_generated_inhabitant_is_adopted_by_the_identity_its_placement_mints() {
        use tokio::sync::mpsc;

        use crate::login::{LoggedInProfile, offline_uuid};
        use crate::play::PlayerPose;
        use crate::script::storage::PluginStorage;
        use crate::script::storage::world_inventory::InventoryRuntime;
        use mc_script::{ScriptOperation, ScriptOperationRequest, ScriptResidentOperation};

        const OWNER: &str = "settlement-authority-test";

        let registry = std::sync::Arc::new(SessionRegistry::new());
        let authority = SimulationAuthority::for_test();
        // The session's channel stays alive for the whole test: a closed channel
        // is a disconnected actor, and a claim from one is refused.
        let (tx, _rx) = mpsc::channel(4);
        let (actor, _) = registry.register(
            &LoggedInProfile {
                uuid: offline_uuid("Adopter"),
                name: "Adopter".to_owned(),
            },
            (4, 5),
            2,
            std::collections::HashSet::new(),
            tx,
            PlayerPose::new(72.5, 66.0, 8.5),
        );

        // The generator's own lane places the villager, and the generated UUID
        // is what the site descriptor publishes.
        let spawn = spawn();
        let uuid = crate::settlement_identity::settlement_entity_uuid(&spawn.claim).to_string();
        registry.ensure_settlement_inhabitants(&authority, std::slice::from_ref(&spawn));

        let root = tempfile::tempdir().unwrap();
        let mut storage = PluginStorage::open(root.path()).unwrap();
        let runtime = InventoryRuntime::player_only_for_test(
            root.path(),
            std::sync::Arc::clone(&registry),
            std::sync::Arc::new(mc_data::items::solaris_required_items()),
            std::sync::Arc::new(mc_data::item_components::solaris_required_item_facts()),
        );

        let claim = |operation_id: &str| {
            ScriptOperationRequest::try_new(
                "adopt",
                ScriptOperation::Resident {
                    operation: ScriptResidentOperation::Claim {
                        operation_id: operation_id.to_owned(),
                        actor_id: actor,
                        entity_uuid: uuid.clone(),
                        expected_entity_revision: 0,
                    },
                },
            )
            .unwrap()
        };

        let adopted = runtime
            .execute_resident_operation(&mut storage, OWNER, &claim("adopt-village"))
            .await
            .expect("the claim reaches the durable boundary");
        assert_eq!(
            adopted.failure(),
            None,
            "a live generated villager is adoptable"
        );
        let handle = resident_handle_of(&adopted).to_owned();
        assert!(!handle.is_empty());

        // The same operation replays its committed outcome instead of adopting a
        // second time, and the ledger holds exactly one resident.
        let replayed = runtime
            .execute_resident_operation(&mut storage, OWNER, &claim("adopt-village"))
            .await
            .expect("the replay reaches the durable boundary");
        assert_eq!(replayed.failure(), None);
        assert_eq!(
            resident_handle_of(&replayed),
            handle,
            "one operation adopts one resident"
        );

        // A restart keeps the same single resident: the identity is durable, so
        // re-reading the village after a restart cannot mint a second one.
        drop(storage);
        let mut reopened = PluginStorage::open(root.path()).unwrap();
        let listed = runtime
            .execute_resident_operation(
                &mut reopened,
                OWNER,
                &ScriptOperationRequest::try_new(
                    "list",
                    ScriptOperation::Resident {
                        operation: ScriptResidentOperation::Query {
                            handles: vec![handle.clone()],
                            cursor: None,
                        },
                    },
                )
                .unwrap(),
            )
            .await
            .expect("the query reaches the durable boundary");
        assert_eq!(listed.failure(), None);
        assert_eq!(
            resident_handles_of(&listed),
            vec![handle],
            "the adopted resident survives a restart exactly once"
        );
    }

    /// The one resident handle an outcome reports.
    fn resident_handle_of(outcome: &mc_script::ScriptOperationOutcome) -> &str {
        match outcome.payload() {
            mc_script::ScriptOperationPayload::Resident { result } => match result.as_ref() {
                mc_script::ScriptResidentResult::Snapshot { resident } => resident.handle.as_str(),
                other => panic!("expected one resident, got {other:?}"),
            },
            other => panic!("expected a resident payload, got {other:?}"),
        }
    }

    /// Every resident handle a query outcome reports.
    fn resident_handles_of(outcome: &mc_script::ScriptOperationOutcome) -> Vec<String> {
        match outcome.payload() {
            mc_script::ScriptOperationPayload::Resident { result } => match result.as_ref() {
                mc_script::ScriptResidentResult::Page { residents, .. } => residents
                    .iter()
                    .map(|resident| resident.handle.clone())
                    .collect(),
                other => panic!("expected a resident page, got {other:?}"),
            },
            other => panic!("expected a resident payload, got {other:?}"),
        }
    }

    #[test]
    fn settlement_command_is_idempotent_and_keeps_villager_job_state() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();

        registry.ensure_settlement_inhabitants(&authority, &[spawn()]);
        registry.ensure_settlement_inhabitants(&authority, &[spawn()]);

        let records = registry.persisted_entity_records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].snapshot.retained.villager,
            Some(VillagerData::new(
                VillagerKind::Plains,
                VillagerProfession::Toolsmith,
                1,
            ))
        );
        let brain = records[0]
            .snapshot
            .retained
            .villager_brain
            .as_ref()
            .expect("settlement villager brain persists");
        assert_eq!(
            brain.activity,
            mc_entity::villager_26_1_2::VillagerActivity::Rest
        );
        assert_eq!(
            records[0].snapshot.goal,
            mc_entity::GoalState::FollowPosition {
                target: Vec3::new(72.5, 66.0, 8.5),
                speed: 3.0,
            }
        );
        let merchant = records[0]
            .snapshot
            .retained
            .villager_merchant
            .as_ref()
            .expect("toolsmith merchant state persists");
        assert_eq!(merchant.offers.len(), 5);
        assert_eq!(merchant.offers[0].cost_a.count, 15);
        assert_eq!(merchant.offers[0].max_uses, 16);
    }

    #[test]
    fn restored_claim_prevents_respawn_after_inhabitant_is_absent() {
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let mut checkpoint = PersistedEntityCheckpoint::new(0, Vec::<EntitySnapshot>::new());
        checkpoint
            .settlement_claims
            .insert("owner:smith@72,8".to_owned());
        assert_eq!(registry.restore_persisted_entities(checkpoint), 0);

        registry.ensure_settlement_inhabitants(&authority, &[spawn()]);

        assert!(registry.persisted_entity_records().is_empty());
    }

    /// The marker names become the core's enums here: every village type the
    /// templates author, the two professions they author, and the entity type's
    /// registry id. A name this build does not carry drops the marker rather
    /// than approximating it.
    #[test]
    fn inhabitant_markers_resolve_into_village_villagers() {
        let entity_types = mc_data::entity_types::solaris_required_entity_types();
        let items = mc_data::items::solaris_required_items();
        let marker =
            |kind: &str, profession: &str, age: i32| mc_world::SettlementInhabitantMarker {
                claim: format!("test:village/{kind}/villagers@{age}"),
                entity_type: "minecraft:villager".to_owned(),
                position: [72.5, 66.0, 8.5],
                villager_kind: kind.to_owned(),
                profession: profession.to_owned(),
                level: 1,
                home: None,
                job_site: None,
                meeting_point: None,
                age,
                yaw: 48.821_632,
                pitch: -25.827_711,
            };

        let cases = [
            (
                "desert",
                "none",
                VillagerKind::Desert,
                VillagerProfession::None,
            ),
            (
                "plains",
                "nitwit",
                VillagerKind::Plains,
                VillagerProfession::Nitwit,
            ),
            (
                "savanna",
                "none",
                VillagerKind::Savanna,
                VillagerProfession::None,
            ),
            ("snow", "none", VillagerKind::Snow, VillagerProfession::None),
            (
                "taiga",
                "none",
                VillagerKind::Taiga,
                VillagerProfession::None,
            ),
        ];
        for (kind, profession, expected_kind, expected_profession) in cases {
            let resolved =
                settlement_inhabitant_spawn(marker(kind, profession, 0), &entity_types, &items)
                    .unwrap_or_else(|| panic!("{kind}/{profession} must resolve"));
            assert_eq!(resolved.entity_type_name, "minecraft:villager");
            assert!(
                resolved.entity_type_id > 0,
                "the entity registry resolves the id"
            );
            assert_eq!(resolved.villager.kind, expected_kind);
            assert_eq!(resolved.villager.profession, expected_profession);
            assert_eq!(resolved.villager.level, 1);
            assert_eq!(resolved.age, 0);
            assert_eq!(resolved.yaw, 48.821_632);
            assert_eq!(resolved.pitch, -25.827_711);
            assert_eq!(resolved.position, Vec3::new(72.5, 66.0, 8.5));
            // The village templates author no POI, so the brain starts without
            // one and with the adult schedule.
            assert_eq!(
                resolved.villager_brain.schedule,
                mc_entity::villager_26_1_2::VillagerScheduleKind::Adult,
            );
            assert_eq!(
                resolved.villager_brain.pois,
                mc_entity::villager_26_1_2::VillagerPoiSet::default(),
            );
            assert!(resolved.villager_merchant.is_none());
        }

        // A baby: the authored `Age` selects the baby schedule.
        let baby =
            settlement_inhabitant_spawn(marker("plains", "none", -21_359), &entity_types, &items)
                .expect("a baby marker resolves");
        assert_eq!(baby.age, -21_359);
        assert_eq!(
            baby.villager_brain.schedule,
            mc_entity::villager_26_1_2::VillagerScheduleKind::Baby,
        );

        // Names outside the core's enums, and an entity type outside the
        // registry, are dropped.
        assert!(
            settlement_inhabitant_spawn(marker("jungle", "none", 0), &entity_types, &items)
                .is_none(),
            "a villager type the village templates never author is not approximated",
        );
        assert!(
            settlement_inhabitant_spawn(marker("plains", "farmer", 0), &entity_types, &items)
                .is_none(),
            "a profession the village templates never author is not approximated",
        );
        let mut unknown = marker("plains", "none", 0);
        unknown.entity_type = "test:not_registered".to_owned();
        assert!(settlement_inhabitant_spawn(unknown, &entity_types, &items).is_none());
    }

    /// A template-spawned baby is a baby, and it is spawned with the rotation its
    /// placement computed: the rotated authored yaw, and the authored pitch.
    #[test]
    fn a_baby_marker_spawns_a_baby_with_its_authored_rotation() {
        let entity_types = mc_data::entity_types::solaris_required_entity_types();
        let items = mc_data::items::solaris_required_items();
        let marker = mc_world::SettlementInhabitantMarker {
            claim: "minecraft:village/plains/villagers@40:64:8#0".to_owned(),
            entity_type: "minecraft:villager".to_owned(),
            position: [40.366_053, 65.0, 8.069_796],
            villager_kind: "plains".to_owned(),
            profession: "none".to_owned(),
            level: 1,
            home: None,
            job_site: None,
            meeting_point: None,
            age: -21_359,
            yaw: 0.0,
            pitch: -25.827_711,
        };
        let spawn = settlement_inhabitant_spawn(marker, &entity_types, &items)
            .expect("the village marker resolves");
        let candidate = settlement_candidate(
            &spawn,
            0,
            6_000,
            &mc_entity::villager_26_1_2::VillagerBrainProfile::vanilla_26_1_2(),
        );

        assert_eq!(candidate.rotation.yaw, 0.0);
        assert_eq!(candidate.rotation.head_yaw, 0.0);
        assert_eq!(
            candidate.rotation.pitch, -25.827_711,
            "the authored pitch is the entity's own, not rotated",
        );
        let population = candidate
            .retained
            .villager_population
            .clone()
            .expect("a villager marker spawns with population state");
        assert_eq!(population.age_ticks, -21_359);
        assert_eq!(
            population.claimed_home.as_deref(),
            Some("minecraft:village/plains/villagers@40:64:8#0"),
            "a baby must name the home it was placed into",
        );
        assert_eq!(
            candidate
                .retained
                .villager_brain
                .as_ref()
                .expect("a villager marker spawns with a brain")
                .schedule,
            mc_entity::villager_26_1_2::VillagerScheduleKind::Baby,
        );

        // The spawned villager must survive the entity persistence contract: the
        // baby's home claim and its age are what `save_persisted_entity_records`
        // validates per record, so writing and reading the real file is the
        // check that the spawn is storable rather than only constructible.
        let registry = SessionRegistry::new();
        let authority = SimulationAuthority::for_test();
        let items = mc_data::items::solaris_required_items();
        registry.ensure_settlement_inhabitants(&authority, std::slice::from_ref(&spawn));
        let records = registry.persisted_entity_records();
        assert_eq!(records.len(), 1, "the baby villager is committed");
        let dir = tempfile::tempdir().expect("a temp world root");
        let checkpoint = PersistedEntityCheckpoint::new(0, records);
        save_persisted_entity_records(dir.path(), &items, &checkpoint)
            .expect("a generated village baby is storable");

        let reloaded = load_persisted_entities(
            dir.path(),
            &items,
            &mc_data::entity_types::solaris_required_entity_types(),
        )
        .expect("the entity file reads back");
        assert_eq!(reloaded.records.len(), 1, "the baby villager is reloaded");
        let restored = reloaded.records[0]
            .snapshot
            .retained
            .villager_population
            .as_ref()
            .expect("the reloaded villager keeps its population state");
        assert_eq!(restored.age_ticks, -21_359);
        assert_eq!(
            restored.claimed_home.as_deref(),
            population.claimed_home.as_deref()
        );
    }
}
