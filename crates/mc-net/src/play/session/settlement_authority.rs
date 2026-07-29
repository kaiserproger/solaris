use mc_data::items::ItemRegistry;
use mc_entity::{EntitySnapshot, SpawnEntity};

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
    entity.uuid = Some(settlement_uuid(&spawn.claim));
    entity.retained.spawn_tick = lifecycle_tick;
    entity.retained.villager = Some(spawn.villager);
    entity.retained.villager_population =
        Some(mc_entity::villager_population_26_1_2::VillagerPopulationState::adult());
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

fn settlement_uuid(claim: &str) -> uuid::Uuid {
    fn hash(seed: u64, bytes: &[u8]) -> u64 {
        bytes.iter().fold(seed, |value, byte| {
            (value ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3)
        })
    }

    let high = hash(0xCBF2_9CE4_8422_2325, claim.as_bytes());
    let low = hash(0x8422_2325_CBF2_9CE4, claim.as_bytes());
    uuid::Uuid::from_u128((u128::from(high) << 64) | u128::from(low))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::persistence::PersistedEntityCheckpoint;
    use crate::play::simulation::SimulationAuthority;
    use mc_entity::{Vec3, VillagerData, VillagerKind, VillagerProfession};

    fn spawn() -> SettlementInhabitantSpawn {
        SettlementInhabitantSpawn {
            claim: "owner:smith@72,8".to_owned(),
            entity_type_id: 99,
            entity_type_name: "minecraft:villager".to_owned(),
            position: Vec3::new(72.5, 66.0, 8.5),
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

    #[test]
    fn settlement_claim_uuid_is_stable_and_claim_scoped() {
        assert_eq!(
            settlement_uuid("owner:villager@72,8"),
            settlement_uuid("owner:villager@72,8")
        );
        assert_ne!(
            settlement_uuid("owner:villager@72,8"),
            settlement_uuid("owner:villager@616,8")
        );
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
                speed: 0.3,
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
}
