//! Native admission for damage whose target is one connected player.
//!
//! This module deliberately stops at admission.  Each owner captures its own
//! revision fence before asking and spends the returned approval immediately
//! before its existing transactional kernel.  Keeping that fence at the owner
//! prevents a hook answer from becoming authority to mutate a newer player
//! state.

use mc_entity::{EntityDamageRequest, EntityId, EntitySnapshot, Vec3};
use mc_script::ScriptPosition;
use mc_script::precommit::{
    Approval, DamageContext, DamageTarget, HookActor, HookContext, HookDecision, HookFailure,
    HookKind, HookPlayer, PendingDecision,
};

use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::{SessionId, SessionRegistry};

/// The producer of a player-damage request.
///
/// A producer snapshots this identity before it releases its own owner lock.
/// `Environment` is intentionally distinct from an absent source: plugins can
/// tell natural damage from damage whose source is an entity or player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) enum PlayerDamageSource {
    Player(SessionId),
    Entity(EntityId),
    Environment,
}

/// The native result of asking the before-damage chain.
///
/// `Direct` is the allocation-free no-handler branch.  `Approved` holds the
/// one approval which the target owner's fenced commit must consume.  Refusal
/// is terminal: callers must not construct reductions, armor durability,
/// attack costs, death work, or publications from it.
#[derive(Debug)]
pub(in crate::play) enum PlayerDamageAdmission {
    Direct,
    Approved { amount: f32, approval: Approval },
    Refused(HookFailure),
}
/// The narrow session-owned boundary for admitting connection-side player
/// damage before native reductions.
///
/// Connection adapters only ask this boundary for an admission.  The session
/// owner retains registry lookup, hook-context construction, bounded ticket
/// resolution, and the approval returned for its fenced commit.
pub(in crate::play) trait PlayerDamagePrecommitAdmission {
    /// Resolve one raw player-damage request to its native precommit outcome.
    async fn admit_player_damage(
        &self,
        target_session: SessionId,
        dimension: &str,
        request: PlayerDamageRequest,
        position: Vec3,
        source: PlayerDamageSource,
    ) -> PlayerDamageAdmission;
}

/// A frozen player-owner continuation re-entering the simulation queue after
/// the guest chain answered.  It owns every value that the originating owner
/// must compare again; it never carries a lock guard or a mutable world handle.
#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "each variant carries its complete frozen player fence and continuation so the resumed precommit path consumes the original ticket against the original ordering boundary"
)]
pub(in crate::play) enum PlayerDamagePrecommitResume {
    Pvp {
        attacker_session: SessionId,
        target_session: SessionId,
        target_entity: EntityId,
        target_fence: PlayerDamageTargetFence,
        attacker_costs: Option<crate::play::simulation::PlayerSurvivalPlan>,
        authority_tick: u64,
        request: PlayerDamageRequest,
        decision: Result<Approval, HookFailure>,
    },
    Projectile {
        target_session: SessionId,
        target_fence: PlayerDamageTargetFence,
        tick: u64,
        request: PlayerDamageRequest,
        continuation: super::projectiles::ProjectileDamageContinuation,
        decision: Result<Approval, HookFailure>,
    },
    Effect {
        target_session: SessionId,
        target_fence: PlayerDamageTargetFence,
        tick: u64,
        request: PlayerDamageRequest,
        decision: Result<Approval, HookFailure>,
    },
}

/// The authoritative player image a deferred PvP decision was made about.
///
/// `PlayerSurvivalPlan` is deliberately not used as the fence: a replacement
/// has to rebuild that plan from its raw amount, while this image remains the
/// exact before-image it must compare before rebuilding.
#[derive(Debug, Clone)]
pub(in crate::play) struct PlayerDamageTargetFence {
    pub(super) survival: crate::play::survival::SurvivalState,
    pub(super) inventory: crate::play::inventory::PlayerInventory,
    pub(super) carried_item: mc_data::item_stack::ItemStack,
    pub(super) xp: crate::play::persistence::XpState,
    pub(super) active_shield: Option<crate::play::combat::ActiveShield>,
    pub(super) pose: crate::play::PlayerPose,
}

/// Frozen entity-owner damage resumed after a guest decision.
///
/// `expected` is the complete entity CAS fence. Owner-specific producers keep
/// their additional transaction state in `completion` and must reject before
/// consuming if their own captured fence no longer matches.
#[derive(Debug, Clone)]
pub(in crate::play) struct EntityDamagePrecommitResume {
    pub(in crate::play) expected: EntitySnapshot,
    pub(in crate::play) request: EntityDamageRequest,
    /// Frozen player-side cost plan for a player-on-entity attack. It is
    /// rechecked and spent only beside the entity CAS when the approval returns.
    pub(in crate::play) attacker_costs:
        Option<(SessionId, crate::play::simulation::PlayerSurvivalPlan)>,
    pub(in crate::play) completion: EntityDamagePrecommitCompletion,
    pub(in crate::play) decision: Result<Approval, HookFailure>,
}

/// The native kernel to resume after spending a before-damage approval.
///
/// Each variant retains the original producer's semantics while sharing the
/// entity snapshot fence and raw replacement amount.
#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "completion variants retain their producer-specific frozen continuation; boxing would add indirection at every resume boundary without reducing the outer queued command allocation"
)]
pub(in crate::play) enum EntityDamagePrecommitCompletion {
    Direct {
        entity_id: EntityId,
        game_mode: crate::play::GameMode,
        player_pose: crate::play::PlayerPose,
        attacker_session: Option<SessionId>,
        dragon_part: Option<mc_entity::dragon_26_1_2::DragonPart>,
    },
    Script,
    Resident {
        prior_health: f32,
    },
    Effect {
        request: mc_entity::EntityEffectRequest,
    },
    Projectile(super::projectiles::ProjectileEntityDamageContinuation),
    Explosion(super::explosion_authority::ExplosionDamageContinuation),
    Golem(super::village_defense::GolemDamageContinuation),
}

/// Producer-specific result after a deferred entity-damage kernel reaches a
/// terminal native outcome.
#[derive(Debug)]
pub(in crate::play) enum EntityDamagePrecommitResult {
    Direct,
    Script(Option<(f32, bool)>),
    Resident(Option<super::resident_orders::ResidentHit>),
    Effect(mc_entity::EntityEffectResult),
    Projectile,
}

const DAMAGE_DIMENSION: &str = "minecraft:overworld";

pub(super) fn damage_kind_name(kind: PlayerDamageKind) -> &'static str {
    match kind {
        PlayerDamageKind::MobAttack => "mob-attack",
        PlayerDamageKind::PlayerAttack => "player-attack",
        PlayerDamageKind::Projectile => "projectile",
        PlayerDamageKind::Fireball => "fireball",
        PlayerDamageKind::LargeFireball => "large-fireball",
        PlayerDamageKind::ShulkerBullet => "shulker-bullet",
        PlayerDamageKind::WindCharge => "wind-charge",
        PlayerDamageKind::SonicBoom => "sonic-boom",
        PlayerDamageKind::Magic => "magic",
        PlayerDamageKind::Wither => "wither",
        PlayerDamageKind::IndirectMagic => "indirect-magic",
        PlayerDamageKind::Fall => "fall",
        PlayerDamageKind::Campfire => "campfire",
        PlayerDamageKind::Fire => "fire",
        PlayerDamageKind::Lava => "lava",
        PlayerDamageKind::Drowning => "drowning",
        PlayerDamageKind::Suffocation => "suffocation",
        PlayerDamageKind::Starvation => "starvation",
        PlayerDamageKind::Generic => "generic",
        PlayerDamageKind::GenericKill => "generic-kill",
        PlayerDamageKind::Explosion => "explosion",
        #[cfg(test)]
        PlayerDamageKind::Unsupported => "unsupported",
    }
}

fn player_hook_actor(
    sessions: &SessionRegistry,
    session: SessionId,
) -> Result<HookActor, HookFailure> {
    let uuid = sessions
        .player_uuid(session)
        .ok_or(HookFailure::Unavailable)?;
    HookPlayer::try_new(uuid, session)
        .map(HookActor::Player)
        .map_err(HookFailure::from)
}

fn damage_source_actor(
    sessions: &SessionRegistry,
    source: &PlayerDamageSource,
) -> Result<HookActor, HookFailure> {
    match source {
        PlayerDamageSource::Player(session) => player_hook_actor(sessions, *session),
        PlayerDamageSource::Entity(entity) => u64::try_from(entity.0)
            .map(HookActor::Entity)
            .map_err(|_| HookFailure::Invalid),
        PlayerDamageSource::Environment => Ok(HookActor::Environment),
    }
}

/// Admit one immutable raw player-damage request before native reductions.
///
/// The no-handler branch returns before loading UUIDs, constructing a context,
/// allocating its strings, queueing work, or awaiting anything.  Once a chain
/// is registered, identity and position are copied into the immutable context
/// and the boundary owns its absolute queue-plus-chain deadline.
///
/// A replacement is raw native damage: the caller must feed `amount` through
/// its normal armor, shield and resistance pipeline.  Replacement with zero is
/// a no-damage outcome, represented as a terminal refusal so a caller cannot
/// accidentally charge armor or attack costs while skipping health mutation.
///
/// Start, but do not wait for, one before-damage decision.
///
/// Simulation-owner producers use this form to release their locks and return
/// to the queue while the guest runs.  The continuation carries the frozen
/// native fence and receives the `PendingDecision` result on that same queue.
pub(super) fn begin_player_damage(
    sessions: &SessionRegistry,
    target_session: SessionId,
    dimension: &str,
    request: PlayerDamageRequest,
    position: Vec3,
    source: PlayerDamageSource,
) -> Result<Option<PendingDecision>, HookFailure> {
    let Some(boundary) = sessions.precommit_boundary() else {
        return Ok(None);
    };
    if !boundary.has_precommit_hooks(HookKind::Damage) {
        return Ok(None);
    }
    let source = damage_source_actor(sessions, &source)?;
    let target = player_hook_actor(sessions, target_session)?;
    let target = match target {
        HookActor::Player(target) => target,
        _ => return Err(HookFailure::Unavailable),
    };
    let position =
        ScriptPosition::try_new(position.x, position.y, position.z).ok_or(HookFailure::Invalid)?;
    let context = DamageContext::try_new(
        source,
        DamageTarget::Player(target),
        damage_kind_name(request.kind),
        if dimension.is_empty() {
            DAMAGE_DIMENSION
        } else {
            dimension
        },
        position,
        request.amount,
    )
    .map(HookContext::Damage)
    .map_err(HookFailure::from)?;
    boundary.begin_precommit(context).map(Some)
}

/// Start a before-damage question for a server-owned entity target.
///
/// Entity producers call this only after their own legality checks and after
/// freezing the exact snapshot they will CAS on resume.
pub(in crate::play) fn begin_entity_damage(
    sessions: &SessionRegistry,
    target: EntityId,
    source: HookActor,
    kind: &str,
    position: Vec3,
    amount: f32,
) -> Result<Option<PendingDecision>, HookFailure> {
    let Some(boundary) = sessions.precommit_boundary() else {
        return Ok(None);
    };
    if !boundary.has_precommit_hooks(HookKind::Damage) {
        return Ok(None);
    }
    let target = u64::try_from(target.0).map_err(|_| HookFailure::Invalid)?;
    let position =
        ScriptPosition::try_new(position.x, position.y, position.z).ok_or(HookFailure::Invalid)?;
    let context = DamageContext::try_new(
        source,
        DamageTarget::Entity(target),
        kind,
        DAMAGE_DIMENSION,
        position,
        amount,
    )
    .map(HookContext::Damage)
    .map_err(HookFailure::from)?;
    boundary.begin_precommit(context).map(Some)
}

/// Resolve a connection-side damage question to its native admission result.
pub(in crate::play) async fn admit_player_damage(
    sessions: &SessionRegistry,
    target_session: SessionId,
    dimension: &str,
    request: PlayerDamageRequest,
    position: Vec3,
    source: PlayerDamageSource,
) -> PlayerDamageAdmission {
    let pending = match begin_player_damage(
        sessions,
        target_session,
        dimension,
        request,
        position,
        source,
    ) {
        Ok(Some(pending)) => pending,
        Ok(None) => return PlayerDamageAdmission::Direct,
        Err(failure) => return PlayerDamageAdmission::Refused(failure),
    };
    let approval = match pending.resolve().await {
        Ok(approval) => approval,
        Err(failure) => return PlayerDamageAdmission::Refused(failure),
    };
    match approval.decision() {
        HookDecision::Keep => PlayerDamageAdmission::Approved {
            amount: request.amount,
            approval,
        },
        HookDecision::Replace(amount) if amount > 0.0 && amount.is_finite() => {
            PlayerDamageAdmission::Approved { amount, approval }
        }
        HookDecision::Replace(0.0) | HookDecision::Cancel => {
            PlayerDamageAdmission::Refused(HookFailure::Cancelled)
        }
        HookDecision::Replace(_) | _ => PlayerDamageAdmission::Refused(HookFailure::Invalid),
    }
}

impl PlayerDamagePrecommitAdmission for SessionRegistry {
    async fn admit_player_damage(
        &self,
        target_session: SessionId,
        dimension: &str,
        request: PlayerDamageRequest,
        position: Vec3,
        source: PlayerDamageSource,
    ) -> PlayerDamageAdmission {
        admit_player_damage(self, target_session, dimension, request, position, source).await
    }
}
